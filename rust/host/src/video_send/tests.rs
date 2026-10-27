use super::*;
use std::net::UdpSocket;
use std::sync::mpsc::{self, Receiver, Sender as Channel};

const WAIT: Duration = Duration::from_secs(5);

fn peer() -> SocketAddr {
    "127.0.0.1:12345".parse().unwrap()
}
fn frame(index: u8) -> Encoded {
    Encoded {
        bytes: vec![index; if index == 1 { 300_000 } else { 3000 }],
        unit: butterpollo_windows::encoder::Unit::Whole,
        idr: index == 1,
        after_invalidation: index == 3,
        frame_cap_bytes: None,
        latency: None,
        presentation: None,
    }
}

/// A manual CPU-only measurement of the production queue and worker handoff.
/// Run with `cargo test --release -p butterpollo --bin butterpollo
/// sender_handoff_benchmark -- --ignored --nocapture --test-threads=1`.
/// This deliberately uses `spawn`, not `new`: no capture priority, timer,
/// socket, display, installed host or streaming scope is created. It measures
/// encrypted packetization plus handoff, not network or end-to-end latency.
#[test]
#[ignore = "manual release-mode CPU-only sender handoff benchmark"]
fn sender_handoff_benchmark() -> Result<()> {
    anyhow::ensure!(
        !cfg!(debug_assertions),
        "run sender_handoff_benchmark with --release"
    );
    const WARMUP: usize = 32;
    const SECONDS: usize = 3;
    const BITRATE: usize = 50_000_000;

    struct Sample {
        index: usize,
        started: Instant,
        work_started: Instant,
        completed: Instant,
        packets: usize,
        bytes: usize,
        digest: u64,
    }
    fn work(packetizer: &mut VideoPacketizer, encoded: Encoded) -> Result<Sample> {
        let work_started = Instant::now();
        let started = encoded.presentation.unwrap();
        let index = u64::from_le_bytes(encoded.bytes[..8].try_into().unwrap()) as usize;
        anyhow::ensure!(packetizer.frame == index as u32 + 1, "frame order changed");
        let (packets, bytes, digest) = {
            let packets = packetizer.encode_recovery(
                &encoded.bytes,
                encoded.idr,
                encoded.after_invalidation,
                index as u32 * 1000,
                0,
            )?;
            let bytes = packets.iter().map(Vec::len).sum();
            // Consume the complete result and retain a reproducible digest,
            // including headers, encrypted payload and parity. Identical
            // inline/threaded runs must produce exactly the same packets.
            let digest = packets.iter().flatten().fold(0u64, |sum, byte| {
                sum.rotate_left(5).wrapping_add(u64::from(*byte))
            });
            (packets.len(), bytes, std::hint::black_box(digest))
        };
        Ok(Sample {
            index,
            started,
            work_started,
            completed: Instant::now(),
            packets,
            bytes,
            digest,
        })
    }
    fn packetizer() -> VideoPacketizer {
        VideoPacketizer {
            sequence: 0,
            iv_counter: 0,
            frame: 1,
            packet_size: 1392,
            fec_percent: 20,
            min_fec: 2,
            key: Some([17; 16]),
        }
    }
    fn distribution(values: &[f64]) -> serde_json::Value {
        let mut sorted = values.to_vec();
        sorted.sort_by(f64::total_cmp);
        let percentile = |percent: usize| sorted[(sorted.len() * percent).div_ceil(100) - 1];
        serde_json::json!({
            "mean": values.iter().sum::<f64>() / values.len() as f64,
            "p50": percentile(50),
            "p95": percentile(95),
            "p99": percentile(99),
            "max": sorted[sorted.len() - 1],
        })
    }
    fn run(fps: usize, threaded: bool, round: usize) -> Result<serde_json::Value> {
        let period = Duration::from_secs_f64(1. / fps as f64);
        let (finished, results) = mpsc::channel();
        let sender = if threaded {
            Some(Sender::spawn("handoff-benchmark".into(), 1, move |slot| {
                let mut packetizer = packetizer();
                slot.run(|frame| {
                    anyhow::ensure!(frame.skipped == 0, "benchmark unexpectedly skipped a frame");
                    let sample = work(&mut packetizer, frame.encoded)?;
                    finished.send(sample)?;
                    Ok(packetizer.frame)
                })
            })?)
        } else {
            None
        };
        let mut inline_packetizer = packetizer();
        let mut due = Instant::now();
        let mut measured = Vec::with_capacity(fps * SECONDS);
        let mut late_frames = 0usize;
        let mut total_bytes = 0usize;
        let mut total_packets = 0usize;
        let mut digest = 0u64;
        for index in 0..WARMUP + fps * SECONDS {
            let mut encoded = Encoded {
                bytes: vec![index as u8; (BITRATE / 8 / fps).max(8)],
                unit: butterpollo_windows::encoder::Unit::Whole,
                idr: index == 0,
                after_invalidation: false,
                frame_cap_bytes: None,
                latency: None,
                presentation: None,
            };
            encoded.bytes[..8].copy_from_slice(&(index as u64).to_le_bytes());
            if let Some(wait) = due.checked_duration_since(Instant::now()) {
                thread::sleep(wait);
            }
            let started = Instant::now();
            if started.saturating_duration_since(due) >= period {
                if index >= WARMUP {
                    late_frames += 1;
                }
                due = started;
            }
            due += period;
            encoded.presentation = Some(started);
            // Both paths own the same single-output Vec as the real caller.
            let mut output = vec![encoded];
            let sample = if let Some(sender) = &sender {
                sender.submit(output, peer(), Duration::ZERO, &mut Claims::default())?;
                results.recv_timeout(WAIT)?
            } else {
                work(&mut inline_packetizer, output.pop().unwrap())?
            };
            anyhow::ensure!(sample.index == index, "completion order changed");
            total_packets += sample.packets;
            total_bytes += sample.bytes;
            digest = digest.rotate_left(7).wrapping_add(sample.digest);
            if index >= WARMUP {
                measured.push(sample);
            }
        }
        if let Some(sender) = &sender {
            anyhow::ensure!(
                sender.next_wire_frame()? == (WARMUP + fps * SECONDS + 1) as u64,
                "worker did not finish every frame"
            );
        }
        let micros = |duration: Duration| duration.as_secs_f64() * 1_000_000.;
        let completed: Vec<_> = measured
            .iter()
            .map(|s| micros(s.completed.duration_since(s.started)))
            .collect();
        let waiting: Vec<_> = measured
            .iter()
            .map(|s| micros(s.work_started.duration_since(s.started)))
            .collect();
        let working: Vec<_> = measured
            .iter()
            .map(|s| micros(s.completed.duration_since(s.work_started)))
            .collect();
        let elapsed = measured.last().unwrap().started - measured[0].started;
        Ok(serde_json::json!({
            "mode": if threaded { "sender" } else { "inline" },
            "round": round,
            "fps_requested": fps,
            "fps_observed": (measured.len() - 1) as f64 / elapsed.as_secs_f64(),
            "warmup_frames": WARMUP,
            "sample_frames": measured.len(),
            "late_by_full_period": late_frames,
            "packets_including_warmup": total_packets,
            "bytes_including_warmup": total_bytes,
            "digest_including_warmup": digest,
            "submit_to_complete_us": distribution(&completed),
            "submit_to_work_us": distribution(&waiting),
            "work_us": distribution(&working),
            "raw_submit_to_complete_us": completed,
            "raw_submit_to_work_us": waiting,
            "raw_work_us": working,
        }))
    }

    let mut runs = Vec::new();
    for fps in [60, 120, 240] {
        let first = runs.len();
        // Alternate ordering to reduce a systematic warm-cache/clock bias.
        for (round, order) in [[false, true], [true, false]].into_iter().enumerate() {
            for threaded in order {
                runs.push(run(fps, threaded, round + 1)?);
            }
        }
        for run in &runs[first + 1..] {
            for field in [
                "packets_including_warmup",
                "bytes_including_warmup",
                "digest_including_warmup",
            ] {
                anyhow::ensure!(
                    run[field] == runs[first][field],
                    "{field} changed at {fps} FPS"
                );
            }
        }
    }
    println!(
        "SENDER_HANDOFF_BENCHMARK={}",
        serde_json::json!({
            "version": 1,
            "release": !cfg!(debug_assertions),
            "workload": "50 Mbps payload, 1392-byte packets, 20% FEC, encrypted; full packet digest",
            "scope": "Sender::spawn/submit/Slot::run at normal thread priority; CPU only; completion timestamp precedes result notification",
            "runs": runs,
        })
    );
    Ok(())
}

