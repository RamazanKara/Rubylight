//! Fragmented MP4 (ISO BMFF) clip muxer: an init segment (`ftyp` + `moov`) with one video
//! track (H.264, HEVC or AV1) and an optional Opus track, then one `moof` + `mdat` per
//! GOP. Also owns what the live [`super::recorder`] shares: the video track description,
//! sample conversion (Annex-B to length-prefixed NAL units without AUD or parameter sets;
//! AV1 OBUs with size fields and no temporal delimiters) and the two track timelines.
//!
//! Decode order is presentation order (the host encoders emit no B-frames). Both tracks
//! start at the clip's first video frame: audio's `tfdt` is its offset from there.

pub(crate) mod boxes;
pub(crate) mod codec;

use super::{AudioPacket, Clip, ParamSets, StreamInfo, VideoCodec, VideoFrame, av1, nal, ticks};
use boxes::{Bmff, Run, SampleMeta, TrackBox};
use std::io::{self, Write};

pub const VIDEO_TIMESCALE: u32 = 90_000;
pub const AUDIO_TIMESCALE: u32 = 48_000;
/// `mvhd` and `mehd` timescale.
pub const MOVIE_TIMESCALE: u32 = 1_000;
pub const VIDEO_TRACK: u32 = 1;
pub const AUDIO_TRACK: u32 = 2;
/// The last frame's duration when nothing follows it and nothing preceded it (60 fps).
const DEFAULT_FRAME_TICKS: u32 = VIDEO_TIMESCALE / 60;

/// What a written clip or recording part holds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClipInfo {
    pub bytes: u64,
    pub duration_ms: u64,
    pub width: u32,
    pub height: u32,
    pub fps: f32,
    pub codec: VideoCodec,
    pub hdr: bool,
    pub ten_bit: bool,
}

#[derive(Debug)]
pub enum MuxError {
    /// The clip has no video frames.
    NoVideo,
    /// The first video frame is not a keyframe.
    NotKeyframe,
    /// The keyframe lacks the parameter sets (SPS/PPS/VPS, AV1 sequence header).
    MissingParameterSets,
    /// The parameter sets do not parse.
    BadParameterSets,
    Io(io::Error),
}

impl std::fmt::Display for MuxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoVideo => f.write_str("the clip has no video"),
            Self::NotKeyframe => f.write_str("the clip does not start on a keyframe"),
            Self::MissingParameterSets => f.write_str("the keyframe carries no parameter sets"),
            Self::BadParameterSets => f.write_str("the keyframe's parameter sets do not parse"),
            Self::Io(error) => write!(f, "writing the clip failed: {error}"),
        }
    }
}

impl std::error::Error for MuxError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for MuxError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// The video track as a keyframe's parameter sets describe it.
pub(crate) struct VideoTrack {
    pub codec: VideoCodec,
    pub width: u32,
    pub height: u32,
    pub ten_bit: bool,
    pub sample_entry: Vec<u8>,
}

pub(crate) fn video_track(
    info: &StreamInfo,
    idr: &VideoFrame,
) -> Result<(VideoTrack, ParamSets), MuxError> {
    if !idr.idr {
        return Err(MuxError::NotKeyframe);
    }
    let params = ParamSets::from_frame(info.codec, &idr.data);
    if !params.is_complete(info.codec) {
        return Err(MuxError::MissingParameterSets);
    }
    let hdr = info.hdr.as_ref();
    let (sample_entry, width, height, depth) = match info.codec {
        VideoCodec::H264 => {
            let parsed = params
                .sps
                .first()
                .and_then(|unit| nal::h264_sps(unit))
                .ok_or(MuxError::BadParameterSets)?;
            let config = codec::Config::Avc {
                sps: &params.sps,
                pps: &params.pps,
                parsed,
            };
            let entry = codec::visual_entry(&config, parsed.width, parsed.height, hdr);
            (entry, parsed.width, parsed.height, parsed.bit_depth_luma)
        }
        VideoCodec::Hevc => {
            let parsed = params
                .sps
                .first()
                .and_then(|unit| nal::hevc_sps(unit))
                .ok_or(MuxError::BadParameterSets)?;
            let config = codec::Config::Hevc {
                vps: &params.vps,
                sps: &params.sps,
                pps: &params.pps,
                parsed,
            };
            let entry = codec::visual_entry(&config, parsed.width, parsed.height, hdr);
            (entry, parsed.width, parsed.height, parsed.bit_depth_luma)
        }
        VideoCodec::Av1 => {
            let sequence_header = params.sequence_header.as_deref().unwrap_or_default();
            let parsed = av1::obus(sequence_header)
                .next()
                .and_then(|obu| av1::sequence_header(obu.payload))
                .ok_or(MuxError::BadParameterSets)?;
            let config = codec::Config::Av1 {
                sequence_header,
                parsed,
            };
            let entry = codec::visual_entry(&config, parsed.width, parsed.height, hdr);
            (entry, parsed.width, parsed.height, parsed.bit_depth())
        }
    };
    let track = VideoTrack {
        codec: info.codec,
        width,
        height,
        ten_bit: depth > 8,
        sample_entry,
    };
    Ok((track, params))
}

