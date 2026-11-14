//! The NVIDIA component: Rubylight's NGX adapter (`butterpollo_truehdr.dll`,
//! built from `rust/truehdr-runtime`) and NVIDIA's RTX Video runtime
//! (`nvngx_truehdr.dll`, `nvngx_vsr.dll` and NVIDIA's licence). RTX HDR and
//! RTX Video enhancement need it. The signed installer carries none of it:
//! the console downloads it at the user's request from the host's own GitHub
//! release into a folder only administrators may change, and the host checks
//! every file before every load.
//!
//! One archive per release holds all of it: NVIDIA's licence lets its files
//! go out only as part of an application, never on their own, so they ship
//! only beside the adapter. What the host trusts: the archive by GitHub's
//! SHA-256 digest of the release asset, the check the updater gives its
//! installers; the adapter by its SHA-256 recorded at install; NVIDIA's files
//! by the SHA-256 compiled in here. Every file is checked again before each
//! load. The design
//! (vendor files trusted by digests in the host binary, kept apart from the
//! adapter's build identity, fixed file roles in a fixed folder, a check
//! before each load, installed only after an explicit user action) follows
//! Foundation Sunshine's native sideload components (AlkaidLab/foundation-sunshine,
//! GPL-3.0, docs/native_sideload_components.md).

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// The adapter's file name.
pub const ADAPTER: &str = "butterpollo_truehdr.dll";
/// The adapter ABI this host speaks (`butterpollo_truehdr_abi`).
pub const ABI: u32 = 3;
/// The RTX Video SDK the runtime files come from.
pub const RUNTIME_VERSION: &str = "1.1.0";

/// A file of the component with its exact size and SHA-256.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pinned {
    pub name: &'static str,
    pub size: u64,
    pub sha256: &'static str,
}

/// NVIDIA's files from RTX Video SDK 1.1.0 (`bin/Windows/x64/rel` and the
/// licence), from the archive `rust/build.ps1` pins by SHA-256
/// (abf4f34e2b5a618e355b0d5a0365d8ecc3db4396e756e4c850a867e1ae2ed69e).
pub const RUNTIME: [Pinned; 3] = [
    Pinned {
        name: "nvngx_truehdr.dll",
        size: 3_955_752,
        sha256: "9a80575f247190c05fe80eac0c4baa1d0d4d932348f26808310b5ec4bf9eeb4b",
    },
    Pinned {
        name: "nvngx_vsr.dll",
        size: 19_140_144,
        sha256: "c3d88eea5ff7a548edefa66414cf6e77464d0947277c904f324dd23abf58a1ed",
    },
    Pinned {
        name: "NVIDIA_RTX_Video_SDK_License.pdf",
        size: 136_535,
        sha256: "5addfcf7c78891a12f1dcd6b5f86c2512351ab71b3872f446ba10e1b6da54464",
    },
];

/// The release asset holding the component built for host `version`
/// (`4.5.0`, no `v`): the adapter and NVIDIA's files, at the archive's top
/// level.
pub fn asset(version: &str) -> String {
    format!("rubylight-nvidia-component-{version}.zip")
}

/// What the archive may contribute, by file name: the adapter and NVIDIA's
/// pinned files. Anything else in it is left out.
pub fn wanted(name: &str) -> bool {
    name == ADAPTER || RUNTIME.iter().any(|file| file.name == name)
}

/// `%ProgramData%\Butterpollo\nvidia` for the installed host, the profile's
/// own `nvidia` folder for any other.
pub fn root(profile: &Path) -> PathBuf {
    if profile == crate::paths::installed_profile() {
        crate::paths::data_root().join("nvidia")
    } else {
        profile.join("nvidia")
    }
}

/// The record of the installed component, in [`root`].
pub const RECORD: &str = "component.json";
/// NGX's data folder (its logs), in [`root`].
pub const NGX_DATA: &str = "ngx";

/// The installed component: written after every file was checked, read before
/// every load.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    /// The host version whose adapter is installed.
    pub version: String,
    /// The folder in [`root`] holding the files (a name, never a path).
    pub folder: String,
    /// The adapter's SHA-256, from its release asset.
    pub adapter_sha256: String,
}

impl Record {
    /// The record in `root`, `None` when there is none.
    pub fn read(root: &Path) -> Result<Option<Self>> {
        let path = root.join(RECORD);
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
        };
        let record: Self = serde_json::from_str(&text)
            .with_context(|| format!("{} is damaged", path.display()))?;
        record.check()?;
        Ok(Some(record))
    }

    /// Writes the record to `root`, replacing the old one in one step.
    pub fn write(&self, root: &Path) -> Result<()> {
        self.check()?;
        let temporary = root.join(format!("{RECORD}.new"));
        std::fs::write(&temporary, serde_json::to_vec_pretty(self)?)
            .with_context(|| format!("writing {}", temporary.display()))?;
        std::fs::rename(&temporary, root.join(RECORD))
            .with_context(|| format!("replacing {}", root.join(RECORD).display()))
    }

    /// The folder with the files.
    pub fn files(&self, root: &Path) -> PathBuf {
        root.join(&self.folder)
    }

    fn check(&self) -> Result<()> {
        ensure!(
            !self.folder.is_empty()
                && self
                    .folder
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.')
                && !self.folder.starts_with('.'),
            "the NVIDIA component record names an invalid folder"
        );
        ensure!(
            is_sha256(&self.adapter_sha256),
            "the NVIDIA component record has no valid adapter SHA-256"
        );
        Ok(())
    }
}

