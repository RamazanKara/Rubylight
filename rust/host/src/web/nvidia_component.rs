//! The console's NVIDIA components card. `GET /api/nvidia-component` shows
//! whether they are installed, a download or removal in progress and the last
//! failure; `POST /api/nvidia-component/install` downloads and installs them
//! from this version's release, `POST /api/nvidia-component/remove` deletes
//! them. Console-only: API tokens cannot reach them.

use super::error;
use crate::{nvidia_component, state::Shared};
use axum::{
    Json,
    http::{Method, StatusCode},
    response::{IntoResponse, Response},
};
use serde_json::json;

pub(super) async fn handle(h: &Shared, method: &Method, path: &str) -> Response {
    let h = h.clone();
    let answer = match (method.as_str(), path) {
        ("GET", "/api/nvidia-component") => {
            return match tokio::task::spawn_blocking(move || nvidia_component::summary(&h)).await {
                Ok(summary) => Json(summary).into_response(),
                Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
            };
        }
        ("POST", "/api/nvidia-component/install") => nvidia_component::install(&h),
        ("POST", "/api/nvidia-component/remove") => {
            match tokio::task::spawn_blocking(move || nvidia_component::remove(&h)).await {
                Ok(answer) => answer,
                Err(e) => return error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
            }
        }
        _ => return error(StatusCode::NOT_FOUND, "not found"),
    };
    match answer {
        Ok(()) => (StatusCode::ACCEPTED, Json(json!({"status": true}))).into_response(),
        Err(reason) => error(StatusCode::CONFLICT, &reason),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn api_tokens_cannot_reach_the_nvidia_components() {
        assert!(
            !super::super::token_catalog()
                .iter()
                .any(|scope| scope.path.contains("nvidia"))
        );
    }
}
