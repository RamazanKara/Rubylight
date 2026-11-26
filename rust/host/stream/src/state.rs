//! Shared host runtime state (`Host`), session aliases, failure tracking, and submodule
//! re-exports for pairing, credentials, lifecycle, and codec probing.

use anyhow::{Context, Result};
use butterpollo_config::config::Config;
use butterpollo_crypto::{crypto::Identity, pairing::Pairings};
use butterpollo_session::{
    session::Warnings,
    state::{App, Credentials, PairedState, ProfileFiles},
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap},
    path::PathBuf,
    sync::{Arc, Mutex, RwLock},
    time::{Duration, Instant},
};

mod auth;
mod codec_probe;
mod library_file;
mod lifecycle;
pub(crate) mod loss_memory;
mod packed_probe;
mod pairing;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;

#[cfg(test)]
pub(crate) use codec_probe::PackedProbe;
#[cfg(test)]
use codec_probe::{
    CODEC_RETRY_MIN, CodecRetry, focus_summary, focus_verdict, pyrowave_pass, pyrowave_retry_delay,
};
use library_file::{load_library, normalize_library};
pub use lifecycle::{AppStop, RetainedDisplay, take_retained};
#[cfg(test)]
use packed_probe::{PACKED_SESSION_TRIES, packed_mode_names};
pub use pairing::{
    ONE_TIME_PIN_LIFETIME, ONE_TIME_PIN_SPENT, OneTimePin, OneTimePins, OtpMisses, PendingPin,
    PinEntry, pairing_approved_notice, pairing_ended_notice, pairing_from_internet, pairing_notice,
    pairing_source,
};

use crate::web_sessions::{self, WebSession};

pub type Shared = Arc<Host>;

type FileStamp = (std::time::SystemTime, u64);

/// When a file was last written and its size; None while it is missing.
fn file_stamp(path: &std::path::Path) -> Option<FileStamp> {
    let metadata = std::fs::metadata(path).ok()?;
    Some((metadata.modified().ok()?, metadata.len()))
}

pub type Launch = butterpollo_session::session::Launch<
    crate::display_session::StreamPreparation,
    butterpollo_win_io::audio_route::Route,
>;
pub type Session = butterpollo_session::session::Session<
    crate::display_session::StreamPreparation,
    butterpollo_win_io::audio_route::Route,
>;
pub type Sessions = butterpollo_session::session::Sessions<
    crate::display_session::StreamPreparation,
    butterpollo_win_io::audio_route::Route,
>;

