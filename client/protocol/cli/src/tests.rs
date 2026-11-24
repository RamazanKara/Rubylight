//! Unit tests for `nightjar-cli` option parsing, look patterns, and link-notice formatting.

use super::*;
use rubylight_protocol::control::{LinkNotice, LinkReason, LinkState};

fn strings(args: &[&str]) -> Vec<String> {
    args.iter().map(|a| a.to_string()).collect()
}

#[test]
fn the_roam_options_come_out_and_leave_the_positional_arguments() {
    let mut args = strings(&[
        "cli", "url", "--roam-at", "2.5", "h264", "--roam-ip", "127.0.0.2", "640",
    ]);
    assert_eq!(
        take_roam_options(&mut args),
        Ok(Some(RoamAt {
            seconds: 2.5,
            ip: Some("127.0.0.2".parse().unwrap())
        }))
    );
    assert_eq!(args, strings(&["cli", "url", "h264", "640"]));

    let mut args = strings(&["cli", "url", "--roam-at", "3"]);
    assert_eq!(
        take_roam_options(&mut args),
        Ok(Some(RoamAt {
            seconds: 3.0,
            ip: None
        }))
    );
    let mut args = strings(&["cli", "url", "h264"]);
    assert_eq!(take_roam_options(&mut args), Ok(None));
    assert_eq!(args.len(), 3);

    for bad in [
        &["cli", "url", "--roam-ip", "127.0.0.2"][..],
        &["cli", "url", "--roam-at"],
        &["cli", "url", "--roam-at", "soon"],
        &["cli", "url", "--roam-at", "-1"],
        &["cli", "url", "--roam-at", "2", "--roam-ip", "localhost"],
    ] {
        assert!(take_roam_options(&mut strings(bad)).is_err(), "{bad:?}");
    }
}

#[test]
fn a_link_notice_is_one_json_line() {
    let limited = LinkNotice {
        state: LinkState::Limited,
        reason: LinkReason::Network,
        ceiling_kbps: 150_000,
        current_kbps: 75_000,
    };
    assert_eq!(
        link_notice_line(&limited),
        "LINK_NOTICE {\"state\":\"limited\",\"reason\":\"network\",\"ceiling_kbps\":150000,\"current_kbps\":75000}"
    );
    let full = LinkNotice {
        state: LinkState::Full,
        reason: LinkReason::None,
        ceiling_kbps: 150_000,
        current_kbps: 150_000,
    };
    assert_eq!(
        link_notice_line(&full),
        "LINK_NOTICE {\"state\":\"full\",\"reason\":\"none\",\"ceiling_kbps\":150000,\"current_kbps\":150000}"
    );
}

#[test]
fn sent_counts_keep_both_signed_axes() {
    let sent = SentMouse::default();
    assert_eq!(sent.get(), (0, 0));
    sent.add(-5, 7);
    sent.add(2, -20);
    assert_eq!(sent.get(), (-3, -13));
    sent.add(i16::MAX, i16::MIN);
    assert_eq!(sent.get(), (32764, -32781));
}

#[test]
fn the_look_pattern_follows_its_segments_exactly_and_keeps_y_bounded() {
    let mut pattern = LookPattern::new(7);
    let mut sums = (0i64, 0i64);
    // (segments begun, start, end, origin, velocity) of each segment as stepped.
    type Segment = (u64, u64, u64, (f64, f64), (f64, f64));
    let mut segments: Vec<Segment> = Vec::new();
    let (mut at, mut last, mut noise) = (0u64, 0u64, 1u64);
    while at < 600_000_000 {
        let (dx, dy) = pattern.step(at);
        sums.0 += i64::from(dx);
        sums.1 += i64::from(dy);
        assert_eq!(sums, pattern.sent);
        assert!(sums.1.abs() <= 250, "y {} at {at}", sums.1);
        if segments.last().is_none_or(|s| s.0 != pattern.segments) {
            segments.push((
                pattern.segments,
                pattern.start_us,
                pattern.end_us,
                pattern.origin,
                pattern.velocity,
            ));
        }
        // Steps 1 to 12 ms apart, as a late or early sleep leaves them.
        noise = noise
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        last = at;
        at += 1_000 + (noise >> 33) % 11_000;
    }
    // The counts sent are the exact position, rounded: fractions carried over.
    let (x, y) = pattern.position(last);
    assert_eq!(sums, (x.round() as i64, y.round() as i64));
    assert!(segments.len() > 400, "{}", segments.len());
    let mut speeds = std::collections::BTreeSet::new();
    for (i, &(begun, start, end, origin, velocity)) in segments.iter().enumerate() {
        assert_eq!(begun, i as u64 + 1);
        assert!(
            (250_000..=1_200_000).contains(&(end - start)),
            "{}",
            end - start
        );
        if i > 0 {
            assert_eq!(start, segments[i - 1].2);
        }
        if i % 4 == 3 {
            assert_eq!(velocity, (0.0, 0.0), "segment {i} pauses");
            continue;
        }
        assert!(LOOK_X_SPEEDS.contains(&(velocity.0 as i32)), "{velocity:?}");
        speeds.insert(velocity.0 as i32);
        assert!(velocity.1.abs() <= 300.0 + 1e-9, "{velocity:?}");
        // At the limit, y turns back toward 0.
        if origin.1.abs() >= LOOK_Y_LIMIT - 0.5 {
            assert!(velocity.1 * origin.1 <= 0.0, "{origin:?} {velocity:?}");
        }
    }
    assert_eq!(speeds.len(), LOOK_X_SPEEDS.len());
    assert!(segments.iter().any(|s| s.3 .1.abs() >= LOOK_Y_LIMIT - 0.5));
}

#[test]
fn the_look_pattern_is_the_seeds() {
    let steps = |seed: u64| {
        let mut pattern = LookPattern::new(seed);
        (0..2_000u64)
            .map(|i| pattern.step(i * 4_000))
            .collect::<Vec<_>>()
    };
    assert_eq!(steps(3), steps(3));
    assert_ne!(steps(3), steps(4));
    // The seed that would leave xorshift at 0 still moves.
    assert!(steps(0x9E37_79B9_7F4A_7C15)
        .iter()
        .any(|&step| step != (0, 0)));
}
