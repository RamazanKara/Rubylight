//! WebRTC sessions through the real signalling server, session actor, media transport and stream
//! worker, beside the Moonlight workers of the reconnect matrix. Only the encoder's output is
//! synthetic; the client is a str0m peer on a loopback UDP socket.
use super::*;
use crate::webrtc::{
    sdp::{VideoCodec, testing as peer_sdp},
    server::{self, tests::Ws},
};
use butterpollo_core::webrtc::auth::proof_hex;
use serde_json::{Value, json};
use str0m::{
    Event, Input, Output,
    channel::ChannelId,
    media::{KeyframeRequestKind, Mid},
    net::{Protocol, Receive},
};

const VIEW: u32 = 1 << 25;
const LIST: u32 = 1 << 24;

fn nal(header: &[u8], seed: u8, len: usize) -> Vec<u8> {
    // Odd bytes only: no start code can appear inside.
    let body = (0..len).map(|i| (i as u8 % 200).wrapping_add(seed).max(1) | 1);
    [&[0, 0, 0, 1][..], header]
        .into_iter()
        .flatten()
        .copied()
        .chain(body)
        .collect()
}

/// An Annex B access unit of a few RTP packets: parameter sets and an IDR slice, or a delta slice.
pub(super) fn access_unit(hevc: bool, idr: bool) -> Vec<u8> {
    match (hevc, idr) {
        (false, true) => [
            nal(&[0x67], 3, 20),
            nal(&[0x68], 5, 6),
            nal(&[0x65], 7, 3000),
        ]
        .concat(),
        (false, false) => nal(&[0x41], 9, 1500),
        (true, true) => [
            nal(&[0x40, 0x01], 3, 20),
            nal(&[0x42, 0x01], 4, 30),
            nal(&[0x44, 0x01], 5, 6),
            nal(&[0x26, 0x01], 7, 3000),
        ]
        .concat(),
        (true, false) => nal(&[0x02, 0x01], 9, 1500),
    }
}

