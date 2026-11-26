//! Suspending and resuming one process by identity, for Quick Suspend.
//!
//! A process is named by its id and creation time; it is opened once and its
//! creation time read through that handle, so an id that Windows gave to
//! another program since is never touched. Suspending uses ntdll's
//! `NtSuspendProcess` / `NtResumeProcess` (loaded from System32), which raise
//! and lower every thread's suspend count by one: a resume undoes exactly one
//! suspend. A resume of a process this host did not suspend (the journal is
//! written before the first suspend, so a crash in between leaves such a
//! record) lowers the count of a thread the game suspended itself; a thread
//! that runs stays running.
use super::{inspect::created_at, owned, raw};
use butterpollo_session::quick_suspend::{self, Resumed};
use std::{os::windows::io::OwnedHandle, sync::OnceLock};
use windows::Win32::{
    Foundation::*,
    System::{
        LibraryLoader::{GetModuleHandleW, GetProcAddress},
        Threading::*,
    },
};

/// Why a process could not be suspended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SuspendError {
    /// It exited, or its id now belongs to another process.
    Gone,
    /// Windows protects it (a protected process, or access was denied).
    Protected,
    /// Any other failure, with Windows' words.
    Os(String),
}

impl std::fmt::Display for SuspendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Gone => f.write_str("the process exited"),
            Self::Protected => f.write_str("Windows protects the process"),
            Self::Os(error) => f.write_str(error),
        }
    }
}

type NtCall = unsafe extern "system" fn(HANDLE) -> NTSTATUS;

/// `NtSuspendProcess` and `NtResumeProcess`, resolved once; None where ntdll
/// does not export them.
fn calls() -> Option<(NtCall, NtCall)> {
    static CALLS: OnceLock<Option<(usize, usize)>> = OnceLock::new();
    let (suspend, resume) = (*CALLS.get_or_init(|| {
        // SAFETY: ntdll is mapped into every process and never unloaded; the
        // names are NUL-terminated literals.
        unsafe {
            let module = GetModuleHandleW(windows::core::w!("ntdll.dll")).ok()?;
            let suspend = GetProcAddress(module, windows::core::s!("NtSuspendProcess"))?;
            let resume = GetProcAddress(module, windows::core::s!("NtResumeProcess"))?;
            Some((suspend as usize, resume as usize))
        }
    }))?;
    // SAFETY: both exports take a process handle and return an NTSTATUS.
    unsafe {
        Some((
            std::mem::transmute::<usize, NtCall>(suspend),
            std::mem::transmute::<usize, NtCall>(resume),
        ))
    }
}

/// The process `pid` opened for suspending, if it still runs as the process
/// created at `started` (100 ns since 1601).
fn open(pid: u32, started: u64) -> Result<Option<OwnedHandle>, SuspendError> {
    if pid <= 4 || started == 0 {
        return Ok(None);
    }
    // SAFETY: OpenProcess has no preconditions; the handle is owned at once.
    let process = match unsafe {
        OpenProcess(
            PROCESS_SUSPEND_RESUME | PROCESS_QUERY_LIMITED_INFORMATION,
            false,
            pid,
        )
    } {
        Ok(process) => owned(process),
        Err(error) if error.code() == ERROR_ACCESS_DENIED.to_hresult() => {
            // It may run under another identity we may not open either; only
            // a process we can see as ours counts as protected, and one whose
            // creation time we cannot read is no proof it is gone.
            return match super::creation_time(pid) {
                created if created == started => Err(SuspendError::Protected),
                0 => Err(SuspendError::Os(format!(
                    "process {pid} cannot be opened or identified: {error}"
                ))),
                _ => Ok(None),
            };
        }
        // No process has this id any more.
        Err(error) if error.code() == ERROR_INVALID_PARAMETER.to_hresult() => return Ok(None),
        Err(error) => {
            return Err(SuspendError::Os(format!(
                "process {pid} cannot be opened: {error}"
            )));
        }
    };
    Ok((created_at(raw(&process)) == started).then_some(process))
}

/// Whether Windows runs the process behind `process` protected (a protected
/// process or a protected process light, as anti-malware and some anti-cheat
/// services run).
fn is_protected(process: HANDLE) -> bool {
    let mut info = PROCESS_PROTECTION_LEVEL_INFORMATION::default();
    // SAFETY: `info` is a local of the size passed, and the handle is open.
    let read = unsafe {
        GetProcessInformation(
            process,
            ProcessProtectionLevelInfo,
            (&raw mut info).cast(),
            size_of::<PROCESS_PROTECTION_LEVEL_INFORMATION>() as u32,
        )
    };
    read.is_ok() && info.ProtectionLevel != PROTECTION_LEVEL_NONE
}

/// Whether the process `pid` created at `started` runs protected. One that
/// cannot be opened at all counts as protected.
pub fn protected(pid: u32, started: u64) -> bool {
    match open(pid, started) {
        Ok(Some(process)) => is_protected(raw(&process)),
        Ok(None) => false,
        Err(_) => true,
    }
}

/// Whether a process runs as the one created at `started`.
pub fn alive(pid: u32, started: u64) -> bool {
    started != 0 && super::creation_time(pid) == started
}

