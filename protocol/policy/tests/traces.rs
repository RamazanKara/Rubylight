//! Replays every trace in tests/traces/<module>/*.json. Android's PolicyTraceTest replays the
//! same files against the Java policies, so both must pass.

use std::fs;
use std::path::{Path, PathBuf};

fn sorted(dir: &Path) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .map(|entry| entry.unwrap().path())
        .collect();
    paths.sort();
    paths
}

#[test]
fn every_trace_replays() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/traces");
    let mut failures = Vec::new();
    let mut replayed = 0;
    for module in sorted(&root) {
        assert!(
            module.is_dir(),
            "{} is not a module folder",
            module.display()
        );
        for file in sorted(&module) {
            if file.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            let result = fs::read_to_string(&file)
                .map_err(|e| e.to_string())
                .and_then(|text| serde_json::from_str(&text).map_err(|e| e.to_string()))
                .and_then(|trace| nightjar_policy::trace::replay(&trace));
            match result {
                Ok(_) => replayed += 1,
                Err(e) => failures.push(format!("{}: {e}", file.display())),
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    assert!(replayed > 0, "no traces under {}", root.display());
}
