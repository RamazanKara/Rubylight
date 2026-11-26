//! Stream DVR on the control stream (0x554A, `docs/control-messages.md`): a device's request
//! reaches the worker, and the worker's state and clip results go back to the device.
//!
//! Only a device that sent a request hears the worker, so an older Nightjar or a Moonlight
//! client never gets a 0x554A frame. A query is answered for anyone; saving and recording need
//! the owner's stream and the launch permission. A stream that cannot be armed (the feature is
//! off, PyroWave, passthrough sound) is answered with its reason and never armed.
use super::{Command, Reply};
use crate::state::{Session, Shared as HostShared};
use crate::stream::control::{ControlPeer, ControlSocket};
use rubylight_protocol::stream_dvr::{
    ClipResult, HostState, Message, Op, Outcome, Reason, Request, State,
};
use rusty_enet::{Host, Packet, PacketKind, PeerID};
use std::net::UdpSocket;
use std::sync::atomic::Ordering;
use std::sync::{Arc, PoisonError};

/// Handles one 0x554A frame from `s`'s device.
pub(crate) fn handle_control(
    h: &HostShared,
    control_wake: &Arc<UdpSocket>,
    s: &Arc<Session>,
    payload: &[u8],
    encrypted: bool,
) {
    let Some(Message::Request(request)) = Message::decode(payload) else {
        tracing::debug!(len = payload.len(), "invalid stream dvr request");
        return;
    };
    let wake = control_wake.clone();
    s.dvr.set_wake(move || {
        let _ = wake.send(&[1]);
    });
    let mutates = request.op != Op::QueryState;
    if mutates && (!encrypted || !may_save(s)) {
        return refuse(s, &request, State::Off, Reason::PermissionDenied);
    }
    let config = h
        .config
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    let shared = h
        .dvr
        .active_for(&s.launch.id)
        .or_else(|| super::start(h, s, &config));
    let Some(shared) = shared else {
        let (state, reason) = match super::eligibility(s) {
            _ if !config.dvr_enabled() => (State::Off, Reason::Disabled),
            Err(reason) => (unsupported(reason), reason),
            // Another stream has the replay buffer.
            Ok(()) if h.dvr.active().is_some() => (State::Off, Reason::Busy),
            // Turned off and on again during this stream: it comes back with the next one.
            Ok(()) => (State::Off, Reason::Unknown),
        };
        return refuse(s, &request, state, reason);
    };
    shared.client_speaks.store(true, Ordering::Release);
    let reply = Reply::Control(request.request_id);
    shared.push(match request.op {
        Op::QueryState => Command::Query {
            request_id: request.request_id,
        },
        Op::SaveClip => Command::SaveClip {
            secs: request.duration_secs,
            reply,
        },
        Op::StartRecord => Command::StartRecord { reply },
        Op::StopRecord => Command::StopRecord { reply },
    });
}

/// Whether `s`'s device may save clips: the owner's plain stream, with the launch permission.
fn may_save(s: &Session) -> bool {
    s.launch.guest().is_none()
        && s.launch.plain_stream()
        && s.launch
            .client
            .allows(butterpollo_session::state::LAUNCH_APPS)
}

fn unsupported(reason: Reason) -> State {
    match reason {
        Reason::Pyrowave | Reason::AudioCodecUnsupported => State::Unsupported,
        _ => State::Off,
    }
}

/// Answers a request the worker never sees: a state for a query, a refusal otherwise.
fn refuse(s: &Session, request: &Request, state: State, reason: Reason) {
    let message = match request.op {
        Op::QueryState => Message::State(HostState {
            request_id: request.request_id,
            state,
            reason,
            ring_buffered_secs: 0,
            ring_capacity_secs: 0,
            recording_elapsed_secs: 0,
            quota_used_bytes: 0,
            quota_limit_bytes: 0,
        }),
        _ => Message::Clip(ClipResult {
            request_id: request.request_id,
            outcome: Outcome::Refused,
            reason,
            duration_secs: 0,
            file_bytes: 0,
            clip_id: String::new(),
        }),
    };
    tracing::debug!(op = ?request.op, reason = reason.as_str(), "stream dvr request answered without a buffer");
    if let Some(frame) = message.encode() {
        s.dvr.post(frame);
    }
}

/// Sends `s`'s queued 0x554A frames to its device, reliable and in order.
pub(crate) fn send_outgoing(
    host: &mut Host<ControlSocket>,
    peer_id: PeerID,
    p: &mut ControlPeer,
    s: &Arc<Session>,
) {
    if !s.dvr.has_outgoing() {
        return;
    }
    for frame in s.dvr.take_outgoing() {
        match p.encrypt(
            s,
            rubylight_protocol::stream_dvr::DVR_STATE_MESSAGE_TYPE,
            &frame,
        ) {
            Ok(message) => {
                if let Err(error) = host
                    .peer_mut(peer_id)
                    .send(0, &Packet::new(message, PacketKind::Reliable))
                {
                    tracing::debug!(%error, "stream dvr frame not sent");
                }
            }
            Err(error) => {
                tracing::debug!(error = %format!("{error:#}"), "stream dvr frame not encrypted")
            }
        }
    }
}
