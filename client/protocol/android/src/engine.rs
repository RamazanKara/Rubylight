//! The one stream the app runs at a time, and the per-stream extras around it: phase lock
//! reports, the microphone encoder and the present clock. The JNI entry points in `lib.rs`
//! are thin wrappers over these functions.

use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use nightjar_core::control::ClientMessage;
use nightjar_core::rtsp::MicrophoneState;
use nightjar_core::sdp::{self, StreamConfig};
use nightjar_core::session::{HostInfo, Interrupt, NetworkHint, Session, SessionConfig};
use rubylight_protocol::control::ClockSyncReply;
use rubylight_protocol::present_clock::{Grid, GridSchedule, PresentClock, Schedule, Settings};

use crate::bridge::Bridge;
use crate::extras::{self, PhaseLockReports};
use crate::listener::Events;
use crate::renderers::{Audio, Video};
use crate::{opus, platform};

/// `MIC_STATE_*` of MoonBridge.
pub const MIC_STATE_OFF: i32 = 0;
pub const MIC_STATE_NOT_OFFERED: i32 = 1;
pub const MIC_STATE_UNAVAILABLE: i32 = 2;
pub const MIC_STATE_READY: i32 = 3;

struct Microphone {
    encoder: opus::Encoder,
    last_frame: Option<Instant>,
    sent: u32,
    errors: u32,
}

struct Engine {
    /// Held only for one short call (input, a query, a send). Stop takes the session out
    /// before tearing it down, so callbacks that query it while threads join cannot deadlock.
    /// Each session has its own id, from 1: USB forwarding tells this stream from the last.
    session: Mutex<Option<(u64, Session)>>,
    sessions: AtomicU64,
    interrupt: Mutex<Interrupt>,
    phase_lock: Mutex<PhaseLockReports>,
    phase_lock_lead_us: AtomicI32,
    microphone: Mutex<Option<Microphone>>,
    microphone_state: AtomicI32,
    present_clock: Mutex<Option<PresentClock>>,
    /// The control message types the host announced, set by the handshake before the video
    /// renderer starts. The session is stored only after every stage ran, and the renderer's
    /// `start` already asks what the host supports (to run the clock sync for the timing CSV).
    host_messages: Mutex<Vec<u16>>,
    /// The packed 4:4:4 modes the host's DESCRIBE offered, -1 until the handshake has them.
    /// Set with `host_messages`, so the video setup can read it before the session is stored.
    host_packed444_modes: AtomicI32,
    /// The HDR format this stream's ANNOUNCE asked for, 0 until the handshake has it.
    announced_hdr_format: AtomicI32,
}

static ENGINE: LazyLock<Engine> = LazyLock::new(|| Engine {
    session: Mutex::new(None),
    sessions: AtomicU64::new(0),
    interrupt: Mutex::new(Interrupt::default()),
    phase_lock: Mutex::new(PhaseLockReports::default()),
    phase_lock_lead_us: AtomicI32::new(i32::MIN),
    microphone: Mutex::new(None),
    microphone_state: AtomicI32::new(MIC_STATE_OFF),
    present_clock: Mutex::new(None),
    host_messages: Mutex::new(Vec::new()),
    host_packed444_modes: AtomicI32::new(-1),
    announced_hdr_format: AtomicI32::new(0),
});

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|p| p.into_inner())
}

fn now_ms() -> u64 {
    platform::monotonic_us() / 1000
}

/// Runs `f` on the running session, or returns `default` without one.
pub fn with_session<R>(default: R, f: impl FnOnce(&Session) -> R) -> R {
    let guard = lock(&ENGINE.session);
    match guard.as_ref() {
        Some((_, session)) => f(session),
        None => default,
    }
}

/// The running session's id, unique in this process, or None without one.
pub fn session_id() -> Option<u64> {
    lock(&ENGINE.session).as_ref().map(|(id, _)| *id)
}

