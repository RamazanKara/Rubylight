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
    if plan.is_empty() {
        return Json(json!({"status":true,"changes":[]})).into_response();
    }
    if let Err(refusal) = crate::wake::request_fix(h, &plan) {
        return error(StatusCode::CONFLICT, &refusal.to_string());
    }
    (
        StatusCode::ACCEPTED,
        Json(json!({"status":true,"changes":plan.changes()})),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::super::parity_tests::{request, value};
    use super::*;
    use crate::{state::test_support::Fixture, wake::fake::Fake};
    use butterpollo_core::session::Role;
    use std::time::{Duration, Instant};

    async fn post_fix(f: &Fixture, body: Value) -> Response {
        request(f, "POST", "/api/power/wake/fix", body).await
    }
    /// The fix thread is done: its result is in `GET /api/power`.
    async fn last_fix(f: &Fixture) -> Value {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let status = value(
                request(f, "GET", "/api/power", Value::Null).await,
                StatusCode::OK,
            )
            .await;
            if !status["fix"]["last"].is_null() {
                assert_eq!(status["fix"]["running"], false);
                return status["fix"]["last"].clone();
            }
            assert!(
                Instant::now() < deadline,
                "the fix never finished: {status}"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    #[tokio::test]
    async fn the_fix_is_refused_while_a_stream_is_active_or_starting() {
        let f = Fixture::new();
        let fake = Fake::new().on(&f.host);
        let _session = f.session(f.client(0), Role::Stream);
        let refused = value(post_fix(&f, json!({})).await, StatusCode::CONFLICT).await;
        assert!(
            refused["error"].as_str().unwrap().contains("end it first"),
            "{refused}"
        );
        f.host.sessions.lock().unwrap().active.clear();
        let launch = f.launch(f.client(0), Role::Stream);
        f.host
            .sessions
            .lock()
            .unwrap()
            .pending
            .insert(launch.id.clone(), launch);
        value(post_fix(&f, json!({})).await, StatusCode::CONFLICT).await;
        // Refused before the adapters are read.
        assert_eq!(fake.nic_reads.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert!(fake.fixes.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn the_fix_lists_its_changes_and_applies_them_a_second_later() {
        let f = Fixture::new();
        let fake = Fake::new().on(&f.host);
        let started = Instant::now();
        let accepted = value(
            post_fix(&f, json!({"fast_startup_off": true})).await,
            StatusCode::ACCEPTED,
        )
        .await;
        let changes = accepted["changes"].as_array().unwrap();
        assert!(!changes.is_empty(), "{accepted}");
        // A second request while it waits or runs is refused.
        let running = value(post_fix(&f, json!({})).await, StatusCode::CONFLICT).await;
        assert!(
            running["error"]
                .as_str()
                .unwrap()
                .contains("already running")
        );
        let last = last_fix(&f).await;
        assert!(started.elapsed() >= crate::wake::FIX_DELAY);
        // Fast Startup is inactive on the fake PC (no hibernate), so the plan leaves it.
        assert_eq!(*fake.fixes.lock().unwrap(), [false]);
        assert_eq!(&last["changes"], &accepted["changes"]);
        assert_eq!(last["errors"], json!([]));
    }

    #[tokio::test]
    async fn a_stream_that_starts_before_the_fix_runs_keeps_the_adapters() {
        let f = Fixture::new();
        let fake = Fake::new().on(&f.host);
        value(post_fix(&f, json!({})).await, StatusCode::ACCEPTED).await;
        let _session = f.session(f.client(0), Role::Stream);
        let last = last_fix(&f).await;
        assert!(fake.fixes.lock().unwrap().is_empty());
        assert!(
            last["errors"][0]
                .as_str()
                .unwrap()
                .starts_with("not applied"),
            "{last}"
        );
    }

    #[tokio::test]
    async fn nothing_to_change_is_200_and_runs_nothing() {
        let f = Fixture::new();
        let fake = Fake::new().on(&f.host);
        if let Ok(nics) = fake.nics.lock().unwrap().as_mut() {
            nics.adapters[0].magic_packet_only = Some(true);
        }
        let done = value(post_fix(&f, json!({})).await, StatusCode::OK).await;
        assert_eq!(done["changes"], json!([]));
        assert_eq!(fake.fixes.lock().unwrap().len(), 0);
        value(
            post_fix(&f, json!({"fast_startup_off": "yes"})).await,
            StatusCode::BAD_REQUEST,
        )
        .await;
    }

    #[tokio::test]
    async fn an_unreadable_nic_report_is_an_error_not_an_empty_fix() {
        let f = Fixture::new();
        let fake = Fake::new().on(&f.host);
        *fake.nics.lock().unwrap() = Err("the NIC script failed".into());
        let failed = value(
            post_fix(&f, json!({})).await,
            StatusCode::INTERNAL_SERVER_ERROR,
        )
        .await;
        assert!(
            failed["error"]
                .as_str()
                .unwrap()
                .contains("NIC script failed")
        );
        let status = value(
            request(&f, "GET", "/api/power", Value::Null).await,
            StatusCode::OK,
        )
        .await;
        assert_eq!(status["fix"]["changes"], Value::Null);
        assert!(
            status["fix"]["error"]
                .as_str()
                .unwrap()
                .contains("NIC script failed")
        );
    }

    #[tokio::test]
    async fn power_status_has_readiness_idle_sleep_and_the_offered_fix() {
        let f = Fixture::new();
        Fake::new().on(&f.host);
        let status = value(
            request(&f, "GET", "/api/power?refresh=1", Value::Null).await,
            StatusCode::OK,
        )
        .await;
        assert_eq!(status["status"], true);
        assert_eq!(status["wake"]["targets"].as_array().unwrap().len(), 2);
        assert_eq!(status["idle_sleep"]["state"], "off");
        assert_eq!(status["last_sleep"], Value::Null);
        assert_eq!(status["fix"]["running"], false);
        assert_eq!(status["fix"]["fast_startup_off_offered"], false);
        assert_eq!(status["fix"]["last"], Value::Null);
        assert!(!status["fix"]["changes"].as_array().unwrap().is_empty());
        // Polling it is not someone at the console; other signed-in requests are.
        assert!(f.host.wake.console.lock().unwrap().is_none());
        request(&f, "GET", "/api/config", Value::Null).await;
        assert!(f.host.wake.console.lock().unwrap().is_some());
    }

    #[tokio::test]
    async fn power_routes_need_the_console_sign_in() {
        use axum::{Extension, body::Body, extract::Request};
        use tower::ServiceExt;
        let f = Fixture::new();
        let fake = Fake::new().on(&f.host);
        // A sign-in exists, so anonymous requests are refused.
        request(&f, "GET", "/api/power", Value::Null).await;
        for (method, path) in [("GET", "/api/power"), ("POST", "/api/power/wake/fix")] {
            let anonymous = super::super::router(f.host.clone())
                .layer(Extension(f.connection(true)))
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri(path)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            value(anonymous, StatusCode::UNAUTHORIZED).await;
        }
        assert!(fake.fixes.lock().unwrap().is_empty());
    }
}
