//! What is left of the removed background child session in the console.
//!
//! `GET /api/child-session` says whether Windows settings the removed
//! feature's setup changed are still changed (`pending`) and why the last
//! restore failed (`problem`); `POST /api/child-session/restore` restores
//! them again. Console-only: API tokens cannot reach them. Every other path
//! under `/api/child-session/` is unknown.

use super::error;
use crate::state::Shared;
use axum::{
    Json,
    http::{Method, StatusCode},
    response::{IntoResponse, Response},
};
use serde_json::Value;

pub(super) async fn handle(h: &Shared, method: &Method, path: &str) -> Response {
    let h = h.clone();
    match (method.as_str(), path) {
        ("GET", "/api/child-session") => blocking(move || Ok(crate::child_session::status())).await,
        ("POST", "/api/child-session/restore") => {
            blocking(move || {
                crate::child_session::restore_again(&h);
                Ok(crate::child_session::status())
            })
            .await
        }
        _ => error(StatusCode::BAD_REQUEST, "unknown API endpoint"),
    }
}

/// Windows calls (the registry, the firewall) leave the async executor; a
/// failure answers 500 with its reason.
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
    async fn the_status_says_whether_settings_are_still_changed_and_why() {
        let f = Fixture::new();
        let body = value(
            request(&f, "GET", "/api/child-session", json!({})).await,
            StatusCode::OK,
        )
        .await;
        let fields: Vec<_> = body.as_object().unwrap().keys().cloned().collect();
        assert_eq!(fields, ["pending", "problem"], "{body}");
        assert!(body["pending"].is_boolean(), "{body}");
        assert!(
            body["problem"].is_null() || body["problem"].is_string(),
            "{body}"
        );
    }

    #[tokio::test]
    async fn the_removed_actions_are_unknown() {
        let f = Fixture::new();
        for path in [
            "/api/child-session/setup",
            "/api/child-session/start",
            "/api/child-session/stop",
            "/api/child-session/show",
            "/api/child-session/reconnect",
            "/api/child-session/forget-password",
            "/api/child-session/end",
            "/api/child-session/nothing",
        ] {
            let body = value(
                request(&f, "POST", path, json!({"password": "hunter2"})).await,
                StatusCode::BAD_REQUEST,
            )
            .await;
            assert_eq!(body["error"], "unknown API endpoint", "{path}: {body}");
        }
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
