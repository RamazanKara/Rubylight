//! The console's Nucleus Co-op panel. `GET /api/coop` shows the background
//! mode and why, Nucleus Co-op's install and every session the host has;
//! `POST /api/coop/kick` removes a player and `POST /api/coop/end` ends a
//! session, and each ends the streams they held. Console-only: API tokens
//! cannot reach them.

use super::error;
use crate::{nucleus::Install, state::Shared};
use axum::{
    Json,
    http::{Method, StatusCode},
    response::{IntoResponse, Response},
};
use butterpollo_core::{
    native_api::coop as wire,
    nucleus::{
        coop::Phase,
        install::VERSION,
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

/// The panel: the background mode, the install and the sessions.
fn status(h: &Shared) -> Value {
    let config = h
        .config
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    let setting = config.get(KEY, "");
    let decision = Decision::nucleus();
    let (state, path, version, install_error) =
        match h.nucleus.install(&crate::nucleus::configured(&config)) {
            Install::Missing => ("missing", None, None, None),
            Install::Installed(path) => ("installed", Some(path), Some(VERSION), None),
            // The user's own copy: its version is theirs.
            Install::Configured(path) => ("configured", Some(path), None, None),
            Install::Installing => ("installing", None, None, None),
            Install::Failed(why) => ("failed", None, None, Some(why)),
        };
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
            let players: Vec<Value> = session
                .players()
                .into_iter()
                .map(|player| {
                    json!({
                        "slot": player.slot,
                        "device": player.device,
                        "name": player.name,
                        "away": player.away,
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
        },
        "available": unavailable.is_none(),
        "reason": unavailable,
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
            json!({"state": "installed", "path": r"C:\Nucleus", "version": butterpollo_core::nucleus::install::VERSION, "error": null})
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
        assert_eq!(
            session["players"],
            json!([
                {"slot": 0, "device": "parity-device", "name": "Parity device", "away": false},
                {"slot": 1, "device": "second-device", "name": "Second device", "away": false},
            ])
        );
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

    #[test]
    fn api_tokens_cannot_reach_the_coop_panel() {
        assert!(
            !super::super::token_catalog()
                .iter()
                .any(|scope| scope.path.contains("coop"))
        );
    }
}
