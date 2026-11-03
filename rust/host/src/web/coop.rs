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
mod tests {
    use super::super::parity_tests::{request, value};
    use crate::nucleus::{Admission, Game, Held, fake};
    use crate::state::{Session, test_support::Fixture};
    use axum::http::StatusCode;
    use butterpollo_core::{
        config::Config,
        nucleus::{
            elevation::{self, Account},
            install::{Problem, Unusable},
        },
        rtsp::Negotiated,
        session::Role,
        state::{App, Client},
    };
    use serde_json::{Value, json};
    use std::path::PathBuf;
    use std::sync::Arc;

    /// A host with Nucleus Co-op (the fake) running a two-player Portal 2
    /// session: the fixture's device streams slot 0, a second device has a
    /// pending launch for slot 1. Returns the session's id.
    struct Coop {
        f: Fixture,
        pc: Arc<fake::Pc>,
        session: u32,
        stream: Arc<Session>,
        pending: String,
    }
    fn coop() -> Coop {
        let f = Fixture::new();
        let h = &f.host;
        let pc = Arc::new(fake::Pc::installed());
        h.nucleus.use_pc(pc.clone());
        let apps = vec![App {
            name: "Portal 2".into(),
            cmd: format!("\"{}\"", fake::EXE),
            working_dir: String::new(),
            prep: Vec::new(),
            computed_id: None,
            aliases: Vec::new(),
            extra: Default::default(),
        }];
        let entry = h.nucleus.entries(&apps, "").remove(0);
        let first = f.client(0);
        let mut second = first.clone();
        second.uuid = "second-device".into();
        second.name = "Second device".into();
        // Joining launches the co-op entry, so it needs Launch apps too.
        second.perm |= butterpollo_core::state::LAUNCH_APPS;
        second.extra.insert("coop_join".into(), true.into());
        let seated = |client: Client| {
            let mut launch = f.launch(client, Role::Stream);
            let seat = h
                .nucleus
                .admit(Admission {
                    launch: &launch.id,
                    client: &launch.client,
                    game: Game::Coop(&entry),
                    size: (1920, 1080),
                    config: &Config::default(),
                    warnings: Default::default(),
                    release_parked: &|| Ok(()),
                    instances: None,
                })
                .unwrap();
            launch.nucleus = Some(seat);
            launch.held = Some(Arc::new(Held::new(h, seat, &launch.id)));
            launch
        };
        let streaming = seated(first);
        let session = streaming.nucleus.unwrap().session;
        let stream = Session::new(streaming, Negotiated::default());
        let waiting = seated(second);
        let pending = waiting.id.clone();
        {
            let mut sessions = h.sessions.lock().unwrap();
            sessions
                .active
                .insert(stream.launch.id.clone(), stream.clone());
            sessions.pending.insert(pending.clone(), waiting);
        }
        assert_eq!(h.nucleus.sessions()[0].held, [true, true]);
        Coop {
            f,
            pc,
            session,
            stream,
            pending,
        }
    }

