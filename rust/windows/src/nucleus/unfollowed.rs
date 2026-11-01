//! Writing below a folder of Nucleus Co-op's without following a link. The
//! host writes Nucleus' own folder as SYSTEM, so a junction or symbolic link
//! below it would point the write elsewhere; the user's copy and profile are
//! written as the user, never through a link either. Each folder from the
//! caller's one (`root`, which may itself be reached through a link) down to
//! the file is opened by a handle that never follows a link and stays open,
//! and must be where its path says (Windows' own path for it,
//! [`crate::user_folder::final_path`]). A file is written whole to a new
//! temporary one there, on the disk before its handle renames it over the
//! name, and removed by its handle.
use anyhow::{Context, Result, bail, ensure};
use std::ffi::OsStr;
use std::fs::File;
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf};
use windows::Win32::Storage::FileSystem::{
    DELETE, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_GENERIC_WRITE,
    FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES, FILE_RENAME_INFO, FILE_SHARE_DELETE,
    FILE_SHARE_READ, FILE_SHARE_WRITE, FileRenameInfo, SetFileInformationByHandle,
};

const REPARSE_POINT: u32 = 0x400;
const DIRECTORY: u32 = 0x10;

/// Write `bytes` to `path` below `root` whole: a crash or power loss never
/// leaves half a file, and Nucleus never reads one. Its folder must exist.
pub(super) fn write(root: &Path, path: &Path, bytes: &[u8]) -> Result<()> {
    let (relative, name) = named(root, path)?;
    let folders = folders(
        root,
        relative.parent().unwrap_or(Path::new("")),
        path,
        false,
    )?;
    let Some(folder) = folders.last() else {
        bail!("{} doesn't exist", path.parent().unwrap_or(path).display());
    };
    let mut temporary_name = name.to_os_string();
    temporary_name.push(".rubylight");
    let shown = path.with_file_name(&temporary_name);
    // Replaced by name: a link there would be replaced, not followed, but
    // it says someone meant the write to go elsewhere.
    if let Some(target) = existing(folder, name, path)? {
        ensure!(
            !target.folder,
            "{} is a folder, so Rubylight doesn't write it: move it away",
            path.display()
        );
    }
    remove_in(folder, &temporary_name, &shown)?;
    let temporary = folder.resolved.join(&temporary_name);
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .access_mode(FILE_GENERIC_WRITE.0 | FILE_READ_ATTRIBUTES.0 | DELETE.0)
        .share_mode(0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(&temporary)
        .with_context(|| format!("writing {}", shown.display()))?;
    let written = (|| -> Result<()> {
        in_place(&file, &temporary, &shown)?;
        let mut writer = &file;
        std::io::Write::write_all(&mut writer, bytes)
            .and_then(|()| file.sync_all())
            .with_context(|| format!("writing {}", shown.display()))?;
        rename(&file, folder, name).with_context(|| format!("replacing {}", path.display()))?;
        in_place(&file, &folder.resolved.join(name), path)
    })();
    if written.is_err() {
        // Its own new file only: nothing else ever had this handle's name.
        let _ = crate::user_folder::delete_on_close(&file);
    }
    written
}

/// Remove the file `path` below `root`. False when it, or its folder, is
/// already gone.
pub(super) fn remove(root: &Path, path: &Path) -> Result<bool> {
    let (relative, name) = named(root, path)?;
    let folders = folders(
        root,
        relative.parent().unwrap_or(Path::new("")),
        path,
        false,
    )?;
    let Some(folder) = folders.last() else {
        return Ok(false);
    };
    remove_in(folder, name, path)
}

/// Create the folder `path` below `root` and those on its way, one at a time.
pub(super) fn create_folders(root: &Path, path: &Path) -> Result<()> {
    let (relative, _) = named(root, path)?;
    folders(root, relative, path, true).map(drop)
}

/// A folder on the way, open, and its path as Windows resolved it (with
/// `\\?\`). Open, it can't be renamed or deleted.
struct Folder {
    _handle: File,
    resolved: PathBuf,
}

/// `path` below `root`, by names only, and its last one.
fn named<'a>(root: &Path, path: &'a Path) -> Result<(&'a Path, &'a OsStr)> {
    let relative = path
        .strip_prefix(root)
        .with_context(|| format!("{} is not in {}", path.display(), root.display()))?;
    ensure!(
        relative
            .components()
            .all(|part| matches!(part, Component::Normal(_))),
        "{} goes through . or ..",
        path.display()
    );
    let name = relative
        .file_name()
        .with_context(|| format!("{} names no file", path.display()))?;
    Ok((relative, name))
}

/// `root` and each folder of `relative` below it, open; empty when one is
/// missing, unless `create` makes it. A link on the way is refused, naming
/// `path`, what the write was for.
fn folders(root: &Path, relative: &Path, path: &Path, create: bool) -> Result<Vec<Folder>> {
    // The caller's folder, by whatever way leads there.
    let handle = std::fs::OpenOptions::new()
        .access_mode(FILE_LIST_DIRECTORY.0 | FILE_READ_ATTRIBUTES.0)
        .share_mode((FILE_SHARE_READ | FILE_SHARE_WRITE).0)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0)
        .open(root);
    let handle = match handle {
        Ok(handle) => handle,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && !create => {
            return Ok(Vec::new());
        }
        Err(error) => return Err(error).with_context(|| format!("opening {}", root.display())),
    };
    let resolved = PathBuf::from(
        crate::user_folder::final_path(&handle)
            .with_context(|| format!("reading the path of {}", root.display()))?,
    );
    let mut folders = vec![Folder {
        _handle: handle,
        resolved,
    }];
    let mut shown = root.to_path_buf();
    for part in relative.components() {
        let name = part.as_os_str();
        shown.push(name);
        let parent = folders.last().expect("the root is first");
        let mut opened = child(parent, name, &shown, path)?;
        if opened.is_none() && create {
            match std::fs::create_dir(parent.resolved.join(name)) {
                Err(error) if error.kind() != std::io::ErrorKind::AlreadyExists => {
                    return Err(error).with_context(|| format!("creating {}", shown.display()));
                }
                _ => {}
            }
            opened = child(parent, name, &shown, path)?;
        }
        let Some(folder) = opened else {
            return if create {
                bail!("{} is gone as it was created", shown.display())
            } else {
                Ok(Vec::new())
            };
        };
        folders.push(folder);
    }
    Ok(folders)
}

