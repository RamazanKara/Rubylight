use super::*;

fn kwin_6_8_hdr_preferred() -> DescriptionInfo {
    // KWin 6.5+: its gamma 2.2 blending space on an HDR output.
    DescriptionInfo {
        tf_named: Some(TF_GAMMA22),
        primaries_named: Some(PRIMARIES_BT2020),
        luminances: Some((50, 700, 250)),
        ..Default::default()
    }
}

fn full_support() -> ColorSupport {
    let mut support = ColorSupport::default();
    for feature in [FEATURE_PARAMETRIC, FEATURE_SET_LUMINANCES] {
        ColorSupport::add(&mut support.features, feature);
    }
    ColorSupport::add(&mut support.tfs, TF_ST2084_PQ);
    ColorSupport::add(&mut support.tfs, TF_GAMMA22);
    ColorSupport::add(&mut support.primaries, PRIMARIES_BT2020);
    ColorSupport::add(&mut support.intents, INTENT_PERCEPTUAL);
    support
}

#[test]
fn switches_are_on_unless_turned_off() {
    assert!(switched_on(None));
    assert!(switched_on(Some("1")));
    assert!(switched_on(Some("bogus")), "a value that is no switch leaves it on");
    for off in ["0", "false", "No", " off "] {
        assert!(!switched_on(Some(off)), "{off}");
    }
    assert!(hdr_wsi_layer(Some("1")));
    assert!(!hdr_wsi_layer(Some("0")));
    assert!(!hdr_wsi_layer(Some("")));
    assert!(!hdr_wsi_layer(None));
}

#[test]
fn the_description_is_nightjars_only_on_a_wayland_desktop_with_the_switch_on() {
    assert_eq!(description_refused(true, false, true, false), None);
    assert_eq!(description_refused(false, false, true, false), Some("the window is not a Wayland one"));
    assert_eq!(description_refused(true, true, true, false), Some("gamescope's WSI layer describes the swapchain"));
    assert_eq!(description_refused(true, false, false, false), Some("NIGHTJAR_WAYLAND_HDR_DESCRIPTION is 0"));
    assert!(description_refused(true, false, true, true).unwrap().contains("ENABLE_HDR_WSI"));
}

#[test]
fn a_pq_preferred_description_is_taken_as_it_is() {
    // KWin 6.4: the output's own PQ description.
    let preferred = DescriptionInfo { tf_named: Some(TF_ST2084_PQ), primaries_named: Some(PRIMARIES_BT2020), luminances: Some((50, 10_000, 203)), target_luminance: Some((50, 800)), ..Default::default() };
    let intent_only = ColorSupport { intents: 1 << INTENT_PERCEPTUAL, ..Default::default() };
    let absolute = |support: &ColorSupport, preferred: &DescriptionInfo| hdr_label(support, preferred, Brightness::Absolute, true, None);
    assert_eq!(absolute(&intent_only, &preferred), Ok(HdrLabel::Preferred), "needs no parametric support");
    let by_coordinates = DescriptionInfo { primaries_named: None, primaries: Some([708_400, 291_800, 170_000, 797_000, 131_000, 46_000, 312_700, 329_000]), ..preferred };
    assert_eq!(absolute(&intent_only, &by_coordinates), Ok(HdrLabel::Preferred));
    assert_eq!(absolute(&ColorSupport::default(), &preferred), Err("the compositor does not offer the perceptual intent".into()));
    assert_eq!(preferred.peak(), Some(800), "PQ's 10 000 is no display's peak");
    assert_eq!(HdrLabel::Preferred.reference(&preferred), 203);
    assert_eq!(HdrLabel::Preferred.encoding(), Encoding::Pq);
}

