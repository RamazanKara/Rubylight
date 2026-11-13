use super::*;
#[test]
fn advertised_limiter_flags_follow_manual_provider_virtual_policy_and_fractional_limits() {
    for (text, virtual_display, expected) in [
        ("", false, (false, true, 0)),
        ("", true, (true, true, 0)),
        (
            "frame_limiter_auto_virtual_framegen=disabled",
            true,
            (false, false, 0),
        ),
        (
            "frame_limiter_enable=true\nframe_limiter_provider=none",
            false,
            (false, true, 0),
        ),
        (
            "frame_limiter_enable=true\nframe_limiter_provider=rtss\nframe_limiter_fps_limit=59.94",
            false,
            (true, true, 59940),
        ),
    ] {
        assert_eq!(
            advertised(&Config::parse(text).unwrap(), virtual_display),
            expected,
            "{text}"
        );
    }
}
#[test]
fn automatic_capture_prefers_wgc_on_physical_and_virtual_displays() {
    for text in ["", "capture=", "capture=auto", "capture= Auto "] {
        let config = Config::parse(text).unwrap();
        let original = config.values.clone();
        for virtual_display in [false, true] {
            for generation in ["none", "game-provided"] {
                for rate in [Rate(59940), Rate(60000), Rate(120000)] {
                    for vrr in [false, true] {
                        let policy = Policy::resolve(
                            &config,
                            rate,
                            virtual_display,
                            generation,
                            false,
                            false,
                        )
                        .unwrap()
                        .with_vrr(&config, virtual_display, vrr);
                        assert_eq!(policy.capture, "wgc", "{text}, {generation}");
                        assert_eq!(policy.rate, rate);
                    }
                }
            }
        }
        assert_eq!(config.values, original);
    }
}
#[test]
fn explicit_capture_choices_survive_virtual_display_and_framegen_policy() {
    for (setting, expected) in [
        ("ddx", "ddx"),
        (" DDX ", "ddx"),
        ("dxgi", "dxgi"),
        ("DXGI", "dxgi"),
        ("wgc", "wgc"),
        ("WGC", "wgc"),
        ("wgcc", "wgc"),
        // Rubylight's own display driver pushes the frames.
        ("idd-push", "idd-push"),
        (" IDD-Push ", "idd-push"),
    ] {
        let config = Config::parse(&format!("capture={setting}")).unwrap();
        let original = config.values.clone();
        for virtual_display in [false, true] {
            for generation in ["none", "game-provided"] {
                for vrr in [false, true] {
                    let policy = Policy::resolve(
                        &config,
                        Rate(59940),
                        virtual_display,
                        generation,
                        true,
                        false,
                    )
                    .unwrap()
                    .with_vrr(&config, virtual_display, vrr);
                    assert_eq!(policy.capture, expected, "{setting}, {generation}");
                }
            }
        }
        assert_eq!(config.values, original);
    }
}
#[test]
fn vrr_virtual_display_uses_fixed_1000_hz_without_changing_stream_rate() {
    let c = Config::default();
    let p = Policy::resolve(&c, Rate(119880), true, "none", false, false)
        .unwrap()
        .with_vrr(&c, true, true);
    assert_eq!(p.display_rate, Rate(1_000_000));
    assert_eq!(p.rate, Rate(119880));
    assert_eq!(p.capture, "wgc");
    let c = Config::parse("frame_limiter_auto_virtual_framegen=disabled\ncapture=ddx").unwrap();
    let p = Policy::resolve(&c, Rate(119880), true, "none", false, false)
        .unwrap()
        .with_vrr(&c, true, true);
    assert_eq!(p.display_rate, Rate(119880));
    assert_eq!(p.capture, "ddx");
    let c = Config::parse("frame_limiter_auto_virtual_framegen=fixed-1000-hz").unwrap();
    let p = Policy::resolve(&c, Rate(59940), false, "none", false, false)
        .unwrap()
        .with_vrr(&c, false, true);
    assert_eq!(p.display_rate, Rate(59940));
}
#[test]
fn a_device_display_mode_keeps_the_streams_limiter_rate() {
    for (setting, limit) in [("", 116_000), ("frame_limiter_fps_limit=58", 58_000)] {
        for (mode, display) in [
            ("legacy", 120_000),
            ("enabled", 240_000),
            ("fixed-1000hz", 1_000_000),
            ("disabled", 60_000),
        ] {
            let c = Config::parse(&format!(
                "{setting}\nframe_limiter_auto_virtual_framegen={mode}"
            ))
            .unwrap();
            for virtual_display in [false, true] {
                let mut p =
                    Policy::resolve(&c, Rate(116_000), virtual_display, "none", false, false)
                        .unwrap();
                p.display_rate = display_rate(Rate(60_000), virtual_display, virtual_refresh(&c));
                assert_eq!(p.rate, Rate(limit));
                assert_eq!(
                    p.display_rate,
                    Rate(if virtual_display { display } else { 60_000 })
                );
                let p = p.with_vrr(&c, virtual_display, true);
                assert_eq!(p.rate, Rate(limit));
                assert_eq!(p.rate.rational(), (limit / 1000, 1));
                assert_eq!(
                    p.display_rate,
                    Rate(if virtual_display && mode != "disabled" {
                        1_000_000
                    } else {
                        60_000
                    })
                );
            }
        }
    }
}
#[test]
fn quoted_frame_rates_are_read_like_the_console_shows_them() {
    assert_eq!(Rate::parse("\"59.94\"").unwrap(), Rate(59940));
    assert_eq!(Rate::parse(" \" 120 \" ").unwrap(), Rate(120_000));
    assert!(Rate::parse("\"59.9400\"").is_err());
    let c = Config::parse(
        "frame_limiter_enable=true\nframe_limiter_provider=rtss\nframe_limiter_fps_limit=\"30\"",
    )
    .unwrap();
    assert_eq!(advertised(&c, false), (true, true, 30_000));
    let p = Policy::resolve(&c, Rate(60_000), false, "none", false, false).unwrap();
    assert_eq!(p.rate, Rate(30_000));
    let c =
        Config::parse("dd_refresh_rate_option=manual\ndd_manual_refresh_rate=\"119.88\"").unwrap();
    assert_eq!(
        c.display_request_rate(1920, 1080, Rate(60_000), false, false)
            .unwrap()
            .refresh,
        Some(119_880)
    );
}
#[test]
fn fractional_rate_round_trip_and_bounds() {
    assert_eq!(Rate::parse("59.940").unwrap().rational(), (2997, 50));
    assert_eq!(Rate::parse("119.880").unwrap().rounded(), 120);
    assert_eq!(Rate(0).rational(), (0, 1));
    for bad in [
        "NaN",
        "-1",
        "1000.001",
        "59.9400",
        "1e2",
        "1.2.3",
        "4294967296",
    ] {
        assert!(Rate::parse(bad).is_err());
    }
}
#[test]
fn inherited_framegen_keeps_explicit_capture_and_sync_contract() {
    let mut c = Config::parse(
        "capture=ddx\nrtss_frame_limit_type=back_edge_sync\nrtss_allow_virtual_display_override=1",
    )
    .unwrap();
    let p = Policy::resolve(&c, Rate(59940), true, "game-provided", false, false).unwrap();
    assert_eq!(p.display_rate, Rate(119880));
    assert_eq!(p.sync_limiter, 1);
    assert_eq!(p.capture, "ddx");
    let p = Policy::resolve(&c, Rate(59940), true, "game-provided", false, true).unwrap();
    assert_eq!(p.sync_limiter, 2);
    c.values.insert(
        "frame_limiter_auto_virtual_framegen".into(),
        "enabled".into(),
    );
    assert_eq!(
        Policy::resolve(&c, Rate(59940), true, "game-provided", false, false)
            .unwrap()
            .display_rate,
        Rate(239760)
    );
}
/// Reflex came back with NVENC: an NVIDIA stream that syncs RTSS itself uses Reflex, and a
/// chosen Reflex mode holds on NVIDIA and becomes front edge sync elsewhere.
#[test]
fn rtss_reflex_follows_the_streams_gpu() {
    let auto = Config::default();
    let p = Policy::resolve(&auto, Rate(60_000), true, "none", false, false).unwrap();
    assert!(p.automatic_sync);
    assert_eq!(p.clone().for_gpu(true).sync_limiter, SYNC_REFLEX);
    assert_eq!(p.for_gpu(false).sync_limiter, SYNC_FRONT_EDGE);
    // A monitor stream without frame generation keeps the configured mode.
    let p = Policy::resolve(&auto, Rate(60_000), false, "none", false, false).unwrap();
    assert!(!p.automatic_sync);
    assert_eq!(p.for_gpu(true).sync_limiter, 0);
    for spelling in ["nvidia-reflex", "nvidia reflex", "Reflex"] {
        assert!(is_reflex(spelling));
        let c = Config::parse(&format!("rtss_frame_limit_type={spelling}")).unwrap();
        let p = Policy::resolve(&c, Rate(60_000), false, "none", false, false).unwrap();
        assert_eq!(p.sync_limiter, SYNC_REFLEX);
        assert_eq!(p.clone().for_gpu(true).sync_limiter, SYNC_REFLEX);
        assert_eq!(p.for_gpu(false).sync_limiter, SYNC_FRONT_EDGE);
    }
    // A per-device or per-app mode (runtime sync) is kept on virtual displays too.
    let c = Config::parse("rtss_frame_limit_type=back-edge-sync").unwrap();
    let p = Policy::resolve(&c, Rate(60_000), true, "none", false, true).unwrap();
    assert_eq!(p.for_gpu(true).sync_limiter, 2);
}
#[test]
fn virtual_refresh_reads_vibepollo_spellings() {
    for (value, mode) in [
        ("2x", VirtualRefresh::Legacy),
        ("fixed_2x", VirtualRefresh::Legacy),
        ("No", VirtualRefresh::Disabled),
        ("disable", VirtualRefresh::Disabled),
        ("smoother", VirtualRefresh::Enabled),
        ("1000hz", VirtualRefresh::Vrr),
        ("sideways", VirtualRefresh::Legacy),
    ] {
        let c = Config::parse(&format!("frame_limiter_auto_virtual_framegen={value}")).unwrap();
        assert_eq!(virtual_refresh(&c), mode, "{value}");
    }
    assert_eq!(virtual_refresh(&Config::default()), VirtualRefresh::Legacy);
    let c = Config::parse("frame_limiter_auto_virtual_framegen=fixed-2x").unwrap();
    let p = Policy::resolve(&c, Rate(60000), true, "none", false, false).unwrap();
    assert_eq!(p.display_rate, Rate(120000));
}
