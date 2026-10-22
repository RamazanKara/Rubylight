//! Native API v1 projections: the host's state turned into the types of
//! [`crate::native_api`].
//!
//! Pure functions over plain inputs, so the rules are tested here and the
//! host's handlers only gather the facts. See `docs/native-api.md`.
use std::collections::BTreeSet;

use serde_json::Value;

use crate::library::Source;
use crate::native_api::error::ErrorCode;
use crate::native_api::library::{Artwork, Game, Library, Store};
use crate::native_api::session::{Action, ActionId, Owner, Session, SessionApp, State};
use crate::remote;
use crate::state::{App, Client};

/// List apps, the same bits as `authenticated()` checks in `nvhttp.rs`.
pub const LIST: u32 = 1 << 24;
pub const VIEW: u32 = 1 << 25;
pub const LAUNCH: u32 = 1 << 26;

/// The games the host's last launcher scan found installed, by `game_id`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Scan {
    installed: BTreeSet<String>,
}
impl Scan {
    pub fn new(game_ids: impl IntoIterator<Item = String>) -> Self {
        Self {
            installed: game_ids.into_iter().collect(),
        }
    }
    pub fn from_games(games: &[crate::library::Game]) -> Self {
        Self::new(
            games
                .iter()
                .map(|game| format!("{}:{}", game.source.key(), game.id)),
        )
    }
    pub fn contains(&self, game_id: &str) -> bool {
        self.installed.contains(game_id)
    }
}

/// The store an app belongs to and its `<source>:<id>` key, from
/// `launcher-source` and `launcher-id`, or `steam-id` for Steam apps.
fn store_of(app: &App) -> Option<(Store, String)> {
    fn text<'a>(app: &'a App, key: &str) -> Option<&'a str> {
        app.extra
            .get(key)
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
    }
    if let (Some(source), Some(id)) = (text(app, "launcher-source"), text(app, "launcher-id")) {
        let name = Source::ALL
            .into_iter()
            .find(|candidate| candidate.key() == source)
            .map_or_else(|| source.to_owned(), |known| known.name().to_owned());
        return Some((
            Store {
                source: source.into(),
                id: id.into(),
                name,
            },
            format!("{source}:{id}"),
        ));
    }
    let mut keys = serde_json::Map::new();
    for key in ["steam-id", "uuid"] {
        if let Some(value) = app.extra.get(key) {
            keys.insert(key.into(), value.clone());
        }
    }
    let id = crate::steam::app_id_of(&Value::Object(keys))?.to_string();
    Some((
        Store {
            source: Source::Steam.key().into(),
            name: Source::Steam.name().into(),
            id: id.clone(),
        },
        format!("steam:{id}"),
    ))
}

/// The app's `uuid` from `apps.json`. An app without one gets a v5 UUID of its
/// name, which is stable until the app is renamed.
pub fn install_id(app: &App) -> String {
    match app.extra.get("uuid").and_then(Value::as_str) {
        Some(uuid) if !uuid.trim().is_empty() => uuid.into(),
        _ => uuid::Uuid::new_v5(
            &uuid::Uuid::NAMESPACE_URL,
            format!("rubylight:app:{}", app.name).as_bytes(),
        )
        .to_string(),
    }
}

fn hash(bytes: &[u8]) -> String {
    format!("{:08x}", crc32fast::hash(bytes))
}

/// [`library_with`] with the app's `image-path` as the cover fingerprint.
pub fn library(apps: &[App], scan: Option<&Scan>, current: Option<u32>) -> Library {
    library_with(apps, scan, current, |app| {
        app.extra
            .get("image-path")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned()
    })
}

