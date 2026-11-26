//! Handshake tests for Dolby audio negotiated with the `x-ss-audio` RTSP lines, and the ANNOUNCE
//! bytes the Rubylight and stock paths keep.

use super::*;
use rubylight_protocol::audio_ext::{CODEC_AC3, CODEC_EAC3, CODEC_PCM};

const ALL: u8 = CODEC_PCM | CODEC_AC3 | CODEC_EAC3;

/// A Sunshine-family DESCRIBE without Rubylight's lines, with surround parameters up to 7.1.
fn stock_describe() -> String {
    DESCRIBE.replace("a=x-rl-control:0x5530,0x5536\r\n", "")
        + "a=fmtp:97 surround-params=642014523\r\na=fmtp:97 surround-params=85301456723\r\n"
}

fn offering(codec: &str) -> String {
    format!("{}a=x-ss-audio-codec-offer:{codec}\n", stock_describe())
}

/// A handshake against [`sunshine_host`] with `describe` and `reply_headers` on the ANNOUNCE
/// reply: the ANNOUNCE sent and the outcome.
fn negotiate(
    audio: AudioConfiguration,
    passthrough: u8,
    describe: &str,
    reply_headers: &str,
) -> (String, Result<RtspOutcome, RtspError>) {
    let mut p = params(None);
    p.config.audio = audio;
    p.config.audio_passthrough = passthrough;
    let mut handshake = Handshake::new(p).unwrap();
    let mut announce = String::new();
    while let Some(request) = handshake.next_plain_request().unwrap() {
        let text = String::from_utf8(request.serialize()).unwrap();
        let cseq = request
            .options
            .iter()
            .find(|(k, _)| k == "CSeq")
            .unwrap()
            .1
            .clone();
        let raw = match request.method.as_str() {
            "DESCRIBE" => ok(2, "", describe),
            "ANNOUNCE" => {
                announce = text;
                ok(1, reply_headers, "")
            }
            method => sunshine_host(method, &request.target),
        };
        let raw = String::from_utf8(raw).unwrap();
        let raw = raw.replacen("CSeq: 1\r\n", &format!("CSeq: {cseq}\r\n"), 1);
        if let Err(error) =
            handshake.handle_plain_response(&parse_response(raw.as_bytes()).unwrap())
        {
            return (announce, Err(error));
        }
    }
    (announce, handshake.finish())
}

#[test]
fn a_confirmed_dolby_offer_streams_unchunked_frames() {
    for (audio, offer, reply, codec) in [
        (
            AudioConfiguration::SURROUND_51,
            "AC3",
            "X-SS-Audio-Codec: AC3\r\n",
            CODEC_AC3,
        ),
        (
            AudioConfiguration::STEREO,
            "EAC3",
            "X-SS-Audio-Codec: EAC3\r\n",
            CODEC_EAC3,
        ),
        (
            AudioConfiguration::SURROUND_51,
            "eac3",
            "x-ss-audio-codec:  eac3 \r\n",
            CODEC_EAC3,
        ),
    ] {
        let (announce, outcome) = negotiate(audio, ALL, &offering(offer), reply);
        let outcome = outcome.unwrap();
        // The offer's token, exactly, once; Rubylight's own key never goes to such a host.
        let line = format!("a=x-ss-audio-codec-accept:{offer} \r\n");
        assert_eq!(announce.matches(&line).count(), 1, "{announce}");
        assert_eq!(announce.matches("x-ss-audio-codec-accept").count(), 1);
        assert!(!announce.contains("x-rl-audio.codec"), "{announce}");
        assert_eq!(
            (outcome.audio_codec, outcome.audio_unchunked),
            (codec, true),
            "{offer}"
        );
        // The host steps its timestamps by one 32 ms frame, as the session's packet duration.
        let channels = outcome.config.audio.channel_count;
        assert_eq!(crate::audio::passthrough::packet_ms(codec, channels, 5), 32);
    }
}

