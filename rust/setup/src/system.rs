//! Windows facilities setup needs: elevation, commands, services, registry,
//! processes, firewall rules, shortcuts and folder permissions.
use crate::log::line;
use anyhow::{Context, Result, bail, ensure};
use std::{
    io::Read,
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Foundation::{
            CloseHandle, ERROR_FILE_NOT_FOUND, ERROR_INSUFFICIENT_BUFFER,
            ERROR_SERVICE_MARKED_FOR_DELETE, ERROR_SUCCESS,
        },
        Security::PSID,
        System::{Registry::*, Services::*, Threading::*},
        UI::Shell::*,
    },
    core::{HSTRING, PCWSTR, PWSTR},
};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

/// Run a program without a window, logging its command line and output.
/// Returns the exit code and the output (stdout, then stderr); a program
/// still running at `timeout` is killed.
pub fn run(program: &str, args: &[&str], timeout: Duration) -> Result<(i32, String)> {
    line(format!("> {program} {}", args.join(" ")));
    let mut command = Command::new(program);
    command.args(args);
    finish(command, program, timeout)
}
/// Run `cmd.exe /D /S /C "<line>"` with `line` passed through untouched, so
/// the quoted paths inside it reach cmd as written. `run` escapes embedded
/// quotes as `\"`, which cmd does not understand.
pub fn run_cmd(line_text: &str, timeout: Duration) -> Result<(i32, String)> {
    let program = system32("cmd.exe");
    line(format!("> {program} /D /S /C \"{line_text}\""));
    let mut command = Command::new(&program);
    command.raw_arg(cmd_line(line_text));
    finish(command, &program, timeout)
}
/// The arguments of `cmd.exe /D /S /C`: with /S, cmd removes only the outer
/// quotes and runs the rest as typed.
pub fn cmd_line(line_text: &str) -> String {
    format!("/D /S /C \"{line_text}\"")
}
fn finish(mut command: Command, program: &str, timeout: Duration) -> Result<(i32, String)> {
    let mut child = command
        // A PowerShell 7 parent (a script running setup) leaves its module
        // path behind, and Windows PowerShell 5.1 then fails to load its own
        // modules: the driver scripts could not check signatures.
        .env_remove("PSModulePath")
        // The host's plan for `--apply-wake-settings`: setup's own run plans
        // for this PC, never with a plan from the environment it inherited.
        .env_remove("BUTTERPOLLO_WAKE_FIX")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .with_context(|| format!("starting {program}"))?;
    let readers: Vec<_> = [
        child
            .stdout
            .take()
            .map(|s| Box::new(s) as Box<dyn Read + Send>),
        child
            .stderr
            .take()
            .map(|s| Box::new(s) as Box<dyn Read + Send>),
    ]
    .into_iter()
    .flatten()
    .map(|mut stream| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = stream.read_to_end(&mut bytes);
            String::from_utf8_lossy(&bytes).into_owned()
        })
    })
    .collect();
    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break Some(status);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let mut text = String::new();
    for reader in readers {
        if let Ok(output) = reader.join()
            && !output.trim().is_empty()
        {
            line(output.trim_end());
            text.push_str(output.trim_end());
            text.push('\n');
        }
    }
    let Some(status) = status else {
        bail!("{program} did not finish within {} s", timeout.as_secs());
    };
    let code = status.code().unwrap_or(-1);
    line(format!("  exit code {code}"));
    Ok((code, text))
}
/// A program run as SYSTEM that had not finished at its deadline. Deleting
/// the task does not stop it: it goes on in the background, as the USB/IP
/// step did 40 s after setup gave up on it, so it has not failed (yet).
#[derive(Debug)]
pub struct StillRunning {
    pub program: String,
    pub seconds: u64,
}
impl std::fmt::Display for StillRunning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} was still running after {} s",
            self.program, self.seconds
        )
    }
}
impl std::error::Error for StillRunning {}
/// Run a program as LocalSystem through a one-time scheduled task and return
/// its exit code and output. Vibepollo's driver scripts run as SYSTEM under
/// Windows Installer: as an administrator, the display driver's health
/// check cannot open the driver and needlessly reinstalls it.
pub fn run_as_system(program: &str, args: &[&str], timeout: Duration) -> Result<(i32, String)> {
    let work = program_data().join("Butterpollo").join("setup-tasks");
    run_as_system_in(&work, program, args, timeout)
}
fn run_as_system_in(
    work: &Path,
    program: &str,
    args: &[&str],
    timeout: Duration,
) -> Result<(i32, String)> {
    static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    std::fs::create_dir_all(work)?;
    let id = format!(
        "ButterpolloSetup-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    let log = work.join(format!("{id}.log"));
    let script = work.join(format!("{id}.cmd"));
    let definition = work.join(format!("{id}.xml"));
    let _ = std::fs::remove_file(&log);
    std::fs::write(&script, task_script(program, args, &log))?;
    std::fs::write(&definition, task_definition(&script))?;
    line(format!("as SYSTEM> {program} {args:?}"));
    let schtasks = system32("schtasks.exe");
    let (created, _) = run(
        &schtasks,
        &[
            "/Create",
            "/TN",
            &id,
            "/XML",
            &definition.display().to_string(),
            "/F",
        ],
        Duration::from_secs(60),
    )?;
    if created != 0 {
        bail!("creating the setup task failed ({created})");
    }
    let result = (|| -> Result<(i32, String)> {
        if run(&schtasks, &["/Run", "/TN", &id], Duration::from_secs(60))?.0 != 0 {
            bail!("starting the setup task failed");
        }
        let deadline = Instant::now() + timeout;
        loop {
            let text = std::fs::read(&log)
                .map(|b| String::from_utf8_lossy(&b).into_owned())
                .unwrap_or_default();
            if let Some(index) = text.rfind("BUTTERPOLLO-EXIT=") {
                let code = text[index + 17..].trim().parse().unwrap_or(-1);
                let output = text[..index].to_owned();
                if !output.trim().is_empty() {
                    line(output.trim_end());
                }
                line(format!("  exit code {code}"));
                return Ok((code, output));
            }
            if Instant::now() >= deadline {
                return Err(StillRunning {
                    program: program.to_owned(),
                    seconds: timeout.as_secs(),
                }
                .into());
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    })();
    let _ = run(
        &schtasks,
        &["/Delete", "/TN", &id, "/F"],
        Duration::from_secs(60),
    );
    let _ = std::fs::remove_file(&script);
    let _ = std::fs::remove_file(&definition);
    let _ = std::fs::remove_file(&log);
    result
}
/// The task file for schtasks /XML: UTF-16 with a byte order mark, as Task
/// Scheduler exports tasks. schtasks refuses UTF-8 ("unable to switch the
/// encoding"), which kept rc.22 to rc.25 from installing the drivers.
fn task_definition(script: &Path) -> Vec<u8> {
    let escape = |value: &str| {
        value
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
    };
    let command = escape(&system32("cmd.exe"));
    let arguments = escape(&format!("/D /S /C \"\"{}\"\"", script.display()));
    // With no timer, a task left by interrupted setup cannot install drivers later.
    // Priority 4 is normal. A task otherwise runs at 7, below normal with low
    // I/O priority: on a busy PC the USB/IP step's checks took 11 minutes as
    // SYSTEM, against 3.6 s as an administrator.
    let text = format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <Triggers />
  <Principals><Principal id="System"><UserId>S-1-5-18</UserId><RunLevel>HighestAvailable</RunLevel></Principal></Principals>
  <Settings><DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries><StopIfGoingOnBatteries>false</StopIfGoingOnBatteries><ExecutionTimeLimit>PT0S</ExecutionTimeLimit><Priority>4</Priority></Settings>
  <Actions Context="System"><Exec><Command>{command}</Command><Arguments>{arguments}</Arguments></Exec></Actions>
</Task>"#
    );
    [0xfeff_u16]
        .into_iter()
        .chain(text.encode_utf16())
        .flat_map(u16::to_le_bytes)
        .collect()
}
fn task_script(program: &str, args: &[&str], log: &Path) -> String {
    let quote = |value: &str| format!("\"{}\"", value.replace('%', "%%"));
    let command = std::iter::once(program)
        .chain(args.iter().copied())
        .map(quote)
        .collect::<Vec<_>>()
        .join(" ");
    let log = quote(&log.display().to_string());
    // cmd otherwise reads the UTF-8 paths using the machine's ANSI code page.
    format!(
        "@echo off\r\nchcp 65001 > nul\r\n{command} > {log} 2>&1\r\n>> {log} echo BUTTERPOLLO-EXIT=%ERRORLEVEL%\r\n"
    )
}
pub fn elevated() -> bool {
    // SAFETY: IsUserAnAdmin takes no arguments and only reads the process token.
    unsafe { IsUserAnAdmin().as_bool() }
}
/// Start this program again elevated with the same arguments and return its
/// exit code (1223 when the user declines the UAC prompt).
pub fn relaunch_elevated() -> Result<i32> {
    let exe = std::env::current_exe()?;
    let arguments: Vec<String> = std::env::args()
        .skip(1)
        .map(|a| {
            if a.contains([' ', '\t', '"']) || a.is_empty() {
                format!("\"{}\"", a.replace('"', "\\\""))
            } else {
                a
            }
        })
        .collect();
    let file = HSTRING::from(exe.as_os_str());
    let parameters = HSTRING::from(arguments.join(" "));
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: windows::core::w!("runas"),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(parameters.as_ptr()),
        nShow: 1,
        ..Default::default()
    };
    // SAFETY: `info` and the wide strings it points at outlive the call; the process handle it returns is closed before returning.
    unsafe {
        if let Err(error) = ShellExecuteExW(&mut info) {
            if error.code() == windows::Win32::Foundation::ERROR_CANCELLED.to_hresult() {
                return Ok(1223);
            }
            return Err(error.into());
        }
        let process = info.hProcess;
        WaitForSingleObject(process, INFINITE);
        let mut code = 0u32;
        GetExitCodeProcess(process, &mut code)?;
        let _ = CloseHandle(process);
        Ok(code as i32)
    }
}

// --- Services -------------------------------------------------------------

struct ServiceHandle(SC_HANDLE);
impl Drop for ServiceHandle {
    fn drop(&mut self) {
        // SAFETY: the handle came from OpenSCManagerW or OpenServiceW and is closed only here.
        unsafe {
            let _ = CloseServiceHandle(self.0);
        }
    }
}
fn manager() -> Result<ServiceHandle> {
    // SAFETY: null machine and database names select the local service manager.
    unsafe {
        Ok(ServiceHandle(OpenSCManagerW(
            PCWSTR::null(),
            PCWSTR::null(),
            SC_MANAGER_ALL_ACCESS,
        )?))
    }
}
fn open_service(name: &str) -> Option<ServiceHandle> {
    let manager = manager().ok()?;
    let name = wide(name);
    // SAFETY: `manager` is a live handle and `name` is NUL-terminated UTF-16 that outlives the call.
    unsafe {
        OpenServiceW(manager.0, PCWSTR(name.as_ptr()), SERVICE_ALL_ACCESS)
            .ok()
            .map(ServiceHandle)
    }
}
/// The service's program path (without quotes or arguments), if installed.
pub fn service_program(name: &str) -> Option<PathBuf> {
    let image = registry_string(
        HKEY_LOCAL_MACHINE,
        &format!("SYSTEM\\CurrentControlSet\\Services\\{name}"),
        "ImagePath",
    )?;
    let image = image.trim();
    let program = match image.strip_prefix('"') {
        Some(rest) => rest.split('"').next().unwrap_or(rest),
        None => image.split(" -").next().unwrap_or(image).trim(),
    };
    Some(PathBuf::from(program))
}
fn service_state(service: &ServiceHandle) -> Option<SERVICE_STATUS_CURRENT_STATE> {
    let mut status = SERVICE_STATUS::default();
    // SAFETY: `service` is a live handle and `status` is a valid out-pointer.
    unsafe { QueryServiceStatus(service.0, &mut status) }
        .ok()
        .map(|_| status.dwCurrentState)
}
/// Whether the service is installed and running or starting.
pub fn service_running(name: &str) -> bool {
    open_service(name)
        .and_then(|service| service_state(&service))
        .is_some_and(|state| state != SERVICE_STOPPED && state != SERVICE_STOP_PENDING)
}
/// Stop the service if it is running and wait for it (35 s, then fail).
pub fn stop_service(name: &str) -> Result<()> {
    let Some(service) = open_service(name) else {
        return Ok(());
    };
    if service_state(&service) == Some(SERVICE_STOPPED) {
        return Ok(());
    }
    line(format!("stopping service {name}"));
    let mut status = SERVICE_STATUS::default();
    // SAFETY: `service` is a live handle and `status` is a valid out-pointer.
    unsafe {
        let _ = ControlService(service.0, SERVICE_CONTROL_STOP, &mut status);
    }
    let deadline = Instant::now() + Duration::from_secs(35);
    while Instant::now() < deadline {
        if service_state(&service) == Some(SERVICE_STOPPED) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    bail!("the {name} service did not stop")
}
/// Wait until the service has stopped; false if it is still running at
/// `timeout`.
pub fn wait_stopped(name: &str, timeout: Duration) -> bool {
    let Some(service) = open_service(name) else {
        return true;
    };
    let deadline = Instant::now() + timeout;
    loop {
        if service_state(&service) == Some(SERVICE_STOPPED) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}
pub fn start_service(name: &str) -> Result<()> {
    let service = open_service(name).with_context(|| format!("the {name} service is missing"))?;
    line(format!("starting service {name}"));
    // SAFETY: `service` is a live handle; no start arguments are passed.
    unsafe { StartServiceW(service.0, None) }.with_context(|| format!("starting {name}"))
}
pub fn delete_service(name: &str) -> Result<()> {
    let Some(service) = open_service(name) else {
        return Ok(());
    };
    line(format!("deleting service {name}"));
    // SAFETY: `service` is a live handle.
    unsafe { DeleteService(service.0) }.with_context(|| format!("deleting {name}"))
}
/// What setup does about a service's startup type and about starting it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StartupPlan {
    /// The startup type to give the service; `SERVICE_NO_CHANGE` keeps the
    /// current one (and with it a delayed start).
    pub start_type: SERVICE_START_TYPE,
    /// Whether setup starts the service once it is installed.
    pub start: bool,
}
/// A new service starts automatically. An existing one keeps what the user
/// set; a disabled one is not started, because Windows refuses to.
pub fn startup_plan(existing: Option<SERVICE_START_TYPE>) -> StartupPlan {
    match existing {
        None => StartupPlan {
            start_type: SERVICE_AUTO_START,
            start: true,
        },
        Some(start_type) => StartupPlan {
            start_type: SERVICE_START_TYPE(SERVICE_NO_CHANGE),
            start: start_type != SERVICE_DISABLED,
        },
    }
}
fn service_start_type(service: &ServiceHandle) -> Result<SERVICE_START_TYPE> {
    let mut needed = 0u32;
    // SAFETY: `service` is a live handle; a null buffer asks only for the size.
    if let Err(error) = unsafe { QueryServiceConfigW(service.0, None, 0, &mut needed) } {
        // Any other error is returned as it is: install_service matches on it.
        if error.code() != ERROR_INSUFFICIENT_BUFFER.to_hresult() {
            return Err(error.into());
        }
    }
    ensure!(needed > 0, "the service configuration could not be read");
    // u64 elements keep the buffer aligned for QUERY_SERVICE_CONFIGW.
    let mut buffer = vec![0u64; (needed as usize).div_ceil(8)];
    let config = buffer.as_mut_ptr().cast::<QUERY_SERVICE_CONFIGW>();
    // SAFETY: `config` points at `needed` writable, aligned bytes; Windows fills them.
    unsafe {
        QueryServiceConfigW(service.0, Some(config), needed, &mut needed)
            .context("reading the service configuration")?;
        Ok((*config).dwStartType)
    }
}
/// Start the service unless it is disabled, which setup leaves as the user
/// set it. Returns whether it was started.
pub fn start_service_unless_disabled(name: &str) -> Result<bool> {
    let service = open_service(name).with_context(|| format!("the {name} service is missing"))?;
    if !startup_plan(Some(service_start_type(&service)?)).start {
        line(format!(
            "service {name} is disabled and stays disabled, so it was not started; set its startup type to Manual or Automatic in Services, then start it"
        ));
        return Ok(false);
    }
    drop(service);
    start_service(name)?;
    Ok(true)
}
/// Create the service, or point an existing one with this name at `program`.
/// Runs as LocalSystem and restarts after failures. A new service starts
/// automatically; an existing one keeps its startup type.
pub fn install_service(name: &str, display: &str, description: &str, program: &Path) -> Result<()> {
    // Apollo's and Sunshine's uninstallers delete this service. While
    // something still holds it open, Windows only marks it for deletion and
    // refuses to create or change one with its name until it is gone.
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        match install_service_once(name, display, description, program) {
            Err(error)
                if Instant::now() < deadline
                    && error
                        .downcast_ref::<windows::core::Error>()
                        .is_some_and(|e| {
                            e.code() == ERROR_SERVICE_MARKED_FOR_DELETE.to_hresult()
                        }) =>
            {
                line(format!("service {name} is still being deleted; waiting"));
                std::thread::sleep(Duration::from_secs(1));
            }
            result => return result,
        }
    }
}
/// Renames an installed service as Windows Services shows it, changing
/// nothing else about it.
pub fn set_service_display_name(name: &str, display: &str) -> Result<()> {
    let manager = manager()?;
    let wide_name = wide(name);
    let wide_display = wide(display);
    // SAFETY: `manager` is a live handle and both strings are NUL-terminated UTF-16 that outlive the calls.
    unsafe {
        let service = ServiceHandle(OpenServiceW(
            manager.0,
            PCWSTR(wide_name.as_ptr()),
            SERVICE_CHANGE_CONFIG,
        )?);
        ChangeServiceConfigW(
            service.0,
            ENUM_SERVICE_TYPE(SERVICE_NO_CHANGE),
            SERVICE_START_TYPE(SERVICE_NO_CHANGE),
            SERVICE_ERROR(SERVICE_NO_CHANGE),
            PCWSTR::null(),
            PCWSTR::null(),
            None,
            PCWSTR::null(),
            PCWSTR::null(),
            PCWSTR::null(),
            PCWSTR(wide_display.as_ptr()),
        )?;
    }
    Ok(())
}
fn install_service_once(
    name: &str,
    display: &str,
    description: &str,
    program: &Path,
) -> Result<()> {
    let manager = manager()?;
    let binary = wide(&format!("\"{}\"", program.display()));
    let wide_name = wide(name);
    let wide_display = wide(display);
    // SAFETY: `manager` is a live handle and every string argument is NUL-terminated UTF-16 that outlives the calls.
    let service = unsafe {
        match OpenServiceW(manager.0, PCWSTR(wide_name.as_ptr()), SERVICE_ALL_ACCESS) {
            Ok(existing) => {
                line(format!("reconfiguring service {name}"));
                let existing = ServiceHandle(existing);
                // An update keeps the startup type the user chose (Manual,
                // Disabled, delayed start).
                let start_type = startup_plan(Some(service_start_type(&existing)?)).start_type;
                ChangeServiceConfigW(
                    existing.0,
                    SERVICE_WIN32_OWN_PROCESS,
                    start_type,
                    SERVICE_ERROR_NORMAL,
                    PCWSTR(binary.as_ptr()),
                    PCWSTR::null(),
                    None,
                    PCWSTR::null(),
                    windows::core::w!("LocalSystem"),
                    PCWSTR::null(),
                    PCWSTR(wide_display.as_ptr()),
                )?;
                existing
            }
            Err(_) => {
                line(format!("creating service {name}"));
                ServiceHandle(CreateServiceW(
                    manager.0,
                    PCWSTR(wide_name.as_ptr()),
                    PCWSTR(wide_display.as_ptr()),
                    SERVICE_ALL_ACCESS,
                    SERVICE_WIN32_OWN_PROCESS,
                    startup_plan(None).start_type,
                    SERVICE_ERROR_NORMAL,
                    PCWSTR(binary.as_ptr()),
                    PCWSTR::null(),
                    None,
                    PCWSTR::null(),
                    PCWSTR::null(),
                    PCWSTR::null(),
                )?)
            }
        }
    };
    let mut text = wide(description);
    let description = SERVICE_DESCRIPTIONW {
        lpDescription: PWSTR(text.as_mut_ptr()),
    };
    let mut actions = [
        SC_ACTION {
            Type: SC_ACTION_RESTART,
            Delay: 5_000,
        },
        SC_ACTION {
            Type: SC_ACTION_RESTART,
            Delay: 5_000,
        },
        SC_ACTION {
            Type: SC_ACTION_RESTART,
            Delay: 30_000,
        },
    ];
    let failure = SERVICE_FAILURE_ACTIONSW {
        dwResetPeriod: 86_400,
        lpRebootMsg: PWSTR::null(),
        lpCommand: PWSTR::null(),
        cActions: actions.len() as u32,
        lpsaActions: actions.as_mut_ptr(),
    };
    // SAFETY: `service` is a live handle and each info structure, with the strings and actions it points at, outlives its call.
    unsafe {
        ChangeServiceConfig2W(
            service.0,
            SERVICE_CONFIG_DESCRIPTION,
            Some((&description as *const SERVICE_DESCRIPTIONW).cast()),
        )?;
        ChangeServiceConfig2W(
            service.0,
            SERVICE_CONFIG_FAILURE_ACTIONS,
            Some((&failure as *const SERVICE_FAILURE_ACTIONSW).cast()),
        )?;
    }
    Ok(())
}

// --- Registry -------------------------------------------------------------

/// A string value (REG_SZ or REG_EXPAND_SZ, unexpanded) in the 64-bit view.
pub fn registry_string(root: HKEY, path: &str, value: &str) -> Option<String> {
    registry_string_view(root, path, value, KEY_WOW64_64KEY)
}
pub fn registry_string_view(
    root: HKEY,
    path: &str,
    value: &str,
    view: REG_SAM_FLAGS,
) -> Option<String> {
    let key = open_key(root, path, KEY_READ | view)?;
    let name = wide(value);
    let mut size = 0u32;
    let mut kind = REG_VALUE_TYPE::default();
    // SAFETY: `key` is an open key, `name` is NUL-terminated and the second read gets a buffer of `bytes` bytes.
    unsafe {
        if RegQueryValueExW(
            key.0,
            PCWSTR(name.as_ptr()),
            None,
            Some(&mut kind),
            None,
            Some(&mut size),
        ) != ERROR_SUCCESS
            || !matches!(kind, REG_SZ | REG_EXPAND_SZ)
            || size > 1 << 20
        {
            return None;
        }
        let mut buffer = vec![0u16; (size as usize).div_ceil(2) + 1];
        let mut bytes = (buffer.len() * 2) as u32;
        if RegQueryValueExW(
            key.0,
            PCWSTR(name.as_ptr()),
            None,
            None,
            Some(buffer.as_mut_ptr().cast()),
            Some(&mut bytes),
        ) != ERROR_SUCCESS
        {
            return None;
        }
        buffer.truncate((bytes as usize) / 2);
        let end = buffer.iter().position(|c| *c == 0).unwrap_or(buffer.len());
        Some(String::from_utf16_lossy(&buffer[..end]))
    }
}
pub fn registry_dword(root: HKEY, path: &str, value: &str, view: REG_SAM_FLAGS) -> Option<u32> {
    let key = open_key(root, path, KEY_READ | view)?;
    let name = wide(value);
    let mut data = 0u32;
    let mut size = 4u32;
    let mut kind = REG_VALUE_TYPE::default();
    // SAFETY: `key` is an open key, `name` is NUL-terminated and `data` holds the four bytes `size` names.
    unsafe {
        (RegQueryValueExW(
            key.0,
            PCWSTR(name.as_ptr()),
            None,
            Some(&mut kind),
            Some((&mut data as *mut u32).cast()),
            Some(&mut size),
        ) == ERROR_SUCCESS
            && kind == REG_DWORD)
            .then_some(data)
    }
}
/// A DWORD under HKLM (64-bit view): None when the key or the value does not
/// exist, Err when it cannot be read or is not a DWORD.
pub fn machine_dword(path: &str, value: &str) -> Result<Option<u32>> {
    let (key, name) = (wide(path), wide(value));
    let mut data = 0u32;
    let mut size = 4u32;
    // SAFETY: `key` and `name` are NUL-terminated and `data` holds the four bytes `size` names.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(key.as_ptr()),
            PCWSTR(name.as_ptr()),
            RRF_RT_REG_DWORD | RRF_SUBKEY_WOW6464KEY,
            None,
            Some((&mut data as *mut u32).cast()),
            Some(&mut size),
        )
    };
    match status {
        ERROR_SUCCESS => Ok(Some(data)),
        ERROR_FILE_NOT_FOUND => Ok(None),
        other => bail!("cannot read HKLM\\{path}\\{value} ({})", other.0),
    }
}
pub struct Key(pub HKEY);
impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: the key came from RegOpenKeyExW or RegCreateKeyExW and is closed only here.
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}
pub fn open_key(root: HKEY, path: &str, access: REG_SAM_FLAGS) -> Option<Key> {
    let path = wide(path);
    let mut key = HKEY::default();
    // SAFETY: `path` is NUL-terminated UTF-16 and `key` is a valid out-pointer.
    unsafe {
        (RegOpenKeyExW(root, PCWSTR(path.as_ptr()), None, access, &mut key) == ERROR_SUCCESS)
            .then_some(Key(key))
    }
}
pub fn subkeys(root: HKEY, path: &str, view: REG_SAM_FLAGS) -> Vec<String> {
    let Some(key) = open_key(root, path, KEY_READ | view) else {
        return Vec::new();
    };
    let mut names = Vec::new();
    for index in 0.. {
        let mut name = [0u16; 256];
        let mut length = name.len() as u32;
        // SAFETY: `key` is an open key and `length` is the capacity of `name` in characters.
        let status = unsafe {
            RegEnumKeyExW(
                key.0,
                index,
                Some(PWSTR(name.as_mut_ptr())),
                &mut length,
                None,
                None,
                None,
                None,
            )
        };
        if status != ERROR_SUCCESS {
            break;
        }
        names.push(String::from_utf16_lossy(&name[..length as usize]));
    }
    names
}
/// Value names and string data of a key's values.
pub fn values(root: HKEY, path: &str, view: REG_SAM_FLAGS) -> Vec<String> {
    let Some(key) = open_key(root, path, KEY_READ | view) else {
        return Vec::new();
    };
    let mut names = Vec::new();
    for index in 0.. {
        let mut name = [0u16; 2048];
        let mut length = name.len() as u32;
        // SAFETY: `key` is an open key and `length` is the capacity of `name` in characters.
        let status = unsafe {
            RegEnumValueW(
                key.0,
                index,
                Some(PWSTR(name.as_mut_ptr())),
                &mut length,
                None,
                None,
                None,
                None,
            )
        };
        if status != ERROR_SUCCESS {
            break;
        }
        names.push(String::from_utf16_lossy(&name[..length as usize]));
    }
    names
}
pub fn delete_value(root: HKEY, path: &str, value: &str, view: REG_SAM_FLAGS) {
    if let Some(key) = open_key(root, path, KEY_SET_VALUE | view) {
        let value = wide(value);
        // SAFETY: `key` is an open key and `value` is NUL-terminated UTF-16.
        unsafe {
            let _ = RegDeleteValueW(key.0, PCWSTR(value.as_ptr()));
        }
    }
}
pub fn delete_key(root: HKEY, path: &str) {
    let path = wide(path);
    // SAFETY: `root` is a predefined key and `path` is NUL-terminated UTF-16.
    unsafe {
        let _ = RegDeleteTreeW(root, PCWSTR(path.as_ptr()));
        let _ = RegDeleteKeyExW(root, PCWSTR(path.as_ptr()), KEY_WOW64_64KEY.0, None);
    }
}
pub enum Value<'a> {
    Text(&'a str),
    Number(u32),
}
pub fn write_key(root: HKEY, path: &str, values: &[(&str, Value)]) -> Result<()> {
    let path = wide(path);
    let mut key = HKEY::default();
    // SAFETY: `path` is NUL-terminated UTF-16, `key` is a valid out-pointer and each value buffer outlives its write.
    unsafe {
        let status = RegCreateKeyExW(
            root,
            PCWSTR(path.as_ptr()),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE | KEY_WOW64_64KEY,
            None,
            &mut key,
            None,
        );
        if status != ERROR_SUCCESS {
            bail!("cannot create registry key ({})", status.0);
        }
        let key = Key(key);
        for (name, value) in values {
            let name = wide(name);
            let status = match value {
                Value::Text(text) => {
                    let data = wide(text);
                    RegSetValueExW(
                        key.0,
                        PCWSTR(name.as_ptr()),
                        None,
                        REG_SZ,
                        Some(std::slice::from_raw_parts(
                            data.as_ptr().cast(),
                            data.len() * 2,
                        )),
                    )
                }
                Value::Number(number) => RegSetValueExW(
                    key.0,
                    PCWSTR(name.as_ptr()),
                    None,
                    REG_DWORD,
                    Some(&number.to_le_bytes()),
                ),
            };
            if status != ERROR_SUCCESS {
                bail!("cannot write registry value ({})", status.0);
            }
        }
    }
    Ok(())
}

