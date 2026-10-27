use super::*;
#[test]
fn repeated_encoder_stalls_back_off_within_the_recovery_budget() {
    let limits: Vec<u64> = (0..6)
        .map(|n| encoder_stall_limit(n).as_millis() as u64)
        .collect();
    assert_eq!(limits, [250, 500, 1000, 2000, 2000, 2000]);
    assert_eq!(encoder_stall_limit(u32::MAX), Duration::from_millis(2000));
    // Four escalating attempts fit well inside the stream's recovery budget.
    assert!(limits[..4].iter().sum::<u64>() < 20_000);
}
#[test]
fn the_fec_and_audio_share_of_the_client_bitrate_is_not_a_warning() {
    let warnings = crate::session::Warnings::default();
    let client = Negotiated {
        configured_bitrate_kbps: 150_000,
        bitrate_kbps: 150_000,
        fps: 120,
        audio_channels: 2,
        ..Default::default()
    };
    for (text, warned) in [
        ("", false),
        (
            "max_bitrate=50000
",
            true,
        ),
    ] {
        let config = Config::parse(text).unwrap();
        let mut stream = client.clone();
        let uncapped = uncapped_bitrate_kbps(&stream, 0, &config);
        apply(&mut stream, 0, &config);
        // FEC and audio always take their share of the client's rate.
        assert!(stream.bitrate_kbps < client.configured_bitrate_kbps);
        report_bitrate(&warnings, uncapped, stream.bitrate_kbps, "max_bitrate");
        assert_eq!(!warnings.snapshot().is_empty(), warned, "{text}");
    }
}

#[test]
fn bitrate_clamps_are_reported_with_requested_and_applied_values() {
    let warnings = crate::session::Warnings::default();
    let config = Config::parse("max_bitrate=25000\n").unwrap();
    report_bitrate(
        &warnings,
        900_000,
        runtime_bitrate_kbps(&config, 900_000),
        "max_bitrate and the 500 Mbps runtime cap",
    );
    assert!(warnings.snapshot()[0].message.contains("900000 to 25000"));
    report_bitrate(
        &warnings,
        900_000,
        runtime_bitrate_kbps(&Config::default(), 900_000),
        "500 Mbps runtime cap",
    );
    assert!(warnings.snapshot()[0].message.contains("500000"));
    report_bitrate(&warnings, 20_000, 20_000, "unchanged");
    assert!(warnings.snapshot().is_empty());
}

