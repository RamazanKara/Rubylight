//! Sleep after idle: `IdleSleep` decides from a snapshot every 30 s
//! (`docs/wake-and-audio-design.md`, A.5).
use super::SystemFacts;
use crate::config::{Config, parse_boolean, parse_integer};
use rubylight_native_api::wake::{self as api, IdleSleepState};
use std::time::{Duration, Instant};

/// How often the host takes a snapshot.
pub const TICK: Duration = Duration::from_secs(30);
/// A gap between the two clocks that grows by more than this between two
/// ticks is a sleep.
const RESUME_GAP: Duration = Duration::from_secs(30);

pub const ES_SYSTEM_REQUIRED: u32 = 0x1;
pub const ES_DISPLAY_REQUIRED: u32 = 0x2;
pub const ES_AWAYMODE_REQUIRED: u32 = 0x40;

/// `sleep_after_idle_minutes` and `sleep_while_suspended`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Settings {
    /// 0 is off.
    pub minutes: u32,
    pub while_suspended: bool,
}

impl Settings {
    /// The settings, refusing a value `Config::update` would have refused,
    /// as a hand-edited config file can hold: the error blocks sleep.
    pub fn read(config: &Config) -> Result<Self, String> {
        let value = |key: &str| {
            config
                .values
                .get(key)
                .map(|value| value.trim())
                .filter(|value| !value.is_empty())
        };
        let minutes = match value("sleep_after_idle_minutes") {
            None => 0,
            Some(text) => parse_integer(text)
                .filter(|minutes| *minutes == 0 || (5..=1440).contains(minutes))
                .ok_or_else(|| {
                    format!(
                        "sleep_after_idle_minutes = {text} is invalid: it must be 0 (off) or between 5 and 1440"
                    )
                })? as u32,
        };
        let while_suspended = match value("sleep_while_suspended") {
            None => false,
            Some(text) => parse_boolean(text).ok_or_else(|| {
                format!("sleep_while_suspended = {text} is invalid: it must be true or false")
            })?,
        };
        Ok(Self {
            minutes,
            while_suspended,
        })
    }
}

/// Local input, from `butterpollo.exe --idle-seconds` in the console session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Input {
    Idle(Duration),
    /// Nobody is signed in: idle time counts from the last session change.
    NobodySignedIn,
    /// The helper failed; this blocks sleep.
    Failed(String),
}

/// What the host sees at one tick.
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub now: Instant,
    pub settings: Result<Settings, String>,
    /// `None` when they could not be read.
    pub system: Option<SystemFacts>,
    /// A session is active or pending, any role or transport, or the session
    /// state isn't idle.
    pub streaming: bool,
    /// A suspended session, or an app the host launched still running.
    pub suspended: bool,
    /// An update is downloading or installing.
    pub updating: bool,
    /// Since the last authenticated console request; `None` if none came.
    pub console: Option<Duration>,
    /// `SystemExecutionState`.
    pub execution: Result<u32, String>,
    pub input: Input,
    /// How long the PC has slept since it started (`wake::slept_ms`).
    pub slept: Duration,
}

/// The answer to a snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    pub state: IdleSleepState,
    pub minutes: u32,
    /// Why it is blocked or unsupported, or, while armed, what it waits for.
    pub reason: Option<String>,
    /// While armed: how long until the PC sleeps if nothing changes.
    pub remaining: Option<Duration>,
    /// Sleep now.
    pub sleep: bool,
}

impl Status {
    fn new(state: IdleSleepState, minutes: u32, reason: Option<String>) -> Self {
        Self {
            state,
            minutes,
            reason,
            remaining: None,
            sleep: false,
        }
    }
    pub fn api(&self) -> api::IdleSleep {
        api::IdleSleep {
            state: self.state,
            minutes: self.minutes,
            reason: match self.state {
                IdleSleepState::Armed | IdleSleepState::Off => None,
                _ => self.reason.clone(),
            },
        }
    }
}

