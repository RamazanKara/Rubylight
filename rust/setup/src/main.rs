#![windows_subsystem = "windows"]
//! Rubylight setup. Installs or updates Rubylight, replacing an installed
//! Vibepollo, Rubylight C++, Apollo or Sunshine in place (settings, paired
//! devices, apps and drivers are kept), and removes Rubylight again.
//!
//! rubylight-setup.exe [--quiet] [--install-dir <folder>] [--no-gamepad-driver]
//!                       [--no-display-driver] [--usbip-driver | --no-usbip-driver]
//!                       [--no-start] [--end-streams]
//! rubylight-setup.exe --uninstall [--quiet] [--factory-reset] [--remove-drivers]
//! rubylight-setup.exe --repair-drivers (the service runs this one)
#![warn(clippy::undocumented_unsafe_blocks)]

mod detect;
mod floor;
// Setup uses the rules, not the release-notes marker.
#[allow(dead_code)]
#[path = "../../core/src/floor.rs"]
mod floor_rules;
mod install;
mod log;
mod payload;
#[cfg(test)]
mod profile_tests;
mod system;
mod ui;
mod uninstall;
mod update;
#[path = "../../core/src/update_files.rs"]
mod update_files;
mod usbip;
#[path = "../../core/src/version.rs"]
mod version;

use anyhow::Context;
use std::path::PathBuf;

const TITLE: &str = "Rubylight setup";

#[derive(Default)]
struct Arguments {
    quiet: bool,
    update: bool,
    uninstall: bool,
    help: bool,
    install_dir: Option<PathBuf>,
    no_gamepad_driver: bool,
    no_display_driver: bool,
    usbip_driver: Option<bool>,
    no_start: bool,
    /// Install even while the host streams, ending the stream. Without it a
    /// quiet setup refuses; the setup window asks instead.
    end_streams: bool,
    factory_reset: bool,
    remove_drivers: bool,
    /// The service found no virtual display driver: set it up again.
    repair_drivers: bool,
    /// Diagnostics: write what setup found to this file and exit.
    detect: Option<PathBuf>,
    /// Diagnostics: unpack and verify the package into this folder and exit.
    extract: Option<PathBuf>,
}
fn arguments() -> Result<Arguments, String> {
    let mut parsed = Arguments::default();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.to_ascii_lowercase().as_str() {
            "--quiet" | "/quiet" | "/s" | "/qn" => parsed.quiet = true,
            "--uninstall" | "/uninstall" => parsed.uninstall = true,
            "--update" => parsed.update = true,
            "--help" | "-h" | "/?" => parsed.help = true,
            "--install-dir" => {
                parsed.install_dir = Some(PathBuf::from(
                    args.next().ok_or("--install-dir needs a folder")?,
                ))
            }
            "--no-gamepad-driver" => parsed.no_gamepad_driver = true,
            "--no-display-driver" => parsed.no_display_driver = true,
            "--usbip-driver" | "--no-usbip-driver" => {
                if parsed.usbip_driver.is_some() {
                    return Err("choose only one USB/IP driver option".into());
                }
                parsed.usbip_driver = Some(arg.eq_ignore_ascii_case("--usbip-driver"));
            }
            "--no-start" => parsed.no_start = true,
            "--end-streams" => parsed.end_streams = true,
            "--factory-reset" => parsed.factory_reset = true,
            "--remove-drivers" => parsed.remove_drivers = true,
            "--repair-drivers" => parsed.repair_drivers = true,
            "--detect" => {
                parsed.detect = Some(PathBuf::from(args.next().ok_or("--detect needs a file")?))
            }
            "--extract" => {
                parsed.extract = Some(PathBuf::from(
                    args.next().ok_or("--extract needs a folder")?,
                ))
            }
            other => return Err(format!("unknown option {other}")),
        }
    }
    Ok(parsed)
}
const USAGE: &str = "rubylight-setup.exe [--quiet] [--install-dir <folder>] [--no-gamepad-driver] [--no-display-driver] [--usbip-driver | --no-usbip-driver] [--no-start] [--end-streams]\n\
rubylight-setup.exe --uninstall [--quiet] [--factory-reset] [--remove-drivers]\n\n\
A quiet setup refuses to install while Rubylight is streaming; --end-streams ends the stream instead, and the client can reconnect once setup has finished.\n\n\
--usbip-driver installs the bundled Steam Deck and DualSense USB audio and haptics driver. It is optional, off by default, and its choice is kept for updates. --no-usbip-driver skips it without removing a shared driver.\n\n\
Exit codes: 0 done, 3010 done but Windows must restart, 1223 cancelled, 1 failed.";

