//! Which apps a co-op entry lists for, and how Rubylight's own one-player
//! handler would run a game: what the library and the console's game page show.
use super::*;

/// `nucleus_path` as the installer reads it.
pub fn configured(config: &Config) -> String {
    config
        .nucleus_path()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The command line that starts an app's program: its command, else its
/// first detached command.
pub(super) fn command(app: &App) -> Option<&str> {
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
pub(super) fn executable(app: &App) -> Option<&str> {
    command(app).map(butterpollo_windows::process::command_target)
}

/// What a Steam game started from an app's link gets: its launch option's
/// `launch` arguments, then what the app's command adds after the
/// `steam://` link. Never the rest of the command: an imported game's
/// `cmd /c start "" steam://rungameid/<id>` passes the link to cmd.
pub(super) fn steam_arguments(launch: &str, command: &str) -> String {
    let after = command
        .split_once("steam://")
        .and_then(|(_, link)| link.split_once(char::is_whitespace))
        .map_or("", |(_, after)| after);
    format!("{} {}", launch.trim(), after.trim())
        .trim()
        .to_string()
}

/// What an app passes its program, for Rubylight's handler.
pub(super) fn arguments(app: &App) -> String {
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
pub(super) enum Listing {
    Listed(Box<CoopApp>),
    /// Its app hides it ([`ENTRY_KEY`]).
    Hidden(Handler),
    /// An app or an earlier entry already has its id.
    Taken,
    Missing(Missing),
}

/// Each app's [`Listing`], in the order of `apps`: what the library lists
/// and what the console's game page says come from this one place.
pub(super) fn listings(catalog: &Catalog, apps: &[App]) -> Vec<Listing> {
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
pub(super) fn entries_of(catalog: &Catalog, apps: &[App]) -> Vec<CoopApp> {
    listings(catalog, apps)
        .into_iter()
        .filter_map(|listing| match listing {
            Listing::Listed(coop) => Some(*coop),
            _ => None,
        })
        .collect()
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
pub(super) fn refuse_path(exe_path: &str) -> Result<(), Missing> {
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

pub(super) fn file_name(path: &str) -> &str {
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
pub(super) fn own_status(pc: &dyn Pc, app: &App) -> Result<Option<Run>, NoRun> {
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
pub(super) struct NoRun {
    pub(super) missing: Missing,
    pub(super) lasting: bool,
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
pub(super) enum NotRun {
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
pub(super) fn run_of(pc: &dyn Pc, setup: &Setup, folder: bool) -> Result<Run, NotRun> {
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

pub(super) fn not_on_this_pc(exe: &str) -> String {
    format!("{exe} is not on this PC: set the app's command to the game's .exe")
}

/// Whether `app` starts a launcher's .exe (`steam.exe -applaunch`), which
/// Rubylight's handler refuses: the console says so rather than "no handler".
pub fn starts_launcher(app: &App) -> bool {
    executable(app).is_some_and(generic::launcher)
}

/// The handler a session starts with.
pub(super) enum Chosen {
    /// A hub or user handler, with the game the user added.
    Hub(Match),
    /// Rubylight's own one-player handler, written as the session starts.
    Own(Setup),
}
impl Chosen {
    pub(super) fn guid(&self) -> String {
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
pub(super) fn own_handler(
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
pub(super) fn own_list(configured: &str) -> Result<(), Missing> {
    if configured.trim().is_empty() {
        return Ok(());
    }
    Err(Missing::Refused {
        reason: "Rubylight adds a game without a hub handler only to the Nucleus Co-op it installs, and nucleus_path names another: add the game's handler from Nucleus Co-op's hub, or clear nucleus_path".into(),
    })
}
