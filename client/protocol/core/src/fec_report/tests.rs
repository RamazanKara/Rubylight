use super::*;

#[test]
fn fec_choices_parse_and_print() {
    for (text, choice) in [
        ("host", FecChoice::Host),
        ("auto", FecChoice::Auto),
        ("0", FecChoice::Fixed(0)),
        ("20", FecChoice::Fixed(20)),
        ("255", FecChoice::Fixed(255)),
    ] {
        assert_eq!(FecChoice::parse(text), Some(choice), "{text}");
        assert_eq!(FecChoice::parse(&choice.as_param()), Some(choice));
    }
    for text in [
        "256", "-1", "+5", "05x", "", " 5", "Host", "AUTO", "1e2", "999999",
    ] {
        assert_eq!(FecChoice::parse(text), None, "{text:?}");
    }
}

#[test]
fn fec_reports_round_trip_and_reject_nonsense() {
    let report = FecReport {
        seq: 7,
        data_shards: 400,
        parity_shards: 80,
        missing_shards: 12,
        unrecovered_blocks: 1,
        interval_ms: 1000,
    };
    let wire = report.encode();
    assert_eq!(wire.len(), FEC_REPORT_BYTES);
    assert_eq!(wire[..4], [1, 0, 0, 0]);
    assert_eq!(wire[22..24], 1000u16.to_le_bytes());
    assert_eq!(FecReport::decode(&wire), Some(report));
    // A newer version's tail is ignored.
    let mut longer = wire.to_vec();
    longer.extend_from_slice(&[9; 8]);
    longer[0] = 2;
    assert_eq!(FecReport::decode(&longer), Some(report));
    assert_eq!(FecReport::decode(&wire[..FEC_REPORT_BYTES - 1]), None);
    let mut zero = wire;
    zero[0] = 0;
    assert_eq!(FecReport::decode(&zero), None);
    let all_missing = FecReport {
        missing_shards: 480,
        ..report
    };
    assert_eq!(FecReport::decode(&all_missing.encode()), Some(all_missing));
    let too_many = FecReport {
        missing_shards: 481,
        ..report
    };
    assert_eq!(FecReport::decode(&too_many.encode()), None);
    // No overflow at the extremes.
    let huge = FecReport {
        data_shards: u32::MAX,
        parity_shards: u32::MAX,
        missing_shards: u32::MAX,
        ..report
    };
    assert_eq!(FecReport::decode(&huge.encode()), Some(huge));
    for interval_ms in [0, 249, 5001, u16::MAX] {
        let report = FecReport {
            interval_ms,
            ..report
        };
        assert_eq!(FecReport::decode(&report.encode()), None, "{interval_ms}");
    }
    for interval_ms in [250, 5000] {
        let report = FecReport {
            interval_ms,
            ..report
        };
        assert_eq!(FecReport::decode(&report.encode()), Some(report));
    }
}

/// A finished block of `data` + `parity` shards at frame `frame`, block 0 of 1.
fn block(
    frame: u32,
    data: u32,
    parity: u32,
    arrived: u32,
    missing: u32,
    lost: bool,
) -> FecBlock {
    FecBlock {
        frame,
        block_index: 0,
        block_count: 1,
        data_shards: data,
        parity_shards: parity,
        arrived_shards: arrived,
        missing_shards: missing,
        unrecovered: lost,
    }
}

#[test]
fn the_tally_reports_once_a_second_and_drops_what_it_cannot_time() {
    let mut tally = FecTally::default();
    assert_eq!(tally.take(5_000), None, "nothing counted yet");
    tally.block(block(1, 10, 2, 11, 1, false), 1_000);
    tally.block(block(2, 10, 2, 0, 0, true), 1_400);
    assert_eq!(tally.take(1_999), None, "under a second");
    let first = tally.take(2_050).unwrap();
    assert_eq!(
        first,
        FecReport {
            seq: 0,
            data_shards: 20,
            parity_shards: 4,
            missing_shards: 13,
            unrecovered_blocks: 1,
            interval_ms: 1_050,
        }
    );
    assert_eq!(FecReport::decode(&first.encode()), Some(first));
    // A second without video sends a report without shards, which the host takes: its
    // reports stay fresh while the picture stands still.
    let idle = tally.take(3_100).unwrap();
    assert_eq!(
        idle,
        FecReport {
            seq: 1,
            interval_ms: 1_050,
            ..FecReport::default()
        }
    );
    assert_eq!(FecReport::decode(&idle.encode()), Some(idle));
    tally.block(block(3, 5, 1, 6, 0, false), 3_200);
    let second = tally.take(4_100).unwrap();
    assert_eq!(
        (second.seq, second.data_shards, second.interval_ms),
        (2, 5, 1_000)
    );
    // A stalled sender: counts older than five seconds are dropped, the sequence goes on.
    tally.block(block(4, 5, 1, 6, 0, false), 4_200);
    assert_eq!(tally.take(9_101), None);
    tally.block(block(5, 5, 1, 6, 0, false), 9_200);
    assert_eq!(tally.take(10_101).unwrap().seq, 3);
    // Counts saturate instead of wrapping.
    let mut full = FecTally::default();
    for frame in 1..4 {
        full.block(block(frame, u32::MAX, u32::MAX, 0, 0, true), 0);
    }
    let report = full.take(1_000).unwrap();
    assert_eq!(report.data_shards, u32::MAX);
    assert_eq!(report.unrecovered_blocks, 3);
}

