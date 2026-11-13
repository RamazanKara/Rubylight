//! The console's co-op: the game page's Play together panel and the Nucleus
//! Co-op panel. `GET /api/coop` shows the background mode and why, where
//! Nucleus Co-op stands (the folder chosen, or why it can't run from it, and
//! the release page the console links to), each app's co-op status and every
//! session the host has. Rubylight downloads nothing: the user unpacks the
//! release and `POST /api/coop/folder` checks the folder they chose and keeps
//! it as `nucleus_path`. `POST /api/coop/open` opens Nucleus Co-op's
//! window on the PC for adding a handler; `POST /api/coop/close` closes that
//! window again, and a session closes it first. `POST /api/coop/kick` removes a
//! player and `POST /api/coop/end` ends a session, and each ends the streams
//! they held. Console-only: API tokens cannot reach them.

use super::error;
use crate::{
    nucleus::{GameStatus, Install, Refusal},
    state::Shared,
};
use axum::{
    Json,
    http::{Method, StatusCode},
    response::{IntoResponse, Response},
};
use butterpollo_core::{
    native_api::coop as wire,
    nucleus::{
        coop::Phase,
        elevation::{self, Account, Need, Run, Script},
        handler::Missing,
        install::{RELEASE_PAGE, VERSION},
        mode::{Decision, KEY},
    },
};
use serde_json::{Value, json};
use std::sync::PoisonError;

pub(super) async fn handle(h: &Shared, method: &Method, path: &str, data: &Value) -> Response {
    let h = h.clone();
    let session = data
        .get("session")
        .and_then(Value::as_u64)
        .and_then(|id| u32::try_from(id).ok());
    match (method.as_str(), path) {
        ("GET", "/api/coop") => blocking(move || Ok(status(&h))).await,
        ("POST", "/api/coop/folder") => {
            let Some(path) = data.get("path").and_then(Value::as_str) else {
                return error(
                    StatusCode::BAD_REQUEST,
                    "path must be the folder Nucleus Co-op was unpacked into",
                );
            };
            let path = path.trim().to_string();
            match tokio::task::spawn_blocking(move || choose(&h, &path)).await {
                Ok(Ok(install)) => Json(install).into_response(),
                Ok(Err(refusal)) => refused(&refusal),
                Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
            }
        }
        ("POST", "/api/coop/open") => {
            let configured = configured(&h);
            match tokio::task::spawn_blocking(move || h.nucleus.open(&configured)).await {
                Ok(Ok(())) => Json(json!({})).into_response(),
                Ok(Err(refusal)) => refused(&refusal),
                Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
            }
        }
        ("POST", "/api/coop/close") => {
            match tokio::task::spawn_blocking(move || h.nucleus.close()).await {
                Ok(Ok(closed)) => Json(json!({"closed": closed})).into_response(),
                Ok(Err(refusal)) => refused(&refusal),
                Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
            }
        }
        ("POST", "/api/coop/kick") => {
            let slot = data
                .get("slot")
                .and_then(Value::as_u64)
                .and_then(|slot| usize::try_from(slot).ok());
            let (Some(session), Some(slot)) = (session, slot) else {
                return error(
                    StatusCode::BAD_REQUEST,
                    "session and slot must be the numbers the panel showed",
                );
            };
            blocking(move || {
                let device = h.nucleus.kick(session, slot).ok_or((
                    StatusCode::NOT_FOUND,
                    "There is no such co-op session, or no player in that slot.",
                ))?;
                // Kicked first, so the stream's slot is not held for it again.
                crate::nucleus::end_streams(&h, session, Some(&device));
                Ok(json!({"device": device}))
            })
            .await
        }
        ("POST", "/api/coop/end") => {
            let Some(session) = session else {
                return error(
                    StatusCode::BAD_REQUEST,
                    "session must be the number the panel showed",
                );
            };
            blocking(move || {
                if !h.nucleus.end(session) {
                    return Err((StatusCode::NOT_FOUND, "There is no such co-op session."));
                }
                crate::nucleus::end_streams(&h, session, None);
                Ok(json!({}))
            })
            .await
        }
        _ => error(StatusCode::BAD_REQUEST, "unknown API endpoint"),
    }
}

/// `nucleus_path` as the host's settings have it now.
fn configured(h: &Shared) -> String {
    crate::nucleus::configured(&h.config.read().unwrap_or_else(PoisonError::into_inner))
}

