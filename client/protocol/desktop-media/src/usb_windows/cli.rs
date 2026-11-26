//! usbipd-win's command line as Nightjar runs it: `usbipd state` in the background, and
//! `usbipd bind` and `unbind` through Windows' administrator prompt (`ShellExecuteExW`'s
//! `runas`), whose exit code tells how they went (usbipd-win's `ExitCode`: 0 success,
//! 1 failure, 2 bad arguments, 3 access denied).

use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::{install_candidates, parse_state, recover, valid_busid, valid_guid, BindError, Journal, StateDevice, Usbipd};
use crate::strings::{tr, trf};

/// `CREATE_NO_WINDOW`: no console flashes up for `usbipd state`.
const NO_WINDOW: u32 = 0x0800_0000;
/// How long an elevated usbipd may take, the prompt included.
const ELEVATED_LIMIT: Duration = Duration::from_secs(600);
/// How long `usbipd state` may take.
const STATE_LIMIT: Duration = Duration::from_secs(15);

/// How an elevated run ended.
enum Elevated {
    Exited(u32),
    /// The prompt was cancelled: nothing ran.
    Cancelled,
    /// It did not start.
    Failed(String),
    /// It may still be running.
    Unfinished,
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
        use windows::Win32::Foundation::{CloseHandle, ERROR_CANCELLED, WAIT_OBJECT_0};
        use windows::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};
        use windows::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
        use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;

        let file = HSTRING::from(self.exe.as_os_str());
        let parameters = HSTRING::from(args);
        let mut info = SHELLEXECUTEINFOW {
            cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC,
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

    fn checked(&self, args: &str) -> Result<(), BindError> {
        let cancelled = || tr("Sharing the device with usbipd-win needs administrator rights, and Windows’ prompt was cancelled.").to_owned();
        let failed = |detail: &dyn std::fmt::Display| trf("usbipd-win could not share the device: {}", &[&detail.to_string()]);
        match self.elevated(args) {
            Elevated::Exited(0) => Ok(()),
            Elevated::Exited(3) | Elevated::Cancelled => Err(BindError::Failed(cancelled())),
            Elevated::Exited(code) => Err(BindError::Failed(failed(&format!("usbipd {args} exited with {code}")))),
            Elevated::Failed(why) => Err(BindError::Failed(failed(&why))),
            Elevated::Unfinished => Err(BindError::Unfinished(failed(&"it did not finish in time"))),
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
        self.checked(&format!("bind --busid {busid}"))
    }

    fn unbind(&self, guid: &str) -> Result<(), String> {
        if !valid_guid(guid) { return Err(trf("usbipd-win could not share the device: {}", &[&"not a share"])); }
        self.checked(&format!("unbind --guid {}", guid.trim_start_matches('{').trim_end_matches('}'))).map_err(|e| e.message().to_owned())
    }
}

/// At start: unshares, on a thread of its own, what Nightjar shared through usbipd-win and could
/// not unshare before it stopped (a crash, a cancelled prompt). Nothing runs without a journal.
pub fn recover_in_background() {
    let journal = match Journal::real() {
        Ok(journal) => journal,
        Err(e) => return eprintln!("nightjar: USB: {e}"),
    };
    if !journal.path().exists() { return; }
    let spawned = std::thread::Builder::new().name("usbipd-recover".into()).spawn(move || {
        let Some(cli) = Cli::find() else {
            return eprintln!("nightjar: USB: Nightjar's usbipd-win journal lists devices, but usbipd-win isn't installed any more");
        };
        match recover(&cli, &journal) {
            Ok(0) => {}
            Ok(unshared) => eprintln!("nightjar: USB: gave {unshared} device(s) shared through usbipd-win back to Windows"),
            Err(e) => eprintln!("nightjar: USB: {e}"),
        }
    });
    if let Err(e) = spawned { eprintln!("nightjar: USB: no thread to unshare usbipd-win devices: {e}"); }
}