#[test]
fn a_block_that_could_not_be_recovered_is_reported_after_a_quarter_second() {
    let mut tally = FecTally::default();
    tally.block(block(1, 10, 2, 12, 0, false), 1_000);
    assert_eq!(tally.take_lost(1_900), None, "a clean link waits out the second");
    assert_eq!(tally.take(1_999), None);
    assert_eq!(tally.take(2_000).unwrap().interval_ms, 1_000);
    tally.block(block(2, 10, 2, 0, 0, true), 2_100);
    assert_eq!(tally.take_lost(2_249), None, "never within 250 ms of the last report");
    let early = tally.take_lost(2_250).unwrap();
    assert_eq!((early.seq, early.unrecovered_blocks, early.interval_ms), (1, 1, 250));
    assert_eq!(FecReport::decode(&early.encode()), Some(early));
    // The next interval starts at the early report.
    tally.block(block(3, 10, 2, 12, 0, false), 2_300);
    assert_eq!(tally.take_lost(2_600), None);
    assert_eq!(tally.take(3_249), None);
    assert_eq!(tally.take(3_250).unwrap().interval_ms, 1_000);
}
#[test]
fn the_tally_counts_what_it_could_see_and_blocks_that_never_came() {
    let mut tally = FecTally::default();
    let mut now = 0;
    let mut counted = |b: FecBlock| {
        tally.block(b, now);
        now += 1_000;
        let r = tally.take(now).unwrap();
        assert!(r.missing_shards <= r.data_shards + r.parity_shards);
        (
            r.data_shards,
            r.parity_shards,
            r.missing_shards,
            r.unrecovered_blocks,
        )
    };
    // Complete before its parity arrived: the parity in flight is neither sent nor lost.
    assert_eq!(counted(block(1, 10, 4, 10, 0, false)), (10, 0, 0, 0));
    // One hole before the last arrival, two parity shards still on the way.
    assert_eq!(counted(block(2, 10, 4, 11, 1, false)), (10, 2, 1, 0));
    // Given up: every shard of the block was due.
    assert_eq!(counted(block(3, 10, 4, 7, 2, true)), (10, 4, 7, 1));
    // Frames 4 and 5 never arrived: lost whole, the size of the next one.
    assert_eq!(counted(block(6, 10, 4, 10, 0, false)), (10 + 20, 8, 28, 2));
    // Block 1 of frame 7 never arrived, then the next frame begins after block 2 of 3.
    let first = FecBlock {
        block_count: 3,
        ..block(7, 4, 1, 5, 0, false)
    };
    assert_eq!(counted(first), (4, 1, 0, 0));
    let third = FecBlock {
        block_index: 2,
        ..first
    };
    assert_eq!(counted(third), (8, 2, 5, 1));
    assert_eq!(counted(block(8, 10, 4, 10, 0, false)), (10, 0, 0, 0));
    // The rest of a frame after a completed block never came; after a given-up block the
    // queue drops it, so it doesn't count.
    let two = |frame, lost| FecBlock {
        block_count: 2,
        ..block(frame, 4, 1, if lost { 0 } else { 5 }, 0, lost)
    };
    assert_eq!(counted(two(9, false)), (4, 1, 0, 0));
    assert_eq!(counted(block(10, 10, 4, 10, 0, false)), (14, 1, 5, 1));
    assert_eq!(counted(two(11, true)), (4, 1, 5, 1));
    assert_eq!(counted(block(12, 10, 4, 10, 0, false)), (10, 0, 0, 0));
    // A jump past MAX_SKIPPED_FRAMES or a step back counts nothing extra.
    let far = 12 + MAX_SKIPPED_FRAMES + 2;
    assert_eq!(counted(block(far, 10, 4, 10, 0, false)), (10, 0, 0, 0));
    assert_eq!(counted(block(far - 5, 10, 4, 10, 0, false)), (10, 0, 0, 0));
    // The longest gap that counts.
    let gap = far - 5 + MAX_SKIPPED_FRAMES + 1;
    let (_, _, missing, lost) = counted(block(gap, 10, 4, 10, 0, false));
    assert_eq!(
        (missing, lost),
        (14 * MAX_SKIPPED_FRAMES, MAX_SKIPPED_FRAMES as u16)
    );
    // Frame numbers wrap.
    let mut wrap = FecTally::default();
    wrap.block(block(u32::MAX, 10, 4, 14, 0, false), 0);
    wrap.block(block(1, 10, 4, 14, 0, false), 10);
    assert_eq!(wrap.take(1_000).unwrap().unrecovered_blocks, 1);
}

