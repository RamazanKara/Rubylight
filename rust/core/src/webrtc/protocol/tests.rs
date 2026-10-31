use super::*;
use serde_json::json;

fn parse_err(text: &str) -> ErrorCode {
    parse_client_message(text).unwrap_err().code
}

/// A host message parses, and serializes back to the same JSON value.
fn round_trip_server(text: &str) -> ServerMessage {
    let message = ServerMessage::from_json(text).unwrap();
    let original: Value = serde_json::from_str(text).unwrap();
    let again: Value = serde_json::from_str(&message.to_json()).unwrap();
    assert_eq!(original, again, "{text}");
    message
}

/// A client message parses, and serializes back to the same JSON value.
fn round_trip_client(text: &str) -> ClientMessage {
    let message = parse_client_message(text).unwrap();
    let original: Value = serde_json::from_str(text).unwrap();
    let again: Value = serde_json::from_str(&message.to_json()).unwrap();
    assert_eq!(original, again, "{text}");
    message
}

#[test]
fn debug_output_never_shows_the_client_secret() {
    let secret = "9f8e7d6c5b4a39281706f5e4d3c2b1a0";
    let message = ServerMessage::paired("client-1", secret);
    assert!(!format!("{message:?}").contains(secret));
    assert!(format!("{message:?}").contains("<redacted>"));
    // The wire form still carries it.
    assert!(message.to_json().contains(secret));
}

#[test]
fn negative_and_huge_numbers_parse_so_validation_can_refuse_them() {
    let text = r#"{"version":2,"type":"start-session","appId":"0","video":{"width":-1,"height":1080,
            "fps":99999999999,"codec":"h264","bitrateKbps":-5,"hdr":false},
            "audio":{"channels":-2}}"#;
    match parse_client_message(text).unwrap() {
        ClientMessage::StartSession { video, audio, .. } => {
            assert_eq!(video.width, -1);
            assert_eq!(video.fps, 99_999_999_999);
            assert_eq!(video.bitrate_kbps, -5);
            assert_eq!(audio.channels, -2);
        }
        other => panic!("{other:?}"),
    }
    // A non-number is still an invalid message.
    let text = text.replace("\"width\":-1", "\"width\":\"wide\"");
    assert_eq!(parse_err(&text), ErrorCode::InvalidMessage);
}

#[test]
fn error_codes_match_the_protocol_strings() {
    for code in ErrorCode::ALL {
        assert_eq!(serde_json::to_value(code).unwrap(), json!(code.as_str()));
    }
    let names: Vec<&str> = ErrorCode::ALL.iter().map(|c| c.as_str()).collect();
    for expected in [
        "invalid-json",
        "unsupported-version",
        "invalid-message",
        "unsupported-message",
        "unsupported-pairing",
        "unsupported-settings",
        "not-authenticated",
        "already-authenticated",
        "authentication-failed",
        "client-disabled",
        "incorrect-pin",
        "pairing-cancelled",
        "pairing-expired",
        "pairing-failed",
        "sunshine-unavailable",
        "host-operation-active",
        "session-active",
        "invalid-application",
        "internal-error",
    ] {
        assert!(names.contains(&expected), "{expected}");
    }
    assert_eq!(names.len(), 19);
}

