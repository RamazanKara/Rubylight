//! The live recorder (fragments, restarts, the duration patch, ending at a fragment the
//! host could not write) and crash salvage of torn recordings, including a no-panic
//! property on arbitrary bytes.

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
    record_until(codec, None)
}

/// [`record`], where writing fragment `fail_at` (1-based) fails and the host ends the part
/// with `finish_written`.
fn record_until(codec: VideoCodec, fail_at: Option<usize>) -> Recording {
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
            RecPush::Fragment(_) if fail_at == Some(fragments + 1) => {
                let finished = recorder.finish_written();
                assert!(finished.tail.is_empty());
                return Recording {
                    file,
                    fragments,
                    info: finished.info,
                    patch: finished.duration_patch.unwrap(),
                };
            }
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
fn a_fragment_the_host_could_not_write_ends_the_part_at_the_one_before() {
    let mut recording = record_until(VideoCodec::H264, Some(2));
    assert_eq!(recording.fragments, 1);
    assert_eq!(recording.info.bytes, recording.file.len() as u64);
    // Frames 0..=60 end at 182998 ticks (2033 ms); their audio, 204 packets, at 2040 ms.
    assert_eq!(recording.info.duration_ms, 2040);
    let (at, bytes) = recording.patch.clone();
    let at = at as usize;
    recording.file[at..at + bytes.len()].copy_from_slice(&bytes);
    let file = &recording.file;
    let tree = parse(file);
    assert_eq!(kinds(&tree), ["ftyp", "moov", "moof", "mdat"]);
    assert_eq!(be64(file, at), 2040);
    let trafs = tree[2].all("traf");
    assert_eq!(trun(file, trafs[0].child("trun")).samples.len(), 61);
    assert_eq!(trun(file, trafs[1].child("trun")).samples.len(), 204);
    let found = salvage(&mut Cursor::new(file)).unwrap();
    assert_eq!((found.valid_len, found.fragments), (file.len() as u64, 1));
    assert_eq!(found.duration_us, 2_033_311);

    // Failing the first fragment leaves the init segment alone.
    let recording = record_until(VideoCodec::H264, Some(1));
    assert_eq!(recording.fragments, 0);
    assert_eq!(recording.info.bytes, recording.file.len() as u64);
    assert_eq!(recording.info.duration_ms, 0);
    assert_eq!(kinds(&parse(&recording.file)), ["ftyp", "moov"]);
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

#[test]
fn recorder_restart_reuses_param_sets_for_same_codec_and_waits_for_new_codec_headers() {
    let base = info(VideoCodec::H264);
    let (mut recorder, _) = Recorder::start(&base, h264_frame(0, true)).unwrap();
    let bare_h264 = VideoFrame::new(FRAME_US, true, annexb(&[H264_AUD, &[0x65, 0x88, 0x11]]));
    let mut video_only = base.clone();
    video_only.audio = None;
    let restarted = match recorder.push_video(&video_only, bare_h264.clone()) {
        RecPush::Restart(frame) => frame,
        other => panic!("unexpected {other:?}"),
    };
    let (next, init) =
        Recorder::start_with_params(&video_only, restarted, Some(recorder.params())).unwrap();
    assert_eq!(kinds(&parse(&init)), ["ftyp", "moov"]);
    assert_eq!(next.params(), recorder.params());

    // A bare keyframe for a different codec cannot reuse the old codec's parameter sets.
    let hevc_info = info(VideoCodec::Hevc);
    let bare_hevc = VideoFrame::new(2 * FRAME_US, true, annexb(&[HEVC_AUD, &[0x26, 0x01, 0xaf]]));
    assert!(matches!(
        recorder.push_video(&hevc_info, bare_hevc),
        RecPush::AwaitingKeyframe
    ));
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
    assert_eq!(salvage(&mut Cursor::new(swapped)).unwrap().fragments, 0);
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

fn replace_mehd(file: &[u8], payload: &[u8]) -> Vec<u8> {
    let tree = parse(file);
    let moov = &tree[1];
    let mvex = moov.child("mvex");
    let mehd = mvex.child("mehd");
    let new_box_len = 8 + payload.len();
    let old_box_len = mehd.end - mehd.start;
    let delta = new_box_len as isize - old_box_len as isize;
    let mut out = Vec::with_capacity((file.len() as isize + delta) as usize);
    out.extend_from_slice(&file[..mehd.start]);
    out.extend_from_slice(&(new_box_len as u32).to_be_bytes());
    out.extend_from_slice(b"mehd");
    out.extend_from_slice(payload);
    out.extend_from_slice(&file[mehd.end..]);
    let patch_size = |buf: &mut [u8], at: usize| {
        let old = u32::from_be_bytes(buf[at..at + 4].try_into().unwrap()) as isize;
        let updated = (old + delta) as u32;
        buf[at..at + 4].copy_from_slice(&updated.to_be_bytes());
    };
    patch_size(&mut out, moov.start);
    patch_size(&mut out, mvex.start);
    out
}

#[test]
fn salvage_ignores_truncated_or_unknown_version_mehd() {
    let recording = record(VideoCodec::H264);
    for bad_payload in [
        // Version 1 needs 12 bytes (4 header + 8 duration).
        &[1u8, 0, 0, 0, 0, 0, 0, 0][..],
        // Version 0 needs 8 bytes (4 header + 4 duration).
        &[0u8, 0, 0, 0, 0, 0][..],
        // Unknown version 2.
        &[2u8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0][..],
    ] {
        let file = replace_mehd(&recording.file, bad_payload);
        let found = salvage(&mut Cursor::new(&file)).unwrap();
        assert_eq!((found.valid_len, found.fragments), (file.len() as u64, 3));
        assert_eq!(found.mehd_patch, None, "{bad_payload:?}");
    }
    // Valid version 0 mehd (8 bytes) produces a 4-byte duration patch.
    let v0_file = replace_mehd(&recording.file, &[0u8; 8]);
    let found = salvage(&mut Cursor::new(&v0_file)).unwrap();
    let (_, patch) = found.mehd_patch.expect("valid v0 mehd");
    assert_eq!(patch, 5000u32.to_be_bytes());
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
