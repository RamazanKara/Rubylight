//! The instant-replay RAM ring: whole GOPs of one config epoch plus the Opus packets
//! beside them, evicted a whole GOP at a time from the front by time and byte limits, and
//! snapshotted into a [`Clip`] without copying frame bytes.

use super::{AudioPacket, ParamSets, StreamInfo, VideoFrame};
use std::collections::VecDeque;

/// Audio kept from before the oldest stored keyframe, for capture-latency skew between
/// the audio and video taps.
pub const AUDIO_SLACK_US: u64 = 500_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// How much recent video must stay available (whole GOPs, so up to one GOP more).
    pub max_us: u64,
    /// Video plus audio bytes. Wins over `max_us`, but the newest GOP always stays.
    pub max_bytes: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pushed {
    Stored,
    /// Dropped: the ring needs a keyframe (none yet, a lost tap packet, a changed stream
    /// configuration, or the newest GOP hit the byte limit). Ask the encoder for an IDR.
    AwaitingKeyframe,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapshotError {
    NoKeyframe,
}

impl std::fmt::Display for SnapshotError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the replay buffer holds no keyframe yet")
    }
}

impl std::error::Error for SnapshotError {}

/// A decodable stretch of one config epoch: video from a keyframe, and the audio from
/// the first video frame on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Clip {
    pub info: StreamInfo,
    pub video: Vec<VideoFrame>,
    pub audio: Vec<AudioPacket>,
}

struct Gop {
    frames: Vec<VideoFrame>,
    bytes: usize,
}

impl Gop {
    fn start(&self) -> u64 {
        self.frames.first().map_or(0, |frame| frame.pts_us)
    }
}

struct Config {
    info: StreamInfo,
    params: ParamSets,
}

pub struct Ring {
    limits: Limits,
    gops: VecDeque<Gop>,
    audio: VecDeque<AudioPacket>,
    video_bytes: usize,
    audio_bytes: usize,
    config: Option<Config>,
    awaiting: bool,
    epoch: u64,
}

impl Ring {
    pub fn new(limits: Limits) -> Self {
        Self {
            limits,
            gops: VecDeque::new(),
            audio: VecDeque::new(),
            video_bytes: 0,
            audio_bytes: 0,
            config: None,
            awaiting: false,
            epoch: 0,
        }
    }

    pub fn limits(&self) -> Limits {
        self.limits
    }

    /// Applies new limits, evicting at once if they shrank.
    pub fn set_limits(&mut self, limits: Limits) {
        self.limits = limits;
        self.evict();
    }

    /// Counts config epochs: it advances whenever a keyframe starts a new one.
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// The stream configuration of the stored epoch.
    pub fn info(&self) -> Option<&StreamInfo> {
        self.config.as_ref().map(|config| &config.info)
    }

    pub fn push_video(&mut self, info: &StreamInfo, frame: VideoFrame) -> Pushed {
        let bytes = frame.data.len();
        if frame.idr {
            let sets = ParamSets::from_frame(info.codec, &frame.data);
            let params = if sets.is_complete(info.codec) {
                Some(sets)
            } else {
                // A keyframe without in-band sets continues the epoch it belongs to.
                self.config
                    .as_ref()
                    .filter(|config| config.info.codec == info.codec)
                    .map(|config| config.params.clone())
            };
            let Some(params) = params else {
                return self.await_keyframe();
            };
            let changed = self
                .config
                .as_ref()
                .is_none_or(|config| config.info != *info || config.params != params);
            if changed {
                // A clip never mixes epochs, so the old one can go now.
                self.gops.clear();
                self.video_bytes = 0;
                self.epoch += 1;
                self.config = Some(Config {
                    info: info.clone(),
                    params,
                });
            }
            self.awaiting = false;
            self.gops.push_back(Gop {
                frames: vec![frame],
                bytes,
            });
        } else {
            let same_config = self
                .config
                .as_ref()
                .is_some_and(|config| config.info == *info);
            if self.awaiting || self.gops.is_empty() || !same_config {
                return self.await_keyframe();
            }
            if self.gops.len() == 1 && self.video_bytes + bytes > self.limits.max_bytes {
                // The newest GOP stays whole and decodable; a keyframe starts the next.
                return self.await_keyframe();
            }
            if let Some(gop) = self.gops.back_mut() {
                gop.frames.push(frame);
                gop.bytes += bytes;
            }
        }
        self.video_bytes += bytes;
        self.evict();
        Pushed::Stored
    }

