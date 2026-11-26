//! Stream DVR core: the instant-replay RAM ring, slice reassembly, the soft keyframe
//! cadence, clip quota planning, and the fragmented MP4 clip muxer, live recorder and
//! crash salvage. Pure and OS-free: no threads and no files; the host's `dvr-mux` worker
//! owns the channel, the clock and the disk.
//!
//! Every timestamp is `pts_us`, microseconds on one monotonic host timeline that video
//! and audio share.

use std::sync::Arc;

pub mod av1;
pub mod cadence;
pub mod fmp4;
pub mod nal;
pub mod params;
pub mod quota;
pub mod recorder;
pub mod recover;
pub mod ring;
pub mod slices;

#[cfg(test)]
mod tests;

pub use cadence::{GOP_SECS, KeyframeCadence, gop};
pub use fmp4::{ClipInfo, MuxError, write_clip};
pub use params::ParamSets;
pub use quota::{
    ClipKind, DEFAULT_QUOTA_BYTES, DISK_FLOOR_BYTES, Refusal, StoredClip, plan, valid_clip_id,
};
pub use recorder::{FRAGMENT_US, Finished, RecPush, Recorder};
pub use recover::{Salvage, salvage};
pub use ring::{Clip, Limits, Pushed, Ring, SnapshotError};
pub use slices::{Reassembler, UnitKind};

/// Video codec of a recordable stream. PyroWave has no MP4 mapping and is refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum VideoCodec {
    H264,
    Hevc,
    Av1,
}

impl VideoCodec {
    /// The negotiated stream codec: 0 H.264, 1 HEVC, 2 AV1; PyroWave (3) and anything
    /// else cannot be recorded.
    pub fn from_negotiated(codec: u8) -> Option<Self> {
        match codec {
            0 => Some(Self::H264),
            1 => Some(Self::Hevc),
            2 => Some(Self::Av1),
            _ => None,
        }
    }
}

/// An HDR stream: BT.2020 primaries with the PQ (SMPTE ST 2084) transfer, plus the
/// mastering display and content light levels when the host knows them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HdrInfo {
    pub metadata: Option<crate::hdr::Metadata>,
}

/// The host's Opus multistream layout. `mapping[i]` is the coded channel of input
/// channel `i`, in WAVE/GameStream order (FL FR FC LFE BL BR SL SR).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpusConfig {
    pub channels: u8,
    pub streams: u8,
    pub coupled: u8,
    pub mapping: Vec<u8>,
}

impl OpusConfig {
    /// Plain stereo: one coupled stream.
    pub fn stereo() -> Self {
        Self {
            channels: 2,
            streams: 1,
            coupled: 1,
            mapping: vec![0, 1],
        }
    }
}

/// What a clip's tracks are made of. A change starts a new config epoch at the next
/// keyframe (see [`Ring`]) or a new recording part (see [`Recorder`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamInfo {
    pub codec: VideoCodec,
    pub hdr: Option<HdrInfo>,
    /// `None` when the stream's audio is not Opus (passthrough): video-only clips.
    pub audio: Option<OpusConfig>,
}

/// One whole access unit: Annex-B for H.264 and HEVC (may carry AUD, SEI and parameter
/// sets), low-overhead OBUs for AV1 (may carry temporal delimiters).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VideoFrame {
    pub pts_us: u64,
    pub idr: bool,
    pub data: Arc<Vec<u8>>,
}

impl VideoFrame {
    pub fn new(pts_us: u64, idr: bool, data: Vec<u8>) -> Self {
        Self {
            pts_us,
            idr,
            data: Arc::new(data),
        }
    }
}

/// One Opus packet; `samples` is its duration at 48 kHz.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioPacket {
    pub pts_us: u64,
    pub samples: u32,
    pub data: Arc<Vec<u8>>,
}

impl AudioPacket {
    pub fn new(pts_us: u64, samples: u32, data: Vec<u8>) -> Self {
        Self {
            pts_us,
            samples,
            data: Arc::new(data),
        }
    }
}

/// `us` microseconds in `timescale` units, rounded to nearest.
pub(crate) fn ticks(us: u64, timescale: u32) -> u64 {
    let scaled = (u128::from(us) * u128::from(timescale) + 500_000) / 1_000_000;
    u64::try_from(scaled).unwrap_or(u64::MAX)
}