fn main() {
    let args = match arguments() {
        Ok(args) => args,
        Err(error) => {
            ui::message_box(TITLE, &format!("{error}\n\n{USAGE}"));
            std::process::exit(87);
        }
    };
    if args.help {
        ui::message_box(TITLE, USAGE);
        return;
    }
    if let Some(file) = &args.detect {
        let found = detect::scan();
        let folder = detect::install_dir(&found, args.install_dir.clone());
        let report = format!(
            "install folder: {}\nprevious installation: {:?}\n\n{}\n\n{found:#?}\n",
            folder.display(),
            found.previous_root(),
            detect::summary(&found, &folder)
        );
        std::process::exit(i32::from(std::fs::write(file, report).is_err()));
    }
    if let Some(folder) = &args.extract {
        let result = (|| -> anyhow::Result<usize> {
            let mut payload = payload::Payload::open()?
                .ok_or_else(|| anyhow::anyhow!("this setup.exe carries no package"))?;
            payload.extract(folder)?;
            Ok(payload::verify(folder)?.len())
        })();
        std::process::exit(match result {
            Ok(_) => 0,
            Err(error) => {
                ui::message_box(TITLE, &format!("{error:#}"));
                1
            }
        });
    }
    if args.no_display_driver && !args.uninstall && !args.repair_drivers {
        // Before the UAC prompt: nothing setup could do would honour the option.
        std::process::exit(failed(
            args.quiet,
            "Rubylight cannot be installed without its display driver",
            &floor::display_driver_required(),
        ));
    }
    if !system::elevated() {
        std::process::exit(system::relaunch_elevated().unwrap_or(1));
    }
    if args.repair_drivers {
        // Run as SYSTEM, whose %TEMP% nobody looks in: log with the host.
        log::open_at(
            install::profile().join("logs").join("driver-repair.log"),
            true,
        );
    } else {
        log::open();
    }
    let code = if args.repair_drivers {
        match ui::progress(TITLE, "Repairing the drivers", true, |progress| {
            install::repair_drivers(&progress)
        }) {
            Ok(true) => 3010,
            Ok(false) => 0,
            Err(error) => failed(true, "The drivers could not be repaired", &error),
        }
    } else if args.update {
        match args.install_dir.as_ref() {
            Some(folder) if args.quiet && !args.uninstall => {
                let folder = folder.clone();
                let end_streams = args.end_streams;
                let usbip_driver = usbip::selected(&install::profile(), args.usbip_driver);
                match ui::progress(TITLE, "Updating Rubylight", true, move |progress| {
                    let mut notes = Vec::new();
                    let mut restart_needed = false;
                    let folder = system::win32_path(&folder)?;
                    // A 2.6.x install that left the display driver out cannot run 3.0.
                    if let Some(refusal) = floor::update_refusal(install::display_driver_declined())
                    {
                        return Err(refusal);
                    }
                    // Hosts from before the floor marker offer this release anyway.
                    let mut payload =
                        payload::Payload::open()?.context("this setup.exe carries no package")?;
                    floor::check(&mut payload)?;
                    drop(payload);
                    // As a reinstall does. In-app updates never set the
                    // drivers up, so hosts updated from the console since
                    // rc.22 had none.
                    update::run(&folder, true, end_streams, &progress, &mut || {
                        restart_needed |= install::install_drivers(
                            &folder,
                            true,
                            true,
                            usbip_driver,
                            &progress,
                            &mut notes,
                        );
                    })?;
                    usbip::remember(&install::profile(), usbip_driver)?;
                    // Hosts updated from the console keep the names Windows
                    // shows from their first install; bring them up to date.
                    if let Err(error) = install::refresh_service_name() {
                        notes.push(format!("The service name could not be updated: {error:#}"));
                    }
                    if let Err(error) = install::secure_install(&folder) {
                        notes.push(format!(
                            "The permissions of {} could not be restricted: {error:#}",
                            folder.display()
                        ));
                    }
                    install::refresh_entries(&folder, &mut notes);
                    for note in notes {
                        log::line(format!("note: {note}"));
                    }
                    anyhow::Ok(restart_needed)
                }) {
                    Ok(true) => 3010,
                    Ok(false) => 0,
                    Err(error) => failed(true, "Rubylight could not be updated", &error),
                }
            }
            _ => failed(
                true,
                "Invalid update arguments",
                &anyhow::anyhow!("--update requires --quiet and --install-dir"),
            ),
        }
    } else if args.uninstall {
        run_uninstall(&args)
    } else {
        run_install(&args)
    };
    log::line(format!("exit code {code}"));
    std::process::exit(code);
}