#[test]
fn doc_discovery_request_and_reply() {
    assert!(is_discovery_request(r#"{"version":2,"type":"discover"}"#));
    assert!(!is_discovery_request(r#"{"version":1,"type":"discover"}"#));
    assert!(!is_discovery_request(
        r#"{"version":2,"type":"authenticate"}"#
    ));
    assert!(!is_discovery_request("not json"));
    let reply = round_trip_server(
        r#"{"version":2,"type":"discovery","name":"Sunshine-PC","port":8000,"macAddress":"2C:F0:5D:7B:E6:D0"}"#,
    );
    assert_eq!(
        reply,
        ServerMessage::discovery("Sunshine-PC", 8000, Some("2C:F0:5D:7B:E6:D0".into()))
    );
    // macAddress is omitted when unknown.
    let bare = ServerMessage::discovery("Rubylight", 8000, None).to_json();
    assert!(!bare.contains("macAddress"));
}

#[test]
fn doc_auth_required_and_authenticate() {
    let nonce = "ab".repeat(32);
    let text = format!(
        r#"{{"version":2,"type":"auth-required","nonce":"{nonce}","pairing":"client-pin","macAddress":"2C:F0:5D:7B:E6:D0","sunshineAvailable":true}}"#
    );
    let message = round_trip_server(&text);
    assert_eq!(
        message,
        ServerMessage::auth_required(nonce, Some("2C:F0:5D:7B:E6:D0".into()))
    );
    let auth = format!(
        r#"{{"version":2,"type":"authenticate","clientId":"{}","proof":"{}"}}"#,
        "0f".repeat(16),
        "cd".repeat(32)
    );
    let ClientMessage::Authenticate { client_id, proof } = round_trip_client(&auth) else {
        panic!("not authenticate");
    };
    assert_eq!(client_id.len(), 32);
    assert_eq!(proof.len(), 64);
    round_trip_server(r#"{"version":2,"type":"authenticated"}"#);
    round_trip_server(r#"{"version":2,"type":"sunshine-availability","sunshineAvailable":true}"#);
}

#[test]
fn doc_pairing_messages() {
    let ClientMessage::RequestPairing { pin, client_name } = round_trip_client(
        r#"{"version":2,"type":"request-pairing","pin":"0421","clientName":"Samsung TV"}"#,
    ) else {
        panic!("not request-pairing");
    };
    assert_eq!((pin.as_str(), client_name.as_str()), ("0421", "Samsung TV"));
    // The name is optional.
    let ClientMessage::RequestPairing { client_name, .. } =
        parse_client_message(r#"{"version":2,"type":"request-pairing","pin":"0421"}"#).unwrap()
    else {
        panic!("not request-pairing");
    };
    assert_eq!(client_name, "");
    let text = format!(
        r#"{{"version":2,"type":"paired","clientId":"{}","clientSecret":"{}"}}"#,
        "0f".repeat(16),
        "cd".repeat(32)
    );
    assert!(matches!(
        round_trip_server(&text),
        ServerMessage::Paired { .. }
    ));
    for pin in ["042", "04211", "04a1", "٠٤٢١", ""] {
        let text = json!({"version": 2, "type": "request-pairing", "pin": pin}).to_string();
        assert_eq!(parse_err(&text), ErrorCode::InvalidMessage, "{pin}");
    }
}

#[test]
fn pair_client_is_unsupported_pairing() {
    assert_eq!(
        parse_err(r#"{"version":2,"type":"pair-client","pin":"1234"}"#),
        ErrorCode::UnsupportedPairing
    );
}

#[test]
fn doc_gateway_status() {
    let message = round_trip_server(
        r#"{"version":2,"type":"gateway-status","gatewayName":"Sunshine-PC","sunshineDetected":true,"sunshinePaired":true,"sessionActive":false,"runningAppId":"7","macAddress":"2C:F0:5D:7B:E6:D0"}"#,
    );
    assert!(matches!(
        message,
        ServerMessage::GatewayStatus {
            session_active: false,
            running_app_id: Some(_),
            ..
        }
    ));
    // runningAppId and macAddress are optional.
    round_trip_server(
        r#"{"version":2,"type":"gateway-status","gatewayName":"PC","sunshineDetected":true,"sunshinePaired":true,"sessionActive":true}"#,
    );
}

#[test]
fn doc_apps_and_artwork() {
    assert_eq!(
        round_trip_client(r#"{"version":2,"type":"get-apps"}"#),
        ClientMessage::GetApps
    );
    let apps = round_trip_server(
        r#"{"version":2,"type":"apps","apps":[{"id":"0","title":"Desktop","artworkAvailable":false,"running":false}]}"#,
    );
    let ServerMessage::Apps { apps } = apps else {
        panic!("not apps");
    };
    assert_eq!(apps[0].title, "Desktop");
    assert_eq!(
        round_trip_client(r#"{"version":2,"type":"get-app-artwork","appId":"0"}"#),
        ClientMessage::GetAppArtwork { app_id: "0".into() }
    );
    round_trip_server(
        r#"{"version":2,"type":"app-artwork","appId":"0","available":true,"mimeType":"image/jpeg","data":"AAEC"}"#,
    );
    round_trip_server(r#"{"version":2,"type":"app-artwork","appId":"0","available":false}"#);
    assert_eq!(
        parse_err(r#"{"version":2,"type":"get-app-artwork","appId":""}"#),
        ErrorCode::InvalidMessage
    );
    assert_eq!(
        parse_err(r#"{"version":2,"type":"get-app-artwork","appId":7}"#),
        ErrorCode::InvalidMessage
    );
}

const START: &str = r#"{
      "version": 2,
      "type": "start-session",
      "appId": "0",
      "video": {
        "width": 3840,
        "height": 2160,
        "fps": 120,
        "codec": "hevc",
        "bitrateKbps": 50000,
        "hdr": true
      },
      "audio": {"channels": 2}
    }"#;

const SWITCH: &str = r#"{
      "version":2,
      "type":"switch-session",
      "appId":"7",
      "video":{"width":1920,"height":1080,"fps":60,"codec":"hevc","bitrateKbps":20000,"hdr":false},
      "audio":{"channels":2}
    }"#;

#[test]
fn doc_start_and_switch_session() {
    let ClientMessage::StartSession {
        app_id,
        video,
        audio,
    } = round_trip_client(START)
    else {
        panic!("not start-session");
    };
    assert_eq!(app_id, "0");
    assert_eq!(
        video,
        VideoSettings {
            width: 3840,
            height: 2160,
            fps: 120,
            codec: "hevc".into(),
            bitrate_kbps: 50000,
            hdr: true,
        }
    );
    assert_eq!(audio.channels, 2);
    assert_eq!(audio.sample_rate, None);
    let ClientMessage::SwitchSession { app_id, video, .. } = round_trip_client(SWITCH) else {
        panic!("not switch-session");
    };
    assert_eq!((app_id.as_str(), video.fps, video.hdr), ("7", 60, false));
}

#[test]
fn session_settings_missing_a_field_are_invalid_messages() {
    let mut value: Value = serde_json::from_str(START).unwrap();
    value["video"].as_object_mut().unwrap().remove("hdr");
    assert_eq!(parse_err(&value.to_string()), ErrorCode::InvalidMessage);
    let mut value: Value = serde_json::from_str(START).unwrap();
    value.as_object_mut().unwrap().remove("audio");
    assert_eq!(parse_err(&value.to_string()), ErrorCode::InvalidMessage);
    let mut value: Value = serde_json::from_str(START).unwrap();
    value["video"]["fps"] = json!("sixty");
    assert_eq!(parse_err(&value.to_string()), ErrorCode::InvalidMessage);
}

#[test]
fn doc_session_status_and_host_session_status() {
    round_trip_server(r#"{"version":2,"type":"session-status","state":"idle"}"#);
    for state in [
        "idle",
        "starting",
        "starting-webrtc",
        "connecting-sunshine",
        "starting-moonlight",
        "streaming",
        "stopping",
        "error",
    ] {
        round_trip_server(
            &json!({"version": 2, "type": "session-status", "state": state, "sessionId": 3})
                .to_string(),
        );
    }
    let streaming = round_trip_server(
        r#"{"version":2,"type":"session-status","state":"streaming","sessionId":3,"video":{"width":1920,"height":1080,"fps":60,"codec":"h264","bitrateKbps":20000,"hdr":false},"audio":{"channels":2,"sampleRate":48000},"message":"ok"}"#,
    );
    assert!(matches!(
        streaming,
        ServerMessage::SessionStatus {
            state: SessionState::Streaming,
            session_id: Some(3),
            ..
        }
    ));
    for state in [
        "stopping",
        "switching",
        "resuming",
        "starting",
        "stopped",
        "failed",
    ] {
        round_trip_server(
            &json!({"version": 2, "type": "host-session-status", "state": state,
                    "runningAppId": "7", "targetAppId": "8", "message": "m"})
            .to_string(),
        );
    }
    round_trip_server(r#"{"version":2,"type":"host-session-status","state":"stopped"}"#);
}

#[test]
fn doc_stop_messages() {
    assert_eq!(
        round_trip_client(r#"{"version":2,"type":"stop-session"}"#),
        ClientMessage::StopSession
    );
    assert_eq!(
        round_trip_client(r#"{"version":2,"type":"stop-host-session"}"#),
        ClientMessage::StopHostSession
    );
}

#[test]
fn doc_error_message() {
    let message = round_trip_server(
        r#"{"version":2,"type":"error","requestType":"start-session","code":"unsupported-settings","message":"Unsupported resolution"}"#,
    );
    assert_eq!(
        message,
        ServerMessage::error(
            "start-session",
            ErrorCode::UnsupportedSettings,
            "Unsupported resolution"
        )
    );
    let reply =
        ProtocolError::new(ErrorCode::ClientDisabled, "disabled").into_message("authenticate");
    assert!(reply.to_json().contains(r#""code":"client-disabled""#));
}

#[test]
fn signalling_messages_carry_a_session_id() {
    let offer = ServerMessage::offer(4, "v=0");
    assert_eq!(round_trip_server(&offer.to_json()), offer);
    let candidate = ServerMessage::candidate(4, "candidate:1 1 udp 1 10.0.0.2 8001 typ host", "0");
    assert_eq!(round_trip_server(&candidate.to_json()), candidate);
    let answer = round_trip_client(r#"{"version":2,"type":"answer","sessionId":4,"sdp":"v=0"}"#);
    assert_eq!(answer.type_name(), "answer");
    let candidate = round_trip_client(
        r#"{"version":2,"type":"candidate","sessionId":4,"candidate":"candidate:1","mid":"0"}"#,
    );
    assert_eq!(candidate.type_name(), "candidate");
    for text in [
        r#"{"version":2,"type":"answer","sessionId":0,"sdp":"v=0"}"#,
        r#"{"version":2,"type":"answer","sessionId":-1,"sdp":"v=0"}"#,
        r#"{"version":2,"type":"answer","sdp":"v=0"}"#,
        r#"{"version":2,"type":"candidate","sessionId":0,"candidate":"c","mid":"0"}"#,
    ] {
        assert_eq!(parse_err(text), ErrorCode::InvalidMessage, "{text}");
    }
}

#[test]
fn client_log_lines_are_bounded_and_single_line() {
    let text = json!({"version": 2, "type": "client-log",
            "lines": ["ok", "a\nb\u{7f}c", "x".repeat(MAX_CLIENT_LOG_LINE_LENGTH + 50), "é".repeat(MAX_CLIENT_LOG_LINE_LENGTH)]})
        .to_string();
    let ClientMessage::ClientLog { lines } = parse_client_message(&text).unwrap() else {
        panic!("not client-log");
    };
    assert_eq!(lines[0], "ok");
    assert_eq!(lines[1], "a b c");
    assert_eq!(lines[2].len(), MAX_CLIENT_LOG_LINE_LENGTH);
    assert!(lines[3].len() <= MAX_CLIENT_LOG_LINE_LENGTH);
    assert!(lines[3].chars().all(|c| c == 'é'));
    let many = json!({"version": 2, "type": "client-log", "lines": vec!["x"; 101]});
    assert_eq!(parse_err(&many.to_string()), ErrorCode::InvalidMessage);
    let not_strings = json!({"version": 2, "type": "client-log", "lines": [1]});
    assert_eq!(
        parse_err(&not_strings.to_string()),
        ErrorCode::InvalidMessage
    );
}

#[test]
fn unknown_fields_are_ignored() {
    let text = r#"{"version":2,"type":"stop-session","future":{"a":1},"x":[]}"#;
    assert_eq!(
        parse_client_message(text).unwrap(),
        ClientMessage::StopSession
    );
    let text = START
        .replace(r#""audio""#, r#""surprise": true, "audio""#)
        .replace(r#""hdr": true"#, r#""hdr": true, "extra": 1"#);
    assert!(parse_client_message(&text).is_ok());
    let host = r#"{"version":2,"type":"authenticated","also":"ignored"}"#;
    assert_eq!(
        ServerMessage::from_json(host).unwrap(),
        ServerMessage::Authenticated
    );
}

#[test]
fn version_must_be_the_integer_two() {
    for text in [
        r#"{"type":"get-apps"}"#,
        r#"{"version":1,"type":"get-apps"}"#,
        r#"{"version":3,"type":"get-apps"}"#,
        r#"{"version":"2","type":"get-apps"}"#,
        r#"{"version":2.0,"type":"get-apps"}"#,
        r#"{"version":null,"type":"get-apps"}"#,
        r#"[]"#,
    ] {
        assert_eq!(parse_err(text), ErrorCode::UnsupportedVersion, "{text}");
    }
    assert_eq!(
        ServerMessage::from_json(r#"{"version":1,"type":"authenticated"}"#)
            .unwrap_err()
            .code,
        ErrorCode::UnsupportedVersion
    );
}

#[test]
fn malformed_messages_get_the_matching_code() {
    assert_eq!(parse_err("{"), ErrorCode::InvalidJson);
    assert_eq!(parse_err(""), ErrorCode::InvalidJson);
    assert_eq!(
        parse_err(r#"{"version":2,"type":"reboot"}"#),
        ErrorCode::UnsupportedMessage
    );
    assert_eq!(parse_err(r#"{"version":2}"#), ErrorCode::InvalidMessage);
    assert_eq!(
        parse_err(r#"{"version":2,"type":7}"#),
        ErrorCode::InvalidMessage
    );
    assert_eq!(
        parse_err(r#"{"version":2,"type":"authenticate","clientId":"a"}"#),
        ErrorCode::InvalidMessage
    );
    let long = "a".repeat(MAX_CLIENT_ID_LENGTH + 1);
    let text = json!({"version": 2, "type": "authenticate", "clientId": long, "proof": "ab"});
    assert_eq!(parse_err(&text.to_string()), ErrorCode::InvalidMessage);
    let text = json!({"version": 2, "type": "authenticate", "clientId": "", "proof": "ab"});
    assert_eq!(parse_err(&text.to_string()), ErrorCode::InvalidMessage);
}

#[test]
fn keyframe_request_is_recognised_only_by_type() {
    assert!(is_keyframe_request(r#"{"type":"request-keyframe"}"#));
    assert!(is_keyframe_request(
        r#"{"v":1,"type":"request-keyframe","x":2}"#
    ));
    assert!(!is_keyframe_request(r#"{"type":"rumble"}"#));
    assert!(!is_keyframe_request(r#"{"note":"request-keyframe"}"#));
    assert!(!is_keyframe_request("request-keyframe"));
}

#[test]
fn control_messages_are_sorted_by_type() {
    assert_eq!(
        classify_control(r#"{"type":"request-keyframe"}"#),
        ControlMessage::KeyframeRequest
    );
    for kind in ["gamepad-connected", "gamepad-disconnected"] {
        assert_eq!(
            classify_control(&format!(r#"{{"v":2,"type":"{kind}","controllerId":0}}"#)),
            ControlMessage::GamepadAnnouncement
        );
    }
    // State snapshots belong on the gamepad channel only.
    assert_eq!(
        classify_control(r#"{"type":"gamepad-state"}"#),
        ControlMessage::Other
    );
    assert_eq!(classify_control("not json"), ControlMessage::Other);
    assert_eq!(
        classify_control(r#"{"note":"gamepad-connected"}"#),
        ControlMessage::Other
    );
}
