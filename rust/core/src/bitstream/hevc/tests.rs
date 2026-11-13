use super::*;

/// AMF's HEVC from the RX 7900 XT run that froze at gap 8: one slice a
/// frame for frames 0 to 259, each cut after its header. Frame 249 is the
/// repair of 241..=248 and names frame 239 as its long-term reference.
const STREAM: &[u8] = include_bytes!("../fixtures/amf-hevc-ltr.bin");

/// Per picture: its POC LSB width and value, and its long-term entries
/// (LSB, used, MSB cycle), read back with the parsers above.
type Header = (u32, u32, Vec<(u32, bool, Option<u32>)>);
fn headers(stream: &[u8]) -> Vec<Header> {
    let (mut sps, mut pps, mut pictures) = (None, None, Vec::new());
    for (start, end) in units(stream) {
        let nal = &stream[start..end];
        let kind = (nal[0] >> 1) & 63;
        let (rbsp, _) = unescape(&nal[2..], usize::MAX);
        match kind {
            33 => sps = Some(sequence_parameter_set(&rbsp).unwrap().1),
            34 => pps = Some(picture_parameter_set(&rbsp).unwrap().1),
            0..=21 => {
                let (sps, pps) = (sps.as_ref().unwrap(), pps.as_ref().unwrap());
                let mut reader = Reader::new(&rbsp);
                assert!(reader.flag().unwrap(), "one slice a frame");
                if (16..=23).contains(&kind) {
                    reader.skip(1).unwrap();
                }
                reader.ue().unwrap();
                reader.skip(pps.extra_bits).unwrap();
                reader.ue().unwrap();
                if kind == 19 || kind == 20 {
                    pictures.push((sps.lsb_bits, 0, Vec::new()));
                    continue;
                }
                let lsb = reader.bits(sps.lsb_bits).unwrap();
                if reader.flag().unwrap() {
                    reader.bits(ceil_log2(sps.sets.len() as u32)).unwrap();
                } else {
                    let count = sps.sets.len();
                    short_term_set(&mut reader, count, count, &sps.sets).unwrap();
                }
                let mut long_term = Vec::new();
                for _ in 0..reader.ue().unwrap() {
                    let lsb = reader.bits(sps.lsb_bits).unwrap();
                    let used = reader.flag().unwrap();
                    let cycle = reader.flag().unwrap().then(|| reader.ue().unwrap());
                    long_term.push((lsb, used, cycle));
                }
                pictures.push((sps.lsb_bits, lsb, long_term));
            }
            _ => {}
        }
    }
    pictures
}

/// The POC and full long-term POCs that a decoder given only the
/// pictures `shown` derives for each (H.265 8.3.1, 8.3.2); a long-term
/// entry without its MSB is `None`.
fn derived(
    headers: &[Header],
    shown: impl Fn(usize) -> bool,
) -> Vec<(usize, i64, Vec<Option<i64>>)> {
    let mut previous = 0i64;
    let mut pictures = Vec::new();
    for (index, (bits, lsb, long_term)) in headers.iter().enumerate() {
        if !shown(index) {
            continue;
        }
        let max = 1i64 << bits;
        let (previous_lsb, lsb) = (previous.rem_euclid(max), i64::from(*lsb));
        let msb = previous - previous_lsb;
        let poc = if index == 0 {
            0
        } else if lsb < previous_lsb && previous_lsb - lsb >= max / 2 {
            msb + max + lsb
        } else if lsb > previous_lsb && lsb - previous_lsb > max / 2 {
            msb - max + lsb
        } else {
            msb + lsb
        };
        let mut cycle = 0;
        let references = long_term
            .iter()
            .map(|&(lt, _, delta)| {
                delta.map(|delta| {
                    cycle += i64::from(delta);
                    poc - cycle * max - poc.rem_euclid(max) + i64::from(lt)
                })
            })
            .collect();
        pictures.push((index, poc, references));
        previous = poc;
    }
    pictures
}

