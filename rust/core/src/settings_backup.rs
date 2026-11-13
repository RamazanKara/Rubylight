//! Settings backup and restore: one ZIP archive with the host's settings
//! (`sunshine.conf`, which holds the webhooks too), the app list, covers, app
//! aliases, each device's own settings and the game settings files kept per
//! device. Pairing identity and credentials (this PC's certificate and key,
//! the paired devices' certificates, the console sign-in, API tokens and the
//! webhook secrets) go in only when the user chooses so: they are secrets.
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
use crate::config::Config;
use crate::state::ProfileFiles;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};

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

/// A `sunshine.conf` key that holds a secret. The host keeps none there
/// itself; an imported file may (a legacy `password`).
pub fn secret_key(key: &str) -> bool {
    key == "password"
        || key.ends_with("_password")
        || key == "api_token"
        || key.ends_with("_secret")
}

/// The configuration as text, without its secrets when `secrets` is false:
/// secret keys are left out and each webhook's secret is emptied.
fn config_text(config: &Config, secrets: bool) -> String {
    if secrets {
        return config.text();
    }
    let mut values = config.values.clone();
    values.retain(|key, _| !secret_key(key));
    if let Some(hooks) = values.get_mut("webhooks")
        && let Ok(Value::Array(mut list)) = serde_json::from_str::<Value>(hooks)
    {
        for hook in &mut list {
            if let Some(object) = hook.as_object_mut() {
                object.insert("secret".into(), Value::String(String::new()));
            }
        }
        *hooks = Value::Array(list).to_string();
    }
    Config { values }.text()
}

/// The keys of a paired device record that are not its own settings, or that
/// are secrets.
fn device_identity_key(key: &str) -> bool {
    matches!(
        key,
        "name" | "cert" | "uuid" | "perm" | "enabled" | "webrtc"
    ) || key.contains("secret")
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
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Ok(());
    };
    for entry in entries {
        let entry = entry?;
        let kind = entry.file_type()?;
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
            out.push((path, entry.path()));
        }
    }
    Ok(())
}

/// The console's sign-in sessions in vibeshine_state.json (`root.session_tokens`: token hashes,
/// user names, addresses, browsers). No backup holds them, with secrets or without, and a restore
/// keeps the target's own.
const SESSIONS: &str = "session_tokens";

/// vibeshine_state.json without the console's sign-in sessions; None for a file that is not the
/// JSON object it should be, which then stays out of the backup rather than go out whole.
fn without_sessions(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut document: Value = serde_json::from_slice(bytes).ok()?;
    let root = document.as_object_mut()?;
    if let Some(inner) = root.get_mut("root").and_then(Value::as_object_mut) {
        inner.remove(SESSIONS);
    }
    root.remove(SESSIONS);
    serde_json::to_vec_pretty(&document).ok()
}

