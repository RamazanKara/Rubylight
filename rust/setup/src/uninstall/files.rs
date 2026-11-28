//! Profile, Vulkan layer, package and staging folder removal during uninstall.
use super::is_package;
use crate::{log::line, payload, system};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};
use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, KEY_WOW64_64KEY};

/// The note for a factory reset that left `left` (from [`remove_profile`])
/// in `root`; None when it removed everything.
pub(super) fn profile_note(root: &Path, left: &[PathBuf]) -> Option<String> {
    const LISTED: usize = 5;
    if left.is_empty() {
        return None;
    }
    let mut paths: Vec<String> = left
        .iter()
        .take(LISTED)
        .map(|path| path.display().to_string())
        .collect();
    if left.len() > LISTED {
        paths.push(format!("{} more", left.len() - LISTED));
    }
    Some(format!(
        "The factory reset could not delete everything in {}, so credentials, paired devices or their certificates may remain on this PC. Still there: {}. Close the programs using them and delete that folder.",
        root.display(),
        paths.join(", ")
    ))
}
/// Delete `root` for a factory reset, all but `keep` (files inside it).
/// Every step is tried; what is still there afterwards is returned.
pub(super) fn remove_profile(root: &Path, keep: &[PathBuf]) -> Vec<PathBuf> {
    let keep: Vec<&PathBuf> = keep
        .iter()
        .filter(|keep| keep.starts_with(root) && keep.is_file())
        .collect();
    remove_profile_entries(root, &keep);
    let mut left = Vec::new();
    leftovers(root, &keep, &mut left);
    left
}
/// One file that cannot be deleted (open in an indexer or antivirus) stops
/// `remove_dir_all`; the rest of that folder is then deleted one by one.
pub(super) fn remove_profile_entries(root: &Path, keep: &[&PathBuf]) {
    if keep.is_empty() && std::fs::remove_dir_all(root).is_ok() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if keep.contains(&&path) {
            continue;
        }
        let inside: Vec<&PathBuf> = keep
            .iter()
            .copied()
            .filter(|keep| keep.starts_with(&path))
            .collect();
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            remove_profile_entries(&path, &inside);
        } else {
            // A link to a folder goes with remove_dir on Windows.
            let _ = std::fs::remove_file(&path).or_else(|_| std::fs::remove_dir(&path));
        }
    }
    if keep.is_empty() {
        let _ = std::fs::remove_dir(root);
    }
}
/// What is still in `root` but `keep`: files, links, and folders that could
/// not be read; folders only through what is in them. Links are not followed.
pub(super) fn leftovers(root: &Path, keep: &[&PathBuf], left: &mut Vec<PathBuf>) {
    match root.symlink_metadata() {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Ok(metadata) if metadata.is_dir() => {}
        _ => {
            left.push(root.to_path_buf());
            return;
        }
    }
    let Ok(entries) = std::fs::read_dir(root) else {
        left.push(root.to_path_buf());
        return;
    };
    for entry in entries {
        let Ok(entry) = entry else {
            left.push(root.to_path_buf());
            return;
        };
        let path = entry.path();
        if keep.contains(&&path) {
            continue;
        }
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            leftovers(&path, keep, left);
        } else {
            left.push(path);
        }
    }
}
/// The HDR Vulkan layer that the host registers from `install` while it runs.
pub(crate) fn remove_layers(install: &Path) {
    for value in system::values(HKEY_LOCAL_MACHINE, LAYERS, KEY_WOW64_64KEY) {
        if layer_in(&value, install) {
            system::delete_value(HKEY_LOCAL_MACHINE, LAYERS, &value, KEY_WOW64_64KEY);
        }
    }
    remove_stale_layers();
}
pub(super) const LAYERS: &str = "SOFTWARE\\Khronos\\Vulkan\\ImplicitLayers";
pub(super) const LAYER_MANIFEST: &str = "VkLayer_butterpollo_hdr.json";
/// Earlier install folders' HDR layers whose manifest is gone (the
/// ButterpolloRust folder before the rename): every Vulkan program would
/// still look for them.
pub(crate) fn remove_stale_layers() {
    for value in system::values(HKEY_LOCAL_MACHINE, LAYERS, KEY_WOW64_64KEY) {
        if stale_layer(&value, |path| path.exists()) {
            system::delete_value(HKEY_LOCAL_MACHINE, LAYERS, &value, KEY_WOW64_64KEY);
        }
    }
}
/// A layer value as a plain path: the host registers the folder it runs
/// from, which is verbatim (`\\?\C:\...`) when the service started it so.
pub(super) fn plain(value: &str) -> String {
    match value.strip_prefix(r"\\?\UNC\") {
        Some(share) => format!(r"\\{share}"),
        None => value.strip_prefix(r"\\?\").unwrap_or(value).to_owned(),
    }
}
pub(super) fn layer_in(value: &str, install: &Path) -> bool {
    // With the separator, so removing ...\Butterpollo keeps ...\Butterpollo2's layer.
    let prefix = format!(
        "{}\\",
        plain(&install.display().to_string()).trim_end_matches('\\')
    )
    .to_ascii_lowercase();
    plain(value).to_ascii_lowercase().starts_with(&prefix)
}
pub(super) fn stale_layer(value: &str, exists: impl Fn(&Path) -> bool) -> bool {
    let path = PathBuf::from(plain(value));
    path.file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case(LAYER_MANIFEST))
        && !exists(&path)
}
/// The folder Rubylight installed into before its rename. Once nothing runs
/// from it, what can remain is empty folders (files deleted at a restart
/// leave theirs behind); they go, anything else in it stays.
pub(crate) fn remove_earlier_folder(install: &Path) {
    let earlier = system::program_files().join("ButterpolloRust");
    if !plain(&install.display().to_string()).eq_ignore_ascii_case(&earlier.display().to_string())
        && !is_package(&earlier)
    {
        remove_empty_folders(&earlier);
    }
}
/// Delete `folder` and its subfolders, deepest first, as long as they are
/// empty; a folder holding any file stays, with the folders above it. Links
/// are not followed: an empty folder elsewhere is not ours.
pub(super) fn remove_empty_folders(folder: &Path) {
    if !std::fs::symlink_metadata(folder).is_ok_and(|meta| meta.is_dir()) {
        return;
    }
    if let Ok(entries) = std::fs::read_dir(folder) {
        for entry in entries.flatten() {
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                remove_empty_folders(&entry.path());
            }
        }
    }
    let _ = std::fs::remove_dir(folder);
}
/// Whether `a` and `b` name the same file (handling verbatim prefixes, case,
/// and 8.3 short names when both exist on disk).
pub(super) fn same_file_path(a: &Path, b: &Path) -> bool {
    if let (Ok(ca), Ok(cb)) = (a.canonicalize(), b.canonicalize()) {
        return ca == cb;
    }
    plain(&a.display().to_string())
        .replace('/', "\\")
        .eq_ignore_ascii_case(&plain(&b.display().to_string()).replace('/', "\\"))
}
/// Delete what the installer put in `install`: the manifest's files, the
/// manifest, the uninstaller and driver folders carried over from Vibepollo.
/// Anything else stays, and so does every folder that is not empty then: the
/// install folder may also hold the user's own files. Returns the folders that
/// held a package file.
#[cfg(test)]
pub(super) fn remove_package(install: &Path) -> BTreeSet<PathBuf> {
    remove_package_with(install, None, &mut system::remove_file_later)
}
/// [`remove_package`] skipping `skip` (the running `uninstall.exe`, which the
/// post-exit `cmd.exe` step removes without queuing a reboot deletion) and
/// delegating file deletions to `later`.
pub(super) fn remove_package_with(
    install: &Path,
    skip: Option<&Path>,
    later: &mut dyn FnMut(&Path),
) -> BTreeSet<PathBuf> {
    let mut folders = BTreeSet::new();
    let mut remove = |path: PathBuf| {
        folders.extend(
            path.ancestors()
                .skip(1)
                .take_while(|folder| *folder != install && folder.starts_with(install))
                .map(Path::to_path_buf),
        );
        if skip.is_some_and(|skipped| same_file_path(&path, skipped)) {
            return;
        }
        later(&path);
    };
    if let Ok(entries) = payload::manifest(install) {
        for entry in entries {
            if let Ok(path) = payload::safe_join(install, &entry.path) {
                remove(path);
            }
        }
    }
    // Drivers carried over from Vibepollo are not in the manifest; only the
    // installer's own driver folders go, recognised by their script.
    for folder in ["drivers\\display", "drivers\\gamepad"] {
        let folder = install.join(folder);
        if folder.join("install.ps1").is_file()
            && let Ok(files) = files_in(&folder)
        {
            files.into_iter().for_each(&mut remove);
        }
    }
    remove(install.join("manifest.json"));
    remove(install.join("uninstall.exe"));
    // Deepest first, so a parent is empty by the time it comes up.
    for folder in folders.iter().rev() {
        let _ = std::fs::remove_dir(folder);
    }
    folders
}
/// Folders earlier versions shipped that are Rubylight's alone, with whatever
/// an update left in them: the Intel VPL (libvpl) licenses of 2.6.x.
pub(super) const LEGACY_FOLDERS: [&str; 1] = ["licenses\\libvpl"];
/// What `remove_package` leaves of the install folder besides the user's own
/// files: the legacy folders no manifest lists any more (an update deletes the
/// files a new version stops shipping, not their folders, and 3.0 stopped
/// shipping libvpl's), setup's staging folder, and the folders that were `touched` (held a package
/// file) or hold a legacy folder, once they are empty. Other empty folders are
/// the user's, in a folder they may have chosen themselves (D:\Games), and stay,
/// as does everything when `install` is a drive root. Everything stays inside
/// `install`: a link is removed itself, never followed.
pub(super) fn remove_leftover_folders(install: &Path, touched: &BTreeSet<PathBuf>) {
    if install.parent().is_none() {
        return;
    }
    let mut folders = touched.clone();
    for legacy in LEGACY_FOLDERS {
        // Every step must be a real folder: a junction at `licenses` would
        // lead the removal out of the install folder.
        let mut folder = install.to_path_buf();
        let real = Path::new(legacy).components().all(|part| {
            folder.push(part);
            std::fs::symlink_metadata(&folder).is_ok_and(|meta| meta.is_dir())
        });
        if real {
            let _ = std::fs::remove_dir_all(&folder);
            folders.extend(
                folder
                    .ancestors()
                    .take_while(|folder| *folder != install && folder.starts_with(install))
                    .map(Path::to_path_buf),
            );
        }
    }
    remove_staging(&install.join(STAGING), &mut system::remove_file_later);
    // Deepest first, so a parent is empty by the time it comes up.
    for folder in folders.iter().rev() {
        let _ = std::fs::remove_dir(folder);
    }
}
/// Setup's staging folder in the install folder: packages being installed,
/// the previous version a transaction keeps for its rollback, and the
/// updater's downloads. No manifest lists it.
pub(super) const STAGING: &str = ".staging";
/// Delete the staging folder with everything in it: it is Rubylight's alone.
/// A link there is removed itself, never followed; a file of that name stays.
/// What can't go now (a file in use) is handed to `later`, as the package's
/// files are, and every failure is logged.
pub(super) fn remove_staging(staging: &Path, later: &mut dyn FnMut(&Path)) {
    match std::fs::symlink_metadata(staging) {
        Ok(meta) if meta.file_type().is_symlink() => remove_link(staging),
        // remove_dir_all removes the links inside without following them.
        Ok(meta) if meta.is_dir() => {
            if let Err(error) = std::fs::remove_dir_all(staging) {
                line(format!(
                    "warning: {} could not be removed now: {error}; what is left goes at the next restart",
                    staging.display()
                ));
                remove_tree_later(staging, later);
            }
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => line(format!(
            "warning: {} could not be read: {error}",
            staging.display()
        )),
    }
}
/// Remove the junction or symbolic link `link` itself: on Windows a link to a
/// folder goes with remove_dir, one to a file with remove_file.
pub(super) fn remove_link(link: &Path) {
    if let Err(error) = std::fs::remove_dir(link).or_else(|_| std::fs::remove_file(link)) {
        line(format!(
            "warning: the link {} could not be removed: {error}",
            link.display()
        ));
    }
}
/// Hand every file in `folder` to `later`, then each folder that isn't empty
/// by then, deepest first. A link is removed now or left: deleting it at a
/// restart might follow it.
pub(super) fn remove_tree_later(folder: &Path, later: &mut dyn FnMut(&Path)) {
    match std::fs::read_dir(folder) {
        Ok(entries) => {
            for entry in entries {
                match entry.and_then(|entry| Ok((entry.path(), entry.file_type()?))) {
                    Ok((path, kind)) if kind.is_symlink() => remove_link(&path),
                    Ok((path, kind)) if kind.is_dir() => remove_tree_later(&path, later),
                    Ok((path, _)) => later(&path),
                    Err(error) => line(format!(
                        "warning: listing {} failed: {error}",
                        folder.display()
                    )),
                }
            }
        }
        Err(error) => line(format!(
            "warning: listing {} failed: {error}",
            folder.display()
        )),
    }
    if std::fs::remove_dir(folder).is_err() {
        later(folder);
    }
}
pub(super) fn files_in(folder: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(folder)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            files.extend(files_in(&entry.path())?);
        } else {
            files.push(entry.path());
        }
    }
    Ok(files)
}
