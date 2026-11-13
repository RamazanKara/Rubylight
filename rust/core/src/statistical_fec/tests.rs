use super::*;

fn report(seq: u32, sent: u32, missing: u32) -> FecReport {
    FecReport {
        seq,
        data_shards: sent - sent / 6,
        parity_shards: sent / 6,
        missing_shards: missing,
        unrecovered_blocks: 0,
        interval_ms: 1000,
    }
}

/// P(X > k) by summing the upper terms with exact binomial coefficients.
fn tail_reference(n: usize, k: usize, p: f64) -> f64 {
    let mut sum = 0.;
    for i in k + 1..=n {
        let mut c = 1f64;
        for j in 0..i {
            c = c * (n - j) as f64 / (j + 1) as f64;
        }
        sum += c * p.powi(i as i32) * (1. - p).powi((n - i) as i32);
    }
    sum
}

#[test]
fn the_binomial_tail_matches_a_direct_sum() {
    for (n, k, p) in [(10, 2, 0.01), (60, 5, 0.03), (255, 20, 0.05), (40, 0, 0.1)] {
        let (fast, exact) = (tail(n, k, p), tail_reference(n, k, p));
        assert!((fast - exact).abs() < 1e-9, "{n} {k} {p}: {fast} {exact}");
    }
    assert_eq!(tail(10, 10, 0.5), 0.);
    assert_eq!(tail(10, 3, 1.), 1.);
    assert_eq!(tail(10, 3, 0.), 0.);
}

#[test]
fn the_chosen_share_is_the_smallest_under_the_target() {
    let start = Instant::now();
    let mut fec = StatisticalFec::new(50, 20, 0, start);
    fec.report(start, &report(0, 1000, 10)).unwrap();
    assert_eq!(fec.loss(), Some(0.01));
    for shards in [1, 5, 30, 120, 255, 600, 900] {
        let percent = fec.percent(start, shards);
        assert!(
            (MINIMUM_PERCENT as usize..=50).contains(&percent),
            "{shards}: {percent}"
        );
        let check = |percent: usize| {
            let (blocks, applied) = conventional_fec(shards, percent, 0);
            assert_eq!(applied, percent);
            let aligned = shards.div_ceil(blocks);
            (0..blocks).all(|b| {
                let data = shards.saturating_sub(b * aligned).min(aligned);
                let parity = fec_parameters(data, percent, 0).0;
                tail(data + parity, parity, 0.01) <= FRAME_FAILURE_TARGET / blocks as f64
            })
        };
        assert!(check(percent), "{shards}: {percent} misses the target");
        assert!(
            percent == fec.floor() || !check(percent - 1),
            "{shards}: {percent} is not the smallest above the floor"
        );
    }
    // Small frames need a larger share than large ones.
    assert!(fec.percent(start, 5) > fec.percent(start, 255));
}

#[test]
fn heavy_loss_is_capped_by_the_maximum() {
    let start = Instant::now();
    let mut fec = StatisticalFec::new(30, 20, 0, start);
    fec.report(start, &report(0, 600, 300)).unwrap();
    assert_eq!(fec.percent(start, 100), 30);
    // Everything lost: still the cap, not nothing.
    fec.report(start + Duration::from_secs(1), &report(1, 600, 600))
        .unwrap();
    assert_eq!(fec.percent(start + Duration::from_secs(1), 100), 30);
    let none = StatisticalFec::new(0, 20, 0, start);
    assert_eq!(none.select(100), 0);
}

#[test]
fn without_fresh_reports_the_host_percentage_applies() {
    let start = Instant::now();
    let mut fec = StatisticalFec::new(50, 20, 2, start);
    assert_eq!(fec.percent(start, 100), 20, "before the first report");
    fec.report(start, &report(0, 600, 0)).unwrap();
    // The first clean report may drop protection to the floor at once.
    assert_eq!(fec.percent(start, 100), 10);
    assert_eq!(fec.percent(start + FEEDBACK_TIMEOUT, 100), 10);
    let stale = start + FEEDBACK_TIMEOUT + Duration::from_millis(1);
    assert_eq!(fec.percent(stale, 100), 20, "stale");
}

