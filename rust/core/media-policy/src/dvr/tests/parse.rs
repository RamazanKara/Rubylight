//! Annex-B, SPS, OBU and sequence header parsing, parameter set extraction, and
//! no-panic properties on arbitrary bytes.

use super::*;
use crate::dvr::av1::{self, Obu};
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
        }
    );
    assert!(sps.high_profile());
    assert_eq!(nal::h264_sps(H264_PPS), None);
    assert_eq!(nal::h264_sps(&H264_SPS[..6]), None);
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
        }
    );
    assert_eq!(header.bit_depth(), 10);
    assert_eq!(av1::sequence_header(&obu.payload[..5]), None);
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