/// The host's configured apps in console order, without the synthetic Remote
/// session tiles. `scan` is the cached launcher scan (`None` before the first
/// one), `current` the id of the host's current game. `cover` returns what
/// identifies the cover image file, such as its path, size and mtime: the
/// artwork `version` changes when it does.
pub fn library_with(
    apps: &[App],
    scan: Option<&Scan>,
    current: Option<u32>,
    cover: impl Fn(&App) -> String,
) -> Library {
    let mut games = Vec::with_capacity(apps.len());
    for app in apps {
        let install_id = install_id(app);
        let id = app.id();
        if remote::identify(id, &install_id).is_some() {
            continue;
        }
        let (store, game_id) = match store_of(app) {
            Some((store, game_id)) => (Some(store), Some(game_id)),
            None => (None, None),
        };
        let installed = match (&game_id, scan) {
            (Some(game_id), Some(scan)) => Some(scan.contains(game_id)),
            _ => None,
        };
        games.push(Game {
            install_id,
            app_id: id,
            title: app.name.clone(),
            game_id,
            store,
            installed,
            running: false,
            artwork: Artwork {
                cover: Some(format!("/appasset?appid={id}&AssetType=2&AssetIdx=0")),
                version: hash(cover(app).as_bytes()),
            },
        });
    }
    // The revision leaves `running` out, so starting a game doesn't make
    // every client refetch the covers.
    let revision = hash(&serde_json::to_vec(&games).unwrap_or_default());
    for game in &mut games {
        game.running = current == Some(game.app_id);
    }
    Library { revision, games }
}

/// What the host's session state machine reports right now
/// ([`crate::session_state::Machine::snapshot`]). `session_view` turns it into
/// the wire type for one caller.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionSnapshot {
    pub revision: u64,
    pub state: State,
    /// There is a running game, whoever owns it. After End session it is true
    /// while the state is `idle`.
    pub game_running: bool,
    /// The uuid of the device that owns the session. Always set unless the
    /// state is `idle`.
    pub owner: Option<String>,
    pub app: Option<SessionApp>,
    pub since_unix_ms: Option<u64>,
    pub suspended_until_unix_ms: Option<u64>,
}
impl Default for SessionSnapshot {
    fn default() -> Self {
        Self {
            revision: 0,
            state: State::Idle,
            game_running: false,
            owner: None,
            app: None,
            since_unix_ms: None,
            suspended_until_unix_ms: None,
        }
    }
}

/// Whether the administrator granted this device Host control. The rule
/// lives in [`Client::allows_host_control`].
pub fn host_control(client: &Client) -> bool {
    client.allows_host_control()
}

