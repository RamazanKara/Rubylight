//! Restart and shut down the PC, and install the host's pending update
//! (`docs/native-api.md`, "Power and update"). Restart and Shut down take
//! Sleep PC's idle gate and a 30 s token of their own (`power_action`), and
//! need the device's own Restart and shut down PC grant; installing needs
//! Host control and no other device streaming. Every refusal is logged.
use super::{
    Failure, Reply, caller, expect_empty_body, json, permission_required, read_body,
    require_host_control,
};
use crate::{
    power_action::{busy_beside_gate, idle_or_busy, streaming},
    state::Shared,
    tls::{Connection, ConnectionEnd},
};
use axum::{
    Extension,
    extract::{Request, State},
    http::{StatusCode, header},
    response::IntoResponse,
};
use butterpollo_core::{
    native_api::{
        error::ErrorCode,
        power::{Availability, Power, PowerAccepted, PowerRequest, Update},
        wake::{AdapterKind, TargetState},
    },
    power_action::{self, Action, Authority, Refused},
    state::Client,
};
use std::{
    net::IpAddr,
    sync::PoisonError,
    time::{Duration, Instant},
};

/// How long an accepted request waits for its answer to leave the host
/// before it gives up without restarting or shutting down.
const DELIVERY_WAIT: Duration = Duration::from_secs(2);
const TOKEN_EXPIRED: &str = "This confirmation expired or was already used. Confirm again.";

fn authority(client: &Client) -> Authority {
    Authority {
        uuid: client.uuid.clone(),
        allow_sleep: client.allows_sleep(),
        allow_restart_shutdown: client.allows_restart_shutdown(),
    }
}

/// `GET /power`.
pub async fn read(State(h): State<Shared>, Extension(c): Extension<Connection>) -> Reply {
    let client = caller(&h, &c)?;
    let local = c.local.ip();
    let power = tokio::task::spawn_blocking(move || offer(&h, &client, local))
        .await
        .map_err(|error| Failure::internal("read the power actions", error))?;
    Ok(json(StatusCode::OK, &power))
}
fn offer(h: &Shared, client: &Client, local: IpAddr) -> Power {
    let offer = h.power.power_tokens.lock().unwrap().offer(
        &authority(client),
        || idle_or_busy(h),
        Instant::now(),
        power_action::os_random,
    );
    let availability = || Availability {
        available: offer.reason.is_none(),
        reason: offer.reason.clone(),
    };
    Power {
        restart: availability(),
        shutdown: availability(),
        token: offer.token.clone(),
        token_lifetime_s: power_action::TOKEN_LIFETIME.as_secs() as u32,
        wake_from_off: wake_from_off(h, local),
        last_refusal: h.power.last_refusal(&client.uuid, Instant::now()),
    }
}
/// A ready wired target: Wi-Fi can't wake a PC that is off.
fn wake_from_off(h: &Shared, local: IpAddr) -> bool {
    match crate::wake::readiness(h, false, Some(local)) {
        Ok(wake) => wake
            .targets
            .iter()
            .any(|t| t.kind == AdapterKind::Ethernet && t.state == TargetState::Ready),
        Err(error) => {
            tracing::warn!(error = %format!("{error:#}"), "power: the wake targets did not read; reporting no wake from off");
            false
        }
    }
}

