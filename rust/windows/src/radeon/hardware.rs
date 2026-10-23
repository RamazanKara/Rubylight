//! Hardware tests for the write layer (plan A5 step 2). They change real driver settings, so they
//! are `#[ignore]`d and only run by hand on a PC whose installed host is idle:
//!
//! ```text
//! cargo test -p butterpollo-windows --lib radeon::hardware -- --ignored --nocapture --test-threads=1
//! ```
//!
//! Every test checks the host is idle first and puts every setting back in a guard that also runs
//! when the test panics. The results are printed and written as JSON next to the repository
//! (`RADEON_HW_OUT`, default `C:\src`).
//!
//! Reads and writes go through `helper()`: a fresh process per call, exactly as the host does it,
//! because a long-lived process's ADLX view goes stale after any other process writes (see
//! `radeon::helper`). Only `session_churn_is_stable` and `measure_view_refresh` use the in-process
//! `AdlxBackend`, and say why.
use super::{
    AdlxBackend, Change, Controller, RadeonBackend, Scope,
    control::JOURNAL_FILE,
    helper::HelperBackend,
    model::{Kind, Setting, State, Value, enum_items},
    probe,
    worker::{Phase, Timing, Worker},
};
use serde::Serialize;
use std::{
    io::{Read, Write},
    net::TcpStream,
    path::{Path, PathBuf},
    process::Command,
    sync::{Mutex, MutexGuard},
    time::Duration,
};

/// The tests share one GPU, so they run one at a time even without `--test-threads=1`.
static HARDWARE: Mutex<()> = Mutex::new(());
fn exclusive() -> MutexGuard<'static, ()> {
    HARDWARE.lock().unwrap_or_else(|e| e.into_inner())
}

/// The installed host must have no stream, pending or running. No host at all counts as idle.
fn assert_host_idle() {
    let Ok(mut stream) = TcpStream::connect_timeout(
        &"127.0.0.1:47989".parse().expect("address"),
        Duration::from_secs(2),
    ) else {
        return;
    };
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("timeout");
    stream
        .write_all(b"GET /serverinfo HTTP/1.0\r\nHost: 127.0.0.1\r\n\r\n")
        .expect("request");
    let mut body = String::new();
    let _ = stream.read_to_string(&mut body);
    for tag in ["RustHostSessionCount", "RustHostPendingSessionCount"] {
        let count = body
            .split(&format!("<{tag}>"))
            .nth(1)
            .and_then(|rest| rest.split('<').next())
            .and_then(|n| n.trim().parse::<u32>().ok());
        assert_eq!(count, Some(0), "the installed host is not idle ({tag})");
    }
}

/// The backend the host uses: every call runs in a new process (this test binary, acting as the
/// helper), so every read sees the driver as it is now.
fn helper() -> HelperBackend {
    HelperBackend::with_command(
        std::env::current_exe().expect("test binary"),
        [
            "--exact",
            "radeon::hardware::helper_child",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ]
        .map(String::from)
        .to_vec(),
    )
}
#[test]
#[ignore = "child of the hardware tests"]
fn helper_child() {
    // The cross-account test asks every helper to say which account it runs as: it creates
    // `helper-accounts.log` in the exchange folder, and each helper appends a line.
    let log = xacct_dir().join("helper-accounts.log");
    if log.exists() {
        use std::io::Write;
        let line = format!(
            "pid {} sid {} system {}\n",
            std::process::id(),
            crate::process::own_sid().unwrap_or_else(|e| format!("unknown ({e:#})")),
            crate::process::is_system()
        );
        if let Ok(mut file) = std::fs::OpenOptions::new().append(true).open(&log) {
            let _ = file.write_all(line.as_bytes());
        }
    }
    assert_eq!(super::helper::run(), 0);
}

fn first_gpu() -> i32 {
    probe().expect("probe").gpus.first().expect("an AMD GPU").id
}

fn out_path(name: &str) -> PathBuf {
    let dir = std::env::var("RADEON_HW_OUT").unwrap_or_else(|_| r"C:\src".into());
    Path::new(&dir).join(name)
}

