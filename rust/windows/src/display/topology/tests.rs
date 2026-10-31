use super::super::monitor;
use super::*;
#[test]
fn a_headless_console_has_an_empty_topology_without_querying_empty_buffers() {
    let topology = Topology::query_with(
        |paths, modes| {
            *paths = 0;
            *modes = 0;
            ERROR_SUCCESS
        },
        |_, _, _, _| panic!("headless topology must not query empty arrays"),
    )
    .unwrap();
    assert!(topology.paths.is_empty());
    assert!(topology.modes.is_empty());
    assert!(topology.monitors().is_empty());
}
#[test]
fn an_inaccessible_desktop_is_not_reported_as_a_headless_topology() {
    for fail_sizes in [true, false] {
        let error = Topology::query_with(
            |paths, modes| {
                *paths = 1;
                *modes = 2;
                if fail_sizes {
                    ERROR_ACCESS_DENIED
                } else {
                    ERROR_SUCCESS
                }
            },
            |_, _, _, _| ERROR_ACCESS_DENIED,
        )
        .err()
        .expect("desktop access error");
        assert_eq!(
            error.downcast_ref::<windows::core::Error>().unwrap().code(),
            windows::core::HRESULT::from_win32(ERROR_ACCESS_DENIED.0)
        );
    }
}
#[test]
fn topology_retries_when_a_display_arrives_and_keeps_returned_counts() {
    let mut calls = 0;
    let topology = Topology::query_with(
        |paths, modes| {
            *paths = 2;
            *modes = 4;
            ERROR_SUCCESS
        },
        |np, paths, nm, modes| {
            calls += 1;
            assert_eq!((*np, *nm), (2, 4));
            assert_eq!((paths.len(), modes.len()), (2, 4));
            if calls == 1 {
                return ERROR_INSUFFICIENT_BUFFER;
            }
            paths[0].targetInfo.id = 7;
            *np = 1;
            *nm = 2;
            ERROR_SUCCESS
        },
    )
    .unwrap();
    assert_eq!(calls, 2);
    assert_eq!((topology.paths.len(), topology.modes.len()), (1, 2));
    assert_eq!(topology.paths[0].targetInfo.id, 7);
}
#[test]
fn an_inactive_display_can_have_no_modes_but_needs_a_real_buffer() {
    let topology = Topology::query_with(
        |paths, modes| {
            *paths = 1;
            *modes = 0;
            ERROR_SUCCESS
        },
        |_, _, nm, modes| {
            assert_eq!(*nm, 1);
            assert!(!modes.is_empty());
            *nm = 0;
            ERROR_SUCCESS
        },
    )
    .unwrap();
    assert_eq!(topology.paths.len(), 1);
    assert!(topology.modes.is_empty());
}
#[test]
#[ignore = "requires an interactive Windows desktop with connected displays"]
fn heartbeat_check_finds_the_monitors_that_monitors_lists() -> Result<()> {
    let topology = Topology::query()?;
    // SAFETY: Topology::shows supplies a live source- or target-name structure with a matching header type and size.
    let device_info = |header: &mut DISPLAYCONFIG_DEVICE_INFO_HEADER| unsafe {
        DisplayConfigGetDeviceInfo(header)
    };
    for monitor in monitors()? {
        assert!(
            topology.shows(&[(monitor.adapter, monitor.target)], &monitor, device_info)?,
            "{}",
            monitor.monitor_device_path
        );
        let other = Monitor {
            monitor_device_path: format!("{}#other", monitor.monitor_device_path),
            ..monitor.clone()
        };
        assert!(!topology.shows(&[(other.adapter, other.target)], &other, device_info)?);
        let moved = Monitor {
            target: monitor.target ^ 0x8000_0000,
            ..monitor
        };
        assert!(!topology.shows(&[(moved.adapter, moved.target)], &moved, device_info)?);
    }
    Ok(())
}
#[test]
fn heartbeat_device_info_errors_are_not_monitor_loss() {
    let owned = monitor("owned", 1, false);
    let mut path = DISPLAYCONFIG_PATH_INFO::default();
    path.targetInfo.adapterId = owned.adapter;
    path.targetInfo.id = owned.target;
    let topology = Topology {
        paths: vec![path],
        modes: vec![],
    };
    for fail_call in [1, 2] {
        let mut calls = 0;
        let result = topology.shows(&[(owned.adapter, owned.target)], &owned, |_| {
            calls += 1;
            if calls == fail_call {
                ERROR_BUSY.0 as i32
            } else {
                0
            }
        });
        assert_eq!(
            result
                .unwrap_err()
                .downcast_ref::<std::io::Error>()
                .unwrap()
                .raw_os_error(),
            Some(ERROR_BUSY.0 as i32)
        );
    }
    let empty = Topology {
        paths: vec![],
        modes: vec![],
    };
    assert!(
        !empty
            .shows(&[(owned.adapter, owned.target)], &owned, |_| panic!(
                "no target to query"
            ))
            .unwrap()
    );
}
#[test]
fn extending_a_cloned_desktop_assigns_distinct_sources_and_rejects_impossible_routes() {
    let path = |source, target| {
        let mut path = DISPLAYCONFIG_PATH_INFO::default();
        path.sourceInfo.id = source;
        path.targetInfo.id = target;
        path
    };
    let result = extended_paths(&[vec![path(0, 10), path(1, 10)], vec![path(0, 20)]]).unwrap();
    assert_eq!(result[0].sourceInfo.id, 1);
    assert_eq!(result[1].sourceInfo.id, 0);
    assert!(extended_paths(&[vec![path(0, 10)], vec![path(0, 20)]]).is_err());
}
#[test]
fn a_display_leaving_a_duplicate_group_is_moved_but_its_partner_and_new_displays_are_not() {
    let path = |source, target, active: bool| {
        let mut path = DISPLAYCONFIG_PATH_INFO::default();
        path.sourceInfo.id = source;
        path.targetInfo.id = target;
        path.flags = if active { DISPLAYCONFIG_PATH_ACTIVE } else { 0 };
        path
    };
    // Targets 10 and 11 duplicate source 1; target 12 is off.
    let all = [
        path(1, 10, true),
        path(1, 11, true),
        path(0, 10, false),
        path(0, 11, false),
        path(2, 12, false),
    ];
    let chosen = [path(0, 10, false), path(1, 11, true), path(2, 12, false)];
    assert_eq!(moving_targets(&chosen, &all), [true, false, false]);
}
#[test]
fn displays_that_stay_on_keep_their_modes_when_the_layout_changes() {
    let path = |source, target, mode: u32, active: bool| {
        let mut path = DISPLAYCONFIG_PATH_INFO::default();
        path.sourceInfo.id = source;
        path.targetInfo.id = target;
        path.sourceInfo.Anonymous.modeInfoIdx = mode;
        path.targetInfo.Anonymous.modeInfoIdx = mode + 1;
        path.flags = if active { DISPLAYCONFIG_PATH_ACTIVE } else { 0 };
        path.targetInfo.refreshRate = DISPLAYCONFIG_RATIONAL {
            Numerator: 120,
            Denominator: 1,
        };
        path.targetInfo.scanLineOrdering = DISPLAYCONFIG_SCANLINE_ORDERING_PROGRESSIVE;
        path
    };
    // An inactive route from QDC_ALL_PATHS has no rotation or scaling.
    let kept = keep_active_modes(&[path(0, 10, 4, true), path(1, 11, 6, false)]);
    assert!(kept.iter().all(|p| p.flags == DISPLAYCONFIG_PATH_ACTIVE));
    // SAFETY: The test initialized the source mode-index union field in this path.
    assert_eq!(unsafe { kept[0].sourceInfo.Anonymous.modeInfoIdx }, 4);
    // SAFETY: The test initialized the target mode-index union field in this path.
    assert_eq!(unsafe { kept[0].targetInfo.Anonymous.modeInfoIdx }, 5);
    assert_eq!(
        // SAFETY: keep_active_modes sets this source mode-index union field for the inactive path.
        unsafe { kept[1].sourceInfo.Anonymous.modeInfoIdx },
        DISPLAYCONFIG_PATH_MODE_IDX_INVALID
    );
    assert_eq!(
        // SAFETY: keep_active_modes sets this target mode-index union field for the inactive path.
        unsafe { kept[1].targetInfo.Anonymous.modeInfoIdx },
        DISPLAYCONFIG_PATH_MODE_IDX_INVALID
    );
    // The display that stays on keeps its timing; Windows chooses the
    // switched-on one's, which needs valid rotation and scaling.
    assert_eq!(kept[0].targetInfo.refreshRate.Numerator, 120);
    assert_eq!(kept[1].targetInfo.refreshRate.Denominator, 0);
    assert_eq!(
        kept[1].targetInfo.scanLineOrdering,
        DISPLAYCONFIG_SCANLINE_ORDERING_UNSPECIFIED
    );
    assert_eq!(kept[1].targetInfo.rotation, DISPLAYCONFIG_ROTATION_IDENTITY);
    assert_eq!(kept[1].targetInfo.scaling, DISPLAYCONFIG_SCALING_PREFERRED);
}
#[test]
fn switching_on_a_virtual_display_keeps_duplicated_tvs_and_their_modes() {
    let gpu = LUID {
        LowPart: 1,
        HighPart: 0,
    };
    let vdd = LUID {
        LowPart: 9,
        HighPart: 0,
    };
    let path = |adapter, source, target, mode: u32, active, available: bool| {
        let mut path = DISPLAYCONFIG_PATH_INFO::default();
        path.sourceInfo.adapterId = adapter;
        path.sourceInfo.id = source;
        path.sourceInfo.Anonymous.modeInfoIdx = mode;
        path.targetInfo.adapterId = adapter;
        path.targetInfo.id = target;
        path.targetInfo.Anonymous.modeInfoIdx = mode + 1;
        path.targetInfo.targetAvailable = available.into();
        path.targetInfo.refreshRate = DISPLAYCONFIG_RATIONAL {
            Numerator: 120,
            Denominator: 1,
        };
        path.targetInfo.scanLineOrdering = DISPLAYCONFIG_SCANLINE_ORDERING_PROGRESSIVE;
        path.flags = if active { DISPLAYCONFIG_PATH_ACTIVE } else { 0 };
        path
    };
    // Two TVs share GPU source 0 (duplicate). The virtual display's own
    // adapter source 0 already drives another client's display.
    let active = [
        path(gpu, 0, 10, 0, true, true),
        path(gpu, 0, 11, 0, true, true),
        path(vdd, 0, 1, 2, true, true),
    ];
    let mut all = active.to_vec();
    all.extend([
        path(gpu, 1, 10, 0, false, true),
        path(vdd, 0, 2, 0, false, true),
        path(vdd, 1, 2, 0, false, true),
    ]);
    let paths = activation_paths(&active, &all, &[(vdd, 2)])
        .unwrap()
        .unwrap();
    assert_eq!(paths.len(), 4);
    for (kept, original) in paths.iter().zip(&active) {
        assert_eq!(kept.targetInfo.id, original.targetInfo.id);
        assert_eq!(kept.sourceInfo.id, original.sourceInfo.id);
        // SAFETY: Both paths have their source mode-index union fields initialized by the test.
        assert_eq!(unsafe { kept.sourceInfo.Anonymous.modeInfoIdx }, unsafe {
            original.sourceInfo.Anonymous.modeInfoIdx
        });
        // SAFETY: Both paths have their target mode-index union fields initialized by the test.
        assert_eq!(unsafe { kept.targetInfo.Anonymous.modeInfoIdx }, unsafe {
            original.targetInfo.Anonymous.modeInfoIdx
        });
    }
    let new = paths[3];
    assert_eq!((new.targetInfo.adapterId, new.targetInfo.id), (vdd, 2));
    assert_eq!(new.sourceInfo.id, 1);
    assert_eq!(new.flags, DISPLAYCONFIG_PATH_ACTIVE);
    assert_eq!(
        // SAFETY: activation_paths initializes this source mode-index union field.
        unsafe { new.sourceInfo.Anonymous.modeInfoIdx },
        DISPLAYCONFIG_PATH_MODE_IDX_INVALID
    );
    assert_eq!(
        // SAFETY: activation_paths initializes this target mode-index union field.
        unsafe { new.targetInfo.Anonymous.modeInfoIdx },
        DISPLAYCONFIG_PATH_MODE_IDX_INVALID
    );
    assert_eq!(new.targetInfo.refreshRate.Denominator, 0);
    assert_eq!(
        new.targetInfo.scanLineOrdering,
        DISPLAYCONFIG_SCANLINE_ORDERING_UNSPECIFIED
    );
    assert_eq!(new.targetInfo.rotation, DISPLAYCONFIG_ROTATION_IDENTITY);
    assert_eq!(new.targetInfo.scaling, DISPLAYCONFIG_SCALING_PREFERRED);
    // Not connected yet: keep waiting rather than failing.
    assert!(
        activation_paths(&active, &all, &[(vdd, 3)])
            .unwrap()
            .is_none()
    );
    let mut gone = all.clone();
    gone.retain(|p| p.targetInfo.id != 2);
    gone.push(path(vdd, 0, 2, 0, false, false));
    assert!(
        activation_paths(&active, &gone, &[(vdd, 2)])
            .unwrap()
            .is_none()
    );
    // Every source of the adapter is in use.
    let busy = [
        active[0],
        active[1],
        active[2],
        path(vdd, 1, 5, 4, true, true),
    ];
    assert!(activation_paths(&busy, &all, &[(vdd, 2)]).is_err());
}

