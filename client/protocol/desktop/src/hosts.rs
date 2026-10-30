//! The PCs Nightjar knows: discovery on the local network (mDNS `_nvstream._tcp`), hosts
//! added by address, their online state, pairing, the app list with box art, and quitting
//! the running app. Every host request runs on a worker thread and comes back as an
//! [`Event`]; the UI thread owns the list.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use nightjar_core::http::native_api::feature;
use nightjar_core::http::native_api::library::Library;
use nightjar_core::http::native_api::session::{ActionId, Session};
use nightjar_core::http::{
    App, AppControl, Availability, ClientIdentity, Feature, HostClient, NativeCapabilities, PairResult, PowerError,
    PowerReply, Reason, ServerInfo, DEFAULT_HTTP_PORT,
};

use rubylight_protocol::pairing_uri::{PairingUri, PREFIX};

use crate::strings::{en, trf};
use crate::config::{HostRecord, Store};

/// Wakes the UI thread after an event was queued.
pub type Waker = Arc<dyn Fn() + Send + Sync>;

const POLL_INTERVAL: Duration = Duration::from_secs(4);
/// After a quit, a closed game or a stream's end the PC changes its state a moment later, so it is
/// asked every second for this long (`Host::recheck_until`).
const RECHECK_INTERVAL: Duration = Duration::from_secs(1);
const RECHECK_WINDOW: Duration = Duration::from_secs(15);
const RECHECK_LIBRARY_INTERVAL: Duration = Duration::from_secs(3);
/// The native library and permissions are read this often; the session on every poll.
const NATIVE_FULL_INTERVAL: Duration = Duration::from_secs(30);
const MDNS_SERVICE: &str = "_nvstream._tcp.local.";
/// A sleep token older than this is asked for again before it is spent: the PC keeps it 30 s.
const SLEEP_TOKEN_REFRESH: Duration = Duration::from_secs(25);

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
    /// Its features and this device's permissions; `Host::info` carries them merged in.
    pub capabilities: Option<NativeCapabilities>,
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
        self.capabilities.as_ref().is_some_and(|c| c.capabilities.permissions.host_control)
    }
}

/// Takes what the native API said into serverinfo's capabilities, so every gate on them
/// (shared folders, co-op, the image clipboard) sees it. Without a native read nothing changes,
/// and a PC without the native API offers none of those features either way.
pub fn merge_native(info: &mut ServerInfo, native: Option<&NativeCapabilities>) {
    if let Some(native) = native {
        info.capabilities = info.capabilities.with_native(native);
    }
}

/// One successful native read.
pub struct NativeRead {
    capabilities: Option<NativeCapabilities>,
    library: Option<Library>,
    session: Result<Session, String>,
    /// The PC's wake targets (`wol::targets`) to keep in place of the stored ones: empty for a
    /// PC without `wake`; None when they were not read, or the read failed (those adapters
    /// still belong to the PC).
    wake: Option<(Vec<String>, Vec<String>)>,
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
    /// Bumped when the PC is unpaired, so a native read that started before is dropped.
    native_epoch: u64,
    /// Counts native reads and actions, in the order they started or landed.
    native_seq: u64,
    /// `native_seq` when the last session action landed: older reads carry an older session.
    action_seq: u64,
    pub error: Option<String>,
    polling: bool,
    last_poll: Option<Instant>,
    /// Polls started, and the newest one whose answer was taken in: an older answer that lands
    /// later (two polls overlap after a quit) must not bring back a state the PC has left.
    poll_seq: u64,
    info_seq: u64,
    /// Until then the PC is polled every second (see `RECHECK_WINDOW`).
    recheck_until: Option<Instant>,
}

impl Host {
    /// How long to leave the PC alone between polls at `now`.
    fn poll_interval(&self, now: Instant) -> Duration {
        if self.recheck_until.is_some_and(|until| now < until) {
            RECHECK_INTERVAL
        } else {
            POLL_INTERVAL
        }
    }

    /// In the recheck window and the library was not read for `RECHECK_LIBRARY_INTERVAL`.
    fn library_recheck_due(&self, now: Instant) -> bool {
        self.recheck_until.is_some_and(|until| now < until)
            && self.native.full_at.is_none_or(|t| now.saturating_duration_since(t) >= RECHECK_LIBRARY_INTERVAL)
    }

    pub fn paired(&self) -> bool {
        self.record.server_certificate.is_some()
            && self.info.as_ref().is_none_or(|i| i.paired)
    }

    /// The PC speaks the native API now, or did when its library was saved.
    /// Whether the PC asks the player itself before a start replaces the running app.
    pub fn confirms_replacement(&self) -> bool {
        self.info.as_ref().is_some_and(|i| i.capabilities.has(Feature::HostConfirmsReplacement))
    }

    pub fn native_api(&self) -> bool {
        self.info.as_ref().is_some_and(|i| i.capabilities.has(Feature::NativeApi))
            || (self.info.is_none() && self.native.library.is_some())
    }

    /// Whether the cover of `app` at `version` has not been asked for yet.
    pub fn art_wanted(&self, app: i32, version: &str) -> bool {
        self.art_versions.get(&app).is_none_or(|v| v != version) || !self.art.contains_key(&app)
    }

    /// The name this device shows: the one chosen here, else the PC's own, else its address.
    pub fn display_name(&self) -> &str {
        if let Some(alias) = self.record.alias.as_deref().filter(|a| !a.is_empty()) {
            alias
        } else if !self.record.name.is_empty() {
            &self.record.name
        } else {
            &self.record.address
        }
    }

    /// What the rename field starts with: the name chosen here, else the PC's own, and empty
    /// when it never reported one (the address is not a name to keep).
    pub fn rename_prefill(&self) -> &str {
        self.record.alias.as_deref().filter(|a| !a.is_empty()).unwrap_or(&self.record.name)
    }

    /// Why pairing cannot start now, as text for the user.
    pub fn pairing_blocked(&self) -> Option<&'static str> {
        if self.reach == Reach::Offline {
            Some(PAIR_OFFLINE)
        } else if self.running_app().is_some() {
            Some(PAIR_IN_GAME)
        } else {
            None
        }
    }

    /// The running app's ID, when the host reports one.
    pub fn running_app(&self) -> Option<i32> {
        self.info
            .as_ref()
            .filter(|i| i.current_game != 0)
            .map(|i| i.current_game)
    }

    /// Sleep PC for a paired PC that answers: None when the PC does not offer it (stock hosts
    /// never do), `Some(Err(why))` when it does but not for this device.
    pub fn sleep_option(&self) -> Option<Result<(), &'static str>> {
        if self.reach != Reach::Online || !self.paired() {
            return None;
        }
        match self.info.as_ref()?.capabilities.availability(Feature::Sleep) {
            Availability::Supported => Some(Ok(())),
            Availability::Unsupported(Some(Reason::SleepPermission)) => Some(Err(SLEEP_NOT_ALLOWED)),
            _ => None,
        }
    }
}

// The pairing and unpairing errors, worded like Nightjar Android's.
pub const PAIR_OFFLINE: &str = en("Wake the PC and start the host app. Use the same network, then retry.");
pub const PAIR_IN_GAME: &str = en("Stop the host's stream, then pair again.");
pub const PAIR_PIN_WRONG: &str =
    en("PIN rejected. Try pairing again and enter the new PIN shown on this device in your host's settings or web UI.");
pub const PAIR_OTP_PIN_WRONG: &str =
    en("PIN rejected. Generate a new one-time PIN in your host's web UI and check the passphrase.");
pub const PAIR_IN_PROGRESS: &str = en("Finish or cancel the pending request on the host.");
pub const PAIR_DECLINED: &str = en("Cancel the old request in your host's settings or web UI, then retry.");
pub const PAIR_NETWORK: &str = en("Use the same network and start the host. Check its firewall, then retry.");
pub const PAIR_LINK_MISMATCH: &str =
    en("This PC's certificate does not match the pairing link, so Nightjar did not pair. Pair only with a link or QR code from your own PC.");
pub const LINK_INVALID: &str = en("That pairing link is not valid. Show a new QR code on the PC and copy its link again.");
pub const ADDRESS_INVALID: &str = en("That address is not valid");
pub const CLIPBOARD_NO_LINK: &str = en("Copy the link under the QR code on the PC's Devices page, then paste it here.");
pub const UNPAIR_FAILED: &str = en("Failed to unpair. The PC may still list this device.");
pub const UNPAIR_NOT_PAIRED: &str = en("Device was not paired.");
// Why the PC handed out no sleep token. A refusal shows the host's own words instead.
pub const SLEEP_NOT_ALLOWED: &str = en("The PC does not let this device put it to sleep. Allow it in the host's device settings.");
pub const SLEEP_UNSUPPORTED: &str = en("The PC cannot sleep.");
pub const SLEEP_BUSY: &str = en("The PC cannot sleep right now. Close games and streams on it, then try again.");

