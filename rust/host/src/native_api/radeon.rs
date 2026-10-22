//! `GET /radeon`, `POST /radeon/settings` and `POST /radeon/restore` (plan item A3, brief 4).
//! The contract is `docs/native-api.md`, "Radeon state and writes".
//!
//! Every driver call goes through the Radeon worker thread (`butterpollo_windows::radeon`), and
//! every handler waits for it on `spawn_blocking`, so neither the async executor nor a stream
//! thread ever waits for ADLX. Reads are cached for one second.
use super::{
    Failure, Reply, caller, expect_empty_body, json, read_body, require, require_host_control,
};
use crate::{
    radeon_session::{CACHE, READ_WAIT, Reading, View, locked},
    state::{Host, Shared},
    tls::Connection,
};
use axum::{
    Extension,
    extract::{Request, State},
    http::StatusCode,
};
use butterpollo_core::{
    native::{self, LAUNCH, VIEW},
    native_api::{
        error::ErrorCode,
        radeon::{self as api, ControlState},
        session::State as Session,
    },
    session_state::Refusal,
    state::Client,
};
use butterpollo_windows::radeon::{
    Entry, ErrorKind, GpuInfo, WriteError,
    model::{self, Value},
    worker::{Panel, Phase, Status},
};
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    sync::{Arc, PoisonError},
    time::Instant,
};

/// How soon a client retries after the driver or the worker was not available.
const UNAVAILABLE_RETRY_MS: u64 = 5000;
/// How soon a client looks again after a restore failed: the worker retries every minute.
const RESTORE_RETRY_MS: u64 = 60_000;

/// The caller as the console has it now, with the permissions every Radeon write needs: View
/// (Launch alone is not enough) and host control.
fn gate(h: &Shared, connection: &Connection) -> Result<Client, Failure> {
    let client = caller(h, connection)?;
    require(&client, VIEW, "View streams")?;
    require_host_control(&client)?;
    Ok(client)
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
            format!("The body is not a valid Radeon write: {error}."),
        )
    })?;
    if request.changes.is_empty() {
        return Err(Failure::new(
            ErrorCode::InvalidRequest,
            "The request changes nothing.",
        ));
    }
    let (view, _) = current_view(h);
    let Some(gpu) = view
        .gpus
        .iter()
        .find(|gpu| i64::from(gpu.id) == request.gpu)
    else {
        return Err(match view.state {
            ControlState::TemporarilyUnavailable => unavailable(view.message.clone()),
            _ => Failure::new(ErrorCode::NotFound, "This host has no GPU with that id."),
        });
    };
    let mut changes = Vec::with_capacity(request.changes.len());
    for change in request.changes {
        if !gpu.settings.iter().any(|setting| setting.id == change.id) {
            return Err(
                Failure::new(ErrorCode::NotFound, "This GPU has no such Radeon setting.")
                    .for_setting(&change.id),
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
                .for_setting(&change.id));
            }
        };
        changes.push(butterpollo_windows::radeon::Change {
            id: change.id,
            value,
        });
    }
    let Some(worker) = h.radeon.worker() else {
        return Err(no_radeon());
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
    h.radeon.note_written(device);
    let result = worker.apply(device, gpu, changes);
    h.radeon.note_written(device);
    // The session may have ended while the driver wrote: bring the restore up to date.
    h.session_tick();
    h.radeon.forget();
    let settings = match result {
        Ok(settings) => settings,
        Err(error) => return Err(write_failure(&error)),
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
            store(h, panel_view(&panel, &status, &failed))
        }
        Err(_) => current_view(h),
    };
    answer(h, &client, &view, revision)
}

fn write_failure(error: &WriteError) -> Failure {
    let failure = match error.kind {
        ErrorKind::InvalidValue => Failure::new(ErrorCode::InvalidValue, &error.message),
        ErrorKind::NotWritable => Failure::new(ErrorCode::NotWritable, &error.message),
        ErrorKind::DriverRejected => Failure::new(ErrorCode::DriverRejected, &error.message),
        ErrorKind::Unavailable => unavailable(Some(error.message.clone())),
        ErrorKind::Journal => {
            tracing::error!(error = %error, "native API: the Radeon journal could not be written; nothing was changed");
            Failure::new(
                ErrorCode::Internal,
                format!(
                    "The host could not write its Radeon journal, so nothing was changed: {}",
                    error.message
                ),
            )
        }
    };
    match &error.id {
        Some(id) => failure.for_setting(id),
        None => failure,
    }
}

