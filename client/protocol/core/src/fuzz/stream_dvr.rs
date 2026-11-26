//! Property tests for Stream DVR (0x554A): requests, host states and clip results.

use super::*;
use crate::stream_dvr::{
    ClipResult, HostState, MAX_CLIP_ID_BYTES, Message, Op, Outcome, Reason, Request, State,
};

fn reason() -> impl Strategy<Value = Reason> {
    any::<u8>().prop_map(Reason::from_u8)
}

fn seeds() -> impl Strategy<Value = Vec<u8>> {
    let request = (any::<u32>(), 1u8..=4, any::<u16>()).prop_map(|(request_id, op, secs)| {
        Message::Request(Request {
            request_id,
            op: Op::from_u8(op).unwrap(),
            duration_secs: secs,
        })
    });
    let state = (
        any::<u32>(),
        0u8..=5,
        reason(),
        (any::<u16>(), any::<u16>(), any::<u32>()),
        (any::<u64>(), any::<u64>()),
    )
        .prop_map(
            |(request_id, state, reason, (buffered, capacity, rec), (used, limit))| {
                Message::State(HostState {
                    request_id,
                    state: State::from_u8(state).unwrap(),
                    reason,
                    ring_buffered_secs: buffered,
                    ring_capacity_secs: capacity,
                    recording_elapsed_secs: rec,
                    quota_used_bytes: used,
                    quota_limit_bytes: limit,
                })
            },
        );
    let clip = (
        any::<u32>(),
        0u8..=2,
        reason(),
        any::<u16>(),
        any::<u64>(),
        "[A-Za-z0-9_-]{0,64}",
    )
        .prop_map(|(request_id, outcome, reason, secs, bytes, id)| {
            Message::Clip(ClipResult {
                request_id,
                outcome: Outcome::from_u8(outcome).unwrap(),
                reason,
                duration_secs: secs,
                file_bytes: bytes,
                clip_id: id.chars().take(MAX_CLIP_ID_BYTES).collect::<String>(),
            })
        });
    prop_oneof![request, state, clip].prop_map(|m| m.encode().unwrap_or_default())
}

round_trip!(stream_dvr, seeds(), |b| Message::decode(b), |v| v.encode());
