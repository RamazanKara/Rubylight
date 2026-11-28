//! Pure UI host-list models, URI entry parsers, pairing/power UI messages and fixture helpers
//! shared by `nightjar-desktop-ui` without mDNS discovery, TLS (`ClientIdentity`) or background
//! networking threads (`nightjar-core`).

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::{Duration, Instant};

use nightjar_policy::game_info::cache::Facts;
use nightjar_policy::game_info::job::Game as FactsGame;
use nightjar_policy::game_info::names;
use nightjar_policy::http::native_api::library::Library;
use nightjar_policy::http::native_api::session::Session;
use nightjar_policy::http::{
    App, AppControl, Availability, Feature, HostUpdate, NativeCapabilities,
    PairingRequest, PcPower, Reason, ServerInfo,
};
use nightjar_policy::hub_sync::{ProfileConflict, SyncStatus};
use nightjar_policy::internet_path::InternetPath;
use rubylight_native_api::hub::{HostOverrideSummary, InputDeckSummary, SyncProfile};
use rubylight_protocol::invite_uri::InviteUri;
use rubylight_protocol::pairing_uri::PairingUri;

use crate::config::{self, HostRecord};
use crate::strings::{en, tr, trf};

pub use crate::hosts_types::Waker;

/// The largest a cover is drawn, in points: the game page's. Every card, Home's hero and a PC's
/// screen are smaller.
pub const COVER_POINTS: [f32; 2] = [420.0, 630.0];

/// How long after a failed box-art load before asking the PC again.
const ART_RETRY_INTERVAL: Duration = Duration::from_secs(30);
/// Grandfathered `strings/*.rs` key anchor; UI now renders `"{} is running. Playing this game quits it."` from `strings_70`.
#[doc(hidden)]
pub const LEGACY_REPLACE_NOTE: &str = en("{} is running. Playing this game closes it.");

/// Notes that the cover `texture` is on screen (or needed off it) in this frame, so it is not one
/// given up for room.
#[derive(Clone, Default)]
struct CoversUsed(std::collections::HashSet<egui::TextureId>);

pub fn cover_used(ctx: &egui::Context, texture: &egui::TextureHandle) {
    ctx.data_mut(|d| d.get_temp_mut_or_default::<CoversUsed>(egui::Id::NULL).0.insert(texture.id()));
}

