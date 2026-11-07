//! `gamepad_driver_host_priority`, off by default: while a VHF pad is
//! plugged, the WUDFHost that hosts the gamepad driver runs at
//! HIGH_PRIORITY_CLASS. Each pad state is a blocking call into that process,
//! which at normal priority stalled up to 22 ms behind a normal game load
//! and 229-279 ms behind a HIGHEST one (`rust/PERFORMANCE.md`, "Virtual
//! gamepad calls"). It changes another process, and with device pooling the
//! other drivers WUDFHost hosts with it, so it waits for an A/B; the log
//! names those drivers. The previous class comes back when the last pad of
//! every session leaves. A host that dies meanwhile leaves the class raised
//! until that WUDFHost restarts.
use anyhow::{Context, Result, bail};
use std::{
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    sync::Mutex,
};
use windows::Win32::{
    Foundation::{ERROR_NO_MORE_FILES, HANDLE},
    System::{
        Diagnostics::ToolHelp::*,
        Threading::{
            GetPriorityClass, HIGH_PRIORITY_CLASS, OpenProcess, PROCESS_CREATION_FLAGS,
            PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_INFORMATION, SetPriorityClass,
        },
    },
};

/// The VHF gamepad driver's UMDF module (VibeshineVhfGamepad.inf).
const DRIVER_DLL: &str = "VibeshineVhfGamepad.dll";

/// Raised once for every holder, put back when the last one goes.
struct Counted<R> {
    holders: usize,
    raised: Option<R>,
}
impl<R> Counted<R> {
    const fn new() -> Self {
        Self {
            holders: 0,
            raised: None,
        }
    }
    fn acquire(&mut self, raise: impl FnOnce() -> Result<R>) {
        self.holders += 1;
        if self.holders > 1 {
            return;
        }
        match raise() {
            Ok(raised) => self.raised = Some(raised),
            // Never fatal: the pads work at the old priority.
            Err(error) => tracing::warn!(
                error = format!("{error:#}"),
                "gamepad_driver_host_priority: the gamepad driver's WUDFHost was not raised"
            ),
        }
    }
    fn release(&mut self) {
        self.holders = self.holders.saturating_sub(1);
        if self.holders == 0 {
            // Dropping it restores the class.
            self.raised = None;
        }
    }
}

static HOST: Mutex<Counted<Raised>> = Mutex::new(Counted::new());

/// Holds the gamepad driver's WUDFHost at high priority while it lives.
pub(super) struct DriverHostPriority(());
impl DriverHostPriority {
    pub(super) fn acquire() -> Self {
        HOST.lock().unwrap().acquire(raise);
        Self(())
    }
}
impl Drop for DriverHostPriority {
    fn drop(&mut self) {
        HOST.lock().unwrap().release();
    }
}

struct Raised {
    /// Held open, so the id cannot pass to another process meanwhile.
    process: OwnedHandle,
    pid: u32,
    previous: PROCESS_CREATION_FLAGS,
}
impl Drop for Raised {
    fn drop(&mut self) {
        // SAFETY: `process` is an open handle with PROCESS_SET_INFORMATION.
        match unsafe { SetPriorityClass(HANDLE(self.process.as_raw_handle()), self.previous) } {
            Ok(()) => tracing::info!(
                pid = self.pid,
                class = format!("{:#x}", self.previous.0),
                "gamepad_driver_host_priority: the gamepad driver's WUDFHost priority restored"
            ),
            Err(error) => tracing::warn!(
                pid = self.pid,
                class = format!("{:#x}", self.previous.0),
                %error,
                "gamepad_driver_host_priority: the gamepad driver's WUDFHost priority could not be restored"
            ),
        }
    }
}

