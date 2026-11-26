//! Stream DVR unit tests and their fixtures: real parameter sets dumped once from FFmpeg
//! 8.1 encodes (libx264 200x120 High; libx265 202x118 Main10; libaom 200x120 10-bit) and
//! synthetic access units around them.

mod bmff;
mod mux;
mod parse;
mod policy;
mod recorder;
mod ring;

use super::{AudioPacket, HdrInfo, OpusConfig, StreamInfo, VideoCodec, VideoFrame};

/// libx264, 200x120 (208x128 coded, cropped), High profile, level 1.2.
pub(super) const H264_SPS: &[u8] = &[
    0x67, 0x64, 0x00, 0x0c, 0xac, 0xd9, 0x43, 0x44, 0x79, 0x65, 0xc0, 0x44, 0x00, 0x00, 0x03, 0x00,
    0x04, 0x00, 0x00, 0x03, 0x00, 0xf0, 0x3c, 0x50, 0xa6, 0x58,
];
pub(super) const H264_PPS: &[u8] = &[0x68, 0xeb, 0xe3, 0xcb, 0x22, 0xc0];

/// libx265, 202x118 (208x120 coded with a conformance window), Main10, level 2.
pub(super) const HEVC_VPS: &[u8] = &[
    0x40, 0x01, 0x0c, 0x01, 0xff, 0xff, 0x02, 0x20, 0x00, 0x00, 0x03, 0x00, 0x90, 0x00, 0x00, 0x03,
    0x00, 0x00, 0x03, 0x00, 0x3c, 0x95, 0x98, 0x09,
];
pub(super) const HEVC_SPS: &[u8] = &[
    0x42, 0x01, 0x01, 0x02, 0x20, 0x00, 0x00, 0x03, 0x00, 0x90, 0x00, 0x00, 0x03, 0x00, 0x00, 0x03,
    0x00, 0x3c, 0xa0, 0x1a, 0x20, 0x79, 0xc9, 0x4d, 0x96, 0x56, 0x69, 0x24, 0xca, 0xf0, 0x16, 0xa1,
    0x22, 0x01, 0x20, 0x80, 0x00, 0x00, 0x03, 0x00, 0x80, 0x00, 0x00, 0x0f, 0x04,
];
pub(super) const HEVC_PPS: &[u8] = &[0x44, 0x01, 0xc1, 0x72, 0xb4, 0x62, 0x40];

/// libaom, 200x120, profile 0, 10-bit 4:2:0, level 2.0: the OBU with its size field.
pub(super) const AV1_SEQUENCE_HEADER: &[u8] = &[
    0x0a, 0x0a, 0x00, 0x00, 0x00, 0x03, 0xb6, 0x3f, 0x73, 0x6b, 0xe5, 0x01,
];

pub(super) const H264_AUD: &[u8] = &[0x09, 0xf0];
pub(super) const H264_SEI: &[u8] = &[0x06, 0x05, 0x01, 0xaa, 0x80];
pub(super) const HEVC_AUD: &[u8] = &[0x46, 0x01, 0x50];
pub(super) const HEVC_SEI: &[u8] = &[0x4e, 0x01, 0x05, 0x01, 0xaa, 0x80];
pub(super) const AV1_TEMPORAL_DELIMITER: &[u8] = &[0x12, 0x00];

/// Annex-B with four-byte start codes.
pub(super) fn annexb(units: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::new();
    for unit in units {
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(unit);
    }
    out
}

/// A non-zero byte that differs per frame, so slices never contain a start code.
fn tag(pts_us: u64) -> u8 {
    (pts_us / 1000 % 250) as u8 + 1
}

