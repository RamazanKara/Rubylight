//! The IddCx client-config callbacks and the PnP `EvtDeviceD0Entry`.
//!
//! From punktfunk's pf-vdisplay `callbacks.rs` by Enrico Bühler (unom), MIT OR Apache-2.0,
//! without the remote-session (seat) presenter and the hardware-cursor re-declare.
//!
//! Every callback is `unsafe extern "C"` to match the `PFN_IDD_CX_*` types. The driver builds
//! with `panic = "abort"`, so a panic in any of them takes WUDFHost down instead of unwinding
//! across the FFI boundary. The `*2` (HDR) mode and commit callbacks are registered beside
//! their v1 twins; each pair shares one fill helper.

use std::sync::Arc;

use rubylight_vdisplay_proto::{edid, modes::flatten};
use wdk_sys::iddcx;
use wdk_sys::{NTSTATUS, WDFDEVICE, WDFOBJECT, WDFREQUEST, call_unsafe_wdf_function_binding};

use crate::{
    STATUS_BUFFER_TOO_SMALL, STATUS_INVALID_PARAMETER, STATUS_NOT_FOUND, STATUS_NOT_IMPLEMENTED,
    STATUS_SUCCESS,
};

/// A successful IddCx adapter survives sleep. Only a PnP stop (ReleaseHardware)
/// permits another initialization; dropping its handle on resume orphans its monitors.
pub unsafe extern "C" fn device_d0_entry(
    device: WDFDEVICE,
    previous_state: wdk_sys::WDF_POWER_DEVICE_STATE,
) -> NTSTATUS {
    dbglog!("[rlvd] device_d0_entry (previous_state={previous_state})");
    crate::adapter::enter(device);
    if crate::adapter::adapter().is_some() {
        crate::watchdog::start();
        return STATUS_SUCCESS;
    }
    // SAFETY: `device` is the framework's live WDFDEVICE for this D0 entry.
    unsafe { crate::adapter::init_adapter(device) }
}

/// Stop capture before power is removed, retaining monitor ownership and IddCx objects.
/// The next assignment recreates the swap-chain worker; the host rebuilds its frame ring.
pub unsafe extern "C" fn device_d0_exit(
    _device: WDFDEVICE,
    target_state: wdk_sys::WDF_POWER_DEVICE_STATE,
) -> NTSTATUS {
    dbglog!("[rlvd] device_d0_exit (target_state={target_state})");
    if target_state == wdk_sys::_WDF_POWER_DEVICE_STATE::WdfPowerDevicePrepareForHibernation {
        return STATUS_SUCCESS;
    }
    let _requests = crate::adapter::POWER_GATE
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    crate::adapter::suspend();
    crate::watchdog::pause();
    crate::monitor::suspend_capture();
    crate::direct_3d_device::retire_pool();
    STATUS_SUCCESS
}

/// A real PnP stop invalidates the adapter and its children, even if the WDFDEVICE
/// survives a resource rebalance. This also runs for removal from an already sleeping state.
pub unsafe extern "C" fn device_release_hardware(
    _device: WDFDEVICE,
    _resources: wdk_sys::WDFCMRESLIST,
) -> NTSTATUS {
    let _requests = crate::adapter::POWER_GATE
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    crate::adapter::suspend();
    crate::watchdog::pause();
    crate::monitor::cleanup_for_device_removal();
    crate::direct_3d_device::retire_pool();
    crate::adapter::stop();
    STATUS_SUCCESS
}

/// Completion of `IddCxAdapterInitAsync`: keep the adapter only if the init succeeded (a
/// failed one is a husk whose monitors never get a swap chain) and arm the watchdog.
pub unsafe extern "C" fn adapter_init_finished(
    adapter: iddcx::IDDCX_ADAPTER,
    p_in: *const iddcx::IDARG_IN_ADAPTER_INIT_FINISHED,
) -> NTSTATUS {
    // SAFETY: the framework passes a valid input pointer for the call.
    let status = unsafe { (*p_in).AdapterInitStatus };
    dbglog!("[rlvd] adapter_init_finished (AdapterInitStatus={status:#010x})");
    crate::adapter::finish_init(adapter, status >= 0);
    if crate::adapter::adapter().is_some() {
        crate::watchdog::start();
    }
    STATUS_SUCCESS
}

