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
mod tests {
    use super::*;

    fn mode(width: u32, height: u32, fps: u32) -> Mode {
        Mode {
            width,
            height,
            fps_millihz: fps * 1000,
        }
    }

    fn config(max_bitrate: i64) -> Config {
        let mut config = Config::default();
        if max_bitrate != 0 {
            config
                .values
                .insert("max_bitrate".into(), max_bitrate.to_string());
        }
        config
    }

    fn stream() -> Stream {
        Stream {
            bitrate_kbps: 20_000,
            fps_millihz: 60_000,
            requested: mode(1920, 1080, 60),
            fps_ceiling_millihz: 144_000,
            fps_refusal: None,
        }
    }

    fn request(serial: u32, bitrate_kbps: u32, fps: u32) -> RateChangeRequest {
        RateChangeRequest {
            serial,
            bitrate_kbps,
            fps_millihz: fps * 1000,
        }
    }

    #[test]
    fn the_launch_parameter_is_off_unless_asked_and_refuses_other_values() {
        let options = |value: Option<&str>| {
            value
                .map(|value| (LAUNCH_PARAMETER.to_owned(), value.to_owned()))
                .into_iter()
                .collect::<BTreeMap<_, _>>()
        };
        assert!(!launch_choice(&options(None)).unwrap());
        assert!(!launch_choice(&options(Some("0"))).unwrap());
        assert!(launch_choice(&options(Some("1"))).unwrap());
        for bad in ["", "2", "true", " 1"] {
            assert_eq!(
                launch_choice(&options(Some(bad))).unwrap_err().to_string(),
                "invalid rlRateChange",
                "{bad:?}"
            );
        }
    }

    #[test]
    fn the_rate_ceiling_follows_the_encoder_probe_at_the_stream_size() {
        let mbs_1080p = 120 * 68;
        // 1080p at exactly 240 fps, and the rounding of a 1366-wide picture.
        assert_eq!(
            fps_ceiling_millihz(mbs_1080p * 240, 1920, 1080, 60_000, 60_000),
            240_000
        );
        assert_eq!(
            fps_ceiling_millihz(86 * 48 * 100, 1366, 768, 60_000, 60_000),
            100_000
        );
        // The same encoder at 4K: under a quarter of it.
        assert_eq!(
            fps_ceiling_millihz(mbs_1080p * 240, 3840, 2160, 30_000, 30_000),
            60_444
        );
        // Never below the running rate, never above the protocol's.
        assert_eq!(fps_ceiling_millihz(1, 3840, 2160, 120_000, 60_000), 120_000);
        assert_eq!(
            fps_ceiling_millihz(u64::MAX, 640, 480, 60_000, 60_000),
            MAX_FPS_MILLIHZ
        );
        // Unknown throughput: 60 fps, or the launch rate if that was higher.
        assert_eq!(fps_ceiling_millihz(0, 1920, 1080, 30_000, 30_000), 60_000);
        assert_eq!(fps_ceiling_millihz(0, 1920, 1080, 60_000, 120_000), 120_000);
        assert_eq!(fps_ceiling_millihz(0, 1920, 1080, 90_000, 30_000), 90_000);
    }

    #[test]
    fn bitrates_are_capped_by_the_device_and_the_runtime_cap() {
        let applied = decide(&request(1, 40_000, 0), &stream(), &config(0));
        assert_eq!(applied.bitrate_kbps, Some(40_000));
        assert_eq!(applied.switch_to, None, "the rate stays");
        assert_eq!(
            applied.result,
            RateChangeResult {
                serial: 1,
                bitrate_kbps: 40_000,
                fps_millihz: 60_000,
                bitrate: Outcome::APPLIED,
                fps: Outcome::UNCHANGED,
            }
        );
        let device = decide(&request(2, 80_000, 0), &stream(), &config(50_000));
        assert_eq!(device.bitrate_kbps, Some(50_000));
        assert_eq!(device.result.bitrate_kbps, 50_000);
        assert_eq!(device.result.bitrate, Outcome::clamped(Reason::HostLimit));
        let runtime = decide(&request(3, 900_000, 0), &stream(), &config(0));
        assert_eq!(runtime.bitrate_kbps, Some(500_000));
        assert_eq!(runtime.result.bitrate, Outcome::clamped(Reason::HostLimit));
        // Lowering is always allowed.
        let low = decide(&request(4, 500, 0), &stream(), &config(50_000));
        assert_eq!(low.bitrate_kbps, Some(500));
        assert_eq!(low.result.bitrate, Outcome::APPLIED);
    }