#[test]
fn runtime_bitrate_honours_the_host_ceiling() {
    assert_eq!(runtime_bitrate_kbps(&Config::default(), 80_000), 80_000);
    assert_eq!(runtime_bitrate_kbps(&Config::default(), 900_000), 500_000);
    let capped = Config::parse("max_bitrate=25000\n").unwrap();
    assert_eq!(runtime_bitrate_kbps(&capped, 80_000), 25_000);
    assert_eq!(runtime_bitrate_kbps(&capped, 10_000), 10_000);
}
/// Claims (claim time, presented time) in milliseconds for source frames
/// presented at `source` and observed `detect` milliseconds later.
fn simulate(period: f64, source: &[f64], detect: impl Fn(usize) -> f64) -> Vec<(f64, f64)> {
    simulate_with(period, source, detect, |pacer| pacer)
}
fn simulate_with(
    period: f64,
    source: &[f64],
    detect: impl Fn(usize) -> f64,
    configure: impl Fn(Pacer) -> Pacer,
) -> Vec<(f64, f64)> {
    let start = Instant::now();
    let at = |ms: f64| start + Duration::from_secs_f64(ms / 1000.);
    let ms = |instant: Instant| instant.duration_since(start).as_secs_f64() * 1000.;
    let mut pacer = configure(Pacer::new(start, Duration::from_secs_f64(period / 1000.)));
    let mut claims = vec![];
    let mut newest: Option<f64> = None;
    let mut deadline: Option<f64> = None;
    let mut interval: Option<Duration> = None;
    let mut evaluate = |now: f64,
                        newest: &mut Option<f64>,
                        deadline: &mut Option<f64>,
                        pacer: &mut Pacer,
                        interval: Option<Duration>| {
        if let Some(presented) = *newest {
            match pacer.decide(at(now), at(presented), interval) {
                Pace::Claim => {
                    pacer.claimed(at(now));
                    claims.push((now, presented));
                    *newest = None;
                    *deadline = None;
                }
                Pace::WaitUntil(t) => *deadline = Some(ms(t).max(now + 0.001)),
            }
        }
    };
    for (index, &presented) in source.iter().enumerate() {
        let detected = presented + detect(index);
        while let Some(t) = deadline.filter(|t| *t < detected) {
            evaluate(t, &mut newest, &mut deadline, &mut pacer, interval);
            if deadline == Some(t) {
                break;
            }
        }
        // The capture worker sees every frame: median of recent intervals.
        let recent = &source[index.saturating_sub(16)..=index];
        let mut intervals: Vec<f64> = recent.windows(2).map(|w| w[1] - w[0]).collect();
        intervals.sort_by(f64::total_cmp);
        interval = (intervals.len() >= 4)
            .then(|| Duration::from_secs_f64(intervals[intervals.len() / 2] / 1000.));
        newest = Some(presented);
        pacer.observe_source(at(presented));
        evaluate(detected, &mut newest, &mut deadline, &mut pacer, interval);
    }
    claims
}
/// Like `simulate_with`, but the pacer's inputs come from the capture
/// worker's `Freshness`, as in the host: the picture interval, start and late
/// interval, and the mean interval as the source rate. Captures are
/// published in order.
fn simulate_captures(
    period: f64,
    captures: &[f64],
    detect: impl Fn(usize) -> f64,
    configure: impl Fn(Pacer) -> Pacer,
) -> Vec<(f64, f64)> {
    let start = Instant::now();
    let ns = |ms: f64| (ms * 1e6).round() as u64;
    let at = |ms: f64| start + Duration::from_nanos(ns(ms));
    let ms = |instant: Instant| instant.duration_since(start).as_secs_f64() * 1000.;
    let mut pacer = configure(Pacer::new(start, Duration::from_secs_f64(period / 1000.)));
    let mut cadence = crate::capture_policy::Freshness::default();
    let mut claims = vec![];
    let mut newest: Option<f64> = None;
    let mut deadline: Option<f64> = None;
    let mut interval: Option<Duration> = None;
    let mut published = 0f64;
    let mut evaluate = |now: f64,
                        newest: &mut Option<f64>,
                        deadline: &mut Option<f64>,
                        pacer: &mut Pacer,
                        interval: Option<Duration>| {
        if let Some(presented) = *newest {
            match pacer.decide(at(now), at(presented), interval) {
                Pace::Claim => {
                    pacer.claimed(at(now));
                    claims.push((now, presented));
                    *newest = None;
                    *deadline = None;
                }
                Pace::WaitUntil(t) => *deadline = Some(ms(t).max(now + 0.001)),
            }
        }
    };
    for (index, &presented) in captures.iter().enumerate() {
        published = published.max(presented + detect(index));
        while let Some(t) = deadline.filter(|t| *t < published) {
            evaluate(t, &mut newest, &mut deadline, &mut pacer, interval);
            if deadline == Some(t) {
                break;
            }
        }
        cadence.observe(ns(presented), ns(published - presented));
        interval = cadence.picture_interval().map(Duration::from_nanos);
        pacer.observe_source_rate(cadence.mean_interval().map(Duration::from_nanos));
        pacer.observe_picture(
            cadence
                .picture_start()
                .map(|nanos| start + Duration::from_nanos(nanos)),
            cadence.late_picture_interval().map(Duration::from_nanos),
        );
        newest = Some(presented);
        pacer.observe_source(at(presented));
        evaluate(published, &mut newest, &mut deadline, &mut pacer, interval);
    }
    claims
}
/// Deterministic noise in [0, 1).
fn noise(i: usize, seed: u64) -> f64 {
    let mut x = (i as u64)
        .wrapping_mul(6364136223846793005)
        .wrapping_add(seed.wrapping_mul(1442695040888963407));
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51afd7ed558ccd);
    x ^= x >> 33;
    (x % 10_000) as f64 / 10_000.
}
fn source(interval: f64, seconds: f64, jitter: impl Fn(usize) -> f64) -> Vec<f64> {
    (0..(seconds * 1000. / interval) as usize)
        .map(|i| i as f64 * interval + jitter(i))
        .collect()
}
const PERIOD: f64 = 1000. / 120.;
#[test]
fn a_double_rate_source_is_claimed_on_arrival_at_the_stream_rate() {
    // Any detection delay: a fresh frame presented just before the claim
    // is allowed must not lose to the older frame it replaces.
    for detect in [0.1, 0.3, 0.5, 0.8, 1.0] {
        let claims = simulate(PERIOD, &source(1000. / 240., 2., |_| 0.), |_| detect);
        let steady = &claims[10..];
        for (claim, presented) in steady {
            assert!(
                claim - presented <= detect + 1e-6,
                "detected after {detect} ms, aged {}",
                claim - presented
            );
        }
        for pair in steady.windows(2) {
            assert!((pair[1].0 - pair[0].0 - PERIOD).abs() < 0.01);
        }
        assert!((claims.len() as f64 - 240.).abs() <= 2.);
    }
}
#[test]
fn a_vrr_stream_claims_on_arrival_but_never_faster_than_the_stream_rate() {
    // VRR: no predictive waits, as the host configures it.
    let vrr = |pacer: Pacer| {
        pacer
            .with_prediction(false)
            .with_source_phase(false)
            .with_spacing(0.5)
    };
    // A game uncapped on the 1000 Hz virtual display: the encoder still
    // gets the stream rate, each frame claimed within a source interval.
    let claims = simulate_with(PERIOD, &source(1., 2., |_| 0.), |_| 0.2, vrr);
    assert!(
        (claims.len() as f64 - 240.).abs() <= 3.,
        "{} claims",
        claims.len()
    );
    assert!(
        claims
            .iter()
            .all(|(claim, presented)| claim - presented <= 1.2 + 1e-6)
    );
    // A game below the stream rate is claimed the moment each frame arrives.
    let frames = source(1000. / 48., 2., |_| 0.);
    let claims = simulate_with(PERIOD, &frames, |_| 0.2, vrr);
    assert_eq!(claims.len(), frames.len());
    assert!(
        claims
            .iter()
            .all(|(claim, presented)| (claim - presented - 0.2).abs() < 1e-6)
    );
}
/// The host's VRR loop: a new frame is claimed through the pacer; an
/// unchanged picture is sent again `repeat` ms after the last encode. Every
/// encode counts toward the stream rate. Returns the encodes, the game frames
/// claimed and the longest wait of a claimed game frame, in milliseconds.
fn vrr_with_repeats(period: f64, frames: &[f64], repeat: f64) -> (usize, usize, f64) {
    let start = Instant::now();
    let at = |ms: f64| start + Duration::from_secs_f64(ms / 1000.);
    let mut pacer = Pacer::new(start, Duration::from_secs_f64(period / 1000.))
        .with_prediction(false)
        .with_source_phase(false)
        .with_spacing(0.5);
    let (mut encodes, mut claimed, mut longest) = (0, 0, 0f64);
    let (mut next, mut newest, mut encoded_at) = (0, None, 0.);
    for step in 0..=((frames.last().unwrap() + period) * 20.) as usize {
        let now = step as f64 / 20.;
        while next < frames.len() && frames[next] <= now {
            newest = Some(frames[next]);
            next += 1;
        }
        let encode = match newest {
            Some(presented) => pacer.decide(at(now), at(presented), None) == Pace::Claim,
            None => now >= encoded_at + repeat,
        };
        if encode {
            if let Some(presented) = newest.take() {
                claimed += 1;
                longest = longest.max(now - presented);
            }
            pacer.claimed(at(now));
            encodes += 1;
            encoded_at = now;
        }
    }
    (encodes, claimed, longest)
}
#[test]
fn repeats_count_toward_the_stream_rate() {
    // A 100 fps VRR stream and a game at about 90 fps whose frames come
    // unevenly, 9 to 13 ms apart.
    let gaps = [9., 13., 10.5, 12.];
    let frames: Vec<f64> = (0..180)
        .scan(0., |t, i| {
            *t += gaps[i % 4];
            Some(*t)
        })
        .collect();
    let seconds = frames.last().unwrap() / 1000.;
    // The default minimum of 20 fps: no repeat while the game runs, every
    // frame claimed the moment it arrives.
    let (encodes, claimed, longest) = vrr_with_repeats(10., &frames, 50.);
    assert_eq!((encodes, claimed), (frames.len(), frames.len()));
    assert!(longest < 0.1, "{longest} ms");
    // A minimum at the stream rate, PyroWave's old default, resends after
    // every gap over 10 ms. Uncounted, those repeats took this stream to
    // 158 fps. Counted, it stays at the stream rate; the game frames that
    // follow a repeat wait for room instead, up to a period, and a few are
    // replaced by the next before they go out.
    let (encodes, claimed, longest) = vrr_with_repeats(10., &frames, 10.);
    let fps = encodes as f64 / seconds;
    assert!(fps <= 102., "{fps} fps");
    assert!(claimed * 100 >= frames.len() * 95, "{claimed} claimed");
    assert!(longest <= 10., "{longest} ms");
}
/// The host's arrival-paced loop with recovery requests (a keyframe or a
/// reference invalidation) from a client losing packets, at `requests` ms.
/// A request served with a new picture skips the pacer, as the host does.
/// `held`: an unchanged picture waits for the next new one per
/// [`reencode_at`] and spends credit per [`counts_toward_rate`]; otherwise it
/// is encoded at once and counted, as in rc.28. Returns the encodes, each
/// game frame's wait from arrival to claim in milliseconds, and the last
/// encode's time.
fn arrival_with_recovery(
    period: f64,
    frames: &[f64],
    requests: &[f64],
    configure: impl Fn(Pacer) -> Pacer,
    held: bool,
) -> (usize, Vec<f64>, f64) {
    let start = Instant::now();
    let at = |ms: f64| start + Duration::from_secs_f64(ms / 1000.);
    let mut pacer = configure(Pacer::new(start, Duration::from_secs_f64(period / 1000.)));
    let span = Duration::from_secs_f64(period / 1000.);
    let (mut encodes, mut waits) = (0, vec![]);
    let (mut next, mut request, mut newest, mut shown) = (0, 0, None, 0.);
    let (mut recovery, mut encoded_at) = (false, 0.);
    let end = frames.last().unwrap().max(*requests.last().unwrap_or(&0.)) + period;
    for step in 0..=(end * 20.) as usize {
        let now = step as f64 / 20.;
        while next < frames.len() && frames[next] <= now {
            newest = Some(frames[next]);
            next += 1;
        }
        while request < requests.len() && requests[request] <= now {
            recovery = true;
            request += 1;
        }
        // The capture worker's median of recent intervals.
        let mut intervals: Vec<f64> = frames[next.saturating_sub(16)..next]
            .windows(2)
            .map(|w| w[1] - w[0])
            .collect();
        intervals.sort_by(f64::total_cmp);
        let interval = (intervals.len() >= 4)
            .then(|| Duration::from_secs_f64(intervals[intervals.len() / 2] / 1000.));
        if let Some(presented) = newest {
            if recovery || pacer.decide(at(now), at(presented), interval) == Pace::Claim {
                pacer.claimed(at(now));
                (encodes, encoded_at) = (encodes + 1, now);
                waits.push(now - presented);
                (newest, shown, recovery) = (None, presented, false);
            }
        } else if recovery
            && (!held || at(now) >= reencode_at(true, at(shown), span, interval, at(end + 1e3)))
        {
            if !held || counts_toward_rate(false, true) {
                pacer.claimed(at(now));
            }
            (encodes, encoded_at) = (encodes + 1, now);
            recovery = false;
        }
    }
    (encodes, waits, encoded_at)
}
#[test]
fn recovery_requests_do_not_hold_back_a_moving_picture() {
    // A client on a lossy link asks for recovery four times a second, at
    // every phase of the frame period. Sources: a game held at the stream
    // rate by an RTSS limit, frame times a little uneven; and the release
    // check's motion probe, 60 fps on a 144 Hz display (14 and 21 ms gaps)
    // streamed at 60 fps.
    let at_rate = source(PERIOD, 10., |i| [0.3, -0.2, 0.1, -0.3][i % 4]);
    let on_144: Vec<f64> = (0..600)
        .map(|i| (i * 144 / 60) as f64 * 1000. / 144.)
        .collect();
    let requests: Vec<f64> = (0..40).map(|i| 100. + i as f64 * 251.3).collect();
    let vrr = |pacer: Pacer| pacer.with_prediction(false).with_spacing(0.5);
    let standard = |pacer: Pacer| pacer.with_prediction(true);
    let percentile = |waits: &[f64], p: f64| {
        let mut sorted = waits.to_vec();
        sorted.sort_by(f64::total_cmp);
        sorted[((sorted.len() - 1) as f64 * p) as usize]
    };
    let mean = |waits: &[f64]| waits.iter().sum::<f64>() / waits.len() as f64;
    let cases = [
        (
            "120 fps game, vrr",
            PERIOD,
            &at_rate,
            &vrr as &dyn Fn(Pacer) -> Pacer,
        ),
        ("120 fps game", PERIOD, &at_rate, &standard),
        ("60 fps probe on 144 Hz", 1000. / 60., &on_144, &standard),
    ];
    for (name, period, frames, configure) in cases {
        let (before, before_waits, _) =
            arrival_with_recovery(period, frames, &requests, configure, false);
        let (after, after_waits, _) =
            arrival_with_recovery(period, frames, &requests, configure, true);
        eprintln!(
            "{name}: encodes {before} -> {after}, game frames sent {} -> {} of {}; game frame wait mean {:.2} -> {:.2} ms, p95 {:.2} -> {:.2} ms, p99 {:.2} -> {:.2} ms, max {:.2} -> {:.2} ms",
            before_waits.len(),
            after_waits.len(),
            frames.len(),
            mean(&before_waits),
            mean(&after_waits),
            percentile(&before_waits, 0.95),
            percentile(&after_waits, 0.95),
            percentile(&before_waits, 0.99),
            percentile(&after_waits, 0.99),
            percentile(&before_waits, 1.),
            percentile(&after_waits, 1.),
        );
        // A request between frames re-encoded the unchanged picture, and its
        // credit held the following game frames back: at four requests a
        // second the deficit is never repaid, so frames wait milliseconds
        // and some are replaced by the next before they go out.
        assert!(before >= before_waits.len() + 8, "{name}: {before} encodes");
        assert!(before_waits.len() < frames.len() - 5, "{name}");
        assert!(mean(&before_waits) > 1., "{name}");
        // Held: every request rides the next game frame, nothing extra is
        // sent, and no game frame waits.
        assert_eq!(after, frames.len(), "{name}");
        assert_eq!(after_waits.len(), frames.len(), "{name}");
        assert!(percentile(&after_waits, 1.) < 0.1, "{name}");
    }
    // A still screen still gets its keyframe at once.
    let still = [0.];
    let requests = [500.];
    let (encodes, _, encoded_at) = arrival_with_recovery(PERIOD, &still, &requests, vrr, true);
    assert_eq!((encodes, encoded_at), (2, 500.));
}
#[test]
fn a_source_at_the_stream_rate_is_never_skipped_despite_jitter() {
    let frames = source(PERIOD, 2., |i| if i % 3 == 0 { 0.3 } else { -0.2 });
    let claims = simulate(PERIOD, &frames, |_| 0.4);
    assert_eq!(claims.len(), frames.len());
    assert!(
        claims
            .iter()
            .all(|(claim, presented)| claim - presented <= 0.4 + 1e-6)
    );
}
#[test]
fn slower_sources_are_claimed_on_arrival() {
    for interval in [1000. / 60., 1000. / 90., 11.] {
        let frames = source(interval, 2., |_| 0.);
        let claims = simulate(PERIOD, &frames, |_| 0.5);
        assert_eq!(claims.len(), frames.len());
        assert!(
            claims
                .iter()
                .all(|(claim, presented)| claim - presented <= 0.5 + 1e-6)
        );
    }
}
#[test]
fn an_unaligned_faster_source_stays_fresh_and_within_the_stream_rate() {
    for interval in [6., 1000. / 144., 1000. / 165., 1.] {
        let claims = simulate(PERIOD, &source(interval, 4., |_| 0.), |_| 0.3);
        let rate = claims.len() as f64 / 4.;
        assert!(rate <= 121.5, "{interval} ms source claimed at {rate} fps");
        assert!(
            rate >= 100.,
            "{interval} ms source claimed at only {rate} fps"
        );
        for pair in claims.windows(2) {
            assert!(pair[1].0 - pair[0].0 >= PERIOD * 0.75 - 1e-6);
        }
        let mean_age = claims.iter().map(|(c, p)| c - p).sum::<f64>() / claims.len() as f64;
        assert!(
            mean_age <= PERIOD / 4.,
            "{interval} ms source aged {mean_age} ms"
        );
    }
}
#[test]
fn a_late_detection_still_claims_the_fresh_double_rate_frame() {
    // A polled capture sometimes notices a frame well after it was presented.
    let detect = |i: usize| if i.is_multiple_of(7) { 1.8 } else { 0.3 };
    let claims = simulate(PERIOD, &source(1000. / 240., 2., |_| 0.), detect);
    for (claim, presented) in &claims[10..] {
        assert!(
            claim - presented <= 1.8 + 1e-6,
            "aged {}",
            claim - presented
        );
    }
    assert!((claims.len() as f64 - 240.).abs() <= 2.);
}
#[test]
fn a_frame_slightly_before_a_drained_credit_slot_is_claimed_at_once() {
    // Extra desktop updates can spend the credit; a later claim then sits
    // exactly on the credit slot. Timers here wake half a millisecond late,
    // so waiting for that slot would make every following frame late too.
    let start = Instant::now();
    let period = Duration::from_secs_f64(PERIOD / 1000.);
    let mut pacer = Pacer::new(start, period);
    for claim in 0..3 {
        pacer.claimed(start + period.mul_f64(0.75) * claim);
    }
    let last = start + period.mul_f64(1.5);
    let arrived = last + period - Duration::from_micros(500);
    assert_eq!(pacer.decide(arrived, arrived, Some(period)), Pace::Claim);
    // The burst guard still holds.
    let early = last + period / 2;
    assert_ne!(pacer.decide(early, early, Some(period)), Pace::Claim);
}
#[test]
fn a_startup_credit_deficit_does_not_delay_a_steady_source_for_long() {
    // A burst at startup leaves the credit short. A source at exactly the
    // stream rate must soon be claimed on arrival again, not one timer
    // wake later for the rest of the session.
    let start = Instant::now();
    let period = Duration::from_secs_f64(PERIOD / 1000.);
    let mut pacer = Pacer::new(start, period);
    for claim in 0..6 {
        pacer.claimed(start + Duration::from_micros(10) * claim);
    }
    let mut delayed_late = 0;
    for frame in 1..=240u32 {
        let arrived = start + period * frame;
        if pacer.decide(arrived, arrived, Some(period)) == Pace::Claim {
            pacer.claimed(arrived);
        } else {
            let at = pacer.allowed_at(arrived);
            pacer.claimed(at);
            delayed_late += u32::from(frame > 120);
        }
    }
    assert_eq!(delayed_late, 0);
}
#[test]
fn a_prediction_floor_leaves_a_1000_hz_source_unchanged_and_waits_for_a_slower_one() {
    // VRR: 16 ms stream period, claims half a period apart. After a claim at
    // 0, a frame arrives at 3 ms; the next claim is allowed at 8 ms.
    let start = Instant::now();
    let period = Duration::from_millis(16);
    let floor = Duration::from_millis(2);
    let vrr = || {
        let mut pacer = Pacer::new(start, period)
            .with_spacing(0.5)
            .with_prediction_floor(floor);
        pacer.claimed(start);
        pacer
    };
    let arrived = start + Duration::from_millis(3);
    let wait =
        |pacer: &mut Pacer, interval: Duration| pacer.decide(arrived, arrived, Some(interval));
    // A 240 Hz game (4.17 ms) and a source of exactly the floor are expected
    // before the slot is over: wait for them, with the detection window.
    assert_eq!(
        wait(&mut vrr(), Duration::from_millis(4)),
        Pace::WaitUntil(start + Duration::from_millis(11))
    );
    assert_eq!(
        wait(&mut vrr(), floor),
        Pace::WaitUntil(start + Duration::from_millis(9))
    );
    // A 1000 Hz source is below the floor: the claim waits only for its slot,
    // exactly as with prediction off, however the floor is set.
    let one_khz = Duration::from_micros(1500);
    assert_eq!(
        wait(&mut vrr(), one_khz),
        Pace::WaitUntil(start + Duration::from_millis(8))
    );
    let mut off = Pacer::new(start, period)
        .with_spacing(0.5)
        .with_prediction(false);
    off.claimed(start);
    assert_eq!(wait(&mut off, one_khz), wait(&mut vrr(), one_khz));
    // Without the floor the same source would have held the claim.
    let mut unfloored = Pacer::new(start, period).with_spacing(0.5);
    unfloored.claimed(start);
    assert_eq!(
        wait(&mut unfloored, one_khz),
        Pace::WaitUntil(start + Duration::from_micros(8500))
    );
}
#[test]
fn prediction_override_keeps_the_earliest_slot_and_does_not_bypass_rate_limits() {
    let start = Instant::now();
    let period = Duration::from_millis(8);
    let mut predicted = Pacer::new(start, period);
    let mut immediate = Pacer::new(start, period).with_prediction(false);
    predicted.claimed(start);
    immediate.claimed(start);
    let arrived = start + Duration::from_millis(3);
    let source = Some(Duration::from_millis(4));
    // The newer source frame is expected at 7 ms. Normal pacing allows
    // its detection window within the 2.5 ms prediction budget; the diagnostic
    // mode only waits for the 6 ms minimum spacing.
    assert_eq!(
        predicted.decide(arrived, arrived, source),
        Pace::WaitUntil(start + Duration::from_micros(8500))
    );
    assert_eq!(
        immediate.decide(arrived, arrived, source),
        Pace::WaitUntil(start + Duration::from_millis(6))
    );
    assert_eq!(predicted.allowed_at(arrived), immediate.allowed_at(arrived));
    // Disabling anticipation must not turn a fast capture into an uncapped
    // stream. Exercise both spacing and the longer-term credit budget.
    let mut claims = Vec::new();
    for tick in 1..=20_000 {
        let at = start + Duration::from_micros(tick * 100);
        if immediate.decide(at, at, source) == Pace::Claim {
            immediate.claimed(at);
            claims.push(at);
        }
    }
    assert!(claims.len() >= 250);
    assert!(claims.len() <= 254);
    assert!(
        claims
            .windows(2)
            .all(|pair| pair[1] - pair[0] >= period.mul_f64(0.75))
    );
}
#[test]
fn a_missed_prediction_does_not_stack_detection_after_the_wait_budget() {
    for fps in [59.94, 90., 120., 144., 240.] {
        let start = Instant::now();
        let period = Duration::from_secs_f64(1. / fps);
        let interval = period / 2;
        let presented = start + interval;
        let detected = presented + Duration::from_micros(200);
        let mut pacer = Pacer::new(start, period);
        pacer.claimed(start);
        let allowed = pacer.allowed_at(detected);
        let Pace::WaitUntil(deadline) = pacer.decide(detected, presented, Some(interval)) else {
            panic!("the minimum spacing still applies");
        };
        assert!(deadline > allowed);
        // The next frame is due half a millisecond before the budget ends; it
        // gets the notice window, not another quarter period.
        assert!(
            deadline <= allowed + period / 4 + Duration::from_micros(500) + Pacer::PREDICTED_NOTICE
        );
        assert_eq!(
            pacer.decide(deadline, presented, Some(interval)),
            Pace::Claim
        );
    }
}
#[test]
fn a_frame_due_at_the_end_of_the_wait_budget_is_still_noticed() {
    // A game at twice the stream rate on a VRR stream: claims half a period
    // apart. The next game frame is due 0.2 ms before the prediction budget
    // ends and the capture notices it 0.3 ms after its present. Cut off at the
    // budget, the claim took the frame before it, half a period old.
    for fps in [59.94, 90., 120., 144.] {
        let start = Instant::now();
        let period = Duration::from_secs_f64(1. / fps);
        let interval = period / 2;
        let mut pacer = Pacer::new(start, period)
            .with_spacing(0.5)
            .with_prediction_floor(Duration::from_millis(2));
        pacer.claimed(start);
        let allowed = start + period.mul_f64(0.5);
        let budget = allowed + period / 4 + Duration::from_micros(500);
        let next = budget - Duration::from_micros(200);
        let presented = next - interval;
        let noticed = |at: Instant| at + Duration::from_micros(300);
        assert_eq!(pacer.allowed_at(noticed(presented)), allowed);
        let Pace::WaitUntil(deadline) = pacer.decide(noticed(presented), presented, Some(interval))
        else {
            panic!("the claim is not allowed yet");
        };
        assert!(
            deadline > noticed(next),
            "{fps} fps: gave up before the frame was noticed"
        );
        assert!(deadline <= budget + Pacer::PREDICTED_NOTICE);
        assert_eq!(
            pacer.decide(noticed(next), next, Some(interval)),
            Pace::Claim
        );
    }
}
#[test]
fn a_picture_later_than_the_median_is_waited_for_within_the_late_interval() {
    // The median predicts the next picture 0.2 ms before the budget ends; it
    // comes an eighth of a period later, as a game's uneven frames do.
    for fps in [59.94, 90., 120., 144.] {
        let start = Instant::now();
        let period = Duration::from_secs_f64(1. / fps);
        let interval = period / 2;
        let mut pacer = Pacer::new(start, period)
            .with_spacing(0.5)
            .with_prediction_floor(Duration::from_millis(2));
        pacer.claimed(start);
        let allowed = start + period.mul_f64(0.5);
        let budget = allowed + period / 4 + Duration::from_micros(500);
        let next = budget - Duration::from_micros(200);
        let presented = next - interval;
        let noticed = |at: Instant| at + Duration::from_micros(300);
        let arrived = noticed(next + period / 8);
        let mut deadline = |late: Option<Duration>| {
            pacer.observe_picture(Some(presented), late);
            match pacer.decide(noticed(presented), presented, Some(interval)) {
                Pace::WaitUntil(deadline) => deadline,
                Pace::Claim => panic!("the claim is not allowed yet"),
            }
        };
        assert!(deadline(None) < arrived, "{fps} fps");
        let waited = deadline(Some(interval + period / 8));
        assert!(
            arrived < waited,
            "{fps} fps: gave up before the late picture"
        );
        assert!(waited <= next + period / 8 + Pacer::PREDICTED_NOTICE);
        // A late interval far past the median extends the wait by at most a
        // quarter period.
        assert!(deadline(Some(interval * 3)) <= next + period / 4 + Pacer::PREDICTED_NOTICE);
    }
}
#[test]
fn a_game_at_twice_a_vrr_stream_rate_is_never_claimed_a_game_frame_old() {
    // 3.0.0-rc.2: a 120 Hz game on a 60 fps VRR stream, its frames a little
    // uneven and noticed 0.15 to 0.45 ms after their present. Claims that gave
    // up just before the next frame took one 7-9 ms old, and each set the next
    // budget a period later, so the miss repeated (3.5 % of claims here, 6-9 %
    // on the RX 7900 XT): the age p95 doubled.
    let vrr = |pacer: Pacer| {
        pacer
            .with_prediction_floor(Duration::from_millis(2))
            .with_source_phase(true)
            .with_spacing(0.5)
    };
    let period = 1000. / 60.;
    for seed in 0..6 {
        let frames = source(1000. / 120., 25., |i| 1. + noise(i, seed) * 0.5);
        let claims = simulate_with(period, &frames, |i| 0.15 + 0.3 * noise(i, seed + 99), vrr);
        let steady = &claims[300..];
        let oldest = steady.iter().map(|(c, p)| c - p).fold(0., f64::max);
        assert!(
            oldest < 4.,
            "seed {seed}: a claim took a frame {oldest} ms old"
        );
        let seconds = (steady.last().unwrap().0 - steady[0].0) / 1000.;
        assert!((steady.len() - 1) as f64 / seconds <= 60.2);
        assert!(
            steady
                .windows(2)
                .all(|pair| pair[1].0 - pair[0].0 >= period / 2. - 1e-6)
        );
    }
}
#[test]
fn a_second_composition_of_each_game_frame_does_not_age_vrr_claims() {
    // 3.0.0-rc.2 on the RX 7900 XT: WGC on the 240 Hz VRR display delivers
    // each frame of a 120 Hz game with a second composition 0.1 or 1 ms later,
    // in some sessions a third 3-4 ms later, all noticed about 0.1 ms after
    // their present. The capture intervals' median read 7 or 3 ms, so a claim
    // waited for a frame that was not coming, then took a second composition
    // 7-8 ms old just before the next frame: 6-9 % of claims, and the age p95
    // went from 3.7 to 7.7 ms. With the capture median, this simulation takes
    // 10 % of claims late (52-57 % with the third composition).
    let vrr = |pacer: Pacer| {
        pacer
            .with_prediction_floor(Duration::from_millis(2))
            .with_source_phase(true)
            .with_spacing(0.5)
    };
    let period = 1000. / 60.;
    for third in [false, true] {
        for seed in 0..6 {
            let mut captures = vec![];
            for frame in 0..3000 {
                let start = frame as f64 * 1000. / 120. + noise(frame, seed) * 1.2;
                captures.push(start);
                captures.push(
                    start
                        + if noise(frame, seed + 7) < 0.35 {
                            0.1
                        } else {
                            1.
                        },
                );
                if third && noise(frame, seed + 13) < 0.25 {
                    captures.push(start + 3. + noise(frame, seed + 17));
                }
            }
            let claims = simulate_captures(
                period,
                &captures,
                |i| 0.08 + 0.12 * noise(i, seed + 99),
                vrr,
            );
            let steady = &claims[300..];
            let mut ages: Vec<f64> = steady.iter().map(|(c, p)| c - p).collect();
            ages.sort_by(f64::total_cmp);
            let late = ages.iter().filter(|age| **age > 4.).count();
            let p99 = ages[ages.len() * 99 / 100];
            assert!(
                late * 200 < ages.len() && p99 < 4.,
                "third={third} seed {seed}: {late} of {} claims late, p99 {p99} ms",
                ages.len()
            );
            let seconds = (steady.last().unwrap().0 - steady[0].0) / 1000.;
            assert!((steady.len() - 1) as f64 / seconds <= 60.2);
            assert!(
                steady
                    .windows(2)
                    .all(|pair| pair[1].0 - pair[0].0 >= period / 2. - 1e-6)
            );
        }
    }
}
#[test]
fn a_fresh_capture_can_replace_the_frame_before_the_prediction_fallback() {
    for fps in [59.94, 90., 120., 144., 240.] {
        let start = Instant::now();
        let period = Duration::from_secs_f64(1. / fps);
        let interval = period / 2;
        let presented = start + interval;
        let detected = presented + Duration::from_micros(200);
        let mut pacer = Pacer::new(start, period);
        pacer.claimed(start);
        let Pace::WaitUntil(deadline) = pacer.decide(detected, presented, Some(interval)) else {
            panic!("the minimum spacing still applies");
        };
        let fresh = presented + interval;
        let arrived = fresh + Duration::from_micros(200);
        assert!(arrived < deadline);
        assert_eq!(pacer.decide(arrived, fresh, Some(interval)), Pace::Claim);
        pacer.claimed(arrived);
        assert!(pacer.allowed_at(arrived) >= arrived + period.mul_f64(0.75));
    }
}
#[test]
fn explicitly_enabling_prediction_preserves_default_decisions() {
    let start = Instant::now();
    let period = Duration::from_millis(8);
    let mut default = Pacer::new(start, period);
    let mut explicit = Pacer::new(start, period).with_prediction(true);
    for tick in 0..2000 {
        let at = start + Duration::from_micros(tick * 700);
        let presented = at - Duration::from_micros((tick % 7) * 100);
        let interval = Some(Duration::from_micros(3500 + (tick % 3) * 500));
        let decision = default.decide(at, presented, interval);
        assert_eq!(decision, explicit.decide(at, presented, interval));
        if decision == Pace::Claim {
            default.claimed(at);
            explicit.claimed(at);
        }
    }
}
#[test]
fn repeat_grace_preserves_credit_for_the_imminent_fresh_frame() {
    let start = Instant::now();
    let period = Duration::from_millis(16);
    let first_claim = start + Duration::from_micros(400);
    let mut guarded = Pacer::new(start, period).with_source_phase(true);
    let mut repeated = Pacer::new(start, period);
    guarded.claimed(first_claim);
    repeated.claimed(first_claim);
    let due = first_claim + period;
    let fresh_presented = start + period;
    let fresh_arrived = fresh_presented + Duration::from_micros(600);
    let deadline = guarded.repeat_deadline(due, start, Some(period));
    assert!(due < fresh_arrived && fresh_arrived < deadline);
    assert_eq!(guarded.last_claim, Some(first_claim));
    assert_eq!(guarded.credit_at, first_claim);
    assert_eq!(guarded.credit, repeated.credit);
    // Without grace, the unchanged frame wins just before capture arrives.
    repeated.claimed(due);
    assert!(matches!(
        repeated.decide(fresh_arrived, fresh_presented, Some(period)),
        Pace::WaitUntil(_)
    ));
    assert_eq!(
        guarded.decide(fresh_arrived, fresh_presented, Some(period)),
        Pace::Claim
    );
}
#[test]
fn repeat_grace_is_fixed_and_does_not_starve_a_static_desktop() {
    let start = Instant::now();
    let period = Duration::from_millis(16);
    let pacer = Pacer::new(start, period).with_source_phase(true);
    let due = start + period;
    let deadline = pacer.repeat_deadline(due, start, Some(period));
    assert_eq!(deadline, due + period / 4);
    for _ in 0..100 {
        assert_eq!(pacer.repeat_deadline(due, start, Some(period)), deadline);
    }
    // No fresh capture arrived: later static repeats cannot keep predicting
    // a new frame relative to the polling time or the last repeated encode.
    for repeat in 1..=20 {
        let next_due = deadline + period * repeat;
        assert_eq!(
            pacer.repeat_deadline(next_due, start, Some(period)),
            next_due
        );
    }
    // A prediction near the edge cannot extend beyond the explicit cap.
    let early_due = start + period - Duration::from_millis(3);
    assert_eq!(
        pacer.repeat_deadline(early_due, start, Some(period)),
        early_due + period / 4 + Duration::from_micros(500)
    );
    let too_early = start + period / 2;
    assert_eq!(
        pacer.repeat_deadline(too_early, start, Some(period)),
        too_early
    );
}
#[test]
fn repeat_grace_handles_fractional_rates_without_delaying_slow_sources() {
    let start = Instant::now();
    for (stream_hz, source_hz) in [(60., 59.94), (59.94, 60.), (120., 119.998)] {
        let period = Duration::from_secs_f64(1. / stream_hz);
        let interval = Duration::from_secs_f64(1. / source_hz);
        let pacer = Pacer::new(start, period).with_source_phase(true);
        let due = start + period;
        let deadline = pacer.repeat_deadline(due, start, Some(interval));
        assert!(deadline > due);
        assert!(deadline <= due + period / 4 + Duration::from_micros(500));
        for unavailable_or_slow in [None, Some(period * 2), Some(period * 4)] {
            assert_eq!(pacer.repeat_deadline(due, start, unavailable_or_slow), due);
        }
    }
}
#[test]
fn repeat_grace_protects_ddx_and_vrr_without_predicting_fresh_claims() {
    let start = Instant::now();
    let period = Duration::from_millis(16);
    let due = start + period;
    for pacer in [
        Pacer::new(start, period),
        Pacer::new(start, period).with_source_phase(false),
        Pacer::new(start, period)
            .with_source_phase(true)
            .with_prediction(false),
    ] {
        assert_eq!(
            pacer.repeat_deadline(due, start, Some(period)),
            due + period / 4
        );
    }
}
#[test]
fn repeat_grace_protects_slow_sources_only_when_their_next_frame_is_due() {
    let start = Instant::now();
    let period = Duration::from_millis(8);
    let pacer = Pacer::new(start, period);
    for multiple in [2, 3, 5, 12] {
        let interval = period * multiple;
        for repeat in 1..multiple {
            let due = start + period * repeat;
            assert_eq!(pacer.repeat_deadline(due, start, Some(interval)), due);
        }
        let due = start + interval;
        assert_eq!(
            pacer.repeat_deadline(due, start, Some(interval)),
            due + period / 4
        );
        let late = due + period;
        assert_eq!(pacer.repeat_deadline(late, start, Some(interval)), late);
    }
}
#[test]
fn learned_phase_anticipates_the_main_update_instead_of_an_extra_composition() {
    let start = Instant::now();
    let period = Duration::from_millis(16);
    let mut pacer = Pacer::new(start, period)
        .with_prediction(false)
        .with_source_phase(true);
    for frame in 0..40 {
        pacer.observe_source(start + period * frame);
        if frame % 4 == 0 {
            pacer.observe_source(start + period * frame + period / 2);
        }
    }
    assert!(pacer.source_phase.as_ref().unwrap().has_surplus());
    let last = start + period * 39;
    pacer.claimed(last);
    let extra = last + period / 2;
    pacer.observe_source(extra);
    let Pace::WaitUntil(deadline) = pacer.decide(extra, extra, Some(period)) else {
        panic!("an extra composition must not consume this source frame's credit");
    };
    assert_eq!(deadline, last + period + period / 4);
    assert!(deadline - pacer.allowed_at(extra) < period);
    let fresh = last + period;
    pacer.observe_source(fresh);
    assert_eq!(pacer.decide(fresh, fresh, Some(period)), Pace::Claim);
    // The feature learns nothing from repeatedly inspecting one Arc.
    let count = pacer.source_phase.as_ref().unwrap().count;
    for _ in 0..100 {
        pacer.observe_source(fresh);
    }
    assert_eq!(pacer.source_phase.as_ref().unwrap().count, count);

    pacer.claimed(fresh);
    let extra = fresh + period - Duration::from_millis(2);
    pacer.observe_source(extra);
    let Pace::WaitUntil(deadline) = pacer.decide(extra, extra, None) else {
        panic!("a funded extra composition must still wait for the imminent update");
    };
    assert_eq!(deadline, fresh + period + period / 4);
    // Surplus compositions distort the interval median, and an already funded
    // slot must still give the imminent source update a chance to arrive.
    for interval in [None, Some(period / 2), Some(period * 2)] {
        assert_eq!(pacer.allowed_at(extra), extra);
        assert_eq!(
            pacer.decide(extra, extra, interval),
            Pace::WaitUntil(deadline)
        );
        assert_eq!(pacer.decide(deadline, extra, interval), Pace::Claim);
    }
}
#[test]
fn stale_in_phase_compositions_wait_for_late_updates_with_a_fixed_fallback() {
    let start = Instant::now();
    let period = Duration::from_millis(16);
    for arrives in [false, true] {
        let mut pacer = Pacer::new(start, period)
            .with_prediction(false)
            .with_source_phase(true)
            .with_spacing(0.5);
        for frame in 0..40 {
            pacer.observe_source(start + period * frame);
            if frame % 2 == 0 {
                pacer.observe_source(start + period * frame + Duration::from_micros(1500));
            }
        }
        let last = start + period * 39;
        pacer.claimed(last - period / 2);
        pacer.claimed(last);
        let extra = last + Duration::from_micros(1500);
        pacer.observe_source(extra);
        let phase = pacer.source_phase.as_ref().unwrap();
        assert!(phase.distance(phase.offset(extra), phase.center.unwrap()) <= phase.radius_ns());
        let now = last + Duration::from_millis(14);
        assert_eq!(pacer.allowed_at(now), now);
        let Pace::WaitUntil(deadline) = pacer.decide(now, extra, None) else {
            panic!("an old in-phase composition must not bypass the next update");
        };
        let late = last + period + period / 8;
        assert_eq!(deadline, last + period + period / 4);
        for step in 0..10 {
            assert_eq!(
                pacer.decide(now + (deadline - now) * step / 10, extra, None),
                Pace::WaitUntil(deadline)
            );
        }
        if arrives {
            pacer.observe_source(late);
            assert_eq!(pacer.decide(late, late, None), Pace::Claim);
            pacer.claimed(late);
        } else {
            assert_eq!(pacer.decide(deadline, extra, None), Pace::Claim);
            pacer.claimed(deadline);
        }
        // Stopping the extra compositions or the animation must not delay
        // newly arriving frames behind the previously learned phase.
        for frame in 1..=64 {
            let at = late + period * (frame * 2);
            pacer.observe_source(at);
            assert_eq!(pacer.decide(at, at, None), Pace::Claim);
            pacer.claimed(at);
        }
        assert!(!pacer.source_phase.as_ref().unwrap().has_surplus());
        let resumed = late + period * 256;
        pacer.observe_source(resumed);
        assert_eq!(pacer.decide(resumed, resumed, None), Pace::Claim);
        assert_eq!(pacer.source_phase.as_ref().unwrap().count, 1);
    }
}
#[test]
fn an_early_new_picture_cannot_wait_past_the_phase_delivery_budget() {
    let start = Instant::now();
    for fps in [60., 120.] {
        let period = Duration::from_secs_f64(1. / fps);
        let mut pacer = Pacer::new(start, period)
            .with_prediction(false)
            .with_source_phase(true)
            .with_spacing(0.5);
        for frame in 0..64 {
            pacer.observe_source(start + period * frame);
            if frame % 2 == 0 {
                pacer.observe_source(start + period * frame + period / 2);
            }
        }
        let last = start + period * 63;
        pacer.claimed(last - period / 2);
        pacer.claimed(last);
        // The compositor can deliver a new game picture before the phase
        // learned from earlier captures, then supply no replacement at it.
        let picture = last + period.mul_f64(0.8);
        pacer.observe_source(picture);
        let seen = picture + Duration::from_micros(150);
        let deadline = last + period + period / 4;
        assert!(pacer.source_phase.as_ref().unwrap().has_surplus());
        for step in 0..10 {
            let now = seen + (deadline - seen) * step / 10;
            assert_eq!(
                pacer.decide(now, picture, Some(period / 2)),
                Pace::WaitUntil(deadline),
                "{fps} fps, poll {step}"
            );
        }
        assert_eq!(
            pacer.decide(deadline, picture, Some(period / 2)),
            Pace::Claim
        );
        assert!(deadline - last < period.mul_f64(1.5));
    }
}
#[test]
fn vrr_source_phase_escapes_startup_alias_with_jittered_extra_compositions() {
    let period = 1000. / 60.;
    let mut lowest_without_phase = 1f64;
    for offset in 0..64 {
        // The startup burst spends credit before the regular source begins.
        // Every second game frame also produces a visually identical capture.
        let mut publications = vec![(0., 0), (period / 2. + 0.1, 0)];
        for frame in 0..600 {
            let at = period * (frame + 1) as f64
                + period * f64::from(offset) / 64.
                + f64::from(frame * 7 % 11) * 0.12;
            publications.push((at, frame + 1));
            if frame % 2 == 0 {
                publications.push((at + 2. + f64::from(frame * 5 % 7) * 0.1, frame + 1));
            }
        }
        let captures: Vec<f64> = publications.iter().map(|&(at, _)| at).collect();
        for phase in [false, true] {
            let claims = simulate_with(
                period,
                &captures,
                |_| 0.,
                |pacer| {
                    pacer
                        .with_prediction(false)
                        .with_source_phase(phase)
                        .with_spacing(0.5)
                },
            );
            let steady: Vec<_> = claims.iter().filter(|(at, _)| *at >= 3000.).collect();
            let mut frames: Vec<_> = steady
                .iter()
                .map(|&&(_, presented)| {
                    let index = publications
                        .binary_search_by(|(at, _)| at.total_cmp(&presented))
                        .unwrap();
                    publications[index].1
                })
                .collect();
            frames.dedup();
            let unique = frames.len() as f64 / steady.len() as f64;
            if phase {
                assert!(
                    steady.len() >= 410,
                    "offset {offset}: {} claims",
                    steady.len()
                );
                assert!(unique >= 0.9, "offset {offset}: {unique} unique share");
            } else {
                lowest_without_phase = lowest_without_phase.min(unique);
            }
        }
    }
    // The faster-source grace repays part of the deficit on its own (0.90
    // unique without the phase, against under 0.8 before it); the alias
    // still shows.
    assert!(
        lowest_without_phase < 0.95,
        "startup alias was not exercised"
    );
}
#[test]
fn phase_learning_tracks_fractional_rate_drift_and_resets_on_jumps_and_recovery() {
    let start = Instant::now();
    for (stream_hz, source_hz) in [(60., 59.94), (59.94, 60.)] {
        let period = Duration::from_secs_f64(1. / stream_hz);
        let mut pacer = Pacer::new(start, period).with_source_phase(true);
        for frame in 0..500 {
            let at = start + Duration::from_secs_f64(f64::from(frame) / source_hz);
            pacer.observe_source(at);
            if frame > 100 {
                let phase = pacer.source_phase.as_ref().unwrap();
                assert!(phase.center.is_some());
                assert!(!phase.has_surplus());
                assert!(
                    phase.distance(phase.offset(at), phase.center.unwrap()) <= phase.radius_ns()
                );
            }
        }
        let last = pacer.source_phase.as_ref().unwrap().last.unwrap();
        for frame in 1..=3 {
            pacer.observe_source(last + period * frame + period / 3);
        }
        assert!(pacer.source_phase.as_ref().unwrap().center.is_none());
        pacer.reset_source_phase();
        assert_eq!(pacer.source_phase.as_ref().unwrap().count, 0);
        assert!(pacer.source_phase.as_ref().unwrap().last.is_none());
    }
}
#[test]
fn learned_phase_survives_increased_surplus_without_repeating_game_frames() {
    let period = 1000. / 60.;
    for offset in 0..64 {
        let mut publications = vec![(0., 0), (period / 2. + 0.1, 0)];
        for frame in 0..600 {
            let at = period * (frame + 1) as f64
                + period * f64::from(offset) / 64.
                + f64::from(frame * 7 % 11) * 0.12;
            publications.push((at, frame + 1));
            // Once the phase is learned, surplus captures rise from 30 to 60 Hz.
            if frame >= 180 || frame % 2 == 0 {
                publications.push((at + 2. + f64::from(frame * 5 % 7) * 0.1, frame + 1));
            }
        }
        let captures: Vec<f64> = publications.iter().map(|&(at, _)| at).collect();
        let claims = simulate_with(
            period,
            &captures,
            |_| 0.,
            |pacer| {
                pacer
                    .with_prediction(false)
                    .with_source_phase(true)
                    .with_spacing(0.5)
            },
        );
        let steady: Vec<_> = claims.iter().filter(|(at, _)| *at >= 4000.).collect();
        let mut frames: Vec<_> = steady
            .iter()
            .map(|&&(_, presented)| {
                let index = publications
                    .binary_search_by(|(at, _)| at.total_cmp(&presented))
                    .unwrap();
                publications[index].1
            })
            .collect();
        frames.dedup();
        let unique = frames.len() as f64 / steady.len() as f64;
        assert!(
            steady.len() >= 350,
            "offset {offset}: {} claims",
            steady.len()
        );
        assert!(unique >= 0.95, "offset {offset}: {unique} unique share");
    }
}
#[test]
fn phase_retention_counts_stream_cycles_instead_of_capture_bursts() {
    let start = Instant::now();
    let period = Duration::from_millis(16);
    let mut phase = SourcePhase::new(period);
    for frame in 0..64 {
        phase.observe(start + period * frame);
        if frame % 2 == 0 {
            phase.observe(start + period * frame + period / 2);
        }
    }
    assert!(phase.center.is_some());
    let last = start + period * 63;
    // Several off-phase captures within one stream period are not a phase jump.
    for offset in 1..=8 {
        phase.observe(last + Duration::from_millis(offset + 3));
        assert!(phase.center.is_some());
    }
    for frame in 1..=3 {
        phase.observe(last + period * frame + period / 2);
    }
    assert!(phase.center.is_none());

    // Many in-phase samples in one cycle cannot support mostly empty cycles.
    phase = SourcePhase::new(period);
    for frame in 0..8 {
        for burst in 0..8 {
            phase.observe(start + period * (frame * 3) + Duration::from_micros(burst * 100));
        }
    }
    assert!(!phase.covers_cycles(0));
}
#[test]
fn phase_override_preserves_default_decisions_without_clear_capture_surplus() {
    let start = Instant::now();
    for prediction in [false, true] {
        for (stream_hz, observed_hz) in [
            (60., 60.),
            (60., 59.94),
            (59.94, 60.),
            (120., 119.998),
            (120., 121.),
            (60., 49.),
            (60., 30.),
        ] {
            let period = Duration::from_secs_f64(1. / stream_hz);
            let source_interval = Duration::from_secs_f64(1. / observed_hz);
            let mut normal = Pacer::new(start, period).with_prediction(prediction);
            let mut learned = Pacer::new(start, period)
                .with_prediction(prediction)
                .with_source_phase(true);
            for frame in 0..256 {
                let presented = start + source_interval * frame;
                learned.observe_source(presented);
                assert!(!learned.source_phase.as_ref().unwrap().has_surplus());
                normal.claimed(presented);
                learned.claimed(presented);
                // Exercise waiting as well as immediate claims. With a stable
                // learned phase these off-phase candidates used to change the
                // next deadline despite there being no surplus to correct.
                for fraction in [0.125, 0.25, 0.5, 0.75, 1.] {
                    let at = presented + period.mul_f64(fraction);
                    for interval in [None, Some(period), Some(source_interval)] {
                        assert_eq!(
                            learned.decide(at, at, interval),
                            normal.decide(at, at, interval),
                            "stream {stream_hz}, captures {observed_hz}, frame {frame}"
                        );
                    }
                }
            }
        }
    }
}
#[test]
fn phase_surplus_gate_excludes_slow_sources_with_extra_compositions() {
    let start = Instant::now();
    let period = Duration::from_secs_f64(1. / 60.);
    let mut phase = SourcePhase::new(period);
    for frame in 0..128 {
        // A 30 Hz source with 15 extra compositions per second has a
        // dominant phase but still supplies fewer than 60 updates/second.
        phase.observe(start + period * (frame * 2));
        if frame % 2 == 0 {
            phase.observe(start + period * (frame * 2) + period / 2);
        }
        assert!(!phase.has_surplus());
    }
    assert!(phase.center.is_some());
}
#[test]
fn phase_surplus_gate_turns_off_after_extra_compositions_stop() {
    let start = Instant::now();
    let period = Duration::from_millis(16);
    let mut phase = SourcePhase::new(period);
    for frame in 0..80 {
        phase.observe(start + period * frame);
        if frame % 4 == 0 {
            phase.observe(start + period * frame + period / 2);
        }
    }
    assert!(phase.has_surplus());
    assert!(phase.center.is_some());
    for frame in 80..144 {
        phase.observe(start + period * frame);
    }
    assert!(!phase.has_surplus());
    assert!(phase.center.is_some());
}
#[test]
fn phase_prediction_falls_back_for_irregular_faster_and_slower_sources() {
    let start = Instant::now();
    let period = Duration::from_millis(16);
    let mut phase = SourcePhase::new(period);
    for frame in 0..128u32 {
        phase.observe(start + period * frame + period.mul_f64(f64::from(frame * 17 % 64) / 64.));
    }
    assert!(phase.center.is_none());
    for prediction in [false, true] {
        for interval in [period / 2, period, period * 2] {
            let mut normal = Pacer::new(start, period).with_prediction(prediction);
            let mut learned = Pacer::new(start, period)
                .with_prediction(prediction)
                .with_source_phase(true);
            for frame in 0..128 {
                let at = start + interval * frame;
                learned.observe_source(at);
                assert_eq!(
                    learned.decide(at, at, Some(interval)),
                    normal.decide(at, at, Some(interval))
                );
                normal.claimed(at);
                learned.claimed(at);
            }
        }
        let mut normal = Pacer::new(start, period).with_prediction(prediction);
        let mut learned = Pacer::new(start, period)
            .with_prediction(prediction)
            .with_source_phase(true);
        for frame in 0..128u32 {
            let at = start + period * frame / 2 + period * (frame * 17 % 64) / 128;
            learned.observe_source(at);
            assert!(learned.source_phase.as_ref().unwrap().center.is_none());
            assert_eq!(learned.decide(at, at, None), normal.decide(at, at, None));
            normal.claimed(at);
            learned.claimed(at);
        }
    }
    phase.observe(start + period * 200);
    assert_eq!(phase.count, 1);
    assert!(phase.center.is_none());
}
#[test]
fn pacing_defaults_to_arrival_and_keeps_the_grid_option() {
    assert_eq!(Pacing::from_config(&Config::default()), Pacing::Arrival);
    let grid = Config::parse("frame_pacing = grid\n").unwrap();
    assert_eq!(Pacing::from_config(&grid), Pacing::Grid);
}
#[test]
fn late_desktop_updates_keep_the_waiting_slot_and_static_resumes_do_not_burst() {
    let start = Instant::now();
    let period = Duration::from_millis(16);
    let mut cadence = Cadence::new(start, period, true);
    cadence.submitted(start);
    // No image at the next slot: checking it must not postpone a frame
    // arriving just afterward until another full refresh interval.
    assert_eq!(cadence.deadline(), start + period);
    let update = start + period + Duration::from_micros(250);
    assert!(cadence.deadline() <= update);
    cadence.submitted(update);
    assert_eq!(cadence.deadline(), start + period * 2);
    let resumed = start + Duration::from_millis(100);
    assert!(cadence.deadline() <= resumed);
    cadence.submitted(resumed);
    assert_eq!(cadence.deadline(), resumed + period);
}
#[test]
fn scheduler_overshoot_does_not_accumulate_or_allow_catch_up_submissions() {
    let start = Instant::now();
    let period = Duration::from_millis(8);
    let mut cadence = Cadence::new(start, period, true);
    for frame in 0..1000 {
        let submitted = start + period * frame + Duration::from_micros(300);
        cadence.submitted(submitted);
        assert_eq!(cadence.deadline(), start + period * (frame + 1));
    }
    let missed = cadence.deadline() + period;
    cadence.submitted(missed);
    assert_eq!(cadence.deadline(), missed + period);
    let mut unsmoothed = Cadence::new(start, period, false);
    unsmoothed.submitted(start + Duration::from_micros(300));
    assert_eq!(
        unsmoothed.deadline(),
        start + period + Duration::from_micros(300)
    );
}
#[test]
fn phase_locked_cadence_follows_the_client_period_and_falls_back_without_reports() {
    use crate::phase_sync::PhaseSync;
    use rubylight_protocol::phase_lock::{PhaseLock, Report};
    // A fake clock: every instant is the start plus an exact offset.
    let start = Instant::now();
    let nominal = Duration::from_nanos(8_333_333); // the host's 120.00 Hz
    let client_ns = 8_331_945u32; // a phone at 120.02 Hz
    let at_margin = Report {
        frames: 60,
        period_ns: client_ns,
        lead_ns: PhaseLock::DEFAULT_MARGIN_NS as i32,
        spread_ns: 0,
    }
    .encode();
    let mut sync = PhaseSync::default();
    let mut cadence = Cadence::new(start, nominal, true);
    let mut now = start;
    for frame in 0..1200u32 {
        // A report every 60 frames (about 500 ms) whose lead is already on target.
        if frame % 60 == 0 {
            assert!(sync.on_payload(now, &at_margin).is_some());
        }
        let due = cadence.deadline();
        cadence.submitted_after(now, sync.interval(now, nominal).unwrap());
        if frame > 0 {
            assert_eq!(
                cadence.deadline() - due,
                Duration::from_nanos(u64::from(client_ns))
            );
        }
        // Submitted a little after each slot; the overshoot must not accumulate.
        now = cadence.deadline() + Duration::from_micros(300);
    }
    // Ten seconds at the client's rate: 1.67 ms of drift that the host's own period
    // would have added to every frame's wait at the client.
    let locked_span = cadence.deadline() - start;
    assert_eq!(
        locked_span,
        Duration::from_nanos(u64::from(client_ns)) * 1200
    );
    assert_eq!(
        nominal * 1200 - locked_span,
        Duration::from_nanos(1_388 * 1200)
    );
    // Reports stop: after the timeout the lock is gone and the fixed period applies again.
    now = cadence.deadline() + PhaseSync::TIMEOUT;
    assert_eq!(sync.interval(now, nominal), None);
    cadence.submitted(now);
    for _ in 0..100 {
        let due = cadence.deadline();
        assert_eq!(sync.interval(due, nominal), None);
        cadence.submitted(due + Duration::from_micros(300));
        assert_eq!(cadence.deadline() - due, nominal);
    }
}
#[test]
fn without_reports_cadence_deadlines_match_the_fixed_period_exactly() {
    use crate::phase_sync::PhaseSync;
    // The cadence before phase lock, kept here as the reference.
    fn previous(due: Instant, period: Duration, smooth: bool, now: Instant) -> Instant {
        let anchored = due + period;
        if smooth && anchored > now {
            anchored
        } else {
            now + period
        }
    }
    let start = Instant::now();
    for smooth in [true, false] {
        for period in [
            Duration::from_nanos(8_333_333),
            Duration::from_nanos(16_666_667),
            Duration::from_millis(4),
        ] {
            let mut sync = PhaseSync::default();
            let mut cadence = Cadence::new(start, period, smooth);
            let mut reference = start;
            for frame in 0..1000u32 {
                // Uneven submissions: on time, late, and after a static pause.
                let late = Duration::from_micros(u64::from(frame % 7) * 400);
                let now = if frame % 97 == 0 {
                    cadence.deadline() + period * 5
                } else {
                    cadence.deadline() + late
                };
                // What the host does without a lock: no interval, the original call.
                assert_eq!(sync.interval(now, period), None);
                cadence.submitted(now);
                reference = previous(reference, period, smooth, now);
                assert_eq!(cadence.deadline(), reference, "frame {frame}");
            }
        }
    }
}
#[test]
fn without_reports_pacer_claims_match_an_untouched_pacer() {
    use crate::phase_sync::PhaseSync;
    let start = Instant::now();
    let period = Duration::from_nanos(8_333_333);
    let mut sync = PhaseSync::default();
    let mut paced = Pacer::new(start, period);
    let mut untouched = Pacer::new(start, period);
    for frame in 0..1000u32 {
        let now = start + period * frame + Duration::from_micros(u64::from(frame % 5) * 700);
        if let Some(interval) = sync.interval(now, period) {
            paced.set_period(interval);
        }
        assert_eq!(
            paced.allowed_at(now),
            untouched.allowed_at(now),
            "frame {frame}"
        );
        paced.claimed(now);
        untouched.claimed(now);
    }
}
#[test]
fn video_packets_always_use_the_device_request() {
    // #52: packetsize=1200 replaced the 1184 a remote IPv6 Moonlight asked for, so every
    // video packet was cut short in its receive buffer; a smaller size breaks the packets
    // Moonlight rebuilds from FEC at its own size.
    let warnings = crate::session::Warnings::default();
    for text in [
        "packetsize=1200
",
        "packetsize=512
",
        "packetsize=1401
",
    ] {
        let config = Config::parse(text).unwrap();
        for requested in [1184, 1024, 1392] {
            let mut stream = Negotiated {
                packet_size: requested,
                ..Default::default()
            };
            apply(&mut stream, 0, &config);
            assert_eq!(stream.packet_size, requested, "{text:?}");
        }
        report_packet_size(&warnings, &config);
        assert!(!warnings.snapshot().is_empty(), "{text:?}");
    }
    for text in [
        "",
        "packetsize=0
",
    ] {
        report_packet_size(&warnings, &Config::parse(text).unwrap());
        assert!(warnings.snapshot().is_empty(), "{text:?}");
    }
}
#[test]
fn configured_wire_budget_deducts_fec_and_audio_after_the_ceiling() {
    let mut stream = Negotiated {
        fps: 120,
        configured_bitrate_kbps: 20000,
        audio_channels: 6,
        ..Default::default()
    };
    apply(&mut stream, 59940, &Config::default());
    // The stream keeps its request's rate; see a_stream_runs_at_the_rate_its_request_asks_for.
    assert_eq!(stream.fps_millihz(), 120_000);
    assert_eq!(stream.bitrate_kbps, 14924);
    let config = Config::parse("max_bitrate=15000\n").unwrap();
    apply(&mut stream, 59940, &config);
    assert_eq!(stream.bitrate_kbps, 10924);
    let mut legacy = Negotiated {
        bitrate_kbps: 20000,
        ..Default::default()
    };
    apply(
        &mut legacy,
        0,
        &Config::parse("max_bitrate=25000\n").unwrap(),
    );
    assert_eq!(legacy.bitrate_kbps, 20000);
    let mut pyro = Negotiated {
        fps: 120,
        codec: 3,
        configured_bitrate_kbps: 800000,
        ..Default::default()
    };
    apply(&mut pyro, 120000, &Config::default());
    // Audio with its 50% parity and control, without generic 20% FEC, as
    // Vibeshine takes them off a PyroWave total: 192 * 3 / 2 + 500.
    assert_eq!(pyro.bitrate_kbps, 799212);
}
#[test]
fn a_stream_runs_at_the_rate_its_request_asks_for() {
    // Artemis's Warp launches at 60 and asks for 120 or 240; Moonlight for
    // Android launches at 120 and asks for 119 on a 120 Hz panel.
    for (launch, request, expected) in [
        (60_000, 60, 60_000),
        (60_000, 120, 120_000),
        (60_000, 240, 240_000),
        (120_000, 119, 119_000),
        // A launch without a rate leaves the request's.
        (0, 120, 120_000),
    ] {
        let mut stream = artemis_announce(request);
        apply(&mut stream, launch, &Config::default());
        assert_eq!(
            stream.fps_millihz(),
            expected,
            "launch {launch}, request {request}"
        );
    }
    // "Use the launch frame rate" keeps the previous behaviour on request.
    let limited = Config::parse("limit_framerate=true\n").unwrap();
    for request in [120, 240] {
        let mut stream = artemis_announce(request);
        apply(&mut stream, 60_000, &limited);
        assert_eq!(stream.fps_millihz(), 60_000, "request {request}");
        assert_eq!(stream.bitrate_kbps, 31_308);
    }
    let mut stream = artemis_announce(120);
    apply(&mut stream, 0, &limited);
    assert_eq!(stream.fps_millihz(), 120_000);
}
/// The ANNOUNCE Artemis sends with its 40 Mbps setting: Warp Drive and Warp 2
/// frame pacing multiply the stream rate (maxFPS) by two and four but launch
/// at 60 fps, and the TV's 60 Hz refresh does not match the request.
fn artemis_announce(max_fps: u32) -> Negotiated {
    Negotiated::from_sdp(
        format!(
            "a=x-nv-video[0].clientViewportWd:2560\na=x-nv-video[0].clientViewportHt:1440\n\
             a=x-nv-video[0].maxFPS:{max_fps}\na=x-nv-video[0].clientRefreshRateX100:6000\n\
             a=x-nv-vqos[0].bw.maximumBitrateKbps:31500\na=x-ml-video.configuredBitrateKbps:40000\n\
             a=x-nv-vqos[0].bitStreamFormat:2\na=x-nv-video[0].packetSize:1392\n"
        )
        .as_bytes(),
    )
    .unwrap()
}
#[test]
fn a_warp_stream_request_keeps_the_clients_bitrate() {
    let config = Config::default();
    let mut plain = artemis_announce(60);
    apply(&mut plain, 60_000, &config);
    assert_eq!(plain.bitrate_kbps, 31_308);
    for warp in [2, 4] {
        let mut warped = artemis_announce(60 * warp);
        let uncapped = uncapped_bitrate_kbps(&warped, 60_000, &config);
        apply(&mut warped, 60_000, &config);
        assert_eq!(warped.bitrate_kbps, plain.bitrate_kbps, "warp {warp}");
        assert_eq!(uncapped, plain.bitrate_kbps, "warp {warp}");
    }
    // A host ceiling still applies to the client's bitrate.
    let mut capped = artemis_announce(240);
    apply(
        &mut capped,
        60_000,
        &Config::parse("max_bitrate=30000\n").unwrap(),
    );
    assert_eq!(capped.bitrate_kbps, 23_308);
}
#[derive(Debug)]
struct ClientPort {
    unrecoverable_frames: usize,
    lost_packets: usize,
    /// The queue left behind the last frame: what a control ping waits behind.
    standing_queue: Duration,
}
/// A deterministic model, not a measurement: a 1 Gbps host feeding a client's
/// 100 Mbps Ethernet port (the Onn 4K Pro's is 10/100) through a tail-drop
/// queue of `queue_bytes`. Ten seconds of 60 fps frames at the encoder's
/// per-frame budget, an 8x keyframe every five seconds, 20% FEC and
/// encryption, packetized and paced as the host sends them.
fn client_port(encoder_kbps: u32, pacing_kbps: i64, queue_bytes: usize) -> ClientPort {
    use crate::network_pacing::{
        Pacer, batch_payload_limit, overhead, rate_bps, video_wire_rate_bps,
    };
    let packetizer = crate::packet::VideoPacketizer {
        sequence: 0,
        iv_counter: 0,
        frame: 1,
        packet_size: 1392,
        fec_percent: 20,
        min_fec: 2,
        key: Some([0; 16]),
    };
    let period = Duration::from_secs_f64(1. / 60.);
    let needed = video_wire_rate_bps(&packetizer, encoder_kbps, period, false);
    let bps = rate_bps(pacing_kbps, encoder_kbps, 1_000_000_000, false, needed);
    let datagram = packetizer.packet_size + 16 + 32;
    let per_batch = (batch_payload_limit(bps, datagram, false, 64 * 1024) / datagram).max(1);
    let wire = |bps: f64| Duration::from_secs_f64((datagram + overhead(false)) as f64 * 8. / bps);
    let (host_time, port_time) = (wire(1e9), wire(100e6));
    let capacity = Duration::from_secs_f64(queue_bytes as f64 * 8. / 100e6);
    let start = Instant::now();
    let mut pacer = Pacer::new(start);
    let (mut now, mut drained_at) = (start, start);
    let mut result = ClientPort {
        unrecoverable_frames: 0,
        lost_packets: 0,
        standing_queue: Duration::ZERO,
    };
    for frame in 0..600u32 {
        now = now.max(start + period * frame);
        let payload = (f64::from(encoder_kbps) * 125. * period.as_secs_f64()).ceil() as usize
            * if frame % 300 == 0 { 8 } else { 1 };
        let mut failed = false;
        for (data, fec) in packetizer.block_layout(payload) {
            let (mut lost_data, mut parity) = (0, 0);
            let mut sent = 0;
            while sent < data + fec {
                now = now.max(pacer.due());
                let count = (data + fec - sent).min(per_batch);
                for index in sent..sent + count {
                    now += host_time;
                    let done = drained_at.max(now) + port_time;
                    if done.saturating_duration_since(now) <= capacity {
                        drained_at = done;
                        parity += usize::from(index >= data);
                    } else {
                        result.lost_packets += 1;
                        lost_data += usize::from(index < data);
                    }
                }
                pacer.sent(now, count * datagram, count, false, bps);
                sent += count;
            }
            failed |= lost_data > parity;
        }
        result.unrecoverable_frames += usize::from(failed);
    }
    result.standing_queue = drained_at.saturating_duration_since(now);
    result
}
#[test]
fn warp_requests_at_the_clients_bitrate_fit_a_100_mbps_client_port() {
    // 1 MiB holds about 84 ms at 100 Mbps, near the 78-80 ms round trip an
    // Onn client on Ethernet reported; 16 KiB is a shallow switch queue.
    let deep = 1024 * 1024;
    for warp in [1, 2, 4] {
        let mut stream = artemis_announce(60 * warp);
        apply(&mut stream, 60_000, &Config::default());
        for queue in [deep, 16 * 1024] {
            let capped = client_port(stream.bitrate_kbps, 80_000, queue);
            assert_eq!(capped.lost_packets, 0, "warp {warp}, {queue} B: {capped:?}");
            assert_eq!(capped.unrecoverable_frames, 0);
            assert!(capped.standing_queue < Duration::from_millis(15));
        }
        // The default 800 Mbps bursts need the deep queue, but fit it.
        let default = client_port(stream.bitrate_kbps, 0, deep);
        assert_eq!(default.unrecoverable_frames, 0, "warp {warp}: {default:?}");
    }
    // Negative control, the previous policy: Warp 2 multiplied the 40 Mbps
    // budget by four. About 165 Mbps cannot fit the port: the queue stays
    // full, as an 80 ms round trip on a LAN shows, and nearly every frame dies.
    let previous = 40_000 * 4 * 80 / 100 - 192 - 500;
    let overloaded = client_port(previous, 0, deep);
    assert!(overloaded.unrecoverable_frames > 540, "{overloaded:?}");
    assert!(overloaded.standing_queue > Duration::from_millis(75));
}
/// Claims (ms, true present ms) for a source pacing on the given stamps, as
/// the host paces WGC: `arrival` stamps each frame when it was detected, as
/// `qpc_timestamps_at` does for the many WGC stamps still in the future.
fn simulate_stamped(period: f64, interval: f64, seconds: f64, arrival: bool) -> Vec<(f64, f64)> {
    let start = Instant::now();
    let at = |ms: f64| start + Duration::from_secs_f64(ms / 1000.);
    let ms = |instant: Instant| instant.duration_since(start).as_secs_f64() * 1000.;
    let mut pacer = Pacer::new(start, Duration::from_secs_f64(period / 1000.))
        .with_prediction(true)
        .with_source_phase(true)
        .with_spacing(0.75);
    let mut seed = 42u64;
    let mut jitter = || {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (seed >> 11) as f64 / (1u64 << 53) as f64
    };
    let present = source(interval, seconds, |_| 0.);
    // Detected 0.1-1.1 ms after presentation, as the polled WGC helper.
    let detected: Vec<f64> = present.iter().map(|p| p + 0.1 + jitter()).collect();
    let stamp = if arrival { &detected } else { &present };
    let mut claims = vec![];
    let mut newest: Option<usize> = None;
    let mut deadline: Option<f64> = None;
    let mut source_interval: Option<Duration> = None;
    let mut evaluate = |now: f64,
                        newest: &mut Option<usize>,
                        deadline: &mut Option<f64>,
                        pacer: &mut Pacer,
                        source_interval: Option<Duration>| {
        if let Some(i) = *newest {
            pacer.observe_source(at(stamp[i]));
            match pacer.decide(at(now), at(stamp[i]), source_interval) {
                Pace::Claim => {
                    pacer.claimed(at(now));
                    claims.push((now, present[i]));
                    *newest = None;
                    *deadline = None;
                }
                Pace::WaitUntil(t) => *deadline = Some(ms(t).max(now + 0.001)),
            }
        }
    };
    for i in 0..present.len() {
        while let Some(t) = deadline.filter(|t| *t < detected[i]) {
            evaluate(t, &mut newest, &mut deadline, &mut pacer, source_interval);
            if deadline == Some(t) {
                break;
            }
        }
        let recent = &stamp[i.saturating_sub(16)..=i];
        let mut intervals: Vec<f64> = recent.windows(2).map(|w| w[1] - w[0]).collect();
        intervals.sort_by(f64::total_cmp);
        source_interval = (intervals.len() >= 4)
            .then(|| Duration::from_secs_f64(intervals[intervals.len() / 2] / 1000.));
        newest = Some(i);
        evaluate(
            detected[i],
            &mut newest,
            &mut deadline,
            &mut pacer,
            source_interval,
        );
    }
    claims
}
fn steady_rate(claims: &[(f64, f64)]) -> f64 {
    let steady = &claims[claims.len() / 10..];
    (steady.len() - 1) as f64 * 1000. / (steady[steady.len() - 1].0 - steady[0].0)
}
fn steady_age_p99(claims: &[(f64, f64)]) -> f64 {
    let mut ages: Vec<f64> = claims[claims.len() / 10..]
        .iter()
        .map(|(claim, presented)| claim - presented)
        .collect();
    ages.sort_by(f64::total_cmp);
    ages[ages.len() * 99 / 100]
}
#[test]
fn a_faster_source_is_claimed_at_the_stream_rate_not_above_it() {
    // A 165 Hz game, and the default 2x virtual display, at 120 and 60 fps.
    // The 1% refill margin used to set the claim rate: 121.2 and 60.6 a
    // second, as measured on the host.
    for (period, interval, limit) in [
        (PERIOD, 1000. / 165., 120.2),
        (PERIOD, 1000. / 240., 120.2),
        (1000. / 60., 1000. / 120., 60.1),
    ] {
        for arrival in [false, true] {
            let claims = simulate_stamped(period, interval, 30., arrival);
            let rate = steady_rate(&claims);
            assert!(
                rate <= limit,
                "{rate:.2} claims/s for a {:.0} Hz source (arrival stamps: {arrival})",
                1000. / interval
            );
        }
        if interval * 2. != period {
            continue;
        }
        // On the 2x display, every other source frame lands on the claim.
        let exact = steady_age_p99(&simulate_stamped(period, interval, 30., false));
        let arrival = steady_age_p99(&simulate_stamped(period, interval, 30., true));
        assert!(
            arrival <= exact + 0.2,
            "picture age p99 {arrival:.2} ms against {exact:.2} ms"
        );
    }
}