/// The console's Choose folder: `path` checked as a launch checks it, then
/// kept as `nucleus_path`; a refused folder saves nothing. An empty `path`
/// forgets the folder, unchecked: an earlier Rubylight's install runs again,
/// if any. What sessions left in the Nucleus Co-op left behind is put back
/// first ([`crate::nucleus::Registry::change_folder`]). A settings save that
/// changes `nucleus_path` comes here too.
pub(super) fn choose(h: &Shared, path: &str) -> Result<Value, Refusal> {
    // The settings file takes # as the start of a comment.
    if path.contains('#') {
        return Err(Refusal {
            status: 400,
            message: "Rubylight can't keep a folder with # in its path: rename the folder, or unpack the release into another one".into(),
        });
    }
    let checked = if path.is_empty() {
        None
    } else {
        Some(h.nucleus.check_folder(path)?)
    };
    h.nucleus.change_folder(&configured(h), path, || {
        super::config::save_setting(h, "nucleus_path", |_| Ok(path.to_string())).map_err(|e| {
            Refusal {
                status: 500,
                message: format!("Saving the Nucleus Co-op folder failed: {e:#}"),
            }
        })
    })?;
    let install = checked.unwrap_or_else(|| h.nucleus.install(path));
    if path.is_empty() {
        tracing::info!("Nucleus Co-op: folder forgotten in the console");
    } else {
        tracing::info!(folder = path, "Nucleus Co-op: folder chosen in the console");
    }
    Ok(install_json(&install, path, h.nucleus.notice(path)))
}

/// Where Nucleus Co-op stands, as the panels show it: `chosen` is
/// `nucleus_path`, for the folder field, and `notice` why the copy an
/// earlier Rubylight installed runs instead of it.
fn install_json(install: &Install, chosen: &str, notice: Option<String>) -> Value {
    let (state, path, version, problem, install_error) = match install {
        Install::Missing => ("missing", None, None, None, None),
        // An earlier Rubylight's install, which still runs.
        Install::Installed(path) => ("installed", Some(path), Some(VERSION), None, None),
        Install::Configured(path) => ("configured", Some(path), Some(VERSION), None, None),
        Install::Failed { problem, error } => (
            "failed",
            None,
            None,
            problem.map(|p| p.as_str()),
            Some(error.as_str()),
        ),
    };
    json!({
        "state": state,
        "path": path,
        "chosen": chosen,
        "version": version,
        "problem": problem,
        "error": install_error,
        "notice": notice,
        // Where the user downloads it: Rubylight downloads nothing.
        "release": {"version": VERSION, "page": RELEASE_PAGE},
    })
}

/// A refused co-op action: its status and the host's text.
fn refused(refusal: &Refusal) -> Response {
    error(
        StatusCode::from_u16(refusal.status).unwrap_or(StatusCode::SERVICE_UNAVAILABLE),
        &refusal.message,
    )
}

