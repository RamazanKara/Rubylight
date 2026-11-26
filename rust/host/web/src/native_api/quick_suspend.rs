//! `GET /session/suspend`, `POST /session/suspend` and `POST /session/resume`
//! (`docs/native-api.md`, "Quick Suspend"): the running game's paused state,
//! and pausing or resuming it. Reading needs View or Launch; acting needs
//! Launch or Host control, and the session's ownership unless the device has
//! Host control (`butterpollo_stream::quick_suspend`). A refusal answers 200
//! with `state` `refused`, its stable `reason` and a plain-English `message`,
//! as the control message `0x554B` does.
use super::{
    Failure, Reply, caller, json, read_body, require, require_host_control, require_view_or_launch,
};
use axum::{
    Extension,
    extract::{Request, State},
    http::StatusCode,
};
use butterpollo_session::{native::LAUNCH, state::Client};
use butterpollo_stream::{
    quick_suspend::{self, Status, Who},
    state::Shared,
    tls::Connection,
};
use rubylight_native_api::{error::ErrorCode, quick_suspend::QuickSuspendRequest};

/// `GET /session/suspend`.
pub async fn read(State(h): State<Shared>, Extension(c): Extension<Connection>) -> Reply {
    let client = caller(&h, &c)?;
    require_view_or_launch(&client)?;
    answer(h, client, |h, _| quick_suspend::status(h)).await
}

/// `POST /session/suspend` with `{}`.
pub async fn suspend(
    State(h): State<Shared>,
    Extension(c): Extension<Connection>,
    request: Request,
) -> Reply {
    let client = acting(&h, &c, request).await?;
    answer(h, client, |h, client| {
        quick_suspend::suspend(h, Who::Device(client))
    })
    .await
}

/// `POST /session/resume` with `{}`.
pub async fn resume(
    State(h): State<Shared>,
    Extension(c): Extension<Connection>,
    request: Request,
) -> Reply {
    let client = acting(&h, &c, request).await?;
    answer(h, client, |h, client| {
        quick_suspend::resume(h, Who::Device(client))
    })
    .await
}

/// The caller of a POST, with Launch or Host control, and its empty body.
async fn acting(h: &Shared, c: &Connection, request: Request) -> Result<Client, Failure> {
    let client = caller(h, c)?;
    require(&client, LAUNCH, "Launch applications").or_else(|_| require_host_control(&client))?;
    let bytes = read_body(request).await?;
    if !bytes.iter().all(u8::is_ascii_whitespace) {
        serde_json::from_slice::<QuickSuspendRequest>(&bytes).map_err(|error| {
            Failure::new(
                ErrorCode::InvalidRequest,
                format!("The body is not a Quick Suspend request: {error}."),
            )
        })?;
    }
    Ok(client)
}

/// Runs `work` off the async task: a suspend lists processes and writes to disk.
async fn answer(
    h: Shared,
    client: Client,
    work: impl FnOnce(&Shared, &Client) -> Status + Send + 'static,
) -> Reply {
    let status = tokio::task::spawn_blocking(move || work(&h, &client))
        .await
        .map_err(|error| Failure::internal("run Quick Suspend", error))?;
    Ok(json(StatusCode::OK, &status.native()))
}
