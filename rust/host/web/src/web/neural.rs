//! The game page's Neural Rendering panel. `GET /api/neural?uuid=` shows
//! whether this PC can have frame generation and why not, whether the user
//! accepted the notice, the app's install with its progress and Rubylight
//! 3.0's upscaling still in its folder. `POST /api/neural/accept` records the
//! acceptance, `POST /api/neural/enable` (`uuid`, optional `exe`) downloads
//! and installs frame generation (taking the earlier upscaling out first),
//! `POST /api/neural/disable` removes both, or with `earlier` only the
//! earlier upscaling. Console-only: API tokens cannot
//! reach them.

use super::error;
use axum::{
    Json,
    http::{Method, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use butterpollo_stream::{
    neural::{self, Refusal},
    state::Shared,
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
            return match tokio::task::spawn_blocking(move || neural::accept(&h)).await {
                Ok(Ok(())) => Json(json!({"status": true})).into_response(),
                Ok(Err(e)) => error(StatusCode::INTERNAL_SERVER_ERROR, &format!("{e:#}")),
                Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
            };
        }
        ("POST", "/api/neural/enable") => {
            let exe = data["exe"]
                .as_str()
                .map(str::trim)
                .filter(|exe| !exe.is_empty())
                .map(std::path::PathBuf::from);
            if exe
                .as_ref()
                .is_some_and(|exe| !neural::folder::is_valid_exe_path(exe))
            {
                return error(
                    StatusCode::BAD_REQUEST,
                    "exe must be the full path of a .exe",
                );
            }
            tokio::task::spawn_blocking(move || neural::enable(&h, &uuid, exe)).await
        }
        ("POST", "/api/neural/disable") => {
            let earlier = data["earlier"].as_bool() == Some(true);
            tokio::task::spawn_blocking(move || match earlier {
                true => neural::remove_earlier(&h, &uuid),
                false => neural::disable(&h, &uuid),
            })
            .await
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
        Ok(Err(Refusal::Streaming)) if path == "/api/neural/disable" => error(
            StatusCode::CONFLICT,
            "the game may be running with Neural Rendering loaded: remove it when no stream is running",
        ),
        Ok(Err(Refusal::Streaming)) => error(
            StatusCode::CONFLICT,
            "Neural Rendering downloads about 14 MB and copies it into the game's folder: turn it on when no stream is running",
        ),
        Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use crate::test_support::{request, value};
    use axum::{
        Extension,
        body::Body,
        extract::Request,
        http::{StatusCode, header},
    };
    use butterpollo_stream::state::test_support::Fixture;
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
        std::fs::write(
            folder.join(butterpollo_session::neural::MANIFEST),
            "damaged",
        )
        .unwrap();
        butterpollo_stream::steam::update_library(h, |apps, _| {
            apps.push(json!({"name": "Game", "uuid": "game",
                butterpollo_stream::neural::KEY: {"exe": folder.join("Game.exe")}}));
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

        butterpollo_stream::neural::start_job(h, "busy", "install", "download").unwrap();
        for (method, path, data) in [
            ("POST", "/api/apps/delete", json!({"uuid": "busy"})),
            ("DELETE", "/api/apps/busy", Value::Null),
        ] {
            let busy = value(request(&f, method, path, data).await, StatusCode::CONFLICT).await;
            assert!(busy["error"].as_str().unwrap().contains("busy"), "{busy}");
        }
        assert!(stored("busy"));
    }

    /// While a stream runs the switch is refused both ways: the streamed game
    /// may hold the provider loaded, so a removal would be left half done.
    #[tokio::test]
    async fn the_switch_is_refused_both_ways_while_streaming() {
        let f = Fixture::new();
        let h = &f.host;
        butterpollo_stream::steam::update_library(h, |apps, _| {
            apps.push(json!({"name": "Game", "uuid": "game"}));
            true
        })
        .unwrap();
        f.session(
            f.client(u32::MAX),
            butterpollo_session::session::Role::Stream,
        );
        let refused = value(
            request(&f, "POST", "/api/neural/disable", json!({"uuid": "game"})).await,
            StatusCode::CONFLICT,
        )
        .await;
        assert!(
            refused["error"]
                .as_str()
                .unwrap()
                .contains("no stream is running"),
            "{refused}"
        );
        assert!(butterpollo_stream::neural::status(h, "game")["job"].is_null());
        assert!(matches!(
            butterpollo_stream::neural::disable(h, "game"),
            Err(butterpollo_stream::neural::Refusal::Streaming)
        ));
        h.sessions.lock().unwrap().active.clear();
        butterpollo_stream::neural::disable(h, "game").unwrap();
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
        assert!(!butterpollo_stream::neural::accepted(h));

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
        assert!(!butterpollo_stream::neural::accepted(h));
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
        assert!(butterpollo_stream::neural::accepted(h));
    }

    /// `POST /api/neural/enable` rejects path traversal (`..`), `.`, NT
    /// namespace, and verbatim/device prefixes (`\\?\`, `\\.\`) with 400
    /// before touching the game folder.
    #[tokio::test]
    async fn enable_rejects_traversal_and_verbatim_or_device_exe_paths() {
        let f = Fixture::new();
        for bad in [
            r"C:\Program Files\DLSSGame\..\OtherApp\target.exe",
            r"\\?\C:\Windows\System32\foo.exe",
            r"\\.\C:\Windows\System32\foo.exe",
            r"C:\Games\Game\..\..\Windows\System32\foo.exe",
            r"C:\Games\EACGame\Binaries\Win64\..\Win64\Game.exe",
            r"C:\Steam\steamapps\common\CSGO\game\bin\win64\..\win64\cs2.exe",
            r"\\?\C:\Steam\steamapps\common\CSGO\game\bin\win64\cs2.exe",
            r"\??\C:\Windows\System32\foo.exe",
            r"C:\Games\Game\.\Game.exe",
        ] {
            let refused = value(
                request(
                    &f,
                    "POST",
                    "/api/neural/enable",
                    json!({"uuid": "game", "exe": bad}),
                )
                .await,
                StatusCode::BAD_REQUEST,
            )
            .await;
            assert!(
                refused["error"]
                    .as_str()
                    .unwrap()
                    .contains("full path of a .exe"),
                "{bad}: {refused}"
            );
        }
    }
}
