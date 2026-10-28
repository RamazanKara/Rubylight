//! Nucleus Co-op on this host: whether it can run here, its install, the
//! handlers and games it offers as co-op library entries, and the one
//! session Nucleus runs at a time.
//!
//! A launch of a co-op entry, or of an app a device streams in the
//! background, is admitted here ([`Registry::admit`], [`Game`]) and
//! carries its [`Seat`] (`Launch::nucleus`) and its [`Held`] slot
//! (`Launch::held`). Its stream reaches the session's [`Instance`] through
//! [`Registry::instance`]; the launch's last copy gives the slot back with
//! [`Registry::leave`], and the launch id that holds a slot makes a stale
//! leave do nothing. [`Pc`] and [`Instance`] are the seams to
//! `butterpollo_windows::nucleus`; the tests use the fakes in `fake`.
use crate::state::{Host, Launch, Shared};
use anyhow::{Context, Result, anyhow, ensure};
use butterpollo_core::config::Config;
use butterpollo_core::input::Input;
use butterpollo_core::input_policy::Policy;
use butterpollo_core::nucleus::coop::{self, Admit, End, Kind, Phase, Refused};
use butterpollo_core::nucleus::generic::{self, Setup};
use butterpollo_core::nucleus::handler::{Catalog, Handler, Missing};
use butterpollo_core::nucleus::mode;
use butterpollo_core::session::{Seat, Warnings};
use butterpollo_core::state::{App, Client};
use butterpollo_windows::input::PadReport;
use butterpollo_windows::nucleus::pads::{self, Dropped};
use butterpollo_windows::nucleus::session::{Opened, Running, Start};
use butterpollo_windows::nucleus::{files, install};
use butterpollo_windows::process::AudioProcess;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, RwLock, Weak};
use std::time::{Duration, Instant};

/// A co-op library entry's name: the app's name, then this, so the entry
/// sorts next to its game and names no tool.
pub const ENTRY_SUFFIX: &str = " (Co-op)";
/// The app setting that hides a game's co-op entry from the library when it
/// is false; the entry shows by default.
pub const ENTRY_KEY: &str = "coop-entry";
/// How long one read of the handlers and games serves the library:
/// Moonlight asks for `/applist` every few seconds.
const CATALOG_TTL: Duration = Duration::from_secs(10);
/// How long an ended session stays listed once no stream holds it, so the
/// console and the devices can show why it ended.
const LINGER: Duration = Duration::from_secs(60);
/// How many feedback reports a slot keeps for its stream, which takes them
/// every few milliseconds; rumble is a state, so the newest matter.
const FEEDBACK_KEPT: usize = 16;
/// How long a co-op launch waits for its player's game window: Nucleus
/// Co-op's own start allows a minute from its last step, and Moonlight
/// waits two for a launch.
pub const WINDOW_WAIT: Duration = Duration::from_secs(90);
/// How often a co-op stream looks at its session.
const WATCH: Duration = Duration::from_millis(100);
/// How long the Nucleus Co-op window the console opened gets to close.
const CLOSE_WAIT: Duration = Duration::from_secs(5);

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
    /// The Nucleus folder, installing the pinned release first; `progress`
    /// hears the bytes downloaded so far.
    fn ensure(&self, configured: &str, progress: &(dyn Fn(u64) + Sync)) -> Result<PathBuf>;
    /// Open Nucleus Co-op's own window at `folder` on the PC's screen, as the
    /// signed-in user, so they can add a handler or a game.
    fn open(&self, folder: &Path) -> Result<Box<dyn Window>>;
    /// A Nucleus Co-op process running on the PC, whoever started it.
    fn running(&self) -> Result<Option<u32>>;
    /// The handlers and the signed-in user's games of the Nucleus at `folder`.
    fn catalog(&self, folder: &Path) -> Result<Catalog>;
    /// The XInput slots the PC's own controllers leave free.
    fn free_pads(&self) -> Result<usize>;
    /// The game of `app` for Rubylight's own one-player handler: its .exe on
    /// this PC, for a Steam link the one Steam starts. Another launcher's
    /// link names none.
    fn one_player(&self, app: &App) -> Result<Setup, Refusal>;
    /// Write Rubylight's one-player handler for `setup` into the Nucleus at
    /// `folder` and add its game for the signed-in user; refused while
    /// Nucleus Co-op runs.
    fn write_own(&self, folder: &Path, setup: &Setup) -> Result<Handler>;
    /// Remove Rubylight's handler `own`, which a hub handler superseded,
    /// and its game; refused while Nucleus Co-op runs.
    fn remove_own(&self, folder: &Path, own: &Handler) -> Result<()>;
    /// What a session's start needs that doesn't need Nucleus Co-op closed
    /// (administrator rights, the signed-in user, `own`'s handler file being
    /// Rubylight's): checked before the console's window closes for it.
    fn ready(&self, folder: &Path, own: Option<&Setup>) -> Result<()>;
    fn start(
        &self,
        start: &Start,
        policy: Policy,
        warnings: Arc<Warnings>,
    ) -> Result<Box<dyn Instance>>;
}

/// The Nucleus Co-op [`Pc::open`] started for the user:
/// `butterpollo_windows::nucleus::session::Opened`.
pub trait Window: Send {
    fn pid(&self) -> u32;
    /// Whether it still runs.
    fn running(&self) -> Result<bool>;
    /// Ask it to close and wait up to `timeout`; it is never ended.
    fn close(&self, timeout: Duration) -> Result<()>;
}
impl Window for butterpollo_windows::nucleus::session::Opened {
    fn pid(&self) -> u32 {
        Opened::pid(self)
    }
    fn running(&self) -> Result<bool> {
        Opened::running(self)
    }
    fn close(&self, timeout: Duration) -> Result<()> {
        Opened::close(self, timeout)
    }
}

/// A running session: `butterpollo_windows::nucleus::session::Running`.
/// No call waits for a [`stop`](Instance::stop) in progress.
pub trait Instance: Send + Sync {
    /// Read or change the session's players and phase.
    fn with_session(&self, f: &mut dyn FnMut(&mut coop::Session));
    /// Why the session ended, or None while it runs.
    fn end(&self) -> Option<End>;
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
impl dyn Instance + '_ {
    /// [`Instance::with_session`] with a result.
    pub fn session<R>(&self, f: impl FnOnce(&mut coop::Session) -> R) -> R {
        let mut f = Some(f);
        let mut out = None;
        self.with_session(&mut |session| out = f.take().map(|f| f(session)));
        out.expect("with_session runs its closure")
    }
}

/// This PC.
#[cfg_attr(test, allow(dead_code, reason = "tests never reach the real PC"))]
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
    fn ensure(&self, configured: &str, progress: &(dyn Fn(u64) + Sync)) -> Result<PathBuf> {
        let mut download = Download(
            tokio::runtime::Handle::try_current()
                .context("downloading Nucleus Co-op needs the host's runtime")?,
            progress,
        );
        install::ensure(
            &butterpollo_core::paths::data_root(),
            configured,
            &mut download,
        )
    }
    fn open(&self, folder: &Path) -> Result<Box<dyn Window>> {
        Ok(Box::new(butterpollo_windows::nucleus::session::open(
            folder,
        )?))
    }
    fn running(&self) -> Result<Option<u32>> {
        butterpollo_windows::nucleus::session::running()
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
    fn one_player(&self, app: &App) -> Result<Setup, Refusal> {
        // A Steam game's link: the .exe Steam starts for it.
        if !starts_exe(app)
            && let Some(appid) = serde_json::to_value(app)
                .ok()
                .and_then(|app| butterpollo_core::steam::game_of(&app))
        {
            let roots = butterpollo_windows::steam::roots();
            let launch =
                butterpollo_core::steam::game_executable(appid, &roots).map_err(|message| {
                    Refusal {
                        status: 404,
                        message,
                    }
                })?;
            return Ok(Setup {
                anti_cheat: launch.vac.then_some("Valve Anti-Cheat"),
                // The launch option's, then any the app adds.
                arguments: format!("{} {}", launch.arguments, arguments(app))
                    .trim()
                    .to_string(),
                ..Setup::new(
                    &launch.exe.to_string_lossy(),
                    &app.name,
                    Some((appid, &launch.install_dir.to_string_lossy())),
                )
            });
        }
        let exe = executable(app)
            .filter(|_| starts_exe(app))
            .filter(|exe| Path::new(exe).is_absolute())
            .ok_or_else(|| {
                Refusal::missing(&Missing::NoExecutable {
                    app: app.name.clone(),
                })
            })?;
        refuse_path(exe)?;
        if !Path::new(exe).is_file() {
            return Err(Refusal {
                status: 404,
                message: format!(
                    "{exe} is not on this PC: set the app's command to the game's .exe"
                ),
            });
        }
        Ok(Setup {
            arguments: arguments(app),
            ..Setup::new(exe, &app.name, None)
        })
    }
    fn write_own(&self, folder: &Path, setup: &Setup) -> Result<Handler> {
        if let Some(pid) = self.running()? {
            anyhow::bail!(butterpollo_windows::nucleus::session::already_running(pid));
        }
        let appdata = butterpollo_windows::user_folder::roaming_app_data().map_err(|error| {
            anyhow!("finding the signed-in user's AppData folder, which holds Nucleus Co-op's game list: {error}")
        })?;
        files::write_own(folder, &appdata, setup)
    }
    fn remove_own(&self, folder: &Path, own: &Handler) -> Result<()> {
        if let Some(pid) = self.running()? {
            anyhow::bail!(butterpollo_windows::nucleus::session::already_running(pid));
        }
        let appdata = butterpollo_windows::user_folder::roaming_app_data().map_err(|error| {
            anyhow!("finding the signed-in user's AppData folder, which holds Nucleus Co-op's game list: {error}")
        })?;
        files::remove_own(folder, &appdata, own)
    }
    fn ready(&self, folder: &Path, own: Option<&Setup>) -> Result<()> {
        butterpollo_windows::nucleus::session::ready()?;
        if let Some(setup) = own {
            butterpollo_windows::user_folder::roaming_app_data().map_err(|error| {
                anyhow!("finding the signed-in user's AppData folder, which holds Nucleus Co-op's game list: {error}")
            })?;
            files::check_own(folder, setup)?;
        }
        Ok(())
    }
    fn start(
        &self,
        start: &Start,
        policy: Policy,
        warnings: Arc<Warnings>,
    ) -> Result<Box<dyn Instance>> {
        let running = butterpollo_windows::nucleus::session::start_on_pc(start, policy, warnings)?;
        Ok(Box::new(OnPcInstance(running)))
    }
}

/// A session on this PC.
#[cfg_attr(test, allow(dead_code, reason = "tests never reach the real PC"))]
struct OnPcInstance(Running);
impl Instance for OnPcInstance {
    fn with_session(&self, f: &mut dyn FnMut(&mut coop::Session)) {
        self.0.with_session(f);
    }
    fn end(&self) -> Option<End> {
        self.0.with_session(|s| ended(s).cloned())
    }
    fn capture_target(&self, slot: usize, display: &str) -> Result<String> {
        self.0.capture_target(slot, display)
    }
    fn resize(&self, slot: usize, size: (u32, u32)) -> Result<()> {
        self.0.resize(slot, size)
    }
    fn input(&self, device: &str, event: Input) -> Result<(), Dropped> {
        self.0.input(device, event)
    }
    fn feedback(&self) -> Vec<(usize, PadReport)> {
        self.0.feedback()
    }
    fn audio(&self, slot: usize) -> Result<AudioProcess> {
        self.0.audio(slot)
    }
    fn stop(&self) {
        self.0.stop();
    }
}

/// Why `session` ended, once it did.
fn ended(session: &coop::Session) -> Option<&End> {
    match &session.phase {
        Phase::Ended(end) => Some(end),
        _ => None,
    }
}

/// Downloads with the updater's HTTPS client, which follows redirects to
/// GitHub's asset hosts only. Blocks, so never on the async runtime.
#[cfg_attr(test, allow(dead_code, reason = "tests never reach the real PC"))]
struct Download<'a>(tokio::runtime::Handle, &'a (dyn Fn(u64) + Sync));
impl install::Fetch for Download<'_> {
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
                (self.1)(body.len() as u64);
            }
            Ok(body)
        })
    }
}

