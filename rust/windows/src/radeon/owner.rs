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
    /// Another process that is still running. An owner that started before the last boot is dead
    /// whatever its pid has become, and needs no handle to say so. When its state cannot otherwise
    /// be determined (the process cannot be opened), it counts as alive: a second host must not
    /// restore a live host's settings.
    pub fn is_other_and_alive(&self) -> bool {
        self.pid != std::process::id() && self.still_running(identity(self.pid), boot_time())
    }
    fn still_running(&self, identity: Result<Option<u64>>, boot: Option<u64>) -> bool {
        if boot.is_some_and(|boot| self.started < boot) {
            return false;
        }
        match identity {
            Ok(started) => started == Some(self.started),
            Err(e) => {
                tracing::warn!(pid = self.pid, error = %format!("{e:#}"), "cannot tell whether the Radeon journal's owner is alive; assuming it is");
                true
            }
        }
    }
}

/// The rights the journal's mutex gives and every opener asks for: SYNCHRONIZE (0x0010_0000) to
/// wait for it and MUTEX_MODIFY_STATE (0x1) to release it.
const MUTEX_RIGHTS: u32 = 0x0010_0001;

/// When the system last booted, as a FILETIME, a little early so a clock that is slightly off
/// never makes a live owner look dead. `None` if the clock makes no sense.
fn boot_time() -> Option<u64> {
    // SAFETY: both calls only return the current time and the uptime.
    let (now, uptime_ms) = unsafe {
        let now = windows::Win32::System::SystemInformation::GetSystemTimeAsFileTime();
        (
            u64::from(now.dwLowDateTime) | (u64::from(now.dwHighDateTime) << 32),
            windows::Win32::System::SystemInformation::GetTickCount64(),
        )
    };
    const SLACK: u64 = 10 * 10_000_000;
    now.checked_sub(uptime_ms.checked_mul(10_000)?)?
        .checked_sub(SLACK)
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
    // `Global\`: the installed host runs as SYSTEM in the console session and a development host
    // as the user, possibly in another session; a `Local\` name would not be shared between them.
    // The mutex is created with an explicit descriptor that lets SYSTEM, Administrators and the
    // account that made it wait for it and release it, and nothing else, and every opener asks
    // for only those two rights. Another account gets an access error from CreateMutexExW, which
    // is loud, not a silent lock-less run.
    let name: Vec<u16> = format!(
        "Global\\Butterpollo.RadeonJournal.{:x}\0",
        u64::from_le_bytes(hash[..8].try_into().unwrap())
    )
    .encode_utf16()
    .collect();
    let sddl: Vec<u16> = format!(
        "D:(A;;{MUTEX_RIGHTS:#x};;;SY)(A;;{MUTEX_RIGHTS:#x};;;BA)(A;;{MUTEX_RIGHTS:#x};;;OW)\0"
    )
    .encode_utf16()
    .collect();
    // SAFETY: name and sddl are terminated and live through the calls. The descriptor
    // ConvertStringSecurityDescriptorToSecurityDescriptorW allocates is freed once with LocalFree
    // after CreateMutexExW has copied it. The returned handle is either closed on failure or
    // transferred to a guard after this thread acquires it.
    unsafe {
        use windows::Win32::Security::{
            Authorization::{
                ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
            },
            PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES,
        };
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            windows::core::PCWSTR(sddl.as_ptr()),
            SDDL_REVISION_1,
            &mut descriptor,
            None,
        )?;
        let attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.0,
            bInheritHandle: false.into(),
        };
        let created = CreateMutexExW(
            Some(&attributes),
            windows::core::PCWSTR(name.as_ptr()),
            0,
            MUTEX_RIGHTS,
        );
        let _ = windows::Win32::Foundation::LocalFree(Some(windows::Win32::Foundation::HLOCAL(
            descriptor.0,
        )));
        let handle = created?;
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
    fn an_owner_that_cannot_be_checked_counts_as_alive() {
        let owner = Owner {
            pid: 4_000_000,
            started: 7,
        };
        assert!(owner.still_running(Err(anyhow::anyhow!("access denied")), None));
        assert!(!owner.still_running(Ok(None), None));
        assert!(!owner.still_running(Ok(Some(8)), None));
        assert!(owner.still_running(Ok(Some(7)), None));
    }

    #[test]
    fn an_owner_from_before_the_boot_is_dead_without_a_handle() {
        let owner = Owner {
            pid: 4,
            started: 1_000,
        };
        // Even when the process cannot be opened, or a process now has that pid and start time.
        assert!(!owner.still_running(Err(anyhow::anyhow!("access denied")), Some(2_000)));
        assert!(!owner.still_running(Ok(Some(1_000)), Some(2_000)));
        // One that started after the boot is judged by its handle as before.
        assert!(owner.still_running(Err(anyhow::anyhow!("access denied")), Some(1_000)));
        assert!(owner.still_running(Ok(Some(1_000)), Some(500)));
    }

    #[test]
    fn the_boot_time_is_in_the_past_and_before_this_process_started() {
        let boot = boot_time().expect("a boot time");
        let started = identity(std::process::id()).unwrap().unwrap();
        assert!(boot < started, "boot {boot} process {started}");
        assert!(
            boot > 100_000_000_000_000_000,
            "{boot} is a plausible FILETIME"
        );
    }

    #[test]
    fn the_journal_mutex_grants_wait_and_release_and_nothing_else() {
        use windows::Win32::System::Threading::{OpenMutexW, SYNCHRONIZATION_ACCESS_RIGHTS};
        let path = std::env::temp_dir().join(format!("radeon-rights-{}.json", std::process::id()));
        let _held = lock(&path).unwrap();
        let hash = butterpollo_core::crypto::hash(path.to_string_lossy().to_lowercase().as_bytes());
        let name: Vec<u16> = format!(
            "Global\\Butterpollo.RadeonJournal.{:x}\0",
            u64::from_le_bytes(hash[..8].try_into().unwrap())
        )
        .encode_utf16()
        .collect();
        // SAFETY: name is terminated; each handle that opens is closed.
        unsafe {
            let ok = OpenMutexW(
                SYNCHRONIZATION_ACCESS_RIGHTS(MUTEX_RIGHTS),
                false,
                windows::core::PCWSTR(name.as_ptr()),
            )
            .expect("the wait and release rights are granted");
            let _ = CloseHandle(ok);
            // WRITE_DAC (0x40000) would let the opener widen the descriptor.
            let denied = OpenMutexW(
                SYNCHRONIZATION_ACCESS_RIGHTS(MUTEX_RIGHTS | 0x0004_0000),
                false,
                windows::core::PCWSTR(name.as_ptr()),
            );
            assert!(denied.is_err(), "WRITE_DAC must not be granted");
        }
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
