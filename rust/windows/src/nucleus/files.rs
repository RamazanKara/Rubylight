//! The files a host session reads and writes in a Nucleus Co-op folder: the
//! handlers and the user's added games, `Settings.ini`, and the game profile
//! titled "Rubylight" that starts the session's players.
use super::layers::{LOCK_WAIT, Lock};
use anyhow::{Context, Result, bail, ensure};
use butterpollo_core::nucleus::generic::{self, Inject, Setup};
use butterpollo_core::nucleus::handler::{self, Catalog, Handler};
use butterpollo_core::nucleus::profile::{self, Player};
use butterpollo_core::nucleus::window::Rect;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The handlers and the games added in the Nucleus Co-op at `folder`, from
/// its game list [`list`], the one Nucleus Co-op 2.4.2 reads (#59). A
/// handler that cannot be read is left out with a log line; an unreadable
/// game list is an error. With `signed_in`, [`Catalog::user`] is the
/// signed-in user, whose sweep may remove the handlers written for them.
/// Every folder the host runs is that release's, the chosen one as well as
/// an earlier install, so its own list is the only one.
pub fn catalog(folder: &Path, signed_in: bool) -> Result<Catalog> {
    let mut catalog = Catalog {
        user: signed_in.then(crate::process::user_sid).flatten(),
        ..Catalog::default()
    };
    catalog.handlers = handlers(folder)?;
    let path = list(folder);
    if let Some(text) = read_list(&path)? {
        let games = handler::user_games(&text)
            .map_err(anyhow::Error::msg)
            .with_context(|| path.display().to_string())?;
        for game in games {
            if !catalog.games.contains(&game) {
                catalog.games.push(game);
            }
        }
    }
    Ok(catalog)
}

/// The handlers in the Nucleus Co-op at `folder`; one that cannot be read is
/// left out with a log line.
fn handlers(folder: &Path) -> Result<Vec<Handler>> {
    let mut found = Vec::new();
    let handlers = folder.join("handlers");
    let entries = match std::fs::read_dir(&handlers) {
        Ok(entries) => entries.collect::<std::io::Result<Vec<_>>>()?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(error).with_context(|| format!("reading {}", handlers.display())),
    };
    let mut files: Vec<PathBuf> = entries
        .into_iter()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("js"))
                && path.is_file()
        })
        .collect();
    files.sort();
    for path in files {
        let file = path.file_name().unwrap_or_default().to_string_lossy();
        let text = read_text(&path)?;
        match handler::parse(&file, &text) {
            Ok(handler) => found.push(handler),
            Err(error) => tracing::info!(%file, %error, "Nucleus Co-op handler skipped"),
        }
    }
    Ok(found)
}

/// The games Nucleus Co-op 2.4.2 shows from the Nucleus Co-op at `folder`,
/// by their GUIDs, as its GameManager loads them: from [`list`] only, each
/// whose GUID a handler has and whose ExePath is a file. A game it does not
/// show can't be selected, and no profile of it appears.
pub fn listed_by_nucleus(folder: &Path) -> Result<Vec<String>> {
    let handlers = handlers(folder)?;
    let path = list(folder);
    let Some(text) = read_list(&path)? else {
        return Ok(Vec::new());
    };
    let games = handler::user_games(&text)
        .map_err(anyhow::Error::msg)
        .with_context(|| path.display().to_string())?;
    let mut listed: Vec<String> = games
        .into_iter()
        .filter(|game| handlers.iter().any(|h| h.guid == game.guid))
        .filter(|game| Path::new(&game.exe_path).is_file())
        .map(|game| game.guid)
        .collect();
    listed.sort();
    listed.dedup();
    Ok(listed)
}

fn read_text(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    String::from_utf8(bytes).with_context(|| format!("{} is not UTF-8 text", path.display()))
}

/// The most of a game list the host reads; Nucleus' own are kilobytes.
const LIST_CAP: u64 = 16 * 1024 * 1024;

