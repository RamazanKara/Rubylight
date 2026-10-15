//! IddCx adapter bring-up and the render-adapter pin.
//!
//! From punktfunk's pf-vdisplay `adapter.rs` by Enrico Bühler (unom), MIT OR Apache-2.0,
//! without the remote-session (seat) role.
//!
//! The adapter is created on the first `EvtDeviceD0Entry` and asynchronously: `init_adapter`
//! calls `IddCxAdapterInitAsync`, and the adapter object arrives in
//! `EvtIddCxAdapterInitFinished` ([`set_adapter`]). The FP16 cap plus the `*2`, gamma and
//! HDR-metadata callbacks registered in `entry.rs` make the monitors HDR-capable.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use rubylight_vdisplay_proto::modes::MAX_MONITORS;
use wdk_sys::{NTSTATUS, WDFDEVICE, iddcx};
use windows::core::w;

use crate::STATUS_SUCCESS;
use crate::worker::Sendable;

type SendAdapter = Sendable<iddcx::IDDCX_ADAPTER>;

/// Last write wins, so a re-init after a power cycle replaces the stale handle.
static ADAPTER: Mutex<Option<SendAdapter>> = Mutex::new(None);
/// The WDFDEVICE of the last init, so an ADD that finds no adapter can ask again.
static DEVICE: AtomicUsize = AtomicUsize::new(0);
/// An init is in flight; a second one would race it.
static INIT_PENDING: AtomicBool = AtomicBool::new(false);

pub fn init_settled() {
    INIT_PENDING.store(false, Ordering::Release);
}

/// Re-issue a failed adapter init from an ADD: a root-enumerated device gets no further D0
/// entry to retry it. The ADD still fails; the host's retry lands after the completion.
pub fn retry_init() -> bool {
    if adapter().is_some() || INIT_PENDING.load(Ordering::Acquire) {
        return false;
    }
    let device = DEVICE.load(Ordering::Acquire);
    if device == 0 {
        return false;
    }
    dbglog!("[rlvd] adapter: none at ADD; re-issuing the init");
    // SAFETY: `DEVICE` is the WDFDEVICE the last D0 entry ran on; an ADD arrives on that
    // device's queue, so it is live, and this WUDFHost hosts no other device.
    unsafe { init_adapter(device as WDFDEVICE) >= 0 }
}

/// Build the FP16-capable adapter caps and start the async adapter creation.
///
/// # Safety
/// `device` must be a live `WDFDEVICE`.
pub unsafe fn init_adapter(device: WDFDEVICE) -> NTSTATUS {
    if adapter().is_some() || INIT_PENDING.load(Ordering::Acquire) {
        return STATUS_SUCCESS;
    }
    dbglog!("[rlvd] init_adapter");
    DEVICE.store(device as usize, Ordering::Release);

    let mut version = iddcx::IDDCX_ENDPOINT_VERSION {
        Size: core::mem::size_of::<iddcx::IDDCX_ENDPOINT_VERSION>() as u32,
        MajorVer: env!("CARGO_PKG_VERSION_MAJOR").parse().unwrap_or(0),
        MinorVer: env!("CARGO_PKG_VERSION_MINOR").parse().unwrap_or(0),
        Build: env!("CARGO_PKG_VERSION_PATCH").parse().unwrap_or(0),
        ..Default::default()
    };
    // The name pointers must be 'static: IddCx reads them after this frame. GammaSupport must
    // be NONE: a zeroed value is UNINITIALIZED, which the adapter validation rejects.
    let diag = iddcx::IDDCX_ENDPOINT_DIAGNOSTIC_INFO {
        Size: core::mem::size_of::<iddcx::IDDCX_ENDPOINT_DIAGNOSTIC_INFO>() as u32,
        GammaSupport: iddcx::IDDCX_FEATURE_IMPLEMENTATION::IDDCX_FEATURE_IMPLEMENTATION_NONE,
        TransmissionType: iddcx::IDDCX_TRANSMISSION_TYPE::IDDCX_TRANSMISSION_TYPE_WIRED_OTHER,
        pEndPointFriendlyName: w!("Rubylight Virtual Display Adapter").as_ptr(),
        pEndPointManufacturerName: w!("Rubylight").as_ptr(),
        pEndPointModelName: w!("Virtual Display").as_ptr(),
        // `version` outlives this call, and IddCxAdapterInitAsync reads it synchronously.
        pFirmwareVersion: (&raw mut version).cast(),
        pHardwareVersion: (&raw mut version).cast(),
    };
    // CAN_PROCESS_FP16 marks the monitors advanced-colour capable; it obligates the `*2`
    // mode, commit and HDR callbacks entry.rs registers.
    let mut caps = iddcx::IDDCX_ADAPTER_CAPS {
        Size: core::mem::size_of::<iddcx::IDDCX_ADAPTER_CAPS>() as u32,
        Flags: iddcx::IDDCX_ADAPTER_FLAGS::IDDCX_ADAPTER_FLAGS_CAN_PROCESS_FP16,
        MaxMonitorsSupported: MAX_MONITORS,
        EndPointDiagnostics: diag,
        ..Default::default()
    };
    let mut attr = wdk_sys::WDF_OBJECT_ATTRIBUTES {
        Size: core::mem::size_of::<wdk_sys::WDF_OBJECT_ATTRIBUTES>() as u32,
        ExecutionLevel: wdk_sys::_WDF_EXECUTION_LEVEL::WdfExecutionLevelInheritFromParent,
        SynchronizationScope:
            wdk_sys::_WDF_SYNCHRONIZATION_SCOPE::WdfSynchronizationScopeInheritFromParent,
        ..Default::default()
    };
    let init = iddcx::IDARG_IN_ADAPTER_INIT {
        WdfDevice: device,
        pCaps: &raw mut caps,
        ObjectAttributes: &raw mut attr,
    };
    let mut out = iddcx::IDARG_OUT_ADAPTER_INIT::default();
    INIT_PENDING.store(true, Ordering::Release);
    // SAFETY: `device` is live per the contract; `init` and `out` are locals the DDI reads
    // synchronously; the adapter arrives later in adapter_init_finished.
    let st = unsafe { wdk_iddcx::IddCxAdapterInitAsync(&init, &mut out) };
    dbglog!("[rlvd] IddCxAdapterInitAsync -> {st:#x}");
    if st < 0 {
        INIT_PENDING.store(false, Ordering::Release);
    }
    st
}

