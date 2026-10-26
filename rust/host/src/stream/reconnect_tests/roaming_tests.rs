//! Client roaming through the real control socket, ENet and media workers: a fake client
//! proves a new address on 127.0.0.2 with the roaming exchange and moves its stream there.
use super::*;
use butterpollo_core::roaming::{Datagram, Header, Payload, RoamKeys, SDP_ATTRIBUTE, open, seal};

/// The address the client moves to.
const MOVED: [u8; 4] = [127, 0, 0, 2];

fn moved_udp() -> Result<UdpSocket> {
    let socket = UdpSocket::bind(SocketAddr::from((MOVED, 0)))?;
    socket.set_nonblocking(true)?;
    Ok(socket)
}

/// The client's half of the roaming exchange.
struct Roamer {
    host: SocketAddr,
    keys: RoamKeys,
    seq: u32,
}
impl Roamer {
    fn new(ports: Ports, launch: &Launch) -> Self {
        Self {
            host: address(ports.control),
            keys: RoamKeys::derive(&launch.key),
            seq: 0,
        }
    }
    /// Sends `payload` from `socket` every 100 ms until the host answers, and returns the
    /// answer; `None` when none came within `timeout`.
    async fn exchange(
        &mut self,
        socket: &UdpSocket,
        payload: Payload,
        timeout: Duration,
    ) -> Result<Option<Payload>> {
        let deadline = Instant::now() + timeout;
        let mut buffer = [0; 2048];
        while Instant::now() < deadline {
            self.seq += 1;
            let header = Header {
                kind: payload.kind(),
                session_tag: self.keys.session_tag,
                seq: self.seq,
            };
            socket.send_to(&seal(&self.keys.key, header, payload), self.host)?;
            let resend = Instant::now() + Duration::from_millis(100);
            while Instant::now() < resend {
                match socket.recv_from(&mut buffer) {
                    Ok((n, from)) => {
                        assert_eq!(from, self.host);
                        // ENet's datagrams to a moved client are not the exchange's.
                        if let Ok(datagram) = Datagram::decode(&buffer[..n]) {
                            let answer = open(&self.keys.key, &datagram);
                            assert!(answer.is_some(), "the host seals with the session's key");
                            return Ok(answer);
                        }
                    }
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            || butterpollo_windows::net::datagram_lost(&error) =>
                    {
                        tokio::time::sleep(Duration::from_millis(5)).await;
                    }
                    Err(error) => return Err(error.into()),
                }
            }
        }
        Ok(None)
    }
}

fn video_peer(h: &Harness, launch: &Launch) -> Option<SocketAddr> {
    let media = h.media.as_ref().unwrap();
    let peers = media.peers.lock().unwrap();
    peers.get(&(launch.id.clone(), false)).copied()
}

/// PROBE, CHALLENGE, CONFIRM and MIGRATED from `control`: the session's client is at
/// its address from now on.
async fn roam(roamer: &mut Roamer, session: &Session, control: &UdpSocket) -> Result<()> {
    let roams = session.roams();
    let probe = Payload::Probe {
        nonce_c: 7,
        flags: 0,
    };
    let Some(Payload::Challenge { nonce_c, challenge }) =
        roamer.exchange(control, probe, WAIT).await?
    else {
        panic!("no CHALLENGE");
    };
    assert_eq!(nonce_c, 7);
    assert_eq!(
        roamer
            .exchange(control, Payload::Confirm { challenge }, WAIT)
            .await?,
        Some(Payload::Migrated { challenge })
    );
    assert_eq!(session.client_ip(), control.local_addr()?.ip());
    assert_eq!(session.roams(), roams + 1);
    Ok(())
}

/// PROBE, CHALLENGE, CONFIRM, MIGRATED from 127.0.0.2: ENet input, video and the host's
/// control messages follow the client there.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_client_that_proves_its_new_address_keeps_its_stream() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    let launch = h.launch(0, false).await?;
    let describe = h.rtsp(&launch, 10, "DESCRIBE", "").await?;
    assert!(describe.contains(SDP_ATTRIBUTE), "{describe}");
    let (session, client) = h.play(&launch).await?;
    let control = moved_udp()?;
    let mut roamer = Roamer::new(h.ports, &launch);
    let probe = Payload::Probe {
        nonce_c: 7,
        flags: 0,
    };
    let Some(Payload::Challenge { nonce_c, challenge }) =
        roamer.exchange(&control, probe, WAIT).await?
    else {
        panic!("no CHALLENGE");
    };
    assert_eq!(nonce_c, 7);
    assert_eq!(session.client_ip(), IpAddr::from([127, 0, 0, 1]));
    assert_eq!(
        roamer
            .exchange(&control, Payload::Confirm { challenge }, WAIT)
            .await?,
        Some(Payload::Migrated { challenge })
    );
    assert_eq!(session.client_ip(), IpAddr::from(MOVED));
    assert_eq!(session.roams(), 1);
    let video = moved_udp()?;
    let moved_video = video.local_addr()?;
    client.move_to(control, video);
    until("video pings from the new address", WAIT, || {
        video_peer(&h, &launch) == Some(moved_video)
    })
    .await;
    // The worker sends from the new sockets only once it dropped the old ones.
    let frames = client.frames.load(Ordering::Acquire);
    let inputs = session.stats.idr_requests.load(Ordering::Acquire);
    until("ENet input and video on the new address", WAIT, || {
        client.frames.load(Ordering::Acquire) > frames + 2
            // The new path's first keyframe request is one of them.
            && session.stats.idr_requests.load(Ordering::Acquire) > inputs + 3
    })
    .await;
    assert!(!session.stopping());
    // The termination message reaches the client at its new address.
    h.end(&session, client).await?;
    h.finish().await
}