#[test]
fn a_gap_forgets_the_history_and_zero_loss_waits_a_window() {
    let start = Instant::now();
    let mut fec = StatisticalFec::new(50, 20, 0, start);
    fec.report(start, &report(0, 600, 30)).unwrap();
    let lossy = fec.percent(start, 100);
    assert!(lossy > 0);
    // Seq 1 is missing: the loss before the gap no longer counts, and zero
    // loss is trusted only after a full window of continuous reports.
    let mut now = start + Duration::from_secs(1);
    fec.report(now, &report(2, 600, 0)).unwrap();
    assert_eq!(fec.loss(), Some(0.));
    assert_eq!(fec.percent(now, 100), 20);
    for seq in 3..13 {
        now += Duration::from_secs(1);
        fec.report(now, &report(seq, 600, 0)).unwrap();
    }
    assert_eq!(fec.percent(now, 100), fec.floor());
    // Loss after a gap counts at once.
    now += FEEDBACK_TIMEOUT + Duration::from_secs(1);
    fec.report(now, &report(13, 600, 30)).unwrap();
    assert_eq!(fec.percent(now, 100), lossy);
}

#[test]
fn old_loss_leaves_the_window() {
    let start = Instant::now();
    let mut fec = StatisticalFec::new(50, 20, 0, start);
    fec.report(start, &report(0, 600, 60)).unwrap();
    let mut now = start;
    for seq in 1..WINDOW_SECONDS as u32 {
        now += Duration::from_secs(1);
        fec.report(now, &report(seq, 600, 6)).unwrap();
        assert_eq!(fec.loss(), Some(0.1), "{seq}");
    }
    now += Duration::from_secs(1);
    fec.report(now, &report(WINDOW_SECONDS as u32, 600, 6))
        .unwrap();
    assert_eq!(fec.loss(), Some(0.01));
}

#[test]
fn reports_in_one_second_are_weighted_by_shards() {
    let start = Instant::now();
    let mut fec = StatisticalFec::new(50, 20, 0, start);
    fec.report(start, &report(0, 100, 10)).unwrap();
    fec.report(start + Duration::from_millis(500), &report(1, 900, 0))
        .unwrap();
    assert_eq!(fec.loss(), Some(0.01));
}

#[test]
fn repeated_old_and_broken_reports_are_refused() {
    let start = Instant::now();
    let mut fec = StatisticalFec::new(50, 20, 0, start);
    fec.report(start, &report(u32::MAX, 600, 6)).unwrap();
    assert_eq!(
        fec.report(start, &report(u32::MAX, 600, 6)),
        Err(Rejected::Stale)
    );
    assert_eq!(
        fec.report(start, &report(u32::MAX - 5, 600, 6)),
        Err(Rejected::Stale)
    );
    // The sequence wraps.
    fec.report(start + Duration::from_secs(1), &report(0, 600, 6))
        .unwrap();
    let huge = FecReport {
        data_shards: 999_999,
        parity_shards: 2,
        ..report(1, 600, 0)
    };
    assert_eq!(fec.report(start, &huge), Err(Rejected::Implausible));
    // A second without video keeps the sequence and the estimate.
    let idle = start + Duration::from_secs(2);
    fec.report(
        idle,
        &FecReport {
            data_shards: 0,
            parity_shards: 0,
            missing_shards: 0,
            ..report(1, 600, 0)
        },
    )
    .unwrap();
    assert_eq!(fec.loss(), Some(0.01));
    assert!(fec.percent(idle, 100) > 0);
}

#[test]
fn a_new_estimate_is_built_outside_and_installed_once() {
    let start = Instant::now();
    let mut fec = StatisticalFec::new(50, 20, 0, start);
    let pending = fec.observe(start, &report(0, 600, 30)).unwrap().unwrap();
    // Until its tables are in, frames keep the previous estimate.
    assert_eq!(fec.loss(), None);
    assert_eq!(fec.percent(start, 100), 20);
    fec.install(pending.build());
    assert_eq!(fec.loss(), Some(0.05));
    let lossy = fec.percent(start, 100);
    assert!(lossy > 0 && lossy < 50, "{lossy}");
    // An estimate overtaken by a newer one is dropped.
    let now = start + Duration::from_secs(1);
    let older = fec.observe(now, &report(1, 600, 60)).unwrap().unwrap();
    let later = now + Duration::from_secs(1);
    let newer = fec.observe(later, &report(2, 600, 300)).unwrap().unwrap();
    fec.install(older.build());
    assert_eq!(fec.loss(), Some(0.05), "the stale estimate is dropped");
    fec.install(newer.build());
    assert_eq!(fec.loss(), Some(0.5));
    // So is one for another stream.
    let mut other = StatisticalFec::new(50, 20, 0, start);
    let pending = fec
        .observe(later + Duration::from_secs(1), &report(3, 600, 600))
        .unwrap()
        .unwrap();
    other.install(pending.build());
    assert_eq!(other.loss(), None);
    // No loss needs no tables.
    let mut clean = StatisticalFec::new(50, 20, 0, start);
    assert_eq!(clean.observe(start, &report(0, 600, 0)).unwrap(), None);
    assert_eq!(clean.loss(), Some(0.));
}