/// The install dialogs; `None` when the user cancelled.
fn ask_install(args: &Arguments, found: &detect::Found) -> Option<Choices> {
    let mut folder = detect::install_dir(found, args.install_dir.clone());
    let mut gamepad_driver = !args.no_gamepad_driver;
    let mut usbip_driver = usbip::selected(&install::profile(), args.usbip_driver);
    let mut end_streams = args.end_streams;
    if !args.quiet {
        // Updating from inside a stream is fine: the stream ends while
        // Rubylight restarts and the client connects again afterwards, as
        // with Vibepollo's installer.
        let streaming = install::streaming(&install::profile());
        let choosable = args.install_dir.is_none() && detect::folder_choosable(found);
        loop {
            let mut text = detect::summary(found, &folder);
            if install::display_driver_declined() {
                text.push_str("\n\n");
                text.push_str(floor::DECLINED_DRIVER_NOTICE);
            }
            if streaming {
                text.push_str("\n\nRubylight is streaming. Installing ends the stream and closes games started from Moonlight; connect again when setup has finished.");
            }
            let choice = ui::ask(
                TITLE,
                &format!("Install Rubylight {}", env!("CARGO_PKG_VERSION")),
                &text,
                if args.usbip_driver.is_none() {
                    "Next"
                } else {
                    "Install"
                },
                choosable.then_some("Change folder…"),
                (!args.no_gamepad_driver).then_some((
                    "Install the virtual gamepad driver for controllers",
                    gamepad_driver,
                )),
            );
            gamepad_driver = !args.no_gamepad_driver && choice.checked;
            if choice.other {
                if let Some(picked) = ui::pick_folder("Choose where to install Rubylight", &folder)
                {
                    if system::local_drive(&picked) {
                        folder = detect::chosen_folder(&picked);
                    } else {
                        ui::message_box(
                            TITLE,
                            "Choose a folder on a drive of this PC. The Rubylight service cannot run from a network or removable drive.",
                        );
                    }
                }
                continue;
            }
            if !choice.accepted {
                return None;
            }
            end_streams |= streaming;
            break;
        }
        if args.usbip_driver.is_none() {
            let choice = ui::ask(
                TITLE,
                "Steam Deck and DualSense USB audio and haptics",
                "The bundled usbip-win2 driver enables native Steam Deck controllers and DualSense USB audio for waveform haptics. A compatible client is required.\n\nInstalling it can reconnect USB devices and require a Windows restart. Setup never restarts Windows automatically. Existing USB/IP installations are kept, and this shared driver remains installed when Rubylight is removed.",
                "Install",
                None,
                Some(("Install USB audio and haptics support", usbip_driver)),
            );
            if !choice.accepted {
                return None;
            }
            usbip_driver = choice.checked;
        }
    }
    Some(Choices {
        folder,
        gamepad_driver,
        usbip_driver,
        end_streams,
    })
}

/// What the install dialogs settled.
struct Choices {
    folder: PathBuf,
    gamepad_driver: bool,
    usbip_driver: bool,
    end_streams: bool,
}

/// Runs `ask` only when `floor` says this PC can run this release, so nobody is walked
/// through the folder, streaming and driver dialogs for an install that is then refused.
fn floor_then<T>(
    floor: impl FnOnce() -> anyhow::Result<()>,
    ask: impl FnOnce() -> T,
) -> anyhow::Result<T> {
    floor()?;
    Ok(ask())
}

/// The floor verdict for this PC, with a progress dialog while the package is checked.
fn check_floor(quiet: bool) -> anyhow::Result<()> {
    ui::progress(TITLE, "Checking this PC", quiet, |_progress| {
        let mut payload = payload::Payload::open()?
            .context("this setup.exe carries no package; build it with build.ps1 -Package")?;
        floor::check(&mut payload)
    })
}