#[test]
fn a_gamma22_blending_space_is_drawn_in_as_it_is() {
    // KWin 6.5 to 6.8 without plane colour pipelines: only a surface in its blending space
    // needs no conversion, so only that one can be scanned out.
    let preferred = kwin_6_8_hdr_preferred();
    let label = hdr_label(&full_support(), &preferred, Brightness::Absolute, true, Some(1000));
    assert_eq!(label, Ok(HdrLabel::Blending { min_lum: 50, max: 700, reference: 250 }));
    let label = label.unwrap();
    assert_eq!(label.encoding(), Encoding::Gamma22 { min_nits: 0.005, max_nits: 700.0 });
    assert_eq!(label.reference(&preferred), 250);
    assert_eq!(describe_label(label, &preferred), "Nightjar's, the compositor's blending space drawn in gamma 2.2 (gamma 2.2 BT.2020, reference 250 nits, peak 700 nits, minimum 0.0050 nits)");
    // It takes the preferred description itself: no parametric support needed.
    let intent_only = ColorSupport { intents: 1 << INTENT_PERCEPTUAL, ..Default::default() };
    assert!(matches!(hdr_label(&intent_only, &preferred, Brightness::Absolute, true, None), Ok(HdrLabel::Blending { .. })));
    // Gamma 2.2 in other primaries is no blending space Nightjar draws in.
    let p3 = DescriptionInfo { primaries_named: Some(cm::PRIMARIES_DISPLAY_P3), ..preferred };
    assert!(matches!(hdr_label(&full_support(), &p3, Brightness::Absolute, true, None), Err(_) | Ok(HdrLabel::Parametric(_))));
    assert!(!matches!(hdr_label(&full_support(), &p3, Brightness::Absolute, true, None), Ok(HdrLabel::Blending { .. })));
    // An empty range would divide by zero in the present: PQ instead.
    let flat = DescriptionInfo { luminances: Some((7_000_000, 700, 250)), ..preferred };
    assert!(!matches!(hdr_label(&full_support(), &flat, Brightness::Absolute, true, None), Ok(HdrLabel::Blending { .. })));
}

#[test]
fn with_the_gamma22_switch_off_the_blending_space_gets_pq_at_its_reference_and_peak() {
    let label = hdr_label(&full_support(), &kwin_6_8_hdr_preferred(), Brightness::Absolute, false, None);
    assert_eq!(label, Ok(HdrLabel::Parametric(PqParams { min_lum: 50, max_lum: 10_001, reference: 250, max_cll: 700 })));
    let label = label.unwrap();
    assert_eq!(label.encoding(), Encoding::Pq);
    assert_eq!(label.reference(&kwin_6_8_hdr_preferred()), 250);
    assert_eq!(
        describe_label(label, &kwin_6_8_hdr_preferred()),
        "Nightjar's, PQ BT.2020 at the output's reference 250 nits and peak 700 nits"
    );
    // The target volume's peak wins over the primary volume's.
    let with_target = DescriptionInfo { target_luminance: Some((50, 650)), ..kwin_6_8_hdr_preferred() };
    assert!(matches!(hdr_label(&full_support(), &with_target, Brightness::Absolute, false, None), Ok(HdrLabel::Parametric(PqParams { max_cll: 650, .. }))));
}