/// The covers noted as in use since they were last taken, for a test of the pages that note them.
pub fn covers_noted(ctx: &egui::Context) -> std::collections::HashSet<egui::TextureId> {
    ctx.data_mut(|d| std::mem::take(&mut d.get_temp_mut_or_default::<CoversUsed>(egui::Id::NULL).0))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reach {
    Unknown,
    Online,
    Offline,
}

#[derive(Clone, Debug)]
pub enum Apps {
    NotLoaded,
    Loading,
    Loaded(Arc<Vec<App>>),
    Failed(String),
}

#[derive(Clone)]
pub enum Art {
    Loading,
    Ready(egui::TextureHandle),
    Missing,
    /// It was loaded and its texture given up for room. Its file is on disk, so it loads again
    /// when it is next on screen, whether its PC is up or not.
    Dropped,
}

/// The time a sleep or PC power token was handed out by the host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Handed(pub Instant);

impl Handed {
    pub fn now() -> Self {
        Self(Instant::now())
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

/// What the Rubylight native API told us about a PC, cached per PC.
#[derive(Clone, Default)]
pub struct Native {
    /// Its features and this device's permissions; `Host::info` carries them merged in.
    pub capabilities: Option<NativeCapabilities>,
    pub library: Option<Arc<Library>>,
    /// When the library's games were last played, by install id (`NativeLibrary::last_played`).
    pub last_played: Arc<BTreeMap<String, u64>>,
    /// How long the library's games were played, by install id (`NativeLibrary::playtime_minutes`).
    pub playtime_minutes: Arc<BTreeMap<String, u64>>,
    /// The collections the library's games are in, by install id (`NativeLibrary::collections`).
    pub collections: Arc<BTreeMap<String, Vec<String>>>,
    /// The library was read from the PC in this run (not loaded from disk).
    pub library_live: bool,
    pub session: Option<Session>,
    /// Why the last read failed; the data above is then the last good one.
    pub error: Option<String>,
    /// The PC's host update, where it lists `update`.
    pub update: Option<HostUpdate>,
    /// Devices waiting to pair, as last read, where this device may approve them.
    pub pairing_requests: Vec<PairingRequest>,
    /// Synced profiles, input decks, cover ETag state, and host override visibility.
    pub hub: HubStateView,
    /// A session action (Close game) is running.
    pub acting: bool,
    pub loading: bool,
    pub full_at: Option<Instant>,
}

/// UI summary of Host Hub Sync state for one paired PC.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HubStateView {
    pub status: SyncStatus,
    pub snapshot_revision: u64,
    pub profiles: Vec<SyncProfile>,
    pub input_decks: Vec<InputDeckSummary>,
    pub host_overrides: Vec<HostOverrideSummary>,
    pub pending_outbox_count: usize,
    pub conflict: Option<Box<ProfileConflict>>,
    pub error: Option<String>,
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
        crate::session_page::Hints {
            launch: permissions.is_some_and(|p| p.launch),
            host_control: self.host_control(),
        }
    }
}

/// Takes what the native API said into serverinfo's capabilities, so every gate on them
/// (shared folders, co-op, the image clipboard) sees it.
pub fn merge_native(info: &mut ServerInfo, native: Option<&NativeCapabilities>) {
    if let Some(native) = native {
        info.capabilities = info.capabilities.with_native(native);
    }
}

/// Pure UI view of one PC.
pub struct Host {
    pub id: u64,
    pub record: HostRecord,
    pub reach: Reach,
    pub info: Option<ServerInfo>,
    pub apps: Apps,
    pub art: Arc<HashMap<i32, Art>>,
    pub ambient: Arc<HashMap<i32, egui::TextureHandle>>,
    pub ambient_glow: Arc<HashMap<i32, egui::Color32>>,
    pub art_versions: Arc<HashMap<i32, String>>,
    pub art_sized: Arc<HashMap<i32, f32>>,
    pub art_retry: Arc<HashMap<i32, Instant>>,
    pub native: Native,
    pub classic_apps: Option<Arc<config::ClassicApps>>,
    pub classic: Option<Arc<Library>>,
    pub error: Option<String>,
    pub checking: bool,
    pub via_remote: Option<String>,
    pub path_override: Option<InternetPath>,
}

impl Host {
    pub fn new(id: u64, record: HostRecord) -> Self {
        Self {
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
            error: None,
            checking: false,
            via_remote: None,
            path_override: None,
        }
    }

    pub fn connect_address(&self) -> &str {
        match self.via_remote.as_deref() {
            Some(remote) if self.record.pinned_address.is_none() => remote,
            _ => &self.record.address,
        }
    }

    pub fn path(&self) -> Option<InternetPath> {
        if self.reach != Reach::Online {
            return None;
        }
        if let Some(path) = self.path_override {
            return Some(path);
        }
        Some(if self.via_remote.is_some() {
            InternetPath::MappedIpv4
        } else {
            InternetPath::Lan
        })
    }

    pub fn art_dropped(&self, app: i32) -> bool {
        matches!(self.art.get(&app), Some(Art::Dropped))
    }

    pub fn art_wanted(&self, app: i32, version: &str) -> bool {
        !self.has_art(app) || self.art_versions.get(&app).is_none_or(|known| known != version && !version.is_empty())
    }

    fn has_art(&self, app: i32) -> bool {
        match self.art.get(&app) {
            Some(Art::Missing) => self.art_retry.get(&app).is_none_or(|&at| Instant::now() < at),
            art => matches!(art, Some(Art::Loading | Art::Ready(_))),
        }
    }

    pub fn begin_art(&mut self, app: i32, version: &str) -> Option<String> {
        let known = self.art_versions.get(&app);
        if known.is_some_and(|known| !version.is_empty() && known != version) {
            Arc::make_mut(&mut self.art).remove(&app);
            Arc::make_mut(&mut self.ambient).remove(&app);
            Arc::make_mut(&mut self.ambient_glow).remove(&app);
            Arc::make_mut(&mut self.art_sized).remove(&app);
        }
        if self.has_art(app) {
            return None;
        }
        Arc::make_mut(&mut self.art_retry).remove(&app);
        let version = match known {
            Some(known) if version.is_empty() => known.clone(),
            _ => version.to_owned(),
        };
        Arc::make_mut(&mut self.art).insert(app, Art::Loading);
        Arc::make_mut(&mut self.art_versions).insert(app, version.clone());
        Some(version)
    }

    pub fn bandwidth_test(&self) -> bool {
        self.info.as_ref().is_some_and(|i| i.capabilities.has(Feature::PyroWaveBandwidthProbe))
    }

    pub fn paired(&self) -> bool {
        self.record.server_certificate.is_some() && self.info.as_ref().is_none_or(|i| i.paired)
    }

    pub fn confirms_replacement(&self) -> bool {
        self.info.as_ref().is_some_and(|i| i.capabilities.has(Feature::HostConfirmsReplacement))
    }

    pub fn native_api(&self) -> bool {
        self.info.as_ref().is_some_and(|i| i.capabilities.has(Feature::NativeApi))
            || (self.info.is_none() && self.native.library.is_some())
    }

    pub fn library_arc(&self) -> Option<&Arc<Library>> {
        self.native.library.as_ref().or(self.classic.as_ref())
    }

    pub fn library(&self) -> Option<&Library> {
        self.library_arc().map(Arc::as_ref)
    }

    pub fn set_classic_apps(&mut self, apps: Option<config::ClassicApps>) -> bool {
        let changed = self.classic_apps.as_deref() != apps.as_ref();
        self.classic_apps = apps.map(Arc::new);
        self.sync_classic();
        changed
    }

    pub fn sync_classic(&mut self) {
        let running = self
            .info
            .as_ref()
            .filter(|_| self.reach == Reach::Online)
            .map(|i| (i.current_game, i.current_game_uuid.as_deref()));
        self.classic = self.classic_apps.as_deref().map(|apps| Arc::new(crate::console::classic_library(apps, running)));
    }

    pub fn cover_version(&self, app: i32) -> &str {
        let listed = self.library().and_then(|l| l.games.iter().find(|g| g.app_id == app as u32));
        match (listed, &self.apps) {
            (Some(game), _) => &game.artwork.version,
            (None, Apps::Loaded(apps)) => apps.iter().find(|a| a.id == app).map_or("", |a| &a.art_version),
            (None, _) => "",
        }
    }

    pub fn display_name(&self) -> &str {
        if let Some(alias) = self.record.alias.as_deref().filter(|a| !a.is_empty()) {
            alias
        } else if !self.record.name.is_empty() {
            &self.record.name
        } else {
            &self.record.address
        }
    }

    pub fn rename_prefill(&self) -> &str {
        self.record.alias.as_deref().filter(|a| !a.is_empty()).unwrap_or(&self.record.name)
    }

    pub fn running_app(&self) -> Option<i32> {
        self.info.as_ref().filter(|i| i.current_game != 0).map(|i| i.current_game)
    }

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

    pub fn pc_power_option(&self, action: PcPower) -> Option<Result<(), &'static str>> {
        if self.reach != Reach::Online || !self.paired() {
            return None;
        }
        let native = self.native.capabilities.as_ref()?;
        if !native.capabilities.features.iter().any(|f| f == action.feature()) {
            return None;
        }
        Some(if native.files.restart_shutdown {
            Ok(())
        } else {
            Err(PC_POWER_NOT_ALLOWED)
        })
    }

    pub fn pair_another_option(&self) -> Option<Result<(), &'static str>> {
        if self.reach != Reach::Online || !self.paired() {
            return None;
        }
        let native = self.native.capabilities.as_ref()?;
        if !native.offers_pairing_approve() {
            return None;
        }
        Some(if native.may_approve_pairing() {
            Ok(())
        } else {
            Err(PAIRING_NOT_ALLOWED)
        })
    }

    pub fn may_approve_pairing(&self) -> bool {
        self.reach == Reach::Online
            && self.paired()
            && self.native.capabilities.as_ref().is_some_and(NativeCapabilities::may_approve_pairing)
    }

    pub fn waiting_to_pair(&self) -> &[PairingRequest] {
        if self.may_approve_pairing() { &self.native.pairing_requests } else { &[] }
    }

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

