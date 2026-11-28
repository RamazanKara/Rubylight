//! Per-PC state (`Host`), native-read payloads (`NativeRead`), address selection and UI snapshot conversion.

use std::collections::{BTreeMap, HashMap};
use std::net::IpAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use nightjar_core::http::native_api::library::Library;
use nightjar_core::http::native_api::session::Session;
use nightjar_core::http::{
    Feature, HostUpdate, NativeCapabilities, NativeLibrary, PairingRequest, ServerInfo,
};

use crate::config::HostRecord;
use super::{
    internet_reply, replaces, Apps, Art, Native, Reach, PAIR_IN_GAME, PAIR_OFFLINE, POLL_INTERVAL,
    RECHECK_INTERVAL, RECHECK_LIBRARY_INTERVAL,
};
#[cfg(test)]
use super::SLEEP_NOT_ALLOWED;

/// One successful native read.
pub struct NativeRead {
    pub(super) capabilities: Option<NativeCapabilities>,
    pub(super) library: Option<NativeLibrary>,
    pub(super) session: Result<Session, String>,
    /// The PC's wake targets (`wol::targets`) to keep in place of the stored ones: empty for a
    /// PC without `wake`; None when they were not read, or the read failed (those adapters
    /// still belong to the PC).
    pub(super) wake: Option<(Vec<String>, Vec<String>)>,
    /// `GET /update` where the PC lists `update`; None when it does not, or the read failed.
    pub(super) update: Option<HostUpdate>,
    /// `GET /pairing/requests` where this device may approve; empty otherwise, or when the read failed.
    pub(super) pairing_requests: Vec<PairingRequest>,
}

pub struct Host {
    pub id: u64,
    pub record: HostRecord,
    pub reach: Reach,
    pub info: Option<ServerInfo>,
    pub apps: Apps,
    pub art: Arc<HashMap<i32, Art>>,
    /// Each cover's ambient, baked with it on the art thread: what the backdrop cross-fades to
    /// (`glass::CrossFade`) and baked glass samples. None while the cover loads, or without one.
    pub ambient: Arc<HashMap<i32, egui::TextureHandle>>,
    /// What each cover glows with on its page, taken from its ambient.
    pub ambient_glow: Arc<HashMap<i32, egui::Color32>>,
    /// The cover version each `art` entry was asked for (empty for classic box art).
    pub(super) art_versions: Arc<HashMap<i32, String>>,
    /// The pixels per point each cover that was made smaller was sized for; a cover kept at its
    /// own size is not in it.
    pub(super) art_sized: Arc<HashMap<i32, f32>>,
    /// When each cover whose download failed may be asked for again; its `art` is `Missing` till then.
    pub(super) art_retry: Arc<HashMap<i32, Instant>>,
    pub native: Native,
    pub(super) hub_engine: nightjar_policy::hub_sync::SyncEngine,
    pub(super) hub_cover_etags: BTreeMap<u32, String>,
    pub(super) hub_inflight: bool,
    pub(super) hub_resync_pending: bool,
    pub(super) hub_seq: u64,
    pub(super) hub_persist_epoch: Arc<AtomicU64>,
    /// The last app list of a PC without the native library, as saved for Home (live or from an
    /// earlier run), and the same as a library with the running app marked ([`Host::library`]).
    pub classic_apps: Option<Arc<crate::config::ClassicApps>>,
    pub classic: Option<Arc<Library>>,
    /// Bumped when the PC is unpaired, so a native read that started before is dropped.
    pub(super) native_epoch: u64,
    /// Counts native reads and actions, in the order they started or landed.
    pub(super) native_seq: u64,
    /// `native_seq` when the last session action landed: older reads carry an older session.
    pub(super) action_seq: u64,
    /// The newest read whose session was taken in.
    pub(super) session_seq: u64,
    pub error: Option<String>,
    /// A check the user asked for (Try again, Check again) runs: the card says Checking… until
    /// the PC answers or the check fails.
    pub checking: bool,
    pub(super) polling: bool,
    pub(super) last_poll: Option<Instant>,
    /// Polls started, and the newest one whose answer was taken in: an older answer that lands
    /// later (two polls overlap after a quit) must not bring back a state the PC has left.
    pub(super) poll_seq: u64,
    pub(super) info_seq: u64,
    /// The background poll out, if one is: a second one never queues behind it.
    pub(super) background_poll: Option<u64>,
    /// The asked poll out, if one is; `asked_again` when another was asked for meanwhile, which
    /// starts once it answers instead of queueing behind it.
    pub(super) asked_poll: Option<u64>,
    pub(super) asked_again: bool,
    /// Until then the PC is polled every second (see `RECHECK_WINDOW`).
    pub(super) recheck_until: Option<Instant>,
    /// A plain HTTP answer for this paired PC was taken in for its address only: ask it again over
    /// pinned HTTPS (`Hosts::drain` starts that poll).
    pub(super) pinned_check: bool,
    /// The remote address, when the last poll reached the PC only there (this device is away
    /// from the PC's network). Never saved: the address the PC is stored under stays its own.
    pub(super) via_remote: Option<String>,
    /// Whether a paired Sunshine-family PC has Foundation Sunshine's bandwidth test, asked once
    /// (`/api/network/capabilities`); None until it answered.
    pub(super) foundation_probe: Option<bool>,
    pub(super) probing_foundation: bool,
    /// When the PC's internet play reply was last asked for on its network; None until then.
    pub(super) internet_read: Option<Instant>,
}