/// Sends a control message; false when it was not sent (no stream, not announced, invalid).
pub fn send(message: ClientMessage) -> bool {
    with_session(false, |s| s.send(message).is_ok())
}

/// The default network changed (`RustBridge.networkChanged`): whether a roam started.
/// False without a stream.
pub fn network_changed(metered: bool) -> bool {
    with_session(false, |s| s.network_changed(NetworkHint { metered }))
}

/// What `MoonBridge.startConnection` passes, minus what only the C engine uses.
pub struct StartParams {
    pub host: HostInfo,
    pub config: SessionConfig,
}

/// Builds the stream as `callbacks.c` fills STREAM_CONFIGURATION.
#[allow(clippy::too_many_arguments)]
pub fn stream_config(
    width: i32,
    height: i32,
    fps: i32,
    bitrate_kbps: i32,
    packet_size: i32,
    audio_configuration: i32,
    supported_video_formats: i32,
    client_refresh_rate_x100: i32,
    key: [u8; 16],
    iv: [u8; 16],
    video_capabilities: i32,
    color_space: i32,
    color_range: i32,
) -> StreamConfig {
    StreamConfig {
        width,
        height,
        fps,
        bitrate_kbps,
        packet_size,
        audio: extras::audio_configuration_from_int(audio_configuration)
            .unwrap_or(sdp::AudioConfiguration::STEREO),
        supported_video_formats: supported_video_formats as u32,
        client_refresh_rate_x100,
        color_space,
        color_range,
        encryption_flags: if extras::has_fast_aes() {
            sdp::ENCFLG_ALL
        } else {
            sdp::ENCFLG_AUDIO
        },
        remote_input_aes_key: key,
        remote_input_aes_iv: iv,
        video_capabilities: video_capabilities as u32,
        audio_capabilities: sdp::CAPABILITY_SUPPORTS_ARBITRARY_AUDIO_DURATION,
        ..Default::default()
    }
}

/// `LiStartConnection`: blocks until the stream runs (0) or a stage failed (its code).
pub fn start(bridge: &'static Bridge, params: StartParams) -> i32 {
    // A stream still stored here was never stopped; stop it first like a new C start would
    // find a stopped core.
    stop();
    *lock(&ENGINE.phase_lock) = PhaseLockReports::default();
    ENGINE.phase_lock_lead_us.store(i32::MIN, Ordering::SeqCst);
    *lock(&ENGINE.microphone) = None;
    ENGINE
        .microphone_state
        .store(MIC_STATE_OFF, Ordering::SeqCst);
    let interrupt = Interrupt::default();
    *lock(&ENGINE.interrupt) = interrupt.clone();
    set_host_messages(&[]);
    ENGINE.host_packed444_modes.store(-1, Ordering::SeqCst);
    ENGINE.announced_hdr_format.store(0, Ordering::SeqCst);

    let microphone_requested = params.config.stream.microphone;
    let audio_passthrough = params.config.stream.audio_passthrough;
    let events = Arc::new(Events::new(bridge));
    platform::info(&format!(
        "Starting a stream with the Rust engine: {}x{}@{} formats=0x{:x} input_only={}",
        params.config.stream.width,
        params.config.stream.height,
        params.config.stream.fps,
        params.config.stream.supported_video_formats,
        params.config.input_only
    ));
    let session = match Session::start(
        &params.host,
        params.config,
        Box::new(Video::new(bridge)),
        Box::new(Audio::new(bridge, audio_passthrough)),
        events.clone(),
        &interrupt,
    ) {
        Ok(session) => session,
        Err(error) => {
            platform::error(&format!("Stream start failed: {error}"));
            set_host_messages(&[]);
            bridge.with_env(|env| {
                let message = env.new_string(format!("Stream start failed: {error}"))?;
                bridge.void(
                    env,
                    bridge.m.display_message,
                    &[crate::bridge::object(message.as_ref())],
                );
                Ok(())
            });
            return if error.code != 0 { error.code } else { -1 };
        }
    };

    let outcome = session.outcome();
    platform::info(&format!(
        "Stream negotiated: format=0x{:x} {}x{} packet={} encryption=0x{:x} control messages {:x?}",
        outcome.negotiated_video_format,
        outcome.config.width,
        outcome.config.height,
        outcome.config.packet_size,
        outcome.encryption_enabled,
        outcome.host_control_messages
    ));
    let microphone_state = match outcome.microphone {
        _ if !microphone_requested => MIC_STATE_OFF,
        MicrophoneState::Off => MIC_STATE_OFF,
        MicrophoneState::NotOffered => MIC_STATE_NOT_OFFERED,
        MicrophoneState::Unavailable => MIC_STATE_UNAVAILABLE,
        MicrophoneState::On if session.microphone_available() => {
            match opus::Encoder::new_microphone() {
                Ok(encoder) => {
                    *lock(&ENGINE.microphone) = Some(Microphone {
                        encoder,
                        last_frame: None,
                        sent: 0,
                        errors: 0,
                    });
                    platform::info("Microphone: ready (encrypted)");
                    MIC_STATE_READY
                }
                Err(code) => {
                    platform::error(&format!("Microphone: Opus encoder creation failed: {code}"));
                    MIC_STATE_UNAVAILABLE
                }
            }
        }
        MicrophoneState::On => MIC_STATE_UNAVAILABLE,
    };
    ENGINE
        .microphone_state
        .store(microphone_state, Ordering::SeqCst);
    let id = ENGINE.sessions.fetch_add(1, Ordering::SeqCst) + 1;
    *lock(&ENGINE.session) = Some((id, session));
    if events.started.load(Ordering::SeqCst) {
        bridge.post(bridge.m.connection_started, &[]);
    }
    0
}

