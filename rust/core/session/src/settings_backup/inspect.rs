//! Streaming ZIP validation and inspection for settings backup archives.
//! Reads and hashes entries one at a time in fixed chunks so inspecting a large backup
//! never holds the full `MAX_ARCHIVE` payload in memory.

use super::{
    ALIASES, APPS, CONFIG, COVERS, CREDENTIALS_FILE, DEVICES, Entry, FORMAT, GAME_SETTINGS,
    LEGACY_WAKE_RELAY_FILE, MANIFEST, MAX_ARCHIVE, MAX_ENTRY, MAX_FILES, Manifest, PAIRED, Summary,
    VERSION, WAKE_RELAY_FILE, allowed, json_object, restore,
};
use anyhow::{Context, Result, bail};
use butterpollo_config::config::Config;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Seek};
use std::path::Path;

/// The largest single metadata JSON/config file buffered during streaming inspection.
const MAX_METADATA: u64 = 32 << 20;
/// Chunk size used when streaming and hashing ZIP entries.
const STREAM_CHUNK: usize = 64 * 1024;

fn is_metadata_file(name: &str) -> bool {
    matches!(
        name,
        CONFIG
            | APPS
            | ALIASES
            | DEVICES
            | PAIRED
            | CREDENTIALS_FILE
            | crate::webhook::SECRETS_FILE
            | WAKE_RELAY_FILE
            | LEGACY_WAKE_RELAY_FILE
    )
}

pub(super) fn validate_manifest_header(manifest: &Manifest) -> Result<()> {
    if manifest.format != FORMAT {
        bail!("the archive is no Rubylight settings backup");
    }
    if manifest.version > VERSION {
        bail!(
            "the backup was made by a newer Rubylight ({}); update this PC first",
            manifest.host_version
        );
    }
    let listed: BTreeSet<&str> = manifest.files.iter().map(|e| e.path.as_str()).collect();
    if listed.len() != manifest.files.len() {
        bail!("the archive's backup.json lists a file twice");
    }
    Ok(())
}

pub(super) fn validate_entry_path(name: &str, secrets: bool) -> Result<()> {
    match allowed(name) {
        None => bail!("{name} is not a file a settings backup holds"),
        Some(true) if !secrets => {
            bail!("{name} holds secrets, but the backup says it has none")
        }
        Some(false) if name == DEVICES && secrets => {
            bail!("a backup with secrets has no devices.json")
        }
        _ => Ok(()),
    }
}

pub(super) fn validate_metadata_files(files: &BTreeMap<String, Vec<u8>>) -> Result<()> {
    let config = files
        .get(CONFIG)
        .context("the archive has no sunshine.conf")?;
    Config::parse(std::str::from_utf8(config).context("the archive's sunshine.conf is not UTF-8")?)
        .context("the archive's sunshine.conf cannot be read")?;
    if let Some(apps) = files.get(APPS) {
        let apps = json_object(apps, "the archive's apps.json")?;
        if !apps.get("apps").is_some_and(Value::is_array) {
            bail!("the archive's apps.json has no app list");
        }
    }
    for (name, what) in [
        (ALIASES, "the archive's vibeshine_state.json"),
        (DEVICES, "the archive's devices.json"),
        (PAIRED, "the archive's sunshine_state.json"),
        (CREDENTIALS_FILE, "the archive's sunshine_credentials.json"),
        (
            crate::webhook::SECRETS_FILE,
            "the archive's webhook_secrets.json",
        ),
        (WAKE_RELAY_FILE, "the archive's wake-relay.json"),
        (LEGACY_WAKE_RELAY_FILE, "the archive's wake-relay.json"),
    ] {
        if let Some(bytes) = files.get(name) {
            json_object(bytes, what)?;
        }
    }
    Ok(())
}

/// Validated archive metadata produced without holding cover/game-settings blobs in RAM.
pub(super) struct Inspected {
    pub(super) manifest: Manifest,
    pub(super) metadata: BTreeMap<String, Vec<u8>>,
    pub(super) covers: usize,
    pub(super) game_settings: usize,
}

