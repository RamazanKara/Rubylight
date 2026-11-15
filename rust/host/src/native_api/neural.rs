//! `GET /neural?app=<id>` and `POST /neural/settings`: a game's Neural
//! Rendering switches from a paired device, as the console's game page has
//! them. The contract is `docs/native-api.md`, "Neural Rendering".
//!
//! Reading needs View streams; a write needs View streams and Host control,
//! judged again right before it runs. A write is refused while a stream
//! runs, as the console's is, and never names a program on the PC: when the
//! host can't find the game's, the switch says to choose it in the console.
use super::{Failure, Reply, caller, json, read_body, require, require_host_control};
use crate::{
    neural::{self, Refusal},
    state::Shared,
    tls::Connection,
};
use axum::{
    Extension,
    extract::{RawQuery, Request, State},
    http::StatusCode,
};
use butterpollo_core::{
    native::VIEW,
    native_api::{
        error::ErrorCode,
        neural::{self as api, Action, FRAME_GENERATION, Job, Neural, Phase, SettingsWrite},
        radeon::ControlState,
    },
    state::Client,
};
use serde_json::Value;

/// How soon a client asks again while the host is busy with the game.
const BUSY_RETRY_MS: u64 = 1500;
/// What a switch says when the host could not find the game's program.
const CHOOSE: &str = "Choose the game's program in the Rubylight console on the PC.";
/// What a switch that is off says while a stream runs.
const STREAMING_ON: &str = "Frame generation downloads about 14 MB and copies it into the game's folder: turn it on when no stream is running.";
/// What a switch that is on says while a stream runs.
const STREAMING_OFF: &str =
    "The game may be running with frame generation loaded: turn it off when no stream is running.";
/// What a switch now `on` says while a stream runs.
fn streaming(on: bool) -> &'static str {
    if on { STREAMING_OFF } else { STREAMING_ON }
}
/// What a switch says to a device without Host control.
const NO_HOST_CONTROL: &str =
    "Turn on Host control for this device in the Rubylight console to change it.";

fn reader(client: &Client) -> Result<(), Failure> {
    require(client, VIEW, "View streams")
}
fn writer(h: &Shared, connection: &Connection) -> Result<Client, Failure> {
    let client = caller(h, connection)?;
    reader(&client)?;
    require_host_control(&client)?;
    Ok(client)
}

pub async fn read(
    State(h): State<Shared>,
    Extension(c): Extension<Connection>,
    RawQuery(query): RawQuery,
) -> Reply {
    let client = caller(&h, &c)?;
    reader(&client)?;
    let app = url::form_urlencoded::parse(query.unwrap_or_default().as_bytes())
        .find(|(key, _)| key == "app")
        .map(|(_, value)| value.into_owned())
        .filter(|app| !app.is_empty())
        .ok_or_else(|| Failure::new(ErrorCode::InvalidRequest, "Name the game: ?app=<id>."))?;
    let host_control = butterpollo_core::native::host_control(&client);
    tokio::task::spawn_blocking(move || {
        state(&h, &app, host_control).map(|state| json(StatusCode::OK, &state))
    })
    .await
    .map_err(|error| Failure::internal("read Neural Rendering", error))?
}

pub async fn write(
    State(h): State<Shared>,
    Extension(c): Extension<Connection>,
    request: Request,
) -> Reply {
    writer(&h, &c)?;
    let body = read_body(request).await?;
    tokio::task::spawn_blocking(move || apply(&h, &c, &body))
        .await
        .map_err(|error| Failure::internal("turn Neural Rendering", error))?
}

fn apply(h: &Shared, connection: &Connection, body: &[u8]) -> Reply {
    // The permissions are judged right before the write, not when it arrived.
    writer(h, connection)?;
    let request: SettingsWrite = serde_json::from_slice(body).map_err(|error| {
        Failure::new(
            ErrorCode::InvalidRequest,
            format!("The body is not a valid Neural Rendering write: {error}."),
        )
    })?;
    if request.switch != FRAME_GENERATION {
        return Err(Failure::new(
            ErrorCode::InvalidValue,
            format!(
                "This host has no Neural Rendering switch {:?}.",
                request.switch
            ),
        )
        .for_setting(&request.switch));
    }
    if neural::status(h, &request.app)["found"] != true {
        return Err(not_found());
    }
    if request.on && request.accept_notice && !neural::accepted(h) {
        neural::accept(h).map_err(|error| Failure::internal("record the notice", error))?;
    }
    let done = if request.on {
        neural::enable(h, &request.app, None)
    } else {
        neural::disable(h, &request.app)
    };
    done.map_err(|refusal| refused(refusal, !request.on).for_setting(FRAME_GENERATION))?;
    Ok(json(StatusCode::ACCEPTED, &state(h, &request.app, true)?))
}

