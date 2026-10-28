//! Nucleus Co-op on this host: whether it can run here, its install, the
//! handlers and games it offers as co-op library entries, and the one
//! session Nucleus runs at a time.
//!
//! A launch of a co-op entry is admitted here ([`Registry::admit`]) and
//! carries its [`Seat`] (`Launch::nucleus`). Its stream reaches the
//! session's [`Instance`] through [`Registry::instance`] and gives the slot
//! back with [`Registry::leave`]; the launch id that holds a slot makes a
//! stale leave do nothing. [`Pc`] and [`Instance`] are the seams to
//! `butterpollo_windows::nucleus`; the tests use the fakes in `fake`.
use anyhow::{Context, Result, anyhow, ensure};
use butterpollo_core::config::Config;
use butterpollo_core::input::Input;
use butterpollo_core::input_policy::Policy;
use butterpollo_core::nucleus::coop::{self, Admit, End, Kind, Refused};
use butterpollo_core::nucleus::handler::{Catalog, Handler, Missing};
use butterpollo_core::session::{Seat, Warnings};
use butterpollo_core::state::{App, Client};
use butterpollo_windows::input::PadReport;
use butterpollo_windows::nucleus::pads::{self, Dropped};
use butterpollo_windows::nucleus::session::{Running, Start};
use butterpollo_windows::nucleus::{files, install};
use butterpollo_windows::process::AudioProcess;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard};
use std::time::{Duration, Instant};

/// A co-op library entry's name: this, then the app's name.
pub const ENTRY_PREFIX: &str = "Co-op (Nucleus): ";
/// How long one read of the handlers and games serves the library:
/// Moonlight asks for `/applist` every few seconds.
const CATALOG_TTL: Duration = Duration::from_secs(10);
/// How long an ended session stays listed once no stream holds it, so the
/// console and the devices can show why it ended.
const LINGER: Duration = Duration::from_secs(60);

/// Why a co-op launch is refused, and the status its answer carries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refusal {
    pub status: u16,
    pub message: String,
}
impl Refusal {
    /// Nucleus Co-op can't run here or now: not available on this host, not
    /// installable, or it did not start.
    pub fn unavailable(message: impl Into<String>) -> Self {
        Self {
            status: 503,
            message: message.into(),
        }
    }
    /// No handler or added game for the app.
    pub fn missing(missing: &Missing) -> Self {
        Self {
            status: 404,
            message: missing.to_string(),
        }
    }
    /// Another session holds Nucleus, or the session refused this device.
    pub fn busy(message: impl Into<String>) -> Self {
        Self {
            status: 409,
            message: message.into(),
        }
    }
}
impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for Refusal {}

/// The PC side: `butterpollo_windows::nucleus` on this PC, a fake in tests.
pub trait Pc: Send + Sync {
    /// Why this host cannot run Nucleus Co-op sessions, or None.
    fn unavailable(&self) -> Option<String>;
    /// The Nucleus folder once it is there; `configured` is `nucleus_path`.
    fn installed(&self, configured: &str) -> Result<Option<PathBuf>>;
    /// The Nucleus folder, installing the pinned release first.
    fn ensure(&self, configured: &str) -> Result<PathBuf>;
    /// The handlers and the signed-in user's games of the Nucleus at `folder`.
    fn catalog(&self, folder: &Path) -> Result<Catalog>;
    /// The XInput slots the PC's own controllers leave free.
    fn free_pads(&self) -> Result<usize>;
    fn start(
        &self,
        start: &Start,
        policy: Policy,
        warnings: Arc<Warnings>,
    ) -> Result<Box<dyn Instance>>;
}

/// A running session: `butterpollo_windows::nucleus::session::Running`.
#[allow(
    dead_code,
    reason = "the co-op stream calls the media methods once it streams a slot"
)]
pub trait Instance: Send + Sync {
    /// Read or change the session's players and phase.
    fn with_session(&self, f: &mut dyn FnMut(&mut coop::Session));
    /// Slot `slot`'s capture target on the GPU of `display`.
    fn capture_target(&self, slot: usize, display: &str) -> Result<String>;
    /// Park slot `slot`'s window at its player's stream size.
    fn resize(&self, slot: usize, size: (u32, u32)) -> Result<()>;
    /// A device's input, for its slot's pad.
    fn input(&self, device: &str, event: Input) -> Result<(), Dropped>;
    /// Feedback for each slot's player since the last call.
    fn feedback(&self) -> Vec<(usize, PadReport)>;
    /// Slot `slot`'s game instance, for its audio.
    fn audio(&self, slot: usize) -> Result<AudioProcess>;
    /// End Nucleus and every instance and wait for them; later calls do
    /// nothing.
    fn stop(&self);
}
impl dyn Instance {
    /// [`Instance::with_session`] with a result.
    pub fn session<R>(&self, f: impl FnOnce(&mut coop::Session) -> R) -> R {
        let mut f = Some(f);
        let mut out = None;
        self.with_session(&mut |session| out = f.take().map(|f| f(session)));
        out.expect("with_session runs its closure")
    }
}