fn raise() -> Result<Raised> {
    let (pid, others) = find_host()?;
    // SAFETY: OpenProcess takes no pointers; the handle goes to an OwnedHandle.
    let process = unsafe {
        OwnedHandle::from_raw_handle(
            OpenProcess(
                PROCESS_SET_INFORMATION | PROCESS_QUERY_LIMITED_INFORMATION,
                false,
                pid,
            )
            .with_context(|| format!("opening WUDFHost {pid}"))?
            .0,
        )
    };
    let handle = HANDLE(process.as_raw_handle());
    // SAFETY: `handle` is open with PROCESS_QUERY_LIMITED_INFORMATION.
    let previous = PROCESS_CREATION_FLAGS(unsafe { GetPriorityClass(handle) });
    if previous.0 == 0 {
        return Err(windows::core::Error::from_thread())
            .with_context(|| format!("reading WUDFHost {pid}'s priority class"));
    }
    // SAFETY: `handle` is open with PROCESS_SET_INFORMATION.
    unsafe { SetPriorityClass(handle, HIGH_PRIORITY_CLASS) }
        .with_context(|| format!("raising WUDFHost {pid}"))?;
    tracing::info!(
        pid,
        previous = format!("{:#x}", previous.0),
        co_hosted = ?others,
        "gamepad_driver_host_priority: the gamepad driver's WUDFHost raised to high priority; the drivers it also hosts run raised too"
    );
    Ok(Raised {
        process,
        pid,
        previous,
    })
}

/// The WUDFHost with the gamepad driver loaded, and the other drivers it
/// hosts.
fn find_host() -> Result<(u32, Vec<String>)> {
    let mut last_error = None;
    for pid in processes("wudfhost.exe")? {
        match modules(pid) {
            Ok(paths) => {
                if let Some(others) = co_hosted(&paths) {
                    return Ok((pid, others));
                }
            }
            // Another WUDFHost that won't list its modules is not ours to judge.
            Err(error) => last_error = Some(error),
        }
    }
    match last_error {
        Some(error) => Err(error.context(format!("no WUDFHost has {DRIVER_DLL} loaded"))),
        None => bail!("no WUDFHost has {DRIVER_DLL} loaded"),
    }
}

/// Driver modules besides the gamepad driver in a WUDFHost whose module
/// paths these are; None when the gamepad driver isn't among them.
fn co_hosted(paths: &[String]) -> Option<Vec<String>> {
    let name = |path: &str| path.rsplit(['\\', '/']).next().unwrap_or(path).to_owned();
    if !paths
        .iter()
        .any(|path| name(path).eq_ignore_ascii_case(DRIVER_DLL))
    {
        return None;
    }
    Some(
        paths
            .iter()
            .filter(|path| {
                let lower = path.to_ascii_lowercase();
                lower.contains("\\driverstore\\") || lower.contains("\\drivers\\umdf\\")
            })
            .map(|path| name(path))
            .filter(|module| !module.eq_ignore_ascii_case(DRIVER_DLL))
            .collect(),
    )
}

fn text(chars: &[u16]) -> String {
    let end = chars.iter().position(|&c| c == 0).unwrap_or(chars.len());
    String::from_utf16_lossy(&chars[..end])
}

struct Snapshot(HANDLE);
impl Drop for Snapshot {
    fn drop(&mut self) {
        // SAFETY: the snapshot handle is open and closed only here.
        let _ = unsafe { windows::Win32::Foundation::CloseHandle(self.0) };
    }
}

/// The ids of the processes named `exe`, case-insensitively.
fn processes(exe: &str) -> Result<Vec<u32>> {
    // SAFETY: the snapshot is closed by Snapshot; `entry` is sized for the calls.
    unsafe {
        let snapshot = Snapshot(CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)?);
        let mut entry = PROCESSENTRY32W {
            dwSize: size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut pids = vec![];
        let mut next = Process32FirstW(snapshot.0, &mut entry);
        loop {
            match next {
                Ok(()) => {
                    if text(&entry.szExeFile).eq_ignore_ascii_case(exe) {
                        pids.push(entry.th32ProcessID);
                    }
                }
                Err(error) if error.code() == ERROR_NO_MORE_FILES.to_hresult() => break,
                Err(error) => return Err(error).context("listing processes"),
            }
            next = Process32NextW(snapshot.0, &mut entry);
        }
        Ok(pids)
    }
}

