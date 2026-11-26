//! usbipd-win's command line as Nightjar runs it: `usbipd state` in the background, and
//! `usbipd bind` and `unbind` through Windows' administrator prompt (`ShellExecuteExW`'s
//! `runas`, owned by Nightjar's window), whose exit code tells how they went (usbipd-win's
//! `ExitCode`: 0 success, 1 failure, 2 bad arguments, 3 access denied). Also what a start finds
//! left shared after a crash, and giving it back when the user asks.

use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use super::{check, install_candidates, parse_state, recover, valid_busid, valid_guid, BindError, Journal, StateDevice, Usbipd};
use crate::strings::{tr, trf};

/// `CREATE_NO_WINDOW`: no console flashes up for `usbipd state`.
const NO_WINDOW: u32 = 0x0800_0000;
/// How long an elevated usbipd may take, the prompt included.
const ELEVATED_LIMIT: Duration = Duration::from_secs(600);
/// How long `usbipd state` may take.
const STATE_LIMIT: Duration = Duration::from_secs(15);

/// Nightjar's window (an HWND), which owns the administrator prompts; 0 before it opens.
static PROMPT_WINDOW: AtomicIsize = AtomicIsize::new(0);

/// Makes `hwnd`, Nightjar's window, the owner of usbipd-win's administrator prompts: Windows
/// then shows a prompt in front of it, a fullscreen stream too, instead of only flashing it in
/// the taskbar as it does for a prompt nothing owns.
pub fn set_prompt_window(hwnd: isize) {
    PROMPT_WINDOW.store(hwnd, Ordering::Relaxed);
}

/// How an elevated run ended.
#[derive(Debug)]
enum Elevated {
    Exited(u32),
    /// The prompt was cancelled: nothing ran.
    Cancelled,
    /// It did not start.
    Failed(String),
    /// It may still be running.
    Unfinished,
}

/// What usbipd's run means for the user; `args` names it in the log-like detail.
fn outcome(args: &str, ended: Elevated) -> Result<(), BindError> {
    let failed = |detail: &dyn std::fmt::Display| trf("usbipd-win could not share the device: {}", &[&detail.to_string()]);
    match ended {
        Elevated::Exited(0) => Ok(()),
        Elevated::Cancelled => Err(BindError::Failed(tr("Sharing the device with usbipd-win needs administrator rights, and Windows’ prompt was cancelled.").to_owned())),
        Elevated::Exited(3) => Err(BindError::Failed(tr("usbipd-win ran without administrator rights, so Windows refused the change. Use an account with administrator rights, then try again.").to_owned())),
        Elevated::Exited(code) => Err(BindError::Failed(failed(&format!("usbipd {args} exited with {code}")))),
        Elevated::Failed(why) => Err(BindError::Failed(failed(&why))),
        Elevated::Unfinished => Err(BindError::Unfinished(failed(&"it did not finish in time"))),
    }
}

/// The installed usbipd.exe.
#[derive(Clone, Debug)]
pub struct Cli {
    exe: PathBuf,
}

impl Cli {
    /// usbipd.exe where its installer puts it ([`install_candidates`]); None when usbipd-win
    /// isn't installed there.
    pub fn find() -> Option<Self> {
        install_candidates(|name| std::env::var_os(name)).into_iter().find(|exe| exe.is_file()).map(|exe| Self { exe })
    }

    /// Runs usbipd with `args` elevated and waits for it.
    fn elevated(&self, args: &str) -> Elevated {
        use windows::core::{w, HSTRING, PCWSTR};
        use windows::Win32::Foundation::{CloseHandle, ERROR_CANCELLED, HWND, WAIT_OBJECT_0};
        use windows::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};
        use windows::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
        use windows::Win32::UI::WindowsAndMessaging::{IsWindow, SW_HIDE};