#[test]
fn brightness_that_follows_the_desktop_gets_the_drivers_reference_and_the_contents_peak() {
    let preferred = kwin_6_8_hdr_preferred();
    let desktop = |gamma22, peak| hdr_label(&full_support(), &preferred, Brightness::Desktop, gamma22, peak);
    let label = desktop(true, Some(1000));
    assert_eq!(label, Ok(HdrLabel::Desktop(PqParams { min_lum: 50, max_lum: 10_001, reference: 203, max_cll: 1000 })), "never drawn in the blending space");
    let label = label.unwrap();
    assert_eq!(label.encoding(), Encoding::Pq);
    assert_eq!(label.reference(&preferred), 203);
    assert_eq!(describe_label(label, &preferred), "Nightjar's, PQ BT.2020 at the default reference 203 nits with MaxCLL 1000 nits (brightness follows the desktop)");
    let unknown = desktop(false, None).unwrap();
    assert_eq!(unknown, HdrLabel::Desktop(PqParams { min_lum: 50, max_lum: 10_001, reference: 203, max_cll: 0 }));
    assert_eq!(describe_label(unknown, &preferred), "Nightjar's, PQ BT.2020 at the default reference 203 nits (brightness follows the desktop)");
    // A PQ preferred description is not taken as it is either: its reference is the output's.
    let pq = DescriptionInfo { tf_named: Some(TF_ST2084_PQ), luminances: Some((50, 10_000, 300)), target_luminance: Some((50, 800)), ..preferred };
    assert!(matches!(hdr_label(&full_support(), &pq, Brightness::Desktop, true, None), Ok(HdrLabel::Desktop(PqParams { reference: 203, .. }))));
    // It still needs what the parametric description needs.
    let mut no_pq = full_support();
    no_pq.tfs = 1 << TF_GAMMA22;
    assert_eq!(hdr_label(&no_pq, &preferred, Brightness::Desktop, true, None), Err("the compositor does not take PQ".into()));
    // No preferred luminances are needed: the description is the protocol's defaults.
    assert!(hdr_label(&full_support(), &DescriptionInfo { luminances: None, ..preferred }, Brightness::Desktop, true, None).is_ok());
}

#[test]
fn the_setting_picks_the_brightness_and_the_content_peak_is_kept_in_whole_nits() {
    assert_eq!(Brightness::of_setting(false), Brightness::Absolute, "the default: what HDR10 means");
    assert_eq!(Brightness::of_setting(true), Brightness::Desktop);
    // One test owns the globals: both are set and read back here only.
    set_brightness(Brightness::Desktop);
    assert_eq!(brightness(), Brightness::Desktop);
    set_brightness(Brightness::Absolute);
    assert_eq!(brightness(), Brightness::Absolute);
    set_content_peak(Some(999.6));
    assert_eq!(content_peak(), Some(1000));
    for unknown in [None, Some(0.0), Some(0.4), Some(f32::NAN), Some(f32::INFINITY), Some(-5.0)] {
        set_content_peak(unknown);
        assert_eq!(content_peak(), None, "{unknown:?}");
    }
}

#[test]
fn the_present_encoding_matches_what_the_patched_sdl_pushes() {
    assert_eq!(Encoding::Pq.properties(), (0, 0.0, 0.0));
    assert_eq!(Encoding::Gamma22 { min_nits: 0.005, max_nits: 700.0 }.properties(), (1, 0.005, 700.0));
    let constants = Encoding::constants(1, 0.005, 700.0);
    assert_eq!(constants[0], 1.0);
    assert_eq!(constants[1], 0.005);
    assert!((constants[2] * 699.995 - 1.0).abs() < 1e-5, "{constants:?}");
    assert_eq!(Encoding::constants(0, 0.0, 0.0), [0.0; 4]);
    assert_eq!(Encoding::constants(1, 700.0, 700.0), [0.0; 4], "an empty range is PQ");
    assert_eq!(Encoding::constants(1, 800.0, 700.0), [0.0; 4]);
    assert_eq!(Encoding::constants(2, 0.0, 700.0), [0.0; 4], "an unknown mode is PQ");
}

