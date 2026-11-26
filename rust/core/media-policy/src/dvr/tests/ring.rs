//! Replay ring: keyframe gating, GOP-aligned time and byte eviction, config epochs,
//! discontinuities, snapshot windows and audio eviction.

use super::*;
use crate::dvr::{Limits, Pushed, Ring, SnapshotError};
use std::sync::Arc;

const SECOND: u64 = 1_000_000;
const STEP: u64 = 100_000;

fn ring(max_us: u64, max_bytes: usize) -> Ring {
    Ring::new(Limits { max_us, max_bytes })
}

fn feed(ring: &mut Ring, info: &StreamInfo, frames: Vec<VideoFrame>) {
    for frame in frames {
        assert_eq!(ring.push_video(info, frame), Pushed::Stored);
    }
}

fn h264_gops(from: u64, to: u64) -> Vec<VideoFrame> {
    frames(VideoCodec::H264, from, to, STEP, SECOND)
}

#[test]
fn frames_before_the_first_keyframe_are_dropped() {
    let info = info(VideoCodec::H264);
    let mut ring = ring(60 * SECOND, usize::MAX);
    assert_eq!(ring.snapshot(SECOND), Err(SnapshotError::NoKeyframe));
    assert_eq!(
        ring.push_video(&info, h264_frame(0, false)),
        Pushed::AwaitingKeyframe
    );
    assert!(!ring.has_keyframe());
    assert_eq!(
        ring.push_video(&info, h264_frame(STEP, true)),
        Pushed::Stored
    );
    assert_eq!(
        ring.push_video(&info, h264_frame(2 * STEP, false)),
        Pushed::Stored
    );
    assert!(ring.has_keyframe());
    assert_eq!(ring.epoch(), 1);
    let clip = ring.snapshot(SECOND).unwrap();
    assert_eq!(clip.video.len(), 2);
    assert!(clip.video[0].idr);
    assert_eq!(clip.info, info);
}

#[test]
fn time_eviction_keeps_the_window_in_whole_gops() {
    let info = info(VideoCodec::H264);
    let mut ring = ring(3 * SECOND, usize::MAX);
    feed(&mut ring, &info, h264_gops(0, 10 * SECOND));
    // Newest 9.9 s: the GOP at 6 s is the latest that still covers 6.9 s.
    assert_eq!(ring.buffered_us(), 3_900_000);
    let clip = ring.snapshot(u64::MAX).unwrap();
    assert_eq!(clip.video[0].pts_us, 6 * SECOND);
    assert!(clip.video[0].idr);
    assert_eq!(clip.video.len(), 40);
    assert_eq!(
        ring.snapshot(3 * SECOND).unwrap().video[0].pts_us,
        6 * SECOND
    );
}

#[test]
fn byte_limit_wins_over_time_and_trims_whole_gops() {
    let info = info(VideoCodec::H264);
    let gop_bytes: usize = h264_gops(0, SECOND)
        .iter()
        .map(|frame| frame.data.len())
        .sum();
    let max_bytes = gop_bytes * 7 / 2;
    let mut ring = ring(60 * SECOND, max_bytes);
    for frame in h264_gops(0, 10 * SECOND) {
        assert_eq!(ring.push_video(&info, frame), Pushed::Stored);
        assert!(ring.bytes() <= max_bytes);
    }
    let clip = ring.snapshot(u64::MAX).unwrap();
    assert_eq!(clip.video[0].pts_us, 7 * SECOND);
    assert_eq!(clip.video.len(), 30);
    assert_eq!(ring.bytes(), 3 * gop_bytes);
}

#[test]
fn the_newest_gop_stays_and_stops_growing_at_the_byte_limit() {
    let info = info(VideoCodec::H264);
    let idr = h264_frame(0, true).data.len();
    let delta = h264_frame(0, false).data.len();
    let mut ring = ring(60 * SECOND, idr + 2 * delta);
    feed(
        &mut ring,
        &info,
        vec![
            h264_frame(0, true),
            h264_frame(STEP, false),
            h264_frame(2 * STEP, false),
        ],
    );
    for pts in [3 * STEP, 4 * STEP] {
        assert_eq!(
            ring.push_video(&info, h264_frame(pts, false)),
            Pushed::AwaitingKeyframe
        );
    }
    assert_eq!(ring.snapshot(u64::MAX).unwrap().video.len(), 3);
    assert_eq!(
        ring.push_video(&info, h264_frame(5 * STEP, true)),
        Pushed::Stored
    );
    let clip = ring.snapshot(u64::MAX).unwrap();
    assert_eq!(clip.video.len(), 1);
    assert_eq!(clip.video[0].pts_us, 5 * STEP);
    assert_eq!(ring.epoch(), 1, "same configuration, same epoch");
}

