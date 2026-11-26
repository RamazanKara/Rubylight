//! The PCs Nightjar knows: discovery on the local network (mDNS `_nvstream._tcp`), hosts
//! added by address, their online state, pairing, the app list with box art, and quitting
//! the running app. Every host request runs on a worker thread and comes back as an
//! [`Event`]; the UI thread owns the list.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime};

use nightjar_core::hosts::client_pool::PooledHostClient;
use nightjar_core::http::native_api::feature;
#[cfg(test)]
use nightjar_core::http::native_api::library::Library;
use nightjar_core::http::native_api::session::ActionId;
#[cfg(test)]
use nightjar_core::http::native_api::session::Session;
#[cfg(test)]
use nightjar_core::http::App;
use nightjar_core::http::{
    AppControl, Availability, ClientIdentity, Feature, HostClient, HostUpdate, InfoPath,
    LateRefusal, NativeCapabilities, NativeLibrary, PcPower,
    ServerInfo, DEFAULT_HTTP_PORT, pem_decode_certificate,
};
#[cfg(test)]
use nightjar_core::http::{PairResult, PcPowerOffer, PowerError, PowerReply};

use rubylight_protocol::invite_uri::InviteUri;
use rubylight_protocol::pairing_uri::PairingUri;

use crate::strings::{en, tr, trf};
use crate::config::{HostRecord, Store};

mod covers;
mod events;
mod game_info;
mod host;
mod native;
mod pairing;
mod poll;
mod power;

pub use covers::*;
pub use host::{Host, NativeRead};
pub use poll::Event;
use poll::{away_address, internet_reply, public_ipv4};
pub(crate) use poll::poll_addresses;
#[cfg(test)]
use poll::punches_now;
pub use native::*;
pub use pairing::*;
pub use power::*;

pub use nightjar_desktop_config::hosts_types::{device_name, merge_native, Waker};
#[cfg(test)]
pub use nightjar_desktop_ui::hosts_model::ADDRESS_INVALID;
pub use nightjar_desktop_ui::hosts_model::{
    parse_entry, Apps, Art, Entry, Native, Reach, ANOTHER_PC_ANSWERS, CLIPBOARD_NO_LINK,
    INVITE_INVALID, INVITE_KNOWN_PC, LINK_INVALID, PAIR_DECLINED, PAIR_IN_GAME, PAIR_IN_PROGRESS,
    PAIR_LINK_MISMATCH, PAIR_NETWORK, PAIR_OFFLINE, PAIR_OTP_PIN_WRONG, PAIR_PIN_WRONG,
    PAIR_UNVERIFIED, PC_POWER_BUSY, QUIT_OTHER_DEVICE, SLEEP_BUSY, SLEEP_NOT_ALLOWED,
    SLEEP_UNSUPPORTED, UNPAIR_FAILED, UNPAIR_NOT_PAIRED, invite_error, quit_failure,
};

/// A request a PC's own worker runs; what it returns is queued for the UI thread.
type PcJob = Box<dyn FnOnce() -> Option<Event> + Send>;

/// Which of a PC's two workers runs a request. The timed background polls have their own, so a
/// slow one (an away PC is tried over the internet too, for up to about 50 s) never holds up a
/// check the user asked for or a native read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Lane {
    /// Checks something asked for (Try again, a quit, a wake, pairing) and native reads.
    Asked,
    /// The timed polls of [`Hosts::poll`].
    Background,
}

const POLL_INTERVAL: Duration = Duration::from_secs(4);
/// After a quit, a closed game or a stream's end the PC changes its state a moment later, so it is
/// asked every second for this long (`Host::recheck_until`).
const RECHECK_INTERVAL: Duration = Duration::from_secs(1);
/// How often a PC's internet play reply is read again while it is reached on its network.
const INTERNET_READ_INTERVAL: Duration = Duration::from_millis(nightjar_policy::hosts::INTERNET_PLAY_READ_INTERVAL_MS);
const RECHECK_WINDOW: Duration = Duration::from_secs(15);
const RECHECK_LIBRARY_INTERVAL: Duration = Duration::from_secs(3);
/// The native library and permissions are read this often; the session on every poll.
const NATIVE_FULL_INTERVAL: Duration = Duration::from_secs(30);
/// A cover whose download failed (no answer in time, the PC busy) shows the title this long, then
/// is asked for again while it is on screen. One the PC has none of stays a title.
const ART_RETRY_INTERVAL: Duration = Duration::from_secs(30);
const MDNS_SERVICE: &str = "_nvstream._tcp.local.";
#[cfg(test)]
const SLEEP_TOKEN_REFRESH: Duration = Duration::from_millis(nightjar_policy::hosts::SLEEP_TOKEN_REFRESH_MS);