/// `EvtCleanupCallback` of the WDFDEVICE: stop the watchdog (waiting out a running reap),
/// then release every monitor's worker and ring. The framework departs the monitors.
pub unsafe extern "C" fn device_cleanup(_object: WDFOBJECT) {
    dbglog!("[rlvd] device cleanup; releasing monitors");
    crate::adapter::suspend();
    crate::watchdog::stop();
    crate::monitor::cleanup_for_device_removal();
    crate::direct_3d_device::retire_pool();
    crate::adapter::stop();
}

fn monitor_mode(width: u32, height: u32, refresh_mhz: u32) -> iddcx::IDDCX_MONITOR_MODE {
    iddcx::IDDCX_MONITOR_MODE {
        Size: core::mem::size_of::<iddcx::IDDCX_MONITOR_MODE>() as u32,
        Origin: iddcx::IDDCX_MONITOR_MODE_ORIGIN::IDDCX_MONITOR_MODE_ORIGIN_MONITORDESCRIPTOR,
        MonitorVideoSignalInfo: crate::monitor::display_info(width, height, refresh_mhz),
    }
}

fn monitor_mode2(width: u32, height: u32, refresh_mhz: u32) -> iddcx::IDDCX_MONITOR_MODE2 {
    iddcx::IDDCX_MONITOR_MODE2 {
        Size: core::mem::size_of::<iddcx::IDDCX_MONITOR_MODE2>() as u32,
        Origin: iddcx::IDDCX_MONITOR_MODE_ORIGIN::IDDCX_MONITOR_MODE_ORIGIN_MONITORDESCRIPTOR,
        MonitorVideoSignalInfo: crate::monitor::display_info(width, height, refresh_mhz),
        BitsPerComponent: crate::monitor::wire_bits(),
    }
}

/// Both `EvtIddCxParseMonitorDescription` variants: the EDID serial selects the monitor's
/// list, `make` writes one entry per mode. A zero `in_count` is a count-only probe.
///
/// # Safety
/// `p_modes` must point at `in_count` writable `M` entries.
unsafe fn fill_monitor_modes<M>(
    edid: &[u8],
    in_count: u32,
    p_modes: *mut M,
    out_args: &mut iddcx::IDARG_OUT_PARSEMONITORDESCRIPTION,
    make: impl Fn(u32, u32, u32) -> M,
) -> NTSTATUS {
    let Ok(id) = edid::get_serial(edid) else {
        return STATUS_INVALID_PARAMETER;
    };
    let Some(modes) = crate::registry::find(|m| m.id == id).map(|m| m.modes()) else {
        return STATUS_NOT_FOUND;
    };
    let count = flatten(&modes).count() as u32;
    out_args.MonitorModeBufferOutputCount = count;
    if in_count < count {
        return if in_count > 0 {
            STATUS_BUFFER_TOO_SMALL
        } else {
            STATUS_SUCCESS
        };
    }
    // SAFETY: the caller's buffer holds `in_count >= count` entries.
    let out = unsafe { core::slice::from_raw_parts_mut(p_modes, count as usize) };
    for (item, slot) in flatten(&modes).zip(out.iter_mut()) {
        *slot = make(item.width, item.height, item.refresh_mhz);
    }
    out_args.PreferredMonitorModeIdx = 0;
    STATUS_SUCCESS
}

