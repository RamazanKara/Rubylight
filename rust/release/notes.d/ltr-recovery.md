# LTR recovery on H.264 and HEVC
- Experimental: a client that adds `rlLtrRecovery=1` to its launch request gets long-term reference recovery on H.264 and HEVC, as AV1 has: after a lost frame the host re-references an older frame instead of waiting for a keyframe. Stock clients are unchanged.