/// The panels: the background mode, the install, each app's co-op status
/// and the sessions.
fn status(h: &Shared) -> Value {
    let config = h
        .config
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    let setting = config.get(KEY, "");
    let decision = Decision::nucleus();
    let chosen = crate::nucleus::configured(&config);
    let install = install_json(
        &h.nucleus.install(&chosen),
        &chosen,
        h.nucleus.notice(&chosen),
    );
    let apps = h
        .apps
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    let unavailable = h.nucleus.unavailable();
    // Whether the account signed in on this PC elevates, or why not: None on
    // a host that runs no sessions, or when Rubylight could not tell.
    let account = unavailable.is_none().then(|| h.nucleus.account()).flatten();
    let refused_account = account.filter(|account| !account.elevates());
    // A hub handler (background streams and co-op) needs an elevated token:
    // without one, the refusal a launch would answer.
    let hub_refusal = refused_account
        .and_then(|account| elevation::choose(Script::Hub, Need::Nothing, account).err());
    let (games, catalog_error) = match h.nucleus.games(&apps, &config) {
        Some(Ok(statuses)) => (statuses.into_iter().map(Some).collect(), None),
        Some(Err(error)) => (vec![None; apps.len()], Some(error)),
        None => (vec![None; apps.len()], None),
    };
    let games: Vec<Value> = apps
        .iter()
        .zip(games)
        .filter(|(app, _)| crate::nucleus::names_executable(app))
        .filter_map(|(app, status)| {
            let uuid = app.extra.get("uuid")?.as_str()?;
            let missing_status = |missing: &Missing| match missing {
                Missing::NoExecutable { .. } => "no_executable",
                Missing::NoHandler { .. } => "no_handler",
                Missing::NotAdded { .. } => "not_added",
                Missing::AntiCheat { .. } => "anti_cheat",
                Missing::Refused { .. } => "refused",
            };
            // How a background stream of it runs Rubylight's own handler.
            let runs = match &status {
                Some(GameStatus::Own { run, .. }) => Some(match run {
                    Run::User => "user",
                    Run::Elevated => "elevated",
                }),
                _ => None,
            };
            let (status, reason, handler, players) = match status {
                // Started through a launcher: no handler can match it.
                _ if !crate::nucleus::starts_exe(app) || crate::nucleus::starts_launcher(app) => {
                    ("launcher", None, None, None)
                }
                None => ("unknown", None, None, None),
                Some(GameStatus::Ready { handler, players }) => {
                    ("ready", hub_refusal.clone(), Some(handler), Some(players))
                }
                Some(GameStatus::Missing(missing) | GameStatus::Own { missing, .. }) => (
                    missing_status(&missing),
                    Some(missing.to_string()),
                    None,
                    None,
                ),
                Some(GameStatus::Taken { entry }) => (
                    "taken",
                    Some(format!(
                        "Another app already lists as \"{entry}\", so the library can't show this one: rename one of them"
                    )),
                    None,
                    None,
                ),
            };
            Some(json!({
                "app": uuid,
                "name": app.name,
                "status": status,
                "reason": reason,
                "handler": handler,
                "players": players,
                "runs": runs,
                "entry": crate::nucleus::entry_name(&app.name),
            }))
        })
        .collect();
    let sessions: Vec<Value> = h
        .nucleus
        .sessions()
        .into_iter()
        .map(|listed| {
            let session = &listed.session;
            let (phase, end) = match &session.phase {
                Phase::Starting => (wire::Phase::Starting, None),
                Phase::Running => (wire::Phase::Running, None),
                Phase::Ended(end) => (
                    wire::Phase::Ended,
                    Some(crate::native_api::coop::end_of(end)),
                ),
            };
            let now = std::time::Instant::now();
            let players: Vec<Value> = session
                .players()
                .into_iter()
                .map(|player| {
                    // Whole seconds an away device has left to come back, rounded up.
                    let rejoin_in = session
                        .rejoin_left(player.slot, now)
                        .map(|left| left.as_millis().div_ceil(1000) as u64);
                    json!({
                        "slot": player.slot,
                        "device": player.device,
                        "name": player.name,
                        "away": player.away,
                        "rejoin_in": rejoin_in,
                    })
                })
                .collect();
            json!({
                "id": session.id,
                "kind": session.kind,
                "app_id": listed.app_id,
                "title": session.title,
                "game": session.game,
                "phase": phase,
                "end": end,
                "players": players,
                // Removed devices: they can't rejoin, so the console invites none of them.
                "kicked": session.kicked().collect::<Vec<_>>(),
            })
        })
        .collect();
    json!({
        "mode": {
            "setting": setting,
            "mode": decision.mode,
            "label": decision.label,
            "reason": decision.reason,
        },
        "install": install,
        "games": games,
        "catalog_error": catalog_error,
        "available": unavailable.is_none(),
        "reason": unavailable,
        // Whether Windows gives the signed-in account an elevated token,
        // which hub handlers, co-op and games that need administrator
        // rights run with; null when unknown.
        "elevates": account.map(Account::elevates),
        // An administrator Windows' Administrator protection gives none:
        // the console says what still runs, not to sign in as one.
        "admin_protection": account == Some(Account::Protected),
        // Why Open Nucleus Co-op on this PC would be refused: an account
        // Windows doesn't elevate, in the words
        // `butterpollo_windows::nucleus::session::open` refuses it with.
        "open_refusal": refused_account.map(butterpollo_windows::nucleus::session::needs_administrator),
        // The Nucleus Co-op window the console opened is still open.
        "opened": h.nucleus.opened(),
        "sessions": sessions,
    })
}

/// Nucleus Co-op's calls (its folder, its handlers, stopping a session) leave
/// the async executor; a refusal answers its status and text.
async fn blocking(
    work: impl FnOnce() -> Result<Value, (StatusCode, &'static str)> + Send + 'static,
) -> Response {
    match tokio::task::spawn_blocking(work).await {
        Ok(Ok(value)) => Json(value).into_response(),
        Ok(Err((status, message))) => error(status, message),
        Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

#[cfg(test)]
mod tests;
