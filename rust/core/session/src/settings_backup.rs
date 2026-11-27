//! Settings backup and restore: one ZIP archive with the host's settings
//! (`sunshine.conf`, which holds webhook URLs/events without secrets), the app list, covers, app
//! aliases, each device's own settings and the game settings files kept per
//! device. Pairing identity and credentials (this PC's certificate and key,
//! the paired devices' certificates, the console sign-in, API tokens and the
//! webhook secrets under `credentials/webhook_secrets.json`) go in only when the user chooses so: they are secrets.
//!
//! The idea and the tray's export, import and restore follow Foundation
//! Sunshine v2025.1011.143611 (AlkaidLab/foundation-sunshine#248, GPL-3.0);
//! the archive format and its checks are Rubylight's.
//!
//! A restore is staged: the console checks the archive and shows what changes
//! ([`summarize`]), the host saves the current state as a backup of its own,
//! stages the archive under `backups/` and restarts; the next start applies it
//! ([`apply_staged`]) before anything is loaded, so no running part of the
//! host writes its old state over the restored files.
use crate::state::ProfileFiles;
use anyhow::{Context, Result, bail};
use butterpollo_config::config::Config;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};

mod restore;
pub use restore::{apply, apply_staged, summarize};

/// The manifest's `format`.
pub const FORMAT: &str = "rubylight-settings";
/// The newest archive version this host reads and the one it writes.
pub const VERSION: u32 = 1;
/// The manifest's name in the archive.
pub const MANIFEST: &str = "backup.json";
/// The folder under the data directory that holds pending, staged and
/// before-restore archives.
pub const BACKUPS: &str = "backups";
/// The archive the next start applies.
pub const STAGED: &str = "restore-staged.zip";
/// The largest archive the host takes.
pub const MAX_ARCHIVE: u64 = 512 << 20;
/// The largest single file in an archive.
const MAX_ENTRY: u64 = 256 << 20;
/// The most files an archive may hold.
const MAX_FILES: usize = 20_000;

const CONFIG: &str = "sunshine.conf";
const APPS: &str = "apps.json";
const ALIASES: &str = "vibeshine_state.json";
const DEVICES: &str = "devices.json";
/// Secrets only: the paired devices with their certificates, and the console
/// sign-in when it is kept in the same file.
const PAIRED: &str = "sunshine_state.json";
/// Secrets only: the console sign-in and API tokens when kept apart.
const CREDENTIALS_FILE: &str = "sunshine_credentials.json";
const COVERS: &str = "covers";
const GAME_SETTINGS: &str = crate::game_settings::FOLDER;
/// Secrets only: this PC's pairing certificate and key, and the console's.
const CREDENTIALS_DIR: &str = "credentials";
const CERTIFICATE: &str = "credentials/cacert.pem";
const KEY: &str = "credentials/cakey.pem";
const WAKE_RELAY_FILE: &str = crate::wake::relay::CREDENTIALS_FILE;
const LEGACY_WAKE_RELAY_FILE: &str = crate::wake::relay::FILE;
/// The settings that say where the host's files are (`ProfileFiles`).
const LOCATION_KEYS: [&str; 6] = [
    "file_state",
    "credentials_file",
    "file_apps",
    "vibeshine_file_state",
    "cert",
    "pkey",
];

/// What an archive holds, written last into it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub format: String,
    pub version: u32,
    pub host_version: String,
    pub created_unix: u64,
    /// Whether the pairing identity and credentials are in it.
    pub secrets: bool,
    pub files: Vec<Entry>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

/// A checked archive, held in memory.
#[derive(Clone, Debug)]
pub struct Archive {
    pub manifest: Manifest,
    files: BTreeMap<String, Vec<u8>>,
}

/// One line of what a restore changes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Change {
    /// What it is about: `settings`, `apps`, `covers`, `game_settings`,
    /// `devices`, `pairing`, `credentials` or `webhooks`.
    pub item: &'static str,
    pub detail: String,
}

/// What a restore of an archive changes on this PC.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Summary {
    pub created_unix: u64,
    pub host_version: String,
    pub secrets: bool,
    pub changes: Vec<Change>,
}

/// A `sunshine.conf` key that holds a secret: the SteamGridDB API key
/// (`steamgriddb_api_key`), or what an imported file may hold (a legacy `password`).
pub fn secret_key(key: &str) -> bool {
    key == "password"
        || key.ends_with("_password")
        || key == "api_token"
        || key.ends_with("_api_key")
        || key.ends_with("_secret")
}

/// The configuration as text, without its secrets when `secrets` is false:
/// secret keys are left out and webhook inline secrets are always stripped.
fn config_text(config: &Config, secrets: bool) -> String {
    let mut values = config.values.clone();
    if !secrets {
        values.retain(|key, _| !secret_key(key));
    }
    if let Some(hooks) = values.get_mut("webhooks")
        && let Some((stripped, _)) = crate::webhook::split_json_secrets(hooks)
    {
        *hooks = stripped;
    }
    Config { values }.text()
}

