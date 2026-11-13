//! Mid-stream bitrate and frame-rate changes a client asks for (control message 0x553C,
//! `rubylight_protocol::rate_change`), for a client that launched with `rlRateChange=1`.
//!
//! The control thread decides each request with [`decide`]. A bitrate is capped by the
//! device's `max_bitrate` and the runtime cap ([`crate::stream_policy::runtime_bitrate_kbps`])
//! and reaches the encoder on its next frame, which AMF takes without a keyframe. A rate is
//! capped by the encoder's measured throughput at the stream's size and becomes a mode
//! switch in [`crate::reconfigure::Reconfiguration`], applied by the session loop like a
//! 0x5532 request. [`RateChanges`] holds a switching rate's answer until the session loop
//! ran it or went back, and every answer until the control thread sends it. A stream that
//! never launched with `rlRateChange` takes none and answers none.
use std::collections::{BTreeMap, VecDeque};
use std::time::Instant;

use anyhow::{Result, bail};

use crate::config::Config;
use crate::reconfigure::{self, Mode, Reconfiguration};

pub use rubylight_protocol::rate_change::{
    LAUNCH_PARAMETER, MAX_FPS_MILLIHZ, MIN_FPS_MILLIHZ, Outcome, RATE_CHANGE_MESSAGE_TYPE,
    RateChangeRequest, RateChangeResult, Reason, Status,
};

/// A throughput the encoder does not report allows this rate, as WebRTC's offer does.
const UNKNOWN_THROUGHPUT_MILLIHZ: u32 = 60_000;

/// The client's `rlRateChange` launch choice: off when it sent none, an error for a value
/// other than `0` or `1`.
pub fn launch_choice(options: &BTreeMap<String, String>) -> Result<bool> {
    match options.get(LAUNCH_PARAMETER).map(String::as_str) {
        None | Some("0") => Ok(false),
        Some("1") => Ok(true),
        Some(_) => bail!("invalid {LAUNCH_PARAMETER}"),
    }
}

/// The highest rate the encoder takes in real time at `width` x `height`, in millihertz:
/// `mb_rate` is its 16x16 macroblocks per second for the stream's codec (0 when the driver
/// reports none). The rate the stream runs at is always allowed, and so is the launch rate
/// when the throughput is unknown.
pub fn fps_ceiling_millihz(
    mb_rate: u64,
    width: u32,
    height: u32,
    current_millihz: u32,
    launch_millihz: u32,
) -> u32 {
    let macroblocks = u64::from(width.div_ceil(16)) * u64::from(height.div_ceil(16));
    let measured = if mb_rate == 0 || macroblocks == 0 {
        UNKNOWN_THROUGHPUT_MILLIHZ.max(launch_millihz)
    } else {
        (u128::from(mb_rate) * 1000 / u128::from(macroblocks)).min(u128::from(u32::MAX)) as u32
    };
    measured.max(current_millihz).min(MAX_FPS_MILLIHZ)
}

/// The stream a request is decided against.
#[derive(Clone, Copy, Debug)]
pub struct Stream {
    /// The encoder's bitrate the client or host set last.
    pub bitrate_kbps: u32,
    /// The rate the stream runs at, or is switching to ([`Reconfiguration::current`]).
    pub fps_millihz: u32,
    /// The mode it switches to next: the latest one requested, else the current one
    /// ([`Reconfiguration::requested`]). A rate change keeps its size.
    pub requested: Mode,
    /// [`fps_ceiling_millihz`] at `requested`'s size.
    pub fps_ceiling_millihz: u32,
    /// Why the stream does not switch rates ([`crate::reconfigure::refusal`]), if it does not.
    pub fps_refusal: Option<Reason>,
}

/// What the control thread does with one request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Decision {
    /// The bitrate to set now.
    pub bitrate_kbps: Option<u32>,
    /// The mode to run at, the requested size at the asked rate, when the rate is to
    /// change; [`RateChanges::record`] queues the switch to it.
    pub switch_to: Option<Mode>,
    /// The answer, which [`RateChanges`] holds until a switch ends. Its rate is the
    /// stream's until then.
    pub result: RateChangeResult,
}

/// Decides `request` for `stream` under `config`, the device's effective configuration.
/// An invalid request changes nothing.
pub fn decide(request: &RateChangeRequest, stream: &Stream, config: &Config) -> Decision {
    let mut result = RateChangeResult {
        serial: request.serial,
        bitrate_kbps: stream.bitrate_kbps,
        fps_millihz: stream.fps_millihz,
        bitrate: Outcome::UNCHANGED,
        fps: Outcome::UNCHANGED,
    };
    if !request.valid() {
        let invalid = Outcome::refused(Reason::Invalid);
        result.bitrate = if request.bitrate_kbps == 0 {
            Outcome::UNCHANGED
        } else {
            invalid
        };
        result.fps = if request.fps_millihz == 0 && request.bitrate_kbps != 0 {
            Outcome::UNCHANGED
        } else {
            invalid
        };
        return Decision {
            bitrate_kbps: None,
            switch_to: None,
            result,
        };
    }
    let mut bitrate_kbps = None;
    if request.bitrate_kbps != 0 {
        let applied = crate::stream_policy::runtime_bitrate_kbps(config, request.bitrate_kbps);
        result.bitrate = if applied < request.bitrate_kbps {
            Outcome::clamped(Reason::HostLimit)
        } else {
            Outcome::APPLIED
        };
        result.bitrate_kbps = applied;
        bitrate_kbps = Some(applied);
    }
    let mut switch_to = None;
    if request.fps_millihz != 0 {
        if let Some(reason) = stream.fps_refusal {
            result.fps = Outcome::refused(reason);
        } else {
            let target = request.fps_millihz.min(stream.fps_ceiling_millihz);
            result.fps = if target < request.fps_millihz {
                Outcome::clamped(Reason::EncoderLimit)
            } else {
                Outcome::APPLIED
            };
            switch_to = Some(Mode {
                fps_millihz: target,
                ..stream.requested
            });
        }
    }
    Decision {
        bitrate_kbps,
        switch_to,
        result,
    }
}

