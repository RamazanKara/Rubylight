//! Wake-on-LAN readiness, the consented NIC fix and sleep after idle
//! (`docs/wake-and-audio-design.md`, A.3 and A.5). Everything Windows answers
//! goes through [`PowerSystem`], which tests replace with a fake.
use crate::state::Host;
use anyhow::{Context, Result};
use butterpollo_core::{
    native_api::{session::State as Session, wake as api},
    wake::{
        self as readiness, Adapter, ApplyReport, FixPlan, NicReport, SystemFacts,
        idle::{self, IdleSleep, Input, Settings, Snapshot, Status},
    },
};
use butterpollo_windows::wake::IDLE_FAILED;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    hash::{DefaultHasher, Hash, Hasher},
    net::IpAddr,
    sync::{Arc, Mutex, PoisonError, RwLock},
    time::{Duration, Instant, SystemTime},
};

/// How long a NIC script reading stays fresh; a fix or `refresh` reads again.
const NIC_CACHE: Duration = Duration::from_secs(60);
/// The console's POST answers before the fix starts, as the adapter restart
/// drops the connection it came on.
pub(crate) const FIX_DELAY: Duration = Duration::from_secs(1);
/// How long the `--idle-seconds` helper may take.
const IDLE_HELPER_WAIT: Duration = Duration::from_secs(10);

/// What the host asks Windows.
pub trait PowerSystem: Send + Sync {
    fn adapters(&self) -> Result<Vec<Adapter>>;
    /// `wake_nics.ps1 read`.
    fn read_nics(&self) -> Result<NicReport>;
    fn system_facts(&self) -> Result<SystemFacts>;
    /// `SystemExecutionState`.
    fn execution_state(&self) -> Result<u32>;
    /// How long the PC has slept since it started.
    fn slept(&self) -> Result<Duration>;
    /// Local input in the console session.
    fn input(&self) -> Input;
    /// Sleep (S3); returns after the PC resumes.
    fn sleep(&self) -> Result<()>;
    /// `wake_nics.ps1 apply`.
    fn apply_nics(&self, plan: &FixPlan) -> Result<ApplyReport>;
    /// `butterpollo.exe --apply-wake-settings` in a child process, as the
    /// host's own account (SYSTEM under the service).
    fn run_fix(&self, fast_startup_off: bool) -> Result<Applied>;
}

/// What `--apply-wake-settings` changed; it prints this as JSON.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Applied {
    /// The plan, in the words the console showed.
    pub changes: Vec<String>,
    /// The adapters restarted for a changed advanced property.
    pub restarted: Vec<String>,
    pub errors: Vec<String>,
}

struct Windows;