/// The game list at `path`, or None when there is none. It is the signed-in
/// user's, read with the host's rights: a link is opened itself and refused,
/// never followed, and only a file on a disk of at most [`LIST_CAP`] bytes
/// is read.
fn read_list(path: &Path) -> Result<Option<String>> {
    use std::io::Read;
    use std::os::windows::fs::MetadataExt;
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Storage::FileSystem::{FILE_TYPE_DISK, GetFileType};
    const REPARSE_POINT: u32 = 0x400;
    let refused = || {
        anyhow::anyhow!(
            "{} is a folder, a link or a device, not Nucleus Co-op's game list, so Rubylight does not read it: move it away, and Nucleus Co-op makes a new one",
            path.display()
        )
    };
    let file = match crate::process::open_unfollowed(path, false) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        // A folder or a link to one can't be opened as a file.
        Err(_)
            if std::fs::symlink_metadata(path).is_ok_and(|metadata| {
                metadata.is_dir() || metadata.file_attributes() & REPARSE_POINT != 0
            }) =>
        {
            return Err(refused());
        }
        Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
    };
    let metadata = file
        .metadata()
        .with_context(|| format!("reading {}", path.display()))?;
    // SAFETY: the handle is open for the call.
    let kind = unsafe { GetFileType(windows::Win32::Foundation::HANDLE(file.as_raw_handle())) };
    if !metadata.is_file()
        || metadata.file_attributes() & REPARSE_POINT != 0
        || kind != FILE_TYPE_DISK
    {
        return Err(refused());
    }
    let mut bytes = Vec::new();
    file.take(LIST_CAP + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("reading {}", path.display()))?;
    ensure!(
        bytes.len() as u64 <= LIST_CAP,
        "{} is larger than {} MB, far more than Nucleus Co-op's game list holds, so Rubylight does not read it: move it away, and Nucleus Co-op makes a new one",
        path.display(),
        LIST_CAP / (1024 * 1024)
    );
    String::from_utf8(bytes)
        .map(Some)
        .with_context(|| format!("{} is not UTF-8 text", path.display()))
}

/// The file's text, or None when there is no file.
fn read_existing(path: &Path) -> Result<Option<String>> {
    match std::fs::metadata(path) {
        Ok(_) => read_text(path).map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
    }
}

/// Refuses `setup`'s handler file in the Nucleus Co-op at `folder` when one
/// Rubylight did not write is there: it is never overwritten.
pub fn check_own(folder: &Path, setup: &Setup) -> Result<()> {
    let path = folder.join("handlers").join(setup.file());
    match read_existing(&path)? {
        Some(old) if !generic::owns(&old) => bail!(
            "{} is not Rubylight's handler, so Rubylight leaves it as it is: move it out of Nucleus Co-op's handlers folder",
            path.display()
        ),
        _ => Ok(()),
    }
}

/// Write Rubylight's one-player handler for `setup` into the Nucleus Co-op
/// at `folder` and add its game to its game list [`list`], with the host's
/// rights: the folder is Administrators'. Call it only while Nucleus Co-op
/// is not running: it would not read them, and its own save would drop the
/// game. A handler file Rubylight did not write is never overwritten; one it
/// did is rewritten when the template changed. Returns the handler as
/// Nucleus reads it.
pub fn write_own(folder: &Path, setup: &Setup) -> Result<Handler> {
    let file = setup.file();
    // Only its account's sweep may find it unused: the list it is in is theirs.
    let owner = crate::process::user_sid()
        .context("finding the signed-in user, whose game list the handler is for")?;
    let setup = &Setup {
        inject: inject_for(folder, &setup.exe_path)?,
        window_title: title_for(folder, &setup.exe_path)?,
        arguments: setup.windowed_arguments(),
        ..setup.clone()
    };
    match setup.inject {
        Inject::Runtime => {}
        Inject::Beside => tracing::info!(
            exe = setup.exe_path,
            title = setup.window_title,
            "Nucleus Co-op: the game refused ProtoInput's input hooks every way, so Rubylight's handler has Nucleus Co-op put its input DLL beside the game"
        ),
        Inject::None => tracing::warn!(
            exe = setup.exe_path,
            "Nucleus Co-op: the game refused ProtoInput's input hooks every way, so Rubylight's handler injects none: a keyboard, mouse or pad on the PC can reach it too"
        ),
        way => tracing::info!(
            exe = setup.exe_path,
            way = way.name(),
            "Nucleus Co-op: the game refused ProtoInput's input hooks before, so Rubylight's handler injects them another way"
        ),
    }
    let text = setup.handler_of(Some(&owner));
    let handler = handler::parse(&file, &text).map_err(anyhow::Error::msg)?;
    let handlers = folder.join("handlers");
    super::unfollowed::create_folders(folder, &handlers)?;
    let path = handlers.join(&file);
    check_own(folder, setup)?;
    match read_existing(&path)? {
        Some(old) if old == text => {}
        _ => {
            super::unfollowed::write(folder, &path, text.as_bytes())?;
            tracing::info!(
                file,
                exe = setup.exe_path,
                "Nucleus Co-op: Rubylight's one-player handler written"
            );
        }
    }
    add_game(folder, setup)?;
    Ok(handler)
}

