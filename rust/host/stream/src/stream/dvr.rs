//! Stream DVR for one stream: arming its tap, the `dvr-mux` worker that keeps the replay buffer
//! and writes clips and recordings, and what the control loop, the native API and the console
//! ask of it (`docs/configuration.md#stream-dvr-and-clips`).
//!
//! With `dvr_enabled` off nothing here runs: the session's tap stays unarmed (one relaxed load
//! per picture and packet), no worker starts and no keyframe is added. A stream is armed when
//! it starts, or when a device asks after the setting was turned on, if it is the owner's plain
//! stream with H.264, HEVC or AV1 and Opus sound; a PyroWave stream is `unsupported`
//! (`pyrowave`), passthrough sound `audio_codec_unsupported`, a guest's stream may not save.
use crate::state::{Session, Shared as HostShared};
use butterpollo_config::config::Config;
use butterpollo_media_policy::audio_passthrough::Codec;
use butterpollo_session::session::Transport;
use rubylight_native_api::dvr::{ClipSummary, DvrReason, DvrState, DvrStatus};
use rubylight_protocol::stream_dvr::{Reason, State};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, sync_channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

mod control;
mod save;
mod worker;
pub(crate) use control::{handle_control, send_outgoing};

/// How long a native API or console request waits for a clip to be written.
pub const API_WAIT: Duration = Duration::from_secs(120);
/// How long the end of a stream waits for a recording or clip to be finished.
pub const END_WAIT: Duration = Duration::from_secs(3);

/// A request for the worker.
pub(crate) enum Command {
    Query { request_id: u32 },
    SaveClip { secs: u16, reply: Reply },
    StartRecord { reply: Reply },
    StopRecord { reply: Reply },
}

/// Where the worker answers.
pub(crate) enum Reply {
    /// A 0x554A frame with this request id to the stream's device.
    Control(u32),
    /// The native API or the console, waiting.
    Api(SyncSender<Answer>),
}

/// The worker's answer to an API request.
#[derive(Debug)]
pub(crate) enum Answer {
    Saved(ClipSummary),
    Started,
    Refused(Reason),
    Failed(Reason),
}

/// What the worker shows the others.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Live {
    pub state: State,
    pub reason: Reason,
    pub buffered_secs: u16,
    pub recording_since: Option<Instant>,
    pub quota_used_bytes: u64,
}

/// One stream's DVR, shared by its worker, the host's [`crate::dvr_store::DvrStore`], the
/// control loop and the APIs.
pub struct Shared {
    pub(crate) session_id: String,
    pub(crate) app_title: String,
    pub(crate) capacity_secs: u16,
    pub(crate) clips_dir: PathBuf,
    pub(crate) quota_limit: u64,
    commands: Mutex<VecDeque<Command>>,
    commands_pending: AtomicBool,
    stop: AtomicBool,
    /// The device sent a 0x554A request, so it reads the frames the worker pushes.
    pub(crate) client_speaks: AtomicBool,
    live: Mutex<Live>,
    worker: Mutex<Option<std::thread::JoinHandle<()>>>,
    pub(crate) recording_id: Mutex<Option<String>>,
    pub(crate) tap_drops: AtomicU64,
}

impl Shared {
    pub(crate) fn push(&self, command: Command) {
        self.commands
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push_back(command);
        self.commands_pending.store(true, Ordering::Release);
    }

    pub(crate) fn take_commands(&self) -> Vec<Command> {
        if !self.commands_pending.swap(false, Ordering::AcqRel) {
            return Vec::new();
        }
        self.commands
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .drain(..)
            .collect()
    }