pub struct Host {
    #[cfg(any(test, feature = "test-support"))]
    pub reconnect_fixture: Option<Arc<crate::stream::reconnect_harness::Fixture>>,
    pub directory: PathBuf,
    pub config_path: PathBuf,
    pub paired_path: PathBuf,
    pub credentials_path: PathBuf,
    pub apps_path: PathBuf,
    pub aliases_path: PathBuf,
    pub aliases: Mutex<Value>,
    pub config: RwLock<Config>,
    /// What [`Host::config_snapshot`] last handed out.
    config_snapshot: Mutex<Option<Arc<Config>>>,
    pub config_version: std::sync::atomic::AtomicU64,
    pub config_save: Mutex<()>,
    pub port_override: Option<u16>,
    pub identity: Identity,
    /// The web console's TLS certificate, separate from the pairing identity.
    pub console: butterpollo_crypto::crypto::ConsoleCertificate,
    pub paired: RwLock<PairedState>,
    /// Serializes route-memory writes with resets; epochs fence old sessions.
    keyframe_loss_epochs: Mutex<HashMap<String, u64>>,
    /// Each client's last live audio choice per app, for its next session.
    pub audio_memory: butterpollo_media_policy::audio_dsp::Memory,
    pub credentials: RwLock<Option<Credentials>>,
    /// The credentials file as the sign-in was last read or written.
    credentials_stamp: Mutex<Option<FileStamp>>,
    pub app_document: RwLock<Value>,
    pub apps: RwLock<Vec<App>>,
    /// The app library file as the host last read or wrote it; see [`Host::reload_apps`].
    apps_stamp: Mutex<Option<FileStamp>>,
    pub sessions: Mutex<Sessions>,
    pub controller_sessions: crate::controller_sessions::Pool,
    pub hardware_metrics: Mutex<Option<std::sync::mpsc::SyncSender<crate::telemetry::Request>>>,
    pub pairings: Mutex<Pairings>,
    /// Pairing requests waiting for a PIN in the console. A request with an
    /// empty `certificate` is a WebRTC device (see `webrtc::clients`).
    pub pins: Mutex<BTreeMap<String, PendingPin>>,
    /// Pairings approved from another paired device.
    pub approvals: Mutex<crate::pairing_approvals::Approvals>,
    /// Epoch bumped whenever WebRTC paired clients change.
    pub webrtc_clients_epoch: Arc<tokio::sync::watch::Sender<u64>>,
    /// The media transport of each WebRTC session between its signalling task and its stream.
    pub webrtc_links: crate::webrtc_bridge::WebRtcLinks,
    /// Shared network/port-mapping state for browser guests.
    pub guest_net: Arc<crate::webrtc_bridge::GuestNetworkState>,
    /// Installed WebRTC web-service implementation (`butterpollo_webrtc::WebRtcRuntime`).
    webrtc_service: Mutex<Option<Arc<dyn crate::webrtc_bridge::WebRtcWebService>>>,
    /// Paired devices joining another device's game as guests (`multiclient`).
    pub device_guests: crate::multiclient::DeviceGuests,
    /// Invites to play over the internet (`invites`).
    pub invites: crate::invites::HostInvites,
    pub otp: Mutex<OneTimePins>,
    pub web_sessions: Mutex<HashMap<String, WebSession>>,
    pub(crate) rotated_web_sessions: Mutex<HashMap<String, RotatedWebSession>>,
    pub stop: std::sync::atomic::AtomicBool,
    pub restart: std::sync::atomic::AtomicBool,
    stop_reason: Mutex<Option<&'static str>>,
    /// Wakes the maintenance loop (`runtime::maintain`) from an idle wait:
    /// [`Host::wake_maintenance`].
    maintain_wake: Arc<tokio::sync::Notify>,
    pub codecs: std::sync::atomic::AtomicU32,
    /// Why the last encoder check found no codec, while it finds none: a launch
    /// it refuses says this, not only that no encoder is ready.
    pub encoder_probe_error: Mutex<Option<String>>,
    /// The macroblocks per second the H.264, HEVC and AV1 encoders take (AMF throughput caps), 0 until probed.
    pub encoder_mb_rates: [std::sync::atomic::AtomicU64; 3],
    pub packed_modes: std::sync::atomic::AtomicU8,
    /// The NVIDIA image enhancements the GPU and driver reported with the
    /// NVIDIA component (`butterpollo_media_policy::nvidia_enhance` bits); 0 without
    /// it and on other GPUs.
    pub nvidia_enhancements: std::sync::atomic::AtomicU8,
    /// Why the startup check found an enhancement unavailable, per
    /// enhancement, for the console; empty in other builds.
    pub nvidia_enhancement_errors: Mutex<Vec<(&'static str, String)>>,
    /// A save narrowing `packed_modes` and a probe publishing them, one at a
    /// time: a probe's result is narrowed to the settings saved by then.
    packed_publish: Mutex<()>,
    /// The packed 4:4:4 modes a save newly allowed, or a probe a stream
    /// stopped left, or one that needed a signed-in user's session left,
    /// which wait to be probed.
    packed_probe_pending: std::sync::atomic::AtomicU8,
    /// Probes in a row that found the signed-in user's session not ready.
    packed_session_waits: std::sync::atomic::AtomicU8,
    /// Packed 4:4:4 modes whose codec the startup probe did not find, warned
    /// about once each.
    packed_codec_missing: std::sync::atomic::AtomicU8,
    /// A thread waits to probe them again, or probes them.
    packed_reprobing: std::sync::atomic::AtomicBool,
    /// One optional child probe at a time: packed 4:4:4 at startup or after
    /// a save, and PyroWave probed again.
    optional_probe: Mutex<()>,
    /// The encoder focus check per codec and bit depth, indexed by
    /// [`butterpollo_media_policy::encoder_focus::support_index`].
    focus_support: Mutex<[butterpollo_media_policy::encoder_focus::Support; 5]>,
    pub probing_codecs: std::sync::atomic::AtomicBool,
    codec_probe_requested: std::sync::atomic::AtomicBool,
    /// True once the startup probe has published its standard codecs, or
    /// found none. Later probes never hold requests back.
    pub startup_codecs: tokio::sync::watch::Sender<bool>,
    /// True once the startup pass has also probed the packed 4:4:4 modes (or left them to a
    /// later probe), so `packed_modes` is what the host knows. Only `/serverinfo`, which
    /// offers the mask to clients, waits for it; a launch needs the standard codecs alone.
    pub(crate) startup_packed: tokio::sync::watch::Sender<bool>,
    pub warnings: Warnings,
    pub current_app: Mutex<Option<crate::process::RunningApp>>,
    pub launch_transition: Mutex<()>,
    /// The launch holding `launch_transition`: its device and what stops its preparation
    /// commands (`Host::stop_launch`). Taken only to read or signal, never across a wait.
    pub(crate) launch_cancel: Mutex<
        Option<(
            String,
            Arc<butterpollo_win_io::process::CommandCancellation>,
        )>,
    >,
    /// Sleep PC tokens and the suspend backend.
    pub power: crate::power_action::PowerActions,
    /// Each paired client's `GET /bandwidth-probe` cooldown and budget.
    pub bandwidth_probes: Mutex<butterpollo_media_policy::bandwidth_probe::Limiter>,
    /// `/pyrowave-bandwidth-probe`'s cooldown and budget per client.
    pub pyrowave_probes: Mutex<butterpollo_media_policy::bandwidth_probe::Limiter>,
    /// File transfer uploads and downloads, all paced together.
    pub file_transfers: crate::file_transfers::Transfers,
    /// The USB devices clients forward to this host.
    pub usb: crate::usb_forward::UsbForwarding,
    /// The session state machine and its journal.
    pub session: Mutex<crate::session_control::Control>,
    /// The game Quick Suspend paused, if any (`quick_suspend`).
    pub quick_suspend: crate::quick_suspend::Tracker,
    /// Ended streams for the console's Sessions page, saved in the profile directory.
    pub session_history: crate::session_history::History,
    pub confirmations: Mutex<butterpollo_session::remote::Confirmations>,
    pub app_audio: Mutex<Option<Arc<butterpollo_win_io::audio_route::Route>>>,
    /// Each client's game display, kept between its streams while the app runs.
    pub app_display: Mutex<BTreeMap<String, RetainedDisplay>>,
    pub monitors: Mutex<BTreeMap<String, Arc<butterpollo_win_display::display::Retained>>>,
    /// The remote monitors still being made, outside the `monitors` lock.
    pub monitors_creating: crate::remote_display::Creating,
    pub updates: Mutex<Value>,
    pub metadata: Mutex<Option<(Instant, Value)>>,
    pub serverinfo_cache: Mutex<crate::nvhttp::ServerInfoCache>,
    /// The `User-Agent` of each paired device's latest `/serverinfo`, by
    /// device UUID, for the console's client warnings. Not saved.
    pub client_agents: Mutex<BTreeMap<String, String>>,
    /// Why each paired device's latest stream failed, by device UUID, for its
    /// `/serverinfo` (`RustHostLastFailure`). Its next launch clears it. Not saved.
    pub last_failures: Mutex<BTreeMap<String, LastFailure>>,
    /// The encoder family the capability probe selected for H.264.
    pub probed_encoder: Mutex<&'static str>,
    /// Why each codec the last probe pass did not advertise is missing.
    pub codec_missing: Mutex<Vec<butterpollo_media_policy::codec_support::Unavailable>>,
    /// Which session changed Radeon settings, and when they go back.
    pub radeon: crate::radeon_session::Radeon,
    /// Which session changed keys in games' settings files, and when they go back.
    pub game_files: crate::game_files::GameFiles,
    /// The Radeon profile applied for the owner's game and a launcher prompt, for the session.
    pub notes: crate::session_notes::SessionNotes,
    pub moves: crate::continue_here::Moves,
    /// Wake-on-LAN readiness, the console's fix and sleep after idle.
    pub wake: crate::wake::Wake,
    /// The `webhooks` setting's deliveries and each hook's last status.
    pub webhooks: crate::webhooks::Webhooks,
    /// Nucleus Co-op: its install, the co-op entries and the session it runs.
    pub nucleus: crate::nucleus::Registry,
    /// Neural Rendering: the GPU check, and each app's install or removal.
    pub neural: crate::neural::Neural,
    /// The NVIDIA component's download, removal and last failure.
    pub nvidia_component: crate::nvidia_component::NvidiaComponent,
    pub assets: PathBuf,
}

/// A device's latest stream failure, and since when it counts.
pub struct LastFailure {
    since: Instant,
    reason: Option<String>,
}

/// Recently rotated web refresh token kept briefly so concurrent tab refreshes
/// receive the newly issued tokens instead of signing the user out.
pub(crate) struct RotatedWebSession {
    pub rotated_at: Instant,
    pub access: String,
    pub refresh: String,
    pub csrf: String,
    pub refresh_deadline: u64,
    pub access_hash: String,
}

impl Host {
    /// The configuration as it is now, shared: the same `Arc` while it is unchanged, so
    /// a `/serverinfo` request, which every client polls and which keeps it across
    /// awaits, compares the settings rather than copying them.
    pub fn config_snapshot(&self) -> Arc<Config> {
        let config = self.config.read().unwrap();
        let mut snapshot = self.config_snapshot.lock().unwrap();
        if let Some(kept) = snapshot
            .as_ref()
            .filter(|kept| kept.values == config.values)
        {
            return kept.clone();
        }
        let fresh = Arc::new(config.clone());
        *snapshot = Some(fresh.clone());
        fresh
    }