/// The keys of a paired device record that are not its own settings, or that
/// are secrets.
fn device_identity_key(key: &str) -> bool {
    matches!(
        key,
        "name"
            | "cert"
            | "uuid"
            | "perm"
            | "enabled"
            | "webrtc"
            | "host_control"
            | "pairing_approve"
            | "allow_sleep"
            | "allow_restart_shutdown"
            | "allow_usb"
            | "coop_join"
            | "shared_folders"
            | "allow_client_commands"
    ) || key == crate::invites::MARKER
        || key.contains("secret")
        || key.contains("token")
        || key.contains("password")
}

fn named_devices(document: &Value) -> Vec<&serde_json::Map<String, Value>> {
    document
        .pointer("/root/named_devices")
        .and_then(Value::as_array)
        .map(|list| list.iter().filter_map(Value::as_object).collect())
        .unwrap_or_default()
}

/// Each paired device's own settings, without its certificate, permissions or
/// secrets: what `devices.json` holds.
fn device_settings(paired: &Value) -> Value {
    let devices: Vec<Value> = named_devices(paired)
        .into_iter()
        .filter_map(|device| {
            let uuid = device.get("uuid")?.as_str()?;
            let settings: serde_json::Map<String, Value> = device
                .iter()
                .filter(|(key, _)| !device_identity_key(key))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect();
            Some(json!({
                "uuid": uuid,
                "name": device.get("name").cloned().unwrap_or(Value::Null),
                "settings": settings,
            }))
        })
        .collect();
    json!({ "devices": devices })
}

/// The files of `folder` and below, as archive paths under `prefix`.
fn folder_files(folder: &Path, prefix: &str, out: &mut Vec<(String, PathBuf)>) -> Result<()> {
    let entries = match std::fs::read_dir(folder) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).with_context(|| format!("{} could not be read", folder.display()));
        }
    };
    for entry in entries {
        let entry = entry.with_context(|| format!("reading {}", folder.display()))?;
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        // Temporary files of an atomic write are not settings.
        if name.starts_with('.') && name.ends_with(".tmp") {
            continue;
        }
        let path = format!("{prefix}/{name}");
        if kind.is_dir() {
            folder_files(&entry.path(), &path, out)?;
        } else if kind.is_file() {
            let len = entry.metadata()?.len();
            if len > MAX_ENTRY {
                bail!(
                    "{path} is too large for a settings backup (over {} MiB)",
                    MAX_ENTRY >> 20
                );
            }
            out.push((path, entry.path()));
        }
    }
    Ok(())
}

/// The console's sign-in sessions in vibeshine_state.json (`root.session_tokens`: token hashes,
/// user names, addresses, browsers). No backup holds them, with secrets or without, and a restore
/// keeps the target's own.
const SESSIONS: &str = "session_tokens";
/// API tokens in vibeshine_state.json (`root.api_tokens`). Secrets-free backups strip them on
/// export and keep the target's own on restore; backups with secrets carry them.
const API_TOKENS: &str = "api_tokens";

/// vibeshine_state.json without the console's sign-in sessions (and without API tokens when
/// `secrets` is false); None for a file that is not the JSON object it should be, which then stays
/// out of the backup rather than go out whole.
fn without_sessions(bytes: &[u8], secrets: bool) -> Option<Vec<u8>> {
    let mut document: Value = serde_json::from_slice(bytes).ok()?;
    let root = document.as_object_mut()?;
    if let Some(inner) = root.get_mut("root").and_then(Value::as_object_mut) {
        inner.remove(SESSIONS);
        if !secrets {
            inner.remove(API_TOKENS);
        }
    }
    root.remove(SESSIONS);
    if !secrets {
        root.remove(API_TOKENS);
    }
    serde_json::to_vec_pretty(&document).ok()
}

