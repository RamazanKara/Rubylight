//! Unit tests for the stream DVR's state, menu entries and toasts (`stream/dvr.rs`).

use super::*;

fn state(state: State, reason: Reason) -> HostState {
    HostState {
        request_id: 0,
        state,
        reason,
        ring_buffered_secs: 60,
        ring_capacity_secs: 60,
        recording_elapsed_secs: 134,
        quota_used_bytes: 0,
        quota_limit_bytes: 20_000_000_000,
    }
}

fn clip(request_id: u32, outcome: Outcome, reason: Reason) -> ClipResult {
    ClipResult { request_id, outcome, reason, duration_secs: 60, file_bytes: 84_000_000, clip_id: "c".into() }
}

#[test]
fn the_entries_follow_the_host_state() {
    let now = Instant::now();
    let mut dvr = Dvr::default();
    // Before the first state both entries work; the host answers with its reason.
    let [save, record] = dvr.entries(now);
    assert!(save.enabled && record.enabled && save.detail.is_empty());
    assert_eq!((save.op, record.op), (Op::SaveClip, Op::StartRecord));

    dvr.state(state(State::Armed, Reason::None));
    let [save, record] = dvr.entries(now);
    assert_eq!((save.label.as_str(), save.detail.as_str()), ("Save clip", "Last 60 s ready"));
    assert_eq!((record.label.as_str(), record.enabled), ("Start recording", true));

    dvr.state(state(State::Recording, Reason::None));
    let [save, record] = dvr.entries(now);
    assert!(save.enabled);
    assert_eq!((record.op, record.label.as_str(), record.detail.as_str()), (Op::StopRecord, "Stop recording", "Recording 02:14"));

    dvr.state(state(State::Unsupported, Reason::Pyrowave));
    let [save, record] = dvr.entries(now);
    assert!(!save.enabled && !record.enabled && save.problem);
    assert!(save.detail.contains("PyroWave"), "{}", save.detail);

    dvr.state(HostState { ring_buffered_secs: 0, ..state(State::Armed, Reason::None) });
    assert!(dvr.entries(now)[0].detail.starts_with("Waiting for the stream"));
}

#[test]
fn a_save_waits_for_its_result_and_toasts_it() {
    let now = Instant::now();
    let mut dvr = Dvr::default();
    dvr.state(state(State::Armed, Reason::None));
    let query = dvr.request(Op::QueryState, now);
    let save = dvr.request(Op::SaveClip, now);
    assert_ne!(query.request_id, save.request_id);
    assert_eq!((save.op, save.duration_secs), (Op::SaveClip, 0));
    let [entry, _] = dvr.entries(now);
    assert!(!entry.enabled);
    assert_eq!(entry.detail, "Saving clip…");
    // A state in between does not end the save; an unanswered one frees it after a while.
    dvr.state(HostState { request_id: save.request_id, ..state(State::Saving, Reason::None) });
    assert!(!dvr.entries(now)[0].enabled);
    assert_eq!(dvr.clip(&clip(save.request_id, Outcome::Saved, Reason::None)), "Clip saved (60 s, 84 MB)");
    dvr.state(state(State::Armed, Reason::None));
    assert!(dvr.entries(now)[0].enabled);
    // A save the host never answers frees the entry after a while.
    dvr.request(Op::SaveClip, now);
    assert!(!dvr.entries(now)[0].enabled);
    assert!(dvr.entries(now + ANSWER).iter().all(|e| e.enabled));

    let refused = dvr.request(Op::SaveClip, now);
    let text = dvr.clip(&clip(refused.request_id, Outcome::Refused, Reason::Disabled));
    assert!(text.starts_with("Instant Replay is turned off on the host PC."), "{text}");
    assert_eq!(dvr.unsent(), "The host PC could not save the clip.");
}

#[test]
fn a_recording_starts_with_a_state_and_stops_with_a_clip() {
    let now = Instant::now();
    let mut dvr = Dvr::default();
    let start = dvr.request(Op::StartRecord, now);
    assert!(!dvr.entries(now)[1].enabled);
    assert_eq!(dvr.state(HostState { request_id: start.request_id, ..state(State::Recording, Reason::None) }), None);
    assert!(dvr.entries(now)[1].enabled);
    let stop = dvr.request(Op::StopRecord, now);
    let done = ClipResult { duration_secs: 134, file_bytes: 1_250_000_000, ..clip(stop.request_id, Outcome::Saved, Reason::None) };
    assert_eq!(dvr.clip(&done), "Recording saved (02:14, 1.2 GB)");

    // The host refuses a start with a clip result, not a state.
    let guest = dvr.request(Op::StartRecord, now);
    let refusal = clip(guest.request_id, Outcome::Refused, Reason::PermissionDenied);
    assert_eq!(dvr.clip(&refusal), reason_text(Reason::PermissionDenied));
    assert!(dvr.entries(now)[1].enabled, "the row works again");

    let refused = dvr.request(Op::StartRecord, now);
    let toast = dvr.state(HostState { request_id: refused.request_id, ..state(State::Error, Reason::DiskFull) });
    assert!(toast.is_some_and(|t| t.starts_with("Not enough free disk space")));
    // The host's own pushes (id 0) never answer a request.
    let pending = dvr.request(Op::StartRecord, now);
    assert_eq!(dvr.state(state(State::Armed, Reason::None)), None);
    assert!(!dvr.entries(now)[1].enabled, "{pending:?} still waits");
}

#[test]
fn every_reason_has_its_own_text_and_sizes_read_plainly() {
    for value in 0..=11 {
        assert!(!reason_text(Reason::from_u8(value)).is_empty());
    }
    assert_eq!(reason_text(Reason::DiskFull), reason_text(Reason::QuotaExceeded));
    assert_eq!(clock(59), "00:59");
    assert_eq!(clock(3734), "1:02:14");
    assert_eq!(size(1), "1 MB");
    assert_eq!(size(84_000_000), "84 MB");
    assert_eq!(size(2_000_000_000), "2.0 GB");
}
