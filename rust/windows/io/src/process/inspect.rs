//! Process inspection, audio-process handles, command-line parsing, and bulk termination.
use super::{
    listing, owned, raw,
    session::{current_session, process_session},
};
use anyhow::Result;
use std::{collections::BTreeMap, mem::size_of, os::windows::io::OwnedHandle, time::Duration};
use windows::{
    Win32::{Foundation::*, System::Threading::*},
    core::{BOOL, PWSTR},
};

/// Read-only process identity held open throughout audio capture. Keeping the
/// process object alive prevents PID reuse between verification and activation.
#[derive(Clone)]
pub struct AudioProcess {
    pub identity: butterpollo_session::audio_scope::Identity,
    handle: std::sync::Arc<OwnedHandle>,
}
impl AudioProcess {
    pub fn open(identity: butterpollo_session::audio_scope::Identity) -> Result<Self> {
        // SAFETY: a newly opened query/synchronize-only handle is owned once;
        // the out-pointers are live FILETIMEs and no process is modified.
        unsafe {
            let handle = owned(OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                false,
                identity.pid,
            )?);
            let mut created = FILETIME::default();
            let (mut exited, mut kernel, mut user) = (created, created, created);
            GetProcessTimes(
                raw(&handle),
                &mut created,
                &mut exited,
                &mut kernel,
                &mut user,
            )?;
            let started =
                u64::from(created.dwHighDateTime) << 32 | u64::from(created.dwLowDateTime);
            anyhow::ensure!(
                started == identity.started && started != 0,
                "game process identity changed"
            );
            anyhow::ensure!(
                process_session(identity.pid)? == current_session()?,
                "game process belongs to another Windows session"
            );
            let target = Self {
                identity,
                handle: std::sync::Arc::new(handle),
            };
            anyhow::ensure!(target.alive(), "game process exited before audio capture");
            Ok(target)
        }
    }
    pub fn alive(&self) -> bool {
        // SAFETY: the read-only process handle stays open; zero timeout never blocks.
        unsafe { WaitForSingleObject(raw(&self.handle), 0) == WAIT_TIMEOUT }
    }
}

/// The program a command line starts: its first argument, without quotes.
pub fn command_target(command: &str) -> &str {
    let command = command.trim_start();
    match command.strip_prefix('"') {
        Some(rest) => rest.split('"').next().unwrap_or(rest),
        None => command.split(char::is_whitespace).next().unwrap_or(""),
    }
}
/// What a command line passes its program: everything after
/// [`command_target`], trimmed.
pub fn command_arguments(command: &str) -> &str {
    let command = command.trim_start();
    let rest = match command.strip_prefix('"') {
        Some(rest) => rest.split_once('"').map_or("", |(_, rest)| rest),
        None => command
            .split_once(char::is_whitespace)
            .map_or("", |(_, rest)| rest),
    };
    rest.trim()
}
/// Whether Windows must open the target through its association: a URL
/// (`steam://`, `ms-settings:`) or a file that is not a program. cmd.exe
/// cannot start a URL, so Steam Big Picture and store links failed.
fn opens_through_shell(target: &str) -> bool {
    if let Some((scheme, _)) = target.split_once(':')
        && scheme.len() > 1
        && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    {
        return true;
    }
    let name = target.rsplit(['\\', '/']).next().unwrap_or(target);
    match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => !matches!(
            extension.to_ascii_lowercase().as_str(),
            "exe" | "com" | "bat" | "cmd"
        ),
        _ => false,
    }
}
/// The command for cmd.exe. URLs and documents are opened as Windows opens
/// them (as Vibepollo resolves associations), waiting for the program they
/// start, which stays in the app's job.
pub fn shell_command(command: &str) -> String {
    let target = command_target(command);
    if target.is_empty() || !opens_through_shell(target) {
        return command.to_owned();
    }
    let trimmed = command.trim_start();
    let rest = if trimmed.starts_with('"') {
        &trimmed[(target.len() + 2).min(trimmed.len())..]
    } else {
        &trimmed[target.len()..]
    };
    format!("start \"\" /wait \"{target}\"{rest}")
}

