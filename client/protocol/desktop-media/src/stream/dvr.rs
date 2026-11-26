//! Stream DVR (instant replay) over control message 0x554A, on a stream whose host offers
//! `Feature::StreamDvr` and listed 0x554A: the host's replay state, the stream menu's Save clip
//! and Start / Stop recording entries built from it, the requests they send, and the toast a
//! clip result or a refusal shows. The menu draws the entries (`stream_menu::dvr`).

use std::time::{Duration, Instant};

use nightjar_core::control::ClientMessage;
use rubylight_protocol::stream_dvr::{ClipResult, HostState, Op, Outcome, Reason, Request, State};

use crate::strings::{tr, trf};

use super::Streaming;

/// A save or a stop the host has not answered in this long frees its entry again.
const ANSWER: Duration = Duration::from_secs(30);

/// One entry of the stream menu's Instant Replay group.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// What pressing it asks the host.
    pub op: Op,
    pub label: String,
    /// The line under it: the buffer, the recording's time or, while it is off, why.
    pub detail: String,
    pub enabled: bool,
    /// The detail says why it is off, in the error colour.
    pub problem: bool,
}

/// The host's DVR as this stream knows it.
#[derive(Debug, Default)]
pub struct Dvr {
    last_id: u32,
    /// The latest state, asked at the start and pushed by the host on every change.
    state: Option<HostState>,
    /// The request waiting on its answer, and when it went.
    pending: Option<(u32, Op, Instant)>,
}

impl Dvr {
    /// The request for `op`, with a fresh id (0 is the host's own pushes).
    pub fn request(&mut self, op: Op, now: Instant) -> Request {
        self.last_id = self.last_id.wrapping_add(1).max(1);
        if op != Op::QueryState {
            self.pending = Some((self.last_id, op, now));
        }
        Request { request_id: self.last_id, op, duration_secs: 0 }
    }

    /// Takes a state; the toast for a recording the host would not start.
    pub fn state(&mut self, state: HostState) -> Option<String> {
        self.state = Some(state);
        // A save or a stop ends with its clip result, a start with the state.
        let started = matches!(self.pending, Some((id, Op::StartRecord, _)) if id == state.request_id && id != 0);
        if !started {
            return None;
        }
        self.pending = None;
        (state.state != State::Recording).then(|| reason_text(state.reason).to_owned())
    }

    /// Takes a clip result; the toast it shows.
    pub fn clip(&mut self, clip: &ClipResult) -> String {
        let op = self.answered(clip.request_id);
        match clip.outcome {
            Outcome::Saved if op == Some(Op::StopRecord) => {
                trf("Recording saved ({}, {})", &[&clock(u32::from(clip.duration_secs)), &size(clip.file_bytes)])
            }
            Outcome::Saved => trf("Clip saved ({} s, {})", &[&clip.duration_secs, &size(clip.file_bytes)]),
            Outcome::Refused | Outcome::Failed => reason_text(clip.reason).to_owned(),
        }
    }

    /// A request that could not be sent frees its entry; the toast says so.
    pub fn unsent(&mut self) -> String {
        self.pending = None;
        reason_text(Reason::Unknown).to_owned()
    }

    /// The pending op `request_id` answers, taken off.
    fn answered(&mut self, request_id: u32) -> Option<Op> {
        match self.pending {
            Some((id, op, _)) if id == request_id && request_id != 0 => {
                self.pending = None;
                Some(op)
            }
            _ => None,
        }
    }

    fn waiting(&self, op: Op, now: Instant) -> bool {
        self.pending.is_some_and(|(_, pending, at)| pending == op && now.duration_since(at) < ANSWER)
    }

