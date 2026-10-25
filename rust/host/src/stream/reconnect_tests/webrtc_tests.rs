//! WebRTC sessions through the real signalling server, session actor, media transport and stream
//! worker, beside the Moonlight workers of the reconnect matrix. Only the encoder's output is
//! synthetic; the client is a str0m peer on a loopback UDP socket.
use super::*;
use crate::webrtc::{
    sdp::{AudioMode, OfferSpec, VideoCodec, testing as peer_sdp},
    server::{self, tests::Ws},
};
use butterpollo_core::{audio::OpusLayout, webrtc::auth::proof_hex};
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

/// One AV1 OBU with a size field.
fn obu(kind: u8, seed: u8, len: usize) -> Vec<u8> {
    let mut out = vec![kind << 3 | 0b10];
    let mut size = len;
    loop {
        let byte = (size & 0x7f) as u8;
        size >>= 7;
        if size == 0 {
            out.push(byte);
            break;
        }
        out.push(byte | 0x80);
    }
    out.extend((0..len).map(|i| (i as u8).wrapping_add(seed)));
    out
}

/// The access unit the encoder hands over (`codec` is the negotiated 0 H.264, 1 HEVC, 2 AV1):
/// Annex B of a few RTP packets, parameter sets and an IDR slice or a delta slice; for AV1 the
/// low-overhead OBUs, a temporal delimiter first, a sequence header on a keyframe.
pub(super) fn access_unit(codec: u8, idr: bool) -> Vec<u8> {
    match (codec, idr) {
        (0, true) => [
            nal(&[0x67], 3, 20),
            nal(&[0x68], 5, 6),
            nal(&[0x65], 7, 3000),
        ]
        .concat(),
        (0, false) => nal(&[0x41], 9, 1500),
        (1, true) => [
            nal(&[0x40, 0x01], 3, 20),
            nal(&[0x42, 0x01], 4, 30),
            nal(&[0x44, 0x01], 5, 6),
            nal(&[0x26, 0x01], 7, 3000),
        ]
        .concat(),
        (1, false) => nal(&[0x02, 0x01], 9, 1500),
        (_, true) => [vec![0x12, 0], obu(1, 3, 12), obu(6, 9, 3000)].concat(),
        (_, false) => [vec![0x12, 0], obu(6, 9, 1500)].concat(),
    }
}

/// What the receiver gets for [`access_unit`]: the same, but AV1 travels without its temporal
/// delimiter.
fn received(codec: u8, idr: bool) -> Vec<u8> {
    let unit = access_unit(codec, idr);
    if codec == 2 { unit[2..].to_vec() } else { unit }
}

/// The audio packet the fixture's encoder stands in with: it names the layout it was built with,
/// so the receiver can tell which one the session encoded. The fixture's worker sends two per tick.
pub(super) fn audio_payload(layout: &OpusLayout) -> Vec<u8> {
    let mut payload = vec![
        layout.channels as u8,
        layout.streams as u8,
        layout.coupled as u8,
    ];
    payload.extend_from_slice(&layout.mapping);
    payload.extend_from_slice(&((layout.bitrate / 1000) as u16).to_be_bytes());
    payload
}

/// What the client asks for and what its answer agrees to.
#[derive(Clone, Copy)]
struct Case {
    video: VideoCodec,
    name: &'static str,
    /// The negotiated codec number: 0 H.264, 1 HEVC, 2 AV1.
    codec: u8,
    hdr: bool,
    channels: u32,
}

impl Case {
    const fn new(video: VideoCodec, name: &'static str, codec: u8) -> Self {
        Self {
            video,
            name,
            codec,
            hdr: false,
            channels: 2,
        }
    }

    const fn hdr(self) -> Self {
        Self { hdr: true, ..self }
    }

    const fn surround(self, channels: u32) -> Self {
        Self { channels, ..self }
    }