/// `ftyp` + `moov`, and the offset of the `mehd` 64-bit duration (milliseconds) in it.
pub(crate) fn init_segment(
    info: &StreamInfo,
    track: &VideoTrack,
    duration_ms: u64,
) -> (Vec<u8>, usize) {
    let mut b = Bmff::new();
    boxes::ftyp(
        &mut b,
        match track.codec {
            VideoCodec::H264 => Some(b"avc1"),
            VideoCodec::Hevc => None,
            VideoCodec::Av1 => Some(b"av01"),
        },
    );
    let audio_entry = info.audio.as_ref().map(codec::opus_entry);
    let mut tracks = vec![TrackBox {
        id: VIDEO_TRACK,
        audio: false,
        width: track.width,
        height: track.height,
        sample_entry: &track.sample_entry,
    }];
    if let Some(entry) = &audio_entry {
        tracks.push(TrackBox {
            id: AUDIO_TRACK,
            audio: true,
            width: 0,
            height: 0,
            sample_entry: entry,
        });
    }
    let mehd = boxes::moov(&mut b, &tracks, duration_ms);
    (b.buf, mehd)
}

/// Appends one access unit as an MP4 sample.
pub(crate) fn append_sample(codec: VideoCodec, data: &[u8], out: &mut Vec<u8>) {
    match codec {
        VideoCodec::H264 | VideoCodec::Hevc => {
            for unit in nal::units(data) {
                if keep_nal(codec, unit) {
                    out.extend_from_slice(&(unit.len() as u32).to_be_bytes());
                    out.extend_from_slice(unit);
                }
            }
        }
        VideoCodec::Av1 => {
            for obu in av1::obus(data) {
                if !matches!(
                    obu.kind,
                    av1::OBU_TEMPORAL_DELIMITER | av1::OBU_TILE_LIST | av1::OBU_PADDING
                ) {
                    obu.write_sized(out);
                }
            }
        }
    }
}

/// Parameter sets live in the sample entry (`avc1`/`hvc1`); AUD and filler carry nothing.
fn keep_nal(codec: VideoCodec, unit: &[u8]) -> bool {
    match codec {
        VideoCodec::H264 => !matches!(
            nal::h264_type(unit),
            Some(
                nal::H264_SPS
                    | nal::H264_PPS
                    | nal::H264_AUD
                    | nal::H264_FILLER
                    | nal::H264_SPS_EXT
            )
        ),
        _ => !matches!(
            nal::hevc_type(unit),
            Some(nal::HEVC_VPS | nal::HEVC_SPS | nal::HEVC_PPS | nal::HEVC_AUD | nal::HEVC_FILLER)
        ),
    }
}

fn clamp_u32(value: u64) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

pub(crate) fn to_ms(ticks: u64, timescale: u32) -> u64 {
    let scale = u64::from(timescale);
    ticks.saturating_mul(1000).saturating_add(scale / 2) / scale
}

/// Video decode times at 90 kHz from frame timestamps: strictly increasing, each frame
/// lasting until the next one, the last as long as the one before.
pub(crate) struct VideoTimeline {
    start_us: u64,
    end: u64,
    last_duration: u32,
    frames: u64,
}