/// Optional game-facts provider (backed by `crate::game_info::GameInfo` in `nightjar-desktop` or
/// test fixtures in `nightjar-desktop-ui`).
pub trait FactsSource: Send + Sync {
    fn offers_facts(&self) -> bool;
    fn cached(&self, key: &str) -> Option<Facts>;
}

/// The UI-level hosts list and cached IGDB facts.
pub struct Hosts {
    pub list: Vec<Host>,
    pub discovering: bool,
    pub has_identity: bool,
    facts_source: Option<Arc<dyn FactsSource>>,
    pub facts: Arc<HashMap<String, Facts>>,
    pub facts_asked: Arc<std::collections::HashSet<String>>,
    _waker: Waker,
}

impl Hosts {
    pub fn without_discovery(records: Vec<HostRecord>, waker: Waker) -> Self {
        let list = records
            .into_iter()
            .enumerate()
            .map(|(i, record)| Host::new(i as u64 + 1, record))
            .collect();
        Self {
            list,
            discovering: false,
            has_identity: false,
            facts_source: None,
            facts: Arc::new(HashMap::new()),
            facts_asked: Arc::new(std::collections::HashSet::new()),
            _waker: waker,
        }
    }

    pub fn get(&self, id: u64) -> Option<&Host> {
        self.list.iter().find(|h| h.id == id)
    }