    #[test]
    fn rates_switch_up_to_the_ceiling_and_keep_the_size() {
        let up = decide(&request(1, 0, 120), &stream(), &config(0));
        assert_eq!(up.bitrate_kbps, None);
        assert_eq!(up.switch_to, Some(mode(1920, 1080, 120)));
        assert_eq!(up.result.fps, Outcome::APPLIED);
        assert_eq!(
            up.result.fps_millihz, 60_000,
            "the stream's rate until it switches"
        );
        assert_eq!(up.result.bitrate, Outcome::UNCHANGED);
        assert_eq!(up.result.bitrate_kbps, 20_000);
        let capped = decide(&request(2, 0, 240), &stream(), &config(0));
        assert_eq!(capped.switch_to, Some(mode(1920, 1080, 144)));
        assert_eq!(capped.result.fps, Outcome::clamped(Reason::EncoderLimit));
        let down = decide(&request(3, 0, 10), &stream(), &config(0));
        assert_eq!(down.switch_to, Some(mode(1920, 1080, 10)));
        // The size of a switch requested since, at the asked rate.
        let resizing = Stream {
            requested: mode(1280, 720, 60),
            ..stream()
        };
        let resized = decide(&request(4, 0, 90), &resizing, &config(0));
        assert_eq!(resized.switch_to, Some(mode(1280, 720, 90)));
        assert_eq!(resized.result.fps_millihz, 60_000);
        // Capped to the running rate.
        let at_ceiling = Stream {
            fps_ceiling_millihz: 60_000,
            ..stream()
        };
        let held = decide(&request(5, 30_000, 90), &at_ceiling, &config(0));
        assert_eq!(held.switch_to, Some(mode(1920, 1080, 60)));
        assert_eq!(held.bitrate_kbps, Some(30_000));
        assert_eq!(held.result.fps, Outcome::clamped(Reason::EncoderLimit));
        assert_eq!(held.result.bitrate, Outcome::APPLIED);
    }

    #[test]
    fn a_stream_that_keeps_its_rate_still_takes_the_bitrate() {
        for (enabled, reason) in [(false, Reason::Disabled), (true, Reason::Codec)] {
            let fixed = Stream {
                fps_refusal: Some(refusal_reason(enabled)),
                ..stream()
            };
            let decision = decide(&request(9, 30_000, 120), &fixed, &config(0));
            assert_eq!(decision.switch_to, None);
            assert_eq!(decision.bitrate_kbps, Some(30_000));
            assert_eq!(decision.result.fps, Outcome::refused(reason));
            assert_eq!(decision.result.fps_millihz, 60_000);
            assert_eq!(decision.result.bitrate, Outcome::APPLIED);
        }
    }

    #[test]
    fn invalid_requests_change_nothing_and_say_which_value_was_wrong() {
        let invalid = Outcome::refused(Reason::Invalid);
        for (request, bitrate, fps) in [
            (request(1, 0, 0), Outcome::UNCHANGED, invalid),
            (
                RateChangeRequest {
                    serial: 2,
                    bitrate_kbps: 30_000,
                    fps_millihz: 9_999,
                },
                invalid,
                invalid,
            ),
            (
                RateChangeRequest {
                    serial: 3,
                    bitrate_kbps: 0,
                    fps_millihz: MAX_FPS_MILLIHZ + 1,
                },
                Outcome::UNCHANGED,
                invalid,
            ),
        ] {
            let decision = decide(&request, &stream(), &config(0));
            assert_eq!(decision.bitrate_kbps, None);
            assert_eq!(decision.switch_to, None);
            assert_eq!(decision.result.serial, request.serial);
            assert_eq!(
                (decision.result.bitrate, decision.result.fps),
                (bitrate, fps)
            );
            assert_eq!(decision.result.bitrate_kbps, 20_000);
            assert_eq!(decision.result.fps_millihz, 60_000);
        }
    }

