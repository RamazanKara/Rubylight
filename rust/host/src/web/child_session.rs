//! Background streaming in a Windows child session (Experimental).
//!
//! `GET /api/child-session` reads; the `POST /api/child-session/...` actions
//! set the PC up, restore it, and start, stop or end the child session. They
//! are console-only: API tokens cannot reach them. A password sent to `start`
//! goes straight to the viewer's pipe and is never logged or stored here:
//! it is read from the raw body into a buffer wiped on drop, never through a
//! `serde_json::Value`. The body itself is not wiped: it is hyper's buffer,
//! usually shared with the connection's, so it is only dropped as soon as the
//! password is out of it.

use super::error;
use crate::state::Shared;
use axum::{
    Json,
    body::Bytes,
    http::{Method, StatusCode},
    response::{IntoResponse, Response},
};
use butterpollo_core::child_session::{Secret, ViewerCommand};
use serde::Deserialize;
use serde_json::{Value, json, value::RawValue};

pub(super) async fn handle(h: &Shared, method: &Method, path: &str, body: Bytes) -> Response {
    let h = h.clone();
    if (method.as_str(), path) == ("POST", "/api/child-session/start") {
        let password = start_password(&body);
        drop(body);
        let password = match password {
            Ok(password) => password,
            Err(problem) => return error(StatusCode::BAD_REQUEST, problem),
        };
        return blocking(move || {
            crate::child_session::start(&h, password)?;
            Ok(json!({"status": true}))
        })
        .await;
    }
    let data: Value = if body.is_empty() {
        json!({})
    } else {
        match serde_json::from_slice(&body) {
            Ok(v) => v,
            Err(_) => return error(StatusCode::BAD_REQUEST, "invalid JSON"),
        }
    };
    let command = match (method.as_str(), path) {
        ("GET", "/api/child-session") => {
            return blocking(move || Ok(crate::child_session::status(&h))).await;
        }
        ("POST", "/api/child-session/setup") => {
            return blocking(move || {
                crate::child_session::setup(&h)?;
                Ok(json!({"status": true}))
            })
            .await;
        }
        ("POST", "/api/child-session/restore") => {
            return blocking(move || {
                let restored = crate::child_session::restore(&h)?;
                Ok(json!({"status": true, "restored": restored}))
            })
            .await;
        }
        ("POST", "/api/child-session/stop") => ViewerCommand::Stop,
        ("POST", "/api/child-session/show") => ViewerCommand::Show,
        ("POST", "/api/child-session/reconnect") => ViewerCommand::Reconnect,
        ("POST", "/api/child-session/forget-password") => ViewerCommand::ForgetPassword,
        ("POST", "/api/child-session/end") => {
            let number = |key: &str| data.get(key).and_then(Value::as_u64);
            match (number("child"), number("generation")) {
                (Some(child), Some(generation)) if child <= u64::from(u32::MAX) => {
                    ViewerCommand::EndSession {
                        child: child as u32,
                        generation,
                    }
                }
                _ => {
                    return error(
                        StatusCode::BAD_REQUEST,
                        "child and generation must be the numbers the status showed",
                    );
                }
            }
        }
        _ => return error(StatusCode::BAD_REQUEST, "unknown API endpoint"),
    };
    blocking(move || {
        crate::child_session::send(command)?;
        Ok(json!({"status": true}))
    })
    .await
}