#[test]
fn gamma22_codes_span_the_range_and_clip_outside_it() {
    assert_eq!(Encoding::gamma22_code(0.005, 0.005, 700.0), 0.0);
    assert_eq!(Encoding::gamma22_code(0.0, 0.005, 700.0), 0.0, "below the minimum clips");
    assert_eq!(Encoding::gamma22_code(700.0, 0.005, 700.0), 1.0);
    assert_eq!(Encoding::gamma22_code(5000.0, 0.005, 700.0), 1.0, "SDL tone maps to the peak first");
    // Reference white of 250 nits on a 700-nit output: (250/700)^(1/2.2).
    let white = Encoding::gamma22_code(250.0, 0.0, 700.0);
    assert!((white - (250.0f32 / 700.0).powf(1.0 / 2.2)).abs() < 1e-6, "{white}");
    // Monotonic, so the dither of half a code never reorders levels.
    let mut last = -1.0;
    for nits in [0.01, 0.1, 1.0, 10.0, 100.0, 203.0, 400.0, 699.0] {
        let code = Encoding::gamma22_code(nits, 0.005, 700.0);
        assert!(code > last, "{nits}");
        last = code;
    }
    // Near black a 10-bit code is coarser than PQ's: one code at 1 nit is a step of a few
    // percent, which the ordered dither of half a code hides.
    let code = |nits| (Encoding::gamma22_code(nits, 0.005, 700.0) * 1023.0).round();
    let one_nit = code(1.0);
    let next = 0.005 + 699.995 * ((one_nit + 1.0) / 1023.0).powf(2.2);
    assert!(next > 1.0 && next < 1.06, "{next}");
}

#[test]
fn what_would_end_the_connection_is_refused_first() {
    let preferred = kwin_6_8_hdr_preferred();
    let absolute = |support: &ColorSupport, preferred: &DescriptionInfo| hdr_label(support, preferred, Brightness::Absolute, false, None);
    let mut no_luminances = full_support();
    no_luminances.features = 1 << FEATURE_PARAMETRIC;
    assert_eq!(absolute(&no_luminances, &preferred), Err("the compositor does not offer set_luminances".into()));
    let mut no_pq = full_support();
    no_pq.tfs = 1 << TF_GAMMA22;
    assert_eq!(absolute(&no_pq, &preferred), Err("the compositor does not take PQ".into()));
    let mut no_bt2020 = full_support();
    no_bt2020.primaries = 0;
    assert!(absolute(&no_bt2020, &preferred).is_err());
    assert!(absolute(&full_support(), &DescriptionInfo { icc: true, ..preferred }).unwrap_err().contains("ICC"));
    assert!(hdr_label(&full_support(), &DescriptionInfo { icc: true, ..preferred }, Brightness::Desktop, true, None).unwrap_err().contains("ICC"));
    assert!(absolute(&full_support(), &DescriptionInfo { luminances: None, ..preferred }).unwrap_err().contains("no luminances"));
    // Reference white not above the minimum: set_luminances' invalid_luminance.
    let dark = DescriptionInfo { luminances: Some((2_500_000, 700, 250)), ..preferred };
    assert!(absolute(&full_support(), &dark).unwrap_err().contains("not above their minimum"));
    assert!(hdr_label(&full_support(), &dark, Brightness::Absolute, true, None).unwrap_err().contains("not above their minimum"), "nor drawn in");
    // A PQ preferred description says nothing about the display's peak without a target.
    let pq_no_peak = DescriptionInfo { tf_named: Some(TF_ST2084_PQ), primaries_named: Some(cm::PRIMARIES_DISPLAY_P3), luminances: Some((50, 10_000, 203)), ..Default::default() };
    assert!(absolute(&full_support(), &pq_no_peak).unwrap_err().contains("no peak"));
}

#[test]
fn descriptions_read_in_words() {
    assert_eq!(kwin_6_8_hdr_preferred().describe(), "gamma 2.2 BT.2020, reference 250 nits, peak 700 nits, minimum 0.0050 nits");
    assert_eq!(DescriptionInfo { icc: true, ..Default::default() }.describe(), "an ICC profile");
    assert_eq!(DescriptionInfo { tf_power: Some(24_000), primaries_named: Some(PRIMARIES_SRGB), ..Default::default() }.describe(), "power 2.40 BT.709");
}