/// This PC.
struct OnPc;
impl Pc for OnPc {
    fn unavailable(&self) -> Option<String> {
        if !butterpollo_windows::process::is_system() {
            return Some("Nucleus Co-op sessions need the host to run as the Rubylight Windows service, which starts Nucleus Co-op as the signed-in user".into());
        }
        butterpollo_core::os_gate::missing(
            butterpollo_windows::display::windows_build(),
            butterpollo_core::os_gate::Feature::WgcWindow,
        )
    }
    fn installed(&self, configured: &str) -> Result<Option<PathBuf>> {
        install::installed(&butterpollo_core::paths::data_root(), configured)
    }
    fn ensure(&self, configured: &str) -> Result<PathBuf> {
        let mut download = Download(
            tokio::runtime::Handle::try_current()
                .context("downloading Nucleus Co-op needs the host's runtime")?,
        );
        install::ensure(
            &butterpollo_core::paths::data_root(),
            configured,
            &mut download,
        )
    }
    fn catalog(&self, folder: &Path) -> Result<Catalog> {
        let appdata = butterpollo_windows::user_folder::roaming_app_data().map_err(|error| {
            anyhow!("finding the signed-in user's AppData folder, which holds Nucleus Co-op's game list: {error}")
        })?;
        files::catalog(folder, Some(&appdata))
    }
    fn free_pads(&self) -> Result<usize> {
        pads::free(&mut pads::System::open()?)
    }
    fn start(
        &self,
        start: &Start,
        policy: Policy,
        warnings: Arc<Warnings>,
    ) -> Result<Box<dyn Instance>> {
        let running = butterpollo_windows::nucleus::session::start_on_pc(start, policy, warnings)?;
        Ok(Box::new(OnPcInstance(RwLock::new(running))))
    }
}

/// A session on this PC. Its calls share it; [`Instance::stop`] waits for them.
struct OnPcInstance(RwLock<Running>);
impl OnPcInstance {
    fn running(&self) -> RwLockReadGuard<'_, Running> {
        self.0.read().unwrap_or_else(PoisonError::into_inner)
    }
}
impl Instance for OnPcInstance {
    fn with_session(&self, f: &mut dyn FnMut(&mut coop::Session)) {
        self.running().with_session(f);
    }
    fn capture_target(&self, slot: usize, display: &str) -> Result<String> {
        self.running().capture_target(slot, display)
    }
    fn resize(&self, slot: usize, size: (u32, u32)) -> Result<()> {
        self.running().resize(slot, size)
    }
    fn input(&self, device: &str, event: Input) -> Result<(), Dropped> {
        self.running().input(device, event)
    }
    fn feedback(&self) -> Vec<(usize, PadReport)> {
        self.running().feedback()
    }
    fn audio(&self, slot: usize) -> Result<AudioProcess> {
        self.running().audio(slot)
    }
    fn stop(&self) {
        self.0
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .stop();
    }
}

/// Downloads with the updater's HTTPS client, which follows redirects to
/// GitHub's asset hosts only. Blocks, so never on the async runtime.
struct Download(tokio::runtime::Handle);
impl install::Fetch for Download {
    fn fetch(&mut self, url: &str, limit: u64) -> Result<Vec<u8>> {
        self.0.block_on(async {
            let mut response = crate::updater::client(Duration::from_secs(600))?
                .get(url)
                .send()
                .await?
                .error_for_status()?;
            let mut body = Vec::new();
            while let Some(part) = response.chunk().await? {
                chunk(&mut body, &part, limit)?;
            }
            Ok(body)
        })
    }
}

/// Add `part` to `body`, refusing a body longer than `limit` bytes.
fn chunk(body: &mut Vec<u8>, part: &[u8], limit: u64) -> Result<()> {
    ensure!(
        body.len() as u64 + part.len() as u64 <= limit,
        "the download is larger than its {limit} bytes"
    );
    body.extend_from_slice(part);
    Ok(())
}

/// `nucleus_path` as the installer reads it.
pub fn configured(config: &Config) -> String {
    config
        .nucleus_path()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The program an app starts: its command's, else its first detached
/// command's.
fn executable(app: &App) -> Option<&str> {
    let detached = app
        .extra
        .get("detached")
        .and_then(Value::as_array)
        .and_then(|commands| commands.first())
        .and_then(Value::as_str);
    [Some(app.cmd.as_str()), detached]
        .into_iter()
        .flatten()
        .map(butterpollo_windows::process::command_target)
        .find(|target| !target.is_empty())
}

/// A co-op library entry: an app whose program Nucleus Co-op has a handler
/// and an added game for.
#[derive(Clone, Debug)]
pub struct CoopApp {
    /// The entry the library lists: [`ENTRY_PREFIX`] and the app's name,
    /// with the id of that name. It runs nothing itself.
    pub entry: App,
    /// The app whose game it starts.
    pub app: App,
    pub handler: Handler,
}
impl CoopApp {
    pub fn id(&self) -> u32 {
        self.entry.id()
    }
}

/// The co-op entries of `apps`, in their order. An entry whose id an app
/// or an earlier entry already has is left out.
fn entries_of(catalog: &Catalog, apps: &[App]) -> Vec<CoopApp> {
    let mut taken: BTreeSet<u32> = apps
        .iter()
        .flat_map(|app| std::iter::once(app.id()).chain(app.aliases.iter().copied()))
        .collect();
    apps.iter()
        .filter_map(|app| {
            let found = catalog.find(&app.name, executable(app)).ok()?;
            let entry = App {
                name: format!("{ENTRY_PREFIX}{}", app.name),
                cmd: String::new(),
                working_dir: String::new(),
                prep: Vec::new(),
                computed_id: None,
                aliases: Vec::new(),
                extra: BTreeMap::new(),
            };
            taken.insert(entry.id()).then(|| CoopApp {
                entry,
                app: app.clone(),
                handler: found.handler,
            })
        })
        .collect()
}

/// Where the host's copy of Nucleus Co-op stands, for the console.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Install {
    Missing,
    Installed(PathBuf),
    /// `nucleus_path`, the user's own copy.
    Configured(PathBuf),
    Installing,
    Failed(String),
}