/// How Rubylight's handler injects ProtoInput's input hooks into the games
/// that refused a way in a session of the Nucleus Co-op at `folder` (the
/// host's, which only SYSTEM and Administrators change):
/// `{"Games": {<.exe path in lower case>: <Inject::name>}}`. For a game at
/// [`Inject::Beside`], by the same path, `"Titles"` holds its window's title
/// ([`keep_title`]) and `"Exits"` its exits before a window ([`note_exit`]).
fn injection(folder: &Path) -> PathBuf {
    folder.join("Rubylight-injection.json")
}

/// That file's sections; none without the file.
fn kept(folder: &Path) -> Result<serde_json::Map<String, serde_json::Value>> {
    let path = injection(folder);
    let Some(text) = read_existing(&path)? else {
        return Ok(serde_json::Map::new());
    };
    serde_json::from_str(&text)
        .with_context(|| format!("{} can't be read: delete it", path.display()))
}

fn keep(folder: &Path, kept: &serde_json::Map<String, serde_json::Value>) -> Result<()> {
    let text = serde_json::to_string_pretty(kept)?;
    replace(folder, &injection(folder), &text)
}

/// What that file's `section` holds for the game at `exe`.
fn record(folder: &Path, section: &str, exe: &str) -> Result<Option<serde_json::Value>> {
    let json = kept(folder)?;
    let records = json.get(section);
    Ok(records.and_then(|records| records.get(game_key(exe).as_str()).cloned()))
}

/// Set or remove what that file's `section` holds for the game at `exe`,
/// leaving the rest as it is.
fn set_record(
    folder: &Path,
    section: &str,
    exe: &str,
    value: Option<serde_json::Value>,
) -> Result<()> {
    let mut json = kept(folder)?;
    // [`injections`] refuses a file without them.
    json.entry("Games").or_insert_with(|| serde_json::json!({}));
    let records = json
        .entry(section)
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .with_context(|| format!("{} can't be read: delete it", injection(folder).display()))?;
    match value {
        Some(value) => records.insert(game_key(exe), value),
        None => records.remove(&game_key(exe)),
    };
    keep(folder, &json)
}

fn injections(folder: &Path) -> Result<BTreeMap<String, Inject>> {
    // A test build before the ladder listed the games that refused the
    // first way here (`{"Games": [<.exe>]}`): they start at the second.
    let mut games: BTreeMap<String, Inject> =
        read_existing(&folder.join("Rubylight-unhooked.json"))?
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
            .and_then(|json| json.get("Games").and_then(|g| g.as_array()).cloned())
            .into_iter()
            .flatten()
            .filter_map(|game| Some((game.as_str()?.to_string(), Inject::Startup)))
            .collect();
    let path = injection(folder);
    let Some(text) = read_existing(&path)? else {
        return Ok(games);
    };
    let unreadable = || format!("{} can't be read: delete it", path.display());
    let json: serde_json::Value = serde_json::from_str(&text).with_context(unreadable)?;
    let Some(kept) = json.get("Games").and_then(serde_json::Value::as_object) else {
        bail!(unreadable());
    };
    for (game, way) in kept {
        let way = way.as_str().and_then(Inject::from_name);
        games.insert(game.clone(), way.with_context(unreadable)?);
    }
    Ok(games)
}

fn game_key(exe: &str) -> String {
    exe.trim().replace('/', "\\").to_lowercase()
}

/// How Rubylight's handler for the game at `exe` injects ProtoInput's
/// input hooks ([`Setup::inject`]): the first way, or the next after one it
/// refused in a session of the Nucleus Co-op at `folder` ([`refuse_inject`]).
/// Nucleus' DLL needs its window's title, which the first session at that
/// way runs without the DLL to see ([`keep_title`]).
pub fn inject_for(folder: &Path, exe: &str) -> Result<Inject> {
    let way = kept_way(folder, exe)?;
    if way == Inject::Beside && title_for(folder, exe)?.is_none() {
        return Ok(Inject::None);
    }
    Ok(way)
}

fn kept_way(folder: &Path, exe: &str) -> Result<Inject> {
    Ok(injections(folder)?
        .get(&game_key(exe))
        .copied()
        .unwrap_or_default())
}

/// Whether the game at `exe` has come to Nucleus' DLL, with or still without
/// its window's title.
pub fn beside_kept(folder: &Path, exe: &str) -> Result<bool> {
    Ok(kept_way(folder, exe)? == Inject::Beside)
}

/// Whether a session of the game at `exe` may have had Nucleus' DLL in its
/// folder: only games past the hooks ever do.
pub fn beside_tried(folder: &Path, exe: &str) -> Result<bool> {
    Ok(matches!(
        kept_way(folder, exe)?,
        Inject::Beside | Inject::None
    ))
}

/// The title kept of the window of the game at `exe`.
fn title_for(folder: &Path, exe: &str) -> Result<Option<String>> {
    let title = record(folder, "Titles", exe)?;
    Ok(title
        .as_ref()
        .and_then(serde_json::Value::as_str)
        .filter(|title| !title.trim().is_empty())
        .map(str::to_string))
}

