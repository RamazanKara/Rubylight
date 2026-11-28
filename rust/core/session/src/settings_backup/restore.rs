//! Restore summary, config/device merging, and staged archive application for settings backups.
use super::{
    ALIASES, APPS, Archive, BACKUPS, CERTIFICATE, CONFIG, COVERS, CREDENTIALS_DIR,
    CREDENTIALS_FILE, Change, DEVICES, GAME_SETTINGS, KEY, LEGACY_WAKE_RELAY_FILE, LOCATION_KEYS,
    Manifest, PAIRED, STAGED, Summary, WAKE_RELAY_FILE, device_identity_key, keeping_sessions,
    named_devices, read, read_optional, secret_key,
};
use crate::state::ProfileFiles;
use anyhow::{Context, Result, bail};
use butterpollo_config::config::Config;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

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

/// All known webhook `url -> secret` pairs from `credentials/webhook_secrets.json` and any legacy
/// inline secrets still inside `config`.
fn existing_webhook_secrets(config: &Config, directory: &Path) -> BTreeMap<String, String> {
    let mut secrets =
        crate::webhook::load_secrets(&crate::webhook::secrets_path(directory)).unwrap_or_default();
    if let Some(raw) = config.values.get("webhooks")
        && let Some((_, inline)) = crate::webhook::split_json_secrets(raw)
    {
        for (url, secret) in inline {
            secrets.entry(url).or_insert(secret);
        }
    }
    secrets
}

fn archived_webhook_secrets(archive: &Archive) -> BTreeMap<String, String> {
    let mut secrets = archive
        .json(crate::webhook::SECRETS_FILE)
        .and_then(|v| v.get("secrets")?.as_object().cloned())
        .map(|map| {
            map.into_iter()
                .filter_map(|(k, v)| Some((k, v.as_str()?.to_owned())))
                .filter(|(_, s)| !s.is_empty())
                .collect::<BTreeMap<String, String>>()
        })
        .unwrap_or_default();
    if let Some(raw) = archive.config().values.get("webhooks")
        && let Some((_, inline)) = crate::webhook::split_json_secrets(raw)
    {
        for (url, secret) in inline {
            secrets.entry(url).or_insert(secret);
        }
    }
    secrets
}

/// What restoring `archive` changes on the host whose data folder is
/// `directory`.
pub fn summarize(archive: &Archive, directory: &Path) -> Summary {
    summarize_parts(
        &archive.manifest,
        &archive.files,
        archive.under(COVERS).count(),
        archive.under(GAME_SETTINGS).count(),
        directory,
    )
}

