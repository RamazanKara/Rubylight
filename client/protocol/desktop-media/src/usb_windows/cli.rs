//! usbipd-win's command line as Nightjar runs it: `usbipd state` in the background, and
//! `usbipd bind` and `unbind` through Windows' administrator prompt (`ShellExecuteExW`'s
//! `runas`), whose exit code tells how they went (usbipd-win's `ExitCode`: 0 success,
//! 1 failure, 2 bad arguments, 3 access denied).

use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use super::{parse_state, recover, valid_busid, valid_guid, Journal, StateDevice, Usbipd};
use crate::strings::{tr, trf};

/// `CREATE_NO_WINDOW`: no console flashes up for `usbipd state`.
const NO_WINDOW: u32 = 0x0800_0000;
/// How long an elevated usbipd may take, the prompt included.
const ELEVATED_LIMIT: Duration = Duration::from_secs(120);

/// The installed usbipd.exe.
#[derive(Clone, Debug)]
pub struct Cli {
    exe: PathBuf,
}

impl Cli {
    /// usbipd.exe on PATH or where its installer puts it; None when usbipd-win isn't installed.
    pub fn find() -> Option<Self> {
        let on_path = std::env::var_os("PATH").and_then(|path| std::env::split_paths(&path).map(|dir| dir.join("usbipd.exe")).find(|exe| exe.is_file()));
        let installed = || {
            ["ProgramW6432", "ProgramFiles"].iter().filter_map(std::env::var_os)
                .map(|dir| PathBuf::from(dir).join("usbipd-win").join("usbipd.exe"))
                .find(|exe| exe.is_file())
        };
        on_path.or_else(installed).map(|exe| Self { exe })
    }

    /// Runs usbipd with `args` elevated and waits for it; its exit code.
    fn elevated(&self, args: &str) -> Result<u32, String> {
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
                if e.code() == ERROR_CANCELLED.to_hresult() {
                    return Err(tr("Sharing the device with usbipd-win needs administrator rights, and Windows’ prompt was cancelled.").to_owned());
                }
                return Err(trf("usbipd-win could not share the device: {}", &[&e.message()]));
            }
            if info.hProcess.is_invalid() {
                return Err(trf("usbipd-win could not share the device: {}", &[&"no process"]));
            }
            let waited = WaitForSingleObject(info.hProcess, ELEVATED_LIMIT.as_millis() as u32);
            let mut code = 1u32;
            let read = GetExitCodeProcess(info.hProcess, &mut code);
            let _ = CloseHandle(info.hProcess);
            if waited != WAIT_OBJECT_0 {
                return Err(trf("usbipd-win could not share the device: {}", &[&"it did not finish in time"]));
            }
            read.map_err(|e| trf("usbipd-win could not share the device: {}", &[&e.message()]))?;
            Ok(code)
        }
    }

    fn checked(&self, args: &str) -> Result<(), String> {
        match self.elevated(args)? {
            0 => Ok(()),
            3 => Err(tr("Sharing the device with usbipd-win needs administrator rights, and Windows’ prompt was cancelled.").to_owned()),
            code => Err(trf("usbipd-win could not share the device: {}", &[&format!("usbipd {args} exited with {code}")])),
        }
    }
}

impl Usbipd for Cli {
    fn state(&self) -> Result<Vec<StateDevice>, String> {
        let output = Command::new(&self.exe).arg("state").creation_flags(NO_WINDOW).output().map_err(|e| super::state_error(&e))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let detail = stderr.lines().map(str::trim).rfind(|l| !l.is_empty()).map_or_else(|| format!("exit {:?}", output.status.code()), str::to_owned);
            return Err(super::state_error(&detail));
        }
        parse_state(&String::from_utf8_lossy(&output.stdout))
    }

    fn bind(&self, busid: &str) -> Result<(), String> {
        if !valid_busid(busid) { return Err(super::not_listed()); }
        self.checked(&format!("bind --busid {busid}"))
    }

    fn unbind(&self, guid: &str) -> Result<(), String> {
        if !valid_guid(guid) { return Err(trf("usbipd-win could not share the device: {}", &[&"not a share"])); }
        self.checked(&format!("unbind --guid {}", guid.trim_start_matches('{').trim_end_matches('}')))
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