#[test]
fn widened_pocs_let_a_decoder_that_skipped_frames_order_the_repair() {
    assert_eq!(ORDER_WINDOW, 128);
    let original = headers(STREAM);
    assert_eq!(original.len(), 260);
    assert!(original.iter().all(|(bits, ..)| *bits == 4));
    let rewritten = PictureOrder::default().rewrite(STREAM).unwrap().unwrap();
    let wide = headers(&rewritten);
    assert_eq!(wide.len(), 260);
    assert!(wide.iter().all(|(bits, ..)| *bits == POC_LSB_BITS));
    // Every picture keeps its order and its long-term references, now
    // named by MSB: the IDR or an anchor marked every fourth frame.
    for (index, poc, references) in derived(&wide, |_| true) {
        assert_eq!(poc, index as i64);
        assert_eq!(references.len(), original[index].2.len(), "frame {index}");
        for reference in references {
            let reference = reference.unwrap();
            assert!(
                reference < poc && (reference == 0 || reference % 4 == 3),
                "frame {index}: {reference}"
            );
        }
    }
    assert_eq!(wide[249].2[0], (239, true, Some(0)));
    // The client dropped 241..=248. From AMF's four bits it derives POC
    // 233 for the repair; from eight, 249, and the anchor it still has.
    let lossy = |index: usize| !(241..=248).contains(&index);
    let repair = |headers: &[Header]| {
        derived(headers, lossy)
            .into_iter()
            .find(|(index, ..)| *index == 249)
            .unwrap()
    };
    assert_eq!(repair(&original).1, 233);
    assert_eq!(repair(&wide).1, 249);
    assert_eq!(repair(&wide).2[0], Some(239));
    assert!(
        derived(&wide, lossy)
            .iter()
            .all(|(index, poc, _)| *poc == *index as i64)
    );
    // The window: 128 frames after the last one shown still order, 129
    // land 256 too low.
    for (last, ordered) in [(101, true), (100, false)] {
        let pictures = derived(&wide, |index| index <= last || index >= 229);
        let after = pictures.iter().find(|(index, ..)| *index == 229).unwrap();
        assert_eq!(after.1 == 229, ordered, "last shown {last}");
    }
    // Frame by frame, as AMF returns them, gives the same stream, and a
    // widened stream needs nothing more.
    let units = units(STREAM);
    let mut cuts: Vec<usize> = units[4..].iter().map(|unit| unit.0 - 4).collect();
    cuts.insert(0, 0);
    cuts.push(STREAM.len());
    let mut order = PictureOrder::default();
    let mut frames = Vec::new();
    for cut in cuts.windows(2) {
        let frame = &STREAM[cut[0]..cut[1]];
        frames.extend(order.rewrite(frame).unwrap().unwrap());
    }
    assert_eq!(frames, rewritten);
    assert_eq!(PictureOrder::default().rewrite(&rewritten), Ok(None));
}

/// A slice like `template`, one of `STREAM`'s P slices, for picture
/// `poc` naming the long-term pictures `long_term` (POCs, newest first,
/// the first used) as AMF writes them: four LSB bits and every MSB.
fn amf_slice(template: &[u8], poc: i64, long_term: &[i64]) -> Vec<u8> {
    let units = units(STREAM);
    let rbsp = |unit: (usize, usize)| unescape(&STREAM[unit.0 + 2..unit.1], usize::MAX).0;
    let sps = sequence_parameter_set(&rbsp(units[1])).unwrap().1;
    let pps = picture_parameter_set(&rbsp(units[2])).unwrap().1;
    assert!(sps.lsb_bits == 4 && sps.long_term && !pps.output_flag);
    let (rbsp, _) = unescape(&template[2..], usize::MAX);
    let mut reader = Reader::new(&rbsp);
    assert!(reader.flag().unwrap(), "one slice a frame");
    reader.ue().unwrap();
    reader.skip(pps.extra_bits).unwrap();
    let slice_type = reader.ue().unwrap();
    let poc_at = reader.at;
    reader.skip(4).unwrap();
    let short_term = if reader.flag().unwrap() {
        sps.sets[reader.bits(ceil_log2(sps.sets.len() as u32)).unwrap() as usize].clone()
    } else {
        let count = sps.sets.len();
        short_term_set(&mut reader, count, count, &sps.sets).unwrap()
    };
    let long_term_at = reader.at;
    let mut used = short_term.deltas().filter(|(_, used)| *used).count() as u32;
    for _ in 0..reader.ue().unwrap() {
        reader.skip(4).unwrap();
        used += u32::from(reader.flag().unwrap());
        if reader.flag().unwrap() {
            reader.ue().unwrap();
        }
    }
    let long_term_end = reader.at;
    // One long-term picture is used, in the template as in its copies.
    let end = rest_of_header(&mut reader, &sps, &pps, slice_type, used).unwrap();

    let mut writer = Writer::default();
    writer.copy(&rbsp, 0, poc_at);
    writer.bits(4, poc.rem_euclid(16) as u32);
    writer.copy(&rbsp, poc_at + 4, long_term_at);
    writer.ue(long_term.len() as u32);
    let mut previous = 0;
    for (index, full) in long_term.iter().enumerate() {
        writer.bits(4, full.rem_euclid(16) as u32);
        writer.flag(index == 0);
        writer.flag(true);
        let cycle = poc.div_euclid(16) - full.div_euclid(16);
        writer.ue((cycle - previous) as u32);
        previous = cycle;
    }
    writer.copy(&rbsp, long_term_end, end);
    let mut nal = vec![0, 0, 0, 1, template[0], template[1]];
    super::super::rbsp::escape(&writer.finish(), &mut nal);
    nal
}