/// Add `part` to `body`, refusing a body longer than `limit` bytes.
fn chunk(body: &mut Vec<u8>, part: &[u8], limit: u64) -> Result<()> {
    ensure!(
        (body.len() as u64).saturating_add(part.len() as u64) <= limit,
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

/// The command line that starts an app's program: its command, else its
/// first detached command.
fn command(app: &App) -> Option<&str> {
    let detached = app
        .extra
        .get("detached")
        .and_then(Value::as_array)
        .and_then(|commands| commands.first())
        .and_then(Value::as_str);
    [Some(app.cmd.as_str()), detached]
        .into_iter()
        .flatten()
        .find(|line| !butterpollo_windows::process::command_target(line).is_empty())
}

/// The program an app starts: its command's, else its first detached
/// command's.
fn executable(app: &App) -> Option<&str> {
    command(app).map(butterpollo_windows::process::command_target)
}

/// What an app passes its program, for Rubylight's handler.
fn arguments(app: &App) -> String {
    command(app)
        .map(butterpollo_windows::process::command_arguments)
        .unwrap_or_default()
        .to_string()
}

/// The co-op entry's name for the app `name`.
pub fn entry_name(name: &str) -> String {
    format!("{name}{ENTRY_SUFFIX}")
}

/// Whether the app lists its co-op entry ([`ENTRY_KEY`], on by default),
/// read as every app switch is and as the console's game page reads it.
fn entry_shown(app: &App) -> bool {
    crate::process::app_bool(app, ENTRY_KEY, true)
}

/// A co-op library entry: an app whose program Nucleus Co-op has a handler
/// and an added game for.
#[derive(Clone, Debug)]
pub struct CoopApp {
    /// The entry the library lists: the app's name and [`ENTRY_SUFFIX`],
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

/// Where one app's co-op entry stands in the library.
enum Listing {
    Listed(Box<CoopApp>),
    /// Its app hides it ([`ENTRY_KEY`]).
    Hidden(Handler),
    /// An app or an earlier entry already has its id.
    Taken,
    Missing(Missing),
}

/// Each app's [`Listing`], in the order of `apps`: what the library lists
/// and what the console's game page says come from this one place.
fn listings(catalog: &Catalog, apps: &[App]) -> Vec<Listing> {
    let mut taken: BTreeSet<u32> = apps
        .iter()
        .flat_map(|app| std::iter::once(app.id()).chain(app.aliases.iter().copied()))
        .collect();
    apps.iter()
        .map(|app| {
            let found = match catalog.find(&app.name, executable(app)) {
                Ok(found) => found,
                Err(missing) => return Listing::Missing(missing),
            };
            if !entry_shown(app) {
                return Listing::Hidden(found.handler);
            }
            let entry = App {
                name: entry_name(&app.name),
                cmd: String::new(),
                working_dir: String::new(),
                prep: Vec::new(),
                computed_id: None,
                aliases: Vec::new(),
                extra: BTreeMap::new(),
            };
            if !taken.insert(entry.id()) {
                return Listing::Taken;
            }
            Listing::Listed(Box::new(CoopApp {
                entry,
                app: app.clone(),
                handler: found.handler,
            }))
        })
        .collect()
}

/// The co-op entries of `apps`, in their order. An entry whose id an app
/// or an earlier entry already has is left out, and so is one its app hides
/// ([`ENTRY_KEY`]).
fn entries_of(catalog: &Catalog, apps: &[App]) -> Vec<CoopApp> {
    listings(catalog, apps)
        .into_iter()
        .filter_map(|listing| match listing {
            Listing::Listed(coop) => Some(*coop),
            _ => None,
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
    /// Downloading, with the bytes received so far; all of them while the
    /// archive is checked and unpacked.
    Installing(u64),
    Failed(String),
}

/// What the console's Set up co-op found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetUp {
    Installed,
    /// Another call downloads it.
    Installing,
    /// This call marked it as installing: run [`Registry::install_now`].
    Started,
}

/// Whether the console's game page offers co-op for `app`: Nucleus Co-op
/// starts games by their .exe, so an app without one (the Desktop, a game
/// Playnite starts) has no co-op.
pub fn names_executable(app: &App) -> bool {
    executable(app).is_some()
}

/// Whether `app` starts a game's .exe itself, which Nucleus Co-op can find a
/// handler and an added game for; a launcher's link (`steam://`, or
/// `cmd /c start` with one) never matches.
pub fn starts_exe(app: &App) -> bool {
    executable(app).is_some_and(|target| {
        target
            .rsplit(['\\', '/'])
            .next()
            .is_some_and(|name| name.to_ascii_lowercase().ends_with(".exe"))
    })
}

/// Refuses a game Rubylight's handler never starts (on a network share, or
/// Windows' own program) before anything opens its path as SYSTEM.
fn refuse_path(exe_path: &str) -> Result<(), Refusal> {
    let windows = std::env::var("SystemRoot").map_err(|error| {
        Refusal::unavailable(format!("Windows' folder is unknown (SystemRoot): {error}"))
    })?;
    match generic::refused_path(exe_path, &windows) {
        Some(message) => Err(Refusal {
            status: 404,
            message,
        }),
        None => Ok(()),
    }
}

/// The handler a session starts with.
enum Chosen {
    /// A hub or user handler, with the game the user added.
    Hub(Handler),
    /// Rubylight's own one-player handler, written as the session starts.
    Own(Setup),
}
impl Chosen {
    fn guid(&self) -> String {
        match self {
            Self::Hub(handler) => handler.guid.clone(),
            Self::Own(setup) => setup.guid(),
        }
    }
}

/// Rubylight's own handler for a background stream of `app`, which has no
/// hub handler. A game shipping anti-cheat is refused: Nucleus Co-op's
/// hooks would look like cheating.
fn own_handler(pc: &dyn Pc, catalog: &Catalog, app: &App) -> Result<Chosen, Refusal> {
    let mut setup = pc.one_player(app)?;
    // Before the anti-cheat scan reads its folders.
    refuse_path(&setup.exe_path)?;
    if generic::launcher(&setup.exe_path) {
        let launcher = setup.exe_path.rsplit('\\').next().unwrap_or_default();
        return Err(Refusal {
            status: 404,
            message: format!(
                "{launcher} is a launcher, not the game: set the app's command to the game's .exe"
            ),
        });
    }
    if let Some(shipping) = generic::unreal_shipping(&setup.exe_path) {
        tracing::info!(
            stub = setup.exe_path,
            shipping,
            "Nucleus Co-op: an Unreal game's stub starts its Shipping .exe"
        );
        setup.exe_path = shipping;
    }
    // Found by another path than the app's command: the game the user added
    // at exactly that path keeps its hub handler. A file-name match is
    // another game's (Portal's hl2.exe is not Half-Life 2).
    if let Some(found) = catalog.added(&setup.exe_path) {
        return Ok(Chosen::Hub(found.handler));
    }
    if let Some(anti_cheat) = generic::anti_cheat(&setup) {
        let executable = setup.exe_path.rsplit('\\').next().unwrap_or_default();
        return Err(Refusal::missing(&Missing::AntiCheat {
            executable: executable.into(),
            anti_cheat: anti_cheat.into(),
        }));
    }
    Ok(Chosen::Own(setup))
}

/// Where one app stands for co-op, for the console's game page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GameStatus {
    /// Nucleus Co-op has a handler and the added game: the entry can list.
    Ready {
        handler: String,
        /// The most players a session of it seats (the handler's, capped by
        /// `nucleus_coop_players`).
        players: usize,
    },
    /// Why not: the host's text, as a launch would answer it.
    Missing(Missing),
    /// The entry's name gives it the id another app or an earlier entry
    /// already has (two apps named alike): the library can't list it.
    Taken { entry: String },
}

/// What a launch asks Nucleus Co-op to run.
#[derive(Clone, Copy)]
pub enum Game<'a> {
    /// A co-op entry: a slot of its game's co-op session.
    Coop(&'a CoopApp),
    /// An app a device streams in the background (`background_stream`,
    /// `background-stream`): a private session of one player.
    Background(&'a App),
}
impl Game<'_> {
    fn app(&self) -> &App {
        match self {
            Self::Coop(coop) => &coop.app,
            Self::Background(app) => app,
        }
    }
    /// The id the lists show the session under.
    fn app_id(&self) -> u32 {
        match self {
            Self::Coop(coop) => coop.id(),
            Self::Background(app) => app.id(),
        }
    }
    fn kind(&self) -> Kind {
        match self {
            Self::Coop(_) => Kind::Coop,
            Self::Background(_) => Kind::Background,
        }
    }
}

/// A launch asking for a slot.
pub struct Admission<'a> {
    /// The launch's id, which holds the slot until its stream ends.
    pub launch: &'a str,
    pub client: &'a Client,
    pub game: Game<'a>,
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
    #[cfg_attr(
        not(test),
        allow(dead_code, reason = "the tests read which slots a launch holds")
    )]
    pub held: Vec<bool>,
}

struct Entry {
    instance: Arc<dyn Instance>,
    app_id: u32,
    /// The launch streaming each slot; a leave from any other launch is stale.
    holders: Vec<Option<String>>,
    /// Each slot's feedback its stream has not taken yet.
    feedback: Vec<Vec<PadReport>>,
    /// When the registry first saw the session ended.
    ended: Option<Instant>,
}

#[derive(Default)]
struct State {
    sessions: BTreeMap<u32, Entry>,
    next: u32,
    /// The bytes downloaded while an install runs, or is claimed to run.
    installing: Option<u64>,
    install_error: Option<String>,
    /// Finished installs: a call waiting for one wakes when it moves.
    installs: u64,
    /// The last install's result, which its waiters share.
    install_result: Option<Result<PathBuf, String>>,
    /// Calls waiting for the install another one runs.
    install_waiters: usize,
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
    /// Signalled when an install finishes.
    install_done: Condvar,
    catalog: Mutex<Option<Cached>>,
    /// The Nucleus Co-op the console opened: the only one the host closes,
    /// for a session or the user.
    opened: Mutex<Option<Box<dyn Window>>>,
}

