use super::{pace, transport, xml, Error, Feature, HostClient, ServerInfo, MAX_XML_RESPONSE_BYTES};
use socket2::{Domain, Protocol, Socket, Type};
use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

impl HostClient {
    pub fn probe_bandwidth(&self, info: &ServerInfo) -> Result<f64, Error> {
        let bytes = super::capabilities::PYROWAVE_BANDWIDTH_PROBE_BYTES as usize;
        if !info.paired || !info.capabilities.has(Feature::PyroWaveBandwidthProbe) || info.pyrowave_bandwidth_probe_bytes as usize != bytes {
            return Err(Error::InvalidArgument("This paired PC does not advertise the bandwidth test.".into()));
        }
        let target = self.url_target("pyrowave-bandwidth-probe", None);
        let started = Instant::now();
        let response = transport::probe(&transport::Request {
            host: &self.host, port: self.https_port(true)?, target: &target, tls: Some(self.tls()?),
            timeouts: transport::Timeouts { connect: Duration::from_secs(5), read: Some(Duration::from_secs(7)) }, body_limit: bytes,
        }, &self.cancel, Duration::from_secs(30), &mut |status, headers, _| {
            status_ok(status)?;
            let get = |name: &str| headers.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str());
            if !get("Content-Type").is_some_and(|s| s.split(';').next().unwrap_or_default().trim().eq_ignore_ascii_case("application/octet-stream"))
                || get("Content-Length").is_some_and(|s| s.parse::<usize>().ok() != Some(bytes)) {
                return Err(Error::Malformed("Unexpected bandwidth test type or size.".into()));
            }
            Ok(())
        })?;
        if response.body.len() != bytes { return Err(Error::Malformed("Bandwidth test ended before the advertised size.".into())); }
        Ok(bytes as f64 * 8.0 / started.elapsed().as_secs_f64().max(0.000001) / 1_000_000.0)
    }

    /// Mbit/s of one `GET /bandwidth-probe` of [`LINK_PROBE_BYTES`] (`Feature::BandwidthProbe`, every
    /// codec), timed from the response headers to the end of the body. The host stops writing after
    /// 3 s, so a body that ends early on a slow link is still a measurement; an empty one is not.
    pub fn probe_link(&self, info: &ServerInfo) -> Result<f64, Error> {
        if !info.paired || !info.capabilities.has(Feature::BandwidthProbe) {
            return Err(Error::InvalidArgument("This paired PC does not offer the bandwidth test.".into()));
        }
        let bytes = LINK_PROBE_BYTES as usize;
        let target = self.url_target("bandwidth-probe", Some(&format!("bytes={bytes}")));
        let mut headers_at = None;
        let response = transport::probe(&transport::Request {
            host: &self.host, port: self.https_port(true)?, target: &target, tls: Some(self.tls()?),
            timeouts: transport::Timeouts { connect: Duration::from_secs(5), read: Some(Duration::from_secs(5)) }, body_limit: bytes,
        }, &self.cancel, Duration::from_secs(10), &mut |status, headers, _| {
            if !(200..=299).contains(&status) {
                return Err(Error::Status { code: status as i32, message: "The PC refused the bandwidth test.".into() });
            }
            let content_type = headers.iter().find(|(n, _)| n.eq_ignore_ascii_case("Content-Type")).map(|(_, v)| v.as_str());
            if !content_type.is_some_and(|s| s.split(';').next().unwrap_or_default().trim().eq_ignore_ascii_case("application/octet-stream")) {
                return Err(Error::Malformed("Unexpected bandwidth test type.".into()));
            }
            headers_at = Some(Instant::now());
            Ok(())
        })?;
        let elapsed = headers_at.map_or(Duration::ZERO, |at| at.elapsed());
        if response.body.is_empty() {
            return Err(Error::Malformed("The bandwidth test sent nothing.".into()));
        }
        Ok(response.body.len() as f64 * 8.0 / elapsed.as_secs_f64().max(0.000001) / 1_000_000.0)
    }

    pub fn probe_pace(&self, kbps: u32, packet_size: u32, fps: u32, pace_kbps: u32) -> Result<pace::Measurement, Error> {
        if !(5000..=3_000_000).contains(&kbps) || !(256..=1392).contains(&packet_size) || !(10..=500).contains(&fps)
            || pace_kbps < kbps || pace_kbps > 9_999_999 {
            return Err(Error::InvalidArgument("Invalid PyroWave probe settings.".into()));
        }
        let tls = self.tls()?;
        let (socket, dual) = match Socket::new(Domain::IPV6, Type::DGRAM, Some(Protocol::UDP)) {
            Ok(socket) if socket.set_only_v6(false).is_ok() => (socket, true),
            _ => (Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?, false),
        };
        socket.set_recv_buffer_size(8 * 1024 * 1024)?;
        socket.bind(&if dual { "[::]:0" } else { "0.0.0.0:0" }.parse::<SocketAddr>().unwrap().into())?;
        let socket: UdpSocket = socket.into();
        socket.set_read_timeout(Some(Duration::from_millis(100)))?;
        let token = transport::random_uuid().replace('-', "");
        let query = format!("kbps={kbps}&port={}&packetsize={packet_size}&token={token}&handshake=1&fps={fps}&pacekbps={pace_kbps}", socket.local_addr()?.port());
        let target = self.url_target("pyrowave-udp-probe", Some(&query));
        let expected = pace::expected_packets(kbps, packet_size);
        let stop = AtomicBool::new(false);
        std::thread::scope(|scope| {
            let mut receiver = None;
            let mut ipv6 = false;
            let response = transport::probe(&transport::Request {
                host: &self.host, port: self.https_port(true)?, target: &target, tls: Some(tls),
                timeouts: transport::Timeouts { connect: Duration::from_secs(5), read: Some(Duration::from_secs(6)) }, body_limit: MAX_XML_RESPONSE_BYTES,
            }, &self.cancel, Duration::from_secs(10), &mut |status, headers, peer| {
                status_ok(status)?;
                let port = headers.iter().find(|(name, _)| name.eq_ignore_ascii_case("X-PyroWave-Udp-Port"))
                    .and_then(|(_, value)| udp_port(value)).ok_or_else(|| Error::Malformed("The PC did not announce a valid UDP probe port.".into()))?;
                ipv6 = peer.is_ipv6();
                let mut peer = peer;
                peer.set_port(port);
                if dual {
                    if let SocketAddr::V4(v4) = peer { peer = SocketAddr::new(v4.ip().to_ipv6_mapped().into(), port); }
                }
                socket.connect(peer)?;
                receiver = Some(scope.spawn(|| receive(&socket, token.as_bytes(), packet_size + 48, expected, &stop, &self.cancel)));
                Ok(())
            });
            if response.is_ok() && !self.cancel.is_cancelled() { std::thread::sleep(Duration::from_millis(100)); }
            stop.store(true, Ordering::Release);
            let arrived = receiver.map(|worker| worker.join().map_err(|_| Error::Malformed("UDP probe receiver stopped.".into())));
            let response = response?;
            let arrived = arrived.ok_or_else(|| Error::Malformed("The PC did not start the UDP handshake.".into()))???;
            let raw = std::str::from_utf8(&response.body).map_err(|_| Error::Malformed("UDP probe result is not UTF-8.".into()))?;
            let mut result = parse_result(raw, expected)?;
            pace::summarize(&mut result, &arrived, fps, pace::burst_group(pace_kbps, packet_size, ipv6));
            Ok(result)
        })
    }
}