const REPAIR: u64 = 801;
/// AMF's stream for wire frames 1..=840 as `References` plans it, POC
/// the wire frame less one, with the loss of `first_lost..REPAIR`
/// reported before frame `REPAIR`: the stream up to the end, or to the
/// repair the host turned into a keyframe, each picture's long-term
/// POCs, and whether it repaired.
fn amf_stream(first_lost: u64) -> (Vec<u8>, Vec<Vec<i64>>, bool) {
    use crate::ltr::{Counter, References};
    let units = units(STREAM);
    let template = &STREAM[units[7].0..units[7].1];
    // VPS, SPS, PPS and the IDR, wire frame 1.
    let mut stream = STREAM[..units[3].1].to_vec();
    let mut pictures = vec![Vec::new()];
    let mut references = References::new(4, Counter::PictureOrder);
    let mut slots = [None; 4];
    for frame in 1..=840u64 {
        if frame == REPAIR && !references.invalidate(first_lost, REPAIR - 1) {
            return (stream, pictures, false);
        }
        let plan = references.plan(frame, frame == 1);
        if let Some(slot) = plan.reference {
            // A repair names only its reference, which is all it keeps.
            slots = std::array::from_fn(|index| slots[index].filter(|_| index == slot));
        }
        if frame > 1 {
            let mut long_term: Vec<i64> = slots.iter().flatten().map(|&f| f as i64 - 1).collect();
            long_term.sort_unstable_by(|a, b| b.cmp(a));
            stream.extend(amf_slice(template, frame as i64 - 1, &long_term));
            pictures.push(long_term);
        }
        references.accepted(frame, &plan);
        if let Some(slot) = plan.mark {
            slots[slot] = Some(frame);
        }
    }
    (stream, pictures, true)
}

#[test]
fn a_long_stream_keeps_its_order_across_wraps_and_repairs_up_to_the_window() {
    // The client last showed wire frame 673 (POC 672), 128 before the
    // repair (POC 800, after the third wrap of eight bits at 768).
    let (stream, pictures, repaired) = amf_stream(674);
    assert!(repaired);
    assert_eq!(pictures.len(), 840);
    let rewritten = PictureOrder::default().rewrite(&stream).unwrap().unwrap();
    let wide = headers(&rewritten);
    assert_eq!(wide.len(), 840);
    assert!(wide.iter().all(|(bits, ..)| *bits == POC_LSB_BITS));
    for (index, poc, references) in derived(&wide, |_| true) {
        assert_eq!(poc, index as i64);
        let expected: Vec<_> = pictures[index].iter().map(|&poc| Some(poc)).collect();
        assert_eq!(references, expected, "frame {index}");
    }
    // Long-term pictures on both sides of the wrap at 768, and the IDR
    // three wraps back.
    assert_eq!(
        wide[772].2,
        [
            (3, true, Some(0)),
            (255, false, Some(1)),
            (251, false, Some(0)),
            (0, false, Some(2)),
        ]
    );
    assert_eq!(wide[800].2, [(0, true, Some(3))]);
    // 128 frames after its last picture the client orders the repair;
    // one more and it would derive POC 544, so the host sends a keyframe.
    for (last, ordered) in [(672, true), (671, false)] {
        let pictures = derived(&wide, |index| index <= last || index >= 800);
        let repair = pictures.iter().find(|(index, ..)| *index == 800).unwrap();
        assert_eq!(repair.1 == 800, ordered, "last shown {last}");
        if ordered {
            assert_eq!(repair.2, [Some(0)]);
            assert!(pictures.iter().all(|(index, poc, _)| *poc == *index as i64));
        } else {
            assert_eq!(repair.1, 544);
        }
    }
    let (_, _, repaired) = amf_stream(673);
    assert!(
        !repaired,
        "129 frames after the last picture shown is a keyframe"
    );
    // Frame by frame, as AMF returns them, gives the same stream.
    let units = super::units(&stream);
    let mut order = PictureOrder::default();
    let mut frames = order.rewrite(&stream[..units[4].0 - 4]).unwrap().unwrap();
    for unit in units[4..].windows(2) {
        frames.extend(
            order
                .rewrite(&stream[unit[0].0 - 4..unit[1].0 - 4])
                .unwrap()
                .unwrap(),
        );
    }
    let last = units.last().unwrap();
    frames.extend(order.rewrite(&stream[last.0 - 4..]).unwrap().unwrap());
    assert_eq!(frames, rewritten);
}