pub(super) fn scan_archive<R: Read + Seek>(reader: R) -> Result<Inspected> {
    let mut zip = zip::ZipArchive::new(reader).context("this is not a ZIP archive")?;
    if zip.len() > MAX_FILES + 1 {
        bail!("the archive holds more than {MAX_FILES} files");
    }
    let manifest_bytes = {
        let mut file = zip.by_name(MANIFEST).map_err(|_| {
            anyhow::anyhow!("the archive is no Rubylight settings backup: it has no backup.json")
        })?;
        if !file.is_file() {
            bail!("{MANIFEST} in the archive is not a plain file");
        }
        if file.size() > MAX_METADATA {
            bail!("{MANIFEST} in the archive is too large");
        }
        let mut bytes = Vec::with_capacity(file.size().min(1 << 20) as usize);
        (&mut file)
            .take(MAX_METADATA + 1)
            .read_to_end(&mut bytes)
            .context("the archive's backup.json could not be read")?;
        if bytes.len() as u64 > MAX_METADATA {
            bail!("the archive is too large");
        }
        bytes
    };
    let manifest: Manifest = serde_json::from_slice(&manifest_bytes)
        .context("the archive's backup.json cannot be read")?;
    validate_manifest_header(&manifest)?;
    let expected_by_path: BTreeMap<&str, &Entry> = manifest
        .files
        .iter()
        .map(|entry| (entry.path.as_str(), entry))
        .collect();
    let mut metadata = BTreeMap::new();
    let mut seen_entries = BTreeSet::new();
    let mut seen_manifest = false;
    let mut total = 0u64;
    let mut covers = 0usize;
    let mut game_settings = 0usize;
    let mut chunk = vec![0u8; STREAM_CHUNK];
    for index in 0..zip.len() {
        let mut file = zip.by_index(index)?;
        if file.is_dir() {
            continue;
        }
        let name = file.name().to_owned();
        if !file.is_file() {
            bail!("{name} in the archive is not a plain file");
        }
        if file.size() > MAX_ENTRY {
            bail!("{name} in the archive is too large");
        }
        if name == MANIFEST {
            if seen_manifest {
                bail!("{name} is in the archive twice");
            }
            seen_manifest = true;
            let mut size = 0u64;
            loop {
                let n = file
                    .read(&mut chunk)
                    .with_context(|| format!("{name} in the archive could not be read"))?;
                if n == 0 {
                    break;
                }
                size = size.saturating_add(n as u64);
                total = total.saturating_add(n as u64);
                if size > MAX_METADATA || total > MAX_ARCHIVE {
                    bail!("the archive is too large");
                }
            }
            continue;
        }
        if !seen_entries.insert(name.clone()) {
            bail!("{name} is in the archive twice");
        }
        let Some(expected) = expected_by_path.get(name.as_str()) else {
            bail!("{name} in the archive is not in its backup.json");
        };
        validate_entry_path(&name, manifest.secrets)?;
        let keep = is_metadata_file(&name);
        let mut buffered = Vec::new();
        let mut hasher = Sha256::new();
        let mut size = 0u64;
        loop {
            let n = file
                .read(&mut chunk)
                .with_context(|| format!("{name} in the archive could not be read"))?;
            if n == 0 {
                break;
            }
            size = size.saturating_add(n as u64);
            total = total.saturating_add(n as u64);
            if size > MAX_ENTRY || total > MAX_ARCHIVE || (keep && size > MAX_METADATA) {
                bail!("the archive is too large");
            }
            hasher.update(&chunk[..n]);
            if keep {
                buffered.extend_from_slice(&chunk[..n]);
            }
        }
        if size != expected.size || hex::encode(hasher.finalize()) != expected.sha256 {
            bail!("{name} in the archive was changed or damaged");
        }
        if keep {
            metadata.insert(name.clone(), buffered);
        }
        if name
            .strip_prefix(COVERS)
            .is_some_and(|r| r.starts_with('/'))
        {
            covers += 1;
        } else if name
            .strip_prefix(GAME_SETTINGS)
            .is_some_and(|r| r.starts_with('/'))
        {
            game_settings += 1;
        }
    }
    for entry in &manifest.files {
        if !seen_entries.contains(&entry.path) {
            bail!("{} is missing from the archive", entry.path);
        }
    }
    validate_metadata_files(&metadata)?;
    Ok(Inspected {
        manifest,
        metadata,
        covers,
        game_settings,
    })
}

/// Validates and summarizes a backup archive directly from a seekable reader, streaming and hashing
/// entries one at a time instead of buffering up to `MAX_ARCHIVE` in memory.
pub fn inspect_reader<R: Read + Seek>(reader: R, directory: &Path) -> Result<Summary> {
    let inspected = scan_archive(reader)?;
    Ok(restore::summarize_parts(
        &inspected.manifest,
        &inspected.metadata,
        inspected.covers,
        inspected.game_settings,
        directory,
    ))
}