/// The paths of the modules process `pid` has loaded.
fn modules(pid: u32) -> Result<Vec<String>> {
    // SAFETY: the snapshot is closed by Snapshot; `entry` is sized for the calls.
    unsafe {
        let snapshot = Snapshot(
            CreateToolhelp32Snapshot(TH32CS_SNAPMODULE, pid)
                .with_context(|| format!("listing WUDFHost {pid}'s modules"))?,
        );
        let mut entry = MODULEENTRY32W {
            dwSize: size_of::<MODULEENTRY32W>() as u32,
            ..Default::default()
        };
        let mut paths = vec![];
        let mut next = Module32FirstW(snapshot.0, &mut entry);
        loop {
            match next {
                Ok(()) => paths.push(text(&entry.szExePath)),
                Err(error) if error.code() == ERROR_NO_MORE_FILES.to_hresult() => break,
                Err(error) => {
                    return Err(error).with_context(|| format!("listing WUDFHost {pid}'s modules"));
                }
            }
            next = Module32NextW(snapshot.0, &mut entry);
        }
        Ok(paths)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, atomic::AtomicUsize, atomic::Ordering};

    /// Counts its restore on drop.
    struct Fake(Arc<AtomicUsize>);
    impl Drop for Fake {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn the_host_is_raised_once_for_all_sessions_and_restored_after_the_last() {
        let restored = Arc::new(AtomicUsize::new(0));
        let mut raises = 0;
        let mut counted = Counted::new();
        for _ in 0..2 {
            counted.acquire(|| {
                raises += 1;
                Ok(Fake(restored.clone()))
            });
        }
        assert_eq!(raises, 1);
        counted.release();
        assert_eq!(
            restored.load(Ordering::SeqCst),
            0,
            "one session still has a pad"
        );
        counted.release();
        assert_eq!(restored.load(Ordering::SeqCst), 1);
        // A release too many changes nothing; the next session raises again.
        counted.release();
        counted.acquire(|| {
            raises += 1;
            Ok(Fake(restored.clone()))
        });
        assert_eq!(raises, 2);
        drop(counted);
        assert_eq!(restored.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_failed_raise_is_not_fatal_and_the_next_session_tries_again() {
        let mut counted = Counted::<Fake>::new();
        counted.acquire(|| bail!("access denied"));
        assert!(counted.raised.is_none());
        counted.release();
        let restored = Arc::new(AtomicUsize::new(0));
        counted.acquire(|| Ok(Fake(restored.clone())));
        assert!(counted.raised.is_some());
    }

    #[test]
    fn the_gamepad_drivers_host_is_found_by_its_module_and_names_its_neighbours() {
        let paths = |list: &[&str]| list.iter().map(|p| p.to_string()).collect::<Vec<_>>();
        let store = r"C:\Windows\System32\DriverStore\FileRepository";
        assert_eq!(
            co_hosted(&paths(&[
                r"C:\Windows\System32\WUDFHost.exe",
                r"C:\Windows\System32\ntdll.dll",
                &format!(r"{store}\vibeshinevhfgamepad.inf_amd64_1\VIBESHINEVHFGAMEPAD.DLL"),
                &format!(r"{store}\other.inf_amd64_2\OtherSensor.dll"),
                r"C:\Windows\System32\drivers\UMDF\WpdFs.dll",
            ])),
            Some(vec!["OtherSensor.dll".into(), "WpdFs.dll".into()])
        );
        assert_eq!(
            co_hosted(&paths(&[
                r"C:\Windows\System32\WUDFHost.exe",
                &format!(r"{store}\other.inf_amd64_2\OtherSensor.dll"),
            ])),
            None
        );
    }
}
