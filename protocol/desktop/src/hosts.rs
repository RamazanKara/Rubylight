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

/// The largest a cover is drawn, in points: the game page's. Every card, Home's hero and a PC's
/// screen are smaller.
pub const COVER_POINTS: [f32; 2] = [420.0, 630.0];

/// The pixels of a cover kept for each pixel of the largest place it is drawn in. Kept at
/// exactly that place's size, a cover shows soft wherever it is drawn a little smaller, as the
/// game page does in a narrow window: the renderer then reads between its pixels. With half as
/// many again it shows as sharp there as a store's 600 by 900 cover always has, which stays as
/// it is on a plain screen.
const COVER_DETAIL: f32 = 1.5;

/// A cover is made smaller only when that takes it under this share of its side: one already
/// near what is kept would lose a little sharpness for a little memory.
const COVER_SHRINK_BELOW: f32 = 0.9;

/// How much the cover textures may hold, in bytes of pixels. Past it the ones longest off screen
/// are given up and load again from their files when they come back; the ones on screen stay
/// whatever they add up to. A few hundred covers fit, so a long list scrolls back and forth
/// without reloading, and a library of thousands no longer keeps every cover it ever showed.
const COVER_BUDGET: usize = 256 << 20;

/// The covers drawn or asked for since [`Hosts::covers_shown`] last took them, by texture. A set,
/// so it holds no more than there are covers however many frames pass before it is taken.
#[derive(Clone, Default)]
struct CoversUsed(std::collections::HashSet<egui::TextureId>);

/// Notes that the cover `texture` is on screen (or needed off it) in this frame, so it is not one
/// given up for room.
pub fn cover_used(ctx: &egui::Context, texture: &egui::TextureHandle) {
    ctx.data_mut(|d| d.get_temp_mut_or_default::<CoversUsed>(egui::Id::NULL).0.insert(texture.id()));
}