/// A browser-like client: it answers the offer, connects over loopback and records the video it
/// receives. It asks for keyframes on request.
struct Peer {
    stop: Arc<AtomicBool>,
    units: Arc<Mutex<Vec<Vec<u8>>>>,
    pli: Arc<AtomicUsize>,
    control: Arc<Mutex<Vec<String>>>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Peer {
    /// The peer and the answer SDP for `offer`.
    fn answer(offer: &str, codec: VideoCodec) -> (Self, String) {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let local = socket.local_addr().unwrap();
        let mut rtc = peer_sdp::client(&peer_sdp::spec(codec, false), local);
        let answer = peer_sdp::answer(&mut rtc, offer);
        let mut peer = Self {
            stop: Arc::new(AtomicBool::new(false)),
            units: Arc::default(),
            pli: Arc::default(),
            control: Arc::default(),
            thread: None,
        };
        let (stop, units, pli, control) = (
            peer.stop.clone(),
            peer.units.clone(),
            peer.pli.clone(),
            peer.control.clone(),
        );
        peer.thread = Some(thread::spawn(move || {
            let mut video: Option<Mid> = None;
            let mut control_channel: Option<ChannelId> = None;
            let mut buffer = vec![0u8; 2048];
            while !stop.load(Ordering::Acquire) {
                let timeout = loop {
                    match rtc.poll_output() {
                        Ok(Output::Timeout(at)) => break at,
                        Ok(Output::Transmit(t)) => {
                            let data: Vec<u8> = t.contents.into();
                            let _ = socket.send_to(&data, t.destination);
                        }
                        Ok(Output::Event(Event::MediaAdded(added))) if added.kind.is_video() => {
                            video = Some(added.mid)
                        }
                        Ok(Output::Event(Event::MediaData(data))) if Some(data.mid) == video => {
                            units.lock().unwrap().push(data.data.to_vec())
                        }
                        Ok(Output::Event(Event::ChannelOpen(id, label))) if label == "control" => {
                            control_channel = Some(id)
                        }
                        Ok(Output::Event(_)) => {}
                        Err(_) => return,
                    }
                };
                if let Some(mid) = video
                    && pli.swap(0, Ordering::AcqRel) > 0
                {
                    rtc.direct_api()
                        .stream_rx_by_mid(mid, None)
                        .expect("receiving video")
                        .request_keyframe(KeyframeRequestKind::Pli);
                }
                if let Some(id) = control_channel {
                    for text in control.lock().unwrap().drain(..) {
                        let _ = rtc.channel(id).map(|mut c| c.write(false, text.as_bytes()));
                    }
                }
                let wait = timeout
                    .saturating_duration_since(Instant::now())
                    .clamp(Duration::from_millis(1), Duration::from_millis(5));
                socket.set_read_timeout(Some(wait)).unwrap();
                let now = Instant::now();
                match socket.recv_from(&mut buffer) {
                    Ok((length, source)) => {
                        if let Ok(receive) =
                            Receive::new(Protocol::Udp, source, local, &buffer[..length])
                        {
                            let _ = rtc.handle_input(Input::Receive(now, receive));
                        }
                    }
                    Err(_) => {
                        let _ = rtc.handle_input(Input::Timeout(now));
                    }
                }
            }
        }));
        (peer, answer)
    }

    fn count(&self) -> usize {
        self.units.lock().unwrap().len()
    }

    fn keyframes(&self, hevc: bool) -> usize {
        let key = access_unit(hevc, true);
        self.units
            .lock()
            .unwrap()
            .iter()
            .filter(|u| **u == key)
            .count()
    }
}

impl Drop for Peer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn media_port() -> u16 {
    static NEXT: AtomicU16 = AtomicU16::new(44000);
    loop {
        let port = NEXT.fetch_add(1, Ordering::Relaxed);
        assert!(port < 50000, "fixture WebRTC media port range exhausted");
        if UdpSocket::bind(address(port)).is_ok() {
            return port;
        }
    }
}

/// A host with the Moonlight workers, the WebRTC server and its media transport.
struct Web {
    harness: Harness,
    server: Option<server::Running>,
    signalling: SocketAddr,
    media_port: u16,
}

impl Web {
    async fn new() -> Result<Self> {
        let harness = Harness::new(10_000, 1).await?;
        harness.h.codecs.fetch_or(0x100, Ordering::AcqRel);
        let media_port = media_port();
        harness
            .h
            .config
            .write()
            .unwrap()
            .update(
                json!({"webrtc_media_port": media_port})
                    .as_object()
                    .unwrap(),
            )
            .unwrap();
        let server = server::start_on(
            harness.h.clone(),
            [127, 0, 0, 1].into(),
            0,
            0,
            harness.media.clone(),
        )
        .context("the WebRTC server did not start")?;
        let signalling = harness
            .h
            .webrtc
            .status()
            .signalling
            .context("the WebRTC server is not listening")?;
        Ok(Self {
            harness,
            server: Some(server),
            signalling,
            media_port,
        })
    }

    /// A paired WebRTC device with these permissions, signed in on a new connection.
    async fn device(&self, name: &str, permission: u32) -> Ws {
        let credentials = self.pair(name, permission);
        self.sign_in(&credentials).await
    }

    /// Pair a WebRTC device: its client id and secret.
    fn pair(&self, name: &str, permission: u32) -> (String, String) {
        let id = hex::encode(crypto::random::<16>());
        let secret = hex::encode(crypto::random::<32>());
        let mut client = PairedClient::paired_over_webrtc(name.into(), &id, &secret);
        client.perm = permission;
        self.harness.h.paired.write().unwrap().clients.push(client);
        (id, secret)
    }

    /// The host's id for the device paired as `name`.
    fn uuid(&self, name: &str) -> String {
        let paired = self.harness.h.paired.read().unwrap();
        let client = paired.clients.iter().find(|c| c.name == name);
        client.expect("the device is paired").uuid.clone()
    }