#[test]
fn fec_summaries_have_the_v_plus_wire_form() {
    // V+'s SS_FEC_SUMMARY: sequence, data packets, missing data packets, all BE32, which
    // Foundation's 0x5510 handler reads with util::endian::big after a 12-byte check.
    let summary = FecSummary {
        seq: 0x0102_0304,
        data_shards: 0x0000_0190,
        missing_data_shards: 0x0000_000c,
    };
    assert_eq!(
        summary.encode(),
        [1, 2, 3, 4, 0, 0, 1, 0x90, 0, 0, 0, 0x0c]
    );
    assert_eq!(FecSummary::decode(&summary.encode()), Some(summary));
    assert_eq!(FecSummary::decode(&summary.encode()[..11]), None);
    assert_eq!(FecSummary::decode(&[0; 13]), None);
    assert_eq!(FEC_SUMMARY_MESSAGE_TYPE, 0x5510);
    assert_eq!(FEC_CONTROL_DESCRIBE_ATTRIBUTE, "x-ss-video[0].fecControlVersion");
    assert_eq!(FEC_PERCENTAGE_ATTRIBUTE, "x-ml-video.fecPercentage");
    assert_eq!(FEC_SUMMARY_VERSION_ATTRIBUTE, "x-ml-video.fecSummaryVersion");
}

#[test]
fn the_summary_percentage_is_v_plus_video_fec_percentage() {
    // STREAM_CONFIGURATION.videoFecPercentage: -2 host, -1 auto, 0-100 fixed; Foundation
    // answers 400 to anything else.
    assert_eq!(FecChoice::Host.summary_percentage(), -2);
    assert_eq!(FecChoice::Auto.summary_percentage(), -1);
    assert_eq!(FecChoice::Fixed(0).summary_percentage(), 0);
    assert_eq!(FecChoice::Fixed(50).summary_percentage(), 50);
    assert_eq!(FecChoice::Fixed(100).summary_percentage(), 100);
    assert_eq!(FecChoice::Fixed(255).summary_percentage(), 100);
}

#[test]
fn the_summary_tally_counts_each_block_once_like_v_plus() {
    let mut tally = FecSummaryTally::default();
    // The first second: nothing finished, so nothing goes out, but the sequence moves on.
    assert_eq!(tally.take(), None);
    // A recovered block reports its final status first; the completion that follows is the
    // same block and doesn't count again.
    tally.block(1, 0, 10, 8);
    tally.block(1, 0, 10, 10);
    tally.block(1, 1, 6, 6);
    // Empty blocks and more received than sent count nothing.
    tally.block(2, 0, 0, 0);
    tally.block(2, 0, 4, 5);
    assert_eq!(
        tally.take(),
        Some(FecSummary {
            seq: 2,
            data_shards: 16,
            missing_data_shards: 2,
        })
    );
    // Counts start over each second; a block repeated after a take still doesn't count.
    tally.block(1, 1, 6, 0);
    tally.block(3, 0, 5, 0);
    assert_eq!(
        tally.take(),
        Some(FecSummary {
            seq: 3,
            data_shards: 5,
            missing_data_shards: 5,
        })
    );
    // A second holds at most a million data shards; the block that would pass it is left out.
    for frame in 0..15 {
        tally.block(10 + frame, 0, u16::MAX, u16::MAX);
    }
    let full = tally.take().unwrap();
    assert_eq!(full.data_shards, 15 * u32::from(u16::MAX));
    tally.block(100, 0, u16::MAX, 0);
    for frame in 0..14 {
        tally.block(200 + frame, 0, u16::MAX, u16::MAX);
    }
    tally.block(300, 0, u16::MAX, u16::MAX);
    let capped = tally.take().unwrap();
    assert!(capped.data_shards <= 1_000_000);
    assert_eq!(capped.data_shards, 15 * u32::from(u16::MAX));
    assert_eq!(capped.missing_data_shards, u32::from(u16::MAX));
}
