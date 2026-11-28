//! Installation targeting, profile trust repair, migration checks and console CA trust.
use super::{moved_aside, secure_install};
use crate::{detect, host_service, log::line, payload, system};
use anyhow::{Context, Result, anyhow, bail};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

/// What setup does with the folder it installs into.
#[derive(Debug, PartialEq)]
pub(super) enum Target {
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
pub(super) fn target(found: &detect::Found, install: &Path) -> Result<Target> {
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
pub(super) fn canonical_prefix(path: &Path) -> PathBuf {
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
pub(super) fn folder_inside(install: &Path, root: &Path) -> Result<bool> {
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
pub(super) fn outside_other_hosts(found: &detect::Found, install: &Path) -> Result<()> {
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
pub(super) fn refuse_foreign_service(program: Option<&Path>) -> Result<()> {
    match program {
        Some(program) if !detect::our_service(program) => bail!(
            "A service named {} already exists and runs {}, which is not Rubylight. Setup does not stop or change it, and Rubylight needs that name to install side by side. Remove that service, then run setup again. Nothing has been changed.",
            host_service::SIDE_BY_SIDE,
            program.display()
        ),
        _ => Ok(()),
    }
}
pub(super) fn profile_empty(profile: &Path) -> Result<bool> {
    match std::fs::read_dir(profile) {
        Ok(mut entries) => Ok(entries.next().transpose()?.is_none()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(error) => Err(error).context("checking the existing Rubylight profile"),
    }
}
pub(super) fn migration_source(
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
#[cfg(test)]
pub(super) fn undo_import_on_error(
    profile: &Path,
    importing: bool,
    steps: impl FnOnce() -> Result<()>,
) -> Result<()> {
    let mut earlier = None;
    undo_profile_on_error(profile, importing, &mut earlier, |_, _| steps())
}
/// Runs profile setup/import and package copy before the previous host is
/// removed. On error, removes any profile created or imported in this run and
/// restores an earlier profile moved aside for side-by-side (`earlier`) back
/// to `profile`.
pub(super) fn undo_profile_on_error(
    profile: &Path,
    mut created_profile: bool,
    earlier: &mut Option<PathBuf>,
    steps: impl FnOnce(&mut bool, &mut Option<PathBuf>) -> Result<()>,
) -> Result<()> {
    let Err(error) = steps(&mut created_profile, earlier) else {
        return Ok(());
    };
    if !created_profile && earlier.is_none() {
        return Err(error);
    }
    match std::fs::remove_dir_all(profile) {
        Ok(()) => {}
        Err(undo) if undo.kind() == std::io::ErrorKind::NotFound => {}
        Err(undo) => {
            return Err(anyhow!(
                "{error:#}\n\nThe settings setup imported into {} could not be removed again ({undo}). Delete that folder, then run setup again.",
                profile.display()
            ));
        }
    }
    if let Some(aside) = earlier.as_ref() {
        match std::fs::rename(aside, profile) {
            Ok(()) => *earlier = None,
            Err(undo) => {
                return Err(anyhow!(
                    "{error:#}\n\nThe earlier settings moved to {} could not be restored to {} ({undo}).",
                    aside.display(),
                    profile.display()
                ));
            }
        }
    }
    Err(error)
}
/// Stop `services`, restarting those that were running if any stop fails.
pub(super) fn stop_running<F: FnMut(&str) -> Result<()>>(
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
pub(super) struct Restart<F: FnMut(&str) -> Result<()>> {
    pub(super) services: Vec<&'static str>,
    pub(super) start: F,
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
pub(super) fn nvprefs_undo_pending(program_data: &Path, previous: &Path) -> bool {
    program_data.join("Sunshine\\nvprefs_undo.json").is_file()
        && previous.join("sunshine.exe").is_file()
}
pub(super) fn import_error(output: &str) -> Option<String> {
    let lines: Vec<&str> = output.lines().collect();
    let start = lines.iter().rposition(|line| line.starts_with("Error: "))?;
    let error = lines[start..].join("\n");
    Some(error["Error: ".len()..].trim().to_owned())
}
/// SYSTEM and Administrators control the service profile, users may read
/// it (the launcher reads the port); credentials are not readable by users.
/// The users' Nucleus copies keep the permissions the host gave them, which
/// let each user write in their own.
pub(super) fn secure_profile(profile: &Path) -> Result<()> {
    let root = profile.parent().context("profile has no parent")?;
    let nucleus = nucleus_user_root(root);
    let credentials = profile.join("credentials");
    let backups = profile.join("backups");
    let root_opaque = [nucleus, profile.to_path_buf()];
    let profile_opaque = [credentials, backups];
    let opaque_for = |folder: &Path| -> &[PathBuf] {
        if folder == root {
            &root_opaque
        } else if folder == profile {
            &profile_opaque
        } else {
            &[]
        }
    };
    secure_profile_with(
        profile,
        |folder, users_read| {
            if std::fs::symlink_metadata(folder).is_err() {
                system::create_restricted(folder, users_read)
            } else {
                system::restrict_except(folder, users_read, opaque_for(folder))
            }
        },
        |folder| system::inherit_contents(folder, opaque_for(folder)),
    )
}
pub(super) fn secure_profile_with(
    profile: &Path,
    mut restrict: impl FnMut(&Path, bool) -> Result<()>,
    mut inherit: impl FnMut(&Path) -> Result<()>,
) -> Result<()> {
    let root = profile.parent().context("profile has no parent")?;
    std::fs::create_dir_all(root)?;
    restrict(root, true)?;
    restrict(profile, true)?;
    let credentials = profile.join("credentials");
    let backups = profile.join("backups");
    // Lock private folders directly without ever resetting their ACLs to
    // inherit users' read access from `profile` or `root`.
    restrict(&credentials, false)?;
    restrict(&backups, false)?;
    let mut result = Ok(());
    for folder in [
        &credentials,
        &backups,
        &profile.to_path_buf(),
        &root.to_path_buf(),
    ] {
        if let Err(error) = inherit(folder)
            && result.is_ok()
        {
            result = Err(error);
        }
    }
    result
}
/// The name of the root the host makes for the console's certificate
/// (`butterpollo_crypto::crypto::CONSOLE_ROOT_NAME`). certutil's `-delstore`
/// by this name also removes the roots whose name adds a host's own unit.
pub(super) const CONSOLE_ROOT_NAME: &str = "Rubylight local console";
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
pub(super) fn trust_console(profile: &Path) -> Result<()> {
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
/// (`butterpollo_session::nucleus::install::user_root`).
pub(super) fn nucleus_user_root(root: &Path) -> PathBuf {
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
pub(super) fn repair_profile(profile: &Path) -> Result<()> {
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
                    "an administrator can delete {} (Rubylight copies the co-op helper for each user again), then run setup again",
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