pub fn set_adapter(adapter: iddcx::IDDCX_ADAPTER) {
    *crate::registry::lock(&ADAPTER) = Some(Sendable(adapter));
}

/// Forget the adapter after a real power cycle, so `init_adapter` runs again.
pub fn clear_adapter() {
    *crate::registry::lock(&ADAPTER) = None;
}

pub(crate) fn adapter() -> Option<iddcx::IDDCX_ADAPTER> {
    crate::registry::lock(&ADAPTER).as_ref().map(|a| a.0)
}

/// The pin in force and the owner that set it. IddCx has one render adapter per adapter, so
/// this cannot be per owner.
static RENDER_PIN: Mutex<Option<(i64, u32)>> = Mutex::new(None);

/// Pin the GPU that renders the swap chains. On a hybrid PC Windows may otherwise render on
/// the integrated GPU, where the frame ring's textures (created on the selected GPU) cannot
/// open. Moving the pin flaps every live swap chain, so another owner's different GPU is
/// refused with `STATUS_ACCESS_DENIED` while the pinning owner still has a monitor.
pub fn set_render_adapter(owner: u32, luid_low: u32, luid_high: i32) -> NTSTATUS {
    let Some(adapter) = adapter() else {
        return crate::STATUS_NOT_FOUND;
    };
    let packed = (i64::from(luid_high) << 32) | i64::from(luid_low);
    let pin = *crate::registry::lock(&RENDER_PIN);
    if let Some((held, by)) = pin
        && held != packed
        && by != owner
        && crate::registry::find(|m| m.owner == by).is_some()
    {
        dbglog!(
            "[rlvd] set_render_adapter: owner {owner} asked for {luid_high:08x}:{luid_low:08x} while owner {by} holds monitors on the pinned GPU; refused"
        );
        return crate::STATUS_ACCESS_DENIED;
    }
    *crate::registry::lock(&RENDER_PIN) = Some((packed, owner));
    let in_args = iddcx::IDARG_IN_ADAPTERSETRENDERADAPTER {
        PreferredRenderAdapter: wdk_sys::LUID {
            LowPart: luid_low,
            HighPart: luid_high,
        },
    };
    dbglog!("[rlvd] set_render_adapter -> {luid_high:08x}:{luid_low:08x}");
    // SAFETY: `adapter` is the stashed adapter; `in_args` is a local read synchronously.
    unsafe { wdk_iddcx::IddCxAdapterSetRenderAdapter(adapter, &in_args) };
    STATUS_SUCCESS
}