/// Keep the title of the window of the game at `exe` for the handler of its
/// next start.
pub fn keep_title(folder: &Path, exe: &str, title: &str) -> Result<()> {
    set_record(folder, "Titles", exe, Some(title.into()))
}

/// Count an exit before its window of the game at `exe`, with Nucleus' DLL
/// beside it. Returns the exits since the DLL last loaded ([`clear_exits`]).
pub fn note_exit(folder: &Path, exe: &str) -> Result<u64> {
    let exits = record(folder, "Exits", exe)?.and_then(|exits| exits.as_u64());
    let exits = exits.unwrap_or(0) + 1;
    set_record(folder, "Exits", exe, Some(exits.into()))?;
    Ok(exits)
}

/// The game at `exe` ran with Nucleus' DLL: its earlier exits were not that.
pub fn clear_exits(folder: &Path, exe: &str) -> Result<()> {
    if record(folder, "Exits", exe)?.is_none() {
        return Ok(());
    }
    set_record(folder, "Exits", exe, None)
}

/// The most of a file read to tell whether it is Nucleus' DLL.
const BESIDE_CAP: u64 = 64 * 1024 * 1024;

/// Whether the file at `path` is Nucleus' DLL ([`generic::BESIDE_MARK`]). A
/// link is opened itself and never followed: it isn't.
fn holds_beside_mark(path: &Path) -> Result<bool> {
    use std::io::Read;
    use std::os::windows::fs::MetadataExt;
    const REPARSE_POINT: u32 = 0x400;
    let reading = || format!("reading {}", path.display());
    let file = match crate::process::open_unfollowed(path, false) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error).with_context(reading),
    };
    let metadata = file.metadata().with_context(reading)?;
    if !metadata.is_file() || metadata.file_attributes() & REPARSE_POINT != 0 {
        return Ok(false);
    }
    let mut bytes = Vec::new();
    file.take(BESIDE_CAP)
        .read_to_end(&mut bytes)
        .with_context(reading)?;
    let mark = generic::BESIDE_MARK.as_bytes();
    Ok(bytes.windows(mark.len()).any(|at| at == mark))
}

/// Take Nucleus' DLL out of the game's folder `game` and put back what
/// Nucleus moved aside for it. Nucleus does so as it ends, if it gets to;
/// left there, the game would load it when played on the PC too. Any other
/// DLL of those names is left alone. Returns the files taken. For the
/// user's game folder: call it acting as the user.
pub fn clear_beside(game: &Path) -> Result<Vec<String>> {
    let removed = |path: &Path| match std::fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).with_context(|| format!("removing {}", path.display())),
    };
    let mut cleared = Vec::new();
    for dll in generic::BESIDE_DLLS {
        let path = game.join(dll);
        if !holds_beside_mark(&path)? {
            continue;
        }
        removed(&path)?;
        cleared.push(dll.to_string());
        let stem = dll.trim_end_matches(".dll");
        let aside = game.join(format!("{stem}_NUCLEUS_BACKUP.dll"));
        match std::fs::rename(&aside, &path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| format!("restoring {}", path.display()));
            }
        }
    }
    if !cleared.is_empty() {
        for ini in [generic::BESIDE_INI, "ncoop_NUCLEUS_BACKUP.ini"] {
            if removed(&game.join(ini))? {
                cleared.push(ini.to_string());
            }
        }
    }
    Ok(cleared)
}

/// Remember that the game at `exe` refused the way ProtoInput's input hooks
/// were injected in a session of the Nucleus Co-op at `folder`, which
/// ended it (#59): its next start gets the next way. Returns that way.
pub fn refuse_inject(folder: &Path, exe: &str) -> Result<Inject> {
    let mut games = injections(folder)?;
    let key = game_key(exe);
    let refused = games.get(&key).copied().unwrap_or_default();
    let Some(next) = refused.next() else {
        return Ok(refused);
    };
    games.insert(key, next);
    let games: serde_json::Map<String, serde_json::Value> = games
        .into_iter()
        .map(|(game, way)| (game, way.name().into()))
        .collect();
    let mut json = kept(folder)?;
    json.insert("Games".into(), games.into());
    keep(folder, &json)?;
    tracing::warn!(
        exe,
        refused = refused.name(),
        next = next.name(),
        "Nucleus Co-op: the game refused ProtoInput's input hooks; its next start gets the next way"
    );
    Ok(next)
}

