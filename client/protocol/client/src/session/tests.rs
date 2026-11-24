//! Unit tests for session configuration, startup helpers, termination gating and stages.

use super::shared::{first_outcome, rtsp_error_code};
use super::start::{decide_location, microphone_wanted, open_microphone_socket};
use super::*;
use crate::priority;
use crate::rtsp::{OpusConfig, RtspError};
use crate::sdp::{self, AudioConfiguration, StreamConfig, StreamLocation};

#[test]
fn silent_audio_starts_on_every_codec() {
    use rubylight_protocol::audio_ext::{CODEC_AC3, CODEC_EAC3, CODEC_OPUS, CODEC_PCM};
    for codec in [CODEC_OPUS, CODEC_PCM, CODEC_AC3, CODEC_EAC3] {
        let setup = AudioSetup {
            configuration: AudioConfiguration::STEREO,
            opus: OpusConfig::default(),
            codec,
            packet_duration_ms: 5,
            samples_per_frame: 240,
        };
        let mut silent = SilentAudio;
        assert_eq!(silent.init(&setup), Ok(()), "codec {codec}");
        silent.play(Some(&[1, 2, 3]));
        silent.play(None);
    }
}

#[test]
fn a_packed_refusal_has_its_own_start_code_and_keeps_its_text() {
    let refused = RtspError::PackedRefused(
        "requested packed codec and bit depth are not supported by both endpoints".into(),
    );
    assert_eq!(rtsp_error_code(&refused), error::PACKED_444_REFUSED);
    assert_eq!(
        refused.to_string(),
        "RTSP: requested packed codec and bit depth are not supported by both endpoints"
    );
    // Every other failure keeps its code, so only a packed refusal reconnects without it.
    assert_eq!(
        rtsp_error_code(&RtspError::Unsupported("x".into())),
        error::FAILED
    );
    assert_eq!(
        rtsp_error_code(&RtspError::Status {
            request: "ANNOUNCE",
            code: 400,
            reason: "Bad Request".into(),
        }),
        400
    );
}

#[test]
fn auto_location_caps_remote_packet_sizes() {
    let mut stream = StreamConfig {
        packet_size: 1392,
        ..Default::default()
    };
    decide_location(Location::Auto, "192.168.1.2".parse().unwrap(), &mut stream);
    assert_eq!(
        (stream.location, stream.packet_size),
        (StreamLocation::Local, 1392)
    );
    decide_location(Location::Auto, "8.8.8.8".parse().unwrap(), &mut stream);
    assert_eq!(
        (stream.location, stream.packet_size),
        (StreamLocation::Remote, 1024)
    );
    stream.packet_size = 1392;
    decide_location(Location::Local, "8.8.8.8".parse().unwrap(), &mut stream);
    assert_eq!(
        (stream.location, stream.packet_size),
        (StreamLocation::Local, 1392)
    );
}

/// Hears terminations and warnings.
struct Heard {
    terminated: Mutex<std::sync::mpsc::Sender<i32>>,
    warnings: Mutex<Vec<String>>,
}

impl Listener for Heard {
    fn connection_terminated(&self, error: i32) {
        let _ = self.terminated.lock().unwrap().send(error);
    }

    fn warn(&self, message: &str) {
        self.warnings.lock().unwrap().push(message.to_owned());
    }
}

fn heard() -> (Arc<Heard>, std::sync::mpsc::Receiver<i32>) {
    let (sender, receiver) = std::sync::mpsc::channel();
    let heard = Arc::new(Heard {
        terminated: Mutex::new(sender),
        warnings: Mutex::new(Vec::new()),
    });
    (heard, receiver)
}

#[test]
fn stream_threads_raised_alike_make_one_log_line_per_stream() {
    let (first, _) = heard();
    let shared = Shared::new(first, true, false);
    assert!(first_outcome(&shared.raise_outcomes, "nice -10".into()));
    assert!(!first_outcome(&shared.raise_outcomes, "nice -10".into()), "the next thread raised alike is not listed");
    assert!(first_outcome(&shared.raise_outcomes, "refused: EPERM".into()), "another outcome is");
    let (next, _) = heard();
    assert!(first_outcome(&Shared::new(next, true, false).raise_outcomes, "nice -10".into()), "each stream says it again");
}

#[test]
fn the_receive_decode_priority_follows_its_setting_and_is_off_by_default() {
    // The protocol default stays off; Android's "Decoder priority for receiving" (on by default)
    // and the desktop pass it in explicitly.
    assert!(!SessionConfig::default().receive_decode_priority);
    if std::env::var_os(priority::DECODE_PRIORITY_ENV).is_none() {
        let (off, _) = heard();
        assert!(!Shared::new(off, true, false).decode_priority);
        let (on, _) = heard();
        assert!(Shared::new(on, true, true).decode_priority);
    }
}

#[test]
fn a_termination_during_a_failed_start_is_not_reported_beside_the_failure() {
    let (heard, terminated) = heard();
    let shared = Shared::new(heard, false, true);
    shared.terminate(7);
    assert_eq!(shared.start_failed(), Some(7), "the held code is logged");
    shared.terminate(8);
    assert!(
        terminated.recv_timeout(Duration::from_millis(200)).is_err(),
        "only the stage failure reports a failed start, so no termination can reach a retry"
    );
}

#[test]
fn a_termination_during_a_successful_start_is_reported_once_it_returns() {
    let (heard, terminated) = heard();
    let shared = Shared::new(heard, false, true);
    shared.terminate(7);
    assert!(terminated.recv_timeout(Duration::from_millis(100)).is_err());
    shared.started();
    assert_eq!(terminated.recv_timeout(Duration::from_secs(5)), Ok(7));
    shared.terminate(8);
    assert!(
        terminated.recv_timeout(Duration::from_millis(100)).is_err(),
        "reported once"
    );
}

#[test]
fn a_termination_after_the_start_is_reported_at_once() {
    let (heard, terminated) = heard();
    let shared = Shared::new(heard, false, true);
    shared.started();
    shared.terminate(9);
    assert_eq!(terminated.recv_timeout(Duration::from_secs(5)), Ok(9));
    assert_eq!(shared.start_failed(), None);
}

#[test]
fn a_remote_monitor_opens_no_microphone() {
    let encrypted = sdp::SS_ENC_MICROPHONE;
    assert_eq!(microphone_wanted(48000, encrypted, false), Ok(true));
    assert!(microphone_wanted(48000, encrypted, true).is_err());
    assert_eq!(
        microphone_wanted(0, encrypted, true),
        Ok(false),
        "none offered: no word"
    );
    assert!(microphone_wanted(48000, 0, false).is_err());
}

#[test]
fn a_microphone_socket_that_does_not_open_is_warned_about() {
    let (heard, _terminated) = heard();
    // TEST-NET-1 is never this machine's address, so the bind fails.
    let local: SocketAddr = "192.0.2.1:0".parse().unwrap();
    assert!(open_microphone_socket(local, false, heard.as_ref()).is_none());
    let warnings = heard.warnings.lock().unwrap();
    assert!(
        warnings
            .iter()
            .any(|w| w.starts_with("Microphone: could not open the socket: ")),
        "{warnings:?}"
    );
    drop(warnings);
    let loopback: SocketAddr = "127.0.0.1:0".parse().unwrap();
    assert!(open_microphone_socket(loopback, false, heard.as_ref()).is_some());
}

#[test]
fn stage_names_match_the_c() {
    assert_eq!(Stage::RtspHandshake.name(), "RTSP handshake");
    assert_eq!(Stage::InputStreamStart as i32, 11);
}