/// Deterministic bytes for the mutation test (xorshift).
fn random(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

#[test]
fn malformed_hevc_is_an_error_and_never_a_panic() {
    let units = units(STREAM);
    let (sps, slice) = (units[1], units[23]);
    let order = |bytes: &[u8]| PictureOrder::default().rewrite(bytes);
    // A slice before its parameter sets, a layered unit, and the SPS or
    // a P slice's header cut short.
    assert_eq!(
        order(&STREAM[slice.0 - 4..]),
        Err(Unsupported("slice before its PPS"))
    );
    let mut layered = STREAM.to_vec();
    layered[sps.0 + 1] |= 8;
    assert_eq!(order(&layered), Err(Unsupported("layered HEVC")));
    for cut in 3..14 {
        let mut bytes = STREAM[..sps.0 + cut].to_vec();
        bytes.extend_from_slice(&STREAM[sps.1..]);
        assert!(order(&bytes).is_err(), "SPS cut to {cut}");
    }
    for cut in 3..6 {
        let mut bytes = STREAM[..slice.0 + cut].to_vec();
        bytes.extend_from_slice(&STREAM[slice.1..]);
        assert!(order(&bytes).is_err(), "slice cut to {cut}");
    }
    // Flipped bits and cuts anywhere in the parameter sets and the first
    // frames: an answer either way, and a rewrite keeps every unit.
    let mut state = 0x9E37_79B9_7F4A_7C15;
    let mut rewrites = 0;
    for _ in 0..3000 {
        let mut bytes = STREAM[..units[12].1].to_vec();
        for _ in 0..1 + random(&mut state) % 4 {
            let at = random(&mut state) as usize % bytes.len();
            bytes[at] ^= 1 << (random(&mut state) % 8);
        }
        bytes.truncate(bytes.len() - random(&mut state) as usize % 64);
        if let Ok(Some(rewritten)) = order(&bytes) {
            assert_eq!(super::units(&rewritten).len(), super::units(&bytes).len());
            rewrites += 1;
        }
    }
    assert!(rewrites > 100, "{rewrites}");
}

/// `rewrite` as it stood before `rewrite_into`: the same units, with the
/// edits assembled here rather than by `assemble_into`.
fn rewritten_by_hand(order: &mut PictureOrder, bytes: &[u8]) -> Result<Option<Vec<u8>>> {
    let mut output = Vec::new();
    let mut cursor = 0;
    let mut changed = false;
    for (start, end) in units(bytes) {
        if let Some((head, rest)) = order.unit(&bytes[start..end])? {
            output.extend_from_slice(&bytes[cursor..start]);
            output.extend_from_slice(&head);
            output.extend_from_slice(&bytes[start + rest..end]);
            cursor = end;
            changed = true;
        }
    }
    output.extend_from_slice(&bytes[cursor..]);
    Ok(changed.then_some(output))
}

#[test]
fn rewrite_into_matches_the_hand_assembled_rewrite_and_keeps_its_buffer() {
    let expected = rewritten_by_hand(&mut PictureOrder::default(), STREAM)
        .unwrap()
        .unwrap();
    let mut output = Vec::with_capacity(STREAM.len() * 2);
    let (pointer, capacity) = (output.as_ptr(), output.capacity());
    // Stale bytes must not survive into the result.
    output.extend_from_slice(&[0xee; 64]);
    assert_eq!(
        PictureOrder::default().rewrite_into(STREAM, &mut output),
        Ok(true)
    );
    assert_eq!(output, expected);
    assert_eq!(output.as_ptr(), pointer);
    assert_eq!(output.capacity(), capacity);
    assert_eq!(
        PictureOrder::default().rewrite(STREAM).unwrap().unwrap(),
        expected
    );
    // A rewritten stream has nothing left to change: `false`, and the
    // buffer comes back empty for the caller to give back.
    let mut again = Vec::with_capacity(STREAM.len() * 2);
    assert_eq!(
        PictureOrder::default().rewrite_into(&expected, &mut again),
        Ok(false)
    );
    assert!(again.is_empty());
    assert_eq!(again.capacity(), STREAM.len() * 2);
}
