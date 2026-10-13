//! The release package appended to setup.exe: the executable, then the ZIP,
//! then a 16-byte footer (`BPSETUP1` and the ZIP's length, little endian).
//! Authenticode signing may append alignment padding and a certificate table.
use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Component, Path, PathBuf},
};

const MAGIC: &[u8; 8] = b"BPSETUP1";

/// A window onto part of a file, so the ZIP reader sees only the payload.
struct Slice {
    file: File,
    start: u64,
    length: u64,
    position: u64,
}
impl Read for Slice {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let remaining = self.length.saturating_sub(self.position);
        let wanted = (buffer.len() as u64).min(remaining) as usize;
        if wanted == 0 {
            return Ok(0);
        }
        self.file
            .seek(SeekFrom::Start(self.start + self.position))?;
        let read = self.file.read(&mut buffer[..wanted])?;
        self.position += read as u64;
        Ok(read)
    }
}
impl Seek for Slice {
    fn seek(&mut self, to: SeekFrom) -> std::io::Result<u64> {
        let next = match to {
            SeekFrom::Start(n) => n as i64,
            SeekFrom::End(n) => self.length as i64 + n,
            SeekFrom::Current(n) => self.position as i64 + n,
        };
        if next < 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "seek before the payload",
            ));
        }
        self.position = next as u64;
        Ok(self.position)
    }
}