/// A display Windows brought up cloned with the panel lists only that
/// one route; switching it on asks for a source of its own.
#[test]
fn a_cloned_display_is_moved_to_a_source_of_its_own() {
    let gpu = LUID {
        LowPart: 7,
        HighPart: 0,
    };
    let path = |source, target, active| {
        let mut path = DISPLAYCONFIG_PATH_INFO::default();
        path.sourceInfo.adapterId = gpu;
        path.sourceInfo.id = source;
        path.targetInfo.adapterId = gpu;
        path.targetInfo.id = target;
        path.targetInfo.targetAvailable = true.into();
        path.flags = if active { DISPLAYCONFIG_PATH_ACTIVE } else { 0 };
        path
    };
    let active = [path(1, 10, true), path(1, 20, true)];
    // The panel's other route lists source 0 as one the adapter drives.
    let mut all = active.to_vec();
    all.push(path(0, 10, false));
    let paths = activation_paths(&active, &all, &[(gpu, 20)])
        .unwrap()
        .unwrap();
    assert_eq!(paths.len(), 2);
    assert_eq!(paths[0].sourceInfo.id, 1);
    assert_eq!((paths[1].targetInfo.id, paths[1].sourceInfo.id), (20, 0));
    // No listed source is free: the error says so; an unlisted source
    // is never tried.
    let Err(error) = activation_paths(&active, &active, &[(gpu, 20)]) else {
        panic!("an unlisted source was tried");
    };
    assert!(format!("{error:#}").contains("no free desktop source"));
    let busy = [path(0, 10, true), path(1, 11, true)];
    let mut all = busy.to_vec();
    all.push(path(1, 20, true));
    assert!(activation_paths(&busy, &all, &[(gpu, 20)]).is_err());
}

