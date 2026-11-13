//! The PCs Nightjar knows: discovery on the local network (mDNS `_nvstream._tcp`), hosts
//! added by address, their online state, pairing, the app list with box art, and quitting
//! the running app. Every host request runs on a worker thread and comes back as an
//! [`Event`]; the UI thread owns the list.

use std::collections::{BTreeMap, HashMap};
use std::net::{IpAddr, Ipv4Addr};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
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

mod covers;
mod events;
mod native;
mod pairing;
mod power;

pub use covers::*;
pub use native::*;
pub use pairing::*;
pub use power::*;

/// Wakes the UI thread after an event was queued.
pub type Waker = Arc<dyn Fn() + Send + Sync>;

const POLL_INTERVAL: Duration = Duration::from_secs(4);
/// After a quit, a closed game or a stream's end the PC changes its state a moment later, so it is
/// asked every second for this long (`Host::recheck_until`).
const RECHECK_INTERVAL: Duration = Duration::from_secs(1);
/// How often a PC's internet play reply is read again while it is reached on its network.
const INTERNET_READ_INTERVAL: Duration = Duration::from_secs(600);
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
    /// It was loaded and its texture given up for room ([`Hosts::covers_shown`]). Its file is on
    /// disk, so it loads again when it is next on screen, whether its PC is up or not.
    Dropped,
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
    /// The pixels per point each cover that was made smaller was sized for; a cover kept at its
    /// own size is not in it.
    art_sized: HashMap<i32, f32>,
    pub native: Native,
    /// The last app list of a PC without the native library, as saved for Home (live or from an
    /// earlier run), and the same as a library with the running app marked ([`Host::library`]).
    pub classic_apps: Option<crate::config::ClassicApps>,
    pub classic: Option<Library>,
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
    /// When the PC's internet play reply was last asked for on its network; None until then.
    internet_read: Option<Instant>,
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

    /// How this device reaches the PC right now, for the PC's page: the local network, IPv6, a
    /// forwarded port or a punched path. None while it is not reached.
    pub fn path(&self) -> Option<nightjar_core::internet::InternetPath> {
        if self.reach != Reach::Online {
            return None;
        }
        let address: IpAddr = self.connect_address().parse().ok()?;
        let lan: Vec<IpAddr> = std::iter::once(&self.record.address)
            .chain(&self.record.other_addresses)
            .filter_map(|a| a.parse().ok())
            .collect();
        let info = internet_reply(&self.record);
        let path = nightjar_core::internet::path_of(address, &lan, info.as_ref());
        // An address that is neither the PC's own nor from its reply is the remote one.
        if self.via_remote.is_some() && path == nightjar_core::internet::InternetPath::Lan {
            return Some(nightjar_core::internet::InternetPath::MappedIpv4);
        }
        Some(path)
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

    /// The games Home shows for the PC: its native library, else its app list as a library
    /// ([`crate::console::classic_library`]).
    pub fn library(&self) -> Option<&Library> {
        self.native.library.as_ref().or(self.classic.as_ref())
    }

    /// Takes in the PC's app list as Home keeps it: None drops it. True when it changed and is to
    /// be saved (or forgotten).
    fn set_classic_apps(&mut self, apps: Option<crate::config::ClassicApps>) -> bool {
        let changed = self.classic_apps != apps;
        self.classic_apps = apps;
        self.sync_classic();
        changed
    }

    /// Makes the app list's library again, with the app the PC reports running now (none while
    /// it is away).
    fn sync_classic(&mut self) {
        let running = self
            .info
            .as_ref()
            .filter(|_| self.reach == Reach::Online)
            .map(|i| (i.current_game, i.current_game_uuid.as_deref()));
        self.classic = self.classic_apps.as_ref().map(|apps| crate::console::classic_library(apps, running));
    }

    pub fn session_reads(&self) -> crate::open_play::SessionReads {
        crate::open_play::SessionReads { asked: self.native_seq, answered: self.session_seq }
    }

    /// Whether the cover of `app` at `version` has not been asked for yet.
    pub fn art_wanted(&self, app: i32, version: &str) -> bool {
        !self.has_art(app) || self.art_versions.get(&app).is_none_or(|known| replaces(known, version))
    }

    /// Whether the cover of `app` is here or on its way. One given up for room is neither.
    fn has_art(&self, app: i32) -> bool {
        matches!(self.art.get(&app), Some(Art::Loading | Art::Ready(_) | Art::Missing))
    }

    /// Whether the cover of `app` was given up for room and loads again from its file.
    pub fn art_dropped(&self, app: i32) -> bool {
        matches!(self.art.get(&app), Some(Art::Dropped))
    }

    /// Takes a cover asked for at `version` as loading and gives the version it loads at, or
    /// nothing when the cover there is, or the one on its way, is the one asked for.
    pub(crate) fn begin_art(&mut self, app: i32, version: &str) -> Option<String> {
        let known = self.art_versions.get(&app);
        if known.is_some_and(|known| replaces(known, version)) {
            self.art.remove(&app);
            self.ambient.remove(&app);
            self.ambient_glow.remove(&app);
            self.art_sized.remove(&app);
        }
        if self.has_art(app) {
            return None;
        }
        // Asked for without a version, a cover that was dropped (a new renderer, or for room)
        // loads as the one it was, from its file.
        let version = match known {
            Some(known) if version.is_empty() => known.clone(),
            _ => version.to_owned(),
        };
        self.art.insert(app, Art::Loading);
        self.art_versions.insert(app, version.clone());
        Some(version)
    }

    /// The version the cover of `app` is asked for with, the same on every page: the native
    /// library's where it lists the app, else the app list's `ArtVersion`, else none. Pages that
    /// asked with versions of their own had each other's covers fetched again.
    pub fn cover_version(&self, app: i32) -> &str {
        let listed = self.library().and_then(|l| l.games.iter().find(|g| g.app_id == app as u32));
        match (listed, &self.apps) {
            (Some(game), _) => &game.artwork.version,
            (None, Apps::Loaded(apps)) => apps.iter().find(|a| a.id == app).map_or("", |a| &a.art_version),
            (None, _) => "",
        }
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

    /// The version "Update the PC’s streaming app" installs: only where the PC lists `update`, has one
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
pub const PAIR_OFFLINE: &str = en("Wake the PC and start its streaming app. Use the same network, then retry.");
pub const PAIR_IN_GAME: &str = en("Stop the stream on the PC, then pair again.");
pub const PAIR_PIN_WRONG: &str =
    en("PIN rejected. Try pairing again and enter the new PIN shown on this device in the PC’s settings or web UI.");
pub const PAIR_OTP_PIN_WRONG: &str =
    en("PIN rejected. Generate a new one-time PIN in the PC’s web UI and check the passphrase.");
pub const PAIR_IN_PROGRESS: &str = en("Finish or cancel the pending request on the PC.");
pub const PAIR_DECLINED: &str = en("Cancel the old request in the PC’s settings or web UI, then retry.");
pub const PAIR_NETWORK: &str = en("Use the same network and start the PC’s streaming app. Check its firewall, then retry.");
pub const PAIR_LINK_MISMATCH: &str =
    en("This PC’s certificate does not match the pairing link, so Nightjar did not pair. Pair only with a link or QR code from your own PC.");
pub const LINK_INVALID: &str = en("That pairing link is not valid. Show a new QR code on the PC and copy its link again.");
pub const ADDRESS_INVALID: &str = en("That address is not valid");
pub const CLIPBOARD_NO_LINK: &str = en("Copy the link under the QR code on the PC’s Devices page, then paste it here.");
pub const UNPAIR_FAILED: &str = en("Failed to unpair. The PC may still list this device.");
pub const UNPAIR_NOT_PAIRED: &str = en("Device was not paired.");
// Why the PC handed out no sleep token. A refusal shows the host's own words instead.
pub const SLEEP_NOT_ALLOWED: &str = en("The PC does not let this device put it to sleep. Allow it in the PC’s device settings.");
pub const SLEEP_UNSUPPORTED: &str = en("The PC cannot sleep.");
pub const SLEEP_BUSY: &str = en("The PC cannot sleep right now. Close games and streams on it, then try again.");
// Restart and shut down. A refusal shows the host's own words instead.
pub const PC_POWER_NOT_ALLOWED: &str = en("The PC does not let this device restart or shut it down. Allow it in the PC’s device settings.");
pub const PC_POWER_BUSY: &str = en("The PC cannot do this right now. Close games and streams on it, then try again.");
// Approving a new device, without the permission for it. The console's own names, in every language.
pub const PAIRING_NOT_ALLOWED: &str = en("Needs Approve new devices on the PC’s Devices page.");

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
    /// The PC's `/internet-play/v1` reply as it came, read on its network.
    InternetPlay {
        id: u64,
        unique_id: String,
        result: Result<String, String>,
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
        /// The pixels per point `image` was made smaller for, None when it is at its own size.
        sized: Option<f32>,
    },
    /// A cover that was given up for room has no file to come back from, and its PC is away: it
    /// is asked for again when the PC is back.
    ArtAway {
        id: u64,
        app: i32,
        version: String,
    },
    /// A cover that was made smaller, loaded again for a screen that shows more of it.
    ArtSharper {
        id: u64,
        app: i32,
        version: String,
        image: egui::ColorImage,
        sized: Option<f32>,
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
    /// The demo's PCs while a first-run page shows a shorter list.
    #[cfg(feature = "demo-library")]
    pub(crate) demo_shelf: Vec<Host>,
}

/// The HTTP port a saved or sighted `port` means: 0 is the default one.
fn http_port(port: u16) -> u16 {
    if port == 0 {
        DEFAULT_HTTP_PORT
    } else {
        port
    }
}

pub fn client_for(identity: &Arc<ClientIdentity>, record: &HostRecord, address: &str) -> Result<HostClient, String> {
    let port = http_port(record.http_port);
    let mut client = HostClient::new(address, port, identity.clone())
        .map_err(|e| e.to_string())?
        .with_device_name(device_name());
    if crate::second_screen::this_process() {
        client = client.with_second_screen();
    }
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
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect(),
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
            art: HashMap::new(),
            ambient: HashMap::new(),
            ambient_glow: HashMap::new(),
            art_versions: HashMap::new(),
            art_sized: HashMap::new(),
            native: Native::default(),
            classic_apps: None,
            classic: None,
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
            internet_read: None,
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

    /// Starts looking for PCs on the network again, when it could not start (a network not up yet,
    /// or its port held). True when it runs now.
    pub fn search_again(&mut self) -> bool {
        if self._mdns.is_none() {
            self._mdns = self.start_discovery();
            self.discovering = self._mdns.is_some();
        }
        self.discovering
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

    /// Adds a host by address and checks it at once. Returns its ID. No port is the default one,
    /// as a PC is an address and a port: a PC listed there under another port is another PC.
    pub fn add_manual(&mut self, address: &str, port: Option<u16>) -> u64 {
        let wanted = http_port(port.unwrap_or(DEFAULT_HTTP_PORT));
        let here = |h: &Host| h.record.address.eq_ignore_ascii_case(address);
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
    /// Keeps the MACs the user added to wake the PC by. False for an unknown PC.
    pub fn set_own_wake_macs(&mut self, id: u64, macs: Vec<String>) -> bool {
        let Some(host) = self.get_mut(id) else {
            return false;
        };
        host.record.own_wake_macs = macs;
        true
    }

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
            let mut tried = Vec::new();
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
                    Err(e) => {
                        tried.push(address);
                        last_error = e;
                    }
                }
            }
            // Every direct path failed: a punched one, where the PC gave this device its key.
            if let Some(reply) = internet_reply(&record).filter(|_| record.pinned_address.is_none()) {
                let asked = |address: String| tried.contains(&address);
                let tried = nightjar_core::internet::Tried {
                    lan: asked(record.address.clone()),
                    ipv6: reply.ipv6.iter().any(|ip| asked(ip.to_string())),
                    mapped: reply.external_ipv4.is_some_and(|ip| asked(ip.to_string())),
                };
                let punched = nightjar_core::internet::punch(&record.unique_id, &reply, tried).and_then(|at| {
                    let address = at.ip().to_string();
                    client_for(&identity, &record, &address)
                        .and_then(|c| c.server_info(true).map_err(|e| describe(&e)))
                        .map(|info| (address, info))
                        .map_err(|e| nightjar_core::internet::unreachable(&reply, tried, Some(e)))
                });
                match punched {
                    Ok((address, info)) => {
                        return Some(Event::Info { id, seq, address, result: Ok(Box::new(info)) })
                    }
                    Err(words) => last_error = words,
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

    /// A new renderer (Linux's switch to the GPU renderer): the covers and ambients went with the
    /// old one and load again from the art cache as they are asked for ([`Self::request_art`]).
    #[cfg_attr(windows, allow(dead_code))]
    pub fn renderer_changed(&mut self) {
        for host in &mut self.list {
            host.art.retain(|_, art| !matches!(art, Art::Ready(_)));
            host.ambient.clear();
            host.art_sized.clear();
        }
        self.art_used.clear();
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
    // Then the internet paths in order: the PC's IPv6 addresses, then its forwarded IPv4 port
    // (the remote address, or the one its internet play reply names).
    let reply = internet_reply(record);
    let mut away: Vec<String> = Vec::new();
    if let Some(reply) = &reply {
        away.extend(reply.ipv6.iter().map(ToString::to_string));
    }
    away.extend(remote.cloned());
    if let Some(mapped) = reply.as_ref().filter(|r| r.mapped_ports).and_then(|r| r.external_ipv4) {
        away.push(mapped.to_string());
    }
    for address in away {
        if !addresses.contains(&address) {
            addresses.push(address);
        }
    }
    addresses
}

/// The PC's stored internet play reply, read again.
fn internet_reply(record: &HostRecord) -> Option<nightjar_core::http::InternetPlay> {
    record.internet_play.as_deref().and_then(|text| nightjar_core::http::InternetPlay::parse(text).ok())
}

/// Whether `address` reaches the PC from outside its network: its remote address, an internet
/// address from its reply, or a punched path. Requests go there for now; the PC stays stored
/// under its own address.
fn away_address(record: &HostRecord, address: &str) -> bool {
    if record.remote_address.as_deref() == Some(address) {
        return true;
    }
    let Ok(ip) = address.parse::<IpAddr>() else { return false };
    if nightjar_core::internet::is_tunnel(ip) {
        return true;
    }
    internet_reply(record).is_some_and(|reply| match ip {
        IpAddr::V6(ip) => reply.ipv6.contains(&ip),
        IpAddr::V4(ip) => reply.external_ipv4 == Some(ip),
    })
}

/// `text` as an IPv4 address that a PC outside this network can be reached at.
fn public_ipv4(text: &str) -> Option<String> {
    let address: Ipv4Addr = text.trim().parse().ok()?;
    let local = address.is_private() || address.is_loopback() || address.is_link_local() || address.is_unspecified()
        || address.is_broadcast() || address.is_multicast() || address.is_documentation();
    (!local).then(|| address.to_string())
}

#[cfg(test)]
pub(crate) mod tests;
