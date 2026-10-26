//! Replays the redaction cases in `tests/fixtures/redact` against `report::redact`. The Android
//! app's `RedactedLogContractTest` replays the same files against `RedactedLog.redact`.

use std::path::{Path, PathBuf};

use nightjar_core::report::redact::Secrets;
use serde_json::Value;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/redact")
}

/// Replays every case under `root`; returns how many ran and the failures.
fn replay_all(root: &Path) -> (usize, Vec<String>) {
    let mut files: Vec<PathBuf> = std::fs::read_dir(root)
        .unwrap_or_else(|e| panic!("missing fixture folder {}: {e}", root.display()))
        .map(|entry| entry.expect("fixture entry").path())
        .filter(|path| path.extension().is_some_and(|e| e == "json"))
        .collect();
    files.sort();
    let mut ran = 0;
    let mut failures = Vec::new();
    for file in files {
        let text = std::fs::read_to_string(&file).expect("fixture file");
        let fixture: Value =
            serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
        let cases = fixture["cases"]
            .as_array()
            .unwrap_or_else(|| panic!("{}: no cases", file.display()));
        for case in cases {
            let name = case["name"].as_str().expect("case name");
            if let Err(why) = replay(case) {
                failures.push(format!("{} / {name}: {why}", file.display()));
            }
            ran += 1;
        }
    }
    (ran, failures)
}

fn replay(case: &Value) -> Result<(), String> {
    let mut secrets = Secrets::new();
    for secret in case["secrets"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        secrets.add(secret.as_str().ok_or("a secret is not a string")?);
    }
    let input = case["input"].as_str().ok_or("no input")?;
    let expect = case["expect"].as_str().ok_or("no expect")?;
    let got = secrets.redact(input);
    if got == expect {
        Ok(())
    } else {
        Err(format!("expected {expect:?}, got {got:?}"))
    }
}

#[test]
fn every_case_matches() {
    let (ran, failures) = replay_all(&fixtures());
    assert!(ran > 0, "no redaction cases under {}", fixtures().display());
    assert!(
        failures.is_empty(),
        "{} of {ran} cases failed:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
#[should_panic(expected = "missing fixture folder")]
fn a_missing_fixture_folder_fails() {
    replay_all(&fixtures().join("no-cases"));
}

#[test]
fn a_wrong_expectation_fails() {
    let case = serde_json::json!({"name": "wrong", "input": "PIN 1234", "expect": "PIN 1234"});
    assert_eq!(
        replay(&case),
        Err("expected \"PIN 1234\", got \"PIN [omitted]\"".into())
    );
}