    pub fn get_mut(&mut self, id: u64) -> Option<&mut Host> {
        self.list.iter_mut().find(|h| h.id == id)
    }

    pub fn iter(&self) -> std::slice::Iter<'_, Host> {
        self.list.iter()
    }

    pub fn set_facts_source(&mut self, source: Option<Arc<dyn FactsSource>>) {
        self.facts_source = source;
    }

    pub fn facts_offered(&self) -> bool {
        self.facts_source.as_ref().is_some_and(|s| s.offers_facts())
    }

    pub fn facts_asked(&self, key: &str) -> bool {
        self.facts_asked.contains(key)
    }

    pub fn facts(&self, key: &str) -> Option<&Facts> {
        self.facts.get(key)
    }

    pub fn put_facts(&mut self, key: &str, facts: Facts) {
        Arc::make_mut(&mut self.facts).insert(key.to_owned(), facts);
        Arc::make_mut(&mut self.facts_asked).insert(key.to_owned());
    }

    pub fn want_facts(&mut self, games: Vec<FactsGame>) {
        if games.is_empty() {
            return;
        }
        let facts_asked = Arc::make_mut(&mut self.facts_asked);
        let facts = Arc::make_mut(&mut self.facts);
        for game in games {
            let key = names::key(&game.name, game.steam);
            if !facts_asked.insert(key.clone()) {
                continue;
            }
            if let Some(cached) = self.facts_source.as_ref().and_then(|s| s.cached(&key)) {
                facts.insert(key, cached);
            }
        }
    }

    /// How many PCs are paired now that `id` has just paired: its last answer may still be the one
    /// from before the pairing, which said it was not.
    pub fn paired_with(&self, id: u64) -> usize {
        self.list.iter().filter(|h| h.id == id || h.paired()).count()
    }

    pub fn set_own_wake_macs(&mut self, id: u64, macs: Vec<String>) -> bool {
        let Some(host) = self.get_mut(id) else {
            return false;
        };
        if host.record.own_wake_macs == macs {
            return false;
        }
        host.record.own_wake_macs = macs;
        true
    }

    pub fn rename(&mut self, id: u64, alias: &str) -> bool {
        let Some(host) = self.get_mut(id) else {
            return false;
        };
        let given = alias.trim();
        let own = given == host.record.name || given == host.record.address;
        host.record.set_alias(if own { "" } else { alias });
        true
    }
}

/// The game a host lists, as a facts/artwork lookup takes it.
pub fn game_of(title: &str, game_id: Option<&str>, store: Option<(&str, &str)>) -> FactsGame {
    let steam = names::steam_of(game_id, store);
    FactsGame { name: title.to_owned(), steam }
}

/// What a launch can do with a PC's app list as it is now.
pub enum LaunchApp {
    Ready(App),
    /// The list is being read; the launch waits for it.
    Wait,
    /// No list was read yet.
    Gone,
    /// The list could not be read; the reason is shown.
    Failed(String),
    /// The list has no such app.
    Missing,
}

pub fn launch_app(apps: &Apps, app_id: i32) -> LaunchApp {
    match apps {
        Apps::Loading => LaunchApp::Wait,
        Apps::Loaded(apps) => apps
            .iter()
            .find(|app| app.id == app_id)
            .cloned()
            .map_or(LaunchApp::Missing, LaunchApp::Ready),
        Apps::NotLoaded => LaunchApp::Gone,
        Apps::Failed(why) => LaunchApp::Failed(why.clone()),
    }
}

/// What a launch starts on this PC.
pub fn launch_target(host: &Host, app_id: i32, resume: bool) -> LaunchApp {
    let listed = launch_app(&host.apps, app_id);
    if !resume {
        return listed;
    }
    let running = host.running_app() == Some(app_id);
    match listed {
        LaunchApp::Wait => LaunchApp::Wait,
        LaunchApp::Ready(app) if !app.is_control_action() => LaunchApp::Ready(app),
        _ if running => LaunchApp::Ready(running_app_unlisted(host, app_id)),
        LaunchApp::Ready(_) => LaunchApp::Missing,
        other => other,
    }
}

