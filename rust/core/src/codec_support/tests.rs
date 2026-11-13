use super::*;

/// A fake probe for one GPU: the codecs its engine encodes succeed, the
/// rest fail with the driver's text.
fn pass(_adapter: &str, found: &[Codec]) -> Vec<(Codec, Outcome)> {
    Codec::ALL
        .iter()
        .map(|c| {
            (
                *c,
                if found.contains(c) {
                    Outcome::Available
                } else {
                    Outcome::Failed("AMF_NOT_SUPPORTED (10)".into())
                },
            )
        })
        .collect()
}
const H264: Codec = Codec::new(0, false, false);
const HEVC: Codec = Codec::new(1, false, false);
const HEVC10: Codec = Codec::new(1, true, false);
const AV1: Codec = Codec::new(2, false, false);
const AV110: Codec = Codec::new(2, true, false);

#[test]
fn names_map_to_engines() {
    for (name, want) in [
        ("AMD Radeon RX 580 Series", Engine::Vce),
        ("Radeon RX 480 Graphics", Engine::Vce),
        ("Radeon RX Vega 64", Engine::Vce),
        ("Radeon Vega 8 Graphics", Engine::Vce),
        ("AMD Radeon RX 5700 XT", Engine::Vcn2),
        ("AMD Radeon RX 5500 XT", Engine::Vcn2),
        ("AMD Radeon RX 6800 XT", Engine::Vcn3),
        ("AMD Radeon RX 6600", Engine::Vcn3),
        ("AMD Radeon RX 6400", Engine::DecodeOnly),
        ("AMD Radeon RX 6500 XT", Engine::DecodeOnly),
        ("AMD Radeon(TM) 680M", Engine::Vcn3),
        ("AMD Radeon RX 7900 XT", Engine::Vcn4),
        ("AMD Radeon RX 7600", Engine::Vcn4),
        ("AMD Radeon(TM) 780M Graphics", Engine::Vcn4),
        ("AMD Radeon RX 9070 XT", Engine::Vcn5),
        ("AMD Radeon RX 9060 XT", Engine::Vcn5),
        ("AMD Radeon(TM) Graphics", Engine::Unknown),
        ("AMD Radeon Pro W6800", Engine::Unknown),
    ] {
        assert_eq!(engine(name), want, "{name}");
    }
}

#[test]
fn rdna1_lacks_av1_and_explains_it() {
    let name = "AMD Radeon RX 5700 XT";
    let missing = explain_all(name, &pass(name, &[H264, HEVC, HEVC10]));
    assert_eq!(missing.len(), 2);
    assert_eq!(missing[0].codec, AV1);
    assert!(missing[0].reason.contains("no AV1 encoder"));
    assert!(missing[0].reason.contains("VCN 2.0"));
    assert!(missing[0].reason.contains("AMF_NOT_SUPPORTED"));
    assert_eq!(missing[1].codec, AV110);
    assert!(warning(&missing).unwrap().contains("AV1"));
}

#[test]
fn rdna2_lacks_av1() {
    let name = "AMD Radeon RX 6800 XT";
    let missing = explain_all(name, &pass(name, &[H264, HEVC, HEVC10]));
    assert!(missing.iter().all(|m| m.codec.id == 2));
    assert!(missing[0].reason.contains("VCN 3.x"));
}

#[test]
fn rdna3_and_rdna4_fail_loudly_when_a_supported_codec_fails() {
    for name in ["AMD Radeon RX 7900 XT", "AMD Radeon RX 9070 XT"] {
        let missing = explain_all(name, &pass(name, &[H264, HEVC, HEVC10, AV1]));
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].codec, AV110);
        assert!(missing[0].reason.contains("should support it"), "{name}");
        assert!(missing[0].reason.contains("Update the Radeon driver"));
    }
}

#[test]
fn all_codecs_present_means_no_warning() {
    let name = "AMD Radeon RX 7900 XT";
    let missing = explain_all(name, &pass(name, &Codec::ALL));
    assert!(missing.is_empty());
    assert_eq!(warning(&missing), None);
}

