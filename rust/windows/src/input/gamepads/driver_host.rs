//! `gamepad_driver_host_priority`, off by default: while a VHF pad is
//! plugged, the WUDFHost that hosts the gamepad driver runs at
//! HIGH_PRIORITY_CLASS. Each pad state is a blocking call into that process,
//! which at normal priority stalled up to 22 ms behind a normal game load
//! and 229-279 ms behind a HIGHEST one (`rust/PERFORMANCE.md`, "Virtual
//! gamepad calls"). It changes another process, and with device pooling the
//! other drivers WUDFHost hosts with it, so it waits for an A/B; the log
//! names those drivers. The previous class comes back when the last pad of
//! every session leaves, unless something else changed it meanwhile. A
//! WUDFHost that restarts is raised again; a host that dies leaves the class
//! raised until that WUDFHost restarts.
//!
//! The process and module scans and the restore run on a short helper
//! thread, never on the gamepad thread or under the lock its pads take.
use anyhow::{Context, Result, bail};
use std::{
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    sync::{Arc, LazyLock, Mutex},
    time::{Duration, Instant},
};
use windows::Win32::{
    Foundation::{ERROR_NO_MORE_FILES, HANDLE, WAIT_TIMEOUT},
    System::{
        Diagnostics::ToolHelp::*,
        Threading::{
            GetPriorityClass, HIGH_PRIORITY_CLASS, OpenProcess, PROCESS_CREATION_FLAGS,
            PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_INFORMATION, PROCESS_SYNCHRONIZE,
            REALTIME_PRIORITY_CLASS, SetPriorityClass, WaitForSingleObject,
        },
    },
};

/// The VHF gamepad driver's UMDF module (VibeshineVhfGamepad.inf).
const DRIVER_DLL: &str = "VibeshineVhfGamepad.dll";
/// How often a holder checks that the raised WUDFHost still runs.
const CHECK_EVERY: Duration = Duration::from_secs(1);

/// A raised process: dropping it restores what it raised.
pub(super) trait Host: Sized + Send + 'static {
    fn raise() -> Result<Self>;
    fn alive(&self) -> bool;
}

struct State<R> {
    holders: usize,
    raised: Option<R>,
    /// The raise failed for these holders: not retried until they all left.
    failed: bool,
    /// A helper thread is bringing `raised` in line with `holders`.
    busy: bool,
    checked: Option<Instant>,
}
/// Raised once for every holder, put back when the last one goes.
struct Holders<R> {
    state: Mutex<State<R>>,
}
impl<R: Host> Holders<R> {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(State {
                holders: 0,
                raised: None,
                failed: false,
                busy: false,
                checked: None,
            }),
        })
    }
    fn acquire(self: &Arc<Self>) -> Hold<R> {
        let mut state = self.state.lock().unwrap();
        state.holders += 1;
        self.kick(&mut state);
        Hold(self.clone())
    }
    fn release(self: &Arc<Self>) {
        let mut state = self.state.lock().unwrap();
        state.holders = state.holders.saturating_sub(1);
        if state.holders == 0 {
            state.failed = false;
        }
        self.kick(&mut state);
    }
    /// A raised WUDFHost that exited is raised again in its new process.
    fn check(self: &Arc<Self>, now: Instant) {
        let mut state = self.state.lock().unwrap();
        if state
            .checked
            .is_some_and(|at| now.saturating_duration_since(at) < CHECK_EVERY)
        {
            return;
        }
        state.checked = Some(now);
        if state.raised.as_ref().is_some_and(|raised| !raised.alive()) {
            tracing::info!(
                "gamepad_driver_host_priority: the gamepad driver's WUDFHost exited; raising its new one"
            );
            let gone = state.raised.take();
            self.kick(&mut state);
            drop(state);
            drop(gone);
        }
    }
    /// Start the helper unless one runs; it sees every change made before
    /// it clears `busy`.
    fn kick(self: &Arc<Self>, state: &mut State<R>) {
        if state.busy {
            return;
        }
        state.busy = true;
        let holders = self.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("gamepad-driver-host".into())
            .spawn(move || holders.sync())
        {
            state.busy = false;
            tracing::warn!(%error, "gamepad_driver_host_priority: could not start its helper thread");
        }
    }
    fn sync(&self) {
        loop {
            let mut state = self.state.lock().unwrap();
            let wanted = state.holders > 0 && !state.failed;
            match (wanted, state.raised.is_some()) {
                (true, false) => {
                    drop(state);
                    let raised = R::raise();
                    let mut state = self.state.lock().unwrap();
                    match raised {
                        Ok(raised) => state.raised = Some(raised),
                        Err(error) => {
                            // Never fatal: the pads work at the old priority.
                            tracing::warn!(
                                error = format!("{error:#}"),
                                "gamepad_driver_host_priority: the gamepad driver's WUDFHost was not raised"
                            );
                            state.failed = true;
                        }
                    }
                }
                (false, true) => {
                    let raised = state.raised.take();
                    drop(state);
                    // Dropping it restores the class, outside the lock.
                    drop(raised);
                }
                _ => {
                    state.busy = false;
                    return;
                }
            }
        }
    }
}

