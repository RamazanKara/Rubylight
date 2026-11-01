//! The part of core's state module (rust/core/src/state.rs) that rolling back
//! an interrupted update uses: reading and atomically writing the update
//! record, compiled from core's own file (rust/core/src/state/file.rs).
use anyhow::{Context, Result};
use serde_json::Value;
use std::path::Path;

#[path = "../../core/src/state/file.rs"]
mod file;
pub use file::load_json;

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("state path has no parent")?;
    std::fs::create_dir_all(parent)?;
    // Core names it with a UUID; the process and the time are as unique here.
    let tmp = parent.join(format!(
        ".{}.{}-{}.tmp",
        path.file_name()
            .context("state path has no name")?
            .to_string_lossy(),
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    ));
    file::write_through(&tmp, path, bytes)
}
pub fn write_json(path: &Path, value: &Value) -> Result<()> {
    atomic_write(path, &serde_json::to_vec_pretty(value)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_record_reads_back_and_blank_or_missing_files_are_the_default() -> Result<()> {
        let root = tempfile::tempdir()?;
        let path = root.path().join("profile/update-result.json");
        assert_eq!(load_json(&path, Value::Null)?, Value::Null);
        write_json(&path, &serde_json::json!({"phase": "installing"}))?;
        assert_eq!(load_json(&path, Value::Null)?["phase"], "installing");
        assert_eq!(std::fs::read_dir(path.parent().unwrap())?.count(), 1);
        std::fs::write(&path, b"\xef\xbb\xbf{\"phase\":\"installed\"}")?;
        assert_eq!(load_json(&path, Value::Null)?["phase"], "installed");
        std::fs::write(&path, b" \r\n")?;
        assert_eq!(load_json(&path, Value::Null)?, Value::Null);
        std::fs::write(&path, b"{")?;
        assert!(load_json(&path, Value::Null).is_err());
        Ok(())
    }
}
