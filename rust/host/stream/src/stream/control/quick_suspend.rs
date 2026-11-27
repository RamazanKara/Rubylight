//! Quick Suspend's control message (0x554B) on the control stream: a device's
//! request runs on a worker thread (`crate::quick_suspend`), never on the
//! control loop, and its answer, or a change another device or the console
//! made, goes back reliable to a device that asked once.
//!
//! A peer has at most one worker at a time. What it asks meanwhile waits,
//! folded into one next request: queries join it, the latest Suspend or Resume
//! decides what it does, and every request it folded in is answered with the
//! state it leaves. A device that repeats its query while a suspend holds the
//! tracker so waits on one thread, not one each.

use super::super::*;
use super::peer::ControlPeer;
use super::socket::ControlSocket;
use rubylight_protocol::quick_suspend::{
    HostState, Message, Op, QUICK_SUSPEND_MESSAGE_TYPE, STATE_BYTES,
};
use std::sync::atomic::{AtomicBool, Ordering};

/// Answers a worker left for the peer, each with the state's revision after it.
type Answers = Arc<Mutex<Vec<(HostState, u64)>>>;

/// The most requests one waiting request answers; a device that asks more
/// often learns the state from the next change it is sent.
const MAX_WAITING: usize = 16;

/// One peer's Quick Suspend: whether its device speaks 0x554B, the state
/// revision it last learnt, its worker and what waits for it, and the answers
/// waiting to be sent.
#[derive(Default)]
pub(in crate::stream) struct PeerSuspend {
    active: bool,
    sent: u64,
    answers: Answers,
    /// A worker runs one of this peer's requests now.
    busy: Arc<AtomicBool>,
    /// What arrived meanwhile: the operation to run next and the request ids
    /// (0: a state change to send) it answers.
    waiting: Option<(Op, Vec<u32>)>,
}

/// Fold request `id` for `op` into what waits: an action replaces a waiting
/// query or the earlier action, a query only adds its id.
fn coalesce(waiting: &mut Option<(Op, Vec<u32>)>, op: Op, id: u32) {
    let (next, ids) = waiting.get_or_insert_with(|| (Op::QueryState, Vec::new()));
    if op != Op::QueryState {
        *next = op;
    }
    if ids.len() < MAX_WAITING && !ids.contains(&id) {
        ids.push(id);
    }
}

/// Run `op` for the peer off the control loop, or fold it into what waits
/// while its worker still runs.
fn ask(
    h: &Shared,
    control_wake: &Arc<UdpSocket>,
    peer: &mut PeerSuspend,
    s: &Arc<Session>,
    op: Op,
    id: u32,
) {
    if peer.busy.load(Ordering::Acquire) {
        coalesce(&mut peer.waiting, op, id);
        return;
    }
    run(h, control_wake, peer, s, op, vec![id]);
}

/// Start the peer's worker for `op`; its state answers every id in `ids`.
fn run(
    h: &Shared,
    control_wake: &Arc<UdpSocket>,
    peer: &mut PeerSuspend,
    s: &Arc<Session>,
    op: Op,
    ids: Vec<u32>,
) {
    peer.busy.store(true, Ordering::Release);
    let (h, wake, answers, busy) = (
        h.clone(),
        control_wake.clone(),
        peer.answers.clone(),
        peer.busy.clone(),
    );
    let client = s.launch.client.clone();
    let started = std::thread::Builder::new()
        .name("quick-suspend".into())
        .spawn(move || {
            use crate::quick_suspend::{Who, resume, status, suspend};
            let status = match op {
                Op::QueryState => status(&h),
                Op::Suspend => suspend(&h, Who::Device(&client)),
                Op::Resume => resume(&h, Who::Device(&client)),
            };
            let revision = h.quick_suspend.revision();
            answers
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .extend(ids.into_iter().map(|id| (status.wire(id), revision)));
            busy.store(false, Ordering::Release);
            let _ = wake.send(&[1]);
        });
    if let Err(error) = started {
        peer.busy.store(false, Ordering::Release);
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
    let Some(Message::Request(request)) = Message::decode(payload) else {
        tracing::debug!(len = payload.len(), "invalid Quick Suspend request");
        return;
    };
    // Turned off while a game is paused, the host still answers and resumes it;
    // otherwise a request only learns the state (Unavailable, turned off), so the
    // device stops offering it instead of waiting for an answer.
    let mut op = request.op;
    if !crate::extensions::quick_suspend(&config)
        && (op == Op::Suspend || !h.quick_suspend.suspended())
    {
        tracing::debug!("Quick Suspend request while it is off");
        op = Op::QueryState;
    }
    if !p.quick_suspend.active {
        p.quick_suspend.active = true;
        p.quick_suspend.sent = h.quick_suspend.revision();
    }
    ask(
        h,
        control_wake,
        &mut p.quick_suspend,
        s,
        op,
        request.request_id,
    );
}

/// The answers waiting for this peer, the request that waited for its worker,
/// and the state when it changed since the peer last learnt it (read on a
/// worker, which the tracker may keep waiting).
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
    let peer = &mut p.quick_suspend;
    if !peer.busy.load(Ordering::Acquire)
        && let Some((op, ids)) = peer.waiting.take()
    {
        run(h, control_wake, peer, s, op, ids);
    }
    let revision = h.quick_suspend.revision();
    if revision != peer.sent {
        peer.sent = revision;
        ask(h, control_wake, peer, s, Op::QueryState, 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_that_arrive_while_a_worker_runs_wait_as_one() {
        let mut waiting = None;
        // Repeated queries: one query, each id answered once.
        for id in [1, 2, 2, 3] {
            coalesce(&mut waiting, Op::QueryState, id);
        }
        assert_eq!(waiting, Some((Op::QueryState, vec![1, 2, 3])));
        // An action decides what the next request does; the latest one wins,
        // and a query after it does not undo it.
        coalesce(&mut waiting, Op::Suspend, 4);
        coalesce(&mut waiting, Op::Resume, 5);
        coalesce(&mut waiting, Op::QueryState, 6);
        assert_eq!(waiting, Some((Op::Resume, vec![1, 2, 3, 4, 5, 6])));
        // However often a device asks, one request answers a bounded number.
        for id in 100..200 {
            coalesce(&mut waiting, Op::QueryState, id);
        }
        assert_eq!(waiting.as_ref().unwrap().1.len(), MAX_WAITING);
        // A state change to send (id 0) folds in once.
        let mut pushes = None;
        coalesce(&mut pushes, Op::QueryState, 0);
        coalesce(&mut pushes, Op::QueryState, 0);
        assert_eq!(pushes, Some((Op::QueryState, vec![0])));
    }

    #[test]
    fn a_peer_never_has_two_workers() {
        // Every worker starts through `run`, which only `ask` (when idle) and
        // `send` (when the last one finished) call.
        let source = include_str!("quick_suspend.rs");
        let body = &source[..source.find("#[cfg(test)]").unwrap()];
        assert_eq!(body.matches(".spawn(move ||").count(), 1);
        assert_eq!(body.matches("run(h, control_wake, ").count(), 2);
        assert!(body.contains("if peer.busy.load(Ordering::Acquire) {\n        coalesce("));
    }
}
