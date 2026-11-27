//! `GET /wake/relay` and `POST /wake/relay/enroll`: the away wake relay (`docs/native-api.md`,
//! "Away wake relay"). The PC hands a device that may launch games the keys it asks a relay
//! with, and a device with Host control on its own network what it needs to be that relay.
use super::{Failure, Reply, caller, json, read_body, require_host_control};
use axum::{
    Extension,
    extract::{Request, State},
    http::StatusCode,
};
use butterpollo_session::wake::relay::{MAX_RELAYS, Refusal};
use butterpollo_stream::{
    state::Shared,
    tls::Connection,
    wake_relay::{self, Refused},
};
use rubylight_native_api::{error::ErrorCode, wake_relay::RelayEnroll};

fn failure(refused: Refused) -> Failure {
    match refused {
        Refused::Permission => Failure::new(
            ErrorCode::PermissionRequired,
            "Turn on Launch applications or Host control for this device in the Rubylight console.",
        ),
        Refused::Off => Failure::new(
            ErrorCode::Forbidden,
            "Away wake relay is off on this PC. Turn it on with Play over the internet in the Rubylight console.",
        ),
        Refused::Relay(Refusal::NotOnLan) => Failure::new(
            ErrorCode::Forbidden,
            "Only a device on this PC's own network can be its wake relay.",
        ),
        Refused::Relay(Refusal::TooManyRelays) => Failure::new(
            ErrorCode::InvalidRequest,
            format!(
                "This PC has {MAX_RELAYS} wake relays. Remove one in the Rubylight console first."
            ),
        ),
        Refused::Relay(Refusal::UnknownRelay) => Failure::new(
            ErrorCode::NotFound,
            "That wake relay is not enrolled on this PC.",
        ),
        Refused::Failed(error) => Failure::internal("update its wake relays", error),
    }
}

/// `GET /wake/relay` (Launch applications or Host control).
pub async fn read(State(h): State<Shared>, Extension(c): Extension<Connection>) -> Reply {
    let client = caller(&h, &c)?;
    let info = tokio::task::spawn_blocking(move || wake_relay::info(&h, &client))
        .await
        .map_err(|error| Failure::internal("read its wake relays", error))?
        .map_err(failure)?;
    Ok(json(StatusCode::OK, &info))
}

/// `POST /wake/relay/enroll` (Host control).
pub async fn enroll(
    State(h): State<Shared>,
    Extension(c): Extension<Connection>,
    request: Request,
) -> Reply {
    let client = caller(&h, &c)?;
    require_host_control(&client)?;
    let body = read_body(request).await?;
    let action: RelayEnroll = serde_json::from_slice(&body).map_err(|error| {
        Failure::new(ErrorCode::InvalidRequest, format!("Invalid body: {error}."))
    })?;
    let peer = c.peer;
    let enrolled =
        tokio::task::spawn_blocking(move || wake_relay::enroll(&h, &client, peer, action))
            .await
            .map_err(|error| Failure::internal("update its wake relays", error))?
            .map_err(failure)?;
    Ok(json(StatusCode::OK, &enrolled))
}
