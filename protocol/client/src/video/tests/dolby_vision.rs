//! Dolby Vision profile 8.1 from the Rubylight host (`hdr_dolby_vision`): each
//! HEVC access unit ends with the RPU, NAL unit type 62, after the last slice.
//! The depacketizer skips only an AUD and SEI at the frame's start, so the RPU
//! must reach the decoder as the end of the frame's picture data, on key frames
//! and on P-frames spanning several packets.

use super::*;

/// NAL type 62, layer 0, TemporalId 0; an RPU ends with 0x80.
fn rpu() -> Vec<u8> {
    let mut nal = vec![0, 0, 0, 1, 62 << 1, 0x01, 0x19];
    nal.extend((0..120u32).map(|i| (i % 233) as u8 | 1));
    nal.push(0x80);
    nal
}

fn hevc_idr() -> Vec<u8> {
    let mut f = Vec::new();
    for header in [[32 << 1, 1], [33 << 1, 1], [34 << 1, 1]] {
        f.extend_from_slice(&[0, 0, 0, 1]);
        f.extend_from_slice(&header);
        f.extend_from_slice(&[0x0c, 0x01, 0xff]);
    }
    // IDR_W_RADL.
    f.extend_from_slice(&[0, 0, 0, 1, 19 << 1, 0x01]);
    f.extend((0..3000u32).map(|i| (i % 251) as u8 | 1));
    f.extend(rpu());
    f
}

fn hevc_p(len: usize) -> Vec<u8> {
    // TRAIL_R.
    let mut f = vec![0, 0, 0, 1, 1 << 1, 0x01];
    f.extend((0..len as u32).map(|i| (i % 249) as u8 | 1));
    f.extend(rpu());
    f
}

#[test]
fn the_rpu_after_the_last_slice_reaches_the_decoder() {
    let mut host = Host::new();
    let mut rx = VideoReceiver::new(config(crate::sdp::VIDEO_FORMAT_H265_MAIN10));
    let mut events = Vec::new();
    let (idr, p) = (hevc_idr(), hevc_p(5000));
    for packet in host.frame(&idr, 2).into_iter().chain(host.frame(&p, 1)) {
        rx.receive(&packet, 1_000_000, &mut events);
    }
    let frames = frames(&events);
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0].frame_type, FrameType::Idr);
    let kinds: Vec<_> = frames[0].buffers().iter().map(|b| b.kind).collect();
    assert_eq!(
        kinds[..4],
        [
            BufferType::Vps,
            BufferType::Sps,
            BufferType::Pps,
            BufferType::PicData
        ]
    );
    assert_eq!(frames[1].frame_type, FrameType::P);
    let rpu = rpu();
    for (frame, payload) in frames.iter().zip([&idr, &p]) {
        // The whole access unit, RPU last, ahead of the last shard's padding.
        assert_eq!(&frame.data()[..payload.len()], &payload[..]);
        let end = payload.len();
        assert_eq!(&frame.data()[end - rpu.len()..end], &rpu[..]);
        assert!(frame.data()[end..].iter().all(|&b| b == 0));
    }
}