/// An H.264 access unit: AUD, then SPS/PPS/SEI on keyframes, then a slice.
pub(super) fn h264_frame_with(pts_us: u64, idr: bool, sps: &[u8]) -> VideoFrame {
    let slice: Vec<u8> = if idr {
        vec![0x65, 0x88, 0x84, 0x21, tag(pts_us)]
    } else {
        vec![0x41, 0x9a, 0x21, 0x43, tag(pts_us)]
    };
    let units: Vec<&[u8]> = if idr {
        vec![H264_AUD, sps, H264_PPS, H264_SEI, slice.as_slice()]
    } else {
        vec![H264_AUD, slice.as_slice()]
    };
    VideoFrame::new(pts_us, idr, annexb(&units))
}

pub(super) fn h264_frame(pts_us: u64, idr: bool) -> VideoFrame {
    h264_frame_with(pts_us, idr, H264_SPS)
}

pub(super) fn hevc_frame(pts_us: u64, idr: bool) -> VideoFrame {
    let units: Vec<Vec<u8>> = if idr {
        vec![
            HEVC_AUD.to_vec(),
            HEVC_VPS.to_vec(),
            HEVC_SPS.to_vec(),
            HEVC_PPS.to_vec(),
            HEVC_SEI.to_vec(),
            vec![0x26, 0x01, 0xaf, 0x11, tag(pts_us)],
        ]
    } else {
        vec![HEVC_AUD.to_vec(), vec![0x02, 0x01, 0xd0, 0x22, tag(pts_us)]]
    };
    let units: Vec<&[u8]> = units.iter().map(Vec::as_slice).collect();
    VideoFrame::new(pts_us, idr, annexb(&units))
}

/// An AV1 temporal unit: TD, the sequence header on keyframes, a sized frame OBU, and a
/// trailing frame OBU without a size field.
pub(super) fn av1_frame(pts_us: u64, idr: bool) -> VideoFrame {
    let mut data = AV1_TEMPORAL_DELIMITER.to_vec();
    if idr {
        data.extend_from_slice(AV1_SEQUENCE_HEADER);
    }
    data.extend_from_slice(&[0x32, 0x03, if idr { 0x10 } else { 0x30 }, 0x44, tag(pts_us)]);
    data.extend_from_slice(&[0x30, 0x55, tag(pts_us)]);
    VideoFrame::new(pts_us, idr, data)
}

pub(super) fn frame(codec: VideoCodec, pts_us: u64, idr: bool) -> VideoFrame {
    match codec {
        VideoCodec::H264 => h264_frame(pts_us, idr),
        VideoCodec::Hevc => hevc_frame(pts_us, idr),
        VideoCodec::Av1 => av1_frame(pts_us, idr),
    }
}

/// A 10 ms Opus packet.
pub(super) fn audio(pts_us: u64) -> AudioPacket {
    AudioPacket::new(pts_us, 480, vec![0xfc, 0xff, 0xfe, tag(pts_us)])
}

pub(super) fn info(codec: VideoCodec) -> StreamInfo {
    StreamInfo {
        codec,
        hdr: None,
        audio: Some(OpusConfig::stereo()),
    }
}

pub(super) fn hdr_metadata() -> crate::hdr::Metadata {
    crate::hdr::Metadata {
        primaries: [[34000, 16000], [13250, 34500], [7500, 3000]],
        white: [15635, 16450],
        maximum_nits: 1000,
        minimum: 50,
        max_cll: 900,
        max_fall: 400,
        full_frame_nits: 400,
    }
}

pub(super) fn hdr_info(codec: VideoCodec) -> StreamInfo {
    StreamInfo {
        codec,
        hdr: Some(HdrInfo {
            metadata: Some(hdr_metadata()),
        }),
        audio: None,
    }
}

/// `seconds` of video at `step_us` with a keyframe every `gop_us`, from `from_us`.
pub(super) fn frames(
    codec: VideoCodec,
    from_us: u64,
    to_us: u64,
    step_us: u64,
    gop_us: u64,
) -> Vec<VideoFrame> {
    (from_us..to_us)
        .step_by(step_us as usize)
        .map(|pts| frame(codec, pts, (pts - from_us) % gop_us == 0))
        .collect()
}