    #[tokio::test]
    async fn the_panel_shows_the_mode_the_install_and_the_sessions() {
        let c = coop();
        let body = value(
            request(&c.f, "GET", "/api/coop", Value::Null).await,
            StatusCode::OK,
        )
        .await;
        for key in ["setting", "mode", "label", "reason"] {
            assert!(body["mode"].get(key).is_some(), "{key}: {body}");
        }
        assert_eq!(
            body["install"],
            json!({"state": "installed", "path": r"C:\Nucleus", "chosen": "", "version": "2.4.2",
                   "problem": null, "error": null, "notice": null,
                   "release": {"version": "2.4.2", "page": "https://github.com/SplitScreen-Me/splitscreenme-nucleus/releases/tag/v2.4.2"}})
        );
        assert_eq!(
            (&body["available"], &body["reason"]),
            (&json!(true), &Value::Null)
        );
        let session = &body["sessions"][0];
        assert_eq!(session["id"], c.session);
        assert_eq!(session["kind"], "coop");
        assert_eq!(session["title"], "Portal 2");
        assert_eq!(session["game"], fake::GUID);
        assert_eq!(session["phase"], "starting");
        assert_eq!(session["end"], Value::Null);
        assert_eq!(session["kicked"], json!([]));
        assert_eq!(
            session["players"],
            json!([
                {"slot": 0, "device": "parity-device", "name": "Parity device", "away": false, "rejoin_in": null},
                {"slot": 1, "device": "second-device", "name": "Second device", "away": false, "rejoin_in": null},
            ])
        );
        // A device whose stream ended is away and shows how long it may come back.
        let h = &c.f.host;
        let seat = h.sessions.lock().unwrap().pending[&c.pending]
            .nucleus
            .unwrap();
        h.nucleus.leave(seat, &c.pending);
        let body = value(
            request(&c.f, "GET", "/api/coop", Value::Null).await,
            StatusCode::OK,
        )
        .await;
        let away = &body["sessions"][0]["players"][1];
        assert_eq!(away["away"], true);
        let left = away["rejoin_in"].as_u64().unwrap();
        assert!(
            (110..=120).contains(&left),
            "{left} of {}",
            butterpollo_core::nucleus::coop::GRACE.as_secs()
        );
        assert_eq!(body["sessions"][0]["players"][0]["rejoin_in"], Value::Null);
        assert_eq!(body["elevates"], true);
        assert_eq!(body["admin_protection"], false);
        assert_eq!(body["open_refusal"], Value::Null);
        // Unavailable on this host: the reason shows.
        let elevations = || c.pc.elevations.load(std::sync::atomic::Ordering::Relaxed);
        let asked = elevations();
        *c.pc.unavailable.lock().unwrap() = Some("not on this test PC".into());
        let body = value(
            request(&c.f, "GET", "/api/coop", Value::Null).await,
            StatusCode::OK,
        )
        .await;
        assert_eq!(body["available"], false);
        assert_eq!(body["reason"], "not on this test PC");
        // A host that runs no sessions doesn't ask whether the account elevates.
        assert_eq!(body["elevates"], Value::Null);
        assert_eq!(body["open_refusal"], Value::Null);
        assert_eq!(elevations(), asked);
    }

    #[tokio::test]
    async fn kick_ends_that_players_stream_only() {
        let c = coop();
        let h = &c.f.host;
        let body = value(
            request(
                &c.f,
                "POST",
                "/api/coop/kick",
                json!({"session": c.session, "slot": 1}),
            )
            .await,
            StatusCode::OK,
        )
        .await;
        assert_eq!(body, json!({"device": "second-device"}));
        assert!(!h.sessions.lock().unwrap().pending.contains_key(&c.pending));
        assert!(!c.stream.stopping());
        let listed = h.nucleus.sessions();
        assert_eq!(listed[0].held, [true, false]);
        assert_eq!(listed[0].session.players()[1].device, None);
        // The panel names the removed device, so the console never invites it back.
        let status = value(
            request(&c.f, "GET", "/api/coop", Value::Null).await,
            StatusCode::OK,
        )
        .await;
        assert_eq!(status["sessions"][0]["kicked"], json!(["second-device"]));
        // The slot is empty now; the player streaming slot 0 is kicked next.
        for (session, slot) in [(c.session, 1), (c.session + 1, 0)] {
            value(
                request(
                    &c.f,
                    "POST",
                    "/api/coop/kick",
                    json!({"session": session, "slot": slot}),
                )
                .await,
                StatusCode::NOT_FOUND,
            )
            .await;
        }
        value(
            request(
                &c.f,
                "POST",
                "/api/coop/kick",
                json!({"session": c.session, "slot": 0}),
            )
            .await,
            StatusCode::OK,
        )
        .await;
        assert!(c.stream.stopping());
        assert!(!c.pc.last().stopped());
    }

    #[tokio::test]
    async fn end_stops_nucleus_and_every_players_stream() {
        let c = coop();
        let h = &c.f.host;
        value(
            request(
                &c.f,
                "POST",
                "/api/coop/end",
                json!({"session": c.session + 1}),
            )
            .await,
            StatusCode::NOT_FOUND,
        )
        .await;
        assert!(!c.stream.stopping());
        let body = value(
            request(&c.f, "POST", "/api/coop/end", json!({"session": c.session})).await,
            StatusCode::OK,
        )
        .await;
        assert_eq!(body, json!({}));
        assert!(c.pc.last().stopped());
        assert!(c.stream.stopping());
        assert!(h.sessions.lock().unwrap().pending.is_empty());
        let body = value(
            request(&c.f, "GET", "/api/coop", Value::Null).await,
            StatusCode::OK,
        )
        .await;
        assert_eq!(body["sessions"][0]["phase"], "ended");
        assert_eq!(
            body["sessions"][0]["end"],
            json!({"reason": "ended", "detail": null})
        );
    }

