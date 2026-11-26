//! The clip muxer read back: box tree, sample entries and codec configuration boxes,
//! colour boxes, `dOps`, fragment timing and sample data, and its errors.

use super::bmff::{self, Node, be16, be32, be64, kinds, parse, tfdt, tfhd_track, trun};
use super::*;
use crate::dvr::fmp4::boxes::{NON_SYNC_SAMPLE, SYNC_SAMPLE};
use crate::dvr::fmp4::codec::{opus_entry, opus_mapping};
use crate::dvr::fmp4::{AudioTimeline, VideoTimeline};
use crate::dvr::{Clip, ClipInfo, MuxError, write_clip};

const FRAME_US: u64 = 33_333;

/// `gops` GOPs of ten 30 fps frames, and 10 ms audio from 5 ms in.
fn clip(codec: VideoCodec, info: StreamInfo, gops: u64) -> Clip {
    let end = gops * 10 * FRAME_US;
    let video = frames(codec, 1_000_000, 1_000_000 + end, FRAME_US, 10 * FRAME_US);
    let audio = if info.audio.is_some() {
        (1_005_000..1_000_000 + end)
            .step_by(10_000)
            .map(audio)
            .collect()
    } else {
        Vec::new()
    };
    Clip { info, video, audio }
}

fn mux(clip: &Clip) -> (Vec<u8>, ClipInfo, Vec<Node>) {
    let mut out = Vec::new();
    let info = write_clip(&mut out, clip).unwrap();
    assert_eq!(info.bytes, out.len() as u64);
    let tree = parse(&out);
    (out, info, tree)
}

fn entry<'a>(tree: &'a [Node], track: usize, kind: &str) -> &'a Node {
    let moov = &tree[1];
    moov.all("trak")[track].path(&["mdia", "minf", "stbl", "stsd", kind])
}

