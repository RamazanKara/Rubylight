use super::*;
use rubylight_protocol::pyrowave::early::{EarlyPush, Finish};
use rubylight_protocol::pyrowave::{
    Records, PACKET_CONTIGUOUS, PACKET_FRAME_START, PACKET_RECORD_START,
};

/// A records frame of `count` records as (flags, payload) packets: the 16-byte prefix with the
/// sequence header, then one 16-byte record per packet, every packet padded to `size` bytes.
fn records_frame(count: u32, size: usize) -> Vec<(u32, Vec<u8>)> {
    let word = |bytes: &mut Vec<u8>, value: u32| bytes.extend_from_slice(&value.to_le_bytes());
    let mut packets = Vec::new();
    let mut first = vec![1, 0, 0, 0, 16, 0, 0, 0];
    word(&mut first, 0x8000_0000 | (1 << 28) | 63 | (63 << 14));
    word(&mut first, count);
    packets.push(first);
    for index in 0..count {
        let mut packet = Vec::new();
        word(&mut packet, (1 << 28) | (4 << 16));
        word(&mut packet, index << 8);
        word(&mut packet, index);
        word(&mut packet, !index);
        packets.push(packet);
    }
    let last = packets.len() - 1;
    packets
        .into_iter()
        .enumerate()
        .map(|(i, mut payload)| {
            if i != last && payload.len() < size {
                // A padding block: its marker, then the words after its 8 bytes.
                let words = ((size - payload.len() - 8) / 4) as u32;
                word(&mut payload, u32::MAX);
                word(&mut payload, words);
                payload.resize(size, 0);
            }
            let mut flags = PACKET_RECORD_START;
            if i == 0 {
                flags |= PACKET_FRAME_START;
            } else {
                flags |= PACKET_CONTIGUOUS;
            }
            if i == last {
                flags |= PACKET_FRAME_END;
            }
            (flags, payload)
        })
        .collect()
}

fn envelope(packets: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let mut out = vec![0; 4];
    for (flags, payload) in packets {
        out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        out.extend_from_slice(&flags.to_le_bytes());
        out.extend_from_slice(payload);
    }
    out
}

fn buffer(kind: BufferType, range: std::ops::Range<usize>) -> FrameBuffer {
    FrameBuffer { kind, range }
}

#[test]
fn pyrowave_hands_off_in_place_by_default_and_mediacodec_only_when_asked() {
    use nightjar_core::sdp::{
        VIDEO_FORMAT_AV1_MAIN8, VIDEO_FORMAT_H265, VIDEO_FORMAT_PYROWAVE,
        VIDEO_FORMAT_PYROWAVE_MAIN10_444,
    };
    assert!(direct_hand_off(
        VIDEO_FORMAT_PYROWAVE_MAIN10_444,
        "",
        "",
        false
    ));
    // Unset properties read as empty.
    assert!(direct_hand_off(VIDEO_FORMAT_PYROWAVE, "", "", false));
    assert!(!direct_hand_off(VIDEO_FORMAT_PYROWAVE, "0", "1", true));
    assert!(!direct_hand_off(VIDEO_FORMAT_H265, "", "", false));
    assert!(!direct_hand_off(VIDEO_FORMAT_AV1_MAIN8, "1", "0", false));
    assert!(direct_hand_off(VIDEO_FORMAT_H265, "0", "1", false));
    assert!(direct_hand_off(VIDEO_FORMAT_AV1_MAIN8, "", "1", false));
}

#[test]
fn the_codec_direct_setting_holds_unless_the_property_is_set() {
    use nightjar_core::sdp::{VIDEO_FORMAT_H265, VIDEO_FORMAT_PYROWAVE};
    assert!(direct_hand_off(VIDEO_FORMAT_H265, "", "", true));
    assert!(direct_hand_off(VIDEO_FORMAT_H265, "", "other", true));
    assert!(!direct_hand_off(VIDEO_FORMAT_H265, "", "0", true));
    assert!(direct_hand_off(VIDEO_FORMAT_H265, "", "1", false));
    // PyroWave has its own switch; the MediaCodec setting does not reach it.
    assert!(!direct_hand_off(VIDEO_FORMAT_PYROWAVE, "0", "", true));
}