/// Description modes, v1 (the OS calls it only below IddCx 1.10).
pub unsafe extern "C" fn parse_monitor_description(
    p_in: *const iddcx::IDARG_IN_PARSEMONITORDESCRIPTION,
    p_out: *mut iddcx::IDARG_OUT_PARSEMONITORDESCRIPTION,
) -> NTSTATUS {
    // SAFETY: the framework passes live in/out args, an EDID of `DataSize` bytes and room for
    // `MonitorModeBufferInputCount` modes.
    unsafe {
        let in_args = &*p_in;
        fill_monitor_modes(
            core::slice::from_raw_parts(
                in_args.MonitorDescription.pData.cast::<u8>(),
                in_args.MonitorDescription.DataSize as usize,
            ),
            in_args.MonitorModeBufferInputCount,
            in_args.pMonitorModes,
            &mut *p_out,
            monitor_mode,
        )
    }
}

/// Description modes with wire bit depths: mandatory under FP16, and what IddCx 1.10 calls.
pub unsafe extern "C" fn parse_monitor_description2(
    p_in: *const iddcx::IDARG_IN_PARSEMONITORDESCRIPTION2,
    p_out: *mut iddcx::IDARG_OUT_PARSEMONITORDESCRIPTION,
) -> NTSTATUS {
    // SAFETY: as for `parse_monitor_description`, with IDDCX_MONITOR_MODE2 entries.
    unsafe {
        let in_args = &*p_in;
        fill_monitor_modes(
            core::slice::from_raw_parts(
                in_args.MonitorDescription.pData.cast::<u8>(),
                in_args.MonitorDescription.DataSize as usize,
            ),
            in_args.MonitorModeBufferInputCount,
            in_args.pMonitorModes,
            &mut *p_out,
            monitor_mode2,
        )
    }
}

/// Only called for monitors without an EDID; ours always have one.
pub unsafe extern "C" fn monitor_get_default_modes(
    _monitor: iddcx::IDDCX_MONITOR,
    _p_in: *const iddcx::IDARG_IN_GETDEFAULTDESCRIPTIONMODES,
    _p_out: *mut iddcx::IDARG_OUT_GETDEFAULTDESCRIPTIONMODES,
) -> NTSTATUS {
    STATUS_NOT_IMPLEMENTED
}

/// Both `EvtIddCxMonitorQueryTargetModes` variants: the count is always reported and the
/// buffer filled only when it is large enough.
///
/// # Safety
/// `p_modes` must point at `in_count` writable `M` entries.
unsafe fn fill_target_modes<M>(
    monitor: iddcx::IDDCX_MONITOR,
    in_count: u32,
    p_modes: *mut M,
    out_args: &mut iddcx::IDARG_OUT_QUERYTARGETMODES,
    make: impl Fn(u32, u32, u32) -> M,
) -> NTSTATUS {
    let Some(modes) = crate::registry::find(|m| m.object() == Some(monitor)).map(|m| m.modes())
    else {
        return STATUS_NOT_FOUND;
    };
    let count = flatten(&modes).count() as u32;
    out_args.TargetModeBufferOutputCount = count;
    if in_count >= count {
        // SAFETY: the caller's buffer holds `in_count >= count` entries.
        let out = unsafe { core::slice::from_raw_parts_mut(p_modes, count as usize) };
        for (item, slot) in flatten(&modes).zip(out.iter_mut()) {
            *slot = make(item.width, item.height, item.refresh_mhz);
        }
    }
    STATUS_SUCCESS
}

pub unsafe extern "C" fn monitor_query_modes(
    monitor: iddcx::IDDCX_MONITOR,
    p_in: *const iddcx::IDARG_IN_QUERYTARGETMODES,
    p_out: *mut iddcx::IDARG_OUT_QUERYTARGETMODES,
) -> NTSTATUS {
    // SAFETY: the framework passes live in/out args with room for
    // `TargetModeBufferInputCount` IDDCX_TARGET_MODE entries.
    unsafe {
        let in_args = &*p_in;
        fill_target_modes(
            monitor,
            in_args.TargetModeBufferInputCount,
            in_args.pTargetModes,
            &mut *p_out,
            crate::monitor::target_mode,
        )
    }
}

