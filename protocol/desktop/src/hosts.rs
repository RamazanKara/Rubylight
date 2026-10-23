//! The PCs Nightjar knows: discovery on the local network (mDNS `_nvstream._tcp`), hosts
//! added by address, their online state, pairing, the app list with box art, and quitting
//! the running app. Every host request runs on a worker thread and comes back as an
//! [`Event`]; the UI thread owns the list.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use nightjar_core::http::native_api::capabilities::Capabilities;
use nightjar_core::http::native_api::library::Library;
use nightjar_core::http::native_api::session::{ActionId, Session};
use nightjar_core::http::{
    App, AppControl, ClientIdentity, Feature, HostClient, PairResult, ServerInfo, DEFAULT_HTTP_PORT,
};

use crate::config::{HostRecord, Store};

/// Wakes the UI thread after an event was queued.
pub type Waker = Arc<dyn Fn() + Send + Sync>;

const POLL_INTERVAL: Duration = Duration::from_secs(4);
/// The native library and permissions are read this often; the session on every poll.
const NATIVE_FULL_INTERVAL: Duration = Duration::from_secs(30);
const MDNS_SERVICE: &str = "_nvstream._tcp.local.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reach {
    Unknown,
    Online,
    Offline,
}

pub enum Apps {
    NotLoaded,
    Loading,
    Loaded(Vec<App>),
    Failed(String),
}

pub enum Art {
    Loading,
    Ready(egui::TextureHandle),
    Missing,
}

/// What the Rubylight native API told us about a PC, cached per PC. The library of a PC that
/// is away is the one saved on disk, so Home still has something to show.
#[derive(Default)]
pub struct Native {
    pub capabilities: Option<Capabilities>,
    pub library: Option<Library>,
    /// The library was read from the PC in this run (not loaded from disk).
    pub library_live: bool,
    pub session: Option<Session>,
    /// Why the last read failed; the data above is then the last good one.
    pub error: Option<String>,
    /// A session action (Close game) is running.
    pub acting: bool,
    loading: bool,
    full_at: Option<Instant>,
}

impl Native {
    /// Whether this device may end games on the PC (the host's `host_control` permission);
    /// unknown counts as no.
    pub fn host_control(&self) -> bool {
        self.capabilities.as_ref().is_some_and(|c| c.permissions.host_control)
    }
}

/// One successful native read.
pub struct NativeRead {
    capabilities: Option<Capabilities>,
    library: Option<Library>,
    session: Result<Session, String>,
}

pub struct Host {
    pub id: u64,
    pub record: HostRecord,
    pub reach: Reach,
    pub info: Option<ServerInfo>,
    pub apps: Apps,
    pub art: HashMap<i32, Art>,
    /// The cover version each `art` entry was asked for (empty for classic box art).
    art_versions: HashMap<i32, String>,
    pub native: Native,
    pub error: Option<String>,
    polling: bool,
    last_poll: Option<Instant>,
}

impl Host {
    pub fn paired(&self) -> bool {
        self.record.server_certificate.is_some()
            && self.info.as_ref().is_none_or(|i| i.paired)
    }

    /// The PC speaks the native API now, or did when its library was saved.
    pub fn native_api(&self) -> bool {
        self.info.as_ref().is_some_and(|i| i.capabilities.has(Feature::NativeApi))
            || (self.info.is_none() && self.native.library.is_some())
    }

    /// Whether the cover of `app` at `version` has not been asked for yet.
    pub fn art_wanted(&self, app: i32, version: &str) -> bool {
        self.art_versions.get(&app).is_none_or(|v| v != version) || !self.art.contains_key(&app)
    }

    pub fn display_name(&self) -> &str {
        if !self.record.name.is_empty() {
            &self.record.name
        } else {
            &self.record.address
        }
    }

    /// The running app's ID, when the host reports one.
    pub fn running_app(&self) -> Option<i32> {
        self.info
            .as_ref()
            .filter(|i| i.current_game != 0)
            .map(|i| i.current_game)
    }
}