#[test]
fn the_frame_array_grows_by_half_so_creeping_frames_rarely_make_a_new_one() {
    assert_eq!(grown_capacity(1000, 0), 1000, "the first array is the frame's size");
    assert_eq!(grown_capacity(1001, 1000), 1500, "a slightly larger frame gets room to grow");
    assert_eq!(grown_capacity(4000, 1000), 4000, "a much larger one its own size");
    assert_eq!(
        grown_capacity(usize::MAX, 1000),
        i32::MAX as usize,
        "never past a Java array's size"
    );
    // Frames creeping up by 1 % from 100 KB to 1 MB: an array each time before, a few now.
    let (mut capacity, mut made, mut needed) = (0usize, 0, 100_000f64);
    while needed < 1_000_000.0 {
        if needed as usize > capacity {
            capacity = grown_capacity(needed as usize, capacity);
            made += 1;
        }
        needed *= 1.01;
    }
    assert!(made <= 8, "{made} arrays");
}

#[test]
fn only_picture_data_in_one_piece_goes_in_place() {
    use BufferType::{PicData, Pps, Sps, Vps};
    // A PyroWave or AV1 frame: one picture buffer, the whole frame.
    assert_eq!(contiguous_picture(&[buffer(PicData, 0..900)]), Some(0..900));
    // An HEVC key frame: parameter sets first, then picture slices back to back.
    let idr = [
        buffer(Vps, 0..24),
        buffer(Sps, 24..64),
        buffer(Pps, 64..72),
        buffer(PicData, 72..500),
        buffer(PicData, 500..1200),
    ];
    assert_eq!(contiguous_picture(&idr), Some(72..1200));
    // With its picture in place the array only carries the largest parameter set.
    assert_eq!(array_bytes(&idr, 1200, true), 40);
    assert_eq!(array_bytes(&idr, 1200, false), 1200);
    assert_eq!(array_bytes(&[buffer(PicData, 0..900)], 900, true), 0);
    // Picture data split by a parameter set keeps the array, which joins it.
    let split = [
        buffer(PicData, 0..100),
        buffer(Sps, 100..120),
        buffer(PicData, 120..300),
    ];
    assert_eq!(contiguous_picture(&split), None);
    // No picture data, or none of it, has nothing to hand over.
    assert_eq!(contiguous_picture(&[buffer(Sps, 0..20)]), None);
    assert_eq!(contiguous_picture(&[buffer(PicData, 40..40)]), None);
}

#[test]
fn early_push_finishes_a_frame_handed_over_in_place() {
    // The in-place picture of a records frame is its envelope, byte for byte, so early push
    // finishes it as it did the array's copy.
    let packets = records_frame(40, 1400);
    let data = envelope(&packets);
    let picture = contiguous_picture(&[buffer(BufferType::PicData, 0..data.len())]).unwrap();
    let mut records = Records::default();
    let mut early = EarlyPush::default();
    let mut batch = ChunkBatch::default();
    for (i, (flags, payload)) in packets.iter().enumerate() {
        if batch.push(i == 0, *flags, payload) {
            early.chunks(&mut records, &batch.bytes, batch.first, |_| true);
            batch.clear();
        }
    }
    assert_eq!(
        early.finish(&mut records, &data[picture], |_| true),
        Finish::Done
    );
}

