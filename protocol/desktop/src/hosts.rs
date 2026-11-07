//! The PCs Nightjar knows: discovery on the local network (mDNS `_nvstream._tcp`), hosts
//! added by address, their online state, pairing, the app list with box art, and quitting
//! the running app. Every host request runs on a worker thread and comes back as an
//! [`Event`]; the UI thread owns the list.

use std::collections::{BTreeMap, HashMap};
use std::net::{IpAddr, Ipv4Addr};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use nightjar_core::http::native_api::feature;
use nightjar_core::http::native_api::library::Library;
use nightjar_core::http::native_api::session::{ActionId, Session};
use nightjar_core::http::{
    App, AppControl, Availability, ClientIdentity, Feature, HostClient, HostUpdate, LateRefusal, NativeCapabilities, NativeLibrary, PairResult, PairingRequest,
    StreamMove,
    PcPower, PcPowerOffer, PowerError, PowerReply, Reason, ServerInfo, DEFAULT_HTTP_PORT,
};

use rubylight_protocol::pairing_uri::PairingUri;

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
    /// When the library's games were last played, by install id (`NativeLibrary::last_played`).
    pub last_played: BTreeMap<String, u64>,
    /// The collections the library's games are in, by install id (`NativeLibrary::collections`).
    pub collections: BTreeMap<String, Vec<String>>,
    /// The library was read from the PC in this run (not loaded from disk).
    pub library_live: bool,
    pub session: Option<Session>,
    /// Why the last read failed; the data above is then the last good one.
    pub error: Option<String>,
    /// The PC's host update, where it lists `update`.
    pub update: Option<HostUpdate>,
    /// Devices waiting to pair, as last read, where this device may approve them.
    pub pairing_requests: Vec<PairingRequest>,
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

    /// This device's permissions for a stream as last read: the stream menu's ways out and the
    /// Session and graphics page start from them. Unknown counts as no.
    pub fn session_hints(&self) -> crate::session_page::Hints {
        let permissions = self.capabilities.as_ref().map(|c| &c.capabilities.permissions);
        crate::session_page::Hints { launch: permissions.is_some_and(|p| p.launch), host_control: self.host_control() }
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
    library: Option<NativeLibrary>,
    session: Result<Session, String>,
    /// The PC's wake targets (`wol::targets`) to keep in place of the stored ones: empty for a
    /// PC without `wake`; None when they were not read, or the read failed (those adapters
    /// still belong to the PC).
    wake: Option<(Vec<String>, Vec<String>)>,
    /// `GET /update` where the PC lists `update`; None when it does not, or the read failed.
    update: Option<HostUpdate>,
    /// `GET /pairing/requests` where this device may approve; empty otherwise, or when the read failed.
    pairing_requests: Vec<PairingRequest>,
}

pub struct Host {
    pub id: u64,
    pub record: HostRecord,
    pub reach: Reach,
    pub info: Option<ServerInfo>,
    pub apps: Apps,
    pub art: HashMap<i32, Art>,
    /// Each cover's ambient, baked with it on the art thread: what the backdrop cross-fades to
    /// (`glass::CrossFade`) and baked glass samples. None while the cover loads, or without one.
    pub ambient: HashMap<i32, egui::TextureHandle>,
    /// What each cover glows with on its page, taken from its ambient.
    pub ambient_glow: HashMap<i32, egui::Color32>,
    /// The cover version each `art` entry was asked for (empty for classic box art).
    art_versions: HashMap<i32, String>,
    pub native: Native,
    /// Bumped when the PC is unpaired, so a native read that started before is dropped.
    native_epoch: u64,
    /// Counts native reads and actions, in the order they started or landed.
    native_seq: u64,
    /// `native_seq` when the last session action landed: older reads carry an older session.
    action_seq: u64,
    /// The newest read whose session was taken in.
    session_seq: u64,
    pub error: Option<String>,
    polling: bool,
    last_poll: Option<Instant>,
    /// Polls started, and the newest one whose answer was taken in: an older answer that lands
    /// later (two polls overlap after a quit) must not bring back a state the PC has left.
    poll_seq: u64,
    info_seq: u64,
    /// Until then the PC is polled every second (see `RECHECK_WINDOW`).
    recheck_until: Option<Instant>,
    /// The remote address, when the last poll reached the PC only there (this device is away
    /// from the PC's network). Never saved: the address the PC is stored under stays its own.
    via_remote: Option<String>,
    /// Whether a paired Sunshine-family PC has Foundation Sunshine's bandwidth test, asked once
    /// (`/api/network/capabilities`); None until it answered.
    foundation_probe: Option<bool>,
    probing_foundation: bool,
}

impl Host {
    /// The address requests and streams go to: the remote address while only it answers,
    /// else the PC's own.
    pub fn connect_address(&self) -> &str {
        match self.via_remote.as_deref() {
            Some(remote) if self.record.pinned_address.is_none() => remote,
            _ => &self.record.address,
        }
    }

    /// Whether Test speed runs here: Rubylight's PyroWave test, or Foundation Sunshine's where
    /// the PC said it has one. Stock Sunshine, Apollo and Vibepollo have neither.
    pub fn bandwidth_test(&self) -> bool {
        self.info.as_ref().is_some_and(|i| {
            i.capabilities.has(Feature::PyroWaveBandwidthProbe)
                || (crate::network_test::has_foundation_probe(i) && self.foundation_probe == Some(true))
        })
    }

    /// The record with [`Host::connect_address`] as its address, for a request or a stream.
    pub fn connect_record(&self) -> HostRecord {
        let mut record = self.record.clone();
        record.address = self.connect_address().to_owned();
        record
    }

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

    pub fn session_reads(&self) -> crate::open_play::SessionReads {
        crate::open_play::SessionReads { asked: self.native_seq, answered: self.session_seq }
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

    /// Restart PC or Shut down PC for a paired PC that answers: None when the PC does not offer
    /// it, `Some(Err(why))` when it does but not for this device.
    pub fn pc_power_option(&self, action: PcPower) -> Option<Result<(), &'static str>> {
        if self.reach != Reach::Online || !self.paired() {
            return None;
        }
        let native = self.native.capabilities.as_ref()?;
        if !native.capabilities.features.iter().any(|f| f == action.feature()) {
            return None;
        }
        Some(if native.files.restart_shutdown { Ok(()) } else { Err(PC_POWER_NOT_ALLOWED) })
    }