impl PowerSystem for Windows {
    fn adapters(&self) -> Result<Vec<Adapter>> {
        butterpollo_windows::wake::adapters()
    }
    fn read_nics(&self) -> Result<NicReport> {
        butterpollo_windows::wake::read_nics()
    }
    fn system_facts(&self) -> Result<SystemFacts> {
        butterpollo_windows::wake::system_facts()
    }
    fn execution_state(&self) -> Result<u32> {
        butterpollo_windows::wake::execution_state()
    }
    fn slept(&self) -> Result<Duration> {
        butterpollo_windows::wake::slept_ms().map(Duration::from_millis)
    }
    fn input(&self) -> Input {
        use butterpollo_windows::process;
        if !process::is_system() {
            // A portable host runs in the user's own session.
            return match butterpollo_windows::wake::idle_seconds() {
                Ok(seconds) => Input::Idle(Duration::from_secs(seconds.into())),
                Err(error) => Input::Failed(format!("{error:#}")),
            };
        }
        if !process::user_signed_in() {
            return Input::NobodySignedIn;
        }
        match idle_helper() {
            Ok(code) => helper_input(code),
            Err(error) => Input::Failed(format!("{error:#}")),
        }
    }
    fn sleep(&self) -> Result<()> {
        butterpollo_windows::wake::suspend()
    }
    fn apply_nics(&self, plan: &FixPlan) -> Result<ApplyReport> {
        butterpollo_windows::wake::apply_nics(plan)
    }
    fn run_fix(&self, fast_startup_off: bool) -> Result<Applied> {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let program = std::env::current_exe()?;
        let mut command = std::process::Command::new(&program);
        command
            .arg("--apply-wake-settings")
            .creation_flags(CREATE_NO_WINDOW);
        if fast_startup_off {
            command.arg("--fast-startup-off");
        }
        // The child bounds its own NIC script runs.
        let output = command
            .output()
            .context("starting butterpollo.exe --apply-wake-settings")?;
        serde_json::from_slice(&output.stdout).with_context(|| {
            format!(
                "--apply-wake-settings ended with {} without a report: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            )
        })
    }
}

/// `butterpollo.exe --idle-seconds` in the signed-in user's session, where
/// `GetLastInputInfo` sees their input; its exit code.
fn idle_helper() -> Result<u32> {
    use butterpollo_windows::process::{Process, Target};
    let program = std::env::current_exe()?;
    let helper = Process::spawn(
        &program,
        &["--idle-seconds".into()],
        program.parent(),
        Target::User { elevated: false },
        &Default::default(),
        true,
    )
    .context("starting the idle helper in the signed-in user's session")?;
    helper.wait(IDLE_HELPER_WAIT).inspect_err(|_| {
        let _ = helper.terminate();
    })
}

/// The idle helper's exit code as local input.
fn helper_input(code: u32) -> Input {
    if code & IDLE_FAILED == 0 {
        return Input::Idle(Duration::from_secs(code.into()));
    }
    let error = std::io::Error::from_raw_os_error((code & !IDLE_FAILED) as i32);
    Input::Failed(format!("the idle helper failed: {error}"))
}

/// `--idle-seconds`: the exit code [`helper_input`] reads.
pub fn idle_seconds_exit_code() -> i32 {
    butterpollo_windows::wake::idle_exit_code() as i32
}

/// `--apply-wake-settings`: apply the plan for this PC, print what changed as
/// JSON and exit 1 if anything failed.
pub fn apply_settings_exit_code(fast_startup_off: bool) -> i32 {
    let applied = apply_settings(&Windows, fast_startup_off).unwrap_or_else(|error| Applied {
        errors: vec![format!("{error:#}")],
        ..Default::default()
    });
    println!(
        "{}",
        serde_json::to_string(&applied).expect("Applied serializes")
    );
    i32::from(!applied.errors.is_empty())
}

/// Read this PC's adapters and settings, plan the changes and apply them.
pub fn apply_settings(power: &dyn PowerSystem, fast_startup_off: bool) -> Result<Applied> {
    let adapters = power.adapters().context("reading the network adapters")?;
    let report = power.read_nics().context("reading the NIC wake settings")?;
    let facts = power
        .system_facts()
        .context("reading the power capabilities")?;
    let plan = FixPlan::new(&adapters, &report, &facts, fast_startup_off);
    let changes = plan.changes();
    if plan.is_empty() {
        return Ok(Applied {
            changes,
            ..Default::default()
        });
    }
    let report = power.apply_nics(&plan)?;
    Ok(Applied {
        changes,
        restarted: report.restarted,
        errors: report.errors,
    })
}

/// The timers and the last answer of sleep after idle.
struct Idle {
    machine: IdleSleep,
    last: Option<Status>,
    /// When the host last put the PC to sleep, and the error if it failed.
    last_sleep: Option<(SystemTime, Option<String>)>,
}

/// The console's fix: one at a time.
#[derive(Default)]
struct Fix {
    running: bool,
    last: Option<(SystemTime, Result<Applied, String>)>,
}

/// The host's wake state, in `Host::wake`.
pub struct Wake {
    power: RwLock<Arc<dyn PowerSystem>>,
    idle: Mutex<Idle>,
    /// The last authenticated console request.
    pub(crate) console: Mutex<Option<Instant>>,
    nics: Mutex<Option<(Instant, Result<NicReport, String>)>>,
    fix: Mutex<Fix>,
    /// The hash of the last `/wake` body and its revision.
    revision: Mutex<(u64, u64)>,
}

impl Default for Wake {
    fn default() -> Self {
        Self {
            power: RwLock::new(Arc::new(Windows)),
            idle: Mutex::new(Idle {
                machine: IdleSleep::new(Instant::now()),
                last: None,
                last_sleep: None,
            }),
            console: Mutex::new(None),
            nics: Mutex::new(None),
            fix: Mutex::default(),
            revision: Mutex::new((0, 0)),
        }
    }
}

fn locked<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Wake {
    fn power(&self) -> Arc<dyn PowerSystem> {
        self.power
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
    #[cfg(test)]
    pub fn set_power(&self, power: Arc<dyn PowerSystem>) {
        *self.power.write().unwrap() = power;
    }
    /// The NIC script's reading, at most [`NIC_CACHE`] old unless `refresh`.
    /// Concurrent readers wait for one run of the script.
    fn nics(&self, power: &dyn PowerSystem, refresh: bool) -> Result<NicReport, String> {
        let mut cache = locked(&self.nics);
        if !refresh
            && let Some((at, report)) = cache.as_ref()
            && at.elapsed() < NIC_CACHE
        {
            return report.clone();
        }
        let report = power.read_nics().map_err(|error| {
            tracing::warn!("Wake-on-LAN: the NIC wake settings could not be read: {error:#}");
            format!("{error:#}")
        });
        *cache = Some((Instant::now(), report.clone()));
        report
    }
    /// The revision of `wake`, raised whenever anything but the per-client
    /// `primary` and `route` changed.
    fn revision(&self, wake: &api::Wake) -> u64 {
        let mut shared = wake.clone();
        shared.primary = None;
        for target in &mut shared.targets {
            target.route = false;
        }
        let mut hasher = DefaultHasher::new();
        serde_json::to_string(&shared)
            .unwrap_or_default()
            .hash(&mut hasher);
        let hash = hasher.finish();
        let mut revision = locked(&self.revision);
        if revision.1 == 0 || revision.0 != hash {
            *revision = (hash, revision.1 + 1);
        }
        revision.1
    }
}

/// An authenticated console request: sleep after idle waits a full interval
/// after the last one.
pub fn note_console_activity(h: &Host) {
    *locked(&h.wake.console) = Some(Instant::now());
}

/// Sleep after idle's state now: the last tick's, unless the settings changed
/// since.
pub fn status(h: &Host) -> Status {
    let settings = Settings::read(&h.config.read().unwrap_or_else(PoisonError::into_inner));
    let state = |state, minutes, reason: Option<&str>| Status {
        state,
        minutes,
        reason: reason.map(Into::into),
        remaining: None,
        sleep: false,
    };
    match settings {
        Err(error) => state(api::IdleSleepState::Blocked, 0, Some(&error)),
        Ok(settings) if settings.minutes == 0 => state(api::IdleSleepState::Off, 0, None),
        Ok(settings) => locked(&h.wake.idle)
            .last
            .clone()
            .filter(|last| last.minutes == settings.minutes)
            .unwrap_or_else(|| {
                state(
                    api::IdleSleepState::Blocked,
                    settings.minutes,
                    Some("not checked yet"),
                )
            }),
    }
}

/// Whether a stream holds the PC, from the host's own maps; `suspended`
/// also counts a launched app and a display kept for a paused stream.
fn holding(h: &Host) -> (bool, bool) {
    let sessions = locked(&h.sessions);
    let streaming = !sessions.active.is_empty() || !sessions.pending.is_empty();
    drop(sessions);
    let suspended = locked(&h.current_app).is_some() || !locked(&h.monitors).is_empty();
    (streaming, suspended)
}

fn updating(h: &Host) -> bool {
    matches!(
        locked(&h.updates)["phase"].as_str(),
        Some("waiting" | "downloading" | "ready" | "installing")
    )
}

/// One sleep-after-idle check, every [`idle::TICK`] on the blocking pool.
pub fn tick(h: &Host) {
    tick_at(h, Instant::now());
}

fn tick_at(h: &Host, now: Instant) {
    let power = h.wake.power();
    let settings = Settings::read(&h.config.read().unwrap_or_else(PoisonError::into_inner));
    let on = settings.as_ref().is_ok_and(|settings| settings.minutes > 0);
    let (mut streaming, mut suspended) = holding(h);
    let state = h.session_snapshot().state;
    streaming |= !matches!(state, Session::Idle | Session::Suspended);
    suspended |= state == Session::Suspended;
    // Off asks Windows nothing and starts no helper.
    let snapshot = Snapshot {
        now,
        settings,
        system: on
            .then(|| {
                power.system_facts().inspect_err(|error| {
                    tracing::warn!("sleep after idle: {error:#}");
                })
            })
            .and_then(Result::ok),
        streaming,
        suspended,
        updating: updating(h),
        console: locked(&h.wake.console).map(|at| now.saturating_duration_since(at)),
        execution: if on {
            power
                .execution_state()
                .map_err(|error| format!("{error:#}"))
        } else {
            Ok(0)
        },
        input: if on && !streaming {
            power.input()
        } else {
            Input::Failed("not read".into())
        },
        slept: if on {
            power.slept().unwrap_or_else(|error| {
                tracing::warn!("sleep after idle: {error:#}");
                Duration::ZERO
            })
        } else {
            Duration::ZERO
        },
    };
    let mut idle = locked(&h.wake.idle);
    let status = idle.machine.tick(&snapshot);
    let previous = idle.last.replace(status.clone());
    drop(idle);
    if previous
        .as_ref()
        .is_none_or(|previous| previous.state != status.state || changed(previous, &status))
    {
        log_status(&status);
    }
    if !status.sleep {
        return;
    }
    let while_suspended = snapshot
        .settings
        .as_ref()
        .is_ok_and(|settings| settings.while_suspended);
    // A launch or teardown holds this; no stream may start while the PC goes
    // down. The session lock is not taken under it: an action holds that one
    // while it waits for this.
    let Ok(_transition) = h.launch_transition.try_lock() else {
        tracing::info!("sleep after idle: a launch or teardown is running; checking again later");
        return;
    };
    let (streaming, suspended) = holding(h);
    if streaming || (suspended && !while_suspended) || updating(h) {
        tracing::info!("sleep after idle: a stream or update started; not sleeping");
        return;
    }
    locked(&h.wake.idle).machine.attempted(now);
    tracing::warn!(
        minutes = status.minutes,
        "sleep after idle: idle for {} minutes; putting the PC to sleep",
        status.minutes
    );
    let result = power.sleep();
    if let Err(error) = &result {
        tracing::error!("sleep after idle: the PC did not sleep: {error:#}");
    } else {
        tracing::info!("sleep after idle: the PC resumed");
    }
    locked(&h.wake.idle).last_sleep = Some((
        SystemTime::now(),
        result.err().map(|error| format!("{error:#}")),
    ));
}

/// A new blocking or unsupported reason; an armed countdown's reason changes
/// on its own.
fn changed(previous: &Status, status: &Status) -> bool {
    status.state != api::IdleSleepState::Armed && previous.reason != status.reason
}

fn log_status(status: &Status) {
    let reason = status.reason.as_deref().unwrap_or("");
    match status.state {
        api::IdleSleepState::Unsupported => {
            tracing::warn!("sleep after idle is unsupported: {reason}")
        }
        api::IdleSleepState::Blocked => tracing::info!("sleep after idle is blocked: {reason}"),
        api::IdleSleepState::Armed => tracing::info!(
            minutes = status.minutes,
            "sleep after idle is armed: the PC sleeps after {} idle minutes",
            status.minutes
        ),
        _ => tracing::info!("sleep after idle is off"),
    }
}

/// One reading of the adapters, the NIC settings and the power facts.
struct Reading {
    adapters: Vec<Adapter>,
    nics: Result<NicReport, String>,
    facts: Result<SystemFacts, String>,
}

fn read(h: &Host, refresh: bool) -> Result<Reading> {
    let power = h.wake.power();
    let adapters = power.adapters().context("reading the network adapters")?;
    let nics = h.wake.nics(&*power, refresh);
    let facts = power.system_facts().map_err(|error| format!("{error:#}"));
    Ok(Reading {
        adapters,
        nics,
        facts,
    })
}

fn wake_of(h: &Host, reading: &Reading, local: Option<IpAddr>) -> api::Wake {
    let facts = reading.facts.as_ref().ok();
    // Loopback gets zeros in /serverinfo, so no primary here either.
    let local = local
        .map(|local| local.to_canonical())
        .filter(|local| !local.is_loopback());
    let (primary, targets) = readiness::targets(
        &reading.adapters,
        reading.nics.as_ref().map_err(String::as_str),
        facts.is_some_and(|facts| facts.modern_standby),
        local,
    );
    let mut wake = api::Wake {
        revision: 0,
        primary,
        targets,
        system: readiness::system(facts),
        idle_sleep: status(h).api(),
    };
    wake.revision = h.wake.revision(&wake);
    wake
}

/// `GET /wake` for a request that reached the PC at `local`.
pub fn readiness(h: &Host, refresh: bool, local: Option<IpAddr>) -> Result<api::Wake> {
    Ok(wake_of(h, &read(h, refresh)?, local))
}

/// Why the console's fix was refused (409).
#[derive(Debug, PartialEq, Eq)]
pub enum FixRefusal {
    Streaming,
    Running,
}

impl std::fmt::Display for FixRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Streaming => {
                "a stream is active or starting; the network drops while adapters restart, so end it first"
            }
            Self::Running => "the Wake-on-LAN fix is already running",
        })
    }
}