impl VideoTimeline {
    pub fn new(start_us: u64) -> Self {
        Self {
            start_us,
            end: 0,
            last_duration: DEFAULT_FRAME_TICKS,
            frames: 0,
        }
    }

    fn position(&self, pts_us: u64) -> u64 {
        ticks(pts_us.saturating_sub(self.start_us), VIDEO_TIMESCALE)
    }

    /// The base decode time and durations of `frames`; `next_pts` is the frame after them.
    pub fn layout(&mut self, frames: &[VideoFrame], next_pts: Option<u64>) -> (u64, Vec<u32>) {
        let mut positions = Vec::with_capacity(frames.len());
        let mut cursor = self.end;
        for frame in frames {
            let at = self.position(frame.pts_us).max(cursor);
            positions.push(at);
            cursor = at + 1;
        }
        let next = next_pts.map(|pts| self.position(pts).max(cursor));
        let base = positions.first().copied().unwrap_or(self.end);
        let mut durations = Vec::with_capacity(frames.len());
        for (index, &at) in positions.iter().enumerate() {
            let duration = match positions.get(index + 1).copied().or(next) {
                Some(following) => {
                    self.last_duration = clamp_u32(following - at);
                    self.last_duration
                }
                None => self.last_duration,
            };
            durations.push(duration);
        }
        self.end = base + durations.iter().map(|&d| u64::from(d)).sum::<u64>();
        self.frames += frames.len() as u64;
        (base, durations)
    }

    /// End of the last laid-out frame, at 90 kHz.
    pub fn end(&self) -> u64 {
        self.end
    }

    pub fn fps(&self) -> f32 {
        if self.end == 0 {
            return 0.0;
        }
        (self.frames as f64 * f64::from(VIDEO_TIMESCALE) / self.end as f64) as f32
    }
}

/// Opus decode times at 48 kHz: packets play back to back for their sample counts, and a
/// gap of more than half a packet in the timestamps (lost packets) is kept as a gap.
pub(crate) struct AudioTimeline {
    start_us: u64,
    end: Option<u64>,
}

impl AudioTimeline {
    pub fn new(start_us: u64) -> Self {
        Self {
            start_us,
            end: None,
        }
    }

    fn position(&self, pts_us: u64) -> u64 {
        ticks(pts_us.saturating_sub(self.start_us), AUDIO_TIMESCALE)
    }

    /// The base decode time and durations of `packets`; `next_pts` is the packet after.
    pub fn layout(&mut self, packets: &[AudioPacket], next_pts: Option<u64>) -> (u64, Vec<u32>) {
        let Some(first) = packets.first() else {
            return (self.end.unwrap_or(0), Vec::new());
        };
        let expected = self.position(first.pts_us);
        let base = match self.end {
            Some(end) if expected <= end + u64::from(first.samples / 2) => end,
            _ => expected,
        };
        let mut cursor = base;
        let mut durations = Vec::with_capacity(packets.len());
        for (index, packet) in packets.iter().enumerate() {
            let samples = u64::from(packet.samples.max(1));
            let following = packets
                .get(index + 1)
                .map(|next| next.pts_us)
                .or(next_pts)
                .map(|pts| self.position(pts));
            let duration = match following {
                Some(next) if next > cursor + samples + samples / 2 => next - cursor,
                _ => samples,
            };
            durations.push(clamp_u32(duration));
            cursor += u64::from(clamp_u32(duration));
        }
        self.end = Some(cursor);
        (base, durations)
    }

    /// End of the last laid-out packet, at 48 kHz.
    pub fn end(&self) -> Option<u64> {
        self.end
    }
}

/// Converts `frames` into one video run's samples and payload.
pub(crate) fn video_samples(
    codec: VideoCodec,
    frames: &[VideoFrame],
    durations: &[u32],
) -> (Vec<SampleMeta>, Vec<u8>) {
    let mut payload = Vec::with_capacity(frames.iter().map(|frame| frame.data.len()).sum());
    let samples = frames
        .iter()
        .zip(durations)
        .map(|(frame, &duration)| {
            let before = payload.len();
            append_sample(codec, &frame.data, &mut payload);
            SampleMeta {
                size: clamp_u32((payload.len() - before) as u64),
                duration,
                sync: frame.idr,
            }
        })
        .collect();
    (samples, payload)
}