    /// A new signalling connection for a paired device.
    async fn sign_in(&self, (id, secret): &(String, String)) -> Ws {
        let mut ws = Ws::connect(self.signalling).await;
        let nonce = ws.expect("auth-required").await["nonce"]
            .as_str()
            .unwrap()
            .to_owned();
        ws.send(json!({"version":2,"type":"authenticate","clientId":id,"proof":proof_hex(secret, &nonce).unwrap()})).await;
        ws.expect("authenticated").await;
        ws.expect("gateway-status").await;
        ws.expect("capabilities").await;
        ws
    }

    fn app(&self) -> String {
        self.harness.h.apps.read().unwrap()[0].id().to_string()
    }

    fn request(&self, kind: &str, codec: &str, hdr: bool, channels: u32) -> Value {
        json!({"version":2,"type":kind,"appId":self.app(),"video":{"width":1920,"height":1080,"fps":60,"codec":codec,"bitrateKbps":20000,"hdr":hdr},"audio":{"channels":channels}})
    }

    /// The one active stream session.
    fn session(&self) -> Arc<Session> {
        let sessions = self.harness.h.sessions.lock().unwrap();
        assert_eq!(sessions.active.len(), 1);
        sessions.active.values().next().unwrap().clone()
    }

    /// A Remote Monitor session of another device, which no WebRTC request stops.
    fn add_monitor(&self) -> Arc<Session> {
        let fixture = crate::state::test_support::Fixture {
            host: self.harness.h.clone(),
        };
        let monitor = fixture.session(fixture.client(u32::MAX), Role::RemoteMonitor);
        // Its drop would end the harness host's app and delete its directory.
        std::mem::forget(fixture);
        monitor
    }

    /// No app runs, so the next launch starts one and records its owner.
    fn clear_app(&self) {
        *self.harness.h.current_app.lock().unwrap() = None;
    }

    /// The owner of the running app.
    fn owner(&self) -> String {
        let app = self.harness.h.current_app.lock().unwrap();
        app.as_ref().expect("an app is running").owner.clone()
    }

