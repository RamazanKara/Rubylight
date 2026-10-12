//! The release package appended to setup.exe: the executable, then the ZIP,
//! then a 16-byte footer (`BPSETUP1` and the ZIP's length, little endian).
//! Authenticode may append alignment padding and a certificate table.
use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Component, Path, PathBuf},
};

const MAGIC: &[u8; 8] = b"BPSETUP1";

fn certificate_offset(file: &mut File, total: u64) -> Result<Option<u64>> {
    if total < 64 {
        return Ok(None);
    }
    file.seek(SeekFrom::Start(0))?;
    let mut dos = [0u8; 64];
    file.read_exact(&mut dos)?;
    if &dos[..2] != b"MZ" {
        return Ok(None);
    }
    let pe = u32::from_le_bytes(dos[60..64].try_into().unwrap()) as u64;
    if pe + 24 > total {
        bail!("invalid setup PE header");
    }
    file.seek(SeekFrom::Start(pe))?;
    let mut header = [0u8; 24];
    file.read_exact(&mut header)?;
    if &header[..4] != b"PE\0\0" {
        bail!("invalid setup PE signature");
    }
    let size = u16::from_le_bytes(header[20..22].try_into().unwrap()) as usize;
    if pe + 24 + size as u64 > total || size < 2 {
        bail!("invalid setup optional header");
    }
    let mut optional = vec![0u8; size];
    file.read_exact(&mut optional)?;
    let directories = match u16::from_le_bytes(optional[..2].try_into().unwrap()) {
        0x10b => 96,
        0x20b => 112,
        _ => bail!("unsupported setup PE format"),
    };
    if size < directories
        || u32::from_le_bytes(optional[directories - 4..directories].try_into().unwrap()) < 5
    {
        return Ok(None);
    }
    if size < directories + 40 {
        bail!("missing setup certificate directory");
    }
    let entry = &optional[directories + 32..directories + 40];
    let offset = u32::from_le_bytes(entry[..4].try_into().unwrap()) as u64;
    let length = u32::from_le_bytes(entry[4..].try_into().unwrap()) as u64;
    if offset == 0 && length == 0 {
        return Ok(None);
    }
    if offset < pe + 24 + size as u64
        || !offset.is_multiple_of(8)
        || length < 8
        || offset + length != total
    {
        bail!("invalid setup certificate table");
    }
    Ok(Some(offset))
}

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
    if total < 16 {
        return Ok(None);
    }
    let certificate = certificate_offset(&mut file, total)?;
    let end = certificate.unwrap_or(total);
    // Signing aligns the certificate table to eight bytes, after our footer.
    let padding = if certificate.is_some() { 7 } else { 0 };
    for pad in 0..=padding.min(end.saturating_sub(16)) {
        let footer_at = end - pad - 16;
        file.seek(SeekFrom::Start(footer_at))?;
        let mut tail = vec![0u8; 16 + pad as usize];
        file.read_exact(&mut tail)?;
        if &tail[..8] != MAGIC || tail[16..].iter().any(|&b| b != 0) {
            continue;
        }
        let length = u64::from_le_bytes(tail[8..16].try_into().unwrap());
        if length == 0 || length > footer_at {
            bail!("setup payload is damaged");
        }
        return Ok(Some((footer_at - length, length)));
    }
    Ok(None)
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
fn stub_length(exe: &Path) -> Result<u64> {
    let mut file = File::open(exe)?;
    let total = file.metadata()?.len();
    // Truncating a signed installer invalidates its signature. Keep it intact.
    if certificate_offset(&mut file, total)?.is_some() {
        Ok(total)
    } else {
        Ok(layout(exe)?.map_or(total, |(start, _)| start))
    }
}
/// Copy setup.exe to serve as the uninstaller, preserving any signature.
pub fn write_stub(destination: &Path) -> Result<()> {
    let exe = std::env::current_exe()?;
    let length = stub_length(&exe)?;
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
        assert_eq!(stub_length(&path).unwrap(), 4);
        std::fs::write(&path, b"plain executable without payload").unwrap();
        assert_eq!(layout(&path).unwrap(), None);
    }

    #[test]
    fn signed_payload_skips_certificate_padding_and_keeps_the_uninstaller_intact() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("setup.exe");
        for (magic, directories) in [(0x10bu16, 96usize), (0x20b, 112)] {
            for length in 1..=8usize {
                let mut bytes = vec![0u8; 512];
                bytes[..2].copy_from_slice(b"MZ");
                bytes[60..64].copy_from_slice(&64u32.to_le_bytes());
                bytes[64..68].copy_from_slice(b"PE\0\0");
                bytes[84..86].copy_from_slice(&240u16.to_le_bytes());
                bytes[88..90].copy_from_slice(&magic.to_le_bytes());
                bytes[88 + directories - 4..88 + directories].copy_from_slice(&16u32.to_le_bytes());
                bytes.extend(vec![42; length]);
                bytes.extend_from_slice(MAGIC);
                bytes.extend_from_slice(&(length as u64).to_le_bytes());
                bytes.resize(bytes.len().next_multiple_of(8), 0);
                let certificate = bytes.len() as u32;
                let entry = 88 + directories + 32;
                bytes[entry..entry + 4].copy_from_slice(&certificate.to_le_bytes());
                bytes[entry + 4..entry + 8].copy_from_slice(&32u32.to_le_bytes());
                bytes.extend_from_slice(&[0xaa; 32]);
                std::fs::write(&path, &bytes).unwrap();
                assert_eq!(layout(&path).unwrap(), Some((512, length as u64)));
                assert_eq!(stub_length(&path).unwrap(), bytes.len() as u64);
                bytes.pop();
                std::fs::write(&path, &bytes).unwrap();
                assert!(layout(&path).is_err());
            }
        }
    }
}