fn run_install(args: &Arguments) -> i32 {
    let found = detect::scan();
    if install::display_driver_declined() {
        log::line(format!("note: {}", floor::DECLINED_DRIVER_NOTICE));
    }
    let choices = match floor_then(|| check_floor(args.quiet), || ask_install(args, &found)) {
        Ok(Some(choices)) => choices,
        Ok(None) => return 1223,
        Err(error) => {
            return failed(
                args.quiet,
                "Rubylight cannot be installed on this PC",
                &error,
            );
        }
    };
    let Choices {
        folder,
        gamepad_driver,
        usbip_driver,
        end_streams,
    } = choices;
    let options = install::Options {
        install_dir: Some(folder),
        gamepad_driver,
        display_driver: true,
        floor_checked: true,
        usbip_driver,
        start: !args.no_start,
        end_streams,
    };
    let result = ui::progress(TITLE, "Installing Rubylight", args.quiet, move |progress| {
        install::install(&options, &progress)
    });
    match result {
        Ok(outcome) => {
            for note in &outcome.notes {
                log::line(format!("note: {note}"));
            }
            if !args.quiet {
                let mut text = format!(
                    "Pair a device and add apps at https://localhost:{}.\nInstalled in {}.",
                    outcome.web_port,
                    outcome.install.display()
                );
                if outcome.restart_needed {
                    text.push_str("\n\nRestart Windows to finish installing the drivers.");
                }
                for note in &outcome.notes {
                    text.push_str("\n\n");
                    text.push_str(note);
                }
                if ui::finished(
                    TITLE,
                    "Rubylight is installed",
                    &text,
                    true,
                    Some("Open Rubylight"),
                ) {
                    open(&format!("https://localhost:{}", outcome.web_port));
                }
            }
            if outcome.restart_needed { 3010 } else { 0 }
        }
        // install() has started again what it stopped.
        Err(error) => failed(args.quiet, "Rubylight could not be installed", &error),
    }
}
fn run_uninstall(args: &Arguments) -> i32 {
    let mut options = uninstall::Options {
        factory_reset: args.factory_reset,
        remove_drivers: args.remove_drivers,
    };
    if !args.quiet {
        let choice = ui::ask(
            TITLE,
            "Remove Rubylight?",
            "Streaming stops and the Rubylight service is removed. Drivers stay installed unless removal was requested; shared USBip always stays installed and can be removed separately in Windows Settings.",
            "Remove",
            None,
            Some((
                "Also delete settings, paired devices and logs",
                args.factory_reset,
            )),
        );
        if !choice.accepted {
            return 1223;
        }
        options.factory_reset = choice.checked;
    }
    let result = ui::progress(TITLE, "Removing Rubylight", args.quiet, move |progress| {
        uninstall::uninstall(&options, &progress)
    });
    match result {
        Ok(()) => {
            if !args.quiet {
                ui::finished(TITLE, "Rubylight was removed", "", true, None);
            }
            0
        }
        Err(error) => failed(args.quiet, "Rubylight could not be removed", &error),
    }
}
fn failed(quiet: bool, heading: &str, error: &anyhow::Error) -> i32 {
    log::line(format!("error: {error:#}"));
    if !quiet {
        let log =
            log::path().map_or_else(String::new, |p| format!("\n\nSetup log: {}", p.display()));
        ui::finished(TITLE, heading, &format!("{error:#}{log}"), false, None);
    }
    1
}
fn open(url: &str) {
    let url = windows::core::HSTRING::from(url);
    // SAFETY: the verb and URL are NUL-terminated and outlive the call.
    unsafe {
        windows::Win32::UI::Shell::ShellExecuteW(
            None,
            windows::core::w!("open"),
            &url,
            None,
            None,
            windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floor_then_asks_only_when_the_floor_is_met() {
        let mut asked = false;
        let refused = floor_then(
            || Err(anyhow::anyhow!("below the floor")),
            || {
                asked = true;
            },
        );
        assert!(refused.is_err());
        assert!(!asked, "the user was asked before the floor verdict");
        assert_eq!(floor_then(|| Ok(()), || 7).unwrap(), 7);
    }
}
