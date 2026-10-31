//! The host's durable state: paired clients, credentials, apps and their
//! preparation commands, written atomically as JSON in the profile.
use crate::{config::Config, crypto};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashSet},
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
};

/// The JSON in a state file, as Vibepollo reads it: a UTF-8 byte order mark
/// is dropped, and a blank file holds nothing (None).
pub fn json_bytes(bytes: &[u8]) -> Option<&[u8]> {
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    bytes
        .iter()
        .any(|b| !b" \t\r\n\x0b\x0c".contains(b))
        .then_some(bytes)
}
/// Never turn an unreadable existing identity file into a fresh empty host.
/// A blank file has nothing to lose and reads as a missing one.
pub fn load_json(path: &Path, default: Value) -> Result<Value> {
    match std::fs::read(path) {
        Ok(b) => match json_bytes(&b) {
            Some(json) => serde_json::from_slice(json)
                .with_context(|| format!("invalid JSON in {}", path.display())),
            None => Ok(default),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(default),
        Err(e) => Err(e.into()),
    }
}
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("state path has no parent")?;
    std::fs::create_dir_all(parent)?;
    let tmp = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name().unwrap().to_string_lossy(),
        uuid::Uuid::new_v4()
    ));
    let result = (|| -> Result<()> {
        let mut f = OpenOptions::new().create_new(true).write(true).open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        #[cfg(windows)]
        crate::update_files::publish(&tmp, path)?;
        #[cfg(not(windows))]
        {
            std::fs::rename(&tmp, path)?;
            std::fs::File::open(parent)?.sync_all()?;
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}
pub fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    atomic_write(path, &serde_json::to_vec_pretty(value)?)
}
fn default_permission() -> u32 {
    FIRST_DEVICE_PERMISSION
}
/// What the first device paired to a host may do: everything but file
/// transfer, which the administrator grants a device on its own.
pub const FIRST_DEVICE_PERMISSION: u32 = 0x07131f00;
/// The paired state's migrations, counted in `root.rubylight_state_version`.
/// 1: Upload files and Download files, which test builds gave the first
/// device with everything else, are taken back once.
const STATE_VERSION: u64 = 1;
/// What every later device may do until the administrator grants more: view only.
pub const LATER_DEVICE_PERMISSION: u32 = 0x03000000;
/// Permission bits, as in the console's permission table.
pub const LIST_APPS: u32 = 0x0100_0000;
pub const VIEW_STREAMS: u32 = 0x0200_0000;
pub const LAUNCH_APPS: u32 = 0x0400_0000;
/// Controllers, touch, pen, mouse and keyboard.
pub const ALL_INPUT: u32 = 0x0000_1f00;
/// "Play games": list, watch and launch apps, with every kind of input.
pub const PLAY_DEVICE_PERMISSION: u32 = LIST_APPS | VIEW_STREAMS | LAUNCH_APPS | ALL_INPUT;
/// The permissions the console's pairing access choice (`access`) grants a
/// later device: `play`, `view` or `all`. Empty means no choice, so the device
/// gets [`PairedState::new_device_permission`].
pub fn pairing_access(access: &str) -> Result<Option<u32>> {
    Ok(match access {
        "" => None,
        "play" => Some(PLAY_DEVICE_PERMISSION),
        "view" => Some(LATER_DEVICE_PERMISSION),
        "all" => Some(FIRST_DEVICE_PERMISSION),
        _ => bail!("access must be play, view or all"),
    })
}
fn enabled() -> bool {
    true
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Client {
    pub name: String,
    pub cert: String,
    pub uuid: String,
    #[serde(default = "default_permission", deserialize_with = "permission_number")]
    pub perm: u32,
    #[serde(default = "enabled", deserialize_with = "compatible_bool")]
    pub enabled: bool,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}
fn permission_number<'de, D: serde::Deserializer<'de>>(d: D) -> std::result::Result<u32, D::Error> {
    let v = Value::deserialize(d)?;
    match v {
        Value::String(s) => s.parse().map_err(serde::de::Error::custom),
        Value::Number(n) => n
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .ok_or_else(|| serde::de::Error::custom("invalid permissions")),
        _ => Err(serde::de::Error::custom("invalid permissions")),
    }
}
fn compatible_bool<'de, D: serde::Deserializer<'de>>(d: D) -> std::result::Result<bool, D::Error> {
    match Value::deserialize(d)? {
        Value::Bool(b) => Ok(b),
        Value::String(s) if matches!(s.as_str(), "true" | "1") => Ok(true),
        Value::String(s) if matches!(s.as_str(), "false" | "0") => Ok(false),
        Value::Number(n) if n.as_u64() == Some(0) => Ok(false),
        Value::Number(n) if n.as_u64() == Some(1) => Ok(true),
        _ => Err(serde::de::Error::custom("invalid boolean")),
    }
}
impl Client {
    pub fn allows(&self, permission: u32) -> bool {
        self.enabled && self.perm & permission == permission
    }
    /// Whether this device may run its connect and disconnect commands
    /// (on unless the administrator turned it off, as in Vibepollo).
    pub fn allows_commands(&self) -> bool {
        !self
            .extra
            .get("allow_client_commands")
            .is_some_and(|v| v == false || v == "false")
    }
    /// Whether the administrator granted this device Host control (change
    /// Radeon settings, close the game). It is a separate key in `extra`, not
    /// a `perm` bit, so an all-ones permission value can't switch it on, and
    /// only `true` and `"true"` count. Pairing and migration never set it.
    pub fn allows_host_control(&self) -> bool {
        self.enabled
            && self
                .extra
                .get("host_control")
                .is_some_and(|v| v == true || v == "true")
    }
    /// Whether the administrator let this device put the host to sleep
    /// (`power_action`). Only a JSON `true` counts, as in Vibepollo, so an
    /// imported Vibepollo opt-in carries over and a permission value never
    /// grants it.
    pub fn allows_sleep(&self) -> bool {
        self.enabled && self.extra.get("allow_sleep") == Some(&Value::Bool(true))
    }
    /// Whether the administrator let this device attach USB devices to the
    /// host (`allow_usb`). Only a JSON `true` counts: no permission bit or Host
    /// control implies it.
    pub fn allows_usb(&self) -> bool {
        self.enabled && self.extra.get("allow_usb") == Some(&Value::Bool(true))
    }
    /// Whether this device may join other devices' co-op sessions (`coop_join`,
    /// `nucleus::coop::may_join`): it needs Launch apps, as joining launches the
    /// co-op entry; never set, it follows them, set, only true counts. A
    /// disabled device may not.
    pub fn allows_coop_join(&self) -> bool {
        self.enabled && crate::nucleus::coop::may_join(&self.extra, self.perm & LAUNCH_APPS != 0)
    }
    /// This device's grant for shared folder `id`, from
    /// `shared_folders: {"<id>": "read" | "write"}`. Any other value, or an id
    /// the console would refuse (a hand-edited `"../x"`), grants nothing, and a
    /// disabled device has no grants.
    pub fn folder_access(&self, id: &str) -> Option<rubylight_native_api::folders::FolderAccess> {
        use rubylight_native_api::folders::FolderAccess;
        if !self.enabled || !crate::config::valid_folder_id(id) {
            return None;
        }
        match self.extra.get("shared_folders")?.get(id)?.as_str()? {
            "read" => Some(FolderAccess::Read),
            "write" => Some(FolderAccess::Write),
            _ => None,
        }
    }
    /// Whether this device has at least one shared folder grant.
    pub fn has_folders(&self) -> bool {
        self.enabled
            && self
                .extra
                .get("shared_folders")
                .and_then(Value::as_object)
                .is_some_and(|grants| grants.keys().any(|id| self.folder_access(id).is_some()))
    }
    pub fn der(&self) -> Result<Vec<u8>> {
        crypto::certificate_der(&self.cert)
    }
    /// The WebRTC credentials of a device paired over the WebRTC signalling
    /// channel: `(client_id, secret)` from `extra.webrtc`, only if both have
    /// the shape the host issues. Such a device has no TLS certificate.
    pub fn webrtc(&self) -> Option<(&str, &str)> {
        use crate::webrtc::auth::{CLIENT_ID_BYTES, SECRET_BYTES, is_lower_hex};
        let webrtc = self.extra.get("webrtc")?;
        let id = webrtc.get("clientId")?.as_str()?;
        let secret = webrtc.get("secret")?.as_str()?;
        (is_lower_hex(id, CLIENT_ID_BYTES) && is_lower_hex(secret, SECRET_BYTES))
            .then_some((id, secret))
    }
    /// A device the administrator approved over WebRTC signalling, with a new
    /// UUID. [`PairedState::add_webrtc`] sets its permissions.
    pub fn paired_over_webrtc(name: String, client_id: &str, secret: &str) -> Self {
        Self {
            name,
            cert: String::new(),
            uuid: uuid::Uuid::new_v4().to_string(),
            perm: default_permission(),
            enabled: true,
            extra: BTreeMap::from([(
                "webrtc".to_owned(),
                json!({"clientId": client_id, "secret": secret}),
            )]),
        }
    }
    /// A WebRTC device: no certificate, valid WebRTC credentials. It can
    /// never be matched to a TLS peer.
    pub fn is_webrtc(&self) -> bool {
        self.cert.trim().is_empty() && self.webrtc().is_some()
    }
    /// The WebRTC device the console user `username` streams as from the console's own stream
    /// page, with a new UUID. [`PairedState::add_webrtc`] gives it every permission: a console
    /// sign-in is the administrator.
    pub fn console_over_webrtc(
        name: String,
        username: &str,
        client_id: &str,
        secret: &str,
    ) -> Self {
        let mut client = Self::paired_over_webrtc(name, client_id, secret);
        client.extra.get_mut("webrtc").unwrap()[CONSOLE] = username.into();
        client
    }
    /// The console user this WebRTC device stands for, if it is a console's own device. Only the
    /// host writes `extra.webrtc`.
    pub fn console(&self) -> Option<&str> {
        if !self.is_webrtc() {
            return None;
        }
        self.extra.get("webrtc")?.get(CONSOLE)?.as_str()
    }
}
/// The key in `extra.webrtc` that names the console user a device stands for.
const CONSOLE: &str = "console";
/// Sunshine's and Apollo's property-tree writer stored an empty list or
/// object as "". Only the known containers are read back as empty, as in
/// Vibepollo; every other value is kept as it is.
fn empty_containers(root: &mut Value) {
    let list = |value: &mut Value| {
        if *value == "" {
            *value = json!([]);
        }
    };
    if let Some(devices) = root.get_mut("named_devices") {
        list(devices);
        for device in devices.as_array_mut().into_iter().flatten() {
            for key in ["do", "undo"] {
                if let Some(commands) = device.get_mut(key) {
                    list(commands);
                }
            }
            if let Some(overrides) = device.get_mut("config_overrides")
                && *overrides == ""
            {
                *overrides = json!({});
            }
        }
    }
    if let Some(devices) = root.get_mut("devices") {
        list(devices);
        for device in devices.as_array_mut().into_iter().flatten() {
            if let Some(certs) = device.get_mut("certs") {
                list(certs);
            }
        }
    }
}
#[derive(Clone)]
pub struct PairedState {
    pub document: Value,
    pub clients: Vec<Client>,
    pub unique_id: String,
    skipped_clients: Vec<Value>,
}
impl PairedState {
    pub fn load(path: &Path) -> Result<Self> {
        let mut document = load_json(
            path,
            json!({"root":{"uniqueid":uuid::Uuid::new_v4().to_string(),"named_devices":[]}}),
        )?;
        if !document.is_object() {
            bail!("paired state must be an object");
        }
        if document.get("root").is_none() {
            document["root"] = json!({});
        }
        let root = &mut document["root"];
        if !root.is_object() {
            bail!("paired state root must be an object");
        }
        empty_containers(root);
        let unique_id = match root.get("uniqueid") {
            Some(Value::String(s)) if !s.is_empty() => s.clone(),
            Some(_) => bail!("invalid persisted host identity"),
            None => {
                let s = uuid::Uuid::new_v4().to_string();
                root["uniqueid"] = Value::String(s.clone());
                s
            }
        };
        let mut skipped = Vec::new();
        let mut clients: Vec<Client> = match root.get("named_devices") {
            None => vec![],
            Some(v) => {
                let records = v.as_array().context("invalid paired client records")?;
                records.iter().filter_map(|record| {
                    match serde_json::from_value(record.clone()) {
                        Ok(client) => Some(client),
                        Err(error) => {
                            tracing::warn!(%error, "invalid paired device record; preserved but skipped");
                            skipped.push(record.clone());
                            None
                        }
                    }
                }).collect()
            }
        };
        // Old Sunshine stored a list of certificates per anonymous device.
        // Consume it once; otherwise a subsequent unpair would resurrect it.
        if let Some(devices) = root.get("devices").and_then(Value::as_array) {
            for device in devices {
                if let Some(certs) = device.get("certs").and_then(Value::as_array) {
                    for cert in certs {
                        let cert = cert.as_str().context("invalid legacy certificate")?;
                        if !clients.iter().any(|c| c.cert == cert) {
                            clients.push(Client {
                                name: "Imported client".into(),
                                cert: cert.into(),
                                uuid: uuid::Uuid::new_v4().to_string(),
                                perm: default_permission(),
                                enabled: true,
                                extra: BTreeMap::new(),
                            });
                        }
                    }
                }
            }
            root.as_object_mut().unwrap().remove("devices");
        }
        // Vibepollo appended a device again when it paired again, so an
        // imported profile can list one certificate twice. Keep the first
        // entry rather than refusing to start.
        let mut certs = HashSet::new();
        let mut ids = HashSet::new();
        let mut unique = Vec::with_capacity(clients.len());
        let mut webrtc_ids = HashSet::new();
        for mut c in clients {
            // A WebRTC device has no certificate; it is known by its client ID.
            if c.is_webrtc() {
                let id = c.webrtc().map(|(id, _)| id.to_string()).unwrap_or_default();
                if !webrtc_ids.insert(id) {
                    // Two records with one client ID would be two owners of
                    // one secret; keep the first, and the second in the file.
                    tracing::warn!(
                        device = %c.name,
                        "WebRTC device repeats an earlier device's client ID; skipped"
                    );
                    skipped.push(serde_json::to_value(&c)?);
                    continue;
                }
                if !ids.insert(c.uuid.clone()) {
                    c.uuid = uuid::Uuid::new_v4().to_string();
                    ids.insert(c.uuid.clone());
                }
                if let Some(overrides) = c
                    .extra
                    .get_mut("config_overrides")
                    .and_then(Value::as_object_mut)
                {
                    crate::config::replace_retired(overrides.iter_mut());
                }
                unique.push(c);
                continue;
            }
            // Such a device could never connect; it must not keep every
            // other device from loading.
            let der = match c.der() {
                Ok(der) => der,
                Err(error) => {
                    tracing::warn!(
                        device = %c.name,
                        error = %format!("{error:#}"),
                        "paired device has an unreadable certificate; skipped"
                    );
                    skipped.push(serde_json::to_value(&c)?);
                    continue;
                }
            };
            if !certs.insert(der) {
                continue;
            }
            if !ids.insert(c.uuid.clone()) {
                c.uuid = uuid::Uuid::new_v4().to_string();
                ids.insert(c.uuid.clone());
            }
            // A device's own settings, such as a ViGEmBus controller type,
            // take their current values; the host saves the state it loads.
            if let Some(overrides) = c
                .extra
                .get_mut("config_overrides")
                .and_then(Value::as_object_mut)
            {
                crate::config::replace_retired(overrides.iter_mut());
            }
            unique.push(c);
        }
        let mut clients = unique;
        if clients.len() > 256 {
            bail!("too many paired clients");
        }
        let version = root
            .get("rubylight_state_version")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        if version < 1 {
            for c in &mut clients {
                c.perm &= !(crate::native::FILE_UPLOAD | crate::native::FILE_DOWNLOAD);
            }
        }
        if version < STATE_VERSION {
            root["rubylight_state_version"] = json!(STATE_VERSION);
        }
        Ok(Self {
            document,
            clients,
            unique_id,
            skipped_clients: skipped,
        })
    }
    pub fn client_by_certificate(&self, der: &[u8]) -> Option<&Client> {
        let mut found = None;
        for c in &self.clients {
            // An empty certificate (a WebRTC device) never matches a peer.
            if c.cert.trim().is_empty() {
                continue;
            }
            if c.enabled && c.perm != 0 && c.der().is_ok_and(|b| crypto::equal(&b, der)) {
                if found.is_some() {
                    return None;
                }
                found = Some(c);
            }
        }
        found
    }
    pub fn save(&self, path: &Path) -> Result<()> {
        let mut d = self.document.clone();
        // The credentials CLI can replace these while the host is running.
        let latest = load_json(path, json!({}))?;
        for key in ["username", "password", "salt"] {
            if let Some(value) = latest.get(key) {
                d[key] = value.clone();
            } else {
                d.as_object_mut().unwrap().remove(key);
            }
        }
        d["root"]["uniqueid"] = json!(self.unique_id);
        let mut devices = self.skipped_clients.clone();
        devices.extend(
            self.clients
                .iter()
                .map(serde_json::to_value)
                .collect::<Result<Vec<_>, _>>()?,
        );
        d["root"]["named_devices"] = Value::Array(devices);
        write_json(path, &d)
    }
    /// Pair a device. A certificate that is already paired, as when Moonlight
    /// forgot this host and pairs again, keeps its entry, identity and
    /// permissions; only the name is updated. Refusing it left the device
    /// unable to pair until someone deleted it in the console.
    pub fn add(&mut self, path: &Path, client: Client) -> Result<()> {
        let der = client.der()?;
        let mut next = self.clone();
        if let Some(existing) = next
            .clients
            .iter_mut()
            .find(|c| c.der().is_ok_and(|b| crypto::equal(&b, &der)))
        {
            existing.name = client.name;
        } else {
            if next.clients.len() >= 256 || next.clients.iter().any(|c| c.uuid == client.uuid) {
                bail!("duplicate client or paired client limit reached");
            }
            next.clients.push(client);
        }
        next.save(path)?;
        *self = next;
        Ok(())
    }
    /// The WebRTC device with this client ID.
    pub fn webrtc_client(&self, client_id: &str) -> Option<&Client> {
        self.clients
            .iter()
            .find(|c| c.is_webrtc() && c.webrtc().is_some_and(|(id, _)| id == client_id))
    }
    /// The permissions of a device paired now, for both protocols: everything
    /// when no device of any kind is paired, view only after that. The console's
    /// own devices don't count: the first phone or TV after them is still the first.
    pub fn new_device_permission(&self) -> u32 {
        if self.clients.iter().all(|c| c.console().is_some()) {
            FIRST_DEVICE_PERMISSION
        } else {
            LATER_DEVICE_PERMISSION
        }
    }
    /// The permissions of a device paired now with the access the owner chose
    /// ([`pairing_access`]): the first device still gets everything, a later one
    /// the choice, or view only without one.
    pub fn paired_device_permission(&self, chosen: Option<u32>) -> u32 {
        match self.new_device_permission() {
            FIRST_DEVICE_PERMISSION => FIRST_DEVICE_PERMISSION,
            later => chosen.unwrap_or(later),
        }
    }
    /// The grants a device paired with `perm` gets besides its permission
    /// bits: one that may launch apps may also join co-op sessions
    /// (`coop_join`), which the owner can take away on the device's page.
    pub fn paired_device_extra(perm: u32) -> BTreeMap<String, Value> {
        let mut extra = BTreeMap::new();
        if perm & LAUNCH_APPS != 0 {
            extra.insert(crate::nucleus::coop::DEVICE_KEY.into(), Value::Bool(true));
        }
        extra
    }
    /// Pair a WebRTC device. `client` must be a WebRTC device ([`Client::is_webrtc`]);
    /// its `perm` is replaced by [`Self::new_device_permission`], or everything for
    /// a console's own device ([`Client::console_over_webrtc`]).
    /// The oldest WebRTC devices beyond `keep` are unpaired in the same save;
    /// their UUIDs are returned. The console's own devices neither count towards
    /// `keep` nor go: opening the console's stream page never unpairs a TV.
    pub fn add_webrtc(&mut self, path: &Path, client: Client, keep: usize) -> Result<Vec<String>> {
        let Some((client_id, _)) = client.webrtc() else {
            bail!("not a WebRTC device");
        };
        if !client.cert.trim().is_empty() {
            bail!("a WebRTC device has no certificate");
        }
        let mut next = self.clone();
        if next
            .clients
            .iter()
            .any(|c| c.uuid == client.uuid || c.webrtc().is_some_and(|(id, _)| id == client_id))
        {
            bail!("duplicate client");
        }
        let mut client = client;
        client.perm = match client.console() {
            Some(_) => FIRST_DEVICE_PERMISSION,
            None => next.new_device_permission(),
        };
        next.clients.push(client);
        // Clients are stored oldest first, so the surplus is at the front.
        let webrtc: Vec<String> = next
            .clients
            .iter()
            .filter(|c| c.is_webrtc() && c.console().is_none())
            .map(|c| c.uuid.clone())
            .collect();
        let surplus: Vec<String> = webrtc[..webrtc.len().saturating_sub(keep)].to_vec();
        next.clients.retain(|c| !surplus.contains(&c.uuid));
        if next.clients.len() > 256 {
            bail!("paired client limit reached");
        }
        next.save(path)?;
        *self = next;
        Ok(surplus)
    }
    pub fn remove(&mut self, path: &Path, id: &str) -> Result<()> {
        let mut next = self.clone();
        next.clients.retain(|c| c.uuid != id);
        next.skipped_clients
            .retain(|record| record["uuid"].as_str() != Some(id));
        next.save(path)?;
        *self = next;
        Ok(())
    }
    pub fn clear(&mut self) {
        self.clients.clear();
        self.skipped_clients.clear();
    }
}
/// Where a profile keeps its files, as its sunshine.conf names them.
pub struct ProfileFiles {
    pub paired: PathBuf,
    pub credentials: PathBuf,
    pub apps: PathBuf,
    pub aliases: PathBuf,
    pub certificate: PathBuf,
    pub key: PathBuf,
}
impl ProfileFiles {
    pub fn new(config: &Config, directory: &Path) -> Self {
        let credentials = if directory.join("sunshine_credentials.json").exists() {
            "sunshine_credentials.json"
        } else {
            "sunshine_state.json"
        };
        Self {
            paired: config.path("file_state", directory, "sunshine_state.json"),
            credentials: config.path("credentials_file", directory, credentials),
            apps: config.path("file_apps", directory, "apps.json"),
            aliases: config.path("vibeshine_file_state", directory, "vibeshine_state.json"),
            certificate: config.path("cert", directory, "credentials/cacert.pem"),
            key: config.path("pkey", directory, "credentials/cakey.pem"),
        }
    }
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Credentials {
    pub username: String,
    pub password: String,
    pub salt: String,
}
impl Credentials {
    /// The web console's credentials in `path`; None until they are set.
    pub fn load(path: &Path) -> Result<Option<Self>> {
        let document = load_json(path, json!({}))?;
        if document.get("username").is_none() {
            return Ok(None);
        }
        Ok(Some(
            serde_json::from_value(document).context("invalid web credentials")?,
        ))
    }
    pub fn new(username: String, password: &str) -> Result<Self> {
        if username.is_empty() || username.len() > 128 || password.len() < 8 {
            bail!("username required; password must have at least 8 characters");
        }
        let salt = hex::encode(crypto::random::<8>());
        Ok(Self {
            username,
            password: crypto::legacy_hash(format!("{password}{salt}").as_bytes()),
            salt,
        })
    }
    pub fn verifies(&self, username: &str, password: &str) -> bool {
        self.username.eq_ignore_ascii_case(username)
            && crypto::matches_hash(
                format!("{password}{}", self.salt).as_bytes(),
                &self.password,
            )
    }
}
#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct App {
    pub name: String,
    #[serde(default)]
    pub cmd: String,
    #[serde(default, rename = "working-dir")]
    pub working_dir: String,
    #[serde(default, rename = "prep-cmd")]
    pub prep: Vec<PrepCommand>,
    #[serde(skip)]
    pub computed_id: Option<u32>,
    #[serde(skip)]
    pub aliases: Vec<u32>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}
#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct PrepCommand {
    #[serde(default)]
    pub r#do: String,
    #[serde(default)]
    pub undo: String,
    #[serde(default, deserialize_with = "compatible_bool")]
    pub elevated: bool,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}
impl App {
    pub fn id(&self) -> u32 {
        if let Some(id) = self.computed_id {
            return id;
        }
        if let Some(n) = self.extra.get("id").and_then(Value::as_u64) {
            return n as u32;
        }
        (crc32fast::hash(self.name.as_bytes()) as i32).unsigned_abs()
    }
    pub fn desktop() -> Self {
        Self {
            name: "Desktop".into(),
            cmd: String::new(),
            working_dir: String::new(),
            prep: vec![],
            computed_id: None,
            aliases: vec![],
            extra: BTreeMap::new(),
        }
    }
}
#[cfg(test)]
mod tests;