/// Add `setup`'s game to the game list of the Nucleus Co-op at `folder`
/// ([`list`]), replacing the file whole so Nucleus never reads half of it.
pub(super) fn add_game(folder: &Path, setup: &Setup) -> Result<()> {
    let path = &list(folder);
    let old = read_list(path)?;
    let Some(new) = generic::add_game(old.as_deref(), &setup.guid(), &setup.exe_path)
        .map_err(anyhow::Error::msg)
        .with_context(|| path.display().to_string())?
    else {
        return Ok(());
    };
    let parent = path.parent().context("the game list has no folder")?;
    super::unfollowed::create_folders(folder, parent)?;
    replace(folder, path, &new)?;
    tracing::info!(
        guid = setup.guid(),
        "Nucleus Co-op: game added to its game list"
    );
    Ok(())
}

/// Replace the game list at `path` below `root` whole, so Nucleus never
/// reads half of it, through no link at its name or on its way.
fn replace(root: &Path, path: &Path, text: &str) -> Result<()> {
    super::unfollowed::write(root, path, text.as_bytes())
}

/// The game list of the Nucleus Co-op at `folder`, the one 2.4.2 reads
/// (its GameManager's ALPHA branch is the released build). Nucleus Co-op
/// never reads `%APPDATA%\Nucleus Coop\userprofile.json`, where earlier
/// Rubylight builds added their games (#59).
pub fn list(folder: &Path) -> PathBuf {
    folder.join("content").join("userprofile.json")
}

/// The game list in the roaming folder `appdata`, which Nucleus Co-op 2.4.2
/// never reads.
fn appdata_list(appdata: &Path) -> PathBuf {
    appdata.join("Nucleus Coop").join("userprofile.json")
}

/// Refuses the game list of the Nucleus Co-op at `folder` when Rubylight
/// could not add `setup`'s game to it or remove the games of `remove` from
/// it. Checked before a session closes the console's window, so a list
/// Nucleus Co-op wrote in a way the host can't edit refuses the launch with
/// the window still open.
pub fn check_list(folder: &Path, setup: Option<&Setup>, remove: &[&str]) -> Result<()> {
    let path = list(folder);
    let Some(text) = read_list(&path)? else {
        return Ok(());
    };
    let edits = || -> Result<(), String> {
        if let Some(setup) = setup {
            generic::add_game(Some(&text), &setup.guid(), &setup.exe_path)?;
        }
        for guid in remove {
            generic::remove_game(&text, guid)?;
        }
        Ok(())
    };
    edits()
        .map_err(anyhow::Error::msg)
        .with_context(|| path.display().to_string())
}

/// Remove Rubylight's own handler `own`, one [`Catalog::stale`] names, and
/// its game from the game list of the Nucleus Co-op at `folder`. Like
/// [`write_own`], only while Nucleus Co-op is not running. Only a handler
/// Rubylight wrote, and only its game, ever go.
pub fn remove_own(folder: &Path, own: &Handler) -> Result<()> {
    ensure!(
        own.own && own.guid.starts_with(generic::PREFIX),
        "{} is not Rubylight's handler",
        own.file
    );
    // The handler first: Nucleus Co-op drops a game whose handler is gone
    // as it loads its list, and a handler that stays is found again.
    let path = folder.join("handlers").join(&own.file);
    if let Some(text) = read_existing(&path)? {
        ensure!(
            generic::owns(&text),
            "{} is not Rubylight's handler, so Rubylight leaves it as it is",
            path.display()
        );
        super::unfollowed::remove(folder, &path)?;
    }
    let list = list(folder);
    remove_game(folder, &own.guid).with_context(|| {
        format!(
            "{} is removed, but {} still lists its game. Nucleus Co-op leaves a game whose handler is gone out of the list as it loads it, and Rubylight's next handler for the game takes the entry over",
            path.display(),
            list.display()
        )
    })?;
    tracing::info!(
        file = own.file,
        "Nucleus Co-op: Rubylight's handler removed, a hub handler replaces it or no game uses it"
    );
    Ok(())
}

/// Remove the games of Rubylight's handlers, and nothing else, from
/// `%APPDATA%\Nucleus Coop\userprofile.json` in the signed-in user's roaming
/// folder `appdata`, as the user: earlier builds added them there, and
/// Nucleus Co-op 2.4.2 never reads that list. Returns how many went; none
/// once they are gone.
pub fn forget_appdata_games(appdata: &Path) -> Result<usize> {
    crate::user_folder::as_session_user(|| forget_games(appdata))
        .context("acting as the signed-in user")?
}