#[test]
fn the_launcher_draws_its_white_at_the_described_reference() {
    // The driver's description (203 nits) and the compositor's reference 250 nits: SDL's white
    // 3.125; the compositor scales 203 to 250.
    let scale = launcher_color_scale(true, false, 203.0, true, true, 250.0 / 80.0);
    assert!((scale - 203.0 / 250.0).abs() < 1e-6, "{scale}");
    // Nightjar's description at the output's reference, in PQ or the blending space.
    let ours = launcher_color_scale(true, false, 250.0, true, true, 250.0 / 80.0);
    assert!((ours - 1.0).abs() < 1e-6, "Nightjar's description has the output's reference: {ours}");
    assert_eq!(launcher_color_scale(true, false, 203.0, false, true, 3.125), 1.0, "a stream keeps its look");
    assert_eq!(launcher_color_scale(false, false, 203.0, true, true, 3.125), 1.0, "scRGB and SDR draw at 1.0 already");
    assert_eq!(launcher_color_scale(true, true, 203.0, true, true, 3.125), 1.0, "gamescope");
    assert_eq!(launcher_color_scale(true, false, 203.0, true, false, 3.125), 1.0, "switched off");
    assert_eq!(launcher_color_scale(true, false, 203.0, true, true, f32::NAN), 1.0);
    assert_eq!(launcher_color_scale(true, false, f32::NAN, true, true, 3.125), 1.0);
    assert_eq!(launcher_color_scale(true, false, 0.0, true, true, 3.125), 1.0);
    assert_eq!(launcher_color_scale(true, false, 203.0, true, true, 0.01), 10.0, "at most a factor of 10");
    let at_203 = launcher_color_scale(true, false, 203.0, true, true, 203.0 / 80.0);
    assert!((at_203 - 1.0).abs() < 1e-6, "203 nits needs nothing: {at_203}");
}

#[test]
fn vrr_tears_only_without_tear_free_vrr() {
    assert!(vrr_tearing(false, true));
    assert!(!vrr_tearing(true, true), "Tear-free VRR keeps Mailbox");
    assert!(!vrr_tearing(false, false), "switched off");
    assert_eq!(tearing_state(Some(1), true), "on (Immediate, the driver's async hint)");
    assert_eq!(tearing_state(Some(2), true), "off (Mailbox)");
    assert_eq!(tearing_state(Some(2), false), "off (Mailbox); the compositor offers no tearing control");
    assert_eq!(tearing_state(Some(0), true), "off (V-Sync)");
    assert_eq!(tearing_state(None, true), "unknown");
}

#[test]
fn swapchain_formats_are_mesas_drm_formats() {
    assert_eq!(fourcc_name(drm_fourcc(SwapFormat::Rgb10a2, true)), "XB30");
    assert_eq!(drm_fourcc(SwapFormat::Rgb10a2, true), 0x3033_4258, "DRM_FORMAT_XBGR2101010");
    assert_eq!(drm_fourcc(SwapFormat::Bgra8, true), 0x3432_5258, "DRM_FORMAT_XRGB8888");
    assert_eq!(fourcc_name(drm_fourcc(SwapFormat::Rgba16f, false)), "AB4H");
    assert_eq!(fourcc_name(drm_fourcc(SwapFormat::Rgba8, true)), "XB24");
    assert_eq!(fourcc_name(0x0000_0001), "????");
}

#[test]
fn globals_name_what_is_offered_and_what_is_not() {
    let mut globals = Globals::default();
    for (interface, version) in [("wp_color_manager_v1", 1), ("wp_tearing_control_manager_v1", 1), ("zwp_linux_dmabuf_v1", 5), ("zwp_linux_dmabuf_v1", 3), ("wl_seat", 9), ("frog_color_management_factory_v1", 1)] {
        globals.note(interface, version);
    }
    assert_eq!(globals.describe(), "offers color-manager v1, frog-color, tearing-control, dmabuf v5; not fifo, commit-timing, explicit-sync");
    assert_eq!(Globals::default().describe(), "offers none of the protocols looked for; not color-manager, tearing-control, fifo, commit-timing, dmabuf, explicit-sync");
}