/// `LiStopConnection`.
pub fn stop() {
    let session = lock(&ENGINE.session).take();
    set_host_messages(&[]);
    ENGINE.host_packed444_modes.store(-1, Ordering::SeqCst);
    ENGINE.announced_hdr_format.store(0, Ordering::SeqCst);
    if let Some(microphone) = lock(&ENGINE.microphone).take() {
        platform::info(&format!(
            "Microphone: stopped after {} packets sent ({} errors)",
            microphone.sent, microphone.errors
        ));
    }
    ENGINE
        .microphone_state
        .store(MIC_STATE_OFF, Ordering::SeqCst);
    // Detaches what is attached here; the host detaches its side as the stream ends.
    if let Some((id, session)) = session {
        crate::usb::end(id);
        session.stop();
        platform::info("Stream stopped");
    }
}

/// `LiInterruptConnection`.
pub fn interrupt() {
    lock(&ENGINE.interrupt).interrupt();
}

/// Records the control messages the host announced (`Listener::host_control_messages`).
pub fn set_host_messages(messages: &[u16]) {
    *lock(&ENGINE.host_messages) = messages.to_vec();
}

/// The listener's side of [`host_packed444_modes`]: like [`announce_host_messages`], nothing is
/// kept once the start was interrupted.
pub fn announce_host_packed444_modes(modes: u8) {
    if lock(&ENGINE.interrupt).is_interrupted() {
        return;
    }
    ENGINE
        .host_packed444_modes
        .store(i32::from(modes), Ordering::SeqCst);
}

/// `MoonBridge.getHostPacked444Modes`: what this stream's DESCRIBE offered, -1 before it.
pub fn host_packed444_modes() -> i32 {
    ENGINE.host_packed444_modes.load(Ordering::SeqCst)
}

/// The listener's side of [`announced_hdr_format`], kept like [`announce_host_packed444_modes`].
pub fn announce_hdr_format(format: u8) {
    if lock(&ENGINE.interrupt).is_interrupted() {
        return;
    }
    ENGINE
        .announced_hdr_format
        .store(i32::from(format), Ordering::SeqCst);
}