fn forget_games(appdata: &Path) -> Result<usize> {
    let path = &appdata_list(appdata);
    let Some(mut text) = read_list(path)? else {
        return Ok(0);
    };
    let games = handler::user_games(&text)
        .map_err(anyhow::Error::msg)
        .with_context(|| path.display().to_string())?;
    let mut guids: Vec<String> = games
        .into_iter()
        .map(|game| game.guid)
        .filter(|guid| guid.starts_with(generic::PREFIX))
        .collect();
    guids.dedup();
    let mut forgotten = 0;
    for guid in &guids {
        if let Some(new) = generic::remove_game(&text, guid)
            .map_err(anyhow::Error::msg)
            .with_context(|| path.display().to_string())?
        {
            text = new;
            forgotten += 1;
        }
    }
    if forgotten > 0 {
        replace(appdata, path, &text)?;
        tracing::info!(
            forgotten,
            list = %path.display(),
            "Nucleus Co-op: Rubylight's games removed from a list Nucleus Co-op does not read"
        );
    }
    Ok(forgotten)
}

/// Remove the game `guid` from the game list of the Nucleus Co-op at
/// `folder` ([`list`]), if it is there.
pub(super) fn remove_game(folder: &Path, guid: &str) -> Result<()> {
    let path = &list(folder);
    let Some(old) = read_list(path)? else {
        return Ok(());
    };
    match generic::remove_game(&old, guid)
        .map_err(anyhow::Error::msg)
        .with_context(|| path.display().to_string())?
    {
        Some(new) => replace(folder, path, &new),
        None => Ok(()),
    }
}

/// The file in Nucleus' folder that keeps the values a session forced in
/// `Settings.ini` until they are put back.
fn settings_journal(folder: &Path) -> PathBuf {
    folder.join("Rubylight-settings.json")
}

/// Write `path` below `root` whole: a temporary file, on the disk before it
/// is renamed over it, so a crash or power loss never leaves half a file.
/// Never through a link below `root` ([`super::unfollowed`]).
fn write_whole(root: &Path, path: &Path, bytes: &[u8]) -> Result<()> {
    super::unfollowed::write(root, path, bytes)
}

/// `Settings.ini` refused when it is UTF-16 text, which the host does not
/// edit ([`profile::utf16_ini`]).
fn ansi_settings(path: &Path, text: &[u8]) -> Result<()> {
    ensure!(
        !profile::utf16_ini(text),
        "{} is saved as UTF-16 (Unicode) text, which Rubylight does not edit: save it as ANSI, or delete it and Rubylight writes Nucleus Co-op's defaults",
        path.display()
    );
    Ok(())
}

/// Put back the user's values of `forced` in `folder`'s `Settings.ini`
/// where the session's still are ([`profile::put_back_settings`]). A file
/// that is gone has nothing to put back.
fn put_back_settings(folder: &Path, forced: &[profile::Forced]) -> Result<usize> {
    let path = folder.join("Settings.ini");
    let text = match std::fs::read(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            tracing::info!(
                file = %path.display(),
                "Nucleus Co-op: Settings.ini is gone, nothing to put back"
            );
            return Ok(0);
        }
        Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
    };
    ansi_settings(&path, &text)?;
    let (text, changed) = profile::put_back_settings(&text, forced);
    if changed > 0 {
        write_whole(folder, &path, &text)?;
    }
    Ok(changed)
}

/// Remove a journal put back from; one already gone is fine.
fn remove_settings_journal(journal: &Path) -> Result<()> {
    let folder = journal.parent().context("the journal has no folder")?;
    super::unfollowed::remove(folder, journal).map(drop)
}

/// Put back what a session the host never ended forced in the
/// `Settings.ini` of the Nucleus at `folder`, and remove its journal, as the
/// service starts or sees a sign-in. Returns how many values it put back,
/// or None while a session or another restore holds the journal's lock: a
/// session's values are never put back under it.
pub fn restore_settings_left(folder: &Path) -> Result<Option<usize>> {
    let journal = settings_journal(folder);
    if !journal.exists() {
        return Ok(Some(0));
    }
    let Some(_lock) = Lock::try_take(&journal)? else {
        return Ok(None);
    };
    restore_left_locked(folder).map(Some)
}

/// [`restore_settings_left`] with the journal's lock held.
fn restore_left_locked(folder: &Path) -> Result<usize> {
    let journal = settings_journal(folder);
    let bytes = match std::fs::read(&journal) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "reading {}, which keeps the Nucleus Co-op settings an earlier session changed",
                    journal.display()
                )
            });
        }
    };
    // Created but never written to the disk, as power loss can leave it.
    if bytes.iter().all(u8::is_ascii_whitespace) {
        tracing::warn!(
            file = %journal.display(),
            "Nucleus Co-op: the settings file of an earlier session is empty; removing it"
        );
        remove_settings_journal(&journal)?;
        return Ok(0);
    }
    let forced: Vec<profile::Forced> = serde_json::from_slice(&bytes).map_err(|error| {
        anyhow::anyhow!(
            "{} can't be read ({error}): delete it, then start the stream again. The Nucleus Co-op settings it kept stay as the earlier session set them",
            journal.display()
        )
    })?;
    let changed = put_back_settings(folder, &forced).with_context(|| {
        format!(
            "the Nucleus Co-op settings an earlier session changed were not put back from {}",
            journal.display()
        )
    })?;
    tracing::info!(
        changed,
        "Nucleus Co-op: an earlier session's changes to Settings.ini are put back"
    );
    remove_settings_journal(&journal)?;
    Ok(changed)
}