impl Registry {
    pub fn new(pc: Arc<dyn Pc>) -> Self {
        Self {
            pc: RwLock::new(pc),
            admit: Mutex::new(()),
            state: Mutex::default(),
            install_done: Condvar::new(),
            catalog: Mutex::new(None),
            opened: Mutex::new(None),
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
    /// or not installed. Each failure still shows: the console's panel names
    /// a broken install, and [`Self::catalog`] logs an unreadable one.
    pub fn entries(&self, apps: &[App], configured: &str) -> Vec<CoopApp> {
        let pc = self.pc();
        if pc.unavailable().is_some() {
            return Vec::new();
        }
        let folder = match pc.installed(configured) {
            Ok(Some(folder)) => folder,
            Ok(None) => return Vec::new(),
            Err(error) => {
                tracing::warn!(error = %format!("{error:#}"), "Nucleus Co-op: no co-op entries");
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
        match (installed, state.installing) {
            (Ok(Some(folder)), _) if !configured.trim().is_empty() => Install::Configured(folder),
            (Ok(Some(folder)), _) => Install::Installed(folder),
            (_, Some(received)) => Install::Installing(received),
            (Err(error), None) => Install::Failed(format!("{error:#}")),
            (Ok(None), None) => state
                .install_error
                .clone()
                .map_or(Install::Missing, Install::Failed),
        }
    }

    /// The Nucleus folder, installing it first. One install runs at a time:
    /// a launch while the console's Set up co-op (or another launch) installs
    /// waits for it and shares its result, so there is one download, one
    /// progress and one result. A failure is kept for the console.
    fn ensure(&self, pc: &dyn Pc, configured: &str) -> Result<PathBuf, Refusal> {
        if let Ok(Some(folder)) = pc.installed(configured) {
            return Ok(folder);
        }
        let mut state = self.lock();
        if state.installing.is_some() {
            let round = state.installs;
            state.install_waiters += 1;
            while state.installs == round {
                state = self
                    .install_done
                    .wait(state)
                    .unwrap_or_else(PoisonError::into_inner);
            }
            state.install_waiters -= 1;
            return match state.install_result.clone() {
                Some(Ok(folder)) => Ok(folder),
                Some(Err(message)) => Err(Refusal::unavailable(message)),
                None => Err(Refusal::unavailable("Nucleus Co-op is not ready")),
            };
        }
        state.installing = Some(0);
        drop(state);
        self.run_install(pc, configured)
    }

    /// Run the install this caller claimed (`installing` set), keep its
    /// result and wake the calls waiting for it.
    /// The install path returns its errors instead of panicking, so every
    /// waiter gets the reason. The release profile aborts on a panic
    /// (`panic = "abort"`), which ends the host loudly; catching one here
    /// only matters where panics unwind (test and debug builds).
    fn run_install(&self, pc: &dyn Pc, configured: &str) -> Result<PathBuf, Refusal> {
        let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            pc.ensure(configured, &|received| {
                self.lock().installing = Some(received);
            })
        }));
        let result = match run {
            Ok(result) => result.map_err(|error| format!("Nucleus Co-op is not ready: {error:#}")),
            Err(panic) => {
                let why = panic
                    .downcast_ref::<&str>()
                    .map(|s| s.to_string())
                    .or_else(|| panic.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "it panicked".into());
                tracing::error!(why, "Nucleus Co-op: the install panicked");
                Err(format!(
                    "Nucleus Co-op is not ready: the install stopped: {why}"
                ))
            }
        };
        let mut state = self.lock();
        state.installing = None;
        state.installs += 1;
        state.install_error = result.as_ref().err().cloned();
        state.install_result = Some(result.clone());
        self.install_done.notify_all();
        result.map_err(Refusal::unavailable)
    }

    /// The console's Set up co-op: whether Nucleus Co-op is there, already
    /// downloading, or now marked as installing, which the caller then runs
    /// with [`Self::install_now`] off the async runtime. Refused where
    /// Nucleus Co-op can't run, so nothing downloads that could never start.
    pub fn set_up(&self, configured: &str) -> Result<SetUp, Refusal> {
        let pc = self.pc();
        if let Some(why) = pc.unavailable() {
            return Err(Refusal::unavailable(why));
        }
        // A folder that can't be trusted (its owner or ACL) is refused, not
        // downloaded over.
        let installed = pc.installed(configured).map_err(|error| {
            Refusal::unavailable(format!("Nucleus Co-op is not ready: {error:#}"))
        })?;
        let mut state = self.lock();
        if installed.is_some() {
            return Ok(SetUp::Installed);
        }
        if state.installing.is_some() {
            return Ok(SetUp::Installing);
        }
        state.installing = Some(0);
        state.install_error = None;
        Ok(SetUp::Started)
    }

    /// Run the install [`Self::set_up`] started; a failure shows in
    /// [`Self::install`], as a launch's does.
    pub fn install_now(&self, configured: &str) {
        let pc = self.pc();
        // `set_up` claimed the install; `Pc::ensure` downloads nothing when
        // it is there after all.
        match self.run_install(pc.as_ref(), configured) {
            Ok(folder) => {
                tracing::info!(folder = %folder.display(), "Nucleus Co-op: set up from the console")
            }
            Err(refusal) => {
                tracing::warn!(error = %refusal, "Nucleus Co-op: setting up from the console failed")
            }
        }
    }

    /// Open Nucleus Co-op's window on the PC, where the user adds a handler
    /// or a game. Refused where it can't run, before it is set up, while a
    /// session runs or starts, which holds it, and while Nucleus Co-op
    /// already runs. The next session closes it first.
    pub fn open(&self, configured: &str) -> Result<(), Refusal> {
        let pc = self.pc();
        if let Some(why) = pc.unavailable() {
            return Err(Refusal::unavailable(why));
        }
        let folder = match pc.installed(configured) {
            Ok(Some(folder)) => folder,
            Ok(None) => {
                return Err(Refusal::busy(
                    "Nucleus Co-op is not set up yet: set up co-op first",
                ));
            }
            Err(error) => {
                return Err(Refusal::unavailable(format!(
                    "Nucleus Co-op is not ready: {error:#}"
                )));
            }
        };
        // A launch being admitted may start Nucleus Co-op any moment.
        let _one = match self.admit.try_lock() {
            Ok(one) => one,
            Err(std::sync::TryLockError::Poisoned(one)) => one.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => {
                return Err(Refusal::busy(
                    "A co-op or background stream is starting: open Nucleus Co-op once it has",
                ));
            }
        };
        // Read with no launch being admitted, so a session can't slip in.
        let running = self.lock().sessions.values().find_map(|entry| {
            entry
                .instance
                .session(|s| (!s.ended()).then(|| s.title.clone()))
        });
        if let Some(title) = running {
            return Err(Refusal::busy(format!(
                "Nucleus Co-op is running {title}: end that session first"
            )));
        }
        let mut opened = self.opened.lock().unwrap_or_else(PoisonError::into_inner);
        // Ours by its job, not its first process id: Nucleus Co-op may have
        // restarted itself.
        if let Some(window) = opened.as_ref() {
            let running = window.running().map_err(|error| {
                Refusal::unavailable(format!(
                    "Nucleus Co-op's window could not be read: {error:#}"
                ))
            })?;
            if running {
                return Err(Refusal::busy(
                    "Nucleus Co-op is already open on the PC's screen: use it there, or close it here",
                ));
            }
            *opened = None;
        }
        match pc.running() {
            Ok(Some(pid)) => {
                return Err(Refusal::busy(format!(
                    "Nucleus Co-op is already running on the PC (process {pid}): close it there first"
                )));
            }
            Ok(None) => {}
            Err(error) => {
                return Err(Refusal::unavailable(format!(
                    "Nucleus Co-op's processes could not be listed: {error:#}"
                )));
            }
        }
        let window = pc.open(&folder).map_err(|error| {
            Refusal::unavailable(format!("Nucleus Co-op did not open: {error:#}"))
        })?;
        tracing::info!(pid = window.pid(), folder = %folder.display(), "Nucleus Co-op: opened on the PC from the console");
        *opened = Some(window);
        Ok(())
    }

    /// Whether the Nucleus Co-op the console opened still runs. One that
    /// can't be read counts as open, so the console offers Close, which then
    /// says why.
    pub fn opened(&self) -> bool {
        match self.opened.try_lock() {
            Ok(opened) => opened
                .as_ref()
                .is_some_and(|window| window.running().unwrap_or(true)),
            // Being opened or closed right now.
            Err(std::sync::TryLockError::WouldBlock) => true,
            Err(std::sync::TryLockError::Poisoned(opened)) => opened
                .into_inner()
                .as_ref()
                .is_some_and(|window| window.running().unwrap_or(true)),
        }
    }

    /// The console's Close Nucleus Co-op: closes the window [`Self::open`]
    /// opened, whether it was still open. A Nucleus Co-op the host did not
    /// start is refused, never ended.
    pub fn close(&self) -> Result<bool, Refusal> {
        let mut opened = self.opened.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(window) = opened.as_ref() {
            let running = window.running().map_err(|error| {
                Refusal::unavailable(format!(
                    "Nucleus Co-op's window could not be read: {error:#}"
                ))
            })?;
            if running {
                window
                    .close(CLOSE_WAIT)
                    .map_err(|error| Refusal::busy(format!("{error:#}")))?;
                *opened = None;
                return Ok(true);
            }
            *opened = None;
        }
        match self.pc().running() {
            Ok(Some(pid)) => Err(Refusal::busy(format!(
                "Nucleus Co-op (process {pid}) was not opened from this console: close it on the PC"
            ))),
            Ok(None) => Ok(false),
            Err(error) => Err(Refusal::unavailable(format!(
                "Nucleus Co-op's processes could not be listed: {error:#}"
            ))),
        }
    }

    /// Before a session starts Nucleus Co-op: close the window the console
    /// opened. One that won't close refuses the session, loudly, as a
    /// Nucleus Co-op the host didn't start does.
    fn close_for_session(&self, title: &str) -> Result<(), Refusal> {
        let mut opened = self.opened.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(window) = opened.as_ref() else {
            return Ok(());
        };
        let pid = window.pid();
        let refuse = |error: anyhow::Error| {
            tracing::warn!(pid, error = %format!("{error:#}"), "Nucleus Co-op: the console's window did not close for a session");
            Refusal::unavailable(format!("Nucleus Co-op did not start {title}: {error:#}"))
        };
        let running = window
            .running()
            .with_context(|| format!("Nucleus Co-op's window (process {pid}) could not be read"));
        if !running.map_err(refuse)? {
            *opened = None;
            return Ok(());
        }
        window.close(CLOSE_WAIT).map_err(refuse)?;
        *opened = None;
        Ok(())
    }

    /// Each app's co-op status, in the order of `apps`; None while Nucleus
    /// Co-op is not installed, and the error while its handlers and games
    /// don't load. The handlers are read as the library reads them, so a
    /// handler added in Nucleus Co-op shows within [`CATALOG_TTL`].
    pub fn games(&self, apps: &[App], config: &Config) -> Option<Result<Vec<GameStatus>, String>> {
        let folder = self.pc().installed(&configured(config)).ok().flatten()?;
        let catalog = match self.catalog(&folder, false) {
            Ok(catalog) => catalog,
            Err(error) => return Some(Err(error)),
        };
        let ready = |handler: Handler| GameStatus::Ready {
            players: coop::players(
                handler.max_players,
                config.nucleus_coop_players(),
                coop::MAX_PLAYERS,
            )
            .unwrap_or(1),
            handler: handler.name,
        };
        // As the library lists them: a hidden entry is ready (its switch is
        // off), one whose id is taken is not.
        let statuses = listings(&catalog, apps)
            .into_iter()
            .zip(apps)
            .map(|(listing, app)| match listing {
                Listing::Listed(coop) => ready(coop.handler),
                Listing::Hidden(handler) => ready(handler),
                Listing::Taken => GameStatus::Taken {
                    entry: entry_name(&app.name),
                },
                Listing::Missing(missing) => GameStatus::Missing(missing),
            })
            .collect();
        Some(Ok(statuses))
    }

    /// Admit a launch: start a session of its game, or seat the device in
    /// the running one. The launch then holds its slot until
    /// [`leave`](Self::leave). A background stream's refusal names the app
    /// and why it can't stream in the background; it never runs another way.
    pub fn admit(&self, request: Admission) -> Result<Seat, Refusal> {
        let game = request.game;
        if game.kind() == Kind::Coop {
            return self.admit_game(request);
        }
        log_background_mapping();
        // A value earlier builds wrote is Nucleus Co-op too, until the next save replaces it.
        let setting = request.config.get(mode::KEY, "");
        let admitted = match mode::parse(setting) {
            Err(reason) if !mode::retired_background(setting) => Err(Refusal::unavailable(reason)),
            _ => self.admit_game(request),
        };
        admitted.map_err(|refusal| Refusal {
            message: format!(
                "{} can't stream in the background: {}",
                game.app().name,
                refusal.message
            ),
            ..refusal
        })
    }

    /// The game of the running background stream of app `app_id` that
    /// `device` plays in, which it returns to.
    fn returning(&self, app_id: u32, device: &str) -> Option<String> {
        self.lock()
            .sessions
            .values()
            .filter(|entry| entry.app_id == app_id)
            .find_map(|entry| {
                entry.instance.session(|s| {
                    (!s.ended() && s.kind == Kind::Background && s.slot_of(device).is_some())
                        .then(|| s.game.clone())
                })
            })
    }

    fn admit_game(&self, request: Admission) -> Result<Seat, Refusal> {
        let pc = self.pc();
        if let Some(why) = pc.unavailable() {
            return Err(Refusal::unavailable(why));
        }
        let kind = request.game.kind();
        let _one = self.admit.lock().unwrap_or_else(PoisonError::into_inner);
        let folder = self.ensure(pc.as_ref(), &configured(request.config))?;
        let catalog = self.catalog(&folder, true).map_err(|error| {
            Refusal::unavailable(format!(
                "Nucleus Co-op's handlers and games did not load: {error}"
            ))
        })?;
        let app = request.game.app();
        let client = request.client;
        stop(self.reap());
        let (found, returning) = match catalog.find(&app.name, executable(app)) {
            Ok(found) => (Some(Chosen::Hub(found.handler)), None),
            // A one-player game without a hub handler streams in the
            // background with ours. Its owner returning to it finds the
            // session's game: no Steam read or folder scan again.
            Err(Missing::NoHandler { .. }) if kind == Kind::Background => {
                match self.returning(request.game.app_id(), &client.uuid) {
                    Some(game) => (None, Some(game)),
                    None => (Some(own_handler(pc.as_ref(), &catalog, app)?), None),
                }
            }
            Err(missing) => return Err(Refusal::missing(&missing)),
        };
        let game = returning
            .or_else(|| found.as_ref().map(Chosen::guid))
            .unwrap_or_default();
        let decision = {
            let state = self.lock();
            let current = state
                .sessions
                .values()
                .find_map(|entry| entry.instance.session(|s| (!s.ended()).then(|| s.clone())));
            coop::admit(current.as_ref(), &game, kind, &client.uuid)
        };
        match decision {
            Admit::Busy(text) => Err(Refusal::busy(text)),
            Admit::Join(id) => {
                let mut state = self.lock();
                let entry = state.sessions.get_mut(&id).ok_or_else(|| {
                    Refusal::busy(format!("The {} session ended: launch it again", app.name))
                })?;
                let slot = entry
                    .instance
                    .session(|s| s.join(&client.uuid, &client.name, client.allows_coop_join()))
                    .map_err(|refused| Refusal::busy(refused.to_string()))?
                    .slot();
                let holder = entry.holders.get_mut(slot).ok_or_else(|| {
                    Refusal::unavailable(format!("the session has no slot {slot}"))
                })?;
                *holder = Some(request.launch.into());
                tracing::info!(session = id, ?kind, slot, client = %client.name, title = %app.name, "Nucleus Co-op: a player joined");
                Ok(Seat { session: id, slot })
            }
            Admit::Start => {
                // The session returned to ended meanwhile.
                let Some(found) = found else {
                    return Err(Refusal::busy(format!(
                        "The {} session ended: launch it again",
                        app.name
                    )));
                };
                let free = pc.free_pads().map_err(|error| {
                    Refusal::unavailable(format!(
                        "Nucleus Co-op could not count the free controller slots: {error:#}"
                    ))
                })?;
                let players = match (kind, &found) {
                    (Kind::Coop, Chosen::Hub(handler)) => coop::players(
                        handler.max_players,
                        request.config.nucleus_coop_players(),
                        free,
                    ),
                    _ => coop::players(1, 1, free),
                }
                .map_err(Refusal::unavailable)?;
                let policy = Policy::resolve(request.config)
                    .map_err(|error| Refusal::unavailable(format!("{error:#}")))?;
                let own = match &found {
                    Chosen::Own(setup) => Some(setup),
                    Chosen::Hub(_) => None,
                };
                pc.ready(&folder, own).map_err(|error| {
                    Refusal::unavailable(format!(
                        "Nucleus Co-op did not start {}: {error:#}",
                        app.name
                    ))
                })?;
                // Last, so a launch refused for another reason leaves the
                // user's window open.
                self.close_for_session(&app.name)?;
                // Ours for a game the user has since added with a hub
                // handler goes while Nucleus is closed. A failure leaves a
                // handler that is never chosen: no reason to refuse.
                for own in catalog.superseded() {
                    if let Err(error) = pc.remove_own(&folder, own) {
                        tracing::warn!(file = own.file, error = %format!("{error:#}"), "Nucleus Co-op: Rubylight's superseded handler was not removed");
                    }
                }
                let (handler, steam_app) = match found {
                    Chosen::Hub(handler) => (handler, None),
                    Chosen::Own(setup) => (
                        pc.write_own(&folder, &setup).map_err(|error| {
                            Refusal::unavailable(format!(
                                "Rubylight's Nucleus Co-op handler for {} was not written: {error:#}",
                                app.name
                            ))
                        })?,
                        setup.steam_app,
                    ),
                };
                let id = {
                    let mut state = self.lock();
                    state.next += 1;
                    state.next
                };
                let start = Start {
                    id,
                    kind,
                    folder: &folder,
                    handler: &handler,
                    steam_app,
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
                        app_id: request.game.app_id(),
                        holders,
                        feedback: (0..players).map(|_| Vec::new()).collect(),
                        ended: None,
                    },
                );
                tracing::info!(session = id, ?kind, players, game = %handler.guid, own = handler.own, client = %client.name, title = %app.name, "Nucleus Co-op: session started");
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

    /// The feedback for `seat`'s player since its last call. One read of
    /// the pads serves every slot; each slot keeps the last
    /// [`FEEDBACK_KEPT`] reports until its stream takes them.
    pub fn feedback(&self, seat: Seat) -> Vec<PadReport> {
        let Some(instance) = self.instance(seat.session) else {
            return Vec::new();
        };
        let reports = instance.feedback();
        let mut state = self.lock();
        let Some(entry) = state.sessions.get_mut(&seat.session) else {
            return Vec::new();
        };
        for (slot, report) in reports {
            if let Some(kept) = entry.feedback.get_mut(slot) {
                if kept.len() == FEEDBACK_KEPT {
                    kept.remove(0);
                }
                kept.push(report);
            }
        }
        entry
            .feedback
            .get_mut(seat.slot)
            .map(std::mem::take)
            .unwrap_or_default()
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
        // A background stream is private: its id is no session to join, and
        // its refusals would name the game.
        if entry.instance.session(|s| s.kind) != Kind::Coop {
            return Err(JoinError::NoSession);
        }
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
        tracing::info!(
            session,
            slot,
            device,
            "Nucleus Co-op: player removed in the web console"
        );
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
        tracing::info!(session, "Nucleus Co-op: session ended");
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

/// The first background stream says once that `background_stream` and
/// `background-stream` now run in Nucleus Co-op.
fn log_background_mapping() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        tracing::info!(
            "Nucleus Co-op: background_stream (device) and background-stream (app) now run background streams in Nucleus Co-op, which needs a handler for the game"
        );
    });
}

fn stop(instances: Vec<Arc<dyn Instance>>) {
    for instance in instances {
        instance.stop();
    }
}

/// A co-op launch's slot, shared by every copy of the launch
/// (`Launch::held`): the last copy dropped gives it back with
/// [`Registry::leave`]. That is an expired, replaced or failed launch at
/// once, a started one with its stream's session.
pub struct Held {
    host: Weak<Host>,
    seat: Seat,
    launch: String,
}
impl Held {
    pub fn new(host: &Shared, seat: Seat, launch: &str) -> Self {
        Self {
            host: Arc::downgrade(host),
            seat,
            launch: launch.into(),
        }
    }
}
impl Drop for Held {
    fn drop(&mut self) {
        if let Some(h) = self.host.upgrade() {
            h.nucleus.leave(self.seat, &self.launch);
        }
    }
}

/// End the streams of co-op session `session`, only `device`'s when given:
/// pending launches are removed, running streams stop. The removed launches
/// are dropped after the sessions lock, so their slots are given back
/// outside it.
pub fn end_streams(h: &Host, session: u32, device: Option<&str>) {
    let canceled: Vec<_> = {
        let mut sessions = h.sessions.lock().unwrap();
        let matches = |launch: &Launch| {
            launch.nucleus.is_some_and(|seat| seat.session == session)
                && device.is_none_or(|device| launch.client.uuid == device)
        };
        let ids: Vec<String> = sessions
            .pending
            .values()
            .filter(|launch| matches(launch))
            .chain(
                sessions
                    .active
                    .values()
                    .map(|s| &s.launch)
                    .filter(|launch| matches(launch)),
            )
            .map(|launch| launch.id.clone())
            .collect();
        ids.iter()
            .flat_map(|id| sessions.request_stop(Some(id)))
            .collect()
    };
    if !canceled.is_empty() {
        tracing::info!(
            session,
            launches = canceled.len(),
            "Nucleus Co-op: pending launches canceled"
        );
    }
    drop(canceled);
}

/// What a player's stream says when its session ended.
fn ended_text(session: &coop::Session, end: &End) -> String {
    let (title, what) = (
        &session.title,
        match session.kind {
            Kind::Coop => "co-op session",
            Kind::Background => "background stream",
        },
    );
    match end {
        End::Ended => format!("The {title} {what} was ended on the PC"),
        end => format!("The {title} {what} ended: {end}"),
    }
}

/// A co-op player's input and feedback. Its first controller drives its
/// slot's pad and nothing else: no event reaches the PC's desktop or a pad
/// of its own.
pub struct Player {
    instance: Arc<dyn Instance>,
    seat: Seat,
    device: String,
    /// The rumble setting of the player's stream.
    rumble: bool,
    /// The kinds of dropped input the player was told about.
    told: Vec<Dropped>,
}
impl Player {
    /// The player `launch` streams; None for any other launch, or once its
    /// session is gone.
    pub fn of(h: &Host, launch: &Launch, config: &Config) -> Option<Self> {
        let seat = launch.nucleus?;
        Some(Self {
            instance: h.nucleus.instance(seat.session)?,
            seat,
            device: launch.client.uuid.clone(),
            rumble: Policy::resolve(config).is_ok_and(|policy| policy.forward_rumble),
            told: Vec::new(),
        })
    }
    /// Drive the slot's pad with `event`. What the pad cannot take is
    /// dropped, and the stream says so once per kind.
    pub fn input(&mut self, event: Input, warnings: &Warnings) {
        let Err(dropped) = self.instance.input(&self.device, event) else {
            return;
        };
        let Some(limit) = dropped.limit() else {
            return;
        };
        if !self.told.contains(&dropped) {
            self.told.push(dropped);
            tracing::info!(
                session = self.seat.session,
                slot = self.seat.slot,
                "Nucleus Co-op: {limit}"
            );
            warnings.set(dropped_code(dropped), limit);
        }
    }
    /// The slot's rumble and other feedback, for the player's controller 0.
    pub fn feedback(&self, h: &Host) -> Vec<PadReport> {
        h.nucleus.feedback(self.seat)
    }
    /// Whether feedback of `kind` goes to the device: rumble only where
    /// the stream's settings forward it.
    pub fn feedback_allowed(&self, kind: u16) -> bool {
        !matches!(kind, 0x010b | 0x5500 | 0x5503 | 0x5601) || self.rumble
    }
}
fn dropped_code(dropped: Dropped) -> &'static str {
    match dropped {
        Dropped::KeyboardMouse => "coop_keyboard_mouse",
        _ => "coop_second_controller",
    }
}

/// Ends a co-op player's stream once its session ends: a crash, a failed
/// start, the console's End or every player gone. It runs beside the
/// stream, never on it, and only for co-op launches.
pub struct Watch {
    stop: Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Watch {
    /// None for a launch that is not a co-op player's.
    pub fn start(h: &Shared, s: &Arc<crate::state::Session>) -> Result<Option<Self>> {
        use std::sync::atomic::{AtomicBool, Ordering};
        let Some(seat) = s.launch.nucleus else {
            return Ok(None);
        };
        let stop = Arc::new(AtomicBool::new(false));
        let thread = std::thread::Builder::new().name("nucleus stream".into()).spawn({
            let (h, s, stop) = (h.clone(), s.clone(), stop.clone());
            move || {
                while !stop.load(Ordering::Acquire) && !s.stopping() {
                    let end = match h.nucleus.instance(seat.session) {
                        Some(instance) => instance
                            .end()
                            .map(|end| (instance.session(|s| ended_text(s, &end)), end)),
                        None => Some(("The Nucleus Co-op session is gone".into(), End::Ended)),
                    };
                    if let Some((text, end)) = end {
                        tracing::warn!(session = seat.session, slot = seat.slot, client = %s.launch.client.name, "Nucleus Co-op: {text}");
                        s.launch.warnings.set("coop_ended", &text);
                        match end {
                            End::Crashed(_) | End::Failed(_) => s.fail(),
                            End::Ended | End::Empty => s.stop(),
                        }
                        return;
                    }
                    std::thread::park_timeout(WATCH);
                }
            }
        })?;
        Ok(Some(Self {
            stop,
            thread: Some(thread),
        }))
    }
}
impl Drop for Watch {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Release);
        if let Some(thread) = self.thread.take() {
            thread.thread().unpark();
            if thread.join().is_err() {
                tracing::error!("the Nucleus Co-op stream watch panicked");
            }
        }
    }
}