/// What a session's pads hold while the raise should last.
pub(super) trait Held: Send {
    /// Notice a raised process that exited, at most every CHECK_EVERY.
    fn check(&self) {}
}
/// Holds a raise while it lives.
pub(super) struct Hold<R: Host>(Arc<Holders<R>>);
impl<R: Host> Held for Hold<R> {
    fn check(&self) {
        self.0.check(Instant::now());
    }
}
impl<R: Host> Drop for Hold<R> {
    fn drop(&mut self) {
        self.0.release();
    }
}

static HOST: LazyLock<Arc<Holders<Raised>>> = LazyLock::new(Holders::new);
/// Holds the gamepad driver's WUDFHost at high priority while it lives.
pub(super) fn acquire() -> Box<dyn Held> {
    Box::new(HOST.acquire())
}

pub(super) struct Raised {
    /// Held open, so the id cannot pass to another process meanwhile.
    process: OwnedHandle,
    pid: u32,
    previous: PROCESS_CREATION_FLAGS,
}
fn priority_class(process: HANDLE) -> Result<PROCESS_CREATION_FLAGS> {
    // SAFETY: `process` is open with PROCESS_QUERY_LIMITED_INFORMATION.
    match unsafe { GetPriorityClass(process) } {
        0 => Err(windows::core::Error::from_thread().into()),
        class => Ok(PROCESS_CREATION_FLAGS(class)),
    }
}
impl Host for Raised {
    fn raise() -> Result<Self> {
        let (pid, others) = find_host()?;
        // SAFETY: OpenProcess takes no pointers; the handle goes to an OwnedHandle.
        let process = unsafe {
            OwnedHandle::from_raw_handle(
                OpenProcess(
                    PROCESS_SET_INFORMATION
                        | PROCESS_QUERY_LIMITED_INFORMATION
                        | PROCESS_SYNCHRONIZE,
                    false,
                    pid,
                )
                .with_context(|| format!("opening WUDFHost {pid}"))?
                .0,
            )
        };
        let handle = HANDLE(process.as_raw_handle());
        let previous = priority_class(handle)
            .with_context(|| format!("reading WUDFHost {pid}'s priority class"))?;
        // Raised already, by another host on this PC or by hand: it is not
        // ours to put back, and saving it would restore high priority.
        if matches!(previous, HIGH_PRIORITY_CLASS | REALTIME_PRIORITY_CLASS) {
            bail!(
                "WUDFHost {pid} already runs at class {:#x}; left as it is",
                previous.0
            );
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
        Ok(Self {
            process,
            pid,
            previous,
        })
    }
    fn alive(&self) -> bool {
        // SAFETY: `process` is open with PROCESS_SYNCHRONIZE.
        unsafe { WaitForSingleObject(HANDLE(self.process.as_raw_handle()), 0) == WAIT_TIMEOUT }
    }
}
impl Drop for Raised {
    fn drop(&mut self) {
        // An exited WUDFHost took its class with it.
        if !self.alive() {
            return;
        }
        let (pid, previous) = (self.pid, format!("{:#x}", self.previous.0));
        let handle = HANDLE(self.process.as_raw_handle());
        match priority_class(handle) {
            Ok(HIGH_PRIORITY_CLASS) => {}
            // Someone else set it since: theirs stays.
            Ok(class) => {
                tracing::info!(
                    pid,
                    class = format!("{:#x}", class.0),
                    "gamepad_driver_host_priority: the gamepad driver's WUDFHost priority was changed meanwhile; left as it is"
                );
                return;
            }
            Err(error) => {
                tracing::warn!(pid, %error, "gamepad_driver_host_priority: the gamepad driver's WUDFHost priority could not be read; not restored");
                return;
            }
        }
        // SAFETY: `handle` is open with PROCESS_SET_INFORMATION.
        match unsafe { SetPriorityClass(handle, self.previous) } {
            Ok(()) => tracing::info!(
                pid,
                class = previous,
                "gamepad_driver_host_priority: the gamepad driver's WUDFHost priority restored"
            ),
            Err(error) => tracing::warn!(
                pid,
                class = previous,
                %error,
                "gamepad_driver_host_priority: the gamepad driver's WUDFHost priority could not be restored"
            ),
        }
    }
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
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    fn settled<R: Host>(holders: &Holders<R>) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while holders.state.lock().unwrap().busy {
            assert!(
                Instant::now() < deadline,
                "the helper thread did not finish"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    static RAISES: AtomicUsize = AtomicUsize::new(0);
    static RESTORES: AtomicUsize = AtomicUsize::new(0);
    static ALIVE: AtomicBool = AtomicBool::new(true);
    struct Counted;
    impl Host for Counted {
        fn raise() -> Result<Self> {
            RAISES.fetch_add(1, Ordering::SeqCst);
            Ok(Self)
        }
        fn alive(&self) -> bool {
            ALIVE.load(Ordering::SeqCst)
        }
    }
    impl Drop for Counted {
        fn drop(&mut self) {
            RESTORES.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn raised_once_for_all_sessions_restored_after_the_last_and_raised_again_after_a_restart() {
        let holders = Holders::<Counted>::new();
        let (first, second) = (holders.acquire(), holders.acquire());
        settled(&holders);
        assert_eq!(RAISES.load(Ordering::SeqCst), 1);
        drop(first);
        settled(&holders);
        assert_eq!(
            RESTORES.load(Ordering::SeqCst),
            0,
            "one session still has a pad"
        );
        drop(second);
        settled(&holders);
        assert_eq!(RESTORES.load(Ordering::SeqCst), 1);
        // The next session raises again; its WUDFHost restarts and the new
        // one is raised at the next check.
        let third = holders.acquire();
        settled(&holders);
        assert_eq!(RAISES.load(Ordering::SeqCst), 2);
        ALIVE.store(false, Ordering::SeqCst);
        third.check();
        settled(&holders);
        assert_eq!(RAISES.load(Ordering::SeqCst), 3);
        ALIVE.store(true, Ordering::SeqCst);
        // Checks come at most every CHECK_EVERY.
        third.check();
        settled(&holders);
        assert_eq!(RAISES.load(Ordering::SeqCst), 3);
        drop(third);
        settled(&holders);
        assert_eq!(RESTORES.load(Ordering::SeqCst), 3);
    }

    static TRIES: AtomicUsize = AtomicUsize::new(0);
    static REFUSE: AtomicBool = AtomicBool::new(true);
    struct Refused;
    impl Host for Refused {
        fn raise() -> Result<Self> {
            TRIES.fetch_add(1, Ordering::SeqCst);
            if REFUSE.load(Ordering::SeqCst) {
                bail!("access denied");
            }
            Ok(Self)
        }
        fn alive(&self) -> bool {
            true
        }
    }

    #[test]
    fn a_failed_raise_is_not_fatal_or_retried_until_the_next_sessions() {
        let holders = Holders::<Refused>::new();
        let first = holders.acquire();
        settled(&holders);
        let second = holders.acquire();
        settled(&holders);
        assert_eq!(TRIES.load(Ordering::SeqCst), 1);
        assert!(holders.state.lock().unwrap().raised.is_none());
        drop((first, second));
        settled(&holders);
        REFUSE.store(false, Ordering::SeqCst);
        let _third = holders.acquire();
        settled(&holders);
        assert_eq!(TRIES.load(Ordering::SeqCst), 2);
        assert!(holders.state.lock().unwrap().raised.is_some());
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
