//! Display identities, topology queries, and desktop route changes.

use super::modes::{apply_mode_with, timing_matches};
use super::*;

#[derive(Clone, Serialize)]
pub struct Monitor {
    pub device_id: String,
    pub monitor_device_path: String,
    pub display_name: String,
    pub friendly_name: String,
    pub hdr_supported: bool,
    pub hdr_enabled: bool,
    pub primary: bool,
    #[serde(skip)]
    pub adapter: LUID,
    #[serde(skip)]
    pub target: u32,
    #[serde(skip)]
    pub source: u32,
}
impl Monitor {
    pub fn matches(&self, hint: &str) -> bool {
        [
            &self.device_id,
            &self.monitor_device_path,
            &self.display_name,
        ]
        .iter()
        .any(|value| value.eq_ignore_ascii_case(hint))
    }
}

fn monitor_edid(path: &str) -> Vec<u8> {
    let parts: Vec<_> = path.split('#').collect();
    if parts.len() < 3 || parts[1..3].iter().any(|p| p.contains(['\\', '/'])) {
        return Vec::new();
    }
    let key: Vec<u16> = format!(
        "SYSTEM\\CurrentControlSet\\Enum\\DISPLAY\\{}\\{}\\Device Parameters\0",
        parts[1], parts[2]
    )
    .encode_utf16()
    .collect();
    // SAFETY: The registry path is terminated and the byte buffer is sized from the registry query.
    unsafe {
        use windows::Win32::System::Registry::*;
        let mut size = 0;
        if RegGetValueW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(key.as_ptr()),
            windows::core::w!("EDID"),
            RRF_RT_REG_BINARY,
            None,
            None,
            Some(&mut size),
        )
        .is_err()
            || size > 65536
        {
            return Vec::new();
        }
        let mut bytes = vec![0; size as usize];
        if RegGetValueW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(key.as_ptr()),
            windows::core::w!("EDID"),
            RRF_RT_REG_BINARY,
            None,
            Some(bytes.as_mut_ptr().cast()),
            Some(&mut size),
        )
        .is_err()
        {
            return Vec::new();
        }
        bytes.truncate(size as usize);
        bytes
    }
}
fn monitor_instance(path: &str) -> Result<String> {
    use windows::Win32::Devices::DeviceAndDriverInstallation::*;
    let guid = GUID::from_u128(0xe6f07b5f_ee97_4a90_b076_33f57bf4eaa7);
    // SAFETY: The SetupAPI structures have their required sizes; aligned buffers and terminated strings live through each call, and the device set is destroyed afterwards.
    unsafe {
        let set = SetupDiGetClassDevsW(Some(&guid), PCWSTR::null(), None, DIGCF_DEVICEINTERFACE)?;
        let result = (|| -> Result<String> {
            let path: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
            let mut interface = SP_DEVICE_INTERFACE_DATA {
                cbSize: size_of::<SP_DEVICE_INTERFACE_DATA>() as u32,
                ..Default::default()
            };
            SetupDiOpenDeviceInterfaceW(set, PCWSTR(path.as_ptr()), 0, Some(&mut interface))?;
            let mut required = 0;
            let mut device = SP_DEVINFO_DATA {
                cbSize: size_of::<SP_DEVINFO_DATA>() as u32,
                ..Default::default()
            };
            let _ = SetupDiGetDeviceInterfaceDetailW(
                set,
                &interface,
                None,
                0,
                Some(&mut required),
                None,
            );
            if !(8..=65536).contains(&required) {
                bail!("invalid monitor interface size");
            }
            let mut buffer = vec![0u64; (required as usize).div_ceil(8)];
            let detail = buffer
                .as_mut_ptr()
                .cast::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>();
            (*detail).cbSize = size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;
            SetupDiGetDeviceInterfaceDetailW(
                set,
                &interface,
                Some(detail),
                required,
                None,
                Some(&mut device),
            )?;
            let _ = SetupDiGetDeviceInstanceIdW(set, &device, None, Some(&mut required));
            if required == 0 || required > 32768 {
                bail!("invalid monitor instance size");
            }
            let mut instance = vec![0u16; required as usize];
            SetupDiGetDeviceInstanceIdW(set, &device, Some(&mut instance), None)?;
            Ok(wide(&instance))
        })();
        let _ = SetupDiDestroyDeviceInfoList(set);
        result
    }
}
/// An adapter and target id a display can be listed under.
pub(super) type Route = (LUID, u32);
/// Whether a monitor device path, `\\?\DISPLAY#RBY0001#1&28a6823a&0&UID258#{guid}`,
/// belongs to a Rubylight virtual display that IddCx gave target id `target`:
/// the maker code is RBY and the instance ends in the target id as its UID.
pub(super) fn is_virtual_monitor(path: &str, target: u32) -> bool {
    let parts: Vec<_> = path.split('#').collect();
    is_rubylight_monitor(path)
        && parts[2]
            .to_ascii_lowercase()
            .ends_with(&format!("&uid{target}"))
}
/// Whether a monitor device path names a Rubylight virtual display (maker RBY).
pub(super) fn is_rubylight_monitor(path: &str) -> bool {
    let parts: Vec<_> = path.split('#').collect();
    parts.len() >= 3
        && parts[1]
            .get(..3)
            .is_some_and(|maker| maker.eq_ignore_ascii_case("RBY"))
}
fn monitor_id(path: &str) -> String {
    static IDS: std::sync::LazyLock<
        std::sync::Mutex<std::collections::BTreeMap<String, (Instant, String)>>,
    > = std::sync::LazyLock::new(Default::default);
    let mut ids = IDS.lock().unwrap();
    ids.retain(|_, (at, _)| at.elapsed() < Duration::from_secs(1));
    if let Some((_, id)) = ids.get(path) {
        return id.clone();
    }
    let id = butterpollo_core::display_policy::legacy_device_id(
        path,
        monitor_instance(path).ok().as_deref(),
        &monitor_edid(path),
    );
    if ids.len() < 512 {
        ids.insert(path.into(), (Instant::now(), id.clone()));
    }
    id
}
pub struct Topology {
    pub(super) paths: Vec<DISPLAYCONFIG_PATH_INFO>,
    pub(super) modes: Vec<DISPLAYCONFIG_MODE_INFO>,
}
fn extended_paths(
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
/// An inactive route from QDC_ALL_PATHS made fit to switch on: Windows
/// chooses its modes, refresh and position. Such routes carry a zero
/// rotation and scaling and a stale refresh, which SetDisplayConfig refuses
/// with ERROR_INVALID_PARAMETER.
fn switch_on_route(path: &mut DISPLAYCONFIG_PATH_INFO) {
    path.flags = DISPLAYCONFIG_PATH_ACTIVE;
    path.sourceInfo.Anonymous.modeInfoIdx = DISPLAYCONFIG_PATH_MODE_IDX_INVALID;
    path.targetInfo.Anonymous.modeInfoIdx = DISPLAYCONFIG_PATH_MODE_IDX_INVALID;
    // A zero refresh lets Windows pick the display's best rate; it requires
    // unspecified scan-line ordering.
    path.targetInfo.refreshRate = DISPLAYCONFIG_RATIONAL::default();
    path.targetInfo.scanLineOrdering = DISPLAYCONFIG_SCANLINE_ORDERING_UNSPECIFIED;
    if path.targetInfo.rotation.0 == 0 {
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
fn moving_targets(
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
/// displays keep timing and position; newly switched-on routes get none.
fn keep_active_modes(paths: &[DISPLAYCONFIG_PATH_INFO]) -> Vec<DISPLAYCONFIG_PATH_INFO> {
    paths
        .iter()
        .map(|path| {
            let mut path = *path;
            if path.flags & DISPLAYCONFIG_PATH_ACTIVE == 0 {
                switch_on_route(&mut path);
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
fn activation_paths(
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
    switch_on_route(&mut route);
    let mut paths: Vec<_> = active.iter().filter(|p| !ours(p)).copied().collect();
    paths.push(route);
    Ok(Some(paths))
}
/// Switch on a connected display beside the current desktop. Returns None
/// while the target is not connected, otherwise whether the other displays
/// kept their timings. `routes` are the (adapter, target) pairs the display
/// goes by: Windows can list it under one pair while it is on and another
/// while it is off.
pub(super) fn activate_target(routes: &[Route]) -> Result<Option<bool>> {
    let active = Topology::query()?;
    let all = Topology::query_all()?;
    let Some(paths) = activation_paths(&active.paths, &all.paths, routes)? else {
        return Ok(None);
    };
    let flags = SDC_APPLY | SDC_USE_SUPPLIED_DISPLAY_CONFIG | SDC_ALLOW_CHANGES;
    // SAFETY: The path and mode slices remain valid for this synchronous display configuration call.
    let kept = unsafe { SetDisplayConfig(Some(&paths), Some(&active.modes), flags) };
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
    pub(super) fn rotations(&self) -> std::collections::BTreeMap<String, u32> {
        self.monitors()
            .iter()
            .filter_map(|m| {
                self.paths
                    .iter()
                    .find(|p| p.targetInfo.adapterId == m.adapter && p.targetInfo.id == m.target)
                    .map(|p| (m.device_id.clone(), p.targetInfo.rotation.0 as u32))
            })
            .collect()
    }
    pub(super) fn set_rotations(
        &mut self,
        rotations: &std::collections::BTreeMap<String, u32>,
    ) -> Result<()> {
        let monitors = self.monitors();
        let mut changed = false;
        for monitor in monitors {
            if let Some(rotation) = rotations.get(&monitor.device_id) {
                if !(1..=4).contains(rotation) {
                    bail!("invalid saved display rotation");
                }
                if let Some(path) = self.paths.iter_mut().find(|p| {
                    p.targetInfo.adapterId == monitor.adapter && p.targetInfo.id == monitor.target
                }) && path.targetInfo.rotation.0 != *rotation as i32
                {
                    let swap = (path.targetInfo.rotation.0 - *rotation as i32).abs() % 2 != 0;
                    if swap
                        && let Some(mode) = self
                            .modes
                            // SAFETY: The path was queried without virtual-mode awareness, so modeInfoIdx is the active union field.
                            .get_mut(unsafe { path.sourceInfo.Anonymous.modeInfoIdx } as usize)
                        && mode.infoType == DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE
                    {
                        // SAFETY: The mode type was checked as SOURCE before accessing its sourceMode fields.
                        unsafe {
                            std::mem::swap(
                                &mut mode.Anonymous.sourceMode.width,
                                &mut mode.Anonymous.sourceMode.height,
                            );
                        }
                    }
                    path.targetInfo.rotation = DISPLAYCONFIG_ROTATION(*rotation as i32);
                    path.targetInfo.Anonymous.modeInfoIdx = DISPLAYCONFIG_PATH_MODE_IDX_INVALID;
                    changed = true;
                }
            }
        }
        if changed {
            self.restore()?;
        }
        Ok(())
    }
    pub(super) fn clone_groups(&self) -> Vec<Vec<String>> {
        let mut groups = std::collections::BTreeMap::<(u32, i32, u32), Vec<String>>::new();
        for monitor in self.monitors() {
            groups
                .entry((
                    monitor.adapter.LowPart,
                    monitor.adapter.HighPart,
                    monitor.source,
                ))
                .or_default()
                .push(monitor.device_id);
        }
        groups
            .into_values()
            .filter(|group| group.len() > 1)
            .collect()
    }
    pub(super) fn restore_clone_groups(&mut self, groups: &[Vec<String>]) -> Result<()> {
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
            return Ok(());
        }
        let mut loose = self.paths.clone();
        for path in &mut loose {
            path.sourceInfo.Anonymous.modeInfoIdx = DISPLAYCONFIG_PATH_MODE_IDX_INVALID;
            path.targetInfo.Anonymous.modeInfoIdx = DISPLAYCONFIG_PATH_MODE_IDX_INVALID;
        }
        let flags = SDC_APPLY | SDC_USE_SUPPLIED_DISPLAY_CONFIG | SDC_ALLOW_CHANGES;
        // SAFETY: The paths remain valid for the call and their mode indices have been invalidated.
        check(unsafe { SetDisplayConfig(Some(&loose), None, flags) })
            .with_context(|| format!("Windows refused the cloned layout (saved modes: {exact:#})"))
    }
    pub fn nodes(&self) -> Result<Vec<butterpollo_core::topology::Node>> {
        use butterpollo_core::topology::{Kind, Mode, Node, Position};
        self.monitors()
            .into_iter()
            .map(|m| {
                let mode = mode(&m.display_name)?;
                let refresh = self.refresh(&m.device_id)?;
                // SAFETY: mode() returns a display DEVMODEW, whose display position union field is initialized.
                let position = unsafe { mode.Anonymous1.Anonymous2.dmPosition };
                Ok(Node {
                    id: m.device_id.clone(),
                    label: m.friendly_name,
                    kind: Kind::Physical,
                    active: true,
                    primary: m.primary,
                    device_id: m.device_id,
                    desired_position: Position {
                        x: position.x,
                        y: position.y,
                    },
                    mode: Mode {
                        width: mode.dmPelsWidth,
                        height: mode.dmPelsHeight,
                        refresh_hz: f64::from(refresh.0) / 1000.0,
                    },
                })
            })
            .collect()
    }
    pub fn set_positions(
        &mut self,
        positions: &std::collections::BTreeMap<String, butterpollo_core::topology::Position>,
    ) -> Result<()> {
        let monitors = self.monitors();
        let mut moved = false;
        for monitor in monitors {
            let Some(position) = positions.get(&monitor.device_id) else {
                continue;
            };
            for path in &self.paths {
                if path.targetInfo.adapterId == monitor.adapter
                    && path.targetInfo.id == monitor.target
                {
                    // SAFETY: The path was queried without virtual-mode awareness, so modeInfoIdx is the active union field.
                    let index = unsafe { path.sourceInfo.Anonymous.modeInfoIdx };
                    let source = self
                        .modes
                        .get_mut(index as usize)
                        .context("display source mode unavailable")?;
                    if source.infoType != DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE {
                        bail!("invalid display source mode");
                    }
                    let next = POINTL {
                        x: position.x,
                        y: position.y,
                    };
                    // SAFETY: The mode type was checked as SOURCE before reading its sourceMode position.
                    let current = unsafe { source.Anonymous.sourceMode.position };
                    if (current.x, current.y) != (next.x, next.y) {
                        source.Anonymous.sourceMode.position = next;
                        moved = true;
                    }
                }
            }
        }
        // Re-applying even an unchanged configuration lets Windows renegotiate
        // display timings: a TV at 1080p120 HDR came back at 60 Hz.
        if !moved {
            return Ok(());
        }
        self.restore()
    }
    pub fn query() -> Result<Self> {
        Self::query_with_flags(QDC_ONLY_ACTIVE_PATHS)
    }
    pub fn query_all() -> Result<Self> {
        Self::query_with_flags(QDC_ALL_PATHS)
    }
    fn query_with_flags(flags: QUERY_DISPLAY_CONFIG_FLAGS) -> Result<Self> {
        Self::query_with(
            // SAFETY: Both counts are live writable references for this call.
            |np, nm| unsafe { GetDisplayConfigBufferSizes(flags, np, nm) },
            // SAFETY: The initialized buffers match the supplied counts and
            // remain live through this synchronous call.
            |np, paths, nm, modes| unsafe {
                QueryDisplayConfig(flags, np, paths.as_mut_ptr(), nm, modes.as_mut_ptr(), None)
            },
        )
    }
    fn query_with(
        mut sizes: impl FnMut(&mut u32, &mut u32) -> WIN32_ERROR,
        mut query: impl FnMut(
            &mut u32,
            &mut [DISPLAYCONFIG_PATH_INFO],
            &mut u32,
            &mut [DISPLAYCONFIG_MODE_INFO],
        ) -> WIN32_ERROR,
    ) -> Result<Self> {
        for _ in 0..8 {
            let (mut np, mut nm) = (0, 0);
            sizes(&mut np, &mut nm).ok()?;
            // QDC_ALL_PATHS includes every source/target combination, not
            // just attached monitors. VDDs can legitimately exceed 256.
            if np > 16384 || nm > 32768 {
                bail!("display configuration exceeds the limit: {np} paths, {nm} modes");
            }
            // A headless console has no paths before its virtual display is
            // created. QueryDisplayConfig requires real array buffers.
            if np == 0 {
                return Ok(Self {
                    paths: Vec::new(),
                    modes: Vec::new(),
                });
            }
            nm = nm.max(1);
            let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); np as usize];
            let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); nm as usize];
            let result = query(&mut np, &mut paths, &mut nm, &mut modes);
            if result == ERROR_INSUFFICIENT_BUFFER {
                continue;
            }
            result.ok()?;
            paths.truncate(np as usize);
            modes.truncate(nm as usize);
            return Ok(Self { paths, modes });
        }
        bail!("display topology is changing")
    }
    pub fn restore(&self) -> Result<()> {
        // SAFETY: The owned path and mode slices remain valid for this synchronous display configuration call.
        unsafe {
            check(SetDisplayConfig(
                Some(&self.paths),
                Some(&self.modes),
                SDC_APPLY | SDC_USE_SUPPLIED_DISPLAY_CONFIG | SDC_ALLOW_CHANGES,
            ))
        }
    }
    /// Save the active layout to the Windows display database for the
    /// connected displays, as the layout Windows recalls for them. Windows
    /// recalls it when an exclusive-fullscreen game loses focus; a layout that
    /// is only applied then loses to the saved one. The modes are the current
    /// ones, so nothing is switched.
    pub fn save_current() -> Result<()> {
        let current = Self::query()?;
        // SAFETY: The owned path and mode slices remain valid for this synchronous display configuration call.
        unsafe {
            check(SetDisplayConfig(
                Some(&current.paths),
                Some(&current.modes),
                SDC_APPLY | SDC_USE_SUPPLIED_DISPLAY_CONFIG | SDC_SAVE_TO_DATABASE,
            ))
        }
    }
    pub fn refresh(&self, id: &str) -> Result<butterpollo_core::framegen::Rate> {
        let monitor = self
            .monitors()
            .into_iter()
            .find(|m| m.device_id == id)
            .context("display unavailable")?;
        let path = self
            .paths
            .iter()
            .find(|p| {
                p.targetInfo.adapterId == monitor.adapter && p.targetInfo.id == monitor.target
            })
            .context("display path unavailable")?;
        let rate = path.targetInfo.refreshRate;
        if rate.Denominator == 0 {
            bail!("display refresh has no denominator");
        }
        Ok(butterpollo_core::framegen::Rate(u32::try_from(
            (u64::from(rate.Numerator) * 1000 + u64::from(rate.Denominator) / 2)
                / u64::from(rate.Denominator),
        )?))
    }
    pub fn set_mode_rate(
        name: &str,
        width: u32,
        height: u32,
        rate: butterpollo_core::framegen::Rate,
    ) -> Result<()> {
        if width == 0 || height == 0 || rate.0 == 0 {
            bail!("invalid display mode");
        }
        let mut topology = Self::query()?;
        let monitor = topology
            .monitors()
            .into_iter()
            .find(|m| m.matches(name))
            .context("display unavailable")?;
        // Windows chooses the target timing itself, even for the mode a display
        // already has, so a display already in this mode is left alone.
        let applied = || -> Result<bool> {
            let current = mode(&monitor.display_name)?;
            let actual = Self::query()?.refresh(&monitor.device_id)?;
            Ok(timing_matches(
                (width, height, rate.0),
                (current.dmPelsWidth, current.dmPelsHeight, actual.0),
            ))
        };
        if applied()? {
            return Ok(());
        }
        let (num, den) = rate.rational();
        for path in &mut topology.paths {
            if path.targetInfo.adapterId == monitor.adapter && path.targetInfo.id == monitor.target
            {
                // SAFETY: The path was queried without virtual-mode awareness, so modeInfoIdx is the active union field.
                let index = unsafe { path.sourceInfo.Anonymous.modeInfoIdx } as usize;
                let source = topology
                    .modes
                    .get_mut(index)
                    .context("display source mode unavailable")?;
                if source.infoType != DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE {
                    bail!("invalid source mode");
                }
                source.Anonymous.sourceMode.width = width;
                source.Anonymous.sourceMode.height = height;
                path.targetInfo.refreshRate = DISPLAYCONFIG_RATIONAL {
                    Numerator: num,
                    Denominator: den,
                };
                path.targetInfo.Anonymous.modeInfoIdx = DISPLAYCONFIG_PATH_MODE_IDX_INVALID;
            }
        }
        topology.restore()?;
        if applied()? {
            return Ok(());
        }
        // Windows reported success with another refresh rate: a TV whose native
        // timing is 4K60 turns 1080p120 into 60 Hz, and a 240 Hz monitor can
        // fall back to its 120 Hz default. The mode list applies the rate.
        let hz = rate.0.saturating_add(500) / 1000;
        tracing::debug!(output = %monitor.display_name, hz, "display rate applied through the mode list");
        let mut next = mode(&monitor.display_name)?;
        next.dmPelsWidth = width;
        next.dmPelsHeight = height;
        next.dmDisplayFrequency = hz;
        next.dmFields = DM_PELSWIDTH | DM_PELSHEIGHT | DM_DISPLAYFREQUENCY;
        apply_mode_with(&monitor.display_name, &next, CDS_TYPE(0))?;
        if !applied()? {
            let current = mode(&monitor.display_name)?;
            bail!(
                "display kept {}x{}@{} instead of {width}x{height}@{:.3}",
                current.dmPelsWidth,
                current.dmPelsHeight,
                current.dmDisplayFrequency,
                f64::from(rate.0) / 1000.
            );
        }
        Ok(())
    }
    pub fn set_active(ids: &[String]) -> Result<()> {
        Self::set_active_once(ids, true)
    }
    fn set_active_once(ids: &[String], may_move: bool) -> Result<()> {
        if ids.is_empty() {
            bail!("display topology cannot be empty");
        }
        let topology = Self::query_all()?;
        let mut candidates_by_target = Vec::new();
        for id in ids {
            let mut candidates = topology
                .paths
                .iter()
                // SAFETY: The initialized target-name structure has the matching header type and size and lives through the query.
                .filter(|path| unsafe {
                    let mut name = DISPLAYCONFIG_TARGET_DEVICE_NAME {
                        header: header(
                            DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
                            size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>(),
                            path.targetInfo.adapterId,
                            path.targetInfo.id,
                        ),
                        ..Default::default()
                    };
                    DisplayConfigGetDeviceInfo(&mut name.header) == 0
                        && (wide(&name.monitorDevicePath).eq_ignore_ascii_case(id)
                            || monitor_id(&wide(&name.monitorDevicePath)).eq_ignore_ascii_case(id))
                        && path.targetInfo.targetAvailable.as_bool()
                })
                .copied()
                .collect::<Vec<_>>();
            candidates.sort_by_key(|p| p.flags & DISPLAYCONFIG_PATH_ACTIVE == 0);
            if !candidates.is_empty() {
                candidates_by_target.push(candidates);
            }
        }
        let paths = extended_paths(&candidates_by_target)?;
        if paths.is_empty() {
            bail!("no saved display targets are currently connected");
        }
        let flags = SDC_APPLY | SDC_USE_SUPPLIED_DISPLAY_CONFIG | SDC_ALLOW_CHANGES;
        // Displays that stay on their current source keep their timing and
        // position, such as a retained remote monitor that a restore must
        // leave alone; Windows chooses for the displays it switches on.
        // Windows can refuse that mix, so fall back to choosing every mode.
        let moving = moving_targets(&paths, &topology.paths);
        let mut paths = keep_active_modes(&paths);
        // SAFETY: These paths use the non-virtual mode-index union field returned by QueryDisplayConfig.
        let kept = if paths.iter().any(|p| unsafe { p.sourceInfo.Anonymous.modeInfoIdx }
            != DISPLAYCONFIG_PATH_MODE_IDX_INVALID)
        {
            // SAFETY: The path and mode slices remain valid for this synchronous display configuration call.
            unsafe { SetDisplayConfig(Some(&paths), Some(&topology.modes), flags) }
        } else {
            -1
        };
        if kept == 0 {
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
            // SAFETY: The retained paths and queried modes remain valid for the synchronous call.
            check(unsafe { SetDisplayConfig(Some(&staying), Some(&topology.modes), flags) })
                .context("cannot switch off the displays that change desktop source")?;
            return Self::set_active_once(ids, false);
        }
        let supplied = describe_paths(&paths);
        for path in &mut paths {
            path.sourceInfo.Anonymous.modeInfoIdx = DISPLAYCONFIG_PATH_MODE_IDX_INVALID;
            path.targetInfo.Anonymous.modeInfoIdx = DISPLAYCONFIG_PATH_MODE_IDX_INVALID;
        }
        // SAFETY: The paths remain valid for the call and their mode indices have been invalidated.
        unsafe { check(SetDisplayConfig(Some(&paths), None, flags)) }.with_context(|| {
            format!("Windows refused the display layout (with modes: {kept}; routes: {supplied})")
        })
    }
    pub fn monitors(&self) -> Vec<Monitor> {
        let primary = crate::capture::displays()
            .unwrap_or_default()
            .into_iter()
            .find(|d| d.primary)
            .map(|d| d.display_name);
        let mut paths: Vec<_> = self.paths.iter().collect();
        paths.sort_by_key(|p| p.flags & DISPLAYCONFIG_PATH_ACTIVE == 0);
        let mut targets = std::collections::BTreeSet::new();
        paths
            .into_iter()
            .filter(|p| {
                targets.insert((
                    p.targetInfo.adapterId.LowPart,
                    p.targetInfo.adapterId.HighPart,
                    p.targetInfo.id,
                ))
            })
            // SAFETY: Both name structures have matching header types and sizes and remain live throughout the queries.
            .filter_map(|p| unsafe {
                let mut source = DISPLAYCONFIG_SOURCE_DEVICE_NAME {
                    header: header(
                        DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
                        size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>(),
                        p.sourceInfo.adapterId,
                        p.sourceInfo.id,
                    ),
                    ..Default::default()
                };
                let mut target = DISPLAYCONFIG_TARGET_DEVICE_NAME {
                    header: header(
                        DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
                        size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>(),
                        p.targetInfo.adapterId,
                        p.targetInfo.id,
                    ),
                    ..Default::default()
                };
                if DisplayConfigGetDeviceInfo(&mut source.header) != 0
                    || DisplayConfigGetDeviceInfo(&mut target.header) != 0
                {
                    return None;
                }
                let color =
                    color_state::query(p.targetInfo.adapterId, p.targetInfo.id).unwrap_or_default();
                let display_name = wide(&source.viewGdiDeviceName);
                let monitor_device_path = wide(&target.monitorDevicePath);
                Some(Monitor {
                    device_id: monitor_id(&monitor_device_path),
                    monitor_device_path,
                    friendly_name: wide(&target.monitorFriendlyDeviceName),
                    primary: primary.as_ref() == Some(&display_name),
                    display_name,
                    hdr_supported: color.supported,
                    hdr_enabled: color.enabled,
                    adapter: p.targetInfo.adapterId,
                    target: p.targetInfo.id,
                    source: p.sourceInfo.id,
                })
            })
            .collect()
    }
    /// Whether a path still leads to this monitor, as `monitors` would list
    /// it: the lease heartbeat's check each second, without the DXGI and
    /// colour queries that take nearly all of the half millisecond. `routes`
    /// are the (adapter, target) pairs the display goes by: Windows can list
    /// it under another pair while it is off than while it is on.
    pub(super) fn shows(
        &self,
        routes: &[Route],
        monitor: &Monitor,
        mut device_info: impl FnMut(&mut DISPLAYCONFIG_DEVICE_INFO_HEADER) -> i32,
    ) -> Result<bool> {
        for p in self.paths.iter().filter(|p| {
            routes.iter().any(|(adapter, target)| {
                p.targetInfo.adapterId == *adapter && p.targetInfo.id == *target
            })
        }) {
            let mut source = DISPLAYCONFIG_SOURCE_DEVICE_NAME {
                header: header(
                    DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
                    size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>(),
                    p.sourceInfo.adapterId,
                    p.sourceInfo.id,
                ),
                ..Default::default()
            };
            let mut target = DISPLAYCONFIG_TARGET_DEVICE_NAME {
                header: header(
                    DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
                    size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>(),
                    p.targetInfo.adapterId,
                    p.targetInfo.id,
                ),
                ..Default::default()
            };
            check(device_info(&mut source.header))?;
            check(device_info(&mut target.header))?;
            if wide(&target.monitorDevicePath).eq_ignore_ascii_case(&monitor.monitor_device_path) {
                return Ok(true);
            }
        }
        Ok(false)
    }
}
pub fn monitors() -> Result<Vec<Monitor>> {
    Ok(Topology::query()?.monitors())
}
pub fn edid_refresh(hint: &str, targets: &[u32]) -> Result<serde_json::Value> {
    let monitors = Topology::query_all()?.monitors();
    let monitor = monitors
        .iter()
        .find(|monitor| monitor.matches(hint) || monitor.friendly_name.eq_ignore_ascii_case(hint))
        .context("display device not found for EDID refresh validation")?;
    let parts: Vec<_> = monitor.monitor_device_path.split('#').collect();
    let mut bytes = Vec::new();
    if parts.len() >= 3 && parts[1..3].iter().all(|part| !part.contains(['\\', '/'])) {
        let key = format!(
            "SYSTEM\\CurrentControlSet\\Enum\\DISPLAY\\{}\\{}\\Device Parameters",
            parts[1], parts[2]
        );
        let key: Vec<_> = key.encode_utf16().chain(Some(0)).collect();
        let name: Vec<_> = "EDID\0".encode_utf16().collect();
        let mut size = 0u32;
        // SAFETY: The registry strings are terminated and the output buffer is resized to the queried byte count.
        unsafe {
            use windows::Win32::System::Registry::*;
            if RegGetValueW(
                HKEY_LOCAL_MACHINE,
                PCWSTR(key.as_ptr()),
                PCWSTR(name.as_ptr()),
                RRF_RT_REG_BINARY,
                None,
                None,
                Some(&mut size),
            )
            .is_ok()
                && size <= 65536
            {
                bytes.resize(size as usize, 0);
                if RegGetValueW(
                    HKEY_LOCAL_MACHINE,
                    PCWSTR(key.as_ptr()),
                    PCWSTR(name.as_ptr()),
                    RRF_RT_REG_BINARY,
                    None,
                    Some(bytes.as_mut_ptr().cast()),
                    Some(&mut size),
                )
                .is_err()
                {
                    bytes.clear();
                }
            }
        }
    }
    let info = butterpollo_core::edid::Refresh::parse(&bytes);
    let mut response = serde_json::to_value(&info)?;
    response["status"] = true.into();
    response["device_id"] = monitor.device_id.clone().into();
    response["device_label"] = monitor.friendly_name.clone().into();
    response["targets"] = targets
        .iter()
        .map(|hz| {
            let (supported, method) = info.support(*hz);
            serde_json::json!({"hz":hz,"supported":supported,"method":method})
        })
        .collect::<Vec<_>>()
        .into();
    Ok(response)
}

#[cfg(test)]
mod tests {
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
}