/// Converts Opus `packets` into one audio run's samples and payload.
pub(crate) fn audio_samples(
    packets: &[AudioPacket],
    durations: &[u32],
) -> (Vec<SampleMeta>, Vec<u8>) {
    let mut payload = Vec::with_capacity(packets.iter().map(|packet| packet.data.len()).sum());
    let samples = packets
        .iter()
        .zip(durations)
        .map(|(packet, &duration)| {
            payload.extend_from_slice(&packet.data);
            SampleMeta {
                size: clamp_u32(packet.data.len() as u64),
                duration,
                sync: true,
            }
        })
        .collect();
    (samples, payload)
}

/// One fragment of video frames and the audio packets beside them.
pub(crate) fn mux_fragment(
    sequence: u32,
    codec: VideoCodec,
    video: (u64, &[VideoFrame], &[u32]),
    audio: (u64, &[AudioPacket], &[u32]),
) -> Vec<u8> {
    let (video_samples, video_payload) = video_samples(codec, video.1, video.2);
    let (audio_samples, audio_payload) = audio_samples(audio.1, audio.2);
    boxes::fragment(
        sequence,
        &[
            Run {
                track_id: VIDEO_TRACK,
                base_time: video.0,
                samples: &video_samples,
                payload: &video_payload,
                sample_flags: true,
            },
            Run {
                track_id: AUDIO_TRACK,
                base_time: audio.0,
                samples: &audio_samples,
                payload: &audio_payload,
                sample_flags: false,
            },
        ],
    )
}

/// Writes `clip` as a complete fragmented MP4: init segment, then one fragment per GOP.
pub fn write_clip<W: Write>(out: &mut W, clip: &Clip) -> Result<ClipInfo, MuxError> {
    let first = clip.video.first().ok_or(MuxError::NoVideo)?;
    let (track, _) = video_track(&clip.info, first)?;
    let start = first.pts_us;
    let audio: &[AudioPacket] = if clip.info.audio.is_some() {
        &clip.audio[clip.audio.partition_point(|packet| packet.pts_us < start)..]
    } else {
        &[]
    };
    let mut video_clock = VideoTimeline::new(start);
    video_clock.layout(&clip.video, None);
    let mut audio_clock = AudioTimeline::new(start);
    audio_clock.layout(audio, None);
    let video_ms = to_ms(video_clock.end(), VIDEO_TIMESCALE);
    let audio_ms = audio_clock
        .end()
        .map_or(0, |end| to_ms(end, AUDIO_TIMESCALE));
    let duration_ms = video_ms.max(audio_ms);
    let fps = video_clock.fps();

    let (init, _) = init_segment(&clip.info, &track, duration_ms);
    out.write_all(&init)?;
    let mut written = init.len() as u64;
    let mut video_clock = VideoTimeline::new(start);
    let mut audio_clock = AudioTimeline::new(start);
    let mut audio_rest = audio;
    let starts: Vec<usize> = (0..clip.video.len())
        .filter(|&index| index == 0 || clip.video[index].idr)
        .collect();
    for (sequence, (index, &from)) in (1u32..).zip(starts.iter().enumerate()) {
        let to = starts.get(index + 1).copied().unwrap_or(clip.video.len());
        let frames = &clip.video[from..to];
        let next_pts = clip.video.get(to).map(|frame| frame.pts_us);
        let (video_base, video_durations) = video_clock.layout(frames, next_pts);
        let cut = next_pts.map_or(audio_rest.len(), |boundary| {
            audio_rest.partition_point(|packet| packet.pts_us < boundary)
        });
        let (packets, rest) = audio_rest.split_at(cut);
        let (audio_base, audio_durations) =
            audio_clock.layout(packets, rest.first().map(|packet| packet.pts_us));
        audio_rest = rest;
        let fragment = mux_fragment(
            sequence,
            track.codec,
            (video_base, frames, &video_durations),
            (audio_base, packets, &audio_durations),
        );
        out.write_all(&fragment)?;
        written += fragment.len() as u64;
    }
    Ok(ClipInfo {
        bytes: written,
        duration_ms,
        width: track.width,
        height: track.height,
        fps,
        codec: track.codec,
        hdr: clip.info.hdr.is_some(),
        ten_bit: track.ten_bit,
    })
}
