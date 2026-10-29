//! The files a host session reads and writes in a Nucleus Co-op folder: the
//! handlers and the user's added games, `Settings.ini`, and the game profile
//! titled "Rubylight" that starts the session's players.
use anyhow::{Context, Result, bail, ensure};
use butterpollo_core::nucleus::generic::{self, Setup};
use butterpollo_core::nucleus::handler::{self, Catalog, Handler};
use butterpollo_core::nucleus::profile::{self, Player};
use butterpollo_core::nucleus::window::Rect;
use std::path::{Path, PathBuf};

/// The handlers and the games the user added in the Nucleus Co-op at
/// `folder`. `appdata` is the signed-in user's roaming folder, where release
/// builds keep `Nucleus Coop\userprofile.json` (ALPHA builds keep it in
/// `content`). A handler that cannot be read is left out with a log line;
/// an unreadable game list is an error. Every account's list comes too
/// ([`Catalog::everyone`]).
pub fn catalog(folder: &Path, appdata: Option<&Path>) -> Result<Catalog> {
    catalog_in(folder, appdata, crate::child_session::profile_folders())
}

/// [`catalog`], with `accounts` the profile folder of each account on the PC.
fn catalog_in(
    folder: &Path,
    appdata: Option<&Path>,
    accounts: Result<Vec<PathBuf>>,
) -> Result<Catalog> {
    let mut catalog = Catalog::default();
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
            Ok(handler) => catalog.handlers.push(handler),
            Err(error) => tracing::info!(%file, %error, "Nucleus Co-op handler skipped"),
        }
    }
    let profiles = appdata
        .map(|appdata| appdata.join("Nucleus Coop").join("userprofile.json"))
        .into_iter()
        .chain([folder.join("content").join("userprofile.json")]);
    for path in profiles {
        if !path.is_file() {
            continue;
        }
        let games = handler::user_games(&read_text(&path)?)
            .map_err(anyhow::Error::msg)
            .with_context(|| path.display().to_string())?;
        for game in games {
            if !catalog.games.contains(&game) {
                catalog.games.push(game);
            }
        }
    }
    catalog.everyone = match accounts.and_then(|accounts| everyones_games(&accounts)) {
        Ok(games) => Some(games),
        Err(error) => {
            tracing::error!(
                error = format!("{error:#}"),
                "Nucleus Co-op: another account's game list was not read, so no handler of Rubylight's counts as unused"
            );
            None
        }
    };
    Ok(catalog)
}

/// The games in the release builds' list of each account whose profile
/// folder is in `profiles` (`AppData\Roaming\Nucleus Coop`). A list that is
/// there but can't be read is an error.
fn everyones_games(profiles: &[PathBuf]) -> Result<Vec<handler::Game>> {
    let mut games = Vec::new();
    for profile in profiles {
        let path = list(&profile.join("AppData").join("Roaming"));
        let Some(text) = read_existing(&path)? else {
            continue;
        };
        games.extend(
            handler::user_games(&text)
                .map_err(anyhow::Error::msg)
                .with_context(|| path.display().to_string())?,
        );
    }
    Ok(games)
}

