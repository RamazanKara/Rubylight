//! Microphone framing negotiation constants shared by Rubylight host and Nightjar clients.
//!
//! Legacy Apollo/Moonlight clients double-pad AES-128-CBC microphone packets (an inner
//! block-alignment pad plus an outer PKCS#7 block), which makes 16-byte-aligned Opus frames
//! ending in valid PKCS#7 bytes indistinguishable from inner padding. Rubylight advertises
//! single-PKCS#7 framing on `SETUP streamid=mic` via [`FRAMING_HEADER`] and clients opt in via
//! [`FRAMING_ATTRIBUTE`] in `ANNOUNCE`, keeping unnegotiated legacy clients 100% compatible.

/// RTSP `SETUP streamid=mic` response header advertising single-PKCS#7 microphone framing.
pub const FRAMING_HEADER: &str = "X-RL-Mic-Framing";
/// RTSP `ANNOUNCE` SDP attribute selecting the microphone payload framing mode.
pub const FRAMING_ATTRIBUTE: &str = "x-rl-mic.framing";
/// Single standard PKCS#7 AES-128-CBC padding (no inner pre-padding, no heuristic strip).
pub const FRAMING_SINGLE_PKCS7: u8 = 1;

/// Whether an RTSP `X-RL-Mic-Framing` header value offers [`FRAMING_SINGLE_PKCS7`].
pub fn offers_single_pkcs7(value: &str) -> bool {
    value.trim().parse::<u8>() == Ok(FRAMING_SINGLE_PKCS7)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_pkcs7_offer_parser_accepts_only_mode_one() {
        assert!(offers_single_pkcs7("1"));
        assert!(offers_single_pkcs7(" 1 "));
        assert!(!offers_single_pkcs7("0"));
        assert!(!offers_single_pkcs7("2"));
        assert!(!offers_single_pkcs7(""));
        assert!(!offers_single_pkcs7("pkcs7"));
    }
}
