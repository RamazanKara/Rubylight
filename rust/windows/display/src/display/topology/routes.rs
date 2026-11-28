//! Display configuration route assignment, target activation, and clone group restoration.

use super::super::*;
use super::{Monitor, Route, Topology, monitor_id};

pub(super) fn extended_paths(
    candidates: &[Vec<DISPLAYCONFIG_PATH_INFO>],
) -> Result<Vec<DISPLAYCONFIG_PATH_INFO>> {
    type Source = (u32, i32, u32);
    fn assign(
        target: usize,
        candidates: &[Vec<DISPLAYCONFIG_PATH_INFO>],
        chosen: &mut [Option<DISPLAYCONFIG_PATH_INFO>],
        occupied: &mut std::collections::BTreeMap<Source, usize>,
        visited: &mut std::collections::BTreeSet<Source>,
    ) -> bool {
        for path in &candidates[target] {
            let source = (
                path.sourceInfo.adapterId.LowPart,
                path.sourceInfo.adapterId.HighPart,
                path.sourceInfo.id,
            );
            if !visited.insert(source) {
                continue;
            }
            if occupied
                .get(&source)
                .copied()
                .is_none_or(|previous| assign(previous, candidates, chosen, occupied, visited))
            {
                occupied.insert(source, target);
                chosen[target] = Some(*path);
                return true;
            }
        }
        false
    }
    let mut chosen = vec![None; candidates.len()];
    let mut occupied = std::collections::BTreeMap::new();
    for target in 0..candidates.len() {
        if !assign(
            target,
            candidates,
            &mut chosen,
            &mut occupied,
            &mut Default::default(),
        ) {
            bail!("connected displays cannot be assigned independent desktop sources");
        }
    }
    Ok(chosen.into_iter().flatten().collect())
}
/// Assign independent desktop sources while preserving each display's saved
/// source ID (and source 0 for the saved primary when no source ID was saved).
/// Higher-priority targets run last in the augmenting-path matching so they
/// reclaim their preferred source from unpreferred targets that have an
/// alternative, without ever blocking a valid assignment.
pub(super) fn extended_paths_preferred(
    targets: &[(Vec<DISPLAYCONFIG_PATH_INFO>, Option<u32>, bool)],
) -> Result<Vec<DISPLAYCONFIG_PATH_INFO>> {
    let mut sorted: Vec<Vec<DISPLAYCONFIG_PATH_INFO>> = targets
        .iter()
        .map(|(candidates, preferred, is_primary)| {
            let mut list = candidates.clone();
            list.sort_by_key(|p| {
                (
                    preferred.is_some_and(|s| p.sourceInfo.id != s),
                    !(preferred.is_none() && *is_primary && p.sourceInfo.id == 0),
                    p.flags & DISPLAYCONFIG_PATH_ACTIVE == 0,
                    p.sourceInfo.id,
                )
            });
            list
        })
        .collect();
    let mut order: Vec<usize> = (0..targets.len()).collect();
    order.sort_by_key(|&i| {
        let (_, preferred, is_primary) = &targets[i];
        match (preferred.is_some(), *is_primary) {
            (false, false) => 0u8,
            (false, true) => 1,
            (true, false) => 2,
            (true, true) => 3,
        }
    });
    let reordered: Vec<_> = order
        .iter()
        .map(|&i| std::mem::take(&mut sorted[i]))
        .collect();
    let assigned = extended_paths(&reordered)?;
    let mut restored = vec![None; targets.len()];
    for (pos, path) in order.into_iter().zip(assigned) {
        restored[pos] = Some(path);
    }
    Ok(restored.into_iter().flatten().collect())
}
/// An inactive route from QDC_ALL_PATHS made fit to switch on: Windows
/// chooses its modes, refresh and position. Such routes carry a zero
/// rotation and scaling and a stale refresh, which SetDisplayConfig refuses
/// with ERROR_INVALID_PARAMETER.
fn switch_on_route(path: &mut DISPLAYCONFIG_PATH_INFO, preferred_rotation: Option<u32>) {
    path.flags = DISPLAYCONFIG_PATH_ACTIVE;
    path.sourceInfo.Anonymous.modeInfoIdx = DISPLAYCONFIG_PATH_MODE_IDX_INVALID;
    path.targetInfo.Anonymous.modeInfoIdx = DISPLAYCONFIG_PATH_MODE_IDX_INVALID;
    // A zero refresh lets Windows pick the display's best rate; it requires
    // unspecified scan-line ordering.
    path.targetInfo.refreshRate = DISPLAYCONFIG_RATIONAL::default();
    path.targetInfo.scanLineOrdering = DISPLAYCONFIG_SCANLINE_ORDERING_UNSPECIFIED;
    if let Some(rotation) = preferred_rotation.filter(|r| (1..=4).contains(r)) {
        path.targetInfo.rotation = DISPLAYCONFIG_ROTATION(rotation as i32);
    } else if path.targetInfo.rotation.0 == 0 {
        path.targetInfo.rotation = DISPLAYCONFIG_ROTATION_IDENTITY;
    }
    if path.targetInfo.scaling.0 == 0 {
        path.targetInfo.scaling = DISPLAYCONFIG_SCALING_PREFERRED;
    }
}
/// Routes for an error report: source > target, flags, source/target mode
/// index, rotation, scaling, refresh and scan-line ordering.
fn describe_paths(paths: &[DISPLAYCONFIG_PATH_INFO]) -> String {
    paths
        .iter()
        // SAFETY: These paths use the non-virtual mode-index union fields populated by QueryDisplayConfig.
        .map(|p| unsafe {
            format!(
                "{}:{}>{}:{} f{} m{}/{} r{} s{} {}/{} o{}",
                p.sourceInfo.adapterId.LowPart,
                p.sourceInfo.id,
                p.targetInfo.adapterId.LowPart,
                p.targetInfo.id,
                p.flags,
                p.sourceInfo.Anonymous.modeInfoIdx as i32,
                p.targetInfo.Anonymous.modeInfoIdx as i32,
                p.targetInfo.rotation.0,
                p.targetInfo.scaling.0,
                p.targetInfo.refreshRate.Numerator,
                p.targetInfo.refreshRate.Denominator,
                p.targetInfo.scanLineOrdering.0,
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}
/// For each chosen route, whether it moves a display that is on to another
/// desktop source: the route is inactive but its target has an active one.
pub(super) fn moving_targets(
    chosen: &[DISPLAYCONFIG_PATH_INFO],
    all: &[DISPLAYCONFIG_PATH_INFO],
) -> Vec<bool> {
    chosen
        .iter()
        .map(|path| {
            path.flags & DISPLAYCONFIG_PATH_ACTIVE == 0
                && all.iter().any(|other| {
                    other.flags & DISPLAYCONFIG_PATH_ACTIVE != 0
                        && other.targetInfo.adapterId == path.targetInfo.adapterId
                        && other.targetInfo.id == path.targetInfo.id
                })
        })
        .collect()
}
/// Chosen routes with the mode indices of those already active, so their
/// displays keep timing and position; newly switched-on routes get none and
/// start in their saved orientation when provided.
pub(super) fn keep_active_modes(
    paths: &[DISPLAYCONFIG_PATH_INFO],
    preferred_rotations: &[Option<u32>],
) -> Vec<DISPLAYCONFIG_PATH_INFO> {
    paths
        .iter()
        .enumerate()
        .map(|(i, path)| {
            let mut path = *path;
            if path.flags & DISPLAYCONFIG_PATH_ACTIVE == 0 {
                switch_on_route(&mut path, preferred_rotations.get(i).copied().flatten());
            }
            path.flags = DISPLAYCONFIG_PATH_ACTIVE;
            path
        })
        .collect()
}
/// The active paths plus one route that switches `target` on, or None while
/// the target is not connected. Active paths keep their mode indices, so the
/// other displays keep their timings, positions and clone groups; Windows
/// chooses the new display's mode, refresh and position.
pub(super) fn activation_paths(
    active: &[DISPLAYCONFIG_PATH_INFO],
    all: &[DISPLAYCONFIG_PATH_INFO],
    routes: &[Route],
) -> Result<Option<Vec<DISPLAYCONFIG_PATH_INFO>>> {
    let ours = |p: &DISPLAYCONFIG_PATH_INFO| {
        routes.iter().any(|(adapter, target)| {
            p.targetInfo.adapterId == *adapter && p.targetInfo.id == *target
        })
    };
    let source = |p: &DISPLAYCONFIG_PATH_INFO| {
        (
            p.sourceInfo.adapterId.LowPart,
            p.sourceInfo.adapterId.HighPart,
            p.sourceInfo.id,
        )
    };
    let available: Vec<_> = all
        .iter()
        .filter(|p| ours(p) && p.targetInfo.targetAvailable.as_bool())
        .collect();
    if available.is_empty() {
        return Ok(None);
    }
    let occupied: std::collections::BTreeSet<_> =
        active.iter().filter(|p| !ours(p)).map(source).collect();
    // Windows lists a display that is on only at the source it is on (the
    // panel's, when it came up cloned). The other sources the adapter lists
    // for its other displays are free to ask for; one nothing lists is not.
    let free_offered = available.iter().find(|p| !occupied.contains(&source(p)));
    let mut route = match free_offered {
        Some(p) => **p,
        None => {
            let template = *available[0];
            let adapter = template.sourceInfo.adapterId;
            let free = all
                .iter()
                .filter(|p| p.sourceInfo.adapterId == adapter)
                .map(|p| p.sourceInfo.id)
                .filter(|id| !occupied.contains(&(adapter.LowPart, adapter.HighPart, *id)))
                .min()
                .with_context(|| {
                    let offered: Vec<_> = available.iter().map(|p| **p).collect();
                    format!(
                        "no free desktop source for the virtual display (offered routes: {}; sources in use: {:?})",
                        describe_paths(&offered),
                        occupied
                    )
                })?;
            let mut route = template;
            route.sourceInfo.id = free;
            route
        }
    };
    switch_on_route(&mut route, None);
    let mut paths: Vec<_> = active.iter().filter(|p| !ours(p)).copied().collect();
    paths.push(route);
    Ok(Some(paths))
}
/// Switch on a connected display beside the current desktop. Returns None
/// while the target is not connected, otherwise whether the other displays
/// kept their timings. `routes` are the (adapter, target) pairs the display
/// goes by: Windows can list it under one pair while it is on and another
/// while it is off.
pub(in super::super) fn activate_target(routes: &[Route]) -> Result<Option<bool>> {
    let active = Topology::query()?;
    let all = Topology::query_all()?;
    let Some(paths) = activation_paths(&active.paths, &all.paths, routes)? else {
        return Ok(None);
    };
    let flags = SDC_APPLY | SDC_USE_SUPPLIED_DISPLAY_CONFIG | SDC_ALLOW_CHANGES;
    let (compacted_paths, compacted_modes) =
        super::super::hotplug::referenced_modes(&paths, &active.modes)?;
    let supplied_modes = (!compacted_modes.is_empty()).then_some(compacted_modes.as_slice());
    // SAFETY: The compacted path and mode slices remain valid for this synchronous display configuration call.
    let kept = unsafe { SetDisplayConfig(Some(&compacted_paths), supplied_modes, flags) };
    if kept == 0 {
        return Ok(Some(true));
    }
    // Some drivers refuse supplied modes beside an unspecified one. Letting
    // Windows choose every mode can retime the other displays, which the
    // stream's layout restore undoes; a stream that cannot start cannot.
    let mut loose = paths;
    for path in &mut loose {
        path.sourceInfo.Anonymous.modeInfoIdx = DISPLAYCONFIG_PATH_MODE_IDX_INVALID;
        path.targetInfo.Anonymous.modeInfoIdx = DISPLAYCONFIG_PATH_MODE_IDX_INVALID;
    }
    // SAFETY: The paths remain valid for the call and their invalid mode indices ask Windows to choose modes.
    check(unsafe { SetDisplayConfig(Some(&loose), None, flags) }).with_context(|| {
        format!(
            "Windows refused the display layout with the virtual display (supplied modes: {})",
            std::io::Error::from_raw_os_error(kept)
        )
    })?;
    Ok(Some(false))
}

impl Topology {
    pub(in super::super) fn clone_groups(&self) -> Vec<Vec<String>> {
        Self::clone_groups_of(&self.monitors())
    }
    /// [`Self::clone_groups`] for monitors already listed, which cost a DXGI enumeration.
    pub(in super::super) fn clone_groups_of(monitors: &[Monitor]) -> Vec<Vec<String>> {
        let mut groups = std::collections::BTreeMap::<(u32, i32, u32), Vec<String>>::new();
        for monitor in monitors {
            groups
                .entry((
                    monitor.adapter.LowPart,
                    monitor.adapter.HighPart,
                    monitor.source,
                ))
                .or_default()
                .push(monitor.device_id.clone());
        }
        groups
            .into_values()
            .filter(|group| group.len() > 1)
            .collect()
    }
    pub(in super::super) fn restore_clone_groups(&mut self, groups: &[Vec<String>]) -> Result<()> {
        let monitors = self.monitors();
        let mut changed = false;
        let mut members = Vec::new();
        for group in groups {
            let selected: Vec<_> = group
                .iter()
                .filter_map(|id| monitors.iter().find(|m| m.device_id == *id))
                .collect();
            let Some(first) = selected.first() else {
                continue;
            };
            let source = self
                .paths
                .iter()
                .find(|p| {
                    p.targetInfo.adapterId == first.adapter && p.targetInfo.id == first.target
                })
                .context("clone source unavailable")?
                .sourceInfo;
            for monitor in &selected[1..] {
                if monitor.adapter != first.adapter {
                    bail!("clone targets now belong to different adapters");
                }
                let path = self
                    .paths
                    .iter_mut()
                    .find(|p| {
                        p.targetInfo.adapterId == monitor.adapter
                            && p.targetInfo.id == monitor.target
                    })
                    .context("clone target unavailable")?;
                path.sourceInfo = source;
                path.targetInfo.Anonymous.modeInfoIdx = DISPLAYCONFIG_PATH_MODE_IDX_INVALID;
                changed = true;
            }
            members.extend(selected.iter().map(|m| (m.adapter, m.target)));
        }
        if !changed {
            return Ok(());
        }
        let Err(exact) = self.restore() else {
            super::super::end_trace::committed("clone groups", false);
            return Ok(());
        };
        // Windows can refuse to show the first member's desktop mode on the
        // others (ERROR_GEN_FAILURE). Let it choose a mode the whole group
        // can show; the caller sets every display's mode again afterwards.
        for path in &mut self.paths {
            if members.iter().any(|(adapter, target)| {
                path.targetInfo.adapterId == *adapter && path.targetInfo.id == *target
            }) {
                path.sourceInfo.Anonymous.modeInfoIdx = DISPLAYCONFIG_PATH_MODE_IDX_INVALID;
                path.targetInfo.Anonymous.modeInfoIdx = DISPLAYCONFIG_PATH_MODE_IDX_INVALID;
            }
        }
        if self.restore().is_ok() {
            super::super::end_trace::committed("clone groups", false);
            return Ok(());
        }
        let mut loose = self.paths.clone();
        for path in &mut loose {
            path.sourceInfo.Anonymous.modeInfoIdx = DISPLAYCONFIG_PATH_MODE_IDX_INVALID;
            path.targetInfo.Anonymous.modeInfoIdx = DISPLAYCONFIG_PATH_MODE_IDX_INVALID;
        }
        let flags = SDC_APPLY | SDC_USE_SUPPLIED_DISPLAY_CONFIG | SDC_ALLOW_CHANGES;
        // SAFETY: The paths remain valid for the call and their mode indices have been invalidated.
        check(unsafe { SetDisplayConfig(Some(&loose), None, flags) }).with_context(|| {
            format!("Windows refused the cloned layout (saved modes: {exact:#})")
        })?;
        super::super::end_trace::committed("clone groups, Windows' modes", false);
        Ok(())
    }
    pub fn set_active(ids: &[String]) -> Result<()> {
        Self::set_active_preferred(
            ids,
            &std::collections::BTreeMap::new(),
            &std::collections::BTreeMap::new(),
            None,
        )
    }
    /// Activate `ids`, preferring each display's saved CCD desktop source ID
    /// (and source 0 for `primary` when no source ID was saved) and saved
    /// rotation so Windows restores the original GDI display names,
    /// orientation, and per-display DPI scale.
    pub fn set_active_preferred(
        ids: &[String],
        preferred_sources: &std::collections::BTreeMap<String, u32>,
        preferred_rotations: &std::collections::BTreeMap<String, u32>,
        primary: Option<&str>,
    ) -> Result<()> {
        Self::set_active_once(ids, preferred_sources, preferred_rotations, primary, true)
    }
    fn set_active_once(
        ids: &[String],
        preferred_sources: &std::collections::BTreeMap<String, u32>,
        preferred_rotations: &std::collections::BTreeMap<String, u32>,
        primary: Option<&str>,
        may_move: bool,
    ) -> Result<()> {
        if ids.is_empty() {
            bail!("display topology cannot be empty");
        }
        let topology = Self::query_all()?;
        let (candidates_by_target, target_rotations) = collect_active_target_candidates(
            &topology.paths,
            ids,
            preferred_sources,
            preferred_rotations,
            primary,
            |adapter, target_id| {
                // SAFETY: The initialized target-name structure has the matching header type and size and lives through the query.
                unsafe {
                    let mut name = DISPLAYCONFIG_TARGET_DEVICE_NAME {
                        header: header(
                            DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
                            size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>(),
                            adapter,
                            target_id,
                        ),
                        ..Default::default()
                    };
                    if DisplayConfigGetDeviceInfo(&mut name.header) != 0 {
                        return None;
                    }
                    let device_path = wide(&name.monitorDevicePath);
                    let id = monitor_id(&device_path);
                    Some((device_path, id))
                }
            },
        );
        let paths = extended_paths_preferred(&candidates_by_target)?;
        if paths.is_empty() {
            bail!("no saved display targets are currently connected");
        }
        let flags = SDC_APPLY | SDC_USE_SUPPLIED_DISPLAY_CONFIG | SDC_ALLOW_CHANGES;
        // Displays that stay on their current source keep their timing and
        // position, such as a retained remote monitor that a restore must
        // leave alone; Windows chooses for the displays it switches on.
        // Windows can refuse that mix, so fall back to choosing every mode.
        let moving = moving_targets(&paths, &topology.paths);
        let mut paths = keep_active_modes(&paths, &target_rotations);
        // SAFETY: These paths use the non-virtual mode-index union field returned by QueryDisplayConfig.
        let kept = if paths.iter().any(|p| unsafe { p.sourceInfo.Anonymous.modeInfoIdx }
            != DISPLAYCONFIG_PATH_MODE_IDX_INVALID)
        {
            let (compacted_paths, compacted_modes) =
                super::super::hotplug::referenced_modes(&paths, &topology.modes)?;
            let supplied_modes =
                (!compacted_modes.is_empty()).then_some(compacted_modes.as_slice());
            // SAFETY: The compacted path and mode slices remain valid for this synchronous display configuration call.
            unsafe { SetDisplayConfig(Some(&compacted_paths), supplied_modes, flags) }
        } else {
            -1
        };
        if kept == 0 {
            super::super::end_trace::committed("switch on", false);
            return Ok(());
        }
        // Windows refuses a display that changes desktop source while it is
        // on, such as one leaving a duplicate group (ERROR_INVALID_PARAMETER).
        // Switch it off with the others unchanged, then on at its new source.
        if may_move && moving.contains(&true) && moving.contains(&false) {
            let staying: Vec<_> = paths
                .iter()
                .zip(&moving)
                .filter(|(_, moving)| !**moving)
                .map(|(path, _)| *path)
                .collect();
            let (staying_paths, staying_modes) =
                super::super::hotplug::referenced_modes(&staying, &topology.modes)?;
            let supplied_modes = (!staying_modes.is_empty()).then_some(staying_modes.as_slice());
            // SAFETY: The compacted staying paths and modes remain valid for the synchronous call.
            check(unsafe { SetDisplayConfig(Some(&staying_paths), supplied_modes, flags) })
                .context("cannot switch off the displays that change desktop source")?;
            super::super::end_trace::committed("switch off the displays that change source", false);
            return Self::set_active_once(
                ids,
                preferred_sources,
                preferred_rotations,
                primary,
                false,
            );
        }
        let supplied = describe_paths(&paths);
        for path in &mut paths {
            path.sourceInfo.Anonymous.modeInfoIdx = DISPLAYCONFIG_PATH_MODE_IDX_INVALID;
            path.targetInfo.Anonymous.modeInfoIdx = DISPLAYCONFIG_PATH_MODE_IDX_INVALID;
        }
        // SAFETY: The paths remain valid for the call and their mode indices have been invalidated.
        unsafe { check(SetDisplayConfig(Some(&paths), None, flags)) }.with_context(|| {
            format!("Windows refused the display layout (with modes: {kept}; routes: {supplied})")
        })?;
        // Issue #129: every position and the primary are Windows' choice now.
        super::super::end_trace::committed("switch on, placed by Windows", true);
        Ok(())
    }
}

type TargetCandidates = (
    Vec<(Vec<DISPLAYCONFIG_PATH_INFO>, Option<u32>, bool)>,
    Vec<Option<u32>>,
);

/// Filter available target paths and resolve each unique `(adapter, target)`
/// identity at most once before matching against `ids`.
pub(super) fn collect_active_target_candidates(
    paths: &[DISPLAYCONFIG_PATH_INFO],
    ids: &[String],
    preferred_sources: &std::collections::BTreeMap<String, u32>,
    preferred_rotations: &std::collections::BTreeMap<String, u32>,
    primary: Option<&str>,
    mut lookup_target: impl FnMut(LUID, u32) -> Option<(String, String)>,
) -> TargetCandidates {
    let mut target_names: std::collections::BTreeMap<(u32, i32, u32), Option<(String, String)>> =
        std::collections::BTreeMap::new();
    let mut available_paths: Vec<(DISPLAYCONFIG_PATH_INFO, (u32, i32, u32))> = Vec::new();
    for path in paths {
        if !path.targetInfo.targetAvailable.as_bool() {
            continue;
        }
        let adapter = path.targetInfo.adapterId;
        let target_id = path.targetInfo.id;
        let key = (adapter.LowPart, adapter.HighPart, target_id);
        target_names
            .entry(key)
            .or_insert_with(|| lookup_target(adapter, target_id));
        available_paths.push((*path, key));
    }

    let mut candidates_by_target = Vec::new();
    let mut target_rotations = Vec::new();
    for id in ids {
        let candidates = available_paths
            .iter()
            .filter(|(_, key)| {
                target_names.get(key).and_then(Option::as_ref).is_some_and(
                    |(device_path, resolved_id)| {
                        device_path.eq_ignore_ascii_case(id) || resolved_id.eq_ignore_ascii_case(id)
                    },
                )
            })
            .map(|(path, _)| *path)
            .collect::<Vec<_>>();
        if !candidates.is_empty() {
            let preferred = preferred_sources.get(id).copied();
            let preferred_rotation = preferred_rotations.get(id).copied();
            let is_primary = primary.is_some_and(|p| p.eq_ignore_ascii_case(id));
            candidates_by_target.push((candidates, preferred, is_primary));
            target_rotations.push(preferred_rotation);
        }
    }
    (candidates_by_target, target_rotations)
}
