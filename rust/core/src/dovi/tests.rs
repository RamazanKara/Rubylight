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
        profile: Profile::P81,
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
fn only_hdr_hevc_and_av1_streams_that_picked_their_profile_carry_rpus() {
    use rubylight_protocol::hdr;
    let stream = |codec, hdr, hdr_format| crate::rtsp::Negotiated {
        codec,
        hdr,
        hdr_format,
        ..Default::default()
    };
    assert_eq!(negotiated(&stream(1, true, hdr::DV_81)), Some(Profile::P81));
    assert_eq!(negotiated(&stream(1, true, hdr::DV_84)), Some(Profile::P84));
    assert_eq!(
        negotiated(&stream(2, true, hdr::DV_101)),
        Some(Profile::P101)
    );
    assert!(stream(1, true, hdr::DV_84).hlg(), "8.4 is HLG underneath");
    assert!(!stream(2, true, hdr::DV_101).hlg(), "10.1 is PQ");
    for codec in [1, 2] {
        assert_eq!(
            negotiated(&stream(codec, true, 0)),
            None,
            "stock clients pick none"
        );
    }
    for (codec, hdr, format) in [
        (1, true, hdr::PQ),
        (1, true, hdr::HLG),
        (2, true, hdr::PQ),
        (2, true, hdr::DV_81),
        (2, true, hdr::DV_84),
        (1, true, hdr::DV_101),
        (0, true, hdr::DV_101),
        (3, true, hdr::DV_101),
        (1, false, hdr::DV_81),
        (1, false, hdr::DV_84),
        (2, false, hdr::DV_101),
    ] {
        assert_eq!(
            negotiated(&stream(codec, hdr, format)),
            None,
            "{codec} {hdr} {format}"
        );
    }
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
fn a_profile_8_4_rpu_reshapes_hlg_for_a_1000_nit_source() {
    // Checked with dovi_tool's `dolby_vision` 3.4.0: `DoviRpu::parse_unspec62_nalu`
    // reads profile 8, a matching CRC, source 62..3079 and the same L1, L2 and
    // L6 as 8.1, its mapping equals `Profile84::rpu_data_mapping()`, and its
    // writer writes these bytes back unchanged.
    let frame = Frame {
        profile: Profile::P84,
        ..frame()
    };
    let rpu = frame.rpu();
    assert_eq!(&rpu[..3], &[0x19, 0x08, 0x09]);
    let crc_at = rpu.len() - 5;
    assert_eq!(
        crc32_mpeg2(&rpu[1..crc_at]).to_be_bytes(),
        rpu[crc_at..rpu.len() - 1]
    );
    assert_eq!(hex(&frame.nal()), GOLDEN_84);
    // The writer keeps the profile it was given, frame after frame.
    let mut writer = Writer::new(600).with_profile(Profile::P84);
    assert_eq!(writer.profile(), Profile::P84);
    assert_eq!(writer.frame(false).profile, Profile::P84);
    assert_eq!(writer.frame(true).profile, Profile::P84);
    assert_eq!(Writer::new(600).frame(false).profile, Profile::P81);
}

const GOLDEN_84: &str = concat!(
    "000000017c0119080908406136506e203f114e6401000941002007801ffc00fffa7e6fec17f26373ca9a",
    "60a7f8bbc14f242c2bba6cfa941a97d175a07c6219aa8164ae7e11628bae32be73af0b4a33191830b8a1",
    "8d335503ac32640202590a2834ef60c01ee81340041528c0281c1db70003e92fde00225cfdc0522319ce",
    "c0aa9d96f85fd0b139b8841f3a802b21f1404a7200c0660f0383fd4b02ec670d991bb4e3ddf8942cbebb",
    "a065c127c024c202e0fe5adad8e2ff96d8c43c712d311ae6680e9bf49809ef955828b7e7400402f250d1",
    "ed28149bd470be8dec022a930d1afa5154b4c2b44e74c7c69c97946f800b052395203027a643ad79e049",
    "96d66418cbe6b118448a39681823ba4088f61b470a876844d8714d12b300001af512b37cfe758e12b322",
    "6500000300800000040000030004000003000e1b112180c3052f1847028a00000300d31f2d7fff800000",
    "030000030000030030103ec070a8803008005dc32000c02b238008008008008004000120c07d00064064",
    "001680ab7ef1fe80",
);

/// The bit at `at` of `bytes`, MSB first.
fn bit(bytes: &[u8], at: usize) -> u64 {
    u64::from(bytes[at / 8] >> (7 - at % 8) & 1)
}

fn bits_at(bytes: &[u8], at: usize, count: usize) -> u64 {
    (at..at + count).fold(0, |value, at| value << 1 | bit(bytes, at))
}

#[test]
fn a_profile_10_1_rpu_is_the_8_1_rpu_in_an_av1_metadata_obu() {
    // Checked with dovi_tool's `dolby_vision` 3.4.0:
    // `DoviRpu::parse_itu_t35_dovi_metadata_obu` reads the T.35 payload (from
    // the country code to the OBU's trailing bits) as the RPU of GOLDEN (its
    // `write_hevc_unspec62_nalu` gives GOLDEN's NAL unit), and
    // `write_av1_rpu_metadata_obu_t35_complete` writes the payload back
    // unchanged.
    let frame = Frame {
        profile: Profile::P101,
        ..frame()
    };
    let rpu = frame.rpu();
    assert_eq!(
        rpu,
        Frame {
            profile: Profile::P81,
            ..frame
        }
        .rpu(),
        "the 8.1 RPU"
    );
    let obu = frame.obu();
    assert_eq!(frame.unit(), obu);
    assert_eq!(hex(&obu), GOLDEN_101);
    // A metadata OBU (type 5) with a size field and no extension, its leb128
    // size, metadata_type 4 (ITU-T T.35) and the trailing bits.
    assert_eq!(obu[0], 5 << 3 | 0x02);
    let size = usize::from(obu[1] & 0x7F) | usize::from(obu[2]) << 7;
    assert!(obu[1] & 0x80 != 0 && obu[2] & 0x80 == 0);
    assert_eq!(3 + size, obu.len());
    assert_eq!(obu[3], 4);
    assert_eq!(obu.last(), Some(&0x80));
    // Country US, Dolby, provider-oriented code 0x800, then the EMDF container.
    let t35 = &obu[4..obu.len() - 1];
    assert_eq!(&t35[..7], &AV1_T35_HEADER);
    let emdf = &t35[7..];
    // emdf_version 0, key_id 6, emdf_payload_id 31 extended by variable_bits
    // 6 (more) and 1 to 256, no offsets, discard_unknown_payload: FFmpeg's
    // fixed 0x1be6841.
    assert_eq!(bits_at(emdf, 0, 27), 0x1BE_6841);
    // The payload size in variable_bits(8) and the RPU without its prefix.
    let payload = rpu.len() - 1;
    assert!(payload < 256, "one group: {payload}");
    assert_eq!(bits_at(emdf, 27, 9), (payload as u64) << 1);
    for (index, &byte) in rpu[1..].iter().enumerate() {
        assert_eq!(bits_at(emdf, 36 + index * 8, 8), u64::from(byte), "{index}");
    }
    // The end of the payloads (payload id 0, 5 bits), the protection lengths 01 and 00 and eight
    // zero protection bits, then one bits to a byte.
    let end = 36 + payload * 8;
    assert_eq!(bits_at(emdf, end, 17), 0b01 << 10);
    assert_eq!(emdf.len() * 8, (end + 17).div_ceil(8) * 8);
    for at in end + 17..emdf.len() * 8 {
        assert_eq!(bit(emdf, at), 1, "padding bit {at}");
    }
    // A writer for 10.1 hands out OBUs.
    let mut writer = Writer::new(600).with_profile(Profile::P101);
    assert_eq!(writer.frame(false).unit()[0], 0x2A);
    assert_eq!(
        Writer::new(600).frame(false).unit()[..6],
        [0, 0, 0, 1, 0x7C, 0x01]
    );
}

const GOLDEN_101: &str = concat!(
    "2a980104b5003b0000080037cd08310080908406136506f003ff801ffc00fffd00000080000068000004",
    "0000034000002000001a2566000035ea2566f9fceb1c256644ca00000100000008000000080000001c36",
    "224301860a5e308e0514000001a63e5affff000000000000000060207d80e15103008005dc32000c02b2",
    "38008008008008004000120c07d00064064001680d76703ed800200780",
);

#[test]
fn emdf_variable_bits_add_one_per_group() {
    let written = |count: u32, value: u64| {
        let mut bits = Bits::with_prefix(0);
        bits.variable(count, value);
        let used = bits.position() as usize - 8;
        (bits_at(&bits.bytes[1..], 0, used), used)
    };
    // The payload id 256 as 31 plus 225: 6, more, 1, end.
    assert_eq!(written(5, 225), ((6 << 7) | (1 << 6) | (1 << 1), 12));
    assert_eq!(written(8, 0), (0, 9));
    assert_eq!(written(8, 255), (0xFF << 1, 9));
    // 256 and up: FFmpeg's (size >> 8) - 1, more, size & 0xFF, end.
    assert_eq!(written(8, 256), (1 << 9, 18));
    assert_eq!(written(8, 300), ((1 << 9) | (0x2C << 1), 18));
    assert_eq!(written(8, 600), ((1 << 10) | (1 << 9) | (0x58 << 1), 18));
    assert_eq!(
        written(8, 65_791),
        ((0xFF << 10) | (1 << 9) | (0xFF << 1), 18)
    );
    // Three groups from 65792 on.
    assert_eq!(written(8, 65_792).1, 27);
}

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