#[test]
fn test_send_outage_follows_its_schedule_and_rejects_bad_values() {
    let outage = SendOutage::parse(" 1500 : 80 ").unwrap();
    let start = Instant::now();
    let at = |ms: u64| start + Duration::from_millis(ms);
    // Nothing during the first interval, so the stream starts cleanly.
    assert!(!outage.active(start, at(0)));
    assert!(!outage.active(start, at(1499)));
    assert!(outage.active(start, at(1500)));
    assert!(outage.active(start, at(1579)));
    assert!(!outage.active(start, at(1580)));
    assert!(outage.active(start, at(3020)));
    assert!(!outage.active(start, at(2999)));
    assert!(!outage.in_air());
    let air = SendOutage::parse("1500:80:air").unwrap();
    assert!(air.in_air());
    assert!(air.active(start, at(1500)));
    assert!(!air.active(start, at(1580)));
    for bad in [
        "",
        "80",
        "80:80",
        "10:0",
        "x:5",
        "1500:-1",
        "1500:80:1",
        "1500:80:air:1",
        "80:80:air",
    ] {
        assert_eq!(SendOutage::parse(bad), None, "{bad}");
    }
}

#[test]
fn send_loss_is_unrecoverable_only_beyond_a_blocks_parity() {
    // Two blocks of 10 data and 2 parity packets, sent in batches of 8.
    let mut loss = SendLoss::new(vec![(10, 2), (10, 2)]);
    assert!(!loss.sent(8, 0));
    // 2 refused within the first block: FEC still repairs it.
    assert!(!loss.sent(4, 2));
    // Packets 12-19 belong to the second block only.
    assert!(!loss.sent(8, 2));
    assert!(loss.sent(4, 1));
    assert!(loss.unrecoverable());

    // A whole batch refused across both blocks loses the frame.
    let mut loss = SendLoss::new(vec![(10, 2), (10, 2)]);
    assert!(loss.sent(24, 24));

    // A batch that spans the boundary is charged to both blocks.
    let mut loss = SendLoss::new(vec![(10, 2), (10, 2)]);
    assert!(!loss.sent(10, 0));
    assert!(!loss.sent(4, 2));
    assert!(loss.sent(4, 1));

    // Without parity any refused packet loses the frame.
    let mut loss = SendLoss::new(vec![(3, 0)]);
    assert!(!loss.sent(2, 0));
    assert!(loss.sent(1, 1));

    // Batches beyond the layout, as with a stale layout, change nothing.
    let mut loss = SendLoss::new(vec![(2, 1)]);
    assert!(!loss.sent(3, 0));
    assert!(!loss.sent(5, 5));
}