fn blocked_sender() -> (Sender, Receiver<Frame>, Channel<()>) {
    let (entered, frames) = mpsc::channel();
    let (release, resume) = mpsc::channel();
    let sender = Sender::spawn("test".into(), 1, move |slot| {
        slot.run(|frame| {
            let next = u32::from(frame.encoded.bytes[0]) + 1;
            entered.send(frame).unwrap();
            resume.recv_timeout(WAIT)?;
            Ok(next)
        })
    })
    .unwrap();
    (sender, frames, release)
}

#[test]
fn each_queued_frame_carries_its_pictures_claim_and_repeats_carry_none() {
    let (sender, frames, release) = blocked_sender();
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
    claims.push(claim(1));
    claims.push(claim(2));
    let mut first = frame(2);
    first.presentation = Some(claim(1).key);
    // A static repeat: the encoder returns a presentation nobody claimed.
    let mut repeat = frame(3);
    repeat.presentation = Some(start + Duration::from_millis(9));
    let mut second = frame(4);
    second.presentation = Some(claim(2).key);
    thread::scope(|scope| {
        let sender = &sender;
        let producer = scope.spawn(move || {
            sender
                .submit(
                    vec![first, repeat, second],
                    peer(),
                    Duration::ZERO,
                    &mut claims,
                )
                .unwrap();
            claims
        });
        let mut seen = vec![];
        for _ in 0..3 {
            seen.push(frames.recv_timeout(WAIT).unwrap().claim);
            release.send(()).unwrap();
        }
        assert_eq!(seen, [Some(claim(1)), None, Some(claim(2))]);
        let mut claims = producer.join().unwrap();
        assert_eq!(claims.take(Some(claim(2).key)), None);
    });
}

#[test]
fn a_keyframe_waiting_for_the_sender_also_blocks_the_next_claim() {
    let now = Instant::now();
    let state = State {
        pending: Some(Frame {
            encoded: frame(1),
            peer: peer(),
            claimed: now,
            polled: now,
            latency: Duration::ZERO,
            processing_started: now,
            skipped: 0,
            claim: None,
        }),
        sending: false,
        sending_keyframe: false,
        ready: None,
        transmitting: false,
        transmitting_keyframe: false,
        transmitting_remaining: None,
        recycled: Vec::new(),
        streaming: false,
        next_wire_frame: 1,
        skipped: 0,
        error: None,
    };
    assert_eq!(state.backlog().unwrap(), 1);
    assert_eq!(state.claim_backlog(None).unwrap(), 2);
}

#[test]
fn capacity_wait_wakes_after_the_sender_releases_a_frame() {
    let (sender, frames, release) = blocked_sender();
    sender
        .submit(
            vec![frame(1)],
            peer(),
            Duration::ZERO,
            &mut Claims::default(),
        )
        .unwrap();
    frames.recv_timeout(WAIT).unwrap();
    assert_eq!(sender.backlog().unwrap(), 1);
    assert_eq!(sender.claim_backlog(None).unwrap(), 2);
    thread::scope(|scope| {
        let (entered, waiting) = mpsc::channel();
        let (finished, done) = mpsc::channel();
        let sender = &sender;
        scope.spawn(move || {
            entered.send(()).unwrap();
            sender.wait_for_capacity(2, WAIT, None).unwrap();
            finished.send(()).unwrap();
        });
        waiting.recv_timeout(WAIT).unwrap();
        assert!(done.recv_timeout(Duration::from_millis(20)).is_err());
        release.send(()).unwrap();
        done.recv_timeout(WAIT).unwrap();
        sender
            .submit(
                vec![frame(2)],
                peer(),
                Duration::ZERO,
                &mut Claims::default(),
            )
            .unwrap();
        frames.recv_timeout(WAIT).unwrap();
        assert_eq!(sender.claim_backlog(None).unwrap(), 1);
        release.send(()).unwrap();
    });
}

#[test]
fn recovery_progress_opens_one_slot_without_changing_order_or_skipping_a_frame() {
    use butterpollo_core::encoder_backlog::{Backpressure, backpressure};
    let (sender, frames, release) = blocked_sender();
    sender
        .submit(
            vec![frame(1)],
            peer(),
            Duration::ZERO,
            &mut Claims::default(),
        )
        .unwrap();
    frames.recv_timeout(WAIT).unwrap();
    // Model the packetizer handing its keyframe to the transmitter. The
    // blocked test worker keeps ownership stable while admission is checked.
    {
        let mut state = sender.slot.state.lock().unwrap();
        state.sending = false;
        state.sending_keyframe = false;
        state.transmitting = true;
        state.transmitting_keyframe = true;
    }
    let lead = Some(Duration::from_millis(4));
    assert_eq!(sender.claim_backlog(lead).unwrap(), 2);
    sender
        .slot
        .recovery_progress(Some(Duration::from_millis(5)));
    assert_eq!(sender.claim_backlog(lead).unwrap(), 2);
    sender
        .slot
        .recovery_progress(Some(Duration::from_millis(4)));
    assert_eq!(sender.claim_backlog(lead).unwrap(), 1);
    assert_eq!(sender.claim_backlog(None).unwrap(), 2);
    assert_eq!(
        backpressure(0, sender.claim_backlog(lead).unwrap(), 1),
        Backpressure::None
    );
    assert_eq!(
        backpressure(1, sender.claim_backlog(lead).unwrap(), 2),
        Backpressure::Sender
    );
    // Progress never spends a wire number, packet sequence or encryption IV.
    {
        let state = sender.slot.state.lock().unwrap();
        assert_eq!(state.next_wire_frame, 1);
        assert_eq!(state.skipped, 0);
    }
    // A socket stall invalidates the estimate and restores the full barrier.
    sender.slot.recovery_progress(None);
    assert_eq!(sender.claim_backlog(lead).unwrap(), 2);
    sender
        .slot
        .recovery_progress(Some(Duration::from_millis(3)));
    // A newly queued recovery remains blocked, even near the old one's end.
    {
        let mut state = sender.slot.state.lock().unwrap();
        state.sending_keyframe = true;
    }
    assert_eq!(sender.claim_backlog(lead).unwrap(), 2);
    release.send(()).unwrap();
}

#[test]
fn recovery_progress_wakes_capacity_wait_before_the_keyframe_finishes() {
    let (sender, frames, release) = blocked_sender();
    sender
        .submit(
            vec![frame(1)],
            peer(),
            Duration::ZERO,
            &mut Claims::default(),
        )
        .unwrap();
    frames.recv_timeout(WAIT).unwrap();
    {
        let mut state = sender.slot.state.lock().unwrap();
        state.sending = false;
        state.sending_keyframe = false;
        state.transmitting = true;
        state.transmitting_keyframe = true;
    }
    thread::scope(|scope| {
        let (finished, done) = mpsc::channel();
        let sender = &sender;
        scope.spawn(move || {
            sender
                .wait_for_capacity(2, WAIT, Some(Duration::from_millis(4)))
                .unwrap();
            finished.send(()).unwrap();
        });
        assert!(done.recv_timeout(Duration::from_millis(20)).is_err());
        sender
            .slot
            .recovery_progress(Some(Duration::from_millis(4)));
        done.recv_timeout(WAIT).unwrap();
        assert_eq!(sender.backlog().unwrap(), 1);
        release.send(()).unwrap();
    });
}