#[test]
fn dmabuf_feedback_keeps_the_last_scanout_tranche() {
    let xb30 = fourcc(*b"XB30");
    let xr24 = fourcc(*b"XR24");
    let mut log = DmabufLog::default();
    assert_eq!(log.describe(Some(xb30)), "none received");
    log.add(&[(false, vec![(xb30, 0), (xr24, 0)])]);
    assert!(log.describe(Some(xb30)).starts_with("no scanout tranche in 1 sets"));
    assert!(!log.format_missing(Some(xb30)));
    log.add(&[(true, vec![(xr24, 0), (xr24, 1)]), (false, vec![(xb30, 0)])]);
    assert!(log.format_missing(Some(xb30)));
    assert!(!log.format_missing(Some(xr24)));
    assert_eq!(log.describe(Some(xb30)), "a scanout tranche in 1 of 2 sets (still there at the end), XB30 not in it");
    log.add(&[(false, vec![(xb30, 0)])]);
    assert_eq!(log.describe(Some(xr24)), "a scanout tranche in 1 of 3 sets (withdrawn at the end), XR24 in it with 2 modifiers");
    assert_eq!(log.describe(None), "a scanout tranche in 1 of 3 sets (withdrawn at the end), the swapchain's format unknown");
}

fn seen() -> Seen {
    let mut globals = Globals::default();
    for interface in ["wp_color_manager_v1", "wp_tearing_control_manager_v1", "wp_fifo_manager_v1", "wp_commit_timing_manager_v1", "wp_linux_drm_syncobj_manager_v1"] {
        globals.note(interface, 1);
    }
    globals.note("zwp_linux_dmabuf_v1", 5);
    let mut dmabuf = DmabufLog::default();
    dmabuf.add(&[(false, vec![(fourcc(*b"XB30"), 0)])]);
    Seen { globals, preferred: Ok(kwin_6_8_hdr_preferred()), dmabuf }
}

#[test]
fn the_line_says_why_kwin_composes_the_drivers_pq() {
    let seen = seen();
    let label = Label::Driver("NIGHTJAR_WAYLAND_HDR_DESCRIPTION is 0".into());
    let facts = SwapFacts { fourcc: Some(fourcc(*b"XB30")), present_mode: Some(2), recreates: 0, pass_through: false };
    let report = Report { seen: &seen, label: Some(&label), facts, zero_copy_seen: false, kde: true };
    assert_eq!(
        report.line(),
        "scanout: the compositor offers color-manager v1, tearing-control, fifo, commit-timing, dmabuf v5, explicit-sync; \
             it prefers gamma 2.2 BT.2020, reference 250 nits, peak 700 nits, minimum 0.0050 nits; \
             HDR description the driver's, PQ BT.2020 at the default reference 203 nits (NIGHTJAR_WAYLAND_HDR_DESCRIPTION is 0); \
             dmabuf feedback no scanout tranche in 1 sets (a compositor sends one when only the buffer's format or modifier stands in the way); \
             swapchain XB30, made again 0 times for it; tearing off (Mailbox); \
             against direct scanout: the surface is PQ and the compositor blends in gamma 2.2, so it converts each frame unless the display engine does \
             (KWin 6.6 and later: KWIN_DRM_USE_COLOR_PIPELINE=1, with a kernel that has plane colour pipelines); \
             the driver's description puts reference white at 203 nits and the output's is 250, so the compositor scales each frame; \
             KWin reports zero-copy only from Plasma 6.8, so 0% says nothing before it"
    );
}

