//! The live recorder (fragments, restarts, the duration patch) and crash salvage of torn
//! recordings, including a no-panic property on arbitrary bytes.

use super::bmff::{be32, be64, kinds, parse, tfdt, trun};
use super::*;
use crate::dvr::{RecPush, Recorder, salvage};
use proptest::prelude::*;
use std::io::Cursor;
use std::sync::OnceLock;

const FRAME_US: u64 = 33_333;

struct Recording {
    file: Vec<u8>,
    fragments: usize,
    info: crate::dvr::ClipInfo,
    patch: (u64, Vec<u8>),
}

/// 150 frames at 30 fps, a keyframe every 30, and 10 ms audio up to the last frame.
fn record(codec: VideoCodec) -> Recording {
    let info = info(codec);
    let video = frames(codec, 0, 150 * FRAME_US, FRAME_US, 30 * FRAME_US);
    let mut video = video.into_iter();
    let (mut recorder, mut file) = Recorder::start(&info, video.next().unwrap()).unwrap();
    let mut fragments = 0;
    let mut next_audio = 0;
    for frame in video {
        while next_audio <= frame.pts_us {
            recorder.push_audio(audio(next_audio));
            next_audio += 10_000;
        }
        match recorder.push_video(&info, frame) {
            RecPush::Buffered => {}
            RecPush::Fragment(bytes) => {
                file.extend_from_slice(&bytes);
                fragments += 1;
            }
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(recorder.bytes(), file.len() as u64);
    }
    let finished = recorder.finish();
    file.extend_from_slice(&finished.tail);
    Recording {
        file,
        fragments,
        info: finished.info,
        patch: finished.duration_patch.unwrap(),
    }
}

#[test]
fn recorder_writes_two_second_fragments_and_patches_the_duration() {
    let mut recording = record(VideoCodec::H264);
    // Frames 0..=60, 61..=121, then the tail.
    assert_eq!(recording.fragments, 2);
    assert_eq!(recording.info.bytes, recording.file.len() as u64);
    assert_eq!((recording.info.width, recording.info.height), (200, 120));
    assert_eq!(recording.info.duration_ms, 5000);
    let (at, bytes) = recording.patch.clone();
    let at = at as usize;
    assert_eq!(be64(&recording.file, at), 0, "placeholder");
    recording.file[at..at + bytes.len()].copy_from_slice(&bytes);
    let file = &recording.file;
    let tree = parse(file);
    assert_eq!(
        kinds(&tree),
        [
            "ftyp", "moov", "moof", "mdat", "moof", "mdat", "moof", "mdat"
        ]
    );
    let mehd = tree[1].path(&["mvex", "mehd"]);
    assert_eq!(mehd.body + 4, at);
    assert_eq!(be64(file, at), 5000);

    let mut video_samples = 0;
    let mut video_end = 0;
    let mut audio_samples = 0;
    for (index, pair) in tree[2..].chunks(2).enumerate() {
        let moof = &pair[0];
        assert_eq!(be32(moof.child("mfhd").payload(file), 4), index as u32 + 1);
        let trafs = moof.all("traf");
        let video = trun(file, trafs[0].child("trun"));
        assert_eq!(tfdt(file, trafs[0]), video_end, "video continues");
        video_end += video
            .samples
            .iter()
            .map(|sample| u64::from(sample.0))
            .sum::<u64>();
        video_samples += video.samples.len();
        audio_samples += trun(file, trafs[1].child("trun")).samples.len();
        let first = video.samples[0].2.unwrap();
        assert_eq!(
            first == 0x0200_0000,
            index == 0,
            "only the first starts on a keyframe"
        );
    }
    assert_eq!(video_samples, 150);
    // Frame 149 at 446996 ticks, lasting as long as frame 148.
    assert_eq!(video_end, 449_996);
    assert_eq!(audio_samples, 497);
}

#[test]
fn recorder_restarts_when_the_configuration_changes() {
    let info = info(VideoCodec::H264);
    let (mut recorder, _) = Recorder::start(&info, h264_frame(0, true)).unwrap();
    assert!(matches!(
        recorder.push_video(&info, h264_frame(FRAME_US, false)),
        RecPush::Buffered
    ));
    assert!(matches!(
        recorder.push_video(&info, h264_frame(2 * FRAME_US, true)),
        RecPush::Buffered
    ));
    let mut sps = H264_SPS.to_vec();
    sps[3] = 0x1f;
    let changed = h264_frame_with(3 * FRAME_US, true, &sps);
    match recorder.push_video(&info, changed.clone()) {
        RecPush::Restart(frame) => assert_eq!(frame, changed),
        other => panic!("unexpected {other:?}"),
    }
    let mut hdr = info.clone();
    hdr.hdr = Some(HdrInfo::default());
    assert!(matches!(
        recorder.push_video(&hdr, h264_frame(4 * FRAME_US, false)),
        RecPush::AwaitingKeyframe
    ));
    assert!(matches!(
        recorder.push_video(&info, h264_frame(5 * FRAME_US, false)),
        RecPush::AwaitingKeyframe
    ));
    assert!(matches!(
        recorder.push_video(&hdr, h264_frame(6 * FRAME_US, true)),
        RecPush::Restart(_)
    ));
    assert!(matches!(
        recorder.push_video(&info, h264_frame(7 * FRAME_US, true)),
        RecPush::Buffered
    ));
    let finished = recorder.finish();
    assert_eq!(trun_count(&finished.tail), 4);
    let (next, init) = Recorder::start(&hdr, h264_frame(6 * FRAME_US, true)).unwrap();
    assert_eq!(kinds(&parse(&init)), ["ftyp", "moov"]);
    assert!(next.info().hdr.is_some());
}

fn trun_count(fragment: &[u8]) -> usize {
    let tree = parse(fragment);
    trun(fragment, tree[0].all("traf")[0].child("trun"))
        .samples
        .len()
}

#[test]
fn recorder_waits_for_a_keyframe_after_a_loss() {
    let info = info(VideoCodec::Av1);
    assert!(Recorder::start(&info, av1_frame(0, false)).is_err());
    let (mut recorder, _) = Recorder::start(&info, av1_frame(0, true)).unwrap();
    recorder.discontinuity();
    assert!(matches!(
        recorder.push_video(&info, av1_frame(FRAME_US, false)),
        RecPush::AwaitingKeyframe
    ));
    assert!(matches!(
        recorder.push_video(&info, av1_frame(2 * FRAME_US, true)),
        RecPush::Buffered
    ));
    recorder.push_audio(audio(0));
    assert_eq!(recorder.duration_us(), 2 * FRAME_US);
    let finished = recorder.finish();
    assert_eq!(trun_count(&finished.tail), 2);
}

#[test]
fn salvage_keeps_complete_fragments_of_a_torn_recording() {
    let recording = record(VideoCodec::Hevc);
    let file = &recording.file;
    let tree = parse(file);
    let full = salvage(&mut Cursor::new(file)).unwrap();
    assert_eq!((full.valid_len, full.fragments), (file.len() as u64, 3));
    assert_eq!(full.duration_us, 4_999_956);
    let (at, bytes) = full.mehd_patch.clone().unwrap();
    assert_eq!(
        (at, bytes),
        (recording.patch.0, 5000u64.to_be_bytes().to_vec())
    );

    let mdat_ends: Vec<usize> = tree
        .iter()
        .filter(|node| node.kind() == "mdat")
        .map(|node| node.end)
        .collect();
    for cut in (0..file.len())
        .step_by(97)
        .chain([tree[1].end, mdat_ends[0], mdat_ends[0] + 9])
    {
        let torn = &file[..cut];
        let found = salvage(&mut Cursor::new(torn)).unwrap();
        let complete = mdat_ends.iter().filter(|&&end| end <= cut).count();
        assert_eq!(found.fragments as usize, complete, "cut at {cut}");
        let expected = mdat_ends.iter().rev().find(|&&end| end <= cut).copied();
        assert_eq!(
            found.valid_len,
            expected.unwrap_or(0) as u64,
            "cut at {cut}"
        );
        if complete == 0 {
            assert_eq!(found.duration_us, 0);
        } else {
            assert!(found.duration_us > 0 && found.mehd_patch.is_some());
        }
    }
    let first = salvage(&mut Cursor::new(&file[..mdat_ends[0]])).unwrap();
    assert_eq!(
        first.duration_us, 2_033_311,
        "frames 0..=60, up to frame 61"
    );
}

#[test]
fn salvage_rejects_files_without_an_init_segment() {
    let recording = record(VideoCodec::H264);
    let tree = parse(&recording.file);
    let without_ftyp = &recording.file[tree[0].end..];
    assert_eq!(
        salvage(&mut Cursor::new(without_ftyp)).unwrap().fragments,
        0
    );
    let mut swapped = recording.file[..tree[1].end].to_vec();
    swapped.extend_from_slice(&recording.file[tree[3].start..tree[3].end]);
    swapped.extend_from_slice(&recording.file[tree[2].start..tree[2].end]);
    assert_eq!(salvage(&mut Cursor::new(&swapped)).unwrap().fragments, 0);
    assert_eq!(
        salvage(&mut Cursor::new(&[] as &[u8])).unwrap(),
        Default::default()
    );
}

#[test]
fn salvage_reads_large_size_boxes() {
    let recording = record(VideoCodec::H264);
    let tree = parse(&recording.file);
    let mut file = recording.file[..tree[3].start].to_vec();
    let mdat = &tree[3];
    let payload = &recording.file[mdat.body..mdat.end];
    file.extend_from_slice(&1u32.to_be_bytes());
    file.extend_from_slice(b"mdat");
    file.extend_from_slice(&(payload.len() as u64 + 16).to_be_bytes());
    file.extend_from_slice(payload);
    let found = salvage(&mut Cursor::new(&file)).unwrap();
    assert_eq!((found.valid_len, found.fragments), (file.len() as u64, 1));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn salvage_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..1024)) {
        let found = salvage(&mut Cursor::new(&bytes)).unwrap();
        prop_assert!(found.valid_len <= bytes.len() as u64);
    }

    #[test]
    fn salvage_survives_corrupted_recordings(
        at in 0usize..4096,
        value in any::<u8>(),
        cut in 0usize..8192,
    ) {
        static FILE: OnceLock<Vec<u8>> = OnceLock::new();
        let mut file = FILE.get_or_init(|| record(VideoCodec::H264).file).clone();
        let index = at % file.len();
        file[index] = value;
        file.truncate(cut.min(file.len()));
        let found = salvage(&mut Cursor::new(&file)).unwrap();
        prop_assert!(found.valid_len <= file.len() as u64);
    }
}
