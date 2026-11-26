//! Quick Suspend's control message (0x554B) on the control stream: a device's
//! request runs on a worker thread of its own (`crate::quick_suspend`), never
//! on the control loop, and its answer, or a change another device or the
//! console made, goes back reliable to a device that asked once.

use super::super::*;
use super::peer::ControlPeer;
use super::socket::ControlSocket;
use rubylight_protocol::quick_suspend::{
    HostState, Message, Op, QUICK_SUSPEND_MESSAGE_TYPE, STATE_BYTES,
};

/// Answers a worker left for the peer, each with the state's revision after it.
type Answers = Arc<Mutex<Vec<(HostState, u64)>>>;

/// One peer's Quick Suspend: whether its device speaks 0x554B, the state
/// revision it last learnt, and the answers waiting for it.
#[derive(Default)]
pub(in crate::stream) struct PeerSuspend {
    active: bool,
    sent: u64,
    answers: Answers,
}

/// Run `work` off the control loop; its state goes to the peer as `request_id`'s answer.
fn spawn(
    h: &Shared,
    control_wake: &Arc<UdpSocket>,
    answers: &Answers,
    request_id: u32,
    work: impl FnOnce(&Shared) -> crate::quick_suspend::Status + Send + 'static,
) {
    let (h, wake, answers) = (h.clone(), control_wake.clone(), answers.clone());
    let started = std::thread::Builder::new()
        .name("quick-suspend".into())
        .spawn(move || {
            let status = work(&h);
            let revision = h.quick_suspend.revision();
            answers
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((status.wire(request_id), revision));
            let _ = wake.send(&[1]);
        });
    if let Err(error) = started {
        tracing::warn!(%error, "Quick Suspend: the request's worker did not start");
    }
}

/// A device's request (0x554B). Only an encrypted one, on a stream the host
/// offered Quick Suspend to.
pub(super) fn request(
    h: &Shared,
    control_wake: &Arc<UdpSocket>,
    p: &mut ControlPeer,
    s: &Arc<Session>,
    payload: &[u8],
    encrypted: bool,
) {
    if !encrypted {
        return;
    }
    let Ok(config) = p.config(h, s) else {
        return;
    };
    if !crate::extensions::quick_suspend(&config) {
        tracing::debug!("unoffered Quick Suspend request");
        return;
    }
    let Some(Message::Request(request)) = Message::decode(payload) else {
        tracing::debug!(len = payload.len(), "invalid Quick Suspend request");
        return;
    };
    if !p.quick_suspend.active {
        p.quick_suspend.active = true;
        p.quick_suspend.sent = h.quick_suspend.revision();
    }
    let client = s.launch.client.clone();
    spawn(
        h,
        control_wake,
        &p.quick_suspend.answers,
        request.request_id,
        move |h| {
            use crate::quick_suspend::{Who, resume, status, suspend};
            match request.op {
                Op::QueryState => status(h),
                Op::Suspend => suspend(h, Who::Device(&client)),
                Op::Resume => resume(h, Who::Device(&client)),
            }
        },
    );
}

/// The answers waiting for this peer, and the state when it changed since the
/// peer last learnt it (read on a worker, which the tracker may keep waiting).
pub(super) fn send(
    h: &Shared,
    control_wake: &Arc<UdpSocket>,
    host: &mut Host<ControlSocket>,
    peer_id: PeerID,
    p: &mut ControlPeer,
    s: &Arc<Session>,
) {
    if !p.quick_suspend.active {
        return;
    }
    let answers = std::mem::take(
        &mut *p
            .quick_suspend
            .answers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    );
    for (state, revision) in answers {
        p.quick_suspend.sent = p.quick_suspend.sent.max(revision);
        let frame: [u8; STATE_BYTES] = state.encode();
        match p.encrypt(s, QUICK_SUSPEND_MESSAGE_TYPE, &frame) {
            Ok(message) => {
                if let Err(e) = host
                    .peer_mut(peer_id)
                    .send(0, &Packet::new(message, PacketKind::Reliable))
                {
                    tracing::warn!(error = %e, "Quick Suspend state not sent");
                }
            }
            Err(e) => {
                tracing::warn!(error = %format!("{e:#}"), "Quick Suspend state not encrypted")
            }
        }
    }
    let revision = h.quick_suspend.revision();
    if revision != p.quick_suspend.sent {
        p.quick_suspend.sent = revision;
        spawn(h, control_wake, &p.quick_suspend.answers, 0, |h| {
            crate::quick_suspend::status(h)
        });
    }
}
