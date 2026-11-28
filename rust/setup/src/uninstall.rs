//! Removing Rubylight. Settings and drivers stay unless asked otherwise,
//! except Rubylight's own display driver.
mod child_session;
mod files;

use child_session::{child_session_record, delete_child_session_passwords, restore_child_session};
use files::{profile_note, remove_leftover_folders, remove_package_with, remove_profile};
pub(crate) use files::{remove_earlier_folder, remove_layers, remove_stale_layers};

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
    path::{Path, PathBuf},
    time::Duration,
};
use windows::Win32::System::Registry::HKEY_LOCAL_MACHINE;

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

/// Whether `folder` holds an installed Rubylight package.
fn is_package(folder: &Path) -> bool {
    payload::manifest(folder).is_ok_and(|entries| {
        entries
            .iter()
            .any(|entry| entry.path.eq_ignore_ascii_case("butterpollo.exe"))
    })
}
/// Why --remove-drivers leaves these drivers in a side-by-side installation.
const SHARED_DRIVERS_KEPT: &str = "The earlier virtual gamepad driver and libvirtualdisplay stay installed, as another host on this PC may use them. Rubylight's own display and virtual gamepad drivers were removed.";
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
fn inside_canonical(path: &Path, folder: &Path) -> bool {
    if let (Ok(path), Ok(folder)) = (path.canonicalize(), folder.canonicalize()) {
        return inside(&path, &folder);
    }
    inside(path, folder)
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
    // Rubylight's own display and virtual gamepad drivers serve only Rubylight,
    // and the certificates they trusted must not outlive it: both go even when
    // the other drivers stay.
    for (script_rel, progress_label, name, device_id) in [
        (
            crate::install::RUBYLIGHT_DISPLAY_SCRIPT,
            "Removing the Rubylight display driver…",
            "Rubylight display",
            "Root\\RubylightVDisplay",
        ),
        (
            crate::install::RUBYLIGHT_GAMEPAD_SCRIPT,
            "Removing the Rubylight virtual gamepad driver…",
            "Rubylight virtual gamepad",
            "Root\\RubylightVirtualGamepad",
        ),
    ] {
        let script = install.join(script_rel);
        if script.is_file() {
            progress.set(progress_label);
            let path = script.display().to_string();
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
            let result =
                system::run_as_system(&install, &powershell, &arguments, Duration::from_secs(600));
            let fallback =
                format!("Its device ({device_id}) or driver package may still be installed");
            notes.extend(removal_note(name, &fallback, result));
        }
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
                let result = system::run_as_system(
                    &install,
                    &powershell,
                    &arguments,
                    Duration::from_secs(600),
                );
                notes.extend(removal_note(name, "It may still be installed", result));
            }
        }
        // The libvirtualdisplay and earlier virtual gamepad drivers; with
        // nothing left these change nothing.
        let removal = crate::libvirtualdisplay::remove(&mut crate::libvirtualdisplay::Native);
        if let Some(error) = removal.error {
            notes.extend(removal_note(
                crate::install::LIBVIRTUALDISPLAY,
                "It may still be installed",
                Err(error),
            ));
        }
        let removal = crate::libvirtualgamepad::remove(&mut crate::libvirtualdisplay::Native);
        if let Some(error) = removal.error {
            notes.extend(removal_note(
                crate::install::LIBVIRTUALGAMEPAD,
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
    let self_exe = std::env::current_exe()
        .ok()
        .filter(|exe| inside_canonical(exe, &install));
    let touched = remove_package_with(
        &install,
        self_exe.as_deref(),
        &mut system::remove_file_later,
    );
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
    // /s refuses a folder that is not empty). Skip queuing it for reboot
    // deletion above so an immediate reinstall does not lose its uninstall.exe.
    if let Some(exe) = self_exe {
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

#[cfg(test)]
use child_session::*;
#[cfg(test)]
use files::*;

#[cfg(test)]
mod tests;
