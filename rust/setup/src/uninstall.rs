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
    // Child-session setup (Experimental) turned on Remote Desktop, child
    // sessions and firewall rules; the host that recorded them puts them back
    // while it is still installed. When they are not restored (the restore
    // failed, or could not run: the host missing, not starting, timing out
    // or crashing), this uninstaller turns Remote Desktop off itself and
    // reads it back, and the record stays, even through a factory reset:
    // Rubylight installed again restores the rest (Restore Windows
    // settings). The uninstaller then exits with UNRESTORED_EXIT.
    let mut notes = Vec::new();
    let unrestored = restore_child_session(&install, progress);
    if let Some(problem) = &unrestored {
        line(format!("warning: {problem}"));
        notes.push(problem.clone());
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
        match system::run_as_system(&powershell, &arguments, Duration::from_secs(600)) {
            Ok((code, output)) if code != 0 || output.contains("DRIVER_WARNING") => line(format!(
                "warning: removing the Rubylight display driver: {output}"
            )),
            Ok(_) => {}
            Err(error) => line(format!(
                "warning: removing the Rubylight display driver: {error:#}"
            )),
        }
    }
    if options.remove_drivers {
        progress.set("Removing the virtual drivers…");
        for (script, args) in [
            (
                "drivers\\gamepad\\cleanup.ps1",
                vec!["-InstallerBestEffort", "-RemoveDriverStorePackage:1"],
            ),
            // Left by an installation from before Rubylight's own driver.
            ("drivers\\display\\install.ps1", vec!["-Uninstall"]),
            (
                crate::install::LIBVIRTUALDISPLAY_REMOVAL,
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
                let _ = system::run_as_system(&powershell, &arguments, Duration::from_secs(600));
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
        remove_profile(profile().parent().unwrap_or(&profile()), &records);
        if !records.is_empty() {
            let kept = format!(
                "The factory reset kept {} so the Windows settings can still be restored: install Rubylight again and choose Restore Windows settings under Maintenance in the console.",
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
/// Put back the Windows settings child-session setup changed, through the
/// installed host; the reason when they were not restored and its record
/// is still needed, with what was done about Remote Desktop. None when there
/// was nothing to restore or it worked.
fn restore_child_session(install: &Path, progress: &Progress) -> Option<String> {
    restore_child_session_with(
        &crate::install::profile(),
        install,
        &mut NativeRegistry,
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
/// or crashed did not turn it off itself.
fn restore_child_session_with(
    profile: &Path,
    install: &Path,
    registry: &mut dyn MachineRegistry,
    run: impl FnOnce(&Path) -> Result<(i32, String)>,
) -> Option<String> {
    let problem = match child_session_restore_pending(profile, install) {
        None if !child_session_recorded(profile) => return None,
        None => format!("{} is missing", install.join("butterpollo.exe").display()),
        Some(program) => match run(&program) {
            Ok((0, _)) => return None,
            Ok((code, output)) => format!("({code}) {}", output.trim()),
            Err(error) => format!("{error:#}"),
        },
    };
    Some(format!(
        "The Windows settings child sessions changed (Remote Desktop, its firewall rules, child sessions) were not restored: {}. {}.",
        problem.trim().trim_end_matches('.'),
        turn_rdp_off(registry)
    ))
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
/// Delete `root` for a factory reset, all but `keep` (files inside it). Best
/// effort, like the rest of the removal.
fn remove_profile(root: &Path, keep: &[PathBuf]) {
    let keep: Vec<&PathBuf> = keep
        .iter()
        .filter(|keep| keep.starts_with(root) && keep.is_file())
        .collect();
    if keep.is_empty() {
        let _ = std::fs::remove_dir_all(root);
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
        if keep.iter().any(|keep| keep.starts_with(&path)) {
            let inside: Vec<PathBuf> = keep.iter().map(|keep| keep.to_path_buf()).collect();
            remove_profile(&path, &inside);
        } else if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            let _ = std::fs::remove_dir_all(&path);
        } else {
            let _ = std::fs::remove_file(&path);
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
        let note = restore_child_session_with(&profile, install.path(), &mut registry, |_| {
            panic!("there is no host to run")
        })
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
            let note =
                restore_child_session_with(&profile, install.path(), &mut registry, |program| {
                    assert_eq!(program, install.path().join("butterpollo.exe"));
                    outcome()
                })
                .unwrap();
            assert_eq!(registry.rdp(), Some(1), "{note}");
            assert!(note.contains("Remote Desktop was turned off"), "{note}");
            assert!(!note.contains(".."), "{note}");
        }
        // Restored, or nothing to restore: RDP is left as the restore put it.
        let mut registry = FakeRegistry::rdp_on();
        assert_eq!(
            restore_child_session_with(&profile, install.path(), &mut registry, |_| Ok((
                0,
                String::new()
            ))),
            None
        );
        let root = tempfile::tempdir().unwrap();
        assert_eq!(
            restore_child_session_with(
                &root.path().join("config"),
                install.path(),
                &mut registry,
                |_| panic!("nothing to restore")
            ),
            None
        );
        assert_eq!((registry.rdp(), registry.writes), (Some(0), 0));
    }
    #[test]
    fn remote_desktop_that_stays_on_is_reported_loudly() {
        let (install, _root, profile) = recorded(false);
        let note = |registry: &mut FakeRegistry| {
            restore_child_session_with(&profile, install.path(), registry, |_| unreachable!())
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
        remove_profile(root.path(), &records);
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
        remove_profile(root.path(), &[]);
        assert!(!root.path().exists());
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
}