/// What [`HostClient::probe_link`] asks for: the host's largest probe, so a fast link is past TCP's
/// slow start for most of it.
pub const LINK_PROBE_BYTES: u32 = 4 * 1024 * 1024;
/// The share of a probed link a stream starts at: room for FEC, audio and other traffic.
pub const PROBE_START_SHARE: f64 = 0.8;
/// No probe starts a stream below this.
pub const PROBE_FLOOR_KBPS: u32 = 2000;

/// The first bitrate of a stream whose bitrate is automatic: `configured_kbps`, lowered to
/// [`PROBE_START_SHARE`] of a link probed at `probe_mbps`, never below [`PROBE_FLOOR_KBPS`]. A
/// probe never raises it.
pub fn probe_start_kbps(configured_kbps: u32, probe_mbps: f64) -> u32 {
    let share = (probe_mbps.max(0.0) * 1000.0 * PROBE_START_SHARE).min(f64::from(u32::MAX)) as u32;
    configured_kbps.min(share).max(PROBE_FLOOR_KBPS.min(configured_kbps))
}

fn status_ok(status: u16) -> Result<(), Error> {
    if (200..=299).contains(&status) { Ok(()) }
    else { Err(Error::Status { code: status as i32, message: "The PC refused the network test; stop any active stream and try again.".into() }) }
}