pub enum Event {
    Info {
        id: u64,
        address: String,
        result: Result<Box<ServerInfo>, String>,
    },
    Discovered {
        name: String,
        addresses: Vec<IpAddr>,
        port: u16,
    },
    Paired {
        id: u64,
        request: u64,
        result: Result<String, String>,
    },
    Apps {
        id: u64,
        result: Result<Vec<App>, String>,
    },
    Art {
        id: u64,
        app: i32,
        version: String,
        image: Option<egui::ColorImage>,
    },
    Native {
        id: u64,
        result: Result<Box<NativeRead>, String>,
    },
    Session {
        id: u64,
        action: ActionId,
        result: Result<Box<Session>, String>,
    },
    Quit {
        id: u64,
        result: Result<(), String>,
    },
    Wake {
        id: u64,
        result: Result<(), String>,
    },
    Action {
        id: u64,
        result: Result<(), String>,
    },
}

pub struct Hosts {
    pub list: Vec<Host>,
    identity: Option<Arc<ClientIdentity>>,
    events: Receiver<Event>,
    sender: Sender<Event>,
    waker: Waker,
    next_id: u64,
    pairing: Option<(u64, Arc<HostClient>)>,
    pairing_generation: u64,
    /// Hosts whose live library changed since the last `take_library_saves`.
    library_saves: Vec<u64>,
    _mdns: Option<mdns_sd::ServiceDaemon>,
}

pub fn client_for(identity: &Arc<ClientIdentity>, record: &HostRecord, address: &str) -> Result<HostClient, String> {
    let port = if record.http_port == 0 {
        DEFAULT_HTTP_PORT
    } else {
        record.http_port
    };
    let mut client = HostClient::new(address, port, identity.clone())
        .map_err(|e| e.to_string())?
        .with_device_name(device_name());
    if record.https_port != 0 {
        client = client.with_https_port(record.https_port);
    }
    if let Some(pem) = &record.server_certificate {
        client = client
            .with_server_certificate_pem(pem)
            .map_err(|e| e.to_string())?;
    }
    Ok(client)
}

/// The name the host lists this client under.
pub fn device_name() -> String {
    let name = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .filter(|n| !n.is_empty())
        .or_else(|| {
            std::fs::read_to_string("/etc/hostname")
                .ok()
                .map(|n| n.trim().to_owned())
                .filter(|n| !n.is_empty())
        });
    match name {
        Some(name) => format!("Nightjar ({name})"),
        None => "Nightjar".into(),
    }
}

/// A four-digit PIN from the OS-seeded hasher keys.
pub fn random_pin() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos()),
    );
    format!("{:04}", hasher.finish() % 10_000)
}

fn describe(error: &nightjar_core::http::Error) -> String {
    use nightjar_core::http::Error;
    match error {
        Error::Io(e) if e.kind() == std::io::ErrorKind::TimedOut => "The PC did not answer.".into(),
        Error::Io(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => {
            "The PC refused the connection. Is the host running?".into()
        }
        Error::Status { code: 401, .. } | Error::NotPaired => "This PC is not paired.".into(),
        Error::CertificateMismatch => {
            "The PC presented a different certificate. Pair it again.".into()
        }
        other => {
            let text = other.to_string();
            let mut chars = text.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect(),
                None => text,
            }
        }
    }
}

impl Hosts {
    pub fn new(records: Vec<HostRecord>, waker: Waker) -> Self {
        let (sender, events) = channel();
        let mut hosts = Self {
            list: Vec::new(),
            identity: None,
            events,
            sender,
            waker,
            next_id: 1,
            pairing: None,
            pairing_generation: 0,
            library_saves: Vec::new(),
            _mdns: None,
        };
        for record in records {
            hosts.insert(record);
        }
        hosts._mdns = hosts.start_discovery();
        hosts
    }

    pub fn set_identity(&mut self, identity: Arc<ClientIdentity>) {
        self.identity = Some(identity);
    }

    pub fn identity(&self) -> Option<&Arc<ClientIdentity>> {
        self.identity.as_ref()
    }

