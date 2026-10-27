//! Prefix SEI reaches the decoder: HDR10+ and HDR Vivid ride in it on HEVC. The AUD
//! still goes, and frame starts, key frames and the parameter sets come out as before.

use super::*;

const HEVC: u32 = 0x100;

fn nal(out: &mut Vec<u8>, header: &[u8], body_len: usize) -> std::ops::Range<usize> {
    let start = out.len();
    out.extend_from_slice(&[0, 0, 0, 1]);
    out.extend_from_slice(header);
    out.extend((0..body_len as u32).map(|i| (i % 247) as u8 | 1));
    start..out.len()
}

/// An HEVC prefix SEI with a T.35 payload, as the host splices HDR10+ in.
fn hevc_sei(out: &mut Vec<u8>) -> std::ops::Range<usize> {
    nal(out, &[0x4e, 0x01, 0x04, 0x40, 0xb5], 60)
}

fn hevc_aud(out: &mut Vec<u8>) {
    nal(out, &[0x46, 0x01, 0x50], 0);
}

/// Receives each payload as one frame (kind 2 IDR, 1 P).
fn receive(format: u32, payloads: &[(&[u8], u8)]) -> Vec<VideoEvent> {
    let mut host = Host::new();
    let mut rx = VideoReceiver::new(config(format));
    let mut events = Vec::new();
    for (payload, kind) in payloads {
        for packet in host.frame(payload, *kind) {
            rx.receive(&packet, 1_000_000, &mut events);
        }
    }
    events
}

fn kinds(frame: &Frame) -> Vec<BufferType> {
    frame.buffers().iter().map(|b| b.kind).collect()
}

/// The picture data as renderers submit it: every PicData buffer back to back.
fn picture(frame: &Frame) -> Vec<u8> {
    frame
        .buffer_slices()
        .filter(|(kind, _)| *kind == BufferType::PicData)
        .flat_map(|(_, bytes)| bytes.iter().copied())
        .collect()
}

#[test]
fn hevc_p_frames_keep_their_prefix_sei() {
    let mut p = Vec::new();
    hevc_aud(&mut p);
    let sei = hevc_sei(&mut p);
    let slice = nal(&mut p, &[0x02, 0x01], 3000);
    let events = receive(HEVC, &[(&hevc_idr(), 2), (&p, 1)]);
    let frames = frames(&events);
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[1].frame_type, FrameType::P);
    assert!(kinds(frames[1]).iter().all(|&k| k == BufferType::PicData));
    // The AUD goes; the SEI and the slice follow each other unchanged.
    assert_eq!(&frames[1].data()[..slice.end - sei.start], &p[sei.start..]);
}

/// A key frame without SEI, so the receiver takes the P-frames after it.
fn hevc_idr() -> Vec<u8> {
    let mut idr = Vec::new();
    nal(&mut idr, &[0x40, 0x01], 20);
    nal(&mut idr, &[0x42, 0x01], 30);
    nal(&mut idr, &[0x44, 0x01], 6);
    nal(&mut idr, &[0x26, 0x01], 1000);
    idr
}

#[test]
fn hevc_key_frames_carry_the_sei_after_their_parameter_sets() {
    let mut idr = Vec::new();
    hevc_aud(&mut idr);
    let vps = nal(&mut idr, &[0x40, 0x01], 20);
    let sps = nal(&mut idr, &[0x42, 0x01], 30);
    let pps = nal(&mut idr, &[0x44, 0x01], 6);
    let sei = hevc_sei(&mut idr);
    let slice = nal(&mut idr, &[0x26, 0x01], 3000);
    let events = receive(HEVC, &[(&idr, 2)]);
    let frames = frames(&events);
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].frame_type, FrameType::Idr);
    let kinds = kinds(frames[0]);
    assert_eq!(
        kinds[..5],
        [
            BufferType::Vps,
            BufferType::Sps,
            BufferType::Pps,
            BufferType::PicData,
            BufferType::PicData
        ]
    );
    let slices: Vec<_> = frames[0].buffer_slices().map(|(_, b)| b).collect();
    assert_eq!(
        slices[..4],
        [&idr[vps], &idr[sps], &idr[pps], &idr[sei.clone()]]
    );
    assert_eq!(
        &picture(frames[0])[..slice.end - sei.start],
        &idr[sei.start..]
    );
}

