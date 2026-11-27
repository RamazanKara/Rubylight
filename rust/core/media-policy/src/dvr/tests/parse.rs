//! Annex-B, SPS, OBU and sequence header parsing, colour signalling, parameter set
//! extraction, and no-panic properties on arbitrary bytes.

use super::*;
use crate::dvr::av1::{self, Obu};
use crate::dvr::colour::{self, Colour};
use crate::dvr::nal::{self, Bits};
use crate::dvr::{ParamSets, fmp4};
use proptest::prelude::*;

#[test]
fn annexb_units_split_on_three_and_four_byte_start_codes() {
    let data = [
        0xaa, 0, 0, 0, 1, 0x67, 1, 2, 0, 0, 1, 0x68, 3, 0, 0, 0, 0, 1, 0x65, 4, 0, 0,
    ];
    let units: Vec<&[u8]> = nal::units(&data).collect();
    assert_eq!(units, [&[0x67, 1, 2][..], &[0x68, 3][..], &[0x65, 4][..]]);
    assert_eq!(nal::units(&[0, 0, 1]).count(), 0);
    assert_eq!(nal::units(&[1, 2, 3]).count(), 0);
}

#[test]
fn rbsp_drops_emulation_prevention_bytes() {
    assert_eq!(
        nal::rbsp(&[0x42, 0, 0, 3, 1, 0, 0, 3, 0, 0, 3]),
        [0x42, 0, 0, 1, 0, 0, 0, 0]
    );
    assert_eq!(nal::rbsp(&[0, 3, 0, 0, 2]), [0, 3, 0, 0, 2]);
}

#[test]
fn exp_golomb_codes_read_back() {
    let data = [0xa6, 0x40];
    let mut bits = Bits::new(&data);
    assert_eq!(
        [bits.ue(), bits.ue(), bits.ue(), bits.ue()],
        [Some(0), Some(1), Some(2), Some(3)]
    );
    let mut bits = Bits::new(&data);
    bits.skip(1).unwrap();
    assert_eq!([bits.se(), bits.se()], [Some(1), Some(-1)]);
    assert_eq!(Bits::new(&[0, 0, 0, 0, 0]).ue(), None);
    assert_eq!(Bits::new(&[]).bit(), None);
}

#[test]
fn h264_sps_gives_profile_level_and_cropped_size() {
    let sps = nal::h264_sps(H264_SPS).unwrap();
    assert_eq!(
        sps,
        nal::H264Sps {
            profile_idc: 100,
            constraint_flags: 0,
            level_idc: 12,
            chroma_format_idc: 1,
            bit_depth_luma: 8,
            bit_depth_chroma: 8,
            width: 200,
            height: 120,
            colour: Colour::default(),
        }
    );
    assert!(sps.high_profile());
    assert_eq!(nal::h264_sps(H264_PPS), None);
    assert_eq!(nal::h264_sps(&H264_SPS[..6]), None);
}

fn synthetic_h264_sps(profile_idc: u8, chroma_format_idc: u32, bit_depth: u8) -> Vec<u8> {
    let mut w = BitWriter::default();
    w.put(u64::from(profile_idc), 8).put(0, 8).put(40, 8);
    w.ue(0);
    if !matches!(profile_idc, 66 | 77 | 88) {
        w.ue(chroma_format_idc);
        if chroma_format_idc == 3 {
            w.put(0, 1);
        }
        w.ue(u32::from(bit_depth - 8));
        w.ue(u32::from(bit_depth - 8));
        w.put(0, 1).put(0, 1);
    }
    w.ue(0).ue(2).ue(1).put(0, 1);
    w.ue(12).ue(7).put(1, 1).put(1, 1).put(1, 1);
    let crop = if chroma_format_idc == 3 { 8 } else { 4 };
    w.ue(0).ue(crop).ue(0).ue(crop).put(0, 1).put(1, 1);
    let mut nal = vec![0x67];
    let mut zeros = 0u8;
    for byte in w.bytes {
        if zeros >= 2 && byte <= 3 {
            nal.push(3);
            zeros = 0;
        }
        nal.push(byte);
        zeros = if byte == 0 { zeros + 1 } else { 0 };
    }
    nal
}

