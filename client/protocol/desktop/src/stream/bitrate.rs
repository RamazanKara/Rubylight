//! Automatic bitrate: nightjar-policy's controllers, Android's, picked by the stream's codec.

use nightjar_core::sdp::{VIDEO_FORMAT_MASK_AV1, VIDEO_FORMAT_MASK_H264, VIDEO_FORMAT_MASK_H265, VIDEO_FORMAT_MASK_PYROWAVE};
use nightjar_policy::bitrate::{AdaptiveBitrateController, Codec};
use nightjar_policy::pyrowave::BitrateController;
use nightjar_policy::stats::Stats;

pub enum Bitrate {
    Codec(AdaptiveBitrateController),
    PyroWave(BitrateController),
}

/// The codec the built-in floor follows; PyroWave has its own controller.
fn codec(format: u32) -> Option<Codec> {
    if format & VIDEO_FORMAT_MASK_AV1 != 0 {
        Some(Codec::Av1)
    } else if format & VIDEO_FORMAT_MASK_H265 != 0 {
        Some(Codec::Hevc)
    } else if format & VIDEO_FORMAT_MASK_H264 != 0 {
        Some(Codec::H264)
    } else {
        None
    }
}

impl Bitrate {
    /// Time is session-relative milliseconds, as in every [`Stats`] tick.
    pub fn new(format: u32, ceiling: u32, current: u32, fps: u32, (width, height): (u32, u32), now_ms: u64) -> Self {
        if format & VIDEO_FORMAT_MASK_PYROWAVE != 0 {
            Self::PyroWave(BitrateController::new(ceiling, current, fps, now_ms))
        } else {
            Self::Codec(AdaptiveBitrateController::with_stream(ceiling, current, fps, now_ms, width, height, codec(format)))
        }
    }

    /// PyroWave's loss counts the frames recovered without their records too, as on Android.
    pub fn sample(&mut self, stats: &Stats) -> Option<u32> {
        match self {
            Self::Codec(policy) => policy.sample(stats),
            Self::PyroWave(policy) => policy.sample(stats),
        }
    }

    pub fn applied(&mut self, requested: u32, applied: u32, now_ms: u64) {
        match self {
            Self::Codec(policy) => policy.applied(requested, applied, now_ms),
            Self::PyroWave(policy) => policy.applied(requested, applied, now_ms),
        }
    }

    pub fn suspend(&mut self) {
        match self {
            Self::Codec(policy) => policy.suspend(),
            Self::PyroWave(policy) => policy.suspend(),
        }
    }

    /// Whether network pressure lowers the bitrate before automatic quality lowers the size.
    /// PyroWave's controller has no such room, as Android's quality ladder sees it.
    pub fn has_room(&self) -> bool {
        match self {
            Self::Codec(policy) => policy.has_room_before_quality_reduction(),
            Self::PyroWave(_) => false,
        }
    }

    /// A new size or frame rate from automatic quality or the display. PyroWave keeps the frame
    /// rate it started with, as on Android.
    pub fn set_mode(&mut self, width: u32, height: u32, fps: u32) {
        if let Self::Codec(policy) = self {
            policy.set_frame_rate(fps);
            policy.set_stream_size(width, height);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tick(now_ms: u64, loss: f32, decode: f32, jitter: f32) -> Stats {
        Stats {
            now_ms,
            recent_video: true,
            fps: 60,
            loss_percent: Some(loss),
            decode_ms: Some(decode),
            jitter_ms: Some(jitter),
            queue_ms: Some(1.0),
            ..Default::default()
        }
    }

    #[test]
    fn congestion_requires_sustained_pressure_and_a_host_cap_is_kept() {
        let mut rate = Bitrate::new(VIDEO_FORMAT_MASK_H265, 20_000, 20_000, 60, (1920, 1080), 0);
        assert_eq!(rate.sample(&tick(1000, 8.0, 2.0, 1.0)), None);
        assert_eq!(rate.sample(&tick(2000, 8.0, 2.0, 1.0)), None);
        assert_eq!(rate.sample(&tick(3000, 8.0, 2.0, 1.0)), Some(16_000));
        rate.applied(16_000, 15_000, 3000);
        assert!(rate.has_room());
        rate.suspend();
        assert_eq!(rate.sample(&Stats { now_ms: 4000, ..Default::default() }), None);
        // The host capped the rate at 15 Mbps: recovery never asks above it.
        for now in 5..60 {
            assert!(rate.sample(&tick(now * 1000, 0.0, 2.0, 1.0)).is_none_or(|kbps| kbps <= 15_000));
        }
    }

    #[test]
    fn pyrowave_reacts_to_rising_loss_and_leaves_room_to_the_ladder() {
        let mut rate = Bitrate::new(VIDEO_FORMAT_MASK_PYROWAVE, 40_000, 40_000, 60, (1920, 1080), 0);
        assert!(matches!(rate, Bitrate::PyroWave(_)));
        assert!(!rate.has_room());
        assert_eq!(rate.sample(&tick(1000, 0.0, 2.0, 1.0)), None);
        assert_eq!(rate.sample(&tick(2000, 0.5, 2.0, 1.0)), None);
        assert_eq!(rate.sample(&tick(3000, 0.8, 2.0, 1.0)), Some(34_000));
        rate.applied(34_000, 34_000, 3000);
        assert_eq!(rate.sample(&tick(4000, 0.0, 2.0, 1.0)), None);
        assert_eq!(rate.sample(&tick(24_000, 0.0, 2.0, 1.0)), Some(34_680));
    }

    #[test]
    fn the_floor_follows_codec_and_size() {
        assert_eq!(codec(VIDEO_FORMAT_MASK_H264), Some(Codec::H264));
        assert_eq!(codec(0x0100), Some(Codec::Hevc));
        assert_eq!(codec(0x1000), Some(Codec::Av1));
        assert_eq!(codec(0), None);
        // HEVC 4K120 has a floor well above 2 Mbps; a lower frame rate lowers it with the ladder.
        let mut rate = Bitrate::new(VIDEO_FORMAT_MASK_H265, 80_000, 80_000, 120, (3840, 2160), 0);
        let mut now = 0;
        let mut cut_to = |rate: &mut Bitrate| {
            let mut last = None;
            for _ in 0..40 {
                now += 3000;
                if let Some(kbps) = rate.sample(&tick(now, 8.0, 2.0, 1.0)) {
                    rate.applied(kbps, kbps, now);
                    last = Some(kbps);
                }
            }
            last
        };
        let floor_120 = cut_to(&mut rate).unwrap();
        assert!(floor_120 > 2000, "{floor_120}");
        rate.set_mode(1280, 720, 30);
        assert!(cut_to(&mut rate).unwrap() < floor_120);
    }
}
