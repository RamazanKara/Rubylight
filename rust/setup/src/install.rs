//! Installing, upgrading and replacing a previous Vibepollo-family host.
use crate::{
    detect::{self, OLD_SERVICES, UNINSTALL},
    host_service,
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
/// The base ports a side-by-side installation tries, in turn. They stay clear
/// of the default 47984-48010 other hosts use and of the Windows ephemeral
/// range above 49151, and of each other's ranges.
const SIDE_BY_SIDE_PORTS: [u16; 6] = [48589, 48689, 48789, 48889, 48989, 49089];
/// `webrtc_port`'s offset from the base port; `webrtc_media_port` and the
/// guests' four media ports follow it.
const WEBRTC_OFFSET: u16 = 30;
/// Every port a base can use: `Ports::from_base`'s, then WebRTC's.
fn ports_of(base: u16) -> std::ops::RangeInclusive<u16> {
    base.saturating_sub(5)..=base.saturating_add(WEBRTC_OFFSET + 5)
}
/// Whether no program holds `port`, on TCP or UDP. The host binds the same
/// way, so what is free here is free for it: another program's bind, a Windows
/// excluded range or another host stopped for the moment but holding the port.
fn port_free(port: u16) -> bool {
    use std::io::ErrorKind::{AddrInUse, PermissionDenied};
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, TcpListener, UdpSocket};
    let v4 = [Ipv4Addr::UNSPECIFIED, Ipv4Addr::LOCALHOST];
    let free = v4.into_iter().map(IpAddr::V4).all(|address| {
        TcpListener::bind((address, port)).is_ok() && UdpSocket::bind((address, port)).is_ok()
    });
    // A PC without IPv6 has nothing there to collide with.
    let v6 = IpAddr::V6(Ipv6Addr::UNSPECIFIED);
    let held = |error: std::io::Error| matches!(error.kind(), AddrInUse | PermissionDenied);
    free && !TcpListener::bind((v6, port)).err().is_some_and(held)
        && !UdpSocket::bind((v6, port)).err().is_some_and(held)
}
/// The first base port whose whole range is free.
fn pick_base_port(free: impl Fn(u16) -> bool) -> Result<u16> {
    SIDE_BY_SIDE_PORTS
        .into_iter()
        .find(|&base| ports_of(base).all(&free))
        .with_context(|| {
            format!(
                "no free ports for Rubylight beside another host: every range from {} to {} is in use. Close what uses them, then run setup again. Nothing has been changed.",
                ports_of(SIDE_BY_SIDE_PORTS[0]).start(),
                ports_of(SIDE_BY_SIDE_PORTS[5]).end(),
            )
        })
}
/// The new profile's settings: its ports, and a name of its own for clients,
/// which would otherwise show two hosts with the PC's name.
fn side_by_side_conf(base: u16, pc: Option<&str>) -> String {
    let name = match pc.map(str::trim).filter(|pc| !pc.is_empty()) {
        Some(pc) => format!("{pc} (Rubylight)"),
        None => "Rubylight".to_owned(),
    };
    format!(
        "port = {base}\nwebrtc_port = {}\nwebrtc_media_port = {}\nsunshine_name = {name}\n",
        base + WEBRTC_OFFSET,
        base + WEBRTC_OFFSET + 1
    )
}
/// Marks a profile that side by side made, so its identity was never imported
/// from another host: it is kept when Rubylight is installed side by side again
/// after an uninstall that kept its settings.
pub(crate) const SIDE_BY_SIDE_MARK: &str = "side-by-side";
/// Readies the profile of a side-by-side installation, which never takes
/// settings or an identity from another host. An installed Rubylight's profile
/// and one an earlier side-by-side installation left stay; a profile from
/// before moves aside unchanged. Otherwise the profile is new, with its own
/// base port. Returns whether this run made it; `moved` is where the earlier
/// profile went.
fn prepare_side_by_side_profile(
    profile: &Path,
    installed: bool,
    pc: Option<&str>,
    free: impl Fn(u16) -> bool,
    moved: &mut Option<PathBuf>,
) -> Result<bool> {
    let empty = profile_empty(profile)?;
    if !empty && (installed || profile.join(SIDE_BY_SIDE_MARK).is_file()) {
        if !installed {
            // Its port was free when it was chosen; nothing has held it since.
            let base = web_port(profile) - 1;
            if !ports_of(base).all(&free) {
                bail!(
                    "Rubylight's kept settings in {} use base port {base}, and a port of its range is in use now. Close what uses it, or set another port in sunshine.conf there, then run setup again. Nothing has been changed.",
                    profile.display()
                );
            }
        }
        return Ok(false);
    }
    let base = pick_base_port(free)?;
    if !empty {
        *moved = Some(move_profile_aside(profile)?);
    }
    std::fs::create_dir_all(profile)?;
    std::fs::write(profile.join("sunshine.conf"), side_by_side_conf(base, pc))?;
    std::fs::write(profile.join(SIDE_BY_SIDE_MARK), b"")?;
    Ok(true)
}
fn earlier_settings_moved(aside: &Path) -> String {
    format!(
        "Side by side starts with new settings, so setup moved the earlier Rubylight settings to {}.",
        aside.display()
    )
}
/// `profile` under the next free name beside it, unchanged.
fn move_profile_aside(profile: &Path) -> Result<PathBuf> {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let name = profile.file_name().context("profile has no name")?;
    let aside = (0..100)
        .map(|n| profile.with_file_name(format!("{}.previous-{seconds}-{n}", name.display())))
        .find(|aside| std::fs::symlink_metadata(aside).is_err())
        .context("no free name to move the earlier settings to")?;
    std::fs::rename(profile, &aside)
        .with_context(|| format!("moving {} to {}", profile.display(), aside.display()))?;
    Ok(aside)
}
/// What the finished window adds for a side-by-side installation: clients
/// must be given the port, and may otherwise reach the other host.
pub fn side_by_side_result(web_port: u16, address: &str, hosts: &[String]) -> String {
    let beside = if hosts.is_empty() {
        String::new()
    } else {
        format!(" beside {}", detect::in_words(hosts))
    };
    format!(
        "Rubylight runs{beside} as the {} service, with its own settings. In Moonlight or Nightjar, add this PC as {address}:{}: without the port, a client reaches whatever host uses the default one.",
        host_service::SIDE_BY_SIDE,
        web_port - 1
    )
}
/// This PC's address on its default route, else its name. Connecting a UDP
/// socket sends nothing; it only picks the route.
pub fn pc_address() -> String {
    let routed = std::net::UdpSocket::bind(("0.0.0.0", 0))
        .and_then(|socket| {
            socket.connect(("192.0.2.1", 9))?;
            socket.local_addr()
        })
        .ok()
        .map(|address| address.ip())
        .filter(|ip| !ip.is_unspecified() && !ip.is_loopback());
    match routed {
        Some(ip) => ip.to_string(),
        None => pc_name().unwrap_or_else(|| "this PC's address".to_owned()),
    }
}
fn pc_name() -> Option<String> {
    std::env::var("COMPUTERNAME")
        .ok()
        .filter(|name| !name.trim().is_empty())
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
        // A package without drivers reuses the signed drivers already
        // installed with Vibepollo.
        for (from, to) in [("drivers\\vhf-gamepad", "drivers\\gamepad")] {
            if !staging.path().join(to).exists() && root.join(from).is_dir() {
                copy_tree(&root.join(from), &staging.path().join(to))?;
            }
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
    let mut restart_needed = false;
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
    Ok(restart_needed)
}

/// Create `install` if needed and give it the service folder's permissions,
/// before setup stages anything in it. Returns whether it was created.
fn create_install(install: &Path) -> Result<bool> {
    let created = std::fs::symlink_metadata(install).is_err();
    std::fs::create_dir_all(install)?;
    secure_install(install)?;
    Ok(created)
}
/// Setup's staging folder, inside the installation folder: the package of a
/// full installation (`package`), each update's transaction (new files and
/// the backup) and the host's update downloads (`download-<id>`) go there. Only administrators may change the
/// installation folder; elevated programs run from ProgramData, where
/// earlier setups staged, look like malware (Defender's cloud model quarantined
/// the staged service there).
pub(crate) fn staging_root(install: &Path) -> PathBuf {
    install.join(".staging")
}
/// [`staging_root`] of an installation folder setup has already secured,
/// limited to SYSTEM and Administrators. A link in its place or inside it
/// is refused, never followed.
pub(crate) fn prepare_staging_root(install: &Path) -> Result<PathBuf> {
    let root = staging_root(install);
    match std::fs::symlink_metadata(&root) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => std::fs::create_dir(&root)?,
        Err(error) => return Err(error.into()),
        Ok(_) => {}
    }
    system::restrict(&root, false).with_context(|| format!("securing {}", root.display()))?;
    Ok(root)
}
/// The staged package of a full installation. Dropping it deletes the
/// package, the staging folder once nothing else is in it (a failed
/// update's backup stays), and an installation folder this run created that
/// is still empty: a setup that fails leaves no folder behind.
struct Staging {
    package: PathBuf,
    /// Set once the staging folder passed its check: a link in its place
    /// would lead the removal elsewhere.
    checked: bool,
    created: Option<PathBuf>,
}
impl Staging {
    fn path(&self) -> &Path {
        &self.package
    }
}
impl Drop for Staging {
    fn drop(&mut self) {
        if self.checked {
            match std::fs::remove_dir_all(&self.package) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => line(format!(
                    "the staged package {} could not be removed: {error}",
                    self.package.display()
                )),
            }
            if let Some(root) = self.package.parent() {
                let _ = std::fs::remove_dir(root);
            }
        }
        if let Some(install) = &self.created {
            let _ = std::fs::remove_dir(install);
        }
    }
}
/// An empty, locked-down package folder in `install`'s staging folder; what
/// an interrupted setup left there is removed first.
fn prepare_staging(install: &Path, created: bool) -> Result<Staging> {
    // First, so that a failure from here on also removes a folder this run
    // created.
    let mut staging = Staging {
        package: staging_root(install).join("package"),
        checked: false,
        created: created.then(|| install.to_path_buf()),
    };
    prepare_staging_root(install)?;
    staging.checked = true;
    // `prepare_staging_root` refused a link anywhere in the folder.
    if std::fs::symlink_metadata(staging.path()).is_ok() {
        std::fs::remove_dir_all(staging.path())
            .context("removing the previous setup staging folder")?;
    }
    // A new folder, so nothing made before this point is inside it.
    std::fs::create_dir(staging.path())?;
    // The staged host runs elevated (settings import) and loads DLLs from its
    // own folder: lock the folder down before anything lands in it.
    system::restrict(staging.path(), false)?;
    Ok(staging)
}
/// Unpack and verify the package in `install`'s staging folder.
fn unpack(
    payload: &mut Payload,
    install: &Path,
    created: bool,
    progress: &Progress,
) -> Result<(Staging, Vec<payload::Entry>)> {
    progress.set("Unpacking Rubylight…");
    let staging = prepare_staging(install, created)?;
    payload.extract(staging.path())?;
    let entries = payload::verify(staging.path())?;
    Ok((staging, entries))
}
/// Where earlier setups staged the package. It may still hold the
/// programs of an earlier version's setup; a link there is removed itself,
/// never followed.
pub(crate) fn remove_legacy_staging(program_data: &Path) -> Result<()> {
    use std::os::windows::fs::MetadataExt;
    let folder = program_data.join("Butterpollo").join("setup");
    let removed = match std::fs::symlink_metadata(&folder) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => Err(error),
        Ok(metadata) if metadata.file_attributes() & 0x400 != 0 => {
            std::fs::remove_dir(&folder).or_else(|_| std::fs::remove_file(&folder))
        }
        Ok(_) => std::fs::remove_dir_all(&folder),
    };
    removed.with_context(|| format!("removing the earlier staging folder {}", folder.display()))?;
    line(format!(
        "removed the earlier staging folder {}",
        folder.display()
    ));
    Ok(())
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
    service: &'static str,
    payload: &mut Payload,
    progress: &Progress,
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
    remember_driver_choice(profile, options.display_driver);
    let mut notes = Vec::new();
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
    crate::usbip::remember(profile, options.usbip_driver)?;
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
/// Whether setup would update an installed Rubylight where it is, which never
/// touches another host, rather than install in full.
pub(crate) fn updates_in_place(found: &detect::Found, install: &Path) -> bool {
    matches!(target(found, install), Ok(Target::InPlace))
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
/// Whether `install` is `root` or inside it.
fn folder_inside(install: &Path, root: &Path) -> Result<bool> {
    // A drive root ends in its separator already (`c:\`), a folder does not.
    let root_path = root.to_string_lossy().to_lowercase();
    let root_path = root_path.trim_end_matches('\\');
    let install_path = system::win32_path(&canonical_prefix(install))?
        .to_string_lossy()
        .to_lowercase();
    let install_path = install_path.trim_end_matches('\\');
    Ok(install_path == root_path || install_path.starts_with(&format!("{root_path}\\")))
}
/// Side by side, no file of another host may be overwritten, whether setup knows
/// that host by its Apps entry or only by its service.
fn outside_other_hosts(found: &detect::Found, install: &Path) -> Result<()> {
    let products = found
        .packages
        .iter()
        .chain(&found.legacy)
        .chain(&found.vibepollo_entries)
        .filter_map(|product| Some((product.location.as_deref()?, product.name.clone())));
    let services = found.other_service_folders.iter().map(|(service, folder)| {
        (
            folder.as_path(),
            format!("the host behind the {service} service"),
        )
    });
    for (root, host) in products.chain(services) {
        if let Ok(root) = root.canonicalize() {
            let root = system::win32_path(&root)?;
            if folder_inside(install, &root)? {
                bail!(
                    "Choose an installation folder outside {}: {host} stays installed there. Nothing has been changed.",
                    root.display()
                );
            }
        }
    }
    Ok(())
}
/// Side by side runs as `RubylightService`. Another program's service of that
/// name is not Rubylight's to stop or point elsewhere; `program` is what it runs.
fn refuse_foreign_service(program: Option<&Path>) -> Result<()> {
    match program {
        Some(program) if !detect::our_service(program) => bail!(
            "A service named {} already exists and runs {}, which is not Rubylight. Setup does not stop or change it, and Rubylight needs that name to install side by side. Remove that service, then run setup again. Nothing has been changed.",
            host_service::SIDE_BY_SIDE,
            program.display()
        ),
        _ => Ok(()),
    }
}
fn profile_empty(profile: &Path) -> Result<bool> {
    match std::fs::read_dir(profile) {
        Ok(mut entries) => Ok(entries.next().transpose()?.is_none()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(error) => Err(error).context("checking the existing Rubylight profile"),
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
        if folder_inside(install, &root)? {
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
    if !profile_empty(profile)? {
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
/// The users' Nucleus copies keep the permissions the host gave them, which
/// let each user write in their own.
fn secure_profile(profile: &Path) -> Result<()> {
    let root = profile.parent().context("profile has no parent")?;
    let opaque = [nucleus_user_root(root)];
    secure_profile_with(
        profile,
        |folder, users_read| system::restrict_except(folder, users_read, &opaque),
        |root| system::inherit_contents(root, &opaque),
    )
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
/// (`butterpollo_core::crypto::CONSOLE_ROOT_NAME`). certutil's `-delstore`
/// by this name also removes the roots whose name adds a host's own unit.
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
        return repair_profile(profile).map(|()| None);
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
/// Makes the installed service's profile, which `prepare_profile` found a
/// moment ago, all the service's. Earlier installers restricted its ACL but
/// left its user owner, and a repair that fails partway leaves the rest, so
/// every run repairs until nothing in it but the users' Nucleus copies
/// (checked as a fresh install does) is anyone else's. Gone now, it is an
/// error, not a folder to make with ProgramData's permissions.
fn repair_profile(profile: &Path) -> Result<()> {
    let root = profile.parent().context("profile has no parent")?;
    let opaque = [nucleus_user_root(root)];
    match system::trusted_tree(root, &opaque) {
        Ok(()) => Ok(()),
        Err(error) if error.downcast_ref::<system::Untrusted>().is_some() => {
            line(format!("repairing {}: {error:#}", root.display()));
            secure_profile(profile)?;
            // The repair leaves the Nucleus copies' folder as it is, even one
            // setup can't trust: it doesn't adopt what may be a user's.
            let nucleus = &opaque[0];
            if let Err(error) = system::trusted_tree(nucleus, &opaque)
                && error.downcast_ref::<system::Untrusted>().is_some()
            {
                return Err(error.context(format!(
                    "an administrator can delete {} (Rubylight copies Nucleus Co-op for each user again), then run setup again",
                    nucleus.display()
                )));
            }
            system::trusted_tree(root, &opaque).with_context(|| {
                format!("{} is not the service's after its repair", root.display())
            })
        }
        Err(error) => Err(error.context(format!("checking {}", root.display()))),
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
        match gamepad_step(detect::side_by_side(), crate::libvirtualdisplay::present_devices) {
            GamepadStep::Install => {
                progress.set("Installing the virtual gamepad driver…");
                let before = notes.len();
                let restart = run_driver_script(
                    &install.join("drivers\\gamepad\\install.ps1"),
                    &["-InstallerBestEffort", "-AllowLocalTestCertificate:0"],
                    notes,
                    "virtual gamepad",
                );
                restart_needed |= restart;
                // A restart to come or a refused package is reported already.
                if !restart && notes.len() == before {
                    restart_needed |= ensure_gamepad_device(install, progress, notes);
                }
            }
            GamepadStep::Reuse => line(
                "the virtual gamepad driver is already here, and another host may use it: using it as it is",
            ),
            GamepadStep::Unknown(error) => notes.push(format!(
                "Setup could not tell whether the virtual gamepad driver is installed ({error:#}), so it left the driver another host may use alone. Controllers need it; if they do not work in a stream, run setup again."
            )),
        }
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
/// The instance ids of the virtual gamepad's source device start with this:
/// the driver's own cleanup script goes by the same prefix.
const GAMEPAD_DEVICE_PREFIX: &str = "ROOT\\VIBESHINEVIRTUALGAMEPAD\\";
#[derive(Debug)]
enum GamepadStep {
    Install,
    /// The device is here. Vibepollo's family shares this driver, and its
    /// script reloads the device under whoever uses it, so a side-by-side host
    /// uses it as it is.
    Reuse,
    /// Side by side, and the devices could not be read.
    Unknown(anyhow::Error),
}
fn gamepad_step(side_by_side: bool, devices: impl FnOnce() -> Result<Vec<String>>) -> GamepadStep {
    if !side_by_side {
        return GamepadStep::Install;
    }
    match devices() {
        Ok(devices)
            if devices.iter().any(|device| {
                device
                    .to_ascii_uppercase()
                    .starts_with(GAMEPAD_DEVICE_PREFIX)
            }) =>
        {
            GamepadStep::Reuse
        }
        Ok(_) => GamepadStep::Install,
        Err(error) => GamepadStep::Unknown(error),
    }
}

/// libvirtualgamepad's own tool for its source device, and the package it
/// binds to that device.
const GAMEPAD_DEVICE_TOOL: &str = "drivers\\gamepad\\tools\\VibeshineVhfGamepadDeviceSetup.exe";
const GAMEPAD_INF: &str = "drivers\\gamepad\\driver\\VibeshineVhfGamepad.inf";
/// Whether the device tool's `status` says the source device answers: its
/// JSON line's `source_interface_ready`, or `None` when there is no such line.
fn gamepad_device_ready(status: &str) -> Option<bool> {
    status.lines().find_map(|line| {
        serde_json::from_str::<serde_json::Value>(line.trim())
            .ok()?
            .get("source_interface_ready")?
            .as_bool()
    })
}
/// The gamepad driver's script skips when Windows' Driver Store already holds
/// the same or a newer package, without looking for the device that runs it.
/// Uninstalling another host that shared the driver (Vibepollo) takes that
/// device away and leaves the package, so no controller can be opened (#86):
/// the driver's own tool then creates the device. Returns whether Windows
/// must restart.
fn ensure_gamepad_device(install: &Path, progress: &Progress, notes: &mut Vec<String>) -> bool {
    let (tool, inf) = (install.join(GAMEPAD_DEVICE_TOOL), install.join(GAMEPAD_INF));
    if !tool.is_file() || !inf.is_file() {
        return false;
    }
    let tool = tool.display().to_string();
    let ready = || {
        system::run_as_system(&tool, &["status"], Duration::from_secs(60))
            .map(|(_, output)| gamepad_device_ready(&output))
    };
    match ready() {
        Ok(Some(false)) => {}
        Ok(Some(true)) => return false,
        Ok(None) => {
            line("the virtual gamepad device tool gave no status; leaving the device as it is");
            return false;
        }
        Err(error) => {
            line(format!(
                "warning: the virtual gamepad device status: {error:#}"
            ));
            return false;
        }
    }
    line("the virtual gamepad driver is installed, but no device runs it: setting the device up");
    progress.set("Setting up the virtual gamepad device…");
    let inf = inf.display().to_string();
    let code =
        match system::run_as_system(&tool, &["install", "--inf", &inf], Duration::from_secs(600)) {
            Ok((code, _)) => code,
            Err(error) => {
                line(format!("warning: {error:#}"));
                notes.push(driver_error_note(&error, "virtual gamepad"));
                return false;
            }
        };
    if code == 3010 {
        return true;
    }
    // A newer package another host left runs the device instead of this one,
    // which the tool reports as a failure although the device answers.
    if code != 0 && !matches!(ready(), Ok(Some(true))) {
        notes.push(
            "The virtual gamepad device could not be set up, and controllers need it. Restart Windows and run setup again; the setup log has the details.".into(),
        );
    }
    false
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
/// still pending before the device starts. Side by side it replaces nothing.
fn libvirtualdisplay_replaced(side_by_side: bool, noted: bool, restart_pending: bool) -> bool {
    !side_by_side && !noted && !restart_pending
}
/// Remove libvirtualdisplay's device, driver packages and HDR layer, best
/// effort. Returns whether Windows needs a restart.
fn remove_libvirtualdisplay(progress: &Progress, notes: &mut Vec<String>) -> bool {
    progress.set("Removing the old virtual display driver…");
    let removal = crate::libvirtualdisplay::remove(&mut crate::libvirtualdisplay::Native);
    notes.extend(libvirtualdisplay_note(&removal));
    removal.restart
}
/// The note for a removal that failed: setup tries again on its next run.
fn libvirtualdisplay_note(removal: &crate::libvirtualdisplay::Removal) -> Option<String> {
    let error = removal.error.as_ref()?;
    line(format!(
        "warning: removing the {LIBVIRTUALDISPLAY} driver: {error:#}"
    ));
    Some(format!(
        "The {LIBVIRTUALDISPLAY} driver could not be removed: {error:#} Setup tries again when it next runs; the setup log has the details."
    ))
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
            "The {name} driver was still being set up after {} s and goes on in the background. If it does not work once that finishes, run setup again.",
            running.seconds
        ),
        None => format!("The {name} driver could not be set up: {error:#}"),
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
        } else if name == "virtual gamepad" {
            "Run setup again; the setup log has the details."
        } else {
            // The service looks for the display driver every minute.
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
    system::inherit_contents(install, &[])
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
mod tests;