#[test]
fn h264_clip_has_one_fragment_per_gop() {
    let clip = clip(VideoCodec::H264, info(VideoCodec::H264), 3);
    let (out, info, tree) = mux(&clip);
    assert_eq!(
        kinds(&tree),
        [
            "ftyp", "moov", "moof", "mdat", "moof", "mdat", "moof", "mdat"
        ]
    );
    let ftyp = tree[0].payload(&out);
    assert_eq!(&ftyp[..4], b"isom");
    assert!(ftyp[8..].chunks(4).any(|brand| brand == b"avc1"));
    assert_eq!((info.width, info.height), (200, 120));
    assert_eq!(
        (info.codec, info.hdr, info.ten_bit),
        (VideoCodec::H264, false, false)
    );
    // Audio runs 5 ms past the last video frame's end (999.99 ms).
    assert_eq!(info.duration_ms, 1005);
    assert!((info.fps - 30.0).abs() < 0.01, "{}", info.fps);

    let avc1 = entry(&tree, 0, "avc1");
    let fixed = avc1.payload(&out);
    assert_eq!((be16(fixed, 24), be16(fixed, 26)), (200, 120));
    let mut expected = vec![1, 100, 0, 12, 0xff, 0xe1];
    expected.extend_from_slice(&(H264_SPS.len() as u16).to_be_bytes());
    expected.extend_from_slice(H264_SPS);
    expected.push(1);
    expected.extend_from_slice(&(H264_PPS.len() as u16).to_be_bytes());
    expected.extend_from_slice(H264_PPS);
    expected.extend_from_slice(&[0xfd, 0xf8, 0xf8, 0]);
    assert_eq!(avc1.child("avcC").payload(&out), expected);
    assert_eq!(
        avc1.child("colr").payload(&out),
        [b'n', b'c', b'l', b'x', 0, 1, 0, 1, 0, 1, 0]
    );
    assert!(!avc1.has("mdcv") && !avc1.has("clli"));

    let opus = entry(&tree, 1, "Opus");
    assert_eq!(be16(opus.payload(&out), 16), 2, "channelcount");
    assert_eq!(
        opus.child("dOps").payload(&out),
        [0, 2, 0, 0, 0, 0, 0xbb, 0x80, 0, 0, 0]
    );

    let moov = &tree[1];
    let mehd = moov.path(&["mvex", "mehd"]).payload(&out);
    assert_eq!((mehd[0], be64(mehd, 4)), (1, 1005));
    assert_eq!(moov.child("mvex").all("trex").len(), 2);
    let handlers: Vec<&[u8]> = moov
        .all("trak")
        .iter()
        .map(|trak| &trak.path(&["mdia", "hdlr"]).payload(&out)[8..12])
        .collect();
    assert_eq!(handlers, [b"vide", b"soun"]);
    let timescales: Vec<u32> = moov
        .all("trak")
        .iter()
        .map(|trak| be32(trak.path(&["mdia", "mdhd"]).payload(&out), 12))
        .collect();
    assert_eq!(timescales, [90_000, 48_000]);

    let mut audio_samples = 0;
    let mut audio_end = 0;
    for (index, pair) in tree[2..].chunks(2).enumerate() {
        let (moof, mdat) = (&pair[0], &pair[1]);
        assert_eq!(be32(moof.child("mfhd").payload(&out), 4), index as u32 + 1);
        let trafs = moof.all("traf");
        assert_eq!(trafs.len(), 2);
        let video = trun(&out, trafs[0].child("trun"));
        assert_eq!(tfhd_track(&out, trafs[0]), 1);
        let gop_start = crate::dvr::ticks(index as u64 * 10 * FRAME_US, 90_000);
        assert_eq!(tfdt(&out, trafs[0]), gop_start);
        assert_eq!(video.samples.len(), 10);
        let durations: u32 = video.samples.iter().map(|sample| sample.0).sum();
        assert!((29_999..=30_001).contains(&durations), "{durations}");
        let flags: Vec<u32> = video
            .samples
            .iter()
            .map(|sample| sample.2.unwrap())
            .collect();
        assert_eq!(flags[0], SYNC_SAMPLE);
        assert!(flags[1..].iter().all(|&flag| flag == NON_SYNC_SAMPLE));
        // Sample data starts the mdat payload: SEI first, AUD and parameter sets gone.
        let data = moof.start + video.data_offset as usize;
        assert_eq!(data, mdat.body);
        let first = video.samples[0].1 as usize;
        assert_eq!(
            &out[data..data + 4 + H264_SEI.len()],
            [&(H264_SEI.len() as u32).to_be_bytes()[..], H264_SEI].concat()
        );
        let video_bytes: u32 = video.samples.iter().map(|sample| sample.1).sum();
        assert!(first > H264_SEI.len());

        let audio = trun(&out, trafs[1].child("trun"));
        assert_eq!(tfhd_track(&out, trafs[1]), 2);
        let base = tfdt(&out, trafs[1]);
        if index == 0 {
            assert_eq!(base, 240, "5 ms after the first video frame");
        } else {
            assert_eq!(base, audio_end, "audio continues");
        }
        assert!(
            audio
                .samples
                .iter()
                .all(|sample| sample.0 == 480 && sample.2.is_none())
        );
        assert_eq!(
            moof.start + audio.data_offset as usize,
            mdat.body + video_bytes as usize
        );
        let audio_bytes: u32 = audio.samples.iter().map(|sample| sample.1).sum();
        assert_eq!(mdat.end - mdat.body, (video_bytes + audio_bytes) as usize);
        audio_samples += audio.samples.len();
        audio_end = base + 480 * audio.samples.len() as u64;
    }
    assert_eq!(audio_samples, clip.audio.len());
}

#[test]
fn hevc_hdr_clip_carries_colour_volume_and_light_levels() {
    let clip = clip(VideoCodec::Hevc, hdr_info(VideoCodec::Hevc), 2);
    let (out, info, tree) = mux(&clip);
    assert_eq!(
        kinds(&tree),
        ["ftyp", "moov", "moof", "mdat", "moof", "mdat"]
    );
    assert_eq!((info.width, info.height), (202, 118));
    assert!(info.hdr && info.ten_bit);
    let moov = &tree[1];
    assert_eq!(moov.all("trak").len(), 1, "no audio track without Opus");
    assert_eq!(
        be32(moov.child("mvhd").payload(&out), 96),
        2,
        "next track id"
    );

    let hvc1 = entry(&tree, 0, "hvc1");
    let mut expected = vec![
        1, 0x02, 0x20, 0, 0, 0, 0x90, 0, 0, 0, 0, 0, 60, 0xf0, 0x00, 0xfc, 0xfd, 0xfa, 0xfa, 0, 0,
        0x0f, 3,
    ];
    for (kind, unit) in [(0xa0, HEVC_VPS), (0xa1, HEVC_SPS), (0xa2, HEVC_PPS)] {
        expected.extend_from_slice(&[kind, 0, 1]);
        expected.extend_from_slice(&(unit.len() as u16).to_be_bytes());
        expected.extend_from_slice(unit);
    }
    assert_eq!(hvc1.child("hvcC").payload(&out), expected);
    assert_eq!(
        hvc1.child("colr").payload(&out),
        [b'n', b'c', b'l', b'x', 0, 9, 0, 16, 0, 9, 0]
    );
    let metadata = hdr_metadata();
    let mut mdcv = Vec::new();
    for [x, y] in [
        metadata.primaries[1],
        metadata.primaries[2],
        metadata.primaries[0],
    ] {
        mdcv.extend_from_slice(&x.to_be_bytes());
        mdcv.extend_from_slice(&y.to_be_bytes());
    }
    mdcv.extend_from_slice(&15635u16.to_be_bytes());
    mdcv.extend_from_slice(&16450u16.to_be_bytes());
    mdcv.extend_from_slice(&10_000_000u32.to_be_bytes());
    mdcv.extend_from_slice(&50u32.to_be_bytes());
    assert_eq!(hvc1.child("mdcv").payload(&out), mdcv);
    assert_eq!(hvc1.child("clli").payload(&out), [0x03, 0x84, 0x01, 0x90]);

    for pair in tree[2..].chunks(2) {
        let trafs = pair[0].all("traf");
        assert_eq!(trafs.len(), 1);
        assert_eq!(trun(&out, trafs[0].child("trun")).samples.len(), 10);
    }
}