/// The answer to `request` when the host cannot decide it: each value asked for is refused
/// with `reason`, and the stream keeps `bitrate_kbps` and `fps_millihz`.
pub fn refuse(
    request: &RateChangeRequest,
    bitrate_kbps: u32,
    fps_millihz: u32,
    reason: Reason,
) -> Decision {
    let refused = |asked: u32| {
        if asked == 0 {
            Outcome::UNCHANGED
        } else {
            Outcome::refused(reason)
        }
    };
    Decision {
        bitrate_kbps: None,
        switch_to: None,
        result: RateChangeResult {
            serial: request.serial,
            bitrate_kbps,
            fps_millihz,
            bitrate: refused(request.bitrate_kbps),
            fps: refused(request.fps_millihz),
        },
    }
}

/// The reason for a [`crate::reconfigure::refusal`]: `enabled` is `stream_reconfigure`.
pub fn refusal_reason(enabled: bool) -> Reason {
    if enabled {
        Reason::Codec
    } else {
        Reason::Disabled
    }
}

/// A rate change whose answer waits for its switch.
#[derive(Clone, Copy, Debug)]
struct Pending {
    mode: Mode,
    result: RateChangeResult,
}

/// One session's rate changes: the switch a request queued, the one the session loop runs,
/// and the answers to send. The control thread records requests and 0x5532 requests; the
/// session loop reports what it took and how each switch ended.
#[derive(Debug, Default)]
pub struct RateChanges {
    queued: Option<Pending>,
    switching: Option<Pending>,
    answers: VecDeque<RateChangeResult>,
}

impl RateChanges {
    /// Records a decided request, queueing its switch in `reconfigure`. A rate the stream
    /// will run at replaces one still queued, which is answered as superseded; a bitrate
    /// alone, or a refused or invalid rate, leaves it queued.
    pub fn record(&mut self, decision: &Decision, reconfigure: &mut Reconfiguration, now: Instant) {
        let mut result = decision.result;
        let Some(mode) = decision.switch_to else {
            self.answers.push_back(result);
            return;
        };
        if let Some(previous) = self.queued.take() {
            self.superseded(previous);
        }
        match reconfigure.on_request(now, mode) {
            reconfigure::Outcome::Queued(_) => self.queued = Some(Pending { mode, result }),
            // The stream runs at it, and asking for it withdrew any queued switch.
            _ => {
                result.fps_millihz = mode.fps_millihz;
                self.answers.push_back(result);
            }
        }
    }

    /// A 0x5532 request came. One for another rate replaces the queued one, and so does one
    /// for the current mode, which withdrew the switch; one for another size takes it along.
    pub fn on_reconfigure(&mut self, outcome: reconfigure::Outcome) {
        let Some(mut pending) = self.queued.take() else {
            return;
        };
        match outcome {
            reconfigure::Outcome::Invalid => {}
            reconfigure::Outcome::Queued(mode) if mode.fps_millihz == pending.mode.fps_millihz => {
                pending.mode = mode
            }
            reconfigure::Outcome::Unchanged | reconfigure::Outcome::Queued(_) => {
                return self.superseded(pending);
            }
        }
        self.queued = Some(pending);
    }

    /// The session loop took `mode` to switch to. A queued rate it carries now switches; a
    /// queued rate it does not carry was replaced by a later 0x5532 request, and a switch
    /// still running by this one.
    pub fn taken(&mut self, mode: Mode) {
        if let Some(previous) = self.switching.take() {
            self.superseded(previous);
        }
        if let Some(pending) = self.queued.take() {
            if pending.mode.fps_millihz == mode.fps_millihz {
                self.switching = Some(pending);
            } else {
                self.superseded(pending);
            }
        }
    }

    /// The session loop's switch ended: the stream now runs at `now`, which is the mode it
    /// had before when `refused` says why it went back. A queued request for that mode was
    /// withdrawn with it (`Reconfiguration::refused`) and is answered as running.
    pub fn switched(&mut self, now: Mode, refused: Option<Reason>) {
        if let Some(mut pending) = self.switching.take() {
            pending.result.fps_millihz = now.fps_millihz;
            if let Some(reason) = refused {
                pending.result.fps = Outcome::refused(reason);
            }
            self.answers.push_back(pending.result);
        }
        if refused.is_some()
            && let Some(mut pending) = self.queued.take_if(|pending| pending.mode == now)
        {
            pending.result.fps_millihz = now.fps_millihz;
            self.answers.push_back(pending.result);
        }
    }

    /// The next answer to send.
    pub fn take(&mut self) -> Option<RateChangeResult> {
        self.answers.pop_front()
    }

    fn superseded(&mut self, mut pending: Pending) {
        pending.result.fps = Outcome::refused(Reason::Superseded);
        self.answers.push_back(pending.result);
    }
}

#[cfg(test)]
mod tests;
