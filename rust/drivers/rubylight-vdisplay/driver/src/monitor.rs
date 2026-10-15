//! Virtual monitors: the [`Monitor`] type and the control-plane verbs on it (create and
//! arrive, in-place mode update, remove, clear, the owner reap, the frame ring), plus the mode
//! structs the DDIs fill from. Every verb takes the calling process as `owner` and reaches
//! only the monitors that process added.
//!
//! From punktfunk's pf-vdisplay `monitor.rs` by Enrico Bühler (unom), MIT OR Apache-2.0,
//! with the encode session and hardware cursor replaced by the frame ring.
//!
//! Ownership: [`crate::registry`] holds the only strong `Arc<Monitor>`; the swap-chain worker
//! holds a `Weak`. Every field a worker, a DDI callback or an IOCTL can race on has its own
//! mutex, held for a field swap and never across a DDI, a join, or a `Drop` that closes a
//! handle. Removal is two steps with no lock between them: the registry hands the `Arc`
//! back, [`Monitor::teardown`] stops the worker and drops the ring, and only then does the
//! caller run `IddCxMonitorDeparture`.

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use rubylight_vdisplay_proto::control::{self, AddRequest, MonitorStatus};
use rubylight_vdisplay_proto::edid;
use rubylight_vdisplay_proto::modes::{self, IdError, Mode, flatten};
use wdk_sys::{NTSTATUS, WDFOBJECT, call_unsafe_wdf_function_binding, iddcx};

use crate::frame_ring::RingHandles;
use crate::registry::{self, lock};
use crate::swap_chain_processor::SwapChainProcessor;
use crate::worker::Sendable;

type SendMonitor = Sendable<iddcx::IDDCX_MONITOR>;

/// [`Monitor::state`]: `IddCxMonitorDeparture` is legal only on an arrived monitor, so one
/// caller wins the `ARRIVED -> DEPARTED` swap and departs it.
const CREATING: u8 = 0;
const ARRIVED: u8 = 1;
const DEPARTED: u8 = 2;

/// What `IddCxMonitorArrival` reported.
#[derive(Clone, Copy)]
pub struct Arrival {
    pub target_id: u32,
    pub luid_low: u32,
    pub luid_high: i32,
}

/// What the swap-chain worker last saw, for `IOCTL_QUERY_MONITOR`.
#[derive(Default)]
pub struct Stats {
    /// `width | height << 32` of the last composed surface.
    pub surface_size: AtomicU64,
    pub surface_format: AtomicU32,
    pub acquired: AtomicU64,
    pub pushed: AtomicU64,
    /// The current ring's textures are open on the rendering device.
    pub ring_open: AtomicBool,
}

pub struct Monitor {
    /// The process whose ADD created this monitor; every later verb must come from it.
    pub owner: u32,
    /// EDID serial and connector index, the key the mode DDIs match on.
    pub id: u32,
    /// The owner's key.
    pub session_id: u64,
    /// The watchdog reap skips a monitor still initialising.
    pub created_at: Instant,
    object: OnceLock<SendMonitor>,
    arrival: OnceLock<Arrival>,
    state: AtomicU8,
    modes: Mutex<Vec<Mode>>,
    /// The live swap-chain worker; dropping it joins the thread.
    swap: Mutex<Option<SwapChainProcessor>>,
    /// The frame ring the host handed over, shared with the worker that has it open.
    ring: Mutex<Option<Arc<RingHandles>>>,
    /// Bumped by every ring install or removal; the worker re-reads the ring only then.
    pub ring_gen: AtomicU32,
    /// Render LUID of the last swap-chain assignment, packed; `0` = none yet.
    render_luid: AtomicI64,
    pub stats: Stats,
    gone: AtomicBool,
    /// DDI calls in flight against `object`; teardown waits for zero.
    ddi_inflight: AtomicU32,
}

/// One DDI call against a monitor's handle; see [`Monitor::ddi`].
pub struct DdiGuard<'a>(&'a Monitor);

impl Drop for DdiGuard<'_> {
    fn drop(&mut self) {
        self.0.ddi_inflight.fetch_sub(1, Ordering::AcqRel);
    }
}

const DDI_DRAIN_BUDGET: Duration = Duration::from_secs(2);

fn take<T>(slot: &Mutex<Option<T>>) -> Option<T> {
    lock(slot).take()
}