    const SUPERSEDED: Outcome = Outcome::refused(Reason::Superseded);

    /// The control thread and the session loop around one stream at 1080p60.
    struct Session {
        now: Instant,
        reconfigure: Reconfiguration,
        changes: RateChanges,
    }

    impl Session {
        fn new() -> Self {
            Self {
                now: Instant::now(),
                reconfigure: Reconfiguration::new(mode(1920, 1080, 60)),
                changes: RateChanges::default(),
            }
        }

        fn ask(&mut self, serial: u32, bitrate_kbps: u32, fps: u32) {
            let stream = Stream {
                fps_millihz: self.reconfigure.current().fps_millihz,
                requested: self.reconfigure.requested(),
                ..stream()
            };
            let decision = decide(&request(serial, bitrate_kbps, fps), &stream, &config(0));
            self.changes
                .record(&decision, &mut self.reconfigure, self.now);
        }

        fn reconfigure(&mut self, mode: Mode) {
            let outcome = self.reconfigure.on_request(self.now, mode);
            self.changes.on_reconfigure(outcome);
        }

        /// The session loop's next switch, once due.
        fn take(&mut self) -> Option<Mode> {
            self.now += Reconfiguration::MIN_INTERVAL;
            let to = self.reconfigure.take_due(self.now)?;
            self.changes.taken(to);
            Some(to)
        }

        fn ran(&mut self, mode: Mode) {
            self.changes.switched(mode, None);
        }

        fn went_back(&mut self, from: Mode, reason: Reason) {
            self.reconfigure.refused(from);
            self.changes.switched(from, Some(reason));
        }

        fn answers(&mut self) -> Vec<(u32, u32, Outcome)> {
            std::iter::from_fn(|| self.changes.take())
                .map(|result| (result.serial, result.fps_millihz, result.fps))
                .collect()
        }
    }

    #[test]
    fn a_rate_is_answered_once_the_encoder_runs_at_it() {
        let mut session = Session::new();
        session.ask(1, 30_000, 120);
        assert!(session.answers().is_empty(), "the switch has not run");
        assert_eq!(session.take(), Some(mode(1920, 1080, 120)));
        assert!(session.answers().is_empty());
        session.ran(mode(1920, 1080, 120));
        assert_eq!(session.answers(), [(1, 120_000, Outcome::APPLIED)]);
        // Bitrate-only and running-rate requests are answered at once.
        session.ask(2, 25_000, 0);
        session.ask(3, 0, 120);
        assert_eq!(
            session.answers(),
            [
                (2, 120_000, Outcome::UNCHANGED),
                (3, 120_000, Outcome::APPLIED)
            ]
        );
        assert_eq!(session.take(), None);
    }

    #[test]
    fn a_switch_the_encoder_cannot_make_is_refused_with_the_rate_kept() {
        let mut session = Session::new();
        session.ask(1, 0, 120);
        session.take().unwrap();
        session.went_back(mode(1920, 1080, 60), Reason::EncoderFailed);
        assert_eq!(
            session.answers(),
            [(1, 60_000, Outcome::refused(Reason::EncoderFailed))]
        );
        // The session loop's own refusal (the setting changed since) answers the same way.
        session.ask(2, 0, 90);
        session.take().unwrap();
        session.went_back(mode(1920, 1080, 60), Reason::Disabled);
        assert_eq!(
            session.answers(),
            [(2, 60_000, Outcome::refused(Reason::Disabled))]
        );
        assert_eq!(session.reconfigure.current(), mode(1920, 1080, 60));
    }

