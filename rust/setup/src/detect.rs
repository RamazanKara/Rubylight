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
pub use host_service::our_service;
/// The service this PC's Rubylight runs as.
pub fn service() -> &'static str {
    host_service::service_named(
        system::registry_string(HKEY_LOCAL_MACHINE, host_service::KEY, host_service::VALUE)
            .as_deref(),
        service_program,
    )
}
/// Whether this PC's Rubylight keeps other hosts: it never stops or removes
/// them, nor the drivers they use.
pub fn side_by_side() -> bool {
    let service = service();
    keeps_other_hosts(
        service,
        service_program(service).is_some_and(|p| our_service(&p)),
        side_by_side_profile(),
    )
}
/// What says an installation keeps other hosts: the record or its service
/// ([`service`]), else, with both lost, the mark in the profile side by side
/// made. A Rubylight running as the default service is a normal one, whatever
/// profile an earlier uninstall left it.
fn keeps_other_hosts(service: &str, service_is_ours: bool, marked: bool) -> bool {
    service == host_service::SIDE_BY_SIDE || (marked && !service_is_ours)
}
fn side_by_side_profile() -> bool {
    crate::install::profile()
        .join(crate::install::SIDE_BY_SIDE_MARK)
        .is_file()
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
/// Where the previous hosts' services that are not Rubylight's run from.
fn other_service_folders(
    program: impl Fn(&str) -> Option<PathBuf>,
) -> Vec<(&'static str, PathBuf)> {
    other_services(&program)
        .into_iter()
        .filter_map(|name| Some((name, program_folder(&program(name)?)?)))
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
    /// Those services' folders, for a host known only by its service.
    pub other_service_folders: Vec<(&'static str, PathBuf)>,
    /// Whether the profile carries side by side's mark.
    pub side_by_side_profile: bool,
}
impl Found {
    /// Whether a Rubylight is installed here that keeps other hosts.
    pub fn installed_side_by_side(&self) -> bool {
        self.current().is_some()
            && keeps_other_hosts(
                self.service,
                self.service_install.is_some(),
                self.side_by_side_profile,
            )
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
    found.other_service_folders = other_service_folders(service_program);
    found.side_by_side_profile = side_by_side_profile();
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
mod tests;