/// A lowercase hexadecimal SHA-256.
pub fn is_sha256(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// The SHA-256 and size of the file at `path`.
pub fn hash_file(path: &Path) -> Result<(String, u64)> {
    use std::io::Read;
    let mut file =
        std::fs::File::open(path).with_context(|| format!("cannot read {}", path.display()))?;
    let mut hash = Sha256::new();
    let mut buffer = vec![0u8; 1 << 20];
    let mut size = 0u64;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        size += read as u64;
        hash.update(&buffer[..read]);
    }
    Ok((hex::encode(hash.finalize()), size))
}

/// Checks every file in `folder`: NVIDIA's against [`RUNTIME`], the adapter
/// against `adapter_sha256`.
pub fn verify(folder: &Path, adapter_sha256: &str) -> Result<()> {
    verify_with(folder, adapter_sha256, &RUNTIME)
}

/// [`verify`] against `runtime` in place of [`RUNTIME`].
pub fn verify_with(folder: &Path, adapter_sha256: &str, runtime: &[Pinned]) -> Result<()> {
    for file in runtime {
        check_pinned(&folder.join(file.name), file)?;
    }
    let (sha256, _) = present(&folder.join(ADAPTER), ADAPTER)?;
    ensure!(
        sha256 == adapter_sha256,
        "{ADAPTER} is not the adapter that was installed; download the NVIDIA components again"
    );
    Ok(())
}

/// [`hash_file`], saying plainly when the component's file `name` is gone.
fn present(path: &Path, name: &str) -> Result<(String, u64)> {
    if !path.is_file() {
        bail!("{name} is missing from the NVIDIA components; download them again");
    }
    hash_file(path)
}

/// Checks one of NVIDIA's files against its pin.
pub fn check_pinned(path: &Path, file: &Pinned) -> Result<()> {
    let (sha256, size) = present(path, file.name)?;
    if size != file.size || sha256 != file.sha256 {
        bail!(
            "{} is not NVIDIA's file from RTX Video SDK {RUNTIME_VERSION}; download the NVIDIA components again",
            file.name
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pins_are_well_formed() {
        for file in RUNTIME {
            assert!(is_sha256(file.sha256), "{}", file.name);
            assert!(file.size > 0);
        }
    }

    #[test]
    fn the_archive_contributes_only_the_components_files() {
        assert_eq!(asset("4.5.0"), "rubylight-nvidia-component-4.5.0.zip");
        assert!(wanted(ADAPTER));
        for file in RUNTIME {
            assert!(wanted(file.name), "{}", file.name);
        }
        for other in ["dxgi.dll", "version.dll", "readme.txt", "nvngx_dlss.dll"] {
            assert!(!wanted(other), "{other}");
        }
    }

    #[test]
    fn a_record_round_trips_and_refuses_paths() -> Result<()> {
        let temp = tempfile::tempdir()?;
        assert_eq!(Record::read(temp.path())?, None);
        let record = Record {
            version: "4.5.0".into(),
            folder: "a1b2c3d4e5f60718".into(),
            adapter_sha256: "0".repeat(64),
        };
        record.write(temp.path())?;
        assert_eq!(Record::read(temp.path())?, Some(record.clone()));
        for folder in ["..", "../x", r"C:\x", "", ".hidden"] {
            let bad = Record {
                folder: folder.into(),
                ..record.clone()
            };
            assert!(bad.write(temp.path()).is_err(), "{folder}");
        }
        std::fs::write(
            temp.path().join(RECORD),
            r#"{"version":"1","folder":"..","adapter_sha256":"x"}"#,
        )?;
        assert!(Record::read(temp.path()).is_err());
        Ok(())
    }

    #[test]
    fn verification_names_the_file_that_differs() -> Result<()> {
        let temp = tempfile::tempdir()?;
        std::fs::write(temp.path().join(ADAPTER), b"adapter")?;
        for file in RUNTIME {
            std::fs::write(temp.path().join(file.name), b"not nvidia")?;
        }
        let error = verify(temp.path(), &"0".repeat(64)).unwrap_err();
        assert!(
            format!("{error:#}").contains("nvngx_truehdr.dll"),
            "{error:#}"
        );
        std::fs::remove_file(temp.path().join("nvngx_truehdr.dll"))?;
        let error = verify(temp.path(), &"0".repeat(64)).unwrap_err();
        assert_eq!(
            format!("{error:#}"),
            "nvngx_truehdr.dll is missing from the NVIDIA components; download them again"
        );
        Ok(())
    }

    #[test]
    fn the_installed_host_keeps_the_component_in_program_data() {
        let installed = crate::paths::installed_profile();
        assert_eq!(root(&installed), crate::paths::data_root().join("nvidia"));
        let other = Path::new(r"D:\test\profile");
        assert_eq!(root(other), other.join("nvidia"));
    }
}
