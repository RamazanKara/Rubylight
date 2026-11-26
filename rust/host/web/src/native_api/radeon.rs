//! `GET /radeon`, `POST /radeon/settings` and `POST /radeon/restore` (plan item A3, brief 4).
//! The contract is `docs/native-api.md`, "Radeon state and writes".
//!
//! Every driver call goes through the Radeon worker thread (`butterpollo_win_graphics::radeon`), and
//! every handler waits for it on `spawn_blocking`, so neither the async executor nor a stream
//! thread ever waits for ADLX. Reads are cached for one second.
use super::{
    Failure, Reply, caller, expect_empty_body, json, read_body, require, require_host_control,
};
use axum::{
    Extension,
    extract::{Request, State},
    http::StatusCode,
};
use butterpollo_session::{
    native::{self, LAUNCH, VIEW},
    session_state::Refusal,
    state::Client,
};
use butterpollo_stream::{
    radeon_session::{READ_WAIT, View, current_view, panel_view, store},
    state::{Host, Shared},
    tls::Connection,
};
use butterpollo_win_graphics::radeon::{
    ErrorKind, WriteError, control::GpuInfo, model::Value, worker::Panel,
};
use rubylight_native_api::{
    error::ErrorCode,
    radeon::{self as api, ControlState},
    session::State as Session,
};
use serde::Deserialize;

/// How soon a client retries after the driver or the worker was not available.
const UNAVAILABLE_RETRY_MS: u64 = 5000;
/// How soon a client looks again after a restore failed: the worker retries every minute.
const RESTORE_RETRY_MS: u64 = 60_000;

/// The caller as the console has it now, with the permissions every Radeon write needs: View
/// (Launch alone is not enough) and host control.
fn gate(h: &Shared, connection: &Connection) -> Result<Client, Failure> {
    let client = caller(h, connection)?;
    may_write(&client)?;
    Ok(client)
}
/// The permissions a write needs, which `writable_now` reports with the same check.
fn may_write(client: &Client) -> Result<(), Failure> {
    require(client, VIEW, "View streams")?;
    require_host_control(client)
}
/// Reading the panel needs View or Launch.
fn reader(client: &Client) -> Result<(), Failure> {
    require(client, VIEW, "View streams")
        .or_else(|_| require(client, LAUNCH, "Launch applications"))
}

/// A write or restore is for the owner of a streaming or suspended session.
fn owner_check(h: &Host, client: &Client) -> Result<(), Failure> {
    let snapshot = h.session_snapshot();
    match snapshot.state {
        Session::Idle => Err(Refusal::NoSession.into()),
        _ if snapshot.owner.as_deref() != Some(client.uuid.as_str()) => {
            Err(Refusal::NotOwner.into())
        }
        Session::Streaming | Session::Suspended => Ok(()),
        _ => Err(Refusal::Busy.into()),
    }
}

pub async fn read(State(h): State<Shared>, Extension(c): Extension<Connection>) -> Reply {
    let client = caller(&h, &c)?;
    reader(&client)?;
    tokio::task::spawn_blocking(move || {
        let (view, revision) = current_view(&h);
        answer(&h, &client, &view, revision)
    })
    .await
    .map_err(|error| Failure::internal("read the Radeon settings", error))?
}

pub async fn write(
    State(h): State<Shared>,
    Extension(c): Extension<Connection>,
    request: Request,
) -> Reply {
    gate(&h, &c)?;
    let body = read_body(request).await?;
    tokio::task::spawn_blocking(move || write_settings(&h, &c, &body))
        .await
        .map_err(|error| Failure::internal("write the Radeon settings", error))?
}

