//! The part of core's state module (rust/core/src/state.rs) that rolling back
//! an interrupted update uses: reading and atomically writing the update
//! record. Keep it in step with core's.
use anyhow::{Context, Result};
use serde_json::Value;
use std::{fs::OpenOptions, io::Write, path::Path};

/// The JSON in a state file: a UTF-8 byte order mark is dropped, and a blank
/// file holds nothing (None).
fn json_bytes(bytes: &[u8]) -> Option<&[u8]> {
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    bytes
        .iter()
        .any(|b| !b" \t\r\n\x0b\x0c".contains(b))
        .then_some(bytes)
}
/// `default` for a missing or blank file; an unreadable one is an error.
pub fn load_json(path: &Path, default: Value) -> Result<Value> {
    match std::fs::read(path) {
        Ok(b) => match json_bytes(&b) {
            Some(json) => serde_json::from_slice(json)
                .with_context(|| format!("invalid JSON in {}", path.display())),
            None => Ok(default),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(default),
        Err(e) => Err(e.into()),
    }
}
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
    let result = (|| -> Result<()> {
        let mut f = OpenOptions::new().create_new(true).write(true).open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        crate::update_files::publish(&tmp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
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