/// The stream of a co-op launch: its player's game window at the stream's
/// size, once Nucleus Co-op shows it within `wait`. No display is prepared,
/// changed or kept. The capture follows a window Nucleus replaces; a window
/// that is gone is said loudly and never shown as another window or a
/// display.
pub fn prepare(
    h: &Shared,
    launch: &Launch,
    stream: &butterpollo_core::rtsp::Negotiated,
    config: &Config,
    wait: Duration,
) -> Result<crate::display_session::StreamPreparation> {
    let seat = launch
        .nucleus
        .context("the launch has no Nucleus Co-op slot")?;
    let instance = h
        .nucleus
        .instance(seat.session)
        .ok_or_else(|| Refusal::unavailable("The Nucleus Co-op session ended: launch it again"))?;
    instance.resize(seat.slot, (stream.width, stream.height))?;
    // The display only picks the GPU the window is captured on.
    let display = butterpollo_windows::capture::primary_display()
        .context("no display is on the desktop to capture the game window on")?;
    let first = first_window(instance.as_ref(), seat.slot, &display, wait)?;
    tracing::info!(session = seat.session, slot = seat.slot, target = %first, "Nucleus Co-op: streaming the player's game window");
    warn_audio(
        butterpollo_windows::display::windows_build(),
        &launch.warnings,
    );
    crate::display_session::Ready::window(
        h,
        stream,
        config,
        first,
        Box::new(move || instance.capture_target(seat.slot, &display)),
        launch.warnings.clone(),
    )
}

/// The game audio a co-op player hears: its own slot's instance.
pub fn audio(h: &Host, seat: Seat) -> Result<AudioProcess> {
    h.nucleus
        .instance(seat.session)
        .context("the co-op session ended")?
        .audio(seat.slot)
}

/// The player's window follows the stream's new size.
pub fn resize(h: &Host, seat: Seat, size: (u32, u32)) -> Result<()> {
    h.nucleus
        .instance(seat.session)
        .context("the co-op session ended")?
        .resize(seat.slot, size)
}

/// Below the build with process loopback capture, the stream says it has
/// no audio of the player's own game.
fn warn_audio(build: Option<u32>, warnings: &Warnings) {
    if let Some(limit) = butterpollo_windows::nucleus::session::audio_limit(build) {
        warnings.set("coop_audio", limit);
    }
}

/// The setting [`isolate`] marks a co-op player's stream with.
pub const PLAYER: &str = "nucleus_player";
/// A co-op player shares the PC with its user: the stream drives no
/// keyboard or mouse of the desktop, captures only its game's audio, moves
/// no window into focus and leaves the clipboard alone ([`player`]).
pub fn isolate(config: &mut Config) {
    config.values.insert(PLAYER.into(), "true".into());
    for (key, value) in butterpollo_core::background::FORCED {
        config.values.insert(key.into(), value.into());
    }
}
pub fn player(config: &Config) -> bool {
    config.boolean(PLAYER, false)
}

