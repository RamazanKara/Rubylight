//! Drives on this PC, for the console's file picker.

/// The drive roots Windows reports, e.g. `C:\`.
pub fn drives() -> Vec<String> {
    // SAFETY: GetLogicalDrives takes no arguments and has no preconditions.
    let mask = unsafe { windows::Win32::Storage::FileSystem::GetLogicalDrives() };
    (0..26u8)
        .filter(|bit| mask & (1 << bit) != 0)
        .map(|bit| format!("{}:\\", char::from(b'A' + bit)))
        .collect()
}

/// Whether `path` is on a local drive this process sees, so that a file
/// missing there is missing for the signed-in user too. A host running as
/// SYSTEM sees neither the drive letters the user mapped or substituted
/// nor, usually, the network shares the user's own token can open.
pub fn local_volume(path: &std::path::Path) -> bool {
    use std::path::{Component, Prefix};
    let letter = match path.components().next() {
        Some(Component::Prefix(prefix)) => match prefix.kind() {
            Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) => letter,
            _ => return false,
        },
        _ => return false,
    };
    let root: Vec<u16> = format!("{}:\\", char::from(letter))
        .encode_utf16()
        .chain(Some(0))
        .collect();
    // SAFETY: The root path is NUL-terminated and outlives the call.
    let kind = unsafe {
        windows::Win32::Storage::FileSystem::GetDriveTypeW(windows::core::PCWSTR(root.as_ptr()))
    };
    // DRIVE_UNKNOWN, DRIVE_NO_ROOT_DIR and DRIVE_REMOTE.
    !matches!(kind, 0 | 1 | 4)
}

#[cfg(test)]
mod tests {
    #[test]
    fn only_drives_this_process_sees_locally_are_local_volumes() {
        let program = std::path::PathBuf::from(std::env::var_os("SystemRoot").unwrap())
            .join("System32\\cmd.exe");
        assert!(super::local_volume(&program));
        let verbatim = format!("\\\\?\\{}", program.display());
        assert!(super::local_volume(std::path::Path::new(&verbatim)));
        let drives = super::drives();
        let unused = (b'D'..=b'Z')
            .map(char::from)
            .find(|letter| !drives.contains(&format!("{letter}:\\")))
            .expect("a free drive letter");
        for path in [
            format!("{unused}:\\Games\\game.exe"),
            "\\\\rubylight-test.invalid\\games\\game.exe".into(),
            "relative\\game.exe".into(),
        ] {
            assert!(!super::local_volume(std::path::Path::new(&path)), "{path}");
        }
    }
}
