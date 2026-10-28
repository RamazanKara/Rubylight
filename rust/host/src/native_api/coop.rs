//! `GET /coop` and `POST /coop/join`: the games this host offers for Nucleus
//! Co-op and the co-op sessions a device can join (the wire types are
//! `butterpollo_core::native_api::coop`). Both answer 503 while this host
//! cannot run Nucleus Co-op sessions, when `coop` is not a feature.
use super::{Failure, Reply, caller, json, permission_required, read_body, require};
use crate::{nucleus::JoinError, state::Shared, tls::Connection};
use axum::{
    Extension,
    extract::{Request, State},
    http::StatusCode,
};
use butterpollo_core::{
    native::{LAUNCH, LIST},
    native_api::{coop as wire, error::ErrorCode},
    nucleus::coop::{End, Kind, Phase, Refused},
    state::Client,
};
use std::sync::PoisonError;

fn unavailable(h: &Shared) -> Result<(), Failure> {
    match h.nucleus.unavailable() {
        Some(why) => Err(Failure::new(ErrorCode::TemporarilyUnavailable, why)),
        None => Ok(()),
    }
}

pub async fn list(State(h): State<Shared>, Extension(c): Extension<Connection>) -> Reply {
    let client = caller(&h, &c)?;
    unavailable(&h)?;
    require(&client, LIST, "List applications")?;
    // The games come from Nucleus Co-op's handler files.
    let coop = tokio::task::spawn_blocking(move || listing(&h, &client))
        .await
        .map_err(|error| Failure::internal("list the co-op sessions", error))?;
    Ok(json(StatusCode::OK, &coop))
}

/// The co-op games and sessions as `client` sees them. A background session
/// is private and never listed; who plays a co-op session is named only to a
/// device with Join co-op or a slot in it.
fn listing(h: &Shared, client: &Client) -> wire::Coop {
    let apps = h
        .apps
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    let configured =
        crate::nucleus::configured(&h.config.read().unwrap_or_else(PoisonError::into_inner));
    let games = h
        .nucleus
        .entries(&apps, &configured)
        .into_iter()
        .map(|coop| wire::Game {
            app_id: coop.id(),
            title: coop.entry.name,
            game: coop.handler.guid,
            max_players: coop.handler.max_players,
        })
        .collect();
    let sessions = h
        .nucleus
        .sessions()
        .into_iter()
        .filter(|listed| listed.session.kind == Kind::Coop)
        .map(|listed| session_of(&listed, client))
        .collect();
    wire::Coop {
        available: true,
        reason: None,
        games,
        sessions,
    }
}

fn session_of(listed: &crate::nucleus::Listed, client: &Client) -> wire::Session {
    let session = &listed.session;
    let (phase, end) = match &session.phase {
        Phase::Starting => (wire::Phase::Starting, None),
        Phase::Running => (wire::Phase::Running, None),
        Phase::Ended(end) => (wire::Phase::Ended, Some(end_of(end))),
    };
    let players: Vec<_> = session.players();
    let free_slots = players.iter().filter(|p| p.device.is_none()).count();
    // Whether a join would succeed now, asked of a copy.
    let refusal = session
        .clone()
        .join(&client.uuid, &client.name, client.allows_coop_join())
        .err();
    let names = client.allows_coop_join() || session.slot_of(&client.uuid).is_some();
    wire::Session {
        id: session.id,
        app_id: listed.app_id,
        title: session.title.clone(),
        game: session.game.clone(),
        phase,
        end,
        players: players
            .into_iter()
            .map(|player| wire::Player {
                slot: player.slot as u32,
                you: player.device.as_deref() == Some(client.uuid.as_str()),
                name: player.name.filter(|_| names),
                away: player.away,
            })
            .collect(),
        free_slots: free_slots as u32,
        joinable: refusal.is_none(),
        refusal: refusal.map(|refused| refused.to_string()),
    }
}

pub(crate) fn end_of(end: &End) -> wire::End {
    let (reason, detail) = match end {
        End::Ended => (wire::EndReason::Ended, None),
        End::Empty => (wire::EndReason::Empty, None),
        End::Crashed(detail) => (wire::EndReason::Crashed, Some(detail.clone())),
        End::Failed(detail) => (wire::EndReason::Failed, Some(detail.clone())),
    };
    wire::End { reason, detail }
}

/// Seats the device, or returns it to its own slot, and holds the slot for
/// its launch of the co-op entry (`app_id`), which streams it.
pub async fn join(
    State(h): State<Shared>,
    Extension(c): Extension<Connection>,
    request: Request,
) -> Reply {
    let client = caller(&h, &c)?;
    unavailable(&h)?;
    require(&client, LAUNCH, "Launch applications")?;
    let body = read_body(request).await?;
    let wire::JoinRequest { session } = serde_json::from_slice(&body).map_err(|error| {
        Failure::new(
            ErrorCode::InvalidRequest,
            format!("The body is not a co-op join: {error}."),
        )
    })?;
    let joined = tokio::task::spawn_blocking(move || h.nucleus.join(session, &client))
        .await
        .map_err(|error| Failure::internal("join the co-op session", error))?;
    match joined {
        Ok((slot, app_id)) => Ok(json(
            StatusCode::OK,
            &wire::Joined {
                session,
                slot: slot as u32,
                app_id,
            },
        )),
        Err(JoinError::NoSession) => Err(Failure::new(
            ErrorCode::NoSession,
            "There is no such co-op session.",
        )),
        Err(JoinError::Refused(Refused::NotPermitted)) => Err(permission_required("Join co-op")),
        Err(JoinError::Refused(refused)) => {
            Err(Failure::new(ErrorCode::CoopRefused, refused.to_string()))
        }
    }
}