    #[test]
    fn later_requests_supersede_a_queued_rate() {
        let mut session = Session::new();
        session.ask(1, 0, 120);
        // A bitrate alone, or an invalid rate, leaves the queued rate alone.
        session.ask(2, 30_000, 0);
        session.ask(3, 0, 5);
        assert_eq!(
            session.answers(),
            [
                (2, 60_000, Outcome::UNCHANGED),
                (3, 60_000, Outcome::refused(Reason::Invalid))
            ]
        );
        // Another rate replaces it.
        session.ask(4, 0, 90);
        assert_eq!(session.answers(), [(1, 60_000, SUPERSEDED)]);
        // The queued rate again: this request waits for the switch, which keeps its wait.
        session.ask(5, 0, 90);
        assert_eq!(session.answers(), [(4, 60_000, SUPERSEDED)]);
        assert_eq!(session.take(), Some(mode(1920, 1080, 90)));
        session.ran(mode(1920, 1080, 90));
        assert_eq!(session.answers(), [(5, 90_000, Outcome::APPLIED)]);
        // Back to the running rate before the switch: nothing switches.
        session.ask(6, 0, 30);
        session.ask(7, 0, 90);
        assert_eq!(
            session.answers(),
            [(6, 90_000, SUPERSEDED), (7, 90_000, Outcome::APPLIED)]
        );
        assert_eq!(session.take(), None);
    }

    #[test]
    fn reconfigures_keep_a_queued_rate_only_at_its_rate() {
        let mut session = Session::new();
        session.ask(1, 0, 90);
        // Another size at the queued rate keeps it, and the switch takes both.
        session.reconfigure(mode(1280, 720, 90));
        assert!(session.answers().is_empty());
        assert_eq!(session.take(), Some(mode(1280, 720, 90)));
        session.ran(mode(1280, 720, 90));
        assert_eq!(session.answers(), [(1, 90_000, Outcome::APPLIED)]);
        // A size asked for before keeps its size.
        session.reconfigure(mode(1920, 1080, 90));
        session.ask(2, 0, 120);
        assert_eq!(session.take(), Some(mode(1920, 1080, 120)));
        session.ran(mode(1920, 1080, 120));
        assert_eq!(session.answers(), [(2, 120_000, Outcome::APPLIED)]);
        // Another rate, or the current mode, replaces it.
        session.ask(3, 0, 60);
        session.reconfigure(mode(1280, 720, 30));
        assert_eq!(session.answers(), [(3, 120_000, SUPERSEDED)]);
        session.ask(4, 0, 60);
        session.reconfigure(mode(1920, 1080, 120));
        assert_eq!(session.answers(), [(4, 120_000, SUPERSEDED)]);
        assert_eq!(session.take(), None);
        // An invalid 0x5532 payload changes nothing.
        session.ask(5, 0, 60);
        session
            .changes
            .on_reconfigure(reconfigure::Outcome::Invalid);
        assert!(session.answers().is_empty());
    }

    #[test]
    fn a_switch_still_running_when_the_next_is_taken_is_superseded() {
        let mut session = Session::new();
        session.ask(1, 0, 90);
        session.take().unwrap();
        session.ask(2, 0, 120);
        session.take().unwrap();
        session.ran(mode(1920, 1080, 120));
        assert_eq!(
            session.answers(),
            [(1, 60_000, SUPERSEDED), (2, 120_000, Outcome::APPLIED)]
        );
    }

    #[test]
    fn a_queued_rate_for_the_mode_a_failed_switch_went_back_to_is_running() {
        let mut session = Session::new();
        session.ask(1, 0, 120);
        session.take().unwrap();
        // Asked back for 60 while the switch to 120 runs: queued against the switching mode.
        session.ask(2, 0, 60);
        assert!(session.answers().is_empty());
        session.went_back(mode(1920, 1080, 60), Reason::EncoderFailed);
        assert_eq!(
            session.answers(),
            [
                (1, 60_000, Outcome::refused(Reason::EncoderFailed)),
                (2, 60_000, Outcome::APPLIED)
            ]
        );
        assert_eq!(session.take(), None);
    }
}