pub(super) fn summarize_parts(
    manifest: &Manifest,
    files_map: &BTreeMap<String, Vec<u8>>,
    covers: usize,
    kept: usize,
    directory: &Path,
) -> Summary {
    let secrets = manifest.secrets;
    let current = current_config(directory);
    let archive_config = files_map
        .get(CONFIG)
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
        .and_then(|text| Config::parse(text).ok())
        .unwrap_or_default();
    let restored = merged_config_from(archive_config, secrets, &current);
    let json_file =
        |name: &str| -> Option<Value> { serde_json::from_slice(files_map.get(name)?).ok() };
    let mut current_clean = current.clone();
    if let Some(hooks) = current_clean.values.get_mut("webhooks")
        && let Some((stripped, _)) = crate::webhook::split_json_secrets(hooks)
    {
        *hooks = stripped;
    }
    let mut changes = Vec::new();
    let keys: BTreeSet<&String> = restored
        .values
        .keys()
        .chain(current_clean.values.keys())
        .collect();
    let changed: Vec<&str> = keys
        .into_iter()
        .filter(|key| secrets || !secret_key(key))
        .filter(|key| restored.values.get(*key) != current_clean.values.get(*key))
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
    if files_map.contains_key(APPS) {
        let before = app_names(
            read_optional(&files.apps)
                .ok()
                .flatten()
                .and_then(|b| serde_json::from_slice(&b).ok())
                .as_ref(),
        );
        let after = app_names(json_file(APPS).as_ref());
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
    if covers > 0 {
        changes.push(Change {
            item: "covers",
            detail: format!("{covers} covers written; covers only on this PC stay."),
        });
    }
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
        let devices = json_file(PAIRED)
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
    } else if let Some(devices) = json_file(DEVICES) {
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
        let existing = existing_webhook_secrets(&current, directory);
        let fresh = if secrets {
            0
        } else {
            hooks
                .iter()
                .filter(|url| existing.get(*url).is_none_or(String::is_empty))
                .count()
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
        created_unix: manifest.created_unix,
        host_version: manifest.host_version.clone(),
        secrets,
        changes,
    }
}

/// The restored configuration: the archive's, preserving this PC's `LOCATION_KEYS`, keeping its
/// secret keys when `!archive.manifest.secrets`, and always keeping webhook inline secrets out of
/// `sunshine.conf`.
fn merged_config(archive: &Archive, current: &Config) -> Config {
    merged_config_from(archive.config(), archive.manifest.secrets, current)
}

fn merged_config_from(mut config: Config, secrets: bool, current: &Config) -> Config {
    // Where this PC keeps its files stays this PC's: a backup from another
    // PC, or a crafted one, never moves them or makes the restore write
    // elsewhere.
    for key in LOCATION_KEYS {
        match current.values.get(key) {
            Some(value) => config.values.insert(key.into(), value.clone()),
            None => config.values.remove(key),
        };
    }
    if !secrets {
        for (key, value) in &current.values {
            if secret_key(key) {
                config.values.insert(key.clone(), value.clone());
            }
        }
    }
    if let Some(hooks) = config.values.get_mut("webhooks")
        && let Some((stripped, _)) = crate::webhook::split_json_secrets(hooks)
    {
        *hooks = stripped;
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

/// Records the original file bytes before each mutation so a mid-restore error rolls back cleanly.
pub(super) struct TxWriter {
    originals: BTreeMap<PathBuf, Option<Vec<u8>>>,
    latest: BTreeMap<PathBuf, Option<Vec<u8>>>,
}

impl TxWriter {
    pub(super) fn new() -> Self {
        Self {
            originals: BTreeMap::new(),
            latest: BTreeMap::new(),
        }
    }

    fn snapshot(&mut self, path: &Path) -> Result<()> {
        if !self.originals.contains_key(path) {
            let original = read_optional(path)?;
            self.latest.insert(path.to_path_buf(), original.clone());
            self.originals.insert(path.to_path_buf(), original);
        }
        Ok(())
    }

    pub(super) fn write(&mut self, path: &Path, bytes: &[u8]) -> Result<()> {
        self.snapshot(path)?;
        crate::state::atomic_write(path, bytes)?;
        self.latest.insert(path.to_path_buf(), Some(bytes.to_vec()));
        Ok(())
    }

    fn remove(&mut self, path: &Path) -> Result<()> {
        self.snapshot(path)?;
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| format!("removing {}", path.display()));
            }
        }
        self.latest.insert(path.to_path_buf(), None);
        Ok(())
    }

    fn read(&self, path: &Path) -> Result<Option<Vec<u8>>> {
        if let Some(cached) = self.latest.get(path) {
            return Ok(cached.clone());
        }
        read_optional(path)
    }

    pub(super) fn rollback(self) -> Result<()> {
        let mut errors = Vec::new();
        for (path, original) in self.originals.into_iter().rev() {
            match original {
                Some(bytes) => {
                    if let Err(error) = crate::state::atomic_write(&path, &bytes) {
                        errors.push(format!("rolling back {}: {error:#}", path.display()));
                    }
                }
                None => match std::fs::remove_file(&path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => {
                        errors.push(format!("rolling back {}: {error}", path.display()));
                    }
                },
            }
        }
        if !errors.is_empty() {
            bail!("{}", errors.join("; "));
        }
        Ok(())
    }
}

/// Writes `archive` over the settings of the host whose data folder is
/// `directory`. Files that only this PC has (covers, game settings files)
/// stay. Any write failure rolls back all changes made so far.
pub fn apply(archive: &Archive, directory: &Path, fresh: impl FnMut() -> String) -> Result<()> {
    let mut tx = TxWriter::new();
    match apply_inner(&mut tx, archive, directory, fresh) {
        Ok(()) => Ok(()),
        Err(error) => match tx.rollback() {
            Ok(()) => Err(error),
            Err(rollback_error) => Err(error.context(format!(
                "restoring files after error failed: {rollback_error:#}"
            ))),
        },
    }
}