/// Why the console's fix would be refused now, before reading the adapters;
/// [`request_fix`] checks again.
pub fn fix_refusal(h: &Host) -> Option<FixRefusal> {
    if locked(&h.wake.fix).running {
        return Some(FixRefusal::Running);
    }
    refusal_now(h)
}

/// The changes the console's fix would make now, from a fresh reading.
pub fn fix_plan(h: &Host, fast_startup_off: bool) -> Result<FixPlan> {
    let reading = read(h, true)?;
    let nics = reading
        .nics
        .map_err(anyhow::Error::msg)
        .context("reading the NIC wake settings")?;
    let facts = reading
        .facts
        .map_err(anyhow::Error::msg)
        .context("reading the power capabilities")?;
    Ok(FixPlan::new(
        &reading.adapters,
        &nics,
        &facts,
        fast_startup_off,
    ))
}

/// Start the console's fix: after [`FIX_DELAY`], with no stream starting
/// meanwhile, `--apply-wake-settings` applies `plan` (which it reads again).
/// The result is in [`console_status`].
pub fn request_fix(h: &Arc<Host>, plan: &FixPlan) -> Result<(), FixRefusal> {
    {
        let mut fix = locked(&h.wake.fix);
        if fix.running {
            return Err(FixRefusal::Running);
        }
        if holding(h).0 {
            return Err(FixRefusal::Streaming);
        }
        fix.running = true;
    }
    tracing::warn!(
        changes = ?plan.changes(),
        "Wake-on-LAN: applying the fix the console asked for"
    );
    let h = h.clone();
    let fast_startup_off = plan.fast_startup_off;
    let started = std::thread::Builder::new().name("wake-fix".into()).spawn({
        let h = h.clone();
        move || {
            std::thread::sleep(FIX_DELAY);
            let result = run_fix(&h, fast_startup_off);
            finish_fix(&h, result);
        }
    });
    if let Err(error) = started {
        finish_fix(&h, Err(format!("starting the fix: {error}")));
    }
    Ok(())
}