/// What keeps Windows awake, from `SystemExecutionState`.
pub fn kept_awake(state: u32) -> Option<String> {
    let names: Vec<_> = [
        (ES_SYSTEM_REQUIRED, "the system"),
        (ES_DISPLAY_REQUIRED, "the display"),
        (ES_AWAYMODE_REQUIRED, "away mode"),
    ]
    .into_iter()
    .filter(|(flag, _)| state & flag != 0)
    .map(|(_, name)| name)
    .collect();
    (!names.is_empty()).then(|| {
        format!(
            "an app keeps {} awake (see powercfg /requests)",
            names.join(" and ")
        )
    })
}

/// The timers behind sleep after idle.
#[derive(Clone, Debug)]
pub struct IdleSleep {
    /// The latest of: the host started, a stream or suspended app last kept
    /// the PC awake, it last resumed, and the last attempt to sleep.
    quiet_since: Instant,
    /// When nobody has been signed in since.
    signed_out: Option<Instant>,
    /// The last snapshot's sleep time, to notice a resume.
    slept: Option<Duration>,
}

impl IdleSleep {
    pub fn new(now: Instant) -> Self {
        Self {
            quiet_since: now,
            signed_out: None,
            slept: None,
        }
    }

    /// The status for `snapshot`; `sleep` only when every condition holds.
    pub fn tick(&mut self, snapshot: &Snapshot) -> Status {
        let now = snapshot.now;
        if self
            .slept
            .replace(snapshot.slept)
            .is_some_and(|slept| snapshot.slept > slept + RESUME_GAP)
        {
            self.quiet_since = now;
        }
        match &snapshot.input {
            Input::NobodySignedIn => {
                self.signed_out.get_or_insert(now);
            }
            _ => self.signed_out = None,
        }
        let settings = match &snapshot.settings {
            Ok(settings) => *settings,
            Err(error) => return Status::new(IdleSleepState::Blocked, 0, Some(error.clone())),
        };
        let minutes = settings.minutes;
        if minutes == 0 {
            return Status::new(IdleSleepState::Off, 0, None);
        }
        let blocked =
            |reason: &str| Status::new(IdleSleepState::Blocked, minutes, Some(reason.into()));
        let unsupported =
            |reason: &str| Status::new(IdleSleepState::Unsupported, minutes, Some(reason.into()));
        match snapshot.system {
            None => return unsupported("the power capabilities could not be read"),
            Some(system) if system.modern_standby => {
                return unsupported(
                    "this PC uses Modern Standby, which Windows lets no service enter",
                );
            }
            Some(system) if !system.s3 => return unsupported("this PC has no sleep state (S3)"),
            Some(_) => {}
        }
        let suspended = snapshot.suspended && !settings.while_suspended;
        if snapshot.streaming || suspended {
            self.quiet_since = now;
        }
        if snapshot.streaming {
            return blocked("a stream is active or starting");
        }
        if suspended {
            return blocked("a suspended stream or a launched app is still running");
        }
        if snapshot.updating {
            return blocked("an update is downloading or installing");
        }
        match &snapshot.execution {
            Err(error) => return blocked(&format!("the execution state is unreadable: {error}")),
            Ok(state) => {
                if let Some(reason) = kept_awake(*state) {
                    return blocked(&reason);
                }
            }
        }
        let input = match &snapshot.input {
            Input::Failed(error) => return blocked(&format!("local input is unknown: {error}")),
            Input::Idle(idle) => *idle,
            Input::NobodySignedIn => now.duration_since(self.signed_out.unwrap_or(now)),
        };
        let interval = Duration::from_secs(u64::from(minutes) * 60);
        let waits = [
            (now.duration_since(self.quiet_since), "quiet"),
            (input, "local input"),
            (
                snapshot.console.unwrap_or(Duration::MAX),
                "console activity",
            ),
        ];
        let (left, reason) = waits
            .iter()
            .map(|(elapsed, reason)| (interval.saturating_sub(*elapsed), *reason))
            .max_by_key(|(left, _)| *left)
            .unwrap();
        Status {
            remaining: Some(left),
            reason: (!left.is_zero()).then(|| format!("waiting for {reason}")),
            sleep: left.is_zero(),
            ..Status::new(IdleSleepState::Armed, minutes, None)
        }
    }