/// A co-op launch asking for a slot.
pub struct Admission<'a> {
    /// The launch's id, which holds the slot until its stream ends.
    pub launch: &'a str,
    pub client: &'a Client,
    pub coop: &'a CoopApp,
    /// The stream's size, at which a new session parks the first window.
    pub size: (u32, u32),
    pub config: &'a Config,
    pub warnings: Arc<Warnings>,
}

/// Why a device's join from its session list failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JoinError {
    /// No session has that id.
    NoSession,
    Refused(Refused),
}

/// A session as the lists show it.
#[derive(Clone, Debug)]
pub struct Listed {
    /// The co-op entry it streams.
    pub app_id: u32,
    pub session: coop::Session,
    /// Whether a launch streams each slot.
    pub held: Vec<bool>,
}

struct Entry {
    instance: Arc<dyn Instance>,
    app_id: u32,
    /// The launch streaming each slot; a leave from any other launch is stale.
    holders: Vec<Option<String>>,
    /// When the registry first saw the session ended.
    ended: Option<Instant>,
}

#[derive(Default)]
struct State {
    sessions: BTreeMap<u32, Entry>,
    next: u32,
    installing: bool,
    install_error: Option<String>,
}

struct Cached {
    at: Instant,
    folder: PathBuf,
    catalog: Result<Arc<Catalog>, String>,
}

/// The host's Nucleus Co-op sessions.
pub struct Registry {
    pc: RwLock<Arc<dyn Pc>>,
    /// One co-op launch is admitted at a time, so two first players of one
    /// game start one session.
    admit: Mutex<()>,
    state: Mutex<State>,
    catalog: Mutex<Option<Cached>>,
}