#[test]
fn h264_extended_profiles_244_and_144_parse_and_write_avcc_extension() {
    use super::bmff::parse;
    use crate::dvr::fmp4::codec::{self, Config};

    let pps = [H264_PPS.to_vec()];
    for (profile_idc, depth, expected_tail) in [
        (244u8, 10u8, [0xffu8, 0xfa, 0xfa, 0]),
        (144u8, 8u8, [0xffu8, 0xf8, 0xf8, 0]),
    ] {
        let sps_bytes = synthetic_h264_sps(profile_idc, 3, depth);
        let parsed = nal::h264_sps(&sps_bytes).expect("extended SPS parses");
        assert_eq!(parsed.profile_idc, profile_idc);
        assert_eq!(parsed.chroma_format_idc, 3);
        assert_eq!(
            (parsed.bit_depth_luma, parsed.bit_depth_chroma),
            (depth, depth)
        );
        assert_eq!((parsed.width, parsed.height), (200, 120));
        assert!(parsed.high_profile());
        let sps = [sps_bytes.clone()];
        let entry = codec::visual_entry(
            &Config::Avc {
                sps: &sps,
                pps: &pps,
                parsed,
            },
            parsed.width,
            parsed.height,
            None,
        );
        let avcc = parse(&entry)[0].child("avcC").payload(&entry);
        assert_eq!(avcc.len(), 15 + sps_bytes.len() + H264_PPS.len());
        assert_eq!(&avcc[avcc.len() - 4..], &expected_tail);
    }

    let main_sps = synthetic_h264_sps(77, 1, 8);
    let parsed = nal::h264_sps(&main_sps).unwrap();
    assert!(!parsed.high_profile());
    assert_eq!((parsed.width, parsed.height), (200, 120));
    let sps = [main_sps.clone()];
    let entry = codec::visual_entry(
        &Config::Avc {
            sps: &sps,
            pps: &pps,
            parsed,
        },
        parsed.width,
        parsed.height,
        None,
    );
    let avcc = parse(&entry)[0].child("avcC").payload(&entry);
    assert_eq!(avcc.len(), 11 + main_sps.len() + H264_PPS.len());
}

#[test]
fn hevc_sps_gives_profile_tier_level_and_conformance_window() {
    let sps = nal::hevc_sps(HEVC_SPS).unwrap();
    assert_eq!(
        sps,
        nal::HevcSps {
            profile_space: 0,
            tier: false,
            profile_idc: 2,
            compatibility: 0x2000_0000,
            constraints: 0x9000_0000_0000,
            level_idc: 60,
            max_sub_layers: 1,
            temporal_id_nesting: true,
            chroma_format_idc: 1,
            bit_depth_luma: 10,
            bit_depth_chroma: 10,
            width: 202,
            height: 118,
            colour: Colour {
                description: Some((9, 16, 9)),
                full_range: false,
            },
        }
    );
    assert_eq!(nal::hevc_sps(HEVC_VPS), None);
    assert_eq!(nal::hevc_sps(&HEVC_SPS[..20]), None);
}

#[test]
fn av1_sequence_header_fields() {
    let obu = av1::obus(AV1_SEQUENCE_HEADER).next().unwrap();
    assert_eq!(obu.kind, av1::OBU_SEQUENCE_HEADER);
    let header = av1::sequence_header(obu.payload).unwrap();
    assert_eq!(
        header,
        av1::SequenceHeader {
            profile: 0,
            level_idx: 0,
            tier: false,
            high_bitdepth: true,
            twelve_bit: false,
            monochrome: false,
            subsampling_x: true,
            subsampling_y: true,
            chroma_sample_position: 0,
            width: 200,
            height: 120,
            colour: Colour::default(),
        }
    );
    assert_eq!(header.bit_depth(), 10);
    assert_eq!(av1::sequence_header(&obu.payload[..5]), None);
}

#[test]
fn colour_signalling_comes_from_the_vui_and_color_config() {
    let bt601 = nal::h264_sps(H264_SPS_BT601_FULL).unwrap();
    assert_eq!((bt601.width, bt601.height), (200, 120));
    assert_eq!(
        bt601.colour,
        Colour {
            description: Some((6, 6, 6)),
            full_range: true,
        }
    );
    let hlg = Colour {
        description: Some((9, 18, 9)),
        full_range: false,
    };
    assert_eq!(nal::hevc_sps(HEVC_SPS_HLG).unwrap().colour, hlg);
    let obu = av1::obus(AV1_SEQUENCE_HEADER_HLG_FULL).next().unwrap();
    let header = av1::sequence_header(obu.payload).unwrap();
    assert_eq!(
        (header.width, header.height, header.bit_depth()),
        (200, 120, 10)
    );
    assert_eq!(
        header.colour,
        Colour {
            full_range: true,
            ..hlg
        }
    );
    // A VUI cut short leaves the colour unsignalled, not the SPS unreadable.
    let cut = nal::hevc_sps(&HEVC_SPS_HLG[..33]).unwrap();
    assert_eq!(cut.colour, Colour::default());
    assert_eq!((cut.width, cut.height, cut.bit_depth_luma), (202, 118, 10));
}

