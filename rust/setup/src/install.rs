//! Installing, upgrading and replacing a previous Vibepollo-family host.
use crate::{
    detect::{self, OLD_SERVICES, SERVICE, UNINSTALL},
    log::line,
    payload::{self, Payload},
    system::{self, Value},
    ui::Progress,
};
use anyhow::{Context, Result, anyhow, bail};
use std::{
    net::{IpAddr, SocketAddr},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use windows::Win32::System::Registry::HKEY_LOCAL_MACHINE;

pub struct Options {
    pub install_dir: Option<PathBuf>,
    pub gamepad_driver: bool,
    pub display_driver: bool,
    pub usbip_driver: bool,
    pub start: bool,
    /// End active streams instead of refusing to install. The user chose
    /// this; tests and automatic updates never set it.
    pub end_streams: bool,
}
pub struct Outcome {
    pub install: PathBuf,
    pub web_port: u16,
    pub restart_needed: bool,
    pub notes: Vec<String>,
}
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
/// The service's name in Windows Services; its key stays `SERVICE`.
pub fn refresh_service_name() -> Result<()> {
    system::set_service_display_name(SERVICE, SERVICE_DISPLAY_NAME)
}
/// The text of a profile's sunshine.conf, empty if it cannot be read. The
/// host also reads one saved with a byte order mark or as UTF-16.
fn conf_text(profile: &Path) -> String {
    let bytes = std::fs::read(profile.join("sunshine.conf")).unwrap_or_default();
    let utf16 = |bytes: &[u8], unit: fn([u8; 2]) -> u16| {
        let units: Vec<u16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&pair| unit(pair))
            .collect();
        String::from_utf16_lossy(&units)
    };
    if let Some(rest) = bytes.strip_prefix(b"\xff\xfe") {
        return utf16(rest, u16::from_le_bytes);
    }
    if let Some(rest) = bytes.strip_prefix(b"\xfe\xff") {
        return utf16(rest, u16::from_be_bytes);
    }
    let text = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&bytes);
    String::from_utf8_lossy(text).into_owned()
}
/// The configured base port of a profile (47989 unless set).
pub fn web_port(profile: &Path) -> u16 {
    let base = conf_text(profile)
        .lines()
        .find_map(|line| {
            let (key, value) = line.split_once('=')?;
            (key.trim() == "port")
                .then(|| {
                    value
                        .split('#')
                        .next()?
                        .trim()
                        .trim_matches('"')
                        .parse::<u16>()
                        .ok()
                })
                .flatten()
        })
        .filter(|port| (1029..=65514).contains(port))
        .unwrap_or(47989);
    base + 1
}
/// Where the host answers serverinfo: its bind_address, or this PC when
/// that is blank, all interfaces or not an address (the host then listens
/// on all of them or only on this PC). The first value that is not empty
/// counts, as the host reads it.
fn probe_address(conf: &str) -> IpAddr {
    conf.lines()
        .find_map(|line| {
            let (key, value) = line.split('#').next()?.split_once('=')?;
            let value = value.trim().trim_matches('"').trim();
            (key.trim() == "bind_address" && !value.is_empty()).then_some(value)
        })
        .and_then(|value| value.parse::<IpAddr>().ok())
        .filter(|address| !address.is_unspecified())
        .unwrap_or(IpAddr::from([127, 0, 0, 1]))
}
/// The host's serverinfo address for a profile.
pub(crate) fn probe(profile: &Path) -> SocketAddr {
    SocketAddr::new(probe_address(&conf_text(profile)), web_port(profile) - 1)
}

