//! Long-term reference recovery on H.264 and HEVC (Experimental): the client asks with the
//! `/launch` and `/resume` query parameter [`LAUNCH_PARAMETER`]`=1`.
//!
//! A host recovers a lost AV1 frame by reference invalidation on its own. On H.264 and HEVC it
//! answers an invalidation with a keyframe unless the client sends this parameter: some
//! decoders froze on the long-term reference an AMD encoder names after a loss. Any other
//! value, or none, keeps the keyframe. Hosts that don't know the parameter ignore it.

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
