//! What is installed: Rubylight itself, Vibepollo-family MSI packages
//! (Vibepollo, Rubylight C++, Apollo, Sunshine) and legacy NSIS installs.
use crate::{
    host_service,
    system::{self, registry_dword, registry_string_view, service_program, subkeys},
};
use std::path::{Path, PathBuf};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_WOW64_32KEY, KEY_WOW64_64KEY, REG_SAM_FLAGS,
};

pub const UNINSTALL: &str = "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall";
/// Services the previous hosts registered.
pub const OLD_SERVICES: [&str; 5] = [
    "ApolloService",
    "SunshineService",
    "VibeshineService",
    "sunshinesvc",
    "ApolloSvc",
];
/// Whether a service runs Rubylight's program. Another host may hold the name
/// `ApolloService`; only this says it is Rubylight's to stop, change or delete.
pub fn our_service(program: &Path) -> bool {
    program
        .file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case("butterpollo-service.exe"))
}
/// The service this PC's Rubylight runs as.
pub fn service() -> &'static str {
    service_named(
        system::registry_string(HKEY_LOCAL_MACHINE, host_service::KEY, host_service::VALUE)
            .as_deref(),
        service_program,
    )
}
/// A lost record must not turn a side-by-side installation into one that
/// removes the host it was installed beside: its service still says what it is.
fn service_named(
    recorded: Option<&str>,
    program: impl Fn(&str) -> Option<PathBuf>,
) -> &'static str {
    match host_service::from_record(recorded) {
        host_service::DEFAULT
            if program(host_service::SIDE_BY_SIDE).is_some_and(|p| our_service(&p)) =>
        {
            host_service::SIDE_BY_SIDE
        }
        named => named,
    }
}
/// Whether this PC's Rubylight keeps other hosts: it never stops or removes
/// them, nor the drivers they use.
pub fn side_by_side() -> bool {
    service() == host_service::SIDE_BY_SIDE
}
/// What an installation records about its service, before the service is
/// registered, so the drivers' steps and every later run see the same.
pub fn record_service(side_by_side: bool) -> anyhow::Result<()> {
    if side_by_side {
        system::write_key(
            HKEY_LOCAL_MACHINE,
            host_service::KEY,
            &[(
                host_service::VALUE,
                system::Value::Text(host_service::SIDE_BY_SIDE),
            )],
        )
    } else {
        forget_service();
        Ok(())
    }
}
/// The record goes; `HKLM\SOFTWARE\Rubylight` may hold the display driver's.
pub fn forget_service() {
    system::delete_value(
        HKEY_LOCAL_MACHINE,
        host_service::KEY,
        host_service::VALUE,
        KEY_WOW64_64KEY,
    );
}
/// The previous hosts' services that exist and are not Rubylight's.
fn other_services(program: impl Fn(&str) -> Option<PathBuf>) -> Vec<&'static str> {
    OLD_SERVICES
        .iter()
        .copied()
        .filter(|name| program(name).is_some_and(|p| !our_service(&p)))
        .collect()
}

