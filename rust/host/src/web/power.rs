//! Wake-on-LAN readiness, sleep after idle, and the consented NIC fix.
//!
//! `GET /api/power` reads; `POST /api/power/wake/fix` changes adapter and power
//! settings only when the administrator asks, never while a stream is active or
//! starting (the network drops while adapters restart).

use super::error;
use crate::state::Shared;
use axum::{
    Json,
    http::{Method, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};
use std::net::IpAddr;

pub(super) async fn handle(
    h: &Shared,
    method: &Method,
    uri: &Uri,
    local: IpAddr,
    data: &Value,
) -> Response {
    match (method.as_str(), uri.path()) {
        ("GET", "/api/power") => {
            let refresh = uri
                .query()
                .is_some_and(|q| q.split('&').any(|pair| pair == "refresh=1"));
            let h = h.clone();
            match tokio::task::spawn_blocking(move || {
                crate::wake::console_status(&h, refresh, Some(local))
            })
            .await
            {
                Ok(Ok(status)) => Json(status).into_response(),
                Ok(Err(e)) => error(StatusCode::INTERNAL_SERVER_ERROR, &format!("{e:#}")),
                Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
            }
        }
        ("POST", "/api/power/wake/fix") => fix(h, data).await,
        _ => error(StatusCode::BAD_REQUEST, "unknown API endpoint"),
    }
}

/// 202 with every change it will make, applied about a second later; 200 when
/// there is nothing to change; 409 while a stream holds the PC or a fix runs.
async fn fix(h: &Shared, data: &Value) -> Response {
    let fast_startup_off = match data.get("fast_startup_off") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(off)) => *off,
        Some(_) => {
            return error(
                StatusCode::BAD_REQUEST,
                "fast_startup_off must be a boolean",
            );
        }
    };
    // The changes the administrator saw in `GET /api/power`; when given, only
    // exactly those are applied.
    let approved: Option<Vec<String>> = match data.get("changes") {
        None | Some(Value::Null) => None,
        Some(changes) => match serde_json::from_value(changes.clone()) {
            Ok(changes) => Some(changes),
            Err(_) => {
                return error(
                    StatusCode::BAD_REQUEST,
                    "changes must be a list of the offered changes",
                );
            }
        },
    };
    if let Some(refusal) = crate::wake::fix_refusal(h) {
        return error(StatusCode::CONFLICT, &refusal.to_string());
    }
    let reading = h.clone();
    let plan = match tokio::task::spawn_blocking(move || {
        crate::wake::fix_plan(&reading, fast_startup_off)
    })
    .await
    {
        Ok(Ok(plan)) => plan,
        Ok(Err(e)) => return error(StatusCode::INTERNAL_SERVER_ERROR, &format!("{e:#}")),
        Err(e) => return error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };
    // `GET /api/power` lists the adapters' changes; Fast Startup is its own
    // choice, `fast_startup_off`.
    let adapters = butterpollo_core::wake::FixPlan {
        fast_startup_off: false,
        ..plan.clone()
    }
    .changes();
    if approved.is_some_and(|approved| approved != adapters) {
        return (
            StatusCode::CONFLICT,
            Json(json!({
                "status": false,
                "error": "not applied: the adapters' settings changed since you checked; check again",
                "changes": adapters,
                "skipped": plan.skipped,
            })),
        )
            .into_response();
    }
    if plan.is_empty() {
        return Json(json!({"status":true,"changes":[],"skipped":plan.skipped})).into_response();
    }
    if let Err(refusal) = crate::wake::request_fix(h, &plan) {
        return error(StatusCode::CONFLICT, &refusal.to_string());
    }
    (
        StatusCode::ACCEPTED,
        Json(json!({"status":true,"changes":plan.changes(),"skipped":plan.skipped})),
    )
        .into_response()
}

#[cfg(test)]
mod tests;