fn udp_port(text: &str) -> Option<u16> {
    (text.len() >= 4 && text.len() <= 5 && text.bytes().all(|b| b.is_ascii_digit()))
        .then(|| text.parse::<u16>().ok()).flatten().filter(|&p| p >= 1024)
}

fn receive(socket: &UdpSocket, token: &[u8], packet_size: u32, expected: u32, stop: &AtomicBool, cancel: &transport::Cancel) -> Result<Vec<bool>, Error> {
    let mut arrived = vec![false; expected as usize];
    let mut buffer = [0; 2048];
    let mut authenticated = false;
    let mut last_hello = None;
    let mut attempts = 0;
    while !stop.load(Ordering::Acquire) && !cancel.is_cancelled() {
        if !authenticated && attempts < 15 && last_hello.map_or(true, |t: Instant| t.elapsed() >= Duration::from_millis(100)) {
            socket.send(token)?;
            last_hello = Some(Instant::now());
            attempts += 1;
        }
        match socket.recv(&mut buffer) {
            Ok(n) if n == packet_size as usize && buffer[..32] == *token => {
                authenticated = true;
                let seq = u32::from_be_bytes(buffer[32..36].try_into().unwrap()) as usize;
                if let Some(present) = arrived.get_mut(seq) { *present = true; }
            }
            Ok(_) => {}
            Err(e) if matches!(e.kind(), std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted) => {}
            Err(e) => return Err(e.into()),
        }
    }
    if cancel.is_cancelled() { Err(Error::Cancelled) } else { Ok(arrived) }
}

fn parse_result(raw: &str, expected: u32) -> Result<pace::Measurement, Error> {
    let root = xml::parse_reply(raw)?;
    let invalid = || Error::Malformed("Invalid PyroWave UDP probe result.".into());
    let reported = root.required_text("expected")?.parse::<u32>().map_err(|_| invalid())?;
    let sent = root.required_text("sent")?.parse::<u32>().map_err(|_| invalid())?;
    let sender_ms = root.required_text("elapsedMs")?.trim().parse::<f64>().map_err(|_| invalid())?;
    if reported != expected || sent > expected || !sender_ms.is_finite() { return Err(invalid()); }
    Ok(pace::Measurement { expected, sent, sender_ms, ..Default::default() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_untrusted_ports_and_invalid_measurements() {
        for port in ["0", "80", "1023", "65536", "-1234", "+1234", "1234 ", "999999999999"] { assert_eq!(udp_port(port), None); }
        assert_eq!(udp_port("47998"), Some(47998));
        let reply = |count, sent, elapsed| format!("<root status_code=\"200\"><expected>{count}</expected><sent>{sent}</sent><elapsedMs>{elapsed}</elapsedMs></root>");
        assert!(parse_result(&reply(100, 99, "2000"), 100).is_ok());
        assert!(parse_result(&reply(101, 100, "2000"), 100).is_err());
        assert!(parse_result(&reply(100, 101, "2000"), 100).is_err());
        assert!(parse_result(&reply(100, 100, "NaN"), 100).is_err());
    }

    #[test]
    fn a_probe_only_lowers_the_first_bitrate() {
        assert_eq!(probe_start_kbps(20_000, 500.0), 20_000);
        assert_eq!(probe_start_kbps(75_000, 50.0), 40_000);
        assert_eq!(probe_start_kbps(20_000, 1.0), PROBE_FLOOR_KBPS);
        assert_eq!(probe_start_kbps(1_500, 1.0), 1_500, "never above the setting, also under the floor");
        assert_eq!(probe_start_kbps(20_000, f64::NAN), PROBE_FLOOR_KBPS);
        assert_eq!(probe_start_kbps(u32::MAX, f64::INFINITY), u32::MAX);
    }
}