fn scratch_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("radeon-hw-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

/// Writes every setting back to its starting value on drop, panic or not. A setting that will
/// not go back is named loudly with the value it must return to. It reads and writes through
/// `helper()`, so it sees the driver as it is now and never skips a write because this process
/// believes the value is already right.
struct Restore {
    gpu: i32,
    originals: Vec<(String, Value)>,
}
impl Restore {
    fn new(gpu: i32, start: &[Setting]) -> Self {
        snapshot(start);
        Self {
            gpu,
            originals: start
                .iter()
                .filter(|s| s.kind != Kind::Action)
                .filter_map(|s| Some((s.id.to_string(), s.value.clone()?)))
                .collect(),
        }
    }
    fn differing(&self) -> Vec<(String, Value, Option<Value>)> {
        let Ok(now) = helper().read(self.gpu) else {
            return self
                .originals
                .iter()
                .map(|(id, value)| (id.clone(), value.clone(), None))
                .collect();
        };
        self.originals
            .iter()
            .filter_map(|(id, original)| {
                let current = now.iter().find(|s| s.id == id)?.value.clone();
                (current.as_ref() != Some(original))
                    .then(|| (id.clone(), original.clone(), current))
            })
            .collect()
    }
    /// Write back whatever differs, up to three rounds. Returns what would not go back.
    fn put_back(&self) -> Vec<(String, Value, Option<Value>)> {
        for _ in 0..3 {
            let differing = self.differing();
            if differing.is_empty() {
                return vec![];
            }
            let Ok(now) = helper().read(self.gpu) else {
                continue;
            };
            // Fields before their `.enabled` toggles, so a field of a feature that is off can
            // borrow the toggle for its write.
            let mut order = differing;
            order.sort_by_key(|(id, _, _)| id.ends_with(".enabled"));
            for (id, original, _) in &order {
                let toggle = super::model::feature_toggle(id);
                let borrowed = toggle.filter(|toggle| {
                    now.iter()
                        .find(|s| s.id == toggle.as_str())
                        .and_then(|s| s.value.as_ref())
                        == Some(&Value::Bool(false))
                });
                if let Some(toggle) = &borrowed {
                    let _ = helper().write(self.gpu, toggle, &Value::Bool(true));
                }
                // vsync.enabled follows vsync.mode and has no setter; the write error is expected.
                let _ = helper().write(self.gpu, id, original);
                if let Some(toggle) = &borrowed {
                    let _ = helper().write(self.gpu, toggle, &Value::Bool(false));
                }
            }
        }
        self.differing()
    }
}
impl Drop for Restore {
    fn drop(&mut self) {
        let stuck = self.put_back();
        if stuck.is_empty() {
            return;
        }
        for (id, original, current) in &stuck {
            eprintln!(
                "RESTORE FAILED: {id} must return to {} and is {}",
                original.display(),
                current.as_ref().map_or("unreadable".into(), Value::display)
            );
        }
        if !std::thread::panicking() {
            panic!("could not restore {} setting(s)", stuck.len());
        }
    }
}

/// A different value the validator accepts, close to the current one.
fn alternative(setting: &Setting) -> Option<Value> {
    match (setting.kind, setting.value.as_ref()?) {
        (Kind::Toggle, Value::Bool(on)) => Some(Value::Bool(!on)),
        (Kind::Integer, Value::Int(value)) => {
            let range = setting.range?;
            [value + range.step, value - range.step, range.min, range.max]
                .into_iter()
                .find(|c| {
                    c != value
                        && *c >= range.min
                        && *c <= range.max
                        && (c - range.min) % range.step == 0
                })
                .map(Value::Int)
        }
        (Kind::Choice, Value::Choice(current)) => setting
            .options
            .iter()
            .find(|o| &o.value != current)
            .map(|o| Value::Choice(o.value.clone())),
        _ => None,
    }
}

/// The requests to try for one setting. A setting of a feature that is off may only take a value
/// while the same request turns the feature on, so it is tried alone first and then with the
/// feature's toggle; the matrix records which settings need that.
fn attempts(setting: &Setting, start: &[Setting], tried: &Value) -> Vec<Vec<Change>> {
    let target = Change {
        id: setting.id.into(),
        value: tried.clone(),
    };
    let group_enable = setting
        .id
        .split_once('.')
        .filter(|(_, field)| *field != "enabled")
        .map(|(group, _)| format!("{group}.enabled"))
        .filter(|enable| {
            start.iter().any(|s| {
                s.id == enable
                    && s.state == State::Configurable
                    && s.value == Some(Value::Bool(false))
            })
        });
    let mut attempts = vec![vec![target.clone()]];
    if let Some(enable) = group_enable {
        attempts.push(vec![
            Change {
                id: enable,
                value: Value::Bool(true),
            },
            target,
        ]);
    }
    attempts
}

/// Every setting's value, for the before and after comparison.
fn values(settings: &[Setting]) -> Vec<(&'static str, Option<Value>)> {
    settings.iter().map(|s| (s.id, s.value.clone())).collect()
}
fn diff(start: &[Setting], now: &[Setting]) -> Vec<String> {
    let (a, b) = (values(start), values(now));
    let mut out = vec![];
    for (id, was) in &a {
        let is = b
            .iter()
            .find(|(i, _)| i == id)
            .and_then(|(_, v)| v.as_ref());
        if was.as_ref() != is {
            out.push(format!("{id}: was {was:?}, now {is:?}"));
        }
    }
    out
}

#[derive(Serialize)]
struct Proof {
    id: &'static str,
    kind: Kind,
    /// "yes", "no" or "action".
    proven_writable: &'static str,
    original: Option<Value>,
    tried: Option<Value>,
    detail: String,
}

fn save<T: Serialize>(name: &str, value: &T) {
    let path = out_path(name);
    std::fs::write(&path, serde_json::to_vec_pretty(value).expect("json")).expect("write results");
    println!("results written to {}", path.display());
}

/// Starting values on disk before a test writes anything, so a run that dies hard (an access
/// violation does not run the guard) can be put back with `restore_snapshot`.
fn snapshot(start: &[Setting]) {
    let values: std::collections::BTreeMap<&str, &Value> = start
        .iter()
        .filter(|s| s.kind != Kind::Action)
        .filter_map(|s| Some((s.id, s.value.as_ref()?)))
        .collect();
    // A run that died before restoring leaves changed values in the next run's start, so the
    // snapshot it replaces is kept beside it.
    let path = out_path("rl-a5-snapshot.json");
    if let Ok(previous) = std::fs::read(&path)
        && previous != serde_json::to_vec_pretty(&values).expect("json")
    {
        let _ = std::fs::write(out_path("rl-a5-snapshot.prev.json"), previous);
    }
    save("rl-a5-snapshot.json", &values);
}

/// Read-only, and deliberately in this process: `AdlxBackend` opens an ADLX session per call, so
/// many in a row must stay stable.
#[test]
#[ignore = "talks to the driver"]
fn session_churn_is_stable() {
    let _exclusive = exclusive();
    let gpu = first_gpu();
    let first = AdlxBackend.read(gpu).expect("read");
    for round in 0..300 {
        let again = AdlxBackend
            .read(gpu)
            .unwrap_or_else(|e| panic!("round {round}: {e}"));
        assert!(diff(&first, &again).is_empty(), "round {round}");
    }
}

/// The path the native API takes (plan A3 brief 4): the worker thread owns the controller. One
/// real read through `Worker::panel`, one write of `anti_lag.enabled` (proven to round-trip) and
/// the restore of that session, with the driver checked after each step.
#[test]
#[ignore = "changes real driver settings"]
fn worker_writes_and_restores_anti_lag() {
    let _exclusive = exclusive();
    assert_host_idle();
    let gpu = first_gpu();
    let start = helper().read(gpu).expect("read");
    let _restore = Restore::new(gpu, &start);
    let dir = scratch_dir("worker");
    let worker = Worker::start(helper(), dir.join(JOURNAL_FILE), Timing::default());
    let until = std::time::Instant::now() + Duration::from_secs(60);
    while worker.status().phase == Phase::Recovering {
        assert!(
            std::time::Instant::now() < until,
            "the recovery never ended"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let panel = worker
        .panel(None, Duration::from_secs(60))
        .expect("panel")
        .expect("an AMD GPU");
    assert_eq!(panel.gpu.id, gpu);
    assert!(panel.pending.is_empty());
    println!(
        "gpu {} {} driver {}",
        panel.gpu.id, panel.gpu.name, panel.gpu.driver
    );
    let anti_lag = panel
        .settings
        .iter()
        .find(|s| s.id == "anti_lag.enabled")
        .expect("anti_lag.enabled");
    assert_eq!(anti_lag.state, State::Configurable);
    let original = anti_lag.value.clone().expect("a value");
    let Value::Bool(on) = original else {
        panic!("anti_lag.enabled is not a toggle");
    };
    let written = Value::Bool(!on);
    println!(
        "anti_lag.enabled: {} -> {}",
        original.display(),
        written.display()
    );
    let after = worker
        .apply(
            "hw-session",
            gpu,
            vec![Change {
                id: "anti_lag.enabled".into(),
                value: written.clone(),
            }],
        )
        .expect("apply");
    assert_eq!(
        after
            .iter()
            .find(|s| s.id == "anti_lag.enabled")
            .and_then(|s| s.value.clone()),
        Some(written.clone())
    );
    assert_eq!(
        helper().read_value(gpu, "anti_lag.enabled").expect("read"),
        written
    );
    let panel = worker
        .panel(None, Duration::from_secs(60))
        .expect("panel")
        .expect("an AMD GPU");
    let entry = panel
        .pending
        .iter()
        .find(|e| e.id == "anti_lag.enabled")
        .expect("the change is journaled");
    assert_eq!((&entry.original, &entry.written), (&original, &written));
    let report = worker.restore_session("hw-session").expect("restore");
    assert!(report.is_clean(), "{:?}", report.outcomes);
    assert_eq!(
        helper().read_value(gpu, "anti_lag.enabled").expect("read"),
        original
    );
    let moved = diff(&start, &helper().read(gpu).expect("read"));
    assert!(
        moved.is_empty(),
        "after the restore the settings differ: {moved:?}"
    );
    assert!(
        worker
            .panel(None, Duration::from_secs(60))
            .expect("panel")
            .expect("an AMD GPU")
            .pending
            .is_empty()
    );
    println!(
        "anti_lag.enabled written, read back and restored to {}",
        original.display()
    );
}

/// The per-game profiles of plan B5 (`butterpollo_core::radeon_profile`) against the real
/// driver: each named profile is written as the host writes it at launch (the stream's 60 fps
/// in place of `$stream_fps`), every value is read back, and the session's restore must leave
/// the driver as it was found.
#[test]
#[ignore = "changes real driver settings"]
fn game_profiles_apply_and_restore() {
    use butterpollo_core::radeon_profile::selected;
    let _exclusive = exclusive();
    assert_host_idle();
    let gpu = first_gpu();
    let start = helper().read(gpu).expect("read");
    let _restore = Restore::new(gpu, &start);
    let dir = scratch_dir("profiles");
    let worker = Worker::start(helper(), dir.join(JOURNAL_FILE), Timing::default());
    let until = std::time::Instant::now() + Duration::from_secs(60);
    while worker.status().phase == Phase::Recovering {
        assert!(
            std::time::Instant::now() < until,
            "the recovery never ended"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    for name in ["competitive", "smoothness", "quality"] {
        let extra = std::collections::BTreeMap::from([(
            "radeon-profile".to_owned(),
            serde_json::json!(name),
        )]);
        let profile = selected(&extra, Some(60))
            .expect("valid")
            .expect("a profile");
        let changes: Vec<Change> = profile
            .changes
            .iter()
            .map(|item| Change {
                id: item.id.clone(),
                value: match &item.value {
                    serde_json::Value::Bool(on) => Value::Bool(*on),
                    serde_json::Value::Number(n) => Value::Int(n.as_i64().expect("whole")),
                    serde_json::Value::String(text) => Value::Choice(text.clone()),
                    other => panic!("{other}"),
                },
            })
            .collect();
        let session = format!("hw-profile-{name}");
        let after = worker
            .apply(&session, gpu, changes.clone())
            .unwrap_or_else(|e| panic!("{name}: the driver refused the profile: {e}"));
        for change in &changes {
            let now = after
                .iter()
                .find(|s| s.id == change.id)
                .and_then(|s| s.value.clone());
            println!(
                "{name}: {} = {:?}",
                change.id,
                now.as_ref().map(Value::display)
            );
            assert_eq!(now.as_ref(), Some(&change.value), "{name}: {}", change.id);
            assert_eq!(
                helper().read_value(gpu, &change.id).expect("read"),
                change.value,
                "{name}: {} read in a new process",
                change.id
            );
        }
        let report = worker.restore_session(&session).expect("restore");
        assert!(report.is_clean(), "{name}: {:?}", report.outcomes);
        let moved = diff(&start, &helper().read(gpu).expect("read"));
        assert!(moved.is_empty(), "{name}: after the restore: {moved:?}");
        println!("{name}: applied, read back and restored");
    }
}

/// Safety net: write back every setting to the values in `RADEON_SNAPSHOT` (default the file
/// `snapshot` leaves behind).
#[test]
#[ignore = "changes real driver settings"]
fn restore_snapshot() {
    let _exclusive = exclusive();
    assert_host_idle();
    let path = std::env::var("RADEON_SNAPSHOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| out_path("rl-a5-snapshot.json"));
    let wanted: std::collections::BTreeMap<String, Value> =
        serde_json::from_slice(&std::fs::read(&path).expect("snapshot")).expect("snapshot json");
    let gpu = first_gpu();
    let restore = Restore {
        gpu,
        originals: wanted.into_iter().collect(),
    };
    println!("differing before: {:?}", restore.differing());
}

/// Every `configurable` setting: write a different valid value through the journal, read it back,
/// restore, and check nothing else moved. Ends with an empty probe diff.
#[test]
#[ignore = "changes real driver settings"]
fn prove_configurable_settings_round_trip() {
    let _exclusive = exclusive();
    assert_host_idle();
    let gpu = first_gpu();
    let start = helper().read(gpu).expect("read");
    let _restore = Restore::new(gpu, &start);
    snapshot(&start);
    let dir = scratch_dir("prove");
    let journal = dir.join(JOURNAL_FILE);
    let mut controller = Controller::new(helper(), journal.clone());
    let mut proofs = vec![];
    for setting in start.iter().filter(|s| s.state == State::Configurable) {
        if setting.kind == Kind::Action {
            proofs.push(Proof {
                id: setting.id,
                kind: setting.kind,
                proven_writable: "action",
                original: None,
                tried: None,
                detail: "a destructive action, not a setting; deliberately never written".into(),
            });
            continue;
        }
        assert_host_idle();
        let Some(tried) = alternative(setting) else {
            proofs.push(Proof {
                id: setting.id,
                kind: setting.kind,
                proven_writable: "no",
                original: setting.value.clone(),
                tried: None,
                detail: "no second valid value to write".into(),
            });
            continue;
        };
        let mut refusals: Vec<String> = vec![];
        let mut outcome = None;
        for changes in &attempts(setting, &start, &tried) {
            match controller.apply("prove", gpu, changes) {
                Ok(after) => {
                    let read_back = after
                        .iter()
                        .find(|s| s.id == setting.id)
                        .and_then(|s| s.value.clone());
                    let report = controller
                        .restore(Scope::Session("prove"))
                        .expect("restore");
                    assert!(
                        report.is_clean(),
                        "restoring {} failed: {:?}",
                        setting.id,
                        report.outcomes
                    );
                    let moved = diff(&start, &helper().read(gpu).expect("read"));
                    assert!(
                        moved.is_empty(),
                        "after restoring {} the settings differ: {moved:?}",
                        setting.id
                    );
                    let mut detail = format!(
                        "wrote {}, read back {}, restored {}",
                        tried.display(),
                        read_back.as_ref().map_or("nothing".into(), Value::display),
                        setting.value.as_ref().map_or("?".into(), Value::display)
                    );
                    if changes.len() > 1 {
                        detail.push_str(&format!(
                            "; only with {} on in the same request (alone: {})",
                            changes[0].id,
                            refusals.join("; ")
                        ));
                    }
                    outcome = Some(("yes", detail));
                    break;
                }
                Err(e) => {
                    let moved = diff(&start, &helper().read(gpu).expect("read"));
                    assert!(
                        moved.is_empty(),
                        "{} was refused ({e}) but the settings differ: {moved:?}",
                        setting.id
                    );
                    refusals.push(e.to_string());
                }
            }
        }
        let (proven_writable, detail) = outcome.unwrap_or(("no", refusals.join("; ")));
        println!("{:<40} {proven_writable:<6} {detail}", setting.id);
        proofs.push(Proof {
            id: setting.id,
            kind: setting.kind,
            proven_writable,
            original: setting.value.clone(),
            tried: Some(tried),
            detail,
        });
    }
    let journal = controller.journal().expect("journal");
    assert!(
        journal.entries.is_empty(),
        "journal left: {:?}",
        journal.entries
    );
    let end = helper().read(gpu).expect("read");
    let moved = diff(&start, &end);
    save("rl-a5-prove.json", &proofs);
    assert!(moved.is_empty(), "final probe diff is not empty: {moved:?}");
    println!(
        "final probe diff: empty ({} settings compared)",
        start.len()
    );
}

#[derive(Serialize)]
struct Exploration {
    id: &'static str,
    original: Option<Value>,
    /// Values the driver accepted and read back unchanged.
    round_trips: Vec<String>,
    /// Values the driver refused or kept another value for, with the answer.
    refused: Vec<String>,
    /// The current value could not be written back, so the host could never restore it.
    restorable: bool,
    promotable: bool,
}

/// The enum-backed `read_only` settings: ADLX cannot list the values a driver accepts, so each
/// SDK value is tried once and the setting goes back to its original after every attempt.
#[test]
#[ignore = "changes real driver settings"]
fn explore_enum_settings() {
    const CANDIDATES: &[&str] = &[
        "vsync.mode",
        "afmf.search_mode",
        "afmf.performance_mode",
        "afmf.fast_motion_response",
        "anti_aliasing.mode",
        "anti_aliasing.level",
        "anti_aliasing.method",
        "anisotropic_filtering.level",
        "tessellation.mode",
        "tessellation.level",
    ];
    let _exclusive = exclusive();
    assert_host_idle();
    let gpu = first_gpu();
    let start = helper().read(gpu).expect("read");
    let _restore = Restore::new(gpu, &start);
    snapshot(&start);
    // The AFMF modes answer ADLX_NOT_ACTIVE while AFMF is off; this run turns it on for a while.
    let afmf_on = std::env::var_os("RADEON_EXPLORE_AFMF_ON").is_some();
    let mut current = start.clone();
    if afmf_on {
        helper()
            .write(gpu, "afmf.enabled", &Value::Bool(true))
            .expect("enable AFMF");
        current = helper().read(gpu).expect("read");
    }
    let mut found = vec![];
    for id in CANDIDATES {
        let Some(setting) = current.iter().find(|s| s.id == *id) else {
            continue;
        };
        let (Some(original), Some(_)) = (setting.value.clone(), enum_items(id)) else {
            found.push(Exploration {
                id: setting.id,
                original: setting.value.clone(),
                round_trips: vec![],
                refused: vec!["not readable".into()],
                restorable: false,
                promotable: false,
            });
            continue;
        };
        let mut backend = helper();
        let mut result = Exploration {
            id: setting.id,
            original: Some(original.clone()),
            round_trips: vec![],
            refused: vec![],
            restorable: true,
            promotable: false,
        };
        // The host can only restore what it can write back.
        eprintln!("writing {id} = {} (its own value)", original.display());
        if let Err(e) = backend.write(gpu, id, &original) {
            result.restorable = false;
            result
                .refused
                .push(format!("original {}: {e}", original.display()));
            println!(
                "{id}: current value {} cannot be written back: {e}",
                original.display()
            );
            found.push(result);
            continue;
        }
        for option in setting
            .options
            .iter()
            .filter(|o| Value::Choice(o.value.clone()) != original)
        {
            assert_host_idle();
            let value = Value::Choice(option.value.clone());
            eprintln!("writing {id} = {}", value.display());
            match backend.write(gpu, id, &value) {
                Err(e) => result.refused.push(format!("{}: {e}", option.value)),
                Ok(()) => {
                    let now = backend
                        .read(gpu)
                        .ok()
                        .and_then(|s| s.into_iter().find(|s| s.id == *id)?.value);
                    if now.as_ref() == Some(&value) {
                        result.round_trips.push(option.value.clone());
                    } else {
                        result.refused.push(format!(
                            "{}: driver kept {}",
                            option.value,
                            now.as_ref().map_or("nothing".into(), Value::display)
                        ));
                    }
                }
            }
            backend.write(gpu, id, &original).unwrap_or_else(|e| {
                panic!("could not put {id} back to {}: {e}", original.display())
            });
            let back = backend
                .read(gpu)
                .ok()
                .and_then(|s| s.into_iter().find(|s| s.id == *id)?.value);
            assert_eq!(
                back.as_ref(),
                Some(&original),
                "{id} did not return to {}",
                original.display()
            );
        }
        result.promotable = result.restorable && !result.round_trips.is_empty();
        println!(
            "{id}: original {} round-trips {:?} refused {:?} promotable {}",
            original.display(),
            result.round_trips,
            result.refused,
            result.promotable
        );
        found.push(result);
    }
    if afmf_on {
        let original = start
            .iter()
            .find(|s| s.id == "afmf.enabled")
            .and_then(|s| s.value.clone())
            .expect("afmf.enabled");
        helper()
            .write(gpu, "afmf.enabled", &original)
            .expect("put AFMF back");
    }
    let moved = diff(&start, &helper().read(gpu).expect("read"));
    save(
        if afmf_on {
            "rl-a5-explore-afmf.json"
        } else {
            "rl-a5-explore.json"
        },
        &found,
    );
    assert!(moved.is_empty(), "probe diff after exploring: {moved:?}");
}

const CRASH_DIR: &str = "RADEON_CRASH_DIR";

/// A setting with no dependencies that is quick to see change.
fn crash_target(settings: &[Setting]) -> (&'static str, Value) {
    let wanted = std::env::var("RADEON_CRASH_TARGET").unwrap_or("morphological_aa.enabled".into());
    let setting = settings
        .iter()
        .find(|s| s.id == wanted && s.state == State::Configurable)
        .expect("a configurable crash target");
    (setting.id, alternative(setting).expect("a different value"))
}

/// Child process 1: change a setting through the journal, then die without restoring.
#[test]
#[ignore = "child of crash_then_recover"]
fn crash_child() {
    let Ok(dir) = std::env::var(CRASH_DIR) else {
        return;
    };
    let gpu = first_gpu();
    let settings = helper().read(gpu).expect("read");
    let (id, value) = crash_target(&settings);
    let mut controller = Controller::new(helper(), Path::new(&dir).join(JOURNAL_FILE));
    controller
        .apply(
            "crashed",
            gpu,
            &[Change {
                id: id.into(),
                value,
            }],
        )
        .expect("apply");
    println!("child wrote {id} and now dies without restoring");
    if std::env::var_os("RADEON_CRASH_EXIT").is_some() {
        std::process::exit(3);
    }
    std::process::abort();
}

/// Child process 2: a fresh host start, running the startup recovery with the helper-process
/// backend, with nothing between the crash and this. The crash left the setting changed, so the
/// recovery must have written the original back, not found it already there.
#[test]
#[ignore = "child of crash_then_recover"]
fn recover_child() {
    use super::control::RestoreResult;
    let Ok(dir) = std::env::var(CRASH_DIR) else {
        return;
    };
    let mut controller = Controller::new(helper(), Path::new(&dir).join(JOURNAL_FILE));
    let report = controller.recover_on_startup().expect("recover");
    println!("recovery: {:?}", report.outcomes);
    assert!(report.is_clean(), "recovery failed: {:?}", report.outcomes);
    assert_eq!(report.outcomes.len(), 1, "{:?}", report.outcomes);
    assert_eq!(
        report.outcomes[0].result,
        RestoreResult::Restored,
        "the crash left the setting changed, so recovery must have written it back"
    );
}

/// What a fresh process reads for a setting.
fn fresh_value(gpu: i32, id: &str) -> String {
    helper()
        .read_value(gpu, id)
        .map_or_else(|e| format!("unreadable: {e}"), |v| v.display())
}

fn run_child(test: &str, dir: &Path, exit: bool) -> std::process::ExitStatus {
    let mut child = Command::new(std::env::current_exe().expect("test binary"));
    child
        .args([
            "--exact",
            &format!("radeon::hardware::{test}"),
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CRASH_DIR, dir);
    if exit {
        child.env("RADEON_CRASH_EXIT", "1");
    } else {
        child.env_remove("RADEON_CRASH_EXIT");
    }
    child.status().expect("spawn")
}

/// Write through the journal, kill the process before it restores, and start recovery in a new
/// one at once. `exit` ends the child with `exit(3)` (no destructors); otherwise it `abort()`s,
/// which Windows treats as a fast-fail crash. Returns what a fresh process read the moment after
/// the death and the moment after the recovery.
///
/// The recovery is deliberately not preceded by a check or a wait: it runs in the window where
/// the dead child's write is the newest thing the driver has seen, which is where a recovery that
/// trusted a stale read would wrongly conclude that nothing needs undoing.
fn crash_round(exit: bool) -> (String, Value, String) {
    let gpu = first_gpu();
    let start = helper().read(gpu).expect("read");
    let _restore = Restore::new(gpu, &start);
    snapshot(&start);
    let dir = scratch_dir(if exit { "crash-exit" } else { "crash-abort" });
    let (id, tried) = crash_target(&start);
    let original = start
        .iter()
        .find(|s| s.id == id)
        .and_then(|s| s.value.clone())
        .expect("value");
    let how = if exit { "exit(3)" } else { "abort()" };

    let status = run_child("crash_child", &dir, exit);
    assert!(!status.success(), "the child should have died: {status}");
    let journal = Controller::new(helper(), dir.join(JOURNAL_FILE))
        .journal()
        .expect("journal");
    assert_eq!(journal.entries.len(), 1, "journal: {:?}", journal.entries);
    assert_eq!(journal.entries[0].original, original);
    assert_eq!(journal.entries[0].written, tried);

    // Straight into the recovery, then look: it must have put the original back.
    let waited = std::time::Instant::now();
    let status = run_child("recover_child", &dir, exit);
    assert!(status.success(), "recovery child failed: {status}");
    let now = fresh_value(gpu, id);
    println!(
        "{id}: a new process reads {now} after the recovery ({how}, recovery child took {:?})",
        waited.elapsed()
    );
    assert_eq!(
        now,
        original.display(),
        "{id} should be back to {}",
        original.display()
    );
    // And it stays there: the dead child's write must not land after the recovery.
    std::thread::sleep(Duration::from_secs(5));
    let later = fresh_value(gpu, id);
    assert_eq!(
        later,
        original.display(),
        "{id} changed again after the recovery"
    );
    (id.to_string(), tried, now)
}

/// The host dies after a journaled write. A fresh start restores the original.
#[test]
#[ignore = "changes real driver settings"]
fn crash_then_recover() {
    let _exclusive = exclusive();
    assert_host_idle();
    let (id, tried, held) = crash_round(true);
    println!(
        "exit(3) crash: {id} -> {} (child killed) -> {held} (recovered in a fresh process)",
        tried.display()
    );
    assert_host_idle();
    let (id, tried, held) = crash_round(false);
    println!(
        "abort() crash: {id} -> {} (child killed) -> {held} (recovered in a fresh process)",
        tried.display()
    );
}

/// Where the other account's peer and the test exchange requests (`RADEON_XACCT_DIR`).
fn xacct_dir() -> PathBuf {
    std::env::var("RADEON_XACCT_DIR").map_or_else(|_| PathBuf::from(r"C:\src\rl-xacct"), Into::into)
}

/// Who this process is and in which Windows session, as `whoami` and the session id.
fn identity() -> String {
    let who = Command::new("whoami")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|e| format!("unknown ({e})"));
    let session = crate::process::current_session().map_or("?".into(), |s| s.to_string());
    format!("{who} (session {session})")
}

/// Write a file other processes poll for, so they never see half of it.
fn publish(path: &Path, body: &[u8]) {
    let partial = path.with_extension("part");
    std::fs::write(&partial, body).expect("write exchange file");
    std::fs::rename(&partial, path).expect("publish exchange file");
}

/// The installed host runs as SYSTEM in the console session; a development host runs as the
/// signed-in user. Started under the other account (see `docs/hardware-matrix.md`, "Which
/// account"), this answers helper requests that `RemoteBackend` leaves in the exchange folder,
/// by running each in a fresh helper process of its own account, until a `stop` file appears.
#[test]
#[ignore = "child of the cross-account test"]
fn cross_account_server() {
    use std::time::Instant;
    let dir = xacct_dir();
    let mut backend = helper();
    publish(&dir.join("identity.txt"), identity().as_bytes());
    let started = Instant::now();
    while !dir.join("stop").exists() && started.elapsed() < Duration::from_secs(60 * 60) {
        let next = std::fs::read_dir(&dir)
            .expect("exchange folder")
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|path| {
                path.extension().is_some_and(|e| e == "json")
                    && path
                        .file_name()
                        .is_some_and(|n| n.to_string_lossy().starts_with("req-"))
            });
        let Some(path) = next else {
            std::thread::sleep(Duration::from_millis(20));
            continue;
        };
        let body = std::fs::read_to_string(&path).unwrap_or_default();
        let name = path
            .file_name()
            .expect("name")
            .to_string_lossy()
            .replace("req-", "resp-");
        let _ = std::fs::remove_file(&path);
        let response = super::helper::handle(&body, &mut backend);
        publish(
            &dir.join(name),
            serde_json::to_string(&response).expect("json").as_bytes(),
        );
    }
}

/// A backend that has every call run by the account behind `cross_account_server`.
#[derive(Clone)]
struct RemoteBackend {
    dir: PathBuf,
}
impl RemoteBackend {
    fn call(
        &self,
        request: &super::helper::Request,
    ) -> Result<super::helper::Response, super::BackendError> {
        use std::sync::atomic::{AtomicU32, Ordering};
        static NEXT: AtomicU32 = AtomicU32::new(1);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let answer = self.dir.join(format!("resp-{id}.json"));
        let _ = std::fs::remove_file(&answer);
        publish(
            &self.dir.join(format!("req-{id}.json")),
            serde_json::to_string(request).expect("json").as_bytes(),
        );
        let deadline = std::time::Instant::now() + Duration::from_secs(120);
        while !answer.exists() {
            if std::time::Instant::now() > deadline {
                return Err(super::BackendError(
                    "the other account did not answer".into(),
                ));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let text = std::fs::read_to_string(&answer)
            .map_err(|e| super::BackendError(format!("cannot read the answer: {e}")))?;
        let _ = std::fs::remove_file(&answer);
        serde_json::from_str(&text)
            .map_err(|e| super::BackendError(format!("unreadable answer: {e}")))
    }
}
impl RadeonBackend for RemoteBackend {
    fn read(&mut self, gpu: i32) -> Result<Vec<Setting>, super::BackendError> {
        match self.call(&super::helper::Request::Read { gpu })? {
            super::helper::Response::Settings { settings } => Ok(settings),
            super::helper::Response::Error { message } => Err(super::BackendError(message)),
            super::helper::Response::Done => Err(super::BackendError("read answered done".into())),
        }
    }
    fn write(&mut self, gpu: i32, id: &str, value: &Value) -> Result<(), super::BackendError> {
        let request = super::helper::Request::Write {
            gpu,
            id: id.into(),
            value: value.clone(),
        };
        match self.call(&request)? {
            super::helper::Response::Done => Ok(()),
            super::helper::Response::Error { message } => Err(super::BackendError(message)),
            super::helper::Response::Settings { .. } => {
                Err(super::BackendError("write answered with settings".into()))
            }
        }
    }
}

#[derive(Serialize)]
struct CrossAccountRow {
    writer: String,
    reader: String,
    id: &'static str,
    wrote: Option<Value>,
    /// What the writer's own read-back showed for the setting.
    writer_saw: Option<Value>,
    /// What a fresh helper of the other account showed for it.
    reader_saw: Option<Value>,
    /// Settings (any, not just this one) on which the two accounts' fresh reads differ.
    differing: Vec<String>,
    /// The request was refused, or needed the feature switched on.
    detail: String,
}

/// Write every configurable setting as `writer_name` (through the journal) and read it back from a
/// fresh helper of `reader_name`, restoring afterwards.
fn cross_account_direction<W: RadeonBackend, R: RadeonBackend>(
    writer_name: &str,
    writer: W,
    reader_name: &str,
    reader: &mut R,
    start: &[Setting],
    gpu: i32,
) -> Vec<CrossAccountRow> {
    let dir = scratch_dir(&format!("xacct-{writer_name}"));
    let mut controller = Controller::new(writer, dir.join(JOURNAL_FILE));
    let mut rows = vec![];
    for setting in start
        .iter()
        .filter(|s| s.state == State::Configurable && s.kind != Kind::Action)
    {
        assert_host_idle();
        let Some(tried) = alternative(setting) else {
            continue;
        };
        let mut row = CrossAccountRow {
            writer: writer_name.into(),
            reader: reader_name.into(),
            id: setting.id,
            wrote: Some(tried.clone()),
            writer_saw: None,
            reader_saw: None,
            differing: vec![],
            detail: String::new(),
        };
        let mut refusals = vec![];
        for changes in &attempts(setting, start, &tried) {
            match controller.apply("xacct", gpu, changes) {
                Ok(after) => {
                    let theirs = reader.read(gpu).expect("the other account reads");
                    row.writer_saw = after
                        .iter()
                        .find(|s| s.id == setting.id)
                        .and_then(|s| s.value.clone());
                    row.reader_saw = theirs
                        .iter()
                        .find(|s| s.id == setting.id)
                        .and_then(|s| s.value.clone());
                    row.differing = diff(&after, &theirs);
                    if changes.len() > 1 {
                        row.detail = format!("with {} on", changes[0].id);
                    }
                    let report = controller
                        .restore(Scope::Session("xacct"))
                        .expect("restore");
                    assert!(
                        report.is_clean(),
                        "restoring {} failed: {:?}",
                        setting.id,
                        report.outcomes
                    );
                    let moved = diff(start, &reader.read(gpu).expect("read"));
                    assert!(
                        moved.is_empty(),
                        "after restoring {} the {reader_name} sees: {moved:?}",
                        setting.id
                    );
                    refusals.clear();
                    break;
                }
                Err(e) => refusals.push(e.to_string()),
            }
        }
        if !refusals.is_empty() {
            row.detail = format!("refused: {}", refusals.join("; "));
        }
        println!(
            "{writer_name}->{reader_name} {:<36} wrote {:<12} writer saw {:<12} reader saw {:<12} differing {:?} {}",
            setting.id,
            tried.display(),
            row.writer_saw.as_ref().map_or("-".into(), Value::display),
            row.reader_saw.as_ref().map_or("-".into(), Value::display),
            row.differing,
            row.detail
        );
        rows.push(row);
    }
    assert!(controller.journal().expect("journal").entries.is_empty());
    rows
}

/// The installed host is SYSTEM and a development host the signed-in user: is a setting one
/// account writes the setting the other reads? Needs `cross_account_server` running as the other
/// account; this test is the user. Both directions, every configurable setting.
#[test]
#[ignore = "changes real driver settings"]
fn cross_account_round_trip() {
    let _exclusive = exclusive();
    assert_host_idle();
    let dir = xacct_dir();
    let peer_identity = std::fs::read_to_string(dir.join("identity.txt"))
        .expect("cross_account_server is not running (no identity.txt)");
    let mine = identity();
    println!("peer: {peer_identity}\nthis test: {mine}");
    assert_ne!(peer_identity, mine, "the peer runs as the same account");
    let accounts = dir.join("helper-accounts.log");
    std::fs::write(&accounts, b"").expect("start the helper account log");
    let my_sid = crate::process::own_sid().expect("this test's SID");
    let peer = RemoteBackend { dir };
    let gpu = first_gpu();
    let start = helper().read(gpu).expect("read");
    let _restore = Restore::new(gpu, &start);
    let theirs = peer.clone().read(gpu).expect("the peer reads");
    let moved = diff(&start, &theirs);
    assert!(
        moved.is_empty(),
        "the two accounts read different values before any write: {moved:?}"
    );

    let mut rows =
        cross_account_direction("peer", peer.clone(), "user", &mut helper(), &start, gpu);
    rows.extend(cross_account_direction(
        "user",
        helper(),
        "peer",
        &mut peer.clone(),
        &start,
        gpu,
    ));
    let disagree: Vec<_> = rows
        .iter()
        .filter(|r| !r.differing.is_empty() || r.writer_saw != r.reader_saw)
        .map(|r| format!("{}->{} {}: {:?}", r.writer, r.reader, r.id, r.differing))
        .collect();
    let written = rows.iter().filter(|r| r.writer_saw.is_some()).count();
    save("rl-a5-xacct.json", &rows);
    assert!(disagree.is_empty(), "the accounts disagree: {disagree:?}");
    // Every helper the peer started ran as this test's user, none as the peer's own account.
    let log = std::fs::read_to_string(&accounts).expect("helper account log");
    let lines: Vec<&str> = log.lines().collect();
    println!(
        "{} helpers started by the peer ({peer_identity}); first: {}",
        lines.len(),
        lines.first().copied().unwrap_or("none")
    );
    assert!(!lines.is_empty(), "no helper reported its account");
    let wrong: Vec<_> = lines
        .iter()
        .filter(|l| !l.contains(&format!("sid {my_sid} system false")))
        .collect();
    assert!(
        wrong.is_empty(),
        "helpers not running as {my_sid}: {wrong:?}"
    );
    let end = helper().read(gpu).expect("read");
    let moved = diff(&start, &end);
    assert!(moved.is_empty(), "final probe diff is not empty: {moved:?}");
    println!(
        "{written} writes across both directions, all read back identically by the other account; final probe diff: empty"
    );
}

/// What makes this process see a write another process made? After an `abort()` of the writer
/// this process kept reading the old value for 30 s, while a new process saw the new one at once.
/// Each method is tried against a fresh abort: re-reading (every read already re-acquires the
/// services and interfaces), one `ADLXTerminate` before the next read, and
/// `ADLXInitializeWithIncompatibleDriver`.
#[test]
#[ignore = "changes real driver settings"]
fn measure_view_refresh() {
    let _exclusive = exclusive();
    assert_host_idle();
    let gpu = first_gpu();
    let start = helper().read(gpu).expect("read");
    let restore = Restore::new(gpu, &start);
    snapshot(&start);
    let (id, tried) = crash_target(&start);
    let original = start
        .iter()
        .find(|s| s.id == id)
        .and_then(|s| s.value.clone())
        .expect("value");
    // The one place that reads in this process on purpose: its view is what is being measured.
    let read_target = || {
        AdlxBackend
            .read(gpu)
            .expect("read")
            .into_iter()
            .find(|s| s.id == id)
            .and_then(|s| s.value)
    };
    // `terminate` crashed this process with an access violation the one time it was tried
    // (0xC0000005 on the read after it), so it only runs when asked for.
    let methods = std::env::var("RADEON_REFRESH_METHODS")
        .unwrap_or_else(|_| "re-read,incompatible-driver,wait".into());
    for method in methods.split(',') {
        assert_host_idle();
        assert_eq!(read_target().as_ref(), Some(&original));
        let dir = scratch_dir("refresh");
        let status = run_child("crash_child", &dir, false);
        assert!(!status.success(), "the child should have died: {status}");
        let fresh = fresh_value(gpu, id);
        let began = std::time::Instant::now();
        let mut seen = None;
        let mut applied = String::new();
        while began.elapsed() < Duration::from_secs(12) && seen.is_none() {
            if applied.is_empty() {
                // The shared ADLX lock is held only while the method runs, so the reads below
                // can take it again.
                let runtime = crate::adlx_system::acquire().expect("ADLX");
                let library = runtime.library();
                applied = match method {
                    "terminate" => {
                        // SAFETY: No ADLX session is alive on this thread.
                        let result = unsafe {
                            library
                                .get::<unsafe extern "C" fn() -> i32>(b"ADLXTerminate\0")
                                .map(|f| f())
                        };
                        format!("ADLXTerminate -> {result:?}")
                    }
                    "incompatible-driver" => {
                        let mut system = std::ptr::null_mut();
                        type Init = unsafe extern "C" fn(u64, *mut crate::telemetry::Handle) -> i32;
                        // SAFETY: Same signature as ADLXInitialize; the output is a live pointer.
                        let result = unsafe {
                            library
                                .get::<Init>(b"ADLXInitializeWithIncompatibleDriver\0")
                                .map(|f| f(crate::telemetry::ADLX_VERSION, &mut system))
                        };
                        format!("ADLXInitializeWithIncompatibleDriver -> {result:?}")
                    }
                    _ => "nothing".into(),
                };
            }
            if read_target().as_ref() == Some(&tried) {
                seen = Some(began.elapsed());
            } else {
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        println!(
            "REFRESH {method}: a new process read {fresh}; this process {} ({applied})",
            seen.map_or("did not see it within 12 s".to_string(), |t| format!(
                "saw it after {t:?}"
            ))
        );
        // Put back through fresh helper processes: this process's own writes are not trusted
        // while its view is stale (a write of the value it believes is current is dropped).
        let stuck = restore.put_back();
        assert!(stuck.is_empty(), "could not restore: {stuck:?}");
        assert_eq!(fresh_value(gpu, id), original.display(), "{id} not back");
    }
}
