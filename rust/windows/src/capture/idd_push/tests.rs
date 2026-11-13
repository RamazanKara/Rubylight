use super::*;

#[test]
fn one_capture_per_monitor_registers_a_ring() {
    // Ids no real monitor of the test process has.
    let first = Exclusive::claim(u64::MAX - 1).expect("free");
    assert!(
        Exclusive::claim(u64::MAX - 1).is_none(),
        "a second ring would take the first capture's frames"
    );
    let other = Exclusive::claim(u64::MAX - 2).expect("another monitor is free");
    drop(first);
    assert!(Exclusive::claim(u64::MAX - 1).is_some(), "released on drop");
    drop(other);
}

#[test]
fn rings_that_keep_failing_before_a_frame_send_the_monitor_to_wgc() {
    let mut failures = Failures::default();
    let scope = FenceScope {
        session: 1,
        registration: 5,
        driver_pid: 40,
        adapter: (7, 0),
    };
    let other_gpu = FenceScope {
        adapter: (8, 0),
        ..scope
    };
    assert_eq!(failures.failed(scope), 1);
    assert_eq!(failures.excluded(scope), None, "the first failure retries");
    assert_eq!(failures.failed(scope), FAILURES_BEFORE_WGC);
    assert_eq!(failures.excluded(scope), Some(FAILURES_BEFORE_WGC));
    let other_monitor = FenceScope {
        session: 2,
        ..scope
    };
    assert_eq!(failures.excluded(other_monitor), None, "another monitor");
    assert_eq!(failures.excluded(other_gpu), None, "another GPU retries");
    // Failing on another GPU starts the count again.
    assert_eq!(failures.failed(other_gpu), 1);
    assert_eq!(failures.excluded(scope), None);
    failures.failed(other_gpu);
    assert!(failures.excluded(other_gpu).is_some());
    failures.delivered(1);
    assert_eq!(
        failures.excluded(other_gpu),
        None,
        "a delivered frame clears it"
    );
}

#[test]
fn a_recreated_display_or_restarted_driver_gets_its_frame_ring_back() {
    // A virtual display's session id is its client's, the same every time it is created.
    let mut failures = Failures::default();
    let scope = FenceScope {
        session: 1,
        registration: 5,
        driver_pid: 40,
        adapter: (7, 0),
    };
    for _ in 0..FAILURES_BEFORE_WGC {
        failures.failed(scope);
    }
    assert!(failures.excluded(scope).is_some(), "reopening keeps WGC");
    for fresh in [
        FenceScope {
            registration: 6,
            ..scope
        },
        FenceScope {
            driver_pid: 41,
            ..scope
        },
    ] {
        assert_eq!(failures.excluded(fresh), None);
        // A failure there counts from one.
        let mut again = Failures(failures.0.clone());
        assert_eq!(again.failed(fresh), 1);
        assert_eq!(again.excluded(scope), None);
    }
}

#[test]
fn idd_push_runs_when_asked_or_on_the_hosts_own_display_in_auto() {
    assert!(wanted("idd-push", "wgc", false));
    assert!(wanted("wgc", "auto", true));
    assert!(wanted("wgc", "", true));
    // A physical display, or a display another program created.
    assert!(!wanted("wgc", "auto", false));
    // An explicit WGC or DDX choice is kept.
    assert!(!wanted("wgc", "wgc", true));
    assert!(!wanted("ddx", "ddx", true));
}

#[test]
fn the_ring_follows_the_composed_surface_once_known() {
    let mut status = MonitorStatus {
        width: 2560,
        height: 1440,
        refresh_mhz: 240_000,
        ..Default::default()
    };
    assert_eq!(ring_shape(&status, false), (2560, 1440, ring::FORMAT_BGRA8));
    assert_eq!(
        ring_shape(&status, true),
        (2560, 1440, ring::FORMAT_RGBA16F)
    );
    status.surface_width = 1920;
    status.surface_height = 1080;
    status.surface_format = ring::FORMAT_RGBA16F;
    assert_eq!(
        ring_shape(&status, false),
        (1920, 1080, ring::FORMAT_RGBA16F)
    );
    // A surface format the ring cannot carry falls back to the mode.
    status.surface_format = 24;
    assert_eq!(ring_shape(&status, false), (2560, 1440, ring::FORMAT_BGRA8));
}

