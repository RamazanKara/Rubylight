//! Unit and hostile-input tests for the `x-ss-audio` Dolby negotiation lines.

use super::*;
use crate::rtsp::parse_response;
use rubylight_protocol::audio_ext::CODEC_PCM;

const BOTH: u8 = CODEC_AC3 | CODEC_EAC3;

fn accept(codec: u8, token: &str) -> Accept {
    Accept {
        codec,
        token: token.into(),
    }
}

fn reply(headers: &str) -> RtspResponse {
    parse_response(format!("RTSP/1.0 200 OK\r\nCSeq: 7\r\n{headers}\r\n").as_bytes()).unwrap()
}

#[test]
fn an_offer_names_its_codecs_in_the_hosts_spelling() {
    let ac3 = Offer::parse("AC3").unwrap();
    assert_eq!(ac3.codecs(), CODEC_AC3);
    assert_eq!(pick(Some(&ac3), BOTH, 6), Some(accept(CODEC_AC3, "AC3")));
    let eac3 = Offer::parse("EAC3").unwrap();
    assert_eq!(pick(Some(&eac3), BOTH, 2), Some(accept(CODEC_EAC3, "EAC3")));
    // Any case, sent back as the host wrote it.
    let lower = Offer::parse("eac3").unwrap();
    assert_eq!(
        pick(Some(&lower), BOTH, 6),
        Some(accept(CODEC_EAC3, "eac3"))
    );
    // A list, unknown tokens and repeats skipped; the first spelling counts.
    let list = Offer::parse("PCM|Ac3, TRUEHD eac3 AC3\tEAC3").unwrap();
    assert_eq!(list.codecs(), BOTH);
    assert_eq!(list.ac3.as_deref(), Some("Ac3"));
    assert_eq!(list.eac3.as_deref(), Some("eac3"));
}

#[test]
fn offers_naming_no_dolby_codec_are_none() {
    for value in [
        "", "   ", "|,|", "Opus", "PCM", "AC-3", "E-AC-3", "AC3X", "AAC3", "AC3;EAC3", "AC３",
        "ＡC3", "\u{0}AC3",
    ] {
        assert_eq!(Offer::parse(value), None, "{value:?}");
    }
}

#[test]
fn oversized_offers_are_ignored_and_only_the_first_tokens_count() {
    let long = format!("AC3|{}", "x".repeat(MAX_OFFER_BYTES));
    assert_eq!(Offer::parse(&long), None);
    let at_limit = format!("{}AC3", " ".repeat(MAX_OFFER_BYTES - 3));
    assert_eq!(Offer::parse(&at_limit).map(|o| o.codecs()), Some(CODEC_AC3));
    let late = format!("{}EAC3", "x|".repeat(MAX_OFFER_TOKENS));
    assert_eq!(Offer::parse(&late), None);
    let in_time = format!("{}EAC3", "x|".repeat(MAX_OFFER_TOKENS - 1));
    assert_eq!(Offer::parse(&in_time).map(|o| o.codecs()), Some(CODEC_EAC3));
}

#[test]
fn the_pick_needs_an_offer_an_output_that_plays_it_and_stereo_or_5_1() {
    let both = Offer::parse("AC3|EAC3").unwrap();
    // E-AC3 first, as Rubylight's own pick.
    assert_eq!(
        pick(Some(&both), BOTH, 6).map(|a| a.codec),
        Some(CODEC_EAC3)
    );
    assert_eq!(
        pick(Some(&both), CODEC_AC3 | CODEC_PCM, 6).map(|a| a.codec),
        Some(CODEC_AC3)
    );
    assert_eq!(
        pick(Some(&both), CODEC_EAC3, 2).map(|a| a.codec),
        Some(CODEC_EAC3)
    );
    // Passthrough off, an output that plays neither, or no offer: nothing accepted.
    assert_eq!(pick(Some(&both), 0, 6), None);
    assert_eq!(pick(Some(&both), CODEC_PCM, 6), None);
    assert_eq!(
        pick(Some(&Offer::parse("AC3").unwrap()), CODEC_EAC3, 6),
        None
    );
    assert_eq!(pick(None, BOTH, 6), None);
    // Mono, quad, 7.1 and 7.1.4 keep Opus: the hosts refuse Dolby for them.
    for channels in [0, 1, 3, 4, 5, 7, 8, 12, 255] {
        assert_eq!(pick(Some(&both), BOTH, channels), None, "{channels}");
    }
}

