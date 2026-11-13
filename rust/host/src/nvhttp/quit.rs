//! `/cancel` and what else ends streams: who may close the running app, and what a quit means for a co-op player.

use super::*;

/// Whether `client` may not terminate the running app: remote sessions are active and the client
/// does not own the game. The `cancel` request answers this with a 403.
pub(crate) fn terminate_refused(h: &Shared, client: &str) -> bool {
    let Some(game) = remote_game(h) else {
        return false;
    };
    let remote_active = !h.monitors.lock().unwrap().is_empty()
        || h.sessions
            .lock()
            .unwrap()
            .active
            .values()
            .any(|s| !s.stopping() && s.launch.role != Role::Stream);
    (remote_active
        && butterpollo_core::state::device_uuid(&game.owner)
            != butterpollo_core::state::device_uuid(client))
        || h.claimed_by_other(client)
}
/// Unplug the controllers that streams which ended still park and wait until XInput shows them
/// gone, so a new Nucleus Co-op session's players take the first free slots (#56). A pending
/// launch's controllers stay: Nightjar prepares them before its stream takes them. So do a
/// running stream's, which its control stream takes only at its first input (or never, when
/// its injector failed); a superseded or ended stream is stopping or gone. Nothing to do, and
/// XInput isn't read, when nothing is parked.
pub(super) fn release_parked_pads(h: &Shared) -> Result<()> {
    use butterpollo_windows::nucleus::pads;
    let pending: std::collections::BTreeSet<String> = {
        let sessions = h.sessions.lock().unwrap();
        let running = sessions.active.values().filter(|s| !s.stopping());
        sessions
            .pending
            .values()
            .chain(running.map(|s| &s.launch))
            .map(crate::controller_sessions::key)
            .collect()
    };
    if !h.controller_sessions.parked(&pending) {
        return Ok(());
    }
    pads::released(&mut pads::System::open()?, || {
        h.controller_sessions
            .release_parked(pads::RETIRE_TIMEOUT, &pending)
    })
}
/// A Nucleus Co-op player's quit (Moonlight's cancel, WebRTC's stop-host-session): its own
/// streams end, never the other players' or the host's app, and its slot is kept for the grace
/// period; its private background session ends with it unless only `disconnect`ing. A client
/// whose background stream already ended (Moonlight and Nightjar end the stream before they quit)
/// still ends that session with a `vacated` quit when it streams nothing else and owns no host
/// app, which its quit would then mean (#56); WebRTC's stop and switch, which may return to that
/// game, never do. `None` when `client` streams nothing of Nucleus's and ends no such session,
/// else the ids of the streams it stopped. The caller holds `launch_transition`.
fn quit_nucleus_player(
    h: &Shared,
    client: &str,
    disconnect: bool,
    vacated: bool,
) -> Option<Vec<String>> {
    let (canceled, ids, seats) = {
        let mut sessions = h.sessions.lock().unwrap();
        let mine = |l: &Launch| l.role == Role::Stream && l.client.uuid == client;
        let streams: Vec<_> = sessions
            .pending
            .values()
            .chain(sessions.active.values().map(|s| &s.launch))
            .filter(|l| mine(l))
            .collect();
        let seats: Vec<_> = streams.iter().filter_map(|l| l.nucleus).collect();
        if seats.is_empty() {
            let streams_other = !streams.is_empty();
            drop(sessions);
            return (vacated && !disconnect && !streams_other)
                .then(|| quit_vacated_background(h, client))
                .flatten();
        }
        let ids: Vec<_> = sessions
            .active
            .values()
            .filter(|s| mine(&s.launch))
            .map(|s| s.launch.id.clone())
            .collect();
        (sessions.stop_role(Role::Stream, Some(client)), ids, seats)
    };
    drop(canceled);
    let listed = h.nucleus.sessions();
    for seat in seats {
        let background = listed.iter().any(|listed| {
            listed.session.id == seat.session
                && listed.session.kind == butterpollo_core::nucleus::coop::Kind::Background
        });
        if background && !disconnect {
            tracing::info!(
                client,
                session = seat.session,
                "a quit ends the device's background stream"
            );
            h.nucleus.end(seat.session);
        }
    }
    Some(ids)
}
/// The quit of a client that streams nothing: it ends the background stream it stepped away
/// from, unless it owns the host's app, which Moonlight shows it as running and so quits. No
/// stream is left to stop.
fn quit_vacated_background(h: &Shared, client: &str) -> Option<Vec<String>> {
    if remote_game(h).is_some_and(|game| game.owner == client) {
        return None;
    }
    let session = h.nucleus.background_of(client)?;
    tracing::info!(
        client,
        session,
        "a quit ends the device's background stream, whose stream had already ended"
    );
    h.nucleus.end(session);
    Some(Vec::new())
}
/// The native End session, Close game (or Disconnect, `disconnect`) of a Nucleus Co-op player,
/// who owns no host session: [`quit_nucleus_player`], waiting up to five seconds for its streams.
/// False when it did nothing; `Busy` while a launch holds the launch lock, as the session's own
/// actions answer.
pub(crate) fn quit_nucleus_stream(
    h: &Shared,
    client: &str,
    disconnect: bool,
) -> Result<bool, butterpollo_core::session_state::Refusal> {
    let _transition = h.transition()?;
    let Some(ids) = quit_nucleus_player(h, client, disconnect, true) else {
        return Ok(false);
    };
    if !wait_for_teardown(&h.sessions, &ids, Duration::from_secs(5)) {
        tracing::warn!(client, "a co-op player's streams did not stop within 5 s");
    }
    Ok(true)
}
/// What `stop_streams` did.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum StopStreams {
    Stopped,
    /// The client may not terminate the app (see `terminate_refused`); nothing was stopped.
    Refused,
    /// A stream worker outlived the wait.
    TimedOut,
}
/// Stop every stream, and the running app with them when `terminate_app` names the client asking,
/// as the `cancel` request does, for WebRTC's stop-host-session and switch-session. Remote Monitor
/// and Remote Input sessions stay, and are not waited for.
pub(crate) fn stop_streams(h: &Shared, terminate_app: Option<&str>) -> StopStreams {
    let _transition = h.launch_transition.lock().unwrap();
    if let Some(ids) = terminate_app.and_then(|client| quit_nucleus_player(h, client, false, false))
    {
        return if wait_for_teardown(&h.sessions, &ids, Duration::from_secs(10)) {
            StopStreams::Stopped
        } else {
            StopStreams::TimedOut
        };
    }
    if terminate_app.is_some_and(|client| terminate_refused(h, client)) {
        return StopStreams::Refused;
    }
    let (canceled, ids) = {
        let mut sessions = h.sessions.lock().unwrap();
        // The streams `stop_role` ends, and the teardowns already under way.
        let ids: Vec<String> = sessions
            .active
            .values()
            .filter(|s| s.launch.role == Role::Stream)
            .map(|s| s.launch.id.clone())
            .chain(sessions.teardown.keys().cloned())
            .collect();
        (sessions.stop_role(Role::Stream, None), ids)
    };
    drop(canceled);
    if let Some(client) = terminate_app {
        h.stop_app(AppStop::WebRtc(client));
    }
    if wait_for_teardown(&h.sessions, &ids, Duration::from_secs(10)) {
        StopStreams::Stopped
    } else {
        StopStreams::TimedOut
    }
}

pub(super) async fn cancel(
    State(h): State<Shared>,
    Extension(connection): Extension<Connection>,
) -> Response {
    // Stopping waits for the app to exit and runs its undo commands.
    blocking(move || cancel_app(h, connection)).await
}
fn cancel_app(h: Shared, connection: Connection) -> Response {
    let client = match authenticated(&h, &connection, 1 << 26) {
        Ok(client) => client,
        Err(e) => return xml(401, &[], Some(e.to_string())),
    };
    let _transition = h.launch_transition.lock().unwrap();
    if quit_nucleus_player(&h, &client.uuid, false, true).is_some() {
        return xml(200, &[("cancel", "1".into())], None);
    }
    if remote_game(&h).is_none() {
        return xml(200, &[("cancel", "1".into())], None);
    }
    if terminate_refused(&h, &client.uuid) {
        return xml(
            403,
            &[("cancel", "0".into())],
            Some("Only the game owner may cancel while remote sessions are active".into()),
        );
    }
    let canceled = h.sessions.lock().unwrap().stop_role(Role::Stream, None);
    drop(canceled);
    h.stop_app(AppStop::Cancel(&client.uuid));
    xml(200, &[("cancel", "1".into())], None)
}