#[test]
fn a_shape_change_rebuilds_before_the_first_frame_and_reopens_after() {
    let fp16 = (1920, 1080, ring::FORMAT_RGBA16F);
    assert_eq!(
        verdict((ring::STATUS_FORMAT_MISMATCH, 10), fp16, false),
        Verdict::Reshape(1920, 1080, ring::FORMAT_RGBA16F)
    );
    assert!(matches!(
        verdict((ring::STATUS_SIZE_MISMATCH, 0), fp16, true),
        Verdict::Fail(_)
    ));
    assert!(matches!(
        verdict((ring::STATUS_SIZE_MISMATCH, 0), (0, 0, 0), false),
        Verdict::Fail(_)
    ));
    assert_eq!(
        verdict((ring::STATUS_STREAMING, 0), fp16, true),
        Verdict::Fine
    );
    assert_eq!(
        verdict((ring::STATUS_WAITING, 0), (0, 0, 0), false),
        Verdict::Fine
    );
    assert_eq!(
        verdict((ring::STATUS_RENEW_REQUIRED, 0), fp16, true),
        Verdict::Renew
    );
    for status in [
        ring::STATUS_OPEN_FAILED,
        ring::STATUS_WRONG_ADAPTER,
        ring::STATUS_DEVICE_LOST,
        99,
    ] {
        assert!(matches!(
            verdict((status, 0), fp16, false),
            Verdict::Fail(_)
        ));
    }
}

#[test]
fn ring_generations_are_new_nonzero_and_fit_the_token() {
    assert_eq!(next_generation(0), 1);
    assert_eq!(next_generation(4), 5);
    let last = ring::Token::GENERATION_MASK;
    assert_eq!(next_generation(last - 1), last);
    assert_eq!(next_generation(last), 1, "wraps past 24 bits, never to 0");
    assert_eq!(next_generation(u32::MAX), 1 + u32::MAX % last);
    for previous in [0, 1, last - 1, last, u32::MAX] {
        let next = next_generation(previous);
        assert!((1..=last).contains(&next), "{previous} -> {next}");
    }
}

#[test]
fn qpc_spans_are_zero_when_reversed_or_missing() {
    assert_eq!(qpc_micros(1_000, 1_500, 10_000_000), 50);
    assert_eq!(qpc_micros(1_500, 1_000, 10_000_000), 0);
    assert_eq!(qpc_micros(0, 1_000, 10_000_000), 0);
}

#[test]
fn a_quiet_drain_loop_counts_only_past_the_limit() {
    let frequency = 10_000_000;
    let limit = Duration::from_secs(3);
    assert!(!stalled(0, 50_000_000, frequency, limit), "never stamped");
    assert!(!stalled(40_000_000, 50_000_000, frequency, limit));
    assert!(stalled(10_000_000, 50_000_000, frequency, limit));
    // A stamp ahead of this process's clock read is not a stall.
    assert!(!stalled(60_000_000, 50_000_000, frequency, limit));
}

#[test]
fn fence_rings_require_the_request_capability_and_in_place_compute() {
    assert_eq!(
        ring_kind(CAP_FENCE_RING | CAP_SCOPED_CLEAR, true, true),
        Kind::Fence
    );
    for (caps, requested, zero_copy) in [
        (CAP_FENCE_RING, false, true),
        (CAP_FENCE_RING, true, false),
        (CAP_SCOPED_CLEAR, true, true),
        (0, true, true),
    ] {
        assert_eq!(ring_kind(caps, requested, zero_copy), Kind::KeyedMutex);
    }
}

#[test]
fn the_fence_ring_is_the_default_and_an_explicit_false_selects_the_keyed_ring() {
    let caps = CAP_FENCE_RING | CAP_SCOPED_CLEAR;
    let configured = |text: &str| {
        let config = butterpollo_core::config::Config::parse(text).unwrap();
        choose_ring(caps, config.idd_push_fence_ring(), false, true)
    };
    let fence = RingChoice {
        kind: Kind::Fence,
        warning: None,
    };
    assert_eq!(configured(""), fence);
    assert_eq!(configured("idd_push_fence_ring = true"), fence);
    // Choosing the keyed ring is not a fault: no warning.
    assert_eq!(
        configured("idd_push_fence_ring = false"),
        RingChoice {
            kind: Kind::KeyedMutex,
            warning: None
        }
    );
}

#[test]
fn a_fence_ring_that_cannot_be_used_stays_keyed_and_says_why() {
    let driver = choose_ring(CAP_SCOPED_CLEAR, true, false, true);
    assert_eq!(driver.kind, Kind::KeyedMutex);
    let driver = driver.warning.expect("an old driver is reported");
    assert!(driver.contains("Update the display driver"), "{driver}");
    assert!(driver.contains("idd_push_fence_ring = false"), "{driver}");
    let compute = choose_ring(CAP_FENCE_RING, true, false, false);
    assert_eq!(compute.kind, Kind::KeyedMutex);
    let compute = compute.warning.expect("in-place compute is reported");
    assert!(compute.contains("gpu_compute_conversion"), "{compute}");
    assert!(compute.contains("idd_push_fence_ring = false"), "{compute}");
    let failed = choose_ring(CAP_FENCE_RING, true, true, true);
    assert_eq!(failed.kind, Kind::KeyedMutex);
    assert!(failed.warning.unwrap().contains("fences failed"));
    // Without the request there is nothing to explain, whatever else is missing.
    assert_eq!(
        choose_ring(0, false, false, false),
        RingChoice {
            kind: Kind::KeyedMutex,
            warning: None
        }
    );
}

