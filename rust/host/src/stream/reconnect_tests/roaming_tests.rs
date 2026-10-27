//! Client roaming through the real control socket, ENet and media workers: a fake client
//! proves a new address on 127.0.0.2 with the roaming exchange and moves its stream there.
use super::*;
use butterpollo_core::roaming::{Datagram, Header, Payload, RoamKeys, SDP_ATTRIBUTE, open, seal};

/// The address the client moves to.
const MOVED: [u8; 4] = [127, 0, 0, 2];
/// Where it moves on a second roam.
const MOVED_AGAIN: [u8; 4] = [127, 0, 0, 3];

fn moved_udp() -> Result<UdpSocket> {
    udp_at(SocketAddr::from((MOVED, 0)))
}

fn udp_at(address: SocketAddr) -> Result<UdpSocket> {
    let socket = UdpSocket::bind(address)?;
    socket.set_nonblocking(true)?;
    Ok(socket)
}

/// `address` once the socket a moved client left there is closed.
async fn rebind(address: SocketAddr) -> Result<UdpSocket> {
    let deadline = Instant::now() + WAIT;
    loop {
        match udp_at(address) {
            Ok(socket) => return Ok(socket),
            Err(error) if Instant::now() >= deadline => return Err(error),
            Err(_) => tokio::time::sleep(Duration::from_millis(10)).await,
        }
    }
}

/// The session's keyframe requests and host keyframes so far.
fn keyframes(session: &Session) -> u64 {
    session.stats.idr_requests.load(Ordering::Acquire)
}

/// Moves `client`'s ENet to `control` and its video pings to a new socket on the same
/// IP, and waits until video and a new keyframe reach it there: the client's heartbeat
/// is quiet, so only the host's own keyframe for the new path counts.
async fn follow(
    h: &Harness,
    launch: &Launch,
    session: &Session,
    client: &Client,
    control: UdpSocket,
) -> Result<()> {
    let before = keyframes(session);
    let video = udp_at(SocketAddr::new(control.local_addr()?.ip(), 0))?;
    let moved_video = video.local_addr()?;
    client.move_to(control, video);
    until("video pings from the new address", WAIT, || {
        video_peer(h, launch) == Some(moved_video)
    })
    .await;
    let frames = client.frames.load(Ordering::Acquire);
    until("a keyframe and video on the new address", WAIT, || {
        client.frames.load(Ordering::Acquire) > frames + 2 && keyframes(session) > before
    })
    .await;
    Ok(())
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

/// DESCRIBE, which offers the session roaming.
async fn offer(h: &Harness, launch: &Launch) -> Result<()> {
    let describe = h.rtsp(launch, 10, "DESCRIBE", "").await?;
    assert!(describe.contains(SDP_ATTRIBUTE), "{describe}");
    Ok(())
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
    client.quiet_heartbeat().await;
    let before = keyframes(&session);
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
    // The keyframe waits for the new path's first video ping.
    assert_eq!(keyframes(&session), before);
    // The worker sends from the new sockets only once it dropped the old ones.
    follow(&h, &launch, &session, &client, control).await?;
    // ENet input on the new address: the client's IDR requests are heard there.
    client.keyframe_heartbeat.store(true, Ordering::Release);
    let requests = keyframes(&session);
    until("ENet input on the new address", WAIT, || {
        keyframes(&session) > requests + 3
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
    offer(&h, &launch).await?;
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
    offer(&h, &launch).await?;
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

/// Roaming is on, but a client that never got the DESCRIBE offer does not speak it: its
/// session's key answers no PROBE.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_not_offered_roaming_ignores_probes() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    let launch = h.launch(0, false).await?;
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
    assert_eq!(session.roams(), 0);
    h.end(&session, client).await?;
    h.finish().await
}

/// A stream that ends between CHALLENGE and CONFIRM moves nothing: the CONFIRM goes
/// unanswered and the host releases the session as usual.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_that_ends_mid_roam_moves_nothing() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    let launch = h.launch(0, false).await?;
    offer(&h, &launch).await?;
    let (session, client) = h.play(&launch).await?;
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
    h.end(&session, client).await?;
    assert_eq!(
        roamer
            .exchange(
                &control,
                Payload::Confirm { challenge },
                Duration::from_millis(500)
            )
            .await?,
        None
    );
    assert_eq!(session.client_ip(), IpAddr::from([127, 0, 0, 1]));
    assert_eq!(session.roams(), 0);
    h.finish().await
}

/// A client roams twice: its stream follows it each time, with a keyframe each time.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_second_roam_moves_the_stream_again() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    let launch = h.launch(0, false).await?;
    offer(&h, &launch).await?;
    let (session, client) = h.play(&launch).await?;
    client.quiet_heartbeat().await;
    let mut roamer = Roamer::new(h.ports, &launch);
    let control = moved_udp()?;
    roam(&mut roamer, &session, &control).await?;
    follow(&h, &launch, &session, &client, control).await?;
    // A session moves at most once a second.
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let control = udp_at(SocketAddr::from((MOVED_AGAIN, 0)))?;
    roam(&mut roamer, &session, &control).await?;
    assert_eq!(session.roams(), 2);
    follow(&h, &launch, &session, &client, control).await?;
    assert_eq!(
        video_peer(&h, &launch).unwrap().ip(),
        IpAddr::from(MOVED_AGAIN)
    );
    h.end(&session, client).await?;
    h.finish().await
}

/// A client that roams back to the very address it launched from: ENet's own address
/// for the peer is its wire address again, and the stream follows it there.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_client_can_roam_back_to_its_launch_address() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    let launch = h.launch(0, false).await?;
    offer(&h, &launch).await?;
    let (session, client) = h.play(&launch).await?;
    client.quiet_heartbeat().await;
    let mut roamer = Roamer::new(h.ports, &launch);
    let control = moved_udp()?;
    roam(&mut roamer, &session, &control).await?;
    follow(&h, &launch, &session, &client, control).await?;
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let home = rebind(client.control_address).await?;
    roam(&mut roamer, &session, &home).await?;
    assert_eq!(session.client_ip(), IpAddr::from([127, 0, 0, 1]));
    follow(&h, &launch, &session, &client, home).await?;
    // The termination message reaches the client back home.
    h.end(&session, client).await?;
    h.finish().await
}

