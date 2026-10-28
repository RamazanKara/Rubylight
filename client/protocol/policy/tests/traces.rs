//! Replays every trace in tests/traces/<module>/*.json. Android's PolicyTraceTest replays the
//! same files against the Java policies, so both must pass.

use std::fs;
use std::path::{Path, PathBuf};

/// Every module folder, each with at least one trace, so a renamed or emptied folder fails.
const MODULES: [&str; 7] = [
    "bitrate",
    "client_hint",
    "health",
    "launch",
    "pyrowave",
    "quality",
    "recovery",
];

fn sorted(dir: &Path) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .map(|entry| entry.unwrap().path())
        .collect();
    paths.sort();
    paths
}

/// Replays every trace under `root` and returns how many ran, or every failure.
fn replay_all(root: &Path, modules: &[&str]) -> Result<usize, String> {
    let mut failures = Vec::new();
    let mut found = Vec::new();
    let mut replayed = 0;
    for module in sorted(root) {
        if !module.is_dir() {
            failures.push(format!("{} is not a module folder", module.display()));
            continue;
        }
        let mut traces = 0;
        for file in sorted(&module) {
            // Only exactly .json: a trace named .JSON or .json.bak would silently not run.
            if file.extension().and_then(|ext| ext.to_str()) != Some("json") {
                failures.push(format!("{}: not a .json trace", file.display()));
                continue;
            }
            let result = fs::read_to_string(&file)
                .map_err(|e| e.to_string())
                .and_then(|text| serde_json::from_str(&text).map_err(|e| e.to_string()))
                .and_then(|trace| nightjar_policy::trace::replay(&trace));
            match result {
                Ok(_) => traces += 1,
                Err(e) => failures.push(format!("{}: {e}", file.display())),
            }
        }
        if traces == 0 {
            failures.push(format!("{}: no traces", module.display()));
        }
        replayed += traces;
        found.push(module.file_name().unwrap().to_string_lossy().into_owned());
    }
    if found != modules {
        failures.push(format!(
            "module folders under {} are {found:?}, expected {modules:?}",
            root.display()
        ));
    }
    match failures.is_empty() {
        true => Ok(replayed),
        false => Err(failures.join("\n")),
    }
}

#[test]
fn every_trace_replays() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/traces");
    if let Err(failures) = replay_all(&root, &MODULES) {
        panic!("{failures}");
    }
}

#[test]
fn missing_modules_and_other_files_fail() {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join("policy-traces");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("recovery")).unwrap();
    let trace = r#"{"rust": "recovery::Backoff", "java": null, "new": [],
        "calls": [{"call": "attempts", "args": [], "expect": 0}]}"#;
    fs::write(root.join("recovery/a.json"), trace).unwrap();
    assert_eq!(replay_all(&root, &["recovery"]), Ok(1));
    assert!(replay_all(&root, &["bitrate", "recovery"]).is_err());
    fs::create_dir(root.join("bitrate")).unwrap();
    assert!(replay_all(&root, &["bitrate", "recovery"]).is_err());
    fs::remove_dir(root.join("bitrate")).unwrap();
    fs::write(root.join("recovery/b.JSON"), trace).unwrap();
    let error = replay_all(&root, &["recovery"]).unwrap_err();
    assert!(error.contains("b.JSON: not a .json trace"), "{error}");
    fs::remove_dir_all(&root).unwrap();
}