/// The archive's vibeshine_state.json with the target's console sign-in sessions (and API tokens
/// when `secrets` is false) from `current` in place of any it carries: a restore never signs the
/// console in or out, and a non-secret restore never leaks or overwrites API tokens.
fn keeping_sessions(archived: &[u8], current: Option<&[u8]>, secrets: bool) -> Result<Vec<u8>> {
    let stripped = without_sessions(archived, secrets)
        .context("the archive's vibeshine_state.json is not a JSON object")?;
    let mut document: Value = serde_json::from_slice(&stripped)?;
    let current_root = current
        .and_then(|bytes| serde_json::from_slice::<Value>(bytes).ok())
        .and_then(|current| current.get("root").cloned());
    let mut preserve = |key: &str| {
        if let Some(value) = current_root
            .as_ref()
            .and_then(|root| root.get(key))
            .cloned()
            && let Some(inner) = document
                .as_object_mut()
                .map(|root| root.entry("root").or_insert_with(|| json!({})))
                .and_then(Value::as_object_mut)
        {
            inner.insert(key.into(), value);
        }
    };
    preserve(SESSIONS);
    if !secrets {
        preserve(API_TOKENS);
    }
    Ok(serde_json::to_vec_pretty(&document)?)
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("{} could not be read", path.display())),
    }
}

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Writes the settings of the host whose data folder is `directory` into
/// `out` as a backup archive.
pub fn export<W: Write + Seek>(
    directory: &Path,
    secrets: bool,
    host_version: &str,
    created_unix: u64,
    out: W,
) -> Result<Manifest> {
    let config = Config::load(&directory.join(CONFIG))?;
    let files = ProfileFiles::new(&config, directory);
    let mut contents: Vec<(String, Vec<u8>)> =
        vec![(CONFIG.into(), config_text(&config, secrets).into_bytes())];
    if let Some(bytes) = read_optional(&files.apps)? {
        contents.push((APPS.into(), bytes));
    }
    // Never the console's sign-in sessions (`SESSIONS`), and without secrets never `API_TOKENS`.
    if let Some(bytes) = read_optional(&files.aliases)?
        .as_deref()
        .and_then(|bytes| without_sessions(bytes, secrets))
    {
        contents.push((ALIASES.into(), bytes));
    }
    let mut folders = Vec::new();
    folder_files(&directory.join(COVERS), COVERS, &mut folders)?;
    folder_files(&directory.join(GAME_SETTINGS), GAME_SETTINGS, &mut folders)?;
    let paired = read_optional(&files.paired)?;
    if secrets {
        if let Some(bytes) = paired {
            contents.push((PAIRED.into(), bytes));
        }
        if files.credentials != files.paired
            && let Some(bytes) = read_optional(&files.credentials)?
        {
            contents.push((CREDENTIALS_FILE.into(), bytes));
        }
        folder_files(
            &directory.join(CREDENTIALS_DIR),
            CREDENTIALS_DIR,
            &mut folders,
        )?;
        for (name, path) in [(CERTIFICATE, &files.certificate), (KEY, &files.key)] {
            if !folders.iter().any(|(listed, _)| listed == name)
                && let Some(bytes) = read_optional(path)?
            {
                contents.push((name.into(), bytes));
            }
        }
        // Carry an unmigrated root wake-relay.json inside `credentials/wake-relay.json`.
        if !folders.iter().any(|(listed, _)| listed == WAKE_RELAY_FILE)
            && let Some(bytes) = read_optional(&directory.join(LEGACY_WAKE_RELAY_FILE))?
        {
            contents.push((WAKE_RELAY_FILE.into(), bytes));
        }
        // Carry webhook secrets cleanly inside `credentials/webhook_secrets.json`, merging any
        // unmigrated inline secrets from `sunshine.conf`.
        if let Some((_, inline)) = config
            .values
            .get("webhooks")
            .and_then(|raw| crate::webhook::split_json_secrets(raw))
            && !inline.is_empty()
        {
            let mut merged =
                crate::webhook::load_secrets(&crate::webhook::secrets_path(directory))?;
            for (url, secret) in inline {
                merged.entry(url).or_insert(secret);
            }
            folders.retain(|(listed, _)| listed != crate::webhook::SECRETS_FILE);
            contents.push((
                crate::webhook::SECRETS_FILE.into(),
                crate::webhook::encode_secrets(&merged)?,
            ));
        }
    } else if let Some(bytes) = paired {
        let document: Value =
            serde_json::from_slice(&bytes).context("the paired devices file is not JSON")?;
        contents.push((
            DEVICES.into(),
            serde_json::to_vec_pretty(&device_settings(&document))?,
        ));
    }
    for (name, path) in folders {
        let bytes = std::fs::read(&path)
            .with_context(|| format!("{} could not be read", path.display()))?;
        contents.push((name, bytes));
    }
    if contents.len() > MAX_FILES {
        bail!("the backup holds more than {MAX_FILES} files");
    }
    let mut total_size = 0u64;
    for (name, bytes) in &contents {
        let len = bytes.len() as u64;
        if len > MAX_ENTRY {
            bail!("{name} is too large for a settings backup");
        }
        total_size = total_size.saturating_add(len);
        if total_size > MAX_ARCHIVE {
            bail!("the backup exceeds {} MiB", MAX_ARCHIVE >> 20);
        }
    }
    use zip::write::SimpleFileOptions;
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let mut writer = zip::ZipWriter::new(out);
    let mut entries = Vec::with_capacity(contents.len());
    for (name, bytes) in &contents {
        writer.start_file(name.as_str(), options)?;
        writer.write_all(bytes)?;
        entries.push(Entry {
            path: name.clone(),
            size: bytes.len() as u64,
            sha256: sha256(bytes),
        });
    }
    let manifest = Manifest {
        format: FORMAT.into(),
        version: VERSION,
        host_version: host_version.into(),
        created_unix,
        secrets,
        files: entries,
    };
    writer.start_file(MANIFEST, options)?;
    writer.write_all(&serde_json::to_vec_pretty(&manifest)?)?;
    writer.finish()?;
    Ok(manifest)
}