impl Registry {
    pub fn new(pc: Arc<dyn Pc>) -> Self {
        Self {
            pc: RwLock::new(pc),
            admit: Mutex::new(()),
            state: Mutex::default(),
            catalog: Mutex::new(None),
        }
    }
    /// This PC; in tests a PC without Nucleus Co-op, so no test reads the
    /// real one.
    pub fn on_pc() -> Self {
        #[cfg(not(test))]
        let pc: Arc<dyn Pc> = Arc::new(OnPc);
        #[cfg(test)]
        let pc: Arc<dyn Pc> = Arc::new(fake::Pc::unavailable());
        Self::new(pc)
    }
    #[cfg(test)]
    pub fn use_pc(&self, pc: Arc<dyn Pc>) {
        *self.pc.write().unwrap() = pc;
        *self.catalog.lock().unwrap() = None;
    }
    fn pc(&self) -> Arc<dyn Pc> {
        self.pc
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Why this host cannot run Nucleus Co-op sessions, or None.
    pub fn unavailable(&self) -> Option<String> {
        self.pc().unavailable()
    }

    /// The co-op entries of `apps`: none while Nucleus Co-op is unavailable
    /// or not installed.
    pub fn entries(&self, apps: &[App], configured: &str) -> Vec<CoopApp> {
        let pc = self.pc();
        if pc.unavailable().is_some() {
            return Vec::new();
        }
        let folder = match pc.installed(configured) {
            Ok(Some(folder)) => folder,
            Ok(None) => return Vec::new(),
            Err(error) => {
                tracing::debug!(error = %format!("{error:#}"), "Nucleus Co-op: no co-op entries");
                return Vec::new();
            }
        };
        match self.catalog(&folder, false) {
            Ok(catalog) => entries_of(&catalog, apps),
            Err(_) => Vec::new(),
        }
    }

    /// The co-op entry with id `id`.
    pub fn resolve(&self, apps: &[App], configured: &str, id: u32) -> Option<CoopApp> {
        self.entries(apps, configured)
            .into_iter()
            .find(|coop| coop.id() == id)
    }

    /// The handlers and games of `folder`, read again when `fresh` or once
    /// the last read is older than [`CATALOG_TTL`]. A failure is logged when
    /// it is new.
    fn catalog(&self, folder: &Path, fresh: bool) -> Result<Arc<Catalog>, String> {
        let mut cached = self.catalog.lock().unwrap_or_else(PoisonError::into_inner);
        if !fresh
            && let Some(cached) = cached
                .as_ref()
                .filter(|c| c.folder == folder && c.at.elapsed() < CATALOG_TTL)
        {
            return cached.catalog.clone();
        }
        let catalog = self
            .pc()
            .catalog(folder)
            .map(Arc::new)
            .map_err(|error| format!("{error:#}"));
        if let Err(error) = &catalog
            && cached
                .as_ref()
                .is_none_or(|c| c.catalog.as_ref().err() != Some(error))
        {
            tracing::warn!(folder = %folder.display(), %error, "Nucleus Co-op: the handlers and games did not load");
        }
        *cached = Some(Cached {
            at: Instant::now(),
            folder: folder.into(),
            catalog: catalog.clone(),
        });
        catalog
    }

    /// Where the host's copy of Nucleus Co-op stands.
    pub fn install(&self, configured: &str) -> Install {
        let installed = self.pc().installed(configured);
        let state = self.lock();
        match installed {
            Ok(Some(folder)) if !configured.trim().is_empty() => Install::Configured(folder),
            Ok(Some(folder)) => Install::Installed(folder),
            _ if state.installing => Install::Installing,
            Err(error) => Install::Failed(format!("{error:#}")),
            Ok(None) => state
                .install_error
                .clone()
                .map_or(Install::Missing, Install::Failed),
        }
    }

    /// The Nucleus folder, installing it first; a failure is kept for the console.
    fn ensure(&self, pc: &dyn Pc, configured: &str) -> Result<PathBuf, Refusal> {
        if let Ok(Some(folder)) = pc.installed(configured) {
            return Ok(folder);
        }
        self.lock().installing = true;
        let result = pc.ensure(configured);
        let mut state = self.lock();
        state.installing = false;
        match result {
            Ok(folder) => {
                state.install_error = None;
                Ok(folder)
            }
            Err(error) => {
                let message = format!("Nucleus Co-op is not ready: {error:#}");
                state.install_error = Some(message.clone());
                Err(Refusal::unavailable(message))
            }
        }
    }

    /// Admit a co-op launch: start a session of its game, or seat the device
    /// in the running one. The launch then holds its slot until
    /// [`leave`](Self::leave).
    pub fn admit(&self, request: Admission) -> Result<Seat, Refusal> {
        let pc = self.pc();
        if let Some(why) = pc.unavailable() {
            return Err(Refusal::unavailable(why));
        }
        let _one = self.admit.lock().unwrap_or_else(PoisonError::into_inner);
        let folder = self.ensure(pc.as_ref(), &configured(request.config))?;
        let catalog = self.catalog(&folder, true).map_err(|error| {
            Refusal::unavailable(format!(
                "Nucleus Co-op's handlers and games did not load: {error}"
            ))
        })?;
        let app = &request.coop.app;
        let found = catalog
            .find(&app.name, executable(app))
            .map_err(|missing| Refusal::missing(&missing))?;
        stop(self.reap());
        let client = request.client;
        let decision = {
            let state = self.lock();
            let current = state
                .sessions
                .values()
                .find_map(|entry| entry.instance.session(|s| (!s.ended()).then(|| s.clone())));
            coop::admit(current.as_ref(), &found.handler.guid, Kind::Coop)
        };
        match decision {
            Admit::Busy(text) => Err(Refusal::busy(text)),
            Admit::Join(id) => {
                let mut state = self.lock();
                let entry = state.sessions.get_mut(&id).ok_or_else(|| {
                    Refusal::busy(format!("The {} co-op session ended: launch it again", app.name))
                })?;
                let slot = entry
                    .instance
                    .session(|s| s.join(&client.uuid, &client.name, client.allows_coop_join()))
                    .map_err(|refused| Refusal::busy(refused.to_string()))?
                    .slot();
                let holder = entry
                    .holders
                    .get_mut(slot)
                    .ok_or_else(|| Refusal::unavailable(format!("the session has no slot {slot}")))?;
                *holder = Some(request.launch.into());
                tracing::info!(session = id, slot, client = %client.name, title = %app.name, "Nucleus Co-op: a player joined");
                Ok(Seat { session: id, slot })
            }
            Admit::Start => {
                let free = pc.free_pads().map_err(|error| {
                    Refusal::unavailable(format!(
                        "Nucleus Co-op could not count the free controller slots: {error:#}"
                    ))
                })?;
                let players = coop::players(
                    found.handler.max_players,
                    request.config.nucleus_coop_players(),
                    free,
                )
                .map_err(Refusal::unavailable)?;
                let policy = Policy::resolve(request.config)
                    .map_err(|error| Refusal::unavailable(format!("{error:#}")))?;
                let id = {
                    let mut state = self.lock();
                    state.next += 1;
                    state.next
                };
                let start = Start {
                    id,
                    kind: Kind::Coop,
                    folder: &folder,
                    handler: &found.handler,
                    title: &app.name,
                    players,
                    owner: (&client.uuid, &client.name),
                    size: request.size,
                };
                let instance: Arc<dyn Instance> = pc
                    .start(&start, policy, request.warnings)
                    .map_err(|error| {
                        Refusal::unavailable(format!(
                            "Nucleus Co-op did not start {}: {error:#}",
                            app.name
                        ))
                    })?
                    .into();
                let mut holders = vec![None; players];
                holders[0] = Some(request.launch.into());
                self.lock().sessions.insert(
                    id,
                    Entry {
                        instance,
                        app_id: request.coop.id(),
                        holders,
                        ended: None,
                    },
                );
                tracing::info!(session = id, players, game = %found.handler.guid, client = %client.name, title = %app.name, "Nucleus Co-op: co-op session started");
                Ok(Seat {
                    session: id,
                    slot: 0,
                })
            }
        }
    }

    /// The session's instance while the registry has it.
    pub fn instance(&self, session: u32) -> Option<Arc<dyn Instance>> {
        self.lock()
            .sessions
            .get(&session)
            .map(|entry| entry.instance.clone())
    }

    /// The stream of `launch` ended or its pending launch expired: the device
    /// keeps its slot for [`coop::GRACE`]. A launch that no longer holds the
    /// slot leaves nothing.
    pub fn leave(&self, seat: Seat, launch: &str) {
        {
            let mut state = self.lock();
            if let Some(entry) = state.sessions.get_mut(&seat.session)
                && let Some(holder) = entry.holders.get_mut(seat.slot)
                && holder.as_deref() == Some(launch)
            {
                *holder = None;
                entry.instance.session(|s| {
                    let device = s
                        .slots
                        .get(seat.slot)
                        .and_then(|slot| slot.seat.as_ref())
                        .map(|seat| seat.device.clone());
                    if let Some(device) = device {
                        s.leave(&device, Instant::now());
                    }
                });
                tracing::info!(
                    session = seat.session,
                    slot = seat.slot,
                    "Nucleus Co-op: a player's stream ended"
                );
            }
        }
        stop(self.reap());
    }

    /// Seat `client` in `session`, or return it to its own slot, from its
    /// session list. The slot waits [`coop::GRACE`] for the device to launch
    /// the entry, unless the device streams it now. Returns the slot and the
    /// entry's app id.
    pub fn join(&self, session: u32, client: &Client) -> Result<(usize, u32), JoinError> {
        stop(self.reap());
        let state = self.lock();
        let entry = state.sessions.get(&session).ok_or(JoinError::NoSession)?;
        let slot = entry
            .instance
            .session(|s| {
                let slot = s
                    .join(&client.uuid, &client.name, client.allows_coop_join())?
                    .slot();
                if entry.holders.get(slot).is_none_or(Option::is_none) {
                    s.leave(&client.uuid, Instant::now());
                }
                Ok(slot)
            })
            .map_err(JoinError::Refused)?;
        tracing::info!(session, slot, client = %client.name, "Nucleus Co-op: a device took a slot from its session list");
        Ok((slot, entry.app_id))
    }

    /// Remove the player of `slot`; they cannot rejoin this session.
    /// Returns the device.
    pub fn kick(&self, session: u32, slot: usize) -> Option<String> {
        let mut state = self.lock();
        let entry = state.sessions.get_mut(&session)?;
        let device = entry.instance.session(|s| s.kick(slot))?;
        if let Some(holder) = entry.holders.get_mut(slot) {
            *holder = None;
        }
        tracing::info!(session, slot, device, "Nucleus Co-op: player removed in the web console");
        Some(device)
    }

    /// End `session`: Nucleus and every instance stop. False when there is
    /// no such session.
    pub fn end(&self, session: u32) -> bool {
        let instance = {
            let state = self.lock();
            let Some(entry) = state.sessions.get(&session) else {
                return false;
            };
            entry.instance.session(|s| s.end(End::Ended));
            entry.instance.clone()
        };
        tracing::info!(session, "Nucleus Co-op: session ended in the web console");
        instance.stop();
        stop(self.reap());
        true
    }

    /// Every session the registry has, ended ones until they are reaped.
    pub fn sessions(&self) -> Vec<Listed> {
        stop(self.reap());
        self.lock()
            .sessions
            .values()
            .map(|entry| Listed {
                app_id: entry.app_id,
                session: entry.instance.session(|s| s.clone()),
                held: entry.holders.iter().map(Option::is_some).collect(),
            })
            .collect()
    }

    /// The instances of sessions newly seen ended, for the caller to stop
    /// outside the lock; drops sessions ended for [`LINGER`] that no stream
    /// holds.
    fn reap(&self) -> Vec<Arc<dyn Instance>> {
        let now = Instant::now();
        let mut stopping = Vec::new();
        self.lock().sessions.retain(|_, entry| {
            if !entry.instance.session(|s| s.ended()) {
                return true;
            }
            let since = *entry.ended.get_or_insert_with(|| {
                stopping.push(entry.instance.clone());
                now
            });
            entry.holders.iter().any(Option::is_some) || now.duration_since(since) < LINGER
        });
        stopping
    }
}

fn stop(instances: Vec<Arc<dyn Instance>>) {
    for instance in instances {
        instance.stop();
    }
}

#[cfg(test)]
pub(crate) mod fake {
    //! A PC with Nucleus Co-op and one handler, and sessions that only
    //! keep their state.
    use super::*;
    use butterpollo_core::nucleus::handler::Game;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    pub(crate) const GUID: &str = "Portal 2";
    pub(crate) const EXE: &str = r"C:\Games\Portal 2\portal2.exe";