#[test]
fn chunks_go_in_envelope_batches_that_end_with_the_frame() {
    let packets = records_frame(3, 24);
    let mut batch = ChunkBatch::default();
    assert!(!batch.push(true, packets[0].0, &packets[0].1));
    assert!(!batch.push(false, packets[1].0, &packets[1].1));
    assert!(batch.first);
    // The batch is the envelope's own bytes, without the container's leading zero word.
    assert_eq!(batch.bytes, envelope(&packets[..2])[4..]);
    batch.clear();
    assert!(!batch.push(false, packets[2].0, &packets[2].1));
    assert!(
        !batch.first,
        "a batch after the first does not start the frame"
    );
    assert!(
        batch.push(false, packets[3].0, &packets[3].1),
        "the frame's end goes at once"
    );

    // A full batch goes too, and the buffer keeps its allocation for the next one.
    batch.clear();
    let payload = [0u8; 1400];
    let full = (0..)
        .position(|_| batch.push(false, PACKET_CONTIGUOUS, &payload))
        .unwrap();
    assert_eq!(full + 1, CHUNK_BATCH_BYTES.div_ceil(8 + payload.len()));
    let capacity = batch.bytes.capacity();
    batch.clear();
    assert!(batch.bytes.is_empty() && batch.bytes.capacity() == capacity);

    // A frame that starts drops what was left of one that never came.
    batch.push(false, PACKET_CONTIGUOUS, &payload);
    batch.push(true, packets[0].0, &packets[0].1);
    assert_eq!(batch.bytes, envelope(&packets[..1])[4..]);
    assert!(batch.first);
}

#[test]
fn forwarded_batches_give_the_renderer_the_frame_whole_frame_push_gives() {
    for size in [24, 64, 1400] {
        let packets = records_frame(40, size);
        let data = envelope(&packets);
        let mut whole = Vec::new();
        assert!(Records::default().push_frame(&data, |packet| {
            whole.push(packet.to_vec());
            true
        }));
        // What the renderer does with each bridgeDrSubmitChunks call, then the frame.
        let (mut records, mut early, mut batch) = (
            Records::default(),
            EarlyPush::default(),
            ChunkBatch::default(),
        );
        let mut pushed = Vec::new();
        let mut calls = 0;
        for (i, (flags, payload)) in packets.iter().enumerate() {
            if batch.push(i == 0, *flags, payload) {
                calls += 1;
                early.chunks(&mut records, &batch.bytes, batch.first, |packet| {
                    pushed.push(packet.to_vec());
                    true
                });
                batch.clear();
            }
        }
        assert!(batch.bytes.is_empty(), "the end packet sent the rest");
        let before_frame = pushed.len();
        let finish = early.finish(&mut records, &data, |packet| {
            pushed.push(packet.to_vec());
            true
        });
        assert_eq!(finish, Finish::Done, "{size}-byte packets");
        assert_eq!(before_frame, 40, "every record before the frame");
        assert_eq!(
            pushed.last(),
            whole.first(),
            "only the header waits for the frame"
        );
        pushed.sort();
        whole.sort();
        assert_eq!(pushed, whole);
        assert!(calls >= data.len() / CHUNK_BATCH_BYTES);
    }
}

#[test]
fn pcm_packets_become_samples_and_lost_ones_silence() {
    let mut pcm = [7i16; 4];
    assert_eq!(
        pcm_s16le(Some(&[0x01, 0x00, 0xFF, 0xFF, 0x00, 0x80]), &mut pcm),
        3
    );
    assert_eq!(pcm[..3], [1, -1, i16::MIN]);
    assert_eq!(pcm_s16le(None, &mut pcm), 4);
    assert_eq!(pcm, [0; 4]);
    // A packet longer than the buffer is cut to it, never written past it.
    assert_eq!(pcm_s16le(Some(&[1; 12]), &mut pcm), 4);
    assert_eq!(pcm, [0x0101; 4]);
}

#[test]
fn oversized_passthrough_frames_are_dropped_and_logged_once() {
    let mut logged = false;
    let frame = [0u8; MAX_ENCODED_FRAME];
    assert_eq!(playable_frame(Some(&frame), &mut logged), Some(&frame[..]));
    assert_eq!(playable_frame(None, &mut logged), None);
    assert!(!logged);
    let oversized = [0u8; MAX_ENCODED_FRAME + 1];
    assert_eq!(playable_frame(Some(&oversized), &mut logged), None);
    assert!(logged);
    assert_eq!(playable_frame(Some(&oversized), &mut logged), None);
    assert_eq!(playable_frame(Some(&frame), &mut logged), Some(&frame[..]));
}