#[test]
fn only_a_confirmation_of_the_accepted_codec_streams_dolby() {
    let ac3 = accept(CODEC_AC3, "AC3");
    assert_eq!(
        confirm(&ac3, &reply("X-SS-Audio-Codec: AC3\r\n")).unwrap(),
        CODEC_AC3
    );
    // Header names and values in any case, with blanks around the value.
    assert_eq!(
        confirm(&ac3, &reply("x-ss-audio-codec: ac3\r\n")).unwrap(),
        CODEC_AC3
    );
    assert_eq!(
        confirm(&ac3, &reply("X-SS-AUDIO-CODEC:  Ac3 \t\r\n")).unwrap(),
        CODEC_AC3
    );
    let eac3 = accept(CODEC_EAC3, "eac3");
    assert_eq!(
        confirm(&eac3, &reply("X-SS-Audio-Codec: EAC3\r\n")).unwrap(),
        CODEC_EAC3
    );
    // The same pick twice is still one pick.
    let twice = "X-SS-Audio-Codec: AC3\r\nX-SS-Audio-Codec: ac3\r\n";
    assert_eq!(confirm(&ac3, &reply(twice)).unwrap(), CODEC_AC3);
}

#[test]
fn a_missing_opus_or_unknown_confirmation_keeps_opus() {
    let ac3 = accept(CODEC_AC3, "AC3");
    for headers in [
        "",
        "X-SS-Audio-Codec: Opus\r\n",
        "X-SS-Audio-Codec: opus\r\n",
        "X-SS-Audio-Codec:\r\n",
        "X-SS-Audio-Codec: mat-live\r\n",
        "X-SS-Audio-Codec: AC3 EAC3\r\n",
        "X-SS-Audio-Codec: AC-3\r\n",
        "X-SS-Audio-Codecs: AC3\r\n",
        "X-SS-Audio: AC3\r\n",
    ] {
        assert_eq!(confirm(&ac3, &reply(headers)).unwrap(), 0, "{headers:?}");
    }
    let huge = format!("X-SS-Audio-Codec: {}\r\n", "A".repeat(64 * 1024));
    assert_eq!(confirm(&ac3, &reply(&huge)).unwrap(), 0);
}

#[test]
fn a_confirmation_of_another_codec_or_conflicting_picks_fail_visibly() {
    let ac3 = accept(CODEC_AC3, "AC3");
    let other = confirm(&ac3, &reply("X-SS-Audio-Codec: EAC3\r\n")).unwrap_err();
    assert!(
        other
            .to_string()
            .contains("confirmed E-AC3 audio, but AC3 was accepted"),
        "{other}"
    );
    for headers in [
        "X-SS-Audio-Codec: AC3\r\nX-SS-Audio-Codec: Opus\r\n",
        "X-SS-Audio-Codec: Opus\r\nX-SS-Audio-Codec: AC3\r\n",
        "X-SS-Audio-Codec: AC3\r\nX-SS-Audio-Codec: EAC3\r\n",
    ] {
        let error = confirm(&ac3, &reply(headers)).unwrap_err();
        assert!(
            error.to_string().contains("conflicting"),
            "{headers:?}: {error}"
        );
    }
    // The error names the codecs, never what the host wrote.
    let junk = confirm(
        &ac3,
        &reply("X-SS-Audio-Codec: eac3\r\nX-SS-Audio-Codec: <junk>\r\n"),
    )
    .unwrap_err()
    .to_string();
    assert!(!junk.contains("junk"), "{junk}");
}

#[test]
fn hostile_offers_and_replies_never_panic_and_accept_only_known_tokens() {
    // A fixed xorshift stream over an alphabet that hits the separators and the codec letters.
    const ALPHABET: &[u8] = b"AaCcEe3|, \t\r\n:-x\x00\xff";
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for _ in 0..4000 {
        let len = (next() % 40) as usize;
        let bytes: Vec<u8> = (0..len)
            .map(|_| ALPHABET[(next() % ALPHABET.len() as u64) as usize])
            .collect();
        let value = String::from_utf8_lossy(&bytes);
        if let Some(offer) = Offer::parse(&value) {
            let accepted = pick(Some(&offer), BOTH, 6).expect("an offer has a codec");
            assert!(
                codec_of(&accepted.token) == Some(accepted.codec),
                "{value:?}"
            );
            assert!(accepted.token.bytes().all(|b| b.is_ascii_alphanumeric()));
        }
        let headers = format!("X-SS-Audio-Codec: {}\r\n", value.replace(['\r', '\n'], ""));
        if let Ok(response) = parse_response(format!("RTSP/1.0 200 OK\r\n{headers}\r\n").as_bytes())
        {
            let _ = confirm(&accept(CODEC_EAC3, "EAC3"), &response);
        }
    }
}