/// The values a session forced in `Settings.ini`, put back once when it is
/// dropped or [`restore`](SettingsGuard::restore) is called, after Nucleus
/// closed. Should that fail, its journal keeps them for the next session.
/// It holds the journal's lock until it drops, so the service never puts
/// the user's values back under a starting or running session.
pub struct SettingsGuard {
    folder: PathBuf,
    forced: Vec<profile::Forced>,
    restored: bool,
    /// Tests let go of it to stand in for a host that stopped.
    pub(super) _lock: Option<Lock>,
}

impl SettingsGuard {
    pub fn restore(&mut self) {
        if std::mem::replace(&mut self.restored, true) || self.forced.is_empty() {
            return;
        }
        let journal = settings_journal(&self.folder);
        match put_back_settings(&self.folder, &self.forced) {
            Ok(changed) => {
                tracing::info!(
                    changed,
                    "Nucleus Co-op: Settings.ini is back as the user set it"
                );
                if let Err(error) = remove_settings_journal(&journal) {
                    tracing::warn!(
                        error = format!("{error:#}"),
                        "Nucleus Co-op: the put-back settings' file was not removed"
                    );
                }
            }
            Err(error) => tracing::error!(
                error = format!("{error:#}"),
                file = %journal.display(),
                "Nucleus Co-op: Settings.ini was not put back; the next session tries again"
            ),
        }
    }
}

impl Drop for SettingsGuard {
    fn drop(&mut self) {
        self.restore();
    }
}

/// Set what a session needs in Nucleus' `Settings.ini` (read from Nucleus'
/// working folder), first putting back what an earlier session left
/// ([`restore_settings_left`]). Nucleus stops with a message box on a
/// missing key, so a file missing keys (or missing) is completed from its
/// own defaults ([`profile::settings_ini`]). The user's choices the session
/// changes come back through the returned guard; every other byte stays.
pub fn write_settings(folder: &Path) -> Result<SettingsGuard> {
    let journal = settings_journal(folder);
    let lock = Lock::take(&journal, LOCK_WAIT)?;
    restore_left_locked(folder)?;
    let path = folder.join("Settings.ini");
    let (text, settings) = session_settings(&path)?;
    // The journal first: a host that stops after the next write finds it.
    if !settings.forced.is_empty() {
        write_whole(
            folder,
            &journal,
            &serde_json::to_vec_pretty(&settings.forced)?,
        )?;
    }
    if settings.text != text {
        write_whole(folder, &path, &settings.text)?;
    }
    Ok(SettingsGuard {
        folder: folder.to_path_buf(),
        forced: settings.forced,
        restored: false,
        _lock: Some(lock),
    })
}

/// The `Settings.ini` at `path` (empty when there is none) and what a
/// session makes of it ([`profile::settings_ini`]); refused when it is
/// UTF-16.
fn session_settings(path: &Path) -> Result<(Vec<u8>, profile::Settings)> {
    let text = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
    };
    ansi_settings(path, &text)?;
    let settings = profile::settings_ini(&text);
    if !settings.added.is_empty() {
        tracing::info!(
            file = %path.display(),
            keys = settings.added.join(", "),
            "Nucleus Co-op: Settings.ini completed from Nucleus Co-op {}'s defaults",
            butterpollo_core::nucleus::install::VERSION
        );
    }
    Ok((text, settings))
}

/// Set what a session needs in the `Settings.ini` of the user's own copy at
/// `copy` ([`super::user_copy`]), called as that user. The copy is
/// Rubylight's and only its sessions run it, so the values stay: no
/// journal, and nothing in the copy is ever put back as the host.
pub fn write_copy_settings(copy: &Path) -> Result<()> {
    let path = copy.join("Settings.ini");
    let (text, settings) = session_settings(&path)?;
    if settings.text != text {
        write_whole(copy, &path, &settings.text)?;
    }
    Ok(())
}

/// Nucleus' per-game settings in the user's profile folder `profile`
/// ([`profile::meta_info_by_index`]).
pub fn meta_info(profile: &Path) -> PathBuf {
    profile.join("NucleusCoop").join("metaInfo.json")
}