pub unsafe extern "C" fn monitor_query_modes2(
    monitor: iddcx::IDDCX_MONITOR,
    p_in: *const iddcx::IDARG_IN_QUERYTARGETMODES2,
    p_out: *mut iddcx::IDARG_OUT_QUERYTARGETMODES,
) -> NTSTATUS {
    // SAFETY: as for `monitor_query_modes`, with IDDCX_TARGET_MODE2 entries.
    unsafe {
        let in_args = &*p_in;
        fill_target_modes(
            monitor,
            in_args.TargetModeBufferInputCount,
            in_args.pTargetModes,
            &mut *p_out,
            crate::monitor::target_mode2,
        )
    }
}

/// The framework's path array as a slice; empty when null.
///
/// # Safety
/// A non-null `ptr` must point at `count` initialised entries outliving the slice.
unsafe fn path_slice<'a, P>(ptr: *const P, count: u32) -> &'a [P] {
    if ptr.is_null() {
        return &[];
    }
    // SAFETY: non-null and `count` entries per the contract.
    unsafe { core::slice::from_raw_parts(ptr, count as usize) }
}

/// Commit is a no-op for the driver; the per-path ACTIVE/CHANGED flags are logged so a frame
/// gap can be lined up with the OS idling the head.
pub unsafe extern "C" fn adapter_commit_modes(
    _adapter: iddcx::IDDCX_ADAPTER,
    p_in: *const iddcx::IDARG_IN_COMMITMODES,
) -> NTSTATUS {
    // SAFETY: the framework passes a valid input pointer for the call.
    let in_args = unsafe { &*p_in };
    // SAFETY: `pPaths` holds `PathCount` paths (framework contract).
    for (i, path) in unsafe { path_slice(in_args.pPaths, in_args.PathCount) }
        .iter()
        .enumerate()
    {
        dbglog!(
            "[rlvd] commit_modes: path[{i}] active={} changed={}",
            path.Flags & 0x2 != 0,
            path.Flags & 0x1 != 0
        );
    }
    STATUS_SUCCESS
}

/// The `*2` commit the OS calls once CAN_PROCESS_FP16 is set.
pub unsafe extern "C" fn adapter_commit_modes2(
    _adapter: iddcx::IDDCX_ADAPTER,
    p_in: *const iddcx::IDARG_IN_COMMITMODES2,
) -> NTSTATUS {
    // SAFETY: the framework passes a valid input pointer for the call.
    let in_args = unsafe { &*p_in };
    // SAFETY: `pPaths` holds `PathCount` paths (framework contract).
    for (i, path) in unsafe { path_slice(in_args.pPaths, in_args.PathCount) }
        .iter()
        .enumerate()
    {
        dbglog!(
            "[rlvd] commit_modes2: path[{i}] active={} changed={}",
            path.Flags & 0x2 != 0,
            path.Flags & 0x1 != 0
        );
    }
    STATUS_SUCCESS
}

/// HIGH_COLOR_SPACE for HDR10 and WIDE_COLOR_SPACE for SDR wide colour. Mandatory under FP16.
pub unsafe extern "C" fn query_target_info(
    _adapter: iddcx::IDDCX_ADAPTER,
    _p_in: *mut iddcx::IDARG_IN_QUERYTARGET_INFO,
    p_out: *mut iddcx::IDARG_OUT_QUERYTARGET_INFO,
) -> NTSTATUS {
    // SAFETY: `p_out` is the framework's out buffer; zero it, then set the one field.
    unsafe {
        core::ptr::write(p_out, iddcx::IDARG_OUT_QUERYTARGET_INFO::default());
        (*p_out).TargetCaps = iddcx::IDDCX_TARGET_CAPS::IDDCX_TARGET_CAPS_WIDE_COLOR_SPACE
            | iddcx::IDDCX_TARGET_CAPS::IDDCX_TARGET_CAPS_HIGH_COLOR_SPACE;
    }
    STATUS_SUCCESS
}

