#![windows_subsystem = "windows"]
//! Rubylight setup. Installs or updates Rubylight, replacing an installed
//! Vibepollo, Rubylight C++, Apollo or Sunshine in place (settings, paired
//! devices, apps and drivers are kept), and removes Rubylight again.
//!
//! rubylight-setup.exe [--quiet] [--install-dir <folder>] [--no-gamepad-driver]
//!                       [--no-display-driver] [--no-start] [--end-streams]
//! rubylight-setup.exe --uninstall [--quiet] [--factory-reset] [--remove-drivers]
//! rubylight-setup.exe --repair-drivers (the service runs this one)
#![warn(clippy::undocumented_unsafe_blocks)]

mod detect;
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
#[path = "../../core/src/version.rs"]
mod version;

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
const USAGE: &str = "rubylight-setup.exe [--quiet] [--install-dir <folder>] [--no-gamepad-driver] [--no-display-driver] [--no-start] [--end-streams]\n\
rubylight-setup.exe --uninstall [--quiet] [--factory-reset] [--remove-drivers]\n\n\
A quiet setup refuses to install while Rubylight is streaming; --end-streams ends the stream instead, and the client can reconnect once setup has finished.\n\n\
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
                match ui::progress(TITLE, "Updating Rubylight", true, move |progress| {
                    let mut notes = Vec::new();
                    let folder = system::win32_path(&folder)?;
                    // As a reinstall does. In-app updates never set the
                    // drivers up, so hosts updated from the console since
                    // rc.22 had none.
                    update::run(&folder, true, end_streams, &progress, &mut || {
                        install::install_drivers(
                            &folder,
                            !install::display_driver_declined(),
                            true,
                            &progress,
                            &mut notes,
                        );
                        Ok(())
                    })?;
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
                    anyhow::Ok(())
                }) {
                    Ok(()) => 0,
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

fn run_install(args: &Arguments) -> i32 {
    let found = detect::scan();
    let mut folder = detect::install_dir(&found, args.install_dir.clone());
    let mut gamepad_driver = !args.no_gamepad_driver;
    let mut end_streams = args.end_streams;
    let mut disable_background_downloads = false;
    if !args.quiet {
        // Updating from inside a stream is fine: the stream ends while
        // Rubylight restarts and the client connects again afterwards, as
        // with Vibepollo's installer.
        let streaming = install::streaming(&install::profile());
        let choosable = args.install_dir.is_none() && detect::folder_choosable(&found);
        loop {
            let mut text = detect::summary(&found, &folder);
            text.push_str("\n\nSetup installs the Rubylight Windows service (ApolloService), starts it automatically, and adds inbound Windows Firewall rules. It installs the virtual display driver unless --no-display-driver is used, and the gamepad driver if selected below.\n\nThe host registers a Vulkan HDR implicit layer for all Vulkan apps (machine-wide when running as the service); its HDR changes activate during owned HDR sessions. Streams can create virtual displays, change display modes, turn other monitors off and switch default audio devices, then restore the previous state.\n\nPaired clients with permission can stream and control the Windows lock screen and secure desktop. Only pair devices you trust. Windows authentication is still required.\n\nWindows Settings > Apps removes Rubylight. The uninstaller also offers driver removal; settings can be kept.");
            if streaming {
                text.push_str("\n\nRubylight is streaming. Installing ends the stream and closes games started from Moonlight; connect again when setup has finished.");
            }
            let choice = ui::ask(
                TITLE,
                &format!("Install Rubylight {}", env!("CARGO_PKG_VERSION")),
                &text,
                "Install",
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
                return 1223;
            }
            end_streams |= streaming;
            break;
        }
        let privacy = ui::ask(
            TITLE,
            "Privacy and network access",
            "Rubylight checks GitHub for updates once a day by default; GitHub receives your IP address and the Rubylight user agent. Automatic installation is opt-in. Steam integration and automatic sync, when enabled, download missing artwork from Valve using game IDs. Cover searches you request contact GitHub GameDB and IGDB.\n\nStreaming and optional hardware metrics go to permitted paired clients. Local discovery advertises the host; UPnP contacts your router only when enabled. Logs and crash reports stay on this PC unless you share them.\n\nDisable background downloads to turn off scheduled update checks, automatic updates and Steam auto-sync before the host starts. Manual downloads remain available.\n\nCode signing policy and full privacy statement:\nhttps://ramazankara.github.io/Rubylight/docs/code-signing.html",
            "Keep current settings",
            Some("Disable background downloads"),
            None,
        );
        if !privacy.accepted && !privacy.other {
            return 1223;
        }
        disable_background_downloads = privacy.other;
    }
    let options = install::Options {
        install_dir: Some(folder),
        gamepad_driver,
        display_driver: !args.no_display_driver,
        start: !args.no_start,
        end_streams,
        disable_background_downloads,
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
            "Streaming stops and session display/audio changes are restored. Setup removes the Rubylight service, firewall rules and Vulkan HDR layer registration. Virtual display and gamepad drivers stay installed unless you choose Remove drivers too or use --remove-drivers. Driver removal can require a Windows restart.",
            "Remove",
            Some("Remove drivers too"),
            Some((
                "Also delete settings, paired devices and logs",
                args.factory_reset,
            )),
        );
        if !choice.accepted && !choice.other {
            return 1223;
        }
        options.factory_reset = choice.checked;
        options.remove_drivers |= choice.other;
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
