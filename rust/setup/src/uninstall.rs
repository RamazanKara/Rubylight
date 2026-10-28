//! Removing Rubylight. Settings and drivers stay unless asked otherwise,
//! except Rubylight's own display driver.
use crate::{
    detect::{self, SERVICE, UNINSTALL},
    install::{
        FIREWALL_RULE, HOST_PROCESSES, LEGACY_FIREWALL_RULE, legacy_start_menu_link, profile,
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
/// Remove Rubylight; the notes are problems the user must see, though the
/// removal itself went through.
pub fn uninstall(options: &Options, progress: &Progress) -> Result<Removed> {
    let Some(install) = install_location() else {
        bail!("Rubylight is not installed on this PC");
    };
    line(format!("uninstalling from {}", install.display()));
    progress.set("Stopping Rubylight…");
    let _ = system::stop_service(SERVICE);
    system::kill(&HOST_PROCESSES[..4]);
    if system::service_program(SERVICE).is_some_and(|p| p.starts_with(&install)) {
        system::delete_service(SERVICE)?;
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
    if options.remove_drivers {
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
            (
                crate::install::LIBVIRTUALDISPLAY_REMOVAL,
                "old libvirtualdisplay",
                vec!["-InstallerBestEffort"],
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
    }

    progress.set("Removing files…");
    let _ = std::fs::remove_file(start_menu_link());
    let _ = std::fs::remove_file(legacy_start_menu_link());
    system::delete_key(HKEY_LOCAL_MACHINE, &format!("{UNINSTALL}\\Butterpollo"));
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
/// shipping libvpl's), and the folders that were `touched` (held a package
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
    // Deepest first, so a parent is empty by the time it comes up.
    for folder in folders.iter().rev() {
        let _ = std::fs::remove_dir(folder);
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
mod tests {
    use super::*;
    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    #[test]
    fn child_session_settings_are_restored_only_when_setup_recorded_them() {
        let install = tempfile::tempdir().unwrap();
        write(&install.path().join("butterpollo.exe"), "");
        let root = tempfile::tempdir().unwrap();
        let profile = root.path().join("config");
        assert_eq!(
            child_session_restore_pending(&profile, install.path()),
            None
        );
        // One in the profile, which its user can write, is ignored.
        write(&profile.join(CHILD_SESSION_SETUP_FILE), "{}");
        assert_eq!(
            child_session_restore_pending(&profile, install.path()),
            None
        );
        // Only the PC's record.
        let machine = child_session_record(&profile);
        assert_eq!(
            machine,
            root.path()
                .join("windows-settings")
                .join(CHILD_SESSION_SETUP_FILE)
        );
        write(&machine, "{}");
        assert_eq!(
            child_session_restore_pending(&profile, install.path()),
            Some(install.path().join("butterpollo.exe"))
        );
        std::fs::remove_file(&machine).unwrap();
        assert_eq!(
            child_session_restore_pending(&profile, install.path()),
            None
        );
        write(&machine, "{}");
        std::fs::remove_file(install.path().join("butterpollo.exe")).unwrap();
        assert_eq!(
            child_session_restore_pending(&profile, install.path()),
            None
        );
    }
    /// HKLM as the uninstaller sees it: RDP on, as child-session setup left it.
    #[derive(Default)]
    struct FakeRegistry {
        values: std::collections::BTreeMap<(String, String), u32>,
        /// Writes fail (access denied).
        denied: bool,
        /// Writes succeed without changing the value.
        stuck: bool,
        writes: usize,
    }
    impl FakeRegistry {
        fn rdp_on() -> Self {
            let mut registry = Self::default();
            registry.set(TERMINAL_SERVER, 0);
            registry
        }
        fn set(&mut self, path: &str, value: u32) {
            self.values
                .insert((path.into(), DENY_TS_CONNECTIONS.into()), value);
        }
        fn rdp(&self) -> Option<u32> {
            self.read_dword(TERMINAL_SERVER, DENY_TS_CONNECTIONS)
                .unwrap()
        }
    }
    impl MachineRegistry for FakeRegistry {
        fn read_dword(&self, path: &str, name: &str) -> Result<Option<u32>> {
            Ok(self.values.get(&(path.into(), name.into())).copied())
        }
        fn write_dword(&mut self, path: &str, name: &str, value: u32) -> Result<()> {
            self.writes += 1;
            if self.denied {
                bail!("access denied");
            }
            if !self.stuck {
                self.values.insert((path.into(), name.into()), value);
            }
            Ok(())
        }
    }
    /// A recorded setup with the host installed (`exe`) or not.
    fn recorded(exe: bool) -> (tempfile::TempDir, tempfile::TempDir, PathBuf) {
        let install = tempfile::tempdir().unwrap();
        if exe {
            write(&install.path().join("butterpollo.exe"), "");
        }
        let root = tempfile::tempdir().unwrap();
        let profile = root.path().join("config");
        write(&child_session_record(&profile), "{}");
        (install, root, profile)
    }
    #[test]
    fn a_restore_that_cannot_run_turns_remote_desktop_off_itself() {
        // The host is gone: nothing ran, so the uninstaller turns RDP off and reads it back.
        let (install, _root, profile) = recorded(false);
        let mut registry = FakeRegistry::rdp_on();
        let note = restore_child_session_with(
            &profile,
            install.path(),
            &mut registry,
            || panic!("a record is there"),
            |_| panic!("there is no host to run"),
        )
        .unwrap();
        assert_eq!(registry.rdp(), Some(1));
        assert!(
            note.contains("butterpollo.exe is missing")
                && note.contains("Remote Desktop was turned off (fDenyTSConnections = 1)"),
            "{note}"
        );
        // The host does not start, times out (killed) or crashes: the same.
        let (install, _root, profile) = recorded(true);
        type Outcome = fn() -> Result<(i32, String)>;
        let outcomes: [Outcome; 4] = [
            || bail!("starting butterpollo.exe: access denied"),
            || bail!("butterpollo.exe did not finish within 180 s"),
            || Ok((-1073741819, String::new())),
            // A restore that failed: the host turned RDP off already; again does no harm.
            || Ok((1, "were not restored: access denied.".into())),
        ];
        for outcome in outcomes {
            let mut registry = FakeRegistry::rdp_on();
            let note = restore_child_session_with(
                &profile,
                install.path(),
                &mut registry,
                || panic!("a record is there"),
                |program| {
                    assert_eq!(program, install.path().join("butterpollo.exe"));
                    outcome()
                },
            )
            .unwrap();
            assert_eq!(registry.rdp(), Some(1), "{note}");
            assert!(note.contains("Remote Desktop was turned off"), "{note}");
            assert!(!note.contains(".."), "{note}");
        }
        // Restored, or nothing to restore: RDP is left as the restore put it.
        let mut registry = FakeRegistry::rdp_on();
        assert_eq!(
            restore_child_session_with(
                &profile,
                install.path(),
                &mut registry,
                || panic!("a record is there"),
                |_| Ok((0, String::new()))
            ),
            None
        );
        let root = tempfile::tempdir().unwrap();
        assert_eq!(
            restore_child_session_with(
                &root.path().join("config"),
                install.path(),
                &mut registry,
                || Ok(false),
                |_| panic!("nothing to restore")
            ),
            None
        );
        assert_eq!((registry.rdp(), registry.writes), (Some(0), 0));
    }
    #[test]
    fn a_partly_restored_remote_desktop_is_left_as_the_user_had_it() {
        // The host put RDP back to the user's value (on), then failed on the rest: turned
        // off now, it would stay off, as a later restore skips RDP.
        let (install, _root, profile) = recorded(true);
        let mut registry = FakeRegistry::rdp_on();
        let note = restore_child_session_with(
            &profile,
            install.path(),
            &mut registry,
            || panic!("a record is there"),
            |_| {
                Ok((
                    CHILD_SESSION_RESTORE_PARTLY_EXIT,
                    "were partly restored: access denied.".into(),
                ))
            },
        )
        .unwrap();
        assert_eq!((registry.rdp(), registry.writes), (Some(0), 0), "{note}");
        assert!(
            note.contains("partly restored: access denied")
                && note.contains("Remote Desktop was left as it is")
                && !note.contains("turned off"),
            "{note}"
        );
        assert!(!note.contains(".."), "{note}");
    }
    #[test]
    fn remote_desktop_setups_block_rule_outlived_its_record_for_is_turned_off() {
        // No record, but setup's block rule is still there: RDP on is setup's, and nothing
        // restores it, so it goes off as the host's enforce_unrecorded turns it off.
        let install = tempfile::tempdir().unwrap();
        write(&install.path().join("butterpollo.exe"), "");
        let root = tempfile::tempdir().unwrap();
        let profile = root.path().join("config");
        let unrecorded = |registry: &mut FakeRegistry, block_rule: fn() -> Result<bool>| {
            restore_child_session_with(&profile, install.path(), registry, block_rule, |_| {
                panic!("there is no record to restore")
            })
        };
        let mut registry = FakeRegistry::rdp_on();
        let note = unrecorded(&mut registry, || Ok(true)).unwrap();
        assert_eq!(registry.rdp(), Some(1));
        assert!(
            note.contains("their record is gone")
                && note.contains(CHILD_SESSION_BLOCK_RULE)
                && note.contains("Remote Desktop was turned off (fDenyTSConnections = 1)"),
            "{note}"
        );
        // RDP is missing (on by default): the same.
        let mut registry = FakeRegistry::default();
        assert!(unrecorded(&mut registry, || Ok(true)).is_some());
        assert_eq!(registry.rdp(), Some(1));
        // Without the rule, RDP on is the user's own; with RDP off, nothing is open and the
        // rule is not even looked for. A rule that can't be read changes nothing.
        type BlockRule = fn() -> Result<bool>;
        let cases: [(FakeRegistry, BlockRule); 3] = [
            (FakeRegistry::rdp_on(), || Ok(false)),
            (FakeRegistry::rdp_on(), || {
                bail!("Windows Firewall is not running")
            }),
            (
                {
                    let mut off = FakeRegistry::default();
                    off.set(TERMINAL_SERVER, 1);
                    off
                },
                || panic!("RDP is off"),
            ),
        ];
        for (mut registry, block_rule) in cases {
            let before = registry.rdp();
            assert_eq!(unrecorded(&mut registry, block_rule), None);
            assert_eq!((registry.rdp(), registry.writes), (before, 0));
        }
    }
    #[test]
    fn a_firewall_rule_that_is_not_there_reads_as_missing() {
        // Read-only on this PC: no rule has this name.
        assert!(
            !system::firewall_rule_present("Rubylight test: no rule has this name (TCP)").unwrap()
        );
    }
    #[test]
    fn remote_desktop_that_stays_on_is_reported_loudly() {
        let (install, _root, profile) = recorded(false);
        let note = |registry: &mut FakeRegistry| {
            restore_child_session_with(
                &profile,
                install.path(),
                registry,
                || unreachable!(),
                |_| unreachable!(),
            )
            .unwrap()
        };
        let mut denied = FakeRegistry {
            denied: true,
            ..FakeRegistry::rdp_on()
        };
        let found = note(&mut denied);
        assert!(
            found.contains("Turning Remote Desktop off failed")
                && found.contains("access denied")
                && found.contains("turn it off in Windows' settings"),
            "{found}"
        );
        // Written but read back on.
        let mut stuck = FakeRegistry {
            stuck: true,
            ..FakeRegistry::rdp_on()
        };
        let found = note(&mut stuck);
        assert!(
            found.contains("Turning Remote Desktop off failed")
                && found.contains("it reads Some(0) after writing 1"),
            "{found}"
        );
        // Group policy wins over the value written.
        let mut policy = FakeRegistry::rdp_on();
        policy.set(TERMINAL_SERVICES_POLICY, 0);
        let found = note(&mut policy);
        assert_eq!(policy.rdp(), Some(1));
        assert!(
            found.contains("Turning Remote Desktop off failed") && found.contains("group policy"),
            "{found}"
        );
        // A policy that turns it off is no problem.
        let mut off = FakeRegistry::rdp_on();
        off.set(TERMINAL_SERVICES_POLICY, 1);
        assert!(note(&mut off).contains("Remote Desktop was turned off"));
    }
    #[test]
    fn the_uninstaller_switches_remote_desktop_where_the_host_does() {
        use butterpollo_core::child_session as host;
        assert_eq!(TERMINAL_SERVER, host::TERMINAL_SERVER);
        assert_eq!(TERMINAL_SERVICES_POLICY, host::TERMINAL_SERVICES_POLICY);
        // Read-only on this PC: the value is a DWORD or absent, and a missing one reads as None.
        let value = NativeRegistry
            .read_dword(TERMINAL_SERVER, DENY_TS_CONNECTIONS)
            .unwrap();
        assert!(matches!(value, None | Some(0) | Some(1)), "{value:?}");
        assert_eq!(
            NativeRegistry
                .read_dword(r"SOFTWARE\Rubylight\No\Such\Key", "Missing")
                .unwrap(),
            None
        );
        // A value that is not a DWORD is an error, not a missing one.
        assert!(
            NativeRegistry
                .read_dword(
                    r"SOFTWARE\Microsoft\Windows NT\CurrentVersion",
                    "ProductName"
                )
                .is_err()
        );
    }
    #[test]
    fn a_factory_reset_keeps_the_child_session_record_a_failed_restore_needs() {
        let root = tempfile::tempdir().unwrap();
        let profile = root.path().join("config");
        let records = vec![child_session_record(&profile)];
        let fill = || {
            for record in &records {
                write(record, "{}");
            }
            write(&profile.join("sunshine.conf"), "x");
            write(&profile.join("credentials").join("cacert.pem"), "x");
            write(&profile.join("logs").join("butterpollo.log"), "x");
            write(&root.path().join("state.json"), "x");
            write(&records[0].with_file_name("other.json"), "x");
            // Not a record: it goes with the profile.
            write(&profile.join(CHILD_SESSION_SETUP_FILE), "{}");
        };
        fill();
        assert!(remove_profile(root.path(), &records).is_empty());
        let mut kept = records.clone();
        kept.sort();
        let mut left = files_in(root.path()).unwrap();
        left.sort();
        assert_eq!(
            left, kept,
            "the records stay for a retry, everything else goes"
        );
        assert!(!profile.exists());

        // Restored, or nothing to restore: the whole profile goes.
        fill();
        assert!(remove_profile(root.path(), &[]).is_empty());
        assert!(!root.path().exists());
        assert_eq!(profile_note(root.path(), &[]), None);
    }
    #[test]
    fn a_factory_reset_that_leaves_files_says_which_and_that_pairings_may_remain() {
        use std::os::windows::fs::OpenOptionsExt;
        let root = tempfile::tempdir().unwrap();
        let profile = root.path().join("config");
        let records = [child_session_record(&profile)];
        let key = profile.join("credentials").join("cakey.pem");
        write(&key, "x");
        write(&profile.join("sunshine.conf"), "x");
        write(&records[0], "{}");
        // An indexer or antivirus holds the key open.
        let held = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&key)
            .unwrap();
        for keep in [&records[..], &[]] {
            let left = remove_profile(root.path(), keep);
            assert_eq!(left, std::slice::from_ref(&key));
            assert!(!profile.join("sunshine.conf").exists());
            let note = profile_note(root.path(), &left).unwrap();
            assert!(note.contains("credentials, paired devices"), "{note}");
            assert!(note.contains(&key.display().to_string()), "{note}");
            assert!(note.contains(&root.path().display().to_string()), "{note}");
        }
        drop(held);
        // Long lists are cut short.
        let many: Vec<PathBuf> = (0..7).map(|n| root.path().join(n.to_string())).collect();
        let note = profile_note(root.path(), &many).unwrap();
        assert!(note.contains("2 more"), "{note}");
        assert!(!note.contains(&many[5].display().to_string()), "{note}");
    }
    #[test]
    fn failed_driver_removals_become_notes() {
        let warning = "WARNING: [RubylightVirtualDisplay] DRIVER_WARNING: Rubylight display driver setup did not complete.\nWARNING: [RubylightVirtualDisplay] Installer best-effort driver action failed: pnputil /delete-driver oem9.inf /uninstall /force failed with exit code 5\n";
        let left = "Its device (Root\\RubylightVDisplay) or driver package may still be installed";
        assert_eq!(
            removal_note("Rubylight display", left, Ok((0, warning.into()))).unwrap(),
            "The Rubylight display driver could not be removed: pnputil /delete-driver oem9.inf /uninstall /force failed with exit code 5. Its device (Root\\RubylightVDisplay) or driver package may still be installed: remove it in Device Manager; the setup log has the details."
        );
        let note = removal_note(
            "virtual gamepad",
            "It may still be installed",
            Ok((1, String::new())),
        )
        .unwrap();
        assert!(note.contains("exited with code 1"), "{note}");
        let note = removal_note(
            "old libvirtualdisplay",
            "It may still be installed",
            Err(anyhow::anyhow!("the task did not start")),
        )
        .unwrap();
        assert!(note.contains("the task did not start"), "{note}");
        assert_eq!(
            removal_note(
                "virtual gamepad",
                "It may still be installed",
                Ok((0, "done\n".into()))
            ),
            None
        );
    }
    #[test]
    fn a_removal_whose_restore_failed_has_its_own_exit_code() {
        let removed = |unrestored| Removed {
            notes: Vec::new(),
            unrestored,
        };
        assert_eq!(removed(false).exit_code(), 0);
        let code = removed(true).exit_code();
        assert_eq!(code, UNRESTORED_EXIT);
        // Not done, failed, cancelled or done-but-restart.
        assert!(![0, 1, 1223, 3010].contains(&code));
    }
    #[test]
    fn child_session_restore_uses_the_hosts_record_and_flag() {
        assert_eq!(
            CHILD_SESSION_SETUP_FILE,
            butterpollo_core::child_session::SETUP_FILE
        );
        assert_eq!(
            CHILD_SESSION_SETUP_FOLDER,
            butterpollo_core::child_session::SETUP_FOLDER
        );
        // Beside the installed profile: the PC's record is the host's.
        assert_eq!(
            child_session_record(&profile()),
            butterpollo_core::child_session::setup_record()
        );
        assert_eq!(
            CHILD_SESSION_RESTORE_FLAG,
            butterpollo_core::child_session::RESTORE_FLAG
        );
        assert_eq!(
            CHILD_SESSION_BLOCK_RULE,
            butterpollo_core::child_session::BLOCK_RULE
        );
        assert_eq!(
            CHILD_SESSION_RESTORE_PARTLY_EXIT,
            butterpollo_core::child_session::RESTORE_PARTLY_EXIT
        );
    }
    fn package(root: &Path) {
        write(
            &root.join("manifest.json"),
            r#"[{"path":"butterpollo.exe","sha256":""},{"path":"assets/web/index.html","sha256":""}]"#,
        );
        write(&root.join("butterpollo.exe"), "exe");
        write(&root.join("assets\\web\\index.html"), "html");
        write(&root.join("uninstall.exe"), "stub");
        write(&root.join("drivers\\display\\install.ps1"), "script");
        write(&root.join("drivers\\display\\driver.inf"), "inf");
    }
    /// A package that also ships licenses\README.txt, as 3.0 does.
    fn licensed_package(root: &Path) {
        package(root);
        write(
            &root.join("manifest.json"),
            r#"[{"path":"butterpollo.exe","sha256":""},{"path":"assets/web/index.html","sha256":""},{"path":"licenses/README.txt","sha256":""}]"#,
        );
        write(&root.join("licenses\\README.txt"), "ours");
    }
    #[test]
    fn only_a_folder_holding_the_package_counts_as_an_installation() {
        let downloads = tempfile::tempdir().unwrap();
        write(&downloads.path().join("holiday.jpg"), "photo");
        assert!(!is_package(downloads.path()));
        write(
            &downloads.path().join("manifest.json"),
            r#"[{"path":"other.exe","sha256":""}]"#,
        );
        assert!(!is_package(downloads.path()));
        let install = tempfile::tempdir().unwrap();
        package(install.path());
        assert!(is_package(install.path()));
    }
    #[test]
    fn uninstalling_removes_the_package_but_keeps_the_users_own_files() {
        let install = tempfile::tempdir().unwrap();
        let root = install.path();
        package(root);
        write(&root.join("notes.txt"), "mine");
        write(&root.join("assets\\mine.png"), "mine");
        write(&root.join("drivers\\other\\tool.sys"), "mine");
        let profile_files = [
            "sunshine.conf",
            "sunshine_state.json",
            "sunshine_credentials.json",
            "apps.json",
            "vibeshine_state.json",
            "credentials/cacert.pem",
            "credentials/cakey.pem",
            "covers/game.png",
        ];
        for name in profile_files {
            write(&root.join("config").join(name), "user profile");
        }
        remove_package(root);
        for gone in [
            "butterpollo.exe",
            "assets\\web\\index.html",
            "assets\\web",
            "manifest.json",
            "uninstall.exe",
            "drivers\\display",
        ] {
            assert!(!root.join(gone).exists(), "{gone} remains");
        }
        for kept in ["notes.txt", "assets\\mine.png", "drivers\\other\\tool.sys"] {
            assert!(root.join(kept).exists(), "{kept} was deleted");
        }
        for name in profile_files {
            assert_eq!(
                std::fs::read(root.join("config").join(name)).unwrap(),
                b"user profile"
            );
        }
    }
    #[test]
    fn the_hdr_layer_the_host_registered_verbatim_goes_with_its_folder() {
        let install = Path::new(r"C:\Program Files\ButterpolloRust");
        for registered in [
            r"\\?\C:\Program Files\ButterpolloRust\vulkan-layer\VkLayer_butterpollo_hdr.json",
            r"C:\Program Files\ButterpolloRust\vulkan-layer/VkLayer_butterpollo_hdr.json",
            r"c:\program files\butterpollorust\vulkan-layer\VkLayer_butterpollo_hdr.json",
        ] {
            assert!(layer_in(registered, install), "{registered} stays");
        }
        assert!(layer_in(
            r"\\?\UNC\server\share\Rubylight\vulkan-layer\VkLayer_butterpollo_hdr.json",
            Path::new(r"\\server\share\Rubylight")
        ));
        for other in [
            r"\\?\C:\Program Files\ButterpolloRust2\vulkan-layer\VkLayer_butterpollo_hdr.json",
            r"C:\Program Files\Rubylight\vulkan-layer/VkLayer_butterpollo_hdr.json",
            r"C:\games\Steam\SteamOverlayVulkanLayer64.json",
        ] {
            assert!(!layer_in(other, install), "{other} was removed");
        }
    }
    #[test]
    fn an_earlier_folders_hdr_layer_goes_once_its_manifest_is_gone() {
        let gone = |_: &Path| false;
        assert!(stale_layer(
            r"\\?\C:\Program Files\ButterpolloRust\vulkan-layer\VkLayer_butterpollo_hdr.json",
            gone
        ));
        assert!(!stale_layer(
            r"C:\Program Files\Rubylight\vulkan-layer/VkLayer_butterpollo_hdr.json",
            |_| true
        ));
        // Other programs' layers stay, even with their files gone.
        assert!(!stale_layer(
            r"C:\games\Steam\SteamOverlayVulkanLayer64.json",
            gone
        ));
    }
    #[test]
    fn an_earlier_folder_goes_when_only_empty_folders_remain() {
        let temp = tempfile::tempdir().unwrap();
        let earlier = temp.path().join("ButterpolloRust");
        std::fs::create_dir_all(earlier.join("drivers\\display\\vulkan-layer")).unwrap();
        std::fs::create_dir_all(earlier.join("drivers\\rubylight-display")).unwrap();
        remove_empty_folders(&earlier);
        assert!(!earlier.exists());
        let kept = temp.path().join("Kept");
        std::fs::create_dir_all(kept.join("drivers\\display\\vulkan-layer")).unwrap();
        write(&kept.join("drivers\\rubylight-display\\notes.txt"), "mine");
        remove_empty_folders(&kept);
        assert!(!kept.join("drivers\\display").exists());
        assert!(kept.join("drivers\\rubylight-display\\notes.txt").exists());
    }
    #[test]
    fn the_install_folder_goes_with_the_folders_an_earlier_version_left() {
        // 2.6.x shipped licenses\libvpl; an update to 3.0 deleted its files
        // and left the folders, so the uninstaller found them behind.
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("Rubylight");
        licensed_package(&root);
        std::fs::create_dir_all(root.join("licenses\\libvpl\\third-party\\empty")).unwrap();
        // A libvpl file an update could not delete is Rubylight's too.
        write(&root.join("licenses\\libvpl\\LICENSE"), "intel");
        write(&root.join("licenses\\libvpl\\x\\y.txt"), "intel");
        let touched = remove_package(&root);
        remove_leftover_folders(&root, &touched);
        assert!(!root.join("licenses").exists());
        // Nothing else is left, so the uninstaller's final rd removes it.
        std::fs::remove_dir(&root).unwrap();
    }
    #[test]
    fn empty_folders_of_the_user_survive_the_uninstall() {
        // The install folder may be one the user chose, with empty folders of
        // their own in it, and not every folder in it is one of ours.
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("Games");
        licensed_package(&root);
        std::fs::create_dir_all(root.join("licenses\\libvpl")).unwrap();
        std::fs::create_dir_all(root.join("licenses\\mine\\empty")).unwrap();
        std::fs::create_dir_all(root.join("saves\\empty")).unwrap();
        std::fs::create_dir_all(root.join("vulkan-layer")).unwrap();
        let touched = remove_package(&root);
        remove_leftover_folders(&root, &touched);
        assert!(!root.join("licenses\\libvpl").exists());
        assert!(root.join("licenses\\mine").is_dir());
        for kept in ["licenses\\mine\\empty", "saves\\empty", "vulkan-layer"] {
            assert!(root.join(kept).is_dir(), "{kept} was removed");
        }
        // A drive root is never swept.
        let drive = Path::new("C:\\");
        assert!(drive.parent().is_none());
        remove_leftover_folders(drive, &BTreeSet::from([PathBuf::from("C:\\Windows")]));
        assert!(drive.join("Windows").is_dir());
    }
    #[test]
    fn leftover_folders_keep_the_users_files_and_never_follow_a_link_out() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("Rubylight");
        package(&root);
        write(&root.join("notes.txt"), "mine");
        write(&root.join("licenses\\mine\\note.txt"), "mine");
        std::fs::create_dir_all(root.join("licenses\\libvpl\\deeper")).unwrap();
        // An empty tree and a file outside, reachable by a junction inside.
        let outside = temp.path().join("outside");
        write(&outside.join("libvpl\\keep.txt"), "not ours");
        std::fs::create_dir_all(outside.join("empty")).unwrap();
        let link = |at: &Path, to: &Path| {
            let made = std::process::Command::new("cmd")
                .args(["/c", "mklink", "/J"])
                .arg(at)
                .arg(to)
                .output()
                .unwrap();
            assert!(made.status.success(), "{made:?}");
        };
        link(&root.join("through"), &outside);
        let touched = remove_package(&root);
        remove_leftover_folders(&root, &touched);
        assert!(root.join("notes.txt").exists());
        assert!(root.join("licenses\\mine\\note.txt").exists());
        assert!(!root.join("licenses\\libvpl").exists());
        assert!(outside.join("libvpl\\keep.txt").exists());
        assert!(outside.join("empty").exists());
        // A junction named licenses, standing where the legacy folder's
        // parent is, leads nowhere either.
        let other = temp.path().join("Other");
        package(&other);
        link(&other.join("licenses"), &outside);
        let touched = remove_package(&other);
        remove_leftover_folders(&other, &touched);
        assert!(outside.join("libvpl\\keep.txt").exists());
        assert!(outside.join("empty").exists());
    }
    #[test]
    fn an_installation_of_only_the_package_leaves_nothing_behind() {
        let install = tempfile::tempdir().unwrap();
        let root = install.path().join("Butterpollo");
        package(&root);
        remove_package(&root);
        std::fs::remove_dir(&root).unwrap();
        assert!(!root.exists());
    }
    #[test]
    fn the_saved_child_session_passwords_go_without_a_record() {
        // No record anywhere: a restore can have removed it and left the password.
        let root = tempfile::tempdir().unwrap();
        let folder = |profile: &Path| {
            profile
                .join("AppData")
                .join("Local")
                .join(CHILD_SESSION_PASSWORD_FOLDER)
        };
        let profiles = [root.path().join("ramaz"), root.path().join("guest")];
        for profile in &profiles {
            for name in CHILD_SESSION_PASSWORD_FILES {
                write(&folder(profile).join(name), "DPAPI blob");
            }
        }
        write(&folder(&profiles[1]).join("other.txt"), "kept");
        let kept = root.path().join("kept");
        write(
            &folder(&kept).join(CHILD_SESSION_PASSWORD_FILES[0]),
            "DPAPI blob",
        );
        let held_path = folder(&profiles[0]).join(CHILD_SESSION_PASSWORD_FILES[0]);
        let held = {
            use std::os::windows::fs::OpenOptionsExt;
            std::fs::OpenOptions::new()
                .read(true)
                .share_mode(0)
                .open(&held_path)
                .unwrap()
        };
        let remove_empty = |folder: &Path| match std::fs::remove_dir(folder) {
            Err(error) if error.kind() != std::io::ErrorKind::DirectoryNotEmpty => Err(error),
            _ => Ok(()),
        };
        let (deleted, failed) = delete_child_session_passwords_in(
            &[
                profiles[0].clone(),
                profiles[1].clone(),
                PathBuf::from("relative"),
                root.path().join("x").join("..").join("kept"),
            ],
            // The console user's own AppData\Local, here the guest's again: once.
            &[profiles[1].join("AppData").join("Local")],
            wipe_file,
            remove_empty,
        );
        drop(held);
        // The held file is said with its path; every other one goes.
        assert_eq!(deleted.len(), 3, "{deleted:?}");
        assert_eq!(failed.len(), 1, "{failed:?}");
        assert!(
            failed[0].contains(&held_path.display().to_string())
                && failed[0].contains("delete it by hand"),
            "{failed:?}"
        );
        assert!(
            !folder(&profiles[0])
                .join(CHILD_SESSION_PASSWORD_FILES[1])
                .exists()
        );
        // The guest's folder holds something else, and stays; the skipped one is untouched.
        assert_eq!(std::fs::read_dir(folder(&profiles[1])).unwrap().count(), 1);
        assert!(
            folder(&kept)
                .join(CHILD_SESSION_PASSWORD_FILES[0])
                .is_file()
        );
        // Run again: the held one goes now, then its emptied folder; nothing else is said.
        let (deleted, failed) =
            delete_child_session_passwords_in(&profiles, &[], wipe_file, remove_empty);
        assert_eq!((deleted, failed), (vec![held_path], Vec::<String>::new()));
        assert!(!folder(&profiles[0]).exists());
        assert_eq!(
            expand_environment(r"%SystemDrive%\Users\ramaz"),
            format!(r"{}\Users\ramaz", std::env::var("SystemDrive").unwrap())
        );
        assert_eq!(expand_environment(r"C:\100%\%NOPE_X%"), r"C:\100%\%NOPE_X%");
    }
}
