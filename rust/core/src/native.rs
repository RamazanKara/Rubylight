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
use crate::native_api::session::{
    Action, ActionId, LauncherNotice, Owner, RadeonProfile, Session, SessionApp, State,
};
use crate::remote;
use crate::state::{App, Client};

/// List apps, the same bits as `authenticated()` checks in `nvhttp.rs`.
pub const LIST: u32 = 1 << 24;
pub const VIEW: u32 = 1 << 25;
pub const LAUNCH: u32 = 1 << 26;
/// Upload files and Download files, Apollo's bits; the host's File transfer
/// setting must be on too.
pub const FILE_UPLOAD: u32 = 1 << 18;
pub const FILE_DOWNLOAD: u32 = 1 << 19;

/// The games the host's last launcher scan found installed, by `game_id`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Scan {
    installed: BTreeSet<String>,
    /// Source keys the scan did not read (turned off): their games' state is unknown.
    skipped: BTreeSet<String>,
    /// The game folders the scan read, when it read any: a game folder outside them
    /// (a folder since removed or left out) has an unknown state.
    folders: Option<crate::library::Settings>,
}
impl Scan {
    pub fn new(game_ids: impl IntoIterator<Item = String>) -> Self {
        Self {
            installed: game_ids.into_iter().collect(),
            skipped: BTreeSet::new(),
            folders: None,
        }
    }
    /// The scan did not read these sources, so it says nothing about their games.
    pub fn skipping(mut self, sources: impl IntoIterator<Item = Source>) -> Self {
        self.skipped
            .extend(sources.into_iter().map(|source| source.key().to_owned()));
        self
    }
    /// The scan read the game folders of these settings and no others.
    pub fn reading(mut self, settings: crate::library::Settings) -> Self {
        self.folders = Some(settings);
        self
    }
    /// Whether the game is installed, `None` when the scan did not read its source
    /// or, for a game folder, the folder.
    pub fn installed(&self, game_id: &str) -> Option<bool> {
        let (source, id) = game_id.split_once(':').unwrap_or((game_id, ""));
        let unread = self.skipped.contains(source)
            || (source == Source::Local.key()
                && self
                    .folders
                    .as_ref()
                    .is_some_and(|settings| !settings.reads_local(id)));
        (!unread).then(|| self.contains(game_id))
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
    /// Also installed: games the launcher scan leaves out, such as Steam tools.
    pub fn adding(mut self, game_ids: impl IntoIterator<Item = String>) -> Self {
        self.installed.extend(game_ids);
        self
    }
}

/// The read-time app field the console shows as "Not installed". Never stored.
pub const NOT_INSTALLED: &str = "not-installed";
/// Whether the app's launcher game is installed: `None` for an app without a launcher
/// id, or one whose source or folder the scan did not read.
pub fn installed(app: &App, scan: &Scan) -> Option<bool> {
    store_of(app).and_then(|(_, game_id)| scan.installed(&game_id))
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
            (Some(game_id), Some(scan)) => scan.installed(game_id),
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
    /// A launcher shows a prompt in place of the game. The host adds it; the machine does not
    /// know it.
    pub launcher: Option<LauncherNotice>,
    /// The Radeon profile of the running app, as applied. The host adds it too.
    pub radeon_profile: Option<RadeonProfile>,
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
            launcher: None,
            radeon_profile: None,
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
        // What the host noticed about the owner's own game is the owner's to see.
        launcher: snapshot.launcher.clone().filter(|_| details),
        radeon_profile: snapshot.radeon_profile.clone().filter(|_| details),
        actions,
    }
}

#[cfg(test)]
mod tests;