/// Accept the OS's default HDR10 metadata; the host and client own the stream's. Mandatory
/// under FP16.
pub unsafe extern "C" fn set_default_hdr_metadata(
    _monitor: iddcx::IDDCX_MONITOR,
    _p_in: *const iddcx::IDARG_IN_MONITOR_SET_DEFAULT_HDR_METADATA,
) -> NTSTATUS {
    STATUS_SUCCESS
}

/// Accept, but do not apply, the gamma ramp: the client display applies its own. Mandatory
/// under FP16, or the OS rejects the adapter.
pub unsafe extern "C" fn set_gamma_ramp(
    _monitor: iddcx::IDDCX_MONITOR,
    _p_in: *const iddcx::IDARG_IN_SET_GAMMARAMP,
) -> NTSTATUS {
    STATUS_SUCCESS
}

/// A swap chain was assigned: start the worker that drains it into the ring. Always
/// succeeds; when no device can be made the swap chain is deleted so the OS assigns again.
/// Every displaced worker is joined with no lock held.
pub unsafe extern "C" fn assign_swap_chain(
    monitor: iddcx::IDDCX_MONITOR,
    p_in: *const iddcx::IDARG_IN_SETSWAPCHAIN,
) -> NTSTATUS {
    // SAFETY: the framework passes valid input args for the call.
    let in_args = unsafe { &*p_in };
    let swap_chain = in_args.hSwapChain;
    let render_adapter = in_args.RenderAdapterLuid;
    let luid = windows::Win32::Foundation::LUID {
        LowPart: render_adapter.LowPart,
        HighPart: render_adapter.HighPart,
    };
    dbglog!(
        "[rlvd] assign_swap_chain: render adapter {:08x}:{:08x}",
        render_adapter.HighPart,
        render_adapter.LowPart
    );
    let entry = crate::registry::find(|m| m.object() == Some(monitor));
    if let Some(m) = &entry {
        drop(m.take_swap());
    }
    if let Some(device) = crate::direct_3d_device::pooled_device(luid) {
        if let Some(m) = &entry {
            m.set_render_luid(luid);
        }
        let mut processor = crate::swap_chain_processor::SwapChainProcessor::new();
        processor.run(
            swap_chain,
            device,
            in_args.hNextSurfaceAvailable,
            entry.as_ref().map(Arc::downgrade).unwrap_or_default(),
        );
        match &entry {
            Some(m) => drop(m.set_swap(processor)),
            None => drop(processor),
        }
    } else {
        dbglog!("[rlvd] assign_swap_chain: no D3D device; deleting the swap chain for a retry");
        // SAFETY: `swap_chain` is the framework's live swap chain handle.
        unsafe {
            call_unsafe_wdf_function_binding!(WdfObjectDelete, swap_chain as WDFOBJECT);
        }
    }
    STATUS_SUCCESS
}

/// The monitor went inactive: drop its worker (joins it; the worker deletes the swap chain).
pub unsafe extern "C" fn unassign_swap_chain(monitor: iddcx::IDDCX_MONITOR) -> NTSTATUS {
    let had = crate::registry::find(|m| m.object() == Some(monitor)).and_then(|m| m.take_swap());
    let live = had.is_some();
    let started = std::time::Instant::now();
    drop(had);
    dbglog!(
        "[rlvd] unassign_swap_chain: worker {live}, join took {} us",
        started.elapsed().as_micros()
    );
    STATUS_SUCCESS
}

/// The control plane; completes the request itself.
pub unsafe extern "C" fn device_io_control(
    _device: WDFDEVICE,
    request: WDFREQUEST,
    _output_len: usize,
    _input_len: usize,
    ioctl_code: u32,
) {
    // SAFETY: `request` is this callback's live request; `dispatch` completes it once.
    unsafe { crate::control::dispatch(request, ioctl_code) };
}
