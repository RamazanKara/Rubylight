//! Live manual recording as fragmented MP4, streamed to disk by the host: the init segment
//! up front (with a placeholder `mehd` duration), a `moof` + `mdat` for about every
//! [`FRAGMENT_US`] of video, then a tail fragment and the in-place duration patch. A
//! configuration change at a keyframe ends the part so the host can start the next one,
//! and a fragment the host could not write ends it at the fragment before.

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

/// The file before the latest fragment: its length and the timelines' state then.
struct Written {
    bytes: u64,
    video_clock: VideoTimeline,
    audio_clock: AudioTimeline,
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
    written: Written,
}

impl Recorder {
    /// Starts a part at `first_idr`; returns the recorder and the init segment to write
    /// at the start of the file.
    pub fn start(info: &StreamInfo, first_idr: VideoFrame) -> Result<(Self, Vec<u8>), MuxError> {
        Self::start_with_params(info, first_idr, None)
    }

    /// Starts a part at `first_idr`, falling back to `fallback` parameter sets when
    /// `first_idr` carries none in band.
    pub fn start_with_params(
        info: &StreamInfo,
        first_idr: VideoFrame,
        fallback: Option<&ParamSets>,
    ) -> Result<(Self, Vec<u8>), MuxError> {
        let (track, params) = fmp4::video_track(info, &first_idr, fallback)?;
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
            written: Written {
                bytes: init.len() as u64,
                video_clock: VideoTimeline::new(start_us),
                audio_clock: AudioTimeline::new(start_us),
            },
        };
        Ok((recorder, init))
    }

    pub fn push_video(&mut self, info: &StreamInfo, frame: VideoFrame) -> RecPush {
        if frame.idr {
            let sets = ParamSets::from_frame(info.codec, &frame.data);
            let complete = sets.is_complete(info.codec);
            if info.codec != self.info.codec && !complete {
                self.awaiting = true;
                return RecPush::AwaitingKeyframe;
            }
            if *info != self.info || (complete && sets != self.params) {
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

    pub fn params(&self) -> &ParamSets {
        &self.params
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
        self.written = Written {
            bytes: self.bytes,
            video_clock: self.video_clock.clone(),
            audio_clock: self.audio_clock.clone(),
        };
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

    /// Ends the part: the tail fragment, the duration patch and what the file holds. When
    /// writing the tail fails, [`super::salvage`] of the file finds the end and duration.
    pub fn finish(mut self) -> Finished {
        let tail = self.flush(None);
        self.finished(tail, self.bytes, &self.video_clock, &self.audio_clock)
    }

    /// Ends the part when the host could not write the last [`RecPush::Fragment`] it got
    /// (a failed or refused write), instead of [`Recorder::finish`]: no tail, and the
    /// duration patch and `info` describe the file up to the fragment before it (the init
    /// segment alone when it was the first). The host truncates the file to `info.bytes`,
    /// applies the patch and keeps the part.
    pub fn finish_written(self) -> Finished {
        let written = &self.written;
        let (video, audio) = (&written.video_clock, &written.audio_clock);
        self.finished(Vec::new(), written.bytes, video, audio)
    }

    fn finished(
        &self,
        tail: Vec<u8>,
        bytes: u64,
        video_clock: &VideoTimeline,
        audio_clock: &AudioTimeline,
    ) -> Finished {
        let video_ms = fmp4::to_ms(video_clock.end(), VIDEO_TIMESCALE);
        let audio_ms = audio_clock
            .end()
            .map_or(0, |end| fmp4::to_ms(end, AUDIO_TIMESCALE));
        let duration_ms = video_ms.max(audio_ms);
        Finished {
            tail,
            duration_patch: Some((self.mehd_at, duration_ms.to_be_bytes().to_vec())),
            info: ClipInfo {
                bytes,
                duration_ms,
                width: self.width,
                height: self.height,
                fps: video_clock.fps(),
                codec: self.codec,
                hdr: self.info.hdr.is_some(),
                ten_bit: self.ten_bit,
            },
        }
    }
}