    pub(crate) struct Pc {
        pub unavailable: Mutex<Option<String>>,
        pub installed: Mutex<Option<PathBuf>>,
        /// What an install fails with.
        pub install_error: Mutex<Option<String>>,
        pub catalog: Mutex<Catalog>,
        pub free: Mutex<usize>,
        pub start_error: Mutex<Option<String>>,
        pub started: Mutex<Vec<Arc<Instance>>>,
        pub installs: AtomicUsize,
        pub reads: AtomicUsize,
    }
    impl Pc {
        pub(crate) fn unavailable() -> Self {
            let pc = Self::installed();
            *pc.unavailable.lock().unwrap() = Some("not on this test PC".into());
            pc
        }
        /// Installed, with a two-player handler for portal2.exe and the
        /// game added.
        pub(crate) fn installed() -> Self {
            Self {
                unavailable: Mutex::new(None),
                installed: Mutex::new(Some(PathBuf::from(r"C:\Nucleus"))),
                install_error: Mutex::new(None),
                catalog: Mutex::new(Catalog {
                    handlers: vec![Handler {
                        file: "Portal 2.js".into(),
                        guid: GUID.into(),
                        name: "Portal 2".into(),
                        executable: "portal2.exe".into(),
                        max_players: 2,
                        supports_keyboard: false,
                        hides_taskbar: false,
                    }],
                    games: vec![Game {
                        guid: GUID.into(),
                        exe_path: EXE.into(),
                    }],
                }),
                free: Mutex::new(4),
                start_error: Mutex::new(None),
                started: Mutex::new(Vec::new()),
                installs: AtomicUsize::new(0),
                reads: AtomicUsize::new(0),
            }
        }
        /// The session started last.
        pub(crate) fn last(&self) -> Arc<Instance> {
            self.started.lock().unwrap().last().unwrap().clone()
        }
    }
    impl super::Pc for Pc {
        fn unavailable(&self) -> Option<String> {
            self.unavailable.lock().unwrap().clone()
        }
        fn installed(&self, _configured: &str) -> Result<Option<PathBuf>> {
            Ok(self.installed.lock().unwrap().clone())
        }
        fn ensure(&self, configured: &str) -> Result<PathBuf> {
            if let Some(folder) = self.installed(configured)? {
                return Ok(folder);
            }
            self.installs.fetch_add(1, Ordering::Relaxed);
            if let Some(error) = self.install_error.lock().unwrap().clone() {
                anyhow::bail!(error);
            }
            let folder = PathBuf::from(r"C:\Nucleus");
            *self.installed.lock().unwrap() = Some(folder.clone());
            Ok(folder)
        }
        fn catalog(&self, _folder: &Path) -> Result<Catalog> {
            self.reads.fetch_add(1, Ordering::Relaxed);
            Ok(self.catalog.lock().unwrap().clone())
        }
        fn free_pads(&self) -> Result<usize> {
            Ok(*self.free.lock().unwrap())
        }
        fn start(
            &self,
            start: &Start,
            _policy: Policy,
            _warnings: Arc<Warnings>,
        ) -> Result<Box<dyn super::Instance>> {
            if let Some(error) = self.start_error.lock().unwrap().clone() {
                anyhow::bail!(error);
            }
            let pads: Vec<u8> = (0..start.players as u8).collect();
            let session = coop::Session::new(
                start.id,
                start.kind,
                &start.handler.guid,
                start.title,
                &pads,
                start.owner,
            )
            .map_err(anyhow::Error::msg)?;
            let instance = Arc::new(Instance {
                session: Mutex::new(session),
                stopped: AtomicBool::new(false),
                sizes: Mutex::new(vec![start.size; start.players]),
            });
            self.started.lock().unwrap().push(instance.clone());
            Ok(Box::new(instance))
        }
    }