#[derive(Clone, Debug)]
pub struct Product {
    /// The Uninstall subkey: a product code for Windows Installer packages.
    pub key: String,
    pub name: String,
    pub version: String,
    pub location: Option<PathBuf>,
    pub uninstall: Option<String>,
    pub quiet_uninstall: Option<String>,
    pub msi: bool,
    pub root: HKEY,
}
impl Product {
    pub fn product_code(&self) -> Option<&str> {
        (self.msi && self.key.starts_with('{') && self.key.ends_with('}')).then_some(&self.key)
    }
}
#[derive(Default, Debug)]
pub struct Found {
    /// Rubylight's own entry.
    pub butterpollo: Option<Product>,
    /// Installed with Windows Installer: removed with msiexec.
    pub packages: Vec<Product>,
    /// Apollo or Sunshine installed by their own installers.
    pub legacy: Vec<Product>,
    /// Vibepollo's own Add/Remove Programs entry for its MSI.
    pub vibepollo_entries: Vec<Product>,
    /// The folder of a Rust host the service already runs, if any.
    pub service_install: Option<PathBuf>,
    /// Why the installed Rubylight cannot stay in its folder, which then
    /// moves (install::unrepairable).
    pub relocate: Option<String>,
    /// The service Rubylight runs or would run as ([`service`]).
    pub service: &'static str,
    /// Previous hosts' services that are not Rubylight's.
    pub other_services: Vec<&'static str>,
}
impl Found {
    /// Whether a Rubylight is installed here that keeps other hosts.
    pub fn installed_side_by_side(&self) -> bool {
        self.service == host_service::SIDE_BY_SIDE && self.current().is_some()
    }
    /// Whether this run installs side by side. `requested` is --side-by-side
    /// or the setup window's choice. An installation stays what it is: a
    /// side-by-side one never turns into one that removes the other host, and
    /// a normal one, which owns the default ports and may hold another host's
    /// imported identity, does not become side by side in place.
    pub fn side_by_side(&self, requested: bool) -> anyhow::Result<bool> {
        if self.installed_side_by_side() {
            return Ok(true);
        }
        if requested && self.current().is_some() {
            anyhow::bail!(
                "Rubylight is already installed on this PC as the {} service, so it cannot also install side by side. Uninstall Rubylight first, then run setup with --side-by-side. Side by side starts with new settings; the current ones are moved aside, not deleted. Nothing has been changed.",
                host_service::DEFAULT
            );
        }
        Ok(requested)
    }
    /// The other hosts' names for the user: their Apps entries, else the
    /// services they left.
    pub fn other_hosts(&self) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        for product in self
            .packages
            .iter()
            .chain(&self.legacy)
            .chain(&self.vibepollo_entries)
        {
            if !names.iter().any(|n| n.eq_ignore_ascii_case(&product.name)) {
                names.push(product.name.clone());
            }
        }
        if names.is_empty() {
            names.extend(
                self.other_services
                    .iter()
                    .map(|service| format!("the host behind the {service} service")),
            );
        }
        names
    }
    /// The installed Rubylight's folder: its service's, else its Apps entry's.
    pub fn current(&self) -> Option<&Path> {
        self.service_install
            .as_deref()
            .or_else(|| self.butterpollo.as_ref()?.location.as_deref())
    }
    pub fn check_version(&self) -> anyhow::Result<()> {
        if let Some(product) = &self.butterpollo {
            check_version(&product.version, env!("CARGO_PKG_VERSION"))?;
        }
        Ok(())
    }
    /// The Vibepollo-family installation whose settings to import.
    pub fn previous_root(&self) -> Option<PathBuf> {
        self.packages
            .iter()
            .chain(&self.vibepollo_entries)
            .chain(&self.legacy)
            .filter_map(|p| p.location.clone())
            .find(|root| root.join("config").join("sunshine.conf").is_file())
            .or_else(|| {
                // Older Apollo/Sunshine kept files beside the program.
                self.packages
                    .iter()
                    .chain(&self.legacy)
                    .filter_map(|p| p.location.clone())
                    .find(|root| root.join("sunshine.conf").is_file())
            })
    }
}

fn check_version(installed: &str, incoming: &str) -> anyhow::Result<()> {
    let release = installed
        .trim()
        .trim_start_matches(['v', 'V'])
        .split(['-', '+'])
        .next()
        .unwrap_or("");
    // Without a readable version there is nothing to compare; refusing would
    // keep the installer from repairing that installation.
    if release.split('.').count() < 3 || release.split('.').any(|part| part.parse::<u64>().is_err())
    {
        crate::log::line(format!(
            "the installed Rubylight version {installed:?} is unreadable; not checking for a downgrade"
        ));
        return Ok(());
    }
    if crate::version::newer(installed, incoming) {
        anyhow::bail!(
            "Rubylight {installed} is newer than this installer ({incoming}). Downgrades are not supported; your settings have not been changed."
        );
    }
    Ok(())
}