/// The length of setup.exe without its payload, and the payload's range.
fn layout(path: &Path) -> Result<Option<(u64, u64)>> {
    let mut file = File::open(path)?;
    let total = file.metadata()?.len();
    let Some(certificate) = certificate_start(&mut file, total)? else {
        return footer_layout(&mut file, total, total);
    };
    // Signing aligns the certificate table to eight bytes. Only zero alignment
    // bytes may separate it from our footer; never search arbitrary image data.
    // A footer inside the certificate is not part of the signed payload.
    for padding in 0..=7 {
        let end = certificate - padding;
        if let Some(layout) = footer_layout(&mut file, end, total)? {
            return Ok(Some(layout));
        }
        if read_at::<1>(&mut file, end - 1, total)?[0] != 0 {
            break;
        }
    }
    Ok(None)
}
fn read_at<const N: usize>(file: &mut File, offset: u64, total: u64) -> Result<[u8; N]> {
    if offset.checked_add(N as u64).is_none_or(|end| end > total) {
        bail!("setup PE header or certificate table is truncated");
    }
    file.seek(SeekFrom::Start(offset))?;
    let mut bytes = [0; N];
    file.read_exact(&mut bytes)?;
    Ok(bytes)
}
fn footer_layout(file: &mut File, end: u64, total: u64) -> Result<Option<(u64, u64)>> {
    if end < 16 {
        return Ok(None);
    }
    let footer = read_at::<16>(file, end - 16, total)?;
    if &footer[..8] != MAGIC {
        return Ok(None);
    }
    let length = u64::from_le_bytes(footer[8..].try_into().unwrap());
    if length == 0 || length > end - 16 {
        bail!("setup payload is damaged");
    }
    Ok(Some((end - 16 - length, length)))
}
/// IMAGE_DIRECTORY_ENTRY_SECURITY stores a file offset, unlike other PE data
/// directories. Validate the PE32/PE32+ header and table ranges before using it.
fn certificate_start(file: &mut File, total: u64) -> Result<Option<u64>> {
    if total < 2 || read_at::<2>(file, 0, total)? != *b"MZ" {
        return Ok(None);
    }
    let dos = read_at::<64>(file, 0, total)?;
    let pe = u64::from(u32::from_le_bytes(dos[60..64].try_into().unwrap()));
    if pe < 64 {
        bail!("invalid setup PE header offset");
    }
    let coff = read_at::<24>(file, pe, total)?;
    if &coff[..4] != b"PE\0\0" {
        bail!("invalid setup PE signature");
    }
    let optional = pe + 24;
    let optional_size = u64::from(u16::from_le_bytes(coff[20..22].try_into().unwrap()));
    let sections = u64::from(u16::from_le_bytes(coff[6..8].try_into().unwrap()));
    let headers_end = optional + optional_size + sections * 40;
    if optional_size < 2 || headers_end > total {
        bail!("setup PE headers are truncated");
    }
    let directories = match u16::from_le_bytes(read_at::<2>(file, optional, total)?) {
        0x10b => 96,
        0x20b => 112,
        _ => bail!("unsupported setup PE optional header"),
    };
    if optional_size < directories {
        bail!("setup PE optional header is truncated");
    }
    let count = u64::from(u32::from_le_bytes(read_at::<4>(
        file,
        optional + directories - 4,
        total,
    )?));
    if count * 8 > optional_size - directories {
        bail!("setup PE data directories are truncated");
    }
    if count <= 4 {
        return Ok(None);
    }
    let directory = read_at::<8>(file, optional + directories + 4 * 8, total)?;
    let start = u64::from(u32::from_le_bytes(directory[..4].try_into().unwrap()));
    let size = u64::from(u32::from_le_bytes(directory[4..].try_into().unwrap()));
    if start == 0 && size == 0 {
        return Ok(None);
    }
    let end = start + size;
    let image_headers = u64::from(u32::from_le_bytes(read_at::<4>(
        file,
        optional + 60,
        total,
    )?));
    if image_headers < headers_end
        || start < image_headers
        || !start.is_multiple_of(8)
        || size < 8
        || end > total
    {
        bail!("invalid setup PE certificate table range");
    }
    for section in 0..sections {
        let header = read_at::<40>(file, optional + optional_size + section * 40, total)?;
        let size = u64::from(u32::from_le_bytes(header[16..20].try_into().unwrap()));
        let offset = u64::from(u32::from_le_bytes(header[20..24].try_into().unwrap()));
        if size > 0 && (offset < image_headers || offset + size > start) {
            bail!("setup PE certificate table overlaps image sections");
        }
    }
    let mut entry = start;
    while entry < end {
        let header = read_at::<8>(file, entry, end)?;
        let length = u64::from(u32::from_le_bytes(header[..4].try_into().unwrap()));
        let padded = length.div_ceil(8) * 8;
        if length < 8 || padded > end - entry {
            bail!("invalid setup PE certificate length");
        }
        entry += padded;
    }
    Ok(Some(start))
}
pub struct Payload {
    archive: zip::ZipArchive<Slice>,
}
impl Payload {
    /// The payload of the running setup.exe; None for a build without one.
    pub fn open() -> Result<Option<Self>> {
        let exe = std::env::current_exe()?;
        let Some((start, length)) = layout(&exe)? else {
            return Ok(None);
        };
        let slice = Slice {
            file: File::open(&exe)?,
            start,
            length,
            position: 0,
        };
        Ok(Some(Self {
            archive: zip::ZipArchive::new(slice).context("reading the setup payload")?,
        }))
    }
    /// Extract into `destination`, dropping the archive's top folder.
    /// Entries escaping the destination are rejected.
    pub fn extract(&mut self, destination: &Path) -> Result<()> {
        for index in 0..self.archive.len() {
            let mut entry = self.archive.by_index(index)?;
            let Some(name) = entry.enclosed_name() else {
                bail!("unsafe path in the setup payload");
            };
            let relative: PathBuf = name.components().skip(1).collect();
            if relative.as_os_str().is_empty() {
                continue;
            }
            if relative
                .components()
                .any(|c| !matches!(c, Component::Normal(_)))
            {
                bail!("unsafe path in the setup payload");
            }
            let target = destination.join(&relative);
            if entry.is_dir() {
                std::fs::create_dir_all(&target)?;
                continue;
            }
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut file = File::create(&target)?;
            std::io::copy(&mut entry, &mut file)?;
        }
        Ok(())
    }
}
/// Choose the stub only from the verified manifest, never an unrelated file
/// that happens to exist in the staging or previous installation directory.
pub fn packaged_stub(root: &Path, verified_entries: &[Entry]) -> Result<Option<PathBuf>> {
    verified_entries
        .iter()
        .find(|entry| entry.path.eq_ignore_ascii_case("butterpollo-setup.exe"))
        .map(|entry| safe_join(root, &entry.path))
        .transpose()
}
/// Preserve the separately signed packaged stub in full. Older packages did
/// not carry one, so retain their unsigned current-executable fallback.
pub fn write_stub(destination: &Path, packaged: Option<&Path>) -> Result<()> {
    let (exe, length) = if let Some(packaged) = packaged {
        (packaged.to_owned(), std::fs::metadata(packaged)?.len())
    } else {
        let exe = std::env::current_exe()?;
        let length = layout(&exe)?.map_or(std::fs::metadata(&exe)?.len(), |(start, _)| start);
        (exe, length)
    };
    let mut source = File::open(&exe)?.take(length);
    crate::update_files::write(
        destination,
        |file| {
            std::io::copy(&mut source, file)?;
            Ok(())
        },
        crate::system::remove_file_later,
    )
}