#[test]
fn ordered_output_waits_for_the_single_pending_slot_without_replacing_frames() {
    let (sender, frames, release) = blocked_sender();
    sender
        .submit(
            vec![frame(1)],
            peer(),
            Duration::ZERO,
            &mut Claims::default(),
        )
        .unwrap();
    assert!(frames.recv_timeout(WAIT).unwrap().encoded.idr);
    assert_eq!(sender.backlog().unwrap(), 1);
    thread::scope(|scope| {
        let (submitted, done) = mpsc::channel();
        let sender = &sender;
        let producer = scope.spawn(move || {
            let mut third = frame(3);
            third.latency = Some(Duration::from_millis(3));
            sender
                .submit(
                    vec![frame(2), third],
                    peer(),
                    Duration::from_millis(7),
                    &mut Claims::default(),
                )
                .unwrap();
            submitted.send(()).unwrap();
        });
        // Wait for the producer to fill the slot, not for thread scheduling.
        {
            let state = sender.slot.state.lock().unwrap();
            let (state, timeout) = sender
                .slot
                .changed
                .wait_timeout_while(state, WAIT, |s| s.pending.is_none())
                .unwrap();
            assert!(!timeout.timed_out());
            assert_eq!(state.pending.as_ref().unwrap().encoded.bytes[0], 2);
            assert_eq!(state.backlog().unwrap(), 2);
        }
        assert!(matches!(done.try_recv(), Err(mpsc::TryRecvError::Empty)));
        let released = Instant::now();
        release.send(()).unwrap();
        let second = frames.recv_timeout(WAIT).unwrap();
        done.recv_timeout(WAIT).unwrap();
        assert_eq!(sender.backlog().unwrap(), 2);
        release.send(()).unwrap();
        let third = frames.recv_timeout(WAIT).unwrap();
        assert_eq!(second.encoded.bytes[0], 2);
        assert_eq!(third.encoded.bytes[0], 3);
        assert!(third.encoded.after_invalidation);
        assert_eq!(second.peer, peer());
        assert_eq!(second.polled, third.polled);
        assert!(third.polled <= released);
        assert_eq!(second.polled - second.claimed, Duration::from_millis(7));
        assert_eq!(third.polled - third.claimed, Duration::from_millis(3));
        release.send(()).unwrap();
        producer.join().unwrap();
    });
    assert_eq!(sender.next_wire_frame().unwrap(), 4);
    assert_eq!(sender.backlog().unwrap(), 0);
}

#[test]
fn recreation_waits_for_sending_and_pending_wire_indices() {
    let (sender, frames, release) = blocked_sender();
    sender
        .submit(
            vec![frame(1)],
            peer(),
            Duration::ZERO,
            &mut Claims::default(),
        )
        .unwrap();
    frames.recv_timeout(WAIT).unwrap();
    sender
        .submit(
            vec![frame(2)],
            peer(),
            Duration::ZERO,
            &mut Claims::default(),
        )
        .unwrap();
    thread::scope(|scope| {
        let (mapped, result) = mpsc::channel();
        let sender = &sender;
        scope.spawn(move || mapped.send(sender.next_wire_frame().unwrap()).unwrap());
        assert!(matches!(result.try_recv(), Err(mpsc::TryRecvError::Empty)));
        release.send(()).unwrap();
        frames.recv_timeout(WAIT).unwrap();
        assert!(matches!(result.try_recv(), Err(mpsc::TryRecvError::Empty)));
        release.send(()).unwrap();
        assert_eq!(result.recv_timeout(WAIT).unwrap(), 3);
    });
}

#[test]
fn a_keyframe_request_skips_the_waiting_frame_and_keeps_its_wire_number() {
    let (entered, frames) = mpsc::channel();
    let (release, resume) = mpsc::channel::<()>();
    let sender = Sender::spawn("skip".into(), 1, move |slot| {
        let mut wire = 1;
        slot.run(|frame| {
            wire += frame.skipped;
            entered
                .send((frame.encoded.bytes[0], wire, frame.skipped))
                .unwrap();
            wire += 1;
            resume.recv_timeout(WAIT)?;
            Ok(wire)
        })
    })
    .unwrap();
    sender
        .submit(
            vec![frame(1)],
            peer(),
            Duration::ZERO,
            &mut Claims::default(),
        )
        .unwrap();
    assert_eq!(frames.recv_timeout(WAIT).unwrap(), (1, 1, 0));
    // A request while a frame waits: it is skipped, the one sending is not.
    sender
        .submit(
            vec![frame(2)],
            peer(),
            Duration::ZERO,
            &mut Claims::default(),
        )
        .unwrap();
    assert_eq!(sender.backlog().unwrap(), 2);
    assert_eq!(sender.skip_pending().unwrap(), 1);
    assert_eq!(sender.skip_pending().unwrap(), 1);
    // A keyframe takes the place of a frame waiting before it.
    sender
        .submit(
            vec![frame(3)],
            peer(),
            Duration::ZERO,
            &mut Claims::default(),
        )
        .unwrap();
    let mut keyframe = frame(4);
    keyframe.idr = true;
    thread::scope(|scope| {
        let (submitted, done) = mpsc::channel();
        let sender = &sender;
        scope.spawn(move || {
            sender
                .submit(
                    vec![keyframe],
                    peer(),
                    Duration::ZERO,
                    &mut Claims::default(),
                )
                .unwrap();
            submitted.send(()).unwrap();
        });
        done.recv_timeout(WAIT).unwrap();
    });
    assert_eq!(sender.backlog().unwrap(), 2);
    // A waiting keyframe is never skipped.
    assert_eq!(sender.skip_pending().unwrap(), 2);
    release.send(()).unwrap();
    assert_eq!(frames.recv_timeout(WAIT).unwrap(), (4, 4, 2));
    release.send(()).unwrap();
    assert_eq!(sender.next_wire_frame().unwrap(), 5);
    // A skipped frame with nothing after it still counts for a new encoder.
    sender
        .submit(
            vec![frame(5)],
            peer(),
            Duration::ZERO,
            &mut Claims::default(),
        )
        .unwrap();
    assert_eq!(frames.recv_timeout(WAIT).unwrap(), (5, 5, 0));
    sender
        .submit(
            vec![frame(6)],
            peer(),
            Duration::ZERO,
            &mut Claims::default(),
        )
        .unwrap();
    assert_eq!(sender.skip_pending().unwrap(), 1);
    release.send(()).unwrap();
    assert_eq!(sender.next_wire_frame().unwrap(), 7);
}

#[test]
fn sender_failure_wakes_submit_and_recreation_and_stays_in_the_error_slot() {
    let (entered, sending) = mpsc::channel();
    let (fail, failure) = mpsc::channel();
    let sender = Sender::spawn("error".into(), 1, move |slot| {
        slot.run(|_| {
            entered.send(()).unwrap();
            failure.recv_timeout(WAIT)?;
            bail!("injected socket failure");
        })
    })
    .unwrap();
    sender
        .submit(
            vec![frame(1)],
            peer(),
            Duration::ZERO,
            &mut Claims::default(),
        )
        .unwrap();
    sending.recv_timeout(WAIT).unwrap();
    thread::scope(|scope| {
        let producer = scope.spawn(|| {
            sender.submit(
                vec![frame(2), frame(3)],
                peer(),
                Duration::ZERO,
                &mut Claims::default(),
            )
        });
        let recreation = scope.spawn(|| sender.next_wire_frame());
        let state = sender.slot.state.lock().unwrap();
        let (state, timeout) = sender
            .slot
            .changed
            .wait_timeout_while(state, WAIT, |s| s.pending.is_none())
            .unwrap();
        assert!(!timeout.timed_out());
        assert_eq!(state.backlog().unwrap(), 2);
        drop(state);
        fail.send(()).unwrap();
        for error in [
            producer.join().unwrap().unwrap_err(),
            recreation.join().unwrap().unwrap_err(),
            sender.backlog().unwrap_err(),
            sender
                .submit(vec![], peer(), Duration::ZERO, &mut Claims::default())
                .unwrap_err(),
        ] {
            assert!(error.to_string().contains("injected socket failure"));
        }
    });
    assert!(matches!(
        sending.try_recv(),
        Err(mpsc::TryRecvError::Disconnected)
    ));
}

#[test]
fn initialization_failure_reaches_the_error_slot_and_reconnect_starts_clean() {
    let mut sender =
        Sender::spawn("init".into(), 1, |_| bail!("timer initialization failed")).unwrap();
    sender.worker.take().unwrap().join().unwrap();
    assert!(
        sender
            .backlog()
            .unwrap_err()
            .to_string()
            .contains("timer initialization failed")
    );
    assert!(
        sender
            .submit(
                vec![frame(1)],
                peer(),
                Duration::ZERO,
                &mut Claims::default()
            )
            .is_err()
    );
    drop(sender);
    let (sender, frames, release) = blocked_sender();
    assert_eq!(sender.next_wire_frame().unwrap(), 1);
    sender
        .submit(
            vec![frame(1)],
            peer(),
            Duration::ZERO,
            &mut Claims::default(),
        )
        .unwrap();
    assert!(frames.recv_timeout(WAIT).unwrap().encoded.idr);
    release.send(()).unwrap();
    assert_eq!(sender.next_wire_frame().unwrap(), 2);
}

