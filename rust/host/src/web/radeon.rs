//! `GET /api/radeon`: what the game page's Radeon controls may offer. It is the native API's
//! panel (`native_api/radeon.rs`) read for the console: per setting its id, state (supported and
//! writable is `configurable`), range and options. It only reads; a game's Radeon profile is
//! written when the game starts, never from here.
use super::{error, response};
use crate::state::Shared;
use axum::{http::StatusCode, response::Response};
use serde_json::json;

pub(super) async fn read(h: &Shared) -> Response {
    let h = h.clone();
    // The read waits for the Radeon worker, so it stays off the async executor.
    let view =
        tokio::task::spawn_blocking(move || crate::native_api::radeon::current_view(&h).0).await;
    match view {
        Ok(view) => response(
            StatusCode::OK,
            json!({
                "status": true,
                "state": view.state,
                "reason": view.reason,
                "message": view.message,
                "gpus": view.gpus,
            }),
        ),
        Err(failure) => {
            tracing::error!(error = %failure, "console: the Radeon settings could not be read");
            error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "the Radeon settings could not be read",
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::parity_tests::{request, value};
    use crate::{radeon_session::WorkerRef, state::test_support::Fixture};
    use axum::{
        Extension,
        body::Body,
        extract::Request,
        http::{Method, StatusCode},
    };
    use butterpollo_windows::radeon::{
        BackendError, GpuInfo, RadeonBackend,
        control::JOURNAL_FILE,
        model::{Kind, Range, Setting, State, Value as Raw, Writable},
        worker::{Timing, Worker},
    };
    use serde_json::Value;
    use std::{
        collections::BTreeMap,
        sync::{Arc, Mutex},
        time::{Duration, Instant},
    };
    use tower::ServiceExt;

    /// A driver that records every write. No test here touches the real GPU.
    #[derive(Clone, Default)]
    struct Fake(Arc<Mutex<Vec<String>>>);
    impl RadeonBackend for Fake {
        fn read(&mut self, _gpu: i32) -> Result<Vec<Setting>, BackendError> {
            Ok(vec![
                setting("anti_lag.enabled", Kind::Toggle, State::Configurable, None),
                setting(
                    "frame_rate_target.fps",
                    Kind::Integer,
                    State::Configurable,
                    Some(Range {
                        min: 30,
                        max: 300,
                        step: 1,
                    }),
                ),
                setting("boost.enabled", Kind::Toggle, State::Unsupported, None),
            ])
        }
        fn write(&mut self, _gpu: i32, id: &str, value: &Raw) -> Result<(), BackendError> {
            self.0
                .lock()
                .unwrap()
                .push(format!("{id}={}", value.display()));
            Ok(())
        }
        fn gpus(&mut self) -> Result<Vec<GpuInfo>, BackendError> {
            Ok(vec![GpuInfo {
                id: 7,
                name: "AMD Radeon RX 7900 XT".into(),
                driver: "Adrenalin 26.2.1".into(),
            }])
        }
    }
    fn setting(id: &'static str, kind: Kind, state: State, range: Option<Range>) -> Setting {
        Setting {
            id,
            group: id.split('.').next().unwrap(),
            label: id,
            interface: "fake",
            kind,
            state,
            supported: Some(state != State::Unsupported),
            readable: Some(true),
            writable: Writable::Yes,
            value: Some(match kind {
                Kind::Integer => Raw::Int(60),
                _ => Raw::Bool(false),
            }),
            unit: range.map(|_| "fps"),
            range,
            options: vec![],
            depends: vec![],
            notes: vec![],
            adlx: BTreeMap::new(),
        }
    }
    fn attach(f: &Fixture) -> Fake {
        let fake = Fake::default();
        let worker = Worker::start(
            fake.clone(),
            f.host.directory.join(JOURNAL_FILE),
            Timing::default(),
        );
        assert!(f.host.radeon.attach(WorkerRef::Owned(Arc::new(worker))));
        let until = Instant::now() + Duration::from_secs(15);
        while f.host.radeon.recovering() {
            assert!(
                Instant::now() < until,
                "the worker never finished recovering"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        fake
    }

    #[tokio::test]
    async fn the_console_reads_the_panel_and_writes_nothing() {
        let f = Fixture::new();
        let fake = attach(&f);
        let panel = value(
            request(&f, "GET", "/api/radeon", Value::Null).await,
            StatusCode::OK,
        )
        .await;
        assert_eq!(panel["state"], "configurable");
        let settings = panel["gpus"][0]["settings"].as_array().unwrap();
        let state = |id: &str| {
            settings
                .iter()
                .find(|s| s["id"] == id)
                .map(|s| s["state"].clone())
        };
        assert_eq!(state("anti_lag.enabled").unwrap(), "configurable");
        assert_eq!(state("boost.enabled").unwrap(), "unsupported");
        let fps = settings
            .iter()
            .find(|s| s["id"] == "frame_rate_target.fps")
            .unwrap();
        assert_eq!(fps["range"]["min"], 30);
        assert_eq!(fps["range"]["max"], 300);
        // Reading applies nothing, and a write method is not a route.
        let refused = request(&f, "POST", "/api/radeon", serde_json::json!({})).await;
        assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
        assert_eq!(fake.0.lock().unwrap().clone(), Vec::<String>::new());
    }

    #[tokio::test]
    async fn without_a_radeon_worker_the_console_hears_why() {
        let f = Fixture::new();
        let panel = value(
            request(&f, "GET", "/api/radeon", Value::Null).await,
            StatusCode::OK,
        )
        .await;
        assert_eq!(panel["state"], "unsupported");
        assert_eq!(panel["reason"], "no_radeon");
        assert!(panel["message"].as_str().is_some_and(|m| !m.is_empty()));
        assert_eq!(panel["gpus"], serde_json::json!([]));
    }

    #[tokio::test]
    async fn the_radeon_panel_needs_the_console_sign_in() {
        let f = Fixture::new();
        let fake = attach(&f);
        // A sign-in exists, so an anonymous request is refused.
        request(&f, "GET", "/api/radeon", Value::Null).await;
        let anonymous = super::super::router(f.host.clone())
            .layer(Extension(f.connection(true)))
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/radeon")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let refused = value(anonymous, StatusCode::UNAUTHORIZED).await;
        assert!(refused.get("gpus").is_none());
        assert_eq!(fake.0.lock().unwrap().clone(), Vec::<String>::new());
    }
}