/// Windows lists a virtual display under the render adapter while it is
/// on and under the driver's adapter while it is off (Radeon 780M).
#[test]
fn activation_finds_a_display_by_either_of_its_routes() {
    let gpu = LUID {
        LowPart: 7,
        HighPart: 0,
    };
    let vdd = LUID {
        LowPart: 9,
        HighPart: 0,
    };
    let path = |adapter, source, target, active, available: bool| {
        let mut path = DISPLAYCONFIG_PATH_INFO::default();
        path.sourceInfo.adapterId = adapter;
        path.sourceInfo.id = source;
        path.targetInfo.adapterId = adapter;
        path.targetInfo.id = target;
        path.targetInfo.targetAvailable = available.into();
        path.flags = if active { DISPLAYCONFIG_PATH_ACTIVE } else { 0 };
        path
    };
    // The panel is on; the virtual display is off, available only at the
    // driver's pair, though Windows lists it at the GPU's pair when on.
    let active = [path(gpu, 0, 10, true, true)];
    let off = [
        active[0],
        path(vdd, 0, 2, false, true),
        path(gpu, 1, 20, false, false),
    ];
    let routes = [(gpu, 20), (vdd, 2)];
    let paths = activation_paths(&active, &off, &routes).unwrap().unwrap();
    assert_eq!(paths.len(), 2);
    assert_eq!(
        (paths[1].targetInfo.adapterId, paths[1].targetInfo.id),
        (vdd, 2)
    );
    // The GPU's pair alone finds nothing to switch on.
    assert!(
        activation_paths(&active, &off, &[(gpu, 20)])
            .unwrap()
            .is_none()
    );
    // On at the GPU's pair: switching it on again keeps one route for it.
    let on = [active[0], path(gpu, 1, 20, true, true)];
    let all = [on[0], on[1], path(vdd, 0, 2, false, false)];
    let paths = activation_paths(&on, &all, &routes).unwrap().unwrap();
    assert_eq!(paths.len(), 2);
    assert_eq!(paths[1].targetInfo.id, 20);
}

