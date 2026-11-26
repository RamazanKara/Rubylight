//! The Quick Suspend write-ahead journal: `quick-suspend.json` in the profile.
//!
//! The host writes it, flushed to disk and put in place atomically, before it
//! suspends the first process, and removes it only once every process it
//! names runs again or has exited. Whatever stops the host in between (a
//! crash, a kill, a restart of the host or the service, a reboot) leaves the
//! file behind for the watchdog or the next host to resume what it names.
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The journal's file name in the profile.
pub const FILE: &str = "quick-suspend.json";

/// The journal's format; a host refuses to act on a newer one.
pub const VERSION: u32 = 1;

/// One process the host suspends, as it identifies it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub pid: u32,
    /// Creation time, 100 ns since 1601: the id alone may be reused.
    pub started: u64,
    /// Its program name, for the log.
    pub name: String,
}

/// The processes a host suspends, in the order it suspends them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Journal {
    pub version: u32,
    /// The host that wrote it (id and creation time), so that a watchdog acts
    /// only for its own host and a second host leaves a running one's alone.
    pub host_pid: u32,
    pub host_started: u64,
    /// The suspended app's title, for the recovery notice.
    #[serde(default)]
    pub app_title: String,
    /// When the suspend started (Unix ms).
    #[serde(default)]
    pub since_unix_ms: u64,
    pub processes: Vec<Record>,
}

impl Journal {
    pub fn new(host: (u32, u64), app_title: &str, since_unix_ms: u64) -> Self {
        Self {
            version: VERSION,
            host_pid: host.0,
            host_started: host.1,
            app_title: app_title.to_owned(),
            since_unix_ms,
            processes: Vec::new(),
        }
    }
}

/// The journal's path in `profile`.
pub fn path(profile: &Path) -> PathBuf {
    profile.join(FILE)
}

/// Write `journal` to `profile`, flushed to disk before it replaces any earlier
/// one: once this returns, a crash cannot lose it.
pub fn save(profile: &Path, journal: &Journal) -> Result<()> {
    crate::state::atomic_write(&path(profile), &serde_json::to_vec_pretty(journal)?)
        .with_context(|| format!("writing {}", path(profile).display()))
}

/// The journal in `profile`, or None when there is none. A file that cannot be
/// read as a journal is an error, never an empty journal: the processes it may
/// name are still frozen.
pub fn load(profile: &Path) -> Result<Option<Journal>> {
    let path = path(profile);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
    };
    let Some(json) = crate::state::json_bytes(&bytes) else {
        // A blank file: written by nothing that suspended a process.
        return Ok(None);
    };
    let journal: Journal = serde_json::from_slice(json)
        .with_context(|| format!("invalid Quick Suspend journal {}", path.display()))?;
    anyhow::ensure!(
        journal.version <= VERSION,
        "the Quick Suspend journal {} is of a newer version ({})",
        path.display(),
        journal.version
    );
    Ok(Some(journal))
}

/// Remove the journal from `profile`; one that is already gone is fine.
pub fn remove(profile: &Path) -> Result<()> {
    match std::fs::remove_file(path(profile)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("removing {}", path(profile).display())),
    }
}

/// Keep an unreadable journal for a look by hand, out of the way of the next
/// suspend: renamed with `.bad`. Its processes cannot be identified, so none is
/// resumed by guess.
pub fn set_aside(profile: &Path) -> Result<PathBuf> {
    let aside = profile.join(format!("{FILE}.bad"));
    std::fs::rename(path(profile), &aside)
        .with_context(|| format!("setting {} aside", path(profile).display()))?;
    Ok(aside)
}
