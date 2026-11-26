//! Codec parameter sets a keyframe carries in band (H.264 SPS/PPS, HEVC VPS/SPS/PPS, the
//! AV1 sequence header): extraction, completeness, and the config identity that splits
//! ring epochs and recording parts.

use super::{VideoCodec, av1, nal};

/// The parameter sets of one access unit, in stream order without duplicates.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ParamSets {
    pub vps: Vec<Vec<u8>>,
    pub sps: Vec<Vec<u8>>,
    pub pps: Vec<Vec<u8>>,
    /// The AV1 sequence header OBU, with its size field set.
    pub sequence_header: Option<Vec<u8>>,
}

impl ParamSets {
    pub fn from_frame(codec: VideoCodec, data: &[u8]) -> Self {
        let mut sets = Self::default();
        match codec {
            VideoCodec::H264 => {
                for unit in nal::units(data) {
                    match nal::h264_type(unit) {
                        Some(nal::H264_SPS) => push_unique(&mut sets.sps, unit),
                        Some(nal::H264_PPS) => push_unique(&mut sets.pps, unit),
                        _ => {}
                    }
                }
            }
            VideoCodec::Hevc => {
                for unit in nal::units(data) {
                    match nal::hevc_type(unit) {
                        Some(nal::HEVC_VPS) => push_unique(&mut sets.vps, unit),
                        Some(nal::HEVC_SPS) => push_unique(&mut sets.sps, unit),
                        Some(nal::HEVC_PPS) => push_unique(&mut sets.pps, unit),
                        _ => {}
                    }
                }
            }
            VideoCodec::Av1 => {
                sets.sequence_header = av1::obus(data)
                    .find(|obu| obu.kind == av1::OBU_SEQUENCE_HEADER)
                    .map(|obu| obu.sized());
            }
        }
        sets
    }

    pub fn is_empty(&self) -> bool {
        self.vps.is_empty()
            && self.sps.is_empty()
            && self.pps.is_empty()
            && self.sequence_header.is_none()
    }

    /// Whether these sets alone describe the codec configuration of an MP4 track.
    pub fn is_complete(&self, codec: VideoCodec) -> bool {
        match codec {
            VideoCodec::H264 => !self.sps.is_empty() && !self.pps.is_empty(),
            VideoCodec::Hevc => {
                !self.vps.is_empty() && !self.sps.is_empty() && !self.pps.is_empty()
            }
            VideoCodec::Av1 => self.sequence_header.is_some(),
        }
    }
}

fn push_unique(list: &mut Vec<Vec<u8>>, unit: &[u8]) {
    if !list.iter().any(|known| known == unit) {
        list.push(unit.to_vec());
    }
}