fn apply_inner(
    tx: &mut TxWriter,
    archive: &Archive,
    directory: &Path,
    mut fresh: impl FnMut() -> String,
) -> Result<()> {
    let current = current_config(directory);
    let config = merged_config(archive, &current);
    tx.write(&directory.join(CONFIG), config.text().as_bytes())?;
    let files = ProfileFiles::new(&config, directory);
    if let Some(bytes) = archive.files.get(APPS) {
        tx.write(&files.apps, bytes)?;
    }
    if let Some(bytes) = archive.files.get(ALIASES) {
        // The target's console sign-in sessions (and API tokens when restoring without secrets)
        // stay as they are.
        let current = tx.read(&files.aliases)?;
        tx.write(
            &files.aliases,
            &keeping_sessions(bytes, current.as_deref(), archive.manifest.secrets)?,
        )?;
    }
    for (name, bytes) in archive.under(COVERS).chain(archive.under(GAME_SETTINGS)) {
        tx.write(&folder_target(directory, name), bytes)?;
    }
    let hooks_secrets_file = crate::webhook::secrets_path(directory);
    if archive.manifest.secrets {
        if let Some(bytes) = archive.files.get(PAIRED) {
            tx.write(&files.paired, bytes)?;
        }
        if let Some(bytes) = archive.files.get(CREDENTIALS_FILE) {
            if files.credentials == files.paired {
                // `credentials_file` matches the paired-state file (or no separate credentials
                // file exists): merge sign-in fields into `files.paired` so they are read.
                let creds: Value = serde_json::from_slice(bytes)
                    .context("the archive's sunshine_credentials.json is not JSON")?;
                let mut doc = match tx.read(&files.paired)? {
                    Some(existing) => serde_json::from_slice::<Value>(&existing)
                        .context("the paired devices file is not JSON")?,
                    None => Value::Object(Default::default()),
                };
                if let (Some(dst), Some(src)) = (doc.as_object_mut(), creds.as_object()) {
                    for key in ["username", "password", "salt"] {
                        if let Some(value) = src.get(key) {
                            dst.insert(key.into(), value.clone());
                        } else {
                            dst.remove(key);
                        }
                    }
                }
                tx.write(&files.paired, &serde_json::to_vec_pretty(&doc)?)?;
            } else {
                tx.write(&files.credentials, bytes)?;
            }
        } else if archive.files.contains_key(PAIRED) && files.credentials != files.paired {
            // The backup keeps console sign-in credentials inside sunshine_state.json;
            // a stale separate credentials file on this PC would otherwise shadow them
            // when ProfileFiles::new loads after restart.
            if config.values.contains_key("credentials_file") {
                let mut creds = serde_json::Map::new();
                if let Some(paired) = archive.json(PAIRED) {
                    for key in ["username", "password", "salt"] {
                        if let Some(value) = paired.get(key) {
                            creds.insert(key.into(), value.clone());
                        }
                    }
                }
                tx.write(&files.credentials, &serde_json::to_vec_pretty(&creds)?)?;
            } else {
                tx.remove(&files.credentials)?;
            }
        }
        for (name, bytes) in archive.under(CREDENTIALS_DIR) {
            tx.write(&folder_target(directory, name), bytes)?;
        }
        // Restore a legacy root `wake-relay.json` entry into `credentials/wake-relay.json`
        // and clear any stale root file so non-administrators cannot read relay secrets.
        if !archive.files.contains_key(WAKE_RELAY_FILE)
            && let Some(bytes) = archive.files.get(LEGACY_WAKE_RELAY_FILE)
        {
            tx.write(&folder_target(directory, WAKE_RELAY_FILE), bytes)?;
        }
        if archive.files.contains_key(WAKE_RELAY_FILE)
            || archive.files.contains_key(LEGACY_WAKE_RELAY_FILE)
        {
            tx.remove(&directory.join(LEGACY_WAKE_RELAY_FILE))?;
        }
        for (name, path) in [(CERTIFICATE, &files.certificate), (KEY, &files.key)] {
            if let Some(bytes) = archive.files.get(name)
                && *path != folder_target(directory, name)
            {
                tx.write(path, bytes)?;
            }
        }
        let archived = archived_webhook_secrets(archive);
        if !archived.is_empty() {
            tx.write(
                &hooks_secrets_file,
                &crate::webhook::encode_secrets(&archived)?,
            )?;
        }
    } else {
        if let Some(backup) = archive.json(DEVICES)
            && let Some(bytes) = tx.read(&files.paired)?
        {
            let mut paired: Value =
                serde_json::from_slice(&bytes).context("the paired devices file is not JSON")?;
            merge_devices(&mut paired, &backup);
            tx.write(&files.paired, &serde_json::to_vec_pretty(&paired)?)?;
        }
        let existing = existing_webhook_secrets(&current, directory);
        let urls = webhook_urls(&config);
        if !urls.is_empty() {
            let mut next = BTreeMap::new();
            for url in urls {
                let secret = existing
                    .get(&url)
                    .filter(|s| !s.is_empty())
                    .cloned()
                    .unwrap_or_else(&mut fresh);
                if !secret.is_empty() {
                    next.insert(url, secret);
                }
            }
            if !next.is_empty() {
                tx.write(&hooks_secrets_file, &crate::webhook::encode_secrets(&next)?)?;
            }
        }
    }
    Ok(())
}

/// Applies a restore the console staged, if there is one, before the host
/// loads anything: true when one was applied. A staged archive that fails is
/// moved aside as `restore-failed.zip`, and the host starts with what it had.
pub fn apply_staged(directory: &Path, fresh: impl FnMut() -> String) -> Result<bool> {
    let backups = directory.join(BACKUPS);
    let staged = backups.join(STAGED);
    let applying = backups.join("restore-applying.zip");
    let _ = std::fs::remove_file(&applying);
    match std::fs::rename(&staged, &applying) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(error).context("the staged restore could not be claimed");
        }
    }
    let result = std::fs::File::open(&applying)
        .context("the staged restore could not be opened")
        .and_then(|file| read(std::io::BufReader::new(file)))
        .and_then(|archive| apply(&archive, directory, fresh));
    match result {
        Ok(()) => {
            if let Err(error) = std::fs::remove_file(&applying) {
                tracing::warn!(
                    error = %error,
                    "applied staged restore could not be deleted cleanly; renaming aside"
                );
                let _ = std::fs::rename(&applying, backups.join("restore-done.zip"));
            }
            Ok(true)
        }
        Err(error) => {
            let _ = std::fs::rename(&applying, backups.join("restore-failed.zip"));
            Err(error)
        }
    }
}
