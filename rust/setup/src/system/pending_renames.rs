//! Delayed file deletion and stale `PendingFileRenameOperations` cleanup.
use super::{registry, wide, win32_path};
use std::path::{Path, PathBuf};
use windows::{
    Win32::{Foundation::ERROR_SUCCESS, Storage::FileSystem::*, System::Registry::*},
    core::{HSTRING, PCWSTR},
};

const OLD_FILE_PREFIX: &str = ".butterpollo-old-";

/// Move a locked regular file to a unique sibling name so a later reinstall at
/// the same path is not deleted at reboot by `PendingFileRenameOperations`.
pub(crate) fn rename_aside_for_delete(path: &Path) -> Option<PathBuf> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    if !std::fs::symlink_metadata(path).is_ok_and(|meta| meta.is_file()) {
        return None;
    }
    let name = path.file_name()?.to_string_lossy();
    if name.to_ascii_lowercase().starts_with(OLD_FILE_PREFIX) {
        return None;
    }
    let parent = path.parent()?;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let aside = parent.join(format!(
        "{OLD_FILE_PREFIX}{}-{nanos}-{seq}",
        std::process::id()
    ));
    std::fs::rename(path, &aside).ok().map(|()| aside)
}

/// Delete a file now, or rename it aside and schedule deletion at the next restart.
pub fn remove_file_later(path: &Path) {
    if std::fs::remove_file(path).is_ok() || !path.exists() {
        return;
    }
    let target = rename_aside_for_delete(path).unwrap_or_else(|| path.to_path_buf());
    let source = HSTRING::from(target.as_os_str());
    // SAFETY: `source` is NUL-terminated UTF-16; a null target schedules deletion.
    unsafe {
        let _ = MoveFileExW(&source, PCWSTR::null(), MOVEFILE_DELAY_UNTIL_REBOOT);
    }
}

fn normalize_nt_path(raw: &str) -> String {
    let stripped = if let Some(rest) = raw
        .strip_prefix(r"\??\UNC\")
        .or_else(|| raw.strip_prefix(r"\\?\UNC\"))
    {
        format!(r"\\{rest}")
    } else {
        raw.strip_prefix(r"\??\")
            .or_else(|| raw.strip_prefix(r"\\?\"))
            .unwrap_or(raw)
            .to_owned()
    };
    stripped
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_ascii_lowercase()
}

/// Filter a UTF-16 `REG_MULTI_SZ` `PendingFileRenameOperations` buffer,
/// dropping pending deletions (`dst == ""`) inside `install` except renamed-aside
/// `.butterpollo-old-*` files. Returns `Some(kept)` only when an entry was pruned.
pub(crate) fn prune_pending_rename_words(words: &[u16], install: &Path) -> Option<Vec<u16>> {
    let base = normalize_nt_path(&install.display().to_string());
    if base.is_empty() {
        return None;
    }
    let prefix = format!("{base}\\");
    let mut kept = Vec::with_capacity(words.len());
    let (mut removed, mut i) = (false, 0);
    while i < words.len() {
        let src_end = words[i..].iter().position(|&w| w == 0)?;
        if src_end == 0 {
            break;
        }
        let src = &words[i..i + src_end];
        i += src_end + 1;
        let dst_end = words.get(i..)?.iter().position(|&w| w == 0)?;
        let dst = &words[i..i + dst_end];
        i += dst_end + 1;
        let src_text = normalize_nt_path(&String::from_utf16_lossy(src));
        let name = src_text.rsplit('\\').next().unwrap_or("");
        if dst.is_empty()
            && (src_text == base || src_text.starts_with(&prefix))
            && !name.starts_with(OLD_FILE_PREFIX)
        {
            removed = true;
            continue;
        }
        kept.extend_from_slice(src);
        kept.push(0);
        kept.extend_from_slice(dst);
        kept.push(0);
    }
    if !removed {
        return None;
    }
    if !kept.is_empty() {
        kept.push(0);
    }
    Some(kept)
}

/// Remove stale `PendingFileRenameOperations` deletions targeting `install`
/// (e.g. left by an earlier uninstaller before an immediate reinstall).
pub fn prune_pending_renames(install: &Path) {
    let resolved = win32_path(install).unwrap_or_else(|_| install.to_path_buf());
    let Some(key) = registry::open_key(
        HKEY_LOCAL_MACHINE,
        r"SYSTEM\CurrentControlSet\Control\Session Manager",
        KEY_QUERY_VALUE | KEY_SET_VALUE | KEY_WOW64_64KEY,
    ) else {
        return;
    };
    for value_name in [
        "PendingFileRenameOperations",
        "PendingFileRenameOperations2",
    ] {
        let name = wide(value_name);
        let (mut size, mut kind) = (0u32, REG_VALUE_TYPE::default());
        // SAFETY: `key` is open, `name` is NUL-terminated, and buffers match reported byte lengths.
        unsafe {
            if RegQueryValueExW(
                key.0,
                PCWSTR(name.as_ptr()),
                None,
                Some(&mut kind),
                None,
                Some(&mut size),
            ) != ERROR_SUCCESS
                || kind != REG_MULTI_SZ
                || size == 0
                || size % 2 != 0
                || size > (4 << 20)
            {
                continue;
            }
            let mut buf = vec![0u16; (size as usize) / 2];
            let mut bytes = size;
            if RegQueryValueExW(
                key.0,
                PCWSTR(name.as_ptr()),
                None,
                None,
                Some(buf.as_mut_ptr().cast()),
                Some(&mut bytes),
            ) != ERROR_SUCCESS
                || bytes % 2 != 0
            {
                continue;
            }
            buf.truncate((bytes as usize) / 2);
            let Some(updated) = prune_pending_rename_words(&buf, &resolved) else {
                continue;
            };
            if updated.is_empty() {
                let _ = RegDeleteValueW(key.0, PCWSTR(name.as_ptr()));
            } else {
                let data = std::slice::from_raw_parts(updated.as_ptr().cast(), updated.len() * 2);
                let _ =
                    RegSetValueExW(key.0, PCWSTR(name.as_ptr()), None, REG_MULTI_SZ, Some(data));
            }
        }
    }
}