fn read_text(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    String::from_utf8(bytes).with_context(|| format!("{} is not UTF-8 text", path.display()))
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
/// at `folder` and add its game to the signed-in user's game list in
/// `appdata` (their roaming folder, which release builds read), as the
/// user. Call it only while Nucleus Co-op is not running: it would not read
/// them, and its own save would drop the game. A handler file Rubylight did
/// not write is never overwritten; one it did is rewritten when the template
/// changed. Returns the handler as Nucleus reads it.
pub fn write_own(folder: &Path, appdata: &Path, setup: &Setup) -> Result<Handler> {
    let file = setup.file();
    let text = setup.handler();
    let handler = handler::parse(&file, &text).map_err(anyhow::Error::msg)?;
    let handlers = folder.join("handlers");
    std::fs::create_dir_all(&handlers)
        .with_context(|| format!("creating {}", handlers.display()))?;
    let path = handlers.join(&file);
    check_own(folder, setup)?;
    match read_existing(&path)? {
        Some(old) if old == text => {}
        _ => {
            std::fs::write(&path, &text).with_context(|| format!("writing {}", path.display()))?;
            tracing::info!(
                file,
                exe = setup.exe_path,
                "Nucleus Co-op: Rubylight's one-player handler written"
            );
        }
    }
    let profile = list(appdata);
    crate::user_folder::as_session_user(|| add_game(&profile, setup))
        .context("acting as the signed-in user")??;
    Ok(handler)
}

/// Add `setup`'s game to the game list at `path`, replacing the file whole
/// so Nucleus never reads half of it.
fn add_game(path: &Path, setup: &Setup) -> Result<()> {
    let old = read_existing(path)?;
    let Some(new) = generic::add_game(old.as_deref(), &setup.guid(), &setup.exe_path)
        .map_err(anyhow::Error::msg)
        .with_context(|| path.display().to_string())?
    else {
        return Ok(());
    };
    let parent = path.parent().context("the game list has no folder")?;
    std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    replace(path, &new)?;
    tracing::info!(
        guid = setup.guid(),
        "Nucleus Co-op: game added to the user's game list"
    );
    Ok(())
}

/// Replace the game list at `path` whole, so Nucleus never reads half of it.
fn replace(path: &Path, text: &str) -> Result<()> {
    let temporary = path.with_extension("json.rubylight");
    std::fs::write(&temporary, text).with_context(|| format!("writing {}", temporary.display()))?;
    std::fs::rename(&temporary, path).with_context(|| format!("replacing {}", path.display()))
}

/// The signed-in user's game list in their roaming folder `appdata`.
fn list(appdata: &Path) -> PathBuf {
    appdata.join("Nucleus Coop").join("userprofile.json")
}

/// Refuses the signed-in user's game list in `appdata` when Rubylight could
/// not add `setup`'s game to it or remove the games of `remove` from it.
/// Checked before a session closes the console's window, so a list Nucleus
/// Co-op wrote in a way the host can't edit refuses the launch with the
/// window still open.
pub fn check_list(appdata: &Path, setup: Option<&Setup>, remove: &[&str]) -> Result<()> {
    let path = list(appdata);
    let Some(text) = read_existing(&path)? else {
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
/// its game from the signed-in user's game list in `appdata`, as the user.
/// Like [`write_own`], only while Nucleus Co-op is not running. Only a
/// handler Rubylight wrote, and only its game, ever go.
pub fn remove_own(folder: &Path, appdata: &Path, own: &Handler) -> Result<()> {
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
        std::fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
    }
    let list = list(appdata);
    crate::user_folder::as_session_user(|| remove_game(&list, &own.guid))
        .context("acting as the signed-in user")?
        .with_context(|| {
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

fn remove_game(path: &Path, guid: &str) -> Result<()> {
    let Some(old) = read_existing(path)? else {
        return Ok(());
    };
    match generic::remove_game(&old, guid)
        .map_err(anyhow::Error::msg)
        .with_context(|| path.display().to_string())?
    {
        Some(new) => replace(path, &new),
        None => Ok(()),
    }
}

/// Set what the host's profile needs in Nucleus' `Settings.ini` (read from
/// Nucleus' working folder), keeping the user's other settings.
pub fn write_settings(folder: &Path) -> Result<()> {
    let path = folder.join("Settings.ini");
    let text = match std::fs::read(&path) {
        Ok(bytes) => String::from_utf8(bytes)
            .with_context(|| format!("{} is not UTF-8 text", path.display()))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
    };
    let (bom, body) = match text.strip_prefix('\u{feff}') {
        Some(body) => ("\u{feff}", body),
        None => ("", text.as_str()),
    };
    let updated = format!("{bom}{}", profile::settings_ini(body));
    if updated != text {
        std::fs::write(&path, updated).with_context(|| format!("writing {}", path.display()))?;
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
    std::fs::create_dir_all(&profiles)
        .with_context(|| format!("creating {}", profiles.display()))?;
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
    std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
    let after = listed(&profiles)?;
    let place = after.iter().position(|(file, _, _)| *file == name);
    if place != Some(number as usize - 1) {
        if !ours {
            let _ = std::fs::remove_file(&path);
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
mod tests {
    use super::*;

    const HANDLER: &str = r#"Game.GUID = "Borderlands 2";
Game.GameName = "Borderlands 2";
Game.ExecutableName = "Borderlands2.exe";
Game.MaxPlayers = 4;
"#;

    fn player(xinput: u8) -> Player {
        Player {
            xinput,
            bounds: Rect {
                x: 4000 + i32::from(xinput) * 2000,
                y: 0,
                width: 1920,
                height: 1080,
            },
        }
    }
    const SCREEN: Rect = Rect {
        x: 0,
        y: 0,
        width: 2560,
        height: 1440,
    };

    #[test]
    fn the_catalog_reads_handlers_and_both_game_lists() {
        let nucleus = tempfile::tempdir().unwrap();
        let appdata = tempfile::tempdir().unwrap();
        let handlers = nucleus.path().join("handlers");
        std::fs::create_dir_all(&handlers).unwrap();
        std::fs::write(handlers.join("Borderlands 2.js"), HANDLER).unwrap();
        std::fs::write(handlers.join("broken.js"), "Game.GameName = 1;").unwrap();
        std::fs::write(handlers.join("notes.txt"), HANDLER).unwrap();
        std::fs::create_dir_all(appdata.path().join("Nucleus Coop")).unwrap();
        std::fs::write(
            appdata.path().join("Nucleus Coop").join("userprofile.json"),
            r#"{"Games":[{"GameGuid":"Borderlands 2","ExePath":"D:\\Games\\BL2\\Borderlands2.exe"}]}"#,
        )
        .unwrap();
        std::fs::create_dir_all(nucleus.path().join("content")).unwrap();
        std::fs::write(
            nucleus.path().join("content").join("userprofile.json"),
            "\u{feff}{\"Games\":[{\"GameGuid\":\"Borderlands 2\",\"ExePath\":\"E:\\\\BL2\\\\Borderlands2.exe\"}]}",
        )
        .unwrap();
        let catalog = catalog(nucleus.path(), Some(appdata.path())).unwrap();
        assert_eq!(catalog.handlers.len(), 1);
        assert_eq!(catalog.handlers[0].executable, "Borderlands2.exe");
        assert_eq!(catalog.games.len(), 2);
        let found = catalog
            .find("Borderlands 2", Some(r"E:\BL2\Borderlands2.exe"))
            .unwrap();
        assert_eq!(found.handler.guid, "Borderlands 2");

        // No user folder and no handlers: an empty catalog, not an error.
        let empty = tempfile::tempdir().unwrap();
        let catalog = super::catalog(empty.path(), None).unwrap();
        assert!(catalog.handlers.is_empty() && catalog.games.is_empty());

        // A game list Nucleus wrote but the host cannot read is an error.
        std::fs::create_dir_all(empty.path().join("content")).unwrap();
        std::fs::write(empty.path().join("content").join("userprofile.json"), "{").unwrap();
        let error = format!("{:#}", super::catalog(empty.path(), None).unwrap_err());
        assert!(error.contains("userprofile.json"), "{error}");
    }

    #[test]
    fn rubylights_handler_is_written_and_its_game_added_beside_the_users() {
        let nucleus = tempfile::tempdir().unwrap();
        let appdata = tempfile::tempdir().unwrap();
        let handlers = nucleus.path().join("handlers");
        std::fs::create_dir_all(&handlers).unwrap();
        std::fs::write(handlers.join("Borderlands 2.js"), HANDLER).unwrap();
        let list = appdata.path().join("Nucleus Coop").join("userprofile.json");
        std::fs::create_dir_all(list.parent().unwrap()).unwrap();
        let users = "{\r\n  \"Games\": [\r\n    {\r\n      \"GameGuid\": \"Borderlands 2\",\r\n      \"ExePath\": \"D:\\\\BL2\\\\Borderlands2.exe\"\r\n    }\r\n  ]\r\n}";
        std::fs::write(&list, users).unwrap();
        let setup = Setup::new(r"D:\Puzzle\Puzzle.exe", "Puzzle", None);

        let handler = write_own(nucleus.path(), appdata.path(), &setup).unwrap();
        assert_eq!(handler.guid, setup.guid());
        assert!(handler.own);
        let file = handlers.join(setup.file());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), setup.handler());
        assert_eq!(
            std::fs::read_to_string(handlers.join("Borderlands 2.js")).unwrap(),
            HANDLER
        );
        let written = std::fs::read_to_string(&list).unwrap();
        // The user's game, byte for byte, then ours.
        let end = users.len() - "\r\n  ]\r\n}".len();
        assert!(written.starts_with(&users[..end]), "{written}");
        assert!(written[end..].starts_with(",\r\n    {"), "{written}");
        // Nucleus reads both, and co-op still has only the hub's handler.
        let catalog = catalog(nucleus.path(), Some(appdata.path())).unwrap();
        assert_eq!(catalog.games.len(), 2);
        assert!(catalog.handlers.iter().any(|h| h.own));
        assert!(catalog.find("Puzzle", Some(&setup.exe_path)).is_err());

        // Again: nothing changes. A changed template is rewritten.
        write_own(nucleus.path(), appdata.path(), &setup).unwrap();
        assert_eq!(std::fs::read_to_string(&list).unwrap(), written);
        std::fs::write(&file, format!("{}\r\n// v0", generic::MARKER)).unwrap();
        write_own(nucleus.path(), appdata.path(), &setup).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), setup.handler());
        assert_eq!(std::fs::read_to_string(&list).unwrap(), written);

        // A file with our name that someone else wrote stays as it is.
        std::fs::write(&file, HANDLER).unwrap();
        let error = write_own(nucleus.path(), appdata.path(), &setup)
            .unwrap_err()
            .to_string();
        assert!(error.contains("is not Rubylight's handler"), "{error}");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), HANDLER);

        // A game list the host cannot read is left alone, loudly.
        std::fs::write(&list, "{").unwrap();
        std::fs::remove_file(&file).unwrap();
        let error = format!(
            "{:#}",
            write_own(nucleus.path(), appdata.path(), &setup).unwrap_err()
        );
        assert!(error.contains("not readable"), "{error}");
        assert_eq!(std::fs::read_to_string(&list).unwrap(), "{");

        // No game list yet: Nucleus' folder and file are made.
        let fresh = tempfile::tempdir().unwrap();
        write_own(nucleus.path(), fresh.path(), &setup).unwrap();
        let games = handler::user_games(
            &std::fs::read_to_string(fresh.path().join("Nucleus Coop").join("userprofile.json"))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(games[0].guid, setup.guid());
    }

    #[test]
    fn ours_goes_once_the_user_adds_the_game_with_a_hub_handler() {
        let nucleus = tempfile::tempdir().unwrap();
        let appdata = tempfile::tempdir().unwrap();
        let handlers = nucleus.path().join("handlers");
        let setup = Setup::new(r"D:\BL2\Borderlands2.exe", "Borderlands 2", None);
        write_own(nucleus.path(), appdata.path(), &setup).unwrap();
        // The hub's handler comes, and the user adds the game with it.
        std::fs::write(handlers.join("Borderlands 2.js"), HANDLER).unwrap();
        let list = appdata.path().join("Nucleus Coop").join("userprofile.json");
        let ours = std::fs::read_to_string(&list).unwrap();
        std::fs::write(
            &list,
            generic::add_game(Some(&ours), "Borderlands 2", &setup.exe_path)
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        let before = catalog(nucleus.path(), Some(appdata.path())).unwrap();
        let superseded = before.superseded(None);
        assert_eq!(superseded.len(), 1);
        remove_own(nucleus.path(), appdata.path(), superseded[0]).unwrap();
        assert!(!handlers.join(setup.file()).exists());
        let after = catalog(nucleus.path(), Some(appdata.path())).unwrap();
        assert!(after.handlers.iter().all(|h| !h.own));
        assert_eq!(
            after
                .games
                .iter()
                .map(|g| g.guid.as_str())
                .collect::<Vec<_>>(),
            ["Borderlands 2"]
        );
        // A hub handler is never removed as ours.
        let error = remove_own(nucleus.path(), appdata.path(), &after.handlers[0]).unwrap_err();
        assert!(
            error.to_string().contains("is not Rubylight's handler"),
            "{error}"
        );
        assert!(handlers.join("Borderlands 2.js").exists());
    }

    #[test]
    fn a_handler_that_cannot_be_removed_keeps_its_game_and_is_found_again() {
        use std::os::windows::fs::OpenOptionsExt;
        let nucleus = tempfile::tempdir().unwrap();
        let appdata = tempfile::tempdir().unwrap();
        let setup = Setup::new(r"D:\Puzzle\Game.exe", "Puzzle", None);
        write_own(nucleus.path(), appdata.path(), &setup).unwrap();
        let file = nucleus.path().join("handlers").join(setup.file());
        let ours = || {
            catalog(nucleus.path(), Some(appdata.path()))
                .unwrap()
                .handlers
                .into_iter()
                .find(|h| h.own)
        };
        let own = ours().unwrap();
        // Open elsewhere without sharing: the removal fails, loudly.
        let open = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&file)
            .unwrap();
        let error = format!(
            "{:#}",
            remove_own(nucleus.path(), appdata.path(), &own).unwrap_err()
        );
        assert!(error.contains(&setup.file()), "{error}");
        drop(open);
        // The game stays listed with it, so the next start finds both again.
        let left = catalog(nucleus.path(), Some(appdata.path())).unwrap();
        assert!(left.games.iter().any(|g| g.guid == setup.guid()));
        assert!(ours().is_some());
        remove_own(nucleus.path(), appdata.path(), &own).unwrap();
        let after = catalog(nucleus.path(), Some(appdata.path())).unwrap();
        assert!(!file.exists());
        assert!(after.games.is_empty());
    }

    #[test]
    fn an_unused_handler_of_ours_is_swept_and_a_foreign_one_never() {
        let nucleus = tempfile::tempdir().unwrap();
        let appdata = tempfile::tempdir().unwrap();
        let setup = Setup::new(r"D:\Puzzle\Game.exe", "Puzzle", None);
        write_own(nucleus.path(), appdata.path(), &setup).unwrap();
        // A removal that took the game but not the handler, as before.
        let list = list(appdata.path());
        let text = std::fs::read_to_string(&list).unwrap();
        std::fs::write(
            &list,
            generic::remove_game(&text, &setup.guid()).unwrap().unwrap(),
        )
        .unwrap();
        let handlers = nucleus.path().join("handlers");
        std::fs::write(handlers.join("Other.js"), HANDLER).unwrap();
        // Another account with a list, and one without.
        let (theirs, empty) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let profiles = || Ok(vec![theirs.path().into(), empty.path().into()]);
        let theirs_list = super::list(&theirs.path().join("AppData").join("Roaming"));
        std::fs::create_dir_all(theirs_list.parent().unwrap()).unwrap();
        std::fs::write(&theirs_list, r#"{"Games":[]}"#).unwrap();
        let before = catalog_in(nucleus.path(), Some(appdata.path()), profiles()).unwrap();
        let stale = before.stale(None);
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0].guid, setup.guid());
        remove_own(nucleus.path(), appdata.path(), stale[0]).unwrap();
        assert!(!handlers.join(setup.file()).exists());
        assert!(handlers.join("Other.js").exists());
        let after = catalog_in(nucleus.path(), Some(appdata.path()), profiles()).unwrap();
        assert!(after.stale(None).is_empty());
    }

    #[test]
    fn ours_another_account_plays_stays() {
        let nucleus = tempfile::tempdir().unwrap();
        let (mine, theirs) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let setup = Setup::new(r"D:\Puzzle\Game.exe", "Puzzle", None);
        // Another account wrote it, into the shared handlers folder and its
        // own list in its profile.
        let their_appdata = theirs.path().join("AppData").join("Roaming");
        write_own(nucleus.path(), &their_appdata, &setup).unwrap();
        let catalog = |profiles: Result<Vec<PathBuf>>| {
            catalog_in(nucleus.path(), Some(mine.path()), profiles).unwrap()
        };
        assert!(
            catalog(Ok(vec![theirs.path().into()]))
                .stale(None)
                .is_empty()
        );
        // A list that can't be read, or no account list at all: none goes.
        assert!(
            catalog(Err(anyhow::anyhow!("access denied")))
                .stale(None)
                .is_empty()
        );
        std::fs::write(list(&their_appdata), "{").unwrap();
        let unread = catalog(Ok(vec![theirs.path().into()]));
        assert!(unread.everyone.is_none());
        assert!(unread.stale(None).is_empty());
        // Once their list no longer has it, it goes.
        std::fs::write(list(&their_appdata), r#"{"Games":[]}"#).unwrap();
        assert_eq!(
            catalog(Ok(vec![theirs.path().into()])).stale(None)[0].guid,
            setup.guid()
        );
    }

    #[test]
    fn a_game_entry_left_after_its_handler_went_is_taken_over_again() {
        use std::os::windows::fs::OpenOptionsExt;
        let nucleus = tempfile::tempdir().unwrap();
        let appdata = tempfile::tempdir().unwrap();
        let setup = Setup::new(r"D:\Puzzle\Game.exe", "Puzzle", None);
        let own = write_own(nucleus.path(), appdata.path(), &setup).unwrap();
        let list = list(appdata.path());
        let open = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&list)
            .unwrap();
        let error = format!(
            "{:#}",
            remove_own(nucleus.path(), appdata.path(), &own).unwrap_err()
        );
        drop(open);
        // The warning says what stays and why it does no harm.
        assert!(error.contains("still lists its game"), "{error}");
        assert!(
            error.contains("leaves a game whose handler is gone out"),
            "{error}"
        );
        let left = catalog_in(nucleus.path(), Some(appdata.path()), Ok(Vec::new())).unwrap();
        assert!(left.handlers.iter().all(|h| !h.own));
        assert!(left.stale(None).is_empty());
        assert!(left.playable().is_empty());
        // The game's next background stream writes the handler and keeps
        // the one entry.
        write_own(nucleus.path(), appdata.path(), &setup).unwrap();
        let again = catalog_in(nucleus.path(), Some(appdata.path()), Ok(Vec::new())).unwrap();
        assert_eq!(
            again
                .games
                .iter()
                .filter(|g| g.guid == setup.guid())
                .count(),
            1
        );
        assert!(again.handlers.iter().any(|h| h.own));
    }

    #[test]
    fn a_game_list_the_host_cannot_edit_is_refused_before_anything_changes() {
        let appdata = tempfile::tempdir().unwrap();
        let setup = Setup::new(r"D:\Puzzle\Game.exe", "Puzzle", None);
        // No list yet: a new one is written.
        check_list(appdata.path(), Some(&setup), &[]).unwrap();
        let list = list(appdata.path());
        std::fs::create_dir_all(list.parent().unwrap()).unwrap();
        std::fs::write(
            &list,
            r#"{"Games":[{"GameGuid":"A","ExePath":"C:\\A.exe"}]}"#,
        )
        .unwrap();
        check_list(appdata.path(), Some(&setup), &["Rubylight-x"]).unwrap();
        // A game list the catalog reads but no edit could keep whole.
        for broken in ["{", r#"{"Games":7}"#, "[1]"] {
            std::fs::write(&list, broken).unwrap();
            let error = format!(
                "{:#}",
                check_list(appdata.path(), Some(&setup), &[]).unwrap_err()
            );
            assert!(error.contains("userprofile.json"), "{broken}: {error}");
        }
        std::fs::write(&list, r#"{"Games":[{"GameGuid":}]}"#).unwrap();
        let error = format!(
            "{:#}",
            check_list(appdata.path(), None, &["Rubylight-x"]).unwrap_err()
        );
        assert!(error.contains("userprofile.json"), "{error}");
        assert_eq!(
            std::fs::read_to_string(&list).unwrap(),
            r#"{"Games":[{"GameGuid":}]}"#
        );
    }

    #[test]
    fn settings_are_forced_and_the_rest_kept() {
        let nucleus = tempfile::tempdir().unwrap();
        let path = nucleus.path().join("Settings.ini");
        std::fs::write(
            &path,
            "\u{feff}[Dev]\r\nUseXinputIndex=False\r\n[Misc]\r\nTheme=Dark\r\n",
        )
        .unwrap();
        write_settings(nucleus.path()).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with('\u{feff}'), "{text:?}");
        assert!(text.contains("UseXinputIndex=True"), "{text:?}");
        assert!(text.contains("DisableGameProfiles=False"), "{text:?}");
        assert!(text.contains("Theme=Dark"), "{text:?}");
        assert!(!text.contains("UseXinputIndex=False"), "{text:?}");

        let fresh = tempfile::tempdir().unwrap();
        write_settings(fresh.path()).unwrap();
        let text = std::fs::read_to_string(fresh.path().join("Settings.ini")).unwrap();
        assert!(
            text.contains("[Dev]") && text.contains("UseXinputIndex=True"),
            "{text:?}"
        );
    }

    #[test]
    fn the_profile_takes_its_own_number_or_the_next() {
        let nucleus = tempfile::tempdir().unwrap();
        let profiles = nucleus.path().join("game profiles").join("Borderlands 2");
        std::fs::create_dir_all(&profiles).unwrap();
        std::fs::write(profiles.join("Profile[1].json"), r#"{"Title":"Mine"}"#).unwrap();
        std::fs::write(profiles.join("Profile[2].json"), r#"{"Title":""}"#).unwrap();
        let screens = [SCREEN];
        let number = write_profile(
            nucleus.path(),
            "Borderlands 2",
            &[player(0), player(1)],
            &screens,
        )
        .unwrap();
        assert_eq!(number, 3);
        let written: serde_json::Value =
            serde_json::from_slice(&std::fs::read(profiles.join("Profile[3].json")).unwrap())
                .unwrap();
        assert_eq!(written["Title"], profile::TITLE);
        assert_eq!(written["Player(s)"], 2);
        assert_eq!(written["Data"][1]["GamepadGuid"], profile::gamepad_guid(1));
        assert_eq!(written["Data"][1]["MonitorBounds"]["X"], 6000);
        // The user's profiles are left as they were.
        assert_eq!(
            std::fs::read_to_string(profiles.join("Profile[1].json")).unwrap(),
            r#"{"Title":"Mine"}"#
        );

        // Next time the same file is rewritten, for one player.
        let again = write_profile(nucleus.path(), "Borderlands 2", &[player(2)], &screens).unwrap();
        assert_eq!(again, 3);
        let written: serde_json::Value =
            serde_json::from_slice(&std::fs::read(profiles.join("Profile[3].json")).unwrap())
                .unwrap();
        assert_eq!(written["Player(s)"], 1);
        assert_eq!(written["Data"][0]["GamepadGuid"], profile::gamepad_guid(2));
        assert_eq!(std::fs::read_dir(&profiles).unwrap().count(), 3);
    }

    #[test]
    fn a_foreign_profile_in_the_next_place_is_never_overwritten() {
        let nucleus = tempfile::tempdir().unwrap();
        let profiles = nucleus.path().join("game profiles").join("game");
        std::fs::create_dir_all(&profiles).unwrap();
        // Numbers 1 and 3: the list has two, so a new profile would be the 3rd.
        std::fs::write(profiles.join("Profile[1].json"), r#"{"Title":"A"}"#).unwrap();
        std::fs::write(profiles.join("Profile[3].json"), r#"{"Title":"B"}"#).unwrap();
        let error = write_profile(nucleus.path(), "game", &[player(0)], &[SCREEN])
            .unwrap_err()
            .to_string();
        assert!(error.contains("Profile[3].json"), "{error}");
        assert_eq!(
            std::fs::read_to_string(profiles.join("Profile[3].json")).unwrap(),
            r#"{"Title":"B"}"#
        );

        // Nucleus lists every file of the folder, so a stray one shifts the
        // places: a new profile there would never load and is taken back.
        let stray = nucleus.path().join("game profiles").join("stray");
        std::fs::create_dir_all(&stray).unwrap();
        std::fs::write(stray.join("Profile[1].json"), r#"{"Title":"A"}"#).unwrap();
        std::fs::write(stray.join("Profile[1].json.bak"), r#"{"Title":"A"}"#).unwrap();
        let error = write_profile(nucleus.path(), "stray", &[player(0)], &[SCREEN])
            .unwrap_err()
            .to_string();
        assert!(error.contains("would not load"), "{error}");
        assert!(
            error.contains("Profile[1].json, Profile[3].json, Profile[1].json.bak"),
            "{error}"
        );
        assert!(!stray.join("Profile[3].json").exists());
    }

    #[test]
    fn a_guid_that_names_another_folder_is_refused() {
        let nucleus = tempfile::tempdir().unwrap();
        for guid in ["..", r"..\..\x", "a/b", "C:x", " ", "a\nb"] {
            let error = write_profile(nucleus.path(), guid, &[player(0)], &[SCREEN])
                .unwrap_err()
                .to_string();
            assert!(
                error.contains("cannot name a profile folder"),
                "{guid:?}: {error}"
            );
        }
        assert!(!nucleus.path().join("game profiles").exists());
    }
}