/// The current time on the clock of [`creation_time`] (100 ns since 1601).
pub fn now() -> u64 {
    // SAFETY: GetSystemTimeAsFileTime has no preconditions.
    let time = unsafe { windows::Win32::System::SystemInformation::GetSystemTimeAsFileTime() };
    u64::from(time.dwHighDateTime) << 32 | u64::from(time.dwLowDateTime)
}
/// A process's creation time (100 ns since 1601), 0 when it cannot be read.
pub fn creation_time(pid: u32) -> u64 {
    // SAFETY: the process handle is owned and open for the call.
    unsafe {
        let Ok(process) = OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
            false,
            pid,
        )
        .or_else(|_| OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid)) else {
            return 0;
        };
        let process = owned(process);
        created_at(raw(&process))
    }
}
/// The creation time (100 ns since 1601) of the process behind `process`, a handle opened with
/// `PROCESS_QUERY_LIMITED_INFORMATION` (and ideally `PROCESS_SYNCHRONIZE`); 0 when it cannot be
/// read or the process has exited.
///
/// An exited process can still be opened while any other program (a launcher, Steam, an
/// audio session) holds a handle to it, so being able to open it does not mean it runs.
/// Probing `WaitForSingleObject(process, 0)` first distinguishes a process that exited with
/// code `259` (`STILL_ACTIVE`) from one that is still running.
pub(super) fn created_at(process: windows::Win32::Foundation::HANDLE) -> u64 {
    // SAFETY: the caller's handle is open for the call; zero timeout never blocks. If the
    // handle lacks `PROCESS_SYNCHRONIZE`, `WaitForSingleObject` returns `WAIT_FAILED` (not
    // `WAIT_OBJECT_0`) and the exit-code check below still runs.
    if unsafe { WaitForSingleObject(process, 0) } == WAIT_OBJECT_0 {
        return 0;
    }
    let mut created = FILETIME::default();
    let (mut exited, mut kernel, mut user) = (created, created, created);
    // SAFETY: the caller's handle is open for the call, and each out-pointer is a local FILETIME.
    let read =
        unsafe { GetProcessTimes(process, &mut created, &mut exited, &mut kernel, &mut user) };
    if read.is_err() {
        return 0;
    }
    let mut code = 0;
    // SAFETY: the caller's handle is open for the call, and `code` is a local u32. An exit code
    // that cannot be read, or one that is STILL_ACTIVE, keeps the process counted as running.
    if unsafe { GetExitCodeProcess(process, &mut code) }.is_ok() && code != STILL_ACTIVE.0 as u32 {
        return 0;
    }
    u64::from(created.dwHighDateTime) << 32 | u64::from(created.dwLowDateTime)
}

/// A process's sampled CPU and resident memory usage.
pub struct Usage {
    pub started: u64,
    /// Kernel and user CPU time in 100 ns units.
    pub cpu_time: u64,
    pub working_set: u64,
}
/// CPU time and resident memory, with the creation time to reject reused IDs.
pub fn usage(pid: u32) -> Option<Usage> {
    use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    // SAFETY: the process handle is owned, and the time and memory buffers stay valid for each
    // call. The memory buffer's size matches the type passed to GetProcessMemoryInfo.
    unsafe {
        let process = owned(
            OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, false, pid)
                .or_else(|_| OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid))
                .ok()?,
        );
        let mut created = FILETIME::default();
        let (mut exited, mut kernel, mut user) = (created, created, created);
        GetProcessTimes(
            raw(&process),
            &mut created,
            &mut exited,
            &mut kernel,
            &mut user,
        )
        .ok()?;
        let mut memory = PROCESS_MEMORY_COUNTERS::default();
        let got_memory = GetProcessMemoryInfo(
            raw(&process),
            &mut memory,
            size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        )
        .is_ok();
        let ticks =
            |time: FILETIME| u64::from(time.dwHighDateTime) << 32 | u64::from(time.dwLowDateTime);
        Some(Usage {
            started: ticks(created),
            cpu_time: ticks(kernel) + ticks(user),
            working_set: if got_memory {
                memory.WorkingSetSize as u64
            } else {
                0
            },
        })
    }
}

/// Every running process with its parent, creation time and program name. The creation time is 0
/// for a process this one may not open.
pub fn processes() -> Result<Vec<butterpollo_session::steam::Process>> {
    static LISTING: std::sync::LazyLock<std::sync::Mutex<listing::Listing>> =
        std::sync::LazyLock::new(Default::default);
    static SNAPSHOT: std::sync::Once = std::sync::Once::new();
    let listed = LISTING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .processes(creation_time);
    match listed {
        Some(list) => Ok(list),
        None => {
            SNAPSHOT.call_once(|| {
                tracing::warn!(
                    "Windows gave no process list in one call; every process is opened for each list instead"
                );
            });
            snapshot_processes()
        }
    }
}
/// [`processes`] from a Toolhelp snapshot, opening every process for its creation time: the way
/// that works where the kernel's list cannot be read.
pub(super) fn snapshot_processes() -> Result<Vec<butterpollo_session::steam::Process>> {
    use windows::Win32::System::Diagnostics::ToolHelp::*;
    let mut list = vec![];
    // SAFETY: `entry` has dwSize set to its size and the owned snapshot handle stays open for the
    // loop.
    unsafe {
        let snapshot = owned(CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)?);
        let mut entry = PROCESSENTRY32W {
            dwSize: size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut more = Process32FirstW(raw(&snapshot), &mut entry).is_ok();
        while more {
            let length = entry
                .szExeFile
                .iter()
                .position(|c| *c == 0)
                .unwrap_or(entry.szExeFile.len());
            if entry.th32ProcessID != 0 {
                list.push(butterpollo_session::steam::Process {
                    pid: entry.th32ProcessID,
                    parent: entry.th32ParentProcessID,
                    started: creation_time(entry.th32ProcessID),
                    name: String::from_utf16_lossy(&entry.szExeFile[..length]),
                });
            }
            more = Process32NextW(raw(&snapshot), &mut entry).is_ok();
        }
    }
    Ok(list)
}