/// `MoonBridge.getAnnouncedHdrFormat`: the HDR format this stream's ANNOUNCE asked for, 0 for
/// stock HDR and before the ANNOUNCE.
pub fn announced_hdr_format() -> i32 {
    ENGINE.announced_hdr_format.load(Ordering::SeqCst)
}

/// The listener's side of [`set_host_messages`]: ignores the announcement once the start was
/// interrupted, so a `stop()` during the RTSP handshake cannot leave messages behind for the
/// next stream's renderers.
pub fn announce_host_messages(messages: &[u16]) {
    if lock(&ENGINE.interrupt).is_interrupted() {
        return;
    }
    set_host_messages(messages);
}

/// Whether the host announced `message_type`, from the end of the RTSP handshake until the
/// stream stops, so also while the renderers start.
pub fn host_supports(message_type: u16) -> bool {
    lock(&ENGINE.host_messages).contains(&message_type)
}

pub fn report_frame_slack(slack_ns: i64, period_ns: i64) -> bool {
    if !host_supports(rubylight_protocol::phase_lock::REPORT_MESSAGE_TYPE) {
        return false;
    }
    let mut reports = lock(&ENGINE.phase_lock);
    let ready = reports.push(slack_ns, period_ns, now_ms());
    if ready {
        ENGINE
            .phase_lock_lead_us
            .store(reports.lead_us(), Ordering::SeqCst);
    }
    ready
}

pub fn flush_phase_lock_report() {
    let report = lock(&ENGINE.phase_lock).take(now_ms());
    if let Some(report) = report {
        send(ClientMessage::PhaseLockReport(report));
    }
}

pub fn phase_lock_lead_us() -> i32 {
    ENGINE.phase_lock_lead_us.load(Ordering::SeqCst)
}

pub fn microphone_state() -> i32 {
    ENGINE.microphone_state.load(Ordering::SeqCst)
}

/// Encodes one frame of 48 kHz mono PCM and sends it; the Opus length, or negative.
pub fn send_microphone(pcm: &[i16]) -> i32 {
    if !extras::valid_microphone_frame(pcm.len()) {
        return -1;
    }
    let mut guard = lock(&ENGINE.microphone);
    let Some(mic) = guard.as_mut() else {
        return -1;
    };
    let now = Instant::now();
    if mic
        .last_frame
        .is_some_and(|last| now - last > Duration::from_millis(extras::MIC_RESET_GAP_MS))
    {
        mic.encoder.reset();
    }
    mic.last_frame = Some(now);
    let mut opus = [0u8; nightjar_core::input::microphone::MAX_PACKET_BYTES];
    let length = mic.encoder.encode(pcm, &mut opus);
    if length <= 0 {
        mic.errors += 1;
        return if length < 0 { length } else { -1 };
    }
    match with_session(None, |s| Some(s.send_microphone(&opus[..length as usize]))) {
        Some(Ok(())) => {
            if mic.sent == 0 {
                platform::info(&format!(
                    "Microphone: first packet sent ({length} bytes of Opus)"
                ));
            }
            mic.sent += 1;
            length
        }
        Some(Err(e)) => {
            // The first failure and then every 500th, so a dead route cannot flood the log.
            if mic.errors % 500 == 0 {
                platform::warn(&format!("Microphone: send failed: {e}"));
            }
            mic.errors += 1;
            -e.raw_os_error().unwrap_or(1).abs()
        }
        None => -1,
    }
}

pub fn present_clock_start(min_interval_us: i32) {
    let settings = Settings {
        min_interval_us: i64::from(min_interval_us.max(0)),
        ..Settings::default()
    };
    *lock(&ENGINE.present_clock) = Some(PresentClock::new(settings));
}

pub fn present_clock_stop() {
    *lock(&ENGINE.present_clock) = None;
}

pub fn present_clock_reset() {
    if let Some(clock) = lock(&ENGINE.present_clock).as_mut() {
        clock.reset();
    }
}