/// The password a `start` request carries: None to use the saved one.
fn start_password(body: &[u8]) -> Result<Option<Secret>, &'static str> {
    #[derive(Deserialize)]
    struct Start<'a> {
        #[serde(borrow, default)]
        password: Option<&'a RawValue>,
    }
    if body.is_empty() {
        return Ok(None);
    }
    // The raw value borrows the body, so only Secret::from_json_string copies
    // the password. No serde message: it can quote the body.
    let start: Start = serde_json::from_slice(body).map_err(|_| "invalid JSON")?;
    match start.password.map(RawValue::get) {
        None | Some("null") => Ok(None),
        Some(literal) => match Secret::from_json_string(literal) {
            Ok(password) if !password.is_empty() => Ok(Some(password)),
            _ => Err("password must be the Windows password, or left out to use the saved one"),
        },
    }
}
/// Windows calls (the registry, the firewall's PowerShell, process starts)
/// leave the async executor; a failure answers 500 with its reason.
async fn blocking(work: impl FnOnce() -> anyhow::Result<Value> + Send + 'static) -> Response {
    match tokio::task::spawn_blocking(work).await {
        Ok(Ok(value)) => Json(value).into_response(),
        Ok(Err(e)) => error(StatusCode::INTERNAL_SERVER_ERROR, &format!("{e:#}")),
        Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::super::parity_tests::{request, value};
    use crate::state::test_support::Fixture;
    use axum::http::StatusCode;
    use serde_json::json;

    #[tokio::test]
    async fn child_session_actions_refuse_bad_input_before_touching_windows() {
        let f = Fixture::new();
        for (path, body, expected) in [
            (
                "/api/child-session/start",
                json!({"password": 5}),
                "password must be",
            ),
            (
                "/api/child-session/start",
                json!({"password": ""}),
                "password must be",
            ),
            (
                "/api/child-session/end",
                json!({"child": 3}),
                "child and generation",
            ),
            (
                "/api/child-session/end",
                json!({"child": -1, "generation": 1}),
                "child and generation",
            ),
            (
                "/api/child-session/nothing",
                json!({}),
                "unknown API endpoint",
            ),
        ] {
            let body = value(
                request(&f, "POST", path, body).await,
                StatusCode::BAD_REQUEST,
            )
            .await;
            assert!(
                body["error"].as_str().unwrap().contains(expected),
                "{path}: {body}"
            );
        }
        // Nothing runs: the commands that need the viewer say so.
        let body = value(
            request(&f, "POST", "/api/child-session/stop", json!({})).await,
            StatusCode::INTERNAL_SERVER_ERROR,
        )
        .await;
        assert!(
            body["error"]
                .as_str()
                .unwrap()
                .contains("viewer is not running"),
            "{body}"
        );
    }

    #[test]
    fn the_start_password_comes_from_the_raw_body() {
        let password = |body: &str| super::start_password(body.as_bytes());
        assert!(password("").unwrap().is_none());
        assert!(password("{}").unwrap().is_none());
        assert!(password(r#"{"password":null}"#).unwrap().is_none());
        assert_eq!(
            password(r#"{"password":"a\"b\\cé"}"#)
                .unwrap()
                .unwrap()
                .expose(),
            "a\"b\\c\u{e9}"
        );
        for bad in [r#"{"password":5}"#, r#"{"password":""}"#] {
            assert!(password(bad).unwrap_err().contains("password must be"));
        }
        // A parse error never quotes the body, which may hold the password.
        assert_eq!(
            password(r#"{"password":"hunter2"#).unwrap_err(),
            "invalid JSON"
        );
    }

    #[test]
    fn api_tokens_cannot_reach_the_child_session() {
        assert!(
            !super::super::token_catalog()
                .iter()
                .any(|scope| scope.path.contains("child-session"))
        );
    }

    #[tokio::test]
    async fn an_unclaimed_child_host_cannot_be_claimed_even_from_this_pc() {
        use axum::{Extension, body::Body, http::Request};
        use tower::ServiceExt;
        let first = |f: &Fixture| {
            let request = Request::builder()
                .method("POST")
                .uri("/api/password")
                .header("Content-Type", "application/json")
                .body(Body::from(
                    json!({"newUsername": "intruder", "newPassword": "intruder-pass", "confirmNewPassword": "intruder-pass"})
                        .to_string(),
                ))
                .unwrap();
            super::super::router(f.host.clone())
                .layer(Extension(f.connection(true)))
                .oneshot(request)
        };
        let child = Fixture::new();
        child.host.config.write().unwrap().values.insert(
            butterpollo_core::child_session::CHILD_HOST_KEY.into(),
            "true".into(),
        );
        let body = value(first(&child).await.unwrap(), StatusCode::FORBIDDEN).await;
        assert!(
            body["error"]
                .as_str()
                .unwrap()
                .contains("main console's sign-in"),
            "{body}"
        );
        assert!(child.host.credentials.read().unwrap().is_none());
        // The main host still takes its first sign-in from this PC.
        let main = Fixture::new();
        value(first(&main).await.unwrap(), StatusCode::OK).await;
        assert!(main.host.credentials.read().unwrap().is_some());
    }

    #[tokio::test]
    async fn only_the_main_host_in_child_session_mode_sets_up_or_restores() {
        // The child session's host: the PC's record is the main host's.
        let child = Fixture::new();
        child.host.config.write().unwrap().values.insert(
            butterpollo_core::child_session::CHILD_HOST_KEY.into(),
            "true".into(),
        );
        for path in ["/api/child-session/setup", "/api/child-session/restore"] {
            let body = value(
                request(&child, "POST", path, json!({})).await,
                StatusCode::INTERNAL_SERVER_ERROR,
            )
            .await;
            assert!(
                body["error"]
                    .as_str()
                    .unwrap()
                    .contains("this is the child session's host"),
                "{path}: {body}"
            );
        }
        // Nucleus Co-op, the only background mode, sets nothing up.
        let main = Fixture::new();
        let body = value(
            request(&main, "POST", "/api/child-session/setup", json!({})).await,
            StatusCode::INTERNAL_SERVER_ERROR,
        )
        .await;
        assert!(
            body["error"]
                .as_str()
                .unwrap()
                .contains("Child sessions no longer run"),
            "{body}"
        );
    }
}
