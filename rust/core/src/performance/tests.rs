use super::*;
fn fec(frame: u32, block: u8, data: u16, parity: u16, missing: u16) -> Vec<u8> {
    let mut payload = frame.to_be_bytes().to_vec();
    for word in [11, 0, missing, 10, 2, data, parity] {
        payload.extend(word.to_be_bytes());
    }
    payload.extend([20, block, 2]);
    payload
}
#[test]
fn fec_reports_count_frames_once_and_failed_blocks_override_recovery() {
    let mut p = Performance::default();
    let now = Instant::now();
    use crate::adaptive_fec::Block;
    let recovered = fec(7, 0, 9, 1, 1);
    assert_eq!(
        p.record_fec_status(&recovered, Some(9)),
        Some(Block::Recovered { lost: 1, data: 10 })
    );
    // A duplicate is counted, but not passed on twice.
    assert_eq!(p.record_fec_status(&recovered, Some(9)), None);
    assert_eq!(p.snapshot(now)["fec_recovered_frames"], 1);
    assert_eq!(
        p.record_fec_status(&fec(7, 1, 7, 1, 2), Some(9)),
        Some(Block::Unrecoverable)
    );
    assert_eq!(
        p.record_fec_status(&fec(8, 0, 10, 0, 0), Some(9)),
        Some(Block::Clean)
    );
    p.record_fec_status(&fec(9, 1, 8, 2, 2), Some(9));
    p.record_fec_status(&fec(9, 0, 9, 1, 1), Some(9));
    let snapshot = p.snapshot(now);
    assert_eq!(snapshot["fec_reports"], 6);
    assert_eq!(snapshot["fec_invalid_reports"], 0);
    assert_eq!(snapshot["fec_duplicate_reports"], 1);
    assert_eq!(snapshot["fec_recovered_frames"], 1);
    assert_eq!(snapshot["fec_unrecoverable_frames"], 1);
    assert_eq!(snapshot["fec_missing_packets"], 6);
    // Out-of-order reports must not reclassify a lost frame as recovered.
    p.record_fec_status(&fec(10, 1, 7, 1, 2), Some(10));
    p.record_fec_status(&fec(10, 0, 9, 1, 1), Some(10));
    assert_eq!(p.snapshot(now)["fec_recovered_frames"], 1);
    assert_eq!(p.snapshot(now)["fec_unrecoverable_frames"], 2);
    assert_eq!(p.snapshot(now)["sample_frames"], 0);
}
#[test]
fn fec_telemetry_rejects_invalid_unsent_stale_and_inconsistent_reports() {
    let mut p = Performance::default();
    p.record_fec_status(&fec(u32::MAX, 0, 9, 1, 1), Some(0));
    p.record_fec_status(&fec(0, 0, 9, 1, 1), Some(0));
    for (payload, last) in [
        (vec![0; 20], Some(0)),
        (fec(1, 0, 9, 1, 1), Some(0)),
        (fec(1, 0, 9, 1, 1), None),
        (fec(1, 0, 9, 1, 1), Some(1025)),
    ] {
        assert_eq!(p.record_fec_status(&payload, last), None);
    }
    let mut inconsistent = fec(0, 0, 9, 1, 1);
    inconsistent[20] = 1;
    assert_eq!(p.record_fec_status(&inconsistent, Some(0)), None);
    assert_eq!(p.fec_invalid_reports, 5);
    assert_eq!(p.fec_recovered_frames, 2);
    assert_eq!(p.fec_missing_packets, 2);
}
#[test]
fn reordered_fec_reports_evict_stale_frames_without_recounting_recent_ones() {
    let mut p = Performance::default();
    p.record_fec_status(&fec(1024, 0, 9, 1, 1), Some(1024));
    for frame in 1..1024 {
        p.record_fec_status(&fec(frame, 0, 10, 0, 0), Some(1024));
    }
    p.record_fec_status(&fec(1025, 0, 10, 0, 0), Some(1025));
    p.record_fec_status(&fec(1024, 0, 9, 1, 1), Some(1025));
    assert_eq!(p.fec_frames.len, 1024);
    assert_eq!(p.fec_recovered_frames, 1);
    assert_eq!(p.fec_missing_packets, 1);
    assert_eq!(p.fec_duplicate_reports, 1);
}
#[test]
fn fec_history_and_counters_are_bounded() {
    let mut p = Performance::default();
    for frame in 0..1500 {
        p.record_fec_status(&fec(frame, 0, 10, 0, 0), Some(frame));
    }
    assert_eq!(p.fec_frames.len, 1024);
    assert_eq!(p.fec_recovered_frames, 0);
    assert_eq!(p.fec_missing_packets, 0);
    p.fec_reports = u64::MAX;
    p.fec_invalid_reports = u64::MAX;
    p.record_fec_status(&[], Some(1500));
    assert_eq!(p.fec_reports, u64::MAX);
    assert_eq!(p.fec_invalid_reports, u64::MAX);
    p.fec_missing_packets = u64::MAX;
    p.record_fec_status(&fec(1500, 0, 9, 1, 1), Some(1500));
    assert_eq!(p.fec_missing_packets, u64::MAX);
}
/// The block bookkeeping as it was before [`FecFrames`]: a `retain` and a
/// search over every kept frame for each report.
#[derive(Default)]
struct RetainedFrames {
    frames: Vec<FecFrame>,
    invalid: u64,
    duplicate: u64,
    recovered: u64,
    unrecoverable: u64,
    missing: u64,
}
impl RetainedFrames {
    fn record(
        &mut self,
        payload: &[u8],
        last_sent: Option<u32>,
    ) -> Option<crate::adaptive_fec::Block> {
        let status = crate::fec_status::Status::parse(payload)
            .ok()
            .zip(last_sent)
            .filter(|(s, last)| last.wrapping_sub(s.frame) < 1024);
        let Some((status, last)) = status else {
            self.invalid += 1;
            return None;
        };
        self.frames.retain(|f| last.wrapping_sub(f.index) < 1024);
        let index = match self.frames.iter().position(|f| f.index == status.frame) {
            Some(index) => index,
            None => {
                self.frames.push(FecFrame {
                    index: status.frame,
                    blocks: 0,
                    block_count: status.blocks,
                    recovered: false,
                    unrecoverable: false,
                });
                self.frames.len() - 1
            }
        };
        let frame = &mut self.frames[index];
        if frame.block_count != status.blocks {
            self.invalid += 1;
            return None;
        }
        let block = 1 << status.block;
        if frame.blocks & block != 0 {
            self.duplicate += 1;
            return None;
        }
        frame.blocks |= block;
        self.missing += u64::from(status.missing_before_highest);
        let unrecoverable = u32::from(status.received_data) + u32::from(status.received_parity)
            < u32::from(status.data);
        if unrecoverable && !frame.unrecoverable {
            frame.unrecoverable = true;
            self.unrecoverable += 1;
            if frame.recovered {
                self.recovered -= 1;
            }
        } else if !unrecoverable
            && status.received_data < status.data
            && !frame.recovered
            && !frame.unrecoverable
        {
            frame.recovered = true;
            self.recovered += 1;
        }
        Some(if unrecoverable {
            crate::adaptive_fec::Block::Unrecoverable
        } else if status.received_data < status.data {
            crate::adaptive_fec::Block::Recovered {
                lost: status.data - status.received_data,
                data: status.data,
            }
        } else {
            crate::adaptive_fec::Block::Clean
        })
    }
}
#[test]
fn fec_frame_slots_report_what_retaining_every_frame_did() {
    let mut p = Performance::default();
    let mut reference = RetainedFrames::default();
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let mut random = |below: u32| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed % u64::from(below)) as u32
    };
    // Close to the u32 counter's wrap, which the run passes.
    let mut last = u32::MAX - 3000;
    for report in 0..40_000 {
        // Mostly a frame or none per report, sometimes a jump of a window or
        // more, and now and then backwards.
        last = match random(200) {
            0 => last.wrapping_add(1500 + random(3000)),
            1 => last.wrapping_sub(1 + random(40)),
            2 => last.wrapping_sub(1100 + random(3000)),
            3..=60 => last,
            61..=70 => last.wrapping_add(2 + random(30)),
            _ => last.wrapping_add(1),
        };
        // Up to a little older than the window, and now and then not sent yet.
        let frame = if random(50) == 0 {
            last.wrapping_add(1 + random(5))
        } else {
            last.wrapping_sub(random(1100))
        };
        // Ten data packets: some lost and repaired, some lost for good, and
        // now and then a count the parser refuses.
        let received = 8 + random(4) as u16;
        let mut payload = fec(
            frame,
            random(2) as u8,
            received,
            random(3) as u16,
            random(3) as u16,
        );
        // A frame reported with another block count than before is invalid.
        payload[20] = 1 + u8::from(random(8) != 0);
        let last_sent = (random(500) != 0).then_some(last);
        assert_eq!(
            p.record_fec_status(&payload, last_sent),
            reference.record(&payload, last_sent),
            "report {report}"
        );
    }
    assert_eq!(p.fec_invalid_reports, reference.invalid);
    assert_eq!(p.fec_duplicate_reports, reference.duplicate);
    assert_eq!(p.fec_recovered_frames, reference.recovered);
    assert_eq!(p.fec_unrecoverable_frames, reference.unrecoverable);
    assert_eq!(p.fec_missing_packets, reference.missing);
    assert!(reference.recovered > 0 && reference.unrecoverable > 0);
    assert!(reference.duplicate > 0 && reference.invalid > 0);
    // The same frames are kept, each as the reference keeps it.
    assert_eq!(p.fec_frames.len, reference.frames.len());
    for kept in &reference.frames {
        let slot = p.fec_frames.slots[(kept.index % FEC_WINDOW) as usize].as_ref();
        let slot = slot.expect("a kept frame has its slot");
        assert_eq!(
            (slot.index, slot.blocks, slot.block_count),
            (kept.index, kept.blocks, kept.block_count)
        );
        assert_eq!(
            (slot.recovered, slot.unrecoverable),
            (kept.recovered, kept.unrecoverable)
        );
    }
}
fn timing(encode: u64, host: u64, age: u64) -> Timing {
    Timing {
        period: Duration::from_millis(10),
        encode,
        host,
        age,
        sent: host,
    }
}
#[test]
fn present_to_send_includes_packetization_and_pacing_without_changing_wire_latency() {
    let now = Instant::now();
    let mut p = Performance::default();
    p.record_timing(
        now,
        Timing {
            period: Duration::from_millis(10),
            encode: 2000,
            host: 2500,
            age: 4000,
            sent: 12500,
        },
        1000,
    );
    let snapshot = p.snapshot(now);
    assert_eq!(snapshot["host_processing_mean_ms"], 2.5);
    assert_eq!(snapshot["encode_mean_ms"], 2.);
    assert_eq!(snapshot["frame_age_mean_ms"], 4.);
    assert_eq!(snapshot["present_to_send_mean_ms"], 16.5);
    assert_eq!(snapshot["present_to_send_p99_ms"], 16.5);
}
#[test]
fn frame_age_stays_visible_beside_the_reported_host_latency() {
    let start = Instant::now();
    let mut p = Performance::default();
    for i in 0..100 {
        let age = if i % 2 == 0 { 1000 } else { 15000 };
        p.record_timing(
            start + Duration::from_millis(i * 10),
            timing(2000, 2500, age),
            10000,
        );
    }
    let snapshot = p.snapshot(start + Duration::from_millis(990));
    assert_eq!(snapshot["encode_p95_ms"], 2.);
    assert_eq!(snapshot["encode_mean_ms"], 2.);
    assert_eq!(snapshot["host_processing_mean_ms"], 2.5);
    assert_eq!(snapshot["host_processing_max_ms"], 2.5);
    assert_eq!(snapshot["frame_age_mean_ms"], 8.);
    assert_eq!(snapshot["frame_age_p95_ms"], 15.);
    assert_eq!(snapshot["present_to_send_mean_ms"], 10.5);
    assert_eq!(snapshot["present_to_send_p95_ms"], 17.5);
    assert_eq!(snapshot["send_interval_p99_ms"], 10.);
    p.record_timing(start + Duration::from_secs(1), timing(2000, 3000, 0), 10000);
    let snapshot = p.snapshot(start + Duration::from_secs(1));
    assert_eq!(snapshot["history"][0]["host_processing_mean_ms"], 2.5);
    assert_eq!(snapshot["history"][0]["encode_mean_ms"], 2.);
    assert_eq!(snapshot["history"][0]["frame_age_mean_ms"], 8.);
    assert_eq!(
        p.snapshot(start + Duration::from_secs(4))["host_processing_max_ms"],
        0.
    );
}
#[test]
fn completion_intervals_include_stalls_and_small_samples_have_real_extrema() {
    let start = Instant::now();
    let mut p = Performance::default();
    p.record_timing(start, timing(2000, 3000, 1000), 100);
    p.record_timing(
        start + Duration::from_millis(8),
        timing(2000, 4000, 2000),
        100,
    );
    p.record_timing(
        start + Duration::from_millis(32),
        timing(2100, 3000, 900),
        100,
    );
    let snapshot = p.snapshot(start + Duration::from_millis(32));
    assert_eq!(snapshot["send_interval_max_ms"], 24.);
    assert_eq!(snapshot["send_stutters"], 1);
    assert_eq!(snapshot["frame_age_mean_ms"], 1.3);
    assert_eq!(snapshot["host_processing_max_ms"], 4.);
    assert_eq!(
        p.snapshot(start + Duration::from_secs(3))["send_interval_max_ms"],
        0.
    );
    assert_eq!(
        p.snapshot(start + Duration::from_secs(3))["send_stutters"],
        1
    );
}
#[test]
fn stutters_count_only_gaps_above_two_periods_and_survive_window_eviction() {
    let start = Instant::now();
    let mut p = Performance::default();
    for ms in [0, 10, 30, 51] {
        p.record_timing(start + Duration::from_millis(ms), timing(1, 1, 0), 1);
    }
    assert_eq!(
        p.snapshot(start + Duration::from_millis(51))["send_stutters"],
        1
    );
    for i in 1..=3000 {
        p.record_timing(
            start + Duration::from_millis(51 + i * 10),
            timing(1, 1, 0),
            1,
        );
    }
    assert_eq!(
        p.snapshot(start + Duration::from_millis(30051))["send_stutters"],
        1
    );
}
/// A picture `offsets` microseconds after `start` at each point.
fn stamps(start: Instant, offsets: [u64; 9]) -> Stamps {
    let at = |index: usize| start + Duration::from_micros(offsets[index]);
    Stamps {
        presented: at(0),
        acquired: at(1),
        published: at(2),
        claimed: at(3),
        submitted: at(4),
        encoded: at(5),
        packetized: at(6),
        prepared: at(7),
        first_sent: at(7),
        last_sent: at(8),
        streamed: None,
    }
}
#[test]
fn stages_split_present_to_last_packet_into_consecutive_parts() {
    let start = Instant::now();
    let picture = stamps(start, [0, 4200, 4250, 5100, 5900, 8400, 8460, 8700, 9900]);
    assert_eq!(
        picture.stages(),
        [4200, 50, 850, 800, 2500, 60, 240, 0, 1200]
    );
    assert_eq!(picture.total(), 9900);
    assert_eq!(
        Duration::from_micros(picture.total()),
        picture.last_sent - picture.presented
    );
}
#[test]
fn prepared_packets_waiting_for_the_sender_have_their_own_stage() {
    let start = Instant::now();
    let mut picture = stamps(start, [0, 100, 200, 300, 500, 1500, 1600, 22000, 23000]);
    picture.prepared = start + Duration::from_micros(2000);
    assert_eq!(
        picture.stages(),
        [100, 100, 100, 200, 1000, 100, 400, 20000, 1000]
    );
    assert_eq!(picture.total(), 23000);
}
#[test]
fn a_point_earlier_than_the_one_before_it_adds_no_time_and_keeps_the_total() {
    let start = Instant::now();
    // An encoder output stamped before its submission, and a present
    // stamp after acquisition: neither stage goes negative, the next
    // stage starts from the later point, and the stages still add up
    // to present through the last packet.
    let picture = stamps(start, [1000, 900, 1200, 2000, 3500, 3000, 3600, 3700, 4000]);
    assert_eq!(picture.stages(), [0, 200, 800, 1500, 0, 100, 100, 0, 300]);
    assert_eq!(picture.total(), 3000);
    let late = stamps(start, [5000, 0, 0, 0, 0, 0, 0, 0, 0]);
    assert_eq!(late.stages(), [0; 9]);
    assert_eq!(late.total(), 0);
}
#[test]
fn stage_snapshot_reports_mean_p95_and_p99_for_new_pictures_in_the_last_five_seconds() {
    let start = Instant::now();
    let mut p = Performance::default();
    let empty = p.snapshot(start);
    assert_eq!(empty["stages"]["frames"], 0);
    assert_eq!(empty["stages"]["present_to_sent"]["p99_ms"], 0.);
    for i in 0..100u64 {
        // Capture takes 1 ms, except 2 ms on every tenth picture and
        // 9 ms on the last one; everything after it takes 3 ms.
        let capture = match i {
            99 => 9000,
            _ if i % 10 == 0 => 2000,
            _ => 1000,
        };
        let at = start + Duration::from_millis(i * 10);
        let offsets = [
            0,
            capture,
            capture,
            capture + 500,
            capture + 1000,
            capture + 2000,
            capture + 2100,
            capture + 2500,
            capture + 3000,
        ];
        p.record_stages(&stamps(at, offsets), 2000);
    }
    let now = start + Duration::from_millis(990 + 9);
    let stages = &p.snapshot(now)["stages"];
    assert_eq!(stages["frames"], 100);
    assert_eq!(stages["window_seconds"], 5);
    assert_eq!(stages["capture"]["mean_ms"], 1.18);
    assert_eq!(stages["capture"]["p95_ms"], 2.);
    assert_eq!(stages["capture"]["p99_ms"], 2.);
    assert_eq!(stages["hold"]["p99_ms"], 0.);
    assert_eq!(stages["claim"]["mean_ms"], 0.5);
    assert_eq!(stages["encode"]["p95_ms"], 1.);
    assert_eq!(stages["send"]["mean_ms"], 0.5);
    assert_eq!(stages["present_to_sent"]["mean_ms"], 4.18);
    assert_eq!(stages["present_to_sent"]["p95_ms"], 5.);
    assert_eq!(stages["moonlight_host"]["mean_ms"], 2.);
    // The capture's age at the claim and at the encoder's submit, from its acquire.
    assert_eq!(stages["acquire_to_claim"]["p50_ms"], 0.5);
    assert_eq!(stages["acquire_to_submit"]["p50_ms"], 1.);
    assert_eq!(stages["acquire_to_submit"]["p99_ms"], 1.);
    assert_eq!(stages["capture"]["p50_ms"], 1.);
    // A picture is counted for five seconds after its last packet.
    let later = start + Duration::from_millis(990 + 9) + Duration::from_millis(4500);
    assert_eq!(p.snapshot(later)["stages"]["frames"], 50);
    assert_eq!(
        p.snapshot(start + Duration::from_secs(20))["stages"]["frames"],
        0
    );
}
#[test]
fn claims_match_encoded_frames_by_presentation_and_drop_frames_never_returned() {
    let start = Instant::now();
    let claim = |ms: u64| {
        let at = start + Duration::from_millis(ms);
        Claim {
            key: at,
            presented: at,
            acquired: at,
            published: at,
            claimed: at,
            submitted: at,
        }
    };
    let mut claims = Claims::default();
    for ms in [1, 2, 3, 4] {
        claims.push(claim(ms));
    }
    // A repeat's frame, or a frame without a presentation, has no claim
    // and leaves the others waiting.
    assert_eq!(claims.take(Some(start)), None);
    assert_eq!(claims.take(None), None);
    assert_eq!(claims.take(Some(claim(2).key)), Some(claim(2)));
    // The encoder never returned picture 1.
    assert_eq!(claims.take(Some(claim(1).key)), None);
    assert_eq!(claims.take(Some(claim(4).key)), Some(claim(4)));
    assert_eq!(claims.take(Some(claim(3).key)), None);
    for ms in 0..40 {
        claims.push(claim(ms));
    }
    assert_eq!(claims.0.len(), Claims::LIMIT);
    assert_eq!(claims.take(Some(claim(23).key)), None);
    assert_eq!(claims.take(Some(claim(24).key)), Some(claim(24)));
    let sent = claim(24).sent(
        start + Duration::from_millis(30),
        start + Duration::from_millis(31),
        start + Duration::from_millis(32),
        start + Duration::from_millis(33),
    );
    assert_eq!(sent.claimed, claim(24).claimed);
    assert_eq!(sent.total(), 9000);
}
#[test]
fn stage_samples_are_bounded_and_saturate_instead_of_wrapping() {
    let start = Instant::now();
    let mut p = Performance::default();
    for i in 0..3000u64 {
        p.record_stages(&stamps(start + Duration::from_micros(i), [0; 9]), u64::MAX);
    }
    assert_eq!(p.stages.len(), STAGE_SAMPLES);
    assert_eq!(p.stages.back().unwrap().micros[STAGES.len() + 1], u32::MAX);
    // A picture that took over an hour from present to send.
    let mut stuck = [5_000_000_000; 9];
    stuck[0] = 0;
    p.record_stages(&stamps(start, stuck), 0);
    let sample = p.stages.back().unwrap();
    assert_eq!(sample.micros[0], u32::MAX);
    assert_eq!(sample.micros[STAGES.len()], u32::MAX);
    // Its last packet is long after every other sample's.
    assert_eq!(p.stages.len(), 1);
}
#[test]
fn recent_rates_detect_stalls_and_history_is_bounded() {
    let start = Instant::now();
    let mut p = Performance::default();
    for i in 0..15000 {
        p.record(
            start + Duration::from_millis(i * 10),
            if i % 20 == 0 { 8000 } else { 2000 },
            10000,
        );
    }
    let now = start + Duration::from_millis(149990);
    let s = p.snapshot(now);
    assert!((s["fps"].as_f64().unwrap() - 100.).abs() < 0.1);
    assert_eq!(s["encode_p95_ms"], 2.);
    assert_eq!(s["history"].as_array().unwrap().len(), 120);
    assert!(p.frames.len() <= 1024);
    assert_eq!(p.snapshot(now + Duration::from_secs(3))["fps"], 0.);
}
#[test]
fn a_window_is_a_copy_that_frames_recorded_after_it_do_not_change() {
    let start = Instant::now();
    let mut p = Performance::default();
    for i in 0..300 {
        p.record(start + Duration::from_millis(i * 10), 2000, 10000);
    }
    let now = start + Duration::from_millis(2990);
    let window = p.window(now);
    let expected = p.snapshot(now);
    for i in 300..400 {
        p.record(start + Duration::from_millis(i * 10), 9000, 10000);
    }
    assert_eq!(window.snapshot(), expected);
    assert_eq!(expected["history"].as_array().unwrap().len(), 2);
    assert_eq!(expected["history"][0]["encode_mean_ms"], 2.);
}
