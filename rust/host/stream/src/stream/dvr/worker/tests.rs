//! Unit tests for the Stream DVR worker: state precedence, keyframe cadence, clips-folder
//! creation before quota checks, parameter-set fallback on bare keyframes, and reply delivery
//! when clip-writer spawn fails.

use super::*;
use butterpollo_session::session::Role;
use std::path::PathBuf;
use std::sync::mpsc::sync_channel;

/// libx264, 200x120 High profile SPS and PPS for synthetic H.264 keyframes.
const H264_SPS: &[u8] = &[
    0x67, 0x64, 0x00, 0x0c, 0xac, 0xd9, 0x43, 0x44, 0x79, 0x65, 0xc0, 0x44, 0x00, 0x00, 0x03, 0x00,
    0x04, 0x00, 0x00, 0x03, 0x00, 0xf0, 0x3c, 0x50, 0xa6, 0x58,
];
const H264_PPS: &[u8] = &[0x68, 0xeb, 0xe3, 0xcb, 0x22, 0xc0];

fn h264_idr(pts_us: u64, with_params: bool) -> VideoFrame {
    let mut data = Vec::new();
    let units: Vec<&[u8]> = if with_params {
        vec![H264_SPS, H264_PPS, &[0x65, 0x88, 0x84, 0x21, 0x01]]
    } else {
        vec![&[0x65, 0x88, 0x84, 0x21, 0x02]]
    };
    for unit in units {
        data.extend_from_slice(&[0, 0, 0, 1]);
        data.extend_from_slice(unit);
    }
    VideoFrame::new(pts_us, true, data)
}

struct ScratchDir(PathBuf);

impl ScratchDir {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "rubylight-dvr-worker-{name}-{}-{:08x}",
            std::process::id(),
            rand::random::<u32>()
        ));
        Self(dir)
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn a_running_recording_outranks_a_clip_being_saved_and_a_held_error() {
    let f = crate::state::test_support::Fixture::new();
    let shared = Shared::new(
        "dvr-state",
        "Game".into(),
        60,
        std::env::temp_dir(),
        1 << 30,
    );
    let limits = Limits {
        max_us: 60_000_000,
        max_bytes: 1 << 20,
    };
    let mut worker = Worker::new(f.host.clone(), Weak::new(), shared, limits);
    assert_eq!(worker.state(), (State::Armed, Reason::None));
    worker.error = Some((Reason::QuotaExceeded, Instant::now()));
    worker.pending_recording = Some(("20260101-100000-manual-0001".into(), Instant::now()));
    // The device keeps its Stop: the refused save was answered to its own request.
    assert_eq!(worker.state(), (State::Recording, Reason::None));
    worker.pending_recording = None;
    assert_eq!(worker.state(), (State::Error, Reason::QuotaExceeded));
}

#[test]
fn copies_lost_every_second_ask_no_more_keyframes_than_the_gop_cadence() {
    let f = crate::state::test_support::Fixture::new();
    let s = f.session(f.client(u32::MAX), Role::Stream);
    let shared = Shared::new(
        "dvr-drops",
        "Game".into(),
        60,
        std::env::temp_dir(),
        1 << 30,
    );
    let limits = Limits {
        max_us: 60_000_000,
        max_bytes: 1 << 20,
    };
    let mut worker = Worker::new(f.host.clone(), Arc::downgrade(&s), shared, limits);
    let start = Instant::now();
    worker.cadence.observe_idr(start);
    // A worker that falls behind every second waits for the cadence's next keyframe.
    for second in 1..cadence::GOP_SECS {
        let now = start + Duration::from_secs(second);
        worker.discontinuity(now);
        assert!(!s.dvr.take_keyframe(), "asked after {second} s");
    }
    // Once the last keyframe is a GOP old, one is asked for, and not again a second later.
    let due = start + Duration::from_secs(cadence::GOP_SECS);
    worker.discontinuity(due);
    assert!(s.dvr.take_keyframe());
    worker.discontinuity(due + Duration::from_millis(500));
    assert!(!s.dvr.take_keyframe());
    // A save the player asks for still asks at once.
    worker.cadence.observe_idr(due + Duration::from_secs(2));
    worker.ask_keyframe(due + Duration::from_secs(3));
    assert!(s.dvr.take_keyframe());
}

#[test]
fn save_clip_and_start_recording_create_clips_dir_before_make_room() {
    let f = crate::state::test_support::Fixture::new();
    let limits = Limits {
        max_us: 60_000_000,
        max_bytes: 1 << 20,
    };

    // save_clip: quota_limit = 0 makes `make_room` fail before ReplayJob::spawn ever runs, so
    // the clips folder existing proves `writable` created it prior to `make_room`.
    let save_root = ScratchDir::new("save-dir");
    let save_dir = save_root.0.join("clips");
    assert!(!save_dir.exists());
    let shared = Shared::new("dvr-save-dir", "Game".into(), 60, save_dir.clone(), 0);
    let mut worker = Worker::new(f.host.clone(), Weak::new(), shared, limits);
    worker.video(h264_idr(0, true), false);
    let (tx, rx) = sync_channel(1);
    worker.save_clip(30, Reply::Api(tx));
    assert!(
        save_dir.is_dir(),
        "clips_dir must be created before make_room runs"
    );
    assert!(matches!(
        rx.try_recv(),
        Ok(Answer::Refused(Reason::QuotaExceeded))
    ));

    // start_recording: creates an uncreated clips folder before make_room runs.
    let rec_root = ScratchDir::new("rec-dir");
    let rec_dir = rec_root.0.join("clips");
    assert!(!rec_dir.exists());
    let shared = Shared::new("dvr-rec-dir", "Game".into(), 60, rec_dir.clone(), 1 << 30);
    let mut worker = Worker::new(f.host.clone(), Weak::new(), shared, limits);
    assert_eq!(worker.start_recording(), Ok(()));
    assert!(rec_dir.is_dir());
    assert!(worker.pending_recording.is_some());

    // Refused network paths fail with IoError before touching the filesystem.
    let bad = Shared::new(
        "dvr-unc",
        "Game".into(),
        60,
        PathBuf::from(r"\\server\share\clips"),
        1 << 30,
    );
    let mut worker = Worker::new(f.host.clone(), Weak::new(), bad, limits);
    assert_eq!(worker.start_recording(), Err(Reason::IoError));
}

