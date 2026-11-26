//! The Stream DVR examples of `docs/native-api.md` (`GET /dvr`, `GET /dvr/clips`,
//! `POST /dvr/record`), one fixture each. Nightjar's tests read the same files.
use rubylight_native_api::dvr::{
    ClipKind, ClipList, ClipRequest, DvrReason, DvrState, DvrStatus, RecordAction, RecordRequest,
    RecordResult,
};
use rubylight_native_api::error::ErrorCode;
use rubylight_native_api::feature;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value as Json, json};

fn fixture(name: &str) -> String {
    let path = format!("{}/tests/fixtures/{name}.json", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{path}: {error}"))
}

/// Parses the fixture, writes it back and compares the JSON values, so a
/// field the types drop, rename or add fails here.
fn round_trip<T: DeserializeOwned + Serialize + PartialEq + std::fmt::Debug>(name: &str) -> T {
    let text = fixture(name);
    assert!(!text.contains('\r'), "{name}.json must use LF");
    let expected: Json = serde_json::from_str(&text).unwrap();
    let typed: T = serde_json::from_str(&text).unwrap_or_else(|error| panic!("{name}: {error}"));
    assert_eq!(serde_json::to_value(&typed).unwrap(), expected, "{name}");
    let again: T = serde_json::from_value(serde_json::to_value(&typed).unwrap()).unwrap();
    assert_eq!(again, typed, "{name}");
    typed
}

#[test]
fn status_round_trips() {
    let status: DvrStatus = round_trip("dvr");
    assert_eq!(status.state, DvrState::Armed);
    assert_eq!(status.reason, DvrReason::None);
    assert_eq!(status.container, "mp4");
    assert_eq!(feature::STREAM_DVR, "stream.dvr");
}

#[test]
fn clips_round_trip_newest_first() {
    let list: ClipList = round_trip("dvr_clips");
    assert_eq!(list.clips.len(), 2);
    assert!(list.clips[0].created_unix_ms > list.clips[1].created_unix_ms);
    assert_eq!(list.clips[1].kind, ClipKind::Manual);
    assert!(list.clips[1].recovered);
    let record: RecordResult = round_trip("dvr_record");
    assert_eq!(record.clip.unwrap().kind, ClipKind::Manual);
}

#[test]
fn requests_refuse_unknown_fields() {
    let empty: ClipRequest = serde_json::from_value(json!({})).unwrap();
    assert_eq!(empty.duration_secs, None);
    let sixty: ClipRequest = serde_json::from_value(json!({"duration_secs": 60})).unwrap();
    assert_eq!(sixty.duration_secs, Some(60));
    assert!(serde_json::from_value::<ClipRequest>(json!({"seconds": 60})).is_err());
    let stop: RecordRequest = serde_json::from_value(json!({"action": "stop"})).unwrap();
    assert_eq!(stop.action, RecordAction::Stop);
    assert!(serde_json::from_value::<RecordRequest>(json!({"action": "pause"})).is_err());
    assert!(serde_json::from_value::<RecordRequest>(json!({"action": "start", "x": 1})).is_err());
}

#[test]
fn newer_hosts_values_read_as_unknown() {
    let status: DvrStatus =
        serde_json::from_value(json!({"state": "paused", "reason": "gpu_busy", "extra": 1}))
            .unwrap();
    assert_eq!(status.state, DvrState::Unknown);
    assert_eq!(status.reason, DvrReason::Unknown);
    assert_eq!(
        serde_json::to_value(DvrReason::AudioCodecUnsupported).unwrap(),
        json!("audio_codec_unsupported")
    );
    assert_eq!(ErrorCode::DvrRefused.status(), 409);
}
