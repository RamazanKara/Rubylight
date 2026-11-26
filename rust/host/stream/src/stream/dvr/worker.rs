//! The `dvr-mux` worker of one stream, at below-normal priority: it takes the tapped pictures
//! and Opus packets into the replay buffer (whole GOPs, bounded by `dvr_ring_seconds` and
//! `dvr_max_ram_mb`), asks the encoder for a keyframe every [`cadence::GOP_SECS`] seconds while
//! armed, writes a recording as it goes, starts replay clips on their own thread, answers
//! requests and tells the stream's device its state.
//!
//! It ends with its stream, when `dvr_enabled` is turned off, or when the session is gone, and
//! then finishes a recording and waits for a clip being written.
use super::save::{Recording, RecordingStep, ReplayJob, io_reason};
use super::{Answer, Command, Reply, Shared};
use crate::state::{Session, Shared as HostShared};
use butterpollo_media_policy::dvr::{
    AudioPacket, HdrInfo, OpusConfig, StreamInfo, VideoCodec, VideoFrame,
    cadence::{self, KeyframeCadence},
    quota,
    ring::{Limits, Pushed, Ring},
    slices::{Reassembler, UnitKind},
};
use butterpollo_session::session::dvr_tap::{TapItem, TapUnit};
use rubylight_native_api::dvr::ClipKind;
use rubylight_protocol::stream_dvr::{
    ClipResult, HostState, Message, Outcome, Reason, State, clamp_clip_secs,
};
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, PoisonError, Weak};
use std::time::{Duration, Instant, SystemTime};

/// How long the worker waits for a copy before it looks at requests and timers.
const TICK: Duration = Duration::from_millis(50);
/// How often a device that speaks 0x554A hears the state while nothing changes.
const STATE_EVERY: Duration = Duration::from_secs(5);
/// How often the worker reads `dvr_enabled`.
const CONFIG_EVERY: Duration = Duration::from_secs(1);
/// How long a failed write shows as the error state.
const ERROR_HOLD: Duration = Duration::from_secs(30);
/// The shortest gap between two keyframes the worker asks for.
const KEYFRAME_RETRY: Duration = Duration::from_secs(1);
/// Copies taken at once before requests are looked at again.
const BURST: usize = 64;

pub(super) struct Worker {
    h: HostShared,
    session: Weak<Session>,
    shared: Arc<Shared>,
    origin: Instant,
    info: StreamInfo,
    ring: Ring,
    slices: Reassembler,
    cadence: KeyframeCadence,
    /// The first Opus packet's sample position and time; later packets follow its clock.
    audio_anchor: Option<(u64, u64)>,
    channels: u32,
    drops_seen: u64,
    recording: Option<Recording>,
    /// A recording asked for, which opens at the next keyframe: its id and when it was asked.
    pending_recording: Option<(String, Instant)>,
    job: Option<ReplayJob>,
    error: Option<(Reason, Instant)>,
    sent: Option<(State, Reason)>,
    sent_at: Instant,
    config_at: Instant,
    keyframe_asked: Option<Instant>,
    /// The saved clips' bytes, read after every write.
    quota_on_disk: u64,
}

impl Worker {
    pub(super) fn new(
        h: HostShared,
        session: Weak<Session>,
        shared: Arc<Shared>,
        limits: Limits,
    ) -> Self {
        let s = session.upgrade();
        let codec = s
            .as_ref()
            .and_then(|s| VideoCodec::from_negotiated(s.config.codec))
            .unwrap_or(VideoCodec::H264);
        // Armed after the audio thread opened its encoder: its layout so far.
        let audio = s.and_then(|s| s.dvr.last_opus_layout()).map(
            |(channels, streams, coupled, mapping)| OpusConfig {
                channels,
                streams,
                coupled,
                mapping,
            },
        );
        let channels = audio.as_ref().map_or(2, |a| u32::from(a.channels.max(1)));
        let now = Instant::now();
        Self {
            h,
            session,
            shared,
            // Capture times can be a little older than the worker.
            origin: now.checked_sub(Duration::from_secs(60)).unwrap_or(now),
            info: StreamInfo {
                codec,
                hdr: None,
                audio,
            },
            ring: Ring::new(limits),
            slices: Reassembler::default(),
            cadence: KeyframeCadence::new(cadence::gop(cadence::GOP_SECS)),
            audio_anchor: None,
            channels,
            drops_seen: 0,
            recording: None,
            pending_recording: None,
            job: None,
            error: None,
            sent: None,
            sent_at: now,
            config_at: now,
            keyframe_asked: None,
            quota_on_disk: 0,
        }
    }

