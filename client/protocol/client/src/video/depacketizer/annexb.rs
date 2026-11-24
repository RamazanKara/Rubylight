//! Annex B NAL parsing and key-frame payload splitting for H.264 and HEVC.
//!
//! Ported from moonlight-common-c's `VideoDepacketizer.c` (GPL-3.0, Cameron Gutman and
//! contributors).

use std::ops::Range;

use super::{Depacketizer, Desc};
use crate::video::{BufferType, Codec, FrameBuffer, FrameType};

const H264_NAL_TYPE_SEI: u8 = 6;
const H264_NAL_TYPE_SPS: u8 = 7;
const H264_NAL_TYPE_PPS: u8 = 8;
const H264_NAL_TYPE_AUD: u8 = 9;
const HEVC_NAL_TYPE_VPS: u8 = 32;
const HEVC_NAL_TYPE_SPS: u8 = 33;
const HEVC_NAL_TYPE_PPS: u8 = 34;
const HEVC_NAL_TYPE_AUD: u8 = 35;
const HEVC_NAL_TYPE_SEI: u8 = 39;

impl Depacketizer {
    // Annex B parsing. `data` is the payload after the video packet header.

    /// The length of the start code at `d` (3 or 4), only if a NAL type byte follows it.
    pub(super) fn start_sequence(data: &[u8], d: Desc) -> Option<usize> {
        if d.length <= 3 {
            return None;
        }
        let at = d.offset;
        if data[at] == 0 && data[at + 1] == 0 {
            if data[at + 2] == 0 {
                if d.length > 4 && data[at + 3] == 1 {
                    return Some(4);
                }
            } else if data[at + 2] == 1 {
                return Some(3);
            }
        }
        None
    }

    fn nal_type(&self, data: &[u8], d: Desc) -> Option<u8> {
        let seq = Self::start_sequence(data, d)?;
        let byte = data[d.offset + seq];
        Some(match self.codec {
            Codec::H264 => byte & 0x1F,
            _ => (byte & 0x7E) >> 1,
        })
    }

    fn nal_is(&self, data: &[u8], d: Desc, h264: u8, hevc: u8) -> bool {
        match self.codec {
            Codec::H264 => self.nal_type(data, d) == Some(h264),
            Codec::Hevc => self.nal_type(data, d) == Some(hevc),
            Codec::Opaque => false,
        }
    }

    fn is_seq_reference_frame_start(&self, data: &[u8], d: Desc) -> bool {
        match (self.codec, self.nal_type(data, d)) {
            (Codec::H264, Some(t)) => t == 5,
            (Codec::Hevc, Some(t)) => (16..=21).contains(&t),
            _ => false,
        }
    }

    pub(super) fn is_access_unit_delimiter(&self, data: &[u8], d: Desc) -> bool {
        self.nal_is(data, d, H264_NAL_TYPE_AUD, HEVC_NAL_TYPE_AUD)
    }

    pub(super) fn is_sei_nal(&self, data: &[u8], d: Desc) -> bool {
        self.nal_is(data, d, H264_NAL_TYPE_SEI, HEVC_NAL_TYPE_SEI)
    }

    pub(super) fn is_picture_parameter_set_nal(&self, data: &[u8], d: Desc) -> bool {
        self.nal_is(data, d, H264_NAL_TYPE_PPS, HEVC_NAL_TYPE_PPS)
    }

    pub(super) fn is_idr_frame_start(&self, data: &[u8], d: Desc) -> bool {
        self.nal_is(data, d, H264_NAL_TYPE_SPS, HEVC_NAL_TYPE_VPS)
    }

    /// Advances to the next start code, or to the end.
    pub(super) fn skip_to_next_nal_or_end(data: &[u8], d: &mut Desc) {
        if let Some(seq) = Self::start_sequence(data, *d) {
            d.offset += seq;
            d.length -= seq;
        }
        while Self::start_sequence(data, *d).is_none() {
            if d.length == 0 {
                return;
            }
            d.offset += 1;
            d.length -= 1;
        }
    }

    fn buffer_type(&self, fragment: &[u8]) -> BufferType {
        let d = Desc {
            offset: 0,
            length: fragment.len(),
        };
        match (self.codec, self.nal_type(fragment, d)) {
            (Codec::H264, Some(H264_NAL_TYPE_SPS)) => BufferType::Sps,
            (Codec::H264, Some(H264_NAL_TYPE_PPS)) => BufferType::Pps,
            (Codec::Hevc, Some(HEVC_NAL_TYPE_SPS)) => BufferType::Sps,
            (Codec::Hevc, Some(HEVC_NAL_TYPE_PPS)) => BufferType::Pps,
            (Codec::Hevc, Some(HEVC_NAL_TYPE_VPS)) => BufferType::Vps,
            _ => BufferType::PicData,
        }
    }

    pub(super) fn queue_fragment(&mut self, fragment: &[u8]) {
        let kind = self.buffer_type(fragment);
        let start = self.data.len();
        self.data.extend_from_slice(fragment);
        self.buffers.push(FrameBuffer {
            kind,
            range: start..self.data.len(),
        });
    }

    /// The first packet of an H.264 or HEVC key frame: parameter sets padded apart, then
    /// the picture data to the end of the packet. Prefix SEI (`sei`, the run before the
    /// parameter sets, and any between them) goes with the picture data, after every
    /// parameter set: renderers hand parameter sets to the decoder on their own, so picture
    /// data before one would be overwritten.
    pub(super) fn process_avc_hevc_payload_slow(
        &mut self,
        data: &[u8],
        mut cur: Desc,
        sei: Range<usize>,
    ) {
        let mut held = std::mem::take(&mut self.held);
        held.clear();
        if !sei.is_empty() {
            held.push(sei);
        }
        while cur.length != 0 {
            if Self::start_sequence(data, cur).is_none() {
                Self::skip_to_next_nal_or_end(data, &mut cur);
            }
            while self.is_access_unit_delimiter(data, cur) {
                Self::skip_to_next_nal_or_end(data, &mut cur);
            }
            if self.is_sei_nal(data, cur) {
                let start = cur.offset;
                Self::skip_to_next_nal_or_end(data, &mut cur);
                held.push(start..cur.offset);
                continue;
            }

            let start = cur.offset;
            let mut contains_pic_data = false;
            if self.is_seq_reference_frame_start(data, cur) {
                self.waiting_for_idr_frame = false;
                self.waiting_for_ref_inval_frame = false;
                self.waiting_for_next_successful_frame = false;
                contains_pic_data = true;
                self.frame_type = FrameType::Idr;
            }

            if contains_pic_data {
                cur.offset += cur.length;
                cur.length = 0;
            } else {
                Self::skip_to_next_nal_or_end(data, &mut cur);
            }
            let fragment = &data[start..cur.offset];
            if self.buffer_type(fragment) == BufferType::PicData {
                for range in held.drain(..) {
                    self.queue_fragment(&data[range]);
                }
            }
            self.queue_fragment(fragment);
        }
        // SEI at the packet's end: the picture data follows in the next packet.
        for range in held.drain(..) {
            self.queue_fragment(&data[range]);
        }
        self.held = held;
    }
}