    pub(crate) struct Instance {
        pub session: Mutex<coop::Session>,
        pub stopped: AtomicBool,
        pub sizes: Mutex<Vec<(u32, u32)>>,
    }
    impl Instance {
        pub(crate) fn stopped(&self) -> bool {
            self.stopped.load(Ordering::Relaxed)
        }
    }
    impl super::Instance for Arc<Instance> {
        fn with_session(&self, f: &mut dyn FnMut(&mut coop::Session)) {
            f(&mut self.session.lock().unwrap());
        }
        fn capture_target(&self, slot: usize, display: &str) -> Result<String> {
            Ok(format!("window:{slot}@{display}"))
        }
        fn resize(&self, slot: usize, size: (u32, u32)) -> Result<()> {
            *self
                .sizes
                .lock()
                .unwrap()
                .get_mut(slot)
                .context("no such slot")? = size;
            Ok(())
        }
        fn input(&self, device: &str, _event: Input) -> Result<(), Dropped> {
            match self.session.lock().unwrap().route(device) {
                Some(_) => Ok(()),
                None => Err(Dropped::Ignored),
            }
        }
        fn feedback(&self) -> Vec<(usize, PadReport)> {
            Vec::new()
        }
        fn audio(&self, slot: usize) -> Result<AudioProcess> {
            anyhow::bail!("the fake session has no audio for slot {slot}")
        }
        fn stop(&self) {
            self.stopped.store(true, Ordering::Relaxed);
            self.session.lock().unwrap().end(End::Ended);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use butterpollo_core::nucleus::coop::{Phase, Refused};

    fn app(name: &str, cmd: &str) -> App {
        App {
            name: name.into(),
            cmd: cmd.into(),
            working_dir: String::new(),
            prep: Vec::new(),
            computed_id: None,
            aliases: Vec::new(),
            extra: BTreeMap::new(),
        }
    }
    fn client(uuid: &str, coop_join: bool) -> Client {
        let mut client: Client = serde_json::from_value(serde_json::json!({
            "name": format!("Device {uuid}"), "cert": "", "uuid": uuid,
        }))
        .unwrap();
        if coop_join {
            client.extra.insert("coop_join".into(), true.into());
        }
        client
    }
    fn setup() -> (Registry, Arc<fake::Pc>, Vec<App>) {
        let pc = Arc::new(fake::Pc::installed());
        let apps = vec![
            app("Desktop", ""),
            app("Portal 2", &format!("\"{}\" -novid", fake::EXE)),
            app("Steam", "steam://open/bigpicture"),
        ];
        (Registry::new(pc.clone()), pc, apps)
    }
    fn admit(
        registry: &Registry,
        coop: &CoopApp,
        launch: &str,
        client: &Client,
    ) -> Result<Seat, Refusal> {
        registry.admit(Admission {
            launch,
            client,
            coop,
            size: (1920, 1080),
            config: &Config::default(),
            warnings: Default::default(),
        })
    }

    #[test]
    fn a_download_stops_past_its_size() {
        let mut body = Vec::new();
        chunk(&mut body, &[1; 6], 10).unwrap();
        chunk(&mut body, &[2; 4], 10).unwrap();
        assert_eq!(body.len(), 10);
        assert!(chunk(&mut body, &[3], 10).is_err());
        assert_eq!(body.len(), 10);
    }

    #[test]
    fn entries_list_apps_with_a_handler_and_an_added_game_only() {
        let (registry, pc, apps) = setup();
        let entries = registry.entries(&apps, "");
        assert_eq!(entries.len(), 1);
        let coop = &entries[0];
        assert_eq!(coop.entry.name, "Co-op (Nucleus): Portal 2");
        assert_eq!(coop.app.name, "Portal 2");
        assert_eq!(coop.id(), app("Co-op (Nucleus): Portal 2", "").id());
        assert!(coop.entry.cmd.is_empty());
        assert_eq!(
            registry.resolve(&apps, "", coop.id()).unwrap().app.name,
            "Portal 2"
        );
        assert!(registry.resolve(&apps, "", apps[1].id()).is_none());
        // A detached command names the program too.
        let mut detached = app("Portal 2 detached", "");
        detached
            .extra
            .insert("detached".into(), serde_json::json!([fake::EXE]));
        assert_eq!(registry.entries(&[detached], "").len(), 1);
        // A name an app already has is not listed twice.
        let taken = app("Co-op (Nucleus): Portal 2", "");
        assert!(entries_of(&pc.catalog.lock().unwrap(), &[apps[1].clone(), taken]).is_empty());
        // Not added in Nucleus: no entry.
        pc.catalog.lock().unwrap().games.clear();
        registry.use_pc(pc.clone());
        assert!(registry.entries(&apps, "").is_empty());
    }

    #[test]
    fn no_entries_without_nucleus_or_where_it_cannot_run() {
        let (registry, pc, apps) = setup();
        *pc.installed.lock().unwrap() = None;
        assert!(registry.entries(&apps, "").is_empty());
        assert_eq!(registry.install(""), Install::Missing);
        let unavailable = Arc::new(fake::Pc::unavailable());
        registry.use_pc(unavailable);
        assert!(registry.entries(&apps, "").is_empty());
        assert_eq!(registry.unavailable().as_deref(), Some("not on this test PC"));
    }

    #[test]
    fn the_catalog_is_read_once_for_the_library_and_again_for_a_launch() {
        let (registry, pc, apps) = setup();
        let coop = registry.entries(&apps, "").remove(0);
        registry.entries(&apps, "");
        assert_eq!(pc.reads.load(std::sync::atomic::Ordering::Relaxed), 1);
        admit(&registry, &coop, "a", &client("A", false)).unwrap();
        assert_eq!(pc.reads.load(std::sync::atomic::Ordering::Relaxed), 2);
    }

    #[test]
    fn launches_start_a_session_and_join_it_by_permission_and_free_slot() {
        let (registry, pc, apps) = setup();
        let coop = registry.entries(&apps, "").remove(0);
        let owner = client("A", false);
        assert_eq!(
            admit(&registry, &coop, "a", &owner).unwrap(),
            Seat {
                session: 1,
                slot: 0
            }
        );
        let session = pc.last();
        assert_eq!(session.session.lock().unwrap().slots.len(), 2);
        // Without Join co-op another device is refused.
        let refused = admit(&registry, &coop, "b", &client("B", false)).unwrap_err();
        assert_eq!(refused.status, 409);
        assert_eq!(refused.message, Refused::NotPermitted.to_string());
        let second = client("B", true);
        assert_eq!(
            admit(&registry, &coop, "b", &second).unwrap(),
            Seat {
                session: 1,
                slot: 1
            }
        );
        let full = admit(&registry, &coop, "c", &client("C", true)).unwrap_err();
        assert_eq!(
            full,
            Refusal::busy(
                Refused::Full {
                    title: "Portal 2".into()
                }
                .to_string()
            )
        );
        assert_eq!(pc.started.lock().unwrap().len(), 1);
        // The registry hands the stream its session.
        let instance = registry.instance(1).unwrap();
        assert_eq!(instance.capture_target(1, "d").unwrap(), "window:1@d");
        assert!(registry.instance(2).is_none());
    }

    #[test]
    fn another_game_is_busy_while_a_session_runs() {
        let (registry, pc, mut apps) = setup();
        let coop = registry.entries(&apps, "").remove(0);
        admit(&registry, &coop, "a", &client("A", false)).unwrap();
        {
            let mut catalog = pc.catalog.lock().unwrap();
            let mut handler = catalog.handlers[0].clone();
            handler.guid = "Racer".into();
            handler.executable = "racer.exe".into();
            catalog.handlers.push(handler);
            catalog
                .games
                .push(butterpollo_core::nucleus::handler::Game {
                    guid: "Racer".into(),
                    exe_path: r"C:\Games\racer.exe".into(),
                });
        }
        apps.push(app("Racer", r"C:\Games\racer.exe"));
        registry.use_pc(pc.clone());
        let racer = registry
            .entries(&apps, "")
            .into_iter()
            .find(|coop| coop.app.name == "Racer")
            .unwrap();
        let busy = admit(&registry, &racer, "b", &client("B", true)).unwrap_err();
        assert_eq!(busy.status, 409);
        assert!(busy.message.contains("one game at a time"), "{busy}");
    }

    #[test]
    fn refusals_carry_their_status() {
        let (registry, pc, apps) = setup();
        let coop = registry.entries(&apps, "").remove(0);
        let a = client("A", false);
        // Removed from Nucleus since the library was read: 404 with the reason.
        pc.catalog.lock().unwrap().games.clear();
        let missing = admit(&registry, &coop, "a", &a).unwrap_err();
        assert_eq!(missing.status, 404);
        assert!(missing.message.contains("the game is not added"), "{missing}");
        pc.catalog.lock().unwrap().handlers.clear();
        let missing = admit(&registry, &coop, "a", &a).unwrap_err();
        assert_eq!(
            missing,
            Refusal::missing(&Missing::NoHandler {
                executable: "portal2.exe".into()
            })
        );
        // Unavailable here: 503 with the reason, before any install.
        *pc.unavailable.lock().unwrap() = Some("no service".into());
        *pc.installed.lock().unwrap() = None;
        assert_eq!(
            admit(&registry, &coop, "a", &a).unwrap_err(),
            Refusal::unavailable("no service")
        );
        assert_eq!(pc.installs.load(std::sync::atomic::Ordering::Relaxed), 0);
    }

    #[test]
    fn an_install_runs_at_the_first_launch_and_a_failure_shows() {
        let (registry, pc, apps) = setup();
        let coop = registry.entries(&apps, "").remove(0);
        *pc.installed.lock().unwrap() = None;
        *pc.install_error.lock().unwrap() = Some("hash mismatch".into());
        let failed = admit(&registry, &coop, "a", &client("A", false)).unwrap_err();
        assert_eq!(failed.status, 503);
        assert_eq!(failed.message, "Nucleus Co-op is not ready: hash mismatch");
        assert_eq!(registry.install(""), Install::Failed(failed.message));
        *pc.install_error.lock().unwrap() = None;
        admit(&registry, &coop, "a", &client("A", false)).unwrap();
        assert_eq!(pc.installs.load(std::sync::atomic::Ordering::Relaxed), 2);
        assert_eq!(
            registry.install(""),
            Install::Installed(PathBuf::from(r"C:\Nucleus"))
        );
        assert_eq!(
            registry.install(r"D:\Nucleus"),
            Install::Configured(PathBuf::from(r"C:\Nucleus"))
        );
    }

    #[test]
    fn a_failed_start_or_no_free_pad_is_unavailable() {
        let (registry, pc, apps) = setup();
        let coop = registry.entries(&apps, "").remove(0);
        *pc.free.lock().unwrap() = 0;
        let no_pad = admit(&registry, &coop, "a", &client("A", false)).unwrap_err();
        assert_eq!(no_pad.status, 503);
        assert!(no_pad.message.contains("XInput"), "{no_pad}");
        *pc.free.lock().unwrap() = 1;
        *pc.start_error.lock().unwrap() = Some("Nucleus Co-op is already running".into());
        let failed = admit(&registry, &coop, "a", &client("A", false)).unwrap_err();
        assert_eq!(failed.status, 503);
        assert!(failed.message.contains("already running"), "{failed}");
        assert!(registry.sessions().is_empty());
        // One free pad: a one-player session.
        *pc.start_error.lock().unwrap() = None;
        admit(&registry, &coop, "a", &client("A", false)).unwrap();
        assert_eq!(pc.last().session.lock().unwrap().slots.len(), 1);
    }

    #[test]
    fn only_the_launch_holding_a_slot_leaves_it() {
        let (registry, pc, apps) = setup();
        let coop = registry.entries(&apps, "").remove(0);
        let a = client("A", false);
        let first = admit(&registry, &coop, "a1", &a).unwrap();
        // The same device launches again: the newer launch holds the slot.
        assert_eq!(admit(&registry, &coop, "a2", &a).unwrap(), first);
        registry.leave(first, "a1");
        let session = pc.last();
        let away = |session: &fake::Instance| session.session.lock().unwrap().players()[0].away;
        assert!(!away(&session));
        assert_eq!(registry.sessions()[0].held, [true, false]);
        registry.leave(first, "a2");
        assert!(away(&session));
        assert_eq!(registry.sessions()[0].held, [false, false]);
        // Leaving twice changes nothing.
        registry.leave(first, "a2");
        // Back within the grace: its own slot.
        assert_eq!(admit(&registry, &coop, "a3", &a).unwrap(), first);
        assert!(!away(&session));
    }

    #[test]
    fn a_join_from_the_list_holds_the_slot_until_the_launch() {
        let (registry, pc, apps) = setup();
        let coop = registry.entries(&apps, "").remove(0);
        let a = client("A", false);
        admit(&registry, &coop, "a", &a).unwrap();
        assert_eq!(registry.join(9, &a), Err(JoinError::NoSession));
        assert_eq!(
            registry.join(1, &client("B", false)),
            Err(JoinError::Refused(Refused::NotPermitted))
        );
        let b = client("B", true);
        assert_eq!(registry.join(1, &b), Ok((1, coop.id())));
        let session = pc.last();
        assert!(session.session.lock().unwrap().players()[1].away);
        assert_eq!(
            admit(&registry, &coop, "b", &b).unwrap(),
            Seat {
                session: 1,
                slot: 1
            }
        );
        assert!(!session.session.lock().unwrap().players()[1].away);
        // The streaming owner asking again is not marked away.
        assert_eq!(registry.join(1, &a), Ok((0, coop.id())));
        assert!(!session.session.lock().unwrap().players()[0].away);
    }

    #[test]
    fn kick_and_end_reach_the_session() {
        let (registry, pc, apps) = setup();
        let coop = registry.entries(&apps, "").remove(0);
        let b = client("B", true);
        admit(&registry, &coop, "a", &client("A", false)).unwrap();
        admit(&registry, &coop, "b", &b).unwrap();
        assert_eq!(registry.kick(1, 1).as_deref(), Some("B"));
        assert_eq!(registry.kick(1, 1), None);
        assert_eq!(registry.kick(7, 0), None);
        assert_eq!(registry.sessions()[0].held, [true, false]);
        assert_eq!(
            admit(&registry, &coop, "b2", &b).unwrap_err(),
            Refusal::busy(
                Refused::Kicked {
                    title: "Portal 2".into()
                }
                .to_string()
            )
        );
        assert!(!registry.end(7));
        assert!(registry.end(1));
        let session = pc.last();
        assert!(session.stopped());
        let listed = registry.sessions();
        assert_eq!(listed[0].session.phase, Phase::Ended(End::Ended));
        assert_eq!(
            registry.join(1, &b),
            Err(JoinError::Refused(Refused::Ended {
                title: "Portal 2".into()
            }))
        );
        // An ended session no longer holds Nucleus: the next launch starts anew.
        assert_eq!(
            admit(&registry, &coop, "c", &client("C", false)).unwrap(),
            Seat {
                session: 2,
                slot: 0
            }
        );
        assert_eq!(pc.started.lock().unwrap().len(), 2);
    }

    #[test]
    fn a_session_that_ends_by_itself_is_stopped_once_seen() {
        let (registry, pc, apps) = setup();
        let coop = registry.entries(&apps, "").remove(0);
        let seat = admit(&registry, &coop, "a", &client("A", false)).unwrap();
        let session = pc.last();
        session
            .session
            .lock()
            .unwrap()
            .end(End::Crashed("gone".into()));
        assert!(!session.stopped());
        registry.leave(seat, "a");
        assert!(session.stopped());
        // Still listed for a while so the console can say why.
        assert_eq!(registry.sessions().len(), 1);
    }
}