/// Suspend the process `pid` created at `started`: every thread stops until
/// [`resume`]. A protected process is refused before any call.
pub fn suspend(pid: u32, started: u64) -> Result<(), SuspendError> {
    let process = open(pid, started)?.ok_or(SuspendError::Gone)?;
    if is_protected(raw(&process)) {
        return Err(SuspendError::Protected);
    }
    let (suspend, _) = calls()
        .ok_or_else(|| SuspendError::Os("Windows offers no process suspend in ntdll.dll".into()))?;
    // SAFETY: the handle is open with PROCESS_SUSPEND_RESUME for the call.
    let status = unsafe { suspend(raw(&process)) };
    if status.is_ok() {
        Ok(())
    } else if status == STATUS_ACCESS_DENIED {
        Err(SuspendError::Protected)
    } else if status == STATUS_PROCESS_IS_TERMINATING {
        Err(SuspendError::Gone)
    } else {
        Err(SuspendError::Os(format!(
            "NtSuspendProcess failed ({:#010x})",
            status.0
        )))
    }
}

/// Resume the process `pid` created at `started` once; one that exited or
/// whose id was reused is left alone ([`Resumed::Gone`]).
pub fn resume(pid: u32, started: u64) -> Result<Resumed, String> {
    let process = match open(pid, started) {
        Ok(Some(process)) => process,
        Ok(None) | Err(SuspendError::Gone) => return Ok(Resumed::Gone),
        Err(error) => return Err(error.to_string()),
    };
    let (_, resume) =
        calls().ok_or_else(|| "Windows offers no process resume in ntdll.dll".to_owned())?;
    // SAFETY: the handle is open with PROCESS_SUSPEND_RESUME for the call.
    let status = unsafe { resume(raw(&process)) };
    if status.is_ok() {
        Ok(Resumed::Done)
    } else if status == STATUS_PROCESS_IS_TERMINATING {
        Ok(Resumed::Gone)
    } else {
        Err(format!("NtResumeProcess failed ({:#010x})", status.0))
    }
}

/// [`resume`] for [`quick_suspend::recover`].
pub struct Thaw;

impl quick_suspend::Thaw for Thaw {
    fn resume(&mut self, pid: u32, started: u64) -> Result<Resumed, String> {
        resume(pid, started)
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    /// How often the first thread of `pid` is suspended: SuspendThread answers
    /// with the count before its own, which ResumeThread then takes back.
    pub(in super::super) fn suspend_count(pid: u32) -> u32 {
        use windows::Win32::System::Diagnostics::ToolHelp::*;
        // SAFETY: `entry` has dwSize set to its size; the snapshot and thread handles are
        // owned and open for the calls.
        unsafe {
            let snapshot = owned(CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0).unwrap());
            let mut entry = THREADENTRY32 {
                dwSize: size_of::<THREADENTRY32>() as u32,
                ..Default::default()
            };
            let mut more = Thread32First(raw(&snapshot), &mut entry).is_ok();
            while more {
                if entry.th32OwnerProcessID == pid {
                    let thread = owned(
                        OpenThread(THREAD_SUSPEND_RESUME, false, entry.th32ThreadID).unwrap(),
                    );
                    let previous = SuspendThread(raw(&thread));
                    ResumeThread(raw(&thread));
                    return previous;
                }
                more = Thread32Next(raw(&snapshot), &mut entry).is_ok();
            }
        }
        panic!("process {pid} has no thread");
    }

    /// A child that does nothing for a minute, ended when the guard drops.
    pub(in super::super) struct Sleeper(pub std::process::Child);
    impl Drop for Sleeper {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    pub(in super::super) fn sleeper() -> Sleeper {
        use std::os::windows::process::CommandExt;
        // `ping` to the loopback address only: no network, no firewall prompt.
        let child = std::process::Command::new("ping")
            .args(["-n", "60", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .spawn()
            .unwrap();
        Sleeper(child)
    }

    #[test]
    fn a_process_is_suspended_and_resumed_by_identity_only() {
        let child = sleeper();
        let pid = child.0.id();
        let started = super::super::creation_time(pid);
        assert_ne!(started, 0);
        assert!(alive(pid, started));
        // A creation time that does not match: another process with this id.
        assert_eq!(suspend(pid, started + 1), Err(SuspendError::Gone));
        assert_eq!(resume(pid, started + 1), Ok(Resumed::Gone));
        assert!(!protected(pid, started));
        assert_eq!(suspend_count(pid), 0);
        suspend(pid, started).unwrap();
        assert_eq!(suspend_count(pid), 1);
        assert_eq!(resume(pid, started), Ok(Resumed::Done));
        assert_eq!(suspend_count(pid), 0);
        // A second resume of a running process changes nothing.
        assert_eq!(resume(pid, started), Ok(Resumed::Done));
        assert_eq!(suspend_count(pid), 0);
    }

    #[test]
    fn system_ids_and_unknown_processes_are_never_touched() {
        assert_eq!(suspend(4, 1), Err(SuspendError::Gone));
        assert_eq!(suspend(0, 1), Err(SuspendError::Gone));
        assert_eq!(resume(4, 1), Ok(Resumed::Gone));
        assert!(!alive(4, 0));
    }
}