/// After a roam a legacy 4-byte ping from the client's new IP does not take its video:
/// behind a CGNAT another device may share that IP. The session's own ping does.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn after_a_roam_only_the_sessions_own_ping_takes_its_video() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    let launch = h.launch(0, false).await?;
    let (session, client) = h.play(&launch).await?;
    let control = moved_udp()?;
    let mut roamer = Roamer::new(h.ports, &launch);
    roam(&mut roamer, &session, &control).await?;
    let video = moved_udp()?;
    let moved_video = video.local_addr()?;
    let host_video = address(h.ports.video);
    let deadline = Instant::now() + Duration::from_millis(500);
    while Instant::now() < deadline {
        video.send_to(b"PING", host_video)?;
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(video_peer(&h, &launch), None);
    video.send_to(launch.ping.as_bytes(), host_video)?;
    until("the session's ping takes the video", WAIT, || {
        video_peer(&h, &launch) == Some(moved_video)
    })
    .await;
    client.move_to(control, video);
    let frames = client.frames.load(Ordering::Acquire);
    until("video on the new address", WAIT, || {
        client.frames.load(Ordering::Acquire) > frames + 2
    })
    .await;
    h.end(&session, client).await?;
    h.finish().await
}

/// A CHALLENGE answered with the wrong value moves nothing: the stream stays where it was.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_the_confirm_nothing_moves() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    let launch = h.launch(0, false).await?;
    let (session, client) = h.play(&launch).await?;
    let video = video_peer(&h, &launch);
    let control = moved_udp()?;
    let mut roamer = Roamer::new(h.ports, &launch);
    let probe = Payload::Probe {
        nonce_c: 7,
        flags: 0,
    };
    let Some(Payload::Challenge { challenge, .. }) = roamer.exchange(&control, probe, WAIT).await?
    else {
        panic!("no CHALLENGE");
    };
    let wrong = Payload::Confirm {
        challenge: challenge ^ 1,
    };
    assert_eq!(
        roamer
            .exchange(&control, wrong, Duration::from_millis(500))
            .await?,
        None
    );
    assert_eq!(session.client_ip(), IpAddr::from([127, 0, 0, 1]));
    assert_eq!(session.roams(), 0);
    let frames = client.frames.load(Ordering::Acquire);
    until("video on the old address", WAIT, || {
        client.frames.load(Ordering::Acquire) > frames + 2
    })
    .await;
    assert_eq!(video_peer(&h, &launch), video);
    h.end(&session, client).await?;
    h.finish().await
}

/// `client_roaming = false`: DESCRIBE does not offer roaming and a PROBE goes unanswered.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn roaming_turned_off_is_not_offered_and_ignores_probes() -> Result<()> {
    let h = Harness::configured(10_000, 1, "client_roaming=false\n").await?;
    let launch = h.launch(0, false).await?;
    let describe = h.rtsp(&launch, 10, "DESCRIBE", "").await?;
    assert!(!describe.contains(SDP_ATTRIBUTE), "{describe}");
    let (session, client) = h.play(&launch).await?;
    let control = moved_udp()?;
    let mut roamer = Roamer::new(h.ports, &launch);
    let probe = Payload::Probe {
        nonce_c: 7,
        flags: 0,
    };
    assert_eq!(
        roamer
            .exchange(&control, probe, Duration::from_millis(500))
            .await?,
        None
    );
    assert_eq!(session.client_ip(), IpAddr::from([127, 0, 0, 1]));
    let frames = client.frames.load(Ordering::Acquire);
    until("the stream goes on", WAIT, || {
        client.frames.load(Ordering::Acquire) > frames + 2
    })
    .await;
    h.end(&session, client).await?;
    h.finish().await
}