#[derive(Debug, serde::Deserialize)]
pub struct Entry {
    pub path: String,
    pub sha256: String,
}
/// The package's file list, checked against the files in `root`.
pub fn verify(root: &Path) -> Result<Vec<Entry>> {
    let entries = manifest(root)?;
    for required in [
        "butterpollo.exe",
        "butterpollo-service.exe",
        "Start Rubylight.exe",
        "assets/web/index.html",
    ] {
        if !entries
            .iter()
            .any(|entry| entry.path.replace('\\', "/").eq_ignore_ascii_case(required))
        {
            bail!("The package is missing {required}");
        }
    }
    for entry in &entries {
        let path = safe_join(root, &entry.path)?;
        let mut hasher = Sha256::new();
        std::io::copy(&mut File::open(&path)?, &mut hasher)
            .with_context(|| format!("reading {}", entry.path))?;
        if hex(&hasher.finalize()) != entry.sha256.to_ascii_lowercase() {
            bail!("{} does not match the package manifest", entry.path);
        }
    }
    Ok(entries)
}
pub fn manifest(root: &Path) -> Result<Vec<Entry>> {
    let text = std::fs::read_to_string(root.join("manifest.json"))?;
    let text = text.trim_start_matches('\u{feff}');
    // PowerShell writes a single entry as an object rather than an array.
    let value: serde_json::Value = serde_json::from_str(text)?;
    Ok(match value {
        serde_json::Value::Array(_) => serde_json::from_value(value)?,
        other => vec![serde_json::from_value(other)?],
    })
}
pub fn safe_join(root: &Path, relative: &str) -> Result<PathBuf> {
    let relative = Path::new(relative);
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        bail!("unsafe path in the package manifest");
    }
    Ok(root.join(relative))
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    /// A structural PE fixture; its certificate bytes are not a real signature.
    fn signed_image(pe32plus: bool, payload: Option<&[u8]>) -> (Vec<u8>, usize) {
        let pe = 128;
        let optional = pe + 24;
        let (magic, optional_size, directories) = if pe32plus {
            (0x20bu16, 240usize, 112usize)
        } else {
            (0x10bu16, 224usize, 96usize)
        };
        let mut bytes = vec![0; 1024];
        bytes[..2].copy_from_slice(b"MZ");
        bytes[60..64].copy_from_slice(&(pe as u32).to_le_bytes());
        bytes[pe..pe + 4].copy_from_slice(b"PE\0\0");
        bytes[pe + 6..pe + 8].copy_from_slice(&1u16.to_le_bytes());
        bytes[pe + 20..pe + 22].copy_from_slice(&(optional_size as u16).to_le_bytes());
        bytes[optional..optional + 2].copy_from_slice(&magic.to_le_bytes());
        bytes[optional + 60..optional + 64].copy_from_slice(&512u32.to_le_bytes());
        bytes[optional + directories - 4..optional + directories]
            .copy_from_slice(&16u32.to_le_bytes());
        let section = optional + optional_size;
        bytes[section + 16..section + 20].copy_from_slice(&512u32.to_le_bytes());
        bytes[section + 20..section + 24].copy_from_slice(&512u32.to_le_bytes());
        bytes[512..].fill(0x55);
        if let Some(payload) = payload {
            bytes.extend_from_slice(payload);
            bytes.extend_from_slice(MAGIC);
            bytes.extend_from_slice(&(payload.len() as u64).to_le_bytes());
        }
        bytes.resize(bytes.len().next_multiple_of(8), 0);
        let certificate = bytes.len();
        let directory = optional + directories + 4 * 8;
        bytes[directory..directory + 4].copy_from_slice(&(certificate as u32).to_le_bytes());
        bytes[directory + 4..directory + 8].copy_from_slice(&32u32.to_le_bytes());
        bytes.extend_from_slice(&32u32.to_le_bytes());
        bytes.extend_from_slice(&0x200u16.to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.resize(certificate + 32, 0x55);
        (bytes, directory)
    }

    #[test]
    fn signed_payload_footer_precedes_certificate_alignment_for_both_pe_formats() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("setup.exe");
        for pe32plus in [false, true] {
            for length in 1..=8 {
                let (bytes, _) = signed_image(pe32plus, Some(&vec![b'z'; length]));
                std::fs::write(&path, bytes)?;
                assert_eq!(layout(&path)?, Some((1024, length as u64)));
            }
            let (bytes, _) = signed_image(pe32plus, None);
            std::fs::write(&path, bytes)?;
            assert_eq!(layout(&path)?, None);

            let (mut bytes, _) = signed_image(pe32plus, Some(b"zipdata"));
            let end = bytes.len();
            bytes[end - 16..end - 8].copy_from_slice(MAGIC);
            bytes[end - 8..].copy_from_slice(&1u64.to_le_bytes());
            std::fs::write(&path, bytes)?;
            // Certificate content must never override the signed payload.
            assert_eq!(layout(&path)?, Some((1024, 7)));
        }
        Ok(())
    }

    #[test]
    fn signed_payload_rejects_truncated_headers_and_bogus_certificate_ranges() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("setup.exe");
        let mutations: [fn(&mut Vec<u8>, usize); 14] = [
            |b, _| {
                b.pop();
            },
            |b, _| b[60..64].copy_from_slice(&u32::MAX.to_le_bytes()),
            |b, _| b[128..132].copy_from_slice(b"bad!"),
            |b, _| b[148..150].copy_from_slice(&2u16.to_le_bytes()),
            |b, _| b[152..154].copy_from_slice(&0x999u16.to_le_bytes()),
            |b, d| b[d - 36..d - 32].copy_from_slice(&u32::MAX.to_le_bytes()),
            |b, d| b[d..d + 4].copy_from_slice(&0u32.to_le_bytes()),
            |b, d| b[d..d + 4].copy_from_slice(&128u32.to_le_bytes()),
            |b, d| b[d..d + 4].copy_from_slice(&512u32.to_le_bytes()),
            |b, d| b[d..d + 4].copy_from_slice(&1025u32.to_le_bytes()),
            |b, d| b[d + 4..d + 8].copy_from_slice(&0u32.to_le_bytes()),
            |b, d| b[d + 4..d + 8].copy_from_slice(&u32::MAX.to_le_bytes()),
            |b, d| {
                let start = u32::from_le_bytes(b[d..d + 4].try_into().unwrap()) as usize;
                b[start..start + 4].copy_from_slice(&7u32.to_le_bytes());
            },
            |b, d| {
                let start = u32::from_le_bytes(b[d..d + 4].try_into().unwrap()) as usize;
                b[start..start + 4].copy_from_slice(&33u32.to_le_bytes());
            },
        ];
        for pe32plus in [false, true] {
            let (bytes, _) = signed_image(pe32plus, Some(b"zipdata"));
            for end in [2, 63, 128, 151, 160, 300] {
                std::fs::write(&path, &bytes[..end])?;
                assert!(layout(&path).is_err(), "truncated at {end}");
            }
            for mutate in mutations {
                let (mut bytes, directory) = signed_image(pe32plus, Some(b"zipdata"));
                mutate(&mut bytes, directory);
                std::fs::write(&path, bytes)?;
                assert!(layout(&path).is_err());
            }
            let (mut bytes, directory) = signed_image(pe32plus, Some(b"zipdata"));
            let certificate =
                u32::from_le_bytes(bytes[directory..directory + 4].try_into().unwrap()) as usize;
            bytes[certificate - 1] = 1;
            std::fs::write(&path, bytes)?;
            assert_eq!(
                layout(&path)?,
                None,
                "nonzero bytes are not alignment padding"
            );
        }
        Ok(())
    }

    #[test]
    fn only_a_manifest_verified_packaged_stub_is_copied_with_its_certificate() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let root = directory.path();
        let old_entries = crate::profile_tests::package(root, "old", "old.dll")?;
        let (bytes, _) = signed_image(true, None);
        let stub = root.join("butterpollo-setup.exe");
        std::fs::write(&stub, &bytes)?;
        assert!(
            packaged_stub(root, &old_entries)?.is_none(),
            "ignore a stale unlisted stub"
        );
        let mut manifest: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(root.join("manifest.json"))?)?;
        manifest.push(serde_json::json!({"path":"butterpollo-setup.exe","sha256":hex(&Sha256::digest(&bytes))}));
        std::fs::write(root.join("manifest.json"), serde_json::to_vec(&manifest)?)?;
        let entries = verify(root)?;
        let packaged = packaged_stub(root, &entries)?.unwrap();
        let uninstall = root.join("uninstall.exe");
        write_stub(&uninstall, Some(&packaged))?;
        assert_eq!(std::fs::read(&uninstall)?, bytes);
        assert_eq!(layout(&uninstall)?, None);
        Ok(())
    }

    #[test]
    fn an_incomplete_or_changed_package_is_rejected_before_installation() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let root = directory.path();
        std::fs::create_dir_all(root.join("assets/web"))?;
        let paths = [
            "butterpollo.exe",
            "butterpollo-service.exe",
            "Start Rubylight.exe",
            "assets/web/index.html",
        ];
        let entries: Vec<_> = paths
            .iter()
            .map(|path| {
                std::fs::write(root.join(path), path).unwrap();
                serde_json::json!({"path":path, "sha256":hex(&Sha256::digest(path.as_bytes()))})
            })
            .collect();
        std::fs::write(root.join("manifest.json"), serde_json::to_vec(&entries)?)?;
        assert_eq!(verify(root)?.len(), paths.len());
        for (missing, path) in paths.iter().enumerate() {
            let incomplete: Vec<_> = entries
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != missing)
                .map(|(_, entry)| entry)
                .collect();
            std::fs::write(root.join("manifest.json"), serde_json::to_vec(&incomplete)?)?;
            assert!(verify(root).unwrap_err().to_string().contains(path));
        }
        std::fs::write(root.join("manifest.json"), serde_json::to_vec(&entries)?)?;
        std::fs::write(root.join("assets/web/index.html"), b"partial")?;
        assert!(verify(root).is_err());
        std::fs::remove_file(root.join("assets/web/index.html"))?;
        assert!(verify(root).is_err());
        Ok(())
    }
    #[test]
    fn manifest_paths_cannot_escape_the_package() {
        let root = Path::new("C:\\package");
        assert!(safe_join(root, "drivers\\gamepad\\install.ps1").is_ok());
        assert!(safe_join(root, "..\\Windows\\evil.dll").is_err());
        assert!(safe_join(root, "C:\\Windows\\evil.dll").is_err());
        assert!(safe_join(root, "").is_err());
    }
    #[test]
    fn payload_footer_locates_the_archive() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("setup.exe");
        let mut bytes = b"stub".to_vec();
        bytes.extend_from_slice(b"zipdata");
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&7u64.to_le_bytes());
        std::fs::write(&path, &bytes).unwrap();
        assert_eq!(layout(&path).unwrap(), Some((4, 7)));
        std::fs::write(&path, b"plain executable without payload").unwrap();
        assert_eq!(layout(&path).unwrap(), None);
    }
}