/// Whether `path` is a file an archive may hold, and whether only one with
/// secrets may.
fn allowed(path: &str) -> Option<bool> {
    let safe = !path.is_empty()
        && !path.starts_with('/')
        && !path.contains('\\')
        && !path.contains(':')
        && !path.contains('\0')
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..");
    if !safe {
        return None;
    }
    match path {
        CONFIG | APPS | ALIASES | DEVICES => Some(false),
        PAIRED | CREDENTIALS_FILE | LEGACY_WAKE_RELAY_FILE => Some(true),
        _ => {
            let (top, _rest) = path.split_once('/')?;
            match top {
                COVERS => Some(false),
                GAME_SETTINGS => Some(false),
                CREDENTIALS_DIR => Some(true),
                _ => None,
            }
        }
    }
}

fn json_object(bytes: &[u8], what: &str) -> Result<Value> {
    let value: Value =
        serde_json::from_slice(bytes).with_context(|| format!("{what} is not valid JSON"))?;
    if !value.is_object() {
        bail!("{what} is not a JSON object");
    }
    Ok(value)
}

/// Reads and checks an archive: a known format and version, every file listed
/// in the manifest with its size and checksum and nothing else, only paths
/// the host writes, secrets only in an archive that says it has them, and
/// settings, app list and device files that the host can read.
pub fn read<R: Read + Seek>(reader: R) -> Result<Archive> {
    let mut zip = zip::ZipArchive::new(reader).context("this is not a ZIP archive")?;
    if zip.len() > MAX_FILES + 1 {
        bail!("the archive holds more than {MAX_FILES} files");
    }
    let mut files = BTreeMap::new();
    let mut manifest = None;
    let mut total = 0u64;
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
        let mut bytes = Vec::with_capacity(file.size().min(1 << 20) as usize);
        (&mut file)
            .take(MAX_ENTRY + 1)
            .read_to_end(&mut bytes)
            .with_context(|| format!("{name} in the archive could not be read"))?;
        total += bytes.len() as u64;
        if bytes.len() as u64 > MAX_ENTRY || total > MAX_ARCHIVE {
            bail!("the archive is too large");
        }
        if name == MANIFEST {
            manifest = Some(bytes);
        } else if files.insert(name.clone(), bytes).is_some() {
            bail!("{name} is in the archive twice");
        }
    }
    let manifest: Manifest = serde_json::from_slice(
        &manifest.context("the archive is no Rubylight settings backup: it has no backup.json")?,
    )
    .context("the archive's backup.json cannot be read")?;
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
    for entry in &manifest.files {
        let Some(bytes) = files.get(&entry.path) else {
            bail!("{} is missing from the archive", entry.path);
        };
        if bytes.len() as u64 != entry.size || sha256(bytes) != entry.sha256 {
            bail!("{} in the archive was changed or damaged", entry.path);
        }
    }
    for name in files.keys() {
        if !listed.contains(name.as_str()) {
            bail!("{name} in the archive is not in its backup.json");
        }
        match allowed(name) {
            None => bail!("{name} is not a file a settings backup holds"),
            Some(true) if !manifest.secrets => {
                bail!("{name} holds secrets, but the backup says it has none")
            }
            Some(false) if name == DEVICES && manifest.secrets => {
                bail!("a backup with secrets has no devices.json")
            }
            _ => {}
        }
    }
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
    Ok(Archive { manifest, files })
}

/// Checks a written archive is readable and matches [`read`]'s checks.
pub fn verify<R: Read + Seek>(reader: R) -> Result<Manifest> {
    read(reader).map(|archive| archive.manifest)
}

impl Archive {
    fn config(&self) -> Config {
        // Checked by `read`.
        Config::parse(std::str::from_utf8(&self.files[CONFIG]).unwrap_or_default())
            .unwrap_or_default()
    }
    fn json(&self, name: &str) -> Option<Value> {
        serde_json::from_slice(self.files.get(name)?).ok()
    }
    fn under<'a>(
        &'a self,
        prefix: &'a str,
    ) -> impl Iterator<Item = (&'a String, &'a Vec<u8>)> + 'a {
        self.files.iter().filter(move |(name, _)| {
            name.strip_prefix(prefix)
                .is_some_and(|r| r.starts_with('/'))
        })
    }
}

#[cfg(test)]
mod tests;