#[test]
fn pending_recording_waits_for_headers_then_reuses_rings_param_sets_on_bare_idr() {
    let f = crate::state::test_support::Fixture::new();
    let root = ScratchDir::new("bare-idr");
    let dir = root.0.join("clips");
    let shared = Shared::new("dvr-bare", "Game".into(), 60, dir, 1 << 30);
    let limits = Limits {
        max_us: 60_000_000,
        max_bytes: 1 << 20,
    };
    let mut worker = Worker::new(f.host.clone(), Weak::new(), shared, limits);
    assert_eq!(worker.start_recording(), Ok(()));

    // Before the ring has any parameter sets, a bare IDR keeps the recording pending instead of
    // failing with IoError.
    worker.video(h264_idr(0, false), false);
    assert!(worker.recording.is_none());
    assert!(worker.pending_recording.is_some());
    assert!(worker.error.is_none());

    // Once the ring has learned parameter sets from a full IDR, a later recording can open on a
    // bare IDR without repeated SPS/PPS.
    worker.pending_recording = None;
    worker.video(h264_idr(100_000, true), false);
    assert!(worker.ring.params_for(VideoCodec::H264).is_some());
    assert_eq!(worker.start_recording(), Ok(()));
    worker.video(h264_idr(200_000, false), false);
    assert!(worker.recording.is_some());
    assert!(worker.pending_recording.is_none());
    assert!(worker.error.is_none());
    worker.stop_recording(None);
}

#[test]
fn spawn_failure_answers_reply_with_io_error() {
    let f = crate::state::test_support::Fixture::new();
    let s = f.session(f.client(u32::MAX), Role::Stream);
    let shared = Shared::new(
        "dvr-spawn-fail",
        "Game".into(),
        60,
        std::env::temp_dir(),
        1 << 30,
    );
    let limits = Limits {
        max_us: 60_000_000,
        max_bytes: 1 << 20,
    };
    let mut worker = Worker::new(f.host.clone(), Arc::downgrade(&s), shared, limits);
    let err = std::io::Error::other("thread limit");

    let (tx, rx) = sync_channel(1);
    worker.spawn_failed(Reply::Api(tx), &err);
    assert!(matches!(rx.try_recv(), Ok(Answer::Failed(Reason::IoError))));
    assert_eq!(worker.state(), (State::Error, Reason::IoError));

    worker.spawn_failed(Reply::Control(42), &err);
    let frames = s.dvr.take_outgoing();
    assert_eq!(frames.len(), 1);
    let msg = Message::decode(&frames[0]).expect("clip result frame");
    assert!(matches!(
        msg,
        Message::Clip(ClipResult {
            request_id: 42,
            outcome: Outcome::Failed,
            reason: Reason::IoError,
            ..
        })
    ));
}

#[test]
fn buffered_recording_audio_counts_toward_live_quota_and_enforces_limit_when_video_stalls() {
    let f = crate::state::test_support::Fixture::new();
    let root = ScratchDir::new("audio-quota");
    let dir = root.0.join("clips");
    // Quota allows starting the recording (~1 KB init segment) and a small amount of audio,
    // then trips when buffered audio pushes projected usage past 4 KiB without a video flush.
    let quota_limit = 4096;
    let shared = Shared::new("dvr-audio-quota", "Game".into(), 60, dir, quota_limit);
    let limits = Limits {
        max_us: 60_000_000,
        max_bytes: 1 << 20,
    };
    let mut worker = Worker::new(f.host.clone(), Weak::new(), shared.clone(), limits);
    worker.info.audio = Some(OpusConfig::stereo());
    assert_eq!(worker.start_recording(), Ok(()));
    worker.video(h264_idr(0, true), false);
    assert!(worker.recording.is_some());

    // Push a small audio packet within quota and verify live quota includes buffered bytes.
    let written = worker.recording.as_ref().unwrap().written();
    worker.recording.as_mut().unwrap().audio(AudioPacket {
        pts_us: 10_000,
        samples: 240,
        data: Arc::new(vec![0x11; 512]),
    });
    worker.update_live();
    assert!(shared.live().quota_used_bytes >= written + 512);

    // Push enough audio while video stays static to exceed `quota_limit`.
    for i in 1..20u64 {
        worker.recording.as_mut().unwrap().audio(AudioPacket {
            pts_us: i * 20_000,
            samples: 240,
            data: Arc::new(vec![0x22; 512]),
        });
    }
    assert!(worker.tick(Instant::now()));
    assert!(worker.recording.is_none());
    assert_eq!(worker.state(), (State::Error, Reason::QuotaExceeded));
}