#[test]
fn reports_closer_than_the_interval_count_without_an_estimate() {
    let start = Instant::now();
    let mut fec = StatisticalFec::new(50, 20, 0, start);
    fec.report(start, &report(0, 600, 0)).unwrap();
    // A flood: counted, the sequence stays continuous, no tables.
    for seq in 1..100 {
        let at = start + Duration::from_millis(u64::from(seq));
        assert_eq!(fec.observe(at, &report(seq, 600, 60)).unwrap(), None);
    }
    assert_eq!(fec.loss(), Some(0.));
    assert_eq!(fec.percent(start, 100), fec.floor());
    // The next report past the interval estimates with everything counted.
    fec.report(start + MIN_ESTIMATE_INTERVAL, &report(100, 600, 0))
        .unwrap();
    assert_eq!(fec.loss(), Some(99. * 60. / (101. * 600.)));
    assert!(fec.percent(start + MIN_ESTIMATE_INTERVAL, 100) > fec.floor());
    // A gap always estimates afresh.
    let gap = start + MIN_ESTIMATE_INTERVAL + Duration::from_millis(1);
    fec.report(gap, &report(102, 600, 0)).unwrap();
    assert_eq!(fec.loss(), Some(0.));
}

#[test]
fn the_client_minimum_parity_counts() {
    let start = Instant::now();
    let mut fec = StatisticalFec::new(50, 20, 4, start);
    fec.report(start, &report(0, 100_000, 1)).unwrap();
    // At 0.001 % loss one parity shard is plenty, but every block carries
    // the client's four: the floor already reaches it, even for 2 shards.
    assert_eq!(fec.percent(start, 200), fec.floor());
    assert_eq!(fec_parameters(2, fec.percent(start, 2), 4).0, 4);
}

#[test]
fn a_clean_link_keeps_the_floor() {
    // Retroid on Wi-Fi, November 1: after one clean second the share went
    // from 20 to 0 %, and the next lost or late packet cost a frame.
    let start = Instant::now();
    let mut fec = StatisticalFec::new(50, 20, 0, start);
    let mut now = start;
    for seq in 0..WINDOW_SECONDS as u32 * 2 {
        fec.report(now, &report(seq, 3000, 0)).unwrap();
        for shards in [1, 47, 180, 600] {
            assert_eq!(fec.percent(now, shards), 10, "{seq} {shards}");
        }
        now += Duration::from_secs(1);
    }
    assert!(fec_parameters(47, fec.percent(now, 47), 0).0 >= 5);
    // Light loss whose binomial choice is below the floor keeps the floor.
    fec.report(now, &report(WINDOW_SECONDS as u32 * 2, 100_000, 1))
        .unwrap();
    assert_eq!(fec.percent(now, 180), 10);
    // A cap under the floor wins.
    let mut capped = StatisticalFec::new(5, 20, 0, start);
    capped.report(start, &report(0, 3000, 0)).unwrap();
    assert_eq!(capped.percent(start, 180), 5);
}

#[test]
fn changes_are_logged_once_a_second() {
    let start = Instant::now();
    let mut fec = StatisticalFec::new(50, 20, 0, start);
    assert_eq!(fec.log_due(start, 20), None);
    assert_eq!(fec.log_due(start, 9), Some((20, 9)));
    assert_eq!(fec.log_due(start + Duration::from_millis(500), 12), None);
    assert_eq!(
        fec.log_due(start + Duration::from_secs(1), 12),
        Some((9, 12))
    );
}

#[test]
fn the_tables_are_cheap_enough_for_the_control_thread() {
    // One table per changed estimate, at most once a second.
    let started = Instant::now();
    for loss in [0.001, 0.01, 0.05, 0.2, 0.5] {
        let table = required_parity(loss);
        assert_eq!(table.len(), MAX_BLOCKS);
        assert!(table.iter().all(|t| t[0] == 0));
    }
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
}
