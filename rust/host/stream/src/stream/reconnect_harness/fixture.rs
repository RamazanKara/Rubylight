//! Loopback fixture doubles for host display, audio, app launch, and WebRTC/Moonlight streams.
use super::super::{
    Media,
    audio::audio_layout,
    audio_sink::{AudioSink, WebRtcAudio},
    configure_video_fec,
    control::{ControlPeer, ControlSocket},
    effective_config, require_hdr_source,
};
use super::WAIT;
use crate::state::{Launch, Session, Shared};
use anyhow::{Context, Result};
use butterpollo_media_policy::{audio::OpusLayout, packet::VideoPacketizer};
use butterpollo_session::session::{Preparation, Role};
use butterpollo_session::state::App;
use rusty_enet::{Host, PeerID};
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Default)]
pub struct Fixture {
    pub displays: Arc<AtomicUsize>,
    pub audios: Arc<AtomicUsize>,
    pub inputs: Arc<AtomicUsize>,
    /// Type-erased `butterpollo_webrtc::input::Recorder` hook installed by WebRTC reconnect tests.
    pub webrtc_input: Mutex<Option<Arc<dyn std::any::Any + Send + Sync>>>,
    pub session_disconnected: AtomicBool,
    pub disconnect_during_preparation: AtomicBool,
    pub session_checks: AtomicUsize,
    /// The app fails to start after the display was prepared, as a bad app
    /// entry or config does.
    pub fail_app_preparation: AtomicBool,
    pub apps_prepared: AtomicUsize,
    pub resources_retained: AtomicUsize,
    pub input_releases: AtomicUsize,
    /// What the control worker did with the clients' text input requests (0x553B), in place
    /// of starting the desktop's focus watcher.
    pub text_watches: Mutex<Vec<butterpollo_input_policy::text_input::Watch>>,
    pub video_after_input_release: AtomicUsize,
    /// The stream's encoder fails for good, as one that keeps failing past its recovery.
    pub fail_encoder: AtomicBool,
    /// The stream thread panics, as a bug in the encoder or capture path would.
    pub session_panics: AtomicBool,
    /// The panic comes with `h.sessions` locked, which poisons it.
    pub panic_holding_sessions: AtomicBool,
    pub hold_teardown: AtomicBool,
    pub in_teardown: AtomicBool,
    pub reset: Mutex<Option<String>>,
    pub reset_done: AtomicBool,
    pub fail_on_control_connect: AtomicBool,
    pub hold_rtsp_cleanup: AtomicBool,
    pub rtsp_cleanup_waiting: AtomicBool,
    pub resume_rtsp_cleanup: AtomicBool,
    /// A WebRTC worker that fails before it takes its media link.
    pub webrtc_fails_before_its_link: AtomicBool,
    /// The fixture's display is not in HDR. Its frames stay FP16 either way, as a Desktop
    /// Duplication or WGC pool does for an SDR output: only the colour space says.
    pub display_in_sdr: AtomicBool,
    /// Windows does not report the fixture display's colour space.
    pub display_unreadable: AtomicBool,
    /// The WebRTC worker waits before its first frame's source check, as a capture that is slow
    /// to deliver its first frame does.
    pub hold_first_frame: AtomicBool,
    /// How long releasing a launch's fake audio takes, as a driver that blocks does.
    pub audio_release_ms: Arc<AtomicU64>,
    /// The control worker does not run between these two times, as on a loaded machine.
    pub control_held: Mutex<Option<(Instant, Instant)>>,
    /// The next launch's app start waits for `resume_app_preparation`, as a
    /// slow prep command does; `app_preparation_waiting` says it is waiting.
    pub hold_app_preparation: AtomicBool,
    pub app_preparation_waiting: AtomicBool,
    pub resume_app_preparation: AtomicBool,
    /// Runs once while the next launch prepares its display, before its app.
    pub during_display_preparation: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    /// The RTSP handshake deadline in ms, in place of the host's 10 s (0 keeps it).
    pub rtsp_deadline_ms: AtomicU64,
    /// DESCRIBE and ANNOUNCE don't find the folder FFmpeg loads from, as when the host's own
    /// path can't be read.
    pub ffmpeg_directory_unknown: AtomicBool,
    /// DESCRIBE and ANNOUNCE's FFmpeg probe panics.
    pub ffmpeg_probe_panics: AtomicBool,
    /// Ordered/unordered active display names exposed to monitor-switch integration tests.
    pub monitor_displays: Mutex<Vec<String>>,
    /// Monitor names selected via live mid-stream monitor switching in fixture sessions.
    pub monitor_switches: Mutex<Vec<String>>,
    /// Captures non-swallowed input batches without instantiating a host desktop injector.
    pub record_inputs: AtomicBool,
    /// Inputs that passed host filtering during fixture sessions (`record_inputs = true`).
    pub forwarded_inputs: Mutex<Vec<butterpollo_input_policy::input::Input>>,
}