#[test]
fn send_loss_keyframes_back_off_only_when_part_of_a_frame_got_through() {
    let start = Instant::now();
    let period = Duration::from_micros(8333);
    let at = |ms: u64| start + Duration::from_millis(ms);
    let mut recovery = SendLossRecovery::default();
    // A radio outage: every frame refused from its first packet. Each next
    // frame is a keyframe, so the first one through after it recovers.
    for ms in [0, 8, 16, 25, 33, 41, 50, 58, 66, 75, 83] {
        assert!(recovery.lost(at(ms), period, false), "{ms}");
    }
    // Congestion: the forced keyframe got partly in and was lost anyway.
    assert!(!recovery.lost(at(91), period, true));
    assert!(!recovery.lost(at(122), period, true));
    // After the hold another loss starts one again.
    assert!(recovery.lost(at(123), period, true));
    // A keyframe sent whole ends the hold; a P-frame does not.
    recovery.delivered(false);
    assert!(!recovery.lost(at(130), period, true));
    recovery.delivered(true);
    assert!(recovery.lost(at(131), period, true));
    // At 30 fps the hold is four periods.
    let mut slow = SendLossRecovery::default();
    let period = Duration::from_micros(33_333);
    assert!(slow.lost(at(0), period, true));
    assert!(!slow.lost(at(130), period, true));
    assert!(slow.lost(at(134), period, true));
}