    #[tokio::test]
    async fn kick_and_end_refuse_a_body_without_the_numbers() {
        let f = Fixture::new();
        for (path, body) in [
            ("/api/coop/kick", json!({"session": 1})),
            ("/api/coop/kick", json!({"session": "1", "slot": 0})),
            ("/api/coop/end", json!({"session": -1})),
            ("/api/coop/nothing", json!({})),
        ] {
            value(
                request(&f, "POST", path, body).await,
                StatusCode::BAD_REQUEST,
            )
            .await;
        }
    }

    async fn panel(f: &Fixture) -> Value {
        value(
            request(f, "GET", "/api/coop", Value::Null).await,
            StatusCode::OK,
        )
        .await
    }

    /// `nucleus_path` as an earlier Rubylight kept it, or as written into the
    /// settings file by hand: never checked.
    fn kept_by_hand(f: &Fixture, folder: &str) {
        super::super::config::save_setting(&f.host, "nucleus_path", |_| Ok(folder.into())).unwrap();
    }

    #[tokio::test]
    async fn a_settings_save_keeps_the_folder_as_the_panels_do() {
        let f = Fixture::new();
        let pc = Arc::new(fake::Pc::installed());
        let why = r"D:\Downloads holds no NucleusCoop.exe: choose the folder you unpacked Nucleus Co-op 2.4.2 into";
        pc.unusable.lock().unwrap().insert(
            r"D:\Downloads".into(),
            Unusable {
                problem: Problem::Missing,
                message: why.into(),
            },
        );
        f.host.nucleus.use_pc(pc.clone());
        let save = |body: Value| request(&f, "PATCH", "/api/config", body);
        // Refused with the reason, and nothing else in the save kept.
        let body = value(
            save(json!({"nucleus_path": r"D:\Downloads", "locale": "de"})).await,
            StatusCode::BAD_REQUEST,
        )
        .await;
        assert_eq!(body["error"], why);
        assert_eq!(kept(&f), "");
        assert_eq!(f.host.config.read().unwrap().get("locale", "en"), "en");
        let body = value(
            save(json!({"nucleus_path": r"D:\Games #2\Nucleus"})).await,
            StatusCode::BAD_REQUEST,
        )
        .await;
        assert!(
            body["error"].as_str().unwrap().contains("# in its path"),
            "{body}"
        );
        assert_eq!(kept(&f), "");
        // A setting the save gets wrong keeps no folder either.
        value(
            save(json!({"nucleus_path": r"D:\Nucleus", "stream_preset": "fastest"})).await,
            StatusCode::BAD_REQUEST,
        )
        .await;
        assert_eq!(kept(&f), "");
        // Checked and kept, with the rest of the save.
        value(
            save(json!({"nucleus_path": r" D:\Nucleus ", "locale": "de"})).await,
            StatusCode::OK,
        )
        .await;
        assert_eq!(kept(&f), r"D:\Nucleus");
        assert_eq!(f.host.config.read().unwrap().get("locale", "en"), "de");
        // The same folder again is no change, and a save without it leaves it.
        value(
            save(json!({"nucleus_path": r"D:\Nucleus"})).await,
            StatusCode::OK,
        )
        .await;
        value(save(json!({"locale": "en"})).await, StatusCode::OK).await;
        assert_eq!(kept(&f), r"D:\Nucleus");
        // Emptied, it is forgotten.
        value(save(json!({"nucleus_path": ""})).await, StatusCode::OK).await;
        assert_eq!(kept(&f), "");
    }

    fn kept(f: &Fixture) -> String {
        f.host
            .config
            .read()
            .unwrap()
            .get("nucleus_path", "")
            .to_string()
    }