pub fn present_clock_schedule(rtp: u32, ready_us: u64) -> Option<Schedule> {
    lock(&ENGINE.present_clock)
        .as_mut()
        .map(|clock| clock.schedule(rtp, ready_us))
}

/// As [`present_clock_schedule`], onto a vsync grid.
pub fn present_clock_schedule_on_grid(rtp: u32, ready_us: u64, grid: Grid) -> Option<GridSchedule> {
    lock(&ENGINE.present_clock)
        .as_mut()
        .map(|clock| clock.schedule_on_grid(rtp, ready_us, grid))
}

/// Notes a frame for the content rate.
pub fn present_clock_observe(rtp: u32) {
    if let Some(clock) = lock(&ENGINE.present_clock).as_mut() {
        clock.observe(rtp);
    }
}

/// The content rate in millihertz, 0 until it held steady for two seconds or without a clock.
pub fn present_clock_content_rate() -> u32 {
    lock(&ENGINE.present_clock)
        .as_ref()
        .and_then(|clock| clock.content_rate_mhz())
        .unwrap_or(0)
}

/// Frames scheduled, frames late, clock sync round trip (-1 without).
pub fn present_clock_stats() -> Option<(u64, u64, i64)> {
    lock(&ENGINE.present_clock).as_ref().map(|clock| {
        (
            clock.frames,
            clock.late_frames,
            clock.offset.rtt_us().unwrap_or(-1),
        )
    })
}

/// The offset from the host's clock to CLOCK_MONOTONIC (`client = host + offset`, us) at
/// `now_us` and the round trip (us) of its exchange; `None` before the first exchange.
pub fn present_clock_offset(now_us: u64) -> Option<(i64, i64)> {
    let guard = lock(&ENGINE.present_clock);
    let clock = guard.as_ref()?;
    Some((
        clock.offset.offset_at(now_us as i64)?,
        clock.offset.rtt_us()?,
    ))
}

