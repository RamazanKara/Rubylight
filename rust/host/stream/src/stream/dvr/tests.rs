//! Stream DVR's names and texts stay in step with the wire, and a replay clip written from the
//! replay buffer is a complete file.
use super::*;

const REASONS: [Reason; 12] = [
    Reason::None,
    Reason::Disabled,
    Reason::Pyrowave,
    Reason::DiskFull,
    Reason::QuotaExceeded,
    Reason::NoKeyframeYet,
    Reason::Busy,
    Reason::PermissionDenied,
    Reason::AudioCodecUnsupported,
    Reason::IoError,
    Reason::NotStreaming,
    Reason::Unknown,
];

#[test]
fn native_api_names_are_the_wire_names() {
    for reason in REASONS {
        assert_eq!(
            serde_json::to_value(api_reason(reason)).unwrap(),
            serde_json::json!(reason.as_str()),
            "{reason:?}"
        );
    }
    for (state, name) in [
        (State::Off, "off"),
        (State::Armed, "armed"),
        (State::Recording, "recording"),
        (State::Saving, "saving"),
        (State::Unsupported, "unsupported"),
        (State::Error, "error"),
    ] {
        assert_eq!(
            serde_json::to_value(api_state(state)).unwrap(),
            serde_json::json!(name)
        );
    }
}

#[test]
fn every_reason_has_plain_english_without_mod_or_vendor_names() {
    for reason in REASONS {
        let text = reason_text(reason);
        assert!(text.ends_with('.'), "{reason:?}: {text}");
        assert!(
            !text.to_ascii_lowercase().contains("experimental"),
            "{text}"
        );
    }
}

#[test]
fn a_replay_clip_is_written_whole_or_not_at_all() {
    let dir = std::env::temp_dir().join(format!(
        "rubylight-dvr-replay-{}-{:08x}",
        std::process::id(),
        rand::random::<u32>()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let clip = butterpollo_media_policy::dvr::ring::Clip {
        info: butterpollo_media_policy::dvr::StreamInfo {
            codec: butterpollo_media_policy::dvr::VideoCodec::H264,
            hdr: None,
            audio: None,
        },
        params: Default::default(),
        video: Vec::new(),
        audio: Vec::new(),
    };
    // A clip without pictures does not mux: nothing is left behind.
    let id = "20260101-100000-replay-0001";
    let result = save::write_replay(&dir, id, "Game", &clip);
    let left = std::fs::read_dir(&dir).unwrap().count();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(result.is_err());
    assert_eq!(left, 0);
}

#[test]
fn the_first_replay_save_makes_the_clips_folder() {
    // A new profile's `clips` folder does not exist yet (found on hardware: every Instant
    // Replay save failed with io_error until a manual recording had made it).
    let parent = std::env::temp_dir().join(format!(
        "rubylight-dvr-folder-{}-{:08x}",
        std::process::id(),
        rand::random::<u32>()
    ));
    let dir = parent.join("clips");
    let clip = butterpollo_media_policy::dvr::ring::Clip {
        info: butterpollo_media_policy::dvr::StreamInfo {
            codec: butterpollo_media_policy::dvr::VideoCodec::H264,
            hdr: None,
            audio: None,
        },
        params: Default::default(),
        video: Vec::new(),
        audio: Vec::new(),
    };
    let _ = save::write_replay(&dir, "20260101-100000-replay-0002", "Game", &clip);
    let made = dir.is_dir();
    let _ = std::fs::remove_dir_all(&parent);
    assert!(made, "the clips folder was not made");
}

fn shared(session: &str) -> Arc<Shared> {
    Shared::new(session, "Game".into(), 60, std::env::temp_dir(), 1 << 30)
}

#[test]
fn a_finished_worker_takes_no_commands_and_answers_the_queued_ones() {
    let dvr = shared("dvr-closed");
    let (reply, answer) = sync_channel(1);
    let save = Command::SaveClip {
        secs: 30,
        reply: Reply::Api(reply),
    };
    assert!(dvr.push(save).is_ok());
    let queued = dvr.close();
    assert_eq!(queued.len(), 1, "close hands the queue to the worker");
    assert!(dvr.take_commands().is_empty());
    let (late, _) = sync_channel(1);
    let stop = Command::StopRecord {
        reply: Reply::Api(late),
    };
    assert!(
        dvr.push(stop).is_err(),
        "given back at once, not left to wait"
    );
    drop(queued);
    assert!(answer.try_recv().is_err());
}

#[test]
fn one_stream_gets_one_registration_however_many_threads_arm_it() {
    let store = Arc::new(crate::dvr_store::DvrStore::default());
    let starts = Arc::new(AtomicU64::new(0));
    let threads: Vec<_> = (0..4)
        .map(|_| {
            let (store, starts) = (store.clone(), starts.clone());
            std::thread::spawn(move || {
                store.get_or_start("dvr-race", || {
                    starts.fetch_add(1, Ordering::Relaxed);
                    std::thread::sleep(Duration::from_millis(20));
                    Some(shared("dvr-race"))
                })
            })
        })
        .collect();
    let got: Vec<Arc<Shared>> = threads
        .into_iter()
        .map(|t| t.join().unwrap().unwrap())
        .collect();
    assert_eq!(starts.load(Ordering::Relaxed), 1);
    assert!(got.iter().all(|dvr| Arc::ptr_eq(dvr, &got[0])));
    // Another stream gets none while this one has it.
    assert!(
        store
            .get_or_start("dvr-other", || Some(shared("dvr-other")))
            .is_none()
    );
    // Only the running buffer itself unregisters it.
    store.unregister("dvr-race", &shared("dvr-race"));
    assert!(store.active_for("dvr-race").is_some());
    store.unregister("dvr-race", &got[0]);
    assert!(store.active().is_none());
}

#[test]
fn a_stream_arms_when_turned_on_and_once_the_buffer_before_it_is_let_go() {
    use butterpollo_session::session::Role;
    let f = crate::state::test_support::Fixture::new();
    let h = &f.host;
    let first = f.session(f.client(u32::MAX), Role::Stream);
    assert_eq!(eligibility(&first), Ok(()));
    arm_owner(h);
    assert!(h.dvr.active().is_none(), "off by default");
    // Turned on during the stream: the save arms it, and a device that asked hears its state.
    first.dvr.mark_speaks();
    h.config
        .write()
        .unwrap()
        .values
        .insert("dvr_enabled".into(), "true".into());
    arm_owner(h);
    let armed = h
        .dvr
        .active_for(&first.launch.id)
        .expect("armed by the save");
    assert!(armed.client_speaks.load(Ordering::Acquire));
    assert!(first.dvr.is_armed());
    // A second stream waits while the first holds the buffer, and arms once it is let go.
    let second = f.session(f.client(u32::MAX), Role::Stream);
    assert!(start(h, &second, &h.config.read().unwrap().clone()).is_none());
    first.stop();
    end(h, &first);
    assert!(h.dvr.active_for(&second.launch.id).is_some());
    assert!(second.dvr.is_armed());
    second.stop();
    end(h, &second);
    assert!(h.dvr.active().is_none());
}