#[test]
fn navi24_has_no_encoder_at_all() {
    let name = "AMD Radeon RX 6400";
    let missing = explain_all(name, &pass(name, &[]));
    assert_eq!(missing.len(), 5);
    assert!(missing[0].reason.contains("no H.264 encoder"));
    assert!(missing[0].reason.contains("Navi 24"));
}

#[test]
fn legacy_and_unknown_engines_report_the_driver_text_only() {
    for name in ["AMD Radeon RX 580 Series", "AMD Radeon(TM) Graphics"] {
        let missing = explain_all(name, &pass(name, &[H264, HEVC]));
        let hdr = missing.iter().find(|m| m.codec == HEVC10).unwrap();
        assert!(hdr.reason.contains("AMF driver refused"), "{name}");
        let av1 = missing.iter().find(|m| m.codec == AV1).unwrap();
        // Only a known VCE part is told it has no AV1; an unidentified GPU
        // gets the driver's text, since it may be a new one.
        if engine(name) == Engine::Vce {
            assert!(av1.reason.contains("no AV1 encoder"), "{name}");
        } else {
            assert!(av1.reason.contains("AMF driver refused"), "{name}");
            assert_eq!(expected(engine(name), AV1), None, "{name}");
        }
    }
}

#[test]
fn a_setting_is_not_a_warning() {
    let name = "AMD Radeon RX 7900 XT";
    let outcomes: Vec<_> = Codec::ALL
        .iter()
        .map(|c| {
            (
                *c,
                if c.id == 2 {
                    Outcome::Disabled
                } else {
                    Outcome::Available
                },
            )
        })
        .collect();
    let missing = explain_all(name, &outcomes);
    assert_eq!(missing.len(), 2);
    assert!(missing[0].reason.contains("turned off"));
    assert_eq!(warning(&missing), None);
}

#[test]
fn launch_refusal_names_the_probe_reason() {
    let name = "AMD Radeon RX 6800 XT";
    let missing = explain_all(name, &pass(name, &[H264, HEVC, HEVC10]));
    assert!(refusal(&missing, AV1).contains("no AV1 encoder"));
    // A codec the probe never tried (for example a later pass) still gets
    // a sentence, not an empty reason.
    assert!(refusal(&[], AV1).contains("AV1 was not found"));
}

#[test]
fn negotiated_refusals_are_specific() {
    let name = "AMD Radeon RX 6800 XT";
    let missing = explain_all(name, &pass(name, &[H264, HEVC, HEVC10]));
    assert!(refusal_for(&missing, 2, false, false).contains("no AV1 encoder"));
    assert!(refusal_for(&missing, 2, true, false).contains("AV1 10-bit HDR"));
    assert!(refusal_for(&missing, 0, true, false).contains("no 10-bit"));
    assert!(refusal_for(&missing, 1, false, true).contains("4:4:4"));
    assert!(refusal_for(&missing, 3, false, false).contains("host console"));
}

#[test]
fn amf_versions_print_and_retry_only_for_an_older_runtime() {
    let header = (1u64 << 48) | (5 << 32) | (2 << 16);
    let old = (1u64 << 48) | (4 << 32) | (29 << 16) | 1;
    assert_eq!(amf_version_text(header), "1.5.2.0");
    assert_eq!(amf_version_text(old), "1.4.29.1");
    assert_eq!(amf_retry_version(header, Some(old)), Some(old));
    assert_eq!(amf_retry_version(header, Some(header)), None);
    assert_eq!(amf_retry_version(header, Some(header + (1 << 32))), None);
    assert_eq!(amf_retry_version(header, Some(0)), None);
    assert_eq!(amf_retry_version(header, None), None);
    // A different major version is not assumed compatible.
    assert_eq!(amf_retry_version(header, Some(1 << 40)), None);
}

