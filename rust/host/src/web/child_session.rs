//! Background streaming in a Windows child session (Experimental).
//!
//! `GET /api/child-session` reads; the `POST /api/child-session/...` actions
//! set the PC up, restore it, and start, stop or end the child session. They
//! are console-only: API tokens cannot reach them. A password sent to `start`
//! goes straight to the viewer's pipe and is never logged or stored here.

use super::error;
use crate::state::Shared;
use axum::{
    Json,
    http::{Method, StatusCode},
    response::{IntoResponse, Response},
};
use butterpollo_core::child_session::{Secret, ViewerCommand};
use serde_json::{Value, json};

pub(super) async fn handle(h: &Shared, method: &Method, path: &str, data: &Value) -> Response {
    let h = h.clone();
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
        ("POST", "/api/child-session/start") => {
            let password = match data.get("password") {
                None | Some(Value::Null) => None,
                Some(Value::String(password)) if !password.is_empty() => {
                    Some(Secret::new(password.clone()))
                }
                Some(_) => {
                    return error(
                        StatusCode::BAD_REQUEST,
                        "password must be the Windows password, or left out to use the saved one",
                    );
                }
            };
            return blocking(move || {
                crate::child_session::start(&h, password)?;
                Ok(json!({"status": true}))
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
    fn api_tokens_cannot_reach_the_child_session() {
        assert!(
            !super::super::token_catalog()
                .iter()
                .any(|scope| scope.path.contains("child-session"))
        );
    }
}