#[test]
fn dropping_an_idle_sender_wakes_and_joins_it() {
    let finished = Arc::new(AtomicBool::new(false));
    let done = finished.clone();
    let sender = Sender::spawn("idle".into(), 1, move |slot| {
        let result = slot.run(|_| panic!("no frame was submitted"));
        done.store(true, Ordering::Release);
        result
    })
    .unwrap();
    drop(sender);
    assert!(finished.load(Ordering::Acquire));
}

#[test]
fn a_sender_started_mid_stream_continues_the_wire_frame_numbering() {
    let sender = Sender::spawn("handoff".into(), 4242, move |slot| {
        slot.run(|_| panic!("no frame was submitted"))
    })
    .unwrap();
    assert_eq!(sender.next_wire_frame().unwrap(), 4242);
}

#[test]
fn shutdown_interrupts_sending_and_releases_pending_output() {
    let (entered, sending) = mpsc::channel();
    let sender = Sender::spawn("stop".into(), 1, move |slot| {
        slot.run(|frame| {
            entered.send(frame.encoded.bytes[0]).unwrap();
            let state = slot.state.lock().unwrap();
            let (_state, timeout) = slot
                .changed
                .wait_timeout_while(state, WAIT, |_| !slot.stop.load(Ordering::Acquire))
                .unwrap();
            assert!(!timeout.timed_out());
            Ok(2)
        })
    })
    .unwrap();
    sender
        .submit(
            vec![frame(1)],
            peer(),
            Duration::ZERO,
            &mut Claims::default(),
        )
        .unwrap();
    assert_eq!(sending.recv_timeout(WAIT).unwrap(), 1);
    sender
        .submit(
            vec![frame(2)],
            peer(),
            Duration::ZERO,
            &mut Claims::default(),
        )
        .unwrap();
    thread::scope(|scope| {
        let producer = scope.spawn(|| {
            sender.submit(
                vec![frame(3)],
                peer(),
                Duration::ZERO,
                &mut Claims::default(),
            )
        });
        sender.slot.stop();
        producer.join().unwrap().unwrap();
    });
    let slot = sender.slot.clone();
    drop(sender);
    assert!(matches!(
        sending.try_recv(),
        Err(mpsc::TryRecvError::Disconnected)
    ));
    assert_eq!(slot.state.lock().unwrap().backlog().unwrap(), 0);
}

fn packetizer(key: Option<[u8; 16]>) -> VideoPacketizer {
    VideoPacketizer {
        sequence: 0,
        iv_counter: 0,
        frame: 1,
        packet_size: 1024,
        fec_percent: 20,
        min_fec: 2,
        key,
    }
}

fn handoff(packetizer: VideoPacketizer) -> Handoff {
    Handoff {
        packetizer,
        present_stamper: None,
        last_stamp: Instant::now(),
        pacing_due: Instant::now(),
        send_loss: Default::default(),
        slice_blocks: 0,
        slices: 0,
    }
}

fn wait_prepared(sender: &Sender) -> std::sync::MutexGuard<'_, State> {
    let state = sender.slot.state.lock().unwrap();
    let (state, timeout) = sender
        .slot
        .changed
        .wait_timeout_while(state, WAIT, |s| s.ready.is_none())
        .unwrap();
    assert!(!timeout.timed_out(), "packetizer did not prepare a frame");
    state
}

#[test]
fn moonlight_timing_stops_at_packetization_before_a_blocked_sender() {
    for key in [None, Some([23; 16])] {
        let fixture = crate::state::test_support::Fixture::new();
        let session = fixture.session(fixture.client(0), butterpollo_core::session::Role::Stream);
        let socket = Arc::new(ReplySocket::bind("127.0.0.1:0").unwrap());
        let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
        receiver.set_read_timeout(Some(WAIT)).unwrap();
        let start = Instant::now();
        let tick = start - Duration::from_millis(2);
        let mut sender =
            Sender::packetizer(session.clone(), start, handoff(packetizer(key))).unwrap();
        let mut picture = frame(2);
        picture.frame_cap_bytes = Some(112_815);
        picture.latency = Some(Duration::from_millis(12));
        picture.presentation = Some(tick - Duration::from_millis(15));
        let mut claims = Claims::default();
        claims.push(Claim {
            key: picture.presentation.unwrap(),
            presented: picture.presentation.unwrap(),
            acquired: tick - Duration::from_millis(14),
            published: tick - Duration::from_millis(13),
            claimed: tick - Duration::from_millis(10),
            submitted: tick - Duration::from_millis(9),
        });
        sender
            .submit_tick(
                vec![picture],
                receiver.local_addr().unwrap(),
                Duration::ZERO,
                &mut claims,
                tick,
            )
            .unwrap();
        let processing = {
            let state = wait_prepared(&sender);
            let ready = state.ready.as_ref().unwrap();
            assert_eq!(ready.frame_cap_bytes, Some(112_815));
            assert_eq!(
                ready.processing,
                micros(ready.packetized.duration_since(tick))
            );
            assert_eq!(ready.latency, 12_000);
            ready.processing
        };
        // A controlled downstream stall: the production packetizer finishes,
        // but the production paced sender has not started yet.
        thread::sleep(Duration::from_millis(30));
        assert_eq!(
            wait_prepared(&sender).ready.as_ref().unwrap().processing,
            processing
        );
        let shared = sender.slot.clone();
        let s = session.clone();
        let config = fixture.host.config.read().unwrap().clone();
        let host = fixture.host.clone();
        sender.transport = Some(thread::spawn(move || {
            let result = transport::run(
                &shared,
                socket,
                s,
                config,
                host,
                start,
                Instant::now(),
                Default::default(),
            );
            shared.finish(result);
        }));
        assert_eq!(sender.next_wire_frame().unwrap(), 2);
        let mut packet = [0; 2048];
        let len = receiver.recv(&mut packet).unwrap();
        let packet = match key {
            None => packet[..len].to_vec(),
            Some(key) => butterpollo_core::crypto::gcm_open(
                &key,
                &packet[..12],
                &packet[16..32],
                &packet[32..len],
            )
            .unwrap(),
        };
        assert_eq!(
            u16::from_le_bytes(packet[33..35].try_into().unwrap()),
            (processing / 100).min(65535) as u16
        );
        let performance = &session.info()["performance"];
        assert_eq!(performance["encode_mean_ms"], 12.);
        assert_eq!(
            performance["host_processing_mean_ms"],
            processing as f64 / 1000.
        );
        assert!(
            performance["stages"]["send_queue"]["mean_ms"]
                .as_f64()
                .unwrap()
                >= 30.
        );
    }
}

#[test]
fn keyframe_skips_raw_and_prepared_waiters_without_spending_wire_numbers_twice() {
    let fixture = crate::state::test_support::Fixture::new();
    let session = fixture.session(fixture.client(0), butterpollo_core::session::Role::Stream);
    let sender =
        Sender::packetizer(session, Instant::now(), handoff(packetizer(Some([9; 16])))).unwrap();
    sender
        .submit(
            vec![frame(2)],
            peer(),
            Duration::ZERO,
            &mut Claims::default(),
        )
        .unwrap();
    drop(wait_prepared(&sender));
    sender
        .submit(
            vec![frame(3)],
            peer(),
            Duration::ZERO,
            &mut Claims::default(),
        )
        .unwrap();
    {
        let state = sender.slot.state.lock().unwrap();
        let (state, timeout) = sender
            .slot
            .changed
            .wait_timeout_while(state, WAIT, |s| !s.sending)
            .unwrap();
        assert!(!timeout.timed_out());
        assert!(state.ready.is_some());
    }
    // Raw frame 3 is being prepared; frame 4 waits behind it. Prepared wire
    // frame 1 and the raw waiter are discarded, but only the raw waiter still
    // needs a wire number spent. The in-progress frame becomes wire frame 2.
    sender
        .submit(
            vec![frame(4)],
            peer(),
            Duration::ZERO,
            &mut Claims::default(),
        )
        .unwrap();
    sender.skip_pending().unwrap();
    {
        let state = wait_prepared(&sender);
        assert_eq!(state.ready.as_ref().unwrap().wire_frame, 2);
    }
    sender
        .submit(
            vec![frame(1)],
            peer(),
            Duration::ZERO,
            &mut Claims::default(),
        )
        .unwrap();
    let (idr, wire_frame, iv_counter) = {
        let state = wait_prepared(&sender);
        let ready = state.ready.as_ref().unwrap();
        (ready.idr, ready.wire_frame, ready.packetizer.iv_counter)
    };
    assert!(idr);
    assert_eq!(wire_frame, 4);
    // Two prepared 3 KB frames spent five packet IVs apiece (data + FEC).
    assert_eq!(iv_counter, 10);
}

