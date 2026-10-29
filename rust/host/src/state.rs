use anyhow::{Context, Result};
use butterpollo_core::{
    config::Config,
    crypto::Identity,
    pairing::Pairings,
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
pub type Shared = Arc<Host>;
type FileStamp = (std::time::SystemTime, u64);
/// When a file was last written and its size; None while it is missing.
fn file_stamp(path: &std::path::Path) -> Option<FileStamp> {
    let metadata = std::fs::metadata(path).ok()?;
    Some((metadata.modified().ok()?, metadata.len()))
}
pub(crate) mod loss_memory;
#[cfg(test)]
pub(crate) mod test_support;
pub type Launch = butterpollo_core::session::Launch<
    crate::display_session::StreamPreparation,
    butterpollo_windows::audio_route::Route,
>;
pub type Session = butterpollo_core::session::Session<
    crate::display_session::StreamPreparation,
    butterpollo_windows::audio_route::Route,
>;
pub type Sessions = butterpollo_core::session::Sessions<
    crate::display_session::StreamPreparation,
    butterpollo_windows::audio_route::Route,
>;
use crate::web_sessions::{self, WebSession};

const STANDARD_CODECS: u32 = 0x1 | 0x100 | 0x200 | 0x10000 | 0x20000;
/// Bits from the separate PyroWave (Vulkan) probe.
const PYROWAVE_CODECS: u32 = 0x0780_0000;
const CODEC_RETRY_MIN: Duration = Duration::from_secs(5);
const CODEC_RETRY_MAX: Duration = Duration::from_secs(60);

struct CodecRetry {
    delay: Duration,
    next: Option<Instant>,
    on_demand: Instant,
    warning: Option<Instant>,
}
impl CodecRetry {
    fn new(now: Instant) -> Self {
        Self {
            delay: CODEC_RETRY_MIN,
            next: None,
            on_demand: now,
            warning: None,
        }
    }
    fn completed(
        &mut self,
        flags: u32,
        error: Option<&str>,
        warnings: &Warnings,
        now: Instant,
    ) -> bool {
        if flags & !0x40000000 == 0 {
            if self.warn(now) {
                // Clearing makes an unchanged warning log its next reminder.
                warnings.clear("video_encoder");
                warnings.set(
                    "video_encoder",
                    format!(
                        "No video encoder available: {}. Retrying.",
                        error.unwrap_or("the encoder produced no packets")
                    ),
                );
            }
        } else {
            warnings.clear("video_encoder");
            self.warning = None;
        }
        if flags & STANDARD_CODECS != 0 {
            self.next = None;
            self.delay = CODEC_RETRY_MIN;
            return false;
        }
        self.next = Some(now + self.delay);
        self.on_demand = now + CODEC_RETRY_MIN;
        self.delay = (self.delay * 2).min(CODEC_RETRY_MAX);
        true
    }
    fn due(&self, now: Instant, requested: bool) -> bool {
        self.next
            .is_some_and(|next| now >= next || (requested && now >= self.on_demand))
    }
    fn warn(&mut self, now: Instant) -> bool {
        if self.warning.is_some_and(|next| now < next) {
            return false;
        }
        self.warning = Some(now + CODEC_RETRY_MAX);
        true
    }
}
/// A one-time PIN that lets a client pair with a passphrase, valid
/// [`ONE_TIME_PIN_LIFETIME`] and for [`OtpMisses`] wrong tries.
pub struct OneTimePin {
    pub pin: String,
    pub passphrase: String,
    pub device_name: String,
    pub created: Instant,
    pub misses: OtpMisses,
    /// The access the owner chose ([`butterpollo_core::state::pairing_access`]).
    pub permission: Option<u32>,
}
/// Wrong one-time PIN answers by device address. Pairing needs no client
/// certificate, so one global count let any LAN device use up the invitation
/// meant for another.
#[derive(Default)]
pub struct OtpMisses {
    by_peer: HashMap<std::net::IpAddr, u8>,
    total: u8,
}
impl OtpMisses {
    /// Wrong answers one address may give.
    pub const PER_PEER: u8 = 5;
    /// Wrong answers from all addresses before the PIN is spent.
    pub const TOTAL: u8 = 32;
    pub fn refuses(&self, peer: std::net::IpAddr) -> bool {
        self.by_peer
            .get(&peer)
            .is_some_and(|misses| *misses >= Self::PER_PEER)
    }
    /// Counts a wrong answer from `peer`; false once the PIN is spent.
    pub fn miss(&mut self, peer: std::net::IpAddr) -> bool {
        *self.by_peer.entry(peer).or_default() += 1;
        self.total += 1;
        self.total < Self::TOTAL
    }
    /// Wrong answers from `peer` so far.
    #[cfg(test)]
    pub fn count(&self, peer: std::net::IpAddr) -> u8 {
        self.by_peer.get(&peer).copied().unwrap_or(0)
    }
}
pub const ONE_TIME_PIN_LIFETIME: Duration = Duration::from_secs(180);
/// Why a one-time PIN pairing found no live PIN.
pub const ONE_TIME_PIN_SPENT: &str =
    "no one-time pairing code is live: it was used or has expired; show a new one on the PC";
impl OneTimePin {
    pub fn live(&self) -> bool {
        self.created.elapsed() < ONE_TIME_PIN_LIFETIME
    }
}
/// The two one-time PINs: the console's QR code and the PIN typed in Artemis.
/// Making one never replaces the other, and pairing consumes only the one
/// that matched.
#[derive(Default)]
pub struct OneTimePins {
    pub qr: Option<OneTimePin>,
    pub typed: Option<OneTimePin>,
}
impl OneTimePins {
    /// The live slot whose PIN and passphrase hash with `salt` to `auth`
    /// (Artemis' `otpauth`), taken out of its slot. Slots that refuse `peer`
    /// ([`OtpMisses::refuses`]) are not tried. A miss counts a wrong answer
    /// from `peer` against every slot tried and closes a slot once it is
    /// spent. Fails when no slot is live or every live slot refuses `peer`.
    pub fn take_matching(
        &mut self,
        auth: &str,
        salt: &str,
        peer: std::net::IpAddr,
    ) -> Result<Option<OneTimePin>> {
        let auth = hex::decode(auth).ok();
        let (mut live, mut tried) = (false, false);
        for slot in [&mut self.qr, &mut self.typed] {
            if slot.as_ref().is_some_and(|otp| !otp.live()) {
                *slot = None;
            }
            let Some(otp) = slot.as_ref() else { continue };
            live = true;
            if otp.misses.refuses(peer) {
                continue;
            }
            tried = true;
            let expected = butterpollo_core::crypto::hash(
                format!("{}{salt}{}", otp.pin, otp.passphrase).as_bytes(),
            );
            if auth
                .as_deref()
                .is_some_and(|auth| butterpollo_core::crypto::equal(&expected, auth))
            {
                return Ok(slot.take());
            }
        }
        if !live {
            // Used, expired or never shown: `pair` answers 400, which clients
            // show as a refused code rather than an unreachable PC.
            anyhow::bail!("{ONE_TIME_PIN_SPENT}");
        }
        if !tried {
            anyhow::bail!("too many wrong one-time PIN answers from this address");
        }
        for slot in [&mut self.qr, &mut self.typed] {
            if let Some(otp) = slot.as_mut()
                && !otp.misses.refuses(peer)
                && !otp.misses.miss(peer)
            {
                *slot = None;
            }
        }
        Ok(None)
    }
}
/// What the console sends a waiting pairing request: the PIN, the device's
/// name and the access the owner chose (`None` keeps the default).
pub type PinEntry = (String, String, Option<u32>);
pub struct PendingPin {
    pub name: String,
    /// The requesting device's certificate: Moonlight's apps share one
    /// unique ID, so this tells another device apart.
    pub certificate: String,
    pub created: Instant,
    pub sender: tokio::sync::oneshot::Sender<PinEntry>,
}
/// A client's display lease and when it last stopped being streamed.
pub type RetainedDisplay = (Arc<crate::display_session::Ready>, Option<Instant>);
/// The display a stream launch resumes: only this client's own retained
/// display, and only in the mode it asks for. Reuse leaves the entry (and the
/// display) untouched; a retained display in another mode is removed and
/// handed back so the caller drops it, restoring Windows, outside the lock.
pub fn take_retained<L: Clone>(
    displays: &mut BTreeMap<String, (L, Option<Instant>)>,
    client: &str,
    matches: impl FnOnce(&L) -> bool,
) -> Result<L, Option<L>> {
    match displays.get(client) {
        Some((lease, _)) if matches(lease) => Ok(lease.clone()),
        Some(_) => Err(displays.remove(client).map(|(lease, _)| lease)),
        None => Err(None),
    }
}
pub struct Host {
    #[cfg(test)]
    pub reconnect_fixture: Option<Arc<crate::stream::reconnect_tests::Fixture>>,
    pub directory: PathBuf,
    pub config_path: PathBuf,
    pub paired_path: PathBuf,
    pub credentials_path: PathBuf,
    pub apps_path: PathBuf,
    pub aliases_path: PathBuf,
    pub aliases: Mutex<Value>,
    pub config: RwLock<Config>,
    pub config_version: std::sync::atomic::AtomicU64,
    pub config_save: Mutex<()>,
    pub port_override: Option<u16>,
    pub identity: Identity,
    /// The web console's TLS certificate, separate from the pairing identity.
    pub console: butterpollo_core::crypto::ConsoleCertificate,
    pub paired: RwLock<PairedState>,
    /// Serializes route-memory writes with resets; epochs fence old sessions.
    keyframe_loss_epochs: Mutex<HashMap<String, u64>>,
    /// Each client's last live audio choice per app, for its next session.
    pub audio_memory: butterpollo_core::audio_dsp::Memory,
    pub credentials: RwLock<Option<Credentials>>,
    /// The credentials file as the sign-in was last read or written.
    credentials_stamp: Mutex<Option<FileStamp>>,
    pub app_document: RwLock<Value>,
    pub apps: RwLock<Vec<App>>,
    pub sessions: Mutex<Sessions>,
    pub controller_sessions: crate::controller_sessions::Pool,
    pub hardware_metrics: Mutex<Option<std::sync::mpsc::SyncSender<crate::telemetry::Request>>>,
    pub pairings: Mutex<Pairings>,
    /// Pairing requests waiting for a PIN in the console. A request with an
    /// empty `certificate` is a WebRTC device (see `webrtc::clients`).
    pub pins: Mutex<BTreeMap<String, PendingPin>>,
    pub webrtc: crate::webrtc::clients::Registry,
    /// The media transport of each WebRTC session between its signalling task and its stream.
    pub webrtc_links: crate::webrtc::session::Links,
    pub otp: Mutex<OneTimePins>,
    pub web_sessions: Mutex<HashMap<String, WebSession>>,
    pub stop: std::sync::atomic::AtomicBool,
    pub restart: std::sync::atomic::AtomicBool,
    pub codecs: std::sync::atomic::AtomicU32,
    /// The macroblocks per second the H.264, HEVC and AV1 encoders take (AMF throughput caps), 0 until probed.
    pub encoder_mb_rates: [std::sync::atomic::AtomicU64; 3],
    pub packed_modes: std::sync::atomic::AtomicU8,
    /// A save narrowing `packed_modes` and a probe publishing them, one at a
    /// time: a probe's result is narrowed to the settings saved by then.
    packed_publish: Mutex<()>,
    /// The packed 4:4:4 modes a save newly allowed, or a probe a stream
    /// stopped left, which wait to be probed.
    packed_probe_pending: std::sync::atomic::AtomicU8,
    /// Packed 4:4:4 modes whose codec the startup probe did not find, warned
    /// about once each.
    packed_codec_missing: std::sync::atomic::AtomicU8,
    /// A thread waits to probe them again, or probes them.
    packed_reprobing: std::sync::atomic::AtomicBool,
    /// One optional child probe at a time: packed 4:4:4 at startup or after
    /// a save, and PyroWave probed again.
    optional_probe: Mutex<()>,
    /// The encoder focus check per codec and bit depth, indexed by
    /// [`butterpollo_core::encoder_focus::support_index`].
    focus_support: Mutex<[butterpollo_core::encoder_focus::Support; 5]>,
    pub probing_codecs: std::sync::atomic::AtomicBool,
    codec_probe_requested: std::sync::atomic::AtomicBool,
    /// True once the startup probe has published its standard codecs, or
    /// found none. Later probes never hold requests back.
    pub(crate) startup_codecs: tokio::sync::watch::Sender<bool>,
    pub warnings: Warnings,
    pub current_app: Mutex<Option<crate::process::RunningApp>>,
    pub launch_transition: Mutex<()>,
    /// Sleep PC tokens and the suspend backend.
    pub power: crate::power_action::PowerActions,
    /// Each paired client's `GET /bandwidth-probe` cooldown and budget.
    pub bandwidth_probes: Mutex<butterpollo_core::bandwidth_probe::Limiter>,
    /// File transfer uploads and downloads, all paced together.
    pub file_transfers: crate::native_api::files::Transfers,
    /// The USB devices clients forward to this host.
    pub usb: crate::usb_forward::UsbForwarding,
    /// The session state machine and its journal.
    pub session: Mutex<crate::session_control::Control>,
    pub confirmations: Mutex<butterpollo_core::remote::Confirmations>,
    pub app_audio: Mutex<Option<Arc<butterpollo_windows::audio_route::Route>>>,
    /// Each client's game display, kept between its streams while the app runs.
    pub app_display: Mutex<BTreeMap<String, RetainedDisplay>>,
    pub monitors: Mutex<BTreeMap<String, Arc<butterpollo_windows::display::Retained>>>,
    pub updates: Mutex<Value>,
    pub metadata: Mutex<Option<(Instant, Value)>>,
    pub serverinfo_cache: Mutex<crate::nvhttp::ServerInfoCache>,
    /// The `User-Agent` of each paired device's latest `/serverinfo`, by
    /// device UUID, for the console's client warnings. Not saved.
    pub client_agents: Mutex<BTreeMap<String, String>>,
    /// The encoder family the capability probe selected for H.264.
    pub probed_encoder: Mutex<&'static str>,
    /// Why each codec the last probe pass did not advertise is missing.
    pub codec_missing: Mutex<Vec<butterpollo_core::codec_support::Unavailable>>,
    /// Which session changed Radeon settings, and when they go back.
    pub radeon: crate::radeon_session::Radeon,
    /// The Radeon profile applied for the owner's game and a launcher prompt, for the session.
    pub notes: crate::session_notes::SessionNotes,
    /// Wake-on-LAN readiness, the console's fix and sleep after idle.
    pub wake: crate::wake::Wake,
    /// The `webhooks` setting's deliveries and each hook's last status.
    pub webhooks: crate::webhooks::Webhooks,
    /// Nucleus Co-op: its install, the co-op entries and the session it runs.
    pub nucleus: crate::nucleus::Registry,
    pub assets: PathBuf,
}
/// Why the running app is stopped, and the client that asked: each stop logs it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppStop<'a> {
    /// A client's `/cancel`, Moonlight's Quit.
    Cancel(&'a str),
    /// A client's launch with the Terminate control.
    Terminate(&'a str),
    /// WebRTC's stop-host-session or switch-session.
    WebRtc(&'a str),
    /// Close game from the session controls.
    CloseGame(&'a str),
    /// A client's launch of another app (confirmed, when the host asks).
    Replaced(&'a str),
    /// Close in the web UI.
    WebUi,
    /// Quit app in the tray.
    Tray,
    /// The app exited by itself.
    Exited,
    /// The deferred app failed to start.
    LaunchFailed,
    /// The host shuts down.
    Shutdown,
    #[cfg(test)]
    Test,
}
impl<'a> AppStop<'a> {
    pub fn reason(self) -> &'static str {
        match self {
            Self::Cancel(_) => "cancel",
            Self::Terminate(_) => "terminate",
            Self::WebRtc(_) => "webrtc",
            Self::CloseGame(_) => "close game",
            Self::Replaced(_) => "replaced",
            Self::WebUi => "web ui",
            Self::Tray => "tray",
            Self::Exited => "exited",
            Self::LaunchFailed => "launch failed",
            Self::Shutdown => "shutdown",
            #[cfg(test)]
            Self::Test => "test",
        }
    }
    /// The uuid of the client that asked, as the app's owner is.
    pub fn client(self) -> Option<&'a str> {
        match self {
            Self::Cancel(client)
            | Self::Terminate(client)
            | Self::WebRtc(client)
            | Self::CloseGame(client)
            | Self::Replaced(client) => Some(client),
            _ => None,
        }
    }
}
impl Host {
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
    pub fn stop_app(&self, why: AppStop<'_>) {
        let teardown = self.session_teardown(crate::session_control::Scope::Game);
        // Stopping the app waits for it to exit, up to its exit timeout and
        // undo commands: never under the lock every serverinfo request takes.
        let app = self.current_app.lock().unwrap().take();
        self.end_app(app, why);
        let audio = self.app_audio.lock().unwrap().take();
        let displays = std::mem::take(&mut *self.app_display.lock().unwrap());
        drop((audio, displays));
        drop(teardown);
    }
    /// Stops `app`, taken out of `current_app`: logs why, waits for it to
    /// exit and raises app.stopped. Never under the `current_app` lock.
    pub fn end_app(&self, app: Option<crate::process::RunningApp>, why: AppStop<'_>) {
        let mut stopped = None;
        if let Some(app) = &app {
            tracing::info!(
                reason = why.reason(),
                by = why.client(),
                app = %app.name,
                app_id = app.id,
                owner = %app.owner,
                "stopping the application"
            );
            // A placeholder that never started raised no app.started.
            stopped = app.started_event();
        }
        drop(app);
        if let Some((id, name, owner)) = stopped {
            self.webhook(
                "app.stopped",
                json!({
                    "app": {"id": id, "name": name},
                    "device": self.webhook_device(&owner),
                    "reason": why.reason(),
                    "by": self.webhook_device(why.client().unwrap_or("")),
                }),
            );
        }
    }
    pub fn reap_paused_display(&self) {
        let audio = {
            let mut audio = self.app_audio.lock().unwrap();
            if audio
                .as_ref()
                .is_some_and(|route| Arc::strong_count(route) == 1)
            {
                audio.take()
            } else {
                None
            }
        };
        drop(audio);
        if self.app_display.lock().unwrap().is_empty() {
            return;
        }
        let config = self.config.read().unwrap().clone();
        let expired: Vec<String> = {
            let mut displays = self.app_display.lock().unwrap();
            let mut expired = vec![];
            for (owner, (lease, paused)) in displays.iter_mut() {
                if Arc::strong_count(lease) > 1 {
                    *paused = None;
                } else if config
                    .display_disconnect_delay(lease.exclusive_layout())
                    .is_some_and(|delay| paused.get_or_insert_with(Instant::now).elapsed() >= delay)
                {
                    expired.push(owner.clone());
                }
            }
            expired
        };
        if expired.is_empty() {
            return;
        }
        // The grace period ran out: the session ends as End session does,
        // while the display is still held so the state shows the suspension.
        let teardown = self.session_teardown(crate::session_control::Scope::Displays(&expired));
        let released: Vec<_> = {
            let mut displays = self.app_display.lock().unwrap();
            let mut released = vec![];
            for owner in &expired {
                // A stream that took the display back in the meantime keeps it.
                if displays
                    .get(owner)
                    .is_some_and(|(lease, _)| Arc::strong_count(lease) == 1)
                {
                    released.extend(displays.remove(owner));
                }
            }
            released
        };
        // Display leases restore Windows settings; do that outside the lock.
        drop(released);
        drop(teardown);
    }
    pub fn load(directory: PathBuf, assets: PathBuf, port: Option<u16>) -> Result<Shared> {
        std::fs::create_dir_all(&directory)?;
        let config_path = directory.join("sunshine.conf");
        let (mut config, retired) = Config::load_retiring(&config_path)?;
        let warnings = Warnings::default();
        if let Some(warning) = butterpollo_core::config::retired_warning(&retired) {
            // Written back, so the warning shows once. A file that cannot be
            // written keeps the old values and warns again on the next start.
            if let Err(error) = butterpollo_core::state::atomic_write(
                &config_path,
                Config::load(&config_path)?.text().as_bytes(),
            ) {
                tracing::warn!(error = %format!("{error:#}"), "the replaced settings could not be saved");
            }
            warnings.set("settings_retired", warning);
        }
        if let Some(warning) = butterpollo_windows::limiter::take_stale_nvidia(&directory) {
            warnings.set("nvidia_limiter_retired", warning);
        }
        if let Some(port) = port {
            config.values.insert("port".into(), port.to_string());
        }
        config.ports()?;
        let ProfileFiles {
            paired: paired_path,
            credentials: credentials_path,
            apps: apps_path,
            aliases: aliases_path,
            certificate,
            key,
        } = ProfileFiles::new(&config, &directory);
        let mut aliases = butterpollo_core::state::load_json(&aliases_path, json!({"root":{}}))?;
        let mut paired = PairedState::load(&paired_path)?;
        // Vibepollo keeps the shared virtual display's GUID in
        // vibeshine_state.json; reuse it so Windows keeps that display's
        // settings.
        if paired.document["root"]["shared_virtual_display_guid"]
            .as_str()
            .is_none()
            && let Some(id) = aliases["root"]["shared_virtual_display_guid"]
                .as_str()
                .filter(|id| uuid::Uuid::parse_str(id).is_ok())
        {
            paired.document["root"]["shared_virtual_display_guid"] = id.into();
        }
        // Once per host: a saved amd_ltr_frames = 0 from before 2.0.0 is
        // that release's old default, not a choice; drop it from the file
        // too, so the console shows the current default.
        let upgraded = &mut paired.document["root"]["amd_ltr_frames_upgraded"];
        if upgraded.as_bool() != Some(true) {
            let mut stored = Config::load(&config_path)?;
            if stored.upgrade_ltr_default(false) {
                butterpollo_core::state::atomic_write(&config_path, stored.text().as_bytes())?;
                config.upgrade_ltr_default(false);
                tracing::info!(
                    "amd_ltr_frames = 0, the default before 2.0.0, was removed from sunshine.conf: AV1 now recovers lost frames without a keyframe. Set it to 0 again to turn that off"
                );
            }
            *upgraded = true.into();
        }
        paired.save(&paired_path)?;
        let identity = Identity::load(&certificate, &key)?;
        let console =
            butterpollo_core::crypto::ConsoleCertificate::load(&directory.join("credentials"))?;
        let credentials = Credentials::load(&credentials_path)?;
        let credentials_stamp = file_stamp(&credentials_path);
        let app_document = load_library(
            &apps_path,
            // Vibepollo's default library.
            json!({"env":{},"apps":[
                {"name":"Desktop","image-path":"desktop.png","allow-client-commands":false},
                {"name":"Steam Big Picture","prep-cmd":[{"do":"","undo":"steam://close/bigpicture","elevated":false}],
                 "detached":["steam://open/bigpicture"],"image-path":"steam.png"}
            ]}),
        )?;
        let mut app_document = app_document;
        let mut assigned = false;
        if let Some(list) = app_document.get_mut("apps").and_then(Value::as_array_mut) {
            for app in list.iter_mut().filter_map(Value::as_object_mut) {
                if app
                    .get("uuid")
                    .and_then(Value::as_str)
                    .is_none_or(|uuid| uuid.trim().is_empty())
                {
                    app.insert("uuid".into(), json!(uuid::Uuid::new_v4().to_string()));
                    assigned = true;
                }
                // A ViGEmBus controller type, say, becomes its VHF pad.
                assigned |= butterpollo_core::config::replace_retired_in_app(app);
            }
        }
        if assigned {
            butterpollo_core::state::write_json(&apps_path, &app_document)?;
        }
        let mut apps: Vec<App> =
            serde_json::from_value(app_document.get("apps").cloned().unwrap_or(json!([])))?;
        butterpollo_core::catalog::assign(
            &mut apps,
            &mut aliases,
            assets.parent().unwrap_or(&assets),
        )?;
        butterpollo_core::state::write_json(&aliases_path, &aliases)?;
        let web_sessions = web_sessions::load(
            &aliases,
            credentials
                .as_ref()
                .map(|c| c.username.as_str())
                .unwrap_or(""),
        );
        let session = crate::session_control::Control::new(&directory);
        Ok(Arc::new(Self {
            #[cfg(test)]
            reconnect_fixture: None,
            directory,
            config_path,
            paired_path,
            credentials_path,
            apps_path,
            aliases_path,
            aliases: Mutex::new(aliases),
            config: RwLock::new(config),
            config_version: std::sync::atomic::AtomicU64::new(0),
            config_save: Mutex::new(()),
            port_override: port,
            identity,
            console,
            paired: RwLock::new(paired),
            keyframe_loss_epochs: Default::default(),
            audio_memory: Default::default(),
            credentials: RwLock::new(credentials),
            credentials_stamp: Mutex::new(credentials_stamp),
            app_document: RwLock::new(app_document),
            apps: RwLock::new(apps),
            sessions: Mutex::new(Sessions::default()),
            controller_sessions: Default::default(),
            hardware_metrics: Mutex::new(None),
            pairings: Mutex::new(Pairings::default()),
            pins: Mutex::new(BTreeMap::new()),
            webrtc: crate::webrtc::clients::Registry::new(),
            webrtc_links: Default::default(),
            otp: Mutex::new(OneTimePins::default()),
            web_sessions: Mutex::new(web_sessions),
            stop: std::sync::atomic::AtomicBool::new(false),
            restart: std::sync::atomic::AtomicBool::new(false),
            codecs: std::sync::atomic::AtomicU32::new(0),
            encoder_mb_rates: Default::default(),
            packed_modes: std::sync::atomic::AtomicU8::new(0),
            packed_publish: Mutex::new(()),
            packed_probe_pending: std::sync::atomic::AtomicU8::new(0),
            packed_codec_missing: std::sync::atomic::AtomicU8::new(0),
            packed_reprobing: std::sync::atomic::AtomicBool::new(false),
            optional_probe: Mutex::new(()),
            focus_support: Mutex::new(std::array::from_fn(|_| {
                butterpollo_core::encoder_focus::Support::NotProbed
            })),
            probing_codecs: std::sync::atomic::AtomicBool::new(true),
            codec_probe_requested: std::sync::atomic::AtomicBool::new(false),
            startup_codecs: tokio::sync::watch::channel(false).0,
            warnings,
            current_app: Mutex::new(None),
            launch_transition: Mutex::new(()),
            power: Default::default(),
            bandwidth_probes: Default::default(),
            file_transfers: Default::default(),
            usb: Default::default(),
            session: Mutex::new(session),
            confirmations: Mutex::new(Default::default()),
            app_audio: Mutex::new(None),
            app_display: Mutex::new(BTreeMap::new()),
            monitors: Mutex::new(BTreeMap::new()),
            updates: Mutex::new(
                json!({"status":true,"checking":false,"check_failed":false,"checked_at":0,"releases":[]}),
            ),
            assets,
            metadata: Mutex::new(None),
            serverinfo_cache: Mutex::new(Default::default()),
            client_agents: Mutex::new(BTreeMap::new()),
            probed_encoder: Mutex::new(""),
            codec_missing: Mutex::new(Vec::new()),
            radeon: crate::radeon_session::Radeon::default(),
            notes: crate::session_notes::SessionNotes::default(),
            wake: Default::default(),
            webhooks: crate::webhooks::Webhooks::new(crate::webhooks::Policy::DEFAULT)?,
            nucleus: crate::nucleus::Registry::on_pc(),
        }))
    }
    pub fn assign_apps(&self, apps: &mut [App]) -> Result<()> {
        let mut aliases = self.aliases.lock().unwrap();
        let mut next = aliases.clone();
        butterpollo_core::catalog::assign(
            apps,
            &mut next,
            self.assets.parent().unwrap_or(&self.assets),
        )?;
        butterpollo_core::state::write_json(&self.aliases_path, &next)?;
        *aliases = next;
        Ok(())
    }
    pub fn request_codec_probe(&self) {
        if self.codecs.load(std::sync::atomic::Ordering::Acquire) & STANDARD_CODECS == 0 {
            self.codec_probe_requested
                .store(true, std::sync::atomic::Ordering::Release);
        }
    }
    /// Wait up to `limit` for the startup probe's standard codecs. A client
    /// that wakes or starts the host and connects at once otherwise caches no
    /// HEVC, AV1 or HDR, or has its first launch refused. Once that probe has
    /// finished, even without finding an encoder, nothing waits again: a
    /// probe retried while no user is signed in or a driver fails holds back
    /// no request.
    pub async fn wait_for_startup_codecs(&self, limit: Duration) {
        let mut ready = self.startup_codecs.subscribe();
        let _ = tokio::time::timeout(limit, ready.wait_for(|ready| *ready)).await;
    }
    /// `wait_for_startup_codecs` for a launch, which runs on a blocking thread.
    pub fn wait_for_startup_codecs_blocking(&self, limit: Duration) {
        let deadline = Instant::now() + limit;
        while !*self.startup_codecs.borrow() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(25));
        }
    }
    pub fn probe_codecs(self: &Arc<Self>) {
        let h = self.clone();
        std::thread::spawn(move || {
            use std::sync::atomic::Ordering;
            let mut retry = CodecRetry::new(Instant::now());
            let mut retrying = false;
            loop {
                if h.stop.load(Ordering::Acquire) {
                    return;
                }
                h.codec_probe_requested.store(false, Ordering::Release);
                h.probing_codecs.store(true, Ordering::Release);
                let mut config = h.config.read().unwrap().clone();
                // Probe the driver's ability independently of the stream's
                // opt-in: the default asks for long-term references in AV1.
                config.values.remove("amd_ltr_frames");
                let result = butterpollo_windows::input::on_input_desktop(|| {
                    h.probe_codecs_once(&config, retrying)
                });
                retrying = true;
                let Some((flags, error, retry_pyrowave)) =
                    result.unwrap_or_else(|error| Some((0, Some(format!("{error:#}")), None)))
                else {
                    // A stream is starting: leave the encoder to it and probe
                    // again once it has ended, without judging this attempt.
                    h.probing_codecs.store(false, Ordering::Release);
                    h.startup_codecs.send_replace(true);
                    h.metadata.lock().unwrap().take();
                    tracing::debug!("encoder capability probe paused for a stream");
                    if !h.wait_for_codec_retry(&retry) {
                        return;
                    }
                    continue;
                };
                h.codecs.store(flags, Ordering::Release);
                h.probing_codecs.store(false, Ordering::Release);
                h.startup_codecs.send_replace(true);
                if h.stop.load(Ordering::Acquire) {
                    return;
                }
                let now = Instant::now();
                let again = retry.completed(flags, error.as_deref(), &h.warnings, now);
                if flags & !0x40000000 != 0 {
                    tracing::info!(codec_flags = flags, "encoder capability probe completed");
                }
                h.metadata.lock().unwrap().take();
                // A stream stopped the pass's packed probe: probe those modes
                // once it has ended, not every codec again.
                if h.packed_probe_pending.load(Ordering::Acquire) != 0 {
                    h.start_packed_probe(butterpollo_windows::codec_probe::packed);
                }
                if !again {
                    if let Some(delay) = retry_pyrowave {
                        h.retry_pyrowave_probe(
                            &config,
                            delay,
                            butterpollo_windows::codec_probe::pyrowave,
                        );
                    }
                    return;
                }
                if !h.wait_for_codec_retry(&retry) {
                    return;
                }
            }
        });
    }
    /// The encoder focus check's verdict for a codec and bit depth.
    pub fn focus_support(
        &self,
        codec: u8,
        ten_bit: bool,
    ) -> butterpollo_core::encoder_focus::Support {
        butterpollo_core::encoder_focus::support_index(codec, ten_bit).map_or(
            butterpollo_core::encoder_focus::Support::NotProbed,
            |index| self.focus_support.lock().unwrap()[index].clone(),
        )
    }
    /// Keeps the throughput the encoder reports for its codec, for the WebRTC frame-rate
    /// offer. A driver that reports none leaves it 0, which limits the codec to 60 fps.
    fn record_throughput(&self, codec: u8, encoder: &butterpollo_windows::encoder::Encoder) {
        static WARNED: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
        let name = match codec {
            0 => "MaxThroughput",
            1 => "HevcMaxThroughput",
            _ => "Av1CapMaxThroughput",
        };
        let rate = encoder
            .capability(name)
            .and_then(|rate| u64::try_from(rate).ok())
            .unwrap_or(0);
        tracing::info!(
            codec,
            rate,
            cap = name,
            "encoder throughput (16x16 macroblocks per second)"
        );
        let bit = 1 << codec.min(2);
        if rate == 0 && WARNED.fetch_or(bit, std::sync::atomic::Ordering::Relaxed) & bit == 0 {
            tracing::warn!(
                codec,
                cap = name,
                "the encoder reports no throughput; WebRTC offers this codec at 60 fps and below"
            );
        }
        self.encoder_mb_rates[codec as usize].store(rate, std::sync::atomic::Ordering::Release);
    }
    /// One pass over every codec. A retry (`yield_to_streams`) stops before
    /// opening another encoder once a launch or stream appears, returning
    /// None: a probe must not compete with a stream for the encoder. Once the
    /// standard codecs are probed, it completes instead and leaves the
    /// optional probes for after the stream.
    fn probe_codecs_once(
        &self,
        config: &Config,
        yield_to_streams: bool,
    ) -> Result<Option<ProbePass>> {
        let _com = butterpollo_windows::capture::ComGuard::new()?;
        let busy = || yield_to_streams && !self.sessions.lock().unwrap().idle();
        // Until this pass publishes its result, keep advertising what the
        // separate PyroWave probe found last time.
        let pyrowave = self.codecs.load(std::sync::atomic::Ordering::Acquire) & PYROWAVE_CODECS;
        let mut first_error = None;
        let image = butterpollo_windows::capture::Image {
            width: 640,
            height: 480,
            stride: 2560,
            bytes: vec![128; 640 * 480 * 4],
            captured: Instant::now(),
            pixel: butterpollo_windows::capture::Pixel::Bgra8,
        };
        let mut flags = 0u32;
        let mut outcomes: Vec<(
            butterpollo_core::codec_support::Codec,
            butterpollo_core::codec_support::Outcome,
        )> = Vec::new();
        // The GPU the first encoder opened on, which wording a missing codec
        // must name: the probe may encode on a card other than the biggest.
        let mut adapter = String::new();
        let mut focus: [butterpollo_core::encoder_focus::Support; 5] =
            std::array::from_fn(|_| butterpollo_core::encoder_focus::Support::NotProbed);
        // Bits follow moonlight-common-c's SCM_* values. AMF encodes 4:2:0
        // only, so H.264, HEVC and AV1 advertise no 4:4:4 mode.
        for (codec, hdr, bit) in [
            (0, false, 1),
            (1, false, 0x100),
            (1, true, 0x200),
            (2, false, 0x10000),
            (2, true, 0x20000),
        ] {
            let mode = config.integer(if codec == 1 { "hevc_mode" } else { "av1_mode" }, 0);
            let probed = butterpollo_core::codec_support::Codec { id: codec, hdr };
            if matches!(codec, 1 | 2) && (mode == 1 || (hdr && mode == 2)) {
                outcomes.push((probed, butterpollo_core::codec_support::Outcome::Disabled));
                continue;
            }
            if self.stop.load(std::sync::atomic::Ordering::Acquire) {
                break;
            }
            if busy() {
                return Ok(None);
            }
            let negotiated = butterpollo_core::rtsp::Negotiated {
                width: 640,
                height: 480,
                fps: 30,
                bitrate_kbps: 2000,
                codec,
                hdr,
                ..Default::default()
            };
            match butterpollo_windows::encoder::Encoder::new_options(
                &negotiated,
                config.get("output_name", ""),
                config,
            ) {
                Ok(mut encoder) => {
                    if adapter.is_empty() {
                        adapter = encoder.adapter().unwrap_or_default().to_string();
                    }
                    if codec == 0 {
                        *self.probed_encoder.lock().unwrap() = encoder.backend();
                    }
                    if !hdr {
                        self.record_throughput(codec, &encoder);
                    }
                    let mut outcome = butterpollo_core::codec_support::Outcome::Failed(
                        "the encoder produced no packets".into(),
                    );
                    for frame in 0..8 {
                        match encoder.encode(&image, frame == 0, negotiated.bitrate_kbps) {
                            Ok(packets) if !packets.is_empty() => {
                                flags |= bit;
                                outcome = butterpollo_core::codec_support::Outcome::Available;
                                self.codecs
                                    .store(flags | pyrowave, std::sync::atomic::Ordering::Release);
                                if encoder.supports_invalidation() {
                                    flags |= 0x40000000;
                                }
                                break;
                            }
                            Err(error) => {
                                first_error.get_or_insert_with(|| format!("{error:#}"));
                                tracing::debug!(%error, codec, hdr, "encoder capability probe failed");
                                outcome = butterpollo_core::codec_support::Outcome::Failed(
                                    format!("{error:#}"),
                                );
                                break;
                            }
                            _ => std::thread::sleep(Duration::from_millis(5)),
                        }
                    }
                    if outcome == butterpollo_core::codec_support::Outcome::Available
                        && !busy()
                        && let Some(index) =
                            butterpollo_core::encoder_focus::support_index(codec, hdr)
                    {
                        focus[index] = focus_verdict(encoder.probe_focus());
                    }
                    outcomes.push((probed, outcome));
                }
                Err(error) => {
                    first_error.get_or_insert_with(|| format!("{error:#}"));
                    tracing::debug!(%error, codec, hdr, "encoder capability initialization failed");
                    outcomes.push((
                        probed,
                        butterpollo_core::codec_support::Outcome::Failed(format!("{error:#}")),
                    ));
                }
            }
        }
        self.explain_codecs(&adapter, &outcomes, flags & STANDARD_CODECS != 0);
        tracing::info!("encoder focus check: {}", focus_summary(&focus));
        *self.focus_support.lock().unwrap() = focus;
        // Publish standard codecs together before the optional Vulkan
        // probe. A driver/overlay failure in that child cannot kill them.
        self.codecs
            .store(flags | pyrowave, std::sync::atomic::Ordering::Release);
        // The standard codecs are probed: a stream that starts from here on
        // leaves the optional probes for after it, and the pass completes.
        {
            let _probe = self.optional_probe.lock().unwrap();
            let all = butterpollo_core::packed444::ALL_MODES;
            let live = || packed_modes_allowed(&self.config.read().unwrap());
            match probe_packed_modes(
                config,
                flags,
                all,
                &busy,
                &live,
                butterpollo_windows::codec_probe::packed,
            ) {
                Some(packed_modes) => self.publish_packed_modes(packed_modes, all),
                None => {
                    self.packed_probe_pending
                        .fetch_or(all, std::sync::atomic::Ordering::AcqRel);
                    self.publish_packed_modes(0, 0)
                }
            };
        }
        self.startup_codecs.send_replace(true);
        let mut retry_pyrowave = None;
        if config.boolean("pyrowave", true) && !self.stop.load(std::sync::atomic::Ordering::Acquire)
        {
            let result = if busy() {
                Err(butterpollo_windows::codec_probe::Interrupted.into())
            } else {
                butterpollo_windows::codec_probe::pyrowave(config, &busy)
            };
            let (optional, retry) = pyrowave_pass(result, pyrowave, flags & STANDARD_CODECS != 0);
            flags |= optional;
            retry_pyrowave = retry;
        }
        Ok(Some((flags, first_error, retry_pyrowave)))
    }
    /// Say why each missing codec is missing, once per pass: a log line per
    /// codec, the reason kept for a refused launch, and a console warning when
    /// another standard codec works. With none working, `CodecRetry` already
    /// warns "No video encoder available".
    fn explain_codecs(
        &self,
        adapter: &str,
        outcomes: &[(
            butterpollo_core::codec_support::Codec,
            butterpollo_core::codec_support::Outcome,
        )],
        any_codec: bool,
    ) {
        let missing = butterpollo_core::codec_support::explain_all(adapter, outcomes);
        for m in &missing {
            // A setting is the user's choice, not a fault.
            if m.reason.contains("turned off in the host settings") {
                tracing::info!(adapter, codec = m.codec.label(), reason = %m.reason, "codec not advertised");
            } else {
                tracing::warn!(adapter, codec = m.codec.label(), reason = %m.reason, "codec not advertised");
            }
        }
        match butterpollo_core::codec_support::warning(&missing).filter(|_| any_codec) {
            Some(message) => self.warnings.set("codec_support", message),
            None => self.warnings.clear("codec_support"),
        }
        *self.codec_missing.lock().unwrap() = missing;
    }
    fn wait_for_codec_retry(&self, retry: &CodecRetry) -> bool {
        use std::sync::atomic::Ordering;
        let mut session_check = Instant::now();
        loop {
            if self.stop.load(Ordering::Acquire) {
                return false;
            }
            let now = Instant::now();
            if now >= session_check
                && retry.due(now, self.codec_probe_requested.load(Ordering::Acquire))
            {
                // Standard encoders also serve the sign-in screen. Only the
                // optional user-session helper must wait for a signed-in user.
                if self.sessions.lock().unwrap().idle() {
                    return true;
                }
                session_check = now + CODEC_RETRY_MIN;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }
    /// A save changed the settings from `before`. The packed 4:4:4 modes they
    /// no longer allow are withdrawn at once, and modes they newly allow are
    /// probed once no stream runs or starts. Until then the startup probe was
    /// the only one, so turning full chroma on took a host restart.
    pub fn packed_settings_saved(self: &Arc<Self>, before: &Config) {
        self.packed_settings_saved_with(before, butterpollo_windows::codec_probe::packed);
    }
    pub(crate) fn packed_settings_saved_with(
        self: &Arc<Self>,
        before: &Config,
        probe: PackedProbe,
    ) {
        use std::sync::atomic::Ordering;
        // The settings alone, not the codecs probed so far: a save during the
        // startup probe must still ask for a probe.
        let was = packed_modes_allowed(before);
        let now = packed_modes_allowed(&self.config.read().unwrap());
        if was == now {
            return;
        }
        let offered = {
            let _publish = self.packed_publish.lock().unwrap();
            self.packed_modes.fetch_and(now, Ordering::AcqRel) & now
        };
        // Only the modes the save newly allows are probed: the others keep
        // what their last probe found. During the startup probe the codecs
        // are not known yet, and the probe after it checks them. With no
        // standard codec found yet, the codec retry's next pass probes them.
        let mut new = now & !was;
        let codecs = self.codecs.load(Ordering::Acquire);
        if !self.probing_codecs.load(Ordering::Acquire) {
            if codecs & STANDARD_CODECS == 0 {
                new = 0;
            }
            let offerable = packed_modes_offerable(&self.config.read().unwrap(), codecs);
            self.warn_packed_codec_missing(new & !offerable);
            new &= offerable;
        }
        if new == 0 {
            tracing::info!(
                modes = packed_mode_names(offered),
                "packed 4:4:4 modes offered to clients"
            );
            return;
        }
        self.packed_probe_pending.fetch_or(new, Ordering::AcqRel);
        self.start_packed_probe(probe);
    }
    /// Probe the pending packed 4:4:4 modes on a thread once no stream runs,
    /// unless one already waits or probes.
    fn start_packed_probe(self: &Arc<Self>, probe: PackedProbe) {
        use std::sync::atomic::Ordering;
        if self.packed_reprobing.swap(true, Ordering::AcqRel) {
            return;
        }
        let h = self.clone();
        std::thread::spawn(move || {
            loop {
                while h.packed_probe_pending.load(Ordering::Acquire) != 0 {
                    if !h.wait_until_idle("packed 4:4:4") {
                        h.packed_reprobing.store(false, Ordering::Release);
                        return;
                    }
                    h.reprobe_packed_modes(probe);
                }
                h.packed_reprobing.store(false, Ordering::Release);
                // A save between the last check and the store found this
                // thread still running and left its modes to it.
                if h.packed_probe_pending.load(Ordering::Acquire) == 0
                    || h.packed_reprobing.swap(true, Ordering::AcqRel)
                {
                    return;
                }
            }
        });
    }
    /// Warn once about each packed 4:4:4 mode in `modes`, which the settings
    /// allow but whose codec the startup probe did not find.
    fn warn_packed_codec_missing(&self, modes: u8) {
        use std::sync::atomic::Ordering;
        let new = modes & !self.packed_codec_missing.fetch_or(modes, Ordering::AcqRel);
        for (codec, ten_bit) in PACKED_MODES {
            let bit = butterpollo_core::packed444::mode_bit(codec, ten_bit);
            if new & bit != 0 {
                let codec = if codec == 1 { "HEVC" } else { "AV1" };
                tracing::warn!(
                    mode = packed_mode_names(bit),
                    "packed 4:4:4 mode not offered: {codec} not found by the startup probe; restart the host"
                );
            }
        }
    }
    /// Wait until a user is signed in, the startup probe has finished and no
    /// stream has run or started for `PROBE_IDLE`: a client that reconnects
    /// at once finds the encoder free. False when the host is stopping.
    fn wait_until_idle(&self, probe: &str) -> bool {
        use std::sync::atomic::Ordering;
        let mut idle_since = None;
        let mut waiting = false;
        loop {
            if self.stop.load(Ordering::Acquire) {
                return false;
            }
            if !self.probing_codecs.load(Ordering::Acquire) && self.probe_session_ready() {
                if idle_since.get_or_insert_with(Instant::now).elapsed() >= PROBE_IDLE {
                    return true;
                }
            } else {
                idle_since = None;
                if !std::mem::replace(&mut waiting, true) {
                    tracing::info!(probe, "optional codec probe waits until no stream runs");
                }
            }
            std::thread::sleep(PROBE_POLL);
        }
    }
    /// Probe the pending packed 4:4:4 modes the saved settings still allow
    /// and publish them. A stream that starts ends the probe and leaves its
    /// modes pending for after.
    fn reprobe_packed_modes(&self, probe: PackedProbe) -> bool {
        use std::sync::atomic::Ordering;
        let _probe = self.optional_probe.lock().unwrap();
        let pending = self.packed_probe_pending.swap(0, Ordering::AcqRel);
        let mut config = self.config.read().unwrap().clone();
        config.values.remove("amd_ltr_frames");
        let busy = || !self.sessions.lock().unwrap().idle();
        let live = || packed_modes_allowed(&self.config.read().unwrap());
        let codecs = self.codecs.load(Ordering::Acquire);
        let offerable = packed_modes_offerable(&config, codecs);
        // With no standard codec found, the codec retry's next pass probes
        // the packed modes as well: nothing is missing yet.
        if codecs & STANDARD_CODECS != 0 {
            self.warn_packed_codec_missing(pending & packed_modes_allowed(&config) & !offerable);
        }
        let modes = pending & offerable;
        if modes == 0 {
            return true;
        }
        match probe_packed_modes(&config, codecs, modes, &busy, &live, probe) {
            Some(found) => {
                self.publish_packed_modes(found, modes);
                true
            }
            None => {
                self.packed_probe_pending.fetch_or(modes, Ordering::AcqRel);
                tracing::info!(
                    "packed 4:4:4 probe stopped for a stream; it runs again once no stream runs"
                );
                false
            }
        }
    }
    /// Offer what a probe of the `probed` modes found, beside the modes it
    /// did not probe, narrowed to the modes the settings saved by now allow:
    /// a save during the probe that turned a mode off withdrew it, and one
    /// that turned a mode on asked for another probe. Packed modes of a codec
    /// clients are not offered stay off.
    fn publish_packed_modes(&self, found: u8, probed: u8) -> u8 {
        use std::sync::atomic::Ordering;
        let _publish = self.packed_publish.lock().unwrap();
        let config = self.config.read().unwrap();
        let offered = (self.packed_modes.load(Ordering::Acquire) & !probed | found & probed)
            & packed_modes_offerable(&config, self.codecs.load(Ordering::Acquire));
        self.packed_modes.store(offered, Ordering::Release);
        if config.boolean("experimental_packed_444", false) {
            tracing::info!(
                modes = packed_mode_names(offered),
                "packed 4:4:4 modes offered to clients"
            );
        }
        offered
    }
    /// A user is signed in, and no stream runs or is being launched: a
    /// launch asks for a probe, which must wait until its stream has ended.
    fn probe_session_ready(&self) -> bool {
        !(butterpollo_windows::process::is_system()
            && !butterpollo_windows::process::user_signed_in())
            && self.sessions.lock().unwrap().idle()
    }
    /// Wait `delay`, then until a user is signed in and no stream has run or
    /// started for `PROBE_IDLE`. False when the host is stopping.
    fn wait_for_session(&self, delay: Duration) -> bool {
        use std::sync::atomic::Ordering;
        let until = Instant::now() + delay;
        while Instant::now() < until {
            if self.stop.load(Ordering::Acquire) {
                return false;
            }
            std::thread::sleep(PROBE_POLL);
        }
        // Wait for a sign-in, and leave the GPU to a running stream.
        self.wait_until_idle("PyroWave")
    }
    /// Probe PyroWave again when the first probe ran before the signed-in
    /// user's session was ready, as when the host starts with Windows, or
    /// timed out, as under a busy CPU. PyroWave otherwise stayed unavailable
    /// until the host restarted. The first try waits `delay`; a timeout is
    /// not tried again, and a stream that starts stops the probe until it has
    /// ended, which counts as no attempt. Returns the attempts made.
    fn retry_pyrowave_probe(&self, config: &Config, delay: Duration, probe: PyroWaveProbe) -> u32 {
        use butterpollo_windows::codec_probe;
        use std::sync::atomic::Ordering;
        let mut delay = delay;
        let mut attempts = 0;
        while attempts < 6 {
            if !self.wait_for_session(delay) {
                return attempts;
            }
            let result = {
                let _probe = self.optional_probe.lock().unwrap();
                probe(config, &|| !self.sessions.lock().unwrap().idle())
            };
            if matches!(&result, Err(error) if error.is::<codec_probe::Interrupted>()) {
                tracing::debug!(
                    "optional PyroWave probe stopped for a stream; it runs again once no stream runs"
                );
                continue;
            }
            attempts += 1;
            match result {
                Ok(optional) => {
                    let flags = self.codecs.fetch_or(optional, Ordering::AcqRel) | optional;
                    self.metadata.lock().unwrap().take();
                    tracing::info!(
                        codec_flags = flags,
                        attempts,
                        "optional PyroWave probe completed when probed again"
                    );
                    return attempts;
                }
                Err(error) if error.is::<codec_probe::SessionNotReady>() && attempts < 6 => {
                    tracing::debug!(error = %format!("{error:#}"), attempts, "optional PyroWave probe waits for the session");
                }
                Err(error) => {
                    tracing::warn!(error = %format!("{error:#}"), attempts, "optional PyroWave probe failed again; standard codecs remain available");
                    return attempts;
                }
            }
            delay = (delay * 2).min(Duration::from_secs(60));
        }
        attempts
    }
    pub fn save_credentials(&self, credentials: &Credentials) -> Result<()> {
        let mut paired = self.paired.write().unwrap();
        let mut document = butterpollo_core::state::load_json(&self.credentials_path, json!({}))?;
        let values = serde_json::to_value(credentials)?;
        for (key, value) in values.as_object().unwrap() {
            document[key] = value.clone();
        }
        butterpollo_core::state::write_json(&self.credentials_path, &document)?;
        if self.credentials_path == self.paired_path {
            paired.document = document;
        }
        *self.credentials_stamp.lock().unwrap() = file_stamp(&self.credentials_path);
        Ok(())
    }
    /// Take up a sign-in changed on disk, as `--creds` changes it while the
    /// host runs, so the console accepts it at once. Apollo and Sunshine
    /// profiles keep the sign-in in the paired-state file; updating that
    /// copy too keeps the next save of the paired state from writing the
    /// previous sign-in back. Signed-in browsers sign in again, as after a
    /// password change in the console.
    pub fn reload_credentials(&self) {
        if *self.credentials_stamp.lock().unwrap() == file_stamp(&self.credentials_path) {
            return;
        }
        // The password change's order: sessions, paired state, sign-in.
        let mut sessions = self.web_sessions.lock().unwrap();
        {
            let mut paired = self.paired.write().unwrap();
            let mut seen = self.credentials_stamp.lock().unwrap();
            let current = file_stamp(&self.credentials_path);
            if *seen == current {
                return;
            }
            *seen = current;
            let loaded = match Credentials::load(&self.credentials_path) {
                Ok(loaded) => loaded,
                Err(error) => {
                    tracing::warn!(
                        error = %format!("{error:#}"),
                        "the console sign-in file is unreadable; the current sign-in stays"
                    );
                    return;
                }
            };
            let mut credentials = self.credentials.write().unwrap();
            if *credentials == loaded {
                return;
            }
            if self.credentials_path == self.paired_path
                && let Some(document) = paired.document.as_object_mut()
            {
                for key in ["username", "password", "salt"] {
                    document.remove(key);
                }
                if let Some(Value::Object(values)) =
                    loaded.as_ref().and_then(|c| serde_json::to_value(c).ok())
                {
                    document.extend(values);
                }
            }
            *credentials = loaded;
            tracing::info!(
                "the console sign-in was changed outside the console; browsers sign in again"
            );
        }
        if let Err(error) = self.revoke_sign_ins() {
            tracing::warn!(error = %format!("{error:#}"), "browser sessions and API tokens could not be cleared");
        }
        sessions.clear();
    }
    pub fn save_web_sessions(&self, sessions: &HashMap<String, WebSession>) -> Result<()> {
        let mut aliases = self.aliases.lock().unwrap();
        let mut document = aliases.clone();
        if !document["root"].is_object() {
            document["root"] = json!({});
        }
        document["root"]["session_tokens"] = Value::Array(
            sessions
                .iter()
                .filter(|(_, s)| s.refresh_expires > Instant::now())
                .map(|(key, s)| web_sessions::record(key, s))
                .collect(),
        );
        butterpollo_core::state::write_json(&self.aliases_path, &document)?;
        *aliases = document;
        Ok(())
    }
    /// A changed console sign-in ends what the old one let in: browser
    /// sessions and API tokens, in one write. A token is bound only to the
    /// username, so it would otherwise outlive the password it was made with.
    pub fn revoke_sign_ins(&self) -> Result<()> {
        let mut aliases = self.aliases.lock().unwrap();
        let mut document = aliases.clone();
        if !document["root"].is_object() {
            document["root"] = json!({});
        }
        document["root"]["session_tokens"] = json!([]);
        document["root"]["api_tokens"] = json!([]);
        butterpollo_core::state::write_json(&self.aliases_path, &document)?;
        *aliases = document;
        Ok(())
    }
    pub fn new_web_session(
        &self,
        username: String,
        remember_me: bool,
        user_agent: String,
        remote_address: String,
        previous: Option<WebSession>,
    ) -> Result<(String, String, String, u64)> {
        let ttl = self
            .config
            .read()
            .unwrap()
            .integer("session_token_ttl_seconds", 7200)
            .clamp(60, 604800) as u64;
        let access = hex::encode(butterpollo_core::crypto::random::<32>());
        let refresh = hex::encode(butterpollo_core::crypto::random::<32>());
        let csrf = hex::encode(butterpollo_core::crypto::random::<32>());
        let wall = web_sessions::now();
        let refresh_deadline = previous
            .as_ref()
            .map(|s| s.refresh_deadline)
            .unwrap_or_else(|| {
                wall + if remember_me {
                    self.config
                        .read()
                        .unwrap()
                        .integer("remember_me_refresh_token_ttl_seconds", 604800)
                        .clamp(60, 366 * 86400) as u64
                } else {
                    ttl.max(86400)
                }
            });
        let access_deadline = (wall + ttl).min(refresh_deadline);
        let mut extra = previous
            .as_ref()
            .map(|s| s.extra.clone())
            .unwrap_or_default();
        extra.insert(
            "rotation_id".into(),
            hex::encode(butterpollo_core::crypto::random::<12>()).into(),
        );
        let mut sessions = self.web_sessions.lock().unwrap();
        let live = previous.as_ref().map(|previous| {
            sessions
                .values()
                .find(|s| s.refresh == previous.refresh && s.refresh_expires > Instant::now())
        });
        if let Some(None) = live {
            return Err(web_sessions::Rotated.into());
        }
        // A refresh keeps the sign-in. It is read under the lock: the console's stream socket may
        // have given the live session its ID after `previous` was copied.
        let sign_in = live
            .flatten()
            .and_then(|s| s.extra.get(web_sessions::SIGN_IN_ID).cloned())
            .unwrap_or_else(|| hex::encode(butterpollo_core::crypto::random::<16>()).into());
        extra.insert(web_sessions::SIGN_IN_ID.into(), sign_in);
        let mut next = sessions.clone();
        next.retain(|_, s| s.refresh_expires > Instant::now());
        if let Some(previous) = previous.as_ref() {
            next.retain(|_, s| s.refresh != previous.refresh);
        }
        if next.len() >= 64
            && let Some(key) = next
                .iter()
                .min_by_key(|(_, s)| s.created)
                .map(|(key, _)| key.clone())
        {
            next.remove(&key);
        }
        next.insert(
            web_sessions::hash(&access),
            WebSession {
                created: wall,
                last_seen: wall,
                refresh: web_sessions::hash(&refresh),
                csrf: csrf.clone(),
                expires: Instant::now() + Duration::from_secs(access_deadline.saturating_sub(wall)),
                refresh_expires: Instant::now()
                    + Duration::from_secs(refresh_deadline.saturating_sub(wall)),
                access_deadline,
                refresh_deadline,
                username,
                remember_me,
                user_agent,
                remote_address,
                extra,
            },
        );
        self.save_web_sessions(&next)?;
        *sessions = next;
        Ok((access, refresh, csrf, refresh_deadline.saturating_sub(wall)))
    }
}
/// The focus check's verdict. Its error marks only this codec unsupported:
/// the codec itself passed and stays offered.
fn focus_verdict(
    probe: Result<butterpollo_core::encoder_focus::Support>,
) -> butterpollo_core::encoder_focus::Support {
    probe.unwrap_or_else(
        |error| butterpollo_core::encoder_focus::Support::Unsupported {
            reason: butterpollo_core::encoder_focus::Reason::CodecUnsupported,
            message: format!("the focus check failed: {error:#}"),
        },
    )
}
/// `h264=verified(+10.0) hevc=... av1_10=not_probed`.
fn focus_summary(support: &[butterpollo_core::encoder_focus::Support; 5]) -> String {
    butterpollo_core::encoder_focus::SUPPORT_NAMES
        .iter()
        .zip(support)
        .map(|(name, support)| format!("{name}={}", support.summary()))
        .collect::<Vec<_>>()
        .join(" ")
}
/// A finished probe pass: the codec flags, the first encoder error, and
/// when to probe PyroWave again if its probe is worth repeating.
type ProbePass = (u32, Option<String>, Option<Duration>);
/// How long the host must have had no stream before an optional probe
/// starts, so a client that reconnects at once finds the encoder free.
const PROBE_IDLE: Duration = if cfg!(test) {
    Duration::from_millis(300)
} else {
    Duration::from_secs(5)
};
const PROBE_POLL: Duration = Duration::from_millis(if cfg!(test) { 25 } else { 250 });
/// When the startup PyroWave probe is worth trying again, and after how
/// long: it ran before the user's session was ready, or it timed out, as
/// under a busy CPU, which a game that keeps running would busy again soon
/// after. A crashing driver or overlay stays a failure.
fn pyrowave_retry_delay(error: &anyhow::Error) -> Option<Duration> {
    use butterpollo_windows::codec_probe::{SessionNotReady, TimedOut};
    if error.is::<SessionNotReady>() {
        Some(Duration::from_secs(5))
    } else if error.is::<TimedOut>() {
        Some(Duration::from_secs(60))
    } else {
        None
    }
}
/// The PyroWave flags a codec pass advertises, and when to probe PyroWave
/// again. A stream that stopped the probe keeps `previous`, what the last
/// probe found, and the probe runs again once the stream has ended: the
/// pass still completes, and its standard codecs are not probed again.
fn pyrowave_pass(result: Result<u32>, previous: u32, standard: bool) -> (u32, Option<Duration>) {
    match result {
        Ok(optional) => (optional, None),
        Err(error) if error.is::<butterpollo_windows::codec_probe::Interrupted>() => {
            tracing::debug!(
                "optional PyroWave probe stopped for a stream; it runs again once no stream runs"
            );
            (previous, Some(Duration::from_secs(5)))
        }
        Err(error) => {
            let retry = pyrowave_retry_delay(&error);
            if standard {
                tracing::warn!(error = %format!("{error:#}"), retry_in_s = retry.map(|delay| delay.as_secs()), "optional PyroWave probe failed; standard codecs remain available");
            } else {
                tracing::debug!(error = %format!("{error:#}"), "optional PyroWave probe failed");
            }
            (0, retry)
        }
    }
}
/// Checks PyroWave on the encoder: `codec_probe::pyrowave`, or a fake in
/// tests. The closure says a stream wants the encoder.
pub(crate) type PyroWaveProbe = fn(&Config, &dyn Fn() -> bool) -> Result<u32>;
/// Checks one packed 4:4:4 mode on the encoder: `codec_probe::packed`, or a
/// fake in tests. The closure says a stream wants the encoder.
pub(crate) type PackedProbe =
    fn(&Config, &butterpollo_core::rtsp::Negotiated, &dyn Fn() -> bool) -> Result<()>;
const PACKED_MODES: [(u8, bool); 4] = [(1, false), (1, true), (2, false), (2, true)];
/// The packed 4:4:4 modes the settings let clients be offered: none while
/// full chroma is off, and none for a codec or 10-bit mode turned off.
fn packed_modes_allowed(config: &Config) -> u8 {
    if !config.boolean("experimental_packed_444", false) {
        return 0;
    }
    PACKED_MODES
        .into_iter()
        .filter(|&(codec, ten_bit)| {
            let mode = config.integer(if codec == 1 { "hevc_mode" } else { "av1_mode" }, 0);
            !(mode == 1 || (ten_bit && mode == 2))
        })
        .fold(0, |modes, (codec, ten_bit)| {
            modes | butterpollo_core::packed444::mode_bit(codec, ten_bit)
        })
}
/// `packed_modes_allowed`, less the modes of a codec the startup probe did
/// not find in `codecs` (`ServerCodecModeSupport`): turning HEVC or AV1 on
/// takes a restart, and a packed mode of a codec clients are not offered
/// is never chosen.
fn packed_modes_offerable(config: &Config, codecs: u32) -> u8 {
    PACKED_MODES
        .into_iter()
        .filter(|&(codec, _)| codecs & if codec == 1 { 0x100 } else { 0x10000 } != 0)
        .fold(0, |modes, (codec, ten_bit)| {
            modes | butterpollo_core::packed444::mode_bit(codec, ten_bit)
        })
        & packed_modes_allowed(config)
}
/// Probe each packed 4:4:4 mode in `modes` the settings and `codecs` allow,
/// warning about each that fails. None once `busy` says a stream wants the
/// encoder, which also stops a probe that runs. A mode the settings saved by
/// now (`live`) no longer allow is skipped, or stopped while it is probed.
fn probe_packed_modes(
    config: &Config,
    codecs: u32,
    modes: u8,
    busy: &dyn Fn() -> bool,
    live: &dyn Fn() -> u8,
    probe: PackedProbe,
) -> Option<u8> {
    let allowed = modes & packed_modes_offerable(config, codecs);
    let mut packed_modes = 0;
    for (codec, ten_bit) in PACKED_MODES {
        let bit = butterpollo_core::packed444::mode_bit(codec, ten_bit);
        if allowed & bit == 0 || live() & bit == 0 {
            continue;
        }
        if busy() {
            return None;
        }
        let negotiated = butterpollo_core::rtsp::Negotiated {
            width: 640,
            height: 480,
            fps: 30,
            bitrate_kbps: 2000,
            codec,
            sdr_10bit: ten_bit,
            chroma_packing: butterpollo_core::packed444::VERSION,
            ..Default::default()
        };
        let probing = std::time::Instant::now();
        match probe(config, &negotiated, &|| busy() || live() & bit == 0) {
            Ok(()) => packed_modes |= bit,
            Err(error) if error.is::<butterpollo_windows::codec_probe::Interrupted>() => {
                if live() & bit != 0 {
                    return None;
                }
                tracing::info!(
                    mode = packed_mode_names(bit),
                    "packed 4:4:4 probe stopped: the mode was turned off"
                );
            }
            // Packed 4:4:4 is opt-in: a mode the user turned on but clients
            // are not offered must show in the log.
            Err(error) => tracing::warn!(
                error = %format!("{error:#}"),
                mode = packed_mode_names(bit),
                elapsed_ms = probing.elapsed().as_millis() as u64,
                "packed 4:4:4 probe failed; clients are not offered this mode"
            ),
        }
    }
    Some(packed_modes)
}
/// Names of the packed 4:4:4 modes in a `packed444Modes` mask.
fn packed_mode_names(modes: u8) -> String {
    let names: Vec<_> = [
        (1, false, "HEVC 8-bit"),
        (1, true, "HEVC 10-bit"),
        (2, false, "AV1 8-bit"),
        (2, true, "AV1 10-bit"),
    ]
    .into_iter()
    .filter(|&(codec, ten_bit, _)| {
        modes & butterpollo_core::packed444::mode_bit(codec, ten_bit) != 0
    })
    .map(|(_, _, name)| name)
    .collect();
    if names.is_empty() {
        "none".into()
    } else {
        names.join(", ")
    }
}
/// The app library. One that cannot be read as a library starts the host
/// with no apps, as in Vibepollo, rather than keeping the host and its web
/// console down; the file is copied beside it so a later save cannot lose it.
fn load_library(path: &std::path::Path, default: Value) -> Result<Value> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(default),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    let Some(json) = butterpollo_core::state::json_bytes(&bytes) else {
        return Ok(default);
    };
    let parsed = serde_json::from_slice::<Value>(json)
        .map_err(anyhow::Error::from)
        .and_then(|document| {
            serde_json::from_value::<Vec<App>>(document.get("apps").cloned().unwrap_or(json!([])))?;
            Ok(document)
        });
    match parsed {
        Ok(document) => Ok(document),
        Err(error) => {
            let mut kept = path.as_os_str().to_owned();
            kept.push(".invalid");
            let kept = PathBuf::from(kept);
            butterpollo_core::state::atomic_write(&kept, &bytes)?;
            tracing::error!(
                error = %format!("{error:#}"),
                file = %path.display(),
                kept = %kept.display(),
                "the app library is not valid; starting with no apps"
            );
            Ok(json!({"env":{},"apps":[]}))
        }
    }
}
#[cfg(test)]
mod tests {
    #[test]
    fn a_focus_check_error_marks_only_that_codec_unsupported() {
        use butterpollo_core::encoder_focus::{Reason, Support};
        let mut support: [Support; 5] = std::array::from_fn(|_| Support::NotProbed);
        support[0] = focus_verdict(Ok(Support::Verified { delta: 10.0 }));
        support[1] = focus_verdict(Err(anyhow::anyhow!("frame 3 was not encoded")));
        support[3] = focus_verdict(Ok(Support::Verified { delta: 50.0 }));
        assert_eq!(
            support[1],
            Support::Unsupported {
                reason: Reason::CodecUnsupported,
                message: "the focus check failed: frame 3 was not encoded".into(),
            }
        );
        assert_eq!(
            focus_summary(&support),
            "h264=verified(+10.0) hevc=unsupported(the focus check failed: frame 3 was not encoded) hevc10=not_probed av1=verified(+50.0) av1_10=not_probed"
        );
    }
    #[test]
    fn a_timed_out_or_early_pyrowave_probe_is_probed_again() {
        use butterpollo_windows::codec_probe::{SessionNotReady, TimedOut};
        use std::time::Duration;
        // A probe that stalled under a busy CPU left PyroWave unoffered
        // until the host restarted. The load that stalled it, such as a
        // game, is given a minute to pass.
        assert_eq!(
            super::pyrowave_retry_delay(&TimedOut.into()),
            Some(Duration::from_secs(60))
        );
        assert_eq!(
            super::pyrowave_retry_delay(&SessionNotReady::new("no user").into()),
            Some(Duration::from_secs(5))
        );
        assert_eq!(
            super::pyrowave_retry_delay(&anyhow::anyhow!(
                "optional codec probe exited before replying (0xc0000005)"
            )),
            None
        );
    }

    /// PyroWave probed again after the startup probe: a timeout is tried once
    /// more only, a session not ready yet until it is, and a stream that
    /// starts stops the probe until it has ended.
    mod pyrowave_retry {
        use crate::state::test_support::Fixture;
        use butterpollo_core::config::Config;
        use butterpollo_windows::codec_probe::{Interrupted, SessionNotReady, TimedOut};
        use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
        use std::time::{Duration, Instant};

        const PYROWAVE: u32 = 0x0080_0000;

        fn until(what: &str, mut done: impl FnMut() -> bool) {
            let deadline = Instant::now() + Duration::from_secs(10);
            while !done() {
                assert!(Instant::now() < deadline, "{what}");
                std::thread::sleep(Duration::from_millis(10));
            }
        }

        #[test]
        fn a_timeout_is_probed_again_exactly_once() {
            static CALLS: AtomicU32 = AtomicU32::new(0);
            fn probe(_: &Config, _: &dyn Fn() -> bool) -> anyhow::Result<u32> {
                CALLS.fetch_add(1, Ordering::AcqRel);
                Err(TimedOut.into())
            }
            let f = Fixture::new();
            let config = f.host.config.read().unwrap().clone();
            assert_eq!(
                f.host.retry_pyrowave_probe(&config, Duration::ZERO, probe),
                1
            );
            assert_eq!(CALLS.load(Ordering::Acquire), 1);
            assert_eq!(f.host.codecs.load(Ordering::Acquire) & PYROWAVE, 0);
        }

        #[test]
        fn a_session_not_ready_is_probed_until_it_is() {
            static CALLS: AtomicU32 = AtomicU32::new(0);
            fn probe(_: &Config, _: &dyn Fn() -> bool) -> anyhow::Result<u32> {
                if CALLS.fetch_add(1, Ordering::AcqRel) < 2 {
                    return Err(SessionNotReady::new("nobody is signed in").into());
                }
                Ok(PYROWAVE)
            }
            let f = Fixture::new();
            let config = f.host.config.read().unwrap().clone();
            assert_eq!(
                f.host.retry_pyrowave_probe(&config, Duration::ZERO, probe),
                3
            );
            assert_ne!(f.host.codecs.load(Ordering::Acquire) & PYROWAVE, 0);
        }

        #[test]
        fn a_stream_that_starts_stops_the_probe_until_it_ends() {
            static CALLS: AtomicU32 = AtomicU32::new(0);
            static PROBING: AtomicBool = AtomicBool::new(false);
            fn probe(_: &Config, stop: &dyn Fn() -> bool) -> anyhow::Result<u32> {
                if CALLS.fetch_add(1, Ordering::AcqRel) > 0 {
                    return Ok(PYROWAVE);
                }
                PROBING.store(true, Ordering::Release);
                let deadline = Instant::now() + Duration::from_secs(10);
                while !stop() {
                    anyhow::ensure!(
                        Instant::now() < deadline,
                        "the stream did not stop the probe"
                    );
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(Interrupted.into())
            }
            let f = Fixture::new();
            let config = f.host.config.read().unwrap().clone();
            let host = f.host.clone();
            let retry = std::thread::spawn(move || {
                host.retry_pyrowave_probe(&config, Duration::ZERO, probe)
            });
            until("the probe started", || PROBING.load(Ordering::Acquire));
            f.host
                .sessions
                .lock()
                .unwrap()
                .teardown
                .insert("stream".into(), Instant::now());
            until("the stream stopped the probe", || {
                f.host.optional_probe.try_lock().is_ok()
            });
            std::thread::sleep(Duration::from_millis(400));
            assert_eq!(
                CALLS.load(Ordering::Acquire),
                1,
                "no probe while the stream runs"
            );
            f.host.sessions.lock().unwrap().teardown.clear();
            // The stopped probe was no attempt.
            assert_eq!(retry.join().unwrap(), 1);
            assert_eq!(CALLS.load(Ordering::Acquire), 2);
            assert_ne!(f.host.codecs.load(Ordering::Acquire) & PYROWAVE, 0);
        }

        /// Streams that stop the probe use up none of its six attempts: one
        /// more stream no longer leaves PyroWave off until a restart.
        #[test]
        fn streams_that_stop_the_probe_use_up_no_attempt() {
            static CALLS: AtomicU32 = AtomicU32::new(0);
            fn probe(_: &Config, _: &dyn Fn() -> bool) -> anyhow::Result<u32> {
                if CALLS.fetch_add(1, Ordering::AcqRel) < 7 {
                    return Err(Interrupted.into());
                }
                Ok(PYROWAVE)
            }
            let f = Fixture::new();
            let config = f.host.config.read().unwrap().clone();
            assert_eq!(
                f.host.retry_pyrowave_probe(&config, Duration::ZERO, probe),
                1
            );
            assert_eq!(CALLS.load(Ordering::Acquire), 8);
            assert_ne!(f.host.codecs.load(Ordering::Acquire) & PYROWAVE, 0);
        }

        /// A stream that stops a codec pass's PyroWave probe, or starts
        /// before it, completes the pass with the PyroWave modes found before
        /// and a probe after the stream: the "No video encoder" retry ends
        /// and the standard codecs are not probed again.
        #[test]
        fn a_stream_that_stops_the_pass_probe_keeps_its_modes_and_probes_after() {
            let (flags, retry) =
                crate::state::pyrowave_pass(Err(Interrupted.into()), PYROWAVE, true);
            assert_eq!((flags, retry), (PYROWAVE, Some(Duration::from_secs(5))));
            let (flags, retry) = crate::state::pyrowave_pass(Ok(PYROWAVE), 0, true);
            assert_eq!((flags, retry), (PYROWAVE, None));
            let (flags, retry) = crate::state::pyrowave_pass(Err(TimedOut.into()), PYROWAVE, true);
            assert_eq!((flags, retry), (0, Some(Duration::from_secs(60))));
            let failed =
                crate::state::pyrowave_pass(Err(anyhow::anyhow!("no DLL")), PYROWAVE, true);
            assert_eq!(failed, (0, None));
        }
    }

    /// Packed 4:4:4 probed again after a save, with saves and streams that
    /// land while the probe runs.
    mod packed_reprobe {
        use crate::state::test_support::Fixture;
        use butterpollo_core::{config::Config, packed444::mode_bit, rtsp::Negotiated};
        use butterpollo_windows::codec_probe::Interrupted;
        use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
        use std::time::{Duration, Instant};

        /// H.264, HEVC and AV1 found by the startup probe.
        const CODECS: u32 = 0x1 | 0x100 | 0x200 | 0x10000 | 0x20000;

        fn until(what: &str, mut done: impl FnMut() -> bool) {
            let deadline = Instant::now() + Duration::from_secs(10);
            while !done() {
                assert!(Instant::now() < deadline, "{what}");
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        fn save(f: &Fixture, key: &str, value: &str, probe: crate::state::PackedProbe) {
            let before = f.host.config.read().unwrap().clone();
            f.host
                .config
                .write()
                .unwrap()
                .values
                .insert(key.into(), value.into());
            f.host.packed_settings_saved_with(&before, probe);
        }
        fn offered(f: &Fixture) -> u8 {
            f.host.packed_modes.load(Ordering::Acquire)
        }

        #[test]
        fn a_stream_that_starts_stops_the_probe_and_it_runs_again_after() {
            static CALLS: AtomicU32 = AtomicU32::new(0);
            static PROBING: AtomicBool = AtomicBool::new(false);
            fn probe(_: &Config, _: &Negotiated, stop: &dyn Fn() -> bool) -> anyhow::Result<()> {
                if CALLS.fetch_add(1, Ordering::AcqRel) > 0 {
                    return Ok(());
                }
                PROBING.store(true, Ordering::Release);
                let deadline = Instant::now() + Duration::from_secs(10);
                while !stop() {
                    anyhow::ensure!(
                        Instant::now() < deadline,
                        "the stream did not stop the probe"
                    );
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(Interrupted.into())
            }
            let f = Fixture::new();
            f.host.codecs.store(CODECS, Ordering::Release);
            save(&f, "experimental_packed_444", "true", probe);
            until("the probe started", || PROBING.load(Ordering::Acquire));
            f.host
                .sessions
                .lock()
                .unwrap()
                .teardown
                .insert("stream".into(), Instant::now());
            until("the probe stopped", || {
                f.host.packed_probe_pending.load(Ordering::Acquire)
                    == butterpollo_core::packed444::ALL_MODES
            });
            std::thread::sleep(Duration::from_millis(400));
            assert_eq!(
                CALLS.load(Ordering::Acquire),
                1,
                "no probe while the stream runs"
            );
            assert_eq!(offered(&f), 0);
            f.host.sessions.lock().unwrap().teardown.clear();
            until("the modes are probed after the stream", || {
                offered(&f) == butterpollo_core::packed444::ALL_MODES
            });
        }

        /// A save that turns a mode off while the probe runs narrows what it
        /// finds instead of throwing it away; one that turns a mode on gets a
        /// probe of its own.
        #[test]
        fn saves_during_the_probe_narrow_its_result_or_probe_again() {
            static PROBING: AtomicBool = AtomicBool::new(false);
            static RELEASE: AtomicBool = AtomicBool::new(false);
            fn probe(_: &Config, _: &Negotiated, _: &dyn Fn() -> bool) -> anyhow::Result<()> {
                if !PROBING.swap(true, Ordering::AcqRel) {
                    let deadline = Instant::now() + Duration::from_secs(10);
                    while !RELEASE.load(Ordering::Acquire) {
                        anyhow::ensure!(Instant::now() < deadline, "never released");
                        std::thread::sleep(Duration::from_millis(5));
                    }
                }
                Ok(())
            }
            let f = Fixture::new();
            f.host.codecs.store(CODECS, Ordering::Release);
            f.host
                .config
                .write()
                .unwrap()
                .values
                .insert("hevc_mode".into(), "1".into());
            save(&f, "experimental_packed_444", "true", probe);
            until("the probe started", || PROBING.load(Ordering::Acquire));
            // AV1 SDR only: the running probe skips 10-bit AV1.
            save(&f, "av1_mode", "2", probe);
            save(&f, "hevc_mode", "0", probe);
            RELEASE.store(true, Ordering::Release);
            let expected = mode_bit(1, false) | mode_bit(1, true) | mode_bit(2, false);
            until("the narrowed and follow-up results are offered", || {
                offered(&f) == expected
            });
            until("the follow-up probe finished", || {
                !f.host.packed_reprobing.load(Ordering::Acquire)
            });
            assert_eq!(offered(&f), expected);
        }

        /// Turning HEVC on while the host runs offers no packed HEVC: clients
        /// see HEVC in ServerCodecModeSupport only after a restart.
        #[test]
        fn packed_modes_of_a_codec_not_offered_stay_off() {
            static AV1_PROBED: AtomicBool = AtomicBool::new(false);
            fn probe(_: &Config, stream: &Negotiated, _: &dyn Fn() -> bool) -> anyhow::Result<()> {
                if stream.codec == 2 {
                    AV1_PROBED.store(true, Ordering::Release);
                }
                Ok(())
            }
            let f = Fixture::new();
            // The startup probe found no AV1.
            f.host.codecs.store(0x1 | 0x100 | 0x200, Ordering::Release);
            save(&f, "experimental_packed_444", "true", probe);
            until("the HEVC modes are probed", || {
                offered(&f) == mode_bit(1, false) | mode_bit(1, true)
            });
            until("the probe finished", || {
                !f.host.packed_reprobing.load(Ordering::Acquire)
            });
            assert!(!AV1_PROBED.load(Ordering::Acquire));
        }

        fn logged(path: &std::path::Path, run: impl FnOnce()) -> String {
            let subscriber = tracing_subscriber::fmt()
                .with_max_level(tracing::Level::WARN)
                .without_time()
                .with_ansi(false)
                .with_target(false)
                .with_writer(std::sync::Mutex::new(std::fs::File::create(path).unwrap()))
                .finish();
            tracing::subscriber::with_default(subscriber, run);
            std::fs::read_to_string(path).unwrap()
        }

        /// A save that allows only modes of a codec the startup probe did not
        /// find probes nothing, keeps what is offered and says once why.
        #[test]
        fn a_save_that_allows_only_modes_of_a_missing_codec_probes_nothing() {
            fn probe(_: &Config, _: &Negotiated, _: &dyn Fn() -> bool) -> anyhow::Result<()> {
                panic!("nothing to probe")
            }
            let f = Fixture::new();
            // HEVC was off at startup: only AV1 was found and offered.
            f.host
                .codecs
                .store(0x1 | 0x10000 | 0x20000, Ordering::Release);
            let av1 = mode_bit(2, false) | mode_bit(2, true);
            {
                let mut config = f.host.config.write().unwrap();
                config
                    .values
                    .insert("experimental_packed_444".into(), "true".into());
                config.values.insert("hevc_mode".into(), "1".into());
            }
            f.host.packed_modes.store(av1, Ordering::Release);
            let path = f.host.directory.join("missing.log");
            let log = logged(&path, || {
                save(&f, "hevc_mode", "0", probe);
                save(&f, "hevc_mode", "1", probe);
                save(&f, "hevc_mode", "0", probe);
            });
            assert!(!f.host.packed_reprobing.load(Ordering::Acquire));
            assert_eq!(offered(&f), av1);
            for mode in ["HEVC 8-bit", "HEVC 10-bit"] {
                assert_eq!(log.matches(&format!("mode=\"{mode}\"")).count(), 1, "{log}");
            }
            assert!(
                log.contains("HEVC not found by the startup probe; restart the host"),
                "{log}"
            );
        }

        /// While the startup probe found no standard codec and retries, a
        /// save leaves the packed modes to the next pass: no probe, and no
        /// restart warning that would use up the one a missing codec gets.
        #[test]
        fn a_save_before_any_codec_is_found_leaves_the_modes_to_the_next_pass() {
            fn probe(_: &Config, _: &Negotiated, _: &dyn Fn() -> bool) -> anyhow::Result<()> {
                panic!("the next codec pass probes them")
            }
            let f = Fixture::new();
            f.host.codecs.store(0, Ordering::Release);
            let path = f.host.directory.join("no-codec.log");
            let log = logged(&path, || save(&f, "experimental_packed_444", "true", probe));
            assert!(log.is_empty(), "{log}");
            assert!(!f.host.packed_reprobing.load(Ordering::Acquire));
            assert_eq!(f.host.packed_probe_pending.load(Ordering::Acquire), 0);
            assert_eq!(offered(&f), 0);
            // The next pass finds AV1 but not HEVC: the warning is still due.
            f.host
                .codecs
                .store(0x1 | 0x10000 | 0x20000, Ordering::Release);
            f.host
                .packed_modes
                .store(mode_bit(2, false) | mode_bit(2, true), Ordering::Release);
            save(&f, "hevc_mode", "1", probe);
            let path = f.host.directory.join("hevc-missing.log");
            let log = logged(&path, || save(&f, "hevc_mode", "0", probe));
            assert!(
                log.contains("HEVC not found by the startup probe; restart the host"),
                "{log}"
            );
        }

        /// A save that newly allows a mode probes only it: a mode already
        /// offered is neither probed again nor withdrawn by a failed probe.
        #[test]
        fn a_save_probes_only_the_modes_it_newly_allows() {
            static AV1_PROBED: AtomicBool = AtomicBool::new(false);
            fn probe(_: &Config, stream: &Negotiated, _: &dyn Fn() -> bool) -> anyhow::Result<()> {
                if stream.codec == 2 {
                    AV1_PROBED.store(true, Ordering::Release);
                    anyhow::bail!("timed out under a game");
                }
                Ok(())
            }
            let f = Fixture::new();
            f.host.codecs.store(CODECS, Ordering::Release);
            {
                let mut config = f.host.config.write().unwrap();
                config
                    .values
                    .insert("experimental_packed_444".into(), "true".into());
                config.values.insert("hevc_mode".into(), "2".into());
            }
            let offered_before = mode_bit(1, false) | mode_bit(2, false) | mode_bit(2, true);
            f.host.packed_modes.store(offered_before, Ordering::Release);
            save(&f, "hevc_mode", "0", probe);
            until("HEVC 10-bit is probed", || {
                offered(&f) == butterpollo_core::packed444::ALL_MODES
            });
            until("the probe finished", || {
                !f.host.packed_reprobing.load(Ordering::Acquire)
            });
            assert!(!AV1_PROBED.load(Ordering::Acquire));
            assert_eq!(offered(&f), butterpollo_core::packed444::ALL_MODES);
        }

        /// Turning full chroma off while its probe runs stops the probe at
        /// once and probes nothing after.
        #[test]
        fn turning_full_chroma_off_stops_its_running_probe() {
            static CALLS: AtomicU32 = AtomicU32::new(0);
            static PROBING: AtomicBool = AtomicBool::new(false);
            static STOPPED: AtomicBool = AtomicBool::new(false);
            fn probe(_: &Config, _: &Negotiated, stop: &dyn Fn() -> bool) -> anyhow::Result<()> {
                CALLS.fetch_add(1, Ordering::AcqRel);
                PROBING.store(true, Ordering::Release);
                let deadline = Instant::now() + Duration::from_secs(10);
                while !stop() {
                    anyhow::ensure!(Instant::now() < deadline, "the save did not stop the probe");
                    std::thread::sleep(Duration::from_millis(5));
                }
                STOPPED.store(true, Ordering::Release);
                Err(Interrupted.into())
            }
            let f = Fixture::new();
            f.host.codecs.store(CODECS, Ordering::Release);
            save(&f, "experimental_packed_444", "true", probe);
            until("the probe started", || PROBING.load(Ordering::Acquire));
            save(&f, "experimental_packed_444", "false", probe);
            until("the probe finished", || {
                !f.host.packed_reprobing.load(Ordering::Acquire)
            });
            assert!(STOPPED.load(Ordering::Acquire));
            assert_eq!(CALLS.load(Ordering::Acquire), 1);
            assert_eq!(f.host.packed_probe_pending.load(Ordering::Acquire), 0);
            assert_eq!(offered(&f), 0);
        }
    }
    #[test]
    fn packed_modes_are_logged_by_name() {
        assert_eq!(super::packed_mode_names(0), "none");
        assert_eq!(super::packed_mode_names(1 | 4), "HEVC 8-bit, AV1 8-bit");
        assert_eq!(
            super::packed_mode_names(butterpollo_core::packed444::ALL_MODES),
            "HEVC 8-bit, HEVC 10-bit, AV1 8-bit, AV1 10-bit"
        );
    }
    #[test]
    fn settings_of_removed_encoders_and_limiters_migrate_with_one_warning() {
        use super::test_support::Fixture;
        let directory = Fixture::profile();
        let path = directory.join("sunshine.conf");
        std::fs::write(
            &path,
            "encoder = nvenc\nframe_limiter_provider = nvidia-control-panel\nnvenc_preset = 4\nsunshine_name = Host\n",
        )
        .unwrap();
        let first = Fixture::load(directory.clone());
        {
            let config = first.host.config.read().unwrap();
            assert_eq!(config.get("encoder", ""), "amf");
            assert_eq!(config.get("frame_limiter_provider", ""), "auto");
            assert!(!config.values.contains_key("nvenc_preset"));
            assert_eq!(config.get("sunshine_name", ""), "Host");
        }
        let warnings = first.host.warnings.snapshot();
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert_eq!(warnings[0].code, "settings_retired");
        assert!(warnings[0].message.contains("encoder nvenc is now amf"));
        assert!(
            warnings[0]
                .message
                .contains("frame_limiter_provider nvidia-control-panel is now auto")
        );
        let saved = std::fs::read_to_string(&path).unwrap();
        assert!(saved.contains("encoder = amf"), "{saved}");
        assert!(!saved.contains("nvenc"), "{saved}");
        // The next start reads the migrated file without a warning.
        let second = Fixture::load(directory);
        assert!(second.host.warnings.snapshot().is_empty());
        assert_eq!(second.host.config.read().unwrap().get("encoder", ""), "amf");
    }

    #[test]
    fn nvidia_settings_a_2_6_host_left_pending_are_reported_at_start() {
        use super::test_support::Fixture;
        let directory = Fixture::profile();
        std::fs::write(
            directory.join("frame-limiter-recovery.json"),
            r#"{"version":1,"rtss":null,"nvidia":[{"id":277041154,"before":{"effective":0,"override_value":null},"applied":60}]}"#,
        )
        .unwrap();
        let first = Fixture::load(directory.clone());
        let warnings = first.host.warnings.snapshot();
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert_eq!(warnings[0].code, "nvidia_limiter_retired");
        assert!(warnings[0].message.contains("NVIDIA Control Panel"));
        // Reported once.
        assert!(Fixture::load(directory).host.warnings.snapshot().is_empty());
    }

    #[test]
    fn a_rejoin_in_the_same_mode_reuses_the_clients_own_display_untouched() {
        use super::take_retained;
        use std::collections::BTreeMap;
        use std::sync::Arc;
        use std::time::Instant;
        let paused = Some(Instant::now());
        let mine = Arc::new((2560, 1440, 120_000, true));
        let theirs = Arc::new((2560, 1440, 120_000, true));
        let mut displays = BTreeMap::from([
            ("me".to_string(), (mine.clone(), paused)),
            ("other".to_string(), (theirs.clone(), None)),
        ]);
        let same = |lease: &Arc<(u32, u32, u32, bool)>| **lease == (2560, 1440, 120_000, true);

        // The same client in the same mode gets the very display it had, and
        // the retained entry stays as it was: nothing is created or released.
        let reused = take_retained(&mut displays, "me", same).unwrap();
        assert!(Arc::ptr_eq(&reused, &mine));
        assert_eq!(displays.len(), 2);
        assert!(Arc::ptr_eq(&displays["me"].0, &mine));
        assert_eq!(displays["me"].1, paused);
        drop(reused);
        // Rejoining again still finds it.
        assert!(Arc::ptr_eq(
            &take_retained(&mut displays, "me", same).unwrap(),
            &mine
        ));

        // A client with nothing retained never takes another client's display.
        assert_eq!(take_retained(&mut displays, "new", same), Err(None));
        assert!(Arc::ptr_eq(&displays["other"].0, &theirs));
        assert!(Arc::ptr_eq(&displays["me"].0, &mine));
    }
    #[test]
    fn a_rejoin_in_another_mode_releases_only_the_clients_own_display() {
        use super::take_retained;
        use std::collections::BTreeMap;
        use std::sync::Arc;
        let mine = Arc::new((2560, 1440, 120_000, true));
        let theirs = Arc::new((2560, 1440, 120_000, true));
        let mut displays = BTreeMap::from([
            ("me".to_string(), (mine.clone(), None)),
            ("other".to_string(), (theirs.clone(), None)),
        ]);
        let sdr = |lease: &Arc<(u32, u32, u32, bool)>| **lease == (2560, 1440, 120_000, false);
        let released = take_retained(&mut displays, "me", sdr)
            .unwrap_err()
            .unwrap();
        assert!(Arc::ptr_eq(&released, &mine));
        assert!(!displays.contains_key("me"));
        assert!(Arc::ptr_eq(&displays["other"].0, &theirs));
        assert_eq!(take_retained(&mut displays, "me", sdr), Err(None));
    }
    #[test]
    fn a_sign_in_set_while_the_host_runs_is_taken_up_and_not_written_back() {
        use super::test_support::Fixture;
        use butterpollo_core::state::{Credentials, load_json, write_json};
        let f = Fixture::new();
        let h = &f.host;
        // As in Apollo and Sunshine profiles, the sign-in shares the
        // paired-state file.
        assert_eq!(h.credentials_path, h.paired_path);
        let old = Credentials::new("apollo".into(), "old-password").unwrap();
        h.save_credentials(&old).unwrap();
        *h.credentials.write().unwrap() = Some(old);
        h.new_web_session(
            "apollo".into(),
            false,
            String::new(),
            "127.0.0.1".into(),
            None,
        )
        .unwrap();
        let tokens = |h: &Host| h.aliases.lock().unwrap()["root"]["api_tokens"].clone();
        {
            let mut aliases = h.aliases.lock().unwrap();
            aliases["root"]["api_tokens"] = serde_json::json!([{"hash": "made with old-password"}]);
            write_json(&h.aliases_path, &*aliases).unwrap();
        }
        // The console's own write is no change from outside.
        h.reload_credentials();
        assert_eq!(h.web_sessions.lock().unwrap().len(), 1);
        assert_eq!(tokens(h).as_array().map(Vec::len), Some(1));
        // --creds writes the file while the host runs.
        let set = |credentials: Option<&Credentials>| {
            let mut document = load_json(&h.credentials_path, serde_json::json!({})).unwrap();
            let document = document.as_object_mut().unwrap();
            for key in ["username", "password", "salt"] {
                document.remove(key);
            }
            if let Some(c) = credentials {
                document.extend(
                    serde_json::to_value(c)
                        .unwrap()
                        .as_object()
                        .unwrap()
                        .clone(),
                );
            }
            write_json(&h.credentials_path, &document).unwrap();
        };
        let new = Credentials::new("Ramazan K".into(), "new-password").unwrap();
        set(Some(&new));
        h.paired.write().unwrap().save(&h.paired_path).unwrap();
        h.reload_credentials();
        let current = h.credentials.read().unwrap().clone().unwrap();
        assert!(current.verifies("ramazan k", "new-password"));
        assert!(!current.verifies("apollo", "old-password"));
        assert!(h.web_sessions.lock().unwrap().is_empty());
        // So are the API tokens made while the old password was in force.
        assert_eq!(tokens(h), serde_json::json!([]));
        assert_eq!(
            load_json(&h.aliases_path, serde_json::json!({})).unwrap()["root"]["api_tokens"],
            serde_json::json!([])
        );
        // A later save of the paired state keeps the new sign-in.
        let paired = h.paired.read().unwrap();
        paired.save(&h.paired_path).unwrap();
        drop(paired);
        let saved = Credentials::load(&h.credentials_path).unwrap().unwrap();
        assert!(saved.verifies("Ramazan K", "new-password"));
        assert!(h.paired.read().unwrap().document["root"]["named_devices"].is_array());
        // A sign-in removed from the file offers the first-run setup.
        set(None);
        h.reload_credentials();
        assert!(h.credentials.read().unwrap().is_none());
        assert!(h.paired.read().unwrap().document.get("username").is_none());
    }
    #[test]
    fn host_load_reuses_the_shared_virtual_display_guid_without_replacing_an_existing_one() {
        use super::test_support::Fixture;
        let f = Fixture::new();
        let imported = "f773d31b-43da-470c-80d5-02e777a6d993";
        let existing = "15ff84bd-9db8-45ef-bfde-e60337744358";
        butterpollo_core::state::write_json(
            &f.host.aliases_path,
            &serde_json::json!({"root":{"shared_virtual_display_guid":imported}}),
        )
        .unwrap();
        let load =
            || super::Host::load(f.host.directory.clone(), f.host.assets.clone(), None).unwrap();
        let host = load();
        assert_eq!(
            host.paired.read().unwrap().document["root"]["shared_virtual_display_guid"],
            imported
        );
        let mut state = host.paired.write().unwrap();
        state.document["root"]["shared_virtual_display_guid"] = existing.into();
        state.save(&host.paired_path).unwrap();
        drop(state);
        assert_eq!(
            load().paired.read().unwrap().document["root"]["shared_virtual_display_guid"],
            existing
        );
    }
    use super::*;
    #[test]
    #[ignore = "requires SYSTEM in a console session with nobody signed in"]
    fn standard_codec_retries_do_not_wait_for_windows_sign_in() {
        assert!(butterpollo_windows::process::is_system());
        assert!(!butterpollo_windows::process::user_signed_in());
        let f = test_support::Fixture::new();
        let now = Instant::now() - CODEC_RETRY_MIN;
        let mut retry = CodecRetry::new(now);
        retry.completed(0, Some("GPU not ready at startup"), &f.host.warnings, now);
        let host = f.host.clone();
        let (send, receive) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            send.send(host.wait_for_codec_retry(&retry)).unwrap();
        });
        let result = receive.recv_timeout(Duration::from_secs(2));
        f.host
            .stop
            .store(true, std::sync::atomic::Ordering::Release);
        worker.join().unwrap();
        assert!(result.unwrap());
    }
    #[test]
    fn codecs_keep_retrying_with_capped_backoff_until_the_probe_recovers() {
        let mut now = Instant::now();
        let mut retry = CodecRetry::new(now);
        let warnings = Warnings::default();
        let mut attempts = 0;
        let mut probe = || {
            attempts += 1;
            if attempts <= 100 {
                (0, Some("AMF error 1"))
            } else {
                (0x101, None)
            }
        };
        for delay in [5, 10, 20, 40]
            .into_iter()
            .chain(std::iter::repeat_n(60, 96))
        {
            let (flags, error) = probe();
            assert!(retry.completed(flags, error, &warnings, now));
            assert_eq!(
                warnings.snapshot()[0].message,
                "No video encoder available: AMF error 1. Retrying."
            );
            now += Duration::from_secs(delay);
            assert!(!retry.due(now - Duration::from_millis(1), false));
            assert!(retry.due(now, false));
        }
        let (flags, error) = probe();
        assert!(!retry.completed(flags, error, &warnings, now));
        assert!(warnings.snapshot().is_empty());
        assert!(!retry.due(now + Duration::from_secs(600), true));
        assert_eq!(attempts, 101);
    }
    #[test]
    fn client_requests_bypass_backoff_but_never_the_minimum_interval() {
        let mut now = Instant::now();
        let mut retry = CodecRetry::new(now);
        let warnings = Warnings::default();
        let mut calls = 0;
        let mut probe = || {
            calls += 1;
            0
        };
        for _ in 0..10 {
            assert!(retry.completed(probe(), Some("driver unavailable"), &warnings, now));
            for milliseconds in [0, 1, 250, 4999] {
                assert!(!retry.due(now + Duration::from_millis(milliseconds), true));
            }
            now += CODEC_RETRY_MIN;
            assert!(retry.due(now, true));
        }
        assert_eq!(calls, 10);
        assert!(!retry.due(now, false));
        assert!(retry.due(now + Duration::from_secs(55), false));
    }
    #[test]
    fn codec_failure_reminders_are_limited_and_recovery_resets_the_delay() {
        let now = Instant::now();
        let mut retry = CodecRetry::new(now);
        assert!(retry.warn(now));
        for seconds in 1..60 {
            assert!(!retry.warn(now + Duration::from_secs(seconds)));
        }
        assert!(retry.warn(now + Duration::from_secs(60)));
        let warnings = Warnings::default();
        for _ in 0..4 {
            assert!(retry.completed(0, None, &warnings, now));
        }
        assert!(!retry.due(now + Duration::from_secs(39), false));
        assert!(!retry.completed(1, None, &warnings, now));
        assert!(retry.completed(0, Some("COM initialization failed"), &warnings, now));
        assert_eq!(
            warnings.snapshot()[0].message,
            "No video encoder available: COM initialization failed. Retrying."
        );
        assert!(!retry.due(now + Duration::from_secs(4), false));
        assert!(retry.due(now + Duration::from_secs(5), false));
    }
    #[test]
    fn codec_modifiers_do_not_count_as_video_and_optional_codecs_keep_standard_retries() {
        let now = Instant::now();
        let mut retry = CodecRetry::new(now);
        let warnings = Warnings::default();
        assert!(retry.completed(0x40000000, None, &warnings, now));
        assert_eq!(
            warnings.snapshot()[0].message,
            "No video encoder available: the encoder produced no packets. Retrying."
        );
        assert!(retry.completed(0x800000, None, &warnings, now));
        assert!(warnings.snapshot().is_empty());
        assert!(retry.completed(0, Some("AMF error 1"), &warnings, now));
        assert_eq!(warnings.snapshot().len(), 1);
        assert!(!retry.completed(0x10000, None, &warnings, now));
        assert!(warnings.snapshot().is_empty());
    }
    #[test]
    fn an_invalid_library_starts_empty_and_is_kept() {
        let dir = std::env::temp_dir().join(format!("butterpollo-apps-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("apps.json");
        let default = json!({"apps":[{"name":"Desktop"}]});
        assert_eq!(load_library(&path, default.clone()).unwrap(), default);
        std::fs::write(&path, b"{\"apps\":[{\"name\":\"Game\",}]}").unwrap();
        assert_eq!(
            load_library(&path, default.clone()).unwrap()["apps"],
            json!([])
        );
        assert_eq!(
            std::fs::read(dir.join("apps.json.invalid")).unwrap(),
            b"{\"apps\":[{\"name\":\"Game\",}]}"
        );
        std::fs::write(&path, b"{\"apps\":[{\"name\":\"Game\"}]}").unwrap();
        assert_eq!(
            load_library(&path, default.clone()).unwrap()["apps"][0]["name"],
            "Game"
        );
        // Notepad's byte order mark, and a blank file Vibepollo reads as missing.
        std::fs::write(&path, b"\xef\xbb\xbf{\"apps\":[{\"name\":\"Noted\"}]}").unwrap();
        assert_eq!(
            load_library(&path, default.clone()).unwrap()["apps"][0]["name"],
            "Noted"
        );
        std::fs::write(&path, b"\r\n").unwrap();
        assert_eq!(load_library(&path, default.clone()).unwrap(), default);
        std::fs::remove_dir_all(&dir).ok();
    }
}