    /// Save clip and Start / Stop recording, as the latest state has them.
    pub fn entries(&self, now: Instant) -> [Entry; 2] {
        let state = self.state.as_ref();
        let recording = state.is_some_and(|s| s.state == State::Recording);
        let record_op = if recording { Op::StopRecord } else { Op::StartRecord };
        let record_label = if recording { tr("Stop recording") } else { tr("Start recording") };
        let mut save = Entry { op: Op::SaveClip, label: tr("Save clip").to_owned(), detail: String::new(), enabled: true, problem: false };
        let mut record = Entry { op: record_op, label: record_label.to_owned(), detail: String::new(), enabled: true, problem: false };
        match state.map(|s| (s.state, s.reason)) {
            // Not heard yet: the host answers a press with its reason.
            None => {}
            Some((State::Off | State::Unsupported | State::Error, reason)) => {
                save.detail = reason_text(reason).to_owned();
                save.problem = true;
                save.enabled = false;
                record.enabled = false;
            }
            Some((State::Saving, _)) => {
                save.detail = tr("Saving clip…").to_owned();
                save.enabled = false;
            }
            Some((State::Armed | State::Recording, _)) => {
                save.detail = buffered(state.map_or(0, |s| s.ring_buffered_secs));
            }
        }
        if recording {
            record.detail = trf("Recording {}", &[&clock(state.map_or(0, |s| s.recording_elapsed_secs))]);
        }
        if self.waiting(Op::SaveClip, now) {
            save.detail = tr("Saving clip…").to_owned();
            save.enabled = false;
        }
        if self.waiting(Op::StopRecord, now) || self.waiting(Op::StartRecord, now) {
            record.enabled = false;
        }
        [save, record]
    }
}

/// The plain-English reason the host gave for a DVR that is off or a clip it did not save.
pub fn reason_text(reason: Reason) -> &'static str {
    match reason {
        Reason::Disabled => tr("Instant Replay is turned off on the host PC. Turn on Stream DVR in the host PC’s settings to save clips."),
        Reason::Pyrowave => tr("Clips cannot be saved while streaming with PyroWave. Switch to H.264, HEVC, or AV1 to use Stream DVR."),
        Reason::DiskFull | Reason::QuotaExceeded => {
            tr("Not enough free disk space on the host PC to save this clip. Free up space or delete older clips.")
        }
        Reason::NoKeyframeYet => tr("Waiting for the stream to capture a full frame. Try saving again in a moment."),
        Reason::AudioCodecUnsupported => tr("Clips require Opus audio. Switch audio codec from passthrough to Opus to record clips."),
        Reason::Busy => tr("The host PC is still saving the last clip. Try again in a moment."),
        Reason::PermissionDenied => tr("Only the device that started this stream can save clips."),
        Reason::IoError => tr("The host PC could not write the clip to its disk."),
        Reason::NotStreaming => tr("Clips can be saved only while a stream is running."),
        Reason::None | Reason::Unknown => tr("The host PC could not save the clip."),
    }
}

/// "Last 60 s ready", or the frame wait before the buffer holds anything.
fn buffered(secs: u16) -> String {
    if secs == 0 { reason_text(Reason::NoKeyframeYet).to_owned() } else { trf("Last {} s ready", &[&secs]) }
}

/// A recording's time: 02:14, or 1:02:14 past an hour.
pub fn clock(secs: u32) -> String {
    let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m:02}:{s:02}") }
}

/// A clip's size in decimal units: 84 MB, 1.2 GB.
pub fn size(bytes: u64) -> String {
    const MB: u64 = 1_000_000;
    if bytes >= 1000 * MB {
        format!("{:.1} GB", bytes as f64 / (1000 * MB) as f64)
    } else {
        format!("{} MB", bytes.div_ceil(MB).max(1))
    }
}

impl Streaming {
    /// Asks the host for its DVR state once the stream runs.
    pub(super) fn dvr_start(&mut self) {
        if self.dvr.is_some() {
            self.dvr_request(Op::QueryState);
        }
    }

    /// The stream menu's Instant Replay entries, on a stream that offers them.
    pub fn dvr_menu(&self) -> Option<[Entry; 2]> {
        self.dvr.as_ref().map(|dvr| dvr.entries(Instant::now()))
    }

    /// Sends `op` from the menu (or the start's state query).
    pub fn dvr_request(&mut self, op: Op) {
        let Some(dvr) = &mut self.dvr else { return };
        let request = dvr.request(op, Instant::now());
        if let Err(error) = self.session.send(ClientMessage::StreamDvr(request)) {
            eprintln!("nightjar: stream DVR: could not send {op:?}: {error}");
            let toast = dvr.unsent();
            if op != Op::QueryState {
                self.show_toast(toast);
            }
        }
        self.needs_redraw = true;
    }

    pub(super) fn dvr_state(&mut self, state: HostState) {
        let Some(dvr) = &mut self.dvr else { return };
        if let Some(toast) = dvr.state(state) {
            self.show_toast(toast);
        }
        self.needs_redraw = true;
    }

    pub(super) fn dvr_clip(&mut self, clip: &ClipResult) {
        let Some(dvr) = &mut self.dvr else { return };
        let toast = dvr.clip(clip);
        self.show_toast(toast);
    }
}

#[cfg(test)]
#[path = "dvr_tests.rs"]
mod tests;
