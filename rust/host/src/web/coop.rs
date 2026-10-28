//! The console's co-op: the game page's Play together panel and the Nucleus
//! Co-op panel. `GET /api/coop` shows the background mode and why, Nucleus
//! Co-op's install with its download progress, each app's co-op status and
//! every session the host has. `POST /api/coop/install` sets Nucleus Co-op
//! up (the pinned, checked release) and `POST /api/coop/open` opens its
//! window on the PC for adding a handler; `POST /api/coop/close` closes that
//! window again, and a session closes it first. `POST /api/coop/kick` removes a
//! player and `POST /api/coop/end` ends a session, and each ends the streams
//! they held. Console-only: API tokens cannot reach them.

use super::error;
use crate::{
    nucleus::{GameStatus, Install, Refusal, SetUp},
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
        handler::Missing,
        install::{SIZE, VERSION},
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
        ("POST", "/api/coop/install") => {
            let configured = configured(&h);
            // One blocking task claims the install and runs it, so a dropped
            // request can't leave it claimed with nothing running it.
            let (answer, set_up) = tokio::sync::oneshot::channel();
            tokio::task::spawn_blocking(move || {
                let started = h.nucleus.set_up(&configured);
                let run = matches!(started, Ok(SetUp::Started));
                let _ = answer.send(started);
                if run {
                    h.nucleus.install_now(&configured);
                }
            });
            match set_up.await {
                Ok(Ok(SetUp::Started)) => {
                    // The download runs on; `GET /api/coop` shows its progress
                    // and, loudly, a refusal.
                    (StatusCode::ACCEPTED, Json(json!({"state": "installing"}))).into_response()
                }
                Ok(Ok(SetUp::Installing)) => {
                    (StatusCode::ACCEPTED, Json(json!({"state": "installing"}))).into_response()
                }
                Ok(Ok(SetUp::Installed)) => Json(json!({"state": "installed"})).into_response(),
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
    let (state, path, version, install_error, received) =
        match h.nucleus.install(&crate::nucleus::configured(&config)) {
            Install::Missing => ("missing", None, None, None, None),
            Install::Installed(path) => ("installed", Some(path), Some(VERSION), None, None),
            // The user's own copy: its version is theirs.
            Install::Configured(path) => ("configured", Some(path), None, None, None),
            Install::Installing(received) => ("installing", None, None, None, Some(received)),
            Install::Failed(why) => ("failed", None, None, Some(why), None),
        };
    let apps = h
        .apps
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
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
            let (status, reason, handler, players) = match status {
                // Started through a launcher: no handler can match it.
                _ if !crate::nucleus::starts_exe(app) => ("launcher", None, None, None),
                None => ("unknown", None, None, None),
                Some(GameStatus::Ready { handler, players }) => {
                    ("ready", None, Some(handler), Some(players))
                }
                Some(GameStatus::Missing(missing)) => {
                    let status = match missing {
                        Missing::NoExecutable { .. } => "no_executable",
                        Missing::NoHandler { .. } => "no_handler",
                        Missing::NotAdded { .. } => "not_added",
                    };
                    (status, Some(missing.to_string()), None, None)
                }
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
                "entry": crate::nucleus::entry_name(&app.name),
            }))
        })
        .collect();
    let unavailable = h.nucleus.unavailable();
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
        "install": {
            "state": state,
            "path": path,
            "version": version,
            "error": install_error,
            // What Set up co-op downloads, and how much of it has arrived.
            "release": {"version": VERSION, "size": SIZE},
            "received": received,
        },
        "games": games,
        "catalog_error": catalog_error,
        "available": unavailable.is_none(),
        "reason": unavailable,
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
        rtsp::Negotiated,
        session::Role,
        state::{App, Client},
    };
    use serde_json::{Value, json};
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
            json!({"state": "installed", "path": r"C:\Nucleus", "version": butterpollo_core::nucleus::install::VERSION, "error": null,
                   "release": {"version": butterpollo_core::nucleus::install::VERSION, "size": butterpollo_core::nucleus::install::SIZE},
                   "received": null})
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
        // Unavailable on this host: the reason shows.
        *c.pc.unavailable.lock().unwrap() = Some("not on this test PC".into());
        let body = value(
            request(&c.f, "GET", "/api/coop", Value::Null).await,
            StatusCode::OK,
        )
        .await;
        assert_eq!(body["available"], false);
        assert_eq!(body["reason"], "not on this test PC");
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

    /// `GET /api/coop` until the install is no longer downloading.
    async fn settled(f: &Fixture) -> Value {
        for _ in 0..500 {
            let body = value(
                request(f, "GET", "/api/coop", Value::Null).await,
                StatusCode::OK,
            )
            .await;
            if body["install"]["state"] != "installing" {
                return body;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("the install never finished");
    }

    #[tokio::test]
    async fn set_up_refuses_a_download_with_another_hash_loudly() {
        let f = Fixture::new();
        let pc = Arc::new(fake::Pc::installed());
        *pc.installed.lock().unwrap() = None;
        // The pinned size, another SHA-256.
        let size = butterpollo_core::nucleus::install::SIZE;
        *pc.archive.lock().unwrap() = Some(vec![0; size as usize]);
        f.host.nucleus.use_pc(pc.clone());
        let before = value(
            request(&f, "GET", "/api/coop", Value::Null).await,
            StatusCode::OK,
        )
        .await;
        assert_eq!(before["install"]["state"], "missing");
        assert_eq!(
            before["install"]["release"],
            json!({"version": butterpollo_core::nucleus::install::VERSION, "size": size})
        );
        let started = value(
            request(&f, "POST", "/api/coop/install", json!({})).await,
            StatusCode::ACCEPTED,
        )
        .await;
        assert_eq!(started, json!({"state": "installing"}));
        let body = settled(&f).await;
        assert_eq!(body["install"]["state"], "failed");
        let error = body["install"]["error"].as_str().unwrap();
        assert!(
            error.starts_with(
                "Nucleus Co-op is not ready: The Nucleus Co-op download's SHA-256 is "
            ),
            "{error}"
        );
        assert!(error.ends_with("it was deleted."), "{error}");
        assert_eq!(*pc.installed.lock().unwrap(), None);
        // A good download next time installs it, once.
        *pc.archive.lock().unwrap() = None;
        value(
            request(&f, "POST", "/api/coop/install", json!({})).await,
            StatusCode::ACCEPTED,
        )
        .await;
        assert_eq!(settled(&f).await["install"]["state"], "installed");
        let again = value(
            request(&f, "POST", "/api/coop/install", json!({})).await,
            StatusCode::OK,
        )
        .await;
        assert_eq!(again, json!({"state": "installed"}));
        assert_eq!(pc.installs.load(std::sync::atomic::Ordering::Relaxed), 2);
    }

    #[tokio::test]
    async fn a_set_up_request_dropped_after_its_first_poll_still_installs() {
        let f = Fixture::new();
        let pc = Arc::new(fake::Pc::installed());
        *pc.installed.lock().unwrap() = None;
        f.host.nucleus.use_pc(pc.clone());
        // The client hangs up at once: the request is polled once and dropped.
        let dropped = tokio::time::timeout(
            std::time::Duration::ZERO,
            request(&f, "POST", "/api/coop/install", json!({})),
        )
        .await;
        assert!(
            dropped.is_err(),
            "the request was answered before it was dropped"
        );
        // The install it claimed still runs, rather than staying "installing".
        let mut state = Value::Null;
        for _ in 0..500 {
            state = settled(&f).await["install"]["state"].clone();
            if state == "installed" || state == "failed" {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(state, "installed");
        assert_eq!(pc.installs.load(std::sync::atomic::Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn set_up_and_open_answer_503_when_the_install_folder_is_refused() {
        let f = Fixture::new();
        let pc = Arc::new(fake::Pc::installed());
        let why = r"C:\Nucleus is not owned by Administrators or SYSTEM";
        *pc.installed_error.lock().unwrap() = Some(why.into());
        f.host.nucleus.use_pc(pc.clone());
        for path in ["/api/coop/open", "/api/coop/install"] {
            let body = value(
                request(&f, "POST", path, json!({})).await,
                StatusCode::SERVICE_UNAVAILABLE,
            )
            .await;
            assert_eq!(
                body["error"],
                format!("Nucleus Co-op is not ready: {why}"),
                "{path}"
            );
        }
        // Nothing opened, nothing downloaded over the refused folder.
        assert_eq!(pc.opened.load(std::sync::atomic::Ordering::Relaxed), 0);
        assert_eq!(pc.installs.load(std::sync::atomic::Ordering::Relaxed), 0);
        let body = value(
            request(&f, "GET", "/api/coop", Value::Null).await,
            StatusCode::OK,
        )
        .await;
        assert_eq!(body["install"]["state"], "failed");
    }

    #[tokio::test]
    async fn set_up_and_open_are_refused_without_the_service() {
        let f = Fixture::new();
        let pc = Arc::new(fake::Pc::installed());
        let why = "Nucleus Co-op sessions need the host to run as the Rubylight Windows service, which starts Nucleus Co-op as the signed-in user";
        *pc.unavailable.lock().unwrap() = Some(why.into());
        f.host.nucleus.use_pc(pc.clone());
        for path in ["/api/coop/open", "/api/coop/install"] {
            let body = value(
                request(&f, "POST", path, json!({})).await,
                StatusCode::SERVICE_UNAVAILABLE,
            )
            .await;
            assert_eq!(body["error"], why, "{path}");
        }
        assert_eq!(pc.opened.load(std::sync::atomic::Ordering::Relaxed), 0);
        assert_eq!(pc.installs.load(std::sync::atomic::Ordering::Relaxed), 0);
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
        ];
        let body = value(
            request(&f, "GET", "/api/coop", Value::Null).await,
            StatusCode::OK,
        )
        .await;
        assert_eq!(
            body["games"],
            json!([
                {"app": "portal", "name": "Portal 2", "status": "ready", "reason": null,
                 "handler": "Portal 2", "players": 2, "entry": "Portal 2 (Co-op)"},
                {"app": "puzzle", "name": "Puzzle", "status": "no_handler",
                 "reason": "No Nucleus Co-op handler for Puzzle.exe: add one in Nucleus Co-op's handler hub",
                 "handler": null, "players": null, "entry": "Puzzle (Co-op)"},
                {"app": "cs2", "name": "Counter-Strike 2", "status": "launcher", "reason": null,
                 "handler": null, "players": null, "entry": "Counter-Strike 2 (Co-op)"},
            ])
        );
        assert_eq!(body["catalog_error"], Value::Null);
        // Before the install nothing is known yet.
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