#[test]
fn hdr_without_metadata_writes_only_colr() {
    let mut info = hdr_info(VideoCodec::Hevc);
    info.hdr = Some(HdrInfo::default());
    let (out, _, tree) = mux(&clip(VideoCodec::Hevc, info, 1));
    let hvc1 = entry(&tree, 0, "hvc1");
    assert_eq!(hvc1.child("colr").payload(&out)[4..10], [0, 9, 0, 16, 0, 9]);
    assert!(!hvc1.has("mdcv") && !hvc1.has("clli"));
}

#[test]
fn av1_clip_has_av1c_and_sized_obus_without_temporal_delimiters() {
    let clip = clip(VideoCodec::Av1, info(VideoCodec::Av1), 2);
    let (out, info, tree) = mux(&clip);
    assert_eq!((info.width, info.height, info.ten_bit), (200, 120, true));
    let ftyp = tree[0].payload(&out);
    assert!(ftyp[8..].chunks(4).any(|brand| brand == b"av01"));
    let av01 = entry(&tree, 0, "av01");
    let mut expected = vec![0x81, 0x00, 0x4c, 0x00];
    expected.extend_from_slice(AV1_SEQUENCE_HEADER);
    assert_eq!(av01.child("av1C").payload(&out), expected);
    let (moof, mdat) = (&tree[2], &tree[3]);
    let video = trun(&out, moof.all("traf")[0].child("trun"));
    let mut at = mdat.body;
    for (index, sample) in video.samples.iter().enumerate() {
        let data = &out[at..at + sample.1 as usize];
        let obus: Vec<(u8, bool)> = crate::dvr::av1::obus(data)
            .map(|obu| (obu.kind, obu.header[0] & 2 != 0))
            .collect();
        if index == 0 {
            assert_eq!(obus, [(1, true), (6, true), (6, true)]);
        } else {
            assert_eq!(obus, [(6, true), (6, true)]);
        }
        at += sample.1 as usize;
    }
}

#[test]
fn opus_mapping_follows_vorbis_order() {
    assert_eq!(opus_mapping(&OpusConfig::stereo()), (0, vec![]));
    let split_stereo = OpusConfig {
        channels: 2,
        streams: 2,
        coupled: 0,
        mapping: vec![0, 1],
    };
    assert_eq!(opus_mapping(&split_stereo), (1, vec![0, 1]));
    let surround51 = OpusConfig {
        channels: 6,
        streams: 4,
        coupled: 2,
        mapping: vec![0, 4, 1, 5, 2, 3],
    };
    assert_eq!(opus_mapping(&surround51), (1, vec![0, 1, 4, 2, 3, 5]));
    let surround71 = OpusConfig {
        channels: 8,
        streams: 5,
        coupled: 3,
        mapping: vec![0, 6, 1, 7, 2, 3, 4, 5],
    };
    assert_eq!(opus_mapping(&surround71), (1, vec![0, 1, 6, 4, 5, 2, 3, 7]));
    let many = OpusConfig {
        channels: 10,
        streams: 10,
        coupled: 0,
        mapping: (0..10).collect(),
    };
    assert_eq!(opus_mapping(&many), (255, (0..10).collect::<Vec<u8>>()));

    let entry = opus_entry(&surround51);
    let tree = parse(&entry);
    let dops = tree[0].child("dOps").payload(&entry);
    assert_eq!(
        dops,
        [
            0, 6, 0, 0, 0, 0, 0xbb, 0x80, 0, 0, 1, 4, 2, 0, 1, 4, 2, 3, 5
        ]
    );
    assert_eq!(be16(tree[0].payload(&entry), 16), 6);
}