#[test]
fn bits_match_the_advertised_flags() {
    let bits: Vec<u32> = Codec::ALL.iter().map(|c| c.bit()).collect();
    assert_eq!(bits, [1, 0x100, 0x200, 0x10000, 0x20000]);
    // moonlight-common-c Limelight.h: SCM_H264_HIGH8_444,
    // SCM_HEVC_REXT8_444, SCM_HEVC_REXT10_444, SCM_AV1_HIGH8_444 and
    // SCM_AV1_HIGH10_444, the bits `rtsp_server` checks a 4:4:4 ANNOUNCE
    // against.
    let bits: Vec<u32> = Codec::NATIVE_444.iter().map(|c| c.bit()).collect();
    assert_eq!(bits, [0x40000, 0x80000, 0x100000, 0x200000, 0x400000]);
    assert_eq!(Codec::new(0, true, true).bit(), 0);
    for codec in Codec::NATIVE_444 {
        assert!(codec.label().ends_with("4:4:4"), "{codec:?}");
        assert_eq!(codec.chroma_420().bit() & codec.bit(), 0);
        assert!(Codec::ALL.contains(&codec.chroma_420()));
    }
}

#[test]
fn the_console_names_the_advertised_444_modes_only() {
    assert!(native_444_labels(0x1 | 0x100 | 0x200 | 0x10000 | 0x20000).is_empty());
    assert_eq!(
        native_444_labels(0x1 | 0x100 | 0x200 | 0x40000 | 0x80000 | 0x100000 | 0x40000000),
        ["H.264 4:4:4", "HEVC 4:4:4", "HEVC 10-bit 4:4:4"]
    );
    // PyroWave's 4:4:4 bits (0x1000000, 0x4000000) are not native ones.
    assert!(native_444_labels(0x0780_0000).is_empty());
}

const NO_HEVC_444: &str = "this NVIDIA GPU's NVENC reports no HEVC 4:4:4 encoding";
const NO_AV1_444: &str = "NVENC SDK 13.0 has no AV1 4:4:4 mode";

#[test]
fn a_missing_444_mode_is_explained_by_the_encoder_and_never_warns() {
    let name = "NVIDIA GeForce RTX 5070";
    let outcomes = [
        (Codec::NATIVE_444[0], Outcome::Available),
        (Codec::NATIVE_444[1], Outcome::Failed(NO_HEVC_444.into())),
        (Codec::NATIVE_444[3], Outcome::Failed(NO_AV1_444.into())),
    ];
    let missing = explain_all(name, &outcomes);
    assert_eq!(missing.len(), 2);
    assert_eq!(
        missing[0].reason,
        format!("HEVC 4:4:4 is not available: {NO_HEVC_444}")
    );
    assert!(
        missing[1]
            .reason
            .starts_with("AV1 4:4:4 is not available: NVENC SDK")
    );
    assert_eq!(warning(&missing), None);
    // AMD's table never words a 4:4:4 gap as a driver fault.
    let missing = explain_all(
        "AMD Radeon RX 7900 XT",
        &[(Codec::NATIVE_444[1], Outcome::Failed(AMF_NATIVE_444.into()))],
    );
    assert!(missing[0].reason.contains("supports only 4:2:0"));
    assert!(!missing[0].reason.contains("Update the Radeon driver"));
    assert_eq!(warning(&missing), None);
}

#[test]
fn a_444_launch_is_refused_with_the_probe_reason_or_a_plain_sentence() {
    let missing = explain_all(
        "NVIDIA GeForce RTX 5070",
        &[(Codec::NATIVE_444[2], Outcome::Failed(NO_HEVC_444.into()))],
    );
    assert!(refusal_for(&missing, 1, true, true).contains("reports no HEVC 4:4:4"));
    // Not probed (the 4:2:0 mode is missing): the sentence names the mode
    // and what the device can do instead.
    let unprobed = refusal_for(&missing, 2, false, true);
    assert!(
        unprobed.starts_with("AV1 4:4:4 is not advertised"),
        "{unprobed}"
    );
    assert!(unprobed.contains("turn off YUV 4:4:4"), "{unprobed}");
    // H.264 has no 10-bit mode in any chroma.
    assert!(refusal_for(&missing, 0, true, true).contains("no 10-bit"));
    // 4:2:0 refusals keep their own reasons.
    assert!(refusal_for(&missing, 1, true, false).contains("HEVC 10-bit HDR was not found"));
}
