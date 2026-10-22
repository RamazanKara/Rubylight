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
        let target = Change {
            id: setting.id.into(),
            value: tried.clone(),
        };
        // A setting of a feature that is off may only take a value while the same request turns
        // the feature on. Try it alone first so the matrix records which settings need that.
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
        if let Some(enable) = &group_enable {
            attempts.push(vec![
                Change {
                    id: enable.clone(),
                    value: Value::Bool(true),
                },
                target,
            ]);
        }
        let mut refusals: Vec<String> = vec![];
        let mut outcome = None;
        for changes in &attempts {
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