pub struct Hosts {
    pub list: Vec<Host>,
    identity: Option<Arc<ClientIdentity>>,
    events: Receiver<Event>,
    sender: Sender<Event>,
    waker: Waker,
    next_id: u64,
    pairing: Option<(u64, Arc<PooledHostClient>)>,
    pairing_generation: u64,
    /// The certificate a pairing link named, for each record it was opened on.
    link_claims: HashMap<u64, [u8; 32]>,
    /// Hosts whose live library changed since the last `take_library_saves`.
    library_saves: Vec<u64>,
    /// Hosts whose app list as Home keeps it changed since the last `take_classic_saves`.
    classic_saves: Vec<u64>,
    /// The threads covers load on.
    art_pool: ArtPool,
    /// The cover files on disk, for the workers that save one.
    art_files: Arc<Mutex<crate::config::ArtFiles>>,
    /// The pixels per point covers are sized for: the window's, as of the last frame.
    art_scale: f32,
    /// The frame each cover texture was last drawn or asked for in, counted in `art_frame`.
    art_used: HashMap<egui::TextureId, u64>,
    art_frame: u64,
    /// A cover landed since the textures were last added up.
    art_landed: bool,
    /// STUN for the remote address of PCs found on the LAN; off in tests, which stay offline.
    stun: bool,
    _mdns: Option<mdns_sd::ServiceDaemon>,
    /// Whether this network is being searched for PCs: the first run's orbit runs on it.
    pub(crate) discovering: bool,
    /// Each PC's workers, one per [`Lane`]: a thread that lives as long as the PC is listed runs
    /// its lane's requests in turn, instead of a new thread for each. Dropping the sender ends it.
    pc_workers: HashMap<(u64, Lane), Sender<PcJob>>,
    /// Artwork and facts from SteamGridDB and IGDB with the player's keys; None in tests and the
    /// demo, which stay offline.
    game_info: Option<crate::game_info::Shared>,
    /// The facts known for each game, by its lookup key ([`crate::game_info::entry_key`]).
    facts: Arc<HashMap<String, nightjar_policy::game_info::cache::Facts>>,
    /// The games whose facts were asked for since the keys last changed.
    facts_asked: Arc<std::collections::HashSet<String>>,
    /// The worker that looks facts up, one game at a time.
    facts_worker: Option<Sender<nightjar_policy::game_info::job::Game>>,
    /// The worker that fetches SteamGridDB covers for the gaps the cover workers found, so a
    /// lookup waiting on the services never holds a cover worker.
    cover_worker: Option<Sender<game_info::CoverJob>>,
    /// Old host IDs replaced by a `unique_id` merge, mapped to the surviving host ID.
    merged_into: HashMap<u64, u64>,
    /// In-flight `/cancel` requests so shutdown can wait for quit-on-leave to finish.
    pending_quits: Vec<Arc<(Mutex<bool>, Condvar)>>,
    /// The demo's PCs while a first-run page shows a shorter list.
    #[cfg(feature = "demo-library")]
    pub(crate) demo_shelf: Vec<Host>,
}

struct QuitGuard(Arc<(Mutex<bool>, Condvar)>);

impl Drop for QuitGuard {
    fn drop(&mut self) {
        let (lock, cvar) = &*self.0;
        let mut done = lock.lock().unwrap_or_else(|e| e.into_inner());
        *done = true;
        cvar.notify_all();
    }
}

/// The HTTP port a saved or sighted `port` means: 0 is the default one.
fn http_port(port: u16) -> u16 {
    if port == 0 {
        DEFAULT_HTTP_PORT
    } else {
        port
    }
}

