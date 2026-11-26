//! Installing, upgrading and replacing a previous Vibepollo-family host.
mod driver_repair;
mod drivers;
mod profile;
mod side_by_side;
mod staging;

pub use driver_repair::{Driver, RepairRun, repair_drivers};
pub(crate) use driver_repair::{display_driver_declined, repair_drivers_in};
pub(crate) use drivers::{
    LIBVIRTUALDISPLAY, LIBVIRTUALGAMEPAD, RUBYLIGHT_DISPLAY_SCRIPT, RUBYLIGHT_GAMEPAD_SCRIPT,
    install_drivers,
};
pub(crate) use profile::{
    prepare_install, prepare_profile, trust_console_or_warn, unrepairable, untrust_console,
    updates_in_place,
};
pub(crate) use side_by_side::{SIDE_BY_SIDE_MARK, ensure_idle, probe, streaming, wait_ready};
pub use side_by_side::{pc_address, side_by_side_result, web_port};
pub(crate) use staging::{
    prepare_staging_root, register, remove_legacy_staging, replace_file, secure_install,
    staging_root,
};

use driver_repair::remember_driver_choice;
use profile::{
    Target, import_error, migration_source, nvprefs_undo_pending, outside_other_hosts,
    refuse_foreign_service, secure_profile, stop_running, target, trust_console,
    undo_import_on_error,
};
use side_by_side::{earlier_settings_moved, pc_name, port_free, prepare_side_by_side_profile};
use staging::{SwitchFailure, copy_package, create_install, remove_legacy, switch_service, unpack};

use crate::{
    detect::{self, OLD_SERVICES, UNINSTALL},
    host_service,
    log::line,
    payload::{self, Payload},
    system,
    ui::Progress,
};
use anyhow::{Context, Result, anyhow, bail};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

pub struct Options {
    pub install_dir: Option<PathBuf>,
    pub gamepad_driver: bool,
    pub display_driver: bool,
    pub usbip_driver: bool,
    pub start: bool,
    /// End active streams instead of refusing to install. The user chose
    /// this; tests and automatic updates never set it.
    pub end_streams: bool,
    /// Keep other hosts and install alongside them ([`detect::Found::side_by_side`]).
    pub side_by_side: bool,
}
pub struct Outcome {
    pub install: PathBuf,
    pub web_port: u16,
    pub side_by_side: bool,
    pub restart_needed: bool,
    pub notes: Vec<String>,
}
/// Rubylight's own processes.
pub const OWN_PROCESSES: [&str; 4] = [
    "butterpollo.exe",
    "butterpollo-service.exe",
    "Start Rubylight.exe",
    "Start Butterpollo.exe",
];
/// Processes of Rubylight and of the hosts it replaces.
pub const HOST_PROCESSES: [&str; 10] = [
    "butterpollo.exe",
    "butterpollo-service.exe",
    "Start Rubylight.exe",
    "Start Butterpollo.exe",
    "sunshine.exe",
    "sunshinesvc.exe",
    "sunshine_wgc_capture.exe",
    "sunshine_display_helper.exe",
    "playnite-launcher.exe",
    "playnite_launcher.exe",
];
const SERVICE_DISPLAY_NAME: &str = "Rubylight";
const SERVICE_DESCRIPTION: &str = "Streams games and the desktop to Moonlight and Artemis clients.";
pub fn profile() -> PathBuf {
    system::program_data().join("Butterpollo").join("config")
}
/// The product was called Butterpollo before rc.30. Its firewall rule and
/// Start menu entry carry the name, so setup replaces the old ones; the
/// service, folders and file names keep theirs so upgrades stay in place.
pub const FIREWALL_RULE: &str = "Rubylight";
pub const LEGACY_FIREWALL_RULE: &str = "Butterpollo";
pub fn start_menu_link() -> PathBuf {
    system::program_data().join("Microsoft\\Windows\\Start Menu\\Programs\\Rubylight.lnk")
}
pub fn legacy_start_menu_link() -> PathBuf {
    system::program_data().join("Microsoft\\Windows\\Start Menu\\Programs\\Butterpollo.lnk")
}
/// After an in-place update: the firewall rule and Start menu entry under
/// the current name, replacing Butterpollo's once the new one exists.
pub fn refresh_entries(install: &Path, notes: &mut Vec<String>) {
    match system::firewall_allow(FIREWALL_RULE, &install.join("butterpollo.exe")) {
        Ok(()) => system::firewall_remove(LEGACY_FIREWALL_RULE),
        Err(error) => notes.push(format!(
            "The Windows Firewall rule could not be updated: {error:#}"
        )),
    }
    match system::shortcut(
        &start_menu_link(),
        &install.join("Start Rubylight.exe"),
        "Open the Rubylight console",
    ) {
        Ok(()) => {
            let _ = std::fs::remove_file(legacy_start_menu_link());
        }
        Err(error) => notes.push(format!(
            "The Start menu shortcut could not be created: {error:#}"
        )),
    }
}
/// The service's name in Windows Services; its key stays as it was installed.
pub fn refresh_service_name() -> Result<()> {
    system::set_service_display_name(detect::service(), SERVICE_DISPLAY_NAME)
}
/// What a full installation does to other hosts. Replacing, it takes them all
/// over; side by side, it leaves them alone and acts only on what is Rubylight's.
#[derive(Debug, PartialEq)]
struct Takeover {
    /// Services stopped first, restarted if setup fails before removing a host.
    stop: &'static [&'static str],
    /// Processes ended after that.
    end: &'static [&'static str],
    /// Their packages, Apps entries and own uninstallers run; their other
    /// services, Start menu folder and stale HDR layers go. Their profile is
    /// imported first.
    remove_hosts: bool,
    /// Firewall rules removed.
    firewall_rules: &'static [&'static str],
}
fn takeover(side_by_side: bool) -> Takeover {
    if side_by_side {
        Takeover {
            stop: &[host_service::SIDE_BY_SIDE],
            end: &OWN_PROCESSES,
            remove_hosts: false,
            firewall_rules: &[LEGACY_FIREWALL_RULE],
        }
    } else {
        Takeover {
            stop: &OLD_SERVICES,
            end: &HOST_PROCESSES,
            remove_hosts: true,
            firewall_rules: &[
                LEGACY_FIREWALL_RULE,
                "Vibepollo",
                "Vibepollo Service",
                "Apollo",
            ],
        }
    }
}