    async fn idle(&self) {
        until("sessions and WebRTC links released", WAIT, || {
            self.harness.h.sessions.lock().unwrap().idle() && self.harness.h.webrtc_links.len() == 0
        })
        .await;
    }
}

/// The next message that is not a gateway or capability refresh.
async fn next(ws: &mut Ws) -> Value {
    loop {
        let message = ws.next().await.expect("the socket stays open");
        if !matches!(
            message["type"].as_str(),
            Some("gateway-status" | "capabilities")
        ) {
            return message;
        }
    }
}

async fn status(ws: &mut Ws, kind: &str, state: &str) -> Value {
    let message = next(ws).await;
    assert_eq!(message["type"], kind, "{message}");
    assert_eq!(message["state"], state, "{message}");
    message
}

async fn error(ws: &mut Ws, request: &str, code: &str) {
    let message = next(ws).await;
    assert_eq!(message["type"], "error", "{message}");
    assert_eq!(message["requestType"], request, "{message}");
    assert_eq!(message["code"], code, "{message}");
}

/// Offer, answer and the first video, from the start of `status(.., "starting-webrtc")` on.
async fn answer(ws: &mut Ws, codec: VideoCodec) -> (u64, Peer) {
    let starting = status(ws, "session-status", "starting-webrtc").await;
    let offer = next(ws).await;
    assert_eq!(offer["type"], "offer", "{offer}");
    let id = offer["sessionId"].as_u64().unwrap();
    assert_eq!(starting["sessionId"], id);
    let (peer, sdp) = Peer::answer(offer["sdp"].as_str().unwrap(), codec);
    ws.send(json!({"version":2,"type":"answer","sessionId":id,"sdp":sdp}))
        .await;
    (id, peer)
}

async fn streaming(ws: &mut Ws, id: u64) {
    let message = status(ws, "session-status", "streaming").await;
    assert_eq!(message["sessionId"], id);
    assert!(message["video"].is_object() && message["audio"].is_object());
}

async fn stream_and_stop(codec: VideoCodec, name: &str, hevc: bool) -> Result<()> {
    let web = Web::new().await?;
    let mut ws = web.device("TV", u32::MAX).await;
    ws.send(web.request("start-session", name, false, 2)).await;
    status(&mut ws, "session-status", "starting").await;
    let (id, peer) = answer(&mut ws, codec).await;
    streaming(&mut ws, id).await;

    // Video arrives as the encoder produced it, starting with a keyframe.
    until("video received", WAIT, || peer.count() >= 5).await;
    let (key, delta) = (access_unit(hevc, true), access_unit(hevc, false));
    {
        let units = peer.units.lock().unwrap();
        assert_eq!(units[0], key);
        assert!(units.iter().all(|u| *u == key || *u == delta));
    }
    let session = web.session();
    assert_eq!(
        session.launch.transport,
        butterpollo_core::session::Transport::WebRtc
    );
    assert!(session.stats.frames.load(Ordering::Acquire) >= 5);
    assert!(session.stats.bytes.load(Ordering::Acquire) > 0);

    // A PLI is a keyframe request, as Moonlight's 0x0302.
    let requests = session.stats.idr_requests.load(Ordering::Acquire);
    let keyframes = peer.keyframes(hevc);
    peer.pli.store(1, Ordering::Release);
    until("the PLI reached the session", WAIT, || {
        session.stats.idr_requests.load(Ordering::Acquire) > requests
    })
    .await;
    until("a keyframe followed the PLI", WAIT, || {
        peer.keyframes(hevc) > keyframes
    })
    .await;

    // So is the control channel's request-keyframe.
    let requests = session.stats.idr_requests.load(Ordering::Acquire);
    peer.control
        .lock()
        .unwrap()
        .push(r#"{"type":"request-keyframe"}"#.into());
    until("the control request reached the session", WAIT, || {
        session.stats.idr_requests.load(Ordering::Acquire) > requests
    })
    .await;

    // stop-session ends the stream and frees the port; the app keeps running.
    ws.send(json!({"version":2,"type":"stop-session"})).await;
    status(&mut ws, "session-status", "stopping").await;
    status(&mut ws, "session-status", "idle").await;
    web.idle().await;
    assert!(web.harness.h.current_app.lock().unwrap().is_some());
    assert!(UdpSocket::bind(address(web.media_port)).is_ok());
    drop(peer);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn h264_sdr_streams_to_a_webrtc_client_and_stops() -> Result<()> {
    stream_and_stop(VideoCodec::H264, "h264", false).await
}

#[tokio::test(flavor = "multi_thread")]
async fn hevc_sdr_streams_to_a_webrtc_client_and_stops() -> Result<()> {
    stream_and_stop(VideoCodec::Hevc, "hevc", true).await
}

#[tokio::test(flavor = "multi_thread")]
async fn hdr_av1_and_surround_are_refused_not_streamed_as_something_else() -> Result<()> {
    let web = Web::new().await?;
    web.harness.h.codecs.fetch_or(0x10000, Ordering::AcqRel);
    let mut ws = web.device("TV", u32::MAX).await;
    for (codec, hdr, channels) in [("hevc", true, 2), ("av1", false, 2), ("h264", false, 6)] {
        ws.send(web.request("start-session", codec, hdr, channels))
            .await;
        error(&mut ws, "start-session", "unsupported-settings").await;
    }
    // A switch asks the same, and nothing was started or stopped.
    ws.send(web.request("switch-session", "hevc", true, 2))
        .await;
    error(&mut ws, "switch-session", "unsupported-settings").await;
    let sessions = web.harness.h.sessions.lock().unwrap();
    assert!(sessions.idle() && sessions.pending.is_empty());
    drop(sessions);
    assert_eq!(web.harness.h.webrtc_links.len(), 0);
    assert!(web.harness.h.current_app.lock().unwrap().is_some());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_device_that_may_not_launch_or_names_no_app_starts_nothing() -> Result<()> {
    let web = Web::new().await?;
    let mut viewer = web.device("Viewer", VIEW | LIST).await;
    viewer
        .send(web.request("start-session", "h264", false, 2))
        .await;
    error(&mut viewer, "start-session", "not-authenticated").await;
    viewer
        .send(json!({"version":2,"type":"stop-host-session"}))
        .await;
    error(&mut viewer, "stop-host-session", "not-authenticated").await;

    let mut ws = web.device("TV", u32::MAX).await;
    let mut request = web.request("start-session", "h264", false, 2);
    request["appId"] = json!("99999999");
    ws.send(request).await;
    status(&mut ws, "session-status", "starting").await;
    error(&mut ws, "start-session", "invalid-application").await;
    let mut request = web.request("start-session", "h264", false, 2);
    request["appId"] = json!("not a number");
    ws.send(request).await;
    error(&mut ws, "start-session", "invalid-application").await;
    web.idle().await;
    assert!(web.harness.h.sessions.lock().unwrap().pending.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn switch_session_resumes_the_running_app_and_stop_host_ends_it() -> Result<()> {
    let web = Web::new().await?;
    let mut ws = web.device("TV", u32::MAX).await;
    ws.send(web.request("start-session", "h264", false, 2))
        .await;
    status(&mut ws, "session-status", "starting").await;
    let (first, peer) = answer(&mut ws, VideoCodec::H264).await;
    streaming(&mut ws, first).await;
    until("video received", WAIT, || peer.count() >= 2).await;

    // The target is the running app: the stream restarts and the app stays.
    ws.send(web.request("switch-session", "hevc", false, 2))
        .await;
    let resuming = status(&mut ws, "host-session-status", "resuming").await;
    assert_eq!(resuming["runningAppId"], web.app());
    assert_eq!(resuming["targetAppId"], web.app());
    status(&mut ws, "session-status", "stopping").await;
    status(&mut ws, "session-status", "idle").await;
    status(&mut ws, "host-session-status", "starting").await;
    drop(peer);
    let (second, peer) = answer(&mut ws, VideoCodec::Hevc).await;
    assert_ne!(first, second);
    streaming(&mut ws, second).await;
    until("video of the new session", WAIT, || {
        peer.keyframes(true) >= 1
    })
    .await;
    assert!(web.harness.h.current_app.lock().unwrap().is_some());

    // stop-host-session ends the stream and the app.
    ws.send(json!({"version":2,"type":"stop-host-session"}))
        .await;
    let stopping = status(&mut ws, "host-session-status", "stopping").await;
    assert_eq!(stopping["runningAppId"], web.app());
    status(&mut ws, "session-status", "stopping").await;
    status(&mut ws, "session-status", "idle").await;
    status(&mut ws, "host-session-status", "stopped").await;
    web.idle().await;
    assert!(web.harness.h.current_app.lock().unwrap().is_none());
    // With nothing running it says so at once.
    ws.send(json!({"version":2,"type":"stop-host-session"}))
        .await;
    status(&mut ws, "host-session-status", "stopped").await;
    drop(peer);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_second_webrtc_device_is_refused_while_one_streams() -> Result<()> {
    let web = Web::new().await?;
    let mut first = web.device("TV", u32::MAX).await;
    first
        .send(web.request("start-session", "h264", false, 2))
        .await;
    status(&mut first, "session-status", "starting").await;
    let (id, peer) = answer(&mut first, VideoCodec::H264).await;
    streaming(&mut first, id).await;

    let mut second = web.device("Phone", u32::MAX).await;
    second
        .send(web.request("start-session", "h264", false, 2))
        .await;
    status(&mut second, "session-status", "starting").await;
    error(&mut second, "start-session", "session-active").await;
    // The first stream did not notice.
    assert!(!web.session().stopping());
    // The same device cannot start a second one on its own connection either.
    first
        .send(web.request("start-session", "h264", false, 2))
        .await;
    error(&mut first, "start-session", "session-active").await;
    drop(peer);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_moonlight_launch_replaces_a_webrtc_stream() -> Result<()> {
    let web = Web::new().await?;
    let mut ws = web.device("TV", u32::MAX).await;
    ws.send(web.request("start-session", "h264", false, 2))
        .await;
    status(&mut ws, "session-status", "starting").await;
    let (id, peer) = answer(&mut ws, VideoCodec::H264).await;
    streaming(&mut ws, id).await;
    until("video received", WAIT, || peer.count() >= 2).await;
    let webrtc = web.session();

    let launch = web.harness.launch(0, false).await?;
    let idle = status(&mut ws, "session-status", "idle").await;
    assert_eq!(idle["sessionId"], id);
    assert!(webrtc.stopping());
    let (session, client) = web.harness.play(&launch).await?;
    assert_eq!(
        session.launch.transport,
        butterpollo_core::session::Transport::GameStream
    );
    web.harness.counts(1, 0).await?;
    web.harness.end(&session, client).await?;
    drop(peer);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_webrtc_launch_replaces_a_moonlight_stream() -> Result<()> {
    let web = Web::new().await?;
    let launch = web.harness.launch(0, false).await?;
    let (moonlight, client) = web.harness.play(&launch).await?;

    let mut ws = web.device("TV", u32::MAX).await;
    ws.send(web.request("start-session", "h264", false, 2))
        .await;
    status(&mut ws, "session-status", "starting").await;
    let (id, peer) = answer(&mut ws, VideoCodec::H264).await;
    streaming(&mut ws, id).await;
    until("the Moonlight stream ended", WAIT, || moonlight.stopping()).await;
    until("video received", WAIT, || peer.count() >= 2).await;
    assert_eq!(
        web.session().launch.transport,
        butterpollo_core::session::Transport::WebRtc
    );
    drop(client);

    ws.send(json!({"version":2,"type":"stop-session"})).await;
    status(&mut ws, "session-status", "stopping").await;
    status(&mut ws, "session-status", "idle").await;
    web.idle().await;
    drop(peer);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn closing_the_signalling_socket_ends_the_stream() -> Result<()> {
    let web = Web::new().await?;
    let mut ws = web.device("TV", u32::MAX).await;
    ws.send(web.request("start-session", "h264", false, 2))
        .await;
    status(&mut ws, "session-status", "starting").await;
    let (id, peer) = answer(&mut ws, VideoCodec::H264).await;
    streaming(&mut ws, id).await;
    let session = web.session();
    drop(ws);
    until("the session stopped", WAIT, || session.stopping()).await;
    web.idle().await;
    drop(peer);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unanswered_offer_is_dropped_when_the_client_stops() -> Result<()> {
    let web = Web::new().await?;
    let mut ws = web.device("TV", u32::MAX).await;
    ws.send(web.request("start-session", "h264", false, 2))
        .await;
    status(&mut ws, "session-status", "starting").await;
    status(&mut ws, "session-status", "starting-webrtc").await;
    assert_eq!(next(&mut ws).await["type"], "offer");
    assert_eq!(web.harness.h.sessions.lock().unwrap().pending.len(), 1);
    // Nothing was streaming, so there is nothing to stop: straight to idle.
    ws.send(json!({"version":2,"type":"stop-session"})).await;
    status(&mut ws, "session-status", "idle").await;
    web.idle().await;
    assert!(web.harness.h.sessions.lock().unwrap().pending.is_empty());
    // The port is free for the next offer, and the app was left running.
    assert!(UdpSocket::bind(address(web.media_port)).is_ok());
    assert!(web.harness.h.current_app.lock().unwrap().is_some());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn with_the_setting_off_no_media_socket_opens() -> Result<()> {
    let mut web = Web::new().await?;
    // The harness host has the setting off: `start` opens nothing, not even the media port.
    web.server = None;
    assert!(!web.harness.h.config.read().unwrap().webrtc_server());
    assert!(
        server::start(
            web.harness.h.clone(),
            [127, 0, 0, 1].into(),
            web.harness.media.clone()
        )
        .is_none()
    );
    assert!(UdpSocket::bind(address(web.media_port)).is_ok());
    assert_eq!(web.harness.h.webrtc_links.len(), 0);
    Ok(())
}

/// Start a stream for a signed-in device and wait for its first video.
async fn stream(web: &Web, ws: &mut Ws) -> Peer {
    ws.send(web.request("start-session", "h264", false, 2))
        .await;
    status(ws, "session-status", "starting").await;
    let (id, peer) = answer(ws, VideoCodec::H264).await;
    streaming(ws, id).await;
    until("video received", WAIT, || peer.count() >= 2).await;
    peer
}

#[tokio::test(flavor = "multi_thread")]
async fn remote_sessions_do_not_hold_up_a_switch_or_stop_host_session() -> Result<()> {
    let web = Web::new().await?;
    let mut ws = web.device("TV", u32::MAX).await;
    web.clear_app();
    let peer = stream(&web, &mut ws).await;
    assert_eq!(web.owner(), web.uuid("TV"));
    let monitor = web.add_monitor();

    // Resuming the app stops the stream only: waiting for the monitor would time out.
    let started = Instant::now();
    ws.send(web.request("switch-session", "h264", false, 2))
        .await;
    status(&mut ws, "host-session-status", "resuming").await;
    status(&mut ws, "session-status", "stopping").await;
    status(&mut ws, "session-status", "idle").await;
    status(&mut ws, "host-session-status", "starting").await;
    assert!(started.elapsed() < WAIT);
    drop(peer);
    let (id, peer) = answer(&mut ws, VideoCodec::H264).await;
    streaming(&mut ws, id).await;

    // So does ending the app, which its owner may do whoever else is connected.
    ws.send(json!({"version":2,"type":"stop-host-session"}))
        .await;
    status(&mut ws, "host-session-status", "stopping").await;
    status(&mut ws, "session-status", "stopping").await;
    status(&mut ws, "session-status", "idle").await;
    status(&mut ws, "host-session-status", "stopped").await;
    assert!(web.harness.h.current_app.lock().unwrap().is_none());
    assert!(!monitor.stopping());
    assert_eq!(web.harness.h.sessions.lock().unwrap().active.len(), 1);
    drop(peer);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn only_the_owner_may_end_the_app_while_remote_sessions_are_active() -> Result<()> {
    let web = Web::new().await?;
    let mut tv = web.device("TV", u32::MAX).await;
    web.clear_app();
    let peer = stream(&web, &mut tv).await;
    assert_eq!(web.owner(), web.uuid("TV"));
    let session = web.session();
    let monitor = web.add_monitor();

    // Another device with every permission is refused, as Moonlight's cancel answers a 403,
    // before anything is announced or stopped.
    let mut phone = web.device("Phone", u32::MAX).await;
    phone
        .send(json!({"version":2,"type":"stop-host-session"}))
        .await;
    error(&mut phone, "stop-host-session", "session-active").await;
    let mut other_app = web.request("switch-session", "h264", false, 2);
    other_app["appId"] = json!("12345");
    phone.send(other_app).await;
    error(&mut phone, "switch-session", "session-active").await;
    assert!(!session.stopping());
    assert!(web.harness.h.current_app.lock().unwrap().is_some());

    // With no remote session left anyone may end it.
    web.harness
        .h
        .sessions
        .lock()
        .unwrap()
        .active
        .remove(&monitor.launch.id);
    phone
        .send(json!({"version":2,"type":"stop-host-session"}))
        .await;
    status(&mut phone, "host-session-status", "stopping").await;
    status(&mut phone, "host-session-status", "stopped").await;
    assert!(session.stopping());
    assert!(web.harness.h.current_app.lock().unwrap().is_none());
    drop(peer);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_worker_that_fails_before_taking_its_link_still_frees_the_media_port() -> Result<()> {
    let web = Web::new().await?;
    web.harness
        .fixture
        .webrtc_fails_before_its_link
        .store(true, Ordering::Release);
    let mut ws = web.device("TV", u32::MAX).await;
    ws.send(web.request("start-session", "h264", false, 2))
        .await;
    status(&mut ws, "session-status", "starting").await;
    let (_, peer) = answer(&mut ws, VideoCodec::H264).await;
    // The link was handed over and the worker never took it: the teardown releases it.
    web.idle().await;
    assert!(UdpSocket::bind(address(web.media_port)).is_ok());
    drop(peer);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_device_that_starts_again_gets_the_media_socket_from_its_unanswered_start() -> Result<()>
{
    let web = Web::new().await?;
    let credentials = web.pair("TV", u32::MAX);
    let mut old = web.sign_in(&credentials).await;
    old.send(web.request("start-session", "h264", false, 2))
        .await;
    status(&mut old, "session-status", "starting").await;
    status(&mut old, "session-status", "starting-webrtc").await;
    assert_eq!(next(&mut old).await["type"], "offer");

    // The offer is still waiting for its answer, on the one media port, when the device returns
    // on a new connection, which closes the old one. Releasing the old start's launch is slow, as
    // a virtual display that takes seconds to go is, and the new start must not wait for it.
    web.harness
        .fixture
        .audio_release_ms
        .store(4_000, Ordering::Release);
    let mut new = web.sign_in(&credentials).await;
    while old.next().await.is_some() {}
    new.send(web.request("start-session", "h264", false, 2))
        .await;
    status(&mut new, "session-status", "starting").await;
    let (id, peer) = answer(&mut new, VideoCodec::H264).await;
    streaming(&mut new, id).await;
    until("video received", WAIT, || peer.count() >= 2).await;
    assert_eq!(web.harness.h.sessions.lock().unwrap().pending.len(), 0);
    web.harness
        .fixture
        .audio_release_ms
        .store(0, Ordering::Release);
    drop(peer);
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn cancelling_a_start_releases_its_resources_off_the_async_tasks() -> Result<()> {
    let web = Web::new().await?;
    let mut ws = web.device("TV", u32::MAX).await;
    ws.send(web.request("start-session", "h264", false, 2))
        .await;
    status(&mut ws, "session-status", "starting").await;
    status(&mut ws, "session-status", "starting-webrtc").await;
    assert_eq!(next(&mut ws).await["type"], "offer");

    // Dropping the launch takes this long, as a virtual display or audio route that blocks does.
    // The one thread of this runtime runs the session actor, the signalling server and this
    // test, so a release on it shows as a gap between ticks.
    web.harness
        .fixture
        .audio_release_ms
        .store(600, Ordering::Release);
    let gap = Arc::new(AtomicU64::new(0));
    let ticker = tokio::spawn({
        let gap = gap.clone();
        async move {
            loop {
                let before = Instant::now();
                tokio::time::sleep(Duration::from_millis(10)).await;
                gap.fetch_max(before.elapsed().as_millis() as u64, Ordering::AcqRel);
            }
        }
    });
    ws.send(json!({"version":2,"type":"stop-session"})).await;
    status(&mut ws, "session-status", "idle").await;
    web.idle().await;
    // Let the ticker report a stall that ended just before.
    tokio::time::sleep(Duration::from_millis(50)).await;
    ticker.abort();
    web.harness
        .fixture
        .audio_release_ms
        .store(0, Ordering::Release);
    let longest = gap.load(Ordering::Acquire);
    assert!(longest < 300, "the runtime stalled for {longest} ms");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_webrtc_launch_records_its_device_as_the_apps_owner() -> Result<()> {
    let web = Web::new().await?;
    web.clear_app();
    let mut ws = web.device("TV", u32::MAX).await;
    let peer = stream(&web, &mut ws).await;
    // As a Moonlight launch does, so the owner guard of cancel applies to this app.
    assert_eq!(web.owner(), web.uuid("TV"));
    assert_eq!(web.session().launch.client.uuid, web.uuid("TV"));
    drop(peer);
    Ok(())
}