impl Monitor {
    pub(crate) fn pending(owner: u32, id: u32, session_id: u64, modes: Vec<Mode>) -> Self {
        Self {
            owner,
            id,
            session_id,
            created_at: Instant::now(),
            object: OnceLock::new(),
            arrival: OnceLock::new(),
            state: AtomicU8::new(CREATING),
            modes: Mutex::new(modes),
            swap: Mutex::new(None),
            ring: Mutex::new(None),
            ring_gen: AtomicU32::new(0),
            render_luid: AtomicI64::new(0),
            stats: Stats::default(),
            gone: AtomicBool::new(false),
            ddi_inflight: AtomicU32::new(0),
        }
    }

    /// Hold while calling a DDI against this monitor's handle. `None` once teardown began:
    /// the increment lands before the `gone` read, so a teardown that saw zero cannot be
    /// passed by a caller that saw `!gone`.
    pub fn ddi(&self) -> Option<DdiGuard<'_>> {
        self.ddi_inflight.fetch_add(1, Ordering::AcqRel);
        if self.gone.load(Ordering::Acquire) {
            self.ddi_inflight.fetch_sub(1, Ordering::AcqRel);
            return None;
        }
        Some(DdiGuard(self))
    }

    pub fn set_render_luid(&self, luid: windows::Win32::Foundation::LUID) {
        self.render_luid
            .store(crate::direct_3d_device::luid_key(luid), Ordering::Release);
    }

    pub fn object(&self) -> Option<iddcx::IDDCX_MONITOR> {
        self.object.get().map(|o| o.0)
    }

    fn arrived(&self) -> bool {
        self.state.load(Ordering::Acquire) == ARRIVED
    }

    fn claim_departure(&self) -> bool {
        self.state.swap(DEPARTED, Ordering::AcqRel) == ARRIVED
    }

    /// The OS target id; 0 until arrival, which no host request names.
    pub fn target_id(&self) -> u32 {
        self.arrival.get().map_or(0, |a| a.target_id)
    }

    pub fn modes(&self) -> Vec<Mode> {
        lock(&self.modes).clone()
    }

    /// Install the worker for a fresh assignment. Returns what it displaced, or `proc` itself
    /// once torn down, for the caller to drop (join) with no lock held.
    #[must_use]
    pub fn set_swap(&self, proc: SwapChainProcessor) -> Option<SwapChainProcessor> {
        let mut slot = lock(&self.swap);
        if self.gone.load(Ordering::Acquire) {
            return Some(proc);
        }
        slot.replace(proc)
    }

    #[must_use]
    pub fn take_swap(&self) -> Option<SwapChainProcessor> {
        take(&self.swap)
    }

    /// Install a frame ring and wake the worker. `Err(ring)` once torn down; otherwise the
    /// ring it replaced, to drop with no lock held.
    #[allow(clippy::result_large_err)]
    pub fn set_ring(
        &self,
        ring: Arc<RingHandles>,
    ) -> Result<Option<Arc<RingHandles>>, Arc<RingHandles>> {
        let displaced = {
            let mut slot = lock(&self.ring);
            if self.gone.load(Ordering::Acquire) {
                return Err(ring);
            }
            slot.replace(ring)
        };
        self.bump_ring_gen();
        Ok(displaced)
    }

    pub fn take_ring(&self) -> Option<Arc<RingHandles>> {
        let ring = take(&self.ring);
        if ring.is_some() {
            self.bump_ring_gen();
        }
        ring
    }

    /// [`Self::take_ring`], only while the ring is the one of `generation`.
    pub fn take_ring_generation(&self, generation: u32) -> Option<Arc<RingHandles>> {
        let ring = {
            let mut slot = lock(&self.ring);
            if slot.as_ref().map(|r| r.generation) != Some(generation) {
                return None;
            }
            slot.take()
        };
        self.bump_ring_gen();
        ring
    }

    /// The ring generation counter and the ring, read together.
    pub fn ring(&self) -> (u32, Option<Arc<RingHandles>>) {
        let slot = lock(&self.ring);
        (self.ring_gen.load(Ordering::Acquire), slot.clone())
    }

    fn bump_ring_gen(&self) {
        self.ring_gen.fetch_add(1, Ordering::AcqRel);
        self.stats.ring_open.store(false, Ordering::Release);
        if let Some(p) = &*lock(&self.swap) {
            p.wake();
        }
    }

    /// The `IOCTL_QUERY_MONITOR` reply.
    pub fn status(&self) -> MonitorStatus {
        let mut flags = 0;
        if self.arrived() {
            flags |= control::MONITOR_ARRIVED;
        }
        if lock(&self.swap).is_some() {
            flags |= control::MONITOR_SWAP_CHAIN;
        }
        let ring_generation = lock(&self.ring).as_ref().map_or(0, |r| r.generation);
        if ring_generation != 0 {
            flags |= control::MONITOR_RING;
            if self.stats.ring_open.load(Ordering::Acquire) {
                flags |= control::MONITOR_RING_OPEN;
            }
        }
        let luid = self.render_luid.load(Ordering::Acquire);
        let size = self.stats.surface_size.load(Ordering::Acquire);
        let head = flatten(&self.modes()).next();
        MonitorStatus {
            target_id: self.target_id(),
            monitor_id: self.id,
            render_luid_low: luid as u32,
            render_luid_high: (luid >> 32) as i32,
            flags,
            surface_width: size as u32,
            surface_height: (size >> 32) as u32,
            surface_format: self.stats.surface_format.load(Ordering::Acquire),
            frames_acquired: self.stats.acquired.load(Ordering::Acquire),
            frames_pushed: self.stats.pushed.load(Ordering::Acquire),
            ring_generation,
            width: head.map_or(0, |m| m.width),
            height: head.map_or(0, |m| m.height),
            refresh_mhz: head.map_or(0, |m| m.refresh_mhz),
        }
    }

    /// Stop everything whose drop blocks, each taken under its guard and dropped after it:
    /// the swap-chain worker (it may be using the ring), then the ring, whose drop closes the
    /// host's handles. `gone` is set first, so a racing install hands its value back.
    pub fn teardown(&self) {
        self.gone.store(true, Ordering::Release);
        let started = Instant::now();
        while self.ddi_inflight.load(Ordering::Acquire) != 0 {
            if started.elapsed() > DDI_DRAIN_BUDGET {
                dbglog!(
                    "[rlvd] monitor teardown: {} DDI call(s) still in flight after {} ms",
                    self.ddi_inflight.load(Ordering::Acquire),
                    DDI_DRAIN_BUDGET.as_millis()
                );
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        drop(take(&self.swap));
        drop(take(&self.ring));
        let took = started.elapsed();
        if took > Duration::from_millis(250) {
            dbglog!("[rlvd] monitor teardown took {} ms", took.as_millis());
        }
    }
}

/// Tear every removed monitor down, then depart the ones this call claims.
fn depart(removed: Vec<Arc<Monitor>>) {
    for m in &removed {
        m.teardown();
    }
    for m in removed {
        if m.claim_departure()
            && let Some(object) = m.object()
        {
            // SAFETY: `object` arrived, and this call won its one departure claim.
            unsafe { wdk_iddcx::IddCxMonitorDeparture(object) };
        }
    }
}

/// Depart every monitor of `owner` older than `grace` (the watchdog reap). Returns the count.
pub fn reap_owner(owner: u32, grace: Duration) -> usize {
    let removed = registry::remove(|m| m.owner == owner && m.created_at.elapsed() >= grace);
    let n = removed.len();
    depart(removed);
    n
}

/// [`modes::signal_info`] stamped into the OS struct. Monitor modes pass divider 0, target
/// modes 1; total size equals active size.
fn signal_info(
    width: u32,
    height: u32,
    refresh_mhz: u32,
    v_sync_freq_divider: u32,
) -> wdk_sys::DISPLAYCONFIG_VIDEO_SIGNAL_INFO {
    let n = modes::signal_info(width, height, refresh_mhz, v_sync_freq_divider);
    let region = wdk_sys::DISPLAYCONFIG_2DREGION {
        cx: width,
        cy: height,
    };
    let mut si = wdk_sys::DISPLAYCONFIG_VIDEO_SIGNAL_INFO {
        pixelRate: n.pixel_rate,
        hSyncFreq: wdk_sys::DISPLAYCONFIG_RATIONAL {
            Numerator: n.h_sync.0,
            Denominator: n.h_sync.1,
        },
        vSyncFreq: wdk_sys::DISPLAYCONFIG_RATIONAL {
            Numerator: n.v_sync.0,
            Denominator: n.v_sync.1,
        },
        totalSize: region,
        activeSize: region,
        scanLineOrdering:
            wdk_sys::DISPLAYCONFIG_SCANLINE_ORDERING::DISPLAYCONFIG_SCANLINE_ORDERING_PROGRESSIVE,
        ..Default::default()
    };
    si.__bindgen_anon_1.videoStandard = n.video_standard;
    si
}

/// A monitor mode's signal (divider 0).
pub fn display_info(
    width: u32,
    height: u32,
    refresh_mhz: u32,
) -> wdk_sys::DISPLAYCONFIG_VIDEO_SIGNAL_INFO {
    signal_info(width, height, refresh_mhz, 0)
}

/// A scan-out mode (divider 1).
pub fn target_mode(width: u32, height: u32, refresh_mhz: u32) -> iddcx::IDDCX_TARGET_MODE {
    iddcx::IDDCX_TARGET_MODE {
        Size: core::mem::size_of::<iddcx::IDDCX_TARGET_MODE>() as u32,
        TargetVideoSignalInfo: wdk_sys::DISPLAYCONFIG_TARGET_MODE {
            targetVideoSignalInfo: signal_info(width, height, refresh_mhz, 1),
        },
        ..Default::default()
    }
}

/// The wire bit depths every mode offers: 8 and 10 bpc RGB, so Windows offers HDR10.
pub fn wire_bits() -> iddcx::IDDCX_WIRE_BITS_PER_COMPONENT {
    let rgb = iddcx::IDDCX_BITS_PER_COMPONENT::IDDCX_BITS_PER_COMPONENT_8
        | iddcx::IDDCX_BITS_PER_COMPONENT::IDDCX_BITS_PER_COMPONENT_10;
    iddcx::IDDCX_WIRE_BITS_PER_COMPONENT {
        Rgb: rgb,
        YCbCr444: iddcx::IDDCX_BITS_PER_COMPONENT::IDDCX_BITS_PER_COMPONENT_NONE,
        YCbCr422: iddcx::IDDCX_BITS_PER_COMPONENT::IDDCX_BITS_PER_COMPONENT_NONE,
        YCbCr420: iddcx::IDDCX_BITS_PER_COMPONENT::IDDCX_BITS_PER_COMPONENT_NONE,
    }
}

/// A scan-out mode for the `*2` (HDR) DDIs.
pub fn target_mode2(width: u32, height: u32, refresh_mhz: u32) -> iddcx::IDDCX_TARGET_MODE2 {
    let m1 = target_mode(width, height, refresh_mhz);
    iddcx::IDDCX_TARGET_MODE2 {
        Size: core::mem::size_of::<iddcx::IDDCX_TARGET_MODE2>() as u32,
        TargetVideoSignalInfo: m1.TargetVideoSignalInfo,
        BitsPerComponent: wire_bits(),
        ..Default::default()
    }
}

/// `IOCTL_ADD`: create and arrive `owner`'s monitor. Returns `(monitor_id, target_id,
/// adapter_luid_low, adapter_luid_high)`, or the status to fail the IOCTL with.
///
/// The entry is registered pending before `IddCxMonitorCreate`, so the mode DDIs the create
/// re-enters find it by id. A create failure reclaims the id; an arrival failure also deletes
/// the created object, since only an arrived monitor may depart.
pub fn create_monitor(owner: u32, req: &AddRequest) -> Result<(u32, u32, u32, i32), NTSTATUS> {
    // No adapter yet: busy, so the host retries instead of reporting the display missing.
    let adapter = crate::adapter::adapter().ok_or(crate::STATUS_DEVICE_BUSY)?;
    // One identity per owner and session: a re-ADD departs the stale monitor first.
    if registry::find(|m| m.owner == owner && m.session_id == req.session_id).is_some() {
        dbglog!(
            "[rlvd] create_monitor: owner {owner} session {:#x} already live; departing the stale monitor",
            req.session_id
        );
        remove_monitor(owner, req.session_id);
    }
    let requested = Mode {
        width: req.width,
        height: req.height,
        refresh_mhz: vec![req.refresh_mhz],
    };
    let monitor = registry::insert(
        owner,
        req.session_id,
        req.preferred_monitor_id,
        req.flags & control::ADD_FLAG_EXACT_ID != 0,
        modes::advertised_modes(requested, &[]),
    )
    .map_err(|e| match e {
        IdError::Busy => crate::STATUS_DEVICE_BUSY,
        IdError::Full => crate::STATUS_INSUFFICIENT_RESOURCES,
    })?;
    let id = monitor.id;

    let lum = edid::ClientLuminance {
        max_nits: req.max_luminance_nits,
        max_frame_avg_nits: req.max_frame_avg_nits,
        min_millinits: req.min_luminance_millinits,
    };
    let mut edid = edid::generate(id, lum, Some((req.width, req.height, req.refresh_mhz)));
    let desc = iddcx::IDDCX_MONITOR_DESCRIPTION {
        Size: core::mem::size_of::<iddcx::IDDCX_MONITOR_DESCRIPTION>() as u32,
        Type: iddcx::IDDCX_MONITOR_DESCRIPTION_TYPE::IDDCX_MONITOR_DESCRIPTION_TYPE_EDID,
        DataSize: edid.len() as u32,
        // `edid` outlives the synchronous IddCxMonitorCreate below.
        pData: edid.as_mut_ptr().cast(),
    };
    let (d1, d2, d3, d4) = modes::container_guid(id);
    let mut info = iddcx::IDDCX_MONITOR_INFO {
        Size: core::mem::size_of::<iddcx::IDDCX_MONITOR_INFO>() as u32,
        MonitorContainerId: wdk_sys::GUID {
            Data1: d1,
            Data2: d2,
            Data3: d3,
            Data4: d4,
        },
        MonitorType:
            wdk_sys::DISPLAYCONFIG_VIDEO_OUTPUT_TECHNOLOGY::DISPLAYCONFIG_OUTPUT_TECHNOLOGY_HDMI,
        ConnectorIndex: id,
        MonitorDescription: desc,
    };
    let mut attr = wdk_sys::WDF_OBJECT_ATTRIBUTES {
        Size: core::mem::size_of::<wdk_sys::WDF_OBJECT_ATTRIBUTES>() as u32,
        ExecutionLevel: wdk_sys::_WDF_EXECUTION_LEVEL::WdfExecutionLevelInheritFromParent,
        SynchronizationScope:
            wdk_sys::_WDF_SYNCHRONIZATION_SCOPE::WdfSynchronizationScopeInheritFromParent,
        ..Default::default()
    };
    let create_in = iddcx::IDARG_IN_MONITORCREATE {
        ObjectAttributes: &raw mut attr,
        pMonitorInfo: &raw mut info,
    };
    let mut create_out = iddcx::IDARG_OUT_MONITORCREATE::default();
    // SAFETY: `adapter` is the live adapter; `create_in` points at locals read synchronously.
    let st = unsafe { wdk_iddcx::IddCxMonitorCreate(adapter, &create_in, &mut create_out) };
    dbglog!("[rlvd] IddCxMonitorCreate(id={id}) -> {st:#x}");
    if !wdk_iddcx::nt_success(st) {
        remove_by_id(id);
        return Err(st);
    }
    let object = create_out.MonitorObject;
    let _ = monitor.object.set(Sendable(object));

    let mut arrival_out = iddcx::IDARG_OUT_MONITORARRIVAL::default();
    // SAFETY: `object` is the monitor just created; nothing departs it before it arrives.
    let st = unsafe { wdk_iddcx::IddCxMonitorArrival(object, &mut arrival_out) };
    dbglog!("[rlvd] IddCxMonitorArrival(id={id}) -> {st:#x}");
    if !wdk_iddcx::nt_success(st) {
        monitor.state.store(DEPARTED, Ordering::Release);
        remove_by_id(id);
        // SAFETY: `object` never arrived, so no departure ran on it, and only this thread
        // deletes it.
        unsafe {
            call_unsafe_wdf_function_binding!(WdfObjectDelete, object as WDFOBJECT);
        }
        return Err(st);
    }

    // A removal that ran while the create was in flight marked or unlinked the monitor;
    // nothing else will depart it, so undo it here.
    let arrived = monitor
        .state
        .compare_exchange(CREATING, ARRIVED, Ordering::AcqRel, Ordering::Acquire)
        .is_ok();
    let linked = registry::find(|m| m.id == id).is_some_and(|m| Arc::ptr_eq(&m, &monitor));
    if !arrived || !linked {
        dbglog!("[rlvd] create_monitor(id={id}): cleared mid-create; departing it");
        monitor.teardown();
        if !arrived || monitor.claim_departure() {
            // SAFETY: `object` arrived just above, and this is its one departure.
            unsafe { wdk_iddcx::IddCxMonitorDeparture(object) };
        }
        return Err(crate::STATUS_NOT_FOUND);
    }
    let arrival = Arrival {
        target_id: arrival_out.OsTargetId,
        luid_low: arrival_out.OsAdapterLuid.LowPart,
        luid_high: arrival_out.OsAdapterLuid.HighPart,
    };
    let _ = monitor.arrival.set(arrival);
    Ok((id, arrival.target_id, arrival.luid_low, arrival.luid_high))
}

/// `IOCTL_UPDATE_MODES`: lead the live monitor's list with a new mode and push the target
/// list with `IddCxMonitorUpdateModes2`. The list is a union (the OS pins the settable set at
/// arrival), and the stored list reverts if the DDI fails. The DDI runs with no lock held:
/// it re-enters the mode callbacks.
pub fn update_monitor_modes(
    owner: u32,
    session_id: u64,
    width: u32,
    height: u32,
    refresh_mhz: u32,
) -> NTSTATUS {
    let Some(m) = registry::find(|m| m.owner == owner && m.session_id == session_id) else {
        return crate::STATUS_NOT_FOUND;
    };
    let Some(object) = m.object().filter(|_| m.arrived()) else {
        return crate::STATUS_NOT_FOUND;
    };
    let Some(_ddi) = m.ddi() else {
        return crate::STATUS_NOT_FOUND;
    };
    let (old_modes, new_modes) = {
        let mut list = lock(&m.modes);
        let new_modes = modes::advertised_modes(
            Mode {
                width,
                height,
                refresh_mhz: vec![refresh_mhz],
            },
            &list,
        );
        (core::mem::replace(&mut *list, new_modes.clone()), new_modes)
    };
    let mut targets: Vec<iddcx::IDDCX_TARGET_MODE2> = flatten(&new_modes)
        .map(|item| target_mode2(item.width, item.height, item.refresh_mhz))
        .collect();
    let in_args = iddcx::IDARG_IN_UPDATEMODES2 {
        Reason: iddcx::IDDCX_UPDATE_REASON::IDDCX_UPDATE_REASON_OTHER,
        TargetModeCount: targets.len() as u32,
        pTargetModes: targets.as_mut_ptr(),
    };
    // SAFETY: `object` arrived and `_ddi` holds teardown and departure off until this
    // returns; `targets` outlives the synchronous call.
    let st = unsafe { wdk_iddcx::IddCxMonitorUpdateModes2(object, &in_args) };
    dbglog!(
        "[rlvd] IddCxMonitorUpdateModes2(session={session_id:#x}, {width}x{height}@{refresh_mhz}mHz) -> {st:#x}"
    );
    if !wdk_iddcx::nt_success(st) {
        *lock(&m.modes) = old_modes;
        return st;
    }
    crate::STATUS_SUCCESS
}

/// `IOCTL_REMOVE`. True if a monitor was removed.
pub fn remove_monitor(owner: u32, session_id: u64) -> bool {
    let removed = registry::remove(|m| m.owner == owner && m.session_id == session_id);
    let any = !removed.is_empty();
    depart(removed);
    any
}

/// `IOCTL_CLEAR_ALL`: only the caller's own monitors.
pub fn clear_all(owner: u32) {
    depart(registry::remove(|m| m.owner == owner));
}

/// Device removal: release every monitor's worker and ring without departing them; the
/// framework tears the IddCx monitors down with the device.
pub fn cleanup_for_device_removal() {
    for m in registry::remove(|_| true) {
        m.teardown();
    }
}

fn remove_by_id(id: u32) {
    for m in registry::remove(|m| m.id == id) {
        m.teardown();
    }
}

/// `owner`'s live monitor for `session_id`.
pub fn find_session(owner: u32, session_id: u64) -> Option<Arc<Monitor>> {
    registry::find(|m| m.owner == owner && m.session_id == session_id && m.arrived())
}

/// Whether `owner` still has `session_id`, for the lease heartbeat.
pub fn alive(owner: u32, session_id: u64) -> bool {
    find_session(owner, session_id).is_some()
}

/// Drop `owner`'s ring for `session_id`. True if there was one.
pub fn clear_frame_ring(owner: u32, session_id: u64) -> bool {
    find_session(owner, session_id)
        .and_then(|m| m.take_ring())
        .is_some()
}

/// Drop `owner`'s ring for `session_id` if it is still the ring of `generation`.
pub fn clear_frame_ring_generation(owner: u32, session_id: u64, generation: u32) -> bool {
    find_session(owner, session_id)
        .and_then(|m| m.take_ring_generation(generation))
        .is_some()
}