pub fn install(options: &Options, progress: &Progress) -> Result<Outcome> {
    let mut set_aside = None;
    let mut earlier = None;
    install_with(options, progress, &mut set_aside, &mut earlier).map_err(|error| {
        let error = match &earlier {
            Some(aside) => anyhow!("{error:#}\n\n{}", earlier_settings_moved(aside)),
            None => error,
        };
        match &set_aside {
            Some(aside) => moved_aside(error, aside),
            None => error,
        }
    })
}
/// An error that also says where `prepare_profile` moved a folder: the next
/// run no longer finds it to say so.
fn moved_aside(error: anyhow::Error, aside: &Path) -> anyhow::Error {
    anyhow!(
        "{error:#}\n\nAn earlier Rubylight settings folder could be changed by accounts other than Administrators, so setup moved it to {}.",
        aside.display()
    )
}
fn install_with(
    options: &Options,
    progress: &Progress,
    set_aside: &mut Option<PathBuf>,
    earlier: &mut Option<PathBuf>,
) -> Result<Outcome> {
    let mut payload = Payload::open()?
        .context("this setup.exe carries no package; build it with build.ps1 -Package")?;
    let found = detect::scan();
    found.check_version()?;
    line(format!("found: {found:#?}"));
    let side_by_side = found.side_by_side(options.side_by_side)?;
    let service = if side_by_side {
        refuse_foreign_service(system::service_program(host_service::SIDE_BY_SIDE).as_deref())?;
        host_service::SIDE_BY_SIDE
    } else {
        host_service::DEFAULT
    };
    let plan = takeover(side_by_side);
    let install = system::win32_path(&detect::install_dir(&found, options.install_dir.clone()))?;
    let profile = profile();
    let target = target(&found, &install)?;
    let updating = target == Target::InPlace;
    if updating {
        prepare_install(&install)?;
    } else {
        system::trusted_ancestors(&install)
            .map_err(|error| anyhow!("{error:#}; choose another installation folder"))?;
        // A new folder or a move alike would write over the other host's files.
        if side_by_side {
            outside_other_hosts(&found, &install)?;
        }
    }
    // The installed service identifies Rubylight's profile, even when its
    // folder is gone and setup installs in full.
    *set_aside = prepare_profile(&profile, found.service_install.is_some())?;
    let mut notes = Vec::new();
    if let Some(aside) = set_aside {
        notes.push(format!(
            "An earlier Rubylight settings folder could be changed by accounts other than Administrators, so setup moved it to {} and Rubylight starts with new settings. You can delete that folder.",
            aside.display()
        ));
    }
    if let Err(error) = remove_legacy_staging(&system::program_data()) {
        notes.push(format!("{error:#}. Delete that folder."));
    }
    let mut restart_needed = false;

    if updating {
        // Before changing the service; update::run checks again before stopping.
        ensure_idle(probe(&profile), options.end_streams)?;
        // The launcher and the scripts read the record, should it be lost.
        detect::record_service(side_by_side)?;
        // As a reinstall did before, the service starts automatically again;
        // a disabled service would otherwise fail the update's start check.
        system::install_service(
            service,
            SERVICE_DISPLAY_NAME,
            SERVICE_DESCRIPTION,
            &install.join("butterpollo-service.exe"),
        )?;
        remember_driver_choice(&profile, options.display_driver, options.gamepad_driver);
        crate::update::run(
            &install,
            options.start,
            options.end_streams,
            progress,
            &mut || {
                restart_needed |= install_drivers(
                    &install,
                    options.display_driver,
                    options.gamepad_driver,
                    options.usbip_driver,
                    progress,
                    &mut notes,
                );
            },
        )?;
        if let Err(error) = crate::usbip::remember(&profile, options.usbip_driver) {
            notes.push(format!(
                "The USB audio and haptics driver choice could not be saved: {error:#}"
            ));
        }
        // A reinstall also repairs the profile's permissions and the firewall
        // rule, as it did before.
        if let Err(error) = secure_install(&install) {
            notes.push(format!(
                "The permissions of {} could not be restricted: {error:#}",
                install.display()
            ));
        }
        if let Err(error) = secure_profile(&profile) {
            notes.push(format!(
                "The permissions of {} could not be restricted: {error:#}",
                profile.display()
            ));
        }
        refresh_entries(&install, &mut notes);
        return Ok(Outcome {
            web_port: web_port(&profile),
            side_by_side,
            install,
            restart_needed,
            notes,
        });
    }
    if let Target::Move(old) = &target {
        return move_install(
            options,
            old,
            &install,
            &profile,
            service,
            &mut payload,
            progress,
            notes,
        );
    }

    // Previous hosts are only removed by a full installation.
    let previous = if side_by_side {
        None
    } else {
        migration_source(&found, &profile, &install)?
    };
    let created = create_install(&install)?;
    let (staging, entries) = unpack(&mut payload, &install, created, progress)?;
    let new_profile = side_by_side
        && prepare_side_by_side_profile(
            &profile,
            found.current().is_some(),
            pc_name().as_deref(),
            port_free,
            earlier,
        )?;
    if let Some(aside) = earlier {
        notes.push(earlier_settings_moved(aside));
    }

    let active_profile = previous
        .as_ref()
        .map(|root| {
            if root.join("config/sunshine.conf").is_file() {
                root.join("config")
            } else {
                root.clone()
            }
        })
        .unwrap_or_else(|| profile.clone());
    ensure_idle(probe(&active_profile), options.end_streams)?;
    progress.set("Stopping the streaming host…");
    // Until the previous host is removed, a failure starts again exactly the
    // services that were running.
    let mut restart = stop_running(
        plan.stop,
        system::service_running,
        system::stop_service,
        system::start_service,
    )?;
    system::kill(plan.end);
    // An update that did not finish is rolled back first; its record would
    // otherwise make the service put that backup over this installation.
    crate::update::recover(&profile)?;

    if let Some(root) = &previous {
        line(format!("previous installation: {}", root.display()));
        // Vibepollo journals NVIDIA profile changes; only its own program can
        // put them back, so do it before that program is removed.
        if nvprefs_undo_pending(&system::program_data(), root) {
            progress.set("Restoring NVIDIA settings changed by Vibepollo…");
            let _ = system::run(
                &root.join("sunshine.exe").display().to_string(),
                &["--restore-nvprefs-undo"],
                Duration::from_secs(60),
            );
        }
    }

    undo_import_on_error(&profile, previous.is_some() || new_profile, || {
        if let Some(root) = &previous {
            progress.set("Importing settings, paired devices and apps…");
            let source = if root.join("config\\sunshine.conf").is_file() {
                root.join("config")
            } else {
                root.clone()
            };
            let (code, output) = system::run(
                &staging.path().join("butterpollo.exe").display().to_string(),
                &[
                    "--config-dir",
                    &profile.display().to_string(),
                    "--import-config",
                    &source.display().to_string(),
                ],
                Duration::from_secs(300),
            )?;
            if code != 0 {
                // Nothing has been removed yet: the services that were running
                // start again when this error returns.
                bail!(
                    "importing the settings from {} failed:\n\n{}",
                    source.display(),
                    import_error(&output)
                        .unwrap_or_else(|| format!("butterpollo.exe exited with code {code}"))
                );
            }
            notes.push(format!(
                "Settings, paired devices and apps were imported from {}. The console keeps that host's sign-in: use its username and password.",
                source.display()
            ));
        }

        progress.set("Installing files…");
        copy_package(staging.path(), &install, &entries)?;
        payload::write_stub(
            &install.join("uninstall.exe"),
            payload::packaged_stub(staging.path(), &entries)?.as_deref(),
        )
    })?;

    // The previous host is removed from here on, so a failure now starts
    // the service Rubylight runs as instead.
    restart.services = vec![service];
    if plan.remove_hosts {
        restart_needed |= remove_hosts(&found, progress, &mut notes)?;
    }

    progress.set("Registering the Rubylight service…");
    if plan.remove_hosts {
        // Remaining old services would hold the streaming ports.
        for old in OLD_SERVICES.iter().filter(|s| **s != service) {
            let _ = system::delete_service(old);
        }
    }
    detect::record_service(side_by_side)?;
    system::install_service(
        service,
        SERVICE_DISPLAY_NAME,
        SERVICE_DESCRIPTION,
        &install.join("butterpollo-service.exe"),
    )?;
    system::firewall_allow(FIREWALL_RULE, &install.join("butterpollo.exe"))?;
    for rule in plan.firewall_rules {
        system::firewall_remove(rule);
    }
    secure_profile(&profile)?;

    remember_driver_choice(&profile, options.display_driver, options.gamepad_driver);
    restart_needed |= install_drivers(
        &install,
        options.display_driver,
        options.gamepad_driver,
        options.usbip_driver,
        progress,
        &mut notes,
    );
    if let Err(error) = crate::usbip::remember(&profile, options.usbip_driver) {
        notes.push(format!(
            "The USB audio and haptics driver choice could not be saved: {error:#}"
        ));
    }

    progress.set("Adding Rubylight to Start and Apps…");
    match system::shortcut(
        &start_menu_link(),
        &install.join("Start Rubylight.exe"),
        "Open the Rubylight console",
    ) {
        Ok(()) => {
            let _ = std::fs::remove_file(legacy_start_menu_link());
        }
        Err(error) => notes.push(format!(
            "The Start menu shortcut could not be created: {error:#}"
        )),
    }
    if plan.remove_hosts {
        let _ = std::fs::remove_dir_all(
            system::program_data().join("Microsoft\\Windows\\Start Menu\\Programs\\Vibepollo"),
        );
    }
    // Earlier installers' entry, with Apollo's icon.
    let _ = std::fs::remove_file(
        system::program_data()
            .join("Microsoft\\Windows\\Start Menu\\Programs\\Butterpollo Rust.lnk"),
    );
    if plan.remove_hosts {
        crate::uninstall::remove_stale_layers();
    }
    crate::uninstall::remove_earlier_folder(&install);
    register(&install, &entries)?;

    let web_port = web_port(&profile);
    if options.start {
        progress.set("Starting Rubylight…");
        if system::start_service_unless_disabled(service)? {
            wait_ready(probe(&profile), Some(env!("CARGO_PKG_VERSION")))?;
            // The host made the console's certificate as it started.
            if let Err(error) = trust_console(&profile) {
                notes.push(format!(
                    "Browsers will warn about the console's certificate: its root could not be trusted: {error:#}"
                ));
            }
        } else {
            notes.push(crate::update::DISABLED_WARNING.into());
        }
    }
    restart.services.clear();
    crate::update::record_installed(&profile);
    drop(staging);
    Ok(Outcome {
        install,
        web_port,
        side_by_side,
        restart_needed,
        notes,
    })
}
/// Uninstall the hosts `found` lists. A failure to remove one is a note; only
/// a removal that cannot run at all fails. Returns whether Windows must restart.
fn remove_hosts(
    found: &detect::Found,
    progress: &Progress,
    notes: &mut Vec<String>,
) -> Result<bool> {
    use windows::Win32::System::Registry::{HKEY, HKEY_CURRENT_USER};
    let mut restart_needed = false;
    let msiexec = system::system32("msiexec.exe");
    let mut removed_msis: Vec<(&str, HKEY)> = Vec::new();
    for package in &found.packages {
        if package.root == HKEY_CURRENT_USER {
            notes.push(format!(
                "{} is installed for the current user and could not be removed automatically; remove it from Settings > Apps.",
                package.name
            ));
            continue;
        }
        let Some(code) = package.product_code() else {
            continue;
        };
        progress.set(&format!("Removing {} {}…", package.name, package.version));
        let log = std::env::temp_dir().join("butterpollo-setup-previous-uninstall.log");
        let args = [
            "/x",
            code,
            "/qn",
            "/norestart",
            "/l*v",
            &log.display().to_string(),
            "REBOOT=ReallySuppress",
            "SUPPRESSMSGBOXES=1",
            "FACTORYRESET=0",
            "REMOVEVIRTUALDISPLAYDRIVER=0",
            "REMOVEVIRTUALGAMEPADDRIVER=0",
        ];
        let mut result = system::run(&msiexec, &args, Duration::from_secs(900))?.0;
        for _ in 0..6 {
            if result != 1618 {
                break;
            }
            std::thread::sleep(Duration::from_secs(5));
            result = system::run(&msiexec, &args, Duration::from_secs(900))?.0;
        }
        match result {
            0 | 1605 => removed_msis.push((&package.name, package.root)),
            3010 => {
                restart_needed = true;
                removed_msis.push((&package.name, package.root));
            }
            code => notes.push(format!(
                "{} could not be removed completely (msiexec {code}); remove it from Settings > Apps.",
                package.name
            )),
        }
    }
    for entry in &found.vibepollo_entries {
        if entry.root == HKEY_CURRENT_USER {
            notes.push(format!(
                "{} is installed for the current user and could not be removed automatically; remove it from Settings > Apps.",
                entry.name
            ));
            continue;
        }
        let has_matching_msi = found.packages.iter().any(|p| {
            p.root == entry.root
                && p.name.eq_ignore_ascii_case(&entry.name)
                && p.product_code().is_some()
        });
        if has_matching_msi {
            let matches_removed = removed_msis
                .iter()
                .any(|(name, root)| *root == entry.root && name.eq_ignore_ascii_case(&entry.name));
            // The Vibepollo uninstaller's own entry, if the removed MSI package left it.
            if matches_removed
                && system::registry_string(
                    entry.root,
                    &format!("{UNINSTALL}\\{}", entry.key),
                    "DisplayName",
                )
                .is_some()
            {
                system::delete_key(entry.root, &format!("{UNINSTALL}\\{}", entry.key));
            }
        } else {
            progress.set(&format!("Removing {} {}…", entry.name, entry.version));
            if let Err(error) = remove_legacy(entry) {
                notes.push(format!("{} could not be removed: {error:#}", entry.name));
            }
        }
    }
    for legacy in &found.legacy {
        if legacy.root == HKEY_CURRENT_USER {
            notes.push(format!(
                "{} is installed for the current user and could not be removed automatically; remove it from Settings > Apps.",
                legacy.name
            ));
            continue;
        }
        progress.set(&format!("Removing {} {}…", legacy.name, legacy.version));
        if let Err(error) = remove_legacy(legacy) {
            notes.push(format!("{} could not be removed: {error:#}", legacy.name));
        }
    }
    Ok(restart_needed)
}