/// The program folder of an Uninstall entry: its InstallLocation, else the
/// folder of the program its DisplayIcon or UninstallString names. Some
/// installers leave InstallLocation out.
fn resolve_location(
    install_location: Option<&str>,
    display_icon: Option<&str>,
    uninstall: Option<&str>,
) -> Option<PathBuf> {
    if let Some(location) = install_location.map(str::trim).filter(|l| !l.is_empty()) {
        return Some(PathBuf::from(location.trim_end_matches('\\')));
    }
    [display_icon, uninstall]
        .into_iter()
        .flatten()
        .find_map(|text| program_folder(&program(text)?))
}
/// The program a DisplayIcon ("C:\App\app.exe,0") or a command line
/// ("\"C:\App\uninstall.exe\" /S") names, if it is a full path.
pub(crate) fn program(text: &str) -> Option<PathBuf> {
    let text = text.trim();
    let path = match text.strip_prefix('"') {
        Some(quoted) => quoted.split('"').next()?,
        None => match text.to_ascii_lowercase().find(".exe") {
            Some(end) => &text[..end + 4],
            None => text
                .rsplit_once(',')
                .filter(|(_, index)| index.trim().parse::<i32>().is_ok())
                .map_or(text, |(path, _)| path),
        },
    };
    let path = PathBuf::from(path.trim());
    path.is_absolute().then_some(path)
}
/// The folder a host's program is installed in (its service runs from
/// tools). Windows Installer's msiexec and icon cache belong to no host.
fn program_folder(program: &Path) -> Option<PathBuf> {
    if program
        .file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case("msiexec.exe"))
        || program
            .to_string_lossy()
            .to_ascii_lowercase()
            .contains("\\windows\\installer\\")
    {
        return None;
    }
    let folder = program.parent()?;
    if folder
        .file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case("tools"))
    {
        return folder.parent().map(Path::to_path_buf);
    }
    Some(folder.to_path_buf())
}
/// Whether a previous host's folder holds settings to import.
pub(crate) fn has_profile(root: &Path) -> bool {
    root.join("config").join("sunshine.conf").is_file() || root.join("sunshine.conf").is_file()
}

fn read(root: HKEY, view: REG_SAM_FLAGS, key: &str) -> Option<Product> {
    let path = format!("{UNINSTALL}\\{key}");
    let name = registry_string_view(root, &path, "DisplayName", view)?;
    let uninstall = registry_string_view(root, &path, "UninstallString", view);
    let location = resolve_location(
        registry_string_view(root, &path, "InstallLocation", view).as_deref(),
        registry_string_view(root, &path, "DisplayIcon", view).as_deref(),
        uninstall.as_deref(),
    );
    Some(Product {
        key: key.to_owned(),
        name: name.trim().to_owned(),
        version: registry_string_view(root, &path, "DisplayVersion", view).unwrap_or_default(),
        location,
        uninstall,
        quiet_uninstall: registry_string_view(root, &path, "QuietUninstallString", view),
        msi: registry_dword(root, &path, "WindowsInstaller", view) == Some(1),
        root,
    })
}