        let window = HWND(PROMPT_WINDOW.load(Ordering::Relaxed) as *mut std::ffi::c_void);
        // SAFETY: IsWindow only asks whether the handle names a window; a stale one is refused.
        let owner = if !window.is_invalid() && unsafe { IsWindow(Some(window)) }.as_bool() { window } else { HWND::default() };
        let file = HSTRING::from(self.exe.as_os_str());
        let parameters = HSTRING::from(args);
        let mut info = SHELLEXECUTEINFOW {
            cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC,
            hwnd: owner,
            lpVerb: w!("runas"),
            lpFile: PCWSTR(file.as_ptr()),
            lpParameters: PCWSTR(parameters.as_ptr()),
            nShow: SW_HIDE.0,
            ..Default::default()
        };
        // SAFETY: `info` and the strings it points to outlive the call; the process handle it
        // returns is waited on and closed here.
        unsafe {
            if let Err(e) = ShellExecuteExW(&mut info) {
                if e.code() == ERROR_CANCELLED.to_hresult() { return Elevated::Cancelled; }
                return Elevated::Failed(e.message());
            }
            if info.hProcess.is_invalid() { return Elevated::Failed("no process".into()); }
            let waited = WaitForSingleObject(info.hProcess, ELEVATED_LIMIT.as_millis() as u32);
            let mut code = 1u32;
            let read = GetExitCodeProcess(info.hProcess, &mut code);
            let _ = CloseHandle(info.hProcess);
            if waited != WAIT_OBJECT_0 || read.is_err() { return Elevated::Unfinished; }
            Elevated::Exited(code)
        }
    }
}

impl Usbipd for Cli {
    /// `usbipd state`, given [`STATE_LIMIT`] (a hung one is killed).
    fn state(&self) -> Result<Vec<StateDevice>, String> {
        use std::io::Read;
        let mut child = Command::new(&self.exe).arg("state").creation_flags(NO_WINDOW)
            .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped())
            .spawn().map_err(|e| super::state_error(&e))?;
        let (mut stdout, mut stderr) = (child.stdout.take().expect("piped"), child.stderr.take().expect("piped"));
        // Read on threads of their own, so a long list never fills a pipe the child waits on.
        let out = std::thread::spawn(move || { let mut text = Vec::new(); let _ = stdout.read_to_end(&mut text); text });
        let err = std::thread::spawn(move || { let mut text = Vec::new(); let _ = stderr.read_to_end(&mut text); text });
        let until = Instant::now() + STATE_LIMIT;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if Instant::now() < until => std::thread::sleep(Duration::from_millis(20)),
                Ok(None) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(super::state_error(&"usbipd state did not answer in time"));
                }
                Err(e) => return Err(super::state_error(&e)),
            }
        };
        let (out, err) = (out.join().unwrap_or_default(), err.join().unwrap_or_default());
        if !status.success() {
            let stderr = String::from_utf8_lossy(&err);
            let detail = stderr.lines().map(str::trim).rfind(|l| !l.is_empty()).map_or_else(|| format!("exit {:?}", status.code()), str::to_owned);
            return Err(super::state_error(&detail));
        }
        parse_state(&String::from_utf8_lossy(&out))
    }

    fn bind(&self, busid: &str) -> Result<(), BindError> {
        if !valid_busid(busid) { return Err(BindError::Failed(super::not_listed())); }
        let args = format!("bind --busid {busid}");
        outcome(&args, self.elevated(&args))
    }

    fn unbind(&self, guid: &str) -> Result<(), String> {
        if !valid_guid(guid) { return Err(trf("usbipd-win could not share the device: {}", &[&"not a share"])); }
        let args = format!("unbind --guid {}", guid.trim_start_matches('{').trim_end_matches('}'));
        outcome(&args, self.elevated(&args)).map_err(|e| e.message().to_owned())
    }
}

/// The journal and usbipd-win, when there is a journal to act on.
fn journal_and_cli() -> Result<Option<(Journal, Cli)>, String> {
    let journal = Journal::real()?;
    if !journal.path().exists() { return Ok(None); }
    let cli = Cli::find().ok_or_else(|| tr("usbipd-win isn’t installed. Install it, then press Refresh.").to_owned())?;
    Ok(Some((journal, cli)))
}

/// The shares the start's check found to ask about, until the launcher takes the number.
static START_NOTICE: AtomicUsize = AtomicUsize::new(0);