#[test]
fn new_parameter_sets_start_a_new_epoch() {
    let info = info(VideoCodec::H264);
    let mut ring = ring(60 * SECOND, usize::MAX);
    feed(&mut ring, &info, h264_gops(0, 2 * SECOND));
    let mut sps = H264_SPS.to_vec();
    sps[3] = 0x1f;
    feed(
        &mut ring,
        &info,
        vec![
            h264_frame_with(2 * SECOND, true, &sps),
            h264_frame(2 * SECOND + STEP, false),
        ],
    );
    assert_eq!(ring.epoch(), 2);
    let clip = ring.snapshot(u64::MAX).unwrap();
    assert_eq!(clip.video.len(), 2);
    assert_eq!(clip.video[0].pts_us, 2 * SECOND);
    // A keyframe repeating the same sets stays in the epoch.
    feed(
        &mut ring,
        &info,
        vec![h264_frame_with(3 * SECOND, true, &sps)],
    );
    assert_eq!(ring.epoch(), 2);
    assert_eq!(ring.snapshot(u64::MAX).unwrap().video.len(), 3);
}

#[test]
fn a_config_change_drops_frames_until_the_next_keyframe() {
    let sdr = info(VideoCodec::H264);
    let mut hdr = sdr.clone();
    hdr.hdr = Some(HdrInfo::default());
    let mut ring = ring(60 * SECOND, usize::MAX);
    feed(&mut ring, &sdr, h264_gops(0, 2 * STEP));
    assert_eq!(
        ring.push_video(&hdr, h264_frame(2 * STEP, false)),
        Pushed::AwaitingKeyframe
    );
    let clip = ring.snapshot(u64::MAX).unwrap();
    assert_eq!((clip.info.clone(), clip.video.len()), (sdr.clone(), 2));
    assert_eq!(
        ring.push_video(&sdr, h264_frame(3 * STEP, false)),
        Pushed::AwaitingKeyframe,
        "a lost keyframe is not resumed mid-GOP"
    );
    assert_eq!(
        ring.push_video(&hdr, h264_frame(4 * STEP, true)),
        Pushed::Stored
    );
    let clip = ring.snapshot(u64::MAX).unwrap();
    assert_eq!((clip.info, clip.video.len()), (hdr, 1));
    assert_eq!(ring.epoch(), 2);
}

#[test]
fn a_discontinuity_waits_for_the_next_keyframe() {
    let info = info(VideoCodec::Hevc);
    let mut ring = ring(60 * SECOND, usize::MAX);
    feed(
        &mut ring,
        &info,
        vec![hevc_frame(0, true), hevc_frame(STEP, false)],
    );
    ring.discontinuity();
    assert_eq!(
        ring.push_video(&info, hevc_frame(2 * STEP, false)),
        Pushed::AwaitingKeyframe
    );
    assert_eq!(ring.snapshot(u64::MAX).unwrap().video.len(), 2);
    feed(
        &mut ring,
        &info,
        vec![hevc_frame(3 * STEP, true), hevc_frame(4 * STEP, false)],
    );
    let clip = ring.snapshot(u64::MAX).unwrap();
    let times: Vec<u64> = clip.video.iter().map(|frame| frame.pts_us).collect();
    assert_eq!(times, [0, STEP, 3 * STEP, 4 * STEP]);
    assert_eq!(ring.epoch(), 1);
}

#[test]
fn keyframes_without_parameter_sets_only_continue_an_epoch() {
    let info = info(VideoCodec::H264);
    let bare = VideoFrame::new(0, true, annexb(&[H264_AUD, &[0x65, 0x88, 0x11]]));
    let mut ring = ring(60 * SECOND, usize::MAX);
    assert_eq!(
        ring.push_video(&info, bare.clone()),
        Pushed::AwaitingKeyframe
    );
    feed(&mut ring, &info, vec![h264_frame(STEP, true)]);
    let later = VideoFrame {
        pts_us: 2 * STEP,
        ..bare
    };
    assert_eq!(ring.push_video(&info, later), Pushed::Stored);
    assert_eq!(ring.epoch(), 1);
    assert_eq!(ring.snapshot(0).unwrap().video.len(), 1);
}