/// The session as `caller` may see it, with the actions it may run.
pub fn session_view(snapshot: &SessionSnapshot, caller: &Client) -> Session {
    let state = snapshot.state;
    let idle = state == State::Idle;
    // The host always names the owner of a session that isn't idle. Without
    // one, every caller would read as another device.
    debug_assert!(
        idle || snapshot.owner.is_some(),
        "a {state:?} session needs an owner"
    );
    let owns = !idle && snapshot.owner.as_deref() == Some(caller.uuid.as_str());
    let owner = if idle {
        Owner::None
    } else if owns {
        Owner::You
    } else {
        Owner::OtherDevice
    };
    let can_see = caller.allows(VIEW) || caller.allows(LAUNCH);
    let details = owns && can_see;
    let host_control = host_control(caller);
    let reason = |id: ActionId| -> Option<ErrorCode> {
        // Another device's session is refused whatever the permissions.
        if !idle && !owns {
            return Some(ErrorCode::NotSessionOwner);
        }
        match id {
            ActionId::Resume => {
                if !caller.allows(VIEW) {
                    return Some(ErrorCode::PermissionRequired);
                }
                match state {
                    // While streaming too: `/resume` supersedes the same
                    // device's stream, which is how it reconnects after a
                    // crash.
                    State::Suspended | State::Streaming => None,
                    State::Idle => Some(ErrorCode::NoSession),
                    State::Preparing | State::Launching | State::Stopping => {
                        Some(ErrorCode::SessionBusy)
                    }
                    State::Unknown => Some(ErrorCode::Internal),
                }
            }
            ActionId::Disconnect => {
                if !can_see {
                    return Some(ErrorCode::PermissionRequired);
                }
                match state {
                    State::Streaming => None,
                    State::Idle => Some(ErrorCode::NoSession),
                    _ => Some(ErrorCode::NotStreaming),
                }
            }
            ActionId::End => {
                if !caller.allows(LAUNCH) {
                    return Some(ErrorCode::PermissionRequired);
                }
                match state {
                    State::Idle => Some(ErrorCode::NoSession),
                    // The launch holds `launch_transition` for minutes.
                    State::Preparing | State::Launching => Some(ErrorCode::SessionBusy),
                    // Stopping: repeating End is safe.
                    _ => None,
                }
            }
            ActionId::CloseGame => {
                if !caller.allows(LAUNCH) {
                    return Some(ErrorCode::PermissionRequired);
                }
                if !host_control {
                    return Some(ErrorCode::HostControlRequired);
                }
                match state {
                    State::Preparing | State::Launching => Some(ErrorCode::SessionBusy),
                    // With no session, it closes a game End session left
                    // behind.
                    State::Idle if !snapshot.game_running => Some(ErrorCode::NoSession),
                    _ => None,
                }
            }
            ActionId::Unknown => Some(ErrorCode::Internal),
        }
    };
    let actions = [
        ActionId::Resume,
        ActionId::Disconnect,
        ActionId::End,
        ActionId::CloseGame,
    ]
    .into_iter()
    .map(|id| {
        let reason = reason(id);
        Action {
            id,
            allowed: reason.is_none(),
            reason,
        }
    })
    .collect();
    Session {
        revision: snapshot.revision,
        state,
        owner,
        app: snapshot.app.clone().filter(|_| details),
        game_running: snapshot.game_running,
        since_unix_ms: snapshot.since_unix_ms.filter(|_| details),
        suspended_until_unix_ms: snapshot
            .suspended_until_unix_ms
            .filter(|_| details && state == State::Suspended),
        actions,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn app(value: Value) -> App {
        serde_json::from_value(value).unwrap()
    }
    fn client(perm: u32, extra: Value) -> Client {
        let mut client = json!({"name": "Phone", "cert": "", "uuid": "me", "perm": perm});
        client
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        serde_json::from_value(client).unwrap()
    }
    fn dota() -> App {
        app(json!({
            "name": "Dota 2", "uuid": "c6f1a9a4-4b51-5f43-9d0b-7a2b1c2d3e4f",
            "launcher-source": "steam", "launcher-id": "570", "image-path": "dota.png"
        }))
    }
    fn game(id: u32) -> SessionApp {
        SessionApp {
            install_id: "c6f1a9a4".into(),
            app_id: id,
            title: "Dota 2".into(),
        }
    }
    fn snapshot(state: State, owner: &str) -> SessionSnapshot {
        SessionSnapshot {
            revision: 9,
            state,
            game_running: true,
            owner: Some(owner.into()),
            app: Some(game(5)),
            since_unix_ms: Some(10),
            suspended_until_unix_ms: Some(20),
            ..Default::default()
        }
    }
    fn suspended(owner: &str) -> SessionSnapshot {
        snapshot(State::Suspended, owner)
    }
    fn allowed(session: &Session, id: ActionId) -> bool {
        session.actions.iter().find(|a| a.id == id).unwrap().allowed
    }
    fn reason(session: &Session, id: ActionId) -> Option<ErrorCode> {
        session.actions.iter().find(|a| a.id == id).unwrap().reason
    }
    const ALL: u32 = LIST | VIEW | LAUNCH;

    #[test]
    fn library_ids_and_store() {
        let library = library(&[dota()], None, None);
        let game = &library.games[0];
        assert_eq!(game.install_id, "c6f1a9a4-4b51-5f43-9d0b-7a2b1c2d3e4f");
        assert_eq!(game.title, "Dota 2");
        assert_eq!(game.game_id.as_deref(), Some("steam:570"));
        let store = game.store.as_ref().unwrap();
        assert_eq!(
            (
                store.source.as_str(),
                store.id.as_str(),
                store.name.as_str()
            ),
            ("steam", "570", "Steam")
        );
        assert_eq!(game.app_id, dota().id());
        assert_eq!(
            game.artwork.cover.as_deref(),
            Some(format!("/appasset?appid={}&AssetType=2&AssetIdx=0", dota().id()).as_str())
        );
        assert_eq!(game.installed, None);
        assert!(!game.running);
    }

    #[test]
    fn manual_apps_have_no_game_id_store_or_installed_flag() {
        let desktop =
            app(json!({"name": "Desktop", "uuid": "8d4b3f55-0c2e-4f8a-9a61-2f1c6f0e9b17"}));
        let scan = Scan::new(["steam:570".into()]);
        let library = library(&[desktop], Some(&scan), None);
        let game = &library.games[0];
        assert!(game.game_id.is_none() && game.store.is_none() && game.installed.is_none());
    }

    #[test]
    fn installed_comes_from_the_scan() {
        let gone = app(
            json!({"name": "Gone", "uuid": "g", "launcher-source": "epic", "launcher-id": "x"}),
        );
        let scan = Scan::new(["steam:570".into()]);
        let library = library(&[dota(), gone], Some(&scan), None);
        assert_eq!(library.games[0].installed, Some(true));
        assert_eq!(library.games[1].installed, Some(false));
        assert_eq!(library.games[1].store.as_ref().unwrap().name, "Epic Games");
        let none = super::library(&[dota()], None, None);
        assert_eq!(none.games[0].installed, None);
    }

    #[test]
    fn scan_from_discovered_games() {
        let found = crate::library::Game::new(
            Source::Gog,
            "1207658930".into(),
            "Witcher".into(),
            std::path::Path::new("C:/GOG/Witcher"),
            None,
        );
        let scan = Scan::from_games(&[found]);
        assert!(scan.contains("gog:1207658930"));
        assert!(!scan.contains("gog:1"));
    }

    #[test]
    fn steam_apps_without_launcher_keys_use_steam_id() {
        let steam = app(json!({"name": "Half-Life", "uuid": "x", "steam-id": "70"}));
        let game = &library(&[steam], None, None).games[0];
        assert_eq!(game.game_id.as_deref(), Some("steam:70"));
        assert_eq!(game.store.as_ref().unwrap().source, "steam");
        let canonical = app(json!({"name": "Dota", "uuid": crate::steam::canonical_uuid(570)}));
        assert_eq!(
            library(&[canonical], None, None).games[0]
                .game_id
                .as_deref(),
            Some("steam:570")
        );
    }

    #[test]
    fn running_marks_the_current_game_only() {
        let other = app(json!({"name": "Other", "uuid": "o"}));
        let current = dota().id();
        let library = library(&[dota(), other], None, Some(current));
        assert!(library.games[0].running);
        assert!(!library.games[1].running);
    }

    #[test]
    fn control_tiles_are_left_out() {
        let tiles = [
            app(json!({"name": "Resume", "uuid": "9a1c5a25-58fe-40e0-b9aa-7d3f00000001"})),
            app(json!({"name": "Terminate", "uuid": "t", "id": 2147483504})),
        ];
        let library = library(&[tiles[0].clone(), dota(), tiles[1].clone()], None, None);
        assert_eq!(library.games.len(), 1);
        assert_eq!(library.games[0].title, "Dota 2");
    }

    #[test]
    fn order_is_the_console_order() {
        let a = app(json!({"name": "B", "uuid": "1"}));
        let b = app(json!({"name": "A", "uuid": "2"}));
        let titles: Vec<_> = library(&[a, b], None, None)
            .games
            .into_iter()
            .map(|g| g.title)
            .collect();
        assert_eq!(titles, ["B", "A"]);
    }

    #[test]
    fn install_id_survives_a_rename_and_a_missing_uuid_is_derived() {
        let mut renamed = dota();
        renamed.name = "Dota 2 (beta)".into();
        assert_eq!(install_id(&renamed), install_id(&dota()));
        let bare = app(json!({"name": "Bare"}));
        assert_eq!(install_id(&bare), install_id(&bare));
        assert_ne!(
            install_id(&bare),
            install_id(&app(json!({"name": "Other"})))
        );
    }

    #[test]
    fn revision_ignores_running_and_follows_everything_else() {
        let apps = [dota()];
        let idle = library(&apps, None, None);
        let running = library(&apps, None, Some(dota().id()));
        assert_eq!(idle.revision, running.revision);
        assert_eq!(idle.revision.len(), 8);
        let mut renamed = dota();
        renamed.name = "Dota 3".into();
        assert_ne!(library(&[renamed], None, None).revision, idle.revision);
        let scan = Scan::new(["steam:570".into()]);
        assert_ne!(library(&apps, Some(&scan), None).revision, idle.revision);
    }

    #[test]
    fn artwork_version_follows_the_cover_fingerprint() {
        let apps = [dota()];
        let a = library_with(&apps, None, None, |_| "dota.png|10|1".into());
        let b = library_with(&apps, None, None, |_| "dota.png|11|1".into());
        assert_ne!(a.games[0].artwork.version, b.games[0].artwork.version);
        assert_ne!(a.revision, b.revision);
    }

    #[test]
    fn host_control_is_off_unless_true() {
        assert!(!host_control(&client(u32::MAX, json!({}))));
        assert!(host_control(&client(0, json!({"host_control": true}))));
        assert!(host_control(&client(0, json!({"host_control": "true"}))));
        for value in [
            json!(false),
            json!("yes"),
            json!(1),
            json!("1"),
            json!(null),
            json!(""),
        ] {
            assert!(
                !host_control(&client(u32::MAX, json!({"host_control": value}))),
                "{value}"
            );
        }
        let mut disabled = client(0, json!({"host_control": true}));
        disabled.enabled = false;
        assert!(!host_control(&disabled));
    }

    #[test]
    fn the_spec_example_for_a_suspended_owner() {
        let session = session_view(&suspended("me"), &client(ALL, json!({})));
        assert_eq!(session.state, State::Suspended);
        assert_eq!(session.owner, Owner::You);
        assert!(session.app.is_some());
        assert_eq!(session.suspended_until_unix_ms, Some(20));
        let actions: Vec<_> = session
            .actions
            .iter()
            .map(|a| (a.id, a.allowed, a.reason))
            .collect();
        assert_eq!(
            actions,
            [
                (ActionId::Resume, true, None),
                (ActionId::Disconnect, false, Some(ErrorCode::NotStreaming)),
                (ActionId::End, true, None),
                (
                    ActionId::CloseGame,
                    false,
                    Some(ErrorCode::HostControlRequired)
                ),
            ]
        );
    }

    #[test]
    fn a_streaming_owner_can_disconnect_end_and_close_with_host_control() {
        let caller = client(ALL, json!({"host_control": true}));
        let session = session_view(&snapshot(State::Streaming, "me"), &caller);
        assert_eq!(session.state, State::Streaming);
        // Resume supersedes the same device's stream: the reconnect after a
        // crash.
        for id in [
            ActionId::Resume,
            ActionId::Disconnect,
            ActionId::End,
            ActionId::CloseGame,
        ] {
            assert!(allowed(&session, id), "{id:?}");
        }
        assert_eq!(session.suspended_until_unix_ms, None);
        assert_eq!(session.since_unix_ms, Some(10));
    }

    #[test]
    fn another_device_sees_state_and_owner_only() {
        let caller = client(ALL, json!({"host_control": true}));
        for snapshot in [
            snapshot(State::Streaming, "someone"),
            suspended("someone"),
        ] {
            let session = session_view(&snapshot, &caller);
            assert_eq!(session.owner, Owner::OtherDevice);
            assert!(session.app.is_none());
            assert!(session.since_unix_ms.is_none() && session.suspended_until_unix_ms.is_none());
            assert!(session.game_running);
            for action in &session.actions {
                assert!(!action.allowed, "{:?}", action.id);
                assert_eq!(action.reason, Some(ErrorCode::NotSessionOwner));
            }
        }
    }

    #[test]
    fn without_view_or_launch_the_app_is_hidden_and_actions_need_permission() {
        let caller = client(LIST, json!({"host_control": true}));
        let session = session_view(&suspended("me"), &caller);
        assert_eq!(session.owner, Owner::You);
        assert!(session.app.is_none());
        for action in &session.actions {
            assert!(!action.allowed);
            assert_eq!(
                action.reason,
                Some(ErrorCode::PermissionRequired),
                "{:?}",
                action.id
            );
        }
        // View alone shows the app and may resume and disconnect, not end.
        let viewer = client(VIEW, json!({}));
        let session = session_view(&snapshot(State::Streaming, "me"), &viewer);
        assert!(session.app.is_some());
        assert!(allowed(&session, ActionId::Disconnect));
        assert_eq!(
            reason(&session, ActionId::End),
            Some(ErrorCode::PermissionRequired)
        );
        assert_eq!(
            reason(&session, ActionId::CloseGame),
            Some(ErrorCode::PermissionRequired)
        );
    }

    #[test]
    fn a_disabled_device_may_do_nothing() {
        let mut caller = client(ALL, json!({"host_control": true}));
        caller.enabled = false;
        let session = session_view(&suspended("me"), &caller);
        assert!(session.app.is_none());
        assert!(session.actions.iter().all(|a| !a.allowed));
    }

    #[test]
    fn idle_has_nothing_to_resume_end_or_disconnect() {
        let caller = client(ALL, json!({"host_control": true}));
        let session = session_view(&SessionSnapshot::default(), &caller);
        assert_eq!(session.state, State::Idle);
        assert_eq!(session.owner, Owner::None);
        assert!(session.app.is_none() && !session.game_running);
        for id in [
            ActionId::Resume,
            ActionId::Disconnect,
            ActionId::End,
            ActionId::CloseGame,
        ] {
            assert!(!allowed(&session, id), "{id:?}");
            assert_eq!(reason(&session, id), Some(ErrorCode::NoSession), "{id:?}");
        }
    }

    #[test]
    fn close_game_works_without_a_session_when_a_game_is_left_running() {
        let left = SessionSnapshot {
            game_running: true,
            revision: 4,
            ..Default::default()
        };
        let with = client(ALL, json!({"host_control": true}));
        let session = session_view(&left, &with);
        assert_eq!(session.state, State::Idle);
        assert!(session.game_running);
        assert!(allowed(&session, ActionId::CloseGame));
        assert!(!allowed(&session, ActionId::End));
        // Without host control or Launch it stays closed.
        let session = session_view(&left, &client(ALL, json!({})));
        assert_eq!(
            reason(&session, ActionId::CloseGame),
            Some(ErrorCode::HostControlRequired)
        );
        let session = session_view(&left, &client(VIEW, json!({"host_control": true})));
        assert_eq!(
            reason(&session, ActionId::CloseGame),
            Some(ErrorCode::PermissionRequired)
        );
    }

    #[test]
    fn a_launch_in_progress_is_busy_for_every_action() {
        let caller = client(ALL, json!({"host_control": true}));
        for expected in [State::Preparing, State::Launching] {
            let session = session_view(&snapshot(expected, "me"), &caller);
            assert_eq!(session.state, expected);
            for id in [ActionId::Resume, ActionId::End, ActionId::CloseGame] {
                assert!(!allowed(&session, id), "{expected:?} {id:?}");
                assert_eq!(
                    reason(&session, id),
                    Some(ErrorCode::SessionBusy),
                    "{expected:?} {id:?}"
                );
            }
            assert_eq!(
                reason(&session, ActionId::Disconnect),
                Some(ErrorCode::NotStreaming)
            );
        }
    }

    #[test]
    fn stopping_allows_end_and_close_game_again_but_not_resume() {
        let caller = client(ALL, json!({"host_control": true}));
        let session = session_view(&snapshot(State::Stopping, "me"), &caller);
        assert_eq!(session.state, State::Stopping);
        assert!(allowed(&session, ActionId::End));
        assert!(allowed(&session, ActionId::CloseGame));
        assert_eq!(
            reason(&session, ActionId::Resume),
            Some(ErrorCode::SessionBusy)
        );
    }

    #[test]
    fn resume_needs_view() {
        let launcher = client(LAUNCH, json!({}));
        let session = session_view(&snapshot(State::Streaming, "me"), &launcher);
        assert_eq!(
            reason(&session, ActionId::Resume),
            Some(ErrorCode::PermissionRequired)
        );
        assert!(allowed(&session, ActionId::End));
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "needs an owner")]
    fn a_session_without_an_owner_is_a_host_bug() {
        let mut orphan = suspended("me");
        orphan.owner = None;
        session_view(&orphan, &client(ALL, json!({})));
    }

    #[test]
    fn an_idle_session_needs_no_owner() {
        let session = session_view(&SessionSnapshot::default(), &client(ALL, json!({})));
        assert_eq!(session.owner, Owner::None);
    }

    #[test]
    fn a_blank_uuid_is_replaced_like_a_missing_one() {
        let blank = app(json!({"name": "Blank", "uuid": "  "}));
        let missing = app(json!({"name": "Blank"}));
        assert_eq!(install_id(&blank), install_id(&missing));
        assert_ne!(install_id(&blank), "  ");
    }

    #[test]
    fn the_session_serialises_like_the_spec_example() {
        let session = session_view(&suspended("me"), &client(ALL, json!({})));
        let value = serde_json::to_value(&session).unwrap();
        assert_eq!(value["state"], "suspended");
        assert_eq!(value["owner"], "you");
        assert_eq!(
            value["actions"][1],
            json!({"id": "disconnect", "allowed": false, "reason": "not_streaming"})
        );
        assert_eq!(
            value["actions"][0],
            json!({"id": "resume", "allowed": true})
        );
    }
}