fn running_app_unlisted(host: &Host, app_id: i32) -> App {
    App {
        name: crate::ui::at_sentence_start(&crate::ui::running_name(host)),
        id: app_id,
        hdr_supported: false,
        uuid: host
            .info
            .as_ref()
            .and_then(|info| info.current_game_uuid.clone())
            .unwrap_or_default(),
        host_index: None,
        art_version: String::new(),
        control: AppControl::None,
        monitor_resume: false,
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
pub const PAIR_DECLINED: &str =
    en("The PC ended the pairing, for example because the PIN was not entered in time. Start pairing again and enter the new PIN on the PC.");
pub const PAIR_UNVERIFIED: &str = en("The PC failed its identity check, so nothing was paired. Another device on this network may be posing as your PC: pair only on a network you trust.");
pub const PAIR_NETWORK: &str = en("Use the same network and start the PC’s streaming app. Check its firewall, then retry.");
pub const PAIR_LINK_MISMATCH: &str =
    en("This PC’s certificate does not match the pairing link, so Nightjar did not pair. Pair only with a link or QR code from your own PC.");
pub const LINK_INVALID: &str = en("That pairing link is not valid. Show a new QR code on the PC and copy its link again.");
pub const ADDRESS_INVALID: &str = en("That address is not valid");
pub const CLIPBOARD_NO_LINK: &str =
    en("Copy the link under the QR code on the PC’s Devices page, then paste it here.");
pub const INVITE_INVALID: &str = en("This is not a Nightjar invite link");

pub fn invite_error(error: &str) -> String {
    const KNOWN: [&str; 4] = [
        en("This is not a Nightjar invite link"),
        en("This invite has expired. Ask for a new one."),
        en("The PC did not accept this invite: it was already used, taken back or has expired. Ask for a new one."),
        en("Turn on Play over the internet in Nightjar's settings, then open the invite again."),
    ];
    KNOWN
        .iter()
        .find(|known| **known == error)
        .map_or_else(|| error.to_owned(), |known| tr(known).to_owned())
}

pub const INVITE_KNOWN_PC: &str = en("This invite is for a PC that is already on your list with another certificate, so Nightjar did not join it and changed nothing. If that PC was set up again, remove it from the list and open the invite again.");
pub const UNPAIR_FAILED: &str = en("Failed to unpair. The PC may still list this device.");
pub const UNPAIR_NOT_PAIRED: &str = en("Device was not paired.");
pub const QUIT_OTHER_DEVICE: &str = en("Another device started this game, so it can only be quit from that device or on the PC.");

pub fn quit_failure(error: &str, name: &str) -> String {
    if error == QUIT_OTHER_DEVICE {
        return tr(QUIT_OTHER_DEVICE).to_owned();
    }
    eprintln!("nightjar: quitting {name} failed: {error}");
    trf("Couldn’t quit {}. Check that the PC is on and on this network, then try again.", &[&name])
}

pub const SLEEP_NOT_ALLOWED: &str = en("The PC does not let this device put it to sleep. Allow it in the PC’s device settings.");
pub const SLEEP_UNSUPPORTED: &str = en("The PC cannot sleep.");
pub const SLEEP_BUSY: &str = en("The PC cannot sleep right now. Close games and streams on it, then try again.");
pub const PC_POWER_NOT_ALLOWED: &str = en("The PC does not let this device restart or shut it down. Allow it in the PC’s device settings.");
pub const PC_POWER_BUSY: &str = en("The PC cannot do this right now. Close games and streams on it, then try again.");
pub const PAIRING_NOT_ALLOWED: &str = en("Needs Approve new devices on the PC’s Devices page.");
pub const ANOTHER_PC_ANSWERS: &str = en("Another PC or program answers at this PC’s address. If this PC was set up again, remove it and add it again.");

/// What was typed or pasted in Add PC.
#[derive(Debug, PartialEq)]
pub enum Entry {
    Address(String, Option<u16>),
    Link(Box<PairingUri>),
    Invite(Box<InviteUri>),
}

pub fn parse_entry(text: &str) -> Result<Entry, &'static str> {
    let text = text.trim();
    if InviteUri::is_link(text) {
        return parse_invite(text).map(Entry::Invite);
    }
    if PairingUri::is_link(text) {
        return parse_link(text).map(Entry::Link);
    }
    config::parse_address(text)
        .map(|(address, port)| Entry::Address(address, port))
        .ok_or(ADDRESS_INVALID)
}

pub fn parse_link(text: &str) -> Result<Box<PairingUri>, &'static str> {
    let mut link = PairingUri::parse(text.trim()).ok_or(LINK_INVALID)?;
    if let Some(bare) = link.host.strip_prefix('[').and_then(|h| h.strip_suffix(']')) {
        link.host = bare.to_owned();
    }
    Ok(Box::new(link))
}