/// A clock sync reply from the control thread (`BridgeClClockSync`).
pub fn clock_sync(reply: &ClockSyncReply, received_us: u64) {
    if let Some(clock) = lock(&ENGINE.present_clock).as_mut() {
        clock.offset.add(reply, received_us);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tests below share the engine's global state.
    static ENGINE_TESTS: Mutex<()> = Mutex::new(());

    #[test]
    fn an_interrupted_start_leaves_no_host_messages_behind() {
        let _serial = lock(&ENGINE_TESTS);
        const CLOCK_SYNC: u16 = 0x5537;
        *lock(&ENGINE.interrupt) = Interrupt::default();
        set_host_messages(&[]);
        announce_host_messages(&[CLOCK_SYNC]);
        assert!(host_supports(CLOCK_SYNC), "a live start records the announcement");
        set_host_messages(&[]);

        // `LiInterruptConnection` and `LiStopConnection` run while the handshake is still going;
        // the announcement that arrives afterwards is dropped.
        interrupt();
        stop();
        announce_host_messages(&[CLOCK_SYNC]);
        assert!(!host_supports(CLOCK_SYNC), "an interrupted start must not record messages");

        *lock(&ENGINE.interrupt) = Interrupt::default();
    }

    #[test]
    fn the_host_packed_modes_last_from_the_handshake_until_stop() {
        let _serial = lock(&ENGINE_TESTS);
        *lock(&ENGINE.interrupt) = Interrupt::default();
        stop();
        assert_eq!(host_packed444_modes(), -1, "unknown before a handshake");
        announce_host_packed444_modes(0b0101);
        assert_eq!(
            host_packed444_modes(),
            0b0101,
            "known before the session is stored"
        );
        stop();
        assert_eq!(
            host_packed444_modes(),
            -1,
            "a stopped stream leaves none behind"
        );

        // A DESCRIBE answered after an interrupt is not kept for the next stream.
        interrupt();
        announce_host_packed444_modes(0b1111);
        assert_eq!(host_packed444_modes(), -1);
        *lock(&ENGINE.interrupt) = Interrupt::default();
    }

    #[test]
    fn the_announced_hdr_format_lasts_from_the_handshake_until_stop() {
        let _serial = lock(&ENGINE_TESTS);
        *lock(&ENGINE.interrupt) = Interrupt::default();
        stop();
        assert_eq!(announced_hdr_format(), 0, "stock before a handshake");
        announce_hdr_format(rubylight_protocol::hdr::DV_81);
        assert_eq!(announced_hdr_format(), 4, "known before the decoder's setup");
        stop();
        assert_eq!(announced_hdr_format(), 0, "a stopped stream leaves none behind");

        interrupt();
        announce_hdr_format(rubylight_protocol::hdr::HLG);
        assert_eq!(announced_hdr_format(), 0);
        *lock(&ENGINE.interrupt) = Interrupt::default();
    }

    #[test]
    fn a_network_change_without_a_stream_starts_no_roam() {
        let _serial = lock(&ENGINE_TESTS);
        stop();
        // The network callback can fire after the stream ended; it must not panic or roam.
        assert!(!network_changed(false));
        assert!(!network_changed(true));
    }

    #[test]
    fn the_host_messages_are_known_before_a_session_is_stored() {
        let _serial = lock(&ENGINE_TESTS);
        // The video renderer starts inside Session::start, before the engine holds the session;
        // its start asks whether the host announced clock sync to decide on the probes.
        const CLOCK_SYNC: u16 = 0x5537;
        set_host_messages(&[]);
        assert!(!host_supports(CLOCK_SYNC));
        assert!(!with_session(false, |_| true), "no session stored");
        set_host_messages(&[0x5530, CLOCK_SYNC]);
        assert!(host_supports(CLOCK_SYNC));
        assert!(!host_supports(0x5531));
        set_host_messages(&[]);
        assert!(!host_supports(CLOCK_SYNC));
    }

    #[test]
    fn present_clock_offset_waits_for_a_clock_sync_exchange() {
        let _serial = lock(&ENGINE_TESTS);
        assert_eq!(present_clock_offset(1_000), None, "no clock running");
        present_clock_start(0);
        assert_eq!(present_clock_offset(1_000), None, "no exchange yet");
        // client = host + offset: sent at 1000, answered at 1100, the host at 5000 and 5010.
        clock_sync(
            &ClockSyncReply {
                sequence: 1,
                client_us: 1_000,
                host_receive_us: 5_000,
                host_send_us: 5_010,
            },
            1_100,
        );
        assert_eq!(present_clock_offset(1_050), Some((-3_955, 90)));
        present_clock_stop();
        assert_eq!(present_clock_offset(1_050), None);
    }

    #[test]
    fn the_engine_clock_schedules_on_a_grid_and_follows_the_content_rate() {
        let _serial = lock(&ENGINE_TESTS);
        let grid = Grid { anchor_us: 0, interval_us: 16_667, lead_us: 4_000, slack_us: 0 };
        assert_eq!(present_clock_schedule_on_grid(0, 1_000, grid), None, "no clock running");
        assert_eq!(present_clock_content_rate(), 0);
        present_clock_start(0);
        let s = present_clock_schedule_on_grid(0, 1_000_000, grid).expect("scheduled");
        assert!(s.vsync_us >= 1_000_000 && s.release_us >= 1_000_000);
        for i in 0..400u32 {
            present_clock_observe(i * 1_500); // 60 fps in 90 kHz ticks
        }
        let rate = present_clock_content_rate();
        assert!(rate.abs_diff(60_000) <= 100, "{rate}");
        present_clock_reset();
        assert_eq!(present_clock_content_rate(), 0, "a reset starts the rate over");
        present_clock_stop();
        assert_eq!(present_clock_content_rate(), 0);
    }
}