    pub(crate) fn live(&self) -> Live {
        *self.live.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn set_live(&self, update: impl FnOnce(&mut Live)) -> Live {
        let mut live = self.live.lock().unwrap_or_else(PoisonError::into_inner);
        update(&mut live);
        *live
    }

    pub(crate) fn stopping(&self) -> bool {
        self.stop.load(Ordering::Acquire)
    }
}

/// Why `s` cannot have a replay buffer, if it cannot.
pub(crate) fn eligibility(s: &Session) -> Result<(), Reason> {
    if s.config.codec == 3 {
        return Err(Reason::Pyrowave);
    }
    if s.launch.guest().is_some() {
        return Err(Reason::PermissionDenied);
    }
    if !s.launch.plain_stream() {
        return Err(Reason::NotStreaming);
    }
    let opus = s.launch.transport == Transport::WebRtc
        || matches!(
            Codec::from_announce(s.config.audio_codec, s.config.audio_channels),
            Ok(Codec::Opus)
        );
    if !opus {
        return Err(Reason::AudioCodecUnsupported);
    }
    Ok(())
}

/// Arms `s` when the feature is on and the stream can have a replay buffer. Called as the
/// stream starts, before its audio thread, and again when a device asks later.
pub(crate) fn start(h: &HostShared, s: &Arc<Session>, c: &Config) -> Option<Arc<Shared>> {
    if !c.dvr_enabled() {
        return None;
    }
    if let Some(running) = h.dvr.active_for(&s.launch.id) {
        return Some(running);
    }
    if let Err(reason) = eligibility(s) {
        tracing::info!(reason = reason.as_str(), client = %s.launch.client.name, "stream dvr: this stream cannot be clipped");
        return None;
    }
    let clips_dir = crate::dvr_store::clips_dir(&h.config_path, c);
    let capacity_secs = c.dvr_ring_seconds();
    let shared = Arc::new(Shared {
        session_id: s.launch.id.clone(),
        app_title: crate::stream::config::launch_app(h, &s.launch)
            .map_or_else(|| "Desktop".to_owned(), |app| app.name),
        capacity_secs,
        clips_dir,
        quota_limit: crate::dvr_store::quota_bytes(c),
        commands: Mutex::default(),
        commands_pending: AtomicBool::new(false),
        stop: AtomicBool::new(false),
        client_speaks: AtomicBool::new(false),
        live: Mutex::new(Live {
            state: State::Armed,
            reason: Reason::None,
            buffered_secs: 0,
            recording_since: None,
            quota_used_bytes: 0,
        }),
        worker: Mutex::default(),
        recording_id: Mutex::default(),
        tap_drops: AtomicU64::new(0),
    });
    if !h.dvr.register(&s.launch.id, shared.clone()) {
        tracing::info!(client = %s.launch.client.name, "stream dvr: another stream has the replay buffer");
        return None;
    }
    let Some(receiver) = s.dvr.arm() else {
        h.dvr.unregister(&s.launch.id);
        return None;
    };
    let limits = butterpollo_media_policy::dvr::ring::Limits {
        max_us: u64::from(capacity_secs) * 1_000_000,
        max_bytes: c.dvr_max_ram_mb() as usize * 1024 * 1024,
    };
    match spawn(h, s, &shared, receiver, limits) {
        Ok(handle) => {
            *shared.worker.lock().unwrap_or_else(PoisonError::into_inner) = Some(handle);
            tracing::info!(
                capacity_secs,
                max_ram_mb = c.dvr_max_ram_mb(),
                dir = %shared.clips_dir.display(),
                "stream dvr armed"
            );
            Some(shared)
        }
        Err(error) => {
            s.dvr.disarm();
            h.dvr.unregister(&s.launch.id);
            tracing::warn!(%error, "stream dvr: its worker did not start");
            None
        }
    }
}

fn spawn(
    h: &HostShared,
    s: &Arc<Session>,
    shared: &Arc<Shared>,
    receiver: Receiver<butterpollo_session::session::dvr_tap::TapItem>,
    limits: butterpollo_media_policy::dvr::ring::Limits,
) -> std::io::Result<std::thread::JoinHandle<()>> {
    let worker = worker::Worker::new(h.clone(), Arc::downgrade(s), shared.clone(), limits);
    std::thread::Builder::new()
        .name("dvr-mux".into())
        .spawn(move || worker.run(receiver))
}

/// Ends `s`'s DVR at the end of its stream: a recording is finished and a clip being written
/// completes, waiting at most [`END_WAIT`].
pub(crate) fn end(h: &HostShared, s: &Session) {
    s.dvr.disarm();
    let Some(shared) = h.dvr.active_for(&s.launch.id) else {
        return;
    };
    shared.stop.store(true, Ordering::Release);
    let handle = shared
        .worker
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take();
    if let Some(handle) = handle {
        let deadline = Instant::now() + END_WAIT;
        while !handle.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        if handle.is_finished() {
            let _ = handle.join();
        } else {
            tracing::warn!("stream dvr: the worker is still finishing after the stream ended");
        }
    }
    h.dvr.unregister(&s.launch.id);
}

/// Copies a frame's encoded pictures to the DVR worker while armed; one relaxed load when not.
#[inline]
pub(crate) fn tap_video(s: &Session, output: &[butterpollo_win_graphics::encoder::Encoded]) {
    if !s.dvr.is_armed() {
        return;
    }
    tap_video_armed(s, output);
}

#[cold]
fn tap_video_armed(s: &Session, output: &[butterpollo_win_graphics::encoder::Encoded]) {
    use butterpollo_session::session::dvr_tap::{TapItem, TapUnit};
    use butterpollo_win_graphics::encoder::Unit;
    let hdr = s.hdr_active.load(Ordering::Relaxed);
    let now = Instant::now();
    for frame in output {
        s.dvr.send(TapItem::Video {
            bytes: frame.bytes.clone(),
            unit: match frame.unit {
                Unit::Whole => TapUnit::Whole,
                Unit::Slice { index } => TapUnit::Slice(index),
                Unit::Last { index } => TapUnit::Last(index),
            },
            idr: frame.idr,
            at: frame.presentation.unwrap_or(now),
            hdr,
        });
    }
}

/// The host's DVR state for the native API and the console.
pub fn status(h: &HostShared) -> DvrStatus {
    let config = h
        .config
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    let dir = crate::dvr_store::clips_dir(&h.config_path, &config);
    let used = crate::dvr_store::used_bytes(&crate::dvr_store::list(&dir));
    let mut status = DvrStatus {
        ring_capacity_secs: u32::from(config.dvr_ring_seconds()),
        quota_used_bytes: used,
        quota_limit_bytes: crate::dvr_store::quota_bytes(&config),
        clips_dir_available: crate::dvr_store::writable(&dir),
        container: "mp4".into(),
        ..Default::default()
    };
    if let Some(shared) = h.dvr.active() {
        let live = shared.live();
        status.state = api_state(live.state);
        status.reason = api_reason(live.reason);
        status.ring_buffered_secs = u32::from(live.buffered_secs);
        status.ring_capacity_secs = u32::from(shared.capacity_secs);
        status.recording = live.recording_since.is_some();
        status.recording_elapsed_secs = live
            .recording_since
            .map_or(0, |since| since.elapsed().as_secs() as u32);
        return status;
    }
    let (state, reason) = idle(h, &config);
    status.state = api_state(state);
    status.reason = api_reason(reason);
    status
}

/// The state and reason while no stream has a replay buffer.
pub(crate) fn idle(h: &HostShared, config: &Config) -> (State, Reason) {
    if !config.dvr_enabled() {
        return (State::Off, Reason::Disabled);
    }
    match owner_stream(h).map(|s| eligibility(&s)) {
        Some(Err(reason @ (Reason::Pyrowave | Reason::AudioCodecUnsupported))) => {
            (State::Unsupported, reason)
        }
        _ => (State::Off, Reason::NotStreaming),
    }
}

/// The owner's plain stream: the one that has, or would get, the replay buffer.
fn owner_stream(h: &HostShared) -> Option<Arc<Session>> {
    let active = h.dvr.active();
    h.sessions
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .active
        .values()
        .find(|s| match &active {
            Some(shared) => s.launch.id == shared.session_id,
            None => s.launch.guest().is_none() && s.launch.plain_stream(),
        })
        .cloned()
}

/// Whether `device` streams the stream that has, or would get, the replay buffer.
pub fn owned_by(h: &HostShared, device: &str) -> bool {
    owner_stream(h).is_some_and(|s| crate::nvhttp::same_device(&s.launch.client.uuid, device))
}

/// The running replay buffer, arming the owner's stream first when the feature was turned on
/// after it started.
fn running(h: &HostShared) -> Result<Arc<Shared>, Reason> {
    if let Some(shared) = h.dvr.active() {
        return Ok(shared);
    }
    let config = h
        .config
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    if let Some(s) = owner_stream(h)
        && let Some(shared) = start(h, &s, &config)
    {
        return Ok(shared);
    }
    Err(idle(h, &config).1)
}

fn ask(h: &HostShared, command: impl FnOnce(Reply) -> Command) -> Result<Answer, Reason> {
    let shared = running(h)?;
    let (reply, answer) = sync_channel(1);
    shared.push(command(Reply::Api(reply)));
    match answer.recv_timeout(API_WAIT) {
        Ok(answer) => Ok(answer),
        Err(RecvTimeoutError::Timeout) => Err(Reason::Busy),
        Err(RecvTimeoutError::Disconnected) => Err(Reason::NotStreaming),
    }
}

/// Saves the replay buffer's last `secs` (all of it when `None`); blocks until written.
pub fn save_clip(h: &HostShared, secs: Option<u16>) -> Result<ClipSummary, Reason> {
    match ask(h, |reply| Command::SaveClip {
        secs: secs.unwrap_or(0),
        reply,
    })? {
        Answer::Saved(clip) => Ok(clip),
        Answer::Refused(reason) | Answer::Failed(reason) => Err(reason),
        Answer::Started => Err(Reason::Unknown),
    }
}

/// Starts (`true`) or stops a recording; a stop blocks until the file is finished.
pub fn record(h: &HostShared, begin: bool) -> Result<Option<ClipSummary>, Reason> {
    let answer = ask(h, |reply| {
        if begin {
            Command::StartRecord { reply }
        } else {
            Command::StopRecord { reply }
        }
    })?;
    match answer {
        Answer::Started => Ok(None),
        Answer::Saved(clip) => Ok(Some(clip)),
        Answer::Refused(reason) | Answer::Failed(reason) => Err(reason),
    }
}

/// The native API's name of `state`.
pub fn api_state(state: State) -> DvrState {
    match state {
        State::Off => DvrState::Off,
        State::Armed => DvrState::Armed,
        State::Recording => DvrState::Recording,
        State::Saving => DvrState::Saving,
        State::Unsupported => DvrState::Unsupported,
        State::Error => DvrState::Error,
    }
}

/// The native API's name of `reason`.
pub fn api_reason(reason: Reason) -> DvrReason {
    match reason {
        Reason::None => DvrReason::None,
        Reason::Disabled => DvrReason::Disabled,
        Reason::Pyrowave => DvrReason::Pyrowave,
        Reason::DiskFull => DvrReason::DiskFull,
        Reason::QuotaExceeded => DvrReason::QuotaExceeded,
        Reason::NoKeyframeYet => DvrReason::NoKeyframeYet,
        Reason::Busy => DvrReason::Busy,
        Reason::PermissionDenied => DvrReason::PermissionDenied,
        Reason::AudioCodecUnsupported => DvrReason::AudioCodecUnsupported,
        Reason::IoError => DvrReason::IoError,
        Reason::NotStreaming => DvrReason::NotStreaming,
        Reason::Unknown => DvrReason::Unknown,
    }
}

/// Plain English for `reason`, as the console and the native API's errors show it.
pub fn reason_text(reason: Reason) -> &'static str {
    match reason {
        Reason::None => "Saved.",
        Reason::Disabled => {
            "Instant replay is turned off on this PC. Turn on Instant replay in Rubylight's settings to save clips."
        }
        Reason::Pyrowave => {
            "Clips cannot be saved while streaming with PyroWave. Switch to H.264, HEVC or AV1 to save clips."
        }
        Reason::DiskFull | Reason::QuotaExceeded => {
            "Not enough free space on the host PC to save this clip. Free up space or delete older clips."
        }
        Reason::NoKeyframeYet => {
            "Waiting for the stream to capture a full frame. Try saving again in a moment."
        }
        Reason::Busy => "Another clip is being saved. Try again in a moment.",
        Reason::PermissionDenied => "Only the device that started this game can save clips.",
        Reason::AudioCodecUnsupported => {
            "Clips need Opus sound. Switch the device's audio from passthrough to Opus to save clips."
        }
        Reason::IoError => {
            "The clips folder could not be written. Check the folder in Rubylight's settings."
        }
        Reason::NotStreaming => "Nothing is streaming right now.",
        Reason::Unknown => "The clip could not be saved.",
    }
}

#[cfg(test)]
mod tests;
