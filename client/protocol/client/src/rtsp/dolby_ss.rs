//! Dolby AC3 and E-AC3 audio negotiated with the `x-ss-audio` RTSP lines, as hosts without
//! Rubylight's `a=x-rl-audio.codecs` offer it: the DESCRIBE offers a codec
//! (`a=x-ss-audio-codec-offer:AC3`), the ANNOUNCE accepts exactly that token
//! (`a=x-ss-audio-codec-accept:AC3`), and the ANNOUNCE reply's `X-SS-Audio-Codec` header names
//! what the host picked (`AC3`, `EAC3` or `Opus`). Only a confirmed AC3 or E-AC3 streams Dolby:
//! one whole 32 ms frame per RTP packet, without Rubylight's chunk header
//! (`audio::AudioReceiver::unchunked_frames`). Anything else keeps Opus. The hosts take it for
//! stereo and 5.1 only and refuse the stream when a client accepts it for another layout.
//!
//! This is Vibepollo's dialect (`src/rtsp.cpp` and `src/audio.cpp` of Nonary/Vibepollo): its
//! DESCRIBE offers one configured codec, it compares the accept with that offer verbatim and
//! answers `AC3`, `EAC3` or `Opus`. Written from that public source; no code is ported.

use rubylight_protocol::audio_ext::{CODEC_AC3, CODEC_EAC3};

use super::RtspError;
use super::message::RtspResponse;

/// The DESCRIBE attribute naming the Dolby codec the host offers.
pub const OFFER_ATTRIBUTE: &str = "x-ss-audio-codec-offer";
/// The ANNOUNCE attribute accepting it, with the offer's token.
pub const ACCEPT_ATTRIBUTE: &str = "x-ss-audio-codec-accept";
/// The ANNOUNCE reply header naming the codec the host streams.
pub const CONFIRM_HEADER: &str = "X-SS-Audio-Codec";
/// A longer offer is no offer: the hosts name one codec.
const MAX_OFFER_BYTES: usize = 256;
/// The most tokens of an offer looked at.
const MAX_OFFER_TOKENS: usize = 16;
/// The only layouts the hosts stream Dolby in: stereo and 5.1.
const DOLBY_CHANNELS: [u8; 2] = [2, 6];

/// The `audio_ext::CODEC_*` bit a token names, matched without regard to case.
fn codec_of(token: &str) -> Option<u8> {
    if token.eq_ignore_ascii_case("AC3") {
        Some(CODEC_AC3)
    } else if token.eq_ignore_ascii_case("EAC3") {
        Some(CODEC_EAC3)
    } else {
        None
    }
}

/// A DESCRIBE's offer: the Dolby codecs it names, each in the host's own spelling, which the
/// ANNOUNCE repeats as is (the hosts compare it verbatim).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Offer {
    ac3: Option<String>,
    eac3: Option<String>,
}

impl Offer {
    /// The offer in an `a=x-ss-audio-codec-offer` value: tokens split by `|`, `,` or blanks,
    /// unknown ones skipped. `None` when it names no AC3 or E-AC3, or is longer than any host
    /// sends.
    pub fn parse(value: &str) -> Option<Self> {
        if value.len() > MAX_OFFER_BYTES {
            return None;
        }
        let mut offer = Self::default();
        let tokens = value
            .split(['|', ',', ' ', '\t'])
            .filter(|token| !token.is_empty())
            .take(MAX_OFFER_TOKENS);
        for token in tokens {
            let slot = match codec_of(token) {
                Some(CODEC_AC3) => &mut offer.ac3,
                Some(_) => &mut offer.eac3,
                None => continue,
            };
            // The first spelling of a codec counts. A known token is ASCII letters and digits,
            // so it cannot break the ANNOUNCE line it is sent back in.
            slot.get_or_insert_with(|| token.to_owned());
        }
        (offer.codecs() != 0).then_some(offer)
    }

    /// The `audio_ext::CODEC_*` bits offered: AC3, E-AC3 or both.
    pub fn codecs(&self) -> u8 {
        (if self.ac3.is_some() { CODEC_AC3 } else { 0 })
            | (if self.eac3.is_some() { CODEC_EAC3 } else { 0 })
    }
}

/// What the ANNOUNCE accepted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Accept {
    /// One `audio_ext::CODEC_*` bit: AC3 or E-AC3.
    pub codec: u8,
    /// The offer's token for it, sent back as is.
    pub token: String,
}

/// The codec to accept, or `None` to send nothing and stream Opus: one the host offered that
/// `output` (the `audio_ext::CODEC_*` bits the platform's audio output plays for these
/// channels, 0 with passthrough off) takes, E-AC3 first as in `passthrough::pick_codec`, and
/// only for stereo or 5.1.
pub fn pick(offer: Option<&Offer>, output: u8, channels: u8) -> Option<Accept> {
    let offer = offer?;
    if !DOLBY_CHANNELS.contains(&channels) {
        return None;
    }
    [(CODEC_EAC3, &offer.eac3), (CODEC_AC3, &offer.ac3)]
        .into_iter()
        .find_map(|(codec, token)| {
            let token = token.as_ref().filter(|_| output & codec != 0)?;
            Some(Accept {
                codec,
                token: token.clone(),
            })
        })
}

/// The codec the host confirmed for `accepted` in its ANNOUNCE reply: `accepted.codec`, or 0
/// for Opus when the reply names none, Opus, or a codec this dialect does not know. A reply that
/// names the other Dolby codec, or repeats the header with different picks, is an error: the
/// host would stream audio the client did not set up for.
pub fn confirm(accepted: &Accept, response: &RtspResponse) -> Result<u8, RtspError> {
    let mut picks = response
        .options
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case(CONFIRM_HEADER))
        .map(|(_, value)| codec_of(value.trim_matches([' ', '\t'])).unwrap_or(0));
    let Some(pick) = picks.next() else {
        return Ok(0);
    };
    if picks.any(|other| other != pick) {
        return Err(RtspError::Malformed(format!(
            "ANNOUNCE: conflicting {CONFIRM_HEADER} headers"
        )));
    }
    if pick == 0 || pick == accepted.codec {
        return Ok(pick);
    }
    let name = |codec| if codec == CODEC_AC3 { "AC3" } else { "E-AC3" };
    Err(RtspError::Malformed(format!(
        "ANNOUNCE: the host confirmed {} audio, but {} was accepted",
        name(pick),
        name(accepted.codec)
    )))
}

#[cfg(test)]
mod tests;
