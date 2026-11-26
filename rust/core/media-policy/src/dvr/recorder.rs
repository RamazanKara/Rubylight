//! Live manual recording as fragmented MP4, streamed to disk by the host: the init segment
//! up front (with a placeholder `mehd` duration), a `moof` + `mdat` for about every
//! [`FRAGMENT_US`] of video, then a tail fragment and the in-place duration patch. A
//! configuration change at a keyframe ends the part so the host can start the next one.

use super::fmp4::{
    self, AUDIO_TIMESCALE, AudioTimeline, ClipInfo, MuxError, VIDEO_TIMESCALE, VideoTimeline,
};
use super::{AudioPacket, ParamSets, StreamInfo, VideoCodec, VideoFrame};

/// Buffered video that makes a fragment.
pub const FRAGMENT_US: u64 = 2_000_000;

#[derive(Debug)]
pub enum RecPush {
    Buffered,
    /// A `moof` + `mdat` to append to the file.
    Fragment(Vec<u8>),
    /// The stream configuration changed at this keyframe: finish this part, then start
    /// the next one with the frame.
    Restart(VideoFrame),
    /// Dropped until the next keyframe (a lost tap packet or a configuration change on a
    /// non-keyframe). Ask the encoder for an IDR.
    AwaitingKeyframe,
}

#[derive(Debug)]
pub struct Finished {
    /// The last fragment to append (empty when nothing was pending).
    pub tail: Vec<u8>,
    /// Bytes to write at a file offset: the final `mehd` duration.
    pub duration_patch: Option<(u64, Vec<u8>)>,
    pub info: ClipInfo,
}

pub struct Recorder {
    info: StreamInfo,
    params: ParamSets,
    codec: VideoCodec,
    width: u32,
    height: u32,
    ten_bit: bool,
    start_us: u64,
    sequence: u32,
    video: Vec<VideoFrame>,
    audio: Vec<AudioPacket>,
    video_clock: VideoTimeline,
    audio_clock: AudioTimeline,
    bytes: u64,
    mehd_at: u64,
    awaiting: bool,
}

impl Recorder {
    /// Starts a part at `first_idr`; returns the recorder and the init segment to write
    /// at the start of the file.
    pub fn start(info: &StreamInfo, first_idr: VideoFrame) -> Result<(Self, Vec<u8>), MuxError> {
        let (track, params) = fmp4::video_track(info, &first_idr)?;
        let (init, mehd_at) = fmp4::init_segment(info, &track, 0);
        let start_us = first_idr.pts_us;
        let recorder = Self {
            info: info.clone(),
            params,
            codec: track.codec,
            width: track.width,
            height: track.height,
            ten_bit: track.ten_bit,
            start_us,
            sequence: 1,
            video: vec![first_idr],
            audio: Vec::new(),
            video_clock: VideoTimeline::new(start_us),
            audio_clock: AudioTimeline::new(start_us),
            bytes: init.len() as u64,
            mehd_at: mehd_at as u64,
            awaiting: false,
        };
        Ok((recorder, init))
    }

    pub fn push_video(&mut self, info: &StreamInfo, frame: VideoFrame) -> RecPush {
        if frame.idr {
            let sets = ParamSets::from_frame(info.codec, &frame.data);
            if *info != self.info || (sets.is_complete(info.codec) && sets != self.params) {
                return RecPush::Restart(frame);
            }
            self.awaiting = false;
        } else if self.awaiting || *info != self.info {
            self.awaiting = true;
            return RecPush::AwaitingKeyframe;
        }
        let due = self
            .video
            .first()
            .is_some_and(|first| frame.pts_us.saturating_sub(first.pts_us) >= FRAGMENT_US);
        let next_pts = frame.pts_us;
        if due {
            let fragment = self.flush(Some(next_pts));
            self.video.push(frame);
            return RecPush::Fragment(fragment);
        }
        self.video.push(frame);
        RecPush::Buffered
    }

    pub fn push_audio(&mut self, packet: AudioPacket) {
        if self.info.audio.is_some() && packet.pts_us >= self.start_us {
            self.audio.push(packet);
        }
    }

    /// A tap packet was lost: video waits for the next keyframe.
    pub fn discontinuity(&mut self) {
        self.awaiting = true;
    }

    /// Bytes handed out so far (init segment and fragments).
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    /// Recorded video duration so far, including buffered frames.
    pub fn duration_us(&self) -> u64 {
        self.video
            .last()
            .map_or(0, |last| last.pts_us.saturating_sub(self.start_us))
    }

    pub fn info(&self) -> &StreamInfo {
        &self.info
    }

    /// The pending frames up to `next_pts`, and the audio before it, as one fragment.
    fn flush(&mut self, next_pts: Option<u64>) -> Vec<u8> {
        let frames = std::mem::take(&mut self.video);
        let cut = next_pts.map_or(self.audio.len(), |boundary| {
            self.audio
                .partition_point(|packet| packet.pts_us < boundary)
        });
        let rest = self.audio.split_off(cut);
        let packets = std::mem::replace(&mut self.audio, rest);
        if frames.is_empty() && packets.is_empty() {
            return Vec::new();
        }
        let (video_base, video_durations) = self.video_clock.layout(&frames, next_pts);
        let after = self.audio.first().map(|packet| packet.pts_us);
        let (audio_base, audio_durations) = self.audio_clock.layout(&packets, after);
        let fragment = fmp4::mux_fragment(
            self.sequence,
            self.codec,
            (video_base, &frames, &video_durations),
            (audio_base, &packets, &audio_durations),
        );
        self.sequence += 1;
        self.bytes += fragment.len() as u64;
        fragment
    }

    /// Ends the part: the tail fragment, the duration patch and what the file holds.
    pub fn finish(mut self) -> Finished {
        let tail = self.flush(None);
        let video_ms = fmp4::to_ms(self.video_clock.end(), VIDEO_TIMESCALE);
        let audio_ms = self
            .audio_clock
            .end()
            .map_or(0, |end| fmp4::to_ms(end, AUDIO_TIMESCALE));
        let duration_ms = video_ms.max(audio_ms);
        Finished {
            tail,
            duration_patch: Some((self.mehd_at, duration_ms.to_be_bytes().to_vec())),
            info: ClipInfo {
                bytes: self.bytes,
                duration_ms,
                width: self.width,
                height: self.height,
                fps: self.video_clock.fps(),
                codec: self.codec,
                hdr: self.info.hdr.is_some(),
                ten_bit: self.ten_bit,
            },
        }
    }
}