    fn insert(&mut self, record: HostRecord) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.list.push(Host {
            id,
            record,
            reach: Reach::Unknown,
            info: None,
            apps: Apps::NotLoaded,
            art: HashMap::new(),
            art_versions: HashMap::new(),
            native: Native::default(),
            error: None,
            polling: false,
            last_poll: None,
        });
        id
    }

    pub fn get(&self, id: u64) -> Option<&Host> {
        self.list.iter().find(|h| h.id == id)
    }

    pub fn get_mut(&mut self, id: u64) -> Option<&mut Host> {
        self.list.iter_mut().find(|h| h.id == id)
    }

    pub fn records(&self) -> Vec<HostRecord> {
        self.list
            .iter()
            .filter(|h| h.record.manual || h.record.server_certificate.is_some())
            .map(|h| h.record.clone())
            .collect()
    }

    fn spawn(&self, name: &str, work: impl FnOnce() -> Option<Event> + Send + 'static) {
        let sender = self.sender.clone();
        let waker = self.waker.clone();
        let _ = std::thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                if let Some(event) = work() {
                    let _ = sender.send(event);
                    waker();
                }
            });
    }

    fn start_discovery(&self) -> Option<mdns_sd::ServiceDaemon> {
        let daemon = mdns_sd::ServiceDaemon::new().ok()?;
        let receiver = daemon.browse(MDNS_SERVICE).ok()?;
        let sender = self.sender.clone();
        let waker = self.waker.clone();
        std::thread::Builder::new()
            .name("mdns".into())
            .spawn(move || {
                while let Ok(event) = receiver.recv() {
                    if let mdns_sd::ServiceEvent::ServiceResolved(service) = event {
                        let mut addresses: Vec<IpAddr> = service
                            .addresses
                            .iter()
                            .map(|a| a.to_ip_addr())
                            .filter(|a| !a.is_loopback())
                            .collect();
                        // IPv4 first: link-local IPv6 needs a scope the HTTP client may lack.
                        addresses.sort_by_key(|a| !a.is_ipv4());
                        let name = service
                            .fullname
                            .split("._nvstream")
                            .next()
                            .unwrap_or_default()
                            .to_owned();
                        if sender
                            .send(Event::Discovered {
                                name,
                                addresses,
                                port: service.port,
                            })
                            .is_err()
                        {
                            break;
                        }
                        waker();
                    }
                }
            })
            .ok()?;
        Some(daemon)
    }

    /// Adds a host by address and checks it at once. Returns its ID.
    pub fn add_manual(&mut self, address: &str, port: Option<u16>) -> u64 {
        if let Some(host) = self.list.iter_mut().find(|h| {
            h.record.address.eq_ignore_ascii_case(address)
                && port.is_none_or(|p| p == h.record.http_port)
        }) {
            host.record.manual = true;
            let id = host.id;
            self.poll_one(id);
            return id;
        }
        let id = self.insert(HostRecord {
            address: address.to_owned(),
            http_port: port.unwrap_or(DEFAULT_HTTP_PORT),
            manual: true,
            ..Default::default()
        });
        self.poll_one(id);
        id
    }

    pub fn remove(&mut self, id: u64) {
        self.list.retain(|h| h.id != id);
    }

    /// Checks every host that is due.
    pub fn poll(&mut self) {
        let due: Vec<u64> = self
            .list
            .iter()
            .filter(|h| !h.polling && h.last_poll.is_none_or(|t| t.elapsed() >= POLL_INTERVAL))
            .map(|h| h.id)
            .collect();
        for id in due {
            self.poll_one(id);
        }
    }

    pub fn poll_one(&mut self, id: u64) {
        let Some(identity) = self.identity.clone() else {
            return;
        };
        let Some(host) = self.get_mut(id) else {
            return;
        };
        host.polling = true;
        host.last_poll = Some(Instant::now());
        let record = host.record.clone();
        let likely_online = host.reach == Reach::Online;
        self.spawn("serverinfo", move || {
            let mut addresses = vec![record.address.clone()];
            addresses.extend(record.other_addresses.iter().cloned());
            let mut last_error = String::from("No address");
            for address in addresses {
                let result = client_for(&identity, &record, &address)
                    .and_then(|c| c.server_info(likely_online).map_err(|e| describe(&e)));
                match result {
                    Ok(info) => {
                        return Some(Event::Info {
                            id,
                            address,
                            result: Ok(Box::new(info)),
                        })
                    }
                    Err(e) => last_error = e,
                }
            }
            Some(Event::Info {
                id,
                address: record.address.clone(),
                result: Err(last_error),
            })
        });
    }

    /// Starts pairing with `pin`; the host's user types it in its web UI.
    pub fn pair(&mut self, id: u64, pin: String, passphrase: Option<String>) {
        self.cancel_pairing();
        let request = self.pairing_generation;
        let Some(identity) = self.identity.clone() else {
            return;
        };
        let Some(host) = self.get(id) else {
            return;
        };
        let mut record = host.record.clone();
        record.server_certificate = None;
        let client = match client_for(&identity, &record, &record.address) {
            Ok(client) => Arc::new(client),
            Err(e) => {
                let _ = self.sender.send(Event::Paired { id, request, result: Err(e) });
                return;
            }
        };
        self.pairing = Some((id, client.clone()));
        self.spawn("pair", move || {
            let result = client
                .server_info_http(true)
                .and_then(|info| match &passphrase {
                    Some(passphrase) => client.pair_with_passphrase(&info, &pin, passphrase),
                    None => client.pair(&info, &pin),
                })
                .map_err(|e| describe(&e))
                .and_then(|result| match result {
                    PairResult::Paired {
                        server_certificate_pem,
                    } => Ok(server_certificate_pem),
                    PairResult::PinWrong => Err("The PIN entered on the PC was wrong.".into()),
                    PairResult::AlreadyInProgress => {
                        Err("Another device is pairing with this PC right now.".into())
                    }
                    PairResult::Failed(f) => Err(format!("The PC declined pairing ({f:?}).")),
                });
            Some(Event::Paired { id, request, result })
        });
    }

    pub fn cancel_pairing(&mut self) {
        self.pairing_generation += 1;
        if let Some((_, client)) = self.pairing.take() {
            client.cancel_pending_requests();
        }
    }

    pub fn wake(&self, id: u64) {
        let Some(host) = self.get(id) else { return };
        let record = host.record.clone();
        self.spawn("wake", move || Some(Event::Wake { id, result: crate::wol::wake(&record) }));
    }

    pub fn unpair(&mut self, id: u64) {
        let Some(identity) = self.identity.clone() else {
            return;
        };
        let Some(host) = self.get_mut(id) else {
            return;
        };
        let record = host.record.clone();
        host.record.server_certificate = None;
        host.apps = Apps::NotLoaded;
        host.art.clear();
        host.art_versions.clear();
        host.native = Native::default();
        self.spawn("unpair", move || {
            if let Ok(client) = client_for(&identity, &record, &record.address) {
                let _ = client.unpair();
            }
            None
        });
    }

    pub fn load_apps(&mut self, id: u64) {
        let Some(identity) = self.identity.clone() else {
            return;
        };
        let Some(host) = self.get_mut(id) else {
            return;
        };
        if matches!(host.apps, Apps::Loading) {
            return;
        }
        host.apps = Apps::Loading;
        let record = host.record.clone();
        let capabilities = host
            .info
            .as_ref()
            .map(|i| i.capabilities.clone())
            .unwrap_or(nightjar_core::http::HostCapabilities::UNKNOWN);
        self.spawn("applist", move || {
            let result = client_for(&identity, &record, &record.address).and_then(|c| {
                c.app_list(&capabilities).map_err(|e| describe(&e))
            });
            Some(Event::Apps { id, result })
        });
    }

    /// Reads the native API of a paired, online PC that has it: the session every time, the
    /// permissions and library when they are due (or `force`). Does nothing while a read runs.
    pub fn refresh_native(&mut self, id: u64, force: bool) {
        let Some(identity) = self.identity.clone() else {
            return;
        };
        let Some(host) = self.get_mut(id) else {
            return;
        };
        let live = host.reach == Reach::Online
            && host.paired()
            && host.info.as_ref().is_some_and(|i| i.capabilities.has(Feature::NativeApi));
        if !live || host.native.loading {
            return;
        }
        let full = force
            || host.native.library.is_none()
            || !host.native.library_live
            || host.native.full_at.is_none_or(|t| t.elapsed() >= NATIVE_FULL_INTERVAL);
        host.native.loading = true;
        if full {
            host.native.full_at = Some(Instant::now());
        }
        let record = host.record.clone();
        self.spawn("native-api", move || {
            let result = client_for(&identity, &record, &record.address).and_then(|client| {
                let (capabilities, library) = if full {
                    (
                        Some(client.native_capabilities().map_err(|e| describe(&e))?),
                        Some(client.library().map_err(|e| describe(&e))?),
                    )
                } else {
                    (None, None)
                };
                let session = client.session().map_err(|e| describe(&e));
                // Without a library in this read, a failed session is the whole failure; with
                // one, Home still has the games and shows the session error beside them.
                match (&session, full) {
                    (Err(e), false) => Err(e.clone()),
                    _ => Ok(Box::new(NativeRead { capabilities, library, session })),
                }
            });
            Some(Event::Native { id, result })
        });
    }

    /// Runs Close game (or End, Disconnect) through the native API.
    pub fn session_action(&mut self, id: u64, action: ActionId) {
        let Some(identity) = self.identity.clone() else {
            return;
        };
        let Some(host) = self.get_mut(id) else {
            return;
        };
        if host.native.acting {
            return;
        }
        host.native.acting = true;
        let record = host.record.clone();
        self.spawn("session-action", move || {
            let result = client_for(&identity, &record, &record.address).and_then(|client| {
                client.session_action(action).map(Box::new).map_err(|e| describe(&e))
            });
            Some(Event::Session { id, action, result })
        });
    }

    /// Puts back the libraries saved by earlier runs, for PCs not reached yet.
    pub fn load_cached_libraries(&mut self, store: &Store) {
        for host in &mut self.list {
            if host.record.server_certificate.is_some() && !host.record.unique_id.is_empty() {
                host.native.library = store.library(&host.record.unique_id);
            }
        }
    }

    /// The libraries read since the last call, by the PC's `uniqueid`, to be saved.
    pub fn take_library_saves(&mut self) -> Vec<(String, Library)> {
        std::mem::take(&mut self.library_saves)
            .into_iter()
            .filter_map(|id| {
                let host = self.get(id)?;
                Some((host.record.unique_id.clone(), host.native.library.clone()?))
            })
            .filter(|(unique_id, _)| !unique_id.is_empty())
            .collect()
    }

    /// Fetches box art once per app, from the disk cache when present. `version` is the cover
    /// version the native library reports (empty for classic box art); a new one fetches again.
    pub fn request_art(&mut self, id: u64, app: i32, version: &str, store: &Store) {
        let Some(identity) = self.identity.clone() else {
            return;
        };
        let Some(host) = self.get_mut(id) else {
            return;
        };
        if host.art_versions.get(&app).is_some_and(|old| old != version) {
            host.art.remove(&app);
        }
        if host.art.contains_key(&app) {
            return;
        }
        host.art.insert(app, Art::Loading);
        host.art_versions.insert(app, version.to_owned());
        let version = version.to_owned();
        let record = host.record.clone();
        let cache = store.art_path(&record.unique_id, app, &version);
        self.spawn("boxart", move || {
            let bytes = std::fs::read(&cache).ok().or_else(|| {
                let bytes = match client_for(&identity, &record, &record.address).and_then(|c| {
                    c.box_art(app).map_err(|e| e.to_string())
                }) {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        // Hosts answer 404 for apps without art.
                        if !e.starts_with("not found") {
                            eprintln!("nightjar: box art for app {app}: {e}");
                        }
                        return None;
                    }
                };
                if let Some(dir) = cache.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                let _ = std::fs::write(&cache, &bytes);
                Some(bytes)
            });
            let image = bytes.and_then(|b| decode_image(&b));
            Some(Event::Art { id, app, version, image })
        });
    }

    pub fn quit_app(&mut self, id: u64) {
        let Some(identity) = self.identity.clone() else {
            return;
        };
        let Some(host) = self.get(id) else {
            return;
        };
        let record = host.record.clone();
        self.spawn("cancel", move || {
            let result = client_for(&identity, &record, &record.address).and_then(|c| match c.cancel() {
                Ok(true) => Ok(()),
                Ok(false) => Err("The PC declined to quit the app.".into()),
                Err(nightjar_core::http::Error::Status { code: 599, .. }) => {
                    Err("The running app belongs to another device.".into())
                }
                Err(e) => Err(describe(&e)),
            });
            Some(Event::Quit { id, result })
        });
    }

    pub fn control_action(&mut self, id: u64, app_id: i32) {
        let Some(identity) = self.identity.clone() else { return };
        let Some(host) = self.get(id) else { return };
        let Apps::Loaded(apps) = &host.apps else { return };
        let Some(app) = apps.iter().find(|app| app.id == app_id && app.is_control_action()) else { return };
        let request = nightjar_core::http::LaunchRequest {
            app_id,
            app_uuid: app.uuid.clone(),
            role: app.role(false),
            ..Default::default()
        };
        let control = app.control;
        let record = host.record.clone();
        self.spawn("host-action", move || {
            let result = client_for(&identity, &record, &record.address).and_then(|client| {
                let info = client.server_info_https().map_err(|e| describe(&e))?;
                let apps = client.app_list(&info.capabilities).map_err(|e| describe(&e))?;
                if !apps.iter().any(|app| app.id == request.app_id && app.uuid == request.app_uuid && app.control == control) {
                    return Err("The PC action changed. Refresh the library and try again.".into());
                }
                let mut result = client.launch(&request, &info);
                if control == AppControl::Terminate && matches!(&result,
                    Err(nightjar_core::http::Error::Status { code: 410, message })
                    if message == "This will close the active stream but leave Remote Monitor and Remote Input connected. Launch Terminate again within 60 seconds to confirm this was intentional.") {
                    // The launcher already obtained confirmation for this action.
                    result = client.launch(&request, &info);
                }
                match result {
                    Ok(Some(_)) => Ok(()),
                    Ok(None) => Err("The PC declined the action.".into()),
                    Err(error) if completed_action(control, &error) => Ok(()),
                    Err(error) => Err(describe(&error)),
                }
            });
            Some(Event::Action { id, result })
        });
    }

    /// Applies queued events. Returns those the UI reacts to (pairing, quit) after the list
    /// has taken them in, and whether the stored hosts changed.
    pub fn drain(&mut self, ctx: &egui::Context) -> (Vec<Event>, bool) {
        let mut out = Vec::new();
        let mut changed = false;
        while let Ok(event) = self.events.try_recv() {
            match event {
                Event::Info {
                    id,
                    address,
                    result,
                } => {
                    changed |= self.apply_info(id, address, result);
                    self.follow_native(id);
                }
                Event::Discovered {
                    name,
                    addresses,
                    port,
                } => self.apply_discovery(name, addresses, port),
                Event::Paired { id, request, result } => {
                    if request != self.pairing_generation {
                        continue;
                    }
                    self.pairing = None;
                    if let (Ok(pem), Some(host)) = (&result, self.get_mut(id)) {
                        host.record.server_certificate = Some(pem.clone());
                        host.apps = Apps::NotLoaded;
                        changed = true;
                        self.poll_one(id);
                    }
                    out.push(Event::Paired { id, request, result });
                }
                Event::Apps { id, result } => {
                    if let Some(host) = self.get_mut(id) {
                        host.apps = match result {
                            Ok(mut apps) => {
                                apps.sort_by_key(|a| a.host_index.unwrap_or(u32::MAX));
                                Apps::Loaded(apps)
                            }
                            Err(e) => Apps::Failed(e),
                        };
                    }
                }
                Event::Art { id, app, version, image } => {
                    if let Some(host) = self.get_mut(id).filter(|h| h.art_versions.get(&app) == Some(&version)) {
                        let art = match image {
                            Some(image) => Art::Ready(ctx.load_texture(
                                format!("art-{id}-{app}"),
                                image,
                                egui::TextureOptions::LINEAR,
                            )),
                            None => Art::Missing,
                        };
                        host.art.insert(app, art);
                    }
                }
                Event::Native { id, result } => {
                    let mut reload_apps = false;
                    let mut save = false;
                    if let Some(host) = self.get_mut(id) {
                        host.native.loading = false;
                        match result {
                            Ok(read) => {
                                let read = *read;
                                host.native.error = None;
                                if let Some(capabilities) = read.capabilities {
                                    host.native.capabilities = Some(capabilities);
                                }
                                if let Some(library) = read.library {
                                    // The classic list a launch goes through follows the library.
                                    reload_apps = !host.native.library_live
                                        || host.native.library.as_ref().is_none_or(|old| old.revision != library.revision);
                                    save = host.native.library.as_ref() != Some(&library);
                                    host.native.library = Some(library);
                                    host.native.library_live = true;
                                }
                                match read.session {
                                    Ok(session) => host.native.session = Some(session),
                                    Err(e) => host.native.error = Some(format!("The session could not be read. {e}")),
                                }
                            }
                            Err(e) => host.native.error = Some(e),
                        }
                    }
                    if save && !self.library_saves.contains(&id) {
                        self.library_saves.push(id);
                    }
                    if reload_apps {
                        self.load_apps(id);
                    }
                }
                Event::Session { id, action, result } => {
                    if let Some(host) = self.get_mut(id) {
                        host.native.acting = false;
                        if let Ok(session) = &result {
                            host.native.session = Some((**session).clone());
                        }
                    }
                    self.poll_one(id);
                    out.push(Event::Session { id, action, result });
                }
                Event::Quit { id, result } => {
                    self.poll_one(id);
                    out.push(Event::Quit { id, result });
                }
                Event::Wake { id, result } => {
                    self.poll_one(id);
                    out.push(Event::Wake { id, result });
                }
                Event::Action { id, result } => {
                    self.poll_one(id);
                    self.load_apps(id);
                    out.push(Event::Action { id, result });
                }
            }
        }
        (out, changed)
    }

    /// A PC that was just checked and has the native API: read it, and have the classic app
    /// list ready, because a launch goes through that list.
    fn follow_native(&mut self, id: u64) {
        let Some(host) = self.get(id) else { return };
        if host.reach != Reach::Online || !host.paired() || !host.native_api() {
            return;
        }
        let apps = matches!(host.apps, Apps::NotLoaded);
        self.refresh_native(id, false);
        if apps {
            self.load_apps(id);
        }
    }

    fn apply_info(&mut self, id: u64, address: String, result: Result<Box<ServerInfo>, String>) -> bool {
        let mut changed = false;
        let Some(index) = self.list.iter().position(|h| h.id == id) else {
            return false;
        };
        // A newly found address may belong to a host already listed under another one.
        if let Ok(info) = &result {
            if let Some(other) = self
                .list
                .iter()
                .position(|h| h.id != id && !h.record.unique_id.is_empty() && h.record.unique_id == info.unique_id)
            {
                let (keep, drop) = if self.list[other].record.server_certificate.is_some()
                    || !self.list[index].record.server_certificate.is_some()
                {
                    (other, index)
                } else {
                    (index, other)
                };
                let dropped = self.list[drop].record.clone();
                let kept = &mut self.list[keep].record;
                kept.manual |= dropped.manual;
                if kept.pace.is_none() { kept.pace = dropped.pace; }
                if kept.settings.is_none() {
                    kept.settings = dropped.settings;
                }
                for (key, settings) in dropped.app_settings {
                    kept.app_settings.entry(key).or_insert(settings);
                }
                for a in std::iter::once(dropped.address).chain(dropped.other_addresses) {
                    if a != kept.address && !kept.other_addresses.contains(&a) {
                        kept.other_addresses.push(a);
                    }
                }
                let keep_id = self.list[keep].id;
                self.list.remove(drop);
                changed = true;
                let index = self.list.iter().position(|h| h.id == keep_id).unwrap_or(0);
                return self.store_info(index, address, result) || changed;
            }
        }
        changed |= self.store_info(index, address, result);
        changed
    }

    fn store_info(&mut self, index: usize, address: String, result: Result<Box<ServerInfo>, String>) -> bool {
        let host = &mut self.list[index];
        host.polling = false;
        let mut changed = false;
        match result {
            Ok(info) => {
                host.reach = Reach::Online;
                host.error = None;
                if host.record.address != address {
                    let old = std::mem::replace(&mut host.record.address, address.clone());
                    host.record.other_addresses.retain(|a| *a != address);
                    if !host.record.other_addresses.contains(&old) {
                        host.record.other_addresses.push(old);
                    }
                    changed = true;
                }
                if host.record.unique_id != info.unique_id || host.record.name != info.hostname {
                    host.record.unique_id = info.unique_id.clone();
                    host.record.name = info.hostname.clone();
                    changed = true;
                }
                if host.record.https_port != info.https_port {
                    host.record.https_port = info.https_port;
                    changed = true;
                }
                if host.record.mac.is_none() && info.mac_address.is_some() {
                    host.record.mac = info.mac_address.clone();
                    changed = true;
                }
                // The host forgot this client: drop the pin so the user can pair again.
                if !info.paired && host.record.server_certificate.is_some() {
                    host.record.server_certificate = None;
                    host.apps = Apps::NotLoaded;
                    changed = true;
                }
                host.info = Some(*info);
            }
            Err(e) => {
                host.reach = Reach::Offline;
                host.error = Some(e);
            }
        }
        changed
    }

    fn apply_discovery(&mut self, name: String, addresses: Vec<IpAddr>, port: u16) {
        let Some(first) = addresses.first() else {
            return;
        };
        let texts: Vec<String> = addresses.iter().map(IpAddr::to_string).collect();
        if let Some(host) = self.list.iter_mut().find(|h| {
            texts.contains(&h.record.address) || h.record.other_addresses.iter().any(|a| texts.contains(a))
        }) {
            if host.record.name.is_empty() {
                host.record.name = name;
            }
            return;
        }
        let id = self.insert(HostRecord {
            name,
            address: first.to_string(),
            other_addresses: texts[1..].to_vec(),
            http_port: if port == 0 { DEFAULT_HTTP_PORT } else { port },
            ..Default::default()
        });
        self.poll_one(id);
    }
}

