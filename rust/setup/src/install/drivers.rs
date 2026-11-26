//! Virtual display, gamepad and USBip driver setup; their repair is in
//! `driver_repair`.
use super::profile;
use crate::{detect, log::line, system, ui::Progress};
use std::{
    path::Path,
    time::{Duration, Instant},
};

/// Set up the drivers with the scripts in `install`. Setup, an in-app update
/// and the service's repair take turns: two driver scripts at once would
/// stop and restart the service and the device under each other.
pub(crate) fn install_drivers(
    install: &Path,
    display: bool,
    gamepad: bool,
    usbip: bool,
    progress: &Progress,
    notes: &mut Vec<String>,
) -> bool {
    let _turn = driver_lock(Duration::from_secs(900));
    run_drivers(install, display, gamepad, usbip, progress, notes)
}
pub(super) fn run_drivers(
    install: &Path,
    display: bool,
    gamepad: bool,
    usbip: bool,
    progress: &Progress,
    notes: &mut Vec<String>,
) -> bool {
    let mut restart_needed = false;
    if display {
        restart_needed |= display_step(install, progress, notes).0;
    }
    if gamepad {
        restart_needed |= gamepad_step(install, progress, notes).0;
    }

    if usbip {
        let script = install.join("drivers\\usbip\\install.ps1");
        if script.is_file() {
            progress.set("Installing Steam Deck and DualSense USB audio and haptics…");
            restart_needed |=
                run_driver_script(install, &script, &[], notes, "USB audio and haptics");
        } else {
            notes.push("The USB audio and haptics package is missing. Run the complete Rubylight installer to add it.".into());
        }
    }
    restart_needed
}

/// Rubylight's virtual gamepad driver, then the removal of the earlier virtual
/// gamepad driver it replaces. Returns whether Windows must restart and how
/// many notes the driver's own step added.
pub(super) fn gamepad_step(
    install: &Path,
    progress: &Progress,
    notes: &mut Vec<String>,
) -> (bool, usize) {
    let before = notes.len();
    let mut restart_needed = rubylight_gamepad_driver(install, progress, notes);
    let gamepad_notes = notes.len() - before;
    // Rubylight's own driver uses Root\RubylightVirtualGamepad cleanly alongside
    // another host. Remove the legacy virtual gamepad driver only when not
    // installing side by side and the new driver started cleanly without reboot.
    if libvirtualdisplay_replaced(detect::side_by_side(), gamepad_notes > 0, restart_needed) {
        restart_needed |= remove_libvirtualgamepad(progress, notes);
    }
    (restart_needed, gamepad_notes)
}

/// The installer of Rubylight's own virtual gamepad driver in a package.
pub(crate) const RUBYLIGHT_GAMEPAD_SCRIPT: &str = "drivers\\rubylight-gamepad\\install-local.ps1";
/// The earlier gamepad driver's name in notes about removal.
pub(crate) const LIBVIRTUALGAMEPAD: &str = "earlier virtual gamepad";

pub(super) fn rubylight_gamepad_driver(
    install: &Path,
    progress: &Progress,
    notes: &mut Vec<String>,
) -> bool {
    let script = install.join(RUBYLIGHT_GAMEPAD_SCRIPT);
    if !script.is_file() {
        notes.push("The virtual gamepad driver is missing from this package. Run the complete Rubylight installer to add it.".into());
        return false;
    }
    progress.set("Installing the virtual gamepad driver…");
    run_driver_script(
        install,
        &script,
        &["-InstallerBestEffort"],
        notes,
        "virtual gamepad",
    )
}

pub(super) fn remove_libvirtualgamepad(progress: &Progress, notes: &mut Vec<String>) -> bool {
    progress.set("Removing the earlier virtual gamepad driver…");
    let removal = crate::libvirtualgamepad::remove(&mut crate::libvirtualdisplay::Native);
    notes.extend(driver_removal_note(&removal, LIBVIRTUALGAMEPAD));
    removal.restart
}