/// The folder `name` in `parent` (`shown` to people), never through a link;
/// None when it is not there.
fn child(parent: &Folder, name: &OsStr, shown: &Path, path: &Path) -> Result<Option<Folder>> {
    let at = parent.resolved.join(name);
    let handle = match crate::process::open_unfollowed(&at, true) {
        Ok(handle) => handle,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("opening {}", shown.display())),
    };
    let attributes = handle
        .metadata()
        .with_context(|| format!("reading {}", shown.display()))?
        .file_attributes();
    if attributes & REPARSE_POINT != 0 {
        bail!(linked(shown, path));
    }
    ensure!(
        attributes & DIRECTORY != 0,
        "{} is a file, not a folder",
        shown.display()
    );
    in_place(&handle, &at, shown)?;
    Ok(Some(Folder {
        _handle: handle,
        resolved: at,
    }))
}

/// The file or folder `name` in `folder` (`shown` to people), open to be
/// deleted, never through a link: refused when it is one; None when it is
/// not there.
fn existing(folder: &Folder, name: &OsStr, shown: &Path) -> Result<Option<Existing>> {
    let at = folder.resolved.join(name);
    let file = std::fs::OpenOptions::new()
        .access_mode(DELETE.0 | FILE_READ_ATTRIBUTES.0)
        .share_mode((FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE).0)
        .custom_flags((FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS).0)
        .open(&at);
    let file = match file {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("opening {}", shown.display())),
    };
    let attributes = file
        .metadata()
        .with_context(|| format!("reading {}", shown.display()))?
        .file_attributes();
    if attributes & REPARSE_POINT != 0 {
        bail!(linked(shown, shown));
    }
    in_place(&file, &at, shown)?;
    Ok(Some(Existing {
        file,
        folder: attributes & DIRECTORY != 0,
    }))
}

struct Existing {
    file: File,
    folder: bool,
}

/// Remove the file `name` in `folder` (`shown` to people) by its handle:
/// refused when it is a link or a folder; false when it is not there.
fn remove_in(folder: &Folder, name: &OsStr, shown: &Path) -> Result<bool> {
    let Some(existing) = existing(folder, name, shown)? else {
        return Ok(false);
    };
    ensure!(
        !existing.folder,
        "{} is a folder, so Rubylight leaves it: move it away",
        shown.display()
    );
    crate::user_folder::delete_on_close(&existing.file)
        .with_context(|| format!("removing {}", shown.display()))?;
    Ok(true)
}

/// An error unless the open `file` is at `at` (`shown` to people) as Windows
/// resolves it: a folder on the way swapped for a link since it was checked
/// would put it elsewhere.
fn in_place(file: &File, at: &Path, shown: &Path) -> Result<()> {
    let resolved = crate::user_folder::final_path(file)
        .with_context(|| format!("reading the path of {}", shown.display()))?;
    if !crate::user_folder::same_name(&resolved, at.as_os_str()) {
        bail!(
            "{} leads to {} through a link, so Rubylight doesn't write there: move the link away",
            shown.display(),
            Path::new(&resolved).display()
        );
    }
    Ok(())
}

fn linked(link: &Path, path: &Path) -> String {
    if link == path {
        format!(
            "{} is a link to elsewhere, so Rubylight doesn't write through it: move it away",
            link.display()
        )
    } else {
        format!(
            "{} is a link to elsewhere, so Rubylight doesn't write {} through it: move it away",
            link.display(),
            path.display()
        )
    }
}

/// Rename the open file to `name` in `folder`, its own, over a file there.
/// By the folder's whole path, as Windows resolved it: Windows takes a bare
/// name as one in the current directory. The folder and those on its way
/// below the caller's are held open, so none of them can be renamed.
fn rename(file: &File, folder: &Folder, name: &OsStr) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    let name: Vec<u16> = folder
        .resolved
        .join(name)
        .as_os_str()
        .encode_wide()
        .collect();
    let name_bytes = name.len() * size_of::<u16>();
    let bytes = std::mem::offset_of!(FILE_RENAME_INFO, FileName) + name_bytes + size_of::<u16>();
    // u64 keeps the structure's alignment; zeroed is no root folder and a
    // NUL after the name.
    let mut buffer = vec![0u64; bytes.div_ceil(size_of::<u64>())];
    let info = buffer.as_mut_ptr().cast::<FILE_RENAME_INFO>();
    // SAFETY: the buffer is aligned for FILE_RENAME_INFO and holds the name
    // it is followed by; Windows reads it during the call only.
    unsafe {
        (*info).Anonymous.ReplaceIfExists = true;
        (*info).FileNameLength = name_bytes as u32;
        std::ptr::copy_nonoverlapping(
            name.as_ptr(),
            (&raw mut (*info).FileName).cast::<u16>(),
            name.len(),
        );
        SetFileInformationByHandle(
            crate::user_folder::raw(file),
            FileRenameInfo,
            info.cast_const().cast(),
            (buffer.len() * size_of::<u64>()) as u32,
        )
    }
    .map_err(crate::user_folder::os_error)
}

#[cfg(test)]
mod tests;