pub fn scan() -> Found {
    let mut found = Found::default();
    for (root, view) in [
        (HKEY_LOCAL_MACHINE, KEY_WOW64_64KEY),
        (HKEY_LOCAL_MACHINE, KEY_WOW64_32KEY),
        (HKEY_CURRENT_USER, KEY_WOW64_64KEY),
    ] {
        for key in subkeys(root, UNINSTALL, view) {
            let Some(product) = read(root, view, &key) else {
                continue;
            };
            let name = product.name.to_ascii_lowercase();
            if key.eq_ignore_ascii_case("Butterpollo") && root == HKEY_LOCAL_MACHINE {
                found.butterpollo = Some(product);
            } else if !matches!(
                name.as_str(),
                "vibepollo" | "vibeshine" | "apollo" | "sunshine"
            ) {
                continue;
            } else if product.product_code().is_some() {
                found.packages.push(product);
            } else if name == "vibepollo" || name == "vibeshine" {
                found.vibepollo_entries.push(product);
            } else if !found.legacy.iter().any(|p| p.name == product.name) {
                found.legacy.push(product);
            }
        }
    }
    // The hidden MSI entry has no location; take it from Vibepollo's entry.
    // Otherwise, as for any entry without one, the previous host's service
    // tells where it is installed.
    let service = OLD_SERVICES
        .iter()
        .filter_map(|name| service_program(name))
        .filter(|program| {
            program
                .file_name()
                .is_some_and(|n| n.eq_ignore_ascii_case("sunshinesvc.exe"))
        })
        .find_map(|program| program_folder(&program));
    let location = found
        .vibepollo_entries
        .iter()
        .find_map(|p| p.location.clone())
        .or_else(|| service.clone());
    for package in &mut found.packages {
        if package.location.is_none() {
            package.location = location.clone();
        }
    }
    for product in found.vibepollo_entries.iter_mut().chain(&mut found.legacy) {
        if product.location.is_none() {
            product.location = service.clone();
        }
    }
    found.service = self::service();
    found.other_services = other_services(service_program);
    found.service_install = service_program(found.service)
        .filter(|p| our_service(p))
        .and_then(|p| p.parent().map(PathBuf::from));
    found.relocate = found
        .current()
        .and_then(crate::install::unrepairable)
        .map(|reason| format!("{reason:#}"));
    found
}
/// Where a new installation goes.
pub fn default_folder() -> PathBuf {
    crate::system::program_files().join("Rubylight")
}
/// Where Rubylight is or will be installed.
pub fn install_dir(found: &Found, requested: Option<PathBuf>) -> PathBuf {
    // The running service's folder first: setup updates it in place, unless
    // it has to move.
    requested
        .or_else(|| {
            found
                .current()
                .filter(|_| found.relocate.is_none())
                .map(Path::to_path_buf)
        })
        .unwrap_or_else(default_folder)
}
/// Whether setup may install somewhere else. As with Vibepollo, only a new
/// installation can choose; an installed Rubylight is updated where it is,
/// so a failed update can be put back. One that cannot stay there moves.
pub fn folder_choosable(found: &Found) -> bool {
    (found.service_install.is_none() && found.butterpollo.is_none()) || found.relocate.is_some()
}
fn same_folder(a: &Path, b: &Path) -> bool {
    a.canonicalize()
        .ok()
        .is_some_and(|a| b.canonicalize().ok() == Some(a))
}
/// The folder to install into when the user picked `picked`: the folder
/// itself when it is empty or holds Rubylight, else a Rubylight folder in it,
/// so that picking D:\ or D:\Games never fills or locks down that folder.
pub fn chosen_folder(picked: &std::path::Path) -> PathBuf {
    let usable = picked.parent().is_some()
        && match std::fs::read_dir(picked) {
            Ok(mut entries) => entries.next().is_none() || picked.join("manifest.json").is_file(),
            Err(_) => !picked.exists(),
        };
    if usable {
        picked.to_path_buf()
    } else {
        picked.join("Rubylight")
    }
}
/// Names for a sentence: "A", "A and B", "A, B and C".
pub fn in_words(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [name] => name.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}
/// What the choice between the other hosts and side by side says.
pub struct HostChoice {
    pub heading: String,
    pub text: String,
    pub replace: String,
    pub keep: String,
}
/// The choice a new installation offers when other hosts are here, `None`
/// when there are none. Replace comes first and is the default.
pub fn host_choice(found: &Found) -> Option<HostChoice> {
    let names = found.other_hosts();
    if names.is_empty() {
        return None;
    }
    let hosts = in_words(&names);
    Some(HostChoice {
        heading: format!("Replace {hosts}?"),
        text: format!(
            "Replace: setup uninstalls {hosts} and imports its settings, paired devices and apps into Rubylight, so your devices stay paired.\n\nKeep: Rubylight installs alongside it, with its own service, its own ports and new settings. {hosts} is not changed and keeps its drivers, and you pair your devices with Rubylight separately."
        ),
        replace: format!("Replace {hosts} (uninstalls it)"),
        keep: format!("Keep {hosts} and install alongside"),
    })
}
/// Why a side-by-side installation is not converted in place. The other host
/// is untouched, and its import needs an empty profile.
pub fn conversion_refusal(names: &[String]) -> String {
    let hosts = if names.is_empty() {
        "another host".to_owned()
    } else {
        in_words(names)
    };
    format!(
        "Rubylight stays side by side: setup does not convert it in place. To replace {hosts} instead, uninstall Rubylight and delete its settings when asked, then run setup again and choose Replace. Nothing has been changed."
    )
}
/// A short description for the confirmation dialog. `side_by_side`: this run
/// keeps other hosts; `in_place`: it updates an installed Rubylight, which
/// never removes another host.
pub fn summary(
    found: &Found,
    install: &std::path::Path,
    side_by_side: bool,
    in_place: bool,
) -> String {
    let mut lines = Vec::new();
    if let Some(ours) = &found.butterpollo {
        lines.push(format!("Rubylight {} will be updated.", ours.version));
    } else if found.service_install.is_some() {
        lines.push("The Rubylight host already running as a service will be updated.".to_owned());
    }
    if let (Some(current), Some(_)) = (found.current(), &found.relocate) {
        let why = "its service runs as SYSTEM, and users other than administrators could change that folder";
        lines.push(if same_folder(current, install) {
            format!(
                "Rubylight cannot stay in {}: {why}. Choose another folder.",
                current.display()
            )
        } else {
            format!(
                "Rubylight moves out of {}: {why}. Settings and paired devices are kept. Nothing uses {} afterwards, and you can delete it.",
                current.display(),
                current.display()
            )
        });
    }
    let hosts = in_words(&found.other_hosts());
    if side_by_side {
        lines.push(format!(
            "Rubylight runs side by side, with its own service ({}), its own ports and its own settings; nothing is imported.",
            host_service::SIDE_BY_SIDE
        ));
        lines.push(if hosts.is_empty() {
            "Other hosts and the drivers they use are never changed.".to_owned()
        } else {
            format!("{hosts} and its drivers are not changed.")
        });
    } else if in_place && !hosts.is_empty() {
        lines.push(format!("This update does not change {hosts}."));
    }
    let replacing = if side_by_side || in_place {
        Vec::new()
    } else {
        found.packages.iter().chain(&found.legacy).collect()
    };
    for product in replacing {
        // Vibepollo's own entry carries the release version; its package
        // entry only the Windows Installer encoding (2.0.0-beta.3 -> 2.0.33.0).
        let version = found
            .vibepollo_entries
            .iter()
            .find(|entry| entry.location.is_some() && entry.location == product.location)
            .map_or(product.version.as_str(), |entry| entry.version.as_str());
        lines.push(if product.location.as_deref().is_some_and(has_profile) {
            format!(
                "{} {version} will be uninstalled and replaced. Its settings, paired devices, apps and drivers are kept.",
                product.name
            )
        } else {
            format!(
                "{} {version} will be uninstalled and replaced. Its settings were not found, so they cannot be kept.",
                product.name
            )
        });
    }
    lines.push(format!("Install folder: {}", install.display()));
    // SignPath's conditions: say what changes on the system, and where the
    // privacy statement is, before installing.
    lines.push(String::new());
    lines.push(SYSTEM_CHANGES.to_owned());
    lines.join("\n")
}
/// What setup changes beyond the install folder, and what Rubylight sends.
pub const SYSTEM_CHANGES: &str = "Setup installs the Rubylight service, a firewall rule for streaming and a virtual display driver. It adds certificates to Windows so that driver and the local web console are trusted; uninstalling removes them. Rubylight checks GitHub for updates once a day; you can turn that off in Settings. Privacy: ramazankara.github.io/Rubylight/docs/privacy.html";