pub fn client_for(identity: &Arc<ClientIdentity>, record: &HostRecord, address: &str) -> Result<PooledHostClient, String> {
    let der = record.server_certificate.as_deref().map(pem_decode_certificate).transpose().map_err(|e| e.to_string())?;
    let pool = pairing::shared_pool_for(identity);
    pool.set_device_name(device_name());
    pool.borrow_with_options(address, http_port(record.http_port), record.https_port, der.as_deref(), crate::second_screen::this_process()).map_err(|e| e.to_string())
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

/// A poll's failure at one address. Another certificate there is another PC or program
/// answering in this one's place (a new DHCP lease, a second streaming app on the same ports):
/// the pairing is kept, and the PC reads as away until it answers again.
fn poll_error(error: &nightjar_core::http::Error) -> String {
    match error {
        nightjar_core::http::Error::CertificateMismatch => ANOTHER_PC_ANSWERS.into(),
        other => describe(other),
    }
}

/// The failures of one poll's paths and what the PC reads as when none answered. Another
/// certificate on any of them is told over a later path's failure (an IPv6 or remote address that
/// timed out): it is the one the user can act on.
#[derive(Default)]
struct PollFailure {
    last: Option<String>,
    another_pc: bool,
}

impl PollFailure {
    fn failed(&mut self, error: String) {
        self.another_pc |= error == ANOTHER_PC_ANSWERS;
        self.last = Some(error);
    }

    fn reason(self) -> String {
        if self.another_pc {
            ANOTHER_PC_ANSWERS.into()
        } else {
            self.last.unwrap_or_else(|| en("No address").into())
        }
    }
}

pub(crate) fn describe(error: &nightjar_core::http::Error) -> String {
    use nightjar_core::http::Error;
    match error {
        Error::Io(e) if e.kind() == std::io::ErrorKind::TimedOut => en("The PC did not answer.").into(),
        Error::Io(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => {
            en("The PC refused the connection. Is its streaming app running?").into()
        }
        Error::Status { code: 401, .. } | Error::NotPaired => en("This PC is not paired.").into(),
        Error::CertificateMismatch => {
            en("The PC presented a different certificate. Pair it again.").into()
        }
        other => {
            let text = other.to_string();
            let mut chars = text.chars();
            let text: String = match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect(),
                None => text,
            };
            // The PC's title, next step and technical detail, when it sent them.
            match other.api_error() {
                Some(api) => nightjar_desktop_config::host_error::compose(api.title.as_deref(), &text, api.next_step.as_deref(), api.technical_detail.as_deref()),
                None => text,
            }
        }
    }
}

impl Hosts {
    pub fn new(records: Vec<HostRecord>, waker: Waker) -> Self {
        let mut hosts = Self::without_discovery(records, waker);
        hosts._mdns = hosts.start_discovery();
        hosts.discovering = hosts._mdns.is_some();
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
            classic_saves: Vec::new(),
            art_pool: ArtPool::default(),
            art_files: Arc::default(),
            art_scale: 1.0,
            art_used: HashMap::new(),
            art_frame: 0,
            art_landed: false,
            stun: false,
            _mdns: None,
            discovering: false,
            pc_workers: HashMap::new(),
            game_info: None,
            facts: Arc::new(HashMap::new()),
            facts_asked: Arc::new(std::collections::HashSet::new()),
            facts_worker: None,
            cover_worker: None,
            merged_into: HashMap::new(),
            pending_quits: Vec::new(),
            #[cfg(feature = "demo-library")]
            demo_shelf: Vec::new(),
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
            art: Arc::new(HashMap::new()),
            ambient: Arc::new(HashMap::new()),
            ambient_glow: Arc::new(HashMap::new()),
            art_versions: Arc::new(HashMap::new()),
            art_sized: Arc::new(HashMap::new()),
            art_retry: Arc::new(HashMap::new()),
            native: Native::default(),
            classic_apps: None,
            classic: None,
            native_epoch: 0,
            native_seq: 0,
            action_seq: 0,
            session_seq: 0,
            error: None,
            checking: false,
            polling: false,
            last_poll: None,
            poll_seq: 0,
            info_seq: 0,
            background_poll: None,
            asked_poll: None,
            asked_again: false,
            recheck_until: None,
            pinned_check: false,
            via_remote: None,
            internet_read: None,
            foundation_probe: None,
            probing_foundation: false,
        });
        id
    }

    pub fn get(&self, id: u64) -> Option<&Host> {
        self.list.iter().find(|h| h.id == id)
    }

    /// How many PCs are paired now that `id` has just paired: its last answer may still be the one
    /// from before the pairing, which said it was not.
    pub fn paired_with(&self, id: u64) -> usize {
        self.list.iter().filter(|h| h.id == id || h.paired()).count()
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

    /// Opens an invite on a thread: the PC is reached over the internet, paired as a guest and
    /// added (or updated) once [`Event::Invited`] comes back.
    pub fn redeem_invite(&mut self, link: InviteUri) {
        // Anyone can write a link that names a PC of this list: one paired with another
        // certificate than the link's is not what the link leads to, and is neither asked nor
        // changed. Nothing is sent, so the invite is not spent either.
        if self.invite_claims_known_pc(&link.unique_id, &link.cert_sha256) {
            eprintln!("nightjar: an invite names a known PC with another certificate; not opened");
            let _ = self.sender.send(Event::Invited { result: Err(tr(INVITE_KNOWN_PC).to_owned()), id: None });
            (self.waker)();
            return;
        }
        let Some(identity) = self.identity.clone() else {
            let _ = self.sender.send(Event::Invited {
                result: Err(tr(
                    "Nightjar has no client identity yet. Restart Nightjar and try again.",
                )
                .to_owned()),
                id: None,
            });
            (self.waker)();
            return;
        };
        self.spawn("invite", move || {
            let now = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs());
            let result =
                nightjar_core::invite::redeem(&link, identity, &device_name(), now).map(Box::new);
            Some(Event::Invited { result, id: None })
        });
    }

    /// Whether a PC of the list has `unique_id` and is paired with a certificate other than the
    /// one that hashes to `certificate`.
    fn invite_claims_known_pc(&self, unique_id: &str, certificate: &[u8; 32]) -> bool {
        self.list.iter().any(|h| {
            h.record.unique_id == unique_id
                && nightjar_policy::hosts::invite_claims_known_pc(
                    h.record
                        .server_certificate
                        .as_deref()
                        .and_then(nightjar_core::http::certificate_sha256_pem),
                    *certificate,
                )
        })
    }

    /// A PC paired from an invite: updates the PC with that ID, or adds it. Its address is where
    /// it answered; a punched path's loopback end finds the PC through the tunnel again. A PC of
    /// the list that is paired with another certificate stays as it is: the invite proved its own
    /// certificate, not that it is that PC.
    fn apply_invite(&mut self, redeemed: &nightjar_core::invite::Redeemed) -> Result<u64, String> {
        let proved = nightjar_core::http::certificate_sha256(&redeemed.server_certificate_pem).ok();
        if proved.is_none_or(|certificate| self.invite_claims_known_pc(&redeemed.unique_id, &certificate)) {
            eprintln!("nightjar: an invite paired a PC that a known PC with another certificate already is; not kept");
            // The redeem may have pointed this ID's internet path at the invite's PC: the known
            // PC's next poll makes its own again from its kept details.
            nightjar_core::internet::stop(&redeemed.unique_id);
            return Err(tr(INVITE_KNOWN_PC).to_owned());
        }
        let known = self
            .list
            .iter()
            .position(|h| h.record.unique_id == redeemed.unique_id);
        let id = match known {
            Some(at) => {
                let host = &mut self.list[at];
                host.record.server_certificate = Some(redeemed.server_certificate_pem.clone());
                host.record.internet_play = Some(redeemed.reply.clone());
                host.record.https_port = redeemed.https_port;
                host.apps = Apps::NotLoaded;
                crate::logfile::add_secrets(&host.record);
                host.id
            }
            None => self.insert(HostRecord {
                unique_id: redeemed.unique_id.clone(),
                name: redeemed.name.clone(),
                address: redeemed.address.ip().to_string(),
                http_port: redeemed.address.port(),
                https_port: redeemed.https_port,
                server_certificate: Some(redeemed.server_certificate_pem.clone()),
                internet_play: Some(redeemed.reply.clone()),
                manual: true,
                ..Default::default()
            }),
        };
        self.poll_one(id);
        Ok(id)
    }

    /// Adds a host by address and checks it at once. Returns its ID. No port is the default one,
    /// as a PC is an address and a port: a PC listed there under another port is another PC.
    pub fn add_manual(&mut self, address: &str, port: Option<u16>) -> u64 {
        let wanted = http_port(port.unwrap_or(DEFAULT_HTTP_PORT));
        let here = |h: &Host| {
            h.record.address.eq_ignore_ascii_case(address)
                || h.record.other_addresses.iter().any(|a| a.eq_ignore_ascii_case(address))
                || h.record.pinned_address.as_deref().is_some_and(|a| a.eq_ignore_ascii_case(address))
                || h.record.remote_address.as_deref().is_some_and(|a| a.eq_ignore_ascii_case(address))
        };
        let listed = self
            .list
            .iter()
            .position(|h| here(h) && http_port(h.record.http_port) == wanted)
            .or_else(|| self.list.iter().position(|h| here(h) && h.record.other_http_ports.contains(&wanted)));
        if let Some(host) = listed.map(|at| &mut self.list[at]) {
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

    /// Keeps the MACs the user added to wake the PC by. False for an unknown PC.
    pub fn set_own_wake_macs(&mut self, id: u64, macs: Vec<String>) -> bool {
        let Some(host) = self.get_mut(id) else {
            return false;
        };
        host.record.own_wake_macs = macs;
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
        self.merged_into.retain(|&k, &mut v| k != id && v != id);
        self.drop_workers(id);
    }

    /// Follows any `unique_id` merges so an old host ID resolves to the surviving PC record.
    pub fn canonical_id(&self, mut id: u64) -> u64 {
        for _ in 0..8 {
            let Some(&next) = self.merged_into.get(&id) else {
                break;
            };
            id = next;
        }
        id
    }

    /// Records that `gone` was merged into `keep`, updating any transitive chain.
    pub(super) fn record_merge(&mut self, gone: u64, keep: u64) {
        for target in self.merged_into.values_mut() {
            if *target == gone {
                *target = keep;
            }
        }
        self.merged_into.insert(gone, keep);
    }
}

struct HostFactsSource(bool);

impl nightjar_desktop_ui::hosts_model::FactsSource for HostFactsSource {
    fn offers_facts(&self) -> bool {
        self.0
    }
    fn cached(&self, _key: &str) -> Option<nightjar_policy::game_info::cache::Facts> {
        None
    }
}

impl Hosts {
    /// Applies mutations written by `ui::demo::{fill, covers}` onto `self`.
    #[cfg(feature = "demo-library")]
    pub fn apply_ui_demo(&mut self, view: nightjar_desktop_ui::hosts_model::Hosts) {
        self.facts = view.facts;
        for model in view.list {
            if let Some(host) = self.get_mut(model.id) {
                host.record = model.record;
                host.reach = model.reach;
                host.info = model.info;
                host.apps = model.apps;
                host.art = model.art;
                host.ambient = model.ambient;
                host.ambient_glow = model.ambient_glow;
                host.art_versions = model.art_versions;
                host.art_sized = model.art_sized;
                host.art_retry = model.art_retry;
                host.native = model.native;
                host.classic_apps = model.classic_apps;
                host.classic = model.classic;
                host.error = model.error;
                host.checking = model.checking;
                host.via_remote = model.via_remote;
            }
        }
    }

    /// UI-crate view of the hosts list and cached game facts.
    pub fn ui_view(&self) -> nightjar_desktop_ui::hosts_model::Hosts {
        let mut out = nightjar_desktop_ui::hosts_model::Hosts::without_discovery(Vec::new(), self.waker.clone());
        out.discovering = self.discovering;
        out.has_identity = self.identity.is_some();
        out.facts = Arc::clone(&self.facts);
        out.facts_asked = Arc::clone(&self.facts_asked);
        out.set_facts_source(Some(Arc::new(HostFactsSource(self.facts_offered()))));
        out.list = self.list.iter().map(Host::ui_view).collect();
        out
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

    pub fn quit_app(&mut self, id: u64) {
        let Some(identity) = self.identity.clone() else {
            return;
        };
        let Some(host) = self.get(id) else {
            return;
        };
        let record = host.connect_record();
        let pending = Arc::new((Mutex::new(false), Condvar::new()));
        self.pending_quits.push(Arc::clone(&pending));
        let guard = QuitGuard(pending);
        self.spawn("cancel", move || {
            let _guard = guard;
            let result = client_for(&identity, &record, &record.address).and_then(|c| match c.cancel() {
                Ok(true) => Ok(()),
                Ok(false) => Err(en("The PC declined to quit the app.").into()),
                Err(nightjar_core::http::Error::Status { code: 599, .. }) => Err(QUIT_OTHER_DEVICE.into()),
                Err(e) => Err(describe(&e)),
            });
            Some(Event::Quit { id, result })
        });
    }

    /// Whether any `/cancel` quit request is still in flight.
    pub(crate) fn has_pending_quits(&mut self) -> bool {
        self.pending_quits.retain(|p| !*p.0.lock().unwrap_or_else(|e| e.into_inner()));
        !self.pending_quits.is_empty()
    }

    /// Waits up to `timeout` for any in-flight `/cancel` quit requests to finish before exit.
    pub(crate) fn wait_quits(&mut self, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        for pending in self.pending_quits.drain(..) {
            let (lock, cvar) = &*pending;
            let mut done = lock.lock().unwrap_or_else(|e| e.into_inner());
            while !*done {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    break;
                }
                let (guard, result) = cvar.wait_timeout(done, remaining).unwrap_or_else(|e| e.into_inner());
                done = guard;
                if result.timed_out() && !*done {
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests;