/// Give game `guid` its pads by XInput index in the `metaInfo.json` of the
/// profile folder `profile` ([`meta_info`], [`profile::meta_info_by_index`]),
/// called as the user it belongs to. Nucleus' other games and values stay,
/// and so does this one's: like pads by index in `Settings.ini`, it is never
/// put back.
pub fn index_pads(profile: &Path, guid: &str) -> Result<()> {
    let path = &meta_info(profile);
    let text = match std::fs::read(path) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
    };
    let Some(changed) = profile::meta_info_by_index(text.as_deref(), guid)
        .map_err(|why| anyhow::anyhow!("{}: {why}; fix or delete it", path.display()))?
    else {
        return Ok(());
    };
    if let Some(parent) = path.parent() {
        super::unfollowed::create_folders(profile, parent)?;
    }
    write_whole(profile, path, &changed)?;
    if text.is_some() {
        tracing::info!(
            file = %path.display(),
            game = guid,
            "Nucleus Co-op: the game's pads go by XInput index again in its per-game settings"
        );
    } else {
        tracing::info!(
            file = %path.display(),
            "Nucleus Co-op: per-game settings started empty, so each game takes pads by index from Settings.ini"
        );
    }
    Ok(())
}

/// A handler's GUID names its profile folder; one that would name another
/// folder is refused.
fn profile_folder(folder: &Path, guid: &str) -> Result<PathBuf> {
    ensure!(
        !guid.trim().is_empty()
            && guid != "."
            && guid != ".."
            && !guid.contains(['\\', '/', ':', '*', '?', '"', '<', '>', '|'])
            && !guid.chars().any(char::is_control),
        "the Nucleus Co-op handler's Game.GUID {guid:?} cannot name a profile folder"
    );
    Ok(folder.join("game profiles").join(guid))
}

/// The files of a game's profile folder in the order Nucleus lists them
/// (`GameProfile.ListGameProfiles`: every file, shortest path first, ties in
/// the folder's name order), with their profile number and title.
fn listed(profiles: &Path) -> Result<Vec<(String, u32, Option<String>)>> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(profiles)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let number = profile::file_number(&name).unwrap_or(0);
        // A profile the host cannot read keeps its place; it is just not ours.
        let title = std::fs::read(entry.path())
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .and_then(|json| json.get("Title")?.as_str().map(str::to_string));
        files.push((name, number, title));
    }
    files.sort_by_cached_key(|(name, _, _)| (name.chars().count(), name.to_uppercase()));
    Ok(files)
}

/// Write the session's profile for the game `guid`: the host's own
/// "Rubylight" profile if the game has one, else a new one after the
/// user's. Returns its number. Nucleus' n-th listed entry loads
/// `Profile[n].json`, so a number that is not the file's place in the list
/// would load another profile: that is refused, as is overwriting a profile
/// that is not the host's.
pub fn write_profile(
    folder: &Path,
    guid: &str,
    players: &[Player],
    screens: &[Rect],
) -> Result<u32> {
    ensure!(
        !players.is_empty(),
        "a Nucleus Co-op profile needs a player"
    );
    let profiles = profile_folder(folder, guid)?;
    super::unfollowed::create_folders(folder, &profiles)?;
    let before = listed(&profiles)?;
    let titles: Vec<(u32, Option<String>)> = before
        .iter()
        .map(|(_, number, title)| (*number, title.clone()))
        .collect();
    let number = profile::number(&titles);
    let name = format!("Profile[{number}].json");
    let ours = match before
        .iter()
        .find(|(file, _, _)| file.eq_ignore_ascii_case(&name))
    {
        Some((_, _, title)) if title.as_deref() != Some(profile::TITLE) => bail!(
            "Nucleus Co-op's profile {name} for {guid} is not Rubylight's: delete or renumber the game's profiles in Nucleus Co-op"
        ),
        Some(_) => true,
        None => false,
    };
    let path = profiles.join(&name);
    let text = serde_json::to_string_pretty(&profile::build(players, screens))?;
    super::unfollowed::write(folder, &path, text.as_bytes())?;
    let after = listed(&profiles)?;
    let place = after.iter().position(|(file, _, _)| *file == name);
    if place != Some(number as usize - 1) {
        if !ours {
            // The refusal says what to do; a profile left behind is listed.
            let _ = super::unfollowed::remove(folder, &path);
        }
        let files: Vec<&str> = after.iter().map(|(file, _, _)| file.as_str()).collect();
        bail!(
            "Nucleus Co-op would not load Rubylight's profile {name} for {guid} (its folder lists {}): delete or renumber the game's profiles in Nucleus Co-op",
            files.join(", ")
        );
    }
    Ok(number)
}

#[cfg(test)]
mod tests;
