//! NVIDIA image enhancements a host built with them offers: RTX HDR (an SDR
//! game turned into HDR before encoding) and RTX Video enhancement (NVIDIA's
//! video super resolution at the capture's own size, which cleans up and
//! sharpens the picture before encoding).
//!
//! Only Rubylight's separate NVIDIA build has them, and only on a GPU and
//! driver whose runtime reports them available. Such a host lists
//! `NvidiaEnhancements` in serverinfo, a mask of [`RTX_HDR`] and
//! [`RTX_VIDEO`]; every other host, and stock Moonlight, sees nothing. A
//! client may then ask for each one for a stream with a launch parameter,
//! [`RTX_HDR_PARAMETER`] or [`RTX_VIDEO_PARAMETER`], `1` on and `0` off; the
//! stream then follows the client instead of the app's setting. Without the
//! parameter the app's setting applies.

/// The serverinfo element: a decimal mask of the enhancements offered.
pub const SERVERINFO_FIELD: &str = "NvidiaEnhancements";
/// RTX HDR: an SDR picture turned into HDR for an HDR stream.
pub const RTX_HDR: u8 = 1;
/// RTX Video enhancement of SDR and HDR pictures.
pub const RTX_VIDEO: u8 = 2;
/// Every enhancement bit this version knows.
pub const ALL: u8 = RTX_HDR | RTX_VIDEO;

/// The `/launch` and `/resume` parameter for RTX HDR.
pub const RTX_HDR_PARAMETER: &str = "rlRtxHdr";
/// The `/launch` and `/resume` parameter for RTX Video enhancement.
pub const RTX_VIDEO_PARAMETER: &str = "rlRtxVideo";

/// The offered mask from a serverinfo value: unknown bits are dropped, and a
/// missing or malformed value offers nothing.
pub fn parse_offer(value: Option<&str>) -> u8 {
    value
        .and_then(|value| value.trim().parse::<u32>().ok())
        .map_or(0, |mask| (mask & u32::from(ALL)) as u8)
}

/// A launch parameter's choice: `Some(true)` for `1`, `Some(false)` for `0`,
/// `None` (follow the app) for anything else or no parameter.
pub fn launch_choice(value: Option<&str>) -> Option<bool> {
    match value.map(str::trim) {
        Some("1") => Some(true),
        Some("0") => Some(false),
        _ => None,
    }
}

/// The launch parameters a client sends for its choices, in a fixed order,
/// only for what `offered` holds: a client never sends them to a host that
/// does not list the enhancement, and sends nothing for "follow the app".
pub fn launch_parameters(
    offered: u8,
    rtx_hdr: Option<bool>,
    rtx_video: Option<bool>,
) -> Vec<(&'static str, &'static str)> {
    let mut parameters = Vec::new();
    for (bit, parameter, choice) in [
        (RTX_HDR, RTX_HDR_PARAMETER, rtx_hdr),
        (RTX_VIDEO, RTX_VIDEO_PARAMETER, rtx_video),
    ] {
        if offered & bit == 0 {
            continue;
        }
        if let Some(on) = choice {
            parameters.push((parameter, if on { "1" } else { "0" }));
        }
    }
    parameters
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_offer_keeps_only_known_bits() {
        assert_eq!(parse_offer(None), 0);
        assert_eq!(parse_offer(Some("")), 0);
        assert_eq!(parse_offer(Some("x")), 0);
        assert_eq!(parse_offer(Some("-1")), 0);
        assert_eq!(parse_offer(Some("1")), RTX_HDR);
        assert_eq!(parse_offer(Some("2")), RTX_VIDEO);
        assert_eq!(parse_offer(Some(" 3 ")), ALL);
        assert_eq!(parse_offer(Some("255")), ALL);
    }

    #[test]
    fn launch_choices_are_one_zero_or_follow_the_app() {
        assert_eq!(launch_choice(Some("1")), Some(true));
        assert_eq!(launch_choice(Some("0")), Some(false));
        assert_eq!(launch_choice(Some("true")), None);
        assert_eq!(launch_choice(Some("")), None);
        assert_eq!(launch_choice(None), None);
    }

    #[test]
    fn parameters_go_only_to_a_host_that_offers_them() {
        assert!(launch_parameters(0, Some(true), Some(true)).is_empty());
        assert!(launch_parameters(ALL, None, None).is_empty());
        assert_eq!(
            launch_parameters(ALL, Some(true), Some(false)),
            [(RTX_HDR_PARAMETER, "1"), (RTX_VIDEO_PARAMETER, "0")]
        );
        assert_eq!(
            launch_parameters(RTX_VIDEO, Some(true), Some(true)),
            [(RTX_VIDEO_PARAMETER, "1")]
        );
    }
}