    /// Ends the maintenance loop's idle wait, for work that cannot wait for its next
    /// timer: the host stops, an update is queued, an app starts. A launch or a stream
    /// that starts wakes it by itself ([`Sessions::with_waker`]).
    pub fn wake_maintenance(&self) {
        self.maintain_wake.notify_one();
    }

    /// What [`Host::wake_maintenance`] notifies.
    pub(crate) fn maintain_wake(&self) -> &Arc<tokio::sync::Notify> {
        &self.maintain_wake
    }

    /// Notifies WebRTC signalling connections that paired clients changed.
    pub fn notify_webrtc_clients_changed(&self) {
        self.webrtc_clients_epoch
            .send_modify(|n| *n = n.wrapping_add(1));
    }

    pub fn subscribe_webrtc_clients(&self) -> tokio::sync::watch::Receiver<u64> {
        self.webrtc_clients_epoch.subscribe()
    }

    pub fn set_webrtc_service(&self, service: Arc<dyn crate::webrtc_bridge::WebRtcWebService>) {
        *self
            .webrtc_service
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(service);
    }

    pub fn webrtc_service(&self) -> Option<Arc<dyn crate::webrtc_bridge::WebRtcWebService>> {
        self.webrtc_service
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub fn get_or_init_webrtc_service(
        &self,
        init: impl FnOnce() -> Arc<dyn crate::webrtc_bridge::WebRtcWebService>,
    ) -> Arc<dyn crate::webrtc_bridge::WebRtcWebService> {
        let mut guard = self
            .webrtc_service
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.get_or_insert_with(init).clone()
    }

    /// Records the first reason the host was asked to stop so the exit log names it.
    pub fn note_stop_reason(&self, reason: &'static str) {
        let mut guard = self
            .stop_reason
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.get_or_insert(reason);
    }

    /// The first stop reason recorded, if any.
    pub fn stop_reason(&self) -> Option<&'static str> {
        *self
            .stop_reason
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Asks the host to stop (and optionally restart) for `reason` and wakes the
    /// maintenance loop.
    pub fn request_stop(&self, reason: &'static str, restart: bool) {
        self.note_stop_reason(reason);
        if restart {
            self.restart
                .store(true, std::sync::atomic::Ordering::Release);
        }
        self.stop.store(true, std::sync::atomic::Ordering::Release);
        self.wake_maintenance();
    }

    /// Keeps a failed session's reason for its device's `/serverinfo`, unless
    /// the device launched again after that session started: an old stream's
    /// late teardown does not describe the new one.
    pub fn record_failure(&self, s: &Session) {
        let Some(reason) = s.failure() else { return };
        let mut failures = self
            .last_failures
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if failures
            .get(&s.launch.client.uuid)
            .is_some_and(|last| last.since > s.started)
        {
            return;
        }
        failures.insert(
            s.launch.client.uuid.clone(),
            LastFailure {
                since: s.started,
                reason: Some(reason),
            },
        );
    }

    /// Keeps why a launch of device `uuid` was refused, for its `/serverinfo`.
    pub fn record_launch_failure(&self, uuid: &str, reason: &str) {
        self.last_failures.lock().unwrap().insert(
            uuid.to_owned(),
            LastFailure {
                since: Instant::now(),
                reason: Some(butterpollo_session::session::failure_text(reason)),
            },
        );
    }

    /// A new launch of device `uuid`: its last failure no longer describes it.
    pub fn clear_failure(&self, uuid: &str) {
        self.last_failures.lock().unwrap().insert(
            uuid.to_owned(),
            LastFailure {
                since: Instant::now(),
                reason: None,
            },
        );
    }

    /// Why device `uuid`'s latest stream failed, if it did.
    pub fn last_failure(&self, uuid: &str) -> Option<String> {
        self.last_failures
            .lock()
            .unwrap()
            .get(uuid)
            .and_then(|last| last.reason.clone())
    }

    /// The running app, or None while another thread holds it. A stream or
    /// input thread skips its once-a-second look instead of waiting: housekeeping
    /// and launches hold the app, and on a PC busy loading a game a thread
    /// holding it can wait for the CPU. A poisoned lock panics, as
    /// `lock().unwrap()` does.
    pub fn try_current_app(
        &self,
    ) -> Option<std::sync::MutexGuard<'_, Option<crate::process::RunningApp>>> {
        match self.current_app.try_lock() {
            Ok(current) => Some(current),
            Err(std::sync::TryLockError::WouldBlock) => None,
            Err(std::sync::TryLockError::Poisoned(error)) => {
                panic!("the running app's lock is poisoned: {error}")
            }
        }
    }
}

#[cfg(test)]
mod tests;