#[test]
fn stopping_wakes_a_packetizer_blocked_on_the_prepared_slot() {
    let fixture = crate::state::test_support::Fixture::new();
    let session = fixture.session(fixture.client(0), butterpollo_core::session::Role::Stream);
    let sender = Sender::packetizer(session, Instant::now(), handoff(packetizer(None))).unwrap();
    sender
        .submit(
            vec![frame(2)],
            peer(),
            Duration::ZERO,
            &mut Claims::default(),
        )
        .unwrap();
    drop(wait_prepared(&sender));
    sender
        .submit(
            vec![frame(3)],
            peer(),
            Duration::ZERO,
            &mut Claims::default(),
        )
        .unwrap();
    let (done, finished) = mpsc::channel();
    thread::spawn(move || {
        drop(sender);
        done.send(()).unwrap();
    });
    finished.recv_timeout(WAIT).unwrap();
}

#[test]
fn a_sender_taking_over_mid_stream_continues_the_inline_numbering() {
    for key in [None, Some([17; 16])] {
        let fixture = crate::state::test_support::Fixture::new();
        let session = fixture.session(fixture.client(0), butterpollo_core::session::Role::Stream);
        let socket = Arc::new(ReplySocket::bind("127.0.0.1:0").unwrap());
        let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
        receiver.set_read_timeout(Some(WAIT)).unwrap();
        // Where the stream thread's own sends left off.
        let mut inline = packetizer(key);
        inline.frame = 300;
        inline.sequence = 7000;
        inline.iv_counter = 9000;
        let sender = Sender::new(
            socket,
            session.clone(),
            fixture.host.config.read().unwrap().clone(),
            fixture.host.clone(),
            Instant::now(),
            handoff(inline),
        )
        .unwrap();
        assert_eq!(sender.next_wire_frame().unwrap(), 300);
        sender
            .submit(
                vec![frame(1)],
                receiver.local_addr().unwrap(),
                Duration::ZERO,
                &mut Claims::default(),
            )
            .unwrap();
        assert_eq!(sender.next_wire_frame().unwrap(), 301);
        let mut packet = [0; 2048];
        let size = receiver.recv(&mut packet).unwrap();
        let packet = match key {
            None => packet[..size].to_vec(),
            Some(key) => {
                assert_eq!(u64::from_le_bytes(packet[..8].try_into().unwrap()), 9000);
                butterpollo_core::crypto::gcm_open(
                    &key,
                    &packet[..12],
                    &packet[16..32],
                    &packet[32..size],
                )
                .unwrap()
            }
        };
        assert!(size > 24);
        assert_eq!(u16::from_be_bytes([packet[2], packet[3]]), 7000);
        assert_eq!(u32::from_le_bytes(packet[20..24].try_into().unwrap()), 300);
    }
}

#[test]
fn the_sender_thread_accepts_client_reports_for_adaptive_fec() {
    let fixture = crate::state::test_support::Fixture::new();
    let session = fixture.session(fixture.client(0), butterpollo_core::session::Role::Stream);
    session.configure_fec(true, 20);
    let socket = Arc::new(ReplySocket::bind("127.0.0.1:0").unwrap());
    let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
    let sender = Sender::new(
        socket,
        session.clone(),
        fixture.host.config.read().unwrap().clone(),
        fixture.host.clone(),
        Instant::now(),
        handoff(packetizer(None)),
    )
    .unwrap();
    sender
        .submit(
            vec![frame(2)],
            receiver.local_addr().unwrap(),
            Duration::ZERO,
            &mut Claims::default(),
        )
        .unwrap();
    assert_eq!(sender.next_wire_frame().unwrap(), 2);
    assert_eq!(session.stats.video_frame.load(Ordering::Acquire), 2);
    session.record_fec_status(&[
        0, 0, 0, 1, 0, 11, 0, 0, 0, 0, 0, 10, 0, 2, 0, 10, 0, 0, 20, 0, 1,
    ]);
    assert_eq!(session.info()["performance"]["fec_invalid_reports"], 0);
    session.tick_fec(Instant::now() + Duration::from_secs(60));
    assert!(session.fec_percent() < 20);
}

#[test]
fn the_sender_thread_records_where_a_new_pictures_time_went() {
    let fixture = crate::state::test_support::Fixture::new();
    let session = fixture.session(fixture.client(0), butterpollo_core::session::Role::Stream);
    let socket = Arc::new(ReplySocket::bind("127.0.0.1:0").unwrap());
    let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
    // The media clock started 500 ms ago.
    let now = Instant::now();
    let ago = |ms: u64| now - Duration::from_millis(ms);
    let epoch = ago(500);
    let mut handoff = handoff(packetizer(None));
    handoff.last_stamp = ago(10);
    let sender = Sender::new(
        socket,
        session.clone(),
        fixture.host.config.read().unwrap().clone(),
        fixture.host.clone(),
        epoch,
        handoff,
    )
    .unwrap();
    // The host-side record is on.
    let (sink, rows) = butterpollo_core::timing_record::channel(8);
    *session.timing_record.write().unwrap() = Some(Arc::new(sink));
    // A client that asked for host timing reports (0x5536).
    let asked = butterpollo_core::host_timing::HostTimingRequest { enabled: true }.encode();
    assert_eq!(
        session.host_timings.lock().unwrap().on_request(&asked),
        Some(true)
    );
    // A picture presented 6 ms ago, held 1 ms, claimed 3 ms ago and encoded
    // in 2 ms, beside a static repeat that has no present.
    let mut claims = Claims::default();
    claims.push(Claim {
        key: ago(6),
        presented: ago(6),
        acquired: ago(5),
        published: ago(4),
        claimed: ago(3),
        submitted: ago(2),
    });
    let mut picture = frame(2);
    picture.presentation = Some(ago(6));
    picture.latency = Some(Duration::from_millis(3));
    let mut repeat = frame(3);
    repeat.presentation = Some(now);
    sender
        .submit_tick(
            vec![picture, repeat],
            receiver.local_addr().unwrap(),
            Duration::ZERO,
            &mut claims,
            now,
        )
        .unwrap();
    assert_eq!(sender.next_wire_frame().unwrap(), 3);
    let performance = &session.info()["performance"];
    let stages = &performance["stages"];
    let ms = |stage: &str| stages[stage]["mean_ms"].as_f64().unwrap();
    assert_eq!(performance["sample_frames"], 2);
    assert_eq!(stages["frames"], 1);
    assert_eq!(
        (ms("capture"), ms("hold"), ms("claim"), ms("convert")),
        (1., 1., 1., 1.)
    );
    // Encoding ran from the submission to the output reaching the sender.
    assert!(ms("encode") >= 2.);
    let parts: f64 = butterpollo_core::performance::STAGES
        .iter()
        .map(|stage| ms(stage))
        .sum();
    assert!((parts - ms("present_to_sent")).abs() < 0.01);
    assert!(ms("present_to_sent") >= 6.);
    // The tick is exactly 6 ms after presentation. Everything after entry
    // into packetization remains in the full path, outside Moonlight's field.
    let gap = ms("present_to_sent") - ms("moonlight_host");
    let excluded = 6. + ms("packetize") + ms("send_queue") + ms("send");
    assert!((gap - excluded).abs() < 0.01, "{gap} != {excluded}");
    // The client that asked hears the whole span for the new picture alone,
    // numbered as its packets are, with the same stages.
    let report = session.host_timings.lock().unwrap().take_report().unwrap();
    let records = butterpollo_core::host_timing::HostTiming::decode_report(&report).unwrap();
    assert_eq!(records.len(), 1);
    let record = records[0];
    // The extension holds the present on the media clock and the split stages, and the
    // record's row has the stamp points themselves.
    let (_, extra) = butterpollo_core::host_timing::HostTiming::decode_report_with(&report)
        .unwrap()
        .remove(0);
    let extra = extra.unwrap();
    assert_eq!(
        (extra.presented_at_us, extra.capture_us, extra.convert_us),
        (494_000, 1_000, 1_000)
    );
    let row = rows.try_recv().unwrap();
    assert!(rows.try_recv().is_err(), "the static repeat has no row");
    assert_eq!(row.wire_frame, 1);
    assert!(row.rtp_us.abs_diff(494_000) <= 12, "{}", row.rtp_us);
    let [
        present,
        acquired,
        published,
        claimed,
        submitted,
        encoded,
        packetized,
        prepared,
        first,
        last,
    ] = row.points;
    assert_eq!(
        [present, acquired, published, claimed, submitted],
        [494_000, 495_000, 496_000, 497_000, 498_000]
    );
    assert!(submitted <= encoded && encoded <= packetized && packetized <= prepared);
    assert!(prepared <= first && first <= last);
    // The report sums nine stages, each cut to whole microseconds.
    assert!(
        (last - present).abs_diff(i64::from(record.host_us)) <= 9,
        "the row and the report end at the same last packet"
    );
    assert_eq!(record.frame, 1);
    let us = |ms: f64| (ms * 1000.).round() as i64;
    assert!((i64::from(record.host_us) - us(ms("present_to_sent"))).abs() <= 1);
    assert_eq!(
        i64::from(record.queue_us),
        us(ms("capture") + ms("hold") + ms("claim") + ms("convert"))
    );
    assert!((i64::from(record.encode_us) - us(ms("encode"))).abs() <= 1);
    assert!((i64::from(record.send_us) - us(ms("send"))).abs() <= 1);
    assert_eq!(session.host_timings.lock().unwrap().take_report(), None);
}