    /// Pair another device: listed on every PC that offers it, off with its reason on a device
    /// without the permission, so that it can be found.
    pub fn pair_another_option(&self) -> Option<Result<(), &'static str>> {
        if self.reach != Reach::Online || !self.paired() {
            return None;
        }
        let native = self.native.capabilities.as_ref()?;
        if !native.offers_pairing_approve() {
            return None;
        }
        Some(if native.may_approve_pairing() { Ok(()) } else { Err(PAIRING_NOT_ALLOWED) })
    }

    /// Whether this device may approve devices waiting to pair on this PC, as last read.
    pub fn may_approve_pairing(&self) -> bool {
        self.reach == Reach::Online
            && self.paired()
            && self.native.capabilities.as_ref().is_some_and(NativeCapabilities::may_approve_pairing)
    }

    /// The devices waiting on this PC that this device may approve.
    pub fn waiting_to_pair(&self) -> &[PairingRequest] {
        if self.may_approve_pairing() { &self.native.pairing_requests } else { &[] }
    }

    /// The version "Update the PC's host" installs: only where the PC lists `update`, has one
    /// to install, and lets this device control it. Not while it installs or is queued as an
    /// Install now already; an automatic update waiting for an idle PC still takes one.
    pub fn update_offer(&self) -> Option<&str> {
        if self.reach != Reach::Online || !self.paired() || !self.native.host_control() {
            return None;
        }
        let update = self.native.update.as_ref()?;
        if update.installing_now() {
            return None;
        }
        update.offered()
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
// Restart and shut down. A refusal shows the host's own words instead.
pub const PC_POWER_NOT_ALLOWED: &str = en("The PC does not let this device restart or shut it down. Allow it in the host's device settings.");
pub const PC_POWER_BUSY: &str = en("The PC cannot do this right now. Close games and streams on it, then try again.");
// Approving a new device, without the permission for it. The console's own names, in every language.
pub const PAIRING_NOT_ALLOWED: &str = en("Needs Approve new devices on the host's Devices page.");

/// How long selecting a game waits to learn who plays it, before it starts the stream as it always did.
const SELECT_READ_WAIT: Duration = Duration::from_millis(1500);
/// How often an open "Now playing on ..." asks the PC whether the move failed.
const MOVE_WATCH_EVERY: Duration = Duration::from_secs(3);
/// How many times it asks: past the two minutes the PC keeps its note, even one that never answers.
pub const MOVE_WATCH_READS: u32 = 50;

/// `work`'s answer, or an error when it takes longer than `wait`. It then ends on its own.
fn within<T: Send + 'static>(wait: Duration, work: impl FnOnce() -> Result<T, String> + Send + 'static) -> Result<T, String> {
    let (sender, answer) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(work());
    });
    answer.recv_timeout(wait).unwrap_or_else(|_| Err(format!("no answer within {} ms", wait.as_millis())))
}

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
    /// Whether the PC `unique_id` has Foundation Sunshine's bandwidth test; None when the
    /// question did not get through.
    FoundationProbe {
        id: u64,
        unique_id: String,
        found: Option<bool>,
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
        ambient: Option<egui::ColorImage>,
    },
    Native {
        id: u64,
        epoch: u64,
        seq: u64,
        result: Result<Box<NativeRead>, String>,
    },
    /// Who plays the running game, read for selecting `app`; None when the PC did not say in time.
    SelectRead {
        id: u64,
        app: i32,
        resume: bool,
        /// The user pressed Stream here too on "did not move".
        too: bool,
        read: Option<StreamMove>,
    },
    StreamMoved {
        id: u64,
        app: i32,
        /// The device the stream moved to; None when it did not, or the PC could not say.
        device: Option<String>,
    },
    MoveWatched {
        /// The prompt that asked: see [`crate::move_watch`].
        token: u64,
        id: u64,
        app: i32,
        read: Result<StreamMove, String>,
    },
    ContinueHere {
        id: u64,
        app: i32,
        result: Result<(), String>,
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
    /// A Restart PC or Shut down PC step, as [`Event::Sleep`].
    PcPower {
        id: u64,
        ask: u64,
        name: String,
        action: PcPower,
        confirmed: bool,
        result: Result<PcPowerStep, String>,
    },
    /// The PC answered an approval or a denial of a device waiting to pair.
    PairingAnswer {
        id: u64,
        request: String,
        name: String,
        denied: bool,
        result: Result<(), String>,
    },
    /// The PC's host update: `Started` once the PC took the request, then `Back` once it
    /// answers with the new version, or why either failed.
    HostUpdate {
        id: u64,
        name: String,
        result: Result<UpdateStep, String>,
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
    /// The certificate a pairing link named, for each record it was opened on.
    link_claims: HashMap<u64, [u8; 32]>,
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

/// Whether this record is paired with a certificate other than `certificate`. One that cannot be
/// read is another one.
fn pins_another(record: &HostRecord, certificate: &[u8; 32]) -> bool {
    record
        .server_certificate
        .as_deref()
        .is_some_and(|pem| nightjar_core::http::certificate_sha256(pem).ok().as_ref() != Some(certificate))
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
    if PairingUri::is_link(text) {
        return parse_link(text).map(Entry::Link);
    }
    crate::config::parse_address(text)
        .map(|(address, port)| Entry::Address(address, port))
        .ok_or(ADDRESS_INVALID)
}

/// Reads a pairing link; anything else is refused with [`LINK_INVALID`].
pub fn parse_link(text: &str) -> Result<Box<PairingUri>, &'static str> {
    let mut link = PairingUri::parse(text.trim()).ok_or(LINK_INVALID)?;
    // Records keep IPv6 addresses without brackets, as typed ones are.
    if let Some(bare) = link.host.strip_prefix('[').and_then(|h| h.strip_suffix(']')) {
        link.host = bare.to_owned();
    }
    Ok(Box::new(link))
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

pub enum PcPowerStep {
    /// The PC offers it: confirm, then spend `token`. `wake_from_off`: a wired wake target is
    /// ready, so the PC can likely be woken after a shutdown.
    Offer { token: String, at: Handed, wake_from_off: bool },
    /// The PC took the request (202).
    Accepted,
    /// After the 202, Windows did not confirm it in time: the PC may still go down. The host's words.
    Unconfirmed(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdateStep {
    /// The host is installing `version`.
    Started(String),
    /// The install waits for another device's stream, in the host's words.
    Waiting(String),
    /// The PC answers with `version` again.
    Back(String),
}

/// How long an update may take before Nightjar stops waiting for the PC to come back. Time
/// spent waiting for another device's stream does not count.
const UPDATE_WAIT: Duration = Duration::from_secs(15 * 60);
/// The whole wait, another device's stream included: long enough for a film or a long session
/// on the TV to end and the update to go in, short enough that a forgotten wait does not poll the
/// PC for ever. Android's HostUpdateFlow.MAX_WAIT_MS matches.
const UPDATE_MAX_WAIT: Duration = Duration::from_secs(3 * 60 * 60);
const UPDATE_POLL: Duration = Duration::from_secs(5);
/// After the PC took a restart or shutdown, how often and how many times Nightjar reads
/// `GET /power` for a refusal that came after the answer.
const POWER_REFUSAL_POLL: Duration = Duration::from_secs(2);
const POWER_REFUSAL_READS: u32 = 10;
const UPDATE_GONE: &str = en("The PC has no update to install.");
const UPDATE_FAILED: &str = en("The update failed on the PC.");

/// The calls the update wait makes; a fake PC in the tests.
trait UpdateHost {
    fn host_update(&self) -> Result<HostUpdate, nightjar_core::http::Error>;
    /// The host version `/serverinfo` answers with, None while the PC does not answer.
    fn running_version(&self) -> Option<String>;
}

impl UpdateHost for HostClient {
    fn host_update(&self) -> Result<HostUpdate, nightjar_core::http::Error> {
        HostClient::host_update(self)
    }
    fn running_version(&self) -> Option<String> {
        self.server_info(false).ok().and_then(|info| info.capabilities.host_version().map(str::to_owned))
    }
}

/// The version an accepted install brings, from its 202 answer; none means there is nothing to
/// wait for.
fn update_target(update: &HostUpdate) -> Result<String, String> {
    update.offered().map(str::to_owned).ok_or_else(|| UPDATE_GONE.to_owned())
}

/// Follows an accepted install until the PC is back on `target`: the updater's failure as it
/// says it, a PC back on another version, a wait for another device's stream (`said`, once
/// per reason), or `polls` reads without either, and `most` reads in all. `pause` sleeps between
/// reads.
fn follow_update(
    host: &impl UpdateHost,
    target: &str,
    polls: u32,
    most: u32,
    pause: impl Fn(),
    mut said: impl FnMut(UpdateStep),
) -> Result<UpdateStep, String> {
    let (mut counted, mut gone, mut waiting) = (0, false, None::<String>);
    for _ in 0..most {
        if counted >= polls {
            break;
        }
        pause();
        if host.running_version().is_some_and(|v| nightjar_core::http::update_arrived(&v, target)) {
            return Ok(UpdateStep::Back(target.to_owned()));
        }
        let update = match host.host_update() {
            Ok(update) => update,
            Err(_) => {
                gone = true;
                counted += 1;
                continue;
            }
        };
        if let Some(error) = update.failure() {
            return Err(error.unwrap_or(UPDATE_FAILED).to_owned());
        }
        // Back after going away, on another version and with nothing under way: setup gave up.
        if gone && update.gave_up(target) {
            let back = trf("The PC came back on version {}; the update did not install.", &[&update.current_version]);
            return Err(update.error.map_or(back.clone(), |e| format!("{back} {e}")));
        }
        if update.waiting_for != waiting {
            waiting = update.waiting_for.clone();
            if let Some(reason) = &waiting {
                said(UpdateStep::Waiting(reason.clone()));
            }
        }
        if waiting.is_none() {
            counted += 1;
        }
    }
    Err(trf("The PC did not come back with version {} in time.", &[&target]))
}

/// The native calls Restart PC and Shut down PC make; a fake PC in the tests.
trait PcPowerHost {
    fn pc_power(&self) -> Result<PcPowerOffer, nightjar_core::http::Error>;
    fn power_pc(&self, action: PcPower, token: &str) -> Result<(), nightjar_core::http::Error>;
}

impl PcPowerHost for HostClient {
    fn pc_power(&self) -> Result<PcPowerOffer, nightjar_core::http::Error> {
        HostClient::pc_power(self)
    }
    fn power_pc(&self, action: PcPower, token: &str) -> Result<(), nightjar_core::http::Error> {
        HostClient::power_pc(self, action, token)
    }
}

/// A refusal in the host's own words; anything else said like the other host errors.
fn native_error(error: &nightjar_core::http::Error) -> String {
    nightjar_core::http::host_words(error).unwrap_or_else(|| describe(error))
}

/// Asks the PC for a token for `action`, timed as [`sleep_token`].
fn pc_power_token(host: &impl PcPowerHost, action: PcPower) -> Result<PcPowerStep, String> {
    let at = Handed::now();
    let offer = host.pc_power().map_err(|e| native_error(&e))?;
    match offer.token_for(action) {
        Ok(token) => Ok(PcPowerStep::Offer { token: token.to_owned(), at, wake_from_off: offer.wake_from_off }),
        Err(reason) if reason.is_empty() => Err(PC_POWER_BUSY.into()),
        Err(reason) => Err(reason),
    }
}

/// Spends the confirmed token, asking for a new one first when it is about to expire.
fn pc_power_confirmed(host: &impl PcPowerHost, action: PcPower, token: String, at: Handed) -> Result<PcPowerStep, String> {
    let token = match at.age() >= nightjar_core::http::POWER_TOKEN_REFRESH {
        true => match pc_power_token(host, action)? {
            PcPowerStep::Offer { token, .. } => token,
            PcPowerStep::Accepted | PcPowerStep::Unconfirmed(_) => token,
        },
        false => token,
    };
    host.power_pc(action, &token).map_err(|e| native_error(&e))?;
    Ok(PcPowerStep::Accepted)
}

/// After the PC took the request: why it did not go ahead after all (Windows refused, or the PC
/// got busy) or may not have (Windows did not confirm it in time), read from `GET /power` up to
/// `reads` times; None once the PC stops answering or never says.
fn pc_power_refusal(host: &impl PcPowerHost, reads: u32, pause: impl Fn()) -> Option<LateRefusal> {
    for _ in 0..reads {
        pause();
        match host.pc_power() {
            Ok(offer) => {
                if let Some(refusal) = offer.late_refusal() {
                    return Some(refusal);
                }
            }
            // Going down.
            Err(_) => return None,
        }
    }
    None
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
            link_claims: HashMap::new(),
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
            ambient: HashMap::new(),
            ambient_glow: HashMap::new(),
            art_versions: HashMap::new(),
            native: Native::default(),
            native_epoch: 0,
            native_seq: 0,
            action_seq: 0,
            session_seq: 0,
            error: None,
            polling: false,
            last_poll: None,
            poll_seq: 0,
            info_seq: 0,
            recheck_until: None,
            via_remote: None,
            foundation_probe: None,
            probing_foundation: false,
        });
        id
    }

    pub fn get(&self, id: u64) -> Option<&Host> {
        self.list.iter().find(|h| h.id == id)
    }

    /// The poll of the PC in flight, if one is.
    pub fn polling_seq(&self, id: u64) -> Option<u64> {
        self.get(id).filter(|host| host.polling).map(|host| host.poll_seq)
    }

    /// Once poll `seq` (or a later one) answered: the reason the PC gave for this device's
    /// latest failed stream, `Some(None)` when it gave none or could not be asked. `None` while
    /// the poll is still out.
    pub fn failure_after(&self, id: u64, seq: u64) -> Option<Option<String>> {
        let Some(host) = self.get(id) else {
            return Some(None);
        };
        (host.info_seq >= seq).then(|| {
            host.info
                .as_ref()
                .filter(|_| host.error.is_none())
                .and_then(|info| info.last_failure.clone())
        })
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
        // The demo library never reaches for a network, even by accident.
        if cfg!(feature = "demo-library") {
            return;
        }
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
        self.link_claims.remove(&id);
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
        if self.get(id).is_some_and(|host| pins_another(&host.record, &link.cert_sha256)) {
            self.refuse_link(id);
            return;
        }
        self.link_claims.insert(id, link.cert_sha256);
        self.start_pairing(id, Secret::Link(Box::new(link)));
    }

    /// Ends the pairing at `id`, if one runs, with what a link to another certificate gets.
    fn refuse_link(&mut self, id: u64) {
        self.cancel_pairing();
        let result = Err(PAIR_LINK_MISMATCH.into());
        let _ = self.sender.send(Event::Paired { id, request: self.pairing_generation, result });
        (self.waker)();
    }

    /// `id` was added for a link and now says it is a PC known here with a certificate other than
    /// the link names. Any web page can make that address the one tried, and nothing has proved
    /// who answered, so the known PC keeps its address and this record goes.
    fn drop_link_claiming_known_pc(&mut self, id: u64) {
        eprintln!("nightjar: a pairing link's address claims a known PC with another certificate");
        if self.pairing.as_ref().is_some_and(|(pairing, _)| *pairing == id) {
            self.refuse_link(id);
        }
        self.remove(id);
    }

    fn start_pairing(&mut self, id: u64, secret: Secret) {
        self.cancel_pairing();
        let request = self.pairing_generation;
        // The PCs a link's certificate cannot be: known with another one. The PIN is not spent on
        // something that says it is one of them.
        let known_otherwise: Vec<String> = match &secret {
            Secret::Link(link) => self
                .list
                .iter()
                .filter(|host| !host.record.unique_id.is_empty() && pins_another(&host.record, &link.cert_sha256))
                .map(|host| host.record.unique_id.clone())
                .collect(),
            _ => Vec::new(),
        };
        let Some(identity) = self.identity.clone() else {
            return;
        };
        let Some(host) = self.get(id) else {
            return;
        };
        let mut record = host.connect_record();
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
                    Secret::Link(_) if known_otherwise.contains(&info.unique_id) => Err(nightjar_core::http::Error::CertificateMismatch),
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
        let record = host.connect_record();
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

    /// Restart PC or Shut down PC, first step: asks the PC for the token the confirmation spends.
    pub fn pc_power_offer(&self, id: u64, ask: u64, action: PcPower) {
        self.pc_power_step(id, ask, action, false, move |client| pc_power_token(client, action));
    }

    /// Restart PC or Shut down PC, confirmed: spends `token`, or a fresh one when it is old.
    pub fn pc_power(&self, id: u64, ask: u64, action: PcPower, token: String, at: Handed) {
        self.pc_power_step(id, ask, action, true, move |client| pc_power_confirmed(client, action, token, at));
    }

    fn pc_power_step(
        &self,
        id: u64,
        ask: u64,
        action: PcPower,
        confirmed: bool,
        work: impl FnOnce(&HostClient) -> Result<PcPowerStep, String> + Send + 'static,
    ) {
        let Some(host) = self.get(id) else { return };
        let record = host.connect_record();
        let name = host.display_name().to_owned();
        let identity = self.identity.clone();
        let sender = self.sender.clone();
        let waker = self.waker.clone();
        self.spawn("pc-power", move || {
            let client = identity
                .ok_or_else(|| en("Nightjar has no client identity yet. Restart Nightjar and try again.").to_owned())
                .and_then(|identity| client_for(&identity, &record, &record.address));
            let result = client.as_ref().map_err(Clone::clone).and_then(work);
            let client = match (client, matches!(result, Ok(PcPowerStep::Accepted))) {
                (Ok(client), true) => client,
                _ => return Some(Event::PcPower { id, ask, name, action, confirmed, result }),
            };
            // Taken; Windows may still refuse once the answer has left.
            let _ = sender.send(Event::PcPower { id, ask, name: name.clone(), action, confirmed, result });
            waker();
            let result = match pc_power_refusal(&client, POWER_REFUSAL_READS, || std::thread::sleep(POWER_REFUSAL_POLL))? {
                LateRefusal::Refused(refusal) => Err(refusal),
                LateRefusal::Unconfirmed(said) => Ok(PcPowerStep::Unconfirmed(said)),
            };
            Some(Event::PcPower { id, ask, name, action, confirmed, result })
        });
    }

    /// Installs the PC's host update, then waits for the PC to answer with the new version.
    pub fn install_update(&self, id: u64) {
        let Some(host) = self.get(id) else { return };
        let record = host.connect_record();
        let name = host.display_name().to_owned();
        let identity = self.identity.clone();
        let sender = self.sender.clone();
        let waker = self.waker.clone();
        self.spawn("host-update", move || {
            let client = match identity
                .ok_or_else(|| en("Nightjar has no client identity yet. Restart Nightjar and try again.").to_owned())
                .and_then(|identity| client_for(&identity, &record, &record.address))
            {
                Ok(client) => client,
                Err(e) => return Some(Event::HostUpdate { id, name, result: Err(e) }),
            };
            let target = match client.install_host_update().map_err(|e| native_error(&e)).and_then(|u| update_target(&u)) {
                Ok(target) => target,
                Err(e) => return Some(Event::HostUpdate { id, name, result: Err(e) }),
            };
            let say = |step| {
                let _ = sender.send(Event::HostUpdate { id, name: name.clone(), result: Ok(step) });
                waker();
            };
            say(UpdateStep::Started(target.clone()));
            let polls = (UPDATE_WAIT.as_secs() / UPDATE_POLL.as_secs()) as u32;
            let most = (UPDATE_MAX_WAIT.as_secs() / UPDATE_POLL.as_secs()) as u32;
            let result = follow_update(&client, &target, polls, most, || std::thread::sleep(UPDATE_POLL), say);
            Some(Event::HostUpdate { id, name, result })
        });
    }

    /// Sends the PIN a waiting device shows. The answer takes as long as that device's handshake.
    pub fn approve_pairing(&self, id: u64, request: String, name: String, pin: String) {
        self.answer_pairing(id, request, name, false, move |client, request| {
            client.approve_pairing(request, &pin).map(|_| ())
        });
    }

    /// Ends a waiting device's request.
    pub fn deny_pairing(&self, id: u64, request: String, name: String) {
        self.answer_pairing(id, request, name, true, |client, request| client.deny_pairing(request).map(|_| ()));
    }

    fn answer_pairing(
        &self,
        id: u64,
        request: String,
        name: String,
        denied: bool,
        call: impl FnOnce(&HostClient, &str) -> Result<(), nightjar_core::http::Error> + Send + 'static,
    ) {
        let Some(host) = self.get(id) else { return };
        let record = host.connect_record();
        let identity = self.identity.clone();
        self.spawn("approve-device", move || {
            let result = identity
                .ok_or_else(|| en("Nightjar has no client identity yet. Restart Nightjar and try again.").to_owned())
                .and_then(|identity| client_for(&identity, &record, &record.address))
                .and_then(|client| call(&client, &request).map_err(|e| native_error(&e)));
            Some(Event::PairingAnswer { id, request, name, denied, result })
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
        let record = host.connect_record();
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
        host.ambient.clear();
        host.ambient_glow.clear();
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
        let record = host.connect_record();
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
        let approves = host.native.capabilities.as_ref().is_some_and(NativeCapabilities::may_approve_pairing);
        host.native_seq += 1;
        let (epoch, seq) = (host.native_epoch, host.native_seq);
        let record = host.connect_record();
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
                let update = capabilities.as_ref().and_then(|c| {
                    if !c.capabilities.features.iter().any(|f| f == nightjar_core::http::FEATURE_UPDATE) {
                        return None;
                    }
                    client.host_update().map_err(|e| eprintln!("nightjar: host update not read: {}", describe(&e))).ok()
                });
                let pairing_requests = if capabilities.as_ref().map_or(approves, NativeCapabilities::may_approve_pairing) {
                    client.pairing_requests().map(|r| r.requests).unwrap_or_else(|e| {
                        eprintln!("nightjar: devices waiting to pair not read: {}", describe(&e));
                        Vec::new()
                    })
                } else {
                    Vec::new()
                };
                let session = client.session().map_err(|e| describe(&e));
                // Without a library in this read, a failed session is the whole failure; with
                // one, Home still has the games and shows the session error beside them.
                match (&session, full) {
                    (Err(e), false) => Err(e.clone()),
                    _ => Ok(Box::new(NativeRead { capabilities, library, session, wake, update, pairing_requests })),
                }
            });
            Some(Event::Native { id, epoch, seq, result })
        });
    }

    /// Whether this PC offers moving its game's stream between devices.
    pub fn offers_continue_here(&self, id: u64) -> bool {
        self.get(id).and_then(|host| host.info.as_ref()).is_some_and(|info| {
            info.capabilities.availability(Feature::ContinueHere) == Availability::Supported
        })
    }

    /// After a stream ended, unless the player left it on purpose: asks the PC whether it moved
    /// to another device.
    pub fn read_stream_move(&mut self, id: u64, app: i32, left_on_purpose: bool) {
        if !nightjar_policy::continue_here::reads_move_after_end(self.offers_continue_here(id), left_on_purpose, false) {
            return;
        }
        let (Some(identity), Some(host)) = (self.identity.clone(), self.get(id)) else {
            return;
        };
        let record = host.connect_record();
        self.spawn("stream-move", move || {
            let read = client_for(&identity, &record, &record.address)
                .and_then(|client| client.stream_move().map_err(|e| describe(&e)));
            let device = match read {
                Ok(read) => read.moved_to.map(|moved| moved.device),
                Err(e) => {
                    eprintln!("nightjar: could not ask the PC whether the stream moved: {e}");
                    None
                }
            };
            Some(Event::StreamMoved { id, app, device })
        });
    }

    /// While "Now playing on ..." is open: reads the PC once after [`MOVE_WATCH_EVERY`], off the UI
    /// thread, and answers with [`Event::MoveWatched`]. The next read is the answer's to start, so
    /// a closed prompt ends this. False when it cannot ask, and no answer comes.
    pub fn watch_stream_move(&mut self, id: u64, app: i32, token: u64) -> bool {
        let (Some(identity), Some(host)) = (self.identity.clone(), self.get(id)) else {
            return false;
        };
        let record = host.connect_record();
        self.spawn("stream-move-watch", move || {
            std::thread::sleep(MOVE_WATCH_EVERY);
            let read = client_for(&identity, &record, &record.address)
                .and_then(|client| client.stream_move().map_err(|e| describe(&e)));
            Some(Event::MoveWatched { token, id, app, read })
        });
        true
    }

    /// Selecting a game on a PC that offers the move: reads who plays it now, off the UI thread and
    /// for at most [`SELECT_READ_WAIT`], and answers with [`Event::SelectRead`]. False when it cannot
    /// ask, and no answer comes.
    /// `too`: the user pressed Stream here too on "did not move" (`continue_here::joins_after_read`).
    pub fn read_before_start(&mut self, id: u64, app: i32, resume: bool, too: bool) -> bool {
        let (Some(identity), Some(host)) = (self.identity.clone(), self.get(id)) else {
            return false;
        };
        let record = host.connect_record();
        self.spawn("continue-here-read", move || {
            let read = within(SELECT_READ_WAIT, move || {
                client_for(&identity, &record, &record.address).and_then(|client| client.stream_move().map_err(|e| describe(&e)))
            });
            let read = match read {
                Ok(read) => Some(read),
                Err(e) => {
                    eprintln!("nightjar: could not read who plays the game; starting the stream: {e}");
                    None
                }
            };
            Some(Event::SelectRead { id, app, resume, too, read })
        });
        true
    }

    /// Continue here: the other device's stream of the running game ends. The launch follows
    /// the PC's answer.
    pub fn continue_here(&mut self, id: u64, app: i32) {
        let (Some(identity), Some(host)) = (self.identity.clone(), self.get(id)) else {
            return;
        };
        let record = host.connect_record();
        self.spawn("continue-here", move || {
            let result = client_for(&identity, &record, &record.address)
                .and_then(|client| client.continue_here().map(|_| ()).map_err(|e| describe(&e)));
            Some(Event::ContinueHere { id, app, result })
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
        let record = host.connect_record();
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
                let saved = store.library(&host.record.unique_id);
                host.native.last_played = saved.as_ref().map(|l| l.last_played.clone()).unwrap_or_default();
                host.native.collections = saved.as_ref().map(|l| l.collections.clone()).unwrap_or_default();
                host.native.library = saved.map(|l| l.library);
            }
        }
    }

    /// The libraries read since the last call, by the PC's `uniqueid`, to be saved.
    pub fn take_library_saves(&mut self) -> Vec<(String, NativeLibrary)> {
        std::mem::take(&mut self.library_saves)
            .into_iter()
            .filter_map(|id| {
                let host = self.get(id)?;
                // A PC unpaired while its read was in flight must not get its library back.
                host.record.server_certificate.as_ref()?;
                let library = NativeLibrary { library: host.native.library.clone()?, last_played: host.native.last_played.clone(), collections: host.native.collections.clone() };
                Some((host.record.unique_id.clone(), library))
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
            host.ambient.remove(&app);
            host.ambient_glow.remove(&app);
        }
        if host.art.contains_key(&app) {
            return;
        }
        host.art.insert(app, Art::Loading);
        host.art_versions.insert(app, version.to_owned());
        let version = version.to_owned();
        let record = host.connect_record();
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
            let (image, ambient) = art_and_ambient(bytes);
            Some(Event::Art { id, app, version, image, ambient })
        });
    }

    pub fn quit_app(&mut self, id: u64) {
        let Some(identity) = self.identity.clone() else {
            return;
        };
        let Some(host) = self.get(id) else {
            return;
        };
        let record = host.connect_record();
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
        let record = host.connect_record();
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
                    self.check_foundation_probe(id);
                }
                Event::FoundationProbe { id, unique_id, found } => {
                    if let Some(host) = self.get_mut(id) {
                        host.probing_foundation = false;
                        // Another PC answers at this address by now: the answer is not its.
                        if host.record.unique_id == unique_id {
                            host.foundation_probe = found;
                        }
                    }
                }
                Event::Discovered {
                    name,
                    addresses,
                    port,
                } => changed |= self.apply_discovery(name, addresses, port),
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
                Event::Art { id, app, version, image, ambient } => {
                    if let Some(host) = self.get_mut(id).filter(|h| h.art_versions.get(&app) == Some(&version)) {
                        match ambient {
                            Some(ambient) => {
                                host.ambient_glow.insert(app, crate::ambient::glow_of(&ambient));
                                host.ambient.insert(app, ctx.load_texture(format!("ambient-{id}-{app}"), ambient, egui::TextureOptions::LINEAR));
                            }
                            None => {
                                host.ambient_glow.remove(&app);
                                host.ambient.remove(&app);
                            }
                        }
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
                                host.native.pairing_requests = read.pairing_requests;
                                if let Some(capabilities) = read.capabilities {
                                    host.native.update = read.update;
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
                                if let Some(NativeLibrary { library, last_played, collections }) = read.library {
                                    // The classic list a launch goes through follows the library.
                                    reload_apps = !host.native.library_live
                                        || host.native.library.as_ref().is_none_or(|old| old.revision != library.revision);
                                    save = host.native.library.as_ref() != Some(&library)
                                        || host.native.last_played != last_played
                                        || host.native.collections != collections;
                                    host.native.library = Some(library);
                                    host.native.last_played = last_played;
                                    host.native.collections = collections;
                                    host.native.library_live = true;
                                }
                                match read.session {
                                    // A session read before an action landed is older than its answer.
                                    Ok(_) if !newest => {}
                                    Ok(session) => {
                                        host.native.session = Some(session);
                                        host.session_seq = seq;
                                    }
                                    Err(e) => host.native.error = Some(trf("The session could not be read. {}", &[&e])),
                                }
                            }
                            Err(e) => {
                                host.native.pairing_requests.clear();
                                host.native.error = Some(e);
                            }
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
                event @ Event::PcPower { .. } => out.push(event),
                Event::PairingAnswer { id, request, name, denied, result } => {
                    // Whatever the answer, that request is over; the device asks again with a new PIN.
                    if let Some(host) = self.get_mut(id) {
                        host.native.pairing_requests.retain(|r| r.id != request);
                    }
                    self.refresh_native(id, false);
                    out.push(Event::PairingAnswer { id, request, name, denied, result });
                }
                event @ Event::SelectRead { .. } => out.push(event),
                event @ Event::StreamMoved { .. } => out.push(event),
                event @ Event::MoveWatched { .. } => out.push(event),
                event @ Event::ContinueHere { .. } => out.push(event),
                Event::HostUpdate { id, name, result } => {
                    if matches!(result, Ok(UpdateStep::Back(_))) {
                        self.poll_one(id);
                    }
                    // Under way: the menu offers no second install until the next read says.
                    if let (Ok(UpdateStep::Started(_)), Some(update)) =
                        (&result, self.get_mut(id).and_then(|host| host.native.update.as_mut()))
                    {
                        update.phase = "installing".into();
                    }
                    out.push(Event::HostUpdate { id, name, result });
                }
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

    /// Asks a paired Sunshine-family PC once whether it has Foundation Sunshine's bandwidth test,
    /// so Test speed is offered only where it runs. A request that did not get through is asked
    /// again after the next poll.
    fn check_foundation_probe(&mut self, id: u64) {
        let Some(identity) = self.identity.clone() else { return };
        let Some(host) = self.get_mut(id) else { return };
        let candidate = host.info.as_ref().is_some_and(|i| {
            !i.capabilities.has(Feature::PyroWaveBandwidthProbe) && crate::network_test::has_foundation_probe(i)
        });
        if host.reach != Reach::Online || !candidate || host.foundation_probe.is_some() || host.probing_foundation {
            return;
        }
        host.probing_foundation = true;
        let record = host.connect_record();
        self.spawn("network-capabilities", move || {
            let found = match client_for(&identity, &record, &record.address).map(|c| c.network_capabilities()) {
                Ok(Ok(capabilities)) => match capabilities.probe_bytes() {
                    Ok(_) => Some(true),
                    Err(e) => {
                        eprintln!("nightjar: the PC's bandwidth test is not offered: {e}");
                        Some(false)
                    }
                },
                Ok(Err(nightjar_core::http::Error::Io(e))) => {
                    eprintln!("nightjar: could not ask the PC for its bandwidth test: {e}");
                    None
                }
                Ok(Err(nightjar_core::http::Error::Cancelled)) => None,
                // 404 and the like: stock Sunshine, Apollo or Vibepollo.
                Ok(Err(e)) => {
                    eprintln!("nightjar: the PC has no bandwidth test of its own: {e}");
                    Some(false)
                }
                Err(e) => {
                    eprintln!("nightjar: could not ask the PC for its bandwidth test: {e}");
                    None
                }
            };
            Some(Event::FoundationProbe { id, unique_id: record.unique_id, found })
        });
    }

    fn apply_info(&mut self, id: u64, address: String, result: Result<Box<ServerInfo>, String>) -> bool {
        let mut changed = false;
        let Some(index) = self.list.iter().position(|h| h.id == id) else {
            return false;
        };
        // A newly found address may belong to a host already listed under another one.
        if let Ok(info) = &result {
            if let Some(certificate) = self.link_claims.get(&id) {
                if !info.unique_id.is_empty()
                    && self.list.iter().any(|h| h.id != id && h.record.unique_id == info.unique_id && pins_another(&h.record, certificate))
                {
                    self.drop_link_claiming_known_pc(id);
                    return true;
                }
            }
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
                // The remote address answers only when none of the PC's own did: requests go
                // there for now, but the PC stays under its own, so the next poll at home finds
                // it on the LAN again rather than through the router.
                let remote = host.record.remote_address.as_deref() == Some(address.as_str());
                host.via_remote = (remote && host.record.address != address && host.record.pinned_address.is_none())
                    .then(|| address.clone());
                // A pinned PC keeps its address: an answer from another one was still in flight
                // when the user pinned it.
                if !remote && host.record.address != address && host.record.pinned_address.is_none() {
                    let old = std::mem::replace(&mut host.record.address, address.clone());
                    host.record.other_addresses.retain(|a| *a != address);
                    // An older build kept the remote address as the PC's own: it stays the remote one.
                    if !host.record.other_addresses.contains(&old) && host.record.remote_address.as_ref() != Some(&old) {
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
                    host.foundation_probe = None;
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

    /// Takes in an mDNS sighting. True when a listed PC's stored address changed.
    fn apply_discovery(&mut self, name: String, addresses: Vec<IpAddr>, port: u16) -> bool {
        let Some(first) = addresses.first() else {
            return false;
        };
        let texts: Vec<String> = addresses.iter().map(IpAddr::to_string).collect();
        if let Some(host) = self.list.iter_mut().find(|h| {
            texts.contains(&h.record.address) || h.record.other_addresses.iter().any(|a| texts.contains(a))
        }) {
            if host.record.name.is_empty() {
                host.record.name = name;
                crate::logfile::add_secrets(&host.record);
            }
            // Seen on this network: its own address reaches it again, not the router's.
            host.via_remote = None;
            let record = &mut host.record;
            if record.pinned_address.is_none() && record.remote_address.as_ref() == Some(&record.address) {
                if let Some(at) = record.other_addresses.iter().position(|a| texts.contains(a)) {
                    record.address = record.other_addresses.remove(at);
                    return true;
                }
            }
            return false;
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
        false
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

/// The addresses a poll tries in turn: the last that answered, the others, then the remote one,
/// which stays last even where it is listed among the others (a hairpin NAT answers there at
/// home too).
fn poll_addresses(record: &HostRecord) -> Vec<String> {
    if let Some(pinned) = &record.pinned_address {
        return vec![pinned.clone()];
    }
    let remote = record.remote_address.as_ref();
    let mut addresses: Vec<String> = std::iter::once(&record.address)
        .chain(&record.other_addresses)
        .filter(|a| Some(*a) != remote)
        .cloned()
        .collect();
    addresses.extend(remote.cloned());
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

/// An app's cover from its box art bytes. A host's grey placeholder is no cover, so the card shows the title.
fn art_image(bytes: Option<Vec<u8>>) -> Option<egui::ColorImage> {
    bytes.filter(|b| !nightjar_core::http::is_placeholder_box_art(b)).and_then(|b| decode_image(&b))
}

/// A cover and its ambient, both off the UI thread: the first draw only uploads two finished images.
fn art_and_ambient(bytes: Option<Vec<u8>>) -> (Option<egui::ColorImage>, Option<egui::ColorImage>) {
    let image = art_image(bytes);
    let ambient = image.as_ref().map(crate::ambient::bake);
    (image, ambient)
}

fn decode_image(bytes: &[u8]) -> Option<egui::ColorImage> {
    let image = image::load_from_memory(bytes).ok()?.to_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    Some(egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw()))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn the_read_for_a_selected_game_gives_up_at_its_deadline_and_says_why() {
        let started = Instant::now();
        let slow = within(Duration::from_millis(20), || {
            std::thread::sleep(Duration::from_secs(5));
            Ok(1)
        });
        assert!(slow.is_err_and(|e| e.contains("no answer")));
        assert!(started.elapsed() < Duration::from_secs(4), "it does not wait for the read");
        assert_eq!(within(Duration::from_secs(5), || Ok(1)), Ok(1));
        assert_eq!(within(Duration::from_secs(5), || Err::<i32, _>("refused".to_owned())), Err("refused".to_owned()));
    }

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
        let read = NativeRead { pairing_requests: Vec::new(), capabilities: Some(capabilities), library: Some(library().into()), session: Ok(session("idle")), wake: None, update: None };
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
            let read = NativeRead { pairing_requests: Vec::new(), capabilities: Some(granted.clone()), library: Some(library().into()), session: Ok(session("idle")), wake: None, update: None };
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

    // Away from home only the remote address answers: requests go there, but the PC keeps its
    // LAN address, so a hairpin NAT at home does not turn home streams into internet ones.
    #[test]
    fn the_remote_address_reaches_the_pc_but_never_becomes_its_own() {
        let mut hosts = Hosts::without_discovery(vec![paired_record("uid")], Arc::new(|| {}));
        let id = hosts.list[0].id;
        send(&mut hosts, Event::Remote { id, result: Ok(Some(Ipv4Addr::new(93, 184, 216, 34))) });
        let answer = |hosts: &mut Hosts, address: &str| {
            let seq = hosts.get(id).unwrap().info_seq + 1;
            send(hosts, Event::Info { id, seq, address: address.into(), result: Ok(Box::new(native_info())) });
        };
        answer(&mut hosts, "93.184.216.34");
        let host = hosts.get(id).unwrap();
        assert_eq!(host.record.address, "192.0.2.1", "the remote address is not promoted");
        assert_eq!(host.connect_address(), "93.184.216.34", "but requests and streams go there");
        assert_eq!(host.connect_record().address, "93.184.216.34");
        assert_eq!(poll_addresses(&host.record), ["192.0.2.1", "93.184.216.34"], "the LAN is tried first again");

        answer(&mut hosts, "192.0.2.1");
        let host = hosts.get(id).unwrap();
        assert_eq!(host.record.address, "192.0.2.1");
        assert_eq!(host.connect_address(), "192.0.2.1", "back home the LAN is the primary");

        // Pinned, the pinned address is the only one used.
        answer(&mut hosts, "93.184.216.34");
        assert!(hosts.pin_address(id, Some("192.0.2.1")));
        assert_eq!(hosts.get(id).unwrap().connect_address(), "192.0.2.1");
        assert_eq!(poll_addresses(&hosts.get(id).unwrap().record), ["192.0.2.1"]);
    }

    // A record an older build saved with the remote address as its own: the LAN answers first
    // and takes its place back, and an mDNS sighting does so at once.
    #[test]
    fn the_lan_address_comes_back_from_a_saved_remote_one() {
        let mut record = paired_record("uid");
        record.address = "93.184.216.34".into();
        record.other_addresses = vec!["192.0.2.1".into()];
        record.remote_address = Some("93.184.216.34".into());
        assert_eq!(poll_addresses(&record), ["192.0.2.1", "93.184.216.34"], "the remote address goes last");

        let mut hosts = Hosts::without_discovery(vec![record.clone()], Arc::new(|| {}));
        let id = hosts.list[0].id;
        send(&mut hosts, Event::Info { id, seq: 1, address: "192.0.2.1".into(), result: Ok(Box::new(native_info())) });
        let host = hosts.get(id).unwrap();
        assert_eq!(host.record.address, "192.0.2.1");
        assert!(host.record.other_addresses.is_empty(), "the remote address stays only the remote one");
        assert_eq!(host.record.remote_address.as_deref(), Some("93.184.216.34"));

        let mut hosts = Hosts::without_discovery(vec![record], Arc::new(|| {}));
        assert!(hosts.apply_discovery("PC".into(), vec!["192.0.2.1".parse().unwrap()], 0));
        assert_eq!(hosts.list[0].record.address, "192.0.2.1");
        assert_eq!(hosts.list[0].connect_address(), "192.0.2.1");
    }

    // Test speed goes to a Sunshine-family PC only once it said it has Foundation's test; stock
    // Sunshine, Apollo and Vibepollo do not. Rubylight keeps its own test without asking.
    #[test]
    fn the_bandwidth_test_is_offered_only_where_the_pc_has_one() {
        let sunshine = |unique_id: &str| {
            let xml = format!("<root status_code=\"200\"><uniqueid>{unique_id}</uniqueid><state>SUNSHINE_SERVER_FREE</state>\
                <PairStatus>1</PairStatus><appversion>7.1.431.-1</appversion><SunshineVersion>2025.1</SunshineVersion></root>");
            ServerInfo::parse(&xml, 47989).unwrap()
        };
        let mut hosts = Hosts::without_discovery(vec![paired_record("uid")], Arc::new(|| {}));
        let id = hosts.list[0].id;
        info_lands(&mut hosts, id, sunshine("uid"));
        assert!(!hosts.list[0].bandwidth_test(), "not before the PC answered");
        send(&mut hosts, Event::FoundationProbe { id, unique_id: "uid".into(), found: Some(false) });
        assert!(!hosts.list[0].bandwidth_test(), "stock Sunshine");
        hosts.list[0].foundation_probe = None;
        send(&mut hosts, Event::FoundationProbe { id, unique_id: "other".into(), found: Some(true) });
        assert!(!hosts.list[0].bandwidth_test(), "another PC's answer");
        send(&mut hosts, Event::FoundationProbe { id, unique_id: "uid".into(), found: Some(true) });
        assert!(hosts.list[0].bandwidth_test(), "Foundation Sunshine");
        info_lands(&mut hosts, id, sunshine("another-pc-uid"));
        assert_eq!(hosts.list[0].foundation_probe, None, "another PC at this address is asked again");

        let rubylight = "<root status_code=\"200\"><uniqueid>uid</uniqueid><state>SUNSHINE_SERVER_FREE</state><PairStatus>1</PairStatus>\
            <RustHostVersion>3.0.0</RustHostVersion><PyroWaveBandwidthProbeBytes>33554432</PyroWaveBandwidthProbeBytes></root>";
        info_lands(&mut hosts, id, ServerInfo::parse(rubylight, 47989).unwrap());
        assert_eq!(hosts.list[0].foundation_probe, None);
        assert!(hosts.list[0].bandwidth_test(), "Rubylight's PyroWave test, without asking");
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
        Box::new(NativeRead { pairing_requests: Vec::new(), capabilities: None, library: Some(library().into()), session: Ok(session), wake: None, update: None })
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
    fn the_hosts_placeholder_box_art_shows_the_title() {
        let placeholder = include_bytes!("../../client/tests/fixtures/box-placeholder.png").to_vec();
        assert!(decode_image(&placeholder).is_some(), "the placeholder is a real PNG");
        assert!(art_image(Some(placeholder)).is_none(), "and no cover: the card shows the title");
        let mut cover = Vec::new();
        image::RgbaImage::new(30, 40).write_to(&mut std::io::Cursor::new(&mut cover), image::ImageFormat::Png).unwrap();
        assert!(art_image(Some(cover)).is_some());
        assert!(art_image(None).is_none());
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

    const PEM: &str = "-----BEGIN CERTIFICATE-----\nAQID\n-----END CERTIFICATE-----\n";

    fn link_naming(cert_sha256: [u8; 32]) -> PairingUri {
        PairingUri {
            host: "203.0.113.9".into(),
            port: 47989,
            otp: "4821".into(),
            passphrase: "hunter2".into(),
            unique_id: String::new(),
            cert_sha256,
            name: "Claimed".into(),
        }
    }

    fn known_pc_with_link(cert_sha256: [u8; 32]) -> (Hosts, u64) {
        let mut known = paired_record("uid");
        known.server_certificate = Some(PEM.into());
        let mut hosts = Hosts::without_discovery(vec![known], Arc::new(|| {}));
        let id = hosts.add_manual("203.0.113.9", Some(47989));
        hosts.pair_link(id, link_naming(cert_sha256));
        (hosts, id)
    }

    #[test]
    fn a_link_address_claiming_a_known_pc_cannot_move_it() {
        let (mut hosts, id) = known_pc_with_link([9; 32]);
        let info = ServerInfo::parse(INFO, 47989).unwrap();
        assert!(hosts.apply_info(id, "203.0.113.9".into(), Ok(Box::new(info))));
        assert_eq!(hosts.list.len(), 1, "the record the link added is gone");
        let known = &hosts.list[0].record;
        assert_eq!(known.unique_id, "uid");
        assert_eq!(known.address, "192.0.2.1", "the known PC stays where it was");
        assert!(!known.other_addresses.contains(&"203.0.113.9".to_owned()));
        assert_eq!(known.name, "PC");
        assert_eq!(known.server_certificate.as_deref(), Some(PEM));
    }

    #[test]
    fn the_known_pcs_own_link_may_still_find_it_at_a_new_address() {
        let (mut hosts, id) = known_pc_with_link(nightjar_core::http::certificate_sha256(PEM).unwrap());
        let info = ServerInfo::parse(INFO, 47989).unwrap();
        assert!(hosts.apply_info(id, "203.0.113.9".into(), Ok(Box::new(info))));
        assert_eq!(hosts.list.len(), 1, "one PC, as before");
        assert_eq!(hosts.list[0].record.address, "203.0.113.9");
        assert_eq!(hosts.list[0].record.server_certificate.as_deref(), Some(PEM));
    }

    /// A small PNG of one colour, as a host's box art arrives.
    fn png(color: [u8; 3]) -> Vec<u8> {
        let image = image::RgbImage::from_pixel(30, 40, image::Rgb(color));
        let mut bytes = std::io::Cursor::new(Vec::new());
        image.write_to(&mut bytes, image::ImageFormat::Png).expect("a PNG");
        bytes.into_inner()
    }

    #[test]
    fn a_cover_arrives_with_its_ambient_already_baked() {
        let (image, ambient) = art_and_ambient(Some(png([200, 20, 20])));
        assert_eq!(image.expect("the cover").size, [30, 40]);
        let ambient = ambient.expect("baked with the cover, off the UI thread");
        assert_eq!(ambient, crate::ambient::bake(&decode_image(&png([200, 20, 20])).expect("the cover")));
        assert_eq!(art_and_ambient(None), (None, None), "no cover, no ambient");
        assert_eq!(art_and_ambient(Some(b"not an image".to_vec())), (None, None));
    }

    #[test]
    fn the_ambient_lands_with_its_cover_and_goes_with_it() {
        let ctx = egui::Context::default();
        let mut hosts = Hosts::without_discovery(vec![paired_record("uid")], Arc::new(|| {}));
        let id = hosts.list[0].id;
        hosts.list[0].art_versions.insert(7, "v1".into());
        let art = |version: &str, color: Option<[u8; 3]>| {
            let (image, ambient) = art_and_ambient(color.map(png));
            Event::Art { id, app: 7, version: version.into(), image, ambient }
        };
        hosts.sender.send(art("v1", Some([20, 200, 20]))).expect("the channel");
        hosts.drain(&ctx);
        assert!(hosts.list[0].ambient.contains_key(&7));
        assert!(hosts.list[0].ambient_glow[&7].a() > 0, "its glow lands with it");
        assert!(matches!(hosts.list[0].art.get(&7), Some(Art::Ready(_))));

        hosts.sender.send(art("old", None)).expect("the channel");
        hosts.drain(&ctx);
        assert!(hosts.list[0].ambient.contains_key(&7), "an answer for an older cover changes nothing");

        hosts.sender.send(art("v1", None)).expect("the channel");
        hosts.drain(&ctx);
        assert!(!hosts.list[0].ambient.contains_key(&7), "no cover any more, no ambient of it");
        assert!(!hosts.list[0].ambient_glow.contains_key(&7), "nor its glow");
        let mut output = ctx.run_ui(egui::RawInput::default(), |_| {});
        output.textures_delta.clear();
    }

    #[test]
    fn a_link_to_a_known_address_with_another_certificate_is_refused_at_once() {
        let mut known = paired_record("uid");
        known.server_certificate = Some(PEM.into());
        known.http_port = 47989;
        let mut hosts = Hosts::without_discovery(vec![known], Arc::new(|| {}));
        let id = hosts.list[0].id;
        hosts.pair_link(id, link_naming([9; 32]));
        assert!(matches!(
            hosts.events.try_recv(),
            Ok(Event::Paired { id: refused, result: Err(why), .. }) if refused == id && why == PAIR_LINK_MISMATCH
        ));
        assert!(hosts.link_claims.is_empty());
        assert_eq!(hosts.list[0].record.server_certificate.as_deref(), Some(PEM));
    }

    #[test]
    fn a_pc_paired_nowhere_takes_any_link_and_a_typed_address_is_as_before() {
        assert!(!pins_another(&HostRecord::default(), &[9; 32]));
        let mut record = paired_record("uid");
        record.server_certificate = Some("not a certificate".into());
        assert!(pins_another(&record, &[9; 32]), "unreadable counts as another");
        let mut known = paired_record("uid");
        known.server_certificate = Some(PEM.into());
        let mut hosts = Hosts::without_discovery(vec![known], Arc::new(|| {}));
        let id = hosts.add_manual("203.0.113.9", Some(47989));
        let info = ServerInfo::parse(INFO, 47989).unwrap();
        hosts.apply_info(id, "203.0.113.9".into(), Ok(Box::new(info)));
        assert_eq!(hosts.list[0].record.address, "203.0.113.9", "no link, no claim");
    }

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
    fn the_failure_reason_comes_only_from_the_poll_after_the_stream() {
        let mut hosts = Hosts::without_discovery(vec![paired_record("uid")], Arc::new(|| {}));
        let id = hosts.list[0].id;
        let failed = |seq| {
            let text = INFO.replace("<state>", "<RustHostLastFailure>HDR is on, but the display is not in HDR</RustHostLastFailure><state>");
            Event::Info { id, seq, address: "192.0.2.1".into(), result: Ok(Box::new(ServerInfo::parse(&text, 47989).unwrap())) }
        };
        // An answer from before the stream says nothing about it.
        send(&mut hosts, failed(1));
        assert_eq!(hosts.failure_after(id, 2), None, "still waiting for the poll after the stream");
        send(&mut hosts, failed(2));
        assert_eq!(hosts.failure_after(id, 2), Some(Some("HDR is on, but the display is not in HDR".into())));
        // A PC that gives no reason, or could not be asked, ends the wait without one.
        send(&mut hosts, info_event(id, 3, 0));
        assert_eq!(hosts.failure_after(id, 3), Some(None));
        send(&mut hosts, Event::Info { id, seq: 4, address: "192.0.2.1".into(), result: Err("timed out".into()) });
        assert_eq!(hosts.failure_after(id, 4), Some(None));
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
        // As Windows' shell hands a link over; whoever copies it from there pastes this form.
        let from_shell = invitation.to_uri().replacen("pair?", "pair/?", 1);
        assert_eq!(parse_entry(&from_shell), Ok(Entry::Link(Box::new(invitation.clone()))));
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

    /// A PC offering Restart and Shut down: hands out `tokens` in turn, answers a request with `post`.
    struct FakePowerPc {
        tokens: std::cell::RefCell<Vec<&'static str>>,
        reason: Option<&'static str>,
        post: fn() -> Result<(), nightjar_core::http::Error>,
        calls: std::cell::RefCell<Vec<String>>,
    }

    impl PcPowerHost for FakePowerPc {
        fn pc_power(&self) -> Result<PcPowerOffer, nightjar_core::http::Error> {
            use nightjar_core::http::PowerAvailability;
            self.calls.borrow_mut().push("GET".into());
            let mut tokens = self.tokens.borrow_mut();
            let token = (!tokens.is_empty()).then(|| tokens.remove(0).to_owned());
            let available = PowerAvailability { available: token.is_some(), reason: self.reason.map(Into::into) };
            Ok(PcPowerOffer { restart: available.clone(), shutdown: available, token, token_lifetime_s: 30, ..Default::default() })
        }
        fn power_pc(&self, action: PcPower, token: &str) -> Result<(), nightjar_core::http::Error> {
            self.calls.borrow_mut().push(format!("POST {} {token}", action.path()));
            (self.post)()
        }
    }

    fn power_pc(tokens: &[&'static str], reason: Option<&'static str>, post: fn() -> Result<(), nightjar_core::http::Error>) -> FakePowerPc {
        FakePowerPc { tokens: std::cell::RefCell::new(tokens.to_vec()), reason, post, calls: Default::default() }
    }

    #[test]
    fn restart_and_shut_down_spend_a_fresh_token_and_show_the_hosts_refusal() {
        use nightjar_core::http::{native_api::error::ApiError, Error};
        let ok = || Ok(());
        let pc = power_pc(&[TOKEN_A], None, ok);
        let Ok(PcPowerStep::Offer { token, at, wake_from_off: false }) = pc_power_token(&pc, PcPower::Restart) else { panic!("an offer") };
        assert!(matches!(pc_power_confirmed(&pc, PcPower::Restart, token, at), Ok(PcPowerStep::Accepted)));
        assert_eq!(*pc.calls.borrow(), ["GET".to_owned(), format!("POST power/restart {TOKEN_A}")]);

        // An old token is replaced before it is spent, as Sleep PC does.
        let now = Handed::now();
        let old = Handed { at: now.at.checked_sub(Duration::from_secs(26)).unwrap(), wall: now.wall - Duration::from_secs(26) };
        let pc = power_pc(&[TOKEN_B], None, ok);
        assert!(pc_power_confirmed(&pc, PcPower::ShutDown, TOKEN_A.into(), old).is_ok());
        assert_eq!(*pc.calls.borrow(), ["GET".to_owned(), format!("POST power/shutdown {TOKEN_B}")]);

        // A busy PC: its reason as it is, or ours when it gives none; nothing is spent.
        let pc = power_pc(&[], Some("TV is streaming from this PC."), ok);
        assert_eq!(pc_power_token(&pc, PcPower::ShutDown).err().as_deref(), Some("TV is streaming from this PC."));
        assert_eq!(pc_power_token(&power_pc(&[], None, ok), PcPower::Restart).err().as_deref(), Some(PC_POWER_BUSY));

        // A refused request shows the host's words.
        let refused = || {
            let error: ApiError = serde_json::from_str(
                r#"{"code":"host_in_use","message":"Desk is streaming from this PC.","setting":null,"retry_after_ms":null}"#,
            )
            .unwrap();
            Err(Error::Api { status: 409, error })
        };
        let pc = power_pc(&[TOKEN_A], None, refused);
        assert_eq!(
            pc_power_confirmed(&pc, PcPower::Restart, TOKEN_A.into(), Handed::now()).err().as_deref(),
            Some("Desk is streaming from this PC.")
        );
    }

    /// A PC that answers `GET /power` with these refusals in turn, then stops answering; each
    /// marked unconfirmed by the second field.
    struct RefusingPc(std::cell::RefCell<Vec<Option<&'static str>>>, bool);

    impl PcPowerHost for RefusingPc {
        fn pc_power(&self) -> Result<PcPowerOffer, nightjar_core::http::Error> {
            let mut answers = self.0.borrow_mut();
            if answers.is_empty() {
                return Err(nightjar_core::http::Error::NotFound("/power".into()));
            }
            Ok(PcPowerOffer { last_refusal: answers.remove(0).map(Into::into), last_refusal_unconfirmed: self.1, ..Default::default() })
        }
        fn power_pc(&self, _: PcPower, _: &str) -> Result<(), nightjar_core::http::Error> {
            panic!("the refusal check never asks again")
        }
    }

    #[test]
    fn a_refusal_after_the_pc_took_the_request_is_shown() {
        let refusing = |answers: &[Option<&'static str>]| RefusingPc(std::cell::RefCell::new(answers.to_vec()), false);
        let windows = "Windows refused to restart the PC: Access is denied. (exit code 5)";
        assert_eq!(pc_power_refusal(&refusing(&[None, None, Some(windows)]), 10, || {}), Some(LateRefusal::Refused(windows.into())));
        // Windows did not confirm it in time: the host says so, and it is no refusal.
        let unanswered = "Windows did not confirm the request in time; the PC may still restart: process timed out";
        let unconfirmed = RefusingPc(std::cell::RefCell::new(vec![None, Some(unanswered)]), true);
        assert_eq!(pc_power_refusal(&unconfirmed, 10, || {}), Some(LateRefusal::Unconfirmed(unanswered.into())));
        // Going down: the PC stops answering, nothing to show.
        assert_eq!(pc_power_refusal(&refusing(&[None]), 10, || {}), None);
        // Never says within the reads.
        assert_eq!(pc_power_refusal(&refusing(&[None, None, None, Some(windows)]), 3, || {}), None);
    }

    /// A PC during an update: `GET /update` answers in turn (None: not answering), and
    /// `/serverinfo` says `running` once the answers run out.
    struct UpdatingPc {
        answers: std::cell::RefCell<Vec<Option<HostUpdate>>>,
        running: Option<&'static str>,
    }

    impl UpdateHost for UpdatingPc {
        fn host_update(&self) -> Result<HostUpdate, nightjar_core::http::Error> {
            let mut answers = self.answers.borrow_mut();
            match (!answers.is_empty()).then(|| answers.remove(0)).flatten() {
                Some(update) => Ok(update),
                None => Err(nightjar_core::http::Error::NotFound("/update".into())),
            }
        }
        fn running_version(&self) -> Option<String> {
            if self.answers.borrow().is_empty() {
                self.running.map(Into::into)
            } else {
                Some("3.3.0".into())
            }
        }
    }

    #[test]
    fn an_update_fails_at_once_without_a_target_and_follows_the_pcs_phase_error_and_wait() {
        let update = |phase: &str, error: Option<&str>, waiting: Option<&str>| HostUpdate {
            current_version: "3.3.0".into(),
            latest_version: Some("v3.4.0".into()),
            phase: phase.into(),
            install_supported: true,
            error: error.map(Into::into),
            waiting_for: waiting.map(Into::into),
            install_now: false,
        };
        // The 202 offers nothing newer: no 15-minute wait for version "".
        let none = HostUpdate { latest_version: None, ..update("waiting", None, None) };
        assert_eq!(update_target(&none), Err(UPDATE_GONE.to_owned()));
        assert_eq!(update_target(&update("waiting", None, None)), Ok("3.4.0".to_owned()));
        let pc = |answers: Vec<Option<HostUpdate>>, running| UpdatingPc { answers: answers.into(), running };
        let follow_most = |pc: &UpdatingPc, polls, most| {
            let mut said = Vec::new();
            let result = follow_update(pc, "3.4.0", polls, most, || {}, |step| said.push(step));
            (result, said)
        };
        let follow = |pc: &UpdatingPc, polls| follow_most(pc, polls, u32::MAX);
        // Back on the new version.
        let (result, said) = follow(&pc(vec![Some(update("downloading", None, None)), None], Some("3.4.0")), 10);
        assert_eq!((result, said), (Ok(UpdateStep::Back("3.4.0".into())), vec![]));
        // The updater failed: its words, at once.
        let failed = update("failed", Some("Installer verification failed; no files were installed"), None);
        let (result, _) = follow(&pc(vec![Some(failed)], None), 10);
        assert_eq!(result, Err("Installer verification failed; no files were installed".into()));
        let (result, _) = follow(&pc(vec![Some(update("failed", None, None))], None), 10);
        assert_eq!(result, Err(UPDATE_FAILED.into()));
        // Gone, then back on the old version with nothing under way: setup gave up.
        let (result, _) = follow(&pc(vec![Some(update("installing", None, None)), None, Some(update("idle", None, None))], None), 10);
        assert!(result.as_ref().is_err_and(|e| e.contains("3.3.0")), "{result:?}");
        // Waiting for another device's stream: said once, and not counted against the wait.
        let tv = "TV is streaming from this PC.";
        let waits = vec![Some(update("ready", None, Some(tv))); 5];
        let (result, said) = follow(&pc([waits, vec![None]].concat(), Some("3.4.0")), 2);
        assert_eq!((result, said), (Ok(UpdateStep::Back("3.4.0".into())), vec![UpdateStep::Waiting(tv.into())]));
        // Nothing ever comes back.
        let (result, _) = follow(&pc(vec![Some(update("installing", None, None)); 3], None), 3);
        assert_eq!(result, Err(trf("The PC did not come back with version {} in time.", &[&"3.4.0"])));
        // Another device's stream holds it for `most` reads at most, then the wait gives up.
        let held = pc(vec![Some(update("ready", None, Some(tv))); 6], Some("3.4.0"));
        let (result, said) = follow_most(&held, 2, 4);
        assert_eq!(result, Err(trf("The PC did not come back with version {} in time.", &[&"3.4.0"])));
        assert_eq!((said, held.answers.borrow().len()), (vec![UpdateStep::Waiting(tv.into())], 2));
        assert_eq!(UPDATE_MAX_WAIT.as_secs(), 3 * 60 * 60);
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