pub async fn restart(
    State(h): State<Shared>,
    Extension(c): Extension<Connection>,
    Extension(end): Extension<ConnectionEnd>,
    request: Request,
) -> Reply {
    power(h, c, end, request, Action::Restart).await
}
pub async fn shutdown(
    State(h): State<Shared>,
    Extension(c): Extension<Connection>,
    Extension(end): Extension<ConnectionEnd>,
    request: Request,
) -> Reply {
    power(h, c, end, request, Action::Shutdown).await
}
async fn power(
    h: Shared,
    c: Connection,
    end: ConnectionEnd,
    request: Request,
    action: Action,
) -> Reply {
    let token = match power_body(&h, &c, request).await {
        Ok(token) => token,
        Err(failure) => return Err(refused(action.name(), &c, None, failure)),
    };
    let (sender, receiver) = tokio::sync::oneshot::channel();
    tokio::task::spawn_blocking(move || power_blocking(&h, &c, &end, action, &token, sender));
    receiver
        .await
        .map_err(|error| Failure::internal("run the power action", error))?
}
async fn power_body(h: &Shared, c: &Connection, request: Request) -> Result<String, Failure> {
    caller(h, c)?;
    let body = read_body(request).await?;
    let request: PowerRequest = serde_json::from_slice(&body).map_err(|error| {
        Failure::new(
            ErrorCode::InvalidRequest,
            format!("The body is not a valid power request: {error}."),
        )
    })?;
    Ok(request.token)
}
/// Logs a refusal with what was asked and by whom, and hands it back.
fn refused(what: &str, c: &Connection, client: Option<&Client>, failure: Failure) -> Failure {
    tracing::warn!(
        what,
        client = client.map_or("", |c| c.name.as_str()),
        peer = %c.peer,
        code = ?failure.code(),
        message = failure.message(),
        "native API: power request refused"
    );
    failure
}
/// Runs on the blocking pool: the lifecycle gate (a std mutex) is held from
/// the check through the call to Windows, never across an `.await`.
fn power_blocking(
    h: &Shared,
    connection: &Connection,
    end: &ConnectionEnd,
    action: Action,
    token: &str,
    respond: tokio::sync::oneshot::Sender<Reply>,
) {
    let what = action.name();
    let client = match caller(h, connection) {
        Ok(client) => client,
        Err(failure) => {
            let _ = respond.send(Err(refused(what, connection, None, failure)));
            return;
        }
    };
    let decision = h.power.power_tokens.lock().unwrap().power(
        &authority(&client),
        token,
        || idle_or_busy(h),
        Instant::now(),
    );
    let gate = match decision {
        Ok(gate) => gate,
        Err(refusal) => {
            let failure = match refusal {
                Refused::NotAllowed => permission_required("Restart and shut down PC"),
                Refused::Busy(busy) => Failure::new(ErrorCode::HostInUse, busy.message()),
                Refused::Token => Failure::new(ErrorCode::TokenExpired, TOKEN_EXPIRED),
            };
            let _ = respond.send(Err(refused(what, connection, Some(&client), failure)));
            return;
        }
    };
    let peer = connection.peer;
    let name = client.name.as_str();
    tracing::warn!(what, client = name, %peer, "native API: power request accepted");
    // The device's earlier refusal is over; a new one is this request's.
    h.power.set_last_refusal(&client.uuid, None);
    // As Sleep PC: the answer closes its connection, and the PC goes down only
    // once hyper wrote and closed it.
    let delivery = end.watch();
    let answer = (
        StatusCode::ACCEPTED,
        [(header::CONNECTION, "close")],
        json(
            StatusCode::ACCEPTED,
            &PowerAccepted {
                action: what.into(),
            },
        ),
    )
        .into_response();
    if respond.send(Ok(answer)).is_err() {
        tracing::warn!(what, client = name, %peer, "power: the device left before its answer; not going ahead");
        return;
    }
    if !matches!(delivery.recv_timeout(DELIVERY_WAIT), Ok(true)) {
        tracing::warn!(what, client = name, %peer, "power: the answer was not delivered; not going ahead");
        return;
    }
    // A grant revoked meanwhile is obeyed.
    if !caller(h, connection).is_ok_and(|c| c.uuid == client.uuid && c.allows_restart_shutdown()) {
        tracing::warn!(what, client = name, %peer, "power: the grant was revoked; not going ahead");
        return;
    }
    // A browser guest or a controller session doesn't wait for the gate.
    if let Some(busy) = busy_beside_gate(h) {
        let busy = busy.message();
        tracing::warn!(what, client = name, %peer, busy, "power: the PC got busy before the request; not going ahead");
        h.power.set_last_refusal(&client.uuid, Some(busy));
        return;
    }
    h.webhook(
        action.event(),
        serde_json::json!({"device": h.webhook_device(&client.uuid)}),
    );
    let backend = h.power.backend();
    match backend.power_off(action) {
        Ok(()) => {
            let native = backend.native();
            tracing::warn!(what, client = name, %peer, native, "power: Windows took the request");
        }
        Err(error) => {
            let error = format!("{error:#}");
            tracing::warn!(what, client = name, %peer, %error, "power: Windows refused");
            h.power.set_last_refusal(&client.uuid, Some(error));
        }
    }
    drop(gate);
}

fn update_of(h: &Shared) -> Update {
    let status = crate::updater::status(h);
    let text = |key: &str| status[key].as_str().map(str::to_owned);
    Update {
        current_version: env!("CARGO_PKG_VERSION").into(),
        latest_version: text("latest_version"),
        phase: text("phase").unwrap_or_else(|| "idle".into()),
        install_supported: status["install_supported"] == true,
        error: text("error"),
        waiting_for: text("waiting_for"),
    }
}

/// `GET /update`.
pub async fn update(State(h): State<Shared>, Extension(c): Extension<Connection>) -> Reply {
    caller(&h, &c)?;
    let update = tokio::task::spawn_blocking(move || update_of(&h))
        .await
        .map_err(|error| Failure::internal("read the update", error))?;
    Ok(json(StatusCode::OK, &update))
}

/// `POST /update/install`: the console's Install now, which ends the
/// caller's own stream, refused while another device streams. One that
/// starts streaming later holds the install back until it ends.
pub async fn install(
    State(h): State<Shared>,
    Extension(c): Extension<Connection>,
    request: Request,
) -> Reply {
    let client = match caller(&h, &c) {
        Ok(client) => client,
        Err(failure) => return Err(refused("update", &c, None, failure)),
    };
    let checked = async {
        require_host_control(&client)?;
        expect_empty_body(request).await
    };
    if let Err(failure) = checked.await {
        return Err(refused("update", &c, Some(&client), failure));
    }
    tokio::task::spawn_blocking(move || match install_blocking(&h, &client) {
        Ok(update) => Ok(json(StatusCode::ACCEPTED, &update)),
        Err(failure) => Err(refused("update", &c, Some(&client), failure)),
    })
    .await
    .map_err(|error| Failure::internal("install the update", error))?
}
fn install_blocking(h: &Shared, client: &Client) -> Result<Update, Failure> {
    // The gate keeps a launch from starting between the check and the queue.
    let _gate = h.launch_transition.try_lock().map_err(|_| {
        Failure::new(
            ErrorCode::HostInUse,
            power_action::Busy::Transition.message(),
        )
    })?;
    if let Some(busy) = streaming(h, Some(&client.uuid)) {
        return Err(Failure::new(ErrorCode::HostInUse, busy.message()));
    }
    let queued = crate::updater::queue(h, false, true, Some(&client.uuid))
        .map_err(|error| Failure::new(ErrorCode::UpdateUnavailable, format!("{error:#}.")))?;
    let update = update_of(h);
    let version =
        h.updates.lock().unwrap_or_else(PoisonError::into_inner)["queued_version"].clone();
    tracing::warn!(client = %client.name, %version, queued, "native API: update install requested");
    // A second tap while it already installs now raises no second event.
    if !queued {
        return Ok(update);
    }
    h.webhook(
        "update.started",
        serde_json::json!({"device": h.webhook_device(&client.uuid), "version": version}),
    );
    Ok(update)
}