/// SEI before the parameter sets moves behind them: a renderer writes each parameter set
/// at the start of its buffer, over picture data queued before it.
#[test]
fn sei_before_the_parameter_sets_moves_behind_them() {
    let mut idr = Vec::new();
    hevc_aud(&mut idr);
    let first = hevc_sei(&mut idr);
    nal(&mut idr, &[0x40, 0x01], 20);
    nal(&mut idr, &[0x42, 0x01], 30);
    let between = hevc_sei(&mut idr);
    let pps = nal(&mut idr, &[0x44, 0x01], 6);
    let slice = nal(&mut idr, &[0x26, 0x01], 3000);
    let events = receive(HEVC, &[(&idr, 2)]);
    let frames = frames(&events);
    assert_eq!(frames[0].frame_type, FrameType::Idr);
    let kinds = kinds(frames[0]);
    assert_eq!(
        kinds[..3],
        [BufferType::Vps, BufferType::Sps, BufferType::Pps]
    );
    assert!(kinds[3..].iter().all(|&k| k == BufferType::PicData));
    let mut expected = idr[first].to_vec();
    expected.extend_from_slice(&idr[between]);
    expected.extend_from_slice(&idr[slice]);
    assert_eq!(&picture(frames[0])[..expected.len()], &expected[..]);
    assert_eq!(
        frames[0].buffer_slices().nth(2).unwrap().1,
        &idr[pps],
        "the PPS stays whole"
    );
}

#[test]
fn h264_keeps_sei_and_still_drops_a_p_frames_pps() {
    let mut idr = Vec::new();
    nal(&mut idr, &[0x09, 0xf0], 0);
    let sei = nal(&mut idr, &[0x06, 0x05], 20);
    nal(&mut idr, &[0x67, 0x42, 0x00, 0x1f], 8);
    nal(&mut idr, &[0x68, 0xce], 3);
    let idr_slice = nal(&mut idr, &[0x65], 2000);
    let mut p = Vec::new();
    nal(&mut p, &[0x09, 0xf0], 0);
    let p_sei = nal(&mut p, &[0x06, 0x05], 20);
    nal(&mut p, &[0x68, 0xce], 3);
    let p_slice = nal(&mut p, &[0x41], 2000);
    let events = receive(0x1, &[(&idr, 2), (&p, 1)]);
    let frames = frames(&events);
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0].frame_type, FrameType::Idr);
    assert_eq!(
        kinds(frames[0])[..3],
        [BufferType::Sps, BufferType::Pps, BufferType::PicData]
    );
    let mut expected = idr[sei].to_vec();
    expected.extend_from_slice(&idr[idr_slice]);
    assert_eq!(&picture(frames[0])[..expected.len()], &expected[..]);
    assert_eq!(frames[1].frame_type, FrameType::P);
    let mut expected = p[p_sei].to_vec();
    expected.extend_from_slice(&p[p_slice]);
    assert_eq!(&frames[1].data()[..expected.len()], &expected[..]);
}

#[test]
fn frames_without_sei_are_unchanged() {
    let mut idr = Vec::new();
    hevc_aud(&mut idr);
    nal(&mut idr, &[0x40, 0x01], 20);
    nal(&mut idr, &[0x42, 0x01], 30);
    nal(&mut idr, &[0x44, 0x01], 6);
    let slice = nal(&mut idr, &[0x26, 0x01], 3000);
    let mut p = Vec::new();
    hevc_aud(&mut p);
    let p_slice = nal(&mut p, &[0x02, 0x01], 3000);
    let events = receive(HEVC, &[(&idr, 2), (&p, 1)]);
    let frames = frames(&events);
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0].frame_type, FrameType::Idr);
    let (kind, first) = frames[0].buffer_slices().nth(3).unwrap();
    assert_eq!(kind, BufferType::PicData);
    assert!(first.starts_with(&idr[slice.start..slice.start + 6]));
    assert_eq!(frames[1].frame_type, FrameType::P);
    assert_eq!(&frames[1].data()[..p_slice.len()], &p[p_slice]);
}
