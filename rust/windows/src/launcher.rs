//! First-run and repeated-launch experience for the portable Windows package.

use crate::text::to_wide;
use anyhow::{Context, Result, bail};
use butterpollo_core::{config::Config, migration};
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    os::windows::process::CommandExt,
    path::PathBuf,
    process::Command,
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Foundation::CloseHandle,
        System::{Com::*, SystemInformation::GetSystemDirectoryW, Threading::*},
        UI::{Shell::*, WindowsAndMessaging::*},
    },
    core::{PCWSTR, w},
};
fn folder() -> Result<Option<PathBuf>> {
    // SAFETY: COM is initialised on this thread before any COM call and `_com` uninitialises it
    // once, after `dialog` and `item` drop; the display name is CoTaskMemAlloc'd and freed once.
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
        struct Com;
        impl Drop for Com {
            fn drop(&mut self) {
                // SAFETY: this guard exists only after CoInitializeEx succeeded, so the
                // uninitialise is balanced.
                unsafe { CoUninitialize() }
            }
        }
        let _com = Com;
        let dialog: IFileOpenDialog =
            CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)?;
        dialog.SetOptions(
            dialog.GetOptions()? | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM | FOS_PATHMUSTEXIST,
        )?;
        dialog.SetTitle(w!("Select the Vibepollo or Apollo config folder"))?;
        if let Err(error) = dialog.Show(None) {
            if error.code() == windows::core::HRESULT(0x800704c7u32 as i32) {
                return Ok(None);
            }
            return Err(error.into());
        }
        let item = dialog.GetResult()?;
        let value = item.GetDisplayName(SIGDN_FILESYSPATH)?;
        let path = value.to_string();
        CoTaskMemFree(Some(value.0.cast()));
        Ok(Some(PathBuf::from(path?)))
    }
}
pub fn show_error(error: &str) {
    let text = to_wide(error);
    // SAFETY: `text` is NUL-terminated and outlives the modal call.
    unsafe {
        MessageBoxW(
            None,
            PCWSTR(text.as_ptr()),
            w!("Rubylight"),
            MB_OK | MB_ICONERROR,
        );
    }
}
fn status(port: u16) -> Option<String> {
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    let mut socket = TcpStream::connect_timeout(&address, Duration::from_millis(200)).ok()?;
    socket
        .set_read_timeout(Some(Duration::from_millis(400)))
        .ok()?;
    socket
        .set_write_timeout(Some(Duration::from_millis(400)))
        .ok()?;
    socket
        .write_all(b"GET /serverinfo HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
        .ok()?;
    let mut text = String::new();
    socket.take(65536).read_to_string(&mut text).ok()?;
    Some(text)
}
/// What the launcher does about the installed service's state.
#[derive(Debug, PartialEq, Eq)]
enum ServiceNeed {
    /// It is running.
    Ready,
    /// It is starting; wait for it.
    Wait,
    /// It is stopped: offer to start it, with an elevation prompt.
    Offer,
    /// It is stopped and Windows would refuse to start it (disabled).
    Refuse,
}
fn service_need(
    state: windows_service::service::ServiceState,
    start_type: windows_service::service::ServiceStartType,
) -> ServiceNeed {
    use windows_service::service::{ServiceStartType, ServiceState};
    match state {
        ServiceState::Running => ServiceNeed::Ready,
        ServiceState::StartPending | ServiceState::ContinuePending => ServiceNeed::Wait,
        _ if start_type == ServiceStartType::Disabled => ServiceNeed::Refuse,
        ServiceState::Stopped => ServiceNeed::Offer,
        _ => ServiceNeed::Refuse,
    }
}
/// Ask whether to start the stopped service. False on "No".
fn ask_to_start() -> bool {
    // SAFETY: both strings are static NUL-terminated literals and the call is modal.
    let choice = unsafe {
        MessageBoxW(
            None,
            w!(
                "The Rubylight service is stopped.\n\nStart it now? Windows will ask for permission."
            ),
            w!("Rubylight"),
            MB_YESNO | MB_ICONQUESTION,
        )
    };
    choice == IDYES
}
/// Start the service through an elevated `sc.exe`. False if the user
/// cancelled the prompt or the start failed.
fn start_elevated() -> bool {
    /// `sc start` exits with this when the service is already running.
    const ALREADY_RUNNING: u32 = 1056;
    let mut folder = [0u16; 260];
    // SAFETY: the buffer is writable for its whole length.
    let length = unsafe { GetSystemDirectoryW(Some(&mut folder)) } as usize;
    if length == 0 || length >= folder.len() {
        return false;
    }
    let program = to_wide(&format!(
        "{}\\sc.exe",
        String::from_utf16_lossy(&folder[..length])
    ));
    let arguments = to_wide(&format!("start {}", crate::service::NAME));
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: w!("runas"),
        lpFile: PCWSTR(program.as_ptr()),
        lpParameters: PCWSTR(arguments.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    // SAFETY: `info` and the strings it points at outlive the calls; the process handle is
    // owned by this function and closed once.
    unsafe {
        if ShellExecuteExW(&mut info).is_err() || info.hProcess.is_invalid() {
            return false;
        }
        WaitForSingleObject(info.hProcess, 60_000);
        let mut code = 1;
        let read = GetExitCodeProcess(info.hProcess, &mut code);
        let _ = CloseHandle(info.hProcess);
        read.is_ok() && (code == 0 || code == ALREADY_RUNNING)
    }
}
fn service_profile() -> Result<Option<PathBuf>> {
    use windows_service::{
        service::{ServiceAccess, ServiceState},
        service_manager::{ServiceManager, ServiceManagerAccess},
    };
    let manager = match ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
    {
        Ok(manager) => manager,
        Err(_) => return Ok(None),
    };
    let service = match manager.open_service(
        crate::service::NAME,
        ServiceAccess::QUERY_CONFIG | ServiceAccess::QUERY_STATUS,
    ) {
        Ok(service) => service,
        Err(_) => return Ok(None),
    };
    let config = service.query_config()?;
    let installed = config.executable_path;
    let expected = std::env::current_exe()?
        .parent()
        .context("package directory unavailable")?
        .join("butterpollo-service.exe");
    if installed
        .to_string_lossy()
        .trim_matches('"')
        .eq_ignore_ascii_case(&expected.to_string_lossy())
    {
        let stopped = || {
            anyhow::anyhow!(
                "The Rubylight service is installed but stopped. Start Rubylight in Windows Services, then open Start Rubylight.exe again."
            )
        };
        match service_need(service.query_status()?.current_state, config.start_type) {
            ServiceNeed::Ready => {}
            ServiceNeed::Refuse => return Err(stopped()),
            need => {
                if need == ServiceNeed::Offer && !(ask_to_start() && start_elevated()) {
                    return Err(stopped());
                }
                let deadline = Instant::now() + Duration::from_secs(30);
                loop {
                    match service.query_status()?.current_state {
                        ServiceState::Running => break,
                        ServiceState::Stopped => bail!(
                            "The Rubylight service stopped again. Check logs\\service.log in {}.",
                            butterpollo_core::paths::installed_profile().display()
                        ),
                        _ if Instant::now() >= deadline => return Err(stopped()),
                        _ => std::thread::sleep(Duration::from_millis(250)),
                    }
                }
            }
        }
        return Ok(Some(butterpollo_core::paths::installed_profile()));
    }
    Ok(None)
}
/// What the launcher does once it knows what answers on the HTTP port.
#[derive(Debug, PartialEq, Eq)]
enum Launch {
    /// This profile's host answers: open its console.
    Open,
    /// Another host answers.
    Conflict,
    /// The installed service owns the profile and its host is not up yet.
    WaitForService,
    /// Nothing answers and nothing else will start it: run the host here.
    Spawn,
}
fn launch_plan(service_owned: bool, response: Option<&str>, marker: &str) -> Launch {
    match response {
        Some(response) if response.contains(marker) => Launch::Open,
        Some(_) => Launch::Conflict,
        None if service_owned => Launch::WaitForService,
        None => Launch::Spawn,
    }
}
pub fn run() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let mut directory = None;
    let mut import = None;
    let mut interactive = true;
    while let Some(arg) = args.next() {
        if arg == "--config-dir" {
            directory = Some(PathBuf::from(
                args.next().context("config directory required")?,
            ));
            interactive = false;
        } else if arg == "--import-config" {
            import = Some(PathBuf::from(
                args.next().context("existing profile folder required")?,
            ));
            interactive = false;
        } else {
            bail!("unknown start option: {}", arg.to_string_lossy());
        }
    }
    // The installed service runs the host (as SYSTEM) for its profile: the
    // launcher only waits for it and never starts a second host there.
    let mut service_owned = false;
    if interactive && let Some(service) = service_profile()? {
        directory = Some(service);
        interactive = false;
        service_owned = true;
    }
    let directory = match directory {
        Some(directory) => directory,
        None => butterpollo_core::paths::portable_profile()?,
    };
    let has_profile = directory.is_dir() && std::fs::read_dir(&directory)?.next().is_some();
    if !has_profile && import.is_none() && interactive {
        // SAFETY: both strings are static NUL-terminated literals and the call is modal.
        let choice = unsafe {
            MessageBoxW(
                None,
                w!(
                    "Bring your existing Vibepollo or Apollo settings, paired devices and library?\n\nYes: choose your old config folder and copy it into a new Rubylight profile.\nNo: start with a new profile.\n\nThe original profile is kept."
                ),
                w!("Welcome to Rubylight"),
                MB_YESNOCANCEL | MB_ICONQUESTION,
            )
        };
        if choice == IDCANCEL {
            return Ok(());
        }
        if choice == IDYES {
            let Some(mut source) = folder()? else {
                return Ok(());
            };
            if !source.join("sunshine.conf").is_file()
                && source.join("config/sunshine.conf").is_file()
            {
                source = source.join("config");
            }
            import = Some(source);
        }
    }
    if let Some(source) = import {
        migration::import(&source, &directory)?;
    }
    std::fs::create_dir_all(&directory)?;
    let config = Config::load(&directory.join("sunshine.conf"))?;
    let ports = config.ports()?;
    let marker = format!(
        "<RustHostProfile>{}</RustHostProfile>",
        migration::profile_id(&directory)
    );
    match launch_plan(service_owned, status(ports.http).as_deref(), &marker) {
        Launch::Open => return crate::tray::open_web(ports.web),
        Launch::Conflict => bail!(
            "Another streaming host is using port {}. Close Vibepollo or Apollo before starting Rubylight, or choose a different base port in sunshine.conf. Your settings have been preserved.",
            ports.http
        ),
        Launch::WaitForService => {
            // The service reports Running before its host answers: an update
            // recovery or driver repair can come first.
            let deadline = Instant::now() + Duration::from_secs(90);
            while Instant::now() < deadline {
                if status(ports.http).is_some_and(|response| response.contains(&marker)) {
                    return crate::tray::open_web(ports.web);
                }
                std::thread::sleep(Duration::from_millis(250));
            }
            bail!(
                "The Rubylight service is still starting. Open https://localhost:{} after startup, or check logs\\service.log in {}.",
                ports.web,
                directory.display()
            );
        }
        Launch::Spawn => {}
    }
    let executable = std::env::current_exe()?
        .parent()
        .context("package directory unavailable")?
        .join("butterpollo.exe");
    let assets = executable.parent().unwrap().join("assets/web");
    if !executable.is_file() || !assets.is_dir() {
        bail!("Extract the whole Rubylight package before opening Start Rubylight.exe.");
    }
    let mut child = Command::new(&executable)
        .arg("--config-dir")
        .arg(&directory)
        .arg("--assets")
        .arg(assets)
        .current_dir(executable.parent().unwrap())
        .creation_flags(0x08000000)
        .spawn()
        .context("starting Rubylight")?;
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(response) = status(ports.http)
            && response.contains(&marker)
        {
            return crate::tray::open_web(ports.web);
        }
        if let Some(code) = child.try_wait()? {
            bail!(
                "Rubylight could not start ({code}). Check {} for the startup error.",
                config
                    .path("log_path", &directory, "logs/butterpollo.log")
                    .display()
            );
        }
        if Instant::now() >= deadline {
            bail!(
                "Rubylight is still starting. Open https://localhost:{} after startup, or check {}.",
                ports.web,
                directory.display()
            );
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_service::service::{ServiceStartType as Type, ServiceState as State};
    #[test]
    fn a_service_profile_never_gets_a_second_host() {
        let marker = "<RustHostProfile>abc</RustHostProfile>";
        let ours = format!("HTTP/1.1 200 OK\r\n\r\n{marker}");
        let other = "HTTP/1.1 200 OK\r\n\r\n<RustHostProfile>other</RustHostProfile>";
        for owned in [true, false] {
            assert_eq!(launch_plan(owned, Some(&ours), marker), Launch::Open);
            assert_eq!(launch_plan(owned, Some(other), marker), Launch::Conflict);
        }
        // The service reports Running before its host answers.
        assert_eq!(launch_plan(true, None, marker), Launch::WaitForService);
        assert_eq!(launch_plan(false, None, marker), Launch::Spawn);
    }
    #[test]
    fn a_stopped_service_is_offered_a_start_unless_it_is_disabled() {
        for start_type in [Type::AutoStart, Type::OnDemand] {
            assert_eq!(service_need(State::Running, start_type), ServiceNeed::Ready);
            assert_eq!(service_need(State::Stopped, start_type), ServiceNeed::Offer);
            assert_eq!(
                service_need(State::StartPending, start_type),
                ServiceNeed::Wait
            );
            assert_eq!(
                service_need(State::StopPending, start_type),
                ServiceNeed::Refuse
            );
        }
        assert_eq!(
            service_need(State::Stopped, Type::Disabled),
            ServiceNeed::Refuse
        );
        assert_eq!(
            service_need(State::Running, Type::Disabled),
            ServiceNeed::Ready
        );
    }
}