fn completed_action(control: AppControl, error: &nightjar_core::http::Error) -> bool {
    let nightjar_core::http::Error::Status { code: 410, message } = error else { return false; };
    let message = message.trim().trim_end_matches('.');
    let known: &[&str] = match control {
        AppControl::DisconnectMonitor => &["Remote monitor disconnected", "Remote Monitor disconnected successfully"],
        AppControl::DisconnectInput => &["Remote input disconnected", "Remote Input disconnected successfully"],
        AppControl::Terminate => &["Application terminated", "Active stream terminated. Remote Monitor and Remote Input remain connected", "App terminated"],
        _ => &[],
    };
    known.iter().any(|known| message.eq_ignore_ascii_case(known))
}

fn decode_image(bytes: &[u8]) -> Option<egui::ColorImage> {
    let image = image::load_from_memory(bytes).ok()?.to_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    Some(egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pins_are_four_digits() {
        for _ in 0..20 {
            let pin = random_pin();
            assert_eq!(pin.len(), 4);
            assert!(pin.bytes().all(|b| b.is_ascii_digit()));
        }
    }

    #[test]
    fn action_completion_requires_a_known_message_for_the_selected_action() {
        use nightjar_core::http::Error;
        let terminated = Error::Status { code: 410, message: "Application terminated.".into() };
        assert!(completed_action(AppControl::Terminate, &terminated));
        assert!(!completed_action(AppControl::DisconnectInput, &terminated));
        assert!(!completed_action(AppControl::Terminate, &Error::Status { code: 410, message: "Confirm termination".into() }));
        assert!(!completed_action(AppControl::Terminate, &Error::Status { code: 500, message: "Application terminated".into() }));
    }
}