pub struct Slot(Arc<AtomicUsize>);
impl Slot {
    fn new(count: &Arc<AtomicUsize>) -> Self {
        count.fetch_add(1, Ordering::AcqRel);
        Self(count.clone())
    }
}
impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

struct Display {
    _slot: Slot,
    pub fixture: Arc<Fixture>,
}
struct Audio {
    _slot: Slot,
    release_ms: Arc<AtomicU64>,
}
impl Drop for Audio {
    fn drop(&mut self) {
        let ms = self.release_ms.load(Ordering::Acquire);
        if ms > 0 {
            thread::sleep(Duration::from_millis(ms));
        }
    }
}
impl butterpollo_session::session::AudioPreparation<butterpollo_win_io::audio_route::Route>
    for Audio
{
    fn route(&self) -> Arc<butterpollo_win_io::audio_route::Route> {
        panic!("the fake audio must never enter native audio preparation")
    }
}
impl Preparation<crate::display_session::StreamPreparation> for Display {
    fn prepared(&self) -> &crate::display_session::StreamPreparation {
        panic!("the fake display must never enter native display preparation")
    }
    fn into_prepared(self: Box<Self>) -> crate::display_session::StreamPreparation {
        panic!("the fake display must never enter native display preparation")
    }
}
impl Drop for Display {
    fn drop(&mut self) {
        if self.fixture.hold_teardown.load(Ordering::Acquire) {
            self.fixture.in_teardown.store(true, Ordering::Release);
            let deadline = Instant::now() + Duration::from_secs(10);
            while self.fixture.hold_teardown.load(Ordering::Acquire) && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(2));
            }
        }
    }
}