#[test]
fn colour_falls_back_per_unspecified_code_point() {
    assert_eq!(Colour::default().resolve(false), (1, 1, 1));
    assert_eq!(Colour::default().resolve(true), (9, 16, 9));
    let partial = Colour {
        description: Some((2, 2, 9)),
        full_range: false,
    };
    assert_eq!(partial.resolve(false), (1, 1, 9));
    assert_eq!(partial.resolve(true), (9, 16, 9));
    let reserved = Colour {
        description: Some((3, 0, 3)),
        full_range: false,
    };
    assert_eq!(reserved.resolve(false), (1, 1, 1));
    let gbr = Colour {
        description: Some((1, 13, 0)),
        full_range: true,
    };
    assert_eq!(gbr.resolve(true), (1, 13, 0));
}

/// MSB-first bit writer for synthetic syntax.
#[derive(Default)]
struct BitWriter {
    bytes: Vec<u8>,
    len: usize,
}

impl BitWriter {
    fn put(&mut self, value: u64, count: u32) -> &mut Self {
        for shift in (0..count).rev() {
            if self.len.is_multiple_of(8) {
                self.bytes.push(0);
            }
            if (value >> shift) & 1 == 1 {
                *self.bytes.last_mut().unwrap() |= 0x80 >> (self.len % 8);
            }
            self.len += 1;
        }
        self
    }

    fn ue(&mut self, value: u32) -> &mut Self {
        let code = u64::from(value) + 1;
        let width = 64 - code.leading_zeros();
        self.put(0, width - 1).put(code, width)
    }
}

#[test]
fn hevc_sps_tail_walks_scaling_lists_and_predicted_reference_sets() {
    let mut w = BitWriter::default();
    // POC LSB 8 bits; ordering info for both sub-layers; block sizes and depths.
    w.ue(4).put(1, 1);
    for _ in 0..2 * 3 + 6 {
        w.ue(1);
    }
    // Scaling lists: predicted except one explicit 8x8 list with its DC (se 0 is "1").
    w.put(0b11, 2);
    for size in 0..4 {
        let matrices = if size == 3 { 2 } else { 6 };
        for matrix in 0..matrices {
            if size == 2 && matrix == 0 {
                w.put(1, 1).put(1, 1).put(u64::MAX, 64);
            } else {
                w.put(0, 1).ue(0);
            }
        }
    }
    // AMP, SAO; PCM with its depths, sizes and loop filter flag.
    w.put(0b11, 2).put(1, 1).put(0x77, 8).ue(0).ue(1).put(1, 1);
    // Set 0: S0 = {-1, -3}, S1 = {+2}.
    w.ue(3).ue(2).ue(1);
    w.ue(0).put(1, 1).ue(1).put(1, 1);
    w.ue(1).put(0, 1);
    // Set 1 from set 0 with deltaRps -1: four entries, two kept: S0 = {-1}, S1 = {+1}.
    w.put(1, 1).put(1, 1).ue(0);
    w.put(0b00, 2).put(0b00, 2).put(1, 1).put(0b01, 2);
    // Set 2 from set 1 with deltaRps +2: three entries, all used.
    w.put(1, 1).put(0, 1).ue(1).put(0b111, 3);
    // Two long-term pictures (8-bit LSB and a flag each); temporal MVP, strong intra.
    w.put(1, 1).ue(2).put(0x1ff, 9).put(0x1ff, 9).put(0b11, 2);
    // VUI: extended SAR, overscan, then the signal type: full-range HLG.
    w.put(0b11, 2).put(255, 8);
    w.put(0x0001_0001, 32).put(0b11, 2);
    w.put(1, 1).put(5, 3).put(0b11, 2);
    w.put(9, 8).put(18, 8).put(9, 8);
    w.put(0x80, 8);
    assert_eq!(
        colour::hevc(&mut Bits::new(&w.bytes), 1),
        Some(Colour {
            description: Some((9, 18, 9)),
            full_range: true,
        })
    );
}

#[test]
fn leb128_round_trips() {
    for value in [
        0u64,
        1,
        127,
        128,
        300,
        1 << 20,
        u64::from(u32::MAX),
        (1 << 56) - 1,
    ] {
        let mut out = Vec::new();
        av1::write_leb128(&mut out, value);
        assert_eq!(av1::leb128(&out), Some((value, out.len())), "{value}");
    }
    assert_eq!(av1::leb128(&[0x80; 9]), None);
}

#[test]
fn obus_iterate_and_gain_size_fields() {
    let frame = av1_frame(0, true);
    let obus: Vec<Obu<'_>> = av1::obus(&frame.data).collect();
    let kinds: Vec<u8> = obus.iter().map(|obu| obu.kind).collect();
    assert_eq!(kinds, [2, 1, 6, 6]);
    let last = obus[3];
    assert_eq!(last.payload, [0x55, 1]);
    assert_eq!(last.sized(), [0x32, 2, 0x55, 1]);
    assert_eq!(obus[1].sized(), AV1_SEQUENCE_HEADER);
    assert_eq!(av1::obus(&[0x80, 0]).count(), 0);
    assert_eq!(av1::obus(&[0x0a, 5, 1]).count(), 0);
    let extended = [0x0e, 0x20, 1, 0x77];
    let obu = av1::obus(&extended).next().unwrap();
    assert_eq!((obu.header, obu.payload), (&[0x0e, 0x20][..], &[0x77][..]));
}

