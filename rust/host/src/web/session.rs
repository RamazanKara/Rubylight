//! Streaming session and frame-limiter status API handlers.

use crate::state::Shared;
use serde_json::{Value, json};

pub(super) fn console(h: &Shared, method: &str, data: &Value) -> anyhow::Result<Value> {
    if method == "GET" {
        let mut recovery = butterpollo_windows::process::console_recovery()?;
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
    butterpollo_windows::process::reattach_console(session, console)?;
    tracing::info!(
        session,
        console,
        "administrator requested return of the host's RDP session to the console"
    );
    Ok(json!({"status":true,"requested":true}))
}

pub(super) fn handle(h: &Shared, method: &str, path: &str) -> anyhow::Result<Value> {
    Ok(match (method, path) {
        ("GET", "/api/rtss/status" | "/api/frame-limiter/status") => {
            crate::maintenance::integration_status(h)
        }
        ("GET", "/api/rtsp/sessions") => {
            json!({"status":true,"sessions":h.sessions.lock().unwrap().active.values().map(|s|s.info()).collect::<Vec<_>>()})
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
    use crate::state::test_support::Fixture;

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
        let _stream = f.session(client, butterpollo_core::session::Role::Stream);
        let error = console(
            &f.host,
            "POST",
            &json!({"session_id":4,"console_session_id":1}),
        )
        .unwrap_err();
        assert!(error.to_string().contains("disconnect streams"));
    }
}
