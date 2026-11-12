//! The network test a failed start offers: whether this network lets a stream's ports out. Each
//! port a stream uses is tried against Moonlight's test server, which answers on all of them, as
//! moonlight-common-c's `LiTestClientConnectivity` (`src/ConnectionTester.c`) does; Nightjar on
//! Android runs that through MoonBridge. It never touches the PC, so it works on any PC, and it
//! runs only when asked, on its own thread, never during a stream.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream, ToSocketAddrs, UdpSocket};
use std::sync::mpsc::{channel, Receiver};
use std::time::Duration;

/// Moonlight's test server for desktop clients, and the port tried first to find an address of it
/// that this network reaches at all.
pub const SERVER: &str = "qt.conntest.moonlight-stream.org";
const REFERENCE_PORT: u16 = 443;
/// How long each port has to answer.
const TIMEOUT: Duration = Duration::from_secs(3);
/// The ports of `ML_PORT_FLAG_ALL`: HTTPS, HTTP and RTSP over TCP; video, control, audio and RTSP
/// over UDP.
pub const TCP_PORTS: [u16; 3] = [47984, 47989, 48010];
pub const UDP_PORTS: [u16; 4] = [47998, 47999, 48000, 48010];
/// A UDP probe is as large as a stream's packets, so a path that drops those fails here too.
const PROBE_SIZE: usize = 1040;
const PROBE_TEXT: &[u8] = b"moonlight-ctest";
const PROBE_SENDS: usize = 3;
const PROBE_GAP: Duration = Duration::from_millis(50);

/// What the test found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Every port answered.
    Clear,
    /// At least one did not: streaming over the internet may be blocked.
    Blocked,
    /// The test server could not be reached, so nothing is known.
    Unknown,
}

/// Runs the test on its own thread; `waker` runs once the answer is in.
pub fn start(waker: crate::hosts::Waker) -> Receiver<Outcome> {
    let (tx, rx) = channel();
    let spawned = std::thread::Builder::new().name("Network test".into()).spawn(move || {
        let _ = tx.send(run(SERVER));
        waker();
    });
    if let Err(e) = spawned {
        eprintln!("nightjar: the network test could not start: {e}");
    }
    rx
}

/// The test against `server`, blocking until every port answered or timed out.
pub fn run(server: &str) -> Outcome {
    let Some(address) = reachable(server) else {
        return Outcome::Unknown;
    };
    let tcp = TCP_PORTS.map(|port| std::thread::spawn(move || TcpStream::connect_timeout(&SocketAddr::new(address, port), TIMEOUT).is_ok()));
    let udp = UDP_PORTS.map(|port| std::thread::spawn(move || udp_answers(SocketAddr::new(address, port))));
    let open: Vec<bool> = tcp.into_iter().chain(udp).map(|probe| probe.join().unwrap_or(false)).collect();
    outcome(&open)
}

/// All ports open is clear; any closed one, blocked.
fn outcome(open: &[bool]) -> Outcome {
    if open.iter().all(|&o| o) { Outcome::Clear } else { Outcome::Blocked }
}

/// The first of the server's addresses that takes a connection on the reference port.
fn reachable(server: &str) -> Option<IpAddr> {
    let addresses = (server, REFERENCE_PORT).to_socket_addrs().ok()?;
    addresses.into_iter().find(|address| TcpStream::connect_timeout(address, TIMEOUT).is_ok()).map(|address| address.ip())
}

/// Sends the probe a few times and waits for any answer.
fn udp_answers(to: SocketAddr) -> bool {
    let local: IpAddr = if to.is_ipv4() { Ipv4Addr::UNSPECIFIED.into() } else { Ipv6Addr::UNSPECIFIED.into() };
    let Ok(socket) = UdpSocket::bind(SocketAddr::new(local, 0)) else {
        return false;
    };
    let probe = probe();
    for n in 0..PROBE_SENDS {
        if socket.send_to(&probe, to).is_err() {
            return false;
        }
        if n + 1 < PROBE_SENDS {
            std::thread::sleep(PROBE_GAP);
        }
    }
    if socket.set_read_timeout(Some(TIMEOUT)).is_err() {
        return false;
    }
    let mut answer = [0u8; PROBE_SIZE];
    socket.recv_from(&mut answer).is_ok()
}

/// The probe: its text, then zeros up to its size.
fn probe() -> [u8; PROBE_SIZE] {
    let mut probe = [0u8; PROBE_SIZE];
    probe[..PROBE_TEXT.len()].copy_from_slice(PROBE_TEXT);
    probe
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_probe_is_a_stream_packets_size_and_starts_with_its_text() {
        let probe = probe();
        assert_eq!(probe.len(), 1040);
        assert!(probe.starts_with(b"moonlight-ctest"));
        assert!(probe[PROBE_TEXT.len()..].iter().all(|&b| b == 0));
    }

    #[test]
    fn every_port_of_a_stream_is_tried() {
        // Moonlight's ML_PORT_FLAG_ALL.
        assert_eq!(TCP_PORTS, [47984, 47989, 48010]);
        assert_eq!(UDP_PORTS, [47998, 47999, 48000, 48010]);
    }

    #[test]
    fn one_closed_port_is_enough_to_say_blocked() {
        assert_eq!(outcome(&[true; 7]), Outcome::Clear);
        assert_eq!(outcome(&[true, true, true, true, false, true, true]), Outcome::Blocked);
    }

    #[test]
    fn a_server_that_cannot_be_found_says_nothing_is_known() {
        assert_eq!(run("nightjar-conntest.invalid"), Outcome::Unknown);
    }
}