#[test]
fn a_failed_fence_transport_stays_keyed_through_capture_reopen() {
    let monitor = Owned {
        session_id: 41,
        adapter: (3, 0),
        target: 2,
        wudf_pid: 55,
        listed: None,
        registration: 7,
    };
    let scope = FenceScope::new(&monitor, (9, 0));
    let mut failures = FenceFailures::default();
    failures.failed(scope);
    assert_eq!(
        ring_kind(CAP_FENCE_RING, !failures.excluded(scope), true),
        Kind::KeyedMutex,
        "reopening the same capture must not retry the rejected transport"
    );
    for changed in [
        FenceScope {
            registration: 8,
            ..scope
        },
        FenceScope {
            driver_pid: 56,
            ..scope
        },
        FenceScope {
            adapter: (10, 0),
            ..scope
        },
        FenceScope {
            session: 42,
            ..scope
        },
    ] {
        assert!(!failures.excluded(changed), "new owner/device can retry");
    }
    failures.clear(monitor.session_id);
    assert!(
        !failures.excluded(scope),
        "an explicit keyed capture resets the opt-in"
    );
}

#[test]
fn remembered_fence_failures_are_bounded_and_keep_recent_rejections() {
    let mut failures = FenceFailures::default();
    for session in 0..=FenceFailures::LIMIT as u64 {
        failures.failed(FenceScope {
            session,
            registration: 1,
            driver_pid: 2,
            adapter: (3, 0),
        });
    }
    assert_eq!(failures.0.len(), FenceFailures::LIMIT);
    assert_eq!(failures.0[0].session, 1);
    let latest = *failures.0.last().unwrap();
    failures.failed(latest);
    assert_eq!(failures.0.len(), FenceFailures::LIMIT);
    assert!(failures.excluded(latest));
}

#[test]
fn fence_startup_recovery_distinguishes_missing_pixels_from_failed_handoff() {
    let waiting = ring::STATUS_WAITING;
    let idle = Counters::default();
    let acquired = Counters {
        acquired: 1,
        ..idle
    };
    let check = |elapsed, counters| fence_recovery(waiting, false, elapsed, counters, false, false);
    assert_eq!(
        check(FENCE_HANDOFF_TIMEOUT, idle),
        None,
        "Windows may still be starting the display"
    );
    assert_eq!(
        check(FENCE_HANDOFF_TIMEOUT - Duration::from_millis(1), acquired),
        None
    );
    assert_eq!(
        check(FENCE_HANDOFF_TIMEOUT, acquired),
        Some(FenceRecovery::Keyed {
            reason: "could not publish acquired frames",
            remember: true
        })
    );
    assert_eq!(
        check(FIRST_FRAME_TIMEOUT, idle),
        Some(FenceRecovery::Keyed {
            reason: "did not deliver a frame",
            remember: false
        }),
        "no producer activity alone must not blacklist fences"
    );
    assert_eq!(
        check(
            FENCE_HANDOFF_TIMEOUT,
            Counters {
                pushed: 1,
                ..acquired
            }
        ),
        None,
        "give a newly published frame to the consumer"
    );
    assert_eq!(
        fence_recovery(
            ring::STATUS_SIZE_MISMATCH,
            false,
            FIRST_FRAME_TIMEOUT,
            acquired,
            false,
            false
        ),
        None,
        "shape/device recovery must not identify a fence incompatibility"
    );
}

#[test]
fn failed_fence_ordering_and_stalled_queues_reopen_the_capture_device() {
    for (ordering_failed, queue_stuck, reason) in [
        (true, false, "lost GPU ordering"),
        (false, true, "compute queue cannot be reused"),
    ] {
        assert_eq!(
            fence_recovery(
                ring::STATUS_STREAMING,
                true,
                Duration::ZERO,
                Counters::default(),
                ordering_failed,
                queue_stuck
            ),
            Some(FenceRecovery::Reopen(reason)),
            "even a healthy driver heartbeat/device cannot make a stuck queue reusable"
        );
    }
    assert!(matches!(
        fence_recovery(
            ring::STATUS_OPEN_FAILED,
            false,
            Duration::ZERO,
            Counters::default(),
            false,
            false
        ),
        Some(FenceRecovery::Keyed { remember: true, .. })
    ));
    assert!(matches!(
        fence_recovery(
            ring::STATUS_OPEN_FAILED,
            true,
            Duration::ZERO,
            Counters::default(),
            false,
            false
        ),
        Some(FenceRecovery::Reopen(_))
    ));
}