    #[tokio::test]
    async fn a_chosen_folder_is_checked_kept_and_remembered_and_nothing_downloads() {
        let f = Fixture::new();
        let pc = Arc::new(fake::Pc::installed());
        *pc.installed.lock().unwrap() = None;
        f.host.nucleus.use_pc(pc.clone());
        let before = panel(&f).await;
        assert_eq!(before["install"]["state"], "missing");
        // The console links to the release's page: Rubylight downloads nothing.
        assert_eq!(
            before["install"]["release"],
            json!({"version": "2.4.2", "page": "https://github.com/SplitScreen-Me/splitscreenme-nucleus/releases/tag/v2.4.2"})
        );
        let chosen = value(
            request(
                &f,
                "POST",
                "/api/coop/folder",
                json!({"path": r" D:\Nucleus "}),
            )
            .await,
            StatusCode::OK,
        )
        .await;
        assert_eq!(chosen["state"], "configured");
        assert_eq!(chosen["path"], r"D:\Nucleus");
        assert_eq!(kept(&f), r"D:\Nucleus");
        // Saved with the host's settings, so it outlasts a restart.
        let saved = Config::load(&f.host.config_path).unwrap();
        assert_eq!(saved.get("nucleus_path", ""), r"D:\Nucleus");
        let after = panel(&f).await;
        assert_eq!(after["install"]["state"], "configured");
        assert_eq!(after["install"]["chosen"], r"D:\Nucleus");
        assert_eq!(after["install"]["version"], "2.4.2");
        // An empty folder goes back to an earlier install, here none.
        let cleared = value(
            request(&f, "POST", "/api/coop/folder", json!({"path": ""})).await,
            StatusCode::OK,
        )
        .await;
        assert_eq!(cleared["state"], "missing");
        assert_eq!(kept(&f), "");
        // Gone from the settings file too, not just from memory.
        let saved = Config::load(&f.host.config_path).unwrap();
        assert!(!saved.values.contains_key("nucleus_path"), "{saved:?}");
        assert_eq!(panel(&f).await["install"]["state"], "missing");
    }

    #[tokio::test]
    async fn a_folder_is_forgotten_unchecked_even_where_sessions_cant_run() {
        let f = Fixture::new();
        let pc = Arc::new(fake::Pc::installed());
        f.host.nucleus.use_pc(pc.clone());
        let chosen = || {
            request(
                &f,
                "POST",
                "/api/coop/folder",
                json!({"path": r"D:\Nucleus"}),
            )
        };
        value(chosen().await, StatusCode::OK).await;
        assert_eq!(kept(&f), r"D:\Nucleus");
        let put_back = pc.put_back.lock().unwrap().clone();
        // A folder refused now, or a host without the service: forgetting it
        // still works, never a 503.
        pc.unusable.lock().unwrap().insert(
            r"D:\Nucleus".into(),
            Unusable {
                problem: Problem::Untrusted,
                message: r"D:\Nucleus can be changed by S-1-5-11".into(),
            },
        );
        *pc.unavailable.lock().unwrap() = Some("not on this test PC".into());
        let cleared = value(
            request(&f, "POST", "/api/coop/folder", json!({"path": "  "})).await,
            StatusCode::OK,
        )
        .await;
        assert_eq!(cleared["state"], "installed");
        assert_eq!(cleared["path"], r"C:\Nucleus");
        assert_eq!(kept(&f), "");
        assert!(
            !Config::load(&f.host.config_path)
                .unwrap()
                .values
                .contains_key("nucleus_path")
        );
        // Nothing was put back into a folder that is refused, or on a host
        // that runs no sessions.
        assert_eq!(*pc.put_back.lock().unwrap(), put_back);
    }

    #[tokio::test]
    async fn a_hash_in_the_folder_is_refused_before_anything_is_checked() {
        let f = Fixture::new();
        let pc = Arc::new(fake::Pc::installed());
        *pc.unavailable.lock().unwrap() = Some("not on this test PC".into());
        f.host.nucleus.use_pc(pc.clone());
        let body = value(
            request(
                &f,
                "POST",
                "/api/coop/folder",
                json!({"path": r"D:\Games #2\Nucleus"}),
            )
            .await,
            StatusCode::BAD_REQUEST,
        )
        .await;
        assert_eq!(
            body["error"],
            "Rubylight can't keep a folder with # in its path: rename the folder, or unpack the release into another one"
        );
        assert_eq!(kept(&f), "");
    }