pub enum Event {
    Info {
        id: u64,
        seq: u64,
        address: String,
        result: Result<Box<ServerInfo>, String>,
    },
    Discovered {
        name: String,
        addresses: Vec<IpAddr>,
        port: u16,
    },
    /// This device's WAN address from STUN, the remote address of a PC found on the LAN.
    Remote {
        id: u64,
        result: std::io::Result<Option<Ipv4Addr>>,
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
        epoch: u64,
        seq: u64,
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
        run: u64,
        result: Result<(), String>,
    },
    /// A Sleep PC step: the offer to confirm, or (`confirmed`) the answer to the request. `ask`
    /// is the number the UI gave the request, so an answer to an earlier ask is told apart.
    Sleep {
        id: u64,
        ask: u64,
        /// The PC's name when it was asked, for the message about the answer.
        name: String,
        confirmed: bool,
        result: Result<SleepStep, String>,
    },
    /// The PC answered (or not) to being asked to drop this device. The pin is gone either way.
    Unpaired {
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
    /// STUN for the remote address of PCs found on the LAN; off in tests, which stay offline.
    stun: bool,
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

/// What proves the pairing user is at the PC.
enum Secret {
    /// A PIN this device shows; the user types it on the PC.
    Pin(String),
    /// A one-time PIN and passphrase created on the PC.
    OneTime { pin: String, passphrase: String },
    /// A QR pairing link (Experimental): a one-time PIN, and the certificate the PC must have.
    Link(Box<PairingUri>),
}

/// What was typed or pasted in Add PC.
#[derive(Debug, PartialEq)]
pub enum Entry {
    /// An address with an optional HTTP port.
    Address(String, Option<u16>),
    /// A QR pairing link (`rubylight://pair?...`) to add the PC and pair with at once.
    Link(Box<PairingUri>),
}

/// Reads an Add PC entry. A pairing link that does not parse is refused with [`LINK_INVALID`],
/// never read as an address.
pub fn parse_entry(text: &str) -> Result<Entry, &'static str> {
    let text = text.trim();
    if text.starts_with(PREFIX) {
        let mut link = PairingUri::parse(text).ok_or(LINK_INVALID)?;
        // Records keep IPv6 addresses without brackets, as typed ones are.
        if let Some(bare) = link.host.strip_prefix('[').and_then(|h| h.strip_suffix(']')) {
            link.host = bare.to_owned();
        }
        return Ok(Entry::Link(Box::new(link)));
    }
    crate::config::parse_address(text)
        .map(|(address, port)| Entry::Address(address, port))
        .ok_or(ADDRESS_INVALID)
}

/// What a pairing attempt that reached the host says to the user.
fn pair_outcome(result: PairResult, one_time: bool) -> Result<String, String> {
    match result {
        PairResult::Paired { server_certificate_pem } => Ok(server_certificate_pem),
        PairResult::PinWrong if one_time => Err(PAIR_OTP_PIN_WRONG.into()),
        PairResult::PinWrong => Err(PAIR_PIN_WRONG.into()),
        PairResult::AlreadyInProgress => Err(PAIR_IN_PROGRESS.into()),
        PairResult::Failed(_) => Err(PAIR_DECLINED.into()),
    }
}

/// An unpair answer that leaves this device unpaired: done, or the PC never knew it.
pub fn now_unpaired(result: &Result<(), String>) -> bool {
    match result {
        Ok(()) => true,
        Err(e) => e == UNPAIR_NOT_PAIRED,
    }
}

fn unpair_error(error: &nightjar_core::http::Error) -> String {
    use nightjar_core::http::Error;
    match error {
        Error::Status { code: 401, .. } | Error::NotPaired => UNPAIR_NOT_PAIRED.into(),
        _ => UNPAIR_FAILED.into(),
    }
}

/// A pairing request that failed on the way: a network problem, or whatever the client says.
fn pair_error(error: &nightjar_core::http::Error) -> String {
    match error {
        nightjar_core::http::Error::Io(_) => PAIR_NETWORK.into(),
        // Pairing starts without a stored certificate, so only a link's fingerprint refuses one.
        nightjar_core::http::Error::CertificateMismatch => PAIR_LINK_MISMATCH.into(),
        other => describe(other),
    }
}

fn describe(error: &nightjar_core::http::Error) -> String {
    use nightjar_core::http::Error;
    match error {
        Error::Io(e) if e.kind() == std::io::ErrorKind::TimedOut => en("The PC did not answer.").into(),
        Error::Io(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => {
            en("The PC refused the connection. Is the host running?").into()
        }
        Error::Status { code: 401, .. } | Error::NotPaired => en("This PC is not paired.").into(),
        Error::CertificateMismatch => {
            en("The PC presented a different certificate. Pair it again.").into()
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

pub enum SleepStep {
    /// The PC offers to sleep: confirm, then spend `token`, which the PC handed out at `at`.
    Offer { token: String, at: Handed },
    /// The PC took the request (202). It does not confirm that the PC suspended.
    Accepted,
}

/// When the PC handed out a sleep token, on two clocks: `Instant` stops while a Linux client is
/// suspended, the wall clock keeps running but can be set back. The larger age counts, so a
/// token never looks younger than it is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Handed {
    at: Instant,
    wall: SystemTime,
}

impl Handed {
    pub fn now() -> Self {
        Self { at: Instant::now(), wall: SystemTime::now() }
    }

    fn age(&self) -> Duration {
        self.at.elapsed().max(self.wall.elapsed().unwrap_or_default())
    }
}

/// The power-action calls Sleep PC makes; a fake PC in the tests.
trait PowerHost {
    fn power_capabilities(&self) -> Result<PowerReply, PowerError>;
    fn sleep(&self, token: &str) -> Result<PowerReply, PowerError>;
}

impl PowerHost for HostClient {
    fn power_capabilities(&self) -> Result<PowerReply, PowerError> {
        HostClient::power_capabilities(self)
    }
    fn sleep(&self, token: &str) -> Result<PowerReply, PowerError> {
        HostClient::sleep(self, token)
    }
}

/// A refusal in the host's own words; anything else said like the other host errors.
fn power_error(error: &PowerError) -> String {
    match error {
        PowerError::Refused { reply, .. } if !reply.message.is_empty() => reply.message.clone(),
        PowerError::Refused { reply, .. } => trf("The PC refused with status {}.", &[&reply.status]),
        PowerError::Host(e) => describe(e),
    }
}

/// Asks the PC for a sleep token; the time is taken before asking, so the token's age is never
/// under-counted.
fn sleep_token(host: &impl PowerHost) -> Result<(String, Handed), String> {
    let at = Handed::now();
    let reply = host.power_capabilities().map_err(|e| power_error(&e))?;
    match reply.token {
        Some(token) => Ok((token, at)),
        None if !reply.sleep_supported => Err(SLEEP_UNSUPPORTED.into()),
        None if !reply.sleep_allowed => Err(SLEEP_NOT_ALLOWED.into()),
        // Supported and allowed but no token, whatever `sleep_available` says: busy, like Android.
        None => Err(SLEEP_BUSY.into()),
    }
}

/// Spends the confirmed token, asking for a new one first when it is about to expire.
fn sleep_confirmed(host: &impl PowerHost, token: String, at: Handed) -> Result<SleepStep, String> {
    let token = if at.age() >= SLEEP_TOKEN_REFRESH { sleep_token(host)?.0 } else { token };
    host.sleep(&token).map_err(|e| power_error(&e))?;
    Ok(SleepStep::Accepted)
}

impl Hosts {
    pub fn new(records: Vec<HostRecord>, waker: Waker) -> Self {
        let mut hosts = Self::without_discovery(records, waker);
        hosts._mdns = hosts.start_discovery();
        hosts.stun = true;
        hosts
    }

    pub(crate) fn without_discovery(records: Vec<HostRecord>, waker: Waker) -> Self {
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
            stun: false,
            _mdns: None,
        };
        for record in records {
            hosts.insert(record);
        }
        hosts
    }

    pub fn set_identity(&mut self, identity: Arc<ClientIdentity>) {
        self.identity = Some(identity);
    }

    pub fn identity(&self) -> Option<&Arc<ClientIdentity>> {
        self.identity.as_ref()
    }

    fn insert(&mut self, record: HostRecord) -> u64 {
        crate::logfile::add_secrets(&record);
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
            native_epoch: 0,
            native_seq: 0,
            action_seq: 0,
            error: None,
            polling: false,
            last_poll: None,
            poll_seq: 0,
            info_seq: 0,
            recheck_until: None,
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
        // A PC added by its LAN address gets this network's WAN address too (Moonlight V+ PC).
        if address.parse::<Ipv4Addr>().is_ok_and(|a| a.is_private()) {
            self.find_remote_address(id);
        }
        self.poll_one(id);
        id
    }

    /// Asks STUN for this device's WAN address, which reaches a PC on its LAN from outside it
    /// once the router forwards the PC's ports (`LiFindExternalAddressIP4`, as Moonlight V+ PC
    /// does for a PC found by mDNS or added by a private IPv4 address).
    fn find_remote_address(&self, id: u64) {
        if !self.stun {
            return;
        }
        self.spawn("stun", move || {
            let result = nightjar_core::stun::find_external_ipv4(
                nightjar_core::stun::MOONLIGHT_STUN_SERVER,
                nightjar_core::stun::MOONLIGHT_STUN_PORT,
                nightjar_core::stun::STUN_TIMEOUT,
            );
            Some(Event::Remote { id, result })
        });
    }

    /// Names a PC on this device only. The PC's own name (or nothing) brings its name back.
    pub fn rename(&mut self, id: u64, alias: &str) -> bool {
        let Some(host) = self.get_mut(id) else {
            return false;
        };
        let given = alias.trim();
        let own = given == host.record.name || given == host.record.address;
        host.record.set_alias(if own { "" } else { alias });
        crate::logfile::add_secrets(&host.record);
        true
    }

    /// Pins the PC to one of its known addresses (`Some`), which then is the only one polled and
    /// streamed to, or brings back the automatic pick (`None`), as Moonlight V+ PC's
    /// `pinAddress` and `resetToAutomaticAddress`. False for an unknown PC or address.
    pub fn pin_address(&mut self, id: u64, address: Option<&str>) -> bool {
        let Some(host) = self.get_mut(id) else {
            return false;
        };
        let record = &mut host.record;
        match address {
            None => record.pinned_address = None,
            Some(address) => {
                if record.address != address {
                    let Some(at) = record.other_addresses.iter().position(|a| a == address) else {
                        return false;
                    };
                    let old = std::mem::replace(&mut record.address, record.other_addresses.remove(at));
                    record.other_addresses.push(old);
                }
                record.pinned_address = Some(address.to_owned());
            }
        }
        true
    }

    pub fn remove(&mut self, id: u64) {
        self.list.retain(|h| h.id != id);
    }

    /// Checks every host that is due.
    pub fn poll(&mut self) {
        let due: Vec<u64> = self
            .list
            .iter()
            .filter(|h| !h.polling && h.last_poll.is_none_or(|t| t.elapsed() >= h.poll_interval(Instant::now())))
            .map(|h| h.id)
            .collect();
        for id in due {
            self.poll_one(id);
        }
    }

    /// A quit, a closed game or a stream has ended: ask the PC what runs now, and keep asking
    /// every second for a while, since the PC changes its state after it has answered. Reads the
    /// native session and library too, because Home shows the running game from those.
    pub fn refresh_after_exit(&mut self, id: u64) {
        let Some(host) = self.get_mut(id) else { return };
        host.recheck_until = Some(Instant::now() + RECHECK_WINDOW);
        self.poll_one(id);
        self.refresh_native(id, true);
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
        host.poll_seq += 1;
        let seq = host.poll_seq;
        let record = host.record.clone();
        let likely_online = host.reach == Reach::Online;
        self.spawn("serverinfo", move || {
            let mut last_error = String::from(en("No address"));
            for address in poll_addresses(&record) {
                let result = client_for(&identity, &record, &address)
                    .and_then(|c| c.server_info(likely_online).map_err(|e| describe(&e)));
                match result {
                    Ok(info) => {
                        return Some(Event::Info {
                            id,
                            seq,
                            address,
                            result: Ok(Box::new(info)),
                        })
                    }
                    Err(e) => last_error = e,
                }
            }
            Some(Event::Info {
                id,
                seq,
                address: record.address.clone(),
                result: Err(last_error),
            })
        });
    }

    /// Starts pairing with `pin`; the host's user types it in its web UI.
    pub fn pair(&mut self, id: u64, pin: String, passphrase: Option<String>) {
        self.start_pairing(
            id,
            match passphrase {
                Some(passphrase) => Secret::OneTime { pin, passphrase },
                None => Secret::Pin(pin),
            },
        );
    }

    /// Pairs with a QR pairing link (Experimental): its one-time PIN, refused when the PC's
    /// certificate is not the link's.
    pub fn pair_link(&mut self, id: u64, link: PairingUri) {
        self.start_pairing(id, Secret::Link(Box::new(link)));
    }

    fn start_pairing(&mut self, id: u64, secret: Secret) {
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
        let one_time = !matches!(secret, Secret::Pin(_));
        self.spawn("pair", move || {
            let result = client
                .server_info_http(true)
                .and_then(|info| match &secret {
                    Secret::Pin(pin) => client.pair(&info, pin),
                    Secret::OneTime { pin, passphrase } => client.pair_with_passphrase(&info, pin, passphrase),
                    Secret::Link(link) => client.pair_with_link(&info, link),
                })
                .map_err(|e| pair_error(&e))
                .and_then(|result| pair_outcome(result, one_time));
            Some(Event::Paired { id, request, result })
        });
    }

    pub fn cancel_pairing(&mut self) {
        self.pairing_generation += 1;
        if let Some((_, client)) = self.pairing.take() {
            client.cancel_pending_requests();
        }
    }

    /// Sends the wake packets once; `run` comes back with the result (see `wol::Wakes`).
    pub fn wake(&self, id: u64, run: u64) {
        let Some(host) = self.get(id) else { return };
        let record = host.record.clone();
        self.spawn("wake", move || Some(Event::Wake { id, run, result: crate::wol::wake(&record) }));
    }

    /// Sleep PC, first step: asks the PC for the token the confirmation spends.
    pub fn sleep_offer(&self, id: u64, ask: u64) {
        self.sleep_step(id, ask, false, |client| sleep_token(client).map(|(token, at)| SleepStep::Offer { token, at }));
    }

    /// Sleep PC, confirmed: spends `token` (handed out at `at`), or a fresh one when it is old.
    pub fn sleep(&self, id: u64, ask: u64, token: String, at: Handed) {
        self.sleep_step(id, ask, true, move |client| sleep_confirmed(client, token, at));
    }

    fn sleep_step(
        &self,
        id: u64,
        ask: u64,
        confirmed: bool,
        work: impl FnOnce(&HostClient) -> Result<SleepStep, String> + Send + 'static,
    ) {
        let Some(host) = self.get(id) else { return };
        let record = host.record.clone();
        let name = host.display_name().to_owned();
        let identity = self.identity.clone();
        self.spawn("sleep", move || {
            let result = identity
                .ok_or_else(|| en("Nightjar has no client identity yet. Restart Nightjar and try again.").to_owned())
                .and_then(|identity| client_for(&identity, &record, &record.address))
                .and_then(|client| work(&client));
            Some(Event::Sleep { id, ask, name, confirmed, result })
        });
    }

    /// Asks the PC to forget this device. The local certificate stays until the PC has
    /// answered (see `drain`), so a failed or unreachable request leaves the PC paired here.
    pub fn unpair(&mut self, id: u64) -> Result<(), String> {
        let Some(identity) = self.identity.clone() else {
            return Err(en("Nightjar has no client identity yet, so it cannot unpair. Restart Nightjar and try again.").into());
        };
        let Some(host) = self.get(id) else {
            return Ok(());
        };
        let record = host.record.clone();
        self.spawn("unpair", move || {
            let result = client_for(&identity, &record, &record.address)
                .map_err(|_| UNPAIR_FAILED.to_string())
                .and_then(|client| client.unpair().map_err(|e| unpair_error(&e)));
            Some(Event::Unpaired { id, result })
        });
        Ok(())
    }

    /// The PC no longer lists this device: drop the certificate and what needed it.
    fn forget_pairing(&mut self, id: u64) {
        let Some(host) = self.get_mut(id) else { return };
        host.record.server_certificate = None;
        host.apps = Apps::NotLoaded;
        host.art.clear();
        host.art_versions.clear();
        host.native = Native::default();
        host.native_epoch += 1;
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
        host.native_seq += 1;
        let (epoch, seq) = (host.native_epoch, host.native_seq);
        let record = host.record.clone();
        self.spawn("native-api", move || {
            let result = client_for(&identity, &record, &record.address).and_then(|client| {
                let (capabilities, library) = if full {
                    (
                        Some(client.native_capabilities_with_files().map_err(|e| describe(&e))?),
                        Some(client.library().map_err(|e| describe(&e))?),
                    )
                } else {
                    (None, None)
                };
                let wake = capabilities.as_ref().and_then(|c| {
                    if !c.capabilities.features.iter().any(|f| f == feature::WAKE) {
                        return Some((Vec::new(), Vec::new()));
                    }
                    match client.wake() {
                        Ok(wake) => Some(crate::wol::targets(&wake)),
                        Err(e) => {
                            eprintln!("nightjar: wake targets not read: {}", describe(&e));
                            None
                        }
                    }
                });
                let session = client.session().map_err(|e| describe(&e));
                // Without a library in this read, a failed session is the whole failure; with
                // one, Home still has the games and shows the session error beside them.
                match (&session, full) {
                    (Err(e), false) => Err(e.clone()),
                    _ => Ok(Box::new(NativeRead { capabilities, library, session, wake })),
                }
            });
            Some(Event::Native { id, epoch, seq, result })
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
                // A PC unpaired while its read was in flight must not get its library back.
                host.record.server_certificate.as_ref()?;
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
                if std::fs::write(&cache, &bytes).is_ok() {
                    crate::config::prune_art(&cache, app);
                }
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
                Ok(false) => Err(en("The PC declined to quit the app.").into()),
                Err(nightjar_core::http::Error::Status { code: 599, .. }) => {
                    Err(en("The running app belongs to another device.").into())
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
                    return Err(en("The PC action changed. Refresh the library and try again.").into());
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
                    Ok(None) => Err(en("The PC declined the action.").into()),
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
                    seq,
                    address,
                    result,
                } => {
                    // An older answer than one already taken in describes a PC that has moved on.
                    match self.get_mut(id) {
                        Some(host) if seq < host.info_seq => continue,
                        Some(host) => host.info_seq = seq,
                        None => {}
                    }
                    changed |= self.apply_info(id, address, result);
                    self.follow_native(id);
                }
                Event::Discovered {
                    name,
                    addresses,
                    port,
                } => self.apply_discovery(name, addresses, port),
                Event::Remote { id, result } => match result {
                    Ok(Some(address)) => changed |= self.apply_remote_address(id, address),
                    Ok(None) => eprintln!("nightjar: STUN: no answer, the PC has no remote address yet"),
                    Err(e) => eprintln!("nightjar: STUN failed: {e}"),
                },
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
                Event::Native { id, epoch, seq, result } => {
                    let mut reload_apps = false;
                    let mut save = false;
                    // A read that started before an unpair says nothing about the PC now.
                    if let Some(host) = self.get_mut(id).filter(|h| h.native_epoch == epoch && h.record.server_certificate.is_some()) {
                        host.native.loading = false;
                        let newest = seq > host.action_seq;
                        match result {
                            Ok(read) => {
                                let read = *read;
                                host.native.error = None;
                                if let Some(capabilities) = read.capabilities {
                                    if let Some(info) = host.info.as_mut() {
                                        merge_native(info, Some(&capabilities));
                                    }
                                    host.native.capabilities = Some(capabilities);
                                }
                                if let Some((macs, broadcasts)) = read.wake {
                                    if host.record.wake_macs != macs || host.record.wake_broadcasts != broadcasts {
                                        host.record.wake_macs = macs;
                                        host.record.wake_broadcasts = broadcasts;
                                        changed = true;
                                    }
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
                                    // A session read before an action landed is older than its answer.
                                    Ok(_) if !newest => {}
                                    Ok(session) => host.native.session = Some(session),
                                    Err(e) => host.native.error = Some(trf("The session could not be read. {}", &[&e])),
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
                            host.native_seq += 1;
                            host.action_seq = host.native_seq;
                        }
                    }
                    self.refresh_after_exit(id);
                    out.push(Event::Session { id, action, result });
                }
                Event::Quit { id, result } => {
                    self.refresh_after_exit(id);
                    out.push(Event::Quit { id, result });
                }
                Event::Wake { id, run, result } => {
                    self.poll_one(id);
                    out.push(Event::Wake { id, run, result });
                }
                event @ Event::Sleep { .. } => out.push(event),
                Event::Action { id, result } => {
                    self.refresh_after_exit(id);
                    self.load_apps(id);
                    out.push(Event::Action { id, result });
                }
                Event::Unpaired { id, result } => {
                    // The PC may be gone by now (removed right after): nothing to tell then.
                    if self.get(id).is_some() {
                        // Unpaired, or the PC never knew this device: either way it is unpaired.
                        if now_unpaired(&result) {
                            self.forget_pairing(id);
                            changed = true;
                        }
                        out.push(Event::Unpaired { id, result });
                    }
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
        // Just after an exit the library's running flags change too: read it again every few seconds.
        let recheck = host.library_recheck_due(Instant::now());
        self.refresh_native(id, recheck);
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
                if kept.alias.is_none() {
                    kept.alias = dropped.alias;
                }
                if kept.pace.is_none() { kept.pace = dropped.pace; }
                if kept.wake_macs.is_empty() && kept.wake_broadcasts.is_empty() {
                    kept.wake_macs = dropped.wake_macs;
                    kept.wake_broadcasts = dropped.wake_broadcasts;
                }
                if kept.settings.is_none() {
                    kept.settings = dropped.settings;
                }
                for (key, settings) in dropped.app_settings {
                    kept.app_settings.entry(key).or_insert(settings);
                }
                kept.quit_on_leave.extend(dropped.quit_on_leave);
                for a in std::iter::once(dropped.address).chain(dropped.other_addresses) {
                    if a != kept.address && !kept.other_addresses.contains(&a) {
                        kept.other_addresses.push(a);
                    }
                }
                if kept.remote_address.is_none() {
                    kept.remote_address = dropped.remote_address;
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
                // A pinned PC keeps its address: an answer from another one was still in flight
                // when the user pinned it.
                if host.record.address != address && host.record.pinned_address.is_none() {
                    let old = std::mem::replace(&mut host.record.address, address.clone());
                    host.record.other_addresses.retain(|a| *a != address);
                    if !host.record.other_addresses.contains(&old) {
                        host.record.other_addresses.push(old);
                    }
                    changed = true;
                }
                if host.record.unique_id != info.unique_id {
                    // Another PC answers at this address: what the old one said and allowed is not its.
                    if !host.record.unique_id.is_empty() {
                        host.record.notices.clear();
                    }
                    host.native = Native::default();
                    host.native_epoch += 1;
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
                // The PC's own ExternalIP replaces a STUN answer (Moonlight V+ PC's NvComputer).
                if let Some(external) = info.external_ip.as_deref().and_then(public_ipv4) {
                    if host.record.remote_address.as_deref() != Some(external.as_str()) {
                        host.record.remote_address = Some(external);
                        changed = true;
                    }
                }
                // The host forgot this client: drop the pin so the user can pair again.
                if !info.paired && host.record.server_certificate.is_some() {
                    host.record.server_certificate = None;
                    host.apps = Apps::NotLoaded;
                    // Its grants (folders, files, USB, co-op) went with the pairing; a read in flight is dropped.
                    host.native = Native::default();
                    host.native_epoch += 1;
                    changed = true;
                }
                crate::logfile::add_secrets(&host.record);
                // Serverinfo alone says nothing of the native features: keep the last native read's.
                let mut info = *info;
                merge_native(&mut info, host.native.capabilities.as_ref());
                host.info = Some(info);
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
                crate::logfile::add_secrets(&host.record);
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
        if first.is_ipv4() {
            self.find_remote_address(id);
        }
        self.poll_one(id);
    }

    /// Takes a STUN answer as the PC's remote address. The PC's own ExternalIP, once it has
    /// answered with one, stays.
    fn apply_remote_address(&mut self, id: u64, address: Ipv4Addr) -> bool {
        let Some(host) = self.get_mut(id) else { return false };
        if host.info.as_ref().and_then(|i| i.external_ip.as_deref()).and_then(public_ipv4).is_some() {
            return false;
        }
        let address = address.to_string();
        if host.record.remote_address.as_deref() == Some(address.as_str()) {
            return false;
        }
        host.record.remote_address = Some(address);
        crate::logfile::add_secrets(&host.record);
        true
    }
}

/// The addresses a poll tries in turn: the last that answered, the others, then the remote one.
fn poll_addresses(record: &HostRecord) -> Vec<String> {
    if let Some(pinned) = &record.pinned_address {
        return vec![pinned.clone()];
    }
    let mut addresses = vec![record.address.clone()];
    addresses.extend(record.other_addresses.iter().cloned());
    if let Some(remote) = record.remote_address.clone().filter(|a| !addresses.contains(a)) {
        addresses.push(remote);
    }
    addresses
}

/// `text` as an IPv4 address that a PC outside this network can be reached at.
fn public_ipv4(text: &str) -> Option<String> {
    let address: Ipv4Addr = text.trim().parse().ok()?;
    let local = address.is_private() || address.is_loopback() || address.is_link_local() || address.is_unspecified()
        || address.is_broadcast() || address.is_multicast() || address.is_documentation();
    (!local).then(|| address.to_string())
}

/// The addresses a poll tries, in order: only the pinned one when the user pinned the PC
/// ([`Hosts::pin_address`]), otherwise the last that answered, then the others it was known by.
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
pub(crate) mod tests {
    use super::*;

    /// The native-api capabilities fixture with these features added, and the folder grant.
    pub(crate) fn native_caps(features: &[&str], shared_folders: bool) -> NativeCapabilities {
        let mut value: serde_json::Value =
            serde_json::from_str(include_str!("../../client/tests/fixtures/native-api/capabilities.json")).unwrap();
        let listed = value["features"].as_array_mut().unwrap();
        listed.extend(features.iter().map(|f| serde_json::Value::from(*f)));
        value["permissions"]["shared_folders"] = shared_folders.into();
        serde_json::from_value(value).unwrap()
    }

    /// A full native read of `id` with these capabilities lands, as the worker would send it.
    pub(crate) fn native_lands(hosts: &mut Hosts, id: u64, capabilities: NativeCapabilities) {
        let epoch = hosts.get(id).unwrap().native_epoch;
        let read = NativeRead { capabilities: Some(capabilities), library: Some(library()), session: Ok(session("idle")), wake: None };
        send(hosts, Event::Native { id, epoch, seq: 1, result: Ok(Box::new(read)) });
    }

    /// The PC answers serverinfo again, as every poll does.
    pub(crate) fn info_lands(hosts: &mut Hosts, id: u64, info: ServerInfo) {
        let seq = hosts.get(id).unwrap().info_seq + 1;
        let address = hosts.get(id).unwrap().record.address.clone();
        send(hosts, Event::Info { id, seq, address, result: Ok(Box::new(info)) });
    }

    /// Serverinfo of a paired PC with the native API and the clipboard permission.
    pub(crate) fn native_info() -> ServerInfo {
        native_info_of("uid", true)
    }

    /// `native_info` from the PC `unique_id`, which lists this device as paired or not.
    fn native_info_of(unique_id: &str, paired: bool) -> ServerInfo {
        let xml = format!("<?xml version=\"1.0\" encoding=\"utf-8\"?><root status_code=\"200\"><hostname>PC</hostname><uniqueid>{unique_id}</uniqueid>\
             <HttpsPort>47984</HttpsPort><ExternalPort>47989</ExternalPort><PairStatus>{}</PairStatus><currentgame>0</currentgame>\
             <state>SUNSHINE_SERVER_FREE</state><RustHostVersion>3.0.0</RustHostVersion><NativeApiVersion>1</NativeApiVersion>\
             <Permission>100859904</Permission></root>", u8::from(paired));
        ServerInfo::parse(&xml, 47989).unwrap()
    }

    #[test]
    fn grants_go_when_the_pc_forgets_this_device_or_another_pc_answers() {
        let gated = |hosts: &Hosts| {
            let info = hosts.list[0].info.as_ref().unwrap();
            [Feature::SharedFolders, Feature::Coop, Feature::ImageClipboard, Feature::FileUpload].map(|f| info.capabilities.has(f))
        };
        let extras = ["folders", "coop", "clipboard.image", "files.upload"];
        let mut granted = native_caps(&extras, true);
        granted.files.file_upload = true;
        for (answer, why) in [(native_info_of("uid", false), "the PC no longer lists this device"), (native_info_of("other", true), "another PC answers")] {
            let mut hosts = Hosts::without_discovery(vec![paired_record("uid")], Arc::new(|| {}));
            hosts.list[0].record.claim_notice(crate::config::NOTICE_PYROWAVE_FALLBACK);
            let id = hosts.list[0].id;
            info_lands(&mut hosts, id, native_info());
            native_lands(&mut hosts, id, granted.clone());
            assert_eq!(gated(&hosts), [true; 4]);
            let in_flight = hosts.list[0].native_epoch;
            info_lands(&mut hosts, id, answer);
            assert_eq!(gated(&hosts), [false; 4], "{why}: no old grant opens a row or a gate");
            assert!(hosts.list[0].native.capabilities.is_none() && hosts.list[0].native.library.is_none(), "{why}");
            // A read that started before lands now: dropped.
            let read = NativeRead { capabilities: Some(granted.clone()), library: Some(library()), session: Ok(session("idle")), wake: None };
            hosts.list[0].record.server_certificate = Some("pem".into());
            send(&mut hosts, Event::Native { id, epoch: in_flight, seq: 9, result: Ok(Box::new(read)) });
            assert!(hosts.list[0].native.capabilities.is_none(), "{why}: the old read is not taken in");
            // A notice told for the old PC is told again for another one; the same PC keeps it.
            let told = hosts.list[0].record.notices.contains(crate::config::NOTICE_PYROWAVE_FALLBACK);
            assert_eq!(told, why != "another PC answers", "{why}");
        }
    }

    /// The same PC as a stock Moonlight host sees it: no native API.
    pub(crate) fn stock_info() -> ServerInfo {
        let xml = "<?xml version=\"1.0\" encoding=\"utf-8\"?><root status_code=\"200\"><hostname>PC</hostname><uniqueid>uid</uniqueid>\
             <HttpsPort>47984</HttpsPort><ExternalPort>47989</ExternalPort><PairStatus>1</PairStatus><currentgame>0</currentgame>\
             <state>SUNSHINE_SERVER_FREE</state><Permission>100859904</Permission></root>";
        ServerInfo::parse(xml, 47989).unwrap()
    }

    pub(crate) const NATIVE_EXTRAS: &[&str] = &["folders", "coop", "clipboard.image"];

    #[test]
    fn the_native_read_reaches_every_gate_and_stays_across_polls() {
        let gated = |info: &ServerInfo| {
            [Feature::SharedFolders, Feature::Coop, Feature::ImageClipboard].map(|f| info.capabilities.has(f))
        };
        let mut hosts = Hosts::without_discovery(vec![paired_record("uid")], Arc::new(|| {}));
        let id = hosts.list[0].id;
        info_lands(&mut hosts, id, native_info());
        assert_eq!(gated(hosts.list[0].info.as_ref().unwrap()), [false; 3], "serverinfo alone offers none of them");
        native_lands(&mut hosts, id, native_caps(NATIVE_EXTRAS, true));
        assert_eq!(gated(hosts.list[0].info.as_ref().unwrap()), [true; 3], "the native read is merged in");
        info_lands(&mut hosts, id, native_info());
        assert_eq!(gated(hosts.list[0].info.as_ref().unwrap()), [true; 3], "and kept when serverinfo is read again");
        // A resume reads serverinfo again too, and merges the plan's native read the same way.
        let mut fresh = native_info();
        merge_native(&mut fresh, hosts.list[0].native.capabilities.as_ref());
        assert_eq!(gated(&fresh), [true; 3]);
        // Without the folder grant the folders stay hidden, the rest stays.
        native_lands(&mut hosts, id, native_caps(NATIVE_EXTRAS, false));
        assert_eq!(gated(hosts.list[0].info.as_ref().unwrap()), [false, true, true]);
        // A PC that no longer offers the native API offers none of them, whatever was read before.
        info_lands(&mut hosts, id, stock_info());
        assert_eq!(gated(hosts.list[0].info.as_ref().unwrap()), [false; 3]);
        let mut fresh = stock_info();
        merge_native(&mut fresh, Some(&native_caps(NATIVE_EXTRAS, true)));
        assert_eq!(gated(&fresh), [false; 3], "offered by capability, never by an old read");
    }

    // A PC's remote address: its own ExternalIP, else this device's STUN answer, tried last.
    #[test]
    fn the_remote_address_is_the_pcs_external_ip_else_stun() {
        let mut hosts = Hosts::without_discovery(vec![paired_record("uid")], Arc::new(|| {}));
        let id = hosts.list[0].id;
        send(&mut hosts, Event::Remote { id, result: Ok(Some(Ipv4Addr::new(93, 184, 216, 34))) });
        assert_eq!(hosts.list[0].record.remote_address.as_deref(), Some("93.184.216.34"));
        let record = &hosts.list[0].record;
        assert_eq!(poll_addresses(record), ["192.0.2.1", "93.184.216.34"], "after the LAN address");

        let external = |ip: &str| {
            let mut info = native_info();
            info.external_ip = Some(ip.into());
            info
        };
        info_lands(&mut hosts, id, external("10.0.0.5"));
        assert_eq!(hosts.list[0].record.remote_address.as_deref(), Some("93.184.216.34"), "a private ExternalIP is no remote address");
        info_lands(&mut hosts, id, external("151.101.1.1"));
        assert_eq!(hosts.list[0].record.remote_address.as_deref(), Some("151.101.1.1"), "the PC's own replaces STUN's");
        send(&mut hosts, Event::Remote { id, result: Ok(Some(Ipv4Addr::new(93, 184, 216, 34))) });
        assert_eq!(hosts.list[0].record.remote_address.as_deref(), Some("151.101.1.1"), "and stays over a later STUN answer");
        send(&mut hosts, Event::Remote { id, result: Err(std::io::Error::other("unreachable")) });
        assert_eq!(hosts.list[0].record.remote_address.as_deref(), Some("151.101.1.1"));
    }

    // A poll lists each address once; only a public IPv4 ExternalIP becomes the remote address.
    #[test]
    fn the_remote_address_is_listed_once_and_only_public() {
        let mut record = paired_record("uid");
        record.other_addresses = vec!["fe80::1".into()];
        record.remote_address = Some("fe80::1".into());
        assert_eq!(poll_addresses(&record), ["192.0.2.1", "fe80::1"], "listed once");
        assert_eq!(public_ipv4(" 151.101.1.1 "), Some("151.101.1.1".into()));
        for local in ["192.168.1.2", "10.1.1.1", "172.16.0.1", "127.0.0.1", "169.254.1.1", "0.0.0.0", "198.51.100.1", "x"] {
            assert_eq!(public_ipv4(local), None, "{local}");
        }
    }

    fn paired_record(unique_id: &str) -> HostRecord {
        HostRecord {
            pace: None,
            unique_id: unique_id.into(),
            name: "PC".into(),
            alias: None,
            address: "192.0.2.1".into(),
            http_port: 0,
            https_port: 0,
            other_addresses: Vec::new(),
            pinned_address: None,
            remote_address: None,
            server_certificate: Some("pem".into()),
            mac: None,
            wake_macs: Vec::new(),
            wake_broadcasts: Vec::new(),
            manual: true,
            settings: None,
            app_settings: Default::default(),
            quit_on_leave: Default::default(),
            notices: Default::default(),
        }
    }

    fn library() -> Library {
        serde_json::from_str(include_str!("../../client/tests/fixtures/native-api/library.json")).unwrap()
    }

    fn session(state: &str) -> Session {
        let text = include_str!("../../client/tests/fixtures/native-api/session.json")
            .replace("\"suspended\"", &format!("\"{state}\""));
        serde_json::from_str(&text).unwrap()
    }

    fn full_read(session: Session) -> Box<NativeRead> {
        Box::new(NativeRead { capabilities: None, library: Some(library()), session: Ok(session), wake: None })
    }

    #[test]
    fn wake_targets_follow_the_pc_and_stay_when_they_could_not_be_read() {
        let mut hosts = Hosts::without_discovery(vec![paired_record("uid")], Arc::new(|| {}));
        let id = hosts.list[0].id;
        let mut read = |wake: Option<(Vec<String>, Vec<String>)>| {
            let mut native = full_read(session("idle"));
            native.wake = wake;
            hosts.sender.send(Event::Native { id, epoch: 0, seq: 1, result: Ok(native) }).unwrap();
            let changed = hosts.drain(&egui::Context::default()).1;
            (changed, hosts.list[0].record.wake_macs.clone(), hosts.list[0].record.wake_broadcasts.clone())
        };
        let targets = (vec!["0a:00:27:00:00:05".to_string()], vec!["10.0.0.255".to_string()]);
        assert_eq!(read(Some(targets.clone())), (true, targets.0.clone(), targets.1.clone()), "kept, and saved");
        assert_eq!(read(Some(targets.clone())), (false, targets.0.clone(), targets.1.clone()), "the same ones save nothing");
        assert_eq!(read(None), (false, targets.0.clone(), targets.1.clone()), "a failed read keeps them");
        assert_eq!(read(Some((Vec::new(), Vec::new()))), (true, Vec::new(), Vec::new()), "a PC without wake has none");
    }

    fn send(hosts: &mut Hosts, event: Event) {
        hosts.sender.send(event).unwrap();
        hosts.drain(&egui::Context::default());
    }

    #[test]
    fn a_pinned_address_wins_until_the_pin_is_removed() {
        let record = HostRecord { other_addresses: vec!["10.0.0.5".into(), "fd00::5".into()], ..paired_record("uid") };
        let mut hosts = Hosts::without_discovery(vec![record], Arc::new(|| {}));
        let id = hosts.list[0].id;
        assert_eq!(poll_addresses(&hosts.list[0].record), ["192.0.2.1", "10.0.0.5", "fd00::5"], "automatic: last that answered first");
        assert!(!hosts.pin_address(id, Some("198.51.100.9")), "only a known address is pinned");
        assert!(hosts.pin_address(id, Some("fd00::5")));
        let pinned = &hosts.list[0].record;
        assert_eq!((pinned.address.as_str(), pinned.pinned_address.as_deref()), ("fd00::5", Some("fd00::5")), "streams go to it");
        assert_eq!(poll_addresses(pinned), ["fd00::5"], "the only address polled");
        // An answer from the old address, still in flight when the user pinned, keeps the pin.
        let seq = hosts.get(id).unwrap().info_seq + 1;
        send(&mut hosts, Event::Info { id, seq, address: "192.0.2.1".into(), result: Ok(Box::new(native_info())) });
        assert_eq!(hosts.list[0].record.address, "fd00::5");
        assert!(hosts.pin_address(id, None));
        let automatic = &hosts.list[0].record;
        assert_eq!(automatic.pinned_address, None);
        assert_eq!(poll_addresses(automatic), ["fd00::5", "10.0.0.5", "192.0.2.1"], "back to the automatic pick");
    }

    #[test]
    fn a_native_read_that_lands_after_unpairing_is_dropped() {
        let mut hosts = Hosts::without_discovery(vec![paired_record("uid")], Arc::new(|| {}));
        let id = hosts.list[0].id;
        // The read started while the PC was paired (epoch 0).
        send(&mut hosts, Event::Unpaired { id, result: Ok(()) });
        send(&mut hosts, Event::Native { id, epoch: 0, seq: 1, result: Ok(full_read(session("suspended"))) });
        assert!(hosts.list[0].native.library.is_none(), "the library of an unpaired PC is not taken in");
        assert!(hosts.take_library_saves().is_empty(), "and nothing is saved for it");

        // Even when it was queued for saving before the unpair, an unpaired PC is not saved.
        hosts.list[0].record.server_certificate = Some("pem".into());
        let epoch = hosts.list[0].native_epoch;
        send(&mut hosts, Event::Native { id, epoch, seq: 2, result: Ok(full_read(session("suspended"))) });
        assert!(hosts.list[0].native.library.is_some());
        hosts.list[0].record.server_certificate = None;
        assert!(hosts.take_library_saves().is_empty());
    }

    #[test]
    fn the_certificate_stays_until_the_pc_has_unpaired_the_device() {
        let mut hosts = Hosts::without_discovery(vec![paired_record("uid")], Arc::new(|| {}));
        let id = hosts.list[0].id;
        // No identity: the request cannot be made, and the user is told.
        assert!(hosts.unpair(id).is_err());
        assert!(hosts.list[0].record.server_certificate.is_some());
        // The request failed or never got through.
        send(&mut hosts, Event::Unpaired { id, result: Err(UNPAIR_FAILED.into()) });
        assert!(hosts.list[0].record.server_certificate.is_some(), "a failed unpair keeps the pairing");
        // The PC no longer lists this device, or never did.
        send(&mut hosts, Event::Unpaired { id, result: Err(UNPAIR_NOT_PAIRED.into()) });
        assert!(hosts.list[0].record.server_certificate.is_none());
        hosts.list[0].record.server_certificate = Some("pem".into());
        send(&mut hosts, Event::Unpaired { id, result: Ok(()) });
        assert!(hosts.list[0].record.server_certificate.is_none());
    }

    #[test]
    fn a_session_read_from_before_an_action_does_not_overwrite_its_answer() {
        let mut hosts = Hosts::without_discovery(vec![paired_record("uid")], Arc::new(|| {}));
        let id = hosts.list[0].id;
        // A full read starts (seq 1), then Close game lands and says the session is gone.
        hosts.list[0].native_seq = 1;
        send(&mut hosts, Event::Session { id, action: ActionId::End, result: Ok(Box::new(session("idle"))) });
        assert_eq!(hosts.list[0].native.session.as_ref().map(|s| s.state), Some(session("idle").state));
        // The older read lands afterwards: its library counts, its session does not.
        send(&mut hosts, Event::Native { id, epoch: 0, seq: 1, result: Ok(full_read(session("suspended"))) });
        assert!(hosts.list[0].native.library.is_some());
        assert_eq!(hosts.list[0].native.session.as_ref().map(|s| s.state), Some(session("idle").state));
        // A read that started after the action is newer and replaces it.
        let seq = hosts.list[0].native_seq + 1;
        send(&mut hosts, Event::Native { id, epoch: 0, seq, result: Ok(full_read(session("suspended"))) });
        assert_eq!(hosts.list[0].native.session.as_ref().map(|s| s.state), Some(session("suspended").state));
    }

    const INFO: &str = "<?xml version=\"1.0\" encoding=\"utf-8\"?><root status_code=\"200\">        <hostname>Renamed on the PC</hostname><uniqueid>uid</uniqueid><HttpsPort>47984</HttpsPort>        <ExternalPort>47989</ExternalPort><PairStatus>1</PairStatus><currentgame>0</currentgame>        <state>SUNSHINE_SERVER_FREE</state></root>";

    #[test]
    fn a_name_chosen_here_survives_the_pc_reporting_another() {
        let mut record = paired_record("uid");
        record.set_alias("Living room");
        let mut hosts = Hosts::without_discovery(vec![record], Arc::new(|| {}));
        let id = hosts.list[0].id;
        let info = ServerInfo::parse(INFO, 47989).unwrap();
        assert!(hosts.apply_info(id, "192.0.2.1".into(), Ok(Box::new(info))));
        let host = &hosts.list[0];
        assert_eq!(host.record.name, "Renamed on the PC", "the PC's own name keeps following the PC");
        assert_eq!(host.display_name(), "Living room");
        assert_eq!(hosts.records()[0].alias.as_deref(), Some("Living room"), "and the alias is saved");
        hosts.list[0].record.set_alias("");
        assert_eq!(hosts.list[0].display_name(), "Renamed on the PC");
    }

    #[test]
    fn names_a_pc_reports_or_is_given_later_are_kept_out_of_the_log() {
        let mut hosts = Hosts::without_discovery(vec![paired_record("uid")], Arc::new(|| {}));
        let id = hosts.list[0].id;
        let info = INFO.replace("Renamed on the PC", "Secret name from serverinfo").replace("<uniqueid>uid", "<uniqueid>secret-uid-from-serverinfo");
        assert!(hosts.apply_info(id, "192.0.2.1".into(), Ok(Box::new(ServerInfo::parse(&info, 47989).unwrap()))));
        assert!(hosts.rename(id, "Secret alias given here"));
        hosts.apply_discovery("Secret name from mDNS".into(), vec!["192.0.2.99".parse().unwrap()], 0);
        let line = nightjar_core::report::redact::scrub("Secret name from serverinfo, secret-uid-from-serverinfo, Secret alias given here, Secret name from mDNS");
        // Other tests save a PC named "PC": it does not mark the marks again.
        assert_eq!(line, "[PC], [PC], [PC], [PC]");
    }

    #[test]
    fn pairing_texts_follow_the_android_wording() {
        let one_time = pair_outcome(PairResult::PinWrong, true).unwrap_err();
        let plain = pair_outcome(PairResult::PinWrong, false).unwrap_err();
        assert_eq!(one_time, PAIR_OTP_PIN_WRONG);
        assert_eq!(plain, PAIR_PIN_WRONG);
        assert_eq!(pair_outcome(PairResult::AlreadyInProgress, false).unwrap_err(), PAIR_IN_PROGRESS);
        let io = nightjar_core::http::Error::Io(std::io::ErrorKind::TimedOut.into());
        assert_eq!(pair_error(&io), PAIR_NETWORK);
        assert_eq!(unpair_error(&nightjar_core::http::Error::NotPaired), UNPAIR_NOT_PAIRED);
        assert_eq!(unpair_error(&io), UNPAIR_FAILED);

        let mut hosts = Hosts::without_discovery(vec![paired_record("uid")], Arc::new(|| {}));
        assert_eq!(hosts.list[0].pairing_blocked(), None);
        hosts.list[0].reach = Reach::Offline;
        assert_eq!(hosts.list[0].pairing_blocked(), Some(PAIR_OFFLINE));
        hosts.list[0].reach = Reach::Online;
        let busy = INFO.replace("<currentgame>0", "<currentgame>7").replace("SERVER_FREE", "SERVER_BUSY");
        hosts.list[0].info = Some(ServerInfo::parse(&busy, 47989).unwrap());
        assert_eq!(hosts.list[0].pairing_blocked(), Some(PAIR_IN_GAME));
    }

    #[test]
    fn merging_two_records_of_one_pc_keeps_the_name_chosen_here() {
        let mut named = paired_record("uid");
        named.set_alias("Living room");
        named.address = "192.0.2.20".into();
        let mut plain = paired_record("uid");
        plain.server_certificate = None;
        plain.address = "192.0.2.21".into();
        for (first, second) in [(named.clone(), plain.clone()), (plain.clone(), named.clone())] {
            let mut hosts = Hosts::without_discovery(vec![first, second], Arc::new(|| {}));
            let id = hosts.list[0].id;
            let info = ServerInfo::parse(INFO, 47989).unwrap();
            assert!(hosts.apply_info(id, hosts.list[0].record.address.clone(), Ok(Box::new(info))));
            assert_eq!(hosts.list.len(), 1, "the two records are one PC");
            assert_eq!(hosts.list[0].display_name(), "Living room");
        }
    }

    fn info_event(id: u64, seq: u64, game: i32) -> Event {
        let text = INFO
            .replace("<currentgame>0", &format!("<currentgame>{game}"))
            .replace("SERVER_FREE", if game == 0 { "SERVER_FREE" } else { "SERVER_BUSY" });
        Event::Info { id, seq, address: "192.0.2.1".into(), result: Ok(Box::new(ServerInfo::parse(&text, 47989).unwrap())) }
    }

    #[test]
    fn an_older_serverinfo_answer_does_not_bring_back_a_finished_game() {
        let mut hosts = Hosts::without_discovery(vec![paired_record("uid")], Arc::new(|| {}));
        let id = hosts.list[0].id;
        send(&mut hosts, info_event(id, 1, 7));
        assert_eq!(hosts.list[0].running_app(), Some(7));
        // The poll after the quit answers first; the one from before it lands late.
        send(&mut hosts, info_event(id, 3, 0));
        send(&mut hosts, info_event(id, 2, 7));
        assert_eq!(hosts.list[0].running_app(), None, "the app list must not show the quit game as running");
    }

    #[test]
    fn a_quit_a_closed_game_and_a_stream_end_ask_the_pc_again_for_a_while() {
        let mut hosts = Hosts::without_discovery(vec![paired_record("uid")], Arc::new(|| {}));
        let id = hosts.list[0].id;
        let now = Instant::now();
        assert_eq!(hosts.list[0].poll_interval(now), POLL_INTERVAL);
        for event in [
            Event::Quit { id, result: Ok(()) },
            Event::Quit { id, result: Err("declined".into()) },
            Event::Session { id, action: ActionId::CloseGame, result: Ok(Box::new(session("idle"))) },
            Event::Action { id, result: Ok(()) },
        ] {
            hosts.list[0].recheck_until = None;
            send(&mut hosts, event);
            assert_eq!(hosts.list[0].poll_interval(Instant::now()), RECHECK_INTERVAL);
        }
        // The same entry point serves the end of a stream.
        hosts.list[0].recheck_until = None;
        hosts.refresh_after_exit(id);
        let until = hosts.list[0].recheck_until.expect("a stream's end arms the recheck");
        assert_eq!(hosts.list[0].poll_interval(until), POLL_INTERVAL, "and it ends");
    }

    #[test]
    fn the_library_is_read_again_every_few_seconds_while_rechecking_only() {
        let mut hosts = Hosts::without_discovery(vec![paired_record("uid")], Arc::new(|| {}));
        let now = Instant::now();
        let host = &mut hosts.list[0];
        host.native.full_at = Some(now);
        assert!(!host.library_recheck_due(now), "outside the window the 30 s timer rules");
        host.recheck_until = Some(now + RECHECK_WINDOW);
        assert!(!host.library_recheck_due(now + RECHECK_LIBRARY_INTERVAL / 2), "just read");
        assert!(host.library_recheck_due(now + RECHECK_LIBRARY_INTERVAL), "read again after a few seconds");
        host.native.full_at = None;
        assert!(host.library_recheck_due(now), "never read: read now");
        assert!(!host.library_recheck_due(now + RECHECK_WINDOW), "the window ends");
    }

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

    const TOKEN_A: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const TOKEN_B: &str = "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";

    /// A reply as the PC sends it: supported, allowed and available, then the token.
    fn power_reply(status: u16, message: &str, flags: [bool; 3], token: Option<&str>) -> PowerReply {
        PowerReply {
            status,
            message: message.into(),
            version: 1,
            sleep_supported: flags[0],
            sleep_allowed: flags[1],
            sleep_available: flags[2],
            active_session_policy: "idle-only".into(),
            token: token.map(Into::into),
            token_lifetime_seconds: 30,
            accepted: status == 202,
        }
    }

    /// A PC that hands out `tokens` in turn and answers each sleep request with `sleep`.
    struct FakePc {
        tokens: std::cell::RefCell<Vec<&'static str>>,
        sleep: fn() -> Result<PowerReply, PowerError>,
        calls: std::cell::RefCell<Vec<String>>,
    }

    impl FakePc {
        fn new(tokens: &[&'static str], sleep: fn() -> Result<PowerReply, PowerError>) -> Self {
            Self { tokens: std::cell::RefCell::new(tokens.to_vec()), sleep, calls: Default::default() }
        }
        fn calls(&self) -> Vec<String> {
            self.calls.borrow().clone()
        }
    }

    impl PowerHost for FakePc {
        fn power_capabilities(&self) -> Result<PowerReply, PowerError> {
            self.calls.borrow_mut().push("GET".into());
            let mut tokens = self.tokens.borrow_mut();
            let token = (!tokens.is_empty()).then(|| tokens.remove(0));
            Ok(power_reply(200, "", [true, true, token.is_some()], token))
        }
        fn sleep(&self, token: &str) -> Result<PowerReply, PowerError> {
            self.calls.borrow_mut().push(format!("POST {token}"));
            (self.sleep)()
        }
    }

    fn accepted() -> Result<PowerReply, PowerError> {
        Ok(power_reply(202, "Sleep request accepted; native completion is not confirmed", [true, true, false], None))
    }

    #[test]
    fn add_pc_reads_addresses_and_pairing_links() {
        assert_eq!(parse_entry(" 192.168.1.20:48123 "), Ok(Entry::Address("192.168.1.20".into(), Some(48123))));
        assert_eq!(parse_entry("[::1]"), Ok(Entry::Address("::1".into(), None)));
        assert_eq!(parse_entry("pc:port"), Err(ADDRESS_INVALID));
        let invitation = PairingUri {
            host: "192.168.4.2".into(),
            port: 48123,
            otp: "4821".into(),
            passphrase: "a&b=c d".into(),
            unique_id: "0123ABCD".into(),
            cert_sha256: [7; 32],
            name: "Wohnzimmer-PC".into(),
        };
        let pasted = format!("  {}\n", invitation.to_uri());
        assert_eq!(parse_entry(&pasted), Ok(Entry::Link(Box::new(invitation.clone()))));
        let v6 = PairingUri { host: "[fe80::1%eth0]".into(), ..invitation.clone() };
        let Ok(Entry::Link(link)) = parse_entry(&v6.to_uri()) else { panic!("an IPv6 link") };
        assert_eq!(link.host, "fe80::1%eth0", "kept without brackets, like a typed address");
        // A broken link is refused as a link, never added as an address.
        let short = invitation.to_uri().replace(&"07".repeat(32), &"07".repeat(31));
        assert_eq!(parse_entry(&short), Err(LINK_INVALID));
        assert_eq!(parse_entry("rubylight://pair?v=2"), Err(LINK_INVALID));
        assert_eq!(pair_error(&nightjar_core::http::Error::CertificateMismatch), PAIR_LINK_MISMATCH);
    }

    #[test]
    fn sleep_spends_the_offered_token_and_the_answer_reaches_the_ui() {
        let pc = FakePc::new(&[TOKEN_A], accepted);
        let (token, at) = sleep_token(&pc).unwrap();
        assert_eq!(token, TOKEN_A);
        assert!(matches!(sleep_confirmed(&pc, token, at), Ok(SleepStep::Accepted)));
        assert_eq!(pc.calls(), ["GET".to_owned(), format!("POST {TOKEN_A}")], "one offer, one request");

        let mut hosts = Hosts::without_discovery(vec![paired_record("uid")], Arc::new(|| {}));
        let id = hosts.list[0].id;
        hosts.sender.send(Event::Sleep { id, ask: 1, name: "PC".into(), confirmed: true, result: Ok(SleepStep::Accepted) }).unwrap();
        let (events, changed) = hosts.drain(&egui::Context::default());
        assert!(!changed);
        assert!(matches!(events[..], [Event::Sleep { confirmed: true, result: Ok(SleepStep::Accepted), .. }]));
    }

    #[test]
    fn a_token_older_than_25_seconds_is_asked_for_again_before_it_is_spent() {
        let ago = |s| {
            let now = Handed::now();
            Handed { at: now.at.checked_sub(Duration::from_secs(s)).unwrap(), wall: now.wall - Duration::from_secs(s) }
        };
        let pc = FakePc::new(&[TOKEN_B], accepted);
        assert!(sleep_confirmed(&pc, TOKEN_A.into(), ago(24)).is_ok());
        assert_eq!(pc.calls(), [format!("POST {TOKEN_A}")], "a young token is spent as it is");

        let pc = FakePc::new(&[TOKEN_B], accepted);
        assert!(sleep_confirmed(&pc, TOKEN_A.into(), ago(26)).is_ok());
        assert_eq!(pc.calls(), ["GET".to_owned(), format!("POST {TOKEN_B}")], "the fresh token is spent");

        // A PC that no longer offers a token (busy by now) says so, and nothing is spent.
        let pc = FakePc::new(&[], accepted);
        assert_eq!(sleep_confirmed(&pc, TOKEN_A.into(), ago(26)).err().as_deref(), Some(SLEEP_BUSY));
        assert_eq!(pc.calls(), ["GET"]);
    }

    #[test]
    fn a_token_is_as_old_as_either_clock_says_even_across_suspend() {
        let now = Handed::now();
        let secs = Duration::from_secs;
        // Suspended for a minute: the monotonic clock missed it, the wall clock did not.
        let suspended = Handed { at: now.at.checked_sub(secs(1)).unwrap(), wall: now.wall - secs(61) };
        let pc = FakePc::new(&[TOKEN_B], accepted);
        assert!(sleep_confirmed(&pc, TOKEN_A.into(), suspended).is_ok());
        assert_eq!(pc.calls(), ["GET".to_owned(), format!("POST {TOKEN_B}")], "the stale token is not spent");
        // The wall clock set back: the monotonic age still counts.
        let set_back = Handed { at: now.at.checked_sub(secs(26)).unwrap(), wall: now.wall + secs(3600) };
        assert!(set_back.age() >= SLEEP_TOKEN_REFRESH);
        let fresh = Handed { at: now.at.checked_sub(secs(1)).unwrap(), wall: now.wall + secs(3600) };
        assert!(fresh.age() < SLEEP_TOKEN_REFRESH);
    }

    #[test]
    fn a_refusal_shows_the_hosts_words_and_a_failed_request_says_what_failed() {
        use nightjar_core::http::{Error, Refusal};
        let busy = || Err(PowerError::Refused {
            refusal: Refusal::Conflict,
            reply: power_reply(409, "End all applications and sessions before requesting sleep", [true, true, false], None),
        });
        let pc = FakePc::new(&[TOKEN_A], busy);
        assert_eq!(
            sleep_confirmed(&pc, TOKEN_A.into(), Handed::now()).err().as_deref(),
            Some("End all applications and sessions before requesting sleep"),
            "exactly as sent"
        );
        let silent = || Err(PowerError::Refused { refusal: Refusal::Unauthorized, reply: power_reply(401, "", [false; 3], None) });
        assert_eq!(
            sleep_confirmed(&FakePc::new(&[], silent), TOKEN_A.into(), Handed::now()).err().as_deref(),
            Some("The PC refused with status 401.")
        );
        let timeout = || Err(PowerError::Host(Error::Io(std::io::ErrorKind::TimedOut.into())));
        assert_eq!(
            sleep_confirmed(&FakePc::new(&[], timeout), TOKEN_A.into(), Handed::now()).err().as_deref(),
            Some("The PC did not answer.")
        );
    }

    /// The PC's last answer, with the power fields when `power` is (supported, allowed).
    fn sleep_info(rubylight: bool, power: Option<(bool, bool)>) -> ServerInfo {
        let mut extra = String::new();
        if rubylight {
            extra.push_str("<RustHostVersion>3.0.0</RustHostVersion>");
        }
        if let Some((supported, allowed)) = power {
            extra.push_str(&format!(
                "<PowerActionVersion>1</PowerActionVersion><SleepSupported>{}</SleepSupported><SleepAllowed>{}</SleepAllowed>",
                u8::from(supported),
                u8::from(allowed)
            ));
        }
        ServerInfo::parse(&INFO.replace("</root>", &format!("{extra}</root>")), 47989).unwrap()
    }

    #[test]
    fn sleep_is_offered_only_by_a_pc_that_says_so() {
        let mut hosts = Hosts::without_discovery(vec![paired_record("uid")], Arc::new(|| {}));
        let host = &mut hosts.list[0];
        host.reach = Reach::Online;
        host.info = Some(sleep_info(false, None));
        assert_eq!(host.sleep_option(), None, "a stock host never shows Sleep PC");
        host.info = Some(sleep_info(true, None));
        assert_eq!(host.sleep_option(), None, "nor a Rubylight without the power API");
        host.info = Some(sleep_info(true, Some((false, true))));
        assert_eq!(host.sleep_option(), None, "nor a PC that cannot sleep");
        host.info = Some(sleep_info(false, Some((true, false))));
        assert_eq!(host.sleep_option(), Some(Err(SLEEP_NOT_ALLOWED)), "not for this device: shown with the reason");
        host.info = Some(sleep_info(false, Some((true, true))));
        assert_eq!(host.sleep_option(), Some(Ok(())));
        host.reach = Reach::Offline;
        assert_eq!(host.sleep_option(), None, "a PC that does not answer is not asked");
    }

    #[test]
    fn an_offer_without_a_token_says_why() {
        let reply = |flags| move || -> Result<PowerReply, PowerError> { Ok(power_reply(200, "", flags, None)) };
        struct Offer(Box<dyn Fn() -> Result<PowerReply, PowerError>>);
        impl PowerHost for Offer {
            fn power_capabilities(&self) -> Result<PowerReply, PowerError> { (self.0)() }
            fn sleep(&self, _: &str) -> Result<PowerReply, PowerError> { unreachable!("nothing to spend") }
        }
        for (flags, why) in [
            ([false, true, false], SLEEP_UNSUPPORTED),
            ([true, false, false], SLEEP_NOT_ALLOWED),
            ([true, true, false], SLEEP_BUSY),
            ([true, true, true], SLEEP_BUSY),
        ] {
            assert_eq!(sleep_token(&Offer(Box::new(reply(flags)))).err().as_deref(), Some(why));
        }
    }
}