pub fn install(options: &Options, progress: &Progress) -> Result<Outcome> {
    let mut set_aside = None;
    install_with(options, progress, &mut set_aside).map_err(|error| match &set_aside {
        Some(aside) => moved_aside(error, aside),
        None => error,
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
) -> Result<Outcome> {
    let mut payload = Payload::open()?
        .context("this setup.exe carries no package; build it with build.ps1 -Package")?;
    let found = detect::scan();
    found.check_version()?;
    line(format!("found: {found:#?}"));
    let install = system::win32_path(&detect::install_dir(&found, options.install_dir.clone()))?;
    let profile = profile();
    let target = target(&found, &install)?;
    let updating = target == Target::InPlace;
    if updating {
        prepare_install(&install)?;
    } else {
        system::trusted_ancestors(&install)
            .map_err(|error| anyhow!("{error:#}; choose another installation folder"))?;
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
    let mut restart_needed = false;

    if updating {
        // Before changing the service; update::run checks again before stopping.
        ensure_idle(probe(&profile), options.end_streams)?;
        // As a reinstall did before, the service starts automatically again;
        // a disabled service would otherwise fail the update's start check.
        system::install_service(
            SERVICE,
            SERVICE_DISPLAY_NAME,
            SERVICE_DESCRIPTION,
            &install.join("butterpollo-service.exe"),
        )?;
        remember_driver_choice(&profile, options.display_driver);
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
        crate::usbip::remember(&profile, options.usbip_driver)?;
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
            install,
            restart_needed,
            notes,
        });
    }
    if let Target::Move(old) = &target {
        return move_install(options, old, &install, &profile, &mut payload, progress);
    }

    // Previous hosts are only removed by a full installation.
    let previous = migration_source(&found, &profile, &install)?;
    let (staging, entries) = unpack(&mut payload, progress)?;

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
        &OLD_SERVICES,
        system::service_running,
        system::stop_service,
        system::start_service,
    )?;
    system::kill(&HOST_PROCESSES);
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
        // A package without drivers reuses the signed drivers already
        // installed with Vibepollo.
        for (from, to) in [("drivers\\vhf-gamepad", "drivers\\gamepad")] {
            if !staging.join(to).exists() && root.join(from).is_dir() {
                copy_tree(&root.join(from), &staging.join(to))?;
            }
        }
    }

    undo_import_on_error(&profile, previous.is_some(), || {
        if let Some(root) = &previous {
            progress.set("Importing settings, paired devices and apps…");
            let source = if root.join("config\\sunshine.conf").is_file() {
                root.join("config")
            } else {
                root.clone()
            };
            let (code, output) = system::run(
                &staging.join("butterpollo.exe").display().to_string(),
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
        std::fs::create_dir_all(&install)?;
        secure_install(&install)?;
        copy_package(&staging, &install, &entries)?;
        payload::write_stub(
            &install.join("uninstall.exe"),
            payload::packaged_stub(&staging, &entries)?.as_deref(),
        )
    })?;

    // The previous host is removed from here on, so a failure now starts
    // the service Rubylight runs as instead.
    restart.services = vec![SERVICE];
    for package in &found.packages {
        let Some(code) = package.product_code() else {
            continue;
        };
        progress.set(&format!("Removing {} {}…", package.name, package.version));
        let log = std::env::temp_dir().join("butterpollo-setup-previous-uninstall.log");
        let (result, _) = system::run(
            "msiexec.exe",
            &[
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
            ],
            Duration::from_secs(900),
        )?;
        match result {
            0 | 1605 => {}
            3010 => restart_needed = true,
            code => notes.push(format!(
                "{} could not be removed completely (msiexec {code}); remove it from Settings > Apps.",
                package.name
            )),
        }
    }
    for entry in &found.vibepollo_entries {
        // The Vibepollo uninstaller's own entry, if the package left it.
        if system::registry_string(
            entry.root,
            &format!("{UNINSTALL}\\{}", entry.key),
            "DisplayName",
        )
        .is_some()
            && found.packages.iter().any(|p| p.product_code().is_some())
        {
            system::delete_key(entry.root, &format!("{UNINSTALL}\\{}", entry.key));
        }
    }
    for legacy in &found.legacy {
        progress.set(&format!("Removing {} {}…", legacy.name, legacy.version));
        if let Err(error) = remove_legacy(legacy) {
            notes.push(format!("{} could not be removed: {error:#}", legacy.name));
        }
    }

    progress.set("Registering the Rubylight service…");
    // Remaining old services would hold the streaming ports.
    for service in OLD_SERVICES.iter().filter(|s| **s != SERVICE) {
        let _ = system::delete_service(service);
    }
    system::install_service(
        SERVICE,
        SERVICE_DISPLAY_NAME,
        SERVICE_DESCRIPTION,
        &install.join("butterpollo-service.exe"),
    )?;
    system::firewall_allow(FIREWALL_RULE, &install.join("butterpollo.exe"))?;
    for rule in [
        LEGACY_FIREWALL_RULE,
        "Vibepollo",
        "Vibepollo Service",
        "Apollo",
    ] {
        system::firewall_remove(rule);
    }
    secure_profile(&profile)?;

    remember_driver_choice(&profile, options.display_driver);
    restart_needed |= install_drivers(
        &install,
        options.display_driver,
        options.gamepad_driver,
        options.usbip_driver,
        progress,
        &mut notes,
    );
    crate::usbip::remember(&profile, options.usbip_driver)?;

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
    let _ = std::fs::remove_dir_all(
        system::program_data().join("Microsoft\\Windows\\Start Menu\\Programs\\Vibepollo"),
    );
    // Earlier installers' entry, with Apollo's icon.
    let _ = std::fs::remove_file(
        system::program_data()
            .join("Microsoft\\Windows\\Start Menu\\Programs\\Butterpollo Rust.lnk"),
    );
    crate::uninstall::remove_stale_layers();
    crate::uninstall::remove_earlier_folder(&install);
    register(&install, &entries)?;

    let web_port = web_port(&profile);
    if options.start {
        progress.set("Starting Rubylight…");
        if system::start_service_unless_disabled(SERVICE)? {
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
    let _ = std::fs::remove_dir_all(&staging);
    Ok(Outcome {
        install,
        web_port,
        restart_needed,
        notes,
    })
}

/// Unpack and verify the package in the profile's setup folder.
fn unpack(payload: &mut Payload, progress: &Progress) -> Result<(PathBuf, Vec<payload::Entry>)> {
    progress.set("Unpacking Rubylight…");
    let staging = system::program_data().join("Butterpollo").join("setup");
    let staging_root = staging.parent().context("staging folder has no parent")?;
    std::fs::create_dir_all(staging_root)?;
    system::restrict(staging_root, true)?;
    if staging.exists() {
        std::fs::remove_dir_all(&staging).context("removing the previous setup staging folder")?;
    }
    std::fs::create_dir_all(&staging)?;
    // The staged host runs elevated (settings import) and loads DLLs from its
    // own folder: lock the folder down before anything lands in it.
    system::restrict(&staging, false)?;
    payload.extract(&staging)?;
    let entries = payload::verify(&staging)?;
    Ok((staging, entries))
}

/// Moves an installed Rubylight out of a folder it cannot stay in (see
/// `unrepairable`) into `install`. The service, the profile and the drivers
/// stay; only the folder the service runs from changes. Nothing in the old
/// folder is changed or copied: if Rubylight does not start from `install`,
/// the service is pointed back at the old folder and runs from it as before.
/// Afterwards nothing uses the old folder. Its files are left for the user
/// to delete: deleting in a folder another user controls can be redirected
/// to other files.
fn move_install(
    options: &Options,
    old: &Path,
    install: &Path,
    profile: &Path,
    payload: &mut Payload,
    progress: &Progress,
) -> Result<Outcome> {
    line(format!(
        "moving Rubylight from {} to {}",
        old.display(),
        install.display()
    ));
    let (staging, entries) = unpack(payload, progress)?;
    ensure_idle(probe(profile), options.end_streams)?;
    progress.set("Stopping Rubylight…");
    // Until the service has started from `install`, a failure starts it
    // again from the old folder.
    let mut restart = stop_running(
        &[SERVICE],
        system::service_running,
        system::stop_service,
        system::start_service,
    )?;
    crate::update::recover(profile)?;
    progress.set("Installing files…");
    std::fs::create_dir_all(install)?;
    secure_install(install)?;
    copy_package(&staging, install, &entries)?;
    payload::write_stub(
        &install.join("uninstall.exe"),
        payload::packaged_stub(&staging, &entries)?.as_deref(),
    )?;
    remember_driver_choice(profile, options.display_driver);
    let mut notes = Vec::new();
    let mut restart_needed = false;
    let point_at = |folder: &Path| {
        system::install_service(
            SERVICE,
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
                if system::start_service_unless_disabled(SERVICE)? {
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
            system::stop_service(SERVICE)?;
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
    crate::usbip::remember(profile, options.usbip_driver)?;
    crate::update::record_installed(profile);
    notes.push(format!(
        "Rubylight was moved from {} and nothing uses that folder any more. Delete it once you no longer need anything in it.",
        old.display()
    ));
    let _ = std::fs::remove_dir_all(&staging);
    Ok(Outcome {
        install: install.to_path_buf(),
        web_port: web_port(profile),
        restart_needed,
        notes,
    })
}
#[derive(Debug)]
struct SwitchFailure {
    error: anyhow::Error,
    /// Pointing the service back at the previous installation.
    back: Result<()>,
}
/// Point the service at the new installation and start it there. If it does
/// not start, the service is pointed back at the previous installation,
/// whose files were not changed; the caller starts it again.
fn switch_service(
    to_new: impl FnOnce() -> Result<()>,
    start: impl FnOnce() -> Result<()>,
    to_old: impl FnOnce() -> Result<()>,
) -> std::result::Result<(), SwitchFailure> {
    if let Err(error) = to_new() {
        // Windows changes a service's configuration whole or not at all.
        return Err(SwitchFailure {
            error,
            back: Ok(()),
        });
    }
    match start() {
        Ok(()) => Ok(()),
        Err(error) => Err(SwitchFailure {
            error,
            back: to_old(),
        }),
    }
}

/// What setup does with the folder it installs into.
#[derive(Debug, PartialEq)]
enum Target {
    /// A new installation, or a damaged one with nothing to restore.
    Fresh,
    /// The installed service is updated in its folder, with a backup.
    InPlace,
    /// The installed service's folder, here, cannot be made safe (see
    /// `unrepairable`), so the installation moves to another folder.
    Move(PathBuf),
}
/// Whether an installed Rubylight service is updated in place, with a
/// backup and rollback. An installation whose folder or manifest is gone has
/// nothing to restore; the full installation repairs it instead. Another
/// folder is refused unless the installed one has to be left.
fn target(found: &detect::Found, install: &Path) -> Result<Target> {
    let Some(current) = found.service_install.as_deref() else {
        return Ok(Target::Fresh);
    };
    let Ok(current_folder) = current.canonicalize() else {
        line(format!(
            "the service folder {} is missing; installing in full",
            current.display()
        ));
        return Ok(Target::Fresh);
    };
    if install.canonicalize().ok().as_ref() != Some(&current_folder) {
        if let Some(reason) = &found.relocate {
            line(format!(
                "leaving {} for {}: {reason}",
                current.display(),
                install.display()
            ));
            return Ok(Target::Move(current.to_owned()));
        }
        bail!(
            "Rubylight is installed in {}. Install the update into that folder so the previous version can be restored if needed. Nothing has been changed.",
            current.display()
        );
    }
    if let Err(error) = payload::manifest(&current_folder) {
        line(format!(
            "the installed package has no readable manifest ({error:#}); installing in full"
        ));
        return Ok(Target::Fresh);
    }
    Ok(Target::InPlace)
}
/// Why the installed service's folder cannot be made safe where it is: a
/// parent folder that is a link or that someone other than Administrators,
/// SYSTEM or TrustedInstaller owns, or a link inside the folder. Whoever
/// controls a parent can put another folder in its place, so locking this
/// one down would not keep them from the programs the service runs.
pub(crate) fn unrepairable(install: &Path) -> Option<anyhow::Error> {
    system::trusted_parents(install)
        .and_then(|()| match std::fs::symlink_metadata(install) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            _ => system::no_reparse_points(install),
        })
        .err()
}
/// The installed service's folder, which the SYSTEM service runs programs
/// from. Earlier installers locked down a folder the user had created (such
/// as D:\Rubylight) but left that user its owner, who can change its
/// permissions back at any time. Once the caller has identified the
/// installed service, that owner is replaced, as `prepare_profile` does for
/// the profile, but only when no parent lets another user swap the folder.
pub(crate) fn prepare_install(install: &Path) -> Result<()> {
    if let Some(reason) = unrepairable(install) {
        bail!(
            "Rubylight cannot be updated in {}: {reason:#}. Its service runs as SYSTEM from this folder, so only administrators may be allowed to change it. Run the Rubylight installer from the release page: it moves Rubylight to {} and keeps its settings and paired devices.",
            install.display(),
            detect::default_folder().display()
        );
    }
    match system::trusted_folder(install) {
        Err(error) => {
            line(format!(
                "repairing the owner of {}: {error:#}",
                install.display()
            ));
            secure_install(install)?;
            system::trusted_folder(install)
        }
        trusted => trusted,
    }
}
/// Canonicalizes the deepest existing ancestor and appends the rest, so a
/// folder that does not exist yet compares with canonical ones even when it
/// was given through an 8.3 short name (`C:\Users\RUNNER~1\...`).
fn canonical_prefix(path: &Path) -> PathBuf {
    let mut existing = path;
    let mut missing = Vec::new();
    loop {
        if let Ok(canonical) = existing.canonicalize() {
            return missing
                .iter()
                .rev()
                .fold(canonical, |path, name| path.join(name));
        }
        match (existing.parent(), existing.file_name()) {
            (Some(parent), Some(name)) => {
                missing.push(name.to_owned());
                existing = parent;
            }
            _ => return path.to_owned(),
        }
    }
}
fn migration_source(
    found: &detect::Found,
    profile: &Path,
    install: &Path,
) -> Result<Option<PathBuf>> {
    let mut sources: Vec<(&str, PathBuf)> = Vec::new();
    for product in found
        .packages
        .iter()
        .chain(&found.legacy)
        .chain(&found.vibepollo_entries)
    {
        // An entry left behind after its folder was deleted has nothing to
        // import, and its removal deletes nothing.
        let Some(root) = product
            .location
            .as_ref()
            .and_then(|root| root.canonicalize().ok())
        else {
            line(format!(
                "{} has no installation folder; nothing to import",
                product.name
            ));
            continue;
        };
        let root = system::win32_path(&root)?;
        let root_path = root.to_string_lossy().to_lowercase();
        let install_path = system::win32_path(&canonical_prefix(install))?
            .to_string_lossy()
            .to_lowercase();
        if install_path == root_path || install_path.starts_with(&format!("{root_path}\\")) {
            bail!(
                "Choose an installation folder outside {}; removing the previous host could delete Rubylight's files",
                root.display()
            );
        }
        // The confirmation summary already says these settings were not
        // found; such a host is replaced without an import, as before.
        if !detect::has_profile(&root) {
            line(format!(
                "{} in {} has no settings to import",
                product.name,
                root.display()
            ));
        } else if !sources.iter().any(|(_, known)| known == &root) {
            sources.push((&product.name, root));
        }
    }
    let Some((name, source)) = sources.first() else {
        return Ok(None);
    };
    if sources.len() > 1 {
        let hosts = sources
            .iter()
            .map(|(name, root)| format!("{name} ({})", root.display()))
            .collect::<Vec<_>>()
            .join(", ");
        bail!(
            "Setup can import the settings of only one previous host, and these each have their own: {hosts}. Uninstall the ones whose settings you do not need, then run setup again. Nothing has been changed."
        );
    }
    let empty = match std::fs::read_dir(profile) {
        Ok(mut entries) => entries.next().transpose()?.is_none(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
        Err(error) => return Err(error).context("checking the existing Rubylight profile"),
    };
    if !empty {
        bail!(
            "Rubylight already has settings in {}, and {name} has its own in {}. To keep Rubylight's, uninstall {name} first; to import {name}'s instead, move the Rubylight folder elsewhere. Then run setup again. Nothing has been changed.",
            profile.display(),
            source.display()
        );
    }
    Ok(Some(source.clone()))
}
/// Runs the import and the steps before the previous host is removed. When
/// one fails, the profile this run imported into is removed again:
/// `migration_source` found it empty, and a retry refuses to import over
/// settings it would take for Rubylight's own.
fn undo_import_on_error(
    profile: &Path,
    importing: bool,
    steps: impl FnOnce() -> Result<()>,
) -> Result<()> {
    let Err(error) = steps() else {
        return Ok(());
    };
    if !importing {
        return Err(error);
    }
    match std::fs::remove_dir_all(profile) {
        Ok(()) => Err(error),
        Err(undo) if undo.kind() == std::io::ErrorKind::NotFound => Err(error),
        Err(undo) => Err(anyhow!(
            "{error:#}\n\nThe settings setup imported into {} could not be removed again ({undo}). Delete that folder, then run setup again.",
            profile.display()
        )),
    }
}
/// Stop `services`, restarting those that were running if any stop fails.
fn stop_running<F: FnMut(&str) -> Result<()>>(
    services: &[&'static str],
    running: impl Fn(&str) -> bool,
    mut stop: impl FnMut(&str) -> Result<()>,
    start: F,
) -> Result<Restart<F>> {
    let mut restart = Restart {
        services: Vec::new(),
        start,
    };
    for &service in services {
        if running(service) {
            restart.services.push(service);
        }
        stop(service)?;
    }
    Ok(restart)
}
/// Starts `services` when dropped, so that setup returning early with an
/// error never leaves the streaming host stopped; cleared on success.
struct Restart<F: FnMut(&str) -> Result<()>> {
    services: Vec<&'static str>,
    start: F,
}
impl<F: FnMut(&str) -> Result<()>> Drop for Restart<F> {
    fn drop(&mut self) {
        for service in &self.services {
            if let Err(error) = (self.start)(service) {
                line(format!(
                    "the {service} service could not be restarted: {error:#}"
                ));
            }
        }
    }
}
/// What butterpollo.exe printed when it failed: anyhow's "Error: ..." and
/// its "Caused by:" chain, which follow any warnings.
/// Vibepollo's journal of NVIDIA profile changes, and the program that can
/// undo them, both still present.
fn nvprefs_undo_pending(program_data: &Path, previous: &Path) -> bool {
    program_data.join("Sunshine\\nvprefs_undo.json").is_file()
        && previous.join("sunshine.exe").is_file()
}
fn import_error(output: &str) -> Option<String> {
    let lines: Vec<&str> = output.lines().collect();
    let start = lines.iter().rposition(|line| line.starts_with("Error: "))?;
    let error = lines[start..].join("\n");
    Some(error["Error: ".len()..].trim().to_owned())
}
/// SYSTEM and Administrators control the service profile, users may read
/// it (the launcher reads the port); credentials are not readable by users.
fn secure_profile(profile: &Path) -> Result<()> {
    secure_profile_with(profile, system::restrict, system::inherit_contents)
}
fn secure_profile_with(
    profile: &Path,
    mut restrict: impl FnMut(&Path, bool) -> Result<()>,
    inherit: impl FnOnce(&Path) -> Result<()>,
) -> Result<()> {
    let root = profile.parent().context("profile has no parent")?;
    std::fs::create_dir_all(root)?;
    restrict(root, true)?;
    std::fs::create_dir_all(profile.join("credentials"))?;
    // The reset reaches the credentials as well. Lock them down again even
    // when it fails partway (a file the host renames meanwhile), or the
    // host's private key stays readable by users until the host restarts.
    let reset = inherit(root);
    let credentials = restrict(&profile.join("credentials"), false);
    reset.and(credentials)
}
/// The name of the root the host makes for the console's certificate
/// (`butterpollo_core::crypto::CONSOLE_ROOT_NAME`).
const CONSOLE_ROOT_NAME: &str = "Rubylight local console";
/// Browsers open the console without a warning once its root is trusted
/// (see `ConsoleCertificate`). Roots of an earlier console certificate go.
/// [`trust_console`] where setup has no notes to show, as in an update.
pub(crate) fn trust_console_or_warn(profile: &Path) {
    if let Err(error) = trust_console(profile) {
        line(format!(
            "warning: browsers will warn about the console's certificate: its root could not be trusted: {error:#}"
        ));
    }
}
fn trust_console(profile: &Path) -> Result<()> {
    let root = profile.join("credentials").join("console-root.pem");
    if !root.is_file() {
        bail!("the host did not make {}", root.display());
    }
    untrust_console();
    let root = root.display().to_string();
    let (code, output) = system::run(
        &system::system32("certutil.exe"),
        &["-f", "-addstore", "Root", &root],
        Duration::from_secs(30),
    )?;
    if code != 0 {
        bail!("certutil exited with {code}: {}", output.trim());
    }
    Ok(())
}
pub(crate) fn untrust_console() {
    let _ = system::run(
        &system::system32("certutil.exe"),
        &["-delstore", "Root", CONSOLE_ROOT_NAME],
        Duration::from_secs(30),
    );
}
/// The host's folder of users' own Nucleus copies
/// (`butterpollo_core::nucleus::install::user_root`).
fn nucleus_user_root(root: &Path) -> PathBuf {
    root.join("nucleus-user")
}
/// Makes the profile's folder safe for the SYSTEM service, returning where a
/// folder it could not trust was moved. `installed`: a Rubylight service is
/// registered, which identifies the folder as Rubylight's.
pub(crate) fn prepare_profile(profile: &Path, installed: bool) -> Result<Option<PathBuf>> {
    let root = profile.parent().context("profile has no parent")?;
    system::trusted_parents(root)?;
    // One look decides: users may create folders in ProgramData, so a
    // missing root is made here, locked down from the start, and fails if
    // someone made it meanwhile.
    let present = match std::fs::symlink_metadata(root) {
        Ok(_) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(error.into()),
    };
    if !present {
        return system::create_restricted(root, true).map(|()| None);
    }
    if installed {
        return match system::trusted_folder(root) {
            Err(_) => {
                // Earlier installers restricted the profile's ACL but left its
                // user owner. Repair it only after identifying the service.
                secure_profile(profile)?;
                system::trusted_folder(root).map(|()| None)
            }
            Ok(()) => Ok(None),
        };
    }
    // No service vouches for this folder (left by another tool, an old
    // uninstall or anyone who could create it): it is kept only when no one
    // else owns or may change anything in it, so nothing can be added after
    // the check either. Otherwise its settings and pairings are not adopted:
    // it moves aside unchanged (#61). The host checks the Nucleus copies it
    // lets users write itself.
    let opaque = [nucleus_user_root(root)];
    let error = match system::trusted_tree(root, &opaque) {
        Ok(()) => return Ok(None),
        Err(error) if error.downcast_ref::<system::Untrusted>().is_some() => error,
        Err(error) => return Err(error),
    };
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let name = root.file_name().context("profile root has no name")?;
    // Another move in the same second took the first name.
    let aside = (0..100)
        .map(|n| {
            let suffix = if n == 0 {
                String::new()
            } else {
                format!("-{n}")
            };
            root.with_file_name(format!("{}.untrusted-{seconds}{suffix}", name.display()))
        })
        .find(|aside| std::fs::symlink_metadata(aside).is_err())
        .context("no free name to move the profile to")?;
    line(format!(
        "moving {} to {}: {error:#}",
        root.display(),
        aside.display()
    ));
    std::fs::rename(root, &aside)
        .with_context(|| format!("{error:#}; moving it to {} failed", aside.display()))?;
    match system::create_restricted(root, true) {
        Ok(()) => Ok(Some(aside)),
        Err(error) => Err(moved_aside(error, &aside)),
    }
}

/// Copy the package, replacing files still loaded by other programs (the
/// Vulkan layer inside a running game) by renaming them first.
fn copy_package(staging: &Path, install: &Path, entries: &[payload::Entry]) -> Result<()> {
    std::fs::create_dir_all(install)?;
    if let Ok(previous) = payload::manifest(install) {
        for old in previous {
            if !entries
                .iter()
                .any(|e| e.path.eq_ignore_ascii_case(&old.path))
                && let Ok(path) = payload::safe_join(install, &old.path)
            {
                system::remove_file_later(&path);
            }
        }
    }
    for entry in entries {
        let source = payload::safe_join(staging, &entry.path)?;
        let target = payload::safe_join(install, &entry.path)?;
        replace_file(&source, &target)?;
    }
    replace_file(
        &staging.join("manifest.json"),
        &install.join("manifest.json"),
    )?;
    // Drivers reused from a previous installation are not in the manifest.
    let drivers = staging.join("drivers");
    if drivers.is_dir() {
        copy_tree(&drivers, &install.join("drivers"))?;
    }
    Ok(())
}
pub(crate) fn replace_file(source: &Path, target: &Path) -> Result<()> {
    crate::update_files::replace(source, target, system::remove_file_later)
}
fn copy_tree(source: &Path, target: &Path) -> Result<()> {
    std::fs::create_dir_all(target)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let destination = target.join(entry.file_name());
        if kind.is_dir() {
            copy_tree(&entry.path(), &destination)?;
        } else if kind.is_file() {
            replace_file(&entry.path(), &destination)?;
        }
    }
    Ok(())
}
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
fn run_drivers(
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
    if gamepad && install.join("drivers\\gamepad\\install.ps1").is_file() {
        progress.set("Installing the virtual gamepad driver…");
        restart_needed |= run_driver_script(
            &install.join("drivers\\gamepad\\install.ps1"),
            &["-InstallerBestEffort", "-AllowLocalTestCertificate:0"],
            notes,
            "virtual gamepad",
        );
    }

    if usbip {
        let script = install.join("drivers\\usbip\\install.ps1");
        if script.is_file() {
            progress.set("Installing Steam Deck and DualSense USB audio and haptics…");
            restart_needed |= run_driver_script(&script, &[], notes, "USB audio and haptics");
        } else {
            notes.push("The USB audio and haptics package is missing. Run the complete Rubylight installer to add it.".into());
        }
    }
    restart_needed
}

/// Rubylight's display driver, then the removal of libvirtualdisplay it
/// replaces. Returns whether Windows must restart and how many notes the
/// driver's own step added (the removal's come after them).
fn display_step(install: &Path, progress: &Progress, notes: &mut Vec<String>) -> (bool, usize) {
    let before = notes.len();
    let mut restart_needed = rubylight_display_driver(install, progress, notes);
    let display_notes = notes.len() - before;
    // Rubylight's own driver replaced libvirtualdisplay. The old one goes
    // once the new device runs; where Windows refused the new driver or
    // its install failed, nothing uses the old one and it stays.
    if libvirtualdisplay_replaced(display_notes > 0, restart_needed) {
        restart_needed |= remove_libvirtualdisplay(install, progress, notes);
    }
    (restart_needed, display_notes)
}
/// The installer of Rubylight's own display driver in a package.
pub(crate) const RUBYLIGHT_DISPLAY_SCRIPT: &str = "drivers\\rubylight-display\\install-local.ps1";
/// Removes the libvirtualdisplay driver earlier versions installed.
pub(crate) const LIBVIRTUALDISPLAY_REMOVAL: &str =
    "drivers\\rubylight-display\\remove-libvirtualdisplay.ps1";
/// The driver's name in notes about that removal.
const LIBVIRTUALDISPLAY: &str = "old libvirtualdisplay";
/// Set up Rubylight's display driver (again, which also updates it). Returns
/// whether Windows needs a restart.
fn rubylight_display_driver(install: &Path, progress: &Progress, notes: &mut Vec<String>) -> bool {
    let script = install.join(RUBYLIGHT_DISPLAY_SCRIPT);
    if !script.is_file() {
        notes.push("The virtual display driver is missing from this package. Run the complete Rubylight installer to add it.".into());
        return false;
    }
    progress.set("Installing the virtual display driver…");
    run_driver_script(&script, &["-InstallerBestEffort"], notes, "virtual display")
}
/// Whether Rubylight's display driver step left a running device: no note
/// (a refusing policy, a failed install, a missing package) and no restart
/// still pending before the device starts.
fn libvirtualdisplay_replaced(noted: bool, restart_pending: bool) -> bool {
    !noted && !restart_pending
}
/// Remove libvirtualdisplay's device, driver packages and HDR layer, best
/// effort. Returns whether Windows needs a restart.
fn remove_libvirtualdisplay(install: &Path, progress: &Progress, notes: &mut Vec<String>) -> bool {
    let script = install.join(LIBVIRTUALDISPLAY_REMOVAL);
    if !script.is_file() {
        return false;
    }
    progress.set("Removing the old virtual display driver…");
    run_driver_script(&script, &["-InstallerBestEffort"], notes, LIBVIRTUALDISPLAY)
}
/// Run a Vibepollo driver script as SYSTEM. A failure is reported, not
/// fatal: the host still starts without the driver. Returns whether Windows
/// needs a restart.
fn run_driver_script(script: &Path, args: &[&str], notes: &mut Vec<String>, name: &str) -> bool {
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
fn driver_error_note(error: &anyhow::Error, name: &str) -> String {
    match error.downcast_ref::<system::StillRunning>() {
        Some(running) => format!(
            "The {name} driver was still being {} after {} s and goes on in the background. If it does not work once that finishes, run setup again.",
            action(name),
            running.seconds
        ),
        None => format!("The {name} driver could not be {}: {error:#}", action(name)),
    }
}
fn driver_result(code: i32, output: &str, notes: &mut Vec<String>, name: &str) -> bool {
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
        } else if name == LIBVIRTUALDISPLAY {
            "Setup tries again when it next runs; the setup log has the details."
        } else if name == "virtual gamepad" {
            "Run setup again; the setup log has the details."
        } else {
            // The service looks for the display driver every minute.
            "The Rubylight service tries again within a day, or run \"butterpollo-setup.exe --repair-drivers --now\" in an administrator shell; the setup log has the details."
        };
        notes.push(match reason {
            Some(reason) => format!(
                "The {name} driver could not be {}: {reason} {retry}",
                action(name)
            ),
            None => format!("The {name} driver reported a problem; see the setup log."),
        });
    }
    code == 3010 || output.contains("RESTART_REQUIRED") || output.contains("A reboot is required")
}
fn action(name: &str) -> &'static str {
    if name == LIBVIRTUALDISPLAY {
        "removed"
    } else {
        "set up"
    }
}
/// The one driver setup at a time, waited for up to `wait`. Without it the
/// driver scripts run anyway: a stale lock must not keep drivers out.
fn driver_lock(wait: Duration) -> Option<std::fs::File> {
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
const REPAIR_RECORD: &str = "driver-repair.json";
/// What the log tells a person when the service's repair was held back.
const REPAIR_BY_HAND: &str = "To try again now, run \"butterpollo-setup.exe --repair-drivers --now\" from the Rubylight folder in an administrator shell.";
/// A new installation may repair the display driver again, unless it was
/// installed with --no-display-driver.
fn remember_driver_choice(profile: &Path, display_driver: bool) {
    let record = profile.join(REPAIR_RECORD);
    let _ = if display_driver {
        std::fs::remove_file(&record)
    } else {
        std::fs::create_dir_all(profile)
            .and_then(|()| std::fs::write(&record, br#"{"declined":true}"#))
    };
}
/// `butterpollo-setup.exe --repair-drivers`, which the service runs when its
/// host has had no virtual display driver for a minute. An old host's
/// uninstaller, an in-app update before rc.27 or a failed driver step leaves
/// it missing, and Artemis then reports the "SudoVDA" driver as not
/// installed. Sets the display driver up again from this installation, at
/// most once a day per version and never during a stream; the driver script
/// restarts the service. By default (the service's run) the daily limit and
/// the --no-display-driver choice apply and only a positive idle answer from
/// the host lets it go on. `--now`, `by_hand`, is a person asking: neither
/// limit applies, so a failed automatic attempt cannot lock them out for the
/// day, and a host that hides its counts (a LAN bind_address) does not stop
/// them; a stream still does.
/// Returns whether Windows must restart.
pub fn repair_drivers(progress: &Progress, by_hand: bool) -> Result<bool> {
    let exe = std::env::current_exe()?;
    let install = system::win32_path(exe.parent().context("setup folder unavailable")?)?;
    let profile = profile();
    let record = profile.join(REPAIR_RECORD);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();
    let version = env!("CARGO_PKG_VERSION");
    if let Some(reason) = repair_skipped(
        std::fs::read(&record).ok().as_deref(),
        version,
        now,
        by_hand,
    ) {
        line(format!(
            "not repairing the display driver: {reason}. {REPAIR_BY_HAND}"
        ));
        return Ok(false);
    }
    // Setup or an update that is installing the drivers right now restarts
    // the service when done, and the service checks again.
    let Some(_turn) = driver_lock(Duration::ZERO) else {
        return when_locked(by_hand);
    };
    let address = probe(&profile);
    if let Err(error) = ensure_idle_positive(
        &serverinfo(address),
        || system::process_running("butterpollo.exe"),
        by_hand,
    ) {
        if by_hand {
            return Err(error);
        }
        line(format!(
            "not repairing the display driver now: {error:#}. The service asks again later. {REPAIR_BY_HAND}"
        ));
        return Ok(false);
    }
    std::fs::write(
        &record,
        serde_json::to_vec(&serde_json::json!({"version": version, "unix": now}))?,
    )?;
    let mut notes = Vec::new();
    // Automatic display repair never installs an optional USB/IP driver.
    let (restart, display_notes) = display_step(&install, progress, &mut notes);
    for note in &notes {
        line(format!("note: {note}"));
    }
    display_failed(&notes, display_notes)?;
    Ok(restart)
}
/// Another setup holds the driver lock. It restarts the service when done and
/// the service checks again, so the service's run ends quietly; a person who
/// asked is told nothing was repaired.
fn when_locked(by_hand: bool) -> Result<bool> {
    if by_hand {
        bail!("Setup is installing the drivers right now; try again when it finishes.");
    }
    Ok(false)
}
/// A note about the display step is a step that failed or was refused: say so
/// rather than "done". The first `display_notes` notes are that step's; a
/// warning about removing the old libvirtualdisplay stays a plain note.
fn display_failed(notes: &[String], display_notes: usize) -> Result<()> {
    if display_notes > 0 {
        bail!("{}", notes[..display_notes.min(notes.len())].join("\n"));
    }
    Ok(())
}
/// Whether setup ran with --no-display-driver.
pub(crate) fn display_driver_declined() -> bool {
    std::fs::read(profile().join(REPAIR_RECORD))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .is_some_and(|record| record["declined"] == true)
}
fn repair_skipped(
    record: Option<&[u8]>,
    version: &str,
    now: u64,
    by_hand: bool,
) -> Option<&'static str> {
    if by_hand {
        return None;
    }
    let record: serde_json::Value = serde_json::from_slice(record?).ok()?;
    if record["declined"] == true {
        return Some("it was left out with --no-display-driver");
    }
    (record["version"] == version
        && record["unix"]
            .as_u64()
            .is_some_and(|at| now.saturating_sub(at) < 86_400))
    .then_some("this version already tried today")
}
fn remove_legacy(product: &crate::detect::Product) -> Result<()> {
    let (command, in_place) = legacy_uninstall(product, is_nsis).context("no uninstall command")?;
    let (code, _) = system::run_cmd(&command, Duration::from_secs(300))?;
    if code != 0 {
        bail!("its uninstaller exited with {code}");
    }
    // Run in place, an NSIS uninstaller cannot delete itself.
    if let Some(program) = in_place {
        let _ = std::fs::remove_file(&program);
        if let Some(folder) = program.parent() {
            let _ = std::fs::remove_dir(folder);
        }
    }
    Ok(())
}
/// The command that removes a legacy host, and the uninstaller it runs in
/// place. An NSIS uninstaller (Apollo, Sunshine, Vibepollo 1) copies itself
/// to %TEMP%, starts the copy and exits at once: setup went on while the
/// copy still had to stop and delete ApolloService, the service Rubylight
/// reuses, and remove drivers. With "_?=<folder>", always last and
/// unquoted, it runs in place and setup waits for it.
fn legacy_uninstall(
    product: &crate::detect::Product,
    nsis: impl Fn(&Path) -> bool,
) -> Option<(String, Option<PathBuf>)> {
    let command = product
        .quiet_uninstall
        .clone()
        .or_else(|| product.uninstall.clone().map(|u| format!("{u} /S")))?;
    let program = detect::program(&command).filter(|program| nsis(program));
    Some(match program {
        Some(program) if !command.contains("_?=") => {
            let folder = program.parent()?.display().to_string();
            (format!("{command} _?={folder}"), Some(program))
        }
        _ => (command, None),
    })
}
/// Whether a program is an NSIS installer or uninstaller: its data follows
/// the executable with this signature.
fn is_nsis(program: &Path) -> bool {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(program)
        .and_then(|file| file.take(16 << 20).read_to_end(&mut bytes))
        .is_ok()
        && bytes.windows(12).any(|window| window == b"NullsoftInst")
}
pub(crate) fn register(install: &Path, entries: &[payload::Entry]) -> Result<()> {
    // The updater uses canonical paths for file identity and backups. Do not
    // leak their verbatim prefix into the next manual install or shell entry.
    let install = system::win32_path(install)?;
    let size_kb: u64 = entries
        .iter()
        .filter_map(|e| std::fs::metadata(install.join(&e.path)).ok())
        .map(|m| m.len())
        .sum::<u64>()
        / 1024;
    let location = format!("{}\\", install.display());
    let uninstaller = install.join("uninstall.exe").display().to_string();
    let uninstall = format!("\"{uninstaller}\" --uninstall");
    let quiet = format!("\"{uninstaller}\" --uninstall --quiet");
    let icon = install.join("butterpollo.exe").display().to_string();
    system::write_key(
        HKEY_LOCAL_MACHINE,
        &format!("{UNINSTALL}\\Butterpollo"),
        &[
            ("DisplayName", Value::Text("Rubylight")),
            ("DisplayVersion", Value::Text(env!("CARGO_PKG_VERSION"))),
            ("Publisher", Value::Text("Rubylight")),
            ("InstallLocation", Value::Text(&location)),
            ("DisplayIcon", Value::Text(&icon)),
            ("UninstallString", Value::Text(&uninstall)),
            ("QuietUninstallString", Value::Text(&quiet)),
            (
                "URLInfoAbout",
                Value::Text("https://github.com/RamazanKara/Rubylight"),
            ),
            ("NoModify", Value::Number(1)),
            ("NoRepair", Value::Number(1)),
            (
                "EstimatedSize",
                Value::Number(size_kb.min(u64::from(u32::MAX)) as u32),
            ),
        ],
    )
}
/// The service runs as SYSTEM from the install folder. Program Files lets
/// only administrators change it; a folder elsewhere (D:\Rubylight) would
/// let any user replace its programs, so it gets the same rights.
pub(crate) fn secure_install(install: &Path) -> Result<()> {
    system::restrict(install, true)?;
    system::inherit_contents(install)
}
/// Whether the host at `profile`'s address reports a stream, a queued
/// launch or a running app.
pub(crate) fn streaming(profile: &Path) -> bool {
    serverinfo(probe(profile)).is_ok_and(|response| check_idle(&response).is_err())
}
/// Refuse to stop a host that reports a stream, a queued launch or a running
/// app, unless the user chose to end them (`end_streams`). A host that cannot
/// say is stopped as before: none is running (Windows refuses a closed
/// loopback port only after about two seconds), it is Vibepollo, Apollo or
/// Sunshine, or it was asked on a LAN bind_address, where a Rust host leaves
/// its counts blank.
pub(crate) fn ensure_idle(address: SocketAddr, end_streams: bool) -> Result<()> {
    match serverinfo(address) {
        Ok(response) => match check_idle(&response) {
            Err(_) if end_streams => {
                line("the host is streaming; ending the stream as asked");
                Ok(())
            }
            result => result,
        },
        Err(error) => {
            line(format!("no host status at {address} ({error}); continuing"));
            Ok(())
        }
    }
}
fn serverinfo(address: SocketAddr) -> std::io::Result<String> {
    use std::io::{Read, Write};
    let mut socket = std::net::TcpStream::connect_timeout(&address, Duration::from_millis(500))?;
    socket.set_read_timeout(Some(Duration::from_secs(3)))?;
    socket.set_write_timeout(Some(Duration::from_secs(3)))?;
    write!(
        socket,
        "GET /serverinfo HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n"
    )?;
    // A reset or timeout after the answer arrived still leaves the counts.
    let mut response = Vec::new();
    let read = socket.take(65536).read_to_end(&mut response);
    if response.is_empty() {
        read?;
    }
    Ok(String::from_utf8_lossy(&response).into_owned())
}

const IDLE_FIELDS: [&str; 3] = [
    "RustHostSessionCount",
    "RustHostPendingSessionCount",
    "RustHostApplicationActive",
];
fn host_count(response: &str, field: &str) -> Option<u32> {
    response
        .split_once(&format!("<{field}>"))
        .and_then(|(_, value)| value.split_once(&format!("</{field}>")))
        .and_then(|(value, _)| value.trim().parse::<u32>().ok())
}
fn check_idle(response: &str) -> Result<()> {
    for field in IDLE_FIELDS {
        if host_count(response, field).is_some_and(|count| count != 0) {
            bail!(
                "Disconnect all streams and close games started from Moonlight before installing, then run setup again. Nothing has been changed."
            );
        }
    }
    Ok(())
}
/// The repair's idle check: proceeds only on a positive idle answer, every
/// count present and 0. A host that does not answer, or answers with blank
/// counts (it blanks them for a caller that is not on loopback), may be
/// streaming and counts as busy. Only a host process that is not running at
/// all leaves nothing to interrupt. `host_running` says whether one is.
/// `blank_ok` is for a person who asked (`--now`): a host on a LAN
/// bind_address blanks its counts for every caller, this PC included, and
/// would otherwise never be repairable. Counts that are there still count.
fn ensure_idle_positive(
    answer: &std::io::Result<String>,
    host_running: impl FnOnce() -> bool,
    blank_ok: bool,
) -> Result<()> {
    match answer {
        Ok(response) => {
            check_idle(response)?;
            if !blank_ok
                && IDLE_FIELDS
                    .iter()
                    .any(|field| host_count(response, field).is_none())
            {
                bail!("the host does not report whether it is streaming");
            }
            Ok(())
        }
        Err(error) if host_running() => {
            bail!("the host is running but does not answer ({error})")
        }
        Err(_) => Ok(()),
    }
}
/// Wait until the host answers serverinfo at `address` as a Rust host.
pub(crate) fn wait_ready(address: SocketAddr, version: Option<&str>) -> Result<()> {
    use std::io::{Read, Write};
    let host = match address {
        SocketAddr::V4(address) => address.ip().to_string(),
        SocketAddr::V6(address) => format!("[{}]", address.ip()),
    };
    let request = format!("GET /serverinfo HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    let deadline = Instant::now() + Duration::from_secs(90);
    while Instant::now() < deadline {
        if let Ok(mut socket) =
            std::net::TcpStream::connect_timeout(&address, Duration::from_millis(500))
        {
            let _ = socket.set_read_timeout(Some(Duration::from_secs(20)));
            let _ = socket.write_all(request.as_bytes());
            let mut response = String::new();
            let _ = socket.read_to_string(&mut response);
            if version.map_or_else(
                || response.contains("<RustHostVersion>"),
                |version| {
                    response.contains(&format!("<RustHostVersion>{version}</RustHostVersion>"))
                },
            ) {
                line("the host is answering");
                return Ok(());
            }
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    bail!("Rubylight did not start within 90 seconds; see logs\\service.log in the profile")
}

#[cfg(test)]
mod tests {
    /// 2.4.2 trusted the root only on a fresh install; an update, the way
    /// every existing host got 2.4.2, left the console untrusted.
    #[test]
    fn every_start_of_the_new_host_trusts_the_console_root() {
        let ready = "Some(env!(\"CARGO_PKG_VERSION\")))?;";
        for (name, source) in [
            ("install.rs", include_str!("install.rs")),
            ("update.rs", include_str!("update.rs")),
        ] {
            let code = source.split("#[cfg(test)]").next().unwrap();
            let starts: Vec<_> = code.match_indices(ready).collect();
            assert!(!starts.is_empty(), "{name}");
            for (at, _) in starts {
                let after: String = code[at..].lines().skip(1).take(4).collect();
                assert!(after.contains("trust_console"), "{name}: {after}");
            }
        }
    }
    #[test]
    fn setup_removes_the_root_by_the_name_the_host_gives_it() {
        assert_eq!(
            super::CONSOLE_ROOT_NAME,
            butterpollo_core::crypto::CONSOLE_ROOT_NAME
        );
    }
    use super::*;
    #[test]
    fn vibepollos_nvidia_undo_runs_only_with_its_journal_and_program() {
        let base = std::env::temp_dir().join(format!("nvprefs-undo-{}", std::process::id()));
        let (data, previous) = (base.join("data"), base.join("previous"));
        std::fs::create_dir_all(data.join("Sunshine")).unwrap();
        std::fs::create_dir_all(&previous).unwrap();
        assert!(!nvprefs_undo_pending(&data, &previous));
        std::fs::write(data.join("Sunshine\\nvprefs_undo.json"), "{}").unwrap();
        assert!(!nvprefs_undo_pending(&data, &previous));
        std::fs::write(previous.join("sunshine.exe"), "").unwrap();
        assert!(nvprefs_undo_pending(&data, &previous));
        std::fs::remove_file(data.join("Sunshine\\nvprefs_undo.json")).unwrap();
        assert!(!nvprefs_undo_pending(&data, &previous));
        std::fs::remove_dir_all(&base).unwrap();
    }
    #[test]
    fn driver_reboot_exit_codes_are_successful_but_warnings_are_shown() {
        let mut notes = Vec::new();
        assert!(driver_result(3010, "", &mut notes, "display"));
        assert!(notes.is_empty());
        assert!(!driver_result(0, "", &mut notes, "gamepad"));
        assert!(!driver_result(
            0,
            "VIRTUAL_DISPLAY_DRIVER_WARNING",
            &mut notes,
            "display"
        ));
        assert_eq!(notes.len(), 1);
        assert!(!driver_result(1, "failed", &mut notes, "gamepad"));
        assert_eq!(notes.len(), 2);
        // The script's own reason reaches the note.
        driver_result(
            0,
            "WARNING: [SunshineVirtualDisplay] VIRTUAL_DISPLAY_DRIVER_WARNING: Optional virtual display driver setup did not complete.\nWARNING: [SunshineVirtualDisplay] Installer best-effort driver action failed: nefconc.exe failed with exit code 5.\n",
            &mut notes,
            "virtual display",
        );
        assert!(
            notes[2].starts_with(
                "The virtual display driver could not be set up: nefconc.exe failed with exit code 5."
            ),
            "{}",
            notes[2]
        );
        assert!(driver_result(
            1,
            "RESTART_REQUIRED\nUSBIP_DRIVER_WARNING: Restart Windows or repair USBip separately.",
            &mut notes,
            "USB audio and haptics",
        ));
        assert!(notes[3].contains("Run setup again with USB audio and haptics selected"));
        assert!(!notes[3].contains("when its service starts"));
    }
    #[test]
    fn a_driver_script_past_the_deadline_is_not_reported_as_failed() {
        let late = anyhow::Error::from(system::StillRunning {
            program: "powershell.exe".into(),
            seconds: 600,
        });
        let note = driver_error_note(&late, "USB audio and haptics");
        assert!(
            note.starts_with(
                "The USB audio and haptics driver was still being set up after 600 s and goes on in the background."
            ),
            "{note}"
        );
        assert!(!note.contains("could not"), "{note}");
        // The task's own log is removed at the deadline, so its result
        // never reaches the setup log.
        assert!(!note.contains("setup log"), "{note}");
        let late = late.context("waiting for the task");
        assert!(driver_error_note(&late, LIBVIRTUALDISPLAY).contains("still being removed"));
        let failed = anyhow::anyhow!("creating the setup task failed (1)");
        assert_eq!(
            driver_error_note(&failed, "virtual gamepad"),
            "The virtual gamepad driver could not be set up: creating the setup task failed (1)"
        );
    }
    #[test]
    fn nsis_uninstallers_run_in_place_so_setup_waits_for_them() -> Result<()> {
        let product = |uninstall: Option<&str>, quiet: Option<&str>| detect::Product {
            key: "Apollo".into(),
            name: "Apollo".into(),
            version: "0.4.6".into(),
            location: None,
            uninstall: uninstall.map(Into::into),
            quiet_uninstall: quiet.map(Into::into),
            msi: false,
            root: HKEY_LOCAL_MACHINE,
        };
        let apollo = product(Some(r#""C:\Program Files\Apollo\Uninstall.exe""#), None);
        assert_eq!(
            legacy_uninstall(&apollo, |_| true),
            Some((
                r#""C:\Program Files\Apollo\Uninstall.exe" /S _?=C:\Program Files\Apollo"#.into(),
                Some(PathBuf::from(r"C:\Program Files\Apollo\Uninstall.exe"))
            ))
        );
        // Other uninstallers, and a command that already runs in place, are
        // run as registered.
        assert_eq!(
            legacy_uninstall(&apollo, |_| false),
            Some((r#""C:\Program Files\Apollo\Uninstall.exe" /S"#.into(), None))
        );
        let quiet = product(None, Some(r"C:\Apollo\Uninstall.exe /S _?=C:\Apollo"));
        assert_eq!(
            legacy_uninstall(&quiet, |_| true),
            Some((r"C:\Apollo\Uninstall.exe /S _?=C:\Apollo".into(), None))
        );
        assert_eq!(legacy_uninstall(&product(None, None), |_| true), None);

        let folder = tempfile::tempdir()?;
        let nsis = folder.path().join("Uninstall.exe");
        std::fs::write(
            &nsis,
            [&b"MZ"[..], &[0; 4096], b"\xef\xbe\xad\xdeNullsoftInst"].concat(),
        )?;
        let other = folder.path().join("unins000.exe");
        std::fs::write(&other, [&b"MZ"[..], &[0; 4096], b"Inno Setup"].concat())?;
        assert!(is_nsis(&nsis));
        assert!(!is_nsis(&other));
        assert!(!is_nsis(&folder.path().join("missing.exe")));
        Ok(())
    }
    #[test]
    fn a_pc_that_refuses_the_locally_signed_driver_gets_a_note_naming_why() {
        let mut notes = Vec::new();
        let output = "WARNUNG: [RubylightVirtualDisplay] DRIVER_UNSUPPORTED: This PC refuses a display driver signed on the PC (Smart App Control is on). A signed Rubylight release installs it; until then streams use a physical display.\n";
        assert!(!driver_result(0, output, &mut notes, "virtual display"));
        assert_eq!(
            notes,
            [
                "The virtual display driver was not installed: This PC refuses a display driver signed on the PC (Smart App Control is on). A signed Rubylight release installs it; until then streams use a physical display."
            ]
        );
    }
    #[test]
    fn libvirtualdisplay_goes_only_once_rubylights_device_runs() {
        assert!(libvirtualdisplay_replaced(false, false));
        // Smart App Control, S mode, App Control or a failed install: the
        // driver step added a note, and the old driver stays.
        let mut notes = Vec::new();
        let refused = "WARNING: [RubylightVirtualDisplay] DRIVER_UNSUPPORTED: Smart App Control is on, and it refuses a display driver signed on this PC, so streams use a physical display.\n";
        let restart = driver_result(0, refused, &mut notes, "virtual display");
        assert!(!libvirtualdisplay_replaced(!notes.is_empty(), restart));
        let mut notes = Vec::new();
        let failed = "WARNING: [RubylightVirtualDisplay] DRIVER_WARNING: Rubylight display driver setup did not complete.\nWARNING: [RubylightVirtualDisplay] Installer best-effort driver action failed: Windows did not start the display driver (Error, problem 31)\n";
        let restart = driver_result(0, failed, &mut notes, "virtual display");
        assert!(!libvirtualdisplay_replaced(!notes.is_empty(), restart));
        // Installed, but the device starts only after a restart.
        let mut notes = Vec::new();
        let restart = driver_result(3010, "RESTART_REQUIRED\n", &mut notes, "virtual display");
        assert!(!libvirtualdisplay_replaced(!notes.is_empty(), restart));
    }
    #[test]
    fn a_failed_libvirtualdisplay_removal_is_a_note_about_removing_it() {
        let mut notes = Vec::new();
        let output = "WARNING: [RubylightSetup] DRIVER_WARNING: The old libvirtualdisplay driver was not removed completely.\nWARNING: [RubylightSetup] Installer best-effort driver action failed: pnputil /delete-driver oem7.inf /uninstall /force failed with exit code 5\n";
        assert!(!driver_result(0, output, &mut notes, LIBVIRTUALDISPLAY));
        assert_eq!(
            notes,
            [
                "The old libvirtualdisplay driver could not be removed: pnputil /delete-driver oem7.inf /uninstall /force failed with exit code 5 Setup tries again when it next runs; the setup log has the details."
            ]
        );
        // A display still in use goes at the next restart.
        let mut notes = Vec::new();
        assert!(driver_result(
            0,
            "RESTART_REQUIRED\n",
            &mut notes,
            LIBVIRTUALDISPLAY
        ));
        assert!(notes.is_empty());
    }
    #[test]
    fn a_failed_rubylight_display_install_becomes_a_setup_note() {
        let mut notes = Vec::new();
        let output = "WARNING: [RubylightVirtualDisplay] DRIVER_WARNING: Rubylight display driver setup did not complete.\nWARNING: [RubylightVirtualDisplay] Installer best-effort driver action failed: pnputil /add-driver failed with exit code 5\n";
        assert!(!driver_result(0, output, &mut notes, "Rubylight display"));
        assert_eq!(
            notes,
            [
                "The Rubylight display driver could not be set up: pnputil /add-driver failed with exit code 5 The Rubylight service tries again within a day, or run \"butterpollo-setup.exe --repair-drivers --now\" in an administrator shell; the setup log has the details."
            ]
        );
        let mut notes = Vec::new();
        assert!(driver_result(
            0,
            "RESTART_REQUIRED\n",
            &mut notes,
            "Rubylight display"
        ));
        assert!(notes.is_empty());
    }
    #[test]
    fn the_display_driver_is_repaired_once_a_day_per_version_unless_declined() {
        let day = 86_400;
        let record = |text: &str| Some(text.as_bytes().to_vec());
        let skipped =
            |text: Option<Vec<u8>>, now| repair_skipped(text.as_deref(), "2.0.0-rc.27", now, false);
        // A person running the repair (--now) is never held back by an
        // automatic attempt that failed earlier the same day, nor by a
        // declined choice. The default, which the service runs, is limited.
        for text in [
            r#"{"version":"2.0.0-rc.27","unix":86400}"#,
            r#"{"declined":true}"#,
        ] {
            assert_eq!(
                repair_skipped(Some(text.as_bytes()), "2.0.0-rc.27", day + 60, true),
                None
            );
        }
        assert_eq!(skipped(None, day), None);
        assert_eq!(skipped(record("not json"), day), None);
        let tried = record(r#"{"version":"2.0.0-rc.27","unix":86400}"#);
        assert!(skipped(tried.clone(), day + 60).is_some());
        assert_eq!(skipped(tried, 2 * day), None);
        assert_eq!(
            skipped(
                record(r#"{"version":"2.0.0-rc.26","unix":86400}"#),
                day + 60
            ),
            None
        );
        assert!(skipped(record(r#"{"declined":true}"#), day).is_some());
    }
    #[test]
    fn the_unattended_repair_needs_a_positive_idle_answer() {
        let idle = |counts: [&str; 3]| {
            Ok(format!(
                "<RustHostSessionCount>{}</RustHostSessionCount><RustHostPendingSessionCount>{}</RustHostPendingSessionCount><RustHostApplicationActive>{}</RustHostApplicationActive>",
                counts[0], counts[1], counts[2]
            ))
        };
        let refused = |answer: std::io::Result<String>, running| {
            ensure_idle_positive(&answer, || running, false).is_err()
        };
        assert!(!refused(idle(["0", "0", "0"]), true));
        // A stream, a queued launch or a running app.
        assert!(refused(idle(["1", "0", "0"]), true));
        assert!(refused(idle(["0", "1", "0"]), true));
        assert!(refused(idle(["0", "0", "1"]), true));
        // A LAN caller sees blank counts, an old host none: it may stream.
        assert!(refused(idle(["", "", ""]), false));
        assert!(refused(idle(["0", "0", ""]), false));
        assert!(refused(Ok("<root/>".into()), false));
        assert!(refused(Ok(String::new()), false));
        // No answer: busy while a host process runs, idle when none does.
        let silent = || Err(std::io::Error::from(std::io::ErrorKind::TimedOut));
        assert!(refused(silent(), true));
        assert!(!refused(silent(), false));
        // --now refuses the same streams and a host that runs but is silent,
        // but takes blank counts (a LAN bind_address blanks them for this
        // PC too), which would otherwise leave that host unrepairable.
        let by_hand = |answer: std::io::Result<String>, running| {
            ensure_idle_positive(&answer, || running, true).is_err()
        };
        assert!(by_hand(idle(["1", "0", "0"]), true));
        assert!(by_hand(idle(["0", "0", "1"]), true));
        assert!(by_hand(silent(), true));
        assert!(!by_hand(silent(), false));
        assert!(!by_hand(idle(["0", "0", "0"]), true));
        assert!(!by_hand(idle(["", "", ""]), true));
    }
    #[test]
    fn a_repair_that_finds_the_driver_lock_taken_does_not_report_success() {
        // The service's run waits for the service to be restarted and asks
        // again; a person is told nothing was repaired.
        assert!(!when_locked(false).unwrap());
        let error = when_locked(true).unwrap_err().to_string();
        assert_eq!(
            error,
            "Setup is installing the drivers right now; try again when it finishes."
        );
    }
    #[test]
    fn only_the_display_drivers_own_notes_fail_the_repair() {
        let notes = [
            "The virtual display driver could not be set up: pnputil failed".to_owned(),
            "The old libvirtualdisplay driver could not be removed: busy".to_owned(),
        ];
        // The display step added the first one: that is the failure.
        let error = display_failed(&notes, 1).unwrap_err().to_string();
        assert!(error.contains("could not be set up"), "{error}");
        assert!(!error.contains("libvirtualdisplay"), "{error}");
        // Only the removal warned: the driver is repaired, the note stays.
        assert!(display_failed(&notes[1..], 0).is_ok());
        assert!(display_failed(&[], 0).is_ok());
    }
    #[test]
    fn setup_refuses_streams_pending_connections_and_apps() {
        for counts in [[0, 0, 0], [1, 0, 0], [0, 1, 0], [0, 0, 1]] {
            let response = format!(
                "<RustHostSessionCount>{}</RustHostSessionCount><RustHostPendingSessionCount>{}</RustHostPendingSessionCount><RustHostApplicationActive>{}</RustHostApplicationActive>",
                counts[0], counts[1], counts[2]
            );
            assert_eq!(check_idle(&response).is_ok(), counts == [0, 0, 0]);
        }
        // Older hosts, and a Rust host asked on its LAN bind_address, cannot
        // say; they are stopped as before rather than blocking setup forever.
        assert!(check_idle("<state>SUNSHINE_SERVER_FREE</state>").is_ok());
        assert!(check_idle("<RustHostVersion>2.0.0-rc.21</RustHostVersion><RustHostSessionCount></RustHostSessionCount><RustHostPendingSessionCount></RustHostPendingSessionCount><RustHostApplicationActive></RustHostApplicationActive>").is_ok());
        assert!(check_idle("").is_ok());
    }
    #[test]
    fn a_stopped_or_unresponsive_host_does_not_block_setup() -> Result<()> {
        // Windows refuses a closed loopback port only after about two seconds.
        let closed = std::net::TcpListener::bind("127.0.0.1:0")?.local_addr()?;
        ensure_idle(closed, false)?;
        let silent = std::net::TcpListener::bind("127.0.0.1:0")?;
        ensure_idle(silent.local_addr()?, false)?;
        // A busy host is still refused over a real connection, unless the
        // user chose to end its stream.
        let busy = std::net::TcpListener::bind("127.0.0.1:0")?;
        let address = busy.local_addr()?;
        let server = std::thread::spawn(move || -> std::io::Result<()> {
            use std::io::{Read, Write};
            for _ in 0..2 {
                let (mut socket, _) = busy.accept()?;
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    let mut byte = [0];
                    socket.read_exact(&mut byte)?;
                    request.push(byte[0]);
                }
                socket.write_all(b"HTTP/1.1 200 OK\r\n\r\n<root><RustHostSessionCount>1</RustHostSessionCount></root>")?;
            }
            Ok(())
        });
        assert!(ensure_idle(address, false).is_err());
        ensure_idle(address, true)?;
        server.join().unwrap()?;
        Ok(())
    }
    #[test]
    fn upgrades_repair_legacy_profile_ownership_and_preserve_settings() -> Result<()> {
        if !system::elevated() {
            return Ok(());
        }
        let temp = tempfile::tempdir_in(system::program_data())?;
        system::restrict(temp.path(), true)?;
        let root = temp.path().join("Butterpollo");
        let profile = root.join("config");
        std::fs::create_dir_all(profile.join("credentials"))?;
        let settings = profile.join("sunshine.conf");
        std::fs::write(&settings, b"port=48123\n")?;
        let (_, identity) = system::run(
            &system::system32("whoami.exe"),
            &["/user", "/fo", "csv", "/nh"],
            Duration::from_secs(10),
        )?;
        let sid = identity
            .trim()
            .split(',')
            .nth(1)
            .context("whoami omitted SID")?
            .trim_matches('"');
        for path in [&root, &settings] {
            let (code, output) = system::run(
                &system::system32("icacls.exe"),
                &[
                    &path.display().to_string(),
                    "/setowner",
                    &format!("*{sid}"),
                    "/Q",
                ],
                Duration::from_secs(10),
            )?;
            assert_eq!(code, 0, "{output}");
            let (code, output) = system::run(
                &system::system32("icacls.exe"),
                &[
                    &path.display().to_string(),
                    "/grant:r",
                    "*S-1-1-0:(F)",
                    "/Q",
                ],
                Duration::from_secs(10),
            )?;
            assert_eq!(code, 0, "{output}");
        }
        assert_eq!(prepare_profile(&profile, true)?, None);
        system::trusted_ancestors(&root)?;
        system::trusted_folder(&settings)?;
        assert_eq!(std::fs::read(&settings)?, b"port=48123\n");
        let credentials = profile.join("credentials");
        for (path, sid) in [
            (&root, "*S-1-1-0"),
            (&settings, "*S-1-1-0"),
            (&credentials, "*S-1-5-32-545"),
        ] {
            let (_, output) = system::run(
                &system::system32("icacls.exe"),
                &[&path.display().to_string(), "/findsid", sid, "/Q"],
                Duration::from_secs(10),
            )?;
            assert!(!output.contains(&path.display().to_string()), "{output}");
        }
        Ok(())
    }
    #[test]
    fn credentials_are_locked_again_when_resetting_the_profile_fails() -> Result<()> {
        // The reset makes the credentials inherit the profile's read access
        // for users; a file it cannot reset must not leave them that way.
        let temp = tempfile::tempdir()?;
        let profile = temp.path().join("Butterpollo").join("config");
        let mut restricted = Vec::new();
        let error = secure_profile_with(
            &profile,
            |folder, users_read| {
                restricted.push((folder.to_owned(), users_read));
                Ok(())
            },
            |_| bail!("resetting permissions failed (5)"),
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("resetting permissions failed"));
        assert_eq!(
            restricted,
            [
                (temp.path().join("Butterpollo"), true),
                (profile.join("credentials"), false)
            ]
        );
        Ok(())
    }
    #[test]
    fn legacy_profile_repair_rejects_reparse_points_and_untrusted_parents() -> Result<()> {
        if !system::elevated() {
            return Ok(());
        }
        let temp = tempfile::tempdir_in(system::program_data())?;
        system::restrict(temp.path(), true)?;
        let outside = temp.path().join("outside");
        std::fs::create_dir(&outside)?;
        let settings = outside.join("sunshine.conf");
        std::fs::write(&settings, b"unchanged")?;
        let linked = temp.path().join("linked");
        std::os::windows::fs::symlink_dir(&outside, &linked)?;
        for profile in [linked.join("config"), linked.join("Butterpollo/config")] {
            let error = prepare_profile(&profile, true).unwrap_err();
            assert!(error.to_string().contains("reparse point"), "{error:#}");
        }
        let profile = temp.path().join("Butterpollo/config");
        std::fs::create_dir_all(&profile)?;
        std::os::windows::fs::symlink_dir(&outside, profile.join("credentials"))?;
        let error = prepare_profile(&profile, true).unwrap_err();
        assert!(error.to_string().contains("reparse point"), "{error:#}");
        assert_eq!(std::fs::read(&settings)?, b"unchanged");
        assert!(!outside.join("config").exists());
        assert!(!outside.join("Butterpollo").exists());
        let untrusted = temp.path().join("untrusted");
        std::fs::create_dir(&untrusted)?;
        // Not an administrator's: an administrator's account may own a parent.
        let (code, output) = system::run(
            &system::system32("icacls.exe"),
            &[
                &untrusted.display().to_string(),
                "/setowner",
                "*S-1-5-32-545",
                "/Q",
            ],
            Duration::from_secs(10),
        )?;
        assert_eq!(code, 0, "{output}");
        for updating in [true, false] {
            let error =
                prepare_profile(&untrusted.join("Butterpollo/config"), updating).unwrap_err();
            assert!(error.to_string().contains("must be owned by"), "{error:#}");
            assert!(!untrusted.join("Butterpollo").exists());
        }
        Ok(())
    }
    #[test]
    fn fresh_installs_move_an_untrusted_profile_aside_unchanged() -> Result<()> {
        // #61: a ProgramData\Butterpollo that the user owned, with no
        // Rubylight installed, stopped every fresh install.
        if !system::elevated() {
            return Ok(());
        }
        let temp = tempfile::tempdir_in(system::program_data())?;
        system::restrict(temp.path(), true)?;
        let root = temp.path().join("Butterpollo");
        let profile = root.join("config");
        std::fs::create_dir_all(&profile)?;
        std::fs::write(profile.join("sunshine.conf"), b"planted")?;
        let (code, output) = system::run(
            &system::system32("icacls.exe"),
            &[
                &root.display().to_string(),
                "/setowner",
                "*S-1-5-32-545",
                "/Q",
            ],
            Duration::from_secs(10),
        )?;
        assert_eq!(code, 0, "{output}");
        let aside = prepare_profile(&profile, false)?.context("the profile stayed")?;
        // A new root, locked down and empty, replaces it.
        system::trusted_folder(&root)?;
        assert_eq!(std::fs::read_dir(&root)?.count(), 0);
        assert_eq!(aside.parent(), Some(temp.path()));
        assert!(
            aside
                .file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("Butterpollo.untrusted-")),
            "{}",
            aside.display()
        );
        assert_eq!(
            std::fs::read(aside.join("config/sunshine.conf"))?,
            b"planted"
        );
        assert!(system::trusted_folder(&aside).is_err());
        // A trusted profile stays where it is.
        std::fs::create_dir_all(&profile)?;
        assert_eq!(prepare_profile(&profile, false)?, None);
        assert!(profile.is_dir());
        // So is a trusted root holding something someone else owns: its owner
        // could change its permissions back.
        let (code, output) = system::run(
            &system::system32("icacls.exe"),
            &[
                &profile.display().to_string(),
                "/setowner",
                "*S-1-5-32-545",
                "/Q",
            ],
            Duration::from_secs(10),
        )?;
        assert_eq!(code, 0, "{output}");
        let child_aside = prepare_profile(&profile, false)?.context("the child stayed")?;
        // A second move, usually within the same second, gets its own name.
        assert_ne!(child_aside, aside);
        assert!(child_aside.join("config").is_dir());
        assert_eq!(std::fs::read_dir(&root)?.count(), 0);
        // A missing root is made locked down from the start, also for an
        // installed service whose profile was deleted.
        for installed in [false, true] {
            std::fs::remove_dir_all(&root)?;
            assert_eq!(prepare_profile(&profile, installed)?, None);
            system::trusted_tree(&root, &[])?;
        }
        // Something users may change or that they own moves aside too.
        let icacls = |args: &[&str]| -> Result<()> {
            let (code, output) = system::run(
                &system::system32("icacls.exe"),
                args,
                Duration::from_secs(10),
            )?;
            assert_eq!(code, 0, "{output}");
            Ok(())
        };
        let profile_text = profile.display().to_string();
        for change in [
            ["/grant", "*S-1-5-32-545:(OI)(CI)(M)"],
            ["/setowner", "*S-1-5-32-545"],
        ] {
            std::fs::create_dir(&profile)?;
            icacls(&[&profile_text, change[0], change[1], "/Q"])?;
            let error = system::trusted_tree(&root, &[]).unwrap_err();
            assert!(
                error.downcast_ref::<system::Untrusted>().is_some(),
                "{error:#}"
            );
            assert!(prepare_profile(&profile, false)?.is_some());
            assert!(!profile.exists());
        }
        // The host lets users write in their Nucleus copies and checks them
        // itself: a profile holding one stays, unless the copies' folder
        // itself is open to users.
        let nucleus = nucleus_user_root(&root);
        assert_eq!(
            nucleus,
            butterpollo_core::nucleus::install::user_root(&root)
        );
        let copy = nucleus.join("S-1-5-21-1").join("0123456789abcdef");
        std::fs::create_dir_all(&copy)?;
        std::fs::write(copy.join("handler.js"), b"user's")?;
        let copy_text = copy.display().to_string();
        icacls(&[&copy_text, "/grant", "*S-1-5-32-545:(OI)(CI)(M)", "/Q"])?;
        icacls(&[&copy_text, "/setowner", "*S-1-5-32-545", "/T", "/Q"])?;
        assert_eq!(prepare_profile(&profile, false)?, None);
        assert_eq!(std::fs::read(copy.join("handler.js"))?, b"user's");
        let nucleus_text = nucleus.display().to_string();
        icacls(&[&nucleus_text, "/grant", "*S-1-5-32-545:(M)", "/Q"])?;
        assert!(prepare_profile(&profile, false)?.is_some());
        assert!(!nucleus.exists());
        Ok(())
    }
    #[test]
    fn folders_created_restricted_match_restricted_ones() -> Result<()> {
        if !system::elevated() {
            return Ok(());
        }
        let temp = tempfile::tempdir_in(system::program_data())?;
        system::restrict(temp.path(), true)?;
        for users_read in [true, false] {
            let created = temp.path().join(format!("created-{users_read}"));
            let restricted = temp.path().join(format!("restricted-{users_read}"));
            system::create_restricted(&created, users_read)?;
            assert!(system::create_restricted(&created, users_read).is_err());
            std::fs::create_dir(&restricted)?;
            system::restrict(&restricted, users_read)?;
            let acl = |folder: &Path| -> Result<String> {
                let (code, output) = system::run(
                    &system::system32("icacls.exe"),
                    &[&folder.display().to_string()],
                    Duration::from_secs(10),
                )?;
                assert_eq!(code, 0, "{output}");
                // icacls aligns its lines with the folder's name.
                let output = output.replace(&folder.display().to_string(), "");
                Ok(output.split_whitespace().collect::<Vec<_>>().join(" "))
            };
            assert_eq!(acl(&created)?, acl(&restricted)?);
            system::trusted_tree(&created, &[])?;
        }
        Ok(())
    }
    #[test]
    fn fresh_installs_move_a_linked_profile_aside_not_its_target() -> Result<()> {
        if !system::elevated() {
            return Ok(());
        }
        let temp = tempfile::tempdir_in(system::program_data())?;
        system::restrict(temp.path(), true)?;
        let outside = temp.path().join("outside");
        std::fs::create_dir(&outside)?;
        std::fs::write(outside.join("sunshine.conf"), b"unchanged")?;
        let linked = temp.path().join("linked");
        std::os::windows::fs::symlink_dir(&outside, &linked)?;
        let aside = prepare_profile(&linked.join("config"), false)?.context("the link stayed")?;
        assert!(std::fs::symlink_metadata(&aside)?.file_type().is_symlink());
        assert!(!std::fs::symlink_metadata(&linked)?.file_type().is_symlink());
        system::trusted_folder(&linked)?;
        assert_eq!(std::fs::read(outside.join("sunshine.conf"))?, b"unchanged");
        assert_eq!(std::fs::read_dir(&outside)?.count(), 1);
        Ok(())
    }
    #[test]
    fn a_damaged_installation_is_repaired_by_the_full_installer() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let installed = temp.path().join("Butterpollo");
        std::fs::create_dir(&installed)?;
        let service = |folder: &Path| detect::Found {
            service_install: Some(folder.to_owned()),
            ..Default::default()
        };
        assert_eq!(
            target(&detect::Found::default(), &installed)?,
            Target::Fresh
        );
        // A deleted folder or a missing or broken manifest leaves nothing to
        // roll back to, so these do not block a reinstall.
        let deleted = temp.path().join("deleted");
        assert_eq!(target(&service(&deleted), &deleted)?, Target::Fresh);
        assert_eq!(target(&service(&installed), &installed)?, Target::Fresh);
        std::fs::write(installed.join("manifest.json"), "{")?;
        assert_eq!(target(&service(&installed), &installed)?, Target::Fresh);
        std::fs::write(installed.join("manifest.json"), "[]")?;
        assert_eq!(target(&service(&installed), &installed)?, Target::InPlace);
        // Another folder would leave the service without its backup.
        let other = temp.path().join("other");
        assert!(target(&service(&installed), &other).is_err());
        std::fs::create_dir(&other)?;
        assert!(target(&service(&installed), &other).is_err());
        Ok(())
    }
    #[test]
    fn an_installation_that_cannot_stay_moves_to_another_folder() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let installed = temp.path().join("Games/Rubylight");
        std::fs::create_dir_all(&installed)?;
        std::fs::write(installed.join("manifest.json"), "[]")?;
        let found = detect::Found {
            service_install: Some(installed.clone()),
            relocate: Some("Games must be owned by Administrators".into()),
            ..Default::default()
        };
        let program_files = temp.path().join("Program Files/Rubylight");
        assert_eq!(
            target(&found, &program_files)?,
            Target::Move(installed.clone())
        );
        std::fs::create_dir_all(&program_files)?;
        assert_eq!(
            target(&found, &program_files)?,
            Target::Move(installed.clone())
        );
        // Kept in its folder it is updated there, where prepare_install
        // refuses it.
        assert_eq!(target(&found, &installed)?, Target::InPlace);
        Ok(())
    }
    #[test]
    fn a_failed_move_points_the_service_back_at_the_previous_folder() {
        let steps = std::cell::RefCell::new(Vec::new());
        let step = |name: &'static str, ok: bool| {
            let steps = &steps;
            move || {
                steps.borrow_mut().push(name);
                if ok { Ok(()) } else { bail!("{name} failed") }
            }
        };
        assert!(switch_service(step("new", true), step("start", true), step("old", true)).is_ok());
        assert_eq!(steps.take(), ["new", "start"]);
        // The service does not start from the new folder: back to the old
        // one, after it was started there.
        let failure =
            switch_service(step("new", true), step("start", false), step("old", true)).unwrap_err();
        assert_eq!(steps.take(), ["new", "start", "old"]);
        assert!(failure.error.to_string().contains("start failed"));
        assert!(failure.back.is_ok());
        let failure = switch_service(step("new", true), step("start", false), step("old", false))
            .unwrap_err();
        assert_eq!(steps.take(), ["new", "start", "old"]);
        assert!(failure.back.is_err());
        // Not pointed at the new folder: nothing to undo or start there.
        let failure =
            switch_service(step("new", false), step("start", true), step("old", true)).unwrap_err();
        assert_eq!(steps.take(), ["new"]);
        assert!(failure.error.to_string().contains("new failed"));
        assert!(failure.back.is_ok());
    }
    /// Makes the user running the tests own `path`, with full access for
    /// everyone, as a folder that user created and earlier setups locked
    /// down but left to that user.
    fn give_to_user(path: &Path) -> Result<()> {
        let (_, identity) = system::run(
            &system::system32("whoami.exe"),
            &["/user", "/fo", "csv", "/nh"],
            Duration::from_secs(10),
        )?;
        let sid = identity
            .trim()
            .split(',')
            .nth(1)
            .context("whoami omitted SID")?
            .trim_matches('"');
        give_to(path, sid)
    }
    /// As [`give_to_user`], for the Users group: not an administrator, so
    /// not trusted with a folder above the installation either.
    fn give_to_users(path: &Path) -> Result<()> {
        give_to(path, "S-1-5-32-545")
    }
    fn give_to(path: &Path, sid: &str) -> Result<()> {
        let owner = format!("*{sid}");
        for arguments in [["/setowner", owner.as_str()], ["/grant:r", "*S-1-1-0:(F)"]] {
            let (code, output) = system::run(
                &system::system32("icacls.exe"),
                &[
                    &path.display().to_string(),
                    arguments[0],
                    arguments[1],
                    "/Q",
                ],
                Duration::from_secs(10),
            )?;
            assert_eq!(code, 0, "{output}");
        }
        Ok(())
    }
    #[test]
    fn updates_repair_a_user_owned_install_folder_only_under_trusted_parents() -> Result<()> {
        // Taking ownership for SYSTEM and Administrators needs an administrator.
        if !system::elevated() {
            return Ok(());
        }
        let temp = tempfile::tempdir_in(system::program_data())?;
        system::restrict(temp.path(), true)?;
        // D:\Rubylight, created by the user and locked down by 2.2.1.
        let install = temp.path().join("Rubylight");
        std::fs::create_dir_all(install.join("assets"))?;
        let host = install.join("butterpollo-service.exe");
        std::fs::write(&host, b"service")?;
        give_to_user(&install)?;
        give_to_user(&host)?;
        let error = system::trusted_ancestors(&install).unwrap_err();
        assert!(error.to_string().contains("must be owned by"), "{error:#}");
        assert!(unrepairable(&install).is_none());
        prepare_install(&install)?;
        system::trusted_ancestors(&install)?;
        system::trusted_folder(&host)?;
        assert_eq!(std::fs::read(&host)?, b"service");
        for path in [&install, &host] {
            let (_, output) = system::run(
                &system::system32("icacls.exe"),
                &[&path.display().to_string(), "/findsid", "*S-1-1-0", "/Q"],
                Duration::from_secs(10),
            )?;
            assert!(!output.contains(&path.display().to_string()), "{output}");
        }
        // D:\Games\Rubylight: whoever owns D:\Games can put another folder
        // in its place, so nothing is changed and the error says to move.
        let games = temp.path().join("Games");
        let nested = games.join("Rubylight");
        std::fs::create_dir_all(&nested)?;
        give_to_users(&games)?;
        give_to_user(&nested)?;
        assert!(unrepairable(&nested).is_some());
        let error = prepare_install(&nested).unwrap_err().to_string();
        assert!(
            error.contains(&format!("cannot be updated in {}", nested.display())),
            "{error}"
        );
        assert!(error.contains("must be owned by"), "{error}");
        assert!(
            error.contains(&format!(
                "moves Rubylight to {}",
                detect::default_folder().display()
            )),
            "{error}"
        );
        assert!(system::trusted_folder(&nested).is_err());
        // Nor is a folder with a link inside repaired.
        let linked = temp.path().join("Linked");
        std::fs::create_dir(&linked)?;
        give_to_user(&linked)?;
        std::os::windows::fs::symlink_dir(&games, linked.join("assets"))?;
        assert!(unrepairable(&linked).is_some());
        assert!(prepare_install(&linked).is_err());
        assert!(system::trusted_folder(&games).is_err());
        Ok(())
    }
    #[test]
    fn a_previous_host_is_removed_only_after_its_profile_can_be_imported() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let old = temp.path().join("Vibepollo");
        let profile = temp.path().join("Butterpollo/config");
        let install = temp.path().join("programs/Butterpollo");
        std::fs::create_dir_all(old.join("config"))?;
        std::fs::write(
            old.join("config/sunshine.conf"),
            "encoder=amdvce_experimental",
        )?;
        let product = detect::Product {
            key: "Vibepollo".into(),
            name: "Vibepollo".into(),
            version: "2.0.0".into(),
            location: Some(old.clone()),
            uninstall: None,
            quiet_uninstall: None,
            msi: false,
            root: HKEY_LOCAL_MACHINE,
        };
        let mut found = detect::Found {
            legacy: vec![product.clone()],
            ..Default::default()
        };
        let source = system::win32_path(&old.canonicalize()?)?;
        assert_eq!(migration_source(&found, &profile, &install)?, Some(source));
        assert!(migration_source(&found, &profile, &old).is_err());
        assert!(migration_source(&found, &profile, &old.join("nested")).is_err());
        std::fs::create_dir_all(&profile)?;
        std::fs::write(profile.join("sunshine_state.json"), "existing pairings")?;
        assert!(migration_source(&found, &profile, &install).is_err());
        assert_eq!(
            std::fs::read(profile.join("sunshine_state.json"))?,
            b"existing pairings"
        );
        std::fs::remove_file(profile.join("sunshine_state.json"))?;
        let other = temp.path().join("Sunshine");
        std::fs::create_dir(&other)?;
        std::fs::write(other.join("sunshine.conf"), "encoder=software")?;
        found.legacy.push(detect::Product {
            location: Some(other),
            ..product.clone()
        });
        assert!(migration_source(&found, &profile, &install).is_err());
        found.legacy.pop();
        // The same host listed twice (its MSI and its own entry) is one source.
        found.vibepollo_entries.push(product.clone());
        assert!(migration_source(&found, &profile, &install)?.is_some());
        found.vibepollo_entries.clear();
        // Hosts with nothing to import are replaced as the summary says, and
        // stale entries without a folder do not block setup.
        for location in [None, Some(temp.path().join("deleted"))] {
            found.legacy.push(detect::Product {
                location,
                ..product.clone()
            });
        }
        std::fs::remove_file(old.join("config/sunshine.conf"))?;
        assert_eq!(migration_source(&found, &profile, &install)?, None);
        // Even then Rubylight is not installed where a host is removed.
        assert!(migration_source(&found, &profile, &old.join("nested")).is_err());
        std::fs::write(profile.join("sunshine_state.json"), "existing pairings")?;
        assert_eq!(migration_source(&found, &profile, &install)?, None);
        assert_eq!(
            migration_source(&detect::Found::default(), &profile, &install)?,
            None
        );
        Ok(())
    }
    #[test]
    fn an_install_that_fails_after_the_import_can_be_run_again() -> Result<()> {
        use std::os::windows::fs::OpenOptionsExt;
        let temp = tempfile::tempdir()?;
        let old = temp.path().join("Vibepollo");
        let profile = temp.path().join("Butterpollo/config");
        let install = temp.path().join("programs/Butterpollo");
        std::fs::create_dir_all(old.join("config"))?;
        std::fs::write(old.join("config/sunshine.conf"), "encoder=amdvce")?;
        let found = detect::Found {
            legacy: vec![detect::Product {
                key: "Vibepollo".into(),
                name: "Vibepollo".into(),
                version: "2.0.0".into(),
                location: Some(old.clone()),
                uninstall: None,
                quiet_uninstall: None,
                msi: false,
                root: HKEY_LOCAL_MACHINE,
            }],
            ..Default::default()
        };
        let import = || -> Result<()> {
            std::fs::create_dir_all(profile.join("credentials"))?;
            std::fs::write(profile.join("sunshine.conf"), "encoder=amdvce")?;
            std::fs::write(profile.join("credentials/cakey.pem"), "key")?;
            Ok(())
        };
        assert!(migration_source(&found, &profile, &install)?.is_some());
        // The file copy after the import fails: the error is kept and the
        // import is gone, so the retry imports again.
        let error = undo_import_on_error(&profile, true, || {
            import()?;
            bail!("disk full")
        })
        .unwrap_err();
        assert_eq!(format!("{error:#}"), "disk full");
        assert!(!profile.exists());
        assert!(migration_source(&found, &profile, &install)?.is_some());
        // A successful run keeps it.
        undo_import_on_error(&profile, true, import)?;
        assert!(profile.join("sunshine.conf").is_file());
        // Without an import nothing is removed.
        assert!(undo_import_on_error(&profile, false, || bail!("disk full")).is_err());
        assert!(profile.join("sunshine.conf").is_file());
        // An import that cannot be removed says so and where.
        let held = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(profile.join("credentials/cakey.pem"))?;
        let error = undo_import_on_error(&profile, true, || bail!("disk full")).unwrap_err();
        let message = format!("{error:#}");
        assert!(message.starts_with("disk full"), "{message}");
        assert!(
            message.contains(&profile.display().to_string()),
            "{message}"
        );
        drop(held);
        Ok(())
    }
    #[test]
    fn legacy_migrations_keep_complete_profiles_and_own_configured_files_and_covers() -> Result<()>
    {
        use crate::profile_tests::{assert_usable, package, rc23_profile, snapshot};
        use butterpollo_core::{config::Config, migration, state};
        use serde_json::{Value, json};
        use sha2::{Digest, Sha256};

        let temp = tempfile::tempdir()?;
        let template = temp.path().join("rc.23/config");
        rc23_profile(&template)?;
        let staged = temp.path().join("staged");
        let entries = package(&staged, env!("CARGO_PKG_VERSION"), "new.dll")?;
        for family in ["Vibepollo", "Apollo", "Sunshine"] {
            for external in [false, true] {
                let case = temp.path().join(format!("{family}-{external}"));
                let old = case.join(family);
                let source = if family == "Sunshine" {
                    old.clone()
                } else {
                    old.join("config")
                };
                let profile = case.join("Butterpollo/config");
                let install = case.join("programs/Butterpollo");
                copy_tree(&template, &source)?;
                let mut apps = state::load_json(&source.join("apps.json"), Value::Null)?;
                apps["apps"][1]["image-path"] = json!(source.join("covers/custom/Épopée.png"));
                state::write_json(&source.join("apps.json"), &apps)?;
                let mut config = Config::load(&source.join("sunshine.conf"))?;
                config.values.insert(
                    "log_path".into(),
                    source.join("butterpollo.log").display().to_string(),
                );
                let configured = [
                    ("file_state", "sunshine_state.json"),
                    ("file_apps", "apps.json"),
                    ("vibeshine_file_state", "vibeshine_state.json"),
                    ("credentials_file", "sunshine_credentials.json"),
                    ("cert", "credentials/cacert.pem"),
                    ("pkey", "credentials/cakey.pem"),
                ];
                if external {
                    let shared = old.join("shared files");
                    for (key, name) in configured {
                        let target = shared.join(name);
                        replace_file(&source.join(name), &target)?;
                        config
                            .values
                            .insert(key.into(), target.display().to_string());
                    }
                    let cover = shared.join("Épopée.png");
                    replace_file(&source.join("covers/custom/Épopée.png"), &cover)?;
                    apps["apps"][1]["image-path"] = json!(cover);
                    state::write_json(&shared.join("apps.json"), &apps)?;
                } else if family == "Sunshine" {
                    // Sunshine can keep the web password in the paired-state document.
                    std::fs::remove_file(source.join("sunshine_credentials.json"))?;
                }
                std::fs::write(source.join("sunshine.conf"), config.text())?;
                let source_before = snapshot(&old)?;
                let mut expected = snapshot(&source)?;
                expected.retain(|name, _| !name.starts_with("logs"));
                for (key, name) in configured {
                    if external {
                        expected
                            .insert(name.into(), std::fs::read(config.path(key, &source, name))?);
                        config.values.insert(key.into(), name.into());
                    }
                }
                config
                    .values
                    .insert("log_path".into(), "butterpollo.log".into());
                expected.insert("sunshine.conf".into(), config.text().into_bytes());
                let product = detect::Product {
                    key: family.into(),
                    name: family.into(),
                    version: "2.0.0".into(),
                    location: Some(old.clone()),
                    uninstall: None,
                    quiet_uninstall: None,
                    msi: false,
                    root: HKEY_LOCAL_MACHINE,
                };
                let found = detect::Found {
                    legacy: vec![product],
                    ..Default::default()
                };
                let selected = migration_source(&found, &profile, &install)?.unwrap();
                assert_eq!(selected, system::win32_path(&old.canonicalize()?)?);
                let selected_profile = if selected.join("config/sunshine.conf").is_file() {
                    selected.join("config")
                } else {
                    selected
                };
                // --import-config in the staged host calls this exact importer.
                migration::import(&selected_profile, &profile)?;
                copy_package(&staged, &install, &entries)?;
                for app in apps["apps"].as_array_mut().unwrap() {
                    let original = source.join(app["image-path"].as_str().unwrap());
                    let bytes = std::fs::read(original)?;
                    let name =
                        PathBuf::from("covers").join(format!("{:x}.png", Sha256::digest(&bytes)));
                    app["image-path"] = json!(profile.canonicalize()?.join(&name));
                    expected.insert(name, bytes);
                }
                expected.insert("apps.json".into(), serde_json::to_vec_pretty(&apps)?);
                assert_eq!(
                    snapshot(&profile)?,
                    expected,
                    "{family}, external={external}"
                );
                assert_eq!(snapshot(&old)?, source_before);
                assert_usable(&profile)?;
                assert!(migration_source(&found, &profile, &install).is_err());
                assert!(migration::import(&source, &profile).is_err());
                assert_eq!(snapshot(&profile)?, expected);
                assert_eq!(snapshot(&old)?, source_before);
                assert!(old.canonicalize()?.starts_with(temp.path().canonicalize()?));
                std::fs::remove_dir_all(&old)?;
                assert_usable(&profile)?;
                for app in apps["apps"].as_array().unwrap() {
                    let cover = PathBuf::from(app["image-path"].as_str().unwrap());
                    let relative = cover.strip_prefix(profile.canonicalize()?)?;
                    assert_eq!(std::fs::read(&cover)?, expected[relative]);
                }
                assert_eq!(snapshot(&profile)?, expected);
            }
        }
        Ok(())
    }
    #[test]
    fn the_health_check_asks_the_configured_bind_address() {
        let local = IpAddr::from([127, 0, 0, 1]);
        for (conf, expected) in [
            ("", local),
            ("port = 48000\n", local),
            ("bind_address =\n", local),
            ("bind_address = 0.0.0.0\n", local),
            ("bind_address = ::\n", local),
            ("bind_address = Ethernet\n", local),
            ("# bind_address = 192.168.1.50\n", local),
            (
                "bind_address = 192.168.1.50\n",
                IpAddr::from([192, 168, 1, 50]),
            ),
            (
                "bind_address = \"192.168.1.50\" # LAN only\r\n",
                IpAddr::from([192, 168, 1, 50]),
            ),
            (
                "bind_address =\nbind_address = 10.0.0.2\nbind_address = 10.0.0.3\n",
                IpAddr::from([10, 0, 0, 2]),
            ),
            ("bind_address = fd00::5\n", "fd00::5".parse().unwrap()),
        ] {
            assert_eq!(probe_address(conf), expected, "{conf:?}");
        }
        let profile = tempfile::tempdir().unwrap();
        let mut utf16 = vec![0xff, 0xfe];
        utf16.extend(
            "port = 48000\r\nbind_address = 192.168.1.50\r\n"
                .encode_utf16()
                .flat_map(u16::to_le_bytes),
        );
        std::fs::write(profile.path().join("sunshine.conf"), utf16).unwrap();
        assert_eq!(
            probe(profile.path()),
            SocketAddr::from(([192, 168, 1, 50], 48000))
        );
        std::fs::write(
            profile.path().join("sunshine.conf"),
            b"\xef\xbb\xbfport = 48000\n",
        )
        .unwrap();
        assert_eq!(
            probe(profile.path()),
            SocketAddr::from(([127, 0, 0, 1], 48000))
        );
        // The probe reaches a host listening only on that address.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let host = std::thread::spawn(move || {
            use std::io::{Read, Write};
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = [0; 512];
            let length = socket.read(&mut request).unwrap();
            socket
                .write_all(b"HTTP/1.1 200 OK\r\n\r\n<RustHostVersion>9.9</RustHostVersion>")
                .unwrap();
            String::from_utf8_lossy(&request[..length]).into_owned()
        });
        wait_ready(address, Some("9.9")).unwrap();
        assert!(host.join().unwrap().contains("Host: 127.0.0.1\r\n"));
    }
    #[test]
    fn the_import_error_and_its_causes_are_shown() {
        let output = "2026-10-07T10:00:00Z  WARN butterpollo_core::migration: profile file not imported file=C:\\old\\dump.bin reason=\"it is larger than 64 MiB\"\n\
Error: the settings imported from C:\\old would keep Rubylight from starting\n\
\n\
Caused by:\n    0: invalid JSON in C:\\ProgramData\\Butterpollo\\.butterpollo-import-1\\vibeshine_state.json\n    1: expected value at line 1 column 1\n";
        assert_eq!(
            import_error(output).unwrap(),
            "the settings imported from C:\\old would keep Rubylight from starting\n\n\
Caused by:\n    0: invalid JSON in C:\\ProgramData\\Butterpollo\\.butterpollo-import-1\\vibeshine_state.json\n    1: expected value at line 1 column 1"
        );
        assert_eq!(
            import_error("Error: no sunshine.conf\n").unwrap(),
            "no sunshine.conf"
        );
        assert!(import_error("thread 'main' panicked\n").is_none());
        assert!(import_error("").is_none());
    }
    #[test]
    fn a_failed_stop_aborts_installation_and_restarts_the_previous_services() {
        let mut stopped = Vec::new();
        let mut started = Vec::new();
        let result = stop_running(
            &OLD_SERVICES,
            |_| true,
            |service| {
                stopped.push(service.to_owned());
                if service == "SunshineService" {
                    bail!("the service did not stop");
                }
                Ok(())
            },
            |service: &str| {
                started.push(service.to_owned());
                Ok(())
            },
        );
        assert!(result.is_err());
        drop(result);
        assert_eq!(stopped, ["ApolloService", "SunshineService"]);
        assert_eq!(started, stopped);
    }
    #[test]
    fn only_previously_running_services_restart_on_failure() -> Result<()> {
        let mut started = Vec::new();
        let restart = stop_running(
            &OLD_SERVICES,
            |service| service == "ApolloService",
            |_| Ok(()),
            |service: &str| {
                started.push(service.to_owned());
                Ok(())
            },
        )?;
        drop(restart);
        assert_eq!(started, ["ApolloService"]);
        let mut restart = Restart {
            services: vec!["ApolloService"],
            start: |_: &str| panic!("successful installation must not restart twice"),
        };
        restart.services.clear();
        drop(restart);
        Ok(())
    }
}
