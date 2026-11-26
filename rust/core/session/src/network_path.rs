//! Paths the host will not open for the console's file picker (`/api/browse`) or for library
//! artwork: network (UNC) paths, which make Windows sign in to the named computer with this PC's
//! account (SMB and NTLM), and device paths (`\\.\`, `\\?\`, `\??\` and DOS device names such as
//! `NUL` or `COM1`), which reach named pipes, raw disks and serial ports. A drive letter, mapped to
//! a share or not, is an ordinary path: the PC's owner chose to connect it.

/// Why a path is refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// `\\server\share`, `//server/share` or `\\?\UNC\server\share`.
    Network,
    /// `\\.\`, `\\?\`, `\??\`, or a DOS device name.
    Device,
}

impl Refusal {
    /// What the console's file picker says.
    pub fn browse_message(self) -> &'static str {
        match self {
            Self::Network => {
                r"Network paths (\\server\share) can't be browsed: the host would sign in to that computer with this PC's account. Pick a folder on this PC or a drive letter."
            }
            Self::Device => {
                r"Device paths (\\.\ or \\?\, NUL, COM1 and the like) can't be browsed. Pick a folder on this PC."
            }
        }
    }
}

const DEVICE_NAMES: &[&str] = &[
    "con", "prn", "aux", "nul", "conin$", "conout$", "clock$", "com0", "com1", "com2", "com3",
    "com4", "com5", "com6", "com7", "com8", "com9", "com¹", "com²", "com³", "lpt0", "lpt1", "lpt2",
    "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9", "lpt¹", "lpt²", "lpt³",
];

fn separator(c: char) -> bool {
    matches!(c, '\\' | '/')
}

/// Whether a path component is a DOS device name, which Windows opens as the device in any folder
/// and with any extension: `nul`, `NUL.txt`, `com1 .png`.
fn device_name(component: &str) -> bool {
    let stem = component.split('.').next().unwrap_or("");
    let stem = stem.trim_end_matches([' ', ':']).to_lowercase();
    DEVICE_NAMES.contains(&stem.as_str())
}

/// Why `path` is refused, or `None` for an ordinary path. Either separator counts, as it does for
/// Windows, and leading blanks are ignored.
pub fn refused(path: &str) -> Option<Refusal> {
    let path = path.trim_start();
    let mut chars = path.chars();
    let (first, second) = (chars.next(), chars.next());
    if first.is_some_and(separator) && second.is_some_and(separator) {
        let rest: String = path.chars().skip(2).collect();
        let rest = rest.replace('/', "\\").to_ascii_lowercase();
        // `\\?\UNC\server\share` and `\\.\UNC\server\share` are network paths too.
        let network = !rest.starts_with("?\\") && !rest.starts_with(".\\")
            || rest.starts_with("?\\unc\\")
            || rest.starts_with(".\\unc\\");
        return Some(if network && !matches!(rest.as_str(), "?" | ".") {
            Refusal::Network
        } else {
            Refusal::Device
        });
    }
    if path.replace('/', "\\").starts_with("\\??\\") {
        return Some(Refusal::Device);
    }
    path.split(separator)
        .any(device_name)
        .then_some(Refusal::Device)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_paths_are_refused_in_every_spelling() {
        for path in [
            r"\\server\share",
            r"\\server\share\covers\game.png",
            "//10.0.0.1/c$",
            r"/\server\share",
            r"\/server/share",
            r"\\?\UNC\server\share\x.png",
            "//?/unc/server/share",
            r"\\.\UNC\server\share",
            r"  \\server\share",
        ] {
            assert_eq!(refused(path), Some(Refusal::Network), "{path}");
        }
    }

    #[test]
    fn device_paths_and_names_are_refused() {
        for path in [
            r"\\.\pipe\butterpollo",
            r"\\.\PhysicalDrive0",
            r"\\?\C:\Games\cover.png",
            r"\\?\GLOBALROOT\Device\HarddiskVolume1",
            r"\??\C:\x",
            r"\\?",
            "NUL",
            r"C:\covers\nul.png",
            "COM1",
            r"D:\x\com1 .png",
            "conout$",
            r"C:\x\LPT¹.jpg",
        ] {
            assert_eq!(refused(path), Some(Refusal::Device), "{path}");
        }
    }

    #[test]
    fn local_and_mapped_drive_paths_are_ordinary() {
        for path in [
            r"C:\Games\cover.png",
            r"Z:\covers\game.jpg",
            "Z:/covers/game.jpg",
            "./assets/steam.png",
            "cover.png",
            r"\Games\cover.png",
            "/home/user/cover.png",
            r"C:\Games\Console Classics\nullpointer.png",
            r"C:\Games\conveyor\com10.png",
            "",
        ] {
            assert_eq!(refused(path), None, "{path}");
        }
    }
}
