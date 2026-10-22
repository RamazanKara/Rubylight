//! Who owns the Radeon journal: a named mutex serializes every load-modify-save of the file, and
//! the owner's process id and start time say whether the host that wrote it is still alive (the
//! pattern of `display_recovery`). A second host must not put back a live host's settings.
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::path::Path;
use windows::Win32::{Foundation::*, System::Threading::*};

/// The process that last wrote the journal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Owner {
    pub pid: u32,
    /// The process creation time, so a reused pid is not mistaken for the owner.
    pub started: u64,
}
impl Owner {
    /// This process.
    pub fn current() -> Result<Self> {
        let pid = std::process::id();
        let started = identity(pid)?.ok_or_else(|| anyhow::anyhow!("host identity unavailable"))?;
        Ok(Self { pid, started })
    }
    /// Another process that is still running.
    pub fn is_other_and_alive(&self) -> bool {
        self.pid != std::process::id() && matches!(identity(self.pid), Ok(Some(t)) if t == self.started)
    }
}

pub struct JournalLock(HANDLE);
impl Drop for JournalLock {
    fn drop(&mut self) {
        // SAFETY: lock() acquired this mutex on the current thread, and this guard owns the handle
        // until this single release and close.
        unsafe {
            let _ = ReleaseMutex(self.0);
            let _ = CloseHandle(self.0);
        }
    }
}

/// Take the journal's mutex; wait for another thread or process that holds it.
pub fn lock(path: &Path) -> Result<JournalLock> {
    let hash = butterpollo_core::crypto::hash(path.to_string_lossy().to_lowercase().as_bytes());
    let name: Vec<u16> = format!(
        "Local\\Butterpollo.RadeonJournal.{:x}\0",
        u64::from_le_bytes(hash[..8].try_into().unwrap())
    )
    .encode_utf16()
    .collect();
    // SAFETY: name is terminated and lives through CreateMutexW. The returned handle is either
    // closed on failure or transferred to a guard after this thread acquires it.
    unsafe {
        let handle = CreateMutexW(None, false, windows::core::PCWSTR(name.as_ptr()))?;
        let result = WaitForSingleObject(handle, 30000);
        if result != WAIT_OBJECT_0 && result != WAIT_ABANDONED {
            let _ = CloseHandle(handle);
            bail!("the Radeon journal is busy");
        }
        Ok(JournalLock(handle))
    }
}

/// The creation time of a running process, `None` when it has exited or never existed.
pub fn identity(pid: u32) -> Result<Option<u64>> {
    // SAFETY: OpenProcess supplies an owned query handle; the four initialized outputs remain
    // writable for GetProcessTimes, and the handle is closed exactly once afterward.
    unsafe {
        if pid == 0 {
            return Ok(None);
        }
        let process = match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
            Ok(handle) => handle,
            Err(e) if e.code() == windows::core::HRESULT::from_win32(ERROR_INVALID_PARAMETER.0) => {
                return Ok(None);
            }
            Err(e) => return Err(e.into()),
        };
        let (mut created, mut exited, mut kernel, mut user) = (
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
        );
        let result = GetProcessTimes(process, &mut created, &mut exited, &mut kernel, &mut user);
        let _ = CloseHandle(process);
        result?;
        if exited.dwLowDateTime != 0 || exited.dwHighDateTime != 0 {
            return Ok(None);
        }
        Ok(Some(
            u64::from(created.dwLowDateTime) | (u64::from(created.dwHighDateTime) << 32),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_process_is_not_another_owner_and_a_dead_one_is_not_alive() {
        let me = Owner::current().unwrap();
        assert!(!me.is_other_and_alive());
        let mut child = std::process::Command::new("cmd")
            .args(["/c", "exit 0"])
            .spawn()
            .unwrap();
        let pid = child.id();
        let started = identity(pid).ok().flatten().unwrap_or(1);
        child.wait().unwrap();
        assert!(!Owner { pid, started }.is_other_and_alive());
    }

    #[test]
    fn a_live_process_with_its_start_time_is_another_owner_but_a_reused_pid_is_not() {
        let mut child = std::process::Command::new("cmd")
            .args(["/c", "ping -n 20 127.0.0.1 >nul"])
            .spawn()
            .unwrap();
        let started = identity(child.id()).unwrap().unwrap();
        assert!(
            Owner {
                pid: child.id(),
                started
            }
            .is_other_and_alive()
        );
        assert!(
            !Owner {
                pid: child.id(),
                started: started + 1
            }
            .is_other_and_alive()
        );
        child.kill().unwrap();
        let _ = child.wait();
    }

    #[test]
    fn the_lock_is_reentrant_on_a_thread_and_blocks_another_until_released() {
        let path = std::env::temp_dir().join(format!("radeon-lock-{}.json", std::process::id()));
        let first = lock(&path).unwrap();
        let held = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let seen = held.clone();
        let other = path.clone();
        let thread = std::thread::spawn(move || {
            let _second = lock(&other).unwrap();
            // Only reachable after the first guard went away.
            assert!(!seen.load(std::sync::atomic::Ordering::SeqCst));
        });
        std::thread::sleep(std::time::Duration::from_millis(200));
        held.store(false, std::sync::atomic::Ordering::SeqCst);
        drop(first);
        thread.join().unwrap();
    }
}
