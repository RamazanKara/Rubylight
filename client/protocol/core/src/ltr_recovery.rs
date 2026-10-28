//! Long-term reference recovery on H.264 and HEVC: the client asks with the `/launch` and
//! `/resume` query parameter [`LAUNCH_PARAMETER`]`=1`.
//!
//! Released hosts don't need it: Rubylight 3.0.0 recovers a lost H.264, HEVC or AV1 frame by
//! reference invalidation for every client, rewriting the headers a decoder that dropped
//! frames until the repair needs. Only 3.0 development builds answered an H.264 or HEVC
//! invalidation with a keyframe unless the client sent it. Hosts that don't know the parameter
//! ignore it.

/// `/launch` and `/resume` query parameter; `1` asks for the recovery.
pub const LAUNCH_PARAMETER: &str = "rlLtrRecovery";

/// Whether a launch parameter value asks for the recovery: exactly `1`.
pub fn requested(value: Option<&str>) -> bool {
    value == Some("1")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_one_asks() {
        assert!(requested(Some("1")));
        for value in [None, Some(""), Some("0"), Some("true"), Some("01"), Some(" 1")] {
            assert!(!requested(value), "{value:?}");
        }
    }
}