pub async fn restore(
    State(h): State<Shared>,
    Extension(c): Extension<Connection>,
    request: Request,
) -> Reply {
    gate(&h, &c)?;
    expect_empty_body(request).await?;
    tokio::task::spawn_blocking(move || restore_settings(&h, &c))
        .await
        .map_err(|error| Failure::internal("restore the Radeon settings", error))?
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WriteBody {
    /// Read wide so an id no GPU has is `not_found`, not a malformed body.
    gpu: i64,
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
    let client = gate(h, connection)?;
    owner_check(h, &client)?;
    let request: WriteBody = serde_json::from_slice(body).map_err(|error| {
        Failure::new(
            ErrorCode::InvalidRequest,
            format!("The body is not a valid driver settings write: {error}."),
        )
    })?;
    if request.changes.is_empty() {
        return Err(Failure::new(ErrorCode::InvalidRequest, "The request changes nothing.").into());
    }
    let (view, _) = current_view(h);
    let Some(gpu) = view
        .gpus
        .iter()
        .find(|gpu| i64::from(gpu.id) == request.gpu)
    else {
        return Err(match view.state {
            ControlState::TemporarilyUnavailable => unavailable(view.message.clone()).into(),
            _ => Failure::new(ErrorCode::NotFound, "This host has no GPU with that id.").into(),
        });
    };
    let mut changes = Vec::with_capacity(request.changes.len());
    for change in request.changes {
        if !gpu.settings.iter().any(|setting| setting.id == change.id) {
            return Err(
                Failure::new(ErrorCode::NotFound, "This GPU has no such driver setting.")
                    .for_setting(&change.id)
                    .into(),
            );
        }
        let value = match &change.value {
            serde_json::Value::Bool(on) => Value::Bool(*on),
            serde_json::Value::Number(number) if number.is_i64() => {
                Value::Int(number.as_i64().unwrap_or_default())
            }
            serde_json::Value::String(text) => Value::Choice(text.clone()),
            _ => {
                return Err(Failure::new(
                    ErrorCode::InvalidValue,
                    "A value is a boolean, a whole number or a string.",
                )
                .for_setting(&change.id)
                .into());
            }
        };
        changes.push(butterpollo_win_graphics::radeon::Change {
            id: change.id,
            value,
        });
    }
    let Some(worker) = h.radeon.worker() else {
        return Err(no_radeon().into());
    };
    let gpu_info = GpuInfo {
        id: gpu.id,
        name: gpu.name.clone(),
        driver: gpu.driver.clone(),
    };
    let gpu = gpu_info.id;
    // The session restores what this request changes, even when it fails half way or the call
    // gives up waiting: the device is noted before the first write and again after the last.
    let device = client.uuid.as_str();
    let result = h.radeon.apply(device, gpu, changes);
    // The session may have ended while the driver wrote: bring the restore up to date.
    h.session_tick();
    h.radeon.forget();
    let settings = match result {
        Ok(settings) => settings,
        Err(error) => return Err(write_failure(&error).into()),
    };
    // The write read the settings back already; only the journal's entries are new.
    let (view, revision) = match worker.pending(READ_WAIT) {
        Ok(pending) => {
            let panel = Panel {
                gpu: gpu_info,
                settings,
                pending,
            };
            let status = worker.status();
            let failed = h.radeon.failed(&panel.pending);
            store(h, panel_view(&panel, &status, &failed, h.radeon.driver()))
        }
        Err(_) => current_view(h),
    };
    answer(h, &client, &view, revision)
}

fn write_failure(error: &WriteError) -> Failure {
    let failure = match error.kind {
        ErrorKind::InvalidValue => Failure::new(ErrorCode::InvalidValue, &error.message),
        ErrorKind::NotWritable => Failure::new(ErrorCode::NotWritable, &error.message),
        // The driver's own words name codes and values: they are the detail.
        ErrorKind::DriverRejected => {
            Failure::new(ErrorCode::DriverRejected, &error.message).with_detail(error.to_string())
        }
        ErrorKind::Unavailable => unavailable(Some(error.message.clone()))
            .explained(
                "The graphics driver is not answering",
                Some("Wait a moment, then try again. If it keeps failing, restart the PC."),
            )
            .with_detail(error.to_string()),
        ErrorKind::Journal => {
            tracing::error!(error = %error, "native API: the driver settings journal could not be written; nothing was changed");
            Failure::new(
                ErrorCode::Internal,
                format!(
                    "The host could not write its driver settings journal, so nothing was changed: {}",
                    error.message
                ),
            )
            .explained(
                "The PC could not save its graphics settings backup",
                Some(
                    "Nothing was changed. Try again; if it keeps failing, open Help & diagnostics in the Rubylight console on the PC.",
                ),
            )
            .with_detail(error.to_string())
        }
    };
    match &error.id {
        Some(id) => failure.for_setting(id),
        None => failure,
    }
}

/// The host has no Radeon controls at all: the feature is absent, like an unknown path.
fn no_radeon() -> Failure {
    Failure::new(
        ErrorCode::NotFound,
        "This host has no driver settings controls.",
    )
}

fn unavailable(message: Option<String>) -> Failure {
    Failure::new(
        ErrorCode::TemporarilyUnavailable,
        message.unwrap_or_else(|| {
            "The graphics driver is not available. Try again in a moment.".into()
        }),
    )
    .retry_after(UNAVAILABLE_RETRY_MS)
}

fn restore_settings(h: &Shared, connection: &Connection) -> Reply {
    let client = gate(h, connection)?;
    if h.radeon.worker().is_none() {
        return Err(no_radeon().into());
    }
    owner_check(h, &client)?;
    let result = h.radeon.restore_now(&client.uuid);
    h.radeon.forget();
    match result {
        Ok(report) if report.is_clean() => {}
        // The worker logged the setting ids and keeps them journaled.
        Ok(report) => {
            let ids: Vec<&str> = report.failed().map(|outcome| outcome.id.as_str()).collect();
            return Err(Failure::new(
                ErrorCode::TemporarilyUnavailable,
                format!(
                    "These settings could not be put back yet: {}. The host keeps them journaled and tries again.",
                    ids.join(", ")
                ),
            )
            .retry_after(RESTORE_RETRY_MS)
            .explained("Some graphics settings are not back yet", None)
            .into());
        }
        Err(error) => {
            tracing::error!(error = %format!("{error:#}"), "native API: the Radeon settings could not be restored");
            return Err(Failure::new(
                ErrorCode::TemporarilyUnavailable,
                format!("The Radeon settings could not be put back yet: {error:#}"),
            )
            .retry_after(RESTORE_RETRY_MS)
            .explained(
                "The graphics settings are not back yet",
                Some("Wait a moment, then try again."),
            )
            .with_detail(format!("{error:#}"))
            .into());
        }
    }
    let (view, revision) = current_view(h);
    answer(h, &client, &view, revision)
}

/// The panel for `client`: what the driver shows now plus the caller's own flags.
fn answer(h: &Host, client: &Client, view: &View, revision: u64) -> Reply {
    let snapshot = h.session_snapshot();
    let writable_now = may_write(client).is_ok()
        && snapshot.owner.as_deref() == Some(client.uuid.as_str())
        && matches!(snapshot.state, Session::Streaming | Session::Suspended);
    Ok(json(
        StatusCode::OK,
        &api::Radeon {
            revision,
            state: view.state,
            reason: view.reason.clone(),
            message: view.message.clone(),
            host_control: native::host_control(client),
            writable_now,
            gpus: view.gpus.clone(),
        },
    ))
}

#[cfg(test)]
mod error_tests {
    use super::*;

    /// A driver's code or HRESULT, which only the technical detail may carry.
    fn coded(text: &str) -> bool {
        text.to_ascii_lowercase().contains("0x") || text.contains("ADLX")
    }

    #[test]
    fn a_driver_failure_says_what_to_do_and_keeps_its_codes_in_the_detail() {
        for kind in [
            ErrorKind::DriverRejected,
            ErrorKind::Unavailable,
            ErrorKind::Journal,
        ] {
            let error = WriteError {
                kind,
                id: Some("anti_lag.enabled".into()),
                message: "ADLX_FAIL (0x80004005)".into(),
            };
            let body = write_failure(&error).into_api_error();
            let title = body.title.as_deref().unwrap();
            let next_step = body.next_step.as_deref().unwrap();
            assert!(
                !coded(title) && !coded(next_step),
                "{kind:?}: {title} / {next_step}"
            );
            let detail = body.technical_detail.as_deref().unwrap();
            assert!(
                detail.contains("ADLX_FAIL (0x80004005)"),
                "{kind:?}: {detail}"
            );
            assert!(
                body.message.contains("0x80004005"),
                "{kind:?}: the message stays"
            );
            assert_eq!(body.setting.as_deref(), Some("anti_lag.enabled"));
        }
    }

    #[test]
    fn a_refused_value_is_left_to_its_message() {
        let error = WriteError {
            kind: ErrorKind::InvalidValue,
            id: Some("sharpening.level".into()),
            message: "sharpening.level is off the step".into(),
        };
        let body = write_failure(&error).into_api_error();
        assert_eq!(body.title, None);
        assert_eq!(body.next_step, None);
        assert_eq!(body.technical_detail, None);
    }
}