impl Fixture {
    pub(crate) fn check_streaming_session(&self) -> Result<()> {
        self.session_checks.fetch_add(1, Ordering::AcqRel);
        anyhow::ensure!(
            !self.session_disconnected.load(Ordering::Acquire),
            "The host Windows session is disconnected or changing. Retry in a moment."
        );
        Ok(())
    }
    pub(crate) fn prepare_app(
        &self,
        h: &Shared,
        app: Option<&App>,
        launch: &Launch,
    ) -> Result<Option<crate::process::RunningApp>> {
        anyhow::ensure!(
            !self.fail_app_preparation.load(Ordering::Acquire),
            "fixture application failed to start"
        );
        if self.hold_app_preparation.swap(false, Ordering::AcqRel) {
            // Launches prepare on a blocking thread, so waiting here holds
            // only this launch, still pending and preparing.
            self.app_preparation_waiting.store(true, Ordering::Release);
            let deadline = Instant::now() + WAIT;
            while !self.resume_app_preparation.load(Ordering::Acquire) {
                assert!(Instant::now() < deadline, "app preparation was not resumed");
                thread::sleep(Duration::from_millis(1));
            }
            self.app_preparation_waiting.store(false, Ordering::Release);
        }
        *launch.audio_preparation.lock().unwrap() = Some(Box::new(Audio {
            _slot: Slot::new(&self.audios),
            release_ms: self.audio_release_ms.clone(),
        }));
        if launch.role == Role::Stream && h.current_app.lock().unwrap().is_none() {
            self.apps_prepared.fetch_add(1, Ordering::AcqRel);
            return Ok(Some(crate::process::RunningApp::with_environment(
                app.context("fixture application missing")?,
                // As `process::launch` records the launching client, owner of the game.
                [("SUNSHINE_CLIENT_UUID".into(), launch.client.uuid.clone())].into(),
            )?));
        }
        Ok(None)
    }
    pub(crate) fn rtsp_deadline(&self) -> Option<Duration> {
        let ms = self.rtsp_deadline_ms.load(Ordering::Acquire);
        (ms != 0).then(|| Duration::from_millis(ms))
    }
    pub(crate) fn ffmpeg_directory_unknown(&self) -> bool {
        self.ffmpeg_directory_unknown.load(Ordering::Acquire)
    }
    pub(crate) fn ffmpeg_probe_panics(&self) -> bool {
        self.ffmpeg_probe_panics.load(Ordering::Acquire)
    }
    pub(crate) fn retain_resources(&self) {
        self.resources_retained.fetch_add(1, Ordering::AcqRel);
    }
    pub(crate) async fn before_rtsp_cleanup(&self) {
        if !self.hold_rtsp_cleanup.swap(false, Ordering::AcqRel) {
            return;
        }
        self.rtsp_cleanup_waiting.store(true, Ordering::Release);
        let deadline = Instant::now() + WAIT;
        while !self.resume_rtsp_cleanup.load(Ordering::Acquire) {
            assert!(Instant::now() < deadline, "RTSP cleanup was not resumed");
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        self.rtsp_cleanup_waiting.store(false, Ordering::Release);
    }
    pub(crate) fn control_connected(&self, h: &Shared, id: &str) {
        if !self.fail_on_control_connect.swap(false, Ordering::AcqRel) {
            return;
        }
        let session = h.sessions.lock().unwrap().active[id].clone();
        session.fail("the fixture failed on control connect");
        // Model the session worker finishing between the ENet Connect event
        // and the control worker's first sweep. This can happen when capture
        // or encoder startup fails before the first input packet arrives.
        let deadline = Instant::now() + WAIT;
        while h.sessions.lock().unwrap().active.contains_key(id) {
            assert!(
                Instant::now() < deadline,
                "failed session did not tear down"
            );
            thread::sleep(Duration::from_millis(1));
        }
    }
    pub(crate) fn prepare(self: &Arc<Self>, launch: &Launch) {
        // The normal pending-launch slot and session worker own this lease.
        // Counting it verifies custody and teardown without creating a monitor.
        *launch.preparation.lock().unwrap() = Some(Box::new(Display {
            _slot: Slot::new(&self.displays),
            fixture: self.clone(),
        }));
        if self.disconnect_during_preparation.load(Ordering::Acquire) {
            self.session_disconnected.store(true, Ordering::Release);
        }
        let during = self.during_display_preparation.lock().unwrap().take();
        if let Some(during) = during {
            during();
        }
    }
    pub(crate) fn input(&self) -> Slot {
        Slot::new(&self.inputs)
    }
    /// Holds the control worker once it is due, before its next pass.
    pub(crate) fn hold_control(&self) {
        let mut held = self.control_held.lock().unwrap();
        if let Some((from, until)) = *held
            && Instant::now() >= from
        {
            held.take();
            drop(held);
            thread::sleep(until.saturating_duration_since(Instant::now()));
        }
    }
    pub(in crate::stream) fn reset_peer(
        &self,
        host: &mut Host<ControlSocket>,
        peers: &HashMap<PeerID, ControlPeer>,
    ) {
        let mut reset = self.reset.lock().unwrap();
        if let Some(id) = reset.as_ref()
            && let Some((peer, _)) = peers.iter().find(|(_, p)| &p.id == id)
        {
            // Reset ENet itself without fabricating a Disconnect event or
            // deleting the host's bookkeeping: cleanup is the worker's job.
            host.peer_mut(*peer).reset();
            reset.take();
            self.reset_done.store(true, Ordering::Release);
        }
    }
    /// The capture's reading of the display's colour space: HDR10, not, or unreadable.
    fn display_in_hdr(&self) -> Option<bool> {
        (!self.display_unreadable.load(Ordering::Acquire))
            .then(|| !self.display_in_sdr.load(Ordering::Acquire))
    }
    /// A WebRTC session's stream worker: the production `Sender` and audio sink, with synthetic
    /// access units and Opus packets. The keyframe flag is honoured as the encoder does, and the
    /// display check is the production one.
    fn stream_webrtc(&self, h: &Shared, s: &Session) -> Result<()> {
        let _audio = s.launch.audio_preparation.lock().unwrap().take();
        anyhow::ensure!(
            !self.webrtc_fails_before_its_link.load(Ordering::Acquire),
            "the fixture's WebRTC worker failed before taking its link"
        );
        while self.hold_first_frame.load(Ordering::Acquire)
            && !h.stop.load(Ordering::Acquire)
            && !s.stopping()
        {
            thread::sleep(Duration::from_millis(5));
        }
        require_hdr_source(s, || self.display_in_hdr(), false)?;
        s.source_checked.store(true, Ordering::Release);
        // As `spawn_audio`: the audio end first, then the video worker takes the link.
        let mut audio = WebRtcAudio(
            h.webrtc_links
                .audio_sender(&s.launch.id)
                .context("the WebRTC audio link is missing")?,
        );
        // The layout the production audio thread would open its encoder with.
        let layout = audio_layout(s, Some(audio.0.layout()))?;
        let sender = h
            .webrtc_links
            .take(&s.launch.id)
            .context("the WebRTC link is missing")?
            .into_video_sink();
        *s.encoder.write().unwrap() = "reconnect fixture".into();
        let mut claims = butterpollo_media_policy::performance::Claims::default();
        let codec = s.config.codec;
        let mut first = true;
        let mut position = 0u64;
        let result = (|| -> Result<()> {
            while !h.stop.load(Ordering::Acquire) && !s.stopping() {
                require_hdr_source(s, || self.display_in_hdr(), true)?;
                // Two 5 ms packets per tick; the payload names the layout the encoder would use.
                let payload = audio_payload(&layout);
                for _ in 0..2 {
                    AudioSink::send(&mut audio, &payload, Instant::now(), position)?;
                    position += 240;
                }
                let idr = s.idr.swap(false, Ordering::AcqRel) || first;
                first = false;
                let frame = access_unit(codec, idr);
                let started = Instant::now();
                sender.submit(
                    s,
                    vec![butterpollo_win_graphics::encoder::Encoded {
                        framing: None,
                        bytes: frame,
                        unit: butterpollo_win_graphics::encoder::Unit::Whole,
                        idr,
                        after_invalidation: false,
                        frame_cap_bytes: None,
                        reencoded: false,
                        frame_cap_target: None,
                        latency: Some(Duration::from_millis(2)),
                        presentation: Some(started),
                    }],
                    Duration::from_millis(2),
                    started,
                    &mut claims,
                )?;
                thread::sleep(Duration::from_millis(10));
            }
            Ok(())
        })();
        // The production worker marks the session failed while it still holds its sender, so the
        // client hears an error and not the idle that dropping the link would cause.
        if let Err(e) = &result {
            s.fail_with(e);
        }
        drop(sender);
        result
    }
    pub(crate) fn stream(&self, media: &Media, h: &Shared, s: &Session) -> Result<()> {
        if s.launch.transport == butterpollo_session::session::Transport::WebRtc {
            return self.stream_webrtc(h, s);
        }
        let _audio = s.launch.audio_preparation.lock().unwrap().take();
        let mut packetizer = VideoPacketizer {
            sequence: 0,
            iv_counter: 0,
            frame: 1,
            packet_size: 1024,
            fec_percent: 0,
            min_fec: 0,
            key: None,
        };
        *s.encoder.write().unwrap() = "reconnect fixture".into();
        // The video FEC as the video worker starts it; the fixture's own frames
        // keep sending without parity.
        configure_video_fec(s, &effective_config(h, &s.launch)?)?;
        // As the encoder does once it has the display's metadata: the control
        // worker then sends the HDR mode message (0x010e).
        s.set_display_hdr_metadata(butterpollo_media_policy::hdr::Metadata::default());
        // The video clock starts with the stream, as in the video worker: clock sync
        // probes are answered from here on.
        *s.media_epoch.write().unwrap() = Some(Instant::now());
        if let Some(initial) = butterpollo_media_policy::monitor_switch::select_display(
            &self.monitor_displays.lock().unwrap(),
            1,
        ) {
            let mut out = s.output.write().unwrap();
            if out.is_empty() {
                *out = initial;
            }
        }
        while !h.stop.load(Ordering::Acquire) && !s.stopping() {
            if self.session_panics.load(Ordering::Acquire) {
                let _sessions = self
                    .panic_holding_sessions
                    .load(Ordering::Acquire)
                    .then(|| h.sessions.lock().unwrap());
                panic!("the fixture's stream thread panicked");
            }
            if self.fail_encoder.load(Ordering::Acquire) {
                return Err(anyhow::anyhow!("AMF error AMF_FAIL (1)").context(
                    butterpollo_session::session::EncoderFailure("the encoder kept failing"),
                ));
            }
            let peer = media.peers.lock().unwrap().get(&s.launch.id, false);
            if let Some(peer) = peer {
                let released = self.input_releases.load(Ordering::Acquire);
                // A synthetic encoded access unit still uses the production
                // packetizer and video socket. No decoder or GPU is involved. It opens with
                // an SPS and PPS like an encoder's IDR: Moonlight's depacketizer, and
                // Nightjar's (interop_tests), drop a key frame without them. Frame Warp tags
                // it as the video senders tag a frame they are about to send.
                s.frame_warp.tag(packetizer.frame, None);
                for packet in packetizer.encode_recovery(SYNTHETIC_IDR, true, false, 0, 0)? {
                    media.video.send_to(&packet, peer)?;
                    s.stats.packets.fetch_add(1, Ordering::Relaxed);
                }
                s.stats.frames.fetch_add(1, Ordering::Relaxed);
                // Record progress after the real control worker's loss
                // checkpoint, while the session is still live. The test can
                // inspect it even if its async task wakes after timeout.
                if !s.stopping() {
                    self.video_after_input_release
                        .store(released, Ordering::Release);
                }
            }
            thread::sleep(Duration::from_millis(10));
        }
        Ok(())
    }
}

/// The fixture's H.264 key frame: SPS, PPS and an IDR slice, each a stub.
const SYNTHETIC_IDR: &[u8] = &[
    0, 0, 0, 1, 0x67, 0x42, 0, 0, 0, 1, 0x68, 0xce, 0, 0, 0, 1, 0x65, 0x80,
];

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
pub fn access_unit(codec: u8, idr: bool) -> Vec<u8> {
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

/// The audio packet the fixture's encoder stands in with: it names the layout it was built with,
/// so the receiver can tell which one the session encoded. The fixture's worker sends two per tick.
pub fn audio_payload(layout: &OpusLayout) -> Vec<u8> {
    let mut payload = vec![
        layout.channels as u8,
        layout.streams as u8,
        layout.coupled as u8,
    ];
    payload.extend_from_slice(&layout.mapping);
    payload.extend_from_slice(&((layout.bitrate / 1000) as u16).to_be_bytes());
    payload
}