#[test]
fn send_loss_knows_whether_any_packet_reached_the_socket() {
    let mut loss = SendLoss::new(vec![(10, 2)]);
    assert!(loss.sent(8, 8));
    assert!(!loss.reached_socket());
    let mut loss = SendLoss::new(vec![(10, 2)]);
    assert!(!loss.sent(8, 0));
    assert!(loss.sent(4, 4));
    assert!(loss.reached_socket());
}

#[test]
fn streamed_send_loss_matches_the_whole_frame_layout() {
    // Blocks are added as they are built, a batch never spans an unbuilt one.
    let layout = vec![(30, 6), (30, 6), (20, 4), (8, 2)];
    let mut seed = 99u64;
    let mut random = move |range: usize| {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (seed >> 33) as usize % range
    };
    for _ in 0..500 {
        let mut whole = SendLoss::new(layout.clone());
        let mut streamed = SendLoss::streamed();
        assert!(!streamed.unrecoverable() && !streamed.reached_socket());
        for &(data, parity) in &layout {
            streamed.push_block(data, parity);
            let mut left = data + parity;
            while left > 0 {
                let batch = 1 + random(left.min(10));
                let dropped = if random(4) == 0 { random(batch + 1) } else { 0 };
                assert_eq!(
                    whole.sent(batch, dropped),
                    streamed.sent(batch, dropped),
                    "{layout:?}"
                );
                assert_eq!(whole.reached_socket(), streamed.reached_socket());
                left -= batch;
            }
        }
        assert_eq!(whole.unrecoverable(), streamed.unrecoverable());
    }
    // The first block lost beyond its parity ends the frame before the rest
    // of it is built.
    let mut streamed = SendLoss::streamed();
    streamed.push_block(4, 1);
    assert!(streamed.sent(5, 2));
}