/// At start, on a thread of its own and without administrator rights: forgets journal entries
/// that need nothing more, and leaves the number of shares Nightjar still has to give back, and
/// hasn't asked about, for the launcher's notice ([`take_start_notice`]). Those are marked
/// asked, so a declined notice doesn't come back at every start: the USB devices section
/// offers the same ([`leftovers`], [`give_back_in_background`]). Nothing runs without a journal.
pub fn check_in_background() {
    let spawned = std::thread::Builder::new().name("usbipd-check".into()).spawn(|| {
        match journal_and_cli().and_then(|found| found.map(|(journal, cli)| check(&cli, &journal, true)).transpose()) {
            Ok(Some(found)) if found.unasked > 0 => START_NOTICE.store(found.unasked, Ordering::Release),
            Ok(_) => {}
            Err(e) => eprintln!("nightjar: USB: {e}"),
        }
    });
    if let Err(e) = spawned { eprintln!("nightjar: USB: no thread to check usbipd-win shares: {e}"); }
}

/// The number of shares the launcher should offer to give back, once.
pub fn take_start_notice() -> Option<usize> {
    Some(START_NOTICE.swap(0, Ordering::AcqRel)).filter(|&count| count > 0)
}

/// The shares Nightjar still has to give back, for the USB devices section: usbipd-win's list
/// only, no prompt; 0 without a journal. Blocks while usbipd-win answers.
pub fn leftovers() -> usize {
    match journal_and_cli().and_then(|found| found.map(|(journal, cli)| check(&cli, &journal, false)).transpose()) {
        Ok(found) => found.map_or(0, |found| found.all),
        Err(e) => {
            eprintln!("nightjar: USB: {e}");
            0
        }
    }
}

static GIVING_BACK: AtomicBool = AtomicBool::new(false);
static GIVEN_BACK: Mutex<Option<Result<usize, String>>> = Mutex::new(None);

/// Gives back, on a thread of its own, every share Nightjar left behind ([`recover`]), which
/// the user asked for: Windows asks for administrator rights. False when that runs already. The
/// outcome waits for [`take_given_back`].
pub fn give_back_in_background() -> bool {
    if GIVING_BACK.swap(true, Ordering::AcqRel) { return false; }
    let finish = |result: Result<usize, String>| {
        *GIVEN_BACK.lock().unwrap_or_else(PoisonError::into_inner) = Some(result);
        GIVING_BACK.store(false, Ordering::Release);
    };
    let spawned = std::thread::Builder::new().name("usbipd-give-back".into()).spawn(move || {
        finish(journal_and_cli().and_then(|found| found.map_or(Ok(0), |(journal, cli)| recover(&cli, &journal))));
    });
    if let Err(e) = spawned { finish(Err(trf("usbipd-win could not share the device: {}", &[&e]))); }
    true
}

/// Whether [`give_back_in_background`] runs.
pub fn giving_back() -> bool {
    GIVING_BACK.load(Ordering::Acquire)
}

/// How the last [`give_back_in_background`] went, once: the number given back, or why not.
pub fn take_given_back() -> Option<Result<usize, String>> {
    GIVEN_BACK.lock().unwrap_or_else(PoisonError::into_inner).take()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn access_denied_is_told_apart_from_a_cancelled_prompt() {
        let message = |ended| outcome("bind --busid 1-3", ended).unwrap_err().message().to_owned();
        let (denied, cancelled) = (message(Elevated::Exited(3)), message(Elevated::Cancelled));
        assert_ne!(denied, cancelled);
        assert!(denied.contains("administrator rights") && !denied.contains("cancelled"), "{denied}");
        assert!(cancelled.contains("cancelled"), "{cancelled}");
        assert!(message(Elevated::Exited(1)).contains("exited with 1"));
        assert!(outcome("bind --busid 1-3", Elevated::Exited(0)).is_ok());
        assert!(matches!(outcome("unbind --guid x", Elevated::Unfinished), Err(BindError::Unfinished(_))));
    }

    #[test]
    fn the_start_notice_is_taken_once() {
        START_NOTICE.store(2, Ordering::Release);
        assert_eq!(take_start_notice(), Some(2));
        assert_eq!(take_start_notice(), None);
    }
}