    #[tokio::test]
    async fn changing_the_folder_puts_back_what_the_one_left_kept() {
        let f = Fixture::new();
        let pc = Arc::new(fake::Pc::installed());
        f.host.nucleus.use_pc(pc.clone());
        let choose =
            |path: &'static str| request(&f, "POST", "/api/coop/folder", json!({ "path": path }));
        // From the earlier install to a folder chosen: the earlier one's
        // journals are put back first.
        value(choose(r"D:\Nucleus").await, StatusCode::OK).await;
        assert_eq!(*pc.put_back.lock().unwrap(), [PathBuf::from(r"C:\Nucleus")]);
        // The same folder again: nothing to put back.
        value(choose(r"D:\Nucleus").await, StatusCode::OK).await;
        assert_eq!(pc.put_back.lock().unwrap().len(), 1);
        // One that can't be put back keeps the folder, and says why.
        let why = r"D:\Nucleus\Rubylight-layers-S-1-5-21-9.json keeps the compatibility settings a Nucleus Co-op session changed for another Windows account: sign in to this PC as that account, so Rubylight puts them back, then choose the folder again";
        *pc.put_back_error.lock().unwrap() = Some(why.into());
        let body = value(choose(r"E:\Nucleus").await, StatusCode::CONFLICT).await;
        assert_eq!(body["error"], why);
        assert_eq!(kept(&f), r"D:\Nucleus");
        *pc.put_back_error.lock().unwrap() = None;
        value(choose("").await, StatusCode::OK).await;
        assert_eq!(
            *pc.put_back.lock().unwrap(),
            [PathBuf::from(r"C:\Nucleus"), PathBuf::from(r"D:\Nucleus")]
        );
        assert_eq!(kept(&f), "");
    }