#[test]
fn reconnect_keeps_packet_order_fec_recovery_and_iv_ownership_identical_to_inline() {
    for key in [None, Some([17; 16]), Some([29; 16])] {
        let (sent, packets) = mpsc::channel();
        let sender = Sender::spawn("reconnect".into(), 1, move |slot| {
            let mut packetizer = packetizer(key);
            slot.run(|frame| {
                let frame = frame.encoded;
                sent.send(packetizer.encode_recovery(
                    &frame.bytes,
                    frame.idr,
                    frame.after_invalidation,
                    9000,
                    1234,
                )?)
                .unwrap();
                Ok(packetizer.frame)
            })
        })
        .unwrap();
        assert_eq!(sender.next_wire_frame().unwrap(), 1);
        let mut inline = packetizer(key);
        for index in 1..=4 {
            let expected = frame(index);
            sender
                .submit(
                    vec![frame(index)],
                    peer(),
                    Duration::ZERO,
                    &mut Claims::default(),
                )
                .unwrap();
            assert_eq!(
                packets.recv_timeout(WAIT).unwrap(),
                inline
                    .encode_recovery(
                        &expected.bytes,
                        expected.idr,
                        expected.after_invalidation,
                        9000,
                        1234
                    )
                    .unwrap()
            );
        }
        assert_eq!(sender.next_wire_frame().unwrap(), u64::from(inline.frame));
        drop(sender);
        assert!(matches!(
            packets.try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ));
    }
}

/// One unit of a streamed frame.
fn unit(unit: butterpollo_windows::encoder::Unit, shards: usize, fill: u8) -> Encoded {
    Encoded {
        bytes: vec![fill; shards * 1008],
        unit,
        idr: false,
        after_invalidation: false,
        frame_cap_bytes: Some(100 * 1008),
        latency: None,
        presentation: None,
    }
}

fn slice(index: u32, shards: usize, fill: u8) -> Encoded {
    unit(
        butterpollo_windows::encoder::Unit::Slice { index },
        shards,
        fill,
    )
}

fn last(index: u32, shards: usize, fill: u8) -> Encoded {
    unit(
        butterpollo_windows::encoder::Unit::Last { index },
        shards,
        fill,
    )
}

/// A packetizer thread whose prepared slot the test drains by hand, as the
/// paced sender would.
fn streaming_sender(session: Arc<Session>, blocks: usize, slices: usize) -> Sender {
    let mut handoff = handoff(packetizer(None));
    handoff.slice_blocks = blocks;
    handoff.slices = slices;
    Sender::packetizer(session, Instant::now(), handoff).unwrap()
}

fn take_prepared(sender: &Sender) -> Prepared {
    let mut state = wait_prepared(sender);
    let prepared = state.ready.take().unwrap();
    drop(state);
    sender.slot.changed.notify_all();
    prepared
}

/// The next whole frame, after any parts of a streamed one.
fn take_whole(sender: &Sender) -> Prepared {
    loop {
        let prepared = take_prepared(sender);
        if prepared.stream.is_none() {
            return prepared;
        }
    }
}

fn submit_units(sender: &Sender, units: Vec<Encoded>) {
    sender
        .submit(units, peer(), Duration::ZERO, &mut Claims::default())
        .unwrap();
}

#[test]
fn a_streamed_frames_units_wait_in_the_pending_slot_through_a_keyframe_request() {
    let (entered, frames) = mpsc::channel();
    let (release, resume) = mpsc::channel::<()>();
    let sender = Sender::spawn("units".into(), 1, move |slot| {
        slot.run(|frame| {
            entered.send(frame.encoded.unit).unwrap();
            resume.recv_timeout(WAIT)?;
            Ok(1)
        })
    })
    .unwrap();
    submit_units(&sender, vec![slice(0, 1, 1)]);
    assert_eq!(
        frames.recv_timeout(WAIT).unwrap(),
        butterpollo_windows::encoder::Unit::Slice { index: 0 }
    );
    submit_units(&sender, vec![last(1, 1, 2)]);
    // A whole frame waiting is skipped, a unit of a frame being sent is not.
    assert_eq!(sender.backlog().unwrap(), 2);
    assert_eq!(sender.skip_pending().unwrap(), 2);
    release.send(()).unwrap();
    assert_eq!(
        frames.recv_timeout(WAIT).unwrap(),
        butterpollo_windows::encoder::Unit::Last { index: 1 }
    );
    release.send(()).unwrap();
    assert_eq!(sender.next_wire_frame().unwrap(), 1);
}

#[test]
fn only_a_streamed_frames_first_unit_takes_the_pictures_claim() {
    let (entered, frames) = mpsc::channel();
    let sender = Sender::spawn("claims".into(), 1, move |slot| {
        slot.run(|frame| {
            entered.send(frame.claim.is_some()).unwrap();
            Ok(1)
        })
    })
    .unwrap();
    let now = Instant::now();
    let mut claims = Claims::default();
    claims.push(Claim {
        key: now,
        presented: now,
        acquired: now,
        published: now,
        claimed: now,
        submitted: now,
    });
    let mut first = slice(0, 1, 1);
    first.presentation = Some(now);
    let mut second = last(1, 1, 2);
    second.presentation = Some(now);
    sender
        .submit(vec![first, second], peer(), Duration::ZERO, &mut claims)
        .unwrap();
    assert!(frames.recv_timeout(WAIT).unwrap());
    assert!(!frames.recv_timeout(WAIT).unwrap());
}

#[test]
fn the_packetizer_thread_sends_each_block_of_a_streamed_frame_as_one_part() {
    let fixture = crate::state::test_support::Fixture::new();
    let session = fixture.session(fixture.client(0), butterpollo_core::session::Role::Stream);
    let sender = streaming_sender(session, 2, 2);
    submit_units(&sender, vec![slice(0, 20, 1)]);
    // Block 0 is ready before the frame's last unit exists.
    let first = take_prepared(&sender);
    assert_eq!(
        first.stream,
        Some(Part {
            first: true,
            last: false
        })
    );
    assert_eq!(
        (first.wire_frame, first.idr, first.counts.len()),
        (1, false, 1)
    );
    assert_eq!(first.layout.len(), 1);
    assert_eq!(first.counts[0], first.layout[0].0 + first.layout[0].1);
    assert_eq!(first.blocks[0].len(), first.counts[0]);
    // The next frame's number is spent as soon as block 0 is built.
    assert_eq!(first.payload_bytes, 20 * 1008);
    submit_units(&sender, vec![last(1, 20, 2)]);
    let second = take_prepared(&sender);
    assert_eq!(
        second.stream,
        Some(Part {
            first: false,
            last: true
        })
    );
    assert_eq!(second.wire_frame, 1);
    assert_eq!(second.payload_bytes, 40 * 1008);
    assert_eq!(sender.next_wire_frame().unwrap(), 2);
    // The next frame follows, whole.
    submit_units(&sender, vec![frame(2)]);
    let next = take_prepared(&sender);
    assert_eq!((next.stream, next.wire_frame), (None, 2));
}