    /// After an attempt to sleep, failed or not: the next one waits a full
    /// interval.
    pub fn attempted(&mut self, now: Instant) {
        self.quiet_since = now;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINUTE: Duration = Duration::from_secs(60);
    const S3: SystemFacts = SystemFacts {
        s3: true,
        modern_standby: false,
        hibernate: false,
        hiberboot: Some(false),
    };

    struct Pc {
        start: Instant,
        idle: IdleSleep,
        snapshot: Snapshot,
    }

    impl Pc {
        /// Idle for good, with sleep after 10 minutes, at the host's start.
        fn new() -> Self {
            let start = Instant::now();
            Self {
                start,
                idle: IdleSleep::new(start),
                snapshot: Snapshot {
                    now: start,
                    settings: Ok(Settings {
                        minutes: 10,
                        while_suspended: false,
                    }),
                    system: Some(S3),
                    streaming: false,
                    suspended: false,
                    updating: false,
                    console: None,
                    execution: Ok(0),
                    input: Input::Idle(Duration::from_secs(604_800)),
                    slept: Duration::ZERO,
                },
            }
        }
        /// The status `at` after the host started.
        fn at(&mut self, at: Duration) -> Status {
            self.snapshot.now = self.start + at;
            self.idle.tick(&self.snapshot)
        }
        fn blocked(&mut self, at: Duration, reason: &str) {
            let status = self.at(at);
            assert_eq!(status.state, IdleSleepState::Blocked, "{status:?}");
            assert!(!status.sleep);
            assert!(
                status.reason.as_ref().unwrap().contains(reason),
                "{status:?}"
            );
        }
    }

    #[test]
    fn sleeps_once_idle_for_the_interval_since_the_host_started() {
        let mut pc = Pc::new();
        let status = pc.at(9 * MINUTE);
        assert_eq!(status.state, IdleSleepState::Armed);
        assert!(!status.sleep);
        assert_eq!(status.remaining, Some(MINUTE));
        assert_eq!(status.api().reason, None);
        assert!(pc.at(10 * MINUTE).sleep);
        // A failed attempt waits a full interval again, not a tick.
        pc.idle.attempted(pc.start + 10 * MINUTE);
        assert!(!pc.at(10 * MINUTE + TICK).sleep);
        assert!(pc.at(20 * MINUTE).sleep);
    }

    #[test]
    fn off_by_default_and_an_invalid_setting_blocks() {
        let config = Config::default();
        assert_eq!(Settings::read(&config), Ok(Settings::default()));
        let mut pc = Pc::new();
        pc.snapshot.settings = Settings::read(&config);
        let status = pc.at(1440 * MINUTE);
        assert_eq!((status.state, status.sleep), (IdleSleepState::Off, false));
        // A hand-edited value Config::update would refuse.
        for text in [
            "sleep_after_idle_minutes = 3",
            "sleep_after_idle_minutes = soon",
        ] {
            let config = Config::parse(text).unwrap();
            let error = Settings::read(&config).unwrap_err();
            assert!(error.contains("sleep_after_idle_minutes"), "{error}");
            pc.snapshot.settings = Settings::read(&config);
            pc.blocked(1440 * MINUTE, "is invalid");
        }
        let config =
            Config::parse("sleep_after_idle_minutes = 5\nsleep_while_suspended = maybe").unwrap();
        pc.snapshot.settings = Settings::read(&config);
        pc.blocked(1440 * MINUTE, "sleep_while_suspended");
        let config =
            Config::parse("sleep_after_idle_minutes = 1440\nsleep_while_suspended = true").unwrap();
        assert_eq!(
            Settings::read(&config),
            Ok(Settings {
                minutes: 1440,
                while_suspended: true
            })
        );
    }

    #[test]
    fn never_sleeps_with_a_session_and_waits_the_interval_after_it() {
        let mut pc = Pc::new();
        pc.snapshot.streaming = true;
        pc.blocked(60 * MINUTE, "stream");
        pc.snapshot.streaming = false;
        assert!(!pc.at(65 * MINUTE).sleep);
        assert!(pc.at(70 * MINUTE).sleep);
    }

    #[test]
    fn a_suspended_session_blocks_unless_opted_in() {
        let mut pc = Pc::new();
        pc.snapshot.suspended = true;
        pc.blocked(60 * MINUTE, "suspended");
        pc.snapshot.settings = Ok(Settings {
            minutes: 10,
            while_suspended: true,
        });
        // The interval runs from when it last blocked.
        assert!(!pc.at(61 * MINUTE).sleep);
        assert!(pc.at(70 * MINUTE).sleep);
    }

    #[test]
    fn timers_reset_on_resume() {
        let mut pc = Pc::new();
        assert!(pc.at(20 * MINUTE).sleep);
        // The PC slept for an hour (by itself or through us).
        pc.snapshot.slept = 60 * MINUTE;
        assert!(!pc.at(81 * MINUTE).sleep);
        assert!(pc.at(91 * MINUTE).sleep);
        // Clock jitter under the gap isn't a resume.
        pc.snapshot.slept += Duration::from_secs(5);
        assert!(pc.at(92 * MINUTE).sleep);
    }

    #[test]
    fn local_input_and_console_activity_hold_it_off() {
        let mut pc = Pc::new();
        pc.snapshot.input = Input::Idle(3 * MINUTE);
        let status = pc.at(60 * MINUTE);
        assert_eq!(status.state, IdleSleepState::Armed);
        assert!(!status.sleep);
        assert_eq!(status.remaining, Some(7 * MINUTE));
        assert_eq!(status.reason.as_deref(), Some("waiting for local input"));
        pc.snapshot.input = Input::Idle(10 * MINUTE);
        pc.snapshot.console = Some(MINUTE);
        let status = pc.at(60 * MINUTE);
        assert!(!status.sleep);
        assert_eq!(
            status.reason.as_deref(),
            Some("waiting for console activity")
        );
        pc.snapshot.console = Some(10 * MINUTE);
        assert!(pc.at(60 * MINUTE).sleep);
    }

    #[test]
    fn with_nobody_signed_in_idle_counts_from_the_sign_out() {
        let mut pc = Pc::new();
        pc.snapshot.input = Input::NobodySignedIn;
        assert!(!pc.at(30 * MINUTE).sleep);
        assert!(!pc.at(39 * MINUTE).sleep);
        assert!(pc.at(40 * MINUTE).sleep);
        // Signed in and out again: from the new sign-out.
        pc.snapshot.input = Input::Idle(Duration::ZERO);
        pc.at(41 * MINUTE);
        pc.snapshot.input = Input::NobodySignedIn;
        assert!(!pc.at(42 * MINUTE).sleep);
        assert!(pc.at(52 * MINUTE).sleep);
    }

    #[test]
    fn power_requests_updates_and_a_failed_helper_block() {
        let mut pc = Pc::new();
        for (state, name) in [
            (ES_SYSTEM_REQUIRED, "the system"),
            (ES_DISPLAY_REQUIRED, "the display"),
            (ES_AWAYMODE_REQUIRED, "away mode"),
        ] {
            pc.snapshot.execution = Ok(state | 0x8000_0000);
            pc.blocked(60 * MINUTE, name);
        }
        // ES_CONTINUOUS alone keeps nothing awake.
        pc.snapshot.execution = Ok(0x8000_0000);
        assert!(pc.at(60 * MINUTE).sleep);
        pc.snapshot.execution = Err("access denied".into());
        pc.blocked(60 * MINUTE, "access denied");
        pc.snapshot.execution = Ok(0);
        pc.snapshot.updating = true;
        pc.blocked(60 * MINUTE, "update");
        pc.snapshot.updating = false;
        pc.snapshot.input = Input::Failed("no console session".into());
        pc.blocked(60 * MINUTE, "no console session");
    }

    #[test]
    fn modern_standby_and_no_s3_are_unsupported() {
        let mut pc = Pc::new();
        for (system, reason) in [
            (
                Some(SystemFacts {
                    modern_standby: true,
                    ..S3
                }),
                "Modern Standby",
            ),
            (Some(SystemFacts { s3: false, ..S3 }), "S3"),
            (None, "could not be read"),
        ] {
            pc.snapshot.system = system;
            let status = pc.at(60 * MINUTE);
            assert_eq!(status.state, IdleSleepState::Unsupported);
            assert!(!status.sleep);
            assert!(status.api().reason.unwrap().contains(reason));
        }
    }

    #[test]
    fn kept_awake_names_each_flag() {
        assert_eq!(kept_awake(0), None);
        assert_eq!(
            kept_awake(ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED).unwrap(),
            "an app keeps the system and the display awake (see powercfg /requests)"
        );
    }
}
