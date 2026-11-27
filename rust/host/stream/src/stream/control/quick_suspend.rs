//! Quick Suspend's control message (0x554B) on the control stream: a device's
//! request runs on a worker thread (`crate::quick_suspend`), never on the
//! control loop, and its answer, or a change another device or the console
//! made, goes back reliable to a device that asked once.
//!
//! A peer has at most one worker at a time. What it asks meanwhile waits,
//! folded into one next request: queries join it, the latest Suspend or Resume
//! decides what it does, and every request it folded in is answered with the
//! state it leaves. A device that repeats its query while a suspend holds the
//! tracker so waits on one thread, not one each. However many queries wait, a
//! Suspend or Resume is always answered.

use super::super::*;
use super::peer::ControlPeer;
use super::socket::ControlSocket;
use rubylight_protocol::quick_suspend::{
    HostState, Message, Op, QUICK_SUSPEND_MESSAGE_TYPE, STATE_BYTES,
};
use std::sync::atomic::{AtomicBool, Ordering};

/// Answers a worker left for the peer, each with the state's revision after it.
type Answers = Arc<Mutex<Vec<(HostState, u64)>>>;

/// The most requests one waiting request answers. A query beyond it learns
/// the state from the next change it is sent; an action takes a query's place.
const MAX_WAITING: usize = 16;

/// Requests that arrived while the peer's worker ran: the operation to run
/// next and the request ids it answers (0: a state change to send), each with
/// the operation it asked for.
type Waiting = Option<(Op, Vec<(u32, Op)>)>;

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
    /// What arrived meanwhile.
    waiting: Waiting,
}

/// Fold request `id` for `op` into what waits: an action replaces a waiting
/// query or the earlier action, a query only adds its id.
///
/// Full, the request takes the place of a state change to send, which any
/// answer carries. An action, which the next request runs, is never left
/// unanswered: failing that it takes the oldest query's place, or else the
/// oldest action's, which it overrides. A query finds no place.
fn coalesce(waiting: &mut Waiting, op: Op, id: u32) {
    let (next, ids) = waiting.get_or_insert_with(|| (Op::QueryState, Vec::new()));
    let action = op != Op::QueryState;
    if action {
        *next = op;
    }
    if let Some((_, asked)) = ids.iter_mut().find(|(known, _)| *known == id) {
        if action {
            *asked = op;
        }
        return;
    }
    if ids.len() >= MAX_WAITING {
        let place = ids.iter().position(|&(known, _)| known == 0).or_else(|| {
            action.then(|| {
                ids.iter()
                    .position(|&(_, asked)| asked == Op::QueryState)
                    .unwrap_or(0)
            })
        });
        let Some(place) = place else {
            return;
        };
        ids.remove(place);
    }
    ids.push((id, op));
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
        && let Some((op, waited)) = peer.waiting.take()
    {
        let ids = waited.into_iter().map(|(id, _)| id).collect();
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

    /// The operation waiting and the ids it answers.
    fn waits(waiting: &Waiting) -> (Op, Vec<u32>) {
        let (op, ids) = waiting.as_ref().unwrap();
        (*op, ids.iter().map(|&(id, _)| id).collect())
    }

    #[test]
    fn requests_that_arrive_while_a_worker_runs_wait_as_one() {
        let mut waiting = None;
        // Repeated queries: one query, each id answered once.
        for id in [1, 2, 2, 3] {
            coalesce(&mut waiting, Op::QueryState, id);
        }
        assert_eq!(waits(&waiting), (Op::QueryState, vec![1, 2, 3]));
        // An action decides what the next request does; the latest one wins,
        // and a query after it does not undo it.
        coalesce(&mut waiting, Op::Suspend, 4);
        coalesce(&mut waiting, Op::Resume, 5);
        coalesce(&mut waiting, Op::QueryState, 6);
        assert_eq!(waits(&waiting), (Op::Resume, vec![1, 2, 3, 4, 5, 6]));
        // However often a device asks, one request answers a bounded number.
        for id in 100..200 {
            coalesce(&mut waiting, Op::QueryState, id);
        }
        assert_eq!(waits(&waiting).1.len(), MAX_WAITING);
        // A state change to send (id 0) folds in once.
        let mut pushes = None;
        coalesce(&mut pushes, Op::QueryState, 0);
        coalesce(&mut pushes, Op::QueryState, 0);
        assert_eq!(waits(&pushes), (Op::QueryState, vec![0]));
        // Sixteen queries while a suspend runs, then Resume: it runs next and
        // its id is answered, in the oldest query's place.
        let mut waiting = None;
        for id in 1..=MAX_WAITING as u32 {
            coalesce(&mut waiting, Op::QueryState, id);
        }
        coalesce(&mut waiting, Op::Resume, 99);
        let (op, ids) = waits(&waiting);
        assert_eq!(op, Op::Resume);
        assert_eq!(ids.len(), MAX_WAITING);
        assert!(!ids.contains(&1));
        assert_eq!(ids.last(), Some(&99));
    }

    #[test]
    fn a_full_wait_gives_up_a_state_push_first_and_an_action_last() {
        // A state change to send gives its place first, to a query too.
        let mut waiting = None;
        coalesce(&mut waiting, Op::QueryState, 0);
        for id in 1..MAX_WAITING as u32 {
            coalesce(&mut waiting, Op::QueryState, id);
        }
        coalesce(&mut waiting, Op::QueryState, 50);
        let (_, ids) = waits(&waiting);
        assert!(!ids.contains(&0) && ids.contains(&1) && ids.contains(&50));
        // Only actions waiting: a query finds no place, and the newest action
        // takes the oldest's, which it overrides.
        let mut waiting = None;
        for id in 1..=MAX_WAITING as u32 {
            coalesce(&mut waiting, Op::Suspend, id);
        }
        coalesce(&mut waiting, Op::QueryState, 70);
        coalesce(&mut waiting, Op::Resume, 71);
        let (op, ids) = waits(&waiting);
        assert_eq!(op, Op::Resume);
        assert!(!ids.contains(&1) && !ids.contains(&70) && ids.contains(&71));
        assert_eq!(ids.len(), MAX_WAITING);
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