#[test]
fn a_prepared_part_of_a_streamed_frame_survives_a_keyframe_request() {
    let fixture = crate::state::test_support::Fixture::new();
    let session = fixture.session(fixture.client(0), butterpollo_core::session::Role::Stream);
    let sender = streaming_sender(session, 2, 2);
    submit_units(&sender, vec![slice(0, 20, 1)]);
    drop(wait_prepared(&sender));
    sender.skip_pending().unwrap();
    assert!(sender.slot.state.lock().unwrap().ready.is_some());
    assert_eq!(take_prepared(&sender).stream.map(|p| p.first), Some(true));
}

#[test]
fn a_frame_that_begins_over_an_open_streamed_frame_takes_its_number_after_it() {
    let fixture = crate::state::test_support::Fixture::new();
    let session = fixture.session(fixture.client(0), butterpollo_core::session::Role::Stream);
    let sender = streaming_sender(session, 2, 2);
    // One shard: nothing can leave before the last unit.
    submit_units(&sender, vec![slice(0, 1, 1)]);
    // The last unit never comes.
    submit_units(&sender, vec![frame(2)]);
    let next = take_prepared(&sender);
    assert_eq!((next.stream, next.wire_frame), (None, 2));
    // A frame interrupted after block 0 keeps the number it took.
    submit_units(&sender, vec![slice(0, 20, 1)]);
    let part = take_prepared(&sender);
    assert_eq!(
        (part.wire_frame, part.stream.map(|p| p.first)),
        (3, Some(true))
    );
    submit_units(&sender, vec![frame(3)]);
    let next = take_prepared(&sender);
    assert_eq!((next.stream, next.wire_frame), (None, 4));
}

#[test]
fn a_streamed_frame_the_plan_cannot_hold_is_dropped_and_turns_delivery_off() {
    let fixture = crate::state::test_support::Fixture::new();
    let session = fixture.session(fixture.client(0), butterpollo_core::session::Role::Stream);
    let sender = streaming_sender(session.clone(), 2, 2);
    // A frame that grows past what the encoder reported it could return.
    let mut first = slice(0, 2, 1);
    first.frame_cap_bytes = Some(4 * 1008);
    let mut second = last(1, 10, 2);
    second.frame_cap_bytes = Some(4 * 1008);
    submit_units(&sender, vec![first, second]);
    submit_units(&sender, vec![frame(2)]);
    let next = take_whole(&sender);
    assert_eq!(next.wire_frame, 2);
    assert_eq!(
        session
            .stats
            .slice_delivery_overflows
            .load(Ordering::Relaxed),
        1
    );
    let delivery = crate::stream::slices::Delivery::new(2);
    delivery.sync(&session.stats);
    assert!(delivery.joins());
    // A cap no plan holds fails the frame's first unit.
    let mut huge = slice(0, 1, 1);
    huge.frame_cap_bytes = Some(5000 * 1008);
    submit_units(&sender, vec![huge, last(1, 1, 2)]);
    submit_units(&sender, vec![frame(2)]);
    // The refused frame spent number 3.
    let next = take_whole(&sender);
    assert_eq!(next.wire_frame, 4);
    assert_eq!(
        session
            .stats
            .slice_delivery_overflows
            .load(Ordering::Relaxed),
        2
    );
}

/// The shards of a streamed frame's packets in wire order: its payload.
fn streamed_payload(packets: &[Vec<u8>]) -> (Vec<u8>, usize) {
    let mut data = packets
        .iter()
        .filter_map(|packet| {
            let info = u32::from_le_bytes(packet[28..32].try_into().unwrap());
            let (index, count) = ((info >> 12) & 0x3ff, info >> 22);
            let block = (packet[27] >> 4) & 3;
            (index < count).then_some(((block, index), packet))
        })
        .collect::<Vec<_>>();
    data.sort_by_key(|(key, _)| *key);
    let blocks = packets
        .iter()
        .map(|packet| usize::from(packet[27] >> 6) + 1)
        .max()
        .unwrap_or(0);
    (
        data.iter()
            .flat_map(|(_, packet)| packet[32..].iter().copied())
            .collect(),
        blocks,
    )
}

#[test]
fn a_streamed_frame_goes_out_in_order_and_is_timed_once() {
    let fixture = crate::state::test_support::Fixture::new();
    let session = fixture.session(fixture.client(0), butterpollo_core::session::Role::Stream);
    let socket = Arc::new(ReplySocket::bind("127.0.0.1:0").unwrap());
    let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
    receiver
        .set_read_timeout(Some(Duration::from_millis(500)))
        .unwrap();
    let now = Instant::now();
    let epoch = now - Duration::from_millis(500);
    let mut handoff = handoff(packetizer(None));
    handoff.slice_blocks = 2;
    handoff.slices = 2;
    let sender = Sender::new(
        socket,
        session.clone(),
        fixture.host.config.read().unwrap().clone(),
        fixture.host.clone(),
        epoch,
        handoff,
    )
    .unwrap();
    let (sink, rows) = butterpollo_core::timing_record::channel(8);
    *session.timing_record.write().unwrap() = Some(Arc::new(sink));
    let mut claims = Claims::default();
    let key = now - Duration::from_millis(6);
    claims.push(Claim {
        key,
        presented: key,
        acquired: key,
        published: key,
        claimed: key,
        submitted: key,
    });
    let mut first = slice(0, 20, 1);
    first.presentation = Some(key);
    let mut second = last(1, 20, 2);
    second.presentation = Some(key);
    let addr = receiver.local_addr().unwrap();
    sender
        .submit_tick(vec![first], addr, Duration::ZERO, &mut claims, now)
        .unwrap();
    // The first block reaches the client before the encoder produced the rest.
    let mut packets = Vec::new();
    let mut buffer = [0; 2048];
    // The packetizer thread sends it: on a busy PC that took over 500 ms.
    receiver.set_read_timeout(Some(WAIT)).unwrap();
    let early = receiver.recv(&mut buffer).unwrap();
    packets.push(buffer[..early].to_vec());
    receiver
        .set_read_timeout(Some(Duration::from_millis(500)))
        .unwrap();
    while let Ok(len) = receiver.recv(&mut buffer) {
        packets.push(buffer[..len].to_vec());
    }
    let early_packets = packets.len();
    assert!(early_packets > 20, "{early_packets}");
    assert!(rows.try_recv().is_err(), "the frame is not over yet");
    sender
        .submit_tick(vec![second], addr, Duration::ZERO, &mut claims, now)
        .unwrap();
    assert_eq!(sender.next_wire_frame().unwrap(), 2);
    while let Ok(len) = receiver.recv(&mut buffer) {
        packets.push(buffer[..len].to_vec());
    }
    assert!(packets.len() > early_packets);
    assert!(
        packets
            .iter()
            .all(|packet| packet[20..24] == 1u32.to_le_bytes())
    );
    // On the wire in order: blocks never go back, and the RTP sequence and the
    // stream packet index step by one per packet across the parts.
    let block = |packet: &Vec<u8>| (packet[27] >> 4) & 3;
    assert_eq!(block(&packets[0]), 0);
    assert_eq!(block(packets.last().unwrap()), 1);
    for pair in packets.windows(2) {
        assert!(block(&pair[0]) <= block(&pair[1]));
        let sequence = |p: &Vec<u8>| u16::from_be_bytes(p[2..4].try_into().unwrap());
        assert_eq!(sequence(&pair[1]), sequence(&pair[0]).wrapping_add(1));
    }
    // A data shard carries its packet index (the FEC covers it, so parity
    // shards do not): it is the same count.
    for packet in &packets {
        let info = u32::from_le_bytes(packet[28..32].try_into().unwrap());
        if (info >> 12) & 0x3ff < info >> 22 {
            let index = u32::from_le_bytes(packet[16..20].try_into().unwrap()) >> 8;
            assert_eq!(
                index,
                u32::from(u16::from_be_bytes(packet[2..4].try_into().unwrap()))
            );
        }
    }
    let (stream, blocks) = streamed_payload(&packets);
    assert_eq!(blocks, 2);
    let mut expected = vec![0; 8];
    expected.extend(std::iter::repeat_n(1u8, 20 * 1008));
    expected.extend(std::iter::repeat_n(2u8, 20 * 1008));
    assert_eq!(stream[8..8 + 40 * 1008], expected[8..]);
    // One record, one set of stage timings, for the whole frame.
    let row = rows.try_recv().unwrap();
    assert!(rows.try_recv().is_err());
    assert_eq!(row.wire_frame, 1);
    let points = row.points;
    // Packetizing began on the first unit and the first block went out; the last
    // unit was in hand only after that, and the record says how many blocks left
    // before it.
    assert!(points[6] <= points[7] && points[7] <= points[8]);
    assert!(
        points[8] < points[5],
        "the first block left before the last unit"
    );
    assert!(points[5] <= points[9]);
    let first_unit = row.first_unit_us.unwrap();
    assert!(first_unit <= points[6] && first_unit < points[5]);
    assert_eq!(row.early_blocks, Some(1));
    assert_eq!(session.info()["performance"]["stages"]["frames"], 1);
}