/// Another client connecting from an address a roamed client left: while the roamed
/// session lives the address is retired, so the connect is dropped and the roamed
/// stream goes on; once that session ends, the same address connects.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_connect_from_a_retired_address_waits_for_the_roamed_session() -> Result<()> {
    let h = Harness::new(20_000, 2).await?;
    let launch = h.launch(0, true).await?;
    offer(&h, &launch).await?;
    let (session, client) = h.play(&launch).await?;
    let mut roamer = Roamer::new(h.ports, &launch);
    let control = moved_udp()?;
    roam(&mut roamer, &session, &control).await?;
    let video = moved_udp()?;
    let moved_video = video.local_addr()?;
    client.move_to(control, video);
    until("video pings from the new address", WAIT, || {
        video_peer(&h, &launch) == Some(moved_video)
    })
    .await;
    let retired = rebind(client.control_address).await?;
    let other = h.launch(1, true).await?;
    h.rtsp(&other, 1, "ANNOUNCE", "a=x-nv-video[0].clientViewportWd:640\r\na=x-nv-video[0].clientViewportHt:480\r\na=x-ss-general.encryptionEnabled:0\r\n").await?;
    h.rtsp(&other, 2, "PLAY", "").await?;
    let other_session = h.h.sessions.lock().unwrap().active[&other.id].clone();
    let other_client = Client::sending_from(h.ports, other.clone(), retired)?;
    let frames = client.frames.load(Ordering::Acquire);
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(keyframes(&other_session), 0, "its ENet connect was dropped");
    assert!(client.frames.load(Ordering::Acquire) > frames + 2);
    assert!(!session.stopping());
    h.rtsp(&launch, 3, "TEARDOWN", "").await?;
    until("normal termination delivered", WAIT, || {
        client.reason.load(Ordering::Acquire) != 0
    })
    .await;
    drop(client);
    // ENet retries the connect, which now reaches the host.
    until(
        "the retired address connects",
        Duration::from_secs(15),
        || keyframes(&other_session) > 0 && other_client.frames.load(Ordering::Acquire) > 2,
    )
    .await;
    assert!(!other_session.stopping());
    h.end(&other_session, other_client).await?;
    h.finish().await
}