#[test]
fn the_line_with_nightjars_description_and_nothing_against_it() {
    let mut seen = seen();
    seen.preferred = Ok(DescriptionInfo { tf_named: Some(TF_ST2084_PQ), primaries_named: Some(PRIMARIES_BT2020), luminances: Some((50, 10_000, 203)), target_luminance: Some((50, 800)), ..Default::default() });
    let label = Label::Ours { text: describe_label(HdrLabel::Preferred, seen.preferred.as_ref().unwrap()), blending: false, reference: 203 };
    let facts = SwapFacts { fourcc: Some(fourcc(*b"XB30")), present_mode: Some(1), recreates: 1, pass_through: true };
    let line = Report { seen: &seen, label: Some(&label), facts, zero_copy_seen: true, kde: false }.line();
    assert!(line.contains("HDR description Nightjar's, the compositor's preferred one (PQ BT.2020, reference 203 nits, peak 800 nits, minimum 0.0050 nits)"), "{line}");
    assert!(line.contains("made again 1 times for it; tearing on (Immediate, the driver's async hint); nothing seen against direct scanout"), "{line}");
}

#[test]
fn the_line_with_nightjars_blending_space_has_nothing_against_it() {
    // KWin 6.5 to 6.8: the surface in the compositor's own gamma 2.2 BT.2020 space.
    let seen = seen();
    let preferred = kwin_6_8_hdr_preferred();
    let blending = HdrLabel::Blending { min_lum: 50, max: 700, reference: 250 };
    let label = Label::Ours { text: describe_label(blending, &preferred), blending: true, reference: blending.reference(&preferred) };
    let facts = SwapFacts { fourcc: Some(fourcc(*b"XB30")), present_mode: Some(2), recreates: 0, pass_through: true };
    let line = Report { seen: &seen, label: Some(&label), facts, zero_copy_seen: true, kde: true }.line();
    assert!(line.contains("HDR description Nightjar's, the compositor's blending space drawn in gamma 2.2 (gamma 2.2 BT.2020, reference 250 nits"), "{line}");
    assert!(line.contains("swapchain XB30 (HDR10 pass-through)"), "{line}");
    assert!(line.ends_with("nothing seen against direct scanout"), "{line}");
}

#[test]
fn the_line_says_a_description_that_follows_the_desktop_scales_each_frame() {
    let seen = seen();
    let preferred = kwin_6_8_hdr_preferred();
    let desktop = HdrLabel::Desktop(PqParams { min_lum: 50, max_lum: 10_001, reference: 203, max_cll: 1000 });
    let label = Label::Ours { text: describe_label(desktop, &preferred), blending: false, reference: desktop.reference(&preferred) };
    let facts = SwapFacts { fourcc: Some(fourcc(*b"XB30")), present_mode: Some(2), recreates: 0, pass_through: true };
    let line = Report { seen: &seen, label: Some(&label), facts, zero_copy_seen: true, kde: false }.line();
    assert!(line.contains("(brightness follows the desktop)"), "{line}");
    assert!(line.contains("the surface is PQ and the compositor blends in gamma 2.2"), "{line}");
    assert!(line.contains("Nightjar's description puts reference white at 203 nits and the output's is 250, so the compositor scales each frame"), "{line}");
}