/// Slot `slot`'s first capture target, waiting up to `wait` for its window.
fn first_window(
    instance: &dyn Instance,
    slot: usize,
    display: &str,
    wait: Duration,
) -> Result<String> {
    let deadline = Instant::now() + wait;
    loop {
        if let Some(end) = instance.end() {
            let text = instance.session(|s| ended_text(s, &end));
            return Err(Refusal::unavailable(text).into());
        }
        match instance.capture_target(slot, display) {
            Ok(target) => return Ok(target),
            Err(error) if Instant::now() >= deadline => {
                return Err(Refusal::unavailable(format!(
                    "{error:#}: Nucleus Co-op showed no window for Player {} within {} s",
                    slot + 1,
                    wait.as_secs()
                ))
                .into());
            }
            Err(_) => std::thread::sleep(WATCH),
        }
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
        /// What reading the install fails with (a folder whose ACL is refused).
        pub installed_error: Mutex<Option<String>>,
        /// What an install fails with.
        pub install_error: Mutex<Option<String>>,
        /// Whether an install panics instead.
        pub install_panics: Mutex<bool>,
        /// What an install downloads, checked as the pinned release is;
        /// None installs without a download.
        pub archive: Mutex<Option<Vec<u8>>>,
        /// While true an install waits in the download, until `release`.
        pub hold: Mutex<bool>,
        pub release: std::sync::Condvar,
        /// How often Nucleus Co-op's window was opened.
        pub opened: AtomicUsize,
        /// The windows opened, the last one last.
        pub windows: Mutex<Vec<Arc<Window>>>,
        /// A Nucleus Co-op the host did not start.
        pub foreign: Mutex<Option<u32>>,
        pub catalog: Mutex<Catalog>,
        pub free: Mutex<usize>,
        pub start_error: Mutex<Option<String>>,
        pub started: Mutex<Vec<Arc<Instance>>>,
        /// The Steam app each started session's Nucleus got, in order.
        pub steam_apps: Mutex<Vec<Option<u32>>>,
        pub installs: AtomicUsize,
        pub reads: AtomicUsize,
        /// The game each app's .exe is for Rubylight's own handler, by app
        /// name; an app not here starts its command's .exe, as on the PC.
        pub one_player: Mutex<BTreeMap<String, Setup>>,
        /// Rubylight's own handlers written, the last one last.
        pub own: Mutex<Vec<Setup>>,
        pub write_error: Mutex<Option<String>>,
        /// What [`super::Pc::ready`] fails with.
        pub ready_error: Mutex<Option<String>>,
        /// Rubylight's superseded handler files removed, in order.
        pub removed: Mutex<Vec<String>>,
        /// How often an app's game was looked up for Rubylight's handler.
        pub lookups: AtomicUsize,
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
                installed_error: Mutex::new(None),
                install_error: Mutex::new(None),
                install_panics: Mutex::new(false),
                archive: Mutex::new(None),
                hold: Mutex::new(false),
                release: std::sync::Condvar::new(),
                opened: AtomicUsize::new(0),
                windows: Mutex::new(Vec::new()),
                foreign: Mutex::new(None),
                catalog: Mutex::new(Catalog {
                    handlers: vec![Handler {
                        file: "Portal 2.js".into(),
                        guid: GUID.into(),
                        name: "Portal 2".into(),
                        executable: "portal2.exe".into(),
                        max_players: 2,
                        supports_keyboard: false,
                        hides_taskbar: false,
                        own: false,
                        admin: Vec::new(),
                    }],
                    games: vec![Game {
                        guid: GUID.into(),
                        exe_path: EXE.into(),
                    }],
                }),
                free: Mutex::new(4),
                start_error: Mutex::new(None),
                started: Mutex::new(Vec::new()),
                steam_apps: Mutex::new(Vec::new()),
                installs: AtomicUsize::new(0),
                reads: AtomicUsize::new(0),
                one_player: Mutex::new(BTreeMap::new()),
                own: Mutex::new(Vec::new()),
                write_error: Mutex::new(None),
                ready_error: Mutex::new(None),
                removed: Mutex::new(Vec::new()),
                lookups: AtomicUsize::new(0),
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
            if let Some(error) = self.installed_error.lock().unwrap().clone() {
                anyhow::bail!(error);
            }
            Ok(self.installed.lock().unwrap().clone())
        }
        fn ensure(&self, configured: &str, progress: &(dyn Fn(u64) + Sync)) -> Result<PathBuf> {
            if let Some(folder) = self.installed(configured)? {
                return Ok(folder);
            }
            self.installs.fetch_add(1, Ordering::Relaxed);
            progress(1);
            let mut hold = self.hold.lock().unwrap();
            while *hold {
                hold = self.release.wait(hold).unwrap();
            }
            drop(hold);
            if *self.install_panics.lock().unwrap() {
                panic!("the unpacker broke");
            }
            if let Some(error) = self.install_error.lock().unwrap().clone() {
                anyhow::bail!(error);
            }
            if let Some(archive) = self.archive.lock().unwrap().as_deref() {
                progress(archive.len() as u64);
                butterpollo_core::nucleus::install::verify(archive).map_err(anyhow::Error::msg)?;
            }
            let folder = PathBuf::from(r"C:\Nucleus");
            *self.installed.lock().unwrap() = Some(folder.clone());
            Ok(folder)
        }
        fn open(&self, _folder: &Path) -> Result<Box<dyn super::Window>> {
            let count = self.opened.fetch_add(1, Ordering::Relaxed);
            let window = Arc::new(Window {
                pid: 9000 + count as u32,
                open: AtomicBool::new(true),
                stuck: AtomicBool::new(false),
                unreadable: AtomicBool::new(false),
                restarted_as: Mutex::new(None),
                closes: AtomicUsize::new(0),
            });
            self.windows.lock().unwrap().push(window.clone());
            Ok(Box::new(window))
        }
        fn running(&self) -> Result<Option<u32>> {
            if let Some(pid) = *self.foreign.lock().unwrap() {
                return Ok(Some(pid));
            }
            Ok(self
                .windows
                .lock()
                .unwrap()
                .iter()
                .find(|window| window.open.load(Ordering::Relaxed))
                .map(|window| window.restarted_as.lock().unwrap().unwrap_or(window.pid)))
        }
        fn catalog(&self, _folder: &Path) -> Result<Catalog> {
            self.reads.fetch_add(1, Ordering::Relaxed);
            Ok(self.catalog.lock().unwrap().clone())
        }
        fn free_pads(&self) -> Result<usize> {
            Ok(*self.free.lock().unwrap())
        }
        fn one_player(&self, app: &App) -> Result<Setup, Refusal> {
            self.lookups.fetch_add(1, Ordering::Relaxed);
            if let Some(setup) = self.one_player.lock().unwrap().get(&app.name) {
                return Ok(setup.clone());
            }
            match executable(app).filter(|_| starts_exe(app)) {
                Some(exe) if Path::new(exe).is_absolute() => Ok(Setup {
                    arguments: arguments(app),
                    ..Setup::new(exe, &app.name, None)
                }),
                _ => Err(Refusal::missing(&Missing::NoExecutable {
                    app: app.name.clone(),
                })),
            }
        }
        fn write_own(&self, _folder: &Path, setup: &Setup) -> Result<Handler> {
            if let Some(error) = self.write_error.lock().unwrap().clone() {
                anyhow::bail!(error);
            }
            // As on the PC: never while Nucleus Co-op runs.
            if let Some(pid) = super::Pc::running(self)? {
                anyhow::bail!(butterpollo_windows::nucleus::session::already_running(pid));
            }
            self.own.lock().unwrap().push(setup.clone());
            butterpollo_core::nucleus::handler::parse(&setup.file(), &setup.handler())
                .map_err(anyhow::Error::msg)
        }
        fn remove_own(&self, _folder: &Path, own: &Handler) -> Result<()> {
            self.removed.lock().unwrap().push(own.file.clone());
            Ok(())
        }
        fn ready(&self, _folder: &Path, _own: Option<&Setup>) -> Result<()> {
            match self.ready_error.lock().unwrap().clone() {
                Some(error) => anyhow::bail!(error),
                None => Ok(()),
            }
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
            // As the PC's session refuses while Nucleus Co-op runs.
            if let Some(pid) = super::Pc::running(self)? {
                anyhow::bail!(butterpollo_windows::nucleus::session::already_running(pid));
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
                inputs: Mutex::new(Vec::new()),
                reports: Mutex::new(Vec::new()),
            });
            self.started.lock().unwrap().push(instance.clone());
            self.steam_apps.lock().unwrap().push(start.steam_app);
            Ok(Box::new(instance))
        }
    }

    /// Nucleus Co-op's window as the user would leave it: open until closed,
    /// or `stuck` on a question it asks.
    pub(crate) struct Window {
        pub pid: u32,
        pub open: AtomicBool,
        pub stuck: AtomicBool,
        /// Whether reading it fails.
        pub unreadable: AtomicBool,
        /// The process id it runs as after restarting itself.
        pub restarted_as: Mutex<Option<u32>>,
        /// How often it was asked to close.
        pub closes: AtomicUsize,
    }
    impl Pc {
        /// The window opened last.
        pub(crate) fn window(&self) -> Arc<Window> {
            self.windows.lock().unwrap().last().unwrap().clone()
        }
    }
    impl super::Window for Arc<Window> {
        fn pid(&self) -> u32 {
            self.pid
        }
        fn running(&self) -> Result<bool> {
            anyhow::ensure!(!self.unreadable.load(Ordering::Relaxed), "access denied");
            Ok(self.open.load(Ordering::Relaxed))
        }
        fn close(&self, timeout: Duration) -> Result<()> {
            self.closes.fetch_add(1, Ordering::Relaxed);
            anyhow::ensure!(
                !self.stuck.load(Ordering::Relaxed),
                "Nucleus Co-op (process {}) did not close within {} s: close it on the PC",
                self.pid,
                timeout.as_secs()
            );
            self.open.store(false, Ordering::Relaxed);
            Ok(())
        }
    }

    /// A session that starts as Nucleus does, without windows until
    /// [`Instance::show`]; its pads route as the real ones do.
    pub(crate) struct Instance {
        pub session: Mutex<coop::Session>,
        pub stopped: AtomicBool,
        pub sizes: Mutex<Vec<(u32, u32)>>,
        /// What reached each slot's pad.
        pub inputs: Mutex<Vec<(usize, Input)>>,
        /// The pads' feedback the next read returns.
        pub reports: Mutex<Vec<(usize, PadReport)>>,
    }
    impl Instance {
        pub(crate) fn stopped(&self) -> bool {
            self.stopped.load(Ordering::Relaxed)
        }
        /// Every slot's window is there: slot k's is 100 + k.
        pub(crate) fn show(&self) {
            let mut session = self.session.lock().unwrap();
            let windows: Vec<_> = (0..session.slots.len())
                .map(|k| Some(100 + k as u64))
                .collect();
            session.running(&windows);
        }
    }
    impl super::Instance for Arc<Instance> {
        fn with_session(&self, f: &mut dyn FnMut(&mut coop::Session)) {
            f(&mut self.session.lock().unwrap());
        }
        fn end(&self) -> Option<End> {
            ended(&self.session.lock().unwrap()).cloned()
        }
        fn capture_target(&self, slot: usize, display: &str) -> Result<String> {
            let window = self
                .session
                .lock()
                .unwrap()
                .slots
                .get(slot)
                .context("no such slot")?
                .window
                .with_context(|| format!("Player {}'s game window is not open", slot + 1))?;
            Ok(format!("window:{window}@{display}"))
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
        fn input(&self, device: &str, event: Input) -> Result<(), Dropped> {
            let session = self.session.lock().unwrap();
            let slot = session
                .slot_of(device)
                .filter(|_| session.route(device).is_some())
                .ok_or(Dropped::Ignored)?;
            let event = pads::route(event, slot, session.slots.len())?;
            self.inputs.lock().unwrap().push((slot, event));
            Ok(())
        }
        fn feedback(&self) -> Vec<(usize, PadReport)> {
            std::mem::take(&mut self.reports.lock().unwrap())
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
    use serde_json::json;

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
            "perm": butterpollo_core::state::PLAY_DEVICE_PERMISSION,
        }))
        .unwrap();
        // Set either way: never set, a device that launches apps may join.
        client.extra.insert("coop_join".into(), coop_join.into());
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
            game: Game::Coop(coop),
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
        assert_eq!(coop.entry.name, "Portal 2 (Co-op)");
        assert_eq!(coop.app.name, "Portal 2");
        assert_eq!(coop.id(), app("Portal 2 (Co-op)", "").id());
        // The id follows from the name (its CRC-32), as for every app
        // without its own: the rename changed it, before any release.
        assert_eq!(coop.id(), 242_170_942);
        assert!(coop.entry.cmd.is_empty());
        assert_eq!(
            registry.resolve(&apps, "", coop.id()).unwrap().app.name,
            "Portal 2"
        );
        assert!(registry.resolve(&apps, "", apps[1].id()).is_none());
        // A detached command names the program too.
        let mut detached = app("Portal 2 detached", "");
        detached.extra.insert(
            "detached".into(),
            serde_json::json!([format!("\"{}\"", fake::EXE)]),
        );
        assert_eq!(registry.entries(&[detached], "").len(), 1);
        // A name an app already has is not listed twice.
        let taken = app("Portal 2 (Co-op)", "");
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
        assert_eq!(
            registry.unavailable().as_deref(),
            Some("not on this test PC")
        );
    }

    #[test]
    fn an_app_can_hide_its_coop_entry_and_its_status_stays() {
        let (registry, _pc, mut apps) = setup();
        // As every app switch reads (and the console's flagValue): true, "true", "1" and "yes"
        // are on, any other text is off, and any other value is the default, on.
        for hidden in [
            json!(false),
            json!("false"),
            json!("0"),
            json!("off"),
            json!("TRUE"),
        ] {
            apps[1].extra.insert(ENTRY_KEY.into(), hidden.clone());
            assert!(registry.entries(&apps, "").is_empty(), "{hidden}");
        }
        for shown in [
            json!(true),
            json!("true"),
            json!("1"),
            json!("yes"),
            json!(0),
            json!(null),
        ] {
            apps[1].extra.insert(ENTRY_KEY.into(), shown.clone());
            assert_eq!(registry.entries(&apps, "").len(), 1, "{shown}");
        }
        apps[1].extra.insert(ENTRY_KEY.into(), json!(false));
        let games = registry.games(&apps, &Config::default()).unwrap().unwrap();
        assert_eq!(
            games[1],
            GameStatus::Ready {
                handler: "Portal 2".into(),
                players: 2
            }
        );
        assert!(matches!(
            games[0],
            GameStatus::Missing(Missing::NoExecutable { .. })
        ));
        assert!(matches!(
            &games[2],
            GameStatus::Missing(Missing::NoHandler { .. })
        ));
        assert!(!names_executable(&apps[0]) && names_executable(&apps[1]));
        // A launcher's link names no game .exe: no handler can match it.
        let steam = app(
            "Counter-Strike 2",
            "cmd /c start \"\" steam://rungameid/730",
        );
        assert!(names_executable(&steam) && !starts_exe(&steam));
        assert!(!starts_exe(&apps[2]) && starts_exe(&apps[1]));
        assert!(starts_exe(&app("Puzzle", r"D:\Games\PUZZLE.EXE")));
    }

    #[test]
    fn game_statuses_need_an_install_and_follow_the_player_cap() {
        let (registry, pc, apps) = setup();
        let mut config = Config::default();
        config
            .values
            .insert("nucleus_coop_players".into(), "1".into());
        assert_eq!(
            registry.games(&apps, &config).unwrap().unwrap()[1],
            GameStatus::Ready {
                handler: "Portal 2".into(),
                players: 1
            }
        );
        // Not added in Nucleus Co-op yet.
        pc.catalog.lock().unwrap().games.clear();
        registry.use_pc(pc.clone());
        assert!(matches!(
            &registry.games(&apps, &config).unwrap().unwrap()[1],
            GameStatus::Missing(Missing::NotAdded { .. })
        ));
        *pc.installed.lock().unwrap() = None;
        assert!(registry.games(&apps, &config).is_none());
    }

    #[test]
    fn set_up_installs_once_and_reports_its_progress() {
        let (registry, pc, _apps) = setup();
        assert_eq!(registry.set_up("").unwrap(), SetUp::Installed);
        *pc.installed.lock().unwrap() = None;
        assert_eq!(registry.set_up("").unwrap(), SetUp::Started);
        assert_eq!(registry.install(""), Install::Installing(0));
        assert_eq!(registry.set_up("").unwrap(), SetUp::Installing);
        registry.install_now("");
        assert_eq!(
            registry.install(""),
            Install::Installed(PathBuf::from(r"C:\Nucleus"))
        );
        assert_eq!(pc.installs.load(std::sync::atomic::Ordering::Relaxed), 1);
    }

    #[test]
    fn a_launch_during_set_up_waits_for_that_install_and_shares_its_result() {
        use std::sync::atomic::Ordering::Relaxed;
        // Where panics unwind (tests), a panicking install still ends the
        // wait with its reason; a release build aborts the host instead.
        for (error, panics) in [
            (None, false),
            (Some("the download failed"), false),
            (Some("the install stopped: the unpacker broke"), true),
        ] {
            let (registry, pc, apps) = setup();
            let registry = Arc::new(registry);
            let coop = registry.entries(&apps, "").remove(0);
            *pc.installed.lock().unwrap() = None;
            *pc.install_panics.lock().unwrap() = panics;
            if !panics {
                *pc.install_error.lock().unwrap() = error.map(String::from);
            }
            *pc.hold.lock().unwrap() = true;
            assert_eq!(registry.set_up("").unwrap(), SetUp::Started);
            let console = {
                let registry = registry.clone();
                std::thread::spawn(move || registry.install_now(""))
            };
            while pc.installs.load(Relaxed) == 0 {
                std::thread::sleep(Duration::from_millis(5));
            }
            let launch = {
                let registry = registry.clone();
                std::thread::spawn(move || {
                    admit(&registry, &coop, "a", &client("A", false)).map(|_| ())
                })
            };
            while registry.lock().install_waiters == 0 {
                std::thread::sleep(Duration::from_millis(5));
            }
            // One download, one progress: the launch did not start its own.
            assert_eq!(registry.install(""), Install::Installing(1));
            *pc.hold.lock().unwrap() = false;
            pc.release.notify_all();
            console.join().unwrap();
            let launched = launch.join().unwrap();
            assert_eq!(pc.installs.load(Relaxed), 1, "{error:?}");
            match error {
                None => {
                    launched.unwrap();
                    assert_eq!(
                        registry.install(""),
                        Install::Installed(PathBuf::from(r"C:\Nucleus"))
                    );
                }
                Some(error) => {
                    let refused = launched.unwrap_err();
                    let message = format!("Nucleus Co-op is not ready: {error}");
                    assert_eq!(
                        (refused.status, refused.message.as_str()),
                        (503, message.as_str())
                    );
                    assert_eq!(registry.install(""), Install::Failed(message));
                }
            }
        }
    }

    #[test]
    fn set_up_and_open_are_refused_where_nucleus_cannot_run() {
        let (registry, _pc, _apps) = setup();
        let unavailable = Arc::new(fake::Pc::unavailable());
        *unavailable.installed.lock().unwrap() = None;
        registry.use_pc(unavailable.clone());
        let refused = registry.set_up("").unwrap_err();
        assert_eq!(
            (refused.status, refused.message.as_str()),
            (503, "not on this test PC")
        );
        assert_eq!(registry.install(""), Install::Missing);
        let refused = registry.open("").unwrap_err();
        assert_eq!(
            (refused.status, refused.message.as_str()),
            (503, "not on this test PC")
        );
        assert_eq!(
            unavailable
                .opened
                .load(std::sync::atomic::Ordering::Relaxed),
            0
        );
    }

    #[test]
    fn open_needs_the_install_and_no_running_session() {
        let (registry, pc, apps) = setup();
        *pc.installed.lock().unwrap() = None;
        let refused = registry.open("").unwrap_err();
        assert_eq!(refused.status, 409);
        assert!(refused.message.contains("set up co-op first"), "{refused}");
        *pc.installed.lock().unwrap() = Some(PathBuf::from(r"C:\Nucleus"));
        registry.open("").unwrap();
        assert_eq!(pc.opened.load(std::sync::atomic::Ordering::Relaxed), 1);
        let coop = registry.entries(&apps, "").remove(0);
        admit(&registry, &coop, "a", &client("A", false)).unwrap();
        let refused = registry.open("").unwrap_err();
        assert_eq!(
            (refused.status, refused.message.as_str()),
            (
                409,
                "Nucleus Co-op is running Portal 2: end that session first"
            )
        );
        assert_eq!(pc.opened.load(std::sync::atomic::Ordering::Relaxed), 1);
    }

    #[test]
    fn a_session_closes_the_window_the_console_opened_first() {
        use std::sync::atomic::Ordering::Relaxed;
        let (registry, pc, apps) = setup();
        registry.open("").unwrap();
        assert!(registry.opened());
        let window = pc.window();
        let coop = registry.entries(&apps, "").remove(0);
        admit(&registry, &coop, "a", &client("A", false)).unwrap();
        assert_eq!(window.closes.load(Relaxed), 1);
        assert!(!window.open.load(Relaxed));
        assert!(!registry.opened());
        assert_eq!(pc.started.lock().unwrap().len(), 1);

        // A background session closes it too.
        let (registry, pc, apps) = setup();
        registry.open("").unwrap();
        registry
            .admit(Admission {
                launch: "b",
                client: &client("B", false),
                game: Game::Background(&apps[1]),
                size: (1920, 1080),
                config: &Config::default(),
                warnings: Default::default(),
            })
            .unwrap();
        assert!(!pc.window().open.load(Relaxed));
        assert_eq!(pc.started.lock().unwrap().len(), 1);
    }

    #[test]
    fn a_window_that_wont_close_refuses_the_session_and_stays_open() {
        use std::sync::atomic::Ordering::Relaxed;
        let (registry, pc, apps) = setup();
        registry.open("").unwrap();
        let window = pc.window();
        window.stuck.store(true, Relaxed);
        let coop = registry.entries(&apps, "").remove(0);
        let refused = admit(&registry, &coop, "a", &client("A", false)).unwrap_err();
        assert_eq!(
            (refused.status, refused.message.as_str()),
            (
                503,
                "Nucleus Co-op did not start Portal 2: Nucleus Co-op (process 9000) did not close within 5 s: close it on the PC"
            )
        );
        // Asked to close, never ended.
        assert_eq!(window.closes.load(Relaxed), 1);
        assert!(window.open.load(Relaxed));
        assert!(registry.opened());
        assert!(pc.started.lock().unwrap().is_empty());
        // Once the user closes it at the PC, the next launch starts.
        window.stuck.store(false, Relaxed);
        window.open.store(false, Relaxed);
        admit(&registry, &coop, "a", &client("A", false)).unwrap();
        assert_eq!(window.closes.load(Relaxed), 1);
    }

    #[test]
    fn a_launch_refused_for_another_reason_leaves_the_window_open() {
        use std::sync::atomic::Ordering::Relaxed;
        let (registry, pc, apps) = setup();
        registry.open("").unwrap();
        let window = pc.window();
        *pc.free.lock().unwrap() = 0;
        let coop = registry.entries(&apps, "").remove(0);
        admit(&registry, &coop, "a", &client("A", false)).unwrap_err();
        assert_eq!(window.closes.load(Relaxed), 0);
        assert!(registry.opened());
        assert!(pc.started.lock().unwrap().is_empty());
    }

    #[test]
    fn a_launch_the_pc_cant_start_leaves_the_window_open() {
        // Refused for administrator rights or a handler file that isn't
        // ours only after the console's Nucleus window had closed.
        use std::sync::atomic::Ordering::Relaxed;
        let (registry, pc, apps) = setup();
        registry.open("").unwrap();
        let window = pc.window();
        *pc.ready_error.lock().unwrap() = Some("no administrator rights".into());
        let coop = registry.entries(&apps, "").remove(0);
        let refused = admit(&registry, &coop, "a", &client("A", false)).unwrap_err();
        assert_eq!(
            (refused.status, refused.message.as_str()),
            (
                503,
                "Nucleus Co-op did not start Portal 2: no administrator rights"
            )
        );
        let ice = app("Ice", r"C:\NoSuchGames\Ice\Ice.exe");
        let refused = background(
            &registry,
            &ice,
            "b",
            &client("B", false),
            &Config::default(),
        )
        .unwrap_err();
        assert!(
            refused.message.ends_with("no administrator rights"),
            "{refused}"
        );
        assert_eq!(window.closes.load(Relaxed), 0);
        assert!(registry.opened());
        assert!(pc.own.lock().unwrap().is_empty());
        assert!(pc.started.lock().unwrap().is_empty());
    }

    #[test]
    fn a_window_that_cant_be_read_refuses_the_session_with_why() {
        use std::sync::atomic::Ordering::Relaxed;
        let (registry, pc, apps) = setup();
        registry.open("").unwrap();
        let window = pc.window();
        window.unreadable.store(true, Relaxed);
        let coop = registry.entries(&apps, "").remove(0);
        let refused = admit(&registry, &coop, "a", &client("A", false)).unwrap_err();
        assert_eq!(
            (refused.status, refused.message.as_str()),
            (
                503,
                "Nucleus Co-op did not start Portal 2: Nucleus Co-op's window (process 9000) could not be read: access denied"
            )
        );
        assert_eq!(window.closes.load(Relaxed), 0);
        assert!(pc.started.lock().unwrap().is_empty());
    }

    #[test]
    fn open_is_refused_while_a_launch_is_being_admitted() {
        use std::sync::atomic::Ordering::Relaxed;
        let (registry, pc, apps) = setup();
        let registry = Arc::new(registry);
        let coop = registry.entries(&apps, "").remove(0);
        *pc.installed.lock().unwrap() = None;
        *pc.hold.lock().unwrap() = true;
        let launch = {
            let registry = registry.clone();
            std::thread::spawn(move || {
                admit(&registry, &coop, "a", &client("A", false)).map(|_| ())
            })
        };
        while pc.installs.load(Relaxed) == 0 {
            std::thread::sleep(Duration::from_millis(5));
        }
        // The launch holds the admission while it installs; to Open, Nucleus
        // Co-op is there already.
        *pc.installed.lock().unwrap() = Some(PathBuf::from(r"C:\Nucleus"));
        let refused = registry.open("").unwrap_err();
        assert_eq!(
            (refused.status, refused.message.as_str()),
            (
                409,
                "A co-op or background stream is starting: open Nucleus Co-op once it has"
            )
        );
        assert_eq!(pc.opened.load(Relaxed), 0);
        *pc.hold.lock().unwrap() = false;
        pc.release.notify_all();
        launch.join().unwrap().unwrap();
    }

    #[test]
    fn a_nucleus_that_restarted_itself_is_still_the_one_the_console_opened() {
        use std::sync::atomic::Ordering::Relaxed;
        let (registry, pc, _apps) = setup();
        registry.open("").unwrap();
        let window = pc.window();
        *window.restarted_as.lock().unwrap() = Some(9500);
        let refused = registry.open("").unwrap_err();
        assert_eq!(
            (refused.status, refused.message.as_str()),
            (
                409,
                "Nucleus Co-op is already open on the PC's screen: use it there, or close it here"
            )
        );
        assert!(registry.close().unwrap());
        assert_eq!(window.closes.load(Relaxed), 1);
        assert!(!registry.opened());
    }

    #[test]
    fn open_and_close_leave_a_nucleus_the_host_did_not_start_alone() {
        use std::sync::atomic::Ordering::Relaxed;
        let (registry, pc, apps) = setup();
        *pc.foreign.lock().unwrap() = Some(77);
        let refused = registry.open("").unwrap_err();
        assert_eq!(
            (refused.status, refused.message.as_str()),
            (
                409,
                "Nucleus Co-op is already running on the PC (process 77): close it there first"
            )
        );
        assert_eq!(pc.opened.load(Relaxed), 0);
        let refused = registry.close().unwrap_err();
        assert_eq!(
            (refused.status, refused.message.as_str()),
            (
                409,
                "Nucleus Co-op (process 77) was not opened from this console: close it on the PC"
            )
        );
        // A launch is refused as before, loudly.
        let coop = registry.entries(&apps, "").remove(0);
        let refused = admit(&registry, &coop, "a", &client("A", false)).unwrap_err();
        assert!(
            refused
                .message
                .ends_with("Nucleus Co-op is already running on the PC (process 77): close it, then start the stream again"),
            "{refused}"
        );
    }

    #[test]
    fn open_twice_is_refused_and_close_closes_it_once() {
        use std::sync::atomic::Ordering::Relaxed;
        let (registry, pc, _apps) = setup();
        assert!(!registry.close().unwrap(), "nothing to close");
        registry.open("").unwrap();
        let refused = registry.open("").unwrap_err();
        assert_eq!(
            (refused.status, refused.message.as_str()),
            (
                409,
                "Nucleus Co-op is already open on the PC's screen: use it there, or close it here"
            )
        );
        assert_eq!(pc.opened.load(Relaxed), 1);
        // A question it asks keeps it open, and says so.
        pc.window().stuck.store(true, Relaxed);
        let refused = registry.close().unwrap_err();
        assert_eq!(refused.status, 409);
        assert!(refused.message.contains("did not close"), "{refused}");
        assert!(registry.opened());
        pc.window().stuck.store(false, Relaxed);
        assert!(registry.close().unwrap());
        assert!(!registry.opened());
        assert!(!registry.close().unwrap());
        // Closed, it opens again.
        registry.open("").unwrap();
        assert_eq!(pc.opened.load(Relaxed), 2);
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
        assert!(instance.capture_target(1, "d").is_err());
        pc.last().show();
        assert_eq!(instance.capture_target(1, "d").unwrap(), "window:101@d");
        assert!(registry.instance(2).is_none());
    }

    fn background(
        registry: &Registry,
        app: &App,
        launch: &str,
        client: &Client,
        config: &Config,
    ) -> Result<Seat, Refusal> {
        registry.admit(Admission {
            launch,
            client,
            game: Game::Background(app),
            size: (1280, 720),
            config,
            warnings: Default::default(),
        })
    }

    #[test]
    fn a_background_stream_is_a_private_session_its_owner_returns_to() {
        let (registry, pc, apps) = setup();
        let owner = client("A", true);
        let config = Config::default();
        let seat = background(&registry, &apps[1], "a", &owner, &config).unwrap();
        assert_eq!(
            seat,
            Seat {
                session: 1,
                slot: 0
            }
        );
        let listed = registry.sessions();
        assert_eq!(listed[0].session.kind, Kind::Background);
        // One player whatever the handler allows, listed under the app.
        assert_eq!(listed[0].session.slots.len(), 1);
        assert_eq!(listed[0].app_id, apps[1].id());
        assert_eq!(pc.last().sizes.lock().unwrap()[..], [(1280, 720)]);
        // Nobody else joins it, by the app or by its co-op entry.
        let other = background(&registry, &apps[1], "b", &client("B", true), &config).unwrap_err();
        assert_eq!(other.status, 409);
        assert!(
            other
                .message
                .starts_with("Portal 2 can't stream in the background: Nucleus Co-op is running Portal 2 for a background stream"),
            "{}",
            other.message
        );
        let coop = registry.entries(&apps, "").remove(0);
        assert_eq!(
            admit(&registry, &coop, "c", &client("B", true))
                .unwrap_err()
                .status,
            409
        );
        // Nor from a session list: its id is no co-op session, for anyone.
        assert_eq!(
            registry.join(seat.session, &client("B", true)),
            Err(JoinError::NoSession)
        );
        assert_eq!(
            registry.join(seat.session, &owner),
            Err(JoinError::NoSession)
        );
        // Its owner returns to its own slot of the same session.
        registry.leave(seat, "a");
        assert_eq!(
            background(&registry, &apps[1], "d", &owner, &config).unwrap(),
            seat
        );
        assert_eq!(pc.started.lock().unwrap().len(), 1);
    }

    #[test]
    fn a_background_stream_nucleus_cannot_run_is_refused_with_the_reason() {
        let (registry, pc, apps) = setup();
        let owner = client("A", false);
        let config = Config::default();
        // A launcher's link, or no program at all.
        let store = app(
            "Store Game",
            "com.epicgames.launcher://apps/x?action=launch",
        );
        let missing = background(&registry, &store, "a", &owner, &config).unwrap_err();
        assert_eq!(missing.status, 404);
        assert_eq!(
            missing.message,
            "Store Game can't stream in the background: Nucleus Co-op starts games by their .exe, and Store Game does not name one: set the app's command to the game's .exe"
        );
        let desktop = background(&registry, &apps[0], "a", &owner, &config).unwrap_err();
        assert_eq!(desktop.status, 404);
        assert!(
            desktop.message.starts_with(
                "Desktop can't stream in the background: Nucleus Co-op starts games by their .exe"
            ),
            "{}",
            desktop.message
        );
        // Windows' own program, or a game on a network share, which the
        // host as SYSTEM would open: refused before anything reads it.
        let notepad = app("Notepad", r"C:\Windows\notepad.exe");
        let refused = background(&registry, &notepad, "a", &owner, &config).unwrap_err();
        assert_eq!(refused.status, 404);
        assert_eq!(
            refused.message,
            r"Notepad can't stream in the background: C:\Windows\notepad.exe is part of Windows, not a game"
        );
        let shared = app("Shared", r"\\nas\games\Ice\Ice.exe");
        let refused = background(&registry, &shared, "a", &owner, &config).unwrap_err();
        assert!(
            refused.message.ends_with(
                "is on a network share: Rubylight starts only a game on this PC's own drives"
            ),
            "{refused}"
        );
        assert!(pc.own.lock().unwrap().is_empty());
        // A background_mode that is no mode runs none.
        let bad = Config::parse("background_mode = rdp\n").unwrap();
        let refused = background(&registry, &apps[1], "a", &owner, &bad).unwrap_err();
        assert_eq!(refused.status, 503);
        assert!(
            refused
                .message
                .contains("background_mode = rdp is not a mode"),
            "{}",
            refused.message
        );
        // Values earlier builds wrote run in Nucleus Co-op.
        for old in ["auto", "child_session", "virtual_display"] {
            let config = Config::parse(&format!("background_mode = {old}\n")).unwrap();
            let seat = background(&registry, &apps[1], old, &owner, &config).unwrap();
            assert!(registry.end(seat.session), "{old}");
        }
        // Where Nucleus Co-op can't run, the reason.
        registry.use_pc(Arc::new(fake::Pc::unavailable()));
        let unavailable = background(&registry, &apps[1], "a", &owner, &config).unwrap_err();
        assert_eq!(unavailable.status, 503);
        assert_eq!(
            unavailable.message,
            "Portal 2 can't stream in the background: not on this test PC"
        );
        assert_eq!(pc.started.lock().unwrap().len(), 3);
    }

    #[test]
    fn a_one_player_game_without_a_hub_handler_streams_in_the_background_with_ours() {
        let (registry, pc, mut apps) = setup();
        let owner = client("A", false);
        let config = Config::default();
        apps.push(app("Ice", r#""C:\NoSuchGames\Ice\Ice.exe" -windowed"#));
        let seat = background(&registry, &apps[3], "a", &owner, &config).unwrap();
        let own = pc.own.lock().unwrap().clone();
        assert_eq!(own.len(), 1);
        assert_eq!(own[0].exe_path, r"C:\NoSuchGames\Ice\Ice.exe");
        assert_eq!(own[0].name, "Ice");
        // The game starts with the app's arguments.
        assert_eq!(own[0].arguments, "-windowed");
        // One player, our handler's game, listed under the app.
        let listed = registry.sessions();
        assert_eq!(listed[0].session.kind, Kind::Background);
        assert_eq!(listed[0].session.slots.len(), 1);
        assert_eq!(listed[0].session.game, own[0].guid());
        assert_eq!(listed[0].app_id, apps[3].id());
        // Its owner returns to it without another write, Steam read or
        // folder scan.
        assert_eq!(pc.lookups.load(std::sync::atomic::Ordering::Relaxed), 1);
        registry.leave(seat, "a");
        assert_eq!(
            background(&registry, &apps[3], "b", &owner, &config).unwrap(),
            seat
        );
        assert_eq!(pc.lookups.load(std::sync::atomic::Ordering::Relaxed), 1);
        assert_eq!(pc.own.lock().unwrap().len(), 1);
        assert_eq!(pc.started.lock().unwrap().len(), 1);
        // Co-op still needs a hub handler: no entry, and a co-op launch is refused.
        assert!(
            registry
                .entries(&apps, "")
                .iter()
                .all(|coop| coop.app.name != "Ice")
        );
        let coop = CoopApp {
            app: apps[3].clone(),
            ..registry.entries(&apps, "").remove(0)
        };
        let refused = admit(&registry, &coop, "c", &client("B", true)).unwrap_err();
        assert_eq!(refused.status, 404);
        assert!(
            refused
                .message
                .starts_with("No Nucleus Co-op handler for Ice.exe"),
            "{refused}"
        );
    }

    #[test]
    fn a_steam_game_with_ours_starts_nucleus_with_its_app_id() {
        let (registry, pc, _) = setup();
        let owner = client("A", false);
        let config = Config::default();
        let link = app("Black Flag", "steam://rungameid/242050");
        pc.one_player.lock().unwrap().insert(
            link.name.clone(),
            Setup::new(
                r"D:\Steam\steamapps\common\Black Flag\AC4BFSP.exe",
                &link.name,
                Some((242050, r"D:\Steam\steamapps\common\Black Flag")),
            ),
        );
        background(&registry, &link, "a", &owner, &config).unwrap();
        assert_eq!(pc.own.lock().unwrap().len(), 1);
        assert_eq!(*pc.steam_apps.lock().unwrap(), [Some(242050)]);
        // A game started by its own .exe gets none.
        let ice = app("Ice", r"C:\NoSuchGames\Ice\Ice.exe");
        let first = registry.sessions()[0].session.id;
        assert!(registry.end(first));
        background(&registry, &ice, "b", &owner, &config).unwrap();
        assert_eq!(*pc.steam_apps.lock().unwrap(), [Some(242050), None]);
    }

    #[test]
    fn a_hub_handler_always_wins_over_ours() {
        let (registry, pc, apps) = setup();
        let owner = client("A", false);
        let config = Config::default();
        // Portal 2 has the hub's handler and the user added it.
        let seat = background(&registry, &apps[1], "a", &owner, &config).unwrap();
        assert_eq!(registry.sessions()[0].session.game, fake::GUID);
        assert!(pc.own.lock().unwrap().is_empty());
        assert!(registry.end(seat.session));
        // Found from a link rather than the app's command: still the hub's.
        let link = app("Portal 2 Link", "steam://rungameid/620");
        pc.one_player.lock().unwrap().insert(
            link.name.clone(),
            Setup::new(fake::EXE, &link.name, Some((620, r"C:\Games\Portal 2"))),
        );
        background(&registry, &link, "b", &owner, &config).unwrap();
        assert_eq!(pc.last().session.lock().unwrap().game, fake::GUID);
        assert!(pc.own.lock().unwrap().is_empty());
        // A hub handler sets up Steam as it needs: Nucleus gets no app id.
        assert_eq!(*pc.steam_apps.lock().unwrap(), [None, None]);
        // A hub handler for a game the user did not add yet: add it, not ours.
        let other = registry.sessions()[0].session.id;
        assert!(registry.end(other));
        pc.catalog.lock().unwrap().games.clear();
        let refused = background(&registry, &apps[1], "c", &owner, &config).unwrap_err();
        assert_eq!(refused.status, 404);
        assert!(
            refused.message.contains("the game is not added"),
            "{refused}"
        );
        assert!(pc.own.lock().unwrap().is_empty());
    }

    #[test]
    fn ours_goes_once_the_user_added_the_game_with_a_hub_handler() {
        let (registry, pc, apps) = setup();
        let owner = client("A", false);
        let config = Config::default();
        // Ours from before the hub's handler came, for the same .exe.
        let ours = Setup::new(fake::EXE, "Portal 2", None);
        {
            let mut catalog = pc.catalog.lock().unwrap();
            let hub = catalog.handlers[0].clone();
            catalog.handlers.push(Handler {
                file: ours.file(),
                guid: ours.guid(),
                own: true,
                ..hub
            });
            catalog
                .games
                .push(butterpollo_core::nucleus::handler::Game {
                    guid: ours.guid(),
                    exe_path: fake::EXE.into(),
                });
        }
        // A refused launch removes nothing: Nucleus' window stays open.
        *pc.free.lock().unwrap() = 0;
        background(&registry, &apps[1], "a", &owner, &config).unwrap_err();
        assert!(pc.removed.lock().unwrap().is_empty());
        *pc.free.lock().unwrap() = 4;
        background(&registry, &apps[1], "a", &owner, &config).unwrap();
        assert_eq!(registry.sessions()[0].session.game, fake::GUID);
        assert_eq!(*pc.removed.lock().unwrap(), [ours.file()]);
    }

    #[test]
    fn a_launcher_is_refused_and_an_unreal_stub_starts_its_game() {
        let (registry, pc, _) = setup();
        let owner = client("A", false);
        let config = Config::default();
        // Steam would start the game outside Nucleus' hooks.
        let steam = app(
            "Portal",
            r#""C:\Program Files (x86)\Steam\steam.exe" -applaunch 400"#,
        );
        let refused = background(&registry, &steam, "a", &owner, &config).unwrap_err();
        assert_eq!(refused.status, 404);
        assert_eq!(
            refused.message,
            "Portal can't stream in the background: steam.exe is a launcher, not the game: set the app's command to the game's .exe"
        );
        assert!(pc.own.lock().unwrap().is_empty());
        // The stub at an Unreal game's root only starts its Shipping .exe.
        let root = std::env::temp_dir().join(format!("nucleus-ue-{}", uuid::Uuid::new_v4()));
        let win64 = root.join("Stray").join("Binaries").join("Win64");
        std::fs::create_dir_all(&win64).unwrap();
        std::fs::create_dir_all(root.join("Engine")).unwrap();
        std::fs::write(win64.join("Stray-Win64-Shipping.exe"), b"").unwrap();
        let stub = root.join("Stray.exe");
        std::fs::write(&stub, b"").unwrap();
        let stray = app("Stray", &format!("\"{}\" -dx12", stub.display()));
        let started = background(&registry, &stray, "b", &owner, &config);
        std::fs::remove_dir_all(&root).unwrap();
        started.unwrap();
        let own = pc.own.lock().unwrap().clone();
        assert_eq!(
            own[0].exe_path,
            win64.join("Stray-Win64-Shipping.exe").to_string_lossy()
        );
        assert_eq!(own[0].arguments, "-dx12");
    }

    #[test]
    fn a_link_to_another_game_of_the_same_file_name_streams_with_ours() {
        // Portal's hl2.exe found Half-Life 2's handler by its file name.
        let (registry, pc, _) = setup();
        let owner = client("A", false);
        let config = Config::default();
        let link = app("Portal 2 Mod", "steam://rungameid/12345");
        let exe = r"C:\Games\Portal 2 Mod\portal2.exe";
        pc.one_player.lock().unwrap().insert(
            link.name.clone(),
            Setup::new(exe, &link.name, Some((12345, r"C:\Games\Portal 2 Mod"))),
        );
        background(&registry, &link, "a", &owner, &config).unwrap();
        let own = pc.own.lock().unwrap().clone();
        assert_eq!(own.len(), 1);
        assert_eq!(own[0].exe_path, exe);
        assert_eq!(registry.sessions()[0].session.game, own[0].guid());
    }

    #[test]
    fn a_background_stream_of_an_anti_cheat_game_is_refused_and_never_hooked() {
        let (registry, pc, _) = setup();
        let owner = client("A", false);
        let config = Config::default();
        let dir = std::env::temp_dir().join(format!("nucleus-ac-{}", uuid::Uuid::new_v4()));
        let game = dir.join("Shooter");
        std::fs::create_dir_all(game.join("EasyAntiCheat")).unwrap();
        let shooter = app(
            "Shooter",
            &format!("\"{}\"", game.join("Shooter.exe").display()),
        );
        let refused = background(&registry, &shooter, "a", &owner, &config).unwrap_err();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(refused.status, 404);
        assert_eq!(
            refused.message,
            "Shooter can't stream in the background: Shooter.exe uses Easy Anti-Cheat, and Rubylight never hooks a game with anti-cheat"
        );
        assert!(pc.own.lock().unwrap().is_empty());
        assert!(pc.started.lock().unwrap().is_empty());
        // Steam lists Valve Anti-Cheat for a game whose files say nothing.
        let link = app("Portal", "steam://rungameid/400");
        pc.one_player.lock().unwrap().insert(
            link.name.clone(),
            Setup {
                anti_cheat: Some("Valve Anti-Cheat"),
                ..Setup::new(
                    r"Z:\NoSuch\common\Portal\hl2.exe",
                    &link.name,
                    Some((400, r"Z:\NoSuch\common\Portal")),
                )
            },
        );
        let refused = background(&registry, &link, "b", &owner, &config).unwrap_err();
        assert_eq!(
            refused.message,
            "Portal can't stream in the background: hl2.exe uses Valve Anti-Cheat, and Rubylight never hooks a game with anti-cheat"
        );
        assert!(pc.own.lock().unwrap().is_empty());
    }

    #[test]
    fn our_handler_is_never_written_while_nucleus_runs_or_when_it_fails() {
        let (registry, pc, _) = setup();
        let owner = client("A", false);
        let config = Config::default();
        let ice = app("Ice", r"C:\NoSuchGames\Ice\Ice.exe");
        *pc.foreign.lock().unwrap() = Some(77);
        let refused = background(&registry, &ice, "a", &owner, &config).unwrap_err();
        assert_eq!(refused.status, 503);
        assert!(
            refused
                .message
                .starts_with("Ice can't stream in the background: Rubylight's Nucleus Co-op handler for Ice was not written: "),
            "{refused}"
        );
        assert!(pc.own.lock().unwrap().is_empty());
        *pc.foreign.lock().unwrap() = None;
        *pc.write_error.lock().unwrap() = Some("access denied".into());
        let refused = background(&registry, &ice, "b", &owner, &config).unwrap_err();
        assert!(
            refused.message.ends_with("was not written: access denied"),
            "{refused}"
        );
        assert!(pc.started.lock().unwrap().is_empty());
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
        assert!(
            missing.message.contains("the game is not added"),
            "{missing}"
        );
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
        // Join co-op taken away: the player cannot come back to its slot,
        // from the list or by launching the entry; the owner still can.
        let revoked = client("B", false);
        assert_eq!(
            registry.join(1, &revoked),
            Err(JoinError::Refused(Refused::NotPermitted))
        );
        assert_eq!(
            admit(&registry, &coop, "b2", &revoked).unwrap_err(),
            Refusal::busy(Refused::NotPermitted.to_string())
        );
        assert_eq!(registry.join(1, &client("A", false)), Ok((0, coop.id())));
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

    fn pad(id: u16, active: u16, buttons: u32) -> Input {
        Input::Controller {
            id,
            active,
            buttons,
            left_trigger: 0,
            right_trigger: 0,
            sticks: [0; 4],
        }
    }
    fn codes(warnings: &Warnings) -> Vec<String> {
        warnings.snapshot().into_iter().map(|w| w.code).collect()
    }

    #[test]
    fn a_player_drives_its_slot_and_hears_of_each_dropped_kind_once() {
        let (registry, pc, apps) = setup();
        let coop = registry.entries(&apps, "").remove(0);
        admit(&registry, &coop, "a", &client("A", false)).unwrap();
        let seat = admit(&registry, &coop, "b", &client("B", true)).unwrap();
        pc.last().show();
        let mut player = Player {
            instance: registry.instance(seat.session).unwrap(),
            seat,
            device: "B".into(),
            rumble: true,
            told: Vec::new(),
        };
        let warnings = Warnings::default();
        player.input(pad(0, 1, 0x1000), &warnings);
        let key = Input::Keyboard {
            key: 0x41,
            modifiers: 0,
            flags: 0,
            down: true,
        };
        player.input(key.clone(), &warnings);
        player.input(key, &warnings);
        assert_eq!(codes(&warnings), ["coop_keyboard_mouse"]);
        player.input(pad(1, 0b11, 0x1000), &warnings);
        player.input(pad(1, 0b11, 0), &warnings);
        assert_eq!(
            codes(&warnings),
            ["coop_keyboard_mouse", "coop_second_controller"]
        );
        assert_eq!(player.told.len(), 2);
        // Only the first controller reached the pad, as slot 1's.
        let inputs = pc.last().inputs.lock().unwrap().clone();
        assert_eq!(inputs.len(), 1);
        assert_eq!(inputs[0].0, 1);
        assert!(matches!(
            inputs[0].1,
            Input::Controller {
                buttons: 0x1000,
                ..
            }
        ));
    }

    #[test]
    fn rumble_reaches_a_player_only_where_its_stream_forwards_it() {
        let (registry, pc, apps) = setup();
        let coop = registry.entries(&apps, "").remove(0);
        let seat = admit(&registry, &coop, "a", &client("A", false)).unwrap();
        let mut player = Player {
            instance: registry.instance(seat.session).unwrap(),
            seat,
            device: "A".into(),
            rumble: false,
            told: Vec::new(),
        };
        assert!(!player.feedback_allowed(0x010b));
        assert!(player.feedback_allowed(0x5502));
        player.rumble = true;
        assert!(player.feedback_allowed(0x010b));
        drop(pc);
    }

    #[test]
    fn each_slot_reads_its_own_feedback_whoever_drains_the_pads() {
        let (registry, pc, apps) = setup();
        let coop = registry.entries(&apps, "").remove(0);
        let a = admit(&registry, &coop, "a", &client("A", false)).unwrap();
        let b = admit(&registry, &coop, "b", &client("B", true)).unwrap();
        let rumble = |id, data: u8| PadReport::Feedback {
            id,
            kind: 0x010b,
            data: vec![data],
        };
        pc.last()
            .reports
            .lock()
            .unwrap()
            .extend([(0, rumble(0, 1)), (1, rumble(0, 2))]);
        let data = |reports: Vec<PadReport>| -> Vec<u8> {
            reports
                .into_iter()
                .filter_map(|r| match r {
                    PadReport::Feedback { data, .. } => Some(data[0]),
                    _ => None,
                })
                .collect()
        };
        assert_eq!(data(registry.feedback(a)), [1]);
        // Slot 1's report waited for its own stream.
        assert_eq!(data(registry.feedback(b)), [2]);
        assert!(registry.feedback(b).is_empty());
        // A stream that stopped reading keeps only the newest reports.
        pc.last()
            .reports
            .lock()
            .unwrap()
            .extend((0..FEEDBACK_KEPT as u8 + 4).map(|k| (1, rumble(0, k))));
        assert!(registry.feedback(a).is_empty());
        let kept = data(registry.feedback(b));
        assert_eq!(kept.len(), FEEDBACK_KEPT);
        assert_eq!(kept[0], 4);
    }

    #[test]
    fn the_first_window_is_waited_for_and_its_absence_is_refused() {
        let (registry, pc, apps) = setup();
        let coop = registry.entries(&apps, "").remove(0);
        let seat = admit(&registry, &coop, "a", &client("A", false)).unwrap();
        let instance = registry.instance(seat.session).unwrap();
        let none = first_window(instance.as_ref(), 0, "d", Duration::ZERO).unwrap_err();
        assert!(
            format!("{none:#}").contains("showed no window for Player 1 within 0 s"),
            "{none:#}"
        );
        assert_eq!(none.downcast_ref::<Refusal>().unwrap().status, 503);
        let shown = std::thread::spawn({
            let session = pc.last();
            move || {
                std::thread::sleep(Duration::from_millis(50));
                session.show();
            }
        });
        let first = first_window(instance.as_ref(), 0, "d", Duration::from_secs(10)).unwrap();
        assert_eq!(first, "window:100@d");
        shown.join().unwrap();
        pc.last()
            .session
            .lock()
            .unwrap()
            .end(End::Crashed("gone".into()));
        let ended = first_window(instance.as_ref(), 0, "d", Duration::from_secs(10)).unwrap_err();
        assert!(
            format!("{ended:#}").contains("co-op session ended: "),
            "{ended:#}"
        );
    }

    #[test]
    fn a_player_stream_leaves_the_desktop_input_and_clipboard_alone() {
        let mut config = Config::default();
        assert!(!player(&config));
        isolate(&mut config);
        assert!(player(&config));
        assert!(!config.boolean("keyboard", true));
        assert!(!config.boolean("mouse", true));
        assert_eq!(config.get("audio_capture_mode", "system"), "game");
        // Not a background stream: no isolated display is made for it.
        assert!(!butterpollo_core::background::enabled(&config));
        let mut owner = client("A", false);
        owner.perm = u32::MAX;
        assert!(!crate::stream::clipboard_sync::allowed(&config, &owner).any());
    }

    #[test]
    fn the_stream_says_when_windows_cannot_capture_a_players_audio() {
        let warnings = Warnings::default();
        warn_audio(None, &warnings);
        warn_audio(Some(22631), &warnings);
        assert!(codes(&warnings).is_empty());
        warn_audio(Some(19045), &warnings);
        assert_eq!(codes(&warnings), ["coop_audio"]);
        assert!(warnings.snapshot()[0].message.contains("20348"));
    }
}
