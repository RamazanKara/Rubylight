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
use anyhow::{Context, Result, ensure};
use butterpollo_core::config::Config;
use butterpollo_core::input::Input;
use butterpollo_core::input_policy::Policy;
use butterpollo_core::nucleus::coop::{self, Admit, End, Kind, Phase, Refused};
use butterpollo_core::nucleus::elevation::{self, Account, Need, Run, Script};
use butterpollo_core::nucleus::generic::{self, Setup};
use butterpollo_core::nucleus::handler::{self, Catalog, Handler, Match, Missing};
use butterpollo_core::nucleus::mode;
use butterpollo_core::session::{Seat, Warnings};
use butterpollo_core::state::{App, Client};
use butterpollo_windows::input::PadReport;
use butterpollo_windows::nucleus::pads::{self, Dropped};
use butterpollo_windows::nucleus::session::{Opened, Running, Start};
use butterpollo_windows::nucleus::{files, install, user_copy};
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
/// How long the console's answer for one .exe ([`own_status`]) stands while
/// the .exe is unchanged: the console asks every few seconds, and each
/// answer scans the game's folders.
const OWN_STATUS_TTL: Duration = Duration::from_secs(60);
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
/// The stream's warning when Rubylight's replaced or unused handler stays.
const HANDLER_NOT_REMOVED: &str = "nucleus_handler_not_removed";

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
    /// The handlers and the games in the game list of the Nucleus at `folder`.
    fn catalog(&self, folder: &Path) -> Result<Catalog>;
    /// The XInput slots the PC's own controllers leave free.
    fn free_pads(&self) -> Result<usize>;
    /// The game of `app` for Rubylight's own one-player handler: its .exe on
    /// this PC, for a Steam link the one Steam starts. Another launcher's
    /// link names none.
    fn one_player(&self, app: &App) -> Result<Setup, Refusal>;
    /// What Windows gives the signed-in user to start Nucleus Co-op
    /// elevated with: an elevated token, or why none.
    fn account(&self) -> Result<Account>;
    /// What the game at `exe` asks of Windows to start; with `folder`, also
    /// whether the signed-in user may change its folder's files as Nucleus
    /// does (a walk of the folder: at launch, not for the console).
    fn game_need(&self, exe: &str, folder: bool) -> Result<Need>;
    /// The signed-in user's own copy of the Nucleus at `folder`, made again
    /// when it isn't that one's program
    /// ([`butterpollo_windows::nucleus::user_copy`]).
    fn user_copy(&self, folder: &Path) -> Result<PathBuf>;
    /// Write Rubylight's one-player handler for `setup` into the Nucleus at
    /// `folder` and add its game to Nucleus' game list; with the user's own
    /// copy at `copy`, then give that copy `folder`'s handlers and add the
    /// game to its list, as the user. Refused while Nucleus Co-op runs.
    fn write_own(&self, folder: &Path, copy: Option<&Path>, setup: &Setup) -> Result<Handler>;
    /// Remove Rubylight's handler `own`, which a hub handler replaces or no
    /// game uses ([`Catalog::stale`]), and its game, also from the list of
    /// the user's own copy at `copy`, as the user; refused while Nucleus
    /// Co-op runs.
    fn remove_own(&self, folder: &Path, copy: Option<&Path>, own: &Handler) -> Result<()>;
    /// What a session's start needs that doesn't need Nucleus Co-op closed
    /// (administrator rights when it runs elevated, the signed-in user, the
    /// compatibility settings an earlier session left put back, `own`'s
    /// handler file being Rubylight's, a game list that takes `own`'s game
    /// and loses `stale`'s): checked before the console's window closes for
    /// it.
    fn ready(&self, folder: &Path, run: Run, own: Option<&Setup>, stale: &[&Handler])
    -> Result<()>;
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
        // A nucleus_path copy may be a release that reads the AppData list.
        let installed =
            butterpollo_core::nucleus::install::folder(&butterpollo_core::paths::data_root());
        let appdata = if folder == installed {
            None
        } else {
            Some(
                butterpollo_windows::user_folder::roaming_app_data()
                    .map_err(|error| anyhow::anyhow!("{error}"))
                    .context("finding the signed-in user's AppData, whose Nucleus Co-op game list counts for nucleus_path")?,
            )
        };
        files::catalog(folder, true, appdata.as_deref())
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
                arguments: steam_arguments(&launch.arguments, command(app).unwrap_or_default()),
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
        refuse_path(exe).map_err(|missing| Refusal::missing(&missing))?;
        if !Path::new(exe).is_file() {
            return Err(Refusal {
                status: 404,
                message: not_on_this_pc(exe),
            });
        }
        // An .exe in a Steam library is its Steam app's, as a link would be.
        let steam = butterpollo_core::steam::library_app(
            Path::new(exe),
            &arguments(app),
            &butterpollo_windows::steam::roots(),
        )
        .map_err(|message| Refusal {
            status: 404,
            message,
        })?;
        Ok(match steam {
            Some(steam) => Setup {
                anti_cheat: steam.vac.then_some("Valve Anti-Cheat"),
                arguments: arguments(app),
                ..Setup::new(
                    exe,
                    &app.name,
                    Some((steam.appid, &steam.install_dir.to_string_lossy())),
                )
            },
            None => Setup {
                arguments: arguments(app),
                ..Setup::new(exe, &app.name, None)
            },
        })
    }
    fn account(&self) -> Result<Account> {
        butterpollo_windows::process::user_account()
    }
    fn game_need(&self, exe: &str, folder: bool) -> Result<Need> {
        // The game's folder is usually the user's to change: its files are
        // read as them, never as the host.
        let user = user_copy::user()?;
        let sid = user_copy::sid(&user)?;
        let exe = Path::new(exe);
        let need = elevation::most([
            butterpollo_windows::nucleus::elevation::game_need(exe, &sid, Some(&user))?,
            match exe.parent().filter(|_| folder) {
                Some(parent) => {
                    butterpollo_windows::nucleus::elevation::folder_need(parent, Some(&user))?
                }
                None => Need::Nothing,
            },
        ]);
        Ok(need)
    }
    fn user_copy(&self, folder: &Path) -> Result<PathBuf> {
        let user = user_copy::user()?;
        let sid = user_copy::sid(&user)?;
        user_copy::ensure(&butterpollo_core::paths::data_root(), folder, &sid, &user)
    }
    fn write_own(&self, folder: &Path, copy: Option<&Path>, setup: &Setup) -> Result<Handler> {
        if let Some(pid) = self.running()? {
            anyhow::bail!(butterpollo_windows::nucleus::session::already_running(pid));
        }
        let own = match copy {
            None => files::write_own(folder, setup)?,
            Some(copy) => user_copy::write_own(folder, copy, setup, Some(&user_copy::user()?))?,
        };
        // Earlier builds added their games to a list Nucleus Co-op never
        // reads; they go from it once. The list is inert, so a failure is
        // only logged.
        match butterpollo_windows::user_folder::roaming_app_data() {
            Ok(appdata) => {
                if let Err(error) = files::forget_appdata_games(&appdata) {
                    tracing::warn!(
                        error = format!("{error:#}"),
                        "Nucleus Co-op: Rubylight's games left in the AppData list it does not read"
                    );
                }
            }
            Err(error) => tracing::warn!(
                %error,
                "Nucleus Co-op: the AppData list it does not read was not checked for Rubylight's games"
            ),
        }
        Ok(own)
    }
    fn remove_own(&self, folder: &Path, copy: Option<&Path>, own: &Handler) -> Result<()> {
        if let Some(pid) = self.running()? {
            anyhow::bail!(butterpollo_windows::nucleus::session::already_running(pid));
        }
        files::remove_own(folder, own)?;
        match copy {
            Some(copy) => user_copy::remove_game(copy, &own.guid, Some(&user_copy::user()?)),
            None => Ok(()),
        }
    }
    fn ready(
        &self,
        folder: &Path,
        run: Run,
        own: Option<&Setup>,
        stale: &[&Handler],
    ) -> Result<()> {
        butterpollo_windows::nucleus::session::ready(folder, run)?;
        if own.is_some() || !stale.is_empty() {
            if let Some(setup) = own {
                files::check_own(folder, setup)?;
            }
            let guids: Vec<&str> = stale.iter().map(|h| h.guid.as_str()).collect();
            files::check_list(folder, own, &guids)?;
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

/// What a Steam game started from an app's link gets: its launch option's
/// `launch` arguments, then what the app's command adds after the
/// `steam://` link. Never the rest of the command: an imported game's
/// `cmd /c start "" steam://rungameid/<id>` passes the link to cmd.
fn steam_arguments(launch: &str, command: &str) -> String {
    let after = command
        .split_once("steam://")
        .and_then(|(_, link)| link.split_once(char::is_whitespace))
        .map_or("", |(_, after)| after);
    format!("{} {}", launch.trim(), after.trim())
        .trim()
        .to_string()
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
fn refuse_path(exe_path: &str) -> Result<(), Missing> {
    let windows = std::env::var("SystemRoot").map_err(|error| Missing::Refused {
        reason: format!("Windows' folder is unknown (SystemRoot): {error}"),
    })?;
    match generic::refused_path(exe_path, &windows) {
        Some(reason) => Err(Missing::Refused { reason }),
        None => Ok(()),
    }
}

/// `setup` as Rubylight's own handler starts it: refused for a path the host
/// must not open or for a launcher, and an Unreal root stub's game in its
/// place. The hub and anti-cheat checks come after.
fn prepared(mut setup: Setup) -> Result<Setup, Missing> {
    // Before anything reads its folders.
    refuse_path(&setup.exe_path)?;
    if generic::launcher(&setup.exe_path) {
        return Err(Missing::Refused {
            reason: format!(
                "{} is a launcher, not the game: set the app's command to the game's .exe",
                file_name(&setup.exe_path)
            ),
        });
    }
    if let Some(shipping) = generic::unreal_shipping(&setup.exe_path) {
        tracing::debug!(
            stub = setup.exe_path,
            shipping,
            "Nucleus Co-op: an Unreal game's stub starts its Shipping .exe"
        );
        setup.exe_path = shipping;
    }
    Ok(setup)
}

fn file_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

/// Refuses a game shipping anti-cheat: Nucleus Co-op's hooks would look
/// like cheating.
fn refuse_anti_cheat(setup: &Setup) -> Result<(), Missing> {
    match generic::anti_cheat(setup) {
        Some(anti_cheat) => Err(Missing::AntiCheat {
            executable: file_name(&setup.exe_path).into(),
            anti_cheat: anti_cheat.into(),
        }),
        None => Ok(()),
    }
}

/// How a background stream of `app`, which has no hub handler with the game
/// added, runs Nucleus Co-op, or why it would be refused, as far as the
/// console can tell without Steam and without walking the game's folder: a
/// relative or missing .exe, a path the host must not open, a launcher,
/// anti-cheat by name or in the game's folder, a game that needs
/// administrator rights on an account without them. None for a Steam link,
/// whose game is checked at launch.
fn own_status(pc: &dyn Pc, app: &App) -> Result<Option<Run>, NoRun> {
    if !starts_exe(app) {
        return Ok(None);
    }
    let Some(exe) = executable(app) else {
        return Ok(None);
    };
    if !Path::new(exe).is_absolute() {
        return Err(Missing::NoExecutable {
            app: app.name.clone(),
        }
        .into());
    }
    let setup = prepared(Setup::new(exe, &app.name, None))?;
    if !Path::new(&setup.exe_path).is_file() {
        return Err(Missing::Refused {
            reason: not_on_this_pc(exe),
        }
        .into());
    }
    refuse_anti_cheat(&setup)?;
    run_of(pc, &setup, false).map(Some).map_err(|not| NoRun {
        lasting: matches!(not, NotRun::Refused(_)),
        missing: Missing::Refused { reason: not.into() },
    })
}

/// Why [`own_status`] has no run, and whether that stands until the .exe
/// changes: not when Rubylight could not tell ([`NotRun::Unknown`]), which
/// the console asks again at its next refresh.
struct NoRun {
    missing: Missing,
    lasting: bool,
}
impl From<Missing> for NoRun {
    fn from(missing: Missing) -> Self {
        Self {
            missing,
            lasting: true,
        }
    }
}

/// Why [`run_of`] has no run.
#[derive(Debug, PartialEq)]
enum NotRun {
    /// The game or the account rules it out.
    Refused(String),
    /// Rubylight could not tell (no signed-in user yet, a read that failed).
    Unknown(String),
}
impl From<NotRun> for String {
    fn from(not: NotRun) -> Self {
        match not {
            NotRun::Refused(reason) | NotRun::Unknown(reason) => reason,
        }
    }
}

/// How a session of Rubylight's own handler for `setup` runs Nucleus Co-op
/// ([`elevation::choose`]), or why it is refused. `folder` also walks the
/// game's folder as the user, as Nucleus does at launch.
fn run_of(pc: &dyn Pc, setup: &Setup, folder: bool) -> Result<Run, NotRun> {
    let needs_admin = handler::parse(&setup.file(), &setup.handler())
        .map(|own| handler::needs_admin(&own))
        .map_err(|error| {
            NotRun::Refused(format!(
                "Rubylight's handler for {} did not parse: {error}",
                setup.name
            ))
        })?;
    let unknown = |error: anyhow::Error| {
        NotRun::Unknown(format!(
            "Rubylight could not tell whether {} needs administrator rights: {error:#}",
            file_name(&setup.exe_path)
        ))
    };
    let account = pc.account().map_err(unknown)?;
    let script = Script::Own { needs_admin };
    let need = pc.game_need(&setup.exe_path, false).map_err(unknown)?;
    let run = elevation::choose(script, need, account).map_err(NotRun::Refused)?;
    if run == Run::Elevated || !folder {
        return Ok(run);
    }
    // Only when it would run as the user: the walk is as long as the folder.
    let need = elevation::most([need, pc.game_need(&setup.exe_path, true).map_err(unknown)?]);
    elevation::choose(script, need, account).map_err(NotRun::Refused)
}

fn not_on_this_pc(exe: &str) -> String {
    format!("{exe} is not on this PC: set the app's command to the game's .exe")
}

/// Whether `app` starts a launcher's .exe (`steam.exe -applaunch`), which
/// Rubylight's handler refuses: the console says so rather than "no handler".
pub fn starts_launcher(app: &App) -> bool {
    executable(app).is_some_and(generic::launcher)
}

/// The handler a session starts with.
enum Chosen {
    /// A hub or user handler, with the game the user added.
    Hub(Match),
    /// Rubylight's own one-player handler, written as the session starts.
    Own(Setup),
}
impl Chosen {
    fn guid(&self) -> String {
        match self {
            Self::Hub(found) => found.handler.guid.clone(),
            Self::Own(setup) => setup.guid(),
        }
    }
}

/// Rubylight's own handler for a background stream of `app`, which has no
/// hub handler. A game shipping anti-cheat is refused: Nucleus Co-op's
/// hooks would look like cheating. So is a Nucleus Co-op at `nucleus_path`
/// (`configured`) ([`own_list`]).
fn own_handler(
    pc: &dyn Pc,
    catalog: &Catalog,
    app: &App,
    configured: &str,
) -> Result<Chosen, Refusal> {
    let setup = prepared(pc.one_player(app)?).map_err(|missing| Refusal::missing(&missing))?;
    // Found by another path than the app's command: the game the user added
    // at exactly that path keeps its hub handler. A file-name match is
    // another game's (Portal's hl2.exe is not Half-Life 2).
    if let Some(found) = catalog.added(&setup.exe_path) {
        return Ok(Chosen::Hub(found));
    }
    own_list(configured).map_err(|missing| Refusal::missing(&missing))?;
    refuse_anti_cheat(&setup).map_err(|missing| Refusal::missing(&missing))?;
    Ok(Chosen::Own(setup))
}

/// Refuses Rubylight's own handler in a Nucleus Co-op at `nucleus_path`
/// (`configured`): it adds its game to the list Nucleus Co-op 2.4.2 reads
/// (`files::list`), and another release may read another, where the game
/// would never show.
fn own_list(configured: &str) -> Result<(), Missing> {
    if configured.trim().is_empty() {
        return Ok(());
    }
    Err(Missing::Refused {
        reason: "Rubylight adds a game without a hub handler only to the Nucleus Co-op it installs, and nucleus_path names another: add the game's handler from Nucleus Co-op's hub, or clear nucleus_path".into(),
    })
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
    /// Not for co-op, for `missing`, but a background stream of it runs
    /// Rubylight's own handler, `run` says how.
    Own { missing: Missing, run: Run },
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

/// [`own_status`]'s answer for one .exe, as it was modified then.
struct OwnStatus {
    at: Instant,
    modified: Option<std::time::SystemTime>,
    status: Result<Option<Run>, Missing>,
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
    /// The console's [`own_status`] answers by .exe path; only the console
    /// reads them.
    own_statuses: Mutex<BTreeMap<String, OwnStatus>>,
    /// Whether the signed-in account elevates, or why not, as the console
    /// last read it ([`Self::account`]), and when.
    account: Mutex<Option<(Instant, Account)>>,
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
            own_statuses: Mutex::default(),
            account: Mutex::new(None),
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
        self.own_statuses.lock().unwrap().clear();
        *self.account.lock().unwrap() = None;
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

    /// Whether Windows gives the account signed in on this PC an elevated
    /// token, which hub handlers, co-op and games that need administrator
    /// rights run with, or why not: for the console, read again once
    /// [`OWN_STATUS_TTL`] passed, so its poll asks Windows once a minute.
    /// None when Rubylight could not tell (nobody signed in yet), asked
    /// again at the next poll.
    pub fn account(&self) -> Option<Account> {
        let mut kept = self.account.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((at, account)) = *kept
            && at.elapsed() < OWN_STATUS_TTL
        {
            return Some(account);
        }
        match self.pc().account() {
            Ok(account) => {
                *kept = Some((Instant::now(), account));
                Some(account)
            }
            Err(error) => {
                *kept = None;
                tracing::debug!("Nucleus Co-op: whether the signed-in account elevates: {error:#}");
                None
            }
        }
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
        let configured = configured(config);
        let folder = self.pc().installed(&configured).ok().flatten()?;
        let catalog = match self.catalog(&folder, false) {
            Ok(catalog) => catalog,
            Err(error) => return Some(Err(error)),
        };
        // What a background launch without the hub's handler answers.
        let own = |app: &App| -> Result<Option<Run>, Missing> {
            let run = self.own_status(app)?;
            own_list(&configured)?;
            Ok(run)
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
                // Shown as a background launch would answer it: one without
                // the user's added game streams with ours.
                Listing::Missing(missing @ Missing::NoHandler { .. }) => match own(app) {
                    Ok(Some(run)) => GameStatus::Own { missing, run },
                    Ok(None) => GameStatus::Missing(missing),
                    Err(refused) => GameStatus::Missing(refused),
                },
                // Adding the game makes it ready, for co-op and background
                // streams alike, through the hub handler: only what adding
                // can't fix (no .exe, one not on this PC, administrator
                // rights the account lacks) says otherwise.
                Listing::Missing(missing @ Missing::NotAdded { .. }) => {
                    match self.own_status(app) {
                        Err(refused @ (Missing::NoExecutable { .. } | Missing::Refused { .. })) => {
                            GameStatus::Missing(refused)
                        }
                        Ok(Some(run)) if own_list(&configured).is_ok() => {
                            GameStatus::Own { missing, run }
                        }
                        _ => GameStatus::Missing(missing),
                    }
                }
                Listing::Missing(missing) => GameStatus::Missing(missing),
            })
            .collect();
        Some(Ok(statuses))
    }

    /// [`own_status`] for the console, read again once [`OWN_STATUS_TTL`]
    /// passed or the .exe changed; one Rubylight could not tell is read
    /// again at once. The path is checked before anything opens it, and
    /// nothing else waits while the answer is worked out.
    fn own_status(&self, app: &App) -> Result<Option<Run>, Missing> {
        let Some(exe) = executable(app)
            .filter(|_| starts_exe(app))
            .filter(|exe| Path::new(exe).is_absolute())
        else {
            return own_status(self.pc().as_ref(), app).map_err(|no| no.missing);
        };
        refuse_path(exe)?;
        let modified = std::fs::metadata(exe)
            .and_then(|metadata| metadata.modified())
            .ok();
        let fresh = |status: &&OwnStatus| {
            status.modified == modified && status.at.elapsed() < OWN_STATUS_TTL
        };
        if let Some(status) = self
            .own_statuses
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(exe)
            .filter(fresh)
        {
            return status.status.clone();
        }
        let status = own_status(self.pc().as_ref(), app);
        let lasting = status.as_ref().err().is_none_or(|no| no.lasting);
        let status = status.map_err(|no| no.missing);
        let mut statuses = self
            .own_statuses
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        statuses.retain(|_, status| status.at.elapsed() < OWN_STATUS_TTL);
        if lasting {
            statuses.insert(
                exe.to_string(),
                OwnStatus {
                    at: Instant::now(),
                    modified,
                    status: status.clone(),
                },
            );
        } else {
            // Asked again at the next refresh, not a minute later.
            statuses.remove(exe);
        }
        status
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
        let configured = configured(request.config);
        let folder = self.ensure(pc.as_ref(), &configured)?;
        let known_list = own_list(&configured).is_ok();
        let catalog = self.catalog(&folder, true).map_err(|error| {
            Refusal::unavailable(format!(
                "Nucleus Co-op's handlers and games did not load: {error}"
            ))
        })?;
        let app = request.game.app();
        let client = request.client;
        stop(self.reap());
        let (found, returning) = if kind == Kind::Background {
            // A hub handler only for the game the user added at exactly this
            // path: a file-name match is another game's (Portal's hl2.exe is
            // not Half-Life 2). Any other one-player game streams in the
            // background with ours. Its owner returning to it finds the
            // session's game: no Steam read or folder scan again.
            match executable(app).and_then(|exe| catalog.added(exe)) {
                Some(found) => (Some(Chosen::Hub(found)), None),
                None => match self.returning(request.game.app_id(), &client.uuid) {
                    Some(game) => (None, Some(game)),
                    None => (
                        Some(own_handler(pc.as_ref(), &catalog, app, &configured)?),
                        None,
                    ),
                },
            }
        } else {
            match catalog.find(&app.name, executable(app)) {
                Ok(found) => (Some(Chosen::Hub(found)), None),
                Err(missing) => return Err(Refusal::missing(&missing)),
            }
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
                    (Kind::Coop, Chosen::Hub(found)) => coop::players(
                        found.handler.max_players,
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
                let refused = |error: String| {
                    Refusal::unavailable(format!(
                        "Nucleus Co-op did not start {}: {error}",
                        app.name
                    ))
                };
                // A hub handler always runs elevated; ours as the user unless
                // it or the game needs administrator rights.
                let run = match own {
                    Some(setup) => run_of(pc.as_ref(), setup, true).map_err(String::from),
                    None => pc
                        .account()
                        .map_err(|error| format!("{error:#}"))
                        .and_then(|account| elevation::choose(Script::Hub, Need::Nothing, account)),
                }
                .map_err(refused)?;
                // Ours that a hub handler replaces or no game uses, but the
                // one this session starts with.
                let current = own.map(Setup::guid);
                let stale = catalog.stale(current.as_deref());
                pc.ready(&folder, run, own, &stale).map_err(|error| {
                    Refusal::unavailable(format!(
                        "Nucleus Co-op did not start {}: {error:#}",
                        app.name
                    ))
                })?;
                // Last, so a launch refused for another reason leaves the
                // user's window open.
                self.close_for_session(&app.name)?;
                // Copied from the host's once Nucleus Co-op is closed: none of
                // its files changes while they are read.
                let copy = match run {
                    Run::User => Some(pc.user_copy(&folder).map_err(|error| {
                        refused(format!(
                            "the signed-in user's own copy of Nucleus Co-op was not made: {error:#}"
                        ))
                    })?),
                    Run::Elevated => None,
                };
                // They go while Nucleus is closed. A failure leaves a handler
                // that is never chosen and is tried again next time: the
                // stream goes on, and its player sees why.
                let mut kept = Vec::new();
                let mut why = String::new();
                for own in stale {
                    if let Err(error) = pc.remove_own(&folder, copy.as_deref(), own) {
                        tracing::error!(file = own.file, error = %format!("{error:#}"), "Nucleus Co-op: Rubylight's replaced or unused handler was not fully removed");
                        kept.push(own.file.as_str());
                        why = format!("{error:#}");
                    }
                }
                match kept.as_slice() {
                    [] => {}
                    [file] => request.warnings.set(
                        HANDLER_NOT_REMOVED,
                        format!(
                            "Rubylight's Nucleus Co-op handler {file} was not fully removed: {why}"
                        ),
                    ),
                    files => request.warnings.set(
                        HANDLER_NOT_REMOVED,
                        format!(
                            "Rubylight's Nucleus Co-op handlers {} were not fully removed: {why}",
                            files.join(", ")
                        ),
                    ),
                }
                let exe_path = match &found {
                    Chosen::Hub(found) => found.exe_path.clone(),
                    Chosen::Own(setup) => setup.exe_path.clone(),
                };
                let (handler, steam_app) = match found {
                    Chosen::Hub(found) => (found.handler, None),
                    Chosen::Own(setup) => (
                        pc.write_own(&folder, copy.as_deref(), &setup).map_err(|error| {
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
                    user_copy: copy.as_deref(),
                    known_list,
                    handler: &handler,
                    exe_path: &exe_path,
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
                tracing::info!(session = id, ?kind, players, game = %handler.guid, own = handler.own, ?run, client = %client.name, title = %app.name, "Nucleus Co-op: session started");
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
pub(crate) mod fake;

#[cfg(test)]
mod tests;