fn run_fix(h: &Host, fast_startup_off: bool) -> Result<Applied, String> {
    // No launch starts while adapters restart; the session lock is not taken
    // under this (see `tick_at`).
    let _transition = locked(&h.launch_transition);
    if let Some(refusal) = refusal_now(h) {
        return Err(format!("not applied: {refusal}"));
    }
    h.wake
        .power()
        .run_fix(fast_startup_off)
        .map_err(|error| format!("{error:#}"))
}

/// A stream that started between the POST and the fix.
fn refusal_now(h: &Host) -> Option<FixRefusal> {
    holding(h).0.then_some(FixRefusal::Streaming)
}

fn finish_fix(h: &Host, result: Result<Applied, String>) {
    match &result {
        Ok(applied) if applied.errors.is_empty() => tracing::info!(
            changes = ?applied.changes,
            restarted = ?applied.restarted,
            "Wake-on-LAN: the fix is applied"
        ),
        Ok(applied) => tracing::error!(
            changes = ?applied.changes,
            errors = ?applied.errors,
            "Wake-on-LAN: the fix failed in part"
        ),
        Err(error) => tracing::error!("Wake-on-LAN: the fix failed: {error}"),
    }
    // What the adapters report now is new.
    *locked(&h.wake.nics) = None;
    let mut fix = locked(&h.wake.fix);
    fix.running = false;
    fix.last = Some((SystemTime::now(), result));
}