    /// The codecs and audio the peer answers with.
    fn spec(&self) -> OfferSpec {
        OfferSpec {
            audio: match self.channels {
                2 => AudioMode::stereo(),
                channels => AudioMode::chrome_surround(channels as usize).unwrap(),
            },
            ..peer_sdp::spec(self.video, self.hdr)
        }
    }

    /// The payload of the audio the host sends for this case: Chromium's canonical layouts at
    /// GameStream's surround bitrates, and the protocol's 128 kbps stereo.
    fn audio(&self) -> Vec<u8> {
        let (streams, coupled, mapping, kbps): (u8, u8, &[u8], u16) = match self.channels {
            2 => (1, 1, &[0, 1], 128),
            6 => (4, 2, &[0, 4, 1, 2, 3, 5], 256),
            _ => (5, 3, &[0, 6, 1, 2, 3, 4, 5, 7], 450),
        };
        let mut payload = vec![self.channels as u8, streams, coupled];
        payload.extend_from_slice(mapping);
        payload.extend_from_slice(&kbps.to_be_bytes());
        payload
    }
}

/// A browser-like client: it answers the offer, connects over loopback and records the video it
/// receives. It asks for keyframes on request.
struct Peer {
    stop: Arc<AtomicBool>,
    units: Arc<Mutex<Vec<Vec<u8>>>>,
    audio: Arc<Mutex<Vec<Vec<u8>>>>,
    pli: Arc<AtomicUsize>,
    control: Arc<Mutex<Vec<String>>>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Peer {
    /// The peer and the answer SDP for `offer`.
    fn answer(offer: &str, spec: &OfferSpec) -> (Self, String) {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let local = socket.local_addr().unwrap();
        let mut rtc = peer_sdp::client(spec, local);
        let answer = peer_sdp::answer_for(&mut rtc, offer, spec);
        let mut peer = Self {
            stop: Arc::new(AtomicBool::new(false)),
            units: Arc::default(),
            audio: Arc::default(),
            pli: Arc::default(),
            control: Arc::default(),
            thread: None,
        };
        let (stop, units, audio, pli, control) = (
            peer.stop.clone(),
            peer.units.clone(),
            peer.audio.clone(),
            peer.pli.clone(),
            peer.control.clone(),
        );
        peer.thread = Some(thread::spawn(move || {
            let mut video: Option<Mid> = None;
            let mut sound: Option<Mid> = None;
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
                        Ok(Output::Event(Event::MediaAdded(added))) if added.kind.is_audio() => {
                            sound = Some(added.mid)
                        }
                        Ok(Output::Event(Event::MediaData(data))) if Some(data.mid) == video => {
                            units.lock().unwrap().push(data.data.to_vec())
                        }
                        Ok(Output::Event(Event::MediaData(data))) if Some(data.mid) == sound => {
                            audio.lock().unwrap().push(data.data.to_vec())
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

    fn keyframes(&self, codec: u8) -> usize {
        let key = received(codec, true);
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
    answer_case(ws, &Case::new(codec, "", 0)).await
}

async fn answer_case(ws: &mut Ws, case: &Case) -> (u64, Peer) {
    let starting = status(ws, "session-status", "starting-webrtc").await;
    let offer = next(ws).await;
    assert_eq!(offer["type"], "offer", "{offer}");
    let id = offer["sessionId"].as_u64().unwrap();
    assert_eq!(starting["sessionId"], id);
    let (peer, sdp) = Peer::answer(offer["sdp"].as_str().unwrap(), &case.spec());
    ws.send(json!({"version":2,"type":"answer","sessionId":id,"sdp":sdp}))
        .await;
    (id, peer)
}

async fn streaming(ws: &mut Ws, id: u64) {
    let message = status(ws, "session-status", "streaming").await;
    assert_eq!(message["sessionId"], id);
    assert!(message["video"].is_object() && message["audio"].is_object());
}

/// A host that has every encoder the cases ask for: AV1, AV1 10-bit and HEVC Main10 as well.
async fn web_with_every_codec() -> Result<Web> {
    let web = Web::new().await?;
    web.harness
        .h
        .codecs
        .fetch_or(0x10000 | 0x20000 | 0x200, Ordering::AcqRel);
    Ok(web)
}

async fn stream_and_stop(case: Case) -> Result<()> {
    let web = web_with_every_codec().await?;
    let mut ws = web.device("TV", u32::MAX).await;
    ws.send(web.request("start-session", case.name, case.hdr, case.channels))
        .await;
    status(&mut ws, "session-status", "starting").await;
    let (id, peer) = answer_case(&mut ws, &case).await;
    streaming(&mut ws, id).await;

    // Video arrives as the encoder produced it, starting with a keyframe.
    until("video received", WAIT, || peer.count() >= 5).await;
    let codec = case.codec;
    let (key, delta) = (received(codec, true), received(codec, false));
    {
        let units = peer.units.lock().unwrap();
        assert_eq!(units[0], key);
        assert!(units.iter().all(|u| *u == key || *u == delta));
    }
    // So does the audio, in the layout the answer agreed to.
    until("audio received", WAIT, || {
        peer.audio.lock().unwrap().len() >= 10
    })
    .await;
    {
        let audio = peer.audio.lock().unwrap();
        let want = case.audio();
        assert!(
            audio.iter().all(|p| *p == want),
            "{} channels: {:?} but expected {want:?}",
            case.channels,
            audio.iter().find(|p| **p != want)
        );
    }
    let session = web.session();
    assert_eq!(
        session.launch.transport,
        butterpollo_core::session::Transport::WebRtc
    );
    // The stream is configured as the GameStream path would for the same request.
    assert_eq!(session.config.codec, codec);
    assert_eq!(session.config.hdr, case.hdr);
    assert_eq!(session.config.audio_channels as u32, case.channels);
    assert_eq!(session.config.ten_bit(), case.hdr);
    if case.hdr {
        // 10-bit BT.2020 PQ in limited range: what the HEVC VUI and the AV1 sequence header carry.
        assert_eq!(session.config.color_matrix(), 2);
        assert!(!session.config.full_range());
    } else {
        assert_eq!(session.config.color_matrix(), 1);
    }
    assert!(session.stats.frames.load(Ordering::Acquire) >= 5);
    assert!(session.stats.bytes.load(Ordering::Acquire) > 0);

    // A PLI is a keyframe request, as Moonlight's 0x0302.
    let requests = session.stats.idr_requests.load(Ordering::Acquire);
    let keyframes = peer.keyframes(codec);
    peer.pli.store(1, Ordering::Release);
    until("the PLI reached the session", WAIT, || {
        session.stats.idr_requests.load(Ordering::Acquire) > requests
    })
    .await;
    until("a keyframe followed the PLI", WAIT, || {
        peer.keyframes(codec) > keyframes
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

const H264: Case = Case::new(VideoCodec::H264, "h264", 0);
const HEVC: Case = Case::new(VideoCodec::Hevc, "hevc", 1);
const AV1: Case = Case::new(VideoCodec::Av1, "av1", 2);

#[tokio::test(flavor = "multi_thread")]
async fn h264_sdr_streams_to_a_webrtc_client_and_stops() -> Result<()> {
    stream_and_stop(H264).await
}

#[tokio::test(flavor = "multi_thread")]
async fn hevc_sdr_streams_to_a_webrtc_client_and_stops() -> Result<()> {
    stream_and_stop(HEVC).await
}

#[tokio::test(flavor = "multi_thread")]
async fn av1_sdr_streams_to_a_webrtc_client_without_temporal_delimiters() -> Result<()> {
    // `received` is the access unit minus its temporal delimiter, so the whole run is checked.
    assert_eq!(
        access_unit(2, false)[..2],
        [0x12, 0],
        "the encoder's AV1 starts with a temporal delimiter"
    );
    stream_and_stop(AV1).await
}

#[tokio::test(flavor = "multi_thread")]
async fn hevc_main10_hdr_streams_to_a_webrtc_client() -> Result<()> {
    stream_and_stop(HEVC.hdr()).await
}

#[tokio::test(flavor = "multi_thread")]
async fn av1_10_bit_hdr_streams_to_a_webrtc_client() -> Result<()> {
    stream_and_stop(AV1.hdr()).await
}

#[tokio::test(flavor = "multi_thread")]
async fn five_one_surround_streams_to_a_webrtc_client() -> Result<()> {
    stream_and_stop(HEVC.surround(6)).await
}

#[tokio::test(flavor = "multi_thread")]
async fn seven_one_surround_streams_to_a_webrtc_client() -> Result<()> {
    stream_and_stop(H264.surround(8)).await
}

#[tokio::test(flavor = "multi_thread")]
async fn hdr_surround_and_av1_together_stream_to_a_webrtc_client() -> Result<()> {
    stream_and_stop(AV1.hdr().surround(8)).await
}

#[tokio::test(flavor = "multi_thread")]
async fn av1_and_hdr_are_refused_without_the_encoder_probe() -> Result<()> {
    // No AV1, no Main10: the base table (H.264, HEVC) only.
    let web = Web::new().await?;
    web.harness.h.codecs.store(1 | 0x100, Ordering::Release);
    let mut ws = web.device("TV", u32::MAX).await;
    for (codec, hdr) in [
        ("av1", false),
        ("av1", true),
        ("hevc", true),
        ("h264", true),
    ] {
        ws.send(web.request("start-session", codec, hdr, 2)).await;
        error(&mut ws, "start-session", "unsupported-settings").await;
    }
    // A switch asks the same, and nothing was started or stopped.
    ws.send(web.request("switch-session", "av1", false, 2))
        .await;
    error(&mut ws, "switch-session", "unsupported-settings").await;
    // AV1 8-bit alone does not make AV1 HDR; HEVC Main10 does not either.
    web.harness.h.codecs.fetch_or(0x10000, Ordering::AcqRel);
    ws.send(web.request("start-session", "av1", true, 2)).await;
    error(&mut ws, "start-session", "unsupported-settings").await;
    web.harness.h.codecs.fetch_or(0x200, Ordering::AcqRel);
    ws.send(web.request("start-session", "av1", true, 2)).await;
    error(&mut ws, "start-session", "unsupported-settings").await;
    let sessions = web.harness.h.sessions.lock().unwrap();
    assert!(sessions.idle() && sessions.pending.is_empty());
    drop(sessions);
    assert_eq!(web.harness.h.webrtc_links.len(), 0);
    assert!(web.harness.h.current_app.lock().unwrap().is_some());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn hdr_on_an_sdr_display_ends_the_session_with_an_error() -> Result<()> {
    let web = web_with_every_codec().await?;
    web.harness
        .fixture
        .display_in_sdr
        .store(true, Ordering::Release);
    let mut ws = web.device("TV", u32::MAX).await;
    ws.send(web.request("start-session", "hevc", true, 2)).await;
    status(&mut ws, "session-status", "starting").await;
    let (id, peer) = answer_case(&mut ws, &HEVC.hdr()).await;
    // The worker refuses before it sends a frame: the client hears an error, not SDR as HDR.
    let message = next(&mut ws).await;
    assert_eq!(message["type"], "session-status", "{message}");
    assert_eq!(message["state"], "error", "{message}");
    assert_eq!(message["sessionId"], id);
    web.idle().await;
    assert_eq!(peer.count(), 0, "no frame went out");
    assert!(UdpSocket::bind(address(web.media_port)).is_ok());
    drop(peer);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_display_that_leaves_hdr_ends_the_stream_with_an_error() -> Result<()> {
    let web = web_with_every_codec().await?;
    let mut ws = web.device("TV", u32::MAX).await;
    ws.send(web.request("start-session", "av1", true, 2)).await;
    status(&mut ws, "session-status", "starting").await;
    let (id, peer) = answer_case(&mut ws, &AV1.hdr()).await;
    streaming(&mut ws, id).await;
    until("video received", WAIT, || peer.count() >= 3).await;

    // Windows drops out of HDR while the frames stay FP16: the stream ends, it does not carry on
    // as SDR labelled HDR.
    web.harness
        .fixture
        .display_in_sdr
        .store(true, Ordering::Release);
    let message = next(&mut ws).await;
    assert_eq!(message["type"], "session-status", "{message}");
    assert_eq!(message["state"], "error", "{message}");
    web.idle().await;
    drop(peer);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn hdr_needs_a_display_colour_space_that_windows_reports() -> Result<()> {
    // The frames are FP16 either way, so without a reading there is nothing to tell HDR from
    // SDR content in an HDR container: the session ends instead of guessing.
    let web = web_with_every_codec().await?;
    web.harness
        .fixture
        .display_unreadable
        .store(true, Ordering::Release);
    let mut ws = web.device("TV", u32::MAX).await;
    ws.send(web.request("start-session", "hevc", true, 2)).await;
    status(&mut ws, "session-status", "starting").await;
    let (id, peer) = answer_case(&mut ws, &HEVC.hdr()).await;
    let message = next(&mut ws).await;
    assert_eq!(message["state"], "error", "{message}");
    assert_eq!(message["sessionId"], id);
    web.idle().await;
    assert_eq!(peer.count(), 0, "no frame went out");
    drop(peer);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failing_colour_space_reading_mid_stream_is_the_captures_to_report() -> Result<()> {
    let web = web_with_every_codec().await?;
    let mut ws = web.device("TV", u32::MAX).await;
    ws.send(web.request("start-session", "hevc", true, 2)).await;
    status(&mut ws, "session-status", "starting").await;
    let (id, peer) = answer_case(&mut ws, &HEVC.hdr()).await;
    streaming(&mut ws, id).await;
    until("video received", WAIT, || peer.count() >= 3).await;
    web.harness
        .fixture
        .display_unreadable
        .store(true, Ordering::Release);
    let seen = peer.count();
    until("video continues", WAIT, || peer.count() >= seen + 5).await;
    drop(peer);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn an_sdr_stream_does_not_care_about_the_display_mode() -> Result<()> {
    let web = web_with_every_codec().await?;
    web.harness
        .fixture
        .display_in_sdr
        .store(true, Ordering::Release);
    let mut ws = web.device("TV", u32::MAX).await;
    ws.send(web.request("start-session", "hevc", false, 2))
        .await;
    status(&mut ws, "session-status", "starting").await;
    let (id, peer) = answer_case(&mut ws, &HEVC).await;
    streaming(&mut ws, id).await;
    until("video received", WAIT, || peer.count() >= 3).await;
    drop(peer);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_whose_launch_was_hdr_asks_for_hdr_display_mode() -> Result<()> {
    let web = web_with_every_codec().await?;
    let mut ws = web.device("TV", u32::MAX).await;
    ws.send(web.request("start-session", "hevc", true, 2)).await;
    status(&mut ws, "session-status", "starting").await;
    let (id, peer) = answer_case(&mut ws, &HEVC.hdr()).await;
    streaming(&mut ws, id).await;
    let session = web.session();
    assert_eq!(
        session.launch.options.get("hdrMode").map(String::as_str),
        Some("1")
    );
    drop(peer);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unsupported_channel_count_is_still_refused() -> Result<()> {
    let web = Web::new().await?;
    let mut ws = web.device("TV", u32::MAX).await;
    for channels in [1, 4, 7] {
        ws.send(web.request("start-session", "h264", false, channels))
            .await;
        error(&mut ws, "start-session", "unsupported-settings").await;
    }
    assert!(web.harness.h.sessions.lock().unwrap().idle());
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
    until("video of the new session", WAIT, || peer.keyframes(1) >= 1).await;
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