impl Host {
    pub(super) fn reset_hub_state(&mut self) {
        self.hub_engine = nightjar_policy::hub_sync::SyncEngine::default();
        self.hub_cover_etags.clear();
        self.hub_inflight = false;
        self.hub_resync_pending = false;
        self.hub_seq = self.hub_seq.wrapping_add(1);
        self.hub_persist_epoch.fetch_add(1, Ordering::AcqRel);
        self.native.hub = Default::default();
    }

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
    pub(super) fn poll_interval(&self, now: Instant) -> Duration {
        if self.recheck_until.is_some_and(|until| now < until) {
            RECHECK_INTERVAL
        } else {
            POLL_INTERVAL
        }
    }

    /// In the recheck window and the library was not read for `RECHECK_LIBRARY_INTERVAL`.
    pub(super) fn library_recheck_due(&self, now: Instant) -> bool {
        self.recheck_until.is_some_and(|until| now < until)
            && self.native.full_at.is_none_or(|t| now.saturating_duration_since(t) >= RECHECK_LIBRARY_INTERVAL)
    }

    pub fn paired(&self) -> bool {
        self.record.server_certificate.is_some()
            && self.info.as_ref().is_none_or(|i| i.paired)
    }

    /// Whether the PC asks the player itself before a start replaces the running app.
    pub fn confirms_replacement(&self) -> bool {
        self.info.as_ref().is_some_and(|i| i.capabilities.has(Feature::HostConfirmsReplacement))
    }

    /// The PC speaks the native API now, or did when its library was saved.
    pub fn native_api(&self) -> bool {
        self.info.as_ref().is_some_and(|i| i.capabilities.has(Feature::NativeApi))
            || (self.info.is_none() && self.native.library.is_some())
    }

    /// The games Home shows for the PC: its native library, else its app list as a library
    /// ([`crate::console::classic_library`]).
    pub fn library(&self) -> Option<&Library> {
        self.native.library.as_deref().or(self.classic.as_deref())
    }

    /// Takes in the PC's app list as Home keeps it: None drops it. True when it changed and is to
    /// be saved (or forgotten).
    pub(super) fn set_classic_apps(&mut self, apps: Option<crate::config::ClassicApps>) -> bool {
        let changed = self.classic_apps.as_deref() != apps.as_ref();
        self.classic_apps = apps.map(Arc::new);
        self.sync_classic();
        changed
    }

    /// Makes the app list's library again, with the app the PC reports running now (none while
    /// it is away).
    pub(super) fn sync_classic(&mut self) {
        let running = self
            .info
            .as_ref()
            .filter(|_| self.reach == Reach::Online)
            .map(|i| (i.current_game, i.current_game_uuid.as_deref()));
        self.classic = self.classic_apps.as_deref().map(|apps| Arc::new(crate::console::classic_library(apps, running)));
    }

    pub fn session_reads(&self) -> crate::open_play::SessionReads {
        crate::open_play::SessionReads { asked: self.native_seq, answered: self.session_seq }
    }

    /// Whether the cover of `app` at `version` has not been asked for yet.
    #[cfg(test)]
    pub fn art_wanted(&self, app: i32, version: &str) -> bool {
        !self.has_art(app) || self.art_versions.get(&app).is_none_or(|known| replaces(known, version))
    }