fn unix(at: SystemTime) -> u64 {
    at.duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// `GET /api/power`: readiness as `/wake` has it, the changes a fix would
/// make, sleep after idle with its countdown, and the last sleep and fix.
pub fn console_status(h: &Host, refresh: bool, local: Option<IpAddr>) -> Result<Value> {
    let reading = read(h, refresh)?;
    let wake = wake_of(h, &reading, local);
    let changes = match (&reading.nics, &reading.facts) {
        (Ok(nics), Ok(facts)) => Ok(FixPlan::new(&reading.adapters, nics, facts, false).changes()),
        (Err(error), _) | (_, Err(error)) => Err(error.clone()),
    };
    let fast_startup = reading
        .facts
        .as_ref()
        .is_ok_and(SystemFacts::fast_startup_active);
    let idle = status(h);
    let (last_sleep, last_fix, running) = {
        let last_sleep = locked(&h.wake.idle).last_sleep.clone();
        let fix = locked(&h.wake.fix);
        (last_sleep, fix.last.clone(), fix.running)
    };
    Ok(json!({
        "status": true,
        "wake": wake,
        "idle_sleep": {
            "state": idle.state,
            "minutes": idle.minutes,
            "reason": idle.reason,
            "remaining_seconds": idle.remaining.map(|left| left.as_secs()),
        },
        "last_sleep": last_sleep.map(|(at, error)| json!({"at": unix(at), "error": error})),
        "fix": {
            "running": running,
            "changes": changes.as_ref().ok(),
            "error": changes.as_ref().err(),
            "fast_startup_off_offered": fast_startup,
            "last": last_fix.map(|(at, result)| match result {
                Ok(applied) => json!({
                    "at": unix(at),
                    "changes": applied.changes,
                    "restarted": applied.restarted,
                    "errors": applied.errors,
                }),
                Err(error) => json!({"at": unix(at), "errors": [error]}),
            }),
        },
    }))
}

/// The `wake` field of `--diagnostics`.
pub fn diagnostics() -> Value {
    let text = |error: anyhow::Error| format!("{error:#}");
    let power = Windows;
    let adapters = power.adapters().map_err(text);
    let nics = power.read_nics().map_err(text);
    let facts = power.system_facts().map_err(text);
    let execution = power.execution_state().map_err(text);
    let targets = adapters.as_ref().ok().map(|adapters| {
        readiness::targets(
            adapters,
            nics.as_ref().map_err(String::as_str),
            facts.as_ref().is_ok_and(|facts| facts.modern_standby),
            None,
        )
        .1
    });
    let changes = match (&adapters, &nics, &facts) {
        (Ok(adapters), Ok(nics), Ok(facts)) => {
            Some(FixPlan::new(adapters, nics, facts, false).changes())
        }
        _ => None,
    };
    json!({
        "adapters": adapters,
        "targets": targets,
        "nic_script_error": nics.as_ref().err(),
        "system": readiness::system(facts.as_ref().ok()),
        "power_facts": facts,
        "kept_awake": execution.map(idle::kept_awake),
        "fix": changes,
    })
}

#[cfg(test)]
pub(crate) mod fake {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A PC as the tests need it; the defaults are the `/wake` fixture's PC.
    pub struct Fake {
        pub adapters: Mutex<Result<Vec<Adapter>, String>>,
        pub nics: Mutex<Result<NicReport, String>>,
        pub facts: Mutex<Result<SystemFacts, String>>,
        pub execution: Mutex<u32>,
        pub input: Mutex<Input>,
        pub nic_reads: AtomicUsize,
        pub inputs: AtomicUsize,
        pub sleeps: AtomicUsize,
        /// What `apply_nics` was given.
        pub plans: Mutex<Vec<FixPlan>>,
        /// `run_fix`'s `fast_startup_off` for each run.
        pub fixes: Mutex<Vec<bool>>,
    }

    pub const WIRED: &str = "{875511C7-0EF5-45F7-876C-3BE50726AC42}";
    pub const WIFI: &str = "{A5234352-A9A6-4D45-8B0B-2A1CA4957AF2}";

    impl Fake {
        pub fn new() -> Arc<Self> {
            let adapters = serde_json::from_value(json!([
                {
                    "index": 7, "guid": WIRED, "name": "Ethernet",
                    "mac": "02:7C:16:B4:13:4F", "permanent": "02:7C:16:B4:13:4F",
                    "hardware": true, "connector": true, "medium": "wired", "connected": true,
                    "metric": 20, "ipv4": [["192.168.4.20", 24]], "addresses": ["192.168.4.20"]
                },
                {
                    "index": 36, "guid": "{9EDBCD36-5287-4EAC-9C2D-BE5640B6FF4C}",
                    "name": "vEthernet (WSL)", "mac": "00:15:5D:55:02:28",
                    "hardware": false, "connector": false, "medium": "wired", "connected": true,
                    "metric": 5000, "ipv4": [["172.30.112.1", 20]], "addresses": ["172.30.112.1"]
                },
                {
                    "index": 12, "guid": WIFI, "name": "WLAN", "mac": "02:E0:4C:68:00:01",
                    "hardware": true, "connector": true, "medium": "wifi", "connected": false,
                    "metric": 25
                }
            ]))
            .unwrap();
            let nics = NicReport::parse(
                &json!({"adapters": [
                    {
                        "guid": WIRED, "pnp": "PCI\\VEN_10EC&DEV_8125\\01", "name": "Ethernet",
                        "wake_enable": true, "magic_packet_only": false,
                        "keywords": {"*WakeOnMagicPacket": "1"}, "errors": []
                    },
                    {
                        "guid": WIFI, "pnp": "PCI\\VEN_8086&DEV_2725\\01", "name": "WLAN",
                        "wake_enable": false, "magic_packet_only": null,
                        "keywords": {"*WakeOnMagicPacket": "0"}, "errors": []
                    }
                ]})
                .to_string(),
            )
            .unwrap();
            Arc::new(Self {
                adapters: Mutex::new(Ok(adapters)),
                nics: Mutex::new(Ok(nics)),
                facts: Mutex::new(Ok(SystemFacts {
                    s3: true,
                    modern_standby: false,
                    hibernate: false,
                    hiberboot: Some(true),
                })),
                execution: Mutex::new(0),
                input: Mutex::new(Input::Idle(Duration::from_secs(24 * 3600))),
                nic_reads: AtomicUsize::new(0),
                inputs: AtomicUsize::new(0),
                sleeps: AtomicUsize::new(0),
                plans: Mutex::default(),
                fixes: Mutex::default(),
            })
        }
        /// Install it in `h`.
        pub fn on(self: &Arc<Self>, h: &Host) -> Arc<Self> {
            h.wake.set_power(self.clone());
            self.clone()
        }
        pub fn sleeps(&self) -> usize {
            self.sleeps.load(Ordering::SeqCst)
        }
    }

    impl PowerSystem for Fake {
        fn adapters(&self) -> Result<Vec<Adapter>> {
            locked(&self.adapters).clone().map_err(anyhow::Error::msg)
        }
        fn read_nics(&self) -> Result<NicReport> {
            self.nic_reads.fetch_add(1, Ordering::SeqCst);
            locked(&self.nics).clone().map_err(anyhow::Error::msg)
        }
        fn system_facts(&self) -> Result<SystemFacts> {
            locked(&self.facts).clone().map_err(anyhow::Error::msg)
        }
        fn execution_state(&self) -> Result<u32> {
            Ok(*locked(&self.execution))
        }
        fn slept(&self) -> Result<Duration> {
            Ok(Duration::ZERO)
        }
        fn input(&self) -> Input {
            self.inputs.fetch_add(1, Ordering::SeqCst);
            locked(&self.input).clone()
        }
        fn sleep(&self) -> Result<()> {
            self.sleeps.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        fn apply_nics(&self, plan: &FixPlan) -> Result<ApplyReport> {
            locked(&self.plans).push(plan.clone());
            Ok(ApplyReport {
                restarted: plan
                    .adapters
                    .iter()
                    .filter(|fix| !fix.keywords.is_empty())
                    .map(|fix| fix.guid.clone())
                    .collect(),
                errors: Vec::new(),
            })
        }
        fn run_fix(&self, fast_startup_off: bool) -> Result<Applied> {
            locked(&self.fixes).push(fast_startup_off);
            apply_settings(self, fast_startup_off)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{fake::*, *};
    use crate::state::test_support::Fixture;
    use butterpollo_core::{
        session::Role,
        wake::{AdapterFix, idle::ES_SYSTEM_REQUIRED},
    };
    use std::{collections::BTreeMap, sync::atomic::Ordering};

    const MINUTE: Duration = Duration::from_secs(60);

    fn configure(f: &Fixture, values: &[(&str, &str)]) {
        let mut config = f.host.config.write().unwrap();
        for (key, value) in values {
            config.values.insert((*key).into(), (*value).into());
        }
    }
    /// A host with sleep after idle at 10 minutes on the fake PC.
    fn armed() -> (Fixture, Arc<Fake>, Instant) {
        let f = Fixture::new();
        let start = Instant::now();
        let fake = Fake::new().on(&f.host);
        configure(&f, &[("sleep_after_idle_minutes", "10")]);
        (f, fake, start)
    }

    #[test]
    fn the_default_config_never_sleeps_and_asks_windows_nothing() {
        let f = Fixture::new();
        let fake = Fake::new().on(&f.host);
        let start = Instant::now();
        for hours in 1..5 {
            tick_at(&f.host, start + hours * 60 * MINUTE);
        }
        assert_eq!(fake.sleeps(), 0);
        assert_eq!(fake.inputs.load(Ordering::SeqCst), 0);
        assert_eq!(status(&f.host).state, api::IdleSleepState::Off);
    }

    #[test]
    fn it_sleeps_exactly_once_when_due() {
        let (f, fake, start) = armed();
        tick_at(&f.host, start + MINUTE);
        assert_eq!(fake.sleeps(), 0);
        let armed = status(&f.host);
        assert_eq!(armed.state, api::IdleSleepState::Armed);
        assert!(armed.remaining.unwrap() <= 9 * MINUTE);
        tick_at(&f.host, start + 11 * MINUTE);
        assert_eq!(fake.sleeps(), 1);
        // The next attempt waits a full interval.
        tick_at(&f.host, start + 12 * MINUTE);
        tick_at(&f.host, start + 20 * MINUTE);
        assert_eq!(fake.sleeps(), 1);
        tick_at(&f.host, start + 22 * MINUTE);
        assert_eq!(fake.sleeps(), 2);
        assert_eq!(
            locked(&f.host.wake.idle).last_sleep.as_ref().unwrap().1,
            None
        );
    }

    #[test]
    fn an_active_or_pending_stream_blocks_sleep() {
        let (f, fake, start) = armed();
        let session = f.session(f.client(0), Role::Stream);
        tick_at(&f.host, start + 11 * MINUTE);
        assert_eq!(fake.sleeps(), 0);
        assert_eq!(status(&f.host).state, api::IdleSleepState::Blocked);
        // A stream skips the helper.
        assert_eq!(fake.inputs.load(Ordering::SeqCst), 0);
        f.host.sessions.lock().unwrap().active.clear();
        drop(session);
        let launch = f.launch(f.client(0), Role::Stream);
        f.host
            .sessions
            .lock()
            .unwrap()
            .pending
            .insert(launch.id.clone(), launch);
        tick_at(&f.host, start + 30 * MINUTE);
        assert_eq!(fake.sleeps(), 0);
        f.host.sessions.lock().unwrap().pending.clear();
        // The quiet interval counts from the stream's end.
        tick_at(&f.host, start + 35 * MINUTE);
        assert_eq!(fake.sleeps(), 0);
        tick_at(&f.host, start + 41 * MINUTE);
        assert_eq!(fake.sleeps(), 1);
    }

    #[test]
    fn a_launched_app_blocks_sleep_unless_sleep_while_suspended() {
        let (f, fake, start) = armed();
        let app = f.host.apps.read().unwrap()[0].clone();
        *f.host.current_app.lock().unwrap() =
            Some(crate::process::RunningApp::with_environment(&app, BTreeMap::new()).unwrap());
        tick_at(&f.host, start + 11 * MINUTE);
        assert_eq!(fake.sleeps(), 0);
        let blocked = status(&f.host);
        assert_eq!(blocked.state, api::IdleSleepState::Blocked);
        assert!(blocked.reason.unwrap().contains("launched app"));
        configure(&f, &[("sleep_while_suspended", "true")]);
        tick_at(&f.host, start + 22 * MINUTE);
        assert_eq!(fake.sleeps(), 1);
        *f.host.current_app.lock().unwrap() = None;
    }

    #[test]
    fn every_other_blocking_condition_keeps_the_pc_awake() {
        type Setup = fn(&Fixture, &Fake);
        let cases: [(&str, Setup); 7] = [
            ("a power request", |_, fake| {
                *locked(&fake.execution) = ES_SYSTEM_REQUIRED
            }),
            ("a failed helper", |_, fake| {
                *locked(&fake.input) = Input::Failed("helper".into())
            }),
            ("recent local input", |_, fake| {
                *locked(&fake.input) = Input::Idle(MINUTE)
            }),
            ("an update", |f, _| {
                f.host.updates.lock().unwrap()["phase"] = json!("downloading")
            }),
            ("Modern Standby", |_, fake| {
                locked(&fake.facts).as_mut().unwrap().modern_standby = true
            }),
            ("unreadable power facts", |_, fake| {
                *locked(&fake.facts) = Err("denied".into())
            }),
            ("a bad setting", |f, _| {
                configure(f, &[("sleep_while_suspended", "maybe")])
            }),
        ];
        for (name, setup) in cases {
            let (f, fake, start) = armed();
            setup(&f, &fake);
            tick_at(&f.host, start + 11 * MINUTE);
            assert_eq!(fake.sleeps(), 0, "{name}");
        }
        // A console request restarts the interval.
        let (f, fake, start) = armed();
        tick_at(&f.host, start + 9 * MINUTE);
        *locked(&f.host.wake.console) = Some(start + 9 * MINUTE);
        tick_at(&f.host, start + 11 * MINUTE);
        assert_eq!(fake.sleeps(), 0);
        tick_at(&f.host, start + 20 * MINUTE);
        assert_eq!(fake.sleeps(), 1);
        // A launch in flight: the next tick looks again.
        let (f, fake, start) = armed();
        let transition = f.host.launch_transition.lock().unwrap();
        tick_at(&f.host, start + 11 * MINUTE);
        assert_eq!(fake.sleeps(), 0);
        drop(transition);
        tick_at(&f.host, start + 11 * MINUTE + idle::TICK);
        assert_eq!(fake.sleeps(), 1);
    }

    #[test]
    fn the_idle_helpers_exit_code_is_seconds_or_an_error() {
        assert_eq!(helper_input(0), Input::Idle(Duration::ZERO));
        assert_eq!(
            helper_input(604_800),
            Input::Idle(Duration::from_secs(604_800))
        );
        let Input::Failed(error) = helper_input(IDLE_FAILED | 5) else {
            panic!("a failure is not idle time");
        };
        assert!(error.starts_with("the idle helper failed"), "{error}");
        assert!(matches!(helper_input(IDLE_FAILED | 1), Input::Failed(_)));
    }

    #[test]
    fn apply_settings_gives_the_script_the_exact_plan() {
        let fake = Fake::new();
        let applied = apply_settings(&*fake, true).unwrap();
        // Wired adapters only; Fast Startup is on but inactive without hibernate.
        let expected = FixPlan {
            adapters: vec![AdapterFix {
                guid: WIRED.into(),
                pnp: "PCI\\VEN_10EC&DEV_8125\\01".into(),
                name: "Ethernet".into(),
                keywords: BTreeMap::new(),
                wake_enable: false,
                magic_packet_only: true,
            }],
            fast_startup_off: false,
        };
        assert_eq!(*locked(&fake.plans), std::slice::from_ref(&expected));
        assert_eq!(applied.changes, expected.changes());
        assert!(applied.restarted.is_empty() && applied.errors.is_empty());
        // Hibernate on: Fast Startup is active, and the keyword fix restarts the adapter.
        let fake = Fake::new();
        locked(&fake.facts).as_mut().unwrap().hibernate = true;
        if let Ok(nics) = locked(&fake.nics).as_mut() {
            nics.adapters[0]
                .keywords
                .insert("*WAKEONMAGICPACKET".into(), "0".into());
        }
        let applied = apply_settings(&*fake, true).unwrap();
        let plan = locked(&fake.plans)[0].clone();
        assert!(plan.fast_startup_off);
        assert_eq!(
            plan.adapters[0].keywords,
            BTreeMap::from([("*WakeOnMagicPacket".to_string(), "1".to_string())])
        );
        assert_eq!(applied.restarted, [WIRED]);
        assert!(
            applied
                .changes
                .contains(&"Turn off Fast Startup".to_string())
        );
        // Without --fast-startup-off it stays.
        let fake = Fake::new();
        locked(&fake.facts).as_mut().unwrap().hibernate = true;
        apply_settings(&*fake, false).unwrap();
        assert!(!locked(&fake.plans)[0].fast_startup_off);
        // Nothing to change: the script is not run.
        let fake = Fake::new();
        if let Ok(nics) = locked(&fake.nics).as_mut() {
            nics.adapters[0].magic_packet_only = Some(true);
        }
        let applied = apply_settings(&*fake, false).unwrap();
        assert!(applied.changes.is_empty());
        assert!(locked(&fake.plans).is_empty());
        // A failed read is an error, never an empty plan.
        let fake = Fake::new();
        *locked(&fake.nics) = Err("PowerShell is blocked".into());
        assert!(apply_settings(&*fake, false).is_err());
    }

    #[test]
    fn the_revision_rises_only_when_the_readiness_changes() {
        let f = Fixture::new();
        let fake = Fake::new().on(&f.host);
        let local = Some("192.168.4.20".parse().unwrap());
        let first = readiness(&f.host, false, local).unwrap();
        assert_eq!(first.revision, 1);
        assert_eq!(readiness(&f.host, false, None).unwrap().revision, 1);
        assert_eq!(fake.nic_reads.load(Ordering::SeqCst), 1, "cached");
        if let Ok(nics) = locked(&fake.nics).as_mut() {
            nics.adapters[0].magic_packet_only = Some(true);
        }
        assert_eq!(readiness(&f.host, true, local).unwrap().revision, 2);
        configure(&f, &[("sleep_after_idle_minutes", "30")]);
        assert_eq!(readiness(&f.host, false, local).unwrap().revision, 3);
    }

    #[test]
    fn loopback_and_unreadable_nics_are_reported_not_hidden() {
        let f = Fixture::new();
        let fake = Fake::new().on(&f.host);
        let wake = readiness(&f.host, false, Some("127.0.0.1".parse().unwrap())).unwrap();
        assert_eq!(wake.primary, None);
        assert!(wake.targets.iter().all(|target| !target.route));
        *locked(&fake.nics) = Err("PowerShell is blocked".into());
        let wake = readiness(&f.host, true, None).unwrap();
        assert_eq!(wake.targets.len(), 2, "virtual adapters are never listed");
        assert_eq!(wake.targets[0].state, api::TargetState::Unknown);
        *locked(&fake.adapters) = Err("IP Helper failed".into());
        assert!(readiness(&f.host, false, None).is_err());
    }
}