#[test]
fn test_send_loss_is_seeded_per_datagram_and_rejects_bad_values() {
    let mut loss = SendDrop::parse(" 1 : 42 ").unwrap();
    assert!(!loss.in_air());
    let mut again = SendDrop::parse("1:42").unwrap();
    let mut other_seed = SendDrop::parse("1:43").unwrap();
    let first: Vec<bool> = (0..200_000).map(|_| loss.drops()).collect();
    let second: Vec<bool> = (0..200_000).map(|_| again.drops()).collect();
    let third: Vec<bool> = (0..200_000).map(|_| other_seed.drops()).collect();
    assert_eq!(first, second);
    assert_ne!(first, third);
    // About 1% of 200,000.
    let lost = first.iter().filter(|lost| **lost).count();
    assert!((1_700..2_300).contains(&lost), "{lost} lost");
    let mut half = SendDrop::parse("0.5:7:air").unwrap();
    assert!(half.in_air());
    let lost = (0..200_000).filter(|_| half.drops()).count();
    assert!((800..1_200).contains(&lost), "{lost} lost");
    let mut all = SendDrop::parse("100:1").unwrap();
    assert!((0..1000).all(|_| all.drops()));
    for bad in [
        "",
        "1",
        "0:1",
        "-1:1",
        "101:1",
        "x:1",
        "1:x",
        "1:1:1",
        "1:1:air:1",
    ] {
        assert_eq!(SendDrop::parse(bad), None, "{bad}");
    }
}
#[test]
fn a_faster_source_repays_a_startup_deficit_instead_of_lagging_every_claim() {
    // The 2x virtual display at 60 fps. A slow start left the credit short;
    // refilling at exactly the stream rate, every later claim then waited the
    // same 2-3 ms for credit beside a frame already in hand (October 15,
    // idd-push at 60 fps). A bounded grace now repays it.
    let period = 1000. / 60.;
    for burst in [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0] {
        let claims = simulate_with(
            period,
            &source(period / 2., 30., |i| 0.05 * ((i * 7919) % 5) as f64),
            |_| 0.1,
            |mut pacer| {
                let now = Instant::now();
                for ms in [burst, burst + 3., burst + 6.] {
                    pacer.claimed(now + Duration::from_secs_f64(ms / 1000.));
                }
                pacer
            },
        );
        let late: Vec<f64> = claims[claims.len() / 3..]
            .iter()
            .map(|(claim, presented)| claim - presented)
            .collect();
        let mean = late.iter().sum::<f64>() / late.len() as f64;
        assert!(
            mean <= 0.4,
            "claims lag {mean:.2} ms after a burst at {burst} ms"
        );
        let rate = steady_rate(&claims);
        assert!(rate <= 60.1, "{rate:.2} claims/s");
    }
}
#[test]
fn only_wgc_captures_wait_for_the_learned_source_phase() {
    let config = |text: &str| Config::parse(text).unwrap();
    assert!(source_phase(&config(""), "wgc"));
    // Rubylight's display driver pushes every composition: waiting for the
    // next one under GPU load held claims up to 1.25 periods.
    assert!(!source_phase(&config(""), "idd-push"));
    assert!(!source_phase(&config(""), "ddx"));
    assert!(source_phase(
        &config("frame_pacing_source_phase=true"),
        "idd-push"
    ));
    assert!(!source_phase(
        &config("frame_pacing_source_phase=false"),
        "wgc"
    ));
    // Capture `auto` is planned as WGC; the backend that opens decides.
    let start = Instant::now();
    let period = Duration::from_micros(8_333);
    let mut pacer = Pacer::new(start, period).with_source_phase(true);
    pacer.set_source_phase(false);
    assert!(pacer.source_phase.is_none());
    pacer.set_source_phase(true);
    assert!(pacer.source_phase.is_some());
    for n in 0..48 {
        pacer.observe_source(start + period / 2 * n);
    }
    let learned = pacer.source_phase.as_ref().map(|phase| phase.center);
    // The same backend again keeps what was learned.
    pacer.set_source_phase(true);
    assert_eq!(
        pacer.source_phase.as_ref().map(|phase| phase.center),
        learned
    );
}