    #[tokio::test]
    async fn the_folder_stays_while_a_session_runs_from_it() {
        let c = coop();
        let body = value(
            request(
                &c.f,
                "POST",
                "/api/coop/folder",
                json!({"path": r"D:\Nucleus"}),
            )
            .await,
            StatusCode::CONFLICT,
        )
        .await;
        assert_eq!(
            body["error"],
            "Nucleus Co-op is running Portal 2: end that session first, then choose the folder"
        );
        assert_eq!(kept(&c.f), "");
        assert!(c.pc.put_back.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn another_release_kept_runs_the_earlier_install_with_a_notice_and_is_never_chosen() {
        let f = Fixture::new();
        let pc = Arc::new(fake::Pc::installed());
        let why = r"The NucleusCoop.exe in D:\Old isn't Nucleus Co-op 2.4.2's: download 2.4.2 from its release page and choose its folder";
        let notice = format!(
            r"{why}. Until then Rubylight runs the Nucleus Co-op 2.4.2 an earlier Rubylight installed, in C:\Nucleus"
        );
        *pc.instead.lock().unwrap() = Some((
            Unusable {
                problem: Problem::Version,
                message: why.into(),
            },
            notice.clone(),
        ));
        f.host.nucleus.use_pc(pc.clone());
        // Kept by an earlier Rubylight: the earlier install runs, and says why.
        kept_by_hand(&f, r"D:\Old");
        let install = panel(&f).await["install"].clone();
        assert_eq!(install["state"], "configured");
        assert_eq!(install["path"], r"C:\Nucleus");
        assert_eq!(install["chosen"], r"D:\Old");
        assert_eq!(install["notice"], notice);
        // Chosen in the console: refused, as without the earlier install.
        let body = value(
            request(&f, "POST", "/api/coop/folder", json!({"path": r"D:\Other"})).await,
            StatusCode::BAD_REQUEST,
        )
        .await;
        assert_eq!(body["error"], why);
        assert_eq!(kept(&f), r"D:\Old");
    }

    #[tokio::test]
    async fn a_refused_folder_is_never_kept_and_says_why() {
        let f = Fixture::new();
        let pc = Arc::new(fake::Pc::installed());
        f.host.nucleus.use_pc(pc.clone());
        for (problem, why) in [
            (Problem::Missing, r"D:\Downloads holds no NucleusCoop.exe"),
            (
                Problem::Version,
                r"The NucleusCoop.exe in D:\Downloads isn't Nucleus Co-op 2.4.2's",
            ),
            (
                Problem::Untrusted,
                r"Rubylight doesn't run Nucleus Co-op from D:\Downloads",
            ),
        ] {
            pc.unusable.lock().unwrap().insert(
                r"D:\Downloads".into(),
                Unusable {
                    problem,
                    message: why.into(),
                },
            );
            let body = value(
                request(
                    &f,
                    "POST",
                    "/api/coop/folder",
                    json!({"path": r"D:\Downloads"}),
                )
                .await,
                StatusCode::BAD_REQUEST,
            )
            .await;
            assert_eq!(body["error"], why);
            // The earlier install still runs; nothing was saved.
            assert_eq!(kept(&f), "");
            assert_eq!(panel(&f).await["install"]["state"], "installed");
        }
        value(
            request(&f, "POST", "/api/coop/folder", json!({})).await,
            StatusCode::BAD_REQUEST,
        )
        .await;
    }

    #[tokio::test]
    async fn a_kept_folder_nucleus_cant_run_from_shows_and_refuses_open() {
        let f = Fixture::new();
        let pc = Arc::new(fake::Pc::installed());
        let why = r"The NucleusCoop.exe in D:\Old isn't Nucleus Co-op 2.4.2's: download 2.4.2 from its release page and choose its folder";
        pc.unusable.lock().unwrap().insert(
            r"D:\Old".into(),
            Unusable {
                problem: Problem::Version,
                message: why.into(),
            },
        );
        f.host.nucleus.use_pc(pc.clone());
        // Set where the console doesn't check it, or replaced since.
        kept_by_hand(&f, r"D:\Old");
        let body = panel(&f).await;
        assert_eq!(body["install"]["state"], "failed");
        assert_eq!(body["install"]["problem"], "version");
        assert_eq!(body["install"]["error"], why);
        assert_eq!(body["install"]["chosen"], r"D:\Old");
        // The earlier install doesn't stand in for it.
        assert_eq!(body["install"]["path"], Value::Null);
        let refused = value(
            request(&f, "POST", "/api/coop/open", json!({})).await,
            StatusCode::SERVICE_UNAVAILABLE,
        )
        .await;
        assert_eq!(
            refused["error"],
            format!("Nucleus Co-op is not ready: {why}")
        );
        assert_eq!(pc.opened.load(std::sync::atomic::Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn open_answers_503_when_the_folder_could_not_be_checked() {
        let f = Fixture::new();
        let pc = Arc::new(fake::Pc::installed());
        let why = r"reading the owner of C:\Nucleus: access denied";
        *pc.installed_error.lock().unwrap() = Some(why.into());
        f.host.nucleus.use_pc(pc.clone());
        let body = value(
            request(&f, "POST", "/api/coop/open", json!({})).await,
            StatusCode::SERVICE_UNAVAILABLE,
        )
        .await;
        assert_eq!(body["error"], format!("Nucleus Co-op is not ready: {why}"));
        assert_eq!(pc.opened.load(std::sync::atomic::Ordering::Relaxed), 0);
        let body = panel(&f).await;
        assert_eq!(body["install"]["state"], "failed");
        assert_eq!(body["install"]["problem"], Value::Null);
    }

    #[tokio::test]
    async fn choosing_a_folder_and_open_are_refused_without_the_service() {
        let f = Fixture::new();
        let pc = Arc::new(fake::Pc::installed());
        let why = "Nucleus Co-op sessions need the host to run as the Rubylight Windows service, which starts Nucleus Co-op as the signed-in user";
        *pc.unavailable.lock().unwrap() = Some(why.into());
        f.host.nucleus.use_pc(pc.clone());
        for (path, body) in [
            ("/api/coop/open", json!({})),
            ("/api/coop/folder", json!({"path": r"D:\Nucleus"})),
        ] {
            let body = value(
                request(&f, "POST", path, body).await,
                StatusCode::SERVICE_UNAVAILABLE,
            )
            .await;
            assert_eq!(body["error"], why, "{path}");
        }
        assert_eq!(pc.opened.load(std::sync::atomic::Ordering::Relaxed), 0);
        assert_eq!(kept(&f), "");
        // With the service it opens.
        *pc.unavailable.lock().unwrap() = None;
        value(
            request(&f, "POST", "/api/coop/open", json!({})).await,
            StatusCode::OK,
        )
        .await;
        assert_eq!(pc.opened.load(std::sync::atomic::Ordering::Relaxed), 1);
        let body = value(
            request(&f, "GET", "/api/coop", Value::Null).await,
            StatusCode::OK,
        )
        .await;
        assert_eq!(body["opened"], true);
        // Opening it again is refused; Close closes the one the console opened.
        value(
            request(&f, "POST", "/api/coop/open", json!({})).await,
            StatusCode::CONFLICT,
        )
        .await;
        let body = value(
            request(&f, "POST", "/api/coop/close", json!({})).await,
            StatusCode::OK,
        )
        .await;
        assert_eq!(body, json!({"closed": true}));
        assert!(!pc.window().open.load(std::sync::atomic::Ordering::Relaxed));
        let body = value(
            request(&f, "GET", "/api/coop", Value::Null).await,
            StatusCode::OK,
        )
        .await;
        assert_eq!(body["opened"], false);
    }

    #[tokio::test]
    async fn each_game_with_an_exe_shows_its_coop_status() {
        let f = Fixture::new();
        let pc = Arc::new(fake::Pc::installed());
        f.host.nucleus.use_pc(pc.clone());
        let app = |name: &str, cmd: &str, uuid: &str| App {
            name: name.into(),
            cmd: cmd.into(),
            working_dir: String::new(),
            prep: Vec::new(),
            computed_id: None,
            aliases: Vec::new(),
            extra: [("uuid".to_string(), json!(uuid))].into_iter().collect(),
        };
        *f.host.apps.write().unwrap() = vec![
            app("Desktop", "", "desktop"),
            app("Portal 2", &format!("\"{}\"", fake::EXE), "portal"),
            app("Puzzle", r"C:\Games\Puzzle.exe", "puzzle"),
            app(
                "Counter-Strike 2",
                "cmd /c start \"\" steam://rungameid/730",
                "cs2",
            ),
            app(
                "Portal",
                r#""C:\Program Files (x86)\Steam\steam.exe" -applaunch 400"#,
                "portal1",
            ),
        ];
        let dir = std::env::temp_dir().join(format!("coop-runs-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let (ice, fort) = (dir.join("Ice.exe"), dir.join("Fort.exe"));
        std::fs::write(&ice, b"").unwrap();
        std::fs::write(&fort, b"").unwrap();
        pc.needs.lock().unwrap().insert(
            fort.to_string_lossy().into_owned(),
            butterpollo_core::nucleus::elevation::Need::Administrator(
                butterpollo_core::nucleus::elevation::Why::Manifest,
            ),
        );
        f.host.apps.write().unwrap().extend([
            app("Ice", &format!("\"{}\"", ice.display()), "ice"),
            app("Fort", &format!("\"{}\"", fort.display()), "fort"),
        ]);
        let body = value(
            request(&f, "GET", "/api/coop", Value::Null).await,
            StatusCode::OK,
        )
        .await;
        let no_handler = |name: &str| {
            format!(
                "No Nucleus Co-op handler for {name}.exe: add one in Nucleus Co-op's handler hub"
            )
        };
        assert_eq!(
            body["games"],
            json!([
                {"app": "portal", "name": "Portal 2", "status": "ready", "reason": null,
                 "handler": "Portal 2", "players": 2, "runs": null, "entry": "Portal 2 (Co-op)"},
                // Without a handler, as a background launch would answer.
                {"app": "puzzle", "name": "Puzzle", "status": "refused",
                 "reason": r"C:\Games\Puzzle.exe is not on this PC: set the app's command to the game's .exe",
                 "handler": null, "players": null, "runs": null, "entry": "Puzzle (Co-op)"},
                {"app": "cs2", "name": "Counter-Strike 2", "status": "launcher", "reason": null,
                 "handler": null, "players": null, "runs": null, "entry": "Counter-Strike 2 (Co-op)"},
                {"app": "portal1", "name": "Portal", "status": "launcher", "reason": null,
                 "handler": null, "players": null, "runs": null, "entry": "Portal (Co-op)"},
                // Rubylight's own handler: as the user, or elevated as the
                // game asks.
                {"app": "ice", "name": "Ice", "status": "no_handler", "reason": no_handler("Ice"),
                 "handler": null, "players": null, "runs": "user", "entry": "Ice (Co-op)"},
                {"app": "fort", "name": "Fort", "status": "no_handler", "reason": no_handler("Fort"),
                 "handler": null, "players": null, "runs": "elevated", "entry": "Fort (Co-op)"},
            ])
        );
        assert_eq!(body["catalog_error"], Value::Null);
        assert_eq!(body["elevates"], true);
        // On an account Windows doesn't elevate: the hub handler's game says
        // so before a launch, in the launch's words; ours as the user stays.
        *pc.account.lock().unwrap() = Account::Standard;
        f.host.nucleus.use_pc(pc.clone());
        let body = value(
            request(&f, "GET", "/api/coop", Value::Null).await,
            StatusCode::OK,
        )
        .await;
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(body["elevates"], false);
        assert_eq!(body["admin_protection"], false);
        // Open Nucleus Co-op on this PC says why before it is pressed.
        assert_eq!(
            body["open_refusal"],
            "Nucleus Co-op needs administrator rights, and Windows doesn't give this account any: sign in to Windows on this PC as an administrator"
        );
        let games = body["games"].as_array().unwrap();
        assert_eq!(
            (&games[0]["status"], &games[0]["reason"]),
            (
                &json!("ready"),
                &json!(format!(
                    "Nucleus Co-op needs administrator rights for a hub handler, and {}",
                    elevation::STANDARD
                ))
            )
        );
        assert_eq!(
            (&games[4]["status"], &games[4]["reason"], &games[4]["runs"]),
            (
                &json!("no_handler"),
                &json!(no_handler("Ice")),
                &json!("user")
            )
        );
        assert_eq!(games[5]["status"], "refused");
        assert!(
            games[5]["reason"]
                .as_str()
                .unwrap()
                .ends_with(elevation::STANDARD),
            "{}",
            games[5]
        );
        // An administrator under Windows' Administrator protection is told
        // what can't run and what still does, never to turn it off.
        *pc.account.lock().unwrap() = Account::Protected;
        f.host.nucleus.use_pc(pc.clone());
        let body = value(
            request(&f, "GET", "/api/coop", Value::Null).await,
            StatusCode::OK,
        )
        .await;
        assert_eq!(body["elevates"], false);
        assert_eq!(body["admin_protection"], true);
        assert_eq!(
            body["open_refusal"],
            "Nucleus Co-op needs administrator rights, and Windows' Administrator protection keeps them from Rubylight, so Nucleus Co-op can't open on this PC; games Rubylight adds itself still stream if they need no administrator rights"
        );
        assert_eq!(
            body["games"][0]["reason"],
            "Nucleus Co-op needs administrator rights for a hub handler, and Windows' Administrator protection keeps them from Rubylight, so Nucleus Co-op can't run this game on this PC; games Rubylight adds itself still stream if they need no administrator rights"
        );
        // Before Nucleus Co-op is set up nothing is known yet.
        *pc.installed.lock().unwrap() = None;
        let body = value(
            request(&f, "GET", "/api/coop", Value::Null).await,
            StatusCode::OK,
        )
        .await;
        assert_eq!(body["games"][0]["status"], "unknown");
        assert_eq!(body["games"][1]["status"], "unknown");
    }

    #[tokio::test]
    async fn a_second_app_of_the_same_name_is_not_ready_as_the_library_lists_one_entry() {
        let f = Fixture::new();
        let pc = Arc::new(fake::Pc::installed());
        f.host.nucleus.use_pc(pc.clone());
        let app = |uuid: &str| App {
            name: "Portal 2".into(),
            cmd: format!("\"{}\"", fake::EXE),
            working_dir: String::new(),
            prep: Vec::new(),
            computed_id: None,
            aliases: Vec::new(),
            extra: [("uuid".to_string(), json!(uuid))].into_iter().collect(),
        };
        let apps = vec![app("first"), app("second")];
        *f.host.apps.write().unwrap() = apps.clone();
        assert_eq!(f.host.nucleus.entries(&apps, "").len(), 1);
        let body = value(
            request(&f, "GET", "/api/coop", Value::Null).await,
            StatusCode::OK,
        )
        .await;
        assert_eq!(body["games"][0]["status"], "ready");
        assert_eq!(body["games"][1]["status"], "taken");
        assert_eq!(
            body["games"][1]["reason"],
            "Another app already lists as \"Portal 2 (Co-op)\", so the library can't show this one: rename one of them"
        );
    }

    #[test]
    fn api_tokens_cannot_reach_the_coop_panel() {
        assert!(
            !super::super::token_catalog()
                .iter()
                .any(|scope| scope.path.contains("coop"))
        );
    }
}