    /// Whether the cover of `app` is here or on its way. One given up for room is neither, nor is
    /// one whose download failed once its wait is over.
    pub(super) fn has_art(&self, app: i32) -> bool {
        match self.art.get(&app) {
            Some(Art::Missing) => self.art_retry.get(&app).is_none_or(|&at| Instant::now() < at),
            art => matches!(art, Some(Art::Loading | Art::Ready(_))),
        }
    }

    /// Whether the cover of `app` was given up for room and loads again from its file.
    #[cfg(test)]
    pub fn art_dropped(&self, app: i32) -> bool {
        matches!(self.art.get(&app), Some(Art::Dropped))
    }

    /// Takes a cover asked for at `version` as loading and gives the version it loads at, or
    /// nothing when the cover there is, or the one on its way, is the one asked for.
    pub(crate) fn begin_art(&mut self, app: i32, version: &str) -> Option<String> {
        let known = self.art_versions.get(&app);
        if known.is_some_and(|known| replaces(known, version)) {
            Arc::make_mut(&mut self.art).remove(&app);
            Arc::make_mut(&mut self.ambient).remove(&app);
            Arc::make_mut(&mut self.ambient_glow).remove(&app);
            Arc::make_mut(&mut self.art_sized).remove(&app);
        }
        if self.has_art(app) {
            return None;
        }
        Arc::make_mut(&mut self.art_retry).remove(&app);
        // Asked for without a version, a cover that was dropped (a new renderer, or for room)
        // loads as the one it was, from its file.
        let version = match known {
            Some(known) if version.is_empty() => known.clone(),
            _ => version.to_owned(),
        };
        Arc::make_mut(&mut self.art).insert(app, Art::Loading);
        Arc::make_mut(&mut self.art_versions).insert(app, version.clone());
        Some(version)
    }

    /// The version the cover of `app` is asked for with, the same on every page: the native
    /// library's where it lists the app, else the app list's `ArtVersion`, else none. Pages that
    /// asked with versions of their own had each other's covers fetched again.
    #[cfg(test)]
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
    #[cfg(test)]
    pub fn sleep_option(&self) -> Option<Result<(), &'static str>> {
        match nightjar_policy::hosts::sleep_option(
            self.reach == Reach::Online,
            self.paired(),
            self.info.as_ref().map(|i| &i.capabilities),
        ) {
            nightjar_policy::hosts::SleepOfferState::Offered => Some(Ok(())),
            nightjar_policy::hosts::SleepOfferState::PermissionDenied => Some(Err(SLEEP_NOT_ALLOWED)),
            nightjar_policy::hosts::SleepOfferState::Hidden => None,
        }
    }
}

fn to_policy_path(path: nightjar_core::internet::InternetPath) -> nightjar_policy::internet_path::InternetPath {
    match path {
        nightjar_core::internet::InternetPath::Lan => nightjar_policy::internet_path::InternetPath::Lan,
        nightjar_core::internet::InternetPath::Ipv6 => nightjar_policy::internet_path::InternetPath::Ipv6,
        nightjar_core::internet::InternetPath::MappedIpv4 => nightjar_policy::internet_path::InternetPath::MappedIpv4,
        nightjar_core::internet::InternetPath::Punched => nightjar_policy::internet_path::InternetPath::Punched,
    }
}

impl Host {
    /// UI-crate snapshot of this host.
    pub fn ui_view(&self) -> nightjar_desktop_ui::hosts_model::Host {
        nightjar_desktop_ui::hosts_model::Host {
            id: self.id,
            record: self.record.clone(),
            reach: self.reach,
            info: self.info.clone(),
            apps: self.apps.clone(),
            art: Arc::clone(&self.art),
            ambient: Arc::clone(&self.ambient),
            ambient_glow: Arc::clone(&self.ambient_glow),
            art_versions: Arc::clone(&self.art_versions),
            art_sized: Arc::clone(&self.art_sized),
            art_retry: Arc::clone(&self.art_retry),
            native: self.native.clone(),
            classic_apps: self.classic_apps.clone(),
            classic: self.classic.clone(),
            error: self.error.clone(),
            checking: self.checking,
            via_remote: self.via_remote.clone(),
            path_override: self.path().map(to_policy_path),
        }
    }
}
