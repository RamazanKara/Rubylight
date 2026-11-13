//! Mid-stream resolution and frame-rate changes requested by the client.
//!
//! A client whose display changes during a stream (a foldable opening its inner
//! screen, a resized window) sends a [`Request`] (control message 0x5532). The
//! control thread records it in [`Reconfiguration`]; the session loop takes the
//! latest request once the client has stopped sending for [`Reconfiguration::SETTLE`],
//! rebuilds the encoder at the new size and rate and starts the new size with a
//! keyframe. A client that never sends 0x5532 keeps the mode it negotiated.
use crate::rtsp::Negotiated;
use std::time::{Duration, Instant};

pub use rubylight_protocol::control::{RECONFIGURE_MESSAGE_TYPE, Reconfigure as Request};

/// The stream's video mode: the encoder's picture size and the stream rate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mode {
    pub width: u32,
    pub height: u32,
    pub fps_millihz: u32,
}
impl Mode {
    pub fn of(stream: &Negotiated) -> Self {
        Self {
            width: stream.width,
            height: stream.height,
            fps_millihz: stream.fps_millihz(),
        }
    }
    pub fn from_request(request: &Request) -> Self {
        Self {
            width: u32::from(request.width),
            height: u32::from(request.height),
            fps_millihz: request.fps_millihz,
        }
    }
    /// Time between two frames at this mode's rate.
    pub fn period(&self) -> Duration {
        crate::framegen::Rate(self.fps_millihz).period()
    }
    /// The stream configuration at this mode; everything else stays as negotiated.
    pub fn apply_to(&self, stream: &mut Negotiated) {
        stream.width = self.width;
        stream.height = self.height;
        stream.rate_millihz = self.fps_millihz;
        stream.fps = self.fps_millihz.saturating_add(500) / 1000;
    }
}
impl std::fmt::Display for Mode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}x{}@{}",
            self.width,
            self.height,
            crate::framegen::Rate(self.fps_millihz)
        )
    }
}

/// Why a session does not switch modes, or `None` when it can.
pub fn refusal(stream: &Negotiated, enabled: bool) -> Option<&'static str> {
    if !enabled {
        Some("stream_reconfigure is off for this stream")
    } else if stream.chroma_packing != 0 {
        Some("packed full-chroma geometry is negotiated at connection; reconnect to change it")
    } else if stream.codec == 3 {
        // Its sender, error correction and bitrate floor are sized at connect.
        Some("PyroWave streams keep the mode they connected with")
    } else {
        None
    }
}

/// What became of a 0x5532 payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Malformed, an unknown version, or a size or rate outside the protocol's limits.
    Invalid,
    /// The stream already runs at this mode; a pending request is withdrawn.
    Unchanged,
    /// Applied once the client has stopped sending for [`Reconfiguration::SETTLE`].
    Queued(Mode),
}

#[derive(Clone, Copy, Debug)]
struct Pending {
    mode: Mode,
    since: Instant,
}

/// A session's requested and current mode. The control thread records requests;
/// the session loop takes them at a frame boundary.
#[derive(Debug)]
pub struct Reconfiguration {
    current: Mode,
    pending: Option<Pending>,
    switched_at: Option<Instant>,
}
impl Reconfiguration {
    /// A burst of requests (a window being resized, a hinge moving) is applied
    /// once, with the latest request, after this long without another.
    pub const SETTLE: Duration = Duration::from_millis(200);
    /// Each switch rebuilds the encoder and costs a keyframe: at most one a second.
    pub const MIN_INTERVAL: Duration = Duration::from_secs(1);

    pub fn new(current: Mode) -> Self {
        Self {
            current,
            pending: None,
            switched_at: None,
        }
    }
    /// The mode the stream runs at, or is switching to.
    pub fn current(&self) -> Mode {
        self.current
    }
    /// The mode the stream switches to next: the pending request's, else the current one.
    pub fn requested(&self) -> Mode {
        self.pending.map_or(self.current, |pending| pending.mode)
    }
    /// Records a 0x5532 payload.
    pub fn on_payload(&mut self, now: Instant, payload: &[u8]) -> Outcome {
        match Request::decode(payload) {
            Some(request) => self.on_request(now, Mode::from_request(&request)),
            None => Outcome::Invalid,
        }
    }
    pub fn on_request(&mut self, now: Instant, mode: Mode) -> Outcome {
        if mode == self.current {
            self.pending = None;
            return Outcome::Unchanged;
        }
        // The same request again does not restart the wait, so a client repeating
        // it cannot hold the switch off.
        if self.pending.is_none_or(|pending| pending.mode != mode) {
            self.pending = Some(Pending { mode, since: now });
        }
        Outcome::Queued(mode)
    }
    /// The request to apply now: the latest one, once none has come for
    /// [`Self::SETTLE`] and the last switch is [`Self::MIN_INTERVAL`] ago. It becomes
    /// the current mode; call [`Self::refused`] if the session does not switch.
    pub fn take_due(&mut self, now: Instant) -> Option<Mode> {
        let pending = self.pending?;
        let settled = now.saturating_duration_since(pending.since) >= Self::SETTLE;
        let spaced = self
            .switched_at
            .is_none_or(|at| now.saturating_duration_since(at) >= Self::MIN_INTERVAL);
        if !settled || !spaced {
            return None;
        }
        self.pending = None;
        self.switched_at = Some(now);
        self.current = pending.mode;
        Some(pending.mode)
    }
    /// The session did not switch and stays at `mode`.
    pub fn refused(&mut self, mode: Mode) {
        self.current = mode;
        if self.pending.is_some_and(|pending| pending.mode == mode) {
            self.pending = None;
        }
    }
}

/// How the session loop paces frames at a stream rate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rates {
    /// Time between two frames at the stream rate.
    pub period: Duration,
    /// Time after which an unchanged picture is encoded again.
    pub static_period: Duration,
    /// An unchanged picture is resent below the stream rate.
    pub limit_static_rate: bool,
}
impl Rates {
    /// `minimum_fps_target` as configured; as in Vibepollo, an unset minimum is 20 for
    /// every codec, PyroWave too: its old default of the full stream rate repeated a
    /// still picture every period, and under VRR a game frame a little late followed a
    /// repeat.
    pub fn new(stream: &Negotiated, minimum_fps_target: &str) -> Self {
        let fps = f64::from(stream.fps_millihz()) / 1000.;
        let minimum = crate::config::number_text(minimum_fps_target)
            .parse::<f64>()
            .unwrap_or(20.);
        let minimum = if minimum > 0. {
            minimum.clamp(1., fps.max(1.))
        } else if stream.codec == 3 {
            fps
        } else {
            (f64::from(stream.fps_millihz()) / 5000.).max(10.)
        };
        Self {
            period: crate::framegen::Rate(stream.fps_millihz()).period(),
            static_period: Duration::from_secs_f64(1. / minimum),
            limit_static_rate: minimum < fps,
        }
    }
}

#[cfg(test)]
mod tests;
