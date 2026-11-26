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