#[test]
fn snapshot_starts_at_the_latest_keyframe_before_the_window() {
    let info = info(VideoCodec::Av1);
    let mut ring = ring(60 * SECOND, usize::MAX);
    for (index, frame) in frames(VideoCodec::Av1, 0, 5 * SECOND, STEP, SECOND)
        .into_iter()
        .enumerate()
    {
        let pts = frame.pts_us;
        assert_eq!(ring.push_video(&info, frame), Pushed::Stored, "{index}");
        for packet in 0..10 {
            ring.push_audio(audio(pts + packet * 10_000));
        }
    }
    let clip = ring.snapshot(2 * SECOND).unwrap();
    assert_eq!(clip.video[0].pts_us, 2 * SECOND);
    assert_eq!(clip.video.len(), 30);
    assert_eq!(clip.audio[0].pts_us, 2 * SECOND);
    assert_eq!(clip.audio.last().unwrap().pts_us, 4_990_000);
    assert_eq!(clip.audio.len(), 300);
    assert_eq!(ring.snapshot(0).unwrap().video[0].pts_us, 4 * SECOND);
    assert_eq!(ring.snapshot(100 * SECOND).unwrap().video.len(), 50);
}

#[test]
fn snapshots_share_frame_bytes() {
    let info = info(VideoCodec::H264);
    let mut ring = ring(60 * SECOND, usize::MAX);
    let keyframe = h264_frame(0, true);
    feed(&mut ring, &info, vec![keyframe.clone()]);
    let packet = audio(0);
    ring.push_audio(packet.clone());
    let clip = ring.snapshot(SECOND).unwrap();
    assert!(Arc::ptr_eq(&clip.video[0].data, &keyframe.data));
    assert!(Arc::ptr_eq(&clip.audio[0].data, &packet.data));
}

#[test]
fn audio_older_than_the_oldest_gop_is_evicted() {
    let info = info(VideoCodec::H264);
    let mut ring = ring(2 * SECOND, usize::MAX);
    let mut gop_bytes = 0;
    for frame in h264_gops(0, 10 * SECOND) {
        let pts = frame.pts_us;
        if pts >= 7 * SECOND {
            gop_bytes += frame.data.len();
        }
        assert_eq!(ring.push_video(&info, frame), Pushed::Stored);
        for packet in 0..10 {
            ring.push_audio(audio(pts + packet * 10_000));
        }
    }
    // GOPs from 7 s stay; audio from 7 s minus the slack.
    let packets = (6_500_000..10 * SECOND).step_by(10_000).count();
    assert_eq!(ring.bytes(), gop_bytes + packets * audio(0).data.len());
    assert_eq!(ring.snapshot(u64::MAX).unwrap().audio[0].pts_us, 7 * SECOND);
}

#[test]
fn audio_alone_keeps_only_the_slack() {
    let mut ring = ring(60 * SECOND, usize::MAX);
    for pts in (0..SECOND).step_by(10_000) {
        ring.push_audio(audio(pts));
    }
    let kept = (490_000..SECOND).step_by(10_000).count();
    assert_eq!(ring.bytes(), kept * audio(0).data.len());
}

#[test]
fn byte_limit_trims_audio_once_only_one_gop_is_left() {
    let info = info(VideoCodec::H264);
    let idr = h264_frame(0, true).data.len();
    let packet = audio(0).data.len();
    let mut ring = ring(60 * SECOND, idr + 5 * packet);
    feed(&mut ring, &info, vec![h264_frame(0, true)]);
    for pts in (0..100_000).step_by(10_000) {
        ring.push_audio(audio(pts));
    }
    assert_eq!(ring.bytes(), idr + 5 * packet);
    assert_eq!(ring.snapshot(0).unwrap().audio[0].pts_us, 50_000);
}

#[test]
fn clear_starts_over() {
    let info = info(VideoCodec::H264);
    let mut ring = ring(60 * SECOND, usize::MAX);
    feed(&mut ring, &info, h264_gops(0, SECOND));
    ring.push_audio(audio(0));
    ring.clear();
    assert!(!ring.has_keyframe());
    assert_eq!((ring.bytes(), ring.buffered_us()), (0, 0));
    assert_eq!(ring.snapshot(SECOND), Err(SnapshotError::NoKeyframe));
    assert_eq!(
        ring.push_video(&info, h264_frame(SECOND, false)),
        Pushed::AwaitingKeyframe
    );
    feed(&mut ring, &info, vec![h264_frame(2 * SECOND, true)]);
    assert_eq!(ring.epoch(), 2);
}

#[test]
fn shrinking_limits_evict_at_once() {
    let info = info(VideoCodec::H264);
    let mut ring = ring(60 * SECOND, usize::MAX);
    feed(&mut ring, &info, h264_gops(0, 5 * SECOND));
    ring.set_limits(Limits {
        max_us: SECOND,
        max_bytes: usize::MAX,
    });
    assert_eq!(ring.snapshot(u64::MAX).unwrap().video[0].pts_us, 3 * SECOND);
}
