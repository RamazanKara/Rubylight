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
        let minimum = minimum_fps_target.parse::<f64>().unwrap_or(20.);
        let minimum = if minimum > 0. {
            minimum.clamp(1., fps)
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
mod tests {
    use super::*;

    fn mode(width: u32, height: u32, fps: u32) -> Mode {
        Mode {
            width,
            height,
            fps_millihz: fps * 1000,
        }
    }

    fn payload(width: u16, height: u16, fps_millihz: u32) -> Vec<u8> {
        Request {
            width,
            height,
            fps_millihz,
        }
        .encode()
        .to_vec()
    }

    #[test]
    fn decodes_and_validates_requests() {
        let now = Instant::now();
        let mut state = Reconfiguration::new(mode(2400, 1080, 120));
        assert_eq!(
            state.on_payload(now, &payload(2176, 1812, 120_000)),
            Outcome::Queued(mode(2176, 1812, 120))
        );
        // Odd sizes, tiny sizes, rates outside 10-500 fps and short payloads.
        let mut state = Reconfiguration::new(mode(2400, 1080, 120));
        for bad in [
            payload(2177, 1812, 120_000),
            payload(200, 1812, 120_000),
            payload(2176, 1812, 5_000),
            payload(2176, 1812, 600_000),
            payload(2176, 1812, 120_000)[..11].to_vec(),
        ] {
            assert_eq!(state.on_payload(now, &bad), Outcome::Invalid);
        }
        let mut wrong_version = payload(2176, 1812, 120_000);
        wrong_version[0] = 9;
        assert_eq!(state.on_payload(now, &wrong_version), Outcome::Invalid);
        assert_eq!(state.take_due(now + Duration::from_secs(5)), None);
    }

    #[test]
    fn an_identical_request_changes_nothing() {
        let now = Instant::now();
        let mut state = Reconfiguration::new(mode(1920, 1080, 60));
        assert_eq!(
            state.on_request(now, mode(1920, 1080, 60)),
            Outcome::Unchanged
        );
        assert_eq!(state.take_due(now + Duration::from_secs(5)), None);
        // Going back to the current mode within the wait withdraws the request.
        assert_eq!(
            state.on_request(now, mode(2560, 1440, 60)),
            Outcome::Queued(mode(2560, 1440, 60))
        );
        assert_eq!(
            state.on_request(now + Duration::from_millis(50), mode(1920, 1080, 60)),
            Outcome::Unchanged
        );
        assert_eq!(state.take_due(now + Duration::from_secs(5)), None);
        assert_eq!(state.current(), mode(1920, 1080, 60));
    }

    #[test]
    fn a_burst_applies_the_latest_request_once_the_client_settles() {
        let now = Instant::now();
        let ms = |n| now + Duration::from_millis(n);
        let mut state = Reconfiguration::new(mode(2400, 1080, 120));
        state.on_request(now, mode(2000, 1600, 120));
        state.on_request(ms(100), mode(2100, 1700, 120));
        state.on_request(ms(150), mode(2176, 1812, 120));
        // 200 ms after the last request, not the first.
        assert_eq!(state.take_due(ms(299)), None);
        assert_eq!(state.take_due(ms(349)), None);
        assert_eq!(state.take_due(ms(350)), Some(mode(2176, 1812, 120)));
        assert_eq!(state.current(), mode(2176, 1812, 120));
        assert_eq!(state.take_due(ms(1000)), None);
        // Now at that mode, the same request is identical.
        assert_eq!(
            state.on_request(ms(400), mode(2176, 1812, 120)),
            Outcome::Unchanged
        );
    }

    #[test]
    fn a_repeated_request_does_not_restart_the_wait() {
        let now = Instant::now();
        let ms = |n| now + Duration::from_millis(n);
        let mut state = Reconfiguration::new(mode(2400, 1080, 120));
        for at in [0, 100, 190] {
            state.on_request(ms(at), mode(2176, 1812, 120));
        }
        assert_eq!(state.take_due(ms(200)), Some(mode(2176, 1812, 120)));
    }

    #[test]
    fn switches_at_most_once_a_second() {
        let now = Instant::now();
        let ms = |n| now + Duration::from_millis(n);
        let mut state = Reconfiguration::new(mode(2400, 1080, 120));
        state.on_request(now, mode(2176, 1812, 120));
        assert_eq!(state.take_due(ms(200)), Some(mode(2176, 1812, 120)));
        state.on_request(ms(300), mode(2400, 1080, 120));
        assert_eq!(
            state.take_due(ms(600)),
            None,
            "settled, too soon after the switch"
        );
        assert_eq!(state.take_due(ms(1199)), None);
        assert_eq!(state.take_due(ms(1200)), Some(mode(2400, 1080, 120)));
    }

    #[test]
    fn a_refused_switch_keeps_the_mode() {
        let now = Instant::now();
        let mut state = Reconfiguration::new(mode(1920, 1080, 60));
        state.on_request(now, mode(7680, 4320, 60));
        let due = state.take_due(now + Reconfiguration::SETTLE);
        assert_eq!(due, Some(mode(7680, 4320, 60)));
        state.refused(mode(1920, 1080, 60));
        assert_eq!(state.current(), mode(1920, 1080, 60));
        // Asking again is a new request, not an identical one.
        assert_eq!(
            state.on_request(now + Duration::from_secs(1), mode(7680, 4320, 60)),
            Outcome::Queued(mode(7680, 4320, 60))
        );
    }

    #[test]
    fn refuses_disabled_streams_and_pyrowave() {
        let stream = Negotiated::default();
        assert_eq!(refusal(&stream, true), None);
        assert!(refusal(&stream, false).is_some());
        for codec in [0, 1, 2] {
            let stream = Negotiated {
                codec,
                ..Default::default()
            };
            assert_eq!(refusal(&stream, true), None);
        }
        let pyrowave = Negotiated {
            codec: 3,
            ..Default::default()
        };
        assert!(refusal(&pyrowave, true).is_some());
    }

    #[test]
    fn applying_a_mode_changes_only_size_and_rate() {
        let mut stream = Negotiated {
            width: 2400,
            height: 1080,
            fps: 60,
            rate_millihz: 59_940,
            bitrate_kbps: 50_000,
            codec: 1,
            hdr: true,
            ..Default::default()
        };
        let before = stream.clone();
        assert_eq!(Mode::of(&stream), mode(2400, 1080, 0).with_millihz(59_940));
        mode(2176, 1812, 120).apply_to(&mut stream);
        assert_eq!(Mode::of(&stream), mode(2176, 1812, 120));
        assert_eq!((stream.width, stream.height, stream.fps), (2176, 1812, 120));
        assert_eq!(
            (stream.bitrate_kbps, stream.codec, stream.hdr),
            (before.bitrate_kbps, before.codec, before.hdr)
        );
        // A fractional rate keeps its exact millihertz.
        mode(1920, 1080, 0)
            .with_millihz(119_880)
            .apply_to(&mut stream);
        assert_eq!((stream.fps, stream.fps_millihz()), (120, 119_880));
        assert_eq!(
            mode(1920, 1080, 0).with_millihz(119_880).to_string(),
            "1920x1080@119.880"
        );
        assert_eq!(mode(2176, 1812, 120).to_string(), "2176x1812@120");
    }

    impl Mode {
        fn with_millihz(self, fps_millihz: u32) -> Self {
            Self {
                fps_millihz,
                ..self
            }
        }
    }

    #[test]
    fn rates_follow_the_stream_rate() {
        let at = |fps_millihz: u32, codec: u8| Negotiated {
            fps: fps_millihz.div_ceil(1000),
            rate_millihz: fps_millihz,
            codec,
            ..Default::default()
        };
        let rates = Rates::new(&at(60_000, 1), "20");
        assert_eq!(rates.period, Duration::from_nanos(16_666_666));
        assert_eq!(rates.static_period, Duration::from_millis(50));
        assert!(rates.limit_static_rate);
        let rates = Rates::new(&at(120_000, 1), "20");
        assert_eq!(rates.period, Duration::from_nanos(8_333_333));
        assert_eq!(rates.static_period, Duration::from_millis(50));
        // A minimum above the stream rate is the stream rate.
        let rates = Rates::new(&at(30_000, 1), "45");
        assert_eq!(rates.static_period, Duration::from_secs_f64(1. / 30.));
        assert!(!rates.limit_static_rate);
        // 0: a fifth of the stream rate, at least 10; PyroWave resends at the full rate.
        let rates = Rates::new(&at(120_000, 1), "0");
        assert_eq!(rates.static_period, Duration::from_secs_f64(1. / 24.));
        assert_eq!(
            Rates::new(&at(30_000, 1), "0").static_period,
            Duration::from_millis(100)
        );
        let rates = Rates::new(&at(120_000, 3), "0");
        assert_eq!(rates.static_period, Duration::from_secs_f64(1. / 120.));
        assert!(!rates.limit_static_rate);
        // An unreadable value is the default of 20.
        assert_eq!(
            Rates::new(&at(60_000, 1), "fast").static_period,
            Duration::from_millis(50)
        );
    }
}