/// Moves an installed Rubylight out of a folder it cannot stay in (see
/// `unrepairable`) into `install`. The service, the profile and the drivers
/// stay; only the folder the service runs from changes. Nothing in the old
/// folder is changed or copied: if Rubylight does not start from `install`,
/// the service is pointed back at the old folder and runs from it as before.
/// Afterwards nothing uses the old folder. Its files are left for the user
/// to delete: deleting in a folder another user controls can be redirected
/// to other files.
#[allow(clippy::too_many_arguments)]
fn move_install(
    options: &Options,
    old: &Path,
    install: &Path,
    profile: &Path,
    service: &'static str,
    payload: &mut Payload,
    progress: &Progress,
    mut notes: Vec<String>,
) -> Result<Outcome> {
    line(format!(
        "moving Rubylight from {} to {}",
        old.display(),
        install.display()
    ));
    // Staged in the new folder: the old one is the folder that fails the
    // owner check.
    let created = create_install(install)?;
    let (staging, entries) = unpack(payload, install, created, progress)?;
    ensure_idle(probe(profile), options.end_streams)?;
    progress.set("Stopping Rubylight…");
    // Until the service has started from `install`, a failure starts it
    // again from the old folder.
    let mut restart = stop_running(
        &[service],
        system::service_running,
        system::stop_service,
        system::start_service,
    )?;
    crate::update::recover(profile)?;
    progress.set("Installing files…");
    copy_package(staging.path(), install, &entries)?;
    payload::write_stub(
        &install.join("uninstall.exe"),
        payload::packaged_stub(staging.path(), &entries)?.as_deref(),
    )?;
    remember_driver_choice(profile, options.display_driver, options.gamepad_driver);
    let mut restart_needed = false;
    let point_at = |folder: &Path| {
        system::install_service(
            service,
            SERVICE_DISPLAY_NAME,
            SERVICE_DESCRIPTION,
            &folder.join("butterpollo-service.exe"),
        )
    };
    progress.set("Moving the Rubylight service…");
    let switched = switch_service(
        || point_at(install),
        || {
            // While the service is stopped, as an update does.
            restart_needed |= install_drivers(
                install,
                options.display_driver,
                options.gamepad_driver,
                options.usbip_driver,
                progress,
                &mut notes,
            );
            if options.start {
                progress.set("Starting Rubylight…");
                if system::start_service_unless_disabled(service)? {
                    wait_ready(probe(profile), Some(env!("CARGO_PKG_VERSION")))?;
                    trust_console_or_warn(profile);
                } else {
                    notes.push(crate::update::DISABLED_WARNING.into());
                }
            }
            Ok(())
        },
        || {
            progress.set("Returning to the previous installation…");
            system::stop_service(service)?;
            point_at(old)
        },
    );
    if let Err(SwitchFailure { error, back }) = switched {
        bail!(match back {
            Ok(()) => format!(
                "Rubylight could not be moved to {}: {error:#}. The service runs from {} again, as before.",
                install.display(),
                old.display()
            ),
            Err(back) => format!(
                "Rubylight could not be moved to {}: {error:#}. Its service could not be pointed back at {}: {back:#}. Run setup again.",
                install.display(),
                old.display()
            ),
        });
    }
    restart.services.clear();
    // The host registers its HDR layer from the folder it runs from; every
    // Vulkan program would still load the old folder's copy.
    crate::uninstall::remove_layers(old);
    crate::uninstall::remove_earlier_folder(install);
    if let Err(error) = register(install, &entries) {
        notes.push(format!("Rubylight could not be listed in Apps: {error:#}"));
    }
    if let Err(error) = secure_profile(profile) {
        notes.push(format!(
            "The permissions of {} could not be restricted: {error:#}",
            profile.display()
        ));
    }
    refresh_entries(install, &mut notes);
    if let Err(error) = crate::usbip::remember(profile, options.usbip_driver) {
        notes.push(format!(
            "The USB audio and haptics driver choice could not be saved: {error:#}"
        ));
    }
    crate::update::record_installed(profile);
    notes.push(format!(
        "Rubylight was moved from {} and nothing uses that folder any more. Delete it once you no longer need anything in it.",
        old.display()
    ));
    drop(staging);
    Ok(Outcome {
        install: install.to_path_buf(),
        web_port: web_port(profile),
        side_by_side: service == host_service::SIDE_BY_SIDE,
        restart_needed,
        notes,
    })
}

#[cfg(test)]
use driver_repair::*;
#[cfg(test)]
use drivers::*;
#[cfg(test)]
use profile::*;
#[cfg(test)]
use side_by_side::*;
#[cfg(test)]
use staging::*;
#[cfg(test)]
use std::net::{IpAddr, SocketAddr};
#[cfg(test)]
use windows::Win32::System::Registry::HKEY_LOCAL_MACHINE;

#[cfg(test)]
mod tests;