#[test]
fn without_a_confirmation_of_the_accepted_codec_the_stream_keeps_opus() {
    for reply in [
        "",
        "X-SS-Audio-Codec: Opus\r\n",
        "X-SS-Audio-Codec: opus\r\n",
        "X-SS-Audio-Codec: TrueHD\r\n",
    ] {
        let (announce, outcome) = negotiate(
            AudioConfiguration::SURROUND_51,
            ALL,
            &offering("AC3"),
            reply,
        );
        assert!(announce.contains("a=x-ss-audio-codec-accept:AC3 \r\n"));
        let outcome = outcome.unwrap();
        assert_eq!(
            (outcome.audio_codec, outcome.audio_unchunked),
            (0, false),
            "{reply:?}"
        );
    }
    // The other Dolby codec, or picks that disagree: the stream cannot start as set up.
    for reply in [
        "X-SS-Audio-Codec: EAC3\r\n",
        "X-SS-Audio-Codec: AC3\r\nX-SS-Audio-Codec: Opus\r\n",
    ] {
        let (_, outcome) = negotiate(
            AudioConfiguration::SURROUND_51,
            ALL,
            &offering("AC3"),
            reply,
        );
        assert!(
            matches!(outcome, Err(RtspError::Malformed(_))),
            "{reply:?}: {outcome:?}"
        );
    }
}

#[test]
fn nothing_is_accepted_without_passthrough_an_output_that_plays_it_or_stereo_or_5_1() {
    let confirm = "X-SS-Audio-Codec: AC3\r\n";
    for (audio, passthrough, offer) in [
        (AudioConfiguration::SURROUND_51, 0, "AC3"),
        (AudioConfiguration::SURROUND_51, CODEC_PCM, "AC3"),
        (AudioConfiguration::SURROUND_51, CODEC_EAC3, "AC3"),
        (AudioConfiguration::SURROUND_71, ALL, "AC3"),
        (AudioConfiguration::SURROUND_714, ALL, "EAC3"),
        (AudioConfiguration::SURROUND_51, ALL, "AC-3"),
    ] {
        let (with_offer, outcome) = negotiate(audio, passthrough, &offering(offer), confirm);
        // The same bytes as without the offer, and a stray confirmation changes nothing.
        let (without, _) = negotiate(audio, passthrough, &stock_describe(), "");
        assert_eq!(with_offer, without, "{audio:?} {passthrough} {offer}");
        assert!(!with_offer.contains("x-ss-audio-codec-accept"));
        let outcome = outcome.unwrap();
        assert_eq!((outcome.audio_codec, outcome.audio_unchunked), (0, false));
    }
}

#[test]
fn rubylights_own_codec_negotiation_wins_with_unchanged_bytes() {
    let rubylight = format!("{DESCRIBE}a=x-rl-audio.codecs:15\r\n");
    for audio in [AudioConfiguration::STEREO, AudioConfiguration::SURROUND_51] {
        for passthrough in [CODEC_AC3 | CODEC_EAC3, CODEC_AC3] {
            let (plain, outcome) = negotiate(audio, passthrough, &rubylight, "");
            let both = format!("{rubylight}a=x-ss-audio-codec-offer:AC3\r\n");
            // A host that also offered the dialect, and confirmed it, gets the same ANNOUNCE.
            let (with_offer, both_outcome) =
                negotiate(audio, passthrough, &both, "X-SS-Audio-Codec: AC3\r\n");
            assert_eq!(plain, with_offer);
            assert!(plain.contains("a=x-rl-audio.codec:"), "{plain}");
            assert!(!plain.contains("x-ss-audio-codec-accept"));
            let (outcome, both_outcome) = (outcome.unwrap(), both_outcome.unwrap());
            assert!(!outcome.audio_unchunked && !both_outcome.audio_unchunked);
            assert_eq!(outcome.audio_codec, both_outcome.audio_codec);
            let expected = if passthrough & CODEC_EAC3 != 0 {
                CODEC_EAC3
            } else {
                CODEC_AC3
            };
            assert_eq!(outcome.audio_codec, expected);
        }
    }
}

#[test]
fn stock_hosts_get_the_same_announce_with_passthrough_on_or_off() {
    for audio in [
        AudioConfiguration::STEREO,
        AudioConfiguration::SURROUND_51,
        AudioConfiguration::SURROUND_71,
        AudioConfiguration::SURROUND_714,
    ] {
        let (off, outcome) = negotiate(audio, 0, &stock_describe(), "");
        let (on, on_outcome) = negotiate(audio, ALL, &stock_describe(), "");
        assert_eq!(off, on, "{audio:?}");
        let (mut outcome, on_outcome) = (outcome.unwrap(), on_outcome.unwrap());
        // Only the passthrough the config asked for differs.
        outcome.config.audio_passthrough = ALL;
        assert_eq!(outcome, on_outcome);
        assert_eq!((on_outcome.audio_codec, on_outcome.audio_unchunked), (0, false));
    }
}