/// Claims (ms, detected ms) for Rubylight's display driver on a 240 Hz
/// virtual display whose compositor alternates at random between one and two
/// refreshes per picture, as measured on October 16: about 160 frames a
/// second with a median interval of either 4.2 or 8.3 ms.
fn simulate_alternating(rate_known: bool) -> Vec<(f64, f64)> {
    let start = Instant::now();
    let at = |ms: f64| start + Duration::from_secs_f64(ms / 1000.);
    let ms = |instant: Instant| instant.duration_since(start).as_secs_f64() * 1000.;
    let mut pacer = Pacer::new(start, Duration::from_secs_f64(PERIOD / 1000.))
        .with_prediction(true)
        .with_source_phase(false)
        .with_spacing(0.75);
    let mut seed = 7u64;
    let mut coin = || {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        // Slightly more double refreshes, so the median sits on 8.3 ms.
        (seed >> 33) % 100 < 55
    };
    let refresh = 1000. / 240.;
    let mut present = vec![0.];
    while present[present.len() - 1] < 30_000. {
        let steps = if coin() { 2. } else { 1. };
        present.push(present[present.len() - 1] + refresh * steps);
    }
    let detected: Vec<f64> = present.iter().map(|p| p + 0.15).collect();
    let mut claims = vec![];
    let mut newest: Option<usize> = None;
    let mut deadline: Option<f64> = None;
    let (mut median, mut mean) = (None, None);
    let mut evaluate = |now: f64,
                        newest: &mut Option<usize>,
                        deadline: &mut Option<f64>,
                        pacer: &mut Pacer,
                        median: Option<Duration>,
                        mean: Option<Duration>| {
        if let Some(i) = *newest {
            if rate_known {
                pacer.observe_source_rate(mean);
            }
            match pacer.decide(at(now), at(present[i]), median) {
                Pace::Claim => {
                    pacer.claimed(at(now));
                    claims.push((now, detected[i]));
                    *newest = None;
                    *deadline = None;
                }
                Pace::WaitUntil(t) => *deadline = Some(ms(t).max(now + 0.001)),
            }
        }
    };
    for i in 0..present.len() {
        while let Some(t) = deadline.filter(|t| *t < detected[i]) {
            evaluate(t, &mut newest, &mut deadline, &mut pacer, median, mean);
            if deadline == Some(t) {
                break;
            }
        }
        let recent = &present[i.saturating_sub(16)..=i];
        let mut intervals: Vec<f64> = recent.windows(2).map(|w| w[1] - w[0]).collect();
        if intervals.len() >= 4 {
            let average = intervals.iter().sum::<f64>() / intervals.len() as f64;
            mean = Some(Duration::from_secs_f64(average / 1000.));
            intervals.sort_by(f64::total_cmp);
            median = Some(Duration::from_secs_f64(
                intervals[intervals.len() / 2] / 1000.,
            ));
        }
        newest = Some(i);
        evaluate(
            detected[i],
            &mut newest,
            &mut deadline,
            &mut pacer,
            median,
            mean,
        );
    }
    claims
}
#[test]
fn a_source_alternating_between_two_cadences_is_claimed_at_the_stream_rate() {
    let wait = |claims: &[(f64, f64)]| {
        let steady = &claims[claims.len() / 10..];
        steady.iter().map(|(claim, seen)| claim - seen).sum::<f64>() / steady.len() as f64
    };
    let claims = simulate_alternating(true);
    let rate = steady_rate(&claims);
    assert!(rate <= 120.2, "{rate:.2} claims/s");
    // Judged by its median, the source looked no faster than the stream:
    // claims ran at 121 a second, as the host measured, and waited longer.
    let previous = simulate_alternating(false);
    assert!(steady_rate(&previous) > 120.5);
    assert!(wait(&claims) < wait(&previous));
}
/// A client's ten-bit request after RTSP, as `from_sdp` leaves it.
fn ten_bit_request() -> Negotiated {
    Negotiated {
        hdr: true,
        ..Default::default()
    }
}
/// What the launch asked for, then what RTSP negotiated, as the host applies them.
fn negotiate_color(mut stream: Negotiated, sdr_requested: bool, config: &Config) -> Negotiated {
    infer_sdr_10bit(&mut stream, sdr_requested);
    apply_color(&mut stream, config);
    stream
}
#[test]
fn a_ten_bit_request_with_hdr_mode_zero_is_ten_bit_sdr() {
    let mut stream = ten_bit_request();
    assert!(infer_sdr_10bit(&mut stream, true));
    assert!(stream.sdr_10bit && !stream.hdr && stream.ten_bit());
    // `apply_color` after the inference keeps what it inferred, under the
    // default config and with the host option on.
    for config in [
        Config::default(),
        Config::parse("prefer_sdr_10bit=true\n").unwrap(),
    ] {
        let stream = negotiate_color(ten_bit_request(), true, &config);
        assert!(stream.sdr_10bit && !stream.hdr && stream.ten_bit());
    }
    // And the inference is idempotent.
    let mut again = stream.clone();
    assert!(!infer_sdr_10bit(&mut again, true));
    assert!(again.sdr_10bit && !again.hdr);
}
#[test]
fn a_ten_bit_request_without_hdr_mode_zero_stays_hdr() {
    let stream = negotiate_color(ten_bit_request(), false, &Config::default());
    assert!(stream.hdr && !stream.sdr_10bit);
    assert!(!infer_sdr_10bit(&mut ten_bit_request(), false));
}
#[test]
fn prefer_sdr_10bit_still_turns_an_hdr_launch_into_ten_bit_sdr() {
    let config = Config::parse("prefer_sdr_10bit=true\n").unwrap();
    let stream = negotiate_color(ten_bit_request(), false, &config);
    assert!(stream.sdr_10bit && !stream.hdr);
}
#[test]
fn a_stream_turned_into_ten_bit_sdr_drops_its_hdr_format_pick() {
    // An HDR10+ pick the host offered, from a client the host streams as
    // ten-bit SDR: by `prefer_sdr_10bit`, or by `hdrMode=0` at launch.
    let request = Negotiated::from_sdp(
        b"a=x-nv-video[0].dynamicRangeMode:1\na=x-nv-vqos[0].bitStreamFormat:1\na=x-rl-video[0].hdrFormat:2\n",
    )
    .unwrap();
    let offered = crate::extensions::Offered {
        hdr_formats: rubylight_protocol::hdr::ALL,
        ..crate::extensions::Offered::NONE
    };
    offered.check_announce(&request).unwrap();
    let prefer = Config::parse("prefer_sdr_10bit=true\n").unwrap();
    for (sdr_requested, config) in [(true, Config::default()), (false, prefer)] {
        let stream = negotiate_color(request.clone(), sdr_requested, &config);
        assert!(stream.sdr_10bit && !stream.hdr);
        assert_eq!(stream.hdr_format, 0);
        // What is streamed is still something the host offered.
        offered.check_announce(&stream).unwrap();
        stream.validate().unwrap();
    }
    // An HDR stream keeps its pick.
    let stream = negotiate_color(request.clone(), false, &Config::default());
    assert!(stream.hdr && stream.hdr_format == 2);
}
#[test]
fn an_eight_bit_request_is_neither_hdr_nor_ten_bit() {
    let config = Config::parse("prefer_sdr_10bit=true\n").unwrap();
    for sdr_requested in [false, true] {
        let mut stream = Negotiated::default();
        assert!(!infer_sdr_10bit(&mut stream, sdr_requested));
        let stream = negotiate_color(stream, sdr_requested, &config);
        assert!(!stream.hdr && !stream.sdr_10bit && !stream.ten_bit());
    }
}
