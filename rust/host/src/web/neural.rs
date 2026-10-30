//! The game page's Neural Rendering switch. `GET /api/neural?uuid=` shows
//! whether this PC can have it and why not, whether the user accepted the
//! notice, and the app's install with its progress. `POST /api/neural/accept`
//! records the acceptance, `POST /api/neural/enable` (`uuid`, optional `exe`)
//! downloads and installs, `POST /api/neural/disable` removes. Console-only:
//! API tokens cannot reach them.

use super::error;
use crate::{
    neural::{self, Refusal},
    state::Shared,
};
use axum::{
    Json,
    http::{Method, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};

pub(super) async fn handle(
    h: &Shared,
    method: &Method,
    path: &str,
    uri: &Uri,
    data: &Value,
) -> Response {
    let h = h.clone();
    let uuid = data["uuid"].as_str().unwrap_or_default().to_owned();
    let answer = match (method.as_str(), path) {
        ("GET", "/api/neural") => {
            let uuid = url::form_urlencoded::parse(uri.query().unwrap_or("").as_bytes())
                .find(|(key, _)| key == "uuid")
                .map(|(_, value)| value.into_owned())
                .unwrap_or_default();
            return match tokio::task::spawn_blocking(move || neural::status(&h, &uuid)).await {
                Ok(status) => Json(status).into_response(),
                Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
            };
        }
        ("POST", "/api/neural/accept") => {
            return match neural::accept(&h) {
                Ok(()) => Json(json!({"status": true})).into_response(),
                Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, &format!("{e:#}")),
            };
        }
        ("POST", "/api/neural/enable") => {
            let exe = data["exe"]
                .as_str()
                .map(str::trim)
                .filter(|exe| !exe.is_empty())
                .map(std::path::PathBuf::from);
            if exe.as_ref().is_some_and(|exe| {
                !exe.is_absolute()
                    || !exe
                        .extension()
                        .is_some_and(|e| e.eq_ignore_ascii_case("exe"))
            }) {
                return error(
                    StatusCode::BAD_REQUEST,
                    "exe must be the full path of a .exe",
                );
            }
            tokio::task::spawn_blocking(move || neural::enable(&h, &uuid, exe)).await
        }
        ("POST", "/api/neural/disable") => {
            tokio::task::spawn_blocking(move || neural::disable(&h, &uuid)).await
        }
        _ => return error(StatusCode::NOT_FOUND, "not found"),
    };
    match answer {
        Ok(Ok(())) => (StatusCode::ACCEPTED, Json(json!({"status": true}))).into_response(),
        Ok(Err(Refusal::NotAccepted)) => error(
            StatusCode::CONFLICT,
            "accept the Neural Rendering notice first",
        ),
        Ok(Err(Refusal::Unavailable(reason))) => error(StatusCode::CONFLICT, &reason),
        Ok(Err(Refusal::NotFound)) => error(StatusCode::NOT_FOUND, "application not found"),
        Ok(Err(Refusal::Busy)) => error(
            StatusCode::CONFLICT,
            "Neural Rendering is busy with this game",
        ),
        Ok(Err(Refusal::Streaming)) => error(
            StatusCode::CONFLICT,
            "Neural Rendering downloads and copies about 1 GB: turn it on when no stream is running",
        ),
        Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::super::parity_tests::{request, value};
    use crate::state::test_support::Fixture;
    use axum::{
        Extension,
        body::Body,
        extract::Request,
        http::{StatusCode, header},
    };
    use serde_json::{Value, json};
    use tower::ServiceExt;

    /// An app is kept while its switch is busy (409) and when the mod can't
    /// be taken out of its folder.
    #[tokio::test]
    async fn an_app_whose_mod_cannot_go_now_is_kept() {
        let f = Fixture::new();
        let h = &f.host;
        let folder = h.directory.join("Game");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("Game.exe"), "MZ").unwrap();
        std::fs::write(folder.join(butterpollo_core::neural::MANIFEST), "damaged").unwrap();
        crate::steam::update_library(h, |apps, _| {
            apps.push(json!({"name": "Game", "uuid": "game",
                crate::neural::KEY: {"exe": folder.join("Game.exe")}}));
            apps.push(json!({"name": "Busy", "uuid": "busy"}));
            true
        })
        .unwrap();
        let stored = |uuid: &str| {
            h.apps
                .read()
                .unwrap()
                .iter()
                .any(|app| app.extra.get("uuid") == Some(&json!(uuid)))
        };
        let refused = value(
            request(&f, "POST", "/api/apps/delete", json!({"uuid": "game"})).await,
            StatusCode::BAD_REQUEST,
        )
        .await;
        assert!(
            refused["error"].as_str().unwrap().contains("app was kept"),
            "{refused}"
        );
        assert!(stored("game"));

        crate::neural::start_job(h, "busy", "install", "download").unwrap();
        for (method, path, data) in [
            ("POST", "/api/apps/delete", json!({"uuid": "busy"})),
            ("DELETE", "/api/apps/busy", Value::Null),
        ] {
            let busy = value(request(&f, method, path, data).await, StatusCode::CONFLICT).await;
            assert!(busy["error"].as_str().unwrap().contains("busy"), "{busy}");
        }
        assert!(stored("busy"));
    }

    /// API tokens never reach the switch, and a browser session needs its
    /// CSRF token.
    #[tokio::test]
    async fn the_switch_is_the_consoles_alone() {
        let f = Fixture::new();
        let h = &f.host;
        let scopes = value(
            request(&f, "GET", "/api/token/routes", Value::Null).await,
            StatusCode::OK,
        )
        .await["routes"]
            .clone();
        let issued = value(
            request(&f, "POST", "/api/token", json!({"scopes": scopes})).await,
            StatusCode::OK,
        )
        .await;
        let secret = issued["token"].as_str().unwrap();
        let send = |method: &str, path: &str, headers: Vec<(header::HeaderName, String)>| {
            let mut builder = Request::builder()
                .method(method)
                .uri(path)
                .header(header::CONTENT_TYPE, "application/json");
            for (name, text) in headers {
                builder = builder.header(name, text);
            }
            super::super::router(f.host.clone())
                .layer(Extension(f.connection(true)))
                .oneshot(builder.body(Body::from(r#"{"uuid":"game"}"#)).unwrap())
        };
        for (method, path) in [
            ("GET", "/api/neural?uuid=game"),
            ("POST", "/api/neural/accept"),
            ("POST", "/api/neural/enable"),
            ("POST", "/api/neural/disable"),
        ] {
            let bearer = vec![(header::AUTHORIZATION, format!("Bearer {secret}"))];
            let response = send(method, path, bearer).await.unwrap();
            assert_eq!(
                response.status(),
                StatusCode::UNAUTHORIZED,
                "{method} {path}"
            );
        }
        assert!(!crate::neural::accepted(h));

        let (access, _, csrf, _) = h
            .new_web_session(
                "parity".into(),
                false,
                "test".into(),
                "127.0.0.1".into(),
                None,
            )
            .unwrap();
        let cookie = (header::COOKIE, format!("__Host-apollo_session={access}"));
        let csrf_header = header::HeaderName::from_static("x-csrf-token");
        for headers in [
            vec![cookie.clone()],
            vec![cookie.clone(), (csrf_header.clone(), "wrong".into())],
        ] {
            let refused = value(
                send("POST", "/api/neural/accept", headers).await.unwrap(),
                StatusCode::BAD_REQUEST,
            )
            .await;
            assert!(
                refused["error"].as_str().unwrap().contains("CSRF"),
                "{refused}"
            );
        }
        assert!(!crate::neural::accepted(h));
        value(
            send(
                "POST",
                "/api/neural/accept",
                vec![cookie, (csrf_header, csrf)],
            )
            .await
            .unwrap(),
            StatusCode::OK,
        )
        .await;
        assert!(crate::neural::accepted(h));
    }
}
