use super::*;
use crate::hdr_dynamic::StatsSmoother;

fn stats(min_pq: f32, mean_pq: f32, max_pq: f32) -> FrameStats {
    FrameStats {
        min_pq,
        mean_pq,
        max_pq,
        mean_nits: crate::hdr_dynamic::pq_to_nits(mean_pq),
        percentiles_pq: [mean_pq; 9],
    }
}

fn hdr10() -> Metadata {
    Metadata {
        maximum_nits: 1000,
        minimum: 50,
        max_cll: 800,
        max_fall: 180,
        ..Metadata::default()
    }
}

fn frame() -> Frame {
    Frame {
        level1: Level1::clamped(0, 1600, 3000),
        target_nits: 600,
        metadata: hdr10(),
        scene_refresh: true,
    }
}

fn unescape(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut zeros = 0;
    for &byte in data {
        if zeros == 2 && byte == 3 {
            zeros = 0;
            continue;
        }
        out.push(byte);
        zeros = if byte == 0 { zeros + 1 } else { 0 };
    }
    out
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[test]
fn only_hdr_hevc_streams_that_picked_profile_8_1_carry_rpus() {
    use rubylight_protocol::hdr;
    let stream = |codec, hdr, hdr_format| crate::rtsp::Negotiated {
        codec,
        hdr,
        hdr_format,
        ..Default::default()
    };
    assert!(negotiated(&stream(1, true, hdr::DV_81)));
    assert!(!negotiated(&stream(1, true, 0)), "stock clients pick none");
    assert!(!negotiated(&stream(1, true, hdr::PQ)));
    assert!(!negotiated(&stream(1, true, hdr::DV_84)), "8.4 needs HLG");
    assert!(!negotiated(&stream(2, true, hdr::DV_81)), "AV1");
    assert!(!negotiated(&stream(1, false, hdr::DV_81)), "SDR");
}

#[test]
fn the_crc_is_crc32_mpeg2() {
    // The catalogue check value of CRC-32/MPEG-2.
    assert_eq!(crc32_mpeg2(b"123456789"), 0x0376_E6E7);
    assert_eq!(crc32_mpeg2(&[]), u32::MAX);
}

#[test]
fn an_rpu_has_the_profile_8_prefix_its_crc_and_the_final_byte() {
    let rpu = frame().rpu();
    // 0x19, then rpu_type 2 and rpu_format 18: the bytes RPU parsers look for.
    assert_eq!(&rpu[..3], &[0x19, 0x08, 0x09]);
    assert_eq!(rpu.last(), Some(&0x80));
    let crc_at = rpu.len() - 5;
    assert_eq!(
        crc32_mpeg2(&rpu[1..crc_at]).to_be_bytes(),
        rpu[crc_at..rpu.len() - 1]
    );
}

#[test]
fn an_rpu_is_byte_exact_for_the_same_frame() {
    // Checked with dovi_tool's `dolby_vision` 3.4.0: `DoviRpu::parse_unspec62_nalu`
    // reads profile 8, a matching CRC, source 62..3079, scene refresh, L1
    // 0/1600/3000, L2 target 2851 with neutral trims and L6 1000/50/800/180, and
    // its writer writes these bytes back unchanged.
    assert_eq!(hex(&frame().nal()), GOLDEN);
}

const GOLDEN: &str = concat!(
    "000000017c0119080908406136506f003ff801ffc00fffd0000008000006800000400000340000030200",
    "000301a2566000035ea2566f9fceb1c256644ca00000100000030080000003008000000301c362243018",
    "60a5e308e0514000001a63e5affff000000300000300000300060207d80e15103008005dc32000c02b23",
    "8008008008008004000120c07d00064064001680d76703ed80",
);

#[test]
fn the_nal_unit_is_type_62_with_emulation_prevention() {
    let frame = frame();
    let nal = frame.nal();
    assert_eq!(&nal[..6], &[0, 0, 0, 1, 0x7C, 0x01]);
    let body = &nal[6..];
    assert!(
        !body
            .windows(3)
            .any(|window| window[0] == 0 && window[1] == 0 && window[2] <= 2),
        "start code emulated in {}",
        hex(body)
    );
    assert_eq!(unescape(body), frame.rpu());
    // The default RPU carries long runs of zero bits (the identity mapping's
    // zero fractions), so the escape is needed, not merely harmless.
    assert!(body.len() > frame.rpu().len());
}

#[test]
fn level1_comes_from_the_frame_statistics_within_cm_v29_limits() {
    let level1 = Level1::from_stats(&stats(0.001, 0.4, 0.75)).unwrap();
    assert_eq!(
        level1,
        Level1 {
            min_pq: 4,
            avg_pq: 1638,
            max_pq: 3071
        }
    );
    // A black frame still names a 100-nit peak and an average below it.
    assert_eq!(
        Level1::from_stats(&stats(0., 0., 0.)).unwrap(),
        Level1 {
            min_pq: 0,
            avg_pq: 819,
            max_pq: 2081
        }
    );
    // A bright minimum is held at 12, an average above the peak just below it.
    assert_eq!(
        Level1::clamped(900, 4000, 3500),
        Level1 {
            min_pq: 12,
            avg_pq: 3499,
            max_pq: 3500
        }
    );
    assert_eq!(Level1::from_stats(&stats(0., 0.5, 1.5)), None);
    assert_eq!(Level1::from_stats(&stats(f32::NAN, 0.5, 0.9)), None);
}

#[test]
fn frames_without_statistics_take_level1_from_the_static_metadata() {
    // MaxCLL 800 and MaxFALL 180.
    let level1 = Level1::from_static(&hdr10());
    assert_eq!(level1.max_pq, nits_pq12(800.));
    assert_eq!(level1.avg_pq, nits_pq12(180.));
    // Without light levels: the mastering peak, and reference white.
    let level1 = Level1::from_static(&Metadata::default());
    assert_eq!(level1.max_pq, 3079);
    assert_eq!(level1.avg_pq, nits_pq12(203.));
    assert_eq!(nits_pq12(1000.), 3079);
}

#[test]
fn the_writer_refreshes_the_scene_on_the_first_frame_cuts_and_keyframes() {
    let mut writer = Writer::new(1000);
    let first = writer.frame(false);
    assert!(first.scene_refresh);
    assert_eq!(first.level1, Level1::from_static(&Metadata::default()));
    assert!(!writer.frame(false).scene_refresh);
    assert!(writer.frame(true).scene_refresh);
    assert!(!writer.measured());

    let mut smoother = StatsSmoother::default();
    let dark = smoother.update(&stats(0., 0.2, 0.5)).unwrap();
    writer.sample(&dark);
    assert!(writer.measured());
    let measured = writer.frame(false);
    assert_eq!(measured.level1, Level1::from_stats(&dark.stats).unwrap());
    // The first sample starts a scene, measured where the static values stood.
    assert!(measured.scene_refresh);
    assert!(!writer.frame(false).scene_refresh);

    let bright = smoother.update(&stats(0., 0.7, 0.95)).unwrap();
    assert!(bright.scene_cut);
    writer.sample(&bright);
    assert!(writer.frame(false).scene_refresh);
    assert!(!writer.frame(false).scene_refresh);

    // Invalid statistics keep the last measurement and refresh nothing.
    writer.sample(&Smoothed {
        stats: stats(0., 2., 0.9),
        scene_cut: true,
    });
    let kept = writer.frame(false);
    assert_eq!(kept.level1, Level1::from_stats(&bright.stats).unwrap());
    assert!(!kept.scene_refresh);
}

#[test]
fn the_writer_carries_the_static_metadata_and_target() {
    let mut writer = Writer::new(1000);
    writer.set_metadata(hdr10());
    writer.set_target_nits(600);
    assert_eq!(writer.target_nits(), 600);
    let frame = writer.frame(false);
    assert_eq!((frame.metadata, frame.target_nits), (hdr10(), 600));
    // A different target or metadata changes the RPU.
    let mut other = frame;
    other.target_nits = 1000;
    assert_ne!(other.rpu(), frame.rpu());
    other = frame;
    other.metadata.max_cll = 1000;
    assert_ne!(other.rpu(), frame.rpu());
}