#[test]
fn shows_follows_a_display_that_changes_routes() {
    let gpu = LUID {
        LowPart: 7,
        HighPart: 0,
    };
    let vdd = LUID {
        LowPart: 9,
        HighPart: 0,
    };
    let mut off = DISPLAYCONFIG_PATH_INFO::default();
    off.targetInfo.adapterId = vdd;
    off.targetInfo.id = 2;
    let topology = Topology {
        paths: vec![off],
        modes: Vec::new(),
    };
    let mut monitor = monitor("m", gpu.LowPart, false);
    monitor.target = 20;
    monitor.monitor_device_path = r"\\?\DISPLAY#RBY0001#1&28a6823a&0&UID2#{guid}".into();
    let named = |header: &mut DISPLAYCONFIG_DEVICE_INFO_HEADER| {
        if header.r#type == DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME {
            // SAFETY: The header opens a DISPLAYCONFIG_TARGET_DEVICE_NAME.
            let target = unsafe {
                &mut *(header as *mut DISPLAYCONFIG_DEVICE_INFO_HEADER
                    as *mut DISPLAYCONFIG_TARGET_DEVICE_NAME)
            };
            for (slot, unit) in target
                .monitorDevicePath
                .iter_mut()
                .zip(monitor.monitor_device_path.encode_utf16())
            {
                *slot = unit;
            }
        }
        0
    };
    // Listed at the GPU's pair while on, so a lookup there finds nothing
    // when the display is off; the driver's pair finds it.
    assert!(
        !topology
            .shows(&[(monitor.adapter, monitor.target)], &monitor, named)
            .unwrap()
    );
    assert!(
        topology
            .shows(&[(gpu, 20), (vdd, 2)], &monitor, named)
            .unwrap()
    );
}