/// Rubylight's display driver, then the removal of libvirtualdisplay it
/// replaces. Returns whether Windows must restart and how many notes the
/// driver's own step added (the removal's come after them).
pub(super) fn display_step(
    install: &Path,
    progress: &Progress,
    notes: &mut Vec<String>,
) -> (bool, usize) {
    let before = notes.len();
    let mut restart_needed = rubylight_display_driver(install, progress, notes);
    let display_notes = notes.len() - before;
    // Rubylight's own driver replaced libvirtualdisplay. The old one goes
    // once the new device runs; where Windows refused the new driver or
    // its install failed, nothing uses the old one and it stays. A host kept
    // side by side may still use it, in any run: install, update or repair.
    if libvirtualdisplay_replaced(detect::side_by_side(), display_notes > 0, restart_needed) {
        restart_needed |= remove_libvirtualdisplay(progress, notes);
    }
    (restart_needed, display_notes)
}
/// The installer of Rubylight's own display driver in a package.
pub(crate) const RUBYLIGHT_DISPLAY_SCRIPT: &str = "drivers\\rubylight-display\\install-local.ps1";
/// The driver's name in notes about that removal.
pub(crate) const LIBVIRTUALDISPLAY: &str = "old libvirtualdisplay";
/// Set up Rubylight's display driver (again, which also updates it). Returns
/// whether Windows needs a restart.
pub(super) fn rubylight_display_driver(
    install: &Path,
    progress: &Progress,
    notes: &mut Vec<String>,
) -> bool {
    let script = install.join(RUBYLIGHT_DISPLAY_SCRIPT);
    if !script.is_file() {
        notes.push("The virtual display driver is missing from this package. Run the complete Rubylight installer to add it.".into());
        return false;
    }
    progress.set("Installing the virtual display driver…");
    run_driver_script(
        install,
        &script,
        &["-InstallerBestEffort"],
        notes,
        "virtual display",
    )
}
/// Whether Rubylight's display driver step left a running device: no note
/// (a refusing policy, a failed install, a missing package) and no restart
/// still pending before the device starts. Side by side it replaces nothing.
pub(super) fn libvirtualdisplay_replaced(
    side_by_side: bool,
    noted: bool,
    restart_pending: bool,
) -> bool {
    !side_by_side && !noted && !restart_pending
}
/// Remove libvirtualdisplay's device, driver packages and HDR layer, best
/// effort. Returns whether Windows needs a restart.
pub(super) fn remove_libvirtualdisplay(progress: &Progress, notes: &mut Vec<String>) -> bool {
    progress.set("Removing the old virtual display driver…");
    let removal = crate::libvirtualdisplay::remove(&mut crate::libvirtualdisplay::Native);
    notes.extend(libvirtualdisplay_note(&removal));
    removal.restart
}
/// The note for a removal that failed: setup tries again on its next run.
pub(super) fn libvirtualdisplay_note(
    removal: &crate::libvirtualdisplay::Removal,
) -> Option<String> {
    driver_removal_note(removal, LIBVIRTUALDISPLAY)
}
pub(super) fn driver_removal_note(
    removal: &crate::libvirtualdisplay::Removal,
    name: &str,
) -> Option<String> {
    let error = removal.error.as_ref()?;
    line(format!("warning: removing the {name} driver: {error:#}"));
    Some(format!(
        "The {name} driver could not be removed: {error:#} Setup tries again when it next runs; the setup log has the details."
    ))
}
/// Run a Vibepollo driver script as SYSTEM. A failure is reported, not
/// fatal: the host still starts without the driver. Returns whether Windows
/// needs a restart.
pub(super) fn run_driver_script(
    install: &Path,
    script: &Path,
    args: &[&str],
    notes: &mut Vec<String>,
    name: &str,
) -> bool {
    let script = script.display().to_string();
    let mut arguments = vec![
        "-NoLogo",
        "-NonInteractive",
        "-NoProfile",
        "-ExecutionPolicy",
        "Bypass",
        "-File",
        script.as_str(),
    ];
    arguments.extend_from_slice(args);
    let result = system::run_as_system(
        install,
        &format!(
            "{}\\System32\\WindowsPowerShell\\v1.0\\powershell.exe",
            std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into())
        ),
        &arguments,
        Duration::from_secs(600),
    );
    let (code, output) = match result {
        Ok(result) => result,
        Err(error) => {
            line(format!("warning: {error:#}"));
            notes.push(driver_error_note(&error, name));
            return false;
        }
    };
    driver_result(code, &output, notes, name)
}
/// The note for a driver script that gave no result. One still running at
/// the deadline goes on in the background and may yet succeed, so it is not
/// reported as failed.
pub(super) fn driver_error_note(error: &anyhow::Error, name: &str) -> String {
    match error.downcast_ref::<system::StillRunning>() {
        Some(running) => format!(
            "The {name} driver was still being set up after {} s and goes on in the background. If it does not work once that finishes, run setup again.",
            running.seconds
        ),
        None => format!("The {name} driver could not be set up: {error:#}"),
    }
}
pub(super) fn driver_result(code: i32, output: &str, notes: &mut Vec<String>, name: &str) -> bool {
    // This PC refuses the driver as packaged (Smart App Control, S mode, an
    // App Control policy): the script says why and what fixes it, and
    // retrying cannot.
    if let Some(reason) = output
        .lines()
        .find_map(|line| line.split_once("DRIVER_UNSUPPORTED: "))
        .map(|(_, reason)| reason.trim())
        .filter(|reason| !reason.is_empty())
    {
        notes.push(format!("The {name} driver was not installed: {reason}"));
    } else if output.contains("DRIVER_WARNING") || !matches!(code, 0 | 3010) {
        // The scripts name what failed: a blocked certificate or catalog, a
        // tool an antivirus stopped, a DriverStore that kept an old copy.
        let after = |marker: &str| {
            output
                .lines()
                .find_map(|line| line.split_once(marker))
                .map(|(_, reason)| reason.trim())
                .filter(|reason| !reason.is_empty())
        };
        let reason = after("driver action failed: ").or_else(|| after("DRIVER_WARNING: "));
        let retry = if name == "USB audio and haptics" {
            "Run setup again with USB audio and haptics selected; the setup log has the details."
        } else {
            // The service looks for the display and gamepad drivers every
            // minute.
            "The Rubylight service tries again within a day, or run \"butterpollo-setup.exe --repair-drivers --now\" in an administrator shell; the setup log has the details."
        };
        notes.push(match reason {
            Some(reason) => format!("The {name} driver could not be set up: {reason} {retry}"),
            None => format!("The {name} driver reported a problem; see the setup log."),
        });
    }
    code == 3010 || output.contains("RESTART_REQUIRED") || output.contains("A reboot is required")
}
/// The one driver setup at a time, waited for up to `wait`. Without it the
/// driver scripts run anyway: a stale lock must not keep drivers out.
pub(super) fn driver_lock(wait: Duration) -> Option<std::fs::File> {
    use std::os::windows::fs::OpenOptionsExt;
    let profile = profile();
    let _ = std::fs::create_dir_all(&profile);
    let deadline = Instant::now() + wait;
    loop {
        match std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .share_mode(0)
            .open(profile.join("driver-setup.lock"))
        {
            Ok(file) => return Some(file),
            Err(error) if Instant::now() >= deadline => {
                line(format!("another setup is installing drivers ({error})"));
                return None;
            }
            Err(_) => std::thread::sleep(Duration::from_secs(1)),
        }
    }
}
