//! Windows facilities setup needs: elevation, commands, services, registry,
//! processes, firewall rules, shortcuts and folder permissions.
mod pending_renames;
mod registry;
mod security;
mod services;

#[cfg(test)]
pub(crate) use pending_renames::{prune_pending_rename_words, rename_aside_for_delete};
pub use pending_renames::{prune_pending_renames, remove_file_later};
pub use registry::{
    Value, delete_key, delete_value, machine_dword, registry_dword, registry_string,
    registry_string_view, subkeys, values, write_key,
};
#[cfg(test)]
pub(crate) use security::INHERIT_OPERATIONS;
pub use security::{
    Untrusted, create_restricted, inherit_contents, no_reparse_points, restrict, restrict_except,
    trusted_ancestors, trusted_folder, trusted_parents, trusted_tree,
};
pub use services::{
    delete_service, install_service, service_program, service_running, set_service_display_name,
    start_service, start_service_unless_disabled, stop_service, wait_stopped,
};

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
        Foundation::{CloseHandle, ERROR_FILE_NOT_FOUND},
        System::Threading::*,
        UI::Shell::*,
    },
    core::{HSTRING, PCWSTR},
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
///
/// Task files live inside an administrators-only directory: `install\.staging\tasks`
/// when `install` itself is trusted (owned by SYSTEM/Administrators/TrustedInstaller
/// with no non-admin writer or reparse points), otherwise an ephemeral random
/// restricted directory created directly under `%ProgramData%` (never under an
/// unverified `ProgramData\Butterpollo`).
/// Whether `install` is an existing, administrators-only directory whose
/// ancestors are also trusted (fails closed on missing folders or ACL errors).
fn install_task_root_usable(install: &Path) -> bool {
    trusted_parents(install).is_ok() && trusted_dir_only(install).is_ok()
}
/// Create a fresh, administrators-only ephemeral task folder under
/// `fallback_root` after checking `fallback_root` and its ancestors
/// non-recursively (sibling junctions in `%ProgramData%` must not fail it).
fn create_ephemeral_task_root(fallback_root: &Path) -> Result<PathBuf> {
    let ephemeral = fallback_root.join(random_task_id()?);
    trusted_parents(&ephemeral)?;
    create_restricted(&ephemeral, false)?;
    Ok(ephemeral)
}
pub fn run_as_system(
    install: &Path,
    program: &str,
    args: &[&str],
    timeout: Duration,
) -> Result<(i32, String)> {
    if install_task_root_usable(install) {
        let staging = crate::install::staging_root(install);
        match std::fs::symlink_metadata(&staging) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                create_restricted(&staging, false)?;
            }
            Err(error) => return Err(error.into()),
            Ok(_) => trusted_dir_only(&staging)?,
        }
        let work = staging.join("tasks");
        let result = run_as_system_in(&work, program, args, timeout);
        let _ = std::fs::remove_dir(&work);
        let _ = std::fs::remove_dir(&staging);
        return result;
    }
    let root = program_data();
    let ephemeral = create_ephemeral_task_root(&root)?;
    let result = run_as_system_in(&ephemeral, program, args, timeout);
    let _ = std::fs::remove_dir_all(&ephemeral);
    result
}
/// Non-recursive trust check for an existing directory: must be a real
/// directory (no reparse point), owned by SYSTEM/Administrators/TrustedInstaller,
/// and writable by no other account.
fn trusted_dir_only(folder: &Path) -> Result<()> {
    use std::os::windows::fs::MetadataExt;
    let metadata = std::fs::symlink_metadata(folder)?;
    if metadata.file_attributes() & 0x400 != 0 {
        return Err(Untrusted(format!("{} is a reparse point", folder.display())).into());
    }
    ensure!(metadata.is_dir(), "{} is not a directory", folder.display());
    security::trusted_owner(folder, false)?;
    if let Some(writer) = security::other_writer(folder)? {
        return Err(Untrusted(format!(
            "{} may be changed by {writer}, not only Administrators, SYSTEM or TrustedInstaller",
            folder.display()
        ))
        .into());
    }
    Ok(())
}
/// Prepare `work` as a verified administrators-only root for SYSTEM task files.
/// A missing folder is created atomically with restricted permissions; an
/// existing one must pass [`trusted_tree`] (no reparse points, trusted owner,
/// no non-admin writers).
fn prepare_task_root(work: &Path) -> Result<()> {
    match std::fs::symlink_metadata(work) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return create_restricted(work, false);
        }
        Err(error) => return Err(error.into()),
        Ok(_) => trusted_tree(work, &[])?,
    }
    if let Ok(entries) = std::fs::read_dir(work) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            if name.to_string_lossy().starts_with("ButterpolloSetup-") {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }
    Ok(())
}
/// Cryptographically random task identifier so local users cannot pre-create
/// or guess task script / XML filenames.
fn random_task_id() -> Result<String> {
    // SAFETY: CoCreateGuid takes no inputs and returns a fresh 128-bit GUID.
    let guid = unsafe { windows::Win32::System::Com::CoCreateGuid() }
        .context("generating a setup task identifier")?;
    let [d0, d1, d2, d3, d4, d5, d6, d7] = guid.data4;
    Ok(format!(
        "ButterpolloSetup-{:08x}{:04x}{:04x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        guid.data1, guid.data2, guid.data3, d0, d1, d2, d3, d4, d5, d6, d7
    ))
}
/// Create a brand-new file (`create_new(true)`) so an existing file or symlink
/// is never opened or overwritten.
fn write_new_file(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .with_context(|| format!("creating {}", path.display()))?;
    file.write_all(bytes)
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}
fn run_as_system_in(
    work: &Path,
    program: &str,
    args: &[&str],
    timeout: Duration,
) -> Result<(i32, String)> {
    prepare_task_root(work)?;
    let id = random_task_id()?;
    let run_dir = work.join(&id);
    create_restricted(&run_dir, false)?;
    let log = run_dir.join("run.log");
    let script = run_dir.join("run.cmd");
    let definition = run_dir.join("task.xml");
    let result = (|| -> Result<(i32, String)> {
        write_new_file(&script, task_script(program, args, &log).as_bytes())?;
        write_new_file(&definition, &task_definition(&script))?;
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
        let outcome = (|| -> Result<(i32, String)> {
            if run(&schtasks, &["/Run", "/TN", &id], Duration::from_secs(60))?.0 != 0 {
                bail!("starting the setup task failed");
            }
            let deadline = Instant::now() + timeout;
            loop {
                let text = std::fs::read(&log)
                    .map(|b| String::from_utf8_lossy(&b).into_owned())
                    .unwrap_or_default();
                if let Some((output, code)) = parse_completed_task_log(&text) {
                    if !output.trim().is_empty() {
                        line(output.trim_end());
                    }
                    line(format!("  exit code {code}"));
                    return Ok((code, output.to_owned()));
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
        outcome
    })();
    if !result
        .as_ref()
        .is_err_and(|error| error.is::<StillRunning>())
    {
        let _ = std::fs::remove_dir_all(&run_dir);
    }
    result
}

fn parse_completed_task_log(text: &str) -> Option<(&str, i32)> {
    let index = text.rfind("BUTTERPOLLO-EXIT=")?;
    let after = &text[index + "BUTTERPOLLO-EXIT=".len()..];
    let line_end = after.find('\n')?;
    let code = after[..line_end].trim().parse::<i32>().ok()?;
    Some((&text[..index], code))
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
/// One argument as the started program reads it back: quoted when it is
/// empty or has a space, tab or quote, with the backslashes before a quote,
/// and before the closing one, doubled (a folder that ends in a backslash
/// would otherwise take the rest of the line with it).
fn argument(argument: &str) -> String {
    if !argument.is_empty() && !argument.contains([' ', '\t', '"']) {
        return argument.to_owned();
    }
    let mut quoted = String::from('"');
    let mut slashes = 0;
    for character in argument.chars() {
        match character {
            '\\' => slashes += 1,
            '"' => {
                quoted.extend(std::iter::repeat_n('\\', slashes * 2 + 1));
                quoted.push('"');
                slashes = 0;
            }
            other => {
                quoted.extend(std::iter::repeat_n('\\', slashes));
                quoted.push(other);
                slashes = 0;
            }
        }
    }
    quoted.extend(std::iter::repeat_n('\\', slashes * 2));
    quoted.push('"');
    quoted
}
/// Start this program again elevated with the same arguments and return its
/// exit code (1223 when the user declines the UAC prompt).
pub fn relaunch_elevated() -> Result<i32> {
    let exe = std::env::current_exe()?;
    let arguments: Vec<String> = std::env::args().skip(1).map(|a| argument(&a)).collect();
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

/// End every process with this executable name (all sessions).
pub fn kill(names: &[&str]) {
    let taskkill = system32("taskkill.exe");
    for name in names {
        let _ = run(
            &taskkill,
            &["/F", "/T", "/IM", name],
            Duration::from_secs(20),
        );
    }
}
/// Whether a process with this executable name runs. A process list that
/// cannot be read counts as running.
pub fn process_running(name: &str) -> bool {
    let tasklist = system32("tasklist.exe");
    run(
        &tasklist,
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
/// IddCx 1.10 (virtual display driver INF minimum) ships with Windows 11 23H2 (build 22631).
pub const IDDCX_1_10_BUILD: u32 = 22631;
/// Current Windows build number from `HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion`.
pub fn windows_build() -> Option<u32> {
    registry_string(
        windows::Win32::System::Registry::HKEY_LOCAL_MACHINE,
        r"SOFTWARE\Microsoft\Windows NT\CurrentVersion",
        "CurrentBuildNumber",
    )?
    .trim()
    .parse()
    .ok()
}
/// Whether `rubylight-vdisplay` is supported on `build` (`None` fails open).
pub fn virtual_display_supported(build: Option<u32>) -> bool {
    build.is_none_or(|b| b >= IDDCX_1_10_BUILD)
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
use security::{
    administrator_account, not_found, other_writer, system_drive_root, trusted_owner, vanished,
};
#[cfg(test)]
use services::{StartupPlan, open_service, service_start_type, startup_plan};
#[cfg(test)]
use windows::Win32::{
    Security::PSID,
    System::Services::{
        ChangeServiceConfigW, ENUM_SERVICE_TYPE, SERVICE_AUTO_START, SERVICE_DEMAND_START,
        SERVICE_DISABLED, SERVICE_ERROR, SERVICE_NO_CHANGE, SERVICE_START_TYPE,
    },
};

#[cfg(test)]
mod tests;