/// The covers noted as in use since they were last taken, for a test of the pages that note them.
#[cfg(test)]
pub(crate) fn covers_noted(ctx: &egui::Context) -> std::collections::HashSet<egui::TextureId> {
    ctx.data_mut(|d| std::mem::take(&mut d.get_temp_mut_or_default::<CoversUsed>(egui::Id::NULL).0))
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

    /// As [`Self::spawn`], for a cover: on one of the few threads covers load on.
    fn load_art(&self, work: impl FnOnce() -> Option<Event> + Send + 'static) {
        if cfg!(feature = "demo-library") {
            return;
        }
        let sender = self.sender.clone();
        let waker = self.waker.clone();
        self.art_pool.run(move || {
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
        host.art_sized.clear();
        host.native = Native::default();
        host.set_classic_apps(None);
        host.native_epoch += 1;
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

    /// Whether this PC makes a stream of another device's game a guest of that device
    /// (`session.guests`), as its native capabilities last said.
    pub fn offers_session_guests(&self, id: u64) -> bool {
        self.get(id)
            .and_then(|host| host.native.capabilities.as_ref())
            .is_some_and(nightjar_core::http::NativeCapabilities::offers_session_guests)
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
                // A PC without the native library: the app list it sent last.
                let classic = saved.is_none().then(|| store.classic_apps(&host.record.unique_id)).flatten();
                host.native.library = saved.map(|l| l.library);
                host.set_classic_apps(classic);
            }
        }
    }

    /// The app lists read since the last call, by the PC's `uniqueid`: Some to save, None to
    /// forget (the PC turned out to have the native library).
    pub fn take_classic_saves(&mut self) -> Vec<(String, Option<crate::config::ClassicApps>)> {
        std::mem::take(&mut self.classic_saves)
            .into_iter()
            .filter_map(|id| {
                let host = self.get(id)?;
                // A PC unpaired while its list was in flight must not get it back.
                host.record.server_certificate.as_ref()?;
                Some((host.record.unique_id.clone(), host.classic_apps.clone()))
            })
            .filter(|(unique_id, _)| !unique_id.is_empty())
            .collect()
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

    /// Fetches box art once per app, from the disk cache when present. `version` is the cover's
    /// version ([`Host::cover_version`], empty when the PC gives none); a new one fetches again,
    /// and none keeps the cover that is known. At most [`ART_WORKERS`] covers load at once.
    pub fn request_art(&mut self, id: u64, app: i32, version: &str, store: &Store) {
        let Some(identity) = self.identity.clone() else {
            return;
        };
        let Some(host) = self.get_mut(id) else {
            return;
        };
        let Some(version) = host.begin_art(app, version) else {
            return;
        };
        // Only a cover that was given up for room is asked for while its PC is away, for its file.
        let away = host.reach != Reach::Online;
        let record = host.connect_record();
        let cache = store.art_path(&record.unique_id, app, &version);
        let files = self.art_files.clone();
        let scale = self.art_scale;
        self.load_art(move || {
            let file = std::fs::read(&cache).ok();
            if file.is_none() && away {
                return Some(Event::ArtAway { id, app, version });
            }
            let bytes = file.or_else(|| {
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
                    files.lock().unwrap_or_else(std::sync::PoisonError::into_inner).saved(&cache, app);
                }
                Some(bytes)
            });
            let (image, ambient, sized) = art_and_ambient(bytes, scale);
            Some(Event::Art { id, app, version, image, ambient, sized })
        });
    }

    /// What the frame just drawn means for the covers. Call it after each launcher frame.
    ///
    /// The window's pixels per point are what covers are sized for from here on, and when they
    /// grew (a larger text size, a window moved to a denser screen) the covers made smaller for
    /// fewer are loaded again, each showing as it is until the sharper one lands. Then, when
    /// the cover textures hold more than their budget, the ones longest off screen are given up.
    pub fn covers_shown(&mut self, ctx: &egui::Context, store: &Store) {
        self.art_frame += 1;
        let used = ctx.data_mut(|d| std::mem::take(&mut d.get_temp_mut_or_default::<CoversUsed>(egui::Id::NULL).0));
        for texture in used {
            self.art_used.insert(texture, self.art_frame);
        }
        let scale = ctx.pixels_per_point();
        let grew = scale > self.art_scale;
        self.art_scale = scale;
        if grew || self.art_landed {
            self.sharpen_covers(store);
        }
        if std::mem::take(&mut self.art_landed) {
            self.drop_covers(COVER_BUDGET);
        }
    }

    /// Loads again, from their files, the covers that were made smaller for fewer pixels per
    /// point than the window has now.
    fn sharpen_covers(&mut self, store: &Store) {
        let scale = self.art_scale;
        let mut jobs = Vec::new();
        for host in &mut self.list {
            for (&app, sized) in &mut host.art_sized {
                if *sized >= scale || !matches!(host.art.get(&app), Some(Art::Ready(_))) {
                    continue;
                }
                let Some(version) = host.art_versions.get(&app) else { continue };
                // Taken as sized for the new scale from here, so it is asked for once.
                *sized = scale;
                jobs.push((host.id, app, version.clone(), store.art_path(&host.record.unique_id, app, version)));
            }
        }
        for (id, app, version, cache) in jobs {
            self.load_art(move || {
                // Only from its file: the cover that shows is good enough to keep if that is gone.
                let image = art_image(std::fs::read(&cache).ok())?;
                let (image, sized) = cover_sized(image, scale);
                Some(Event::ArtSharper { id, app, version, image, sized })
            });
        }
    }

    /// Gives up cover textures until they hold no more than `budget` bytes: the ones longest off
    /// screen first, and never one that was drawn or asked for in the frame just shown, so a
    /// cover on screen cannot be dropped and loaded in turns. The ambient stays, so a page's
    /// backdrop does not change with it.
    fn drop_covers(&mut self, budget: usize) {
        // The demo's covers are drawn into textures and have no file to come back from.
        if cfg!(feature = "demo-library") {
            return;
        }
        let mut covers = Vec::new();
        let mut held = 0;
        for (at, host) in self.list.iter().enumerate() {
            for (&app, art) in &host.art {
                if let Art::Ready(texture) = art {
                    held += texture.byte_size();
                    covers.push((self.art_used.get(&texture.id()).copied().unwrap_or(0), at, app, texture.id(), texture.byte_size()));
                }
            }
        }
        // The textures that are gone (dropped here, replaced, their PC removed) need no stamp.
        let live: std::collections::HashSet<egui::TextureId> = covers.iter().map(|cover| cover.3).collect();
        self.art_used.retain(|texture, _| live.contains(texture));
        if held <= budget {
            return;
        }
        covers.sort_unstable_by_key(|&(used, at, app, ..)| (used, at, app));
        for (used, at, app, texture, bytes) in covers {
            if held <= budget || used >= self.art_frame {
                break;
            }
            let host = &mut self.list[at];
            host.art.insert(app, Art::Dropped);
            host.art_sized.remove(&app);
            self.art_used.remove(&texture);
            held -= bytes;
        }
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
                    self.follow_classic(id);
                    self.check_foundation_probe(id);
                    self.follow_internet_play(id);
                }
                Event::InternetPlay { id, unique_id, result } => {
                    let Some(host) = self.get_mut(id) else { continue };
                    match result {
                        // Another PC answers at this address by now: the reply is not its.
                        Ok(text) if host.record.unique_id == unique_id => {
                            if host.record.internet_play.as_deref() != Some(text.as_str()) {
                                host.record.internet_play = Some(text);
                                crate::logfile::add_secrets(&host.record);
                                changed = true;
                            }
                        }
                        Ok(_) => {}
                        Err(e) => eprintln!("nightjar: the PC's internet play details could not be read: {e}"),
                    }
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
                    let mut save = false;
                    if let Some(host) = self.get_mut(id) {
                        host.apps = match result {
                            Ok(mut apps) => {
                                apps.sort_by_key(|a| a.host_index.unwrap_or(u32::MAX));
                                // Home shows a paired PC's app list where it has no native library,
                                // and keeps it for when the PC is away.
                                if host.record.server_certificate.is_some() && !host.native_api() {
                                    save = host.set_classic_apps(Some(crate::config::ClassicApps::of(&apps)));
                                } else if host.native_api() {
                                    save = host.set_classic_apps(None);
                                }
                                Apps::Loaded(apps)
                            }
                            Err(e) => Apps::Failed(e),
                        };
                    }
                    if save && !self.classic_saves.contains(&id) {
                        self.classic_saves.push(id);
                    }
                }
                Event::ArtAway { id, app, version } => {
                    if let Some(host) = self.get_mut(id).filter(|h| h.art_versions.get(&app) == Some(&version)) {
                        // As a cover that was never asked for: the pages ask when the PC is up.
                        if matches!(host.art.get(&app), Some(Art::Loading)) {
                            host.art.remove(&app);
                        }
                    }
                }
                Event::ArtSharper { id, app, version, image, sized } => {
                    let frame = self.art_frame;
                    let Some(host) = self.list.iter_mut().find(|h| h.id == id).filter(|h| h.art_versions.get(&app) == Some(&version)) else { continue };
                    // Still the cover it was loaded again for: one given up or replaced since is not.
                    let Some(Art::Ready(texture)) = host.art.get_mut(&app) else { continue };
                    *texture = ctx.load_texture(format!("art-{id}-{app}"), image, egui::TextureOptions::LINEAR);
                    // A newer scale's may be on its way already; it is the one `art_sized` names.
                    match sized {
                        Some(sized) => {
                            let asked = host.art_sized.entry(app).or_insert(sized);
                            *asked = asked.max(sized);
                        }
                        None => {
                            host.art_sized.remove(&app);
                        }
                    }
                    self.art_used.insert(texture.id(), frame);
                    self.art_landed = true;
                }
                Event::Art { id, app, version, image, ambient, sized } => {
                    let frame = self.art_frame;
                    let mut landed = None;
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
                            Some(image) => {
                                let texture = ctx.load_texture(format!("art-{id}-{app}"), image, egui::TextureOptions::LINEAR);
                                landed = Some(texture.id());
                                Art::Ready(texture)
                            }
                            None => Art::Missing,
                        };
                        host.art.insert(app, art);
                        host.art_sized.remove(&app);
                        if let Some(sized) = sized.filter(|_| landed.is_some()) {
                            host.art_sized.insert(app, sized);
                        }
                    }
                    // It was asked for because it is on screen: not the first to go for room.
                    if let Some(texture) = landed {
                        self.art_used.insert(texture, frame);
                        self.art_landed = true;
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

    /// A paired PC that was just checked and has no native API: read its app list once per run,
    /// so Home shows its games and a start from Home has the list it goes through.
    fn follow_classic(&mut self, id: u64) {
        let Some(host) = self.get(id) else { return };
        if host.reach == Reach::Online && host.paired() && host.info.is_some() && !host.native_api() && matches!(host.apps, Apps::NotLoaded) {
            self.load_apps(id);
        }
    }

    /// Asks a paired Sunshine-family PC once whether it has Foundation Sunshine's bandwidth test,
    /// so Test speed is offered only where it runs. A request that did not get through is asked
    /// again after the next poll.
    /// Reads the PC's internet play reply while it is reached on its own network and offers
    /// it, at most every `INTERNET_READ_INTERVAL`: the device keeps it for when it is away.
    fn follow_internet_play(&mut self, id: u64) {
        let Some(identity) = self.identity.clone() else { return };
        let Some(host) = self.get_mut(id) else { return };
        let Some(info) = host.info.as_ref() else { return };
        let offered = info.paired && info.capabilities.has(Feature::InternetPlay);
        let due = host.internet_read.is_none_or(|at| at.elapsed() >= INTERNET_READ_INTERVAL);
        if host.reach != Reach::Online || !offered || !due || host.via_remote.is_some() {
            return;
        }
        host.internet_read = Some(Instant::now());
        let capabilities = info.capabilities.clone();
        let record = host.connect_record();
        self.spawn("internet-play", move || {
            let result = client_for(&identity, &record, &record.address).and_then(|c| {
                c.internet_play(&capabilities).map(|(text, _)| text).map_err(|e| e.to_string())
            });
            Some(Event::InternetPlay { id, unique_id: record.unique_id.clone(), result })
        });
    }

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
                let keep_id = self.list[keep].id;
                let kept = &mut self.list[keep].record;
                // The PC keeps the port it is listed under. It answered on the dropped one's too,
                // so a sighting there is that PC from now on, also after Nightjar restarts.
                let own = http_port(kept.http_port);
                for port in std::iter::once(dropped.http_port).chain(dropped.other_http_ports.iter().copied()).map(http_port) {
                    if port != own && !kept.other_http_ports.contains(&port) {
                        kept.other_http_ports.push(port);
                    }
                }
                kept.manual |= dropped.manual;
                if kept.alias.is_none() {
                    kept.alias = dropped.alias;
                }
                if kept.pace.is_none() { kept.pace = dropped.pace; }
                if kept.wake_macs.is_empty() && kept.wake_broadcasts.is_empty() {
                    kept.wake_macs = dropped.wake_macs;
                    kept.wake_broadcasts = dropped.wake_broadcasts;
                }
                for mac in dropped.own_wake_macs {
                    if !kept.own_wake_macs.contains(&mac) {
                        kept.own_wake_macs.push(mac);
                    }
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
                let remote = away_address(&host.record, &address);
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
                        host.record.other_http_ports.clear();
                        host.record.full_chroma = Default::default();
                    }
                    host.native = Native::default();
                    host.set_classic_apps(None);
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
                    host.set_classic_apps(None);
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
        host.sync_classic();
        changed
    }

    /// Takes in an mDNS sighting. True when a listed PC's stored address changed.
    fn apply_discovery(&mut self, name: String, addresses: Vec<IpAddr>, port: u16) -> bool {
        let Some(first) = addresses.first() else {
            return false;
        };
        let texts: Vec<String> = addresses.iter().map(IpAddr::to_string).collect();
        // A PC is an address and a port: another port on the same address is another PC (a
        // second host on that machine, or one added by hand with its own port), until it
        // answers with a listed PC's unique ID.
        let port = http_port(port);
        let at_address = |h: &Host| texts.contains(&h.record.address) || h.record.other_addresses.iter().any(|a| texts.contains(a));
        let listed = self
            .list
            .iter()
            .position(|h| http_port(h.record.http_port) == port && at_address(h))
            .or_else(|| self.list.iter().position(|h| h.record.other_http_ports.contains(&port) && at_address(h)));
        if let Some(host) = listed.map(|at| &mut self.list[at]) {
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
            http_port: port,
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

/// Whether a cover asked for at `asked` replaces the one known at `known`. No version says nothing
/// of the cover (an app list that is not read yet has none), so it never replaces a known one.
fn replaces(known: &str, asked: &str) -> bool {
    !asked.is_empty() && known != asked
}

/// How many covers load at once. A long list asks for its covers together, and each one is a
/// thread and a TLS handshake with the PC; the rest wait their turn.
const ART_WORKERS: usize = 4;

type ArtJob = Box<dyn FnOnce() + Send>;

/// The covers waiting to load and the threads loading them, at most [`ART_WORKERS`].
#[derive(Default)]
struct ArtQueue {
    waiting: std::collections::VecDeque<ArtJob>,
    workers: usize,
}

/// Loads covers in the order they were asked for. A worker lives while covers wait and ends with
/// the last one, so an idle launcher keeps no thread.
#[derive(Clone, Default)]
struct ArtPool(Arc<Mutex<ArtQueue>>);

impl ArtPool {
    fn queue(&self) -> std::sync::MutexGuard<'_, ArtQueue> {
        // Held only to move a job: a cover that panics does so outside it.
        self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn run(&self, job: impl FnOnce() + Send + 'static) {
        let mut queue = self.queue();
        queue.waiting.push_back(Box::new(job));
        if queue.workers == ART_WORKERS {
            return;
        }
        queue.workers += 1;
        drop(queue);
        let pool = self.clone();
        if std::thread::Builder::new().name("boxart".into()).spawn(move || pool.work()).is_err() {
            // The covers stay queued for the workers there are, or the next one that starts.
            self.queue().workers -= 1;
        }
    }

    fn work(&self) {
        loop {
            // The seat is given up under the lock that found nothing waiting, so a cover queued
            // right after starts a worker of its own.
            let job = {
                let mut queue = self.queue();
                match queue.waiting.pop_front() {
                    Some(job) => job,
                    None => {
                        queue.workers -= 1;
                        return;
                    }
                }
            };
            // A cover that panics takes neither the worker nor the covers behind it.
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job));
        }
    }
}

/// An app's cover from its box art bytes. A host's grey placeholder is no cover, so the card shows the title.
fn art_image(bytes: Option<Vec<u8>>) -> Option<egui::ColorImage> {
    bytes.filter(|b| !nightjar_core::http::is_placeholder_box_art(b)).and_then(|b| decode_image(&b))
}

/// A cover and its ambient, both off the UI thread: the first draw only uploads two finished images.
/// The ambient is baked from the cover as its file has it; the cover is then made no larger than a
/// window of `scale` pixels per point needs ([`cover_sized`]), and the last value says for which.
fn art_and_ambient(bytes: Option<Vec<u8>>, scale: f32) -> (Option<egui::ColorImage>, Option<egui::ColorImage>, Option<f32>) {
    let Some(image) = art_image(bytes) else { return (None, None, None) };
    let ambient = crate::ambient::bake(&image);
    let (image, sized) = cover_sized(image, scale);
    (Some(image), Some(ambient), sized)
}

/// The size a cover of `size` pixels is kept at for a window of `scale` pixels per point, or None
/// to keep it as it is. A cover fills its place and is cut to it (`cards::cover_uv`), so it is
/// the side that needs the most that decides, and what shows keeps [`COVER_DETAIL`] of its
/// pixels for each of the screen's.
fn cover_size(size: [usize; 2], scale: f32) -> Option<[usize; 2]> {
    let [w, h] = size;
    if w == 0 || h == 0 {
        return None;
    }
    let detail = scale * COVER_DETAIL;
    let share = (COVER_POINTS[0] * detail / w as f32).max(COVER_POINTS[1] * detail / h as f32);
    // Up to the next pixel, less what the arithmetic is off by (900 by 0.7 is not to become 631).
    let side = |from: usize| ((from as f32 * share - 0.01).ceil() as usize).clamp(1, from);
    (share < COVER_SHRINK_BELOW).then(|| [side(w), side(h)])
}

/// A cover no larger than a window of `scale` pixels per point needs, and that scale when it
/// was made smaller. Its pixels are already multiplied by their alpha, which is what resampling
/// wants: a clear pixel's colour does not bleed into the ones beside it.
fn cover_sized(image: egui::ColorImage, scale: f32) -> (egui::ColorImage, Option<f32>) {
    let Some([w, h]) = cover_size(image.size, scale) else { return (image, None) };
    let [from_w, from_h] = image.size;
    let bytes: Vec<u8> = image.pixels.iter().flat_map(|pixel| pixel.to_array()).collect();
    let Some(full) = image::RgbaImage::from_raw(from_w as u32, from_h as u32, bytes) else {
        return (image, None);
    };
    let small = image::imageops::resize(&full, w as u32, h as u32, image::imageops::FilterType::Lanczos3);
    (egui::ColorImage::from_rgba_premultiplied([w, h], small.as_raw()), Some(scale))
}

fn decode_image(bytes: &[u8]) -> Option<egui::ColorImage> {
    let image = image::load_from_memory(bytes).ok()?.to_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    Some(egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw()))
}

#[cfg(test)]
pub(crate) mod tests;
