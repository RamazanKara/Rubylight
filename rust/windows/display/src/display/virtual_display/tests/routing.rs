//! Tests for VRR timing, render adapter selection, route matching, and monitor path parsing.

use super::super::*;
#[test]
fn a_vrr_display_requests_1000_hz_in_the_drivers_millihertz_field() {
    for rate in [1_000_000, 116_000, 59_940] {
        let request = rubylight_vdd::add_request(7, (3840, 2160, rate), 1000);
        assert_eq!(
            (request.width, request.height, request.refresh_mhz),
            (3840, 2160, rate)
        );
        assert!(request.validate().is_ok());
    }
    // Windows takes the mode as a reduced rational.
    assert_eq!(
        butterpollo_media_policy::framegen::Rate(1_000_000).rational(),
        (1000, 1)
    );
    assert_eq!(
        butterpollo_media_policy::framegen::Rate(116_000).rational(),
        (116, 1)
    );
    assert_eq!(
        butterpollo_media_policy::framegen::Rate(59_940).rational(),
        (2997, 50)
    );
}
#[test]
fn a_selected_gpu_renders_the_virtual_display_and_automatic_leaves_it_to_windows() {
    // Issue #28: the integrated GPU rendered the virtual display while
    // the selected discrete Radeon was asked to duplicate it.
    assert_eq!(VirtualOptions::default().render_adapter, None);
    use butterpollo_config::config::Config;
    assert!(configured_render_adapter_with(&Config::default(), |_, _| Ok(None)).is_none());
    let missing = Config::parse("adapter_name = No Such GPU\n").unwrap();
    assert!(configured_render_adapter_with(&missing, |_, _| anyhow::bail!("")).is_none());
}
#[test]
fn a_display_windows_lists_under_the_render_adapter_is_still_ours() {
    // From the Ryzen 7 8845HS laptop: the driver reported the IddCx adapter
    // and target 258, and Windows listed the display on the Radeon 780M
    // under target 117440770, with the driver's target id as the UID.
    let luid = |low| LUID {
        LowPart: low,
        HighPart: 0,
    };
    let listing = |path: &str, adapter: u32, target: u32| Monitor {
        monitor_device_path: path.into(),
        adapter: luid(adapter),
        target,
        ..crate::display::monitor("m", adapter, false)
    };
    let panel = listing(
        r"\\?\DISPLAY#LEN8ABA#5&277e72b4&0&UID256#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}",
        0x10687,
        256,
    );
    let ours = listing(
        r"\\?\DISPLAY#RBY0001#1&28a6823a&0&UID258#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}",
        0x10687,
        117_440_770,
    );
    let found = |monitors: &[Monitor], target| {
        listed(monitors, luid(942_669_753), target, &[]).map(|m| m.target)
    };
    assert_eq!(
        found(&[panel.clone(), ours.clone()], 258),
        Some(117_440_770)
    );
    // The UID is the driver's target id: another id is another display, and
    // the panel is never taken for it.
    assert_eq!(found(&[panel.clone(), ours.clone()], 259), None);
    assert_eq!(found(&[panel], 256), None);
    // The route the driver reported wins when Windows lists it there.
    let direct = listing(&ours.monitor_device_path, 942_669_753, 258);
    assert_eq!(found(&[ours.clone(), direct], 258), Some(258));
    // A route without a device path cannot identify the display yet.
    let unnamed = listing("", 942_669_753, 258);
    assert_eq!(found(std::slice::from_ref(&unnamed), 258), None);
    assert_eq!(found(&[unnamed, ours.clone()], 258), Some(117_440_770));
    // The pair another session's display goes by is never taken.
    let taken = [((0x10687, 0), 117_440_770)];
    assert_eq!(
        listed(std::slice::from_ref(&ours), luid(942_669_753), 258, &taken).map(|m| m.target),
        None
    );
}
#[test]
fn a_display_is_followed_to_a_new_pair_by_its_device_path() {
    // Windows listed it at 117440770 before it was switched off and at
    // 117440783 after; the device path is the same.
    let listing = |path: &str, target| Monitor {
        monitor_device_path: path.into(),
        target,
        ..crate::display::monitor("m", 0x10687, false)
    };
    let path = r"\\?\DISPLAY#RBY0001#1&28a6823a&0&UID258#{g}";
    let panel = listing(r"\\?\DISPLAY#LEN8ABA#5&277e72b4&0&UID256#{g}", 256);
    let moved = listing(&path.to_lowercase(), 117_440_783);
    let found = listed_by_path(vec![panel.clone(), moved], path);
    assert_eq!(found.map(|m| m.target), Some(117_440_783));
    // Another display, or no path at all, is never taken for it.
    assert!(listed_by_path(vec![panel], path).is_none());
    assert!(listed_by_path(vec![listing("", 5)], "").is_none());
}
#[test]
fn a_display_that_stays_duplicated_is_refused_with_the_reason() {
    let message = still_duplicated("no free desktop source for the virtual display");
    assert!(message.contains("duplicated with a physical display"));
    assert!(message.contains("no free desktop source"));
}
#[test]
fn a_display_on_the_panels_source_is_duplicated_and_not_ready() {
    let listing = |path: &str, target, source| Monitor {
        monitor_device_path: path.into(),
        target,
        source,
        ..crate::display::monitor("m", 0x10687, false)
    };
    let panel = listing(r"\\?\DISPLAY#LEN8ABA#5&277e72b4&0&UID256#{g}", 256, 0);
    let ours = listing(
        r"\\?\DISPLAY#RBY0001#1&28a6823a&0&UID258#{g}",
        117_440_770,
        0,
    );
    let other_virtual = listing(
        r"\\?\DISPLAY#RBY0001#1&28a6823a&0&UID259#{g}",
        117_440_771,
        0,
    );
    assert!(cloned_with_physical(&[panel.clone(), ours.clone()], &ours));
    // Alone, on a source of its own, or duplicated with a virtual display only.
    assert!(!cloned_with_physical(std::slice::from_ref(&ours), &ours));
    let own_source = Monitor {
        source: 1,
        ..ours.clone()
    };
    assert!(!cloned_with_physical(
        &[panel.clone(), own_source.clone()],
        &own_source
    ));
    assert!(!cloned_with_physical(&[other_virtual, ours.clone()], &ours));
    // Another adapter's source 0 is another desktop source.
    let elsewhere = Monitor {
        adapter: LUID {
            LowPart: 1,
            HighPart: 0,
        },
        ..panel
    };
    assert!(!cloned_with_physical(&[elsewhere, ours.clone()], &ours));
}
#[test]
fn virtual_monitor_paths_name_the_driver_target_in_their_uid() {
    use crate::display::topology::is_virtual_monitor;
    let path = r"\\?\DISPLAY#RBY0001#1&28a6823a&0&UID258#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}";
    assert!(is_virtual_monitor(path, 258));
    assert!(is_virtual_monitor(&path.replace("RBY", "rby"), 258));
    assert!(!is_virtual_monitor(path, 25), "a prefix of the id");
    assert!(!is_virtual_monitor(path, 58), "a suffix of the id");
    assert!(is_virtual_monitor(&path.replace("UID", "uid"), 258));
    let longer = path.replace("UID258", "UID2580");
    assert!(!is_virtual_monitor(&longer, 258), "UID2580 is not 258");
    assert!(is_virtual_monitor(&longer, 2580));
    assert!(!is_virtual_monitor(path, 2580), "UID258 is not 2580");
    assert!(!is_virtual_monitor(&path.replace("RBY", "LEN"), 258));
    assert!(!is_virtual_monitor("", 258));
    assert!(
        !is_virtual_monitor("a#\u{e4}\u{e4}#UID258", 258),
        "non-ASCII maker"
    );
}
#[test]
fn a_monitor_path_says_physical_only_when_it_can_be_read() {
    use crate::display::topology::is_physical_monitor;
    let path = r"\\?\DISPLAY#SAM7456#5&1b2c3d&0&UID516#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}";
    assert!(is_physical_monitor(path));
    assert!(!is_physical_monitor(&path.replace("SAM7456", "RBY0001")));
    assert!(!is_physical_monitor(""), "unreadable for the moment");
}
#[test]
fn the_failure_says_what_switching_the_display_on_came_to() {
    assert_eq!(
        not_active(false, "not tried"),
        "virtual display did not become active before the deadline"
    );
    assert_eq!(
        not_active(true, "Windows refused the display layout"),
        "virtual display was connected but Windows kept it switched off (switching it on: Windows refused the display layout)"
    );
}
#[test]
fn a_restarted_driver_is_given_the_render_gpu_under_its_live_luid() {
    let card = |luid| RenderAdapter {
        name: "AMD Radeon RX 7900 XT".into(),
        luid,
    };
    let frozen = Some(card((2, 0)));
    // A GPU reset or a driver update listed the card under a new LUID: the
    // display asks for that one, never the frozen one.
    let moved = RenderResolver::new(move || Ok(Some(card((7, 0)))));
    assert_eq!(
        reresolve_render_adapter(frozen.clone(), Some(&moved)),
        Some(card((7, 0)))
    );
    // A chosen GPU that is gone: none, so Windows renders it and capture
    // fails with the reason, never a silent move to another GPU.
    let gone = RenderResolver::new(|| {
        anyhow::bail!("The GPU chosen under Graphics card is not available")
    });
    assert_eq!(reresolve_render_adapter(frozen.clone(), Some(&gone)), None);
    // Automatic on a PC with one GPU: still left to Windows.
    let windows = RenderResolver::new(|| Ok(None));
    assert_eq!(
        reresolve_render_adapter(frozen.clone(), Some(&windows)),
        None
    );
    // No resolver (a display created without one): the adapter stays.
    assert_eq!(reresolve_render_adapter(frozen.clone(), None), frozen);
    assert!(VirtualOptions::default().render_resolver.is_none());
}
