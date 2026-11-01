//! Reading and atomically writing a state file. The Windows service compiles
//! this file too (rust/service/src/state.rs, `#[path]`), so it stays within
//! std, anyhow, serde_json and `crate::update_files`, and a change to it is a
//! change to the service (rust/release/service_inputs.py).
use anyhow::{Context, Result};
use serde_json::Value;
use std::{fs::OpenOptions, io::Write, path::Path};

/// The JSON in a state file, as Vibepollo reads it: a UTF-8 byte order mark
/// is dropped, and a blank file holds nothing (None).
pub fn json_bytes(bytes: &[u8]) -> Option<&[u8]> {
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    bytes
        .iter()
        .any(|b| !b" \t\r\n\x0b\x0c".contains(b))
        .then_some(bytes)
}
/// Never turn an unreadable existing identity file into a fresh empty host.
/// A blank file has nothing to lose and reads as a missing one.
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
/// Write `bytes` to `tmp`, a new file beside `path` that the caller names,
/// flush it to disk and put it in `path`'s place; `tmp` is removed when that
/// fails.
pub fn write_through(tmp: &Path, path: &Path, bytes: &[u8]) -> Result<()> {
    let result = (|| -> Result<()> {
        let mut f = OpenOptions::new().create_new(true).write(true).open(tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        #[cfg(windows)]
        crate::update_files::publish(tmp, path)?;
        #[cfg(not(windows))]
        {
            std::fs::rename(tmp, path)?;
            std::fs::File::open(path.parent().context("state path has no parent")?)?.sync_all()?;
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(tmp);
    }
    result
}
