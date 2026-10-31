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

/// Why `session` ended, once it did.
fn ended(session: &coop::Session) -> Option<&End> {
    match &session.phase {
        Phase::Ended(end) => Some(end),
        _ => None,
    }
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

mod apps;
mod on_pc;
mod registry;
mod stream;

pub use apps::{CoopApp, configured, entry_name, names_executable, starts_exe, starts_launcher};
pub use registry::Registry;
pub use stream::{
    Held, PLAYER, Player, Watch, audio, end_streams, isolate, player, prepare, resize,
};

use apps::*;
use on_pc::*;
#[cfg(test)]
use stream::{first_window, warn_audio};

#[cfg(test)]
pub(crate) mod fake;

#[cfg(test)]
mod tests;