/// A streamed frame's first block, then a whole frame: the open frame is
/// abandoned on the packetizer thread, and the sender must account for the
/// second frame alone.
#[test]
fn a_frame_cut_off_after_its_first_block_leaves_no_stats_or_timing_behind() {
    let fixture = crate::state::test_support::Fixture::new();
    let session = fixture.session(fixture.client(0), butterpollo_core::session::Role::Stream);
    let socket = Arc::new(ReplySocket::bind("127.0.0.1:0").unwrap());
    let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
    // Block 0 comes from the packetizer thread: on a busy PC after 500 ms.
    receiver.set_read_timeout(Some(WAIT)).unwrap();
    let now = Instant::now();
    let mut handoff = handoff(packetizer(None));
    handoff.slice_blocks = 2;
    handoff.slices = 2;
    let sender = Sender::new(
        socket,
        session.clone(),
        fixture.host.config.read().unwrap().clone(),
        fixture.host.clone(),
        now - Duration::from_millis(500),
        handoff,
    )
    .unwrap();
    let (sink, rows) = butterpollo_core::timing_record::channel(8);
    *session.timing_record.write().unwrap() = Some(Arc::new(sink));
    let mut claims = Claims::default();
    for ms in [8, 4] {
        let key = now - Duration::from_millis(ms);
        claims.push(Claim {
            key,
            presented: key,
            acquired: key,
            published: key,
            claimed: key,
            submitted: key,
        });
    }
    let addr = receiver.local_addr().unwrap();
    let mut cut = slice(0, 20, 1);
    cut.presentation = Some(now - Duration::from_millis(8));
    sender
        .submit_tick(vec![cut], addr, Duration::ZERO, &mut claims, now)
        .unwrap();
    // Block 0 is on the wire.
    let mut buffer = [0; 2048];
    receiver.recv(&mut buffer).unwrap();
    let mut whole = frame(2);
    whole.presentation = Some(now - Duration::from_millis(4));
    sender
        .submit_tick(vec![whole], addr, Duration::ZERO, &mut claims, now)
        .unwrap();
    assert_eq!(sender.next_wire_frame().unwrap(), 3);
    // One frame was accounted: the whole one, which is number 2.
    let row = rows.try_recv().unwrap();
    assert_eq!(row.wire_frame, 2);
    assert!(rows.try_recv().is_err());
    assert_eq!(session.stats.frames.load(Ordering::Relaxed), 1);
    let performance = &session.info()["performance"];
    assert_eq!(performance["stages"]["frames"], 1);
    assert_eq!(performance["sample_frames"], 1);
}

/// The paced sender with the test loss given directly: block 0 of a streamed
/// frame loses more than its parity, so block 1 is never offered.
#[test]
fn a_streamed_frame_lost_in_its_first_block_stops_and_is_not_stamped() {
    let fixture = crate::state::test_support::Fixture::new();
    let session = fixture.session(fixture.client(0), butterpollo_core::session::Role::Stream);
    let socket = Arc::new(ReplySocket::bind("127.0.0.1:0").unwrap());
    let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
    receiver
        .set_read_timeout(Some(Duration::from_millis(300)))
        .unwrap();
    let now = Instant::now();
    let mut handoff = handoff(packetizer(None));
    handoff.slice_blocks = 2;
    handoff.slices = 2;
    let mut sender = Sender::packetizer(session.clone(), now, handoff).unwrap();
    let (sink, rows) = butterpollo_core::timing_record::channel(8);
    *session.timing_record.write().unwrap() = Some(Arc::new(sink));
    let key = now - Duration::from_millis(4);
    let mut claims = Claims::default();
    claims.push(Claim {
        key,
        presented: key,
        acquired: key,
        published: key,
        claimed: key,
        submitted: key,
    });
    let addr = receiver.local_addr().unwrap();
    let mut first = slice(0, 20, 1);
    first.presentation = Some(key);
    sender
        .submit_tick(vec![first], addr, Duration::ZERO, &mut claims, now)
        .unwrap();
    // Block 0's packet count, read before the paced sender starts.
    let block0 = wait_prepared(&sender).ready.as_ref().unwrap().counts[0];
    let shared = sender.slot.clone();
    let s = session.clone();
    let config = fixture.host.config.read().unwrap().clone();
    let host = fixture.host.clone();
    sender.transport = Some(thread::spawn(move || {
        let result = transport::run_with(
            &shared,
            socket,
            s,
            config,
            host,
            now,
            Instant::now(),
            Default::default(),
            butterpollo_core::stream_policy::SendDrop::parse("100:5"),
        );
        shared.finish(result);
    }));
    sender
        .submit_tick(vec![last(1, 20, 2)], addr, Duration::ZERO, &mut claims, now)
        .unwrap();
    assert_eq!(sender.next_wire_frame().unwrap(), 2);
    let stats = &session.stats;
    // Only block 0 was offered to the socket, and it was refused.
    let offered = stats.packets.load(Ordering::Relaxed) as usize;
    assert!(offered > 0 && offered <= block0, "{offered} of {block0}");
    let mut buffer = [0; 2048];
    assert!(
        receiver.recv(&mut buffer).is_err(),
        "nothing reached the client"
    );
    // The unrecoverable frame is counted, asks for a recovery frame, and has
    // no stage timings or record.
    assert_eq!(stats.send_loss_recoveries.load(Ordering::Relaxed), 1);
    assert!(rows.try_recv().is_err());
    assert_eq!(session.info()["performance"]["stages"]["frames"], 0);
}

#[test]
fn an_open_streamed_frame_counts_toward_the_claim_backlog_only() {
    let fixture = crate::state::test_support::Fixture::new();
    let session = fixture.session(fixture.client(0), butterpollo_core::session::Role::Stream);
    let sender = streaming_sender(session, 2, 2);
    submit_units(&sender, vec![slice(0, 20, 1)]);
    take_prepared(&sender);
    // The packetizer is idle again; the frame still waits for its last unit.
    {
        let state = sender.slot.state.lock().unwrap();
        let (state, timeout) = sender
            .slot
            .changed
            .wait_timeout_while(state, WAIT, |s| s.sending || s.pending.is_some())
            .unwrap();
        assert!(!timeout.timed_out());
        assert!(state.streaming);
    }
    assert_eq!(sender.backlog().unwrap(), 0);
    assert_eq!(sender.claim_backlog(None).unwrap(), 1);
    // The recreated encoder need not wait for it.
    assert_eq!(sender.next_wire_frame().unwrap(), 2);
    submit_units(&sender, vec![last(1, 20, 2)]);
    take_prepared(&sender);
    {
        let state = sender.slot.state.lock().unwrap();
        let (state, timeout) = sender
            .slot
            .changed
            .wait_timeout_while(state, WAIT, |s| s.sending || s.streaming)
            .unwrap();
        assert!(!timeout.timed_out());
        assert_eq!(state.claim_backlog(None).unwrap(), 0);
    }
    // A frame that begins over an open one closes it.
    submit_units(&sender, vec![slice(0, 20, 1)]);
    take_prepared(&sender);
    submit_units(&sender, vec![frame(2)]);
    take_prepared(&sender);
    let state = sender.slot.state.lock().unwrap();
    let (state, timeout) = sender
        .slot
        .changed
        .wait_timeout_while(state, WAIT, |s| s.sending || s.streaming)
        .unwrap();
    assert!(!timeout.timed_out());
    assert_eq!(state.claim_backlog(None).unwrap(), 0);
}
