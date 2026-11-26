//! Streaming session and frame-limiter status API handlers.

use butterpollo_stream::state::Shared;
use serde_json::{Value, json};

pub(super) fn console(h: &Shared, method: &str, data: &Value) -> anyhow::Result<Value> {
    if method == "GET" {
        let mut recovery = butterpollo_win_io::process::console_recovery()?;
        if !h.sessions.lock().unwrap().idle() {
            recovery.can_reattach = false;
            recovery.reason = "Disconnect streams and wait for pending connections to finish before returning to the console.".into();
        }
        return Ok(json!({"status":true,"recovery":recovery}));
    }
    let expected = |key: &str| -> anyhow::Result<u32> {
        data.get(key)
            .and_then(Value::as_u64)
            .and_then(|value| u32::try_from(value).ok())
            .ok_or_else(|| {
                anyhow::anyhow!("missing or invalid {key}; refresh session status first")
            })
    };
    let session = expected("session_id")?;
    let console = expected("console_session_id")?;
    let _transition = h.launch_transition.lock().unwrap();
    let sessions = h.sessions.lock().unwrap();
    if !sessions.idle() {
        anyhow::bail!("disconnect streams and wait for pending connections to finish first");
    }
    butterpollo_win_io::process::reattach_console(session, console)?;
    tracing::info!(
        session,
        console,
        "administrator requested return of the host's RDP session to the console"
    );
    Ok(json!({"status":true,"requested":true}))
}

/// Quick Suspend for the console: `GET /api/session/suspend` reads the state,
/// `POST /api/session/suspend` and `/api/session/resume` act as the
/// administrator. The answer is the native API's `QuickSuspendStatus`; a
/// refusal is its `refused` state with the reason, never an error.
pub(super) fn quick_suspend(h: &Shared, method: &str, path: &str) -> Value {
    use butterpollo_stream::quick_suspend::{Who, resume, status, suspend};
    let status = match (method, path) {
        ("POST", "/api/session/suspend") => suspend(h, Who::Console),
        ("POST", "/api/session/resume") => resume(h, Who::Console),
        _ => status(h),
    };
    let mut answer = serde_json::to_value(status.native()).unwrap_or_default();
    answer["status"] = Value::Bool(true);
    answer
}

pub(super) fn handle(h: &Shared, method: &str, path: &str) -> anyhow::Result<Value> {
    Ok(match (method, path) {
        ("GET", "/api/rtss/status" | "/api/frame-limiter/status") => {
            butterpollo_stream::maintenance::integration_status(h)
        }
        ("GET", "/api/rtsp/sessions") => {
            json!({"status":true,"sessions":h.sessions.lock().unwrap().active.values().map(|s|s.info()).collect::<Vec<_>>()})
        }
        ("GET", "/api/sessions/history") => {
            json!({
                "status": true,
                "sessions": h.session_history.entries(),
                "limit": butterpollo_session::session_history::MAX_ENTRIES,
                "days": butterpollo_session::session_history::MAX_AGE_MS / 86_400_000,
                "bytes": butterpollo_session::session_history::MAX_BYTES,
            })
        }
        ("GET", "/api/session/status") => {
            let current = h.current_app.lock().unwrap();
            let sessions = h.sessions.lock().unwrap();
            let uuid = current.as_ref().and_then(|running| {
                h.apps
                    .read()
                    .unwrap()
                    .iter()
                    .find(|app| app.id() == running.id)
                    .and_then(|app| app.extra.get("uuid"))
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            });
            json!({"status":true,"activeSessions":sessions.active.len(),"appRunning":current.is_some(),"appName":current.as_ref().map(|a|a.name.as_str()).unwrap_or(""),"paused":current.is_some()&&sessions.active.is_empty(),"lastEncoderProbeFailed":h.warnings.snapshot().iter().any(|warning| warning.code == "video_encoder"),"running":!sessions.active.is_empty(),"app":current.as_ref().map(|a|json!({"name":a.name,"id":a.id,"uuid":uuid}))})
        }
        _ => return Err(anyhow::anyhow!("unknown API endpoint")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use butterpollo_stream::state::test_support::Fixture;

    #[test]
    fn console_request_requires_confirmed_ids_and_idle_transports() {
        let f = Fixture::new();
        for data in [json!({}), json!({"session_id":-1,"console_session_id":1})] {
            assert!(
                console(&f.host, "POST", &data)
                    .unwrap_err()
                    .to_string()
                    .contains("invalid session_id")
            );
        }
        let client = f.client(0x071f1f00);
        let _stream = f.session(client, butterpollo_session::session::Role::Stream);
        let error = console(
            &f.host,
            "POST",
            &json!({"session_id":4,"console_session_id":1}),
        )
        .unwrap_err();
        assert!(error.to_string().contains("disconnect streams"));
    }

    #[test]
    fn the_console_reads_quick_suspend_and_a_refusal_is_an_answer() {
        let f = Fixture::new();
        let read = quick_suspend(&f.host, "GET", "/api/session/suspend");
        assert_eq!(read["status"], true);
        assert_eq!(read["state"], "unavailable");
        assert_eq!(read["reason"], "disabled_for_app");
        let refused = quick_suspend(&f.host, "POST", "/api/session/suspend");
        assert_eq!(refused["state"], "refused");
        assert!(refused["message"].as_str().unwrap().contains("off"));
        let resumed = quick_suspend(&f.host, "POST", "/api/session/resume");
        assert_eq!(resumed["state"], "unavailable");
        let routes = super::super::tokens::catalog();
        for path in ["/api/session/suspend", "/api/session/resume"] {
            assert!(routes.iter().any(|scope| scope.path == path), "{path}");
        }
    }
}