// --- Processes, firewall, permissions, shortcuts --------------------------

/// End every process with this executable name (all sessions).
pub fn kill(names: &[&str]) {
    for name in names {
        let _ = run(
            "taskkill.exe",
            &["/F", "/T", "/IM", name],
            Duration::from_secs(20),
        );
    }
}
/// Whether a process with this executable name runs. A process list that
/// cannot be read counts as running.
pub fn process_running(name: &str) -> bool {
    run(
        "tasklist.exe",
        &["/NH", "/FO", "CSV", "/FI", &format!("IMAGENAME eq {name}")],
        Duration::from_secs(20),
    )
    .map_or(true, |(code, output)| {
        code != 0
            || output
                .to_ascii_lowercase()
                .contains(&format!("\"{}\"", name.to_ascii_lowercase()))
    })
}
pub fn system32(program: &str) -> String {
    let windows = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());
    format!("{windows}\\System32\\{program}")
}
/// Filesystem canonicalization returns verbatim paths. Shell tools and the
/// firewall require conventional drive/UNC paths instead. Keep file I/O's
/// canonical paths internal, and reject names whose meaning would change.
pub fn win32_path(path: &Path) -> Result<PathBuf> {
    use std::path::{Component, Prefix};
    let text = path
        .to_str()
        .context("the installation path is not valid Unicode")?;
    if let Some(verbatim) = text.strip_prefix(r"\\?\") {
        ensure!(
            !verbatim.contains('/') && !verbatim.split('\\').any(|part| matches!(part, "." | "..")),
            "the installation path cannot be represented without changing its meaning"
        );
    }
    let path = if let Some(unc) = text.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{unc}"))
    } else if let Some(disk) = text.strip_prefix(r"\\?\") {
        PathBuf::from(disk)
    } else {
        path.to_owned()
    };
    ensure!(
        path.is_absolute()
            && matches!(path.components().next(), Some(Component::Prefix(prefix))
                if matches!(prefix.kind(), Prefix::Disk(_) | Prefix::UNC(_, _))),
        "the installation path must be an absolute drive or network-share path"
    );
    for component in path.components() {
        if let Component::Normal(name) = component {
            let name = name
                .to_str()
                .context("invalid installation path component")?;
            ensure!(
                !name.ends_with(['.', ' ']),
                "the installation path contains a name unsupported by the Windows shell"
            );
        }
    }
    Ok(path)
}
/// Update the inbound application allowance, or create it on a fresh install.
/// A rejected replacement must leave any existing allowance intact.
pub fn firewall_allow(rule: &str, program: &Path) -> Result<()> {
    firewall_allow_with(rule, program, |args| {
        run(&system32("netsh.exe"), args, Duration::from_secs(60)).map(|(code, _)| code)
    })
}
fn firewall_allow_with(
    rule: &str,
    program: &Path,
    mut execute: impl FnMut(&[&str]) -> Result<i32>,
) -> Result<()> {
    let program = win32_path(program)?;
    let name = format!("name={rule}");
    let application = format!("program={}", program.display());
    let updated = execute(&[
        "advfirewall",
        "firewall",
        "set",
        "rule",
        &name,
        "dir=in",
        "new",
        "action=allow",
        &application,
        "enable=yes",
        "profile=any",
    ])?;
    if updated == 0 {
        return Ok(());
    }
    // A missing rule and other netsh errors share a nonzero exit code. Adding
    // the desired rule handles the former without destroying existing rules
    // when either operation is denied or its parameters are rejected.
    let code = execute(&[
        "advfirewall",
        "firewall",
        "add",
        "rule",
        &name,
        "dir=in",
        "action=allow",
        &application,
        "enable=yes",
        "profile=any",
    ])?;
    if code != 0 {
        bail!(
            "adding the firewall rule for {} failed ({code}); existing rules were kept; see the setup log",
            program.display()
        );
    }
    Ok(())
}
pub fn firewall_remove(rule: &str) {
    let _ = run(
        &system32("netsh.exe"),
        &[
            "advfirewall",
            "firewall",
            "delete",
            "rule",
            &format!("name={rule}"),
        ],
        Duration::from_secs(60),
    );
}
/// Whether a firewall rule named `rule` is there, read through Windows
/// Firewall's own API: netsh says "no rule" in the PC's language and with the
/// exit code of its other errors.
pub fn firewall_rule_present(rule: &str) -> Result<bool> {
    use windows::{
        Win32::{
            NetworkManagement::WindowsFirewall::{INetFwPolicy2, NetFwPolicy2},
            System::Com::*,
        },
        core::BSTR,
    };
    // SAFETY: COM is initialised on this thread before the policy is used and released after it is dropped.
    unsafe {
        let initialized = CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok();
        let result = (|| -> Result<bool> {
            let policy: INetFwPolicy2 = CoCreateInstance(&NetFwPolicy2, None, CLSCTX_INPROC_SERVER)
                .context("opening Windows Firewall")?;
            // A missing rule is ERROR_FILE_NOT_FOUND; anything else is not knowing.
            match policy.Rules()?.Item(&BSTR::from(rule)) {
                Ok(_) => Ok(true),
                Err(error) if error.code() == ERROR_FILE_NOT_FOUND.to_hresult() => Ok(false),
                Err(error) => {
                    Err(error).with_context(|| format!("reading the firewall rule {rule}"))
                }
            }
        })();
        if initialized {
            CoUninitialize();
        }
        result
    }
}
pub fn trusted_folder(folder: &Path) -> Result<()> {
    if let Err(error) = std::fs::symlink_metadata(folder) {
        return if error.kind() == std::io::ErrorKind::NotFound {
            Ok(())
        } else {
            Err(error.into())
        };
    }
    no_reparse_points(folder)?;
    trusted_owner(folder, false)
}
pub fn trusted_ancestors(folder: &Path) -> Result<()> {
    trusted_parents(folder)?;
    trusted_folder(folder)
}
pub fn trusted_parents(folder: &Path) -> Result<()> {
    use std::os::windows::fs::MetadataExt;
    for ancestor in folder
        .ancestors()
        .skip(1)
        .filter(|p| !p.as_os_str().is_empty())
    {
        let metadata = match std::fs::symlink_metadata(ancestor) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        ensure!(
            metadata.file_attributes() & 0x400 == 0,
            "{} is a reparse point",
            ancestor.display()
        );
        trusted_owner(ancestor, system_drive_root(ancestor))?;
    }
    Ok(())
}
/// Whether `folder` belongs to Administrators, SYSTEM or TrustedInstaller,
/// or, with `administrator_accounts`, to an account in the local
/// Administrators group: for the system drive's root, which a "take
/// ownership" tweak can give to its administrator and setup must not change.
fn trusted_owner(folder: &Path, administrator_accounts: bool) -> Result<()> {
    use windows::Win32::{
        Foundation::{HLOCAL, LocalFree},
        Security::{Authorization::*, *},
    };
    let mut owner = PSID::default();
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    // SAFETY: the owner SID points into descriptor until it is freed once below.
    unsafe {
        GetNamedSecurityInfoW(
            &HSTRING::from(folder.as_os_str()),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION,
            Some(&mut owner),
            None,
            None,
            None,
            &mut descriptor,
        )
        .ok()?;
        let mut text = PWSTR::null();
        let installer = ConvertSidToStringSidW(owner, &mut text).is_ok()
            && text.to_string().is_ok_and(|sid| {
                sid == "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464"
            });
        if !text.is_null() {
            let _ = LocalFree(Some(HLOCAL(text.0.cast())));
        }
        let trusted = IsWellKnownSid(owner, WinBuiltinAdministratorsSid).as_bool()
            || IsWellKnownSid(owner, WinLocalSystemSid).as_bool()
            || installer;
        let result = if trusted {
            Ok(())
        } else {
            match administrator_accounts.then(|| administrator_account(owner)) {
                Some(Ok(true)) => Ok(()),
                Some(Err(error)) => Err(error.context(format!(
                    "{} is owned by {}; reading the Administrators group",
                    folder.display(),
                    account_name(owner)
                ))),
                _ => Err(anyhow::anyhow!(
                    "{} must be owned by Administrators, SYSTEM or TrustedInstaller{}, not {}",
                    folder.display(),
                    if administrator_accounts {
                        " or an administrator's account"
                    } else {
                        ""
                    },
                    account_name(owner)
                )),
            }
        };
        let _ = LocalFree(Some(HLOCAL(descriptor.0)));
        result
    }
}
/// Whether `folder` is the root of the drive Windows runs from (by
/// GetSystemDirectoryW, not the environment). Whoever owns that root can
/// already swap Program Files and every service in it, so trusting its owner
/// adds nothing; another drive's owner would gain Rubylight's service alone.
fn system_drive_root(folder: &Path) -> bool {
    use std::path::{Component, Prefix};
    fn letter(component: Option<Component>) -> Option<u8> {
        match component {
            Some(Component::Prefix(prefix)) => match prefix.kind() {
                Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) => {
                    Some(letter.to_ascii_uppercase())
                }
                _ => None,
            },
            _ => None,
        }
    }
    let mut buffer = [0u16; 260];
    // SAFETY: the buffer is live and writable for its whole length.
    let length = unsafe {
        windows::Win32::System::SystemInformation::GetSystemDirectoryW(Some(&mut buffer))
    } as usize;
    if length == 0 || length >= buffer.len() {
        return false;
    }
    let system = PathBuf::from(String::from_utf16_lossy(&buffer[..length]));
    let mut parts = folder.components();
    let drive = letter(parts.next());
    drive.is_some()
        && drive == letter(system.components().next())
        && parts.next() == Some(Component::RootDir)
        && parts.next().is_none()
}
/// Whether `account` is listed in the local Administrators group.
///
/// netapi32 is loaded from System32 at run time: as an import, Windows would
/// look for it beside setup.exe first (in Downloads), and run a planted copy
/// as administrator.
///
/// # Safety
/// `account` must point to a valid SID.
unsafe fn administrator_account(account: PSID) -> Result<bool> {
    use windows::{
        Win32::{
            Foundation::{FreeLibrary, HLOCAL, LocalFree},
            Security::{Authorization::ConvertStringSidToSidW, EqualSid},
            System::LibraryLoader::{GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW},
        },
        core::s,
    };
    type GetMembers = unsafe extern "system" fn(
        PCWSTR,
        PCWSTR,
        u32,
        *mut *mut u8,
        u32,
        *mut u32,
        *mut u32,
        *mut usize,
    ) -> u32;
    type BufferFree = unsafe extern "system" fn(*const std::ffi::c_void) -> u32;
    // LOCALGROUP_MEMBERS_INFO_0
    #[repr(C)]
    struct Member {
        sid: PSID,
    }
    // The group's name is translated ("Administratoren"); look it up by SID.
    let mut group = PSID::default();
    // SAFETY: `group` is freed once below, the member list once with
    // NetApiBufferFree and the library once after both functions are done;
    // the signatures are those of lmaccess.h and lmapibuf.h.
    unsafe {
        ConvertStringSidToSidW(&HSTRING::from("S-1-5-32-544"), &mut group)?;
        let name = lookup_account(group);
        let _ = LocalFree(Some(HLOCAL(group.0)));
        let (_, name) = name.context("looking up the Administrators group")?;
        let name = HSTRING::from(name);
        let library = LoadLibraryExW(
            &HSTRING::from("netapi32.dll"),
            None,
            LOAD_LIBRARY_SEARCH_SYSTEM32,
        )
        .context("loading netapi32.dll")?;
        let result = (|| -> Result<bool> {
            let get_members: GetMembers = std::mem::transmute(
                GetProcAddress(library, s!("NetLocalGroupGetMembers"))
                    .context("netapi32.dll has no NetLocalGroupGetMembers")?,
            );
            let free: BufferFree = std::mem::transmute(
                GetProcAddress(library, s!("NetApiBufferFree"))
                    .context("netapi32.dll has no NetApiBufferFree")?,
            );
            let mut members: *mut u8 = std::ptr::null_mut();
            let (mut read, mut total) = (0u32, 0u32);
            let status = get_members(
                PCWSTR::null(),
                PCWSTR(name.as_ptr()),
                0,
                &mut members,
                u32::MAX,
                &mut read,
                &mut total,
                std::ptr::null_mut(),
            );
            let listed = status == 0
                && !members.is_null()
                && std::slice::from_raw_parts(members.cast::<Member>(), read as usize)
                    .iter()
                    .any(|member| EqualSid(member.sid, account).is_ok());
            if !members.is_null() {
                free(members.cast());
            }
            ensure!(status == 0, "NetLocalGroupGetMembers failed ({status})");
            Ok(listed)
        })();
        let _ = FreeLibrary(library);
        result
    }
}
/// `DOMAIN\name` for an account, or its SID when it has none (an account
/// deleted since, or one from an earlier Windows installation).
///
/// # Safety
/// `account` must point to a valid SID.
unsafe fn account_name(account: PSID) -> String {
    use windows::Win32::{
        Foundation::{HLOCAL, LocalFree},
        Security::Authorization::ConvertSidToStringSidW,
    };
    // SAFETY: the caller's SID is only read; the SID text is freed once.
    unsafe {
        if let Ok((domain, name)) = lookup_account(account) {
            return if domain.is_empty() {
                name
            } else {
                format!("{domain}\\{name}")
            };
        }
        let mut text = PWSTR::null();
        if ConvertSidToStringSidW(account, &mut text).is_err() {
            return "an unknown account".into();
        }
        let sid = text.to_string().unwrap_or_default();
        let _ = LocalFree(Some(HLOCAL(text.0.cast())));
        sid
    }
}
/// The domain and name of an account.
///
/// # Safety
/// `account` must point to a valid SID.
unsafe fn lookup_account(account: PSID) -> Result<(String, String)> {
    use windows::Win32::Security::{LookupAccountSidW, SID_NAME_USE};
    let (mut name, mut domain) = ([0u16; 256], [0u16; 256]);
    let (mut name_len, mut domain_len) = (name.len() as u32, domain.len() as u32);
    let mut kind = SID_NAME_USE::default();
    // SAFETY: both buffers are as long as the lengths passed with them.
    unsafe {
        LookupAccountSidW(
            PCWSTR::null(),
            account,
            Some(PWSTR(name.as_mut_ptr())),
            &mut name_len,
            Some(PWSTR(domain.as_mut_ptr())),
            &mut domain_len,
            &mut kind,
        )?;
    }
    Ok((
        String::from_utf16_lossy(&domain[..domain_len as usize]),
        String::from_utf16_lossy(&name[..name_len as usize]),
    ))
}
pub fn no_reparse_points(folder: &Path) -> Result<()> {
    use std::os::windows::fs::MetadataExt;
    let mut pending = vec![folder.to_path_buf()];
    while let Some(path) = pending.pop() {
        let metadata = std::fs::symlink_metadata(&path)?;
        ensure!(
            metadata.file_attributes() & 0x400 == 0,
            "{} is a reparse point",
            path.display()
        );
        if metadata.is_dir() {
            for entry in std::fs::read_dir(&path)? {
                pending.push(entry?.path());
            }
        }
    }
    Ok(())
}
/// Limit a folder to SYSTEM and Administrators (and, if `users_read`, read
/// access for users), replacing the owner and the entire permission list.
pub fn restrict(folder: &Path, users_read: bool) -> Result<()> {
    no_reparse_points(folder)?;
    use windows::Win32::{
        Foundation::{HLOCAL, LocalFree},
        Security::{Authorization::*, *},
    };
    let sddl = if users_read {
        "O:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;0x1200a9;;;BU)"
    } else {
        "O:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)"
    };
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    // SAFETY: descriptor owns the SID and ACL until LocalFree after the last call.
    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            &HSTRING::from(sddl),
            1,
            &mut descriptor,
            None,
        )?;
        let result = (|| -> Result<()> {
            let mut owner = PSID::default();
            let mut defaulted = windows::core::BOOL(0);
            let mut present = windows::core::BOOL(0);
            let mut dacl = std::ptr::null_mut();
            GetSecurityDescriptorOwner(descriptor, &mut owner, &mut defaulted)?;
            GetSecurityDescriptorDacl(descriptor, &mut present, &mut dacl, &mut defaulted)?;
            SetNamedSecurityInfoW(
                &HSTRING::from(folder.as_os_str()),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION
                    | DACL_SECURITY_INFORMATION
                    | PROTECTED_DACL_SECURITY_INFORMATION,
                Some(owner),
                None,
                Some(dacl),
                None,
            )
            .ok()?;
            Ok(())
        })();
        let _ = LocalFree(Some(HLOCAL(descriptor.0)));
        result.with_context(|| format!("setting permissions on {}", folder.display()))
    }
}
/// Reset a folder's contents to inherit the folder's permissions.
pub fn inherit_contents(folder: &Path) -> Result<()> {
    no_reparse_points(folder)?;
    if std::fs::read_dir(folder)?.next().is_none() {
        return Ok(());
    }
    let pattern = folder.join("*").display().to_string();
    let (code, _) = run(
        &system32("icacls.exe"),
        &[&pattern, "/reset", "/T", "/C", "/L", "/Q"],
        Duration::from_secs(300),
    )?;
    ensure!(
        code == 0,
        "resetting permissions on {} failed ({code})",
        folder.display()
    );
    let (code, _) = run(
        &system32("icacls.exe"),
        &[
            &pattern,
            "/setowner",
            "*S-1-5-32-544",
            "/T",
            "/C",
            "/L",
            "/Q",
        ],
        Duration::from_secs(300),
    )?;
    ensure!(
        code == 0,
        "setting ownership on {} failed ({code})",
        folder.display()
    );
    Ok(())
}
pub fn shortcut(link: &Path, target: &Path, description: &str) -> Result<()> {
    use windows::Win32::System::Com::*;
    if let Some(parent) = link.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // SAFETY: COM is initialised on this thread before the shell link is used and released after it is dropped.
    unsafe {
        let initialized = CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok();
        let result = (|| -> Result<()> {
            let shell: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
            shell.SetPath(&HSTRING::from(target.as_os_str()))?;
            if let Some(directory) = target.parent() {
                shell.SetWorkingDirectory(&HSTRING::from(directory.as_os_str()))?;
            }
            shell.SetDescription(&HSTRING::from(description))?;
            shell.SetIconLocation(&HSTRING::from(target.as_os_str()), 0)?;
            let file: IPersistFile = windows::core::Interface::cast(&shell)?;
            file.Save(&HSTRING::from(link.as_os_str()), true)?;
            Ok(())
        })();
        if initialized {
            CoUninitialize();
        }
        result
    }
}
/// Delete a file now, or at the next restart if it is in use.
pub fn remove_file_later(path: &Path) {
    if std::fs::remove_file(path).is_ok() || !path.exists() {
        return;
    }
    let source = HSTRING::from(path.as_os_str());
    // SAFETY: `source` is NUL-terminated UTF-16; a null target schedules deletion.
    unsafe {
        let _ = windows::Win32::Storage::FileSystem::MoveFileExW(
            &source,
            PCWSTR::null(),
            windows::Win32::Storage::FileSystem::MOVEFILE_DELAY_UNTIL_REBOOT,
        );
    }
}
pub fn program_data() -> PathBuf {
    PathBuf::from(std::env::var_os("ProgramData").unwrap_or_else(|| "C:\\ProgramData".into()))
}
/// Whether `path` is on a fixed drive of this PC (C:\, D:\), where the
/// service can run from; not a network share, mapped drive or USB stick.
pub fn local_drive(path: &Path) -> bool {
    const DRIVE_FIXED: u32 = 3;
    use std::path::{Component, Prefix};
    let Some(Component::Prefix(prefix)) = path.components().next() else {
        return false;
    };
    let letter = match prefix.kind() {
        Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) => letter,
        _ => return false,
    };
    let root = wide(&format!("{}:\\", char::from(letter)));
    // SAFETY: `root` is NUL-terminated UTF-16 that outlives the call.
    unsafe {
        windows::Win32::Storage::FileSystem::GetDriveTypeW(PCWSTR(root.as_ptr())) == DRIVE_FIXED
    }
}
pub fn program_files() -> PathBuf {
    PathBuf::from(std::env::var_os("ProgramFiles").unwrap_or_else(|| "C:\\Program Files".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sid_of(text: &str) -> Result<PSID> {
        let mut sid = PSID::default();
        // SAFETY: the test leaks the SID; it lives as long as the test.
        unsafe {
            windows::Win32::Security::Authorization::ConvertStringSidToSidW(
                &HSTRING::from(text),
                &mut sid,
            )?;
        }
        Ok(sid)
    }
    fn set_owner(path: &Path, sid: &str) -> Result<()> {
        let (code, output) = run(
            &system32("icacls.exe"),
            &[
                &path.display().to_string(),
                "/setowner",
                &format!("*{sid}"),
                "/Q",
            ],
            Duration::from_secs(10),
        )?;
        assert_eq!(code, 0, "{output}");
        Ok(())
    }
    #[test]
    fn only_the_system_drive_root_may_belong_to_an_administrator_account() -> Result<()> {
        // Issue 61: a C:\ owned by the PC's administrator refused every install.
        let drive = std::env::var("SystemDrive")?;
        assert!(system_drive_root(Path::new(&format!("{drive}\\"))));
        assert!(system_drive_root(Path::new(&format!(
            "\\\\?\\{}\\",
            drive.to_lowercase()
        ))));
        assert!(!system_drive_root(Path::new(&format!("{drive}\\Games"))));
        assert!(!system_drive_root(Path::new(&drive)));
        let other = if drive.eq_ignore_ascii_case("C:") {
            "D:\\"
        } else {
            "C:\\"
        };
        assert!(!system_drive_root(Path::new(other)));
        assert!(!system_drive_root(Path::new("\\\\server\\share\\")));

        let (_, identity) = run(
            &system32("whoami.exe"),
            &["/user", "/fo", "csv", "/nh"],
            Duration::from_secs(10),
        )?;
        let mut fields = identity.trim().split(',').map(|f| f.trim_matches('"'));
        let name = fields
            .next()
            .context("whoami omitted the name")?
            .to_string();
        let sid = fields.next().context("whoami omitted SID")?.to_string();
        if sid == "S-1-5-18" {
            return Ok(());
        }
        // Direct members, as Windows lists them, independently of netapi32.
        let (code, listed) = run(
            &system32("WindowsPowerShell\\v1.0\\powershell.exe"),
            &[
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                &format!(
                    "if ((Get-LocalGroupMember -SID S-1-5-32-544).SID.Value -contains '{sid}') {{ 'listed' }} else {{ 'not listed' }}"
                ),
            ],
            Duration::from_secs(60),
        )?;
        // Get-LocalGroupMember throws on a member it cannot resolve (a
        // deleted domain account); compare only when it answered.
        let listed = match (code, listed.trim()) {
            (0, "listed") => Some(true),
            (0, "not listed") => Some(false),
            _ => None,
        };
        // SAFETY: the SIDs are valid for the whole test.
        unsafe {
            if let Some(listed) = listed {
                assert_eq!(administrator_account(sid_of(&sid)?)?, listed);
            }
            assert!(!administrator_account(sid_of("S-1-5-32-545")?)?);
        }

        let root = tempfile::tempdir()?;
        let folder = root.path().join("drive");
        std::fs::create_dir(&folder)?;
        set_owner(&folder, &sid)?;
        if let Some(listed) = listed {
            assert_eq!(trusted_owner(&folder, true).is_ok(), listed);
        }
        let error = trusted_owner(&folder, false).unwrap_err().to_string();
        assert!(
            error.to_lowercase().ends_with(&format!(
                "must be owned by administrators, system or trustedinstaller, not {}",
                name.to_lowercase()
            )),
            "{error}"
        );
        // Below the root, nothing changes: the folder itself and D:\Games.
        assert!(trusted_folder(&folder).is_err());
        assert!(trusted_parents(&folder.join("Rubylight")).is_err());
        if elevated() {
            set_owner(&folder, "S-1-5-32-545")?;
            let error = trusted_owner(&folder, true).unwrap_err().to_string();
            assert!(
                error.contains("or an administrator's account, not "),
                "{error}"
            );
        }
        Ok(())
    }
    #[test]
    fn setup_loads_no_dll_planted_beside_it() -> Result<()> {
        // setup.exe runs elevated from Downloads: a DLL it imports that is not
        // a KnownDLL would be loaded from beside it first. These are the
        // DLLs behind setup's own Windows calls (userenv.dll is left out:
        // only the MSVC test build's std imports it).
        let folder = tempfile::tempdir()?;
        let copy = folder.path().join("setup-copy.exe");
        std::fs::copy(std::env::current_exe()?, &copy)?;
        for dll in [
            "netapi32.dll",
            "version.dll",
            "comctl32.dll",
            "wtsapi32.dll",
            "iphlpapi.dll",
            "msi.dll",
            "dbghelp.dll",
        ] {
            std::fs::write(folder.path().join(dll), b"not a DLL")?;
        }
        let (code, output) = run(
            &copy.display().to_string(),
            &["--list"],
            Duration::from_secs(60),
        )?;
        assert_eq!(code, 0, "{output}");
        Ok(())
    }
    #[test]
    fn locked_folders_replace_user_ownership_and_explicit_child_grants() -> Result<()> {
        if !elevated() {
            return Ok(());
        }
        let root = tempfile::tempdir()?;
        let folder = root.path().join("locked");
        std::fs::create_dir(&folder)?;
        let child = folder.join("host.exe");
        std::fs::write(&child, b"fixture")?;
        let (_, identity) = run(
            &system32("whoami.exe"),
            &["/user", "/fo", "csv", "/nh"],
            Duration::from_secs(10),
        )?;
        let sid = identity
            .trim()
            .split(',')
            .nth(1)
            .context("whoami omitted SID")?
            .trim_matches('"');
        for path in [&folder, &child] {
            let (code, output) = run(
                &system32("icacls.exe"),
                &[
                    &path.display().to_string(),
                    "/grant:r",
                    "*S-1-1-0:(F)",
                    "/Q",
                ],
                Duration::from_secs(10),
            )?;
            assert_eq!(code, 0, "{output}");
            let (code, output) = run(
                &system32("icacls.exe"),
                &[
                    &path.display().to_string(),
                    "/setowner",
                    &format!("*{sid}"),
                    "/Q",
                ],
                Duration::from_secs(10),
            )?;
            assert_eq!(code, 0, "{output}");
        }
        assert!(trusted_folder(&folder).is_err());
        restrict(&folder, false)?;
        inherit_contents(&folder)?;
        trusted_folder(&folder)?;
        trusted_folder(&child)?;
        for path in [&folder, &child] {
            let (_, output) = run(
                &system32("icacls.exe"),
                &[&path.display().to_string(), "/findsid", "*S-1-1-0", "/Q"],
                Duration::from_secs(10),
            )?;
            assert!(!output.contains(&path.display().to_string()), "{output}");
        }
        Ok(())
    }
    #[test]
    fn cmd_lines_keep_quoted_paths_with_spaces() -> Result<()> {
        // The shape of an NSIS QuietUninstallString under Program Files, and
        // the uninstaller's own delayed self-deletion.
        let root = tempfile::tempdir()?;
        let folder = root.path().join("Sunshine Test");
        std::fs::create_dir(&folder)?;
        let uninstaller = folder.join("uninstall.cmd");
        let marker = folder.join("ran.txt");
        std::fs::write(&uninstaller, "@echo %1> \"%~dp0ran.txt\"\r\n")?;
        let (code, _) = run_cmd(
            &format!("\"{}\" /S", uninstaller.display()),
            Duration::from_secs(30),
        )?;
        assert_eq!(code, 0);
        assert_eq!(std::fs::read_to_string(&marker)?.trim(), "/S");
        let (code, _) = run_cmd(
            &format!(
                "del /f /q \"{}\" & del /f /q \"{}\"",
                marker.display(),
                uninstaller.display()
            ),
            Duration::from_secs(30),
        )?;
        assert_eq!(code, 0);
        assert!(!marker.exists() && !uninstaller.exists());
        Ok(())
    }

    #[test]
    fn a_wake_plan_in_setups_environment_never_reaches_the_host() -> Result<()> {
        // SAFETY: Windows' environment block is locked per call; no test in
        // this crate reads this variable.
        unsafe {
            std::env::set_var(
                "BUTTERPOLLO_WAKE_FIX",
                r#"{"adapters":[],"fast_startup_off":true}"#,
            )
        };
        let (code, output) = run_cmd(
            "if defined BUTTERPOLLO_WAKE_FIX (exit 7) else (exit 0)",
            Duration::from_secs(30),
        )?;
        // SAFETY: as above.
        unsafe { std::env::remove_var("BUTTERPOLLO_WAKE_FIX") };
        assert_eq!(code, 0, "{output}");
        Ok(())
    }
    #[test]
    fn driver_task_scripts_preserve_unicode_paths_and_literal_arguments() -> Result<()> {
        let root = tempfile::tempdir()?;
        let folder = root.path().join("Çağrı Müller & literal%TEMP%");
        std::fs::create_dir(&folder)?;
        let driver = folder.join("fake.ps1");
        std::fs::write(
            &driver,
            "param([string]$Text)\r\nWrite-Output $Text\r\nexit 3010\r\n",
        )?;
        let log = folder.join("result.log");
        let script = root.path().join("task.cmd");
        let expected = "Çağrı Müller & literal%TEMP%";
        std::fs::write(
            &script,
            task_script(
                &system32("WindowsPowerShell\\v1.0\\powershell.exe"),
                &[
                    "-NoProfile",
                    "-NonInteractive",
                    "-ExecutionPolicy",
                    "Bypass",
                    "-File",
                    &driver.display().to_string(),
                    expected,
                ],
                &log,
            ),
        )?;
        run(
            &system32("cmd.exe"),
            &["/D", "/C", &script.display().to_string()],
            Duration::from_secs(30),
        )?;
        let output = std::fs::read_to_string(log)?;
        assert!(output.contains(expected), "{output}");
        assert!(output.contains("BUTTERPOLLO-EXIT=3010"), "{output}");
        Ok(())
    }
    #[test]
    fn setup_tasks_have_no_timer_run_at_normal_priority_and_xml_paths_round_trip() -> Result<()> {
        let root = tempfile::tempdir()?;
        let script = root.path().join("Çağrı & Müller/task.cmd");
        let definition = root.path().join("task.xml");
        std::fs::write(&definition, task_definition(&script))?;
        let check = root.path().join("check.ps1");
        std::fs::write(
            &check,
            r#"param([string]$Definition)
$task = [xml]::new()
$task.Load($Definition)
if ($task.Task.Triggers.HasChildNodes) { exit 1 }
if ($task.Task.Principals.Principal.UserId -ne 'S-1-5-18') { exit 2 }
if ($task.Task.Settings.Priority -ne '4') { exit 3 }
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
Write-Output $task.Task.Actions.Exec.Arguments
"#,
        )?;
        let (code, output) = run(
            &system32("WindowsPowerShell\\v1.0\\powershell.exe"),
            &[
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
                &check.display().to_string(),
                &definition.display().to_string(),
            ],
            Duration::from_secs(30),
        )?;
        assert_eq!(code, 0, "{output}");
        assert!(
            output.contains(&format!("/D /S /C \"\"{}\"\"", script.display())),
            "{output}"
        );
        Ok(())
    }
    #[test]
    fn system_tasks_run_from_folders_with_spaces_and_unicode() -> Result<()> {
        // Creating a SYSTEM task needs an administrator, as setup and CI are.
        if !elevated() {
            return Ok(());
        }
        let root = tempfile::tempdir()?;
        let work = root.path().join("Çağrı & Müller setup");
        let (code, output) = run_as_system_in(
            &work,
            &system32("whoami.exe"),
            &["/user"],
            Duration::from_secs(60),
        )?;
        assert_eq!(code, 0, "{output}");
        assert!(output.contains("S-1-5-18"), "{output}");
        assert_eq!(std::fs::read_dir(&work)?.count(), 0);
        // Below normal, a busy PC starved the driver scripts past setup's limit.
        let (code, output) = run_as_system_in(
            &work,
            &system32("WindowsPowerShell\\v1.0\\powershell.exe"),
            &[
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "(Get-Process -Id $PID).PriorityClass",
            ],
            Duration::from_secs(60),
        )?;
        assert_eq!(code, 0, "{output}");
        assert_eq!(output.trim(), "Normal");
        Ok(())
    }

    #[test]
    fn windows_consumers_receive_drive_and_unc_paths_without_verbatim_prefixes() -> Result<()> {
        for (input, expected) in [
            (
                r"\\?\C:\Program Files\Butterpollo\butterpollo.exe",
                r"C:\Program Files\Butterpollo\butterpollo.exe",
            ),
            (
                r"\\?\UNC\server\share\Butterpollo ü\butterpollo.exe",
                r"\\server\share\Butterpollo ü\butterpollo.exe",
            ),
            (
                r"C:\Program Files\Butterpollo",
                r"C:\Program Files\Butterpollo",
            ),
            (r"\\server\share\Butterpollo", r"\\server\share\Butterpollo"),
        ] {
            assert_eq!(win32_path(Path::new(input))?, Path::new(expected));
        }
        for invalid in [
            r"relative\host.exe",
            r"C:host.exe",
            r"\\.\PhysicalDrive0",
            r"\\?\GLOBALROOT\Device\host.exe",
            r"\\?\C:\Folder\..\host.exe",
            r"\\?\C:\Folder\.\host.exe",
            r"\\?\C:\Folder.\host.exe",
            r"\\?\C:\Folder \host.exe",
        ] {
            assert!(win32_path(Path::new(invalid)).is_err(), "{invalid}");
        }
        let root = tempfile::tempdir()?;
        let file = root.path().join("Butterpollo ü.exe");
        std::fs::write(&file, b"fixture")?;
        let canonical = std::fs::canonicalize(&file)?;
        assert_eq!(std::fs::canonicalize(win32_path(&canonical)?)?, canonical);
        Ok(())
    }

    #[test]
    fn firewall_updates_or_creates_without_deleting_existing_rules_on_failure() -> Result<()> {
        for (results, expected_operations, succeeds) in [
            (vec![0], vec!["set"], true),
            (vec![1, 0], vec!["set", "add"], true),
            (vec![1, 1], vec!["set", "add"], false),
        ] {
            let mut results = results.into_iter();
            let mut operations = Vec::new();
            let result = firewall_allow_with(
                "Rubylight",
                Path::new(r"\\?\C:\Program Files\Butterpollo\butterpollo.exe"),
                |args| {
                    assert!(
                        args.contains(&r"program=C:\Program Files\Butterpollo\butterpollo.exe")
                    );
                    assert!(!args.contains(&"delete"));
                    operations.push(args[2].to_owned());
                    Ok(results.next().expect("unexpected firewall operation"))
                },
            );
            assert_eq!(result.is_ok(), succeeds);
            assert_eq!(operations, expected_operations);
        }
        firewall_allow_with("Butterpollo", Path::new(r"\\.\PhysicalDrive0"), |_| {
            panic!("invalid paths must fail before touching firewall rules")
        })
        .unwrap_err();
        Ok(())
    }

    #[test]
    fn startup_plan_keeps_what_the_user_set() {
        let keep = SERVICE_START_TYPE(SERVICE_NO_CHANGE);
        assert_eq!(
            startup_plan(None),
            StartupPlan {
                start_type: SERVICE_AUTO_START,
                start: true
            }
        );
        for existing in [SERVICE_AUTO_START, SERVICE_DEMAND_START] {
            assert_eq!(
                startup_plan(Some(existing)),
                StartupPlan {
                    start_type: keep,
                    start: true
                }
            );
        }
        assert_eq!(
            startup_plan(Some(SERVICE_DISABLED)),
            StartupPlan {
                start_type: keep,
                start: false
            }
        );
    }
    /// Deletes the throwaway service when the test ends, even if it fails.
    struct Throwaway(String);
    impl Drop for Throwaway {
        fn drop(&mut self) {
            let _ = delete_service(&self.0);
        }
    }
    fn set_start_type(name: &str, start_type: SERVICE_START_TYPE) -> Result<()> {
        let service = open_service(name).context("throwaway service is missing")?;
        // SAFETY: `service` is a live handle; null strings and NO_CHANGE leave everything else alone.
        unsafe {
            ChangeServiceConfigW(
                service.0,
                ENUM_SERVICE_TYPE(SERVICE_NO_CHANGE),
                start_type,
                SERVICE_ERROR(SERVICE_NO_CHANGE),
                PCWSTR::null(),
                PCWSTR::null(),
                None,
                PCWSTR::null(),
                PCWSTR::null(),
                PCWSTR::null(),
                PCWSTR::null(),
            )?;
        }
        Ok(())
    }
    fn start_type_of(name: &str) -> Result<SERVICE_START_TYPE> {
        service_start_type(&open_service(name).context("throwaway service is missing")?)
    }
    /// Reconfiguring an installed service (every update) must not turn a
    /// Manual or Disabled service back into Automatic.
    #[test]
    fn reconfiguring_a_service_keeps_its_startup_type() -> Result<()> {
        if !elevated() {
            return Ok(());
        }
        // Only ever a service this test created, under a name of its own.
        let name = format!("RubylightSetupTest{}", std::process::id());
        let program = std::env::current_exe()?;
        let _cleanup = Throwaway(name.clone());
        install_service(&name, "Rubylight setup test", "Throwaway", &program)?;
        assert_eq!(start_type_of(&name)?, SERVICE_AUTO_START);
        for kept in [SERVICE_DEMAND_START, SERVICE_DISABLED] {
            set_start_type(&name, kept)?;
            install_service(&name, "Rubylight setup test", "Throwaway", &program)?;
            assert_eq!(start_type_of(&name)?, kept);
        }
        // A disabled service is left alone instead of failing the start.
        assert!(!start_service_unless_disabled(&name)?);
        assert!(!service_running(&name));
        Ok(())
    }
}