    pub(super) fn run(mut self, receiver: Receiver<TapItem>) {
        butterpollo_win_io::background_write::lower_current_thread();
        self.refresh_quota();
        loop {
            match receiver.recv_timeout(TICK) {
                Ok(item) => {
                    self.take(item);
                    for item in receiver.try_iter().take(BURST) {
                        self.take(item);
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            if self.shared.stopping() || self.session.upgrade().is_none_or(|s| s.stopping()) {
                break;
            }
            self.check_drops();
            for command in self.shared.take_commands() {
                self.command(command);
            }
            self.poll_job(false);
            if !self.tick(Instant::now()) {
                break;
            }
        }
        self.finish();
    }

    fn pts(&self, at: Instant) -> u64 {
        at.saturating_duration_since(self.origin).as_micros() as u64
    }

    fn take(&mut self, item: TapItem) {
        match item {
            TapItem::Video {
                bytes,
                unit,
                idr,
                at,
                hdr,
            } => {
                let kind = match unit {
                    TapUnit::Whole => UnitKind::Whole,
                    TapUnit::Slice(index) => UnitKind::Slice { index },
                    TapUnit::Last(index) => UnitKind::Last { index },
                };
                let pts_us = self.pts(at);
                let frame = self.slices.push(kind, idr, pts_us, &bytes);
                if self.slices.take_discontinuity() {
                    self.discontinuity();
                }
                if let Some(frame) = frame {
                    self.video(frame, hdr);
                }
            }
            TapItem::Audio {
                bytes,
                at,
                position,
                interleaved,
            } => {
                let pts_us = self.audio_pts(at, position);
                let packet = AudioPacket {
                    pts_us,
                    samples: interleaved / self.channels.max(1),
                    data: Arc::new(bytes),
                };
                if let Some(recording) = &mut self.recording {
                    recording.audio(packet.clone());
                }
                self.ring.push_audio(packet);
            }
            TapItem::OpusLayout {
                channels,
                streams,
                coupled,
                mapping,
            } => {
                self.channels = u32::from(channels.max(1));
                self.info.audio = Some(OpusConfig {
                    channels,
                    streams,
                    coupled,
                    mapping,
                });
            }
        }
    }

    /// An Opus packet's time: the first packet's capture time, then its sample clock, taken
    /// again when the clock jumps (a new audio device starts over).
    fn audio_pts(&mut self, at: Instant, position: u64) -> u64 {
        let measured = self.pts(at);
        if let Some((anchor_position, anchor_pts)) = self.audio_anchor
            && position >= anchor_position
        {
            let derived = anchor_pts + (position - anchor_position) * 1_000_000 / 48_000;
            if derived.abs_diff(measured) < 200_000 {
                return derived;
            }
        }
        self.audio_anchor = Some((position, measured));
        measured
    }

    fn video(&mut self, frame: VideoFrame, hdr: bool) {
        let now = Instant::now();
        if hdr != self.info.hdr.is_some() {
            self.info.hdr = hdr.then(|| HdrInfo {
                metadata: self.session.upgrade().and_then(|s| {
                    *s.hdr_metadata
                        .read()
                        .unwrap_or_else(PoisonError::into_inner)
                }),
            });
        }
        // A recording that opens here starts with this keyframe.
        let mut opened = false;
        if frame.idr {
            self.cadence.observe_idr(now);
            if let Some((id, asked)) = self.pending_recording.take() {
                match Recording::start(
                    &self.shared.clips_dir,
                    &id,
                    1,
                    &self.shared.app_title,
                    &self.info,
                    frame.clone(),
                    asked,
                ) {
                    Ok(recording) => {
                        *self
                            .shared
                            .recording_id
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner) = Some(recording.id.clone());
                        self.recording = Some(recording);
                        opened = true;
                    }
                    Err(reason) => self.fail(reason),
                }
            }
        }
        if let Some(recording) = self.recording.as_mut().filter(|_| !opened) {
            match recording.video(&self.h.dvr, self.shared.quota_limit, &self.info, &frame) {
                Ok(RecordingStep::Continue) => {}
                Ok(RecordingStep::AwaitKeyframe) => self.ask_keyframe(now),
                Ok(RecordingStep::Restart(first)) => self.next_part(first),
                Err(reason) => {
                    tracing::warn!(reason = reason.as_str(), "stream dvr: recording stopped");
                    self.stop_recording(None);
                    self.fail(reason);
                }
            }
        }
        if let Pushed::AwaitingKeyframe = self.ring.push_video(&self.info, frame) {
            self.ask_keyframe(now);
        }
    }

    /// The stream's format changed mid-recording: this part is finished and the next starts.
    fn next_part(&mut self, first: VideoFrame) {
        let Some(recording) = self.recording.take() else {
            return;
        };
        let (base, part) = recording.next_part();
        let since = recording.since;
        if let Err(reason) = recording.finish() {
            self.fail(reason);
        }
        match Recording::start(
            &self.shared.clips_dir,
            &base,
            part,
            &self.shared.app_title,
            &self.info,
            first,
            since,
        ) {
            Ok(recording) => {
                *self
                    .shared
                    .recording_id
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner) = Some(recording.id.clone());
                self.recording = Some(recording);
            }
            Err(reason) => self.fail(reason),
        }
    }

    fn discontinuity(&mut self) {
        self.ring.discontinuity();
        if let Some(recording) = &mut self.recording {
            recording.discontinuity();
        }
        self.ask_keyframe(Instant::now());
    }

    fn check_drops(&mut self) {
        let Some(s) = self.session.upgrade() else {
            return;
        };
        let drops = s.dvr.drops();
        if drops > self.drops_seen {
            if self.drops_seen == 0 {
                tracing::warn!(
                    "stream dvr: the worker fell behind and dropped copies; clips resume at the next keyframe"
                );
            }
            self.drops_seen = drops;
            self.shared.tap_drops.store(drops, Ordering::Relaxed);
            self.discontinuity();
        }
    }

    fn ask_keyframe(&mut self, now: Instant) {
        if self
            .keyframe_asked
            .is_some_and(|at| now.duration_since(at) < KEYFRAME_RETRY)
        {
            return;
        }
        self.keyframe_asked = Some(now);
        if let Some(s) = self.session.upgrade() {
            s.request_dvr_keyframe();
        }
    }

    fn fail(&mut self, reason: Reason) {
        self.error = Some((reason, Instant::now()));
    }

    fn command(&mut self, command: Command) {
        match command {
            Command::Query { request_id } => {
                self.shared.client_speaks.store(true, Ordering::Release);
                self.post_state(request_id);
            }
            Command::SaveClip { secs, reply } => self.save_clip(secs, reply),
            Command::StartRecord { reply } => {
                let result = self.start_recording();
                match (reply, result) {
                    (Reply::Control(request_id), Ok(())) => self.post_state(request_id),
                    (Reply::Control(request_id), Err(reason)) => {
                        self.post_clip(request_id, Outcome::Refused, reason, None)
                    }
                    (Reply::Api(answer), result) => {
                        let _ = answer
                            .try_send(result.map_or_else(Answer::Refused, |()| Answer::Started));
                    }
                }
            }
            Command::StopRecord { reply } => self.stop_recording(Some(reply)),
        }
    }

    fn save_clip(&mut self, secs: u16, reply: Reply) {
        let result = (|| {
            if self.job.is_some() {
                return Err(Reason::Busy);
            }
            if !self.ring.has_keyframe() {
                self.ask_keyframe(Instant::now());
                return Err(Reason::NoKeyframeYet);
            }
            let secs = clamp_clip_secs(secs, self.shared.capacity_secs);
            let clip = self
                .ring
                .snapshot(u64::from(secs) * 1_000_000)
                .map_err(|_| Reason::NoKeyframeYet)?;
            let bytes: u64 = clip.video.iter().map(|f| f.data.len() as u64).sum::<u64>()
                + clip.audio.iter().map(|p| p.data.len() as u64).sum::<u64>();
            let open: Vec<String> = self.recording.iter().map(|r| r.id.clone()).collect();
            self.h
                .dvr
                .make_room(
                    &self.shared.clips_dir,
                    bytes,
                    bytes,
                    self.shared.quota_limit,
                    &open,
                )
                .map_err(|refusal| match refusal {
                    quota::Refusal::DiskFull => Reason::DiskFull,
                    _ => Reason::QuotaExceeded,
                })?;
            Ok(clip)
        })();
        let clip = match result {
            Ok(clip) => clip,
            Err(reason) => {
                if matches!(reason, Reason::DiskFull | Reason::QuotaExceeded) {
                    self.fail(reason);
                }
                return self.answer(reply, Err((Outcome::Refused, reason)));
            }
        };
        let id = crate::dvr_store::new_id(ClipKind::Replay, SystemTime::now());
        match ReplayJob::spawn(
            self.shared.clips_dir.clone(),
            id,
            self.shared.app_title.clone(),
            clip,
            reply,
        ) {
            Ok(job) => self.job = Some(job),
            Err(error) => {
                tracing::warn!(%error, "stream dvr: the clip writer did not start");
                self.fail(io_reason(&error));
            }
        }
    }

    fn poll_job(&mut self, wait: bool) {
        if !self
            .job
            .as_ref()
            .is_some_and(|job| wait || job.is_finished())
        {
            return;
        }
        let Some(job) = self.job.take() else {
            return;
        };
        let (reply, result) = job.join();
        self.refresh_quota();
        match &result {
            Ok(_) => self.error = None,
            Err(reason) => self.fail(*reason),
        }
        self.answer(reply, result.map_err(|reason| (Outcome::Failed, reason)));
    }

    fn start_recording(&mut self) -> Result<(), Reason> {
        if self.recording.is_some() || self.pending_recording.is_some() {
            return Ok(());
        }
        let open: Vec<String> = Vec::new();
        self.h
            .dvr
            .make_room(&self.shared.clips_dir, 0, 0, self.shared.quota_limit, &open)
            .map_err(|refusal| match refusal {
                quota::Refusal::DiskFull => Reason::DiskFull,
                _ => Reason::QuotaExceeded,
            })?;
        if !crate::dvr_store::writable(&self.shared.clips_dir) {
            return Err(Reason::IoError);
        }
        let id = crate::dvr_store::new_id(ClipKind::Manual, SystemTime::now());
        self.pending_recording = Some((id, Instant::now()));
        self.ask_keyframe(Instant::now());
        Ok(())
    }

    fn stop_recording(&mut self, reply: Option<Reply>) {
        let pending = self.pending_recording.take().is_some();
        let result = match self.recording.take() {
            Some(recording) => recording
                .finish()
                .map_err(|reason| (Outcome::Failed, reason)),
            None => Err((
                Outcome::Refused,
                if pending {
                    Reason::NoKeyframeYet
                } else {
                    Reason::None
                },
            )),
        };
        *self
            .shared
            .recording_id
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = None;
        self.refresh_quota();
        if let Err((Outcome::Failed, reason)) = &result {
            self.fail(*reason);
        }
        if let Some(reply) = reply {
            self.answer(reply, result);
        }
    }

    fn answer(
        &mut self,
        reply: Reply,
        result: Result<rubylight_native_api::dvr::ClipSummary, (Outcome, Reason)>,
    ) {
        match reply {
            Reply::Api(answer) => {
                let _ = answer.try_send(match result {
                    Ok(clip) => Answer::Saved(clip),
                    Err((Outcome::Refused, reason)) => Answer::Refused(reason),
                    Err((_, reason)) => Answer::Failed(reason),
                });
            }
            Reply::Control(request_id) => match result {
                Ok(clip) => self.post_clip(request_id, Outcome::Saved, Reason::None, Some(&clip)),
                Err((outcome, reason)) => self.post_clip(request_id, outcome, reason, None),
            },
        }
    }

    fn state(&self) -> (State, Reason) {
        if let Some((reason, at)) = self.error
            && at.elapsed() < ERROR_HOLD
        {
            return (State::Error, reason);
        }
        if self.job.is_some() {
            (State::Saving, Reason::None)
        } else if self.recording.is_some() || self.pending_recording.is_some() {
            (State::Recording, Reason::None)
        } else {
            (State::Armed, Reason::None)
        }
    }

    fn host_state(&self, request_id: u32) -> HostState {
        let live = self.shared.live();
        HostState {
            request_id,
            state: live.state,
            reason: live.reason,
            ring_buffered_secs: live.buffered_secs,
            ring_capacity_secs: self.shared.capacity_secs,
            recording_elapsed_secs: live
                .recording_since
                .map_or(0, |since| since.elapsed().as_secs() as u32),
            quota_used_bytes: live.quota_used_bytes,
            quota_limit_bytes: self.shared.quota_limit,
        }
    }

    fn post(&self, message: Message) {
        if let (Some(s), Some(frame)) = (self.session.upgrade(), message.encode()) {
            s.dvr.post(frame);
        }
    }

    fn post_state(&mut self, request_id: u32) {
        self.update_live();
        self.post(Message::State(self.host_state(request_id)));
        self.sent = Some(self.state());
        self.sent_at = Instant::now();
    }

    fn post_clip(
        &mut self,
        request_id: u32,
        outcome: Outcome,
        reason: Reason,
        clip: Option<&rubylight_native_api::dvr::ClipSummary>,
    ) {
        self.post(Message::Clip(ClipResult {
            request_id,
            outcome,
            reason,
            duration_secs: clip.map_or(0, |c| {
                (c.duration_ms / 1000).min(u64::from(u16::MAX)) as u16
            }),
            file_bytes: clip.map_or(0, |c| c.size_bytes),
            clip_id: clip.map(|c| c.id.clone()).unwrap_or_default(),
        }));
    }

    fn refresh_quota(&mut self) {
        self.quota_on_disk =
            crate::dvr_store::used_bytes(&crate::dvr_store::list(&self.shared.clips_dir));
    }

    fn update_live(&mut self) {
        let (state, reason) = self.state();
        let quota_used_bytes =
            self.quota_on_disk + self.recording.as_ref().map_or(0, Recording::written);
        let buffered_secs = (self.ring.buffered_us() / 1_000_000).min(u64::from(u16::MAX)) as u16;
        let recording_since = self
            .recording
            .as_ref()
            .map(|r| r.since)
            .or(self.pending_recording.as_ref().map(|(_, at)| *at));
        self.shared.set_live(|live| {
            live.state = state;
            live.reason = reason;
            live.buffered_secs = buffered_secs;
            live.recording_since = recording_since;
            live.quota_used_bytes = quota_used_bytes;
        });
    }

    /// Timers: the keyframe cadence, the state for the device, the setting. `false` ends the
    /// worker.
    fn tick(&mut self, now: Instant) -> bool {
        if self.cadence.due(now) {
            self.ask_keyframe(now);
        }
        if now.duration_since(self.config_at) >= CONFIG_EVERY {
            self.config_at = now;
            let config = self.h.config.read().unwrap_or_else(PoisonError::into_inner);
            if !config.dvr_enabled() {
                tracing::info!("stream dvr: turned off during the stream");
                return false;
            }
        }
        self.update_live();
        if self.shared.client_speaks.load(Ordering::Acquire)
            && (self.sent != Some(self.state()) || now.duration_since(self.sent_at) >= STATE_EVERY)
        {
            self.post_state(0);
        }
        true
    }

    fn finish(mut self) {
        self.poll_job(true);
        self.stop_recording(None);
        self.shared.set_live(|live| {
            live.state = State::Off;
            live.reason = Reason::NotStreaming;
            live.recording_since = None;
        });
        if let Some(s) = self.session.upgrade() {
            s.dvr.disarm();
        }
        self.h.dvr.unregister(&self.shared.session_id);
        tracing::info!(tap_drops = self.drops_seen, "stream dvr ended");
    }
}