/// The archive's vibeshine_state.json with the target's console sign-in sessions (`current`, the
/// target's file) in place of any it carries: a restore never signs the console in or out.
fn keeping_sessions(archived: &[u8], current: Option<&[u8]>) -> Result<Vec<u8>> {
    let stripped = without_sessions(archived)
        .context("the archive's vibeshine_state.json is not a JSON object")?;
    let mut document: Value = serde_json::from_slice(&stripped)?;
    let sessions = current
        .and_then(|bytes| serde_json::from_slice::<Value>(bytes).ok())
        .and_then(|current| current.get("root")?.get(SESSIONS).cloned());
    if let Some(sessions) = sessions {
        let root = document
            .as_object_mut()
            .context("vibeshine_state.json is not a JSON object")?;
        let inner = root.entry("root").or_insert_with(|| json!({}));
        if let Some(inner) = inner.as_object_mut() {
            inner.insert(SESSIONS.into(), sessions);
        }
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
    // Never the console's sign-in sessions (`SESSIONS`).
    if let Some(bytes) = read_optional(&files.aliases)?
        .as_deref()
        .and_then(without_sessions)
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
        PAIRED | CREDENTIALS_FILE => Some(true),
        _ => {
            let (top, rest) = path.split_once('/')?;
            match top {
                COVERS if !rest.contains('/') => Some(false),
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
        let mut bytes = Vec::with_capacity(file.size() as usize);
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
    ] {
        if let Some(bytes) = files.get(name) {
            json_object(bytes, what)?;
        }
    }
    Ok(Archive { manifest, files })
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

fn current_config(directory: &Path) -> Config {
    Config::load(&directory.join(CONFIG)).unwrap_or_default()
}

fn app_names(document: Option<&Value>) -> BTreeMap<String, String> {
    document
        .and_then(|d| d.get("apps"))
        .and_then(Value::as_array)
        .map(|apps| {
            apps.iter()
                .filter_map(|app| {
                    let key = app
                        .get("uuid")
                        .or_else(|| app.get("name"))
                        .and_then(Value::as_str)?
                        .to_owned();
                    Some((key, app.to_string()))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn webhook_urls(config: &Config) -> Vec<String> {
    serde_json::from_str::<Value>(config.get("webhooks", "[]"))
        .ok()
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|hook| hook.get("url")?.as_str().map(str::to_owned))
        .collect()
}

/// What restoring `archive` changes on the host whose data folder is
/// `directory`.
pub fn summarize(archive: &Archive, directory: &Path) -> Summary {
    let secrets = archive.manifest.secrets;
    let current = current_config(directory);
    // As `apply` writes it, so a webhook whose secret this PC keeps does not
    // show as a change; one that gets a fresh secret does.
    let restored = merged_config(archive, &current, String::new);
    let mut changes = Vec::new();
    let keys: BTreeSet<&String> = restored
        .values
        .keys()
        .chain(current.values.keys())
        .collect();
    let changed: Vec<&str> = keys
        .into_iter()
        .filter(|key| secrets || !secret_key(key))
        .filter(|key| restored.values.get(*key) != current.values.get(*key))
        .map(String::as_str)
        .collect();
    changes.push(Change {
        item: "settings",
        detail: if changed.is_empty() {
            "No setting changes.".into()
        } else {
            let shown: Vec<&str> = changed.iter().copied().take(12).collect();
            let more = changed.len() - shown.len();
            format!(
                "{} setting{} change: {}{}.",
                changed.len(),
                if changed.len() == 1 { "" } else { "s" },
                shown.join(", "),
                if more > 0 {
                    format!(" and {more} more")
                } else {
                    String::new()
                }
            )
        },
    });
    let files = ProfileFiles::new(&current, directory);
    if archive.files.contains_key(APPS) {
        let before = app_names(
            read_optional(&files.apps)
                .ok()
                .flatten()
                .and_then(|b| serde_json::from_slice(&b).ok())
                .as_ref(),
        );
        let after = app_names(archive.json(APPS).as_ref());
        let added = after.keys().filter(|k| !before.contains_key(*k)).count();
        let removed = before.keys().filter(|k| !after.contains_key(*k)).count();
        let edited = after
            .iter()
            .filter(|(k, v)| before.get(*k).is_some_and(|old| old != *v))
            .count();
        changes.push(Change {
            item: "apps",
            detail: format!(
                "{} apps in the backup: {added} added, {removed} removed, {edited} changed.",
                after.len()
            ),
        });
    }
    let covers = archive.under(COVERS).count();
    if covers > 0 {
        changes.push(Change {
            item: "covers",
            detail: format!("{covers} covers written; covers only on this PC stay."),
        });
    }
    let kept = archive.under(GAME_SETTINGS).count();
    if kept > 0 {
        changes.push(Change {
            item: "game_settings",
            detail: format!("{kept} game settings files written; files only on this PC stay."),
        });
    }
    let paired_now = read_optional(&files.paired)
        .ok()
        .flatten()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .unwrap_or(Value::Null);
    let here: BTreeSet<String> = named_devices(&paired_now)
        .into_iter()
        .filter_map(|d| d.get("uuid")?.as_str().map(str::to_owned))
        .collect();
    if secrets {
        let devices = archive
            .json(PAIRED)
            .map(|d| named_devices(&d).len())
            .unwrap_or(0);
        changes.push(Change {
            item: "devices",
            detail: format!(
                "Paired devices replaced: {devices} in the backup, {} paired now.",
                here.len()
            ),
        });
        changes.push(Change {
            item: "pairing",
            detail: "This PC's pairing certificate is replaced: devices paired since the backup must pair again.".into(),
        });
        changes.push(Change {
            item: "credentials",
            detail: "The console sign-in and API tokens are replaced by the backup's.".into(),
        });
    } else if let Some(devices) = archive.json(DEVICES) {
        let listed: Vec<String> = devices
            .get("devices")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .filter_map(|d| d.get("uuid")?.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        let matched = listed.iter().filter(|uuid| here.contains(*uuid)).count();
        changes.push(Change {
            item: "devices",
            detail: format!(
                "Settings of {matched} paired devices restored; {} devices in the backup are not paired with this PC and are skipped. Pairings and permissions stay.",
                listed.len() - matched
            ),
        });
        changes.push(Change {
            item: "pairing",
            detail: "Pairing identity, console sign-in and API tokens stay as they are.".into(),
        });
    }
    let hooks = webhook_urls(&restored);
    if !hooks.is_empty() {
        let urls = webhook_urls(&current);
        let fresh = if secrets {
            0
        } else {
            hooks.iter().filter(|url| !urls.contains(url)).count()
        };
        changes.push(Change {
            item: "webhooks",
            detail: if fresh > 0 {
                format!(
                    "{} webhooks; {fresh} get a new secret, to be copied into their receivers.",
                    hooks.len()
                )
            } else {
                format!("{} webhooks.", hooks.len())
            },
        });
    }
    Summary {
        created_unix: archive.manifest.created_unix,
        host_version: archive.manifest.host_version.clone(),
        secrets,
        changes,
    }
}

/// The restored configuration: the archive's, with this PC's secret keys and
/// webhook secrets when the archive has none (a webhook this PC does not have
/// gets `fresh()`).
fn merged_config(archive: &Archive, current: &Config, mut fresh: impl FnMut() -> String) -> Config {
    let mut config = archive.config();
    // Where this PC keeps its files stays this PC's: a backup from another
    // PC, or a crafted one, never moves them or makes the restore write
    // elsewhere.
    for key in LOCATION_KEYS {
        match current.values.get(key) {
            Some(value) => config.values.insert(key.into(), value.clone()),
            None => config.values.remove(key),
        };
    }
    if archive.manifest.secrets {
        return config;
    }
    for (key, value) in &current.values {
        if secret_key(key) {
            config.values.insert(key.clone(), value.clone());
        }
    }
    let secrets: BTreeMap<String, String> =
        serde_json::from_str::<Value>(current.get("webhooks", "[]"))
            .ok()
            .and_then(|v| v.as_array().cloned())
            .unwrap_or_default()
            .iter()
            .filter_map(|hook| {
                Some((
                    hook.get("url")?.as_str()?.to_owned(),
                    hook.get("secret")?.as_str()?.to_owned(),
                ))
            })
            .collect();
    if let Some(hooks) = config.values.get_mut("webhooks")
        && let Ok(Value::Array(mut list)) = serde_json::from_str::<Value>(hooks)
    {
        for hook in &mut list {
            let url = hook
                .get("url")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            if let Some(object) = hook.as_object_mut()
                && object
                    .get("secret")
                    .and_then(Value::as_str)
                    .is_none_or(str::is_empty)
            {
                let secret = secrets
                    .get(&url)
                    .filter(|s| !s.is_empty())
                    .cloned()
                    .unwrap_or_else(&mut fresh);
                object.insert("secret".into(), Value::String(secret));
            }
        }
        *hooks = Value::Array(list).to_string();
    }
    config
}

/// The paired-devices file with each listed device's own settings replaced
/// by the backup's; certificates, permissions, secrets and devices the backup
/// does not list stay.
fn merge_devices(paired: &mut Value, backup: &Value) {
    let Some(list) = paired
        .pointer_mut("/root/named_devices")
        .and_then(Value::as_array_mut)
    else {
        return;
    };
    let restored: BTreeMap<&str, &Value> = backup
        .get("devices")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|d| Some((d.get("uuid")?.as_str()?, d)))
                .collect()
        })
        .unwrap_or_default();
    for device in list.iter_mut().filter_map(Value::as_object_mut) {
        let Some(saved) = device
            .get("uuid")
            .and_then(Value::as_str)
            .and_then(|uuid| restored.get(uuid))
        else {
            continue;
        };
        device.retain(|key, _| device_identity_key(key));
        if let Some(name) = saved.get("name").and_then(Value::as_str) {
            device.insert("name".into(), Value::String(name.to_owned()));
        }
        if let Some(settings) = saved.get("settings").and_then(Value::as_object) {
            for (key, value) in settings {
                if !device_identity_key(key) {
                    device.insert(key.clone(), value.clone());
                }
            }
        }
    }
}

/// Where an archive path such as `covers/a.png` goes under `directory`.
fn folder_target(directory: &Path, name: &str) -> PathBuf {
    name.split('/')
        .fold(directory.to_path_buf(), |path, part| path.join(part))
}

/// Writes `archive` over the settings of the host whose data folder is
/// `directory`. Files that only this PC has (covers, game settings files)
/// stay.
pub fn apply(archive: &Archive, directory: &Path, fresh: impl FnMut() -> String) -> Result<()> {
    use crate::state::atomic_write;
    let current = current_config(directory);
    let config = merged_config(archive, &current, fresh);
    atomic_write(&directory.join(CONFIG), config.text().as_bytes())?;
    let files = ProfileFiles::new(&config, directory);
    if let Some(bytes) = archive.files.get(APPS) {
        atomic_write(&files.apps, bytes)?;
    }
    if let Some(bytes) = archive.files.get(ALIASES) {
        // The target's console sign-in sessions stay as they are.
        let current = read_optional(&files.aliases)?;
        atomic_write(
            &files.aliases,
            &keeping_sessions(bytes, current.as_deref())?,
        )?;
    }
    for (name, bytes) in archive.under(COVERS).chain(archive.under(GAME_SETTINGS)) {
        atomic_write(&folder_target(directory, name), bytes)?;
    }
    if archive.manifest.secrets {
        if let Some(bytes) = archive.files.get(PAIRED) {
            atomic_write(&files.paired, bytes)?;
        }
        if let Some(bytes) = archive.files.get(CREDENTIALS_FILE) {
            let target = if files.credentials == files.paired {
                directory.join(CREDENTIALS_FILE)
            } else {
                files.credentials.clone()
            };
            atomic_write(&target, bytes)?;
        }
        for (name, bytes) in archive.under(CREDENTIALS_DIR) {
            atomic_write(&folder_target(directory, name), bytes)?;
        }
        for (name, path) in [(CERTIFICATE, &files.certificate), (KEY, &files.key)] {
            if let Some(bytes) = archive.files.get(name)
                && *path != folder_target(directory, name)
            {
                atomic_write(path, bytes)?;
            }
        }
    } else if let Some(backup) = archive.json(DEVICES)
        && let Some(bytes) = read_optional(&files.paired)?
    {
        let mut paired: Value =
            serde_json::from_slice(&bytes).context("the paired devices file is not JSON")?;
        merge_devices(&mut paired, &backup);
        crate::state::write_json(&files.paired, &paired)?;
    }
    Ok(())
}

/// Applies a restore the console staged, if there is one, before the host
/// loads anything: true when one was applied. A staged archive that fails is
/// moved aside as `restore-failed.zip`, and the host starts with what it had.
pub fn apply_staged(directory: &Path, fresh: impl FnMut() -> String) -> Result<bool> {
    let backups = directory.join(BACKUPS);
    let staged = backups.join(STAGED);
    if !staged.exists() {
        return Ok(false);
    }
    let result = std::fs::File::open(&staged)
        .context("the staged restore could not be opened")
        .and_then(|file| read(std::io::BufReader::new(file)))
        .and_then(|archive| apply(&archive, directory, fresh));
    match result {
        Ok(()) => {
            std::fs::remove_file(&staged)?;
            Ok(true)
        }
        Err(error) => {
            let _ = std::fs::rename(&staged, backups.join("restore-failed.zip"));
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests;