#[test]
fn parameter_sets_come_from_keyframes_only() {
    let sets = ParamSets::from_frame(VideoCodec::H264, &h264_frame(0, true).data);
    assert_eq!(sets.sps, [H264_SPS.to_vec()]);
    assert_eq!(sets.pps, [H264_PPS.to_vec()]);
    assert!(sets.is_complete(VideoCodec::H264));
    let plain = ParamSets::from_frame(VideoCodec::H264, &h264_frame(1, false).data);
    assert!(plain.is_empty() && !plain.is_complete(VideoCodec::H264));

    let sets = ParamSets::from_frame(VideoCodec::Hevc, &hevc_frame(0, true).data);
    assert_eq!((sets.vps.len(), sets.sps.len(), sets.pps.len()), (1, 1, 1));
    assert!(sets.is_complete(VideoCodec::Hevc));

    let sets = ParamSets::from_frame(VideoCodec::Av1, &av1_frame(0, true).data);
    assert_eq!(sets.sequence_header.as_deref(), Some(AV1_SEQUENCE_HEADER));
    assert!(ParamSets::from_frame(VideoCodec::Av1, &av1_frame(1, false).data).is_empty());

    let doubled = annexb(&[H264_SPS, H264_SPS, H264_PPS]);
    assert_eq!(
        ParamSets::from_frame(VideoCodec::H264, &doubled).sps.len(),
        1
    );
}

#[test]
fn samples_drop_aud_and_parameter_sets_but_keep_sei() {
    let mut out = Vec::new();
    fmp4::append_sample(VideoCodec::H264, &h264_frame(0, true).data, &mut out);
    let mut units = Vec::new();
    let mut at = 0;
    while at < out.len() {
        let len = u32::from_be_bytes(out[at..at + 4].try_into().unwrap()) as usize;
        units.push(out[at + 4] & 0x1f);
        at += 4 + len;
    }
    assert_eq!(at, out.len());
    assert_eq!(units, [nal::H264_SEI, 5]);

    let mut out = Vec::new();
    fmp4::append_sample(VideoCodec::Hevc, &hevc_frame(0, true).data, &mut out);
    assert_eq!(&out[..4], &(HEVC_SEI.len() as u32).to_be_bytes());
    assert_eq!(out[4..4 + HEVC_SEI.len()], *HEVC_SEI);

    let mut out = Vec::new();
    fmp4::append_sample(VideoCodec::Av1, &av1_frame(0, true).data, &mut out);
    let kinds: Vec<(u8, bool)> = av1::obus(&out)
        .map(|obu| (obu.kind, obu.header[0] & 2 != 0))
        .collect();
    assert_eq!(kinds, [(1, true), (6, true), (6, true)]);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn parsers_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..256)) {
        let _ = nal::units(&bytes).count();
        let _ = nal::rbsp(&bytes);
        let mut h264 = vec![0x67];
        h264.extend_from_slice(&bytes);
        let _ = nal::h264_sps(&h264);
        let mut hevc = vec![0x42, 0x01];
        hevc.extend_from_slice(&bytes);
        let _ = nal::hevc_sps(&hevc);
        let _ = av1::sequence_header(&bytes);
        let _ = av1::obus(&bytes).count();
        let layers = bytes.first().map_or(0, |byte| byte & 7);
        let _ = colour::hevc(&mut Bits::new(&bytes), layers);
        let _ = colour::vui(&mut Bits::new(&bytes));
        for codec in [VideoCodec::H264, VideoCodec::Hevc, VideoCodec::Av1] {
            let _ = ParamSets::from_frame(codec, &bytes);
            let mut out = Vec::new();
            fmp4::append_sample(codec, &bytes, &mut out);
        }
    }

    #[test]
    fn mutated_sps_never_panics(index in 0usize..45, value in any::<u8>()) {
        let mut h264 = H264_SPS.to_vec();
        let at = index % h264.len();
        h264[at] = value;
        let _ = nal::h264_sps(&h264);
        let mut hevc = HEVC_SPS.to_vec();
        hevc[index] = value;
        let _ = nal::hevc_sps(&hevc);
        let mut seq = AV1_SEQUENCE_HEADER[2..].to_vec();
        let at = index % seq.len();
        seq[at] = value;
        let _ = av1::sequence_header(&seq);
    }
}
