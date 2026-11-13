//! Removing Rubylight. Settings and drivers stay unless asked otherwise,
//! except Rubylight's own display driver.
use crate::{
    detect::{self, UNINSTALL},
    install::{
        FIREWALL_RULE, LEGACY_FIREWALL_RULE, OWN_PROCESSES, legacy_start_menu_link, profile,
        start_menu_link,
    },
    log::line,
    payload, system,
    ui::Progress,
};
use anyhow::{Result, bail};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    time::Duration,
};
use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, KEY_WOW64_64KEY};

pub struct Options {
    pub factory_reset: bool,
    pub remove_drivers: bool,
}
/// The uninstaller's exit code when Rubylight was removed but the Windows
/// settings child-session setup changed were not restored.
pub const UNRESTORED_EXIT: i32 = 2;
/// A removal that went through.
pub struct Removed {
    /// Problems the user must see.
    pub notes: Vec<String>,
    /// The Windows settings child-session setup changed were not restored.
    pub unrestored: bool,
}
impl Removed {
    /// 0, or [`UNRESTORED_EXIT`] so a quiet uninstall's caller can tell the
    /// settings were not put back.
    pub fn exit_code(&self) -> i32 {
        if self.unrestored { UNRESTORED_EXIT } else { 0 }
    }
}
/// The installation to remove: the registered one, else the folder this
/// program runs from when it holds a Rubylight package. Never a guess:
/// run from Downloads on a PC without Rubylight, this is None.
pub fn install_location() -> Option<PathBuf> {
    let found = detect::scan();
    found
        .butterpollo
        .as_ref()
        .and_then(|p| p.location.clone())
        .or(found.service_install)
        .or_else(|| {
            std::env::current_exe()
                .ok()
                .and_then(|exe| exe.parent().map(Path::to_path_buf))
                .filter(|folder| is_package(folder))
        })
}
/// The record child-session setup leaves
/// (`butterpollo_core::child_session::SETUP_FILE`), in its folder beside the
/// profile (`SETUP_FOLDER`). One of that name in the profile, which its user
/// can write, is never read.
const CHILD_SESSION_SETUP_FILE: &str = "child_session_setup.json";
const CHILD_SESSION_SETUP_FOLDER: &str = "windows-settings";
/// The host's flag that puts back what that setup changed
/// (`butterpollo_core::child_session::RESTORE_FLAG`).
const CHILD_SESSION_RESTORE_FLAG: &str = "--child-session-restore";
/// Its exit code when the restore failed after putting Remote Desktop back
/// as it was before setup: RDP is the user's own again and stays as it is
/// (`butterpollo_core::child_session::RESTORE_PARTLY_EXIT`).
const CHILD_SESSION_RESTORE_PARTLY_EXIT: i32 = 4;
/// The PC's child-session setup record
/// (`butterpollo_core::child_session::setup_record`), beside `profile`.
fn child_session_record(profile: &Path) -> PathBuf {
    profile
        .parent()
        .unwrap_or(profile)
        .join(CHILD_SESSION_SETUP_FOLDER)
        .join(CHILD_SESSION_SETUP_FILE)
}
fn child_session_recorded(profile: &Path) -> bool {
    child_session_record(profile).is_file()
}
/// The installed host when child-session setup left a record for `profile`
/// for it to restore; None when there is nothing to put back or no host to
/// do it.
fn child_session_restore_pending(profile: &Path, install: &Path) -> Option<PathBuf> {
    let program = install.join("butterpollo.exe");
    (child_session_recorded(profile) && program.is_file()).then_some(program)
}
/// Whether `folder` holds an installed Rubylight package.
fn is_package(folder: &Path) -> bool {
    payload::manifest(folder).is_ok_and(|entries| {
        entries
            .iter()
            .any(|entry| entry.path.eq_ignore_ascii_case("butterpollo.exe"))
    })
}
/// Why --remove-drivers leaves these drivers in a side-by-side installation.
const SHARED_DRIVERS_KEPT: &str = "The virtual gamepad driver and libvirtualdisplay stay installed, as another host on this PC may use them. Rubylight's own display driver was removed.";
/// Whether to stop and whether to delete the service named Rubylight's, whose
/// program this is: another host may hold the name, and then it is left alone.
fn service_steps(program: Option<&Path>, install: &Path) -> (bool, bool) {
    let ours = program.filter(|program| detect::our_service(program));
    (
        ours.is_some(),
        ours.is_some_and(|program| inside(program, install)),
    )
}
/// Whether `path` is in `folder`, as Windows compares names: the service and
/// the Apps entry may spell the same folder in different case, and a service
/// that is ours but left registered keeps the next installation from starting.
fn inside(path: &Path, folder: &Path) -> bool {
    let name = |part: std::path::Component| part.as_os_str().to_string_lossy().to_lowercase();
    let mut path = path.components();
    folder
        .components()
        .all(|part| path.next().is_some_and(|own| name(own) == name(part)))
}
/// Remove Rubylight; the notes are problems the user must see, though the
/// removal itself went through.
pub fn uninstall(options: &Options, progress: &Progress) -> Result<Removed> {
    let Some(install) = install_location() else {
        bail!("Rubylight is not installed on this PC");
    };
    line(format!("uninstalling from {}", install.display()));
    progress.set("Stopping Rubylight…");
    let side_by_side = detect::side_by_side();
    let service = detect::service();
    let (stop, delete) = service_steps(system::service_program(service).as_deref(), &install);
    if stop {
        let _ = system::stop_service(service);
    }
    system::kill(&OWN_PROCESSES);
    if delete {
        system::delete_service(service)?;
    }
    system::firewall_remove(FIREWALL_RULE);
    system::firewall_remove(LEGACY_FIREWALL_RULE);
    crate::install::untrust_console();
    remove_layers(&install);
    remove_earlier_folder(&install);
    // Settings for the removed child-session feature: its setup, in
    // development builds, turned on Remote Desktop, child sessions and
    // firewall rules, and the host that recorded them puts them back while
    // it is still installed. When they are not restored (the restore failed,
    // or could not run: the host missing, not starting, timing out or
    // crashing), this uninstaller turns Remote Desktop off itself and reads
    // it back, and the record stays, even through a factory reset: Rubylight
    // installed again restores the rest at its start. So it does when the
    // record is gone but setup's block rule is still there. The uninstaller
    // then exits with UNRESTORED_EXIT. The password those builds saved goes
    // here too, record or not: a restore can have removed the record and
    // left the file, and the host may be missing.
    let mut notes = Vec::new();
    let unrestored = restore_child_session(&install, progress);
    if let Some(problem) = &unrestored {
        line(format!("warning: {problem}"));
        notes.push(problem.clone());
    }
    for problem in delete_child_session_passwords() {
        line(format!("warning: {problem}"));
        notes.push(problem);
    }

    let powershell = format!(
        "{}\\System32\\WindowsPowerShell\\v1.0\\powershell.exe",
        std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into())
    );
    // Rubylight's own display driver serves only Rubylight, and the
    // certificate it trusted must not outlive it: it goes even when the
    // other drivers stay.
    let rubylight_display = install.join(crate::install::RUBYLIGHT_DISPLAY_SCRIPT);
    if rubylight_display.is_file() {
        progress.set("Removing the Rubylight display driver…");
        let path = rubylight_display.display().to_string();
        let arguments = [
            "-NoLogo",
            "-NonInteractive",
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
            path.as_str(),
            "-Uninstall",
            "-InstallerBestEffort",
        ];
        let result = system::run_as_system(&powershell, &arguments, Duration::from_secs(600));
        notes.extend(removal_note(
            "Rubylight display",
            "Its device (Root\\RubylightVDisplay) or driver package may still be installed",
            result,
        ));
    }
    if options.remove_drivers && side_by_side {
        line(format!("note: {SHARED_DRIVERS_KEPT}"));
        notes.push(SHARED_DRIVERS_KEPT.to_owned());
    } else if options.remove_drivers {
        progress.set("Removing the virtual drivers…");
        for (script, name, args) in [
            (
                "drivers\\gamepad\\cleanup.ps1",
                "virtual gamepad",
                vec!["-InstallerBestEffort", "-RemoveDriverStorePackage:1"],
            ),
            // Left by an installation from before Rubylight's own driver.
            (
                "drivers\\display\\install.ps1",
                "earlier virtual display",
                vec!["-Uninstall"],
            ),
        ] {
            let path = install.join(script);
            if path.is_file() {
                let path = path.display().to_string();
                let mut arguments = vec![
                    "-NoLogo",
                    "-NonInteractive",
                    "-NoProfile",
                    "-ExecutionPolicy",
                    "Bypass",
                    "-File",
                    path.as_str(),
                ];
                arguments.extend(args);
                let result =
                    system::run_as_system(&powershell, &arguments, Duration::from_secs(600));
                notes.extend(removal_note(name, "It may still be installed", result));
            }
        }
        // The libvirtualdisplay driver earlier versions installed; with
        // nothing left this changes nothing.
        let removal = crate::libvirtualdisplay::remove(&mut crate::libvirtualdisplay::Native);
        if let Some(error) = removal.error {
            notes.extend(removal_note(
                crate::install::LIBVIRTUALDISPLAY,
                "It may still be installed",
                Err(error),
            ));
        }
    }

    progress.set("Removing files…");
    let _ = std::fs::remove_file(start_menu_link());
    let _ = std::fs::remove_file(legacy_start_menu_link());
    system::delete_key(HKEY_LOCAL_MACHINE, &format!("{UNINSTALL}\\Butterpollo"));
    detect::forget_service();
    let touched = remove_package(&install);
    remove_leftover_folders(&install, &touched);
    if options.factory_reset {
        progress.set("Deleting settings and paired devices…");
        let records: Vec<PathBuf> = [child_session_record(&profile())]
            .into_iter()
            .filter(|record| unrestored.is_some() && record.is_file())
            .collect();
        let root = profile().parent().unwrap_or(&profile()).to_path_buf();
        let left = remove_profile(&root, &records);
        if let Some(note) = profile_note(&root, &left) {
            line(format!("warning: {note}"));
            notes.push(note);
        }
        if !records.is_empty() {
            let kept = format!(
                "The factory reset kept {} so the Windows settings can still be restored: install Rubylight again, which restores them at its start (or choose Restore again under Maintenance in the console).",
                records
                    .iter()
                    .map(|record| record.display().to_string())
                    .collect::<Vec<_>>()
                    .join(" and ")
            );
            line(format!("warning: {kept}"));
            notes.push(kept);
        }
    }
    // This program may run from the folder being removed: delete it once it
    // has exited, then the folder if nothing else is left in it (rd without
    // /s refuses a folder that is not empty).
    if let Ok(exe) = std::env::current_exe()
        && exe.starts_with(&install)
    {
        let command = format!(
            "ping 127.0.0.1 -n 3 > nul & del /f /q \"{}\" & rd \"{}\"",
            exe.display(),
            install.display()
        );
        use std::os::windows::process::CommandExt;
        let _ = std::process::Command::new(system::system32("cmd.exe"))
            .raw_arg(system::cmd_line(&command))
            .creation_flags(0x0800_0000 | 0x0000_0008)
            .spawn();
    } else {
        let _ = std::fs::remove_dir(&install);
    }
    Ok(Removed {
        notes,
        unrestored: unrestored.is_some(),
    })
}
/// Put back the Windows settings the removed child-session setup changed, through the
/// installed host; the reason when they were not restored and its record
/// is still needed, with what was done about Remote Desktop. None when there
/// was nothing to restore or it worked.
fn restore_child_session(install: &Path, progress: &Progress) -> Option<String> {
    restore_child_session_with(
        &crate::install::profile(),
        install,
        &mut NativeRegistry,
        child_session_block_rule_present,
        |program| {
            progress.set("Restoring the Windows settings child sessions changed…");
            // The record is the PC's, not the profile's: no --config-dir.
            system::run(
                &program.display().to_string(),
                &[CHILD_SESSION_RESTORE_FLAG],
                Duration::from_secs(180),
            )
        },
    )
}
/// [`restore_child_session`] for `profile`, running the host with `run`.
/// Whatever keeps the settings from coming back, Remote Desktop is turned
/// off here too: a host that never ran (missing, not starting), timed out
/// or crashed did not turn it off itself. Without a record, so does setup's
/// block rule that `block_rule` finds still there ([`child_session_unrecorded`]).
/// Except after a restore that put RDP back as the user had it and failed on
/// the rest ([`CHILD_SESSION_RESTORE_PARTLY_EXIT`]): RDP is left as it is.
fn restore_child_session_with(
    profile: &Path,
    install: &Path,
    registry: &mut dyn MachineRegistry,
    block_rule: impl FnOnce() -> Result<bool>,
    run: impl FnOnce(&Path) -> Result<(i32, String)>,
) -> Option<String> {
    let problem = match child_session_restore_pending(profile, install) {
        None if !child_session_recorded(profile) => {
            child_session_unrecorded(&*registry, block_rule)?
        }
        None => format!("{} is missing", install.join("butterpollo.exe").display()),
        Some(program) => match run(&program) {
            Ok((0, _)) => return None,
            // RDP is back as the user had it, and a later restore skips it: turned off now,
            // it would stay off.
            Ok((CHILD_SESSION_RESTORE_PARTLY_EXIT, output)) => {
                return Some(format!(
                    "The Windows settings the removed child-session feature changed were partly restored: {}. Remote Desktop was left as it is.",
                    output.trim().trim_end_matches('.')
                ));
            }
            Ok((code, output)) => format!("({code}) {}", output.trim()),
            Err(error) => format!("{error:#}"),
        },
    };
    Some(format!(
        "The Windows settings the removed child-session feature changed (Remote Desktop, its firewall rules, child sessions) were not restored: {}. {}.",
        problem.trim().trim_end_matches('.'),
        turn_rdp_off(registry)
    ))
}
/// The block rule child-session setup adds, as `"<name> (TCP)"` and
/// `"<name> (UDP)"` (`butterpollo_core::child_session::BLOCK_RULE`).
const CHILD_SESSION_BLOCK_RULE: &str = "Rubylight child session: RDP only on this PC";
/// Without a record, as the host's `enforce_unrecorded`: Remote Desktop on
/// while setup's block rule is still there (`block_rule`) was turned on by a
/// setup whose record is gone, and nothing restores it; the problem to say
/// before RDP is turned off. None with RDP off, or without the rule: RDP is
/// then the user's own. A rule that can't be read changes nothing, said in
/// the log.
fn child_session_unrecorded(
    registry: &dyn MachineRegistry,
    block_rule: impl FnOnce() -> Result<bool>,
) -> Option<String> {
    if let Ok(Some(1)) = registry.read_dword(TERMINAL_SERVER, DENY_TS_CONNECTIONS) {
        return None;
    }
    match block_rule() {
        Ok(true) => Some(format!(
            "their record is gone, but the firewall rule \"{CHILD_SESSION_BLOCK_RULE}\" setup adds is still there"
        )),
        Ok(false) => None,
        Err(error) => {
            line(format!(
                "warning: whether the firewall rule \"{CHILD_SESSION_BLOCK_RULE}\" child-session setup adds is still there could not be read, so Remote Desktop was left as it is: {error:#}"
            ));
            None
        }
    }
}
/// Whether either of [`CHILD_SESSION_BLOCK_RULE`]'s rules is on this PC.
fn child_session_block_rule_present() -> Result<bool> {
    for protocol in ["TCP", "UDP"] {
        if system::firewall_rule_present(&format!("{CHILD_SESSION_BLOCK_RULE} ({protocol})"))? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Where the removed child-session feature saved the account's Windows
/// password, encrypted with DPAPI, under each `AppData\Local`, and the file a
/// save cut short left (`butterpollo_core::child_session::PASSWORD_FOLDER`
/// and `PASSWORD_FILES`).
const CHILD_SESSION_PASSWORD_FOLDER: &str = "Rubylight";
const CHILD_SESSION_PASSWORD_FILES: [&str; 2] =
    ["child-session-password.bin", "child-session-password.tmp"];
/// The accounts' profile folders, each a `ProfileImagePath`.
const PROFILE_LIST: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\ProfileList";
/// Delete the saved child-session passwords in every profile (ProfileList's
/// `S-1-5-21-*` `ProfileImagePath`s) and in this account's own
/// `%LOCALAPPDATA%`; what could not be deleted, each with its path.
fn delete_child_session_passwords() -> Vec<String> {
    let profiles: Vec<PathBuf> = system::subkeys(HKEY_LOCAL_MACHINE, PROFILE_LIST, KEY_WOW64_64KEY)
        .into_iter()
        .filter(|sid| sid.starts_with("S-1-5-21-"))
        .filter_map(|sid| {
            system::registry_string(
                HKEY_LOCAL_MACHINE,
                &format!(r"{PROFILE_LIST}\{sid}"),
                "ProfileImagePath",
            )
        })
        .map(|path| PathBuf::from(expand_environment(&path)))
        .collect();
    let local: Vec<PathBuf> = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .into_iter()
        .collect();
    let (deleted, failed) =
        delete_child_session_passwords_in(&profiles, &local, wipe_file, |folder| {
            match std::fs::remove_dir(folder) {
                Err(error) if error.kind() != std::io::ErrorKind::DirectoryNotEmpty => Err(error),
                _ => Ok(()),
            }
        });
    for path in deleted {
        line(format!(
            "deleted the child-session password Rubylight had saved (DPAPI-encrypted) at {}",
            path.display()
        ));
    }
    failed
}
/// `%NAME%` in a REG_EXPAND_SZ value, from this process's environment; a
/// name it doesn't have stays as it is.
fn expand_environment(text: &str) -> String {
    let mut expanded = String::new();
    let mut rest = text;
    while let Some(start) = rest.find('%') {
        let Some(length) = rest[start + 1..].find('%') else {
            break;
        };
        let name = &rest[start + 1..start + 1 + length];
        expanded.push_str(&rest[..start]);
        match std::env::var(name) {
            Ok(value) if !name.is_empty() => expanded.push_str(&value),
            _ => expanded.push_str(&rest[start..start + length + 2]),
        }
        rest = &rest[start + length + 2..];
    }
    expanded.push_str(rest);
    expanded
}
/// [`delete_child_session_passwords`] in each `AppData\Local` under
/// `profiles`, and in each of `local`: a path that is not absolute or holds
/// `..` is skipped, each folder once. Each file through `wipe` (Ok(false)
/// when there is none), then each folder a file went from through
/// `remove_empty`. The files deleted, and each failure with its path; one
/// failure stops none of the others.
fn delete_child_session_passwords_in(
    profiles: &[PathBuf],
    local: &[PathBuf],
    mut wipe: impl FnMut(&Path) -> std::io::Result<bool>,
    mut remove_empty: impl FnMut(&Path) -> std::io::Result<()>,
) -> (Vec<PathBuf>, Vec<String>) {
    let usable = |path: &Path| {
        path.is_absolute()
            && !path
                .components()
                .any(|part| part == std::path::Component::ParentDir)
    };
    let mut folders: Vec<PathBuf> = Vec::new();
    for folder in profiles
        .iter()
        .filter(|profile| usable(profile))
        .map(|profile| profile.join("AppData").join("Local"))
        .chain(local.iter().filter(|local| usable(local)).cloned())
        .map(|local| local.join(CHILD_SESSION_PASSWORD_FOLDER))
    {
        let key = folder.to_string_lossy().to_lowercase();
        if !folders
            .iter()
            .any(|known| known.to_string_lossy().to_lowercase() == key)
        {
            folders.push(folder);
        }
    }
    let (mut deleted, mut failed) = (Vec::new(), Vec::new());
    for folder in folders {
        let mut emptied = false;
        for name in CHILD_SESSION_PASSWORD_FILES {
            let path = folder.join(name);
            match wipe(&path) {
                Ok(true) => {
                    deleted.push(path);
                    emptied = true;
                }
                Ok(false) => {}
                Err(error) => failed.push(format!(
                    "The child-session password Rubylight had saved at {} could not be deleted: {error}; delete it by hand.",
                    path.display()
                )),
            }
        }
        if emptied && let Err(error) = remove_empty(&folder) {
            failed.push(format!(
                "The empty folder {} could not be removed: {error}.",
                folder.display()
            ));
        }
    }
    (deleted, failed)
}
/// Overwrite the file at `path` with zeros, flush it to disk and delete it;
/// false when there is none. Opened without following a link, and refused
/// with nothing written when the handle is not `path` itself (a link or
/// junction on the way led elsewhere).
fn wipe_file(path: &Path) -> std::io::Result<bool> {
    use std::io::Write;
    use std::os::windows::{
        fs::{MetadataExt, OpenOptionsExt},
        io::AsRawHandle,
    };
    use windows::Win32::{
        Foundation::HANDLE,
        Storage::FileSystem::{
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_NAME_NORMALIZED, GetFinalPathNameByHandleW,
        },
    };
    let mut file = match std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    let metadata = file.metadata()?;
    if metadata.file_attributes() & 0x400 != 0 || !metadata.is_file() {
        return Err(std::io::Error::other(
            "it is not a plain file; nothing was changed",
        ));
    }
    let mut buffer = vec![0u16; 32768];
    // SAFETY: `file` stays open for the call and `buffer` is writable for its length.
    let length = unsafe {
        GetFinalPathNameByHandleW(
            HANDLE(file.as_raw_handle()),
            &mut buffer,
            FILE_NAME_NORMALIZED,
        )
    } as usize;
    if length == 0 || length > buffer.len() {
        return Err(std::io::Error::last_os_error());
    }
    let opened = String::from_utf16_lossy(&buffer[..length]);
    let opened = opened.strip_prefix(r"\\?\").unwrap_or(&opened);
    if !opened.eq_ignore_ascii_case(&path.display().to_string()) {
        return Err(std::io::Error::other(format!(
            "it opens as {opened}, through a link on the way; nothing was changed"
        )));
    }
    let zeros = [0u8; 64 * 1024];
    let mut left = metadata.len();
    while left > 0 {
        let chunk = left.min(zeros.len() as u64) as usize;
        file.write_all(&zeros[..chunk])?;
        left -= chunk as u64;
    }
    file.sync_all()?;
    drop(file);
    std::fs::remove_file(path)?;
    Ok(true)
}

/// Where Remote Desktop is switched (`fDenyTSConnections`), and the group
/// policy that wins over it (`butterpollo_core::child_session`'s
/// `TERMINAL_SERVER` and `TERMINAL_SERVICES_POLICY`).
const TERMINAL_SERVER: &str = r"SYSTEM\CurrentControlSet\Control\Terminal Server";
const TERMINAL_SERVICES_POLICY: &str = r"SOFTWARE\Policies\Microsoft\Windows NT\Terminal Services";
const DENY_TS_CONNECTIONS: &str = "fDenyTSConnections";
/// HKLM's DWORD values, all [`turn_rdp_off`] needs: [`NativeRegistry`] on
/// this PC, a fake in tests.
trait MachineRegistry {
    fn read_dword(&self, path: &str, name: &str) -> Result<Option<u32>>;
    fn write_dword(&mut self, path: &str, name: &str, value: u32) -> Result<()>;
}
struct NativeRegistry;
impl MachineRegistry for NativeRegistry {
    fn read_dword(&self, path: &str, name: &str) -> Result<Option<u32>> {
        system::machine_dword(path, name)
    }
    fn write_dword(&mut self, path: &str, name: &str, value: u32) -> Result<()> {
        system::write_key(
            HKEY_LOCAL_MACHINE,
            path,
            &[(name, system::Value::Number(value))],
        )
    }
}
/// Turn Remote Desktop off (`fDenyTSConnections = 1`), read it back, and
/// check that no group policy keeps it on, as the host does
/// (`butterpollo_core::child_session::turn_rdp_off`). What was done, for
/// the note.
fn turn_rdp_off(registry: &mut dyn MachineRegistry) -> String {
    let off = registry
        .write_dword(TERMINAL_SERVER, DENY_TS_CONNECTIONS, 1)
        .and_then(
            |()| match registry.read_dword(TERMINAL_SERVER, DENY_TS_CONNECTIONS)? {
                Some(1) => Ok(()),
                now => bail!("it reads {now:?} after writing 1"),
            },
        )
        .and_then(
            |()| match registry.read_dword(TERMINAL_SERVICES_POLICY, DENY_TS_CONNECTIONS)? {
                Some(0) => bail!(
                    "group policy keeps Remote Desktop on ({DENY_TS_CONNECTIONS} = 0 under HKLM\\{TERMINAL_SERVICES_POLICY}), which wins over this setting"
                ),
                _ => Ok(()),
            },
        );
    match off {
        Ok(()) => format!(
            "Remote Desktop was turned off ({DENY_TS_CONNECTIONS} = 1), so it is not open to the network"
        ),
        Err(error) => format!(
            "Turning Remote Desktop off failed ({DENY_TS_CONNECTIONS}: {error:#}): turn it off in Windows' settings (System, Remote Desktop) now"
        ),
    }
}
/// The note for a driver removal that failed: it exited with an error,
/// reported a `DRIVER_WARNING`, or did not run. `left` says what may remain.
fn removal_note(name: &str, left: &str, result: Result<(i32, String)>) -> Option<String> {
    let reason = match result {
        Ok((code, output)) if code != 0 || output.contains("DRIVER_WARNING") => {
            line(format!("warning: removing the {name} driver: {output}"));
            let after = |marker: &str| {
                output
                    .lines()
                    .find_map(|line| line.split_once(marker))
                    .map(|(_, reason)| reason.trim().to_owned())
                    .filter(|reason| !reason.is_empty())
            };
            after("driver action failed: ")
                .or_else(|| after("DRIVER_WARNING: "))
                .unwrap_or_else(|| format!("its script exited with code {code}"))
        }
        Ok(_) => return None,
        Err(error) => {
            line(format!("warning: removing the {name} driver: {error:#}"));
            format!("{error:#}")
        }
    };
    Some(format!(
        "The {name} driver could not be removed: {}. {left}: remove it in Device Manager; the setup log has the details.",
        reason.trim_end_matches('.')
    ))
}
/// The note for a factory reset that left `left` (from [`remove_profile`])
/// in `root`; None when it removed everything.
fn profile_note(root: &Path, left: &[PathBuf]) -> Option<String> {
    const LISTED: usize = 5;
    if left.is_empty() {
        return None;
    }
    let mut paths: Vec<String> = left
        .iter()
        .take(LISTED)
        .map(|path| path.display().to_string())
        .collect();
    if left.len() > LISTED {
        paths.push(format!("{} more", left.len() - LISTED));
    }
    Some(format!(
        "The factory reset could not delete everything in {}, so credentials, paired devices or their certificates may remain on this PC. Still there: {}. Close the programs using them and delete that folder.",
        root.display(),
        paths.join(", ")
    ))
}
/// Delete `root` for a factory reset, all but `keep` (files inside it).
/// Every step is tried; what is still there afterwards is returned.
fn remove_profile(root: &Path, keep: &[PathBuf]) -> Vec<PathBuf> {
    let keep: Vec<&PathBuf> = keep
        .iter()
        .filter(|keep| keep.starts_with(root) && keep.is_file())
        .collect();
    remove_profile_entries(root, &keep);
    let mut left = Vec::new();
    leftovers(root, &keep, &mut left);
    left
}
/// One file that cannot be deleted (open in an indexer or antivirus) stops
/// `remove_dir_all`; the rest of that folder is then deleted one by one.
fn remove_profile_entries(root: &Path, keep: &[&PathBuf]) {
    if keep.is_empty() && std::fs::remove_dir_all(root).is_ok() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if keep.contains(&&path) {
            continue;
        }
        let inside: Vec<&PathBuf> = keep
            .iter()
            .copied()
            .filter(|keep| keep.starts_with(&path))
            .collect();
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            remove_profile_entries(&path, &inside);
        } else {
            // A link to a folder goes with remove_dir on Windows.
            let _ = std::fs::remove_file(&path).or_else(|_| std::fs::remove_dir(&path));
        }
    }
    if keep.is_empty() {
        let _ = std::fs::remove_dir(root);
    }
}
/// What is still in `root` but `keep`: files, links, and folders that could
/// not be read; folders only through what is in them. Links are not followed.
fn leftovers(root: &Path, keep: &[&PathBuf], left: &mut Vec<PathBuf>) {
    match root.symlink_metadata() {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Ok(metadata) if metadata.is_dir() => {}
        _ => {
            left.push(root.to_path_buf());
            return;
        }
    }
    let Ok(entries) = std::fs::read_dir(root) else {
        left.push(root.to_path_buf());
        return;
    };
    for entry in entries {
        let Ok(entry) = entry else {
            left.push(root.to_path_buf());
            return;
        };
        let path = entry.path();
        if keep.contains(&&path) {
            continue;
        }
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            leftovers(&path, keep, left);
        } else {
            left.push(path);
        }
    }
}
/// The HDR Vulkan layer that the host registers from `install` while it runs.
pub(crate) fn remove_layers(install: &Path) {
    for value in system::values(HKEY_LOCAL_MACHINE, LAYERS, KEY_WOW64_64KEY) {
        if layer_in(&value, install) {
            system::delete_value(HKEY_LOCAL_MACHINE, LAYERS, &value, KEY_WOW64_64KEY);
        }
    }
    remove_stale_layers();
}
const LAYERS: &str = "SOFTWARE\\Khronos\\Vulkan\\ImplicitLayers";
const LAYER_MANIFEST: &str = "VkLayer_butterpollo_hdr.json";
/// Earlier install folders' HDR layers whose manifest is gone (the
/// ButterpolloRust folder before the rename): every Vulkan program would
/// still look for them.
pub(crate) fn remove_stale_layers() {
    for value in system::values(HKEY_LOCAL_MACHINE, LAYERS, KEY_WOW64_64KEY) {
        if stale_layer(&value, |path| path.exists()) {
            system::delete_value(HKEY_LOCAL_MACHINE, LAYERS, &value, KEY_WOW64_64KEY);
        }
    }
}
/// A layer value as a plain path: the host registers the folder it runs
/// from, which is verbatim (`\\?\C:\...`) when the service started it so.
fn plain(value: &str) -> String {
    match value.strip_prefix(r"\\?\UNC\") {
        Some(share) => format!(r"\\{share}"),
        None => value.strip_prefix(r"\\?\").unwrap_or(value).to_owned(),
    }
}
fn layer_in(value: &str, install: &Path) -> bool {
    // With the separator, so removing ...\Butterpollo keeps ...\Butterpollo2's layer.
    let prefix = format!(
        "{}\\",
        plain(&install.display().to_string()).trim_end_matches('\\')
    )
    .to_ascii_lowercase();
    plain(value).to_ascii_lowercase().starts_with(&prefix)
}
fn stale_layer(value: &str, exists: impl Fn(&Path) -> bool) -> bool {
    let path = PathBuf::from(plain(value));
    path.file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case(LAYER_MANIFEST))
        && !exists(&path)
}
/// The folder Rubylight installed into before its rename. Once nothing runs
/// from it, what can remain is empty folders (files deleted at a restart
/// leave theirs behind); they go, anything else in it stays.
pub(crate) fn remove_earlier_folder(install: &Path) {
    let earlier = system::program_files().join("ButterpolloRust");
    if !plain(&install.display().to_string()).eq_ignore_ascii_case(&earlier.display().to_string())
        && !is_package(&earlier)
    {
        remove_empty_folders(&earlier);
    }
}
/// Delete `folder` and its subfolders, deepest first, as long as they are
/// empty; a folder holding any file stays, with the folders above it. Links
/// are not followed: an empty folder elsewhere is not ours.
fn remove_empty_folders(folder: &Path) {
    if !std::fs::symlink_metadata(folder).is_ok_and(|meta| meta.is_dir()) {
        return;
    }
    if let Ok(entries) = std::fs::read_dir(folder) {
        for entry in entries.flatten() {
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                remove_empty_folders(&entry.path());
            }
        }
    }
    let _ = std::fs::remove_dir(folder);
}
/// Delete what the installer put in `install`: the manifest's files, the
/// manifest, the uninstaller and driver folders carried over from Vibepollo.
/// Anything else stays, and so does every folder that is not empty then: the
/// install folder may also hold the user's own files. Returns the folders that
/// held a package file.
fn remove_package(install: &Path) -> BTreeSet<PathBuf> {
    let mut folders = BTreeSet::new();
    let mut remove = |path: PathBuf| {
        folders.extend(
            path.ancestors()
                .skip(1)
                .take_while(|folder| *folder != install && folder.starts_with(install))
                .map(Path::to_path_buf),
        );
        system::remove_file_later(&path);
    };
    if let Ok(entries) = payload::manifest(install) {
        for entry in entries {
            if let Ok(path) = payload::safe_join(install, &entry.path) {
                remove(path);
            }
        }
    }
    // Drivers carried over from Vibepollo are not in the manifest; only the
    // installer's own driver folders go, recognised by their script.
    for folder in ["drivers\\display", "drivers\\gamepad"] {
        let folder = install.join(folder);
        if folder.join("install.ps1").is_file()
            && let Ok(files) = files_in(&folder)
        {
            files.into_iter().for_each(&mut remove);
        }
    }
    remove(install.join("manifest.json"));
    remove(install.join("uninstall.exe"));
    // Deepest first, so a parent is empty by the time it comes up.
    for folder in folders.iter().rev() {
        let _ = std::fs::remove_dir(folder);
    }
    folders
}
/// Folders earlier versions shipped that are Rubylight's alone, with whatever
/// an update left in them: the Intel VPL (libvpl) licenses of 2.6.x.
const LEGACY_FOLDERS: [&str; 1] = ["licenses\\libvpl"];
/// What `remove_package` leaves of the install folder besides the user's own
/// files: the legacy folders no manifest lists any more (an update deletes the
/// files a new version stops shipping, not their folders, and 3.0 stopped
/// shipping libvpl's), setup's staging folder, and the folders that were `touched` (held a package
/// file) or hold a legacy folder, once they are empty. Other empty folders are
/// the user's, in a folder they may have chosen themselves (D:\Games), and stay,
/// as does everything when `install` is a drive root. Everything stays inside
/// `install`: a link is removed itself, never followed.
fn remove_leftover_folders(install: &Path, touched: &BTreeSet<PathBuf>) {
    if install.parent().is_none() {
        return;
    }
    let mut folders = touched.clone();
    for legacy in LEGACY_FOLDERS {
        // Every step must be a real folder: a junction at `licenses` would
        // lead the removal out of the install folder.
        let mut folder = install.to_path_buf();
        let real = Path::new(legacy).components().all(|part| {
            folder.push(part);
            std::fs::symlink_metadata(&folder).is_ok_and(|meta| meta.is_dir())
        });
        if real {
            let _ = std::fs::remove_dir_all(&folder);
            folders.extend(
                folder
                    .ancestors()
                    .take_while(|folder| *folder != install && folder.starts_with(install))
                    .map(Path::to_path_buf),
            );
        }
    }
    remove_staging(&install.join(STAGING), &mut system::remove_file_later);
    // Deepest first, so a parent is empty by the time it comes up.
    for folder in folders.iter().rev() {
        let _ = std::fs::remove_dir(folder);
    }
}
/// Setup's staging folder in the install folder: packages being installed,
/// the previous version a transaction keeps for its rollback, and the
/// updater's downloads. No manifest lists it.
const STAGING: &str = ".staging";
/// Delete the staging folder with everything in it: it is Rubylight's alone.
/// A link there is removed itself, never followed; a file of that name stays.
/// What can't go now (a file in use) is handed to `later`, as the package's
/// files are, and every failure is logged.
fn remove_staging(staging: &Path, later: &mut dyn FnMut(&Path)) {
    match std::fs::symlink_metadata(staging) {
        Ok(meta) if meta.file_type().is_symlink() => remove_link(staging),
        // remove_dir_all removes the links inside without following them.
        Ok(meta) if meta.is_dir() => {
            if let Err(error) = std::fs::remove_dir_all(staging) {
                line(format!(
                    "warning: {} could not be removed now: {error}; what is left goes at the next restart",
                    staging.display()
                ));
                remove_tree_later(staging, later);
            }
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => line(format!(
            "warning: {} could not be read: {error}",
            staging.display()
        )),
    }
}
/// Remove the junction or symbolic link `link` itself: on Windows a link to a
/// folder goes with remove_dir, one to a file with remove_file.
fn remove_link(link: &Path) {
    if let Err(error) = std::fs::remove_dir(link).or_else(|_| std::fs::remove_file(link)) {
        line(format!(
            "warning: the link {} could not be removed: {error}",
            link.display()
        ));
    }
}
/// Hand every file in `folder` to `later`, then each folder that isn't empty
/// by then, deepest first. A link is removed now or left: deleting it at a
/// restart might follow it.
fn remove_tree_later(folder: &Path, later: &mut dyn FnMut(&Path)) {
    match std::fs::read_dir(folder) {
        Ok(entries) => {
            for entry in entries {
                match entry.and_then(|entry| Ok((entry.path(), entry.file_type()?))) {
                    Ok((path, kind)) if kind.is_symlink() => remove_link(&path),
                    Ok((path, kind)) if kind.is_dir() => remove_tree_later(&path, later),
                    Ok((path, _)) => later(&path),
                    Err(error) => line(format!(
                        "warning: listing {} failed: {error}",
                        folder.display()
                    )),
                }
            }
        }
        Err(error) => line(format!(
            "warning: listing {} failed: {error}",
            folder.display()
        )),
    }
    if std::fs::remove_dir(folder).is_err() {
        later(folder);
    }
}
fn files_in(folder: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(folder)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            files.extend(files_in(&entry.path())?);
        } else {
            files.push(entry.path());
        }
    }
    Ok(files)
}

#[cfg(test)]
mod tests;