    fn await_keyframe(&mut self) -> Pushed {
        self.awaiting = true;
        Pushed::AwaitingKeyframe
    }

    pub fn push_audio(&mut self, packet: AudioPacket) {
        self.audio_bytes += packet.data.len();
        self.audio.push_back(packet);
        self.evict();
    }

    /// A tap packet was lost: video waits for the next keyframe; stored frames stay.
    pub fn discontinuity(&mut self) {
        self.awaiting = true;
    }

    /// From the oldest stored keyframe to the newest frame.
    pub fn buffered_us(&self) -> u64 {
        match (self.gops.front(), self.newest_video()) {
            (Some(oldest), Some(newest)) => newest.saturating_sub(oldest.start()),
            _ => 0,
        }
    }

    pub fn has_keyframe(&self) -> bool {
        !self.gops.is_empty()
    }

    /// Stored video plus audio bytes.
    pub fn bytes(&self) -> usize {
        self.video_bytes + self.audio_bytes
    }

    /// Drops everything; the next keyframe starts a new epoch.
    pub fn clear(&mut self) {
        self.gops.clear();
        self.audio.clear();
        self.video_bytes = 0;
        self.audio_bytes = 0;
        self.config = None;
        self.awaiting = false;
    }

    /// The last `window_us` of the stored epoch, from the latest keyframe at or before
    /// the window start (the oldest keyframe when the ring holds less).
    pub fn snapshot(&self, window_us: u64) -> Result<Clip, SnapshotError> {
        let (Some(config), Some(newest)) = (&self.config, self.newest_video()) else {
            return Err(SnapshotError::NoKeyframe);
        };
        let target = newest.saturating_sub(window_us);
        let first = self
            .gops
            .iter()
            .rposition(|gop| gop.start() <= target)
            .unwrap_or(0);
        let video: Vec<VideoFrame> = self
            .gops
            .iter()
            .skip(first)
            .flat_map(|gop| gop.frames.iter().cloned())
            .collect();
        let start = video.first().map_or(0, |frame| frame.pts_us);
        let audio = self
            .audio
            .iter()
            .filter(|packet| packet.pts_us >= start)
            .cloned()
            .collect();
        Ok(Clip {
            info: config.info.clone(),
            video,
            audio,
        })
    }

    fn newest_video(&self) -> Option<u64> {
        self.gops
            .back()
            .and_then(|gop| gop.frames.last())
            .map(|frame| frame.pts_us)
    }

    fn evict(&mut self) {
        if let Some(newest) = self.newest_video() {
            let horizon = newest.checked_sub(self.limits.max_us);
            while self.gops.len() > 1 && horizon.is_some_and(|at| self.gops[1].start() <= at) {
                self.pop_gop();
            }
        }
        while self.gops.len() > 1 && self.bytes() > self.limits.max_bytes {
            self.pop_gop();
        }
        let floor = match self.gops.front() {
            Some(oldest) => oldest.start(),
            None => self.audio.back().map_or(0, |packet| packet.pts_us),
        }
        .saturating_sub(AUDIO_SLACK_US);
        while self
            .audio
            .front()
            .is_some_and(|packet| packet.pts_us < floor || self.bytes() > self.limits.max_bytes)
        {
            self.pop_audio();
        }
    }

    fn pop_gop(&mut self) {
        if let Some(gop) = self.gops.pop_front() {
            self.video_bytes -= gop.bytes;
        }
    }

    fn pop_audio(&mut self) {
        if let Some(packet) = self.audio.pop_front() {
            self.audio_bytes -= packet.data.len();
        }
    }
}
