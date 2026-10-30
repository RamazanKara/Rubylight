//! `GET /audio` and `POST /audio/settings`: night mode and headphone surround of the caller's
//! stream. The contract is `docs/native-api.md`, "Audio processing"; the settings live on the
//! session (`audio_dsp::Controls`) and a write applies from the next audio packet. Each device
//! reads and changes only its own Stream session's.
use super::{Failure, Reply, caller, json, read_body, require_view_or_launch};
use crate::{
    state::{Session, Shared},
    tls::Connection,
};
use axum::{
    Extension,
    extract::{Request, State},
    http::StatusCode,
};
use butterpollo_core::{
    audio_dsp::{self, Settings},
    native_api::{
        audio::AudioWrite,
        error::ErrorCode,
        radeon::{Change, Value},
    },
    session::Role,
    session_state::Refusal,
    state::Client,
};
use serde::Deserialize;
use std::sync::Arc;

pub async fn read(State(h): State<Shared>, Extension(c): Extension<Connection>) -> Reply {
    let client = caller(&h, &c)?;
    require_view_or_launch(&client)?;
    tokio::task::spawn_blocking(move || {
        let audio = match own_stream(&h, &client) {
            Some(s) => audio_dsp::state(Some(&s.audio), base(&h, &s)?),
            // Another device's stream is no more writable for this one than none at all.
            None => audio_dsp::state(None, Settings::default()),
        };
        Ok(json(StatusCode::OK, &audio))
    })
    .await
    .map_err(|error| Failure::internal("read the audio settings", error))?
}

pub async fn write(
    State(h): State<Shared>,
    Extension(c): Extension<Connection>,
    request: Request,
) -> Reply {
    require_view_or_launch(&caller(&h, &c)?)?;
    let body = read_body(request).await?;
    tokio::task::spawn_blocking(move || write_settings(&h, &c, &body))
        .await
        .map_err(|error| Failure::internal("write the audio settings", error))?
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WriteBody {
    changes: Vec<ChangeBody>,
}
/// A change with its value unread, so a value of a shape the API has no type for is an
/// `invalid_value` naming the setting instead of a malformed body.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChangeBody {
    id: String,
    value: serde_json::Value,
}

fn write_settings(h: &Shared, connection: &Connection, body: &[u8]) -> Reply {
    // The permissions and the session are judged right before the write, not when it arrived.
    let client = caller(h, connection)?;
    require_view_or_launch(&client)?;
    let request: WriteBody = serde_json::from_slice(body).map_err(|error| {
        Failure::new(
            ErrorCode::InvalidRequest,
            format!("The body is not a valid audio write: {error}."),
        )
    })?;
    if request.changes.is_empty() {
        return Err(Failure::new(
            ErrorCode::InvalidRequest,
            "The request changes nothing.",
        ));
    }
    let write = AudioWrite {
        changes: request
            .changes
            .into_iter()
            .map(|change| Change {
                id: change.id,
                // Anything but a boolean reaches `apply` as a non-boolean: `invalid_value`.
                value: change
                    .value
                    .as_bool()
                    .map_or_else(|| Value::Choice(change.value.to_string()), Value::Bool),
            })
            .collect(),
    };
    // The settings belong to the caller's own stream, whoever owns the PC's main session: a
    // second device streaming beside it changes its own sound. Without such a stream (none, a
    // suspended one, another device's, or only a monitor or input session) there is nothing to
    // change, as the read says.
    let s = own_stream(h, &client).ok_or(Refusal::NoSession)?;
    let base = base(h, &s)?;
    let audio = audio_dsp::apply(&s.audio, base, &write).map_err(|error| {
        let failure = Failure::new(error.code, error.message);
        match error.setting {
            Some(id) => failure.for_setting(&id),
            None => failure,
        }
    })?;
    tracing::info!(
        client = %client.name,
        changes = ?write.changes,
        "native API: audio settings changed for this stream"
    );
    Ok(json(StatusCode::OK, &audio))
}

/// The caller's live Stream session, whose audio thread reads the settings. A Remote Monitor or
/// input-only session has no sound of its own to change, so it counts as none.
fn own_stream(h: &Shared, client: &Client) -> Option<Arc<Session>> {
    let sessions = h.sessions.lock().unwrap();
    sessions
        .active
        .values()
        .find(|s| {
            !s.stopping() && s.launch.role == Role::Stream && s.launch.client.uuid == client.uuid
        })
        .cloned()
}

/// The choice `s` starts with, for a setting its audio thread has not resolved yet: the launch
/// parameter, the device's remembered live choice for the app, then the app, device and host.
fn base(h: &Shared, s: &Session) -> Result<Settings, Failure> {
    let config = crate::stream::effective_config(h, &s.launch)
        .map_err(|error| Failure::internal("read the stream's audio settings", error))?;
    let remembered = h
        .audio_memory
        .recall(&s.launch.client.uuid, s.launch.app_id);
    Ok(audio_dsp::resolve(&config, &s.launch.options, remembered))
}