/// A process's full program path.
pub fn image_path(pid: u32) -> Option<String> {
    // SAFETY: `buffer` outlives the call and `size` is its capacity in UTF-16 units, updated to the
    // length written.
    unsafe {
        let process = owned(OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?);
        let mut buffer = [0u16; 32768];
        let mut size = buffer.len() as u32;
        QueryFullProcessImageNameW(
            raw(&process),
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &mut size,
        )
        .ok()?;
        Some(String::from_utf16_lossy(&buffer[..size as usize]))
    }
}

/// Ask the windows of `processes` (pid → creation time) to close, wait up to
/// `timeout` for them to exit, then end the rest. An id that now belongs to
/// another process is left alone.
pub fn stop_processes(processes: &BTreeMap<u32, u64>, timeout: Duration) {
    use windows::Win32::UI::WindowsAndMessaging::*;
    let alive = |processes: &BTreeMap<u32, u64>| -> Vec<u32> {
        processes
            .iter()
            .filter(|(pid, started)| creation_time(**pid) == **started && **started != 0)
            .map(|(pid, _)| *pid)
            .collect()
    };
    unsafe extern "system" fn close_window(window: HWND, data: LPARAM) -> BOOL {
        // SAFETY: EnumWindows passes the `&Vec<u32>` from stop_processes as `data`, which outlives
        // the enumeration.
        unsafe {
            let ids = &*(data.0 as *const Vec<u32>);
            let mut pid = 0;
            GetWindowThreadProcessId(window, Some(&mut pid));
            if ids.contains(&pid) {
                let _ = PostMessageW(Some(window), WM_CLOSE, WPARAM(0), LPARAM(0));
            }
            BOOL(1)
        }
    }
    let ids = alive(processes);
    if ids.is_empty() {
        return;
    }
    // SAFETY: `ids` outlives the synchronous EnumWindows call that reads it through `close_window`.
    unsafe {
        let _ = EnumWindows(
            Some(close_window),
            LPARAM((&ids as *const Vec<u32>) as isize),
        );
    }
    let deadline = std::time::Instant::now() + timeout;
    while !alive(processes).is_empty() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    for (&pid, &started) in processes.iter().filter(|(_, started)| **started != 0) {
        // The id may have gone to another process since the last look: the handle that ends
        // the process is the one its creation time is read through.
        // SAFETY: the handle OpenProcess returns is owned and closed once on drop.
        unsafe {
            let access =
                PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE;
            if let Ok(process) = OpenProcess(access, false, pid).or_else(|_| {
                OpenProcess(
                    PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION,
                    false,
                    pid,
                )
            }) {
                let process = owned(process);
                if created_at(raw(&process)) == started {
                    let _ = TerminateProcess(raw(&process), 1);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::windows::{io::AsRawHandle, process::CommandExt};

    #[test]
    fn exited_process_with_still_active_exit_code_259_is_not_counted_as_running() {
        let mut child = std::process::Command::new("cmd.exe")
            .args(["/c", "exit", "/b", "259"])
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .spawn()
            .unwrap();
        let handle = HANDLE(child.as_raw_handle());
        // Wait for exit while still holding the Child handle so the exited process object stays open.
        // SAFETY: `handle` is owned by `child` and stays live across the calls.
        unsafe {
            assert_eq!(WaitForSingleObject(handle, 10_000), WAIT_OBJECT_0);
            let mut code = 0u32;
            GetExitCodeProcess(handle, &mut code).unwrap();
            assert_eq!(code, STILL_ACTIVE.0 as u32);
        }
        assert_eq!(created_at(handle), 0);
        assert_eq!(creation_time(child.id()), 0);
        let _ = child.wait();
    }
}