pub fn parse_invite(text: &str) -> Result<Box<InviteUri>, &'static str> {
    InviteUri::parse(text.trim()).map(Box::new).ok_or(INVITE_INVALID)
}

pub fn new_art_retry_at() -> Instant {
    Instant::now() + ART_RETRY_INTERVAL
}

#[cfg(test)]
pub mod tests {
    use nightjar_policy::http::NativeCapabilities;

    pub const NATIVE_EXTRAS: &[&str] = &["folders", "coop", "clipboard.image"];

    pub fn stock_info() -> nightjar_policy::http::ServerInfo {
        let xml = "<?xml version=\"1.0\" encoding=\"utf-8\"?><root status_code=\"200\"><hostname>PC</hostname><uniqueid>uid</uniqueid><HttpsPort>47984</HttpsPort><ExternalPort>47989</ExternalPort><PairStatus>1</PairStatus><currentgame>0</currentgame><state>SUNSHINE_SERVER_FREE</state></root>";
        nightjar_policy::http::ServerInfo::parse(xml, 47989).unwrap()
    }

    pub fn native_info() -> nightjar_policy::http::ServerInfo {
        let xml = "<?xml version=\"1.0\" encoding=\"utf-8\"?><root status_code=\"200\"><hostname>PC</hostname><uniqueid>uid</uniqueid><HttpsPort>47984</HttpsPort><ExternalPort>47989</ExternalPort><PairStatus>1</PairStatus><currentgame>0</currentgame><state>SUNSHINE_SERVER_FREE</state><RustHostVersion>3.0.0</RustHostVersion><NativeApiVersion>1</NativeApiVersion><Permission>100859904</Permission></root>";
        nightjar_policy::http::ServerInfo::parse(xml, 47989).unwrap()
    }

    pub fn info_lands(hosts: &mut super::Hosts, id: u64, mut info: nightjar_policy::http::ServerInfo) {
        if let Some(host) = hosts.get_mut(id) {
            super::merge_native(&mut info, host.native.capabilities.as_ref());
            host.info = Some(info);
            host.reach = super::Reach::Online;
            host.sync_classic();
        }
    }

    pub fn native_lands(hosts: &mut super::Hosts, id: u64, capabilities: NativeCapabilities) {
        if let Some(host) = hosts.get_mut(id) {
            host.native.capabilities = Some(capabilities);
            if let Some(info) = &mut host.info {
                super::merge_native(info, host.native.capabilities.as_ref());
            }
        }
    }

    pub fn native_caps(features: &[&str], shared_folders: bool) -> NativeCapabilities {
        let mut value: serde_json::Value =
            serde_json::from_str(include_str!("../../native-api/tests/fixtures/capabilities.json")).unwrap();
        let listed = value["features"].as_array_mut().unwrap();
        listed.extend(features.iter().map(|f| serde_json::Value::from(*f)));
        value["permissions"]["shared_folders"] = shared_folders.into();
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn empty_version_cover_art_is_replaced_when_versioned_cover_arrives() {
        let mut host = super::Host::new(1, super::HostRecord::default());
        assert!(host.art_wanted(7, ""));
        assert_eq!(host.begin_art(7, ""), Some(String::new()));
        assert!(!host.art_wanted(7, ""), "same empty version does not re-fetch");
        assert!(host.art_wanted(7, "v1"), "non-empty version must replace initial empty-version art");
        assert_eq!(host.begin_art(7, "v1"), Some("v1".to_owned()));
        assert!(!host.art_wanted(7, "v1"));
        assert!(!host.art_wanted(7, ""), "empty version never replaces a known versioned cover");
        assert!(host.art_wanted(7, "v2"), "newer version replaces earlier version");
        assert_eq!(host.begin_art(7, "v2"), Some("v2".to_owned()));
    }
}