/// The binding against a real compositor: the probe's globals, preferred description and
/// dmabuf feedback, and Nightjar's description made, attached and dropped, with the connection
/// alive after each (a wrong request would end it). Under a headless Weston, with colour
/// management on (`color-management=true` in weston.ini's `[core]`, Weston 14 or later) and the
/// GL renderer for dmabuf feedback: `weston --backend=headless --renderer=gl --socket=nj-test &
/// WAYLAND_DISPLAY=nj-test cargo test -p nightjar-desktop -- --ignored --nocapture
/// stream::wl_scanout`; or under `kwin_wayland --virtual`. Without either it still checks the
/// probe and says what was missing.
#[cfg(target_os = "linux")]
#[test]
#[ignore = "needs a Wayland compositor (WAYLAND_DISPLAY)"]
fn a_compositor_is_probed_and_described() {
    use sdl3_sys::everything::*;
    // SAFETY: plain SDL calls on objects made here, on this thread.
    unsafe {
        SDL_SetHint(SDL_HINT_VIDEO_DRIVER, c"wayland".as_ptr());
        assert!(SDL_Init(SDL_INIT_VIDEO), "{}", crate::sdl::error());
        let window = SDL_CreateWindow(c"scanout".as_ptr(), 320, 240, SDL_WindowFlags::default());
        assert!(!window.is_null(), "{}", crate::sdl::error());
        let renderer = SDL_CreateRenderer(window, c"software".as_ptr());
        assert!(!renderer.is_null(), "{}", crate::sdl::error());
        let present = |frames: u32| {
            for frame in 0..frames {
                SDL_PumpEvents();
                SDL_SetRenderDrawColor(renderer, (frame * 4) as u8, 0, 0, 255);
                SDL_RenderClear(renderer);
                SDL_RenderPresent(renderer);
                std::thread::sleep(std::time::Duration::from_millis(16));
            }
        };
        present(5);
        let mut probe = Probe::open(window).expect("the probe opens on a Wayland window");
        for _ in 0..32 {
            present(1);
            probe.poll();
        }
        let seen = probe.seen().clone();
        let facts = SwapFacts { fourcc: Some(fourcc(*b"XR24")), present_mode: Some(0), ..Default::default() };
        eprintln!("{}", Report { seen: &seen, label: None, facts, zero_copy_seen: false, kde: false }.line());
        if seen.globals.color_manager > 0 {
            let preferred = seen.preferred.clone().expect("a colour manager describes its preferred description");
            eprintln!("preferred: {}", preferred.describe());
            match HdrDescription::prepare(window) {
                Ok(mut description) => {
                    description.attach();
                    eprintln!("ours: {}", description.describe());
                    // A compositor that took a parametric PQ description takes the desktop's one.
                    let parametric = description.describe().contains("PQ BT.2020 at");
                    install(description);
                    assert!(matches!(installed_reference(window), Some(Some(_))));
                    for _ in 0..16 {
                        present(1);
                        pump_installed();
                    }
                    // The setting changes the description in place, without a new renderer.
                    set_brightness(Brightness::Desktop);
                    for _ in 0..16 {
                        present(1);
                        pump_installed();
                    }
                    eprintln!("following the desktop: {:?}", label());
                    if parametric {
                        assert_eq!(installed_reference(window), Some(Some(PQ_DEFAULT_REFERENCE_NITS)));
                    }
                    set_brightness(Brightness::Absolute);
                    for _ in 0..16 {
                        present(1);
                        pump_installed();
                    }
                    assert!(uninstall(window));
                    assert_eq!(installed_reference(window), Some(None));
                    set_label(None);
                }
                Err(why) => eprintln!("no description of ours: {why}"),
            }
        } else {
            eprintln!("no wp_color_manager_v1: the description part is not checked");
        }
        present(5);
        drop(probe);
        // A protocol error would have ended the connection: a new probe could not round trip.
        let mut again = Probe::open(window).expect("the connection is alive");
        assert_eq!(again.seen().globals, seen.globals);
        drop(again);
        SDL_DestroyRenderer(renderer);
        SDL_DestroyWindow(window);
        SDL_Quit();
    }
}

#[test]
fn the_line_for_sdr_and_a_missing_scanout_format() {
    let mut seen = seen();
    seen.preferred = Err("the compositor offers no wp_color_manager_v1".into());
    seen.dmabuf.add(&[(true, vec![(fourcc(*b"XR24"), 0)])]);
    let facts = SwapFacts { fourcc: Some(fourcc(*b"XB4H")), present_mode: Some(2), ..Default::default() };
    let line = Report { seen: &seen, label: None, facts, zero_copy_seen: false, kde: false }.line();
    assert!(line.contains("it prefers unknown (the compositor offers no wp_color_manager_v1); HDR description not PQ (the driver's, if any)"), "{line}");
    assert!(line.ends_with("against direct scanout: the swapchain's format is not in the scanout tranche; an fp16 swapchain is converted for the display"), "{line}");
}