fn not_found() -> Failure {
    Failure::new(ErrorCode::NotFound, "This host has no such game.")
}

/// `on`: the switch is on, so the refused write turned it off.
fn refused(refusal: Refusal, on: bool) -> Failure {
    match refusal {
        Refusal::NotAccepted => Failure::new(
            ErrorCode::NotWritable,
            "Show the Neural Rendering notice and send accept_notice once the user accepts it.",
        ),
        Refusal::Unavailable(reason) => Failure::new(ErrorCode::NotWritable, sentence(&reason)),
        Refusal::NotFound => not_found(),
        Refusal::Busy => Failure::new(
            ErrorCode::TemporarilyUnavailable,
            "Neural Rendering is busy with this game. Try again in a moment.",
        )
        .retry_after(BUSY_RETRY_MS),
        Refusal::Streaming => Failure::new(ErrorCode::HostInUse, streaming(on)),
    }
}

/// `text` as a sentence: a capital first letter and a full stop.
fn sentence(text: &str) -> String {
    let mut chars = text.trim().chars();
    let mut out: String = chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default();
    if !out.ends_with(['.', '!', '?']) {
        out.push('.');
    }
    out
}

/// The game's switches as the console's game page has them. Blocks: the
/// first answer of a host start asks which GPU streams. host_control:
/// the caller may write it.
fn state(h: &Shared, app: &str, host_control: bool) -> Result<Neural, Failure> {
    let status = neural::status(h, app);
    if status["found"] != true {
        return Err(not_found());
    }
    let text = |value: &Value| value.as_str().map(str::to_owned);
    let job = Some(&status["job"])
        .filter(|job| job.is_object())
        .map(|job| Job {
            action: match job["action"].as_str() {
                Some("install") => Action::Install,
                Some("remove") => Action::Remove,
                Some("update") => Action::Update,
                _ => Action::Unknown,
            },
            phase: match job["phase"].as_str() {
                Some("download") => Phase::Download,
                Some("place") => Phase::Place,
                Some("remove") => Phase::Remove,
                Some("done") => Phase::Done,
                Some("failed") => Phase::Failed,
                _ => Phase::Unknown,
            },
            done: job["done"].as_u64().unwrap_or(0),
            total: job["total"].as_u64().unwrap_or(0),
            error: text(&job["error"]).map(|error| sentence(&error)),
        });
    let on = status["enabled"] == true;
    let working = job
        .as_ref()
        .is_some_and(|job| !matches!(job.phase, Phase::Done | Phase::Failed));
    let choose = job.as_ref().is_some_and(|job| job.phase == Phase::Failed)
        && status["job"]["choose_exe"] == true;
    let (state, reason) = if working {
        (
            ControlState::TemporarilyUnavailable,
            Some("Neural Rendering is busy with this game.".to_owned()),
        )
    } else if status["available"] != true && !on {
        (
            ControlState::Unsupported,
            Some(sentence(
                status["reason"].as_str().unwrap_or("This PC can't run it"),
            )),
        )
    } else if !host_control {
        // As Radeon's: the device can see the switch but would be refused a write.
        (ControlState::ReadOnly, Some(NO_HOST_CONTROL.into()))
    } else if crate::updater::streaming(h) {
        (
            ControlState::TemporarilyUnavailable,
            Some(streaming(on).into()),
        )
    } else if choose && !on {
        (ControlState::NeedsExternalSetup, Some(CHOOSE.into()))
    } else {
        (ControlState::Configurable, None)
    };
    Ok(Neural {
        app: app.to_owned(),
        accepted: status["accepted"] == true,
        gpu: text(&status["gpu"]),
        switches: vec![api::Switch {
            id: FRAME_GENERATION.into(),
            state,
            on,
            reason,
        }],
        replaces_earlier_upscaling: !status["earlier_upscaling"].is_null(),
        job,
    })
}