#[cfg(test)]
mod tests {
    use super::*;
    fn product(name: &str, key: &str) -> Product {
        Product {
            key: key.into(),
            name: name.into(),
            version: "2.0.0".into(),
            location: None,
            uninstall: None,
            quiet_uninstall: None,
            msi: key.starts_with('{'),
            root: HKEY_LOCAL_MACHINE,
        }
    }
    fn program(path: &str) -> Option<PathBuf> {
        Some(PathBuf::from(path))
    }
    #[test]
    fn an_apollo_service_with_another_program_is_never_rubylights() {
        let theirs = |name: &str| {
            (name == "ApolloService").then(|| PathBuf::from(r"C:\Vibepollo\tools\sunshinesvc.exe"))
        };
        assert!(!our_service(Path::new(
            r"C:\Vibepollo\tools\sunshinesvc.exe"
        )));
        assert!(our_service(Path::new(
            r"C:\Program Files\Rubylight\Butterpollo-Service.EXE"
        )));
        assert_eq!(other_services(theirs), ["ApolloService"]);
        let ours = |name: &str| {
            (name == "ApolloService")
                .then(|| PathBuf::from(r"C:\Program Files\Rubylight\butterpollo-service.exe"))
        };
        assert!(other_services(ours).is_empty());
        // Service names reach only what setup's scan and the uninstaller
        // check: the constant that named it unconditionally is gone.
        for source in [
            include_str!("install.rs"),
            include_str!("update.rs"),
            include_str!("uninstall.rs"),
        ] {
            let code = source.split("#[cfg(test)]").next().unwrap();
            assert!(!code.contains("\"ApolloService\""));
            assert!(!code.contains("detect::SERVICE"));
        }
    }
    #[test]
    fn the_record_or_rubylights_own_service_marks_side_by_side() {
        let none = |_: &str| None;
        assert_eq!(service_named(None, none), host_service::DEFAULT);
        assert_eq!(
            service_named(Some("RubylightService"), none),
            host_service::SIDE_BY_SIDE
        );
        // The record lost: Rubylight's service still marks the installation.
        let side = |name: &str| {
            (name == host_service::SIDE_BY_SIDE)
                .then(|| PathBuf::from(r"C:\Rubylight\butterpollo-service.exe"))
        };
        assert_eq!(service_named(None, side), host_service::SIDE_BY_SIDE);
        // Someone else's service of that name says nothing.
        let other = |_: &str| program(r"C:\Other\other.exe");
        assert_eq!(service_named(None, other), host_service::DEFAULT);
    }
    /// Replace stays the default of existing upgrades. An installation stays
    /// what it is, whatever a later run asks.
    #[test]
    fn the_mode_follows_the_installation_then_the_request() {
        let rubylight = Some(PathBuf::from(r"C:\Program Files\Rubylight"));
        let none = Found::default();
        assert!(!none.side_by_side(false).unwrap());
        // Without another host too, the flag gives the same layout.
        assert!(none.side_by_side(true).unwrap());
        let normal = Found {
            service: host_service::DEFAULT,
            service_install: rubylight.clone(),
            ..Default::default()
        };
        assert!(!normal.side_by_side(false).unwrap());
        let refused = format!("{:#}", normal.side_by_side(true).unwrap_err());
        assert!(refused.contains("Nothing has been changed"), "{refused}");
        assert!(refused.contains("Uninstall Rubylight first"), "{refused}");
        let side = Found {
            service: host_service::SIDE_BY_SIDE,
            service_install: rubylight,
            ..Default::default()
        };
        assert!(side.installed_side_by_side());
        assert!(side.side_by_side(false).unwrap());
        assert!(side.side_by_side(true).unwrap());
        // A record left by a failed setup, with no Rubylight: nothing to keep.
        let stale = Found {
            service: host_service::SIDE_BY_SIDE,
            ..Default::default()
        };
        assert!(!stale.installed_side_by_side());
        assert!(!stale.side_by_side(false).unwrap());
    }
    #[test]
    fn the_choice_names_every_detected_host_and_says_replace_uninstalls() {
        assert!(host_choice(&Found::default()).is_none());
        let found = Found {
            packages: vec![product("Vibepollo", "{0A1B}")],
            vibepollo_entries: vec![product("Vibepollo", "Vibepollo")],
            legacy: vec![product("Apollo", "Apollo")],
            ..Default::default()
        };
        assert_eq!(found.other_hosts(), ["Vibepollo", "Apollo"]);
        let choice = host_choice(&found).unwrap();
        assert_eq!(choice.heading, "Replace Vibepollo and Apollo?");
        assert!(choice.replace.contains("uninstalls"), "{}", choice.replace);
        assert!(choice.keep.contains("alongside"), "{}", choice.keep);
        assert!(
            choice
                .text
                .contains("setup uninstalls Vibepollo and Apollo"),
            "{}",
            choice.text
        );
        assert!(choice.text.contains("new settings"), "{}", choice.text);
        // A service alone counts, and its name is all setup has to show.
        let service = Found {
            other_services: vec!["ApolloService"],
            ..Default::default()
        };
        let choice = host_choice(&service).unwrap();
        assert!(
            choice.heading.contains("ApolloService"),
            "{}",
            choice.heading
        );
    }
    #[test]
    fn side_by_side_promises_no_change_and_no_replacement() {
        let found = Found {
            legacy: vec![product("Sunshine", "Sunshine")],
            ..Default::default()
        };
        let folder = default_folder();
        let replacing = summary(&found, &folder, false, false);
        assert!(replacing.contains("will be uninstalled and replaced"));
        let keeping = summary(&found, &folder, true, false);
        assert!(!keeping.contains("replaced"), "{keeping}");
        assert!(keeping.contains("Sunshine and its drivers are not changed"));
        assert!(keeping.contains("RubylightService"), "{keeping}");
        assert!(keeping.contains("nothing is imported"), "{keeping}");
        assert!(keeping.ends_with(SYSTEM_CHANGES));
        // An update of an installed Rubylight removes no host, and says so.
        let updating = summary(&found, &folder, false, true);
        assert!(!updating.contains("replaced"), "{updating}");
        assert!(updating.contains("does not change Sunshine"), "{updating}");
    }
    #[test]
    fn a_conversion_is_refused_with_the_way_to_do_it() {
        let text = conversion_refusal(&["Vibepollo".to_owned()]);
        assert!(text.contains("To replace Vibepollo"), "{text}");
        assert!(text.contains("uninstall Rubylight"), "{text}");
        assert!(text.contains("Nothing has been changed"), "{text}");
    }
    #[test]
    fn the_summary_names_system_changes_and_the_privacy_statement() {
        let text = summary(&Found::default(), &default_folder(), false, false);
        assert!(text.ends_with(SYSTEM_CHANGES), "{text}");
        assert!(text.contains("service") && text.contains("firewall rule"));
        assert!(text.contains("certificates") && text.contains("removes them"));
        assert!(text.contains("updates") && text.contains("privacy.html"));
    }
    #[test]
    fn a_picked_folder_is_used_only_when_empty_or_already_rubylight() {
        let root = tempfile::tempdir().unwrap();
        let empty = root.path().join("empty");
        std::fs::create_dir(&empty).unwrap();
        assert_eq!(chosen_folder(&empty), empty);
        let missing = root.path().join("new");
        assert_eq!(chosen_folder(&missing), missing);
        let games = root.path().join("games");
        std::fs::create_dir(&games).unwrap();
        std::fs::write(games.join("game.exe"), b"").unwrap();
        assert_eq!(chosen_folder(&games), games.join("Rubylight"));
        let ours = root.path().join("ours");
        std::fs::create_dir(&ours).unwrap();
        std::fs::write(ours.join("manifest.json"), b"[]").unwrap();
        assert_eq!(chosen_folder(&ours), ours);
        let drive = std::path::Path::new("C:\\");
        assert_eq!(chosen_folder(drive), drive.join("Rubylight"));
    }
    #[test]
    fn an_installation_that_cannot_stay_offers_program_files_instead() {
        let root = tempfile::tempdir().unwrap();
        let installed = root.path().join("Games").join("Rubylight");
        std::fs::create_dir_all(&installed).unwrap();
        let mut found = Found {
            service_install: Some(installed.clone()),
            ..Default::default()
        };
        assert_eq!(install_dir(&found, None), installed);
        assert!(!folder_choosable(&found));
        assert!(!summary(&found, &installed, false, false).contains("Rubylight moves out of"));
        found.relocate = Some("Games must be owned by Administrators".into());
        assert_eq!(install_dir(&found, None), default_folder());
        assert!(folder_choosable(&found));
        let text = summary(&found, &default_folder(), false, false);
        assert!(
            text.contains(&format!(
                "Rubylight moves out of {}: its service runs as SYSTEM",
                installed.display()
            )),
            "{text}"
        );
        assert!(
            text.contains("Settings and paired devices are kept."),
            "{text}"
        );
        // Picked again, the folder is refused before setup starts.
        assert!(summary(&found, &installed, false, false).contains("Choose another folder."));
        // A folder chosen on the command line is still used.
        let requested = root.path().join("Requested");
        assert_eq!(install_dir(&found, Some(requested.clone())), requested);
        // Without a service, the Apps entry names the folder.
        let entry = Found {
            butterpollo: Some(Product {
                key: "Butterpollo".into(),
                name: "Rubylight".into(),
                version: "2.1.0".into(),
                location: Some(installed.clone()),
                uninstall: None,
                quiet_uninstall: None,
                msi: false,
                root: HKEY_LOCAL_MACHINE,
            }),
            ..Default::default()
        };
        assert_eq!(entry.current(), Some(installed.as_path()));
        assert_eq!(install_dir(&entry, None), installed);
        assert!(!folder_choosable(&entry));
    }
    #[test]
    fn upgrades_and_reinstalls_are_allowed_but_downgrades_are_not() {
        for (installed, incoming) in [
            ("2.0.0-rc.9", "2.0.0-rc.10"),
            ("2.0.0-rc.21", "2.0.0-rc.21"),
            ("2.0.0-rc.21", "2.0.0"),
            ("v2.0.0-rc.21+build.5", "2.0.0-rc.21"),
            ("", "2.0.0-rc.21"),
            ("unknown", "2.0.0-rc.21"),
        ] {
            assert!(check_version(installed, incoming).is_ok());
        }
        for (installed, incoming) in [
            ("2.0.0-rc.10", "2.0.0-rc.9"),
            ("2.0.0", "2.0.0-rc.21"),
            ("2.1.0", "2.0.0"),
            ("2.0.0-rc.21", "2.0.0-rc.9"),
        ] {
            assert!(check_version(installed, incoming).is_err());
        }
    }
    #[test]
    fn locations_come_from_the_icon_the_uninstaller_or_the_service() {
        let path = |p: &str| Some(PathBuf::from(p));
        let sunshine = path(r"C:\Program Files\Sunshine");
        assert_eq!(
            resolve_location(Some(r" C:\Program Files\Sunshine\ "), None, None),
            sunshine
        );
        for (icon, uninstall) in [
            (Some(r"C:\Program Files\Sunshine\sunshine.exe,0"), None),
            (Some(r#""C:\Program Files\Sunshine\sunshine.exe",0"#), None),
            (Some(r"C:\Program Files\Sunshine\sunshine.ico"), None),
            (
                Some(""),
                Some(r#""C:\Program Files\Sunshine\uninstall.exe" /S"#),
            ),
            (None, Some(r"C:\Program Files\Sunshine\Uninstall.EXE /S")),
            (
                Some(r"C:\Windows\Installer\{0A1B}\ProductIcon.ico"),
                Some(r"C:\Program Files\Sunshine\tools\uninstall.exe"),
            ),
        ] {
            assert_eq!(
                resolve_location(Some("  "), icon, uninstall),
                sunshine,
                "{icon:?} {uninstall:?}"
            );
        }
        // Windows Installer's own programs and relative paths name no folder.
        assert_eq!(
            resolve_location(
                None,
                Some(r"C:\WINDOWS\Installer\{0A1B}\icon.ico,0"),
                Some(r"MsiExec.exe /X{0A1B}")
            ),
            None
        );
        assert_eq!(
            resolve_location(
                None,
                Some("sunshine.exe"),
                Some(r"C:\Windows\System32\msiexec.exe /X{0A1B}")
            ),
            None
        );
        assert_eq!(
            program_folder(Path::new(r"C:\Program Files\Apollo\tools\sunshinesvc.exe")),
            path(r"C:\Program Files\Apollo")
        );
    }
    #[test]
    fn the_summary_promises_settings_only_when_there_are_some() {
        let root = tempfile::tempdir().unwrap();
        let product = Product {
            key: "{0A1B}".into(),
            name: "Apollo".into(),
            version: "0.4.6".into(),
            location: Some(root.path().to_path_buf()),
            uninstall: None,
            quiet_uninstall: None,
            msi: true,
            root: HKEY_LOCAL_MACHINE,
        };
        let found = Found {
            packages: vec![product],
            ..Default::default()
        };
        let install = Path::new(r"C:\Program Files\Butterpollo");
        assert!(summary(&found, install, false, false).contains("Its settings were not found"));
        std::fs::create_dir(root.path().join("config")).unwrap();
        std::fs::write(root.path().join("config").join("sunshine.conf"), "").unwrap();
        assert!(
            summary(&found, install, false, false)
                .contains("Its settings, paired devices, apps and drivers are kept.")
        );
    }
}