/// The host has no Radeon controls at all: the feature is absent, like an unknown path.
fn no_radeon() -> Failure {
    Failure::new(ErrorCode::NotFound, "This host has no Radeon controls.")
}

fn unavailable(message: Option<String>) -> Failure {
    Failure::new(
        ErrorCode::TemporarilyUnavailable,
        message
            .unwrap_or_else(|| "The Radeon driver is not available. Try again in a moment.".into()),
    )
    .retry_after(UNAVAILABLE_RETRY_MS)
}

fn restore_settings(h: &Shared, connection: &Connection) -> Reply {
    let client = gate(h, connection)?;
    if h.radeon.worker().is_none() {
        return Err(no_radeon());
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
            .retry_after(RESTORE_RETRY_MS));
        }
        Err(error) => {
            tracing::error!(error = %format!("{error:#}"), "native API: the Radeon settings could not be restored");
            return Err(Failure::new(
                ErrorCode::TemporarilyUnavailable,
                format!("The Radeon settings could not be put back yet: {error:#}"),
            )
            .retry_after(RESTORE_RETRY_MS));
        }
    }
    let (view, revision) = current_view(h);
    answer(h, &client, &view, revision)
}

/// The panel for `client`: what the driver shows now plus the caller's own flags.
fn answer(h: &Host, client: &Client, view: &View, revision: u64) -> Reply {
    let snapshot = h.session_snapshot();
    let writable_now = native::host_control(client)
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

/// The panel, read at most once a second. The lock stays held through a read, so a burst of
/// requests shares one driver read. The revision moves when what the panel shows changes.
fn current_view(h: &Host) -> (Arc<View>, u64) {
    let mut cache = locked(&h.radeon.cache);
    if let Some(reading) = cache.as_ref()
        && reading.at.elapsed() < CACHE
    {
        return (reading.view.clone(), reading.revision);
    }
    store_in(h, &mut cache, read_view(h))
}

/// Keep `view` as the newest reading, as a write that read its settings back does.
fn store(h: &Host, view: View) -> (Arc<View>, u64) {
    let mut cache = locked(&h.radeon.cache);
    store_in(h, &mut cache, view)
}

fn store_in(h: &Host, cache: &mut Option<Reading>, view: View) -> (Arc<View>, u64) {
    let view = Arc::new(view);
    let revision = {
        let mut counted = locked(&h.radeon.revision);
        if counted.1.as_deref() != Some(&*view) {
            counted.0 += 1;
            counted.1 = Some(view.clone());
        }
        counted.0
    };
    *cache = Some(Reading {
        at: Instant::now(),
        view: view.clone(),
        revision,
    });
    (view, revision)
}

fn empty(state: ControlState, reason: &str, message: impl Into<String>) -> View {
    View {
        state,
        reason: Some(reason.into()),
        message: Some(message.into()),
        gpus: vec![],
    }
}

fn read_view(h: &Host) -> View {
    let Some(worker) = h.radeon.worker() else {
        return empty(
            ControlState::Unsupported,
            "no_radeon",
            "This host has no Radeon controls.",
        );
    };
    let status = worker.status();
    if status.phase == Phase::Recovering {
        return empty(
            ControlState::TemporarilyUnavailable,
            "recovering",
            "The host is putting back Radeon settings from an earlier run. Try again in a moment.",
        );
    }
    let adapter = h
        .config
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .get("adapter_name", "")
        .to_owned();
    match worker.panel(Some(&adapter), READ_WAIT) {
        Ok(Some(panel)) => panel_view(&panel, &status, &h.radeon.failed(&panel.pending)),
        Ok(None) => empty(
            ControlState::Unsupported,
            "no_amd_gpu",
            "AMD's driver lists no Radeon GPU on this PC.",
        ),
        Err(error) => {
            tracing::warn!(%error, "native API: the Radeon settings could not be read");
            empty(
                ControlState::TemporarilyUnavailable,
                "adlx_unavailable",
                format!("The Radeon driver could not be read: {error}"),
            )
        }
    }
}

/// The five states of the whole panel: the most useful one any setting has.
fn panel_state(settings: &[api::Setting]) -> ControlState {
    [
        ControlState::Configurable,
        ControlState::ReadOnly,
        ControlState::NeedsExternalSetup,
        ControlState::TemporarilyUnavailable,
    ]
    .into_iter()
    .find(|state| settings.iter().any(|setting| setting.state == *state))
    .unwrap_or(ControlState::Unsupported)
}

fn panel_view(panel: &Panel, status: &Status, failed: &BTreeMap<String, String>) -> View {
    let gpu = panel.gpu.id;
    let settings: Vec<api::Setting> = panel
        .settings
        .iter()
        .filter_map(|setting| setting_of(setting, gpu, &panel.pending, failed))
        .collect();
    let message = failed
        .get("*")
        .map(|why| format!("Radeon settings could not be put back: {why}. The host tries again."))
        .or_else(|| {
            (status.phase == Phase::Degraded)
                .then(|| status.error.clone())
                .flatten()
        })
        .or_else(|| {
            (status.phase == Phase::Degraded && status.unrestored > 0 && !panel.pending.is_empty())
                .then(|| {
                    format!(
                        "{} Radeon settings from an earlier session could not be put back. The host tries again.",
                        status.unrestored
                    )
                })
        });
    View {
        state: panel_state(&settings),
        reason: message.as_ref().map(|_| "restore_failed".into()),
        message,
        gpus: vec![api::Gpu {
            id: gpu,
            name: panel.gpu.name.clone(),
            driver: panel.gpu.driver.clone(),
            settings,
        }],
    }
}

fn value_of(value: &Value) -> api::Value {
    match value {
        Value::Bool(on) => api::Value::Bool(*on),
        Value::Int(number) => api::Value::Integer(*number),
        Value::Choice(text) => api::Value::Choice(text.clone()),
    }
}

/// `on_unless_app_specifies` as a label: "On unless app specifies".
fn label_of(option: &str) -> String {
    let mut text = option.replace('_', " ");
    if let Some(first) = text.get(..1) {
        text = first.to_uppercase() + &text[1..];
    }
    text
}

/// One setting as the API shows it. Actions have no value and are not listed.
fn setting_of(
    setting: &model::Setting,
    gpu: i32,
    pending: &[Entry],
    failed: &BTreeMap<String, String>,
) -> Option<api::Setting> {
    let kind = match setting.kind {
        model::Kind::Toggle => api::Kind::Toggle,
        model::Kind::Integer => api::Kind::Integer,
        model::Kind::Choice => api::Kind::Choice,
        model::Kind::Action => return None,
    };
    let state = match setting.state {
        model::State::Configurable => ControlState::Configurable,
        model::State::ReadOnly => ControlState::ReadOnly,
        model::State::NeedsExternalSetup => ControlState::NeedsExternalSetup,
        model::State::Unsupported => ControlState::Unsupported,
        model::State::TemporarilyUnavailable => ControlState::TemporarilyUnavailable,
    };
    // Unsupported and unavailable settings show nothing a client could mistake for a value.
    let shown = !matches!(
        state,
        ControlState::Unsupported | ControlState::TemporarilyUnavailable
    );
    let entry = pending
        .iter()
        .find(|entry| entry.gpu == gpu && entry.id == setting.id);
    let (reason, mut message) = match state {
        ControlState::NeedsExternalSetup => (
            Some("external_setup"),
            Some(format!(
                "{} needs a one-time setup in AMD Software: Adrenalin Edition.",
                setting.label
            )),
        ),
        ControlState::TemporarilyUnavailable => (
            Some("driver_busy"),
            Some("The driver is busy. Try again in a moment.".to_owned()),
        ),
        ControlState::ReadOnly => (Some("read_only"), None),
        _ => (None, None),
    };
    let mut reason = reason.map(String::from);
    if let (Some(entry), Some(why)) = (entry, failed.get(setting.id)) {
        reason = Some("restore_failed".into());
        message = Some(format!(
            "{} could not be put back to {}: {why}. The host tries again.",
            setting.label,
            entry.original.display()
        ));
    }
    Some(api::Setting {
        id: setting.id.into(),
        group: setting.group.into(),
        kind,
        state,
        value: setting.value.as_ref().filter(|_| shown).map(value_of),
        range: setting
            .range
            .filter(|_| shown && kind == api::Kind::Integer)
            .map(|range| api::Range {
                min: range.min,
                max: range.max,
                step: range.step,
            }),
        options: if shown {
            setting
                .options
                .iter()
                .map(|option| api::Choice {
                    value: option.value.clone(),
                    label: label_of(&option.value),
                })
                .collect()
        } else {
            vec![]
        },
        unit: setting.unit.filter(|_| shown).map(String::from),
        restore: entry.map(|entry| api::Restore {
            original: value_of(&entry.original),
            written: value_of(&entry.written),
        }),
        reason,
        message,
    })
}
