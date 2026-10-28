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
/// an unreadable game list is an error.
pub fn catalog(folder: &Path, appdata: Option<&Path>) -> Result<Catalog> {
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
    Ok(catalog)
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
    match read_existing(&path)? {
        Some(old) if old == text => {}
        Some(old) if !generic::owns(&old) => bail!(
            "{} is not Rubylight's handler, so Rubylight leaves it as it is: move it out of Nucleus Co-op's handlers folder",
            path.display()
        ),
        _ => {
            std::fs::write(&path, &text).with_context(|| format!("writing {}", path.display()))?;
            tracing::info!(
                file,
                exe = setup.exe_path,
                "Nucleus Co-op: Rubylight's one-player handler written"
            );
        }
    }
    let profile = appdata.join("Nucleus Coop").join("userprofile.json");
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
    let temporary = path.with_extension("json.rubylight");
    std::fs::write(&temporary, new).with_context(|| format!("writing {}", temporary.display()))?;
    std::fs::rename(&temporary, path).with_context(|| format!("replacing {}", path.display()))?;
    tracing::info!(
        guid = setup.guid(),
        "Nucleus Co-op: game added to the user's game list"
    );
    Ok(())
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