#[test]
fn mux_errors_name_the_problem() {
    let base = info(VideoCodec::H264);
    let mut out = Vec::new();
    let empty = Clip {
        info: base.clone(),
        video: vec![],
        audio: vec![],
    };
    assert!(matches!(
        write_clip(&mut out, &empty),
        Err(MuxError::NoVideo)
    ));
    let late = Clip {
        video: vec![h264_frame(0, false)],
        ..empty.clone()
    };
    assert!(matches!(
        write_clip(&mut out, &late),
        Err(MuxError::NotKeyframe)
    ));
    let bare = Clip {
        video: vec![VideoFrame::new(0, true, annexb(&[&[0x65, 0x88, 0x11]]))],
        ..empty.clone()
    };
    assert!(matches!(
        write_clip(&mut out, &bare),
        Err(MuxError::MissingParameterSets)
    ));
    let broken = Clip {
        video: vec![VideoFrame::new(
            0,
            true,
            annexb(&[&[0x67, 0x64], H264_PPS, &[0x65, 1]]),
        )],
        ..empty
    };
    let error = write_clip(&mut out, &broken).unwrap_err();
    assert!(matches!(error, MuxError::BadParameterSets));
    assert!(error.to_string().contains("parameter sets"));
    assert!(out.is_empty(), "nothing written on error");
}

#[test]
fn audio_before_the_clip_start_is_left_out() {
    let mut clip = clip(VideoCodec::H264, info(VideoCodec::H264), 1);
    clip.audio.insert(0, audio(900_000));
    let (out, _, tree) = mux(&clip);
    let audio = trun(&out, tree[2].all("traf")[1].child("trun"));
    assert_eq!(audio.samples.len(), clip.audio.len() - 1);
}

#[test]
fn audio_timeline_keeps_gaps_and_absorbs_jitter() {
    let packets: Vec<AudioPacket> = [0, 10_000, 20_000, 130_000]
        .into_iter()
        .map(audio)
        .collect();
    let mut timeline = AudioTimeline::new(0);
    assert_eq!(
        timeline.layout(&packets, None),
        (0, vec![480, 480, 5280, 480])
    );
    assert_eq!(timeline.end(), Some(6720));
    let mut timeline = AudioTimeline::new(0);
    let jittery: Vec<AudioPacket> = [0, 10_001, 19_999].into_iter().map(audio).collect();
    assert_eq!(timeline.layout(&jittery, None), (0, vec![480, 480, 480]));
    // The next fragment continues, or jumps over a gap at the boundary.
    assert_eq!(timeline.layout(&[audio(30_002)], None).0, 1440);
    assert_eq!(timeline.layout(&[audio(200_000)], None).0, 9600);
}

#[test]
fn video_timeline_is_strictly_increasing() {
    let frames: Vec<VideoFrame> = [0, 33_333, 33_333, 20_000, 100_000]
        .into_iter()
        .map(|pts| h264_frame(pts, false))
        .collect();
    let mut timeline = VideoTimeline::new(0);
    let (base, durations) = timeline.layout(&frames[..4], Some(100_000));
    assert_eq!(base, 0);
    assert_eq!(durations, [3000, 1, 1, 9000 - 3002]);
    let (base, durations) = timeline.layout(&frames[4..], None);
    assert_eq!((base, durations), (9000, vec![9000 - 3002]));
    assert_eq!(timeline.end(), 9000 + 5998);
}

#[test]
fn salvage_reads_back_a_written_clip() {
    let clip = clip(VideoCodec::H264, info(VideoCodec::H264), 3);
    let (out, info, _) = mux(&clip);
    let salvage = crate::dvr::salvage(&mut std::io::Cursor::new(&out)).unwrap();
    assert_eq!(
        (salvage.valid_len, salvage.fragments),
        (out.len() as u64, 3)
    );
    // Video only: 89999 ticks at 90 kHz.
    assert_eq!(salvage.duration_us, 999_989);
    let (at, bytes) = salvage.mehd_patch.unwrap();
    assert_eq!(bmff::be64(&out, at as usize), info.duration_ms);
    assert_eq!(bytes, 1000u64.to_be_bytes());
}
