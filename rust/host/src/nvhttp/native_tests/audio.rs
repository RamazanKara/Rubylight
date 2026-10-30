//! `GET /audio` and `POST /audio/settings` (`docs/native-api.md`, "Audio processing") through
//! the router.
use super::*;
use butterpollo_core::{
    audio_dsp::Settings,
    native_api::{
        audio::{Audio, HEADPHONE_SURROUND, NIGHT_MODE},
        radeon::{ControlState, Value},
    },
};

async fn write(s: &Setup, connection: &Connection, body: &str) -> Reply {
    send(
        &s.f,
        true,
        connection,
        "POST",
        &format!("{PREFIX}/audio/settings"),
        Some("application/json"),
        body,
    )
    .await
}

fn audio(reply: &Reply) -> Audio {
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    assert_eq!(reply.headers[header::CACHE_CONTROL], "no-store");
    serde_json::from_slice(&reply.body).unwrap()
}

/// Each setting's state and value, night mode first.
fn values(audio: &Audio) -> Vec<(&str, ControlState, Option<&Value>)> {
    audio
        .settings
        .iter()
        .map(|s| (s.id.as_str(), s.state, s.value.as_ref()))
        .collect()
}

#[tokio::test]
async fn without_a_stream_both_settings_wait_for_one() {
    let s = Setup::new();
    let idle = audio(&s.get("/audio").await);
    assert_eq!(
        values(&idle),
        [
            (NIGHT_MODE, ControlState::TemporarilyUnavailable, None),
            (
                HEADPHONE_SURROUND,
                ControlState::TemporarilyUnavailable,
                None
            )
        ]
    );
    assert!(
        idle.settings
            .iter()
            .all(|s| s.reason.as_deref() == Some("no_session"))
    );
    // Another device's stream is not this one's.
    let _session = s.streaming();
    let other = other_device(&s.f, ALL, false);
    let theirs = audio(&api(&s.f, &other, "GET", "/audio").await);
    assert_eq!(theirs, idle);
}

#[tokio::test]
async fn a_write_changes_the_callers_stream_from_the_next_packet() {
    let s = Setup::new();
    let session = s.streaming();
    let start = audio(&s.get("/audio").await);
    let off = Some(&Value::Bool(false));
    assert_eq!(
        values(&start),
        [
            (NIGHT_MODE, ControlState::Configurable, off),
            (HEADPHONE_SURROUND, ControlState::Configurable, off)
        ]
    );
    let changed = audio(
        &write(
            &s,
            &s.connection,
            r#"{"changes":[{"id":"audio.night_mode","value":true}]}"#,
        )
        .await,
    );
    let on = Some(&Value::Bool(true));
    assert_eq!(
        values(&changed),
        [
            (NIGHT_MODE, ControlState::Configurable, on),
            (HEADPHONE_SURROUND, ControlState::Configurable, off)
        ]
    );
    assert!(changed.revision > start.revision);
    // What the audio thread reads for its next packet, and what the next read shows.
    assert_eq!(
        session.audio.settings(Settings::default()),
        Settings {
            night_mode: true,
            headphone_surround: false
        }
    );
    assert_eq!(audio(&s.get("/audio").await), changed);
    // Launch alone may change its own stream too.
    s.host().paired.write().unwrap().clients[0].perm = LAUNCH;
    let both = audio(
        &write(
            &s,
            &s.connection,
            r#"{"changes":[{"id":"audio.night_mode","value":false},{"id":"audio.headphone_surround","value":true}]}"#,
        )
        .await,
    );
    assert_eq!(
        values(&both)
            .into_iter()
            .map(|(_, _, value)| value.cloned())
            .collect::<Vec<_>>(),
        [Some(Value::Bool(false)), Some(Value::Bool(true))]
    );
}

#[tokio::test]
async fn writes_refuse_bad_bodies_other_devices_and_missing_permissions() {
    let s = Setup::new();
    let night = r#"{"changes":[{"id":"audio.night_mode","value":true}]}"#;
    write(&s, &s.connection, night)
        .await
        .assert_error(409, "no_session");
    let session = s.streaming();
    for body in [
        "",
        "{}",
        r#"{"changes":[]}"#,
        r#"{"changes":[{"id":"audio.night_mode"}]}"#,
        r#"{"changes":[{"id":"audio.night_mode","value":true,"scope":"host"}]}"#,
        r#"{"changes":[{"id":"audio.night_mode","value":true}],"gpu":1}"#,
    ] {
        write(&s, &s.connection, body)
            .await
            .assert_error(400, "invalid_request");
    }
    for value in ["null", "0.5", "1", r#""on""#] {
        let reply = write(
            &s,
            &s.connection,
            &format!(r#"{{"changes":[{{"id":"audio.headphone_surround","value":{value}}}]}}"#),
        )
        .await;
        reply.assert_error(422, "invalid_value");
        assert_eq!(reply.json()["error"]["setting"], HEADPHONE_SURROUND);
    }
    let reply = write(
        &s,
        &s.connection,
        r#"{"changes":[{"id":"audio.night_mode","value":true},{"id":"audio.loudness","value":true}]}"#,
    )
    .await;
    reply.assert_error(404, "not_found");
    assert_eq!(reply.json()["error"]["setting"], "audio.loudness");
    let other = other_device(&s.f, ALL, true);
    write(&s, &other, night)
        .await
        .assert_error(409, "not_session_owner");
    // None of these changed anything, all or nothing.
    assert_eq!(
        session.audio.settings(Settings::default()),
        Settings::default()
    );
    let lister = Setup::with(LIST);
    write(&lister, &lister.connection, night)
        .await
        .assert_error(403, "permission_required");
    lister
        .get("/audio")
        .await
        .assert_error(403, "permission_required");
}
