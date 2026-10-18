//! IddCx adapter bring-up and the render-adapter pin.
//!
//! From punktfunk's pf-vdisplay `adapter.rs` by Enrico Bühler (unom), MIT OR Apache-2.0,
//! without the remote-session (seat) role.
//!
//! The adapter is created on the first `EvtDeviceD0Entry` and asynchronously: `init_adapter`
//! calls `IddCxAdapterInitAsync`, and the adapter object arrives in
//! `EvtIddCxAdapterInitFinished` ([`finish_init`]). The FP16 cap plus the `*2`, gamma and
//! HDR-metadata callbacks registered in `entry.rs` make the monitors HDR-capable.

use std::sync::{Mutex, RwLock};

use rubylight_vdisplay_proto::modes::MAX_MONITORS;
use wdk_sys::{NTSTATUS, WDFDEVICE, iddcx};
use windows::core::w;

use crate::STATUS_SUCCESS;
use crate::lifecycle::Lifecycle;

static LIFECYCLE: Mutex<Lifecycle> = Mutex::new(Lifecycle::new());
/// Drain control requests before suspending/removing capture resources. IddCx's mode
/// callbacks do not take this gate, so reentrant monitor creation remains safe.
pub static POWER_GATE: RwLock<()> = RwLock::new(());

pub fn powered() -> bool {
    crate::registry::lock(&LIFECYCLE).powered
}

pub fn enter(device: WDFDEVICE) {
    crate::registry::lock(&LIFECYCLE).enter(device as usize);
}

pub fn suspend() {
    crate::registry::lock(&LIFECYCLE).suspend();
}

pub fn finish_init(adapter: iddcx::IDDCX_ADAPTER, succeeded: bool) {
    crate::registry::lock(&LIFECYCLE).finish_init(adapter as usize, succeeded);
}

/// Re-issue a failed adapter init from an ADD: a root-enumerated device gets no further D0
/// entry to retry it. The ADD still fails; the host's retry lands after the completion.
pub fn retry_init() -> bool {
    let device = crate::registry::lock(&LIFECYCLE).device;
    if device == 0 {
        return false;
    }
    dbglog!("[rlvd] adapter: none at ADD; re-issuing the init");
    // SAFETY: `device` is the WDFDEVICE the last D0 entry ran on; an ADD arrives on that
    // device's queue, so it is live, and this WUDFHost hosts no other device.
    unsafe { init_adapter(device as WDFDEVICE) >= 0 }
}

/// Build the FP16-capable adapter caps and start the async adapter creation.
///
/// # Safety
/// `device` must be a live `WDFDEVICE`.
pub unsafe fn init_adapter(device: WDFDEVICE) -> NTSTATUS {
    let Some(sequence) = crate::registry::lock(&LIFECYCLE).begin_init() else {
        return STATUS_SUCCESS;
    };
    dbglog!("[rlvd] init_adapter");

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
    // SAFETY: `device` is live per the contract; `init` and `out` are locals the DDI reads
    // synchronously; the adapter arrives later in adapter_init_finished.
    let st = unsafe { wdk_iddcx::IddCxAdapterInitAsync(&init, &mut out) };
    dbglog!("[rlvd] IddCxAdapterInitAsync -> {st:#x}");
    crate::registry::lock(&LIFECYCLE)
        .init_returned(sequence, (st >= 0).then_some(out.AdapterObject as usize));
    if adapter().is_some() {
        // Covers an unusually early init-finished callback before the DDI returned.
        crate::watchdog::start();
    }
    st
}

/// Only a PnP stop invalidates the adapter. Sleep does not destroy IddCx objects:
/// https://learn.microsoft.com/windows-hardware/drivers/display/iddcx-objects
pub fn stop() {
    crate::registry::lock(&LIFECYCLE).stop();
    *crate::registry::lock(&RENDER_PIN) = None;
}

pub(crate) fn adapter() -> Option<iddcx::IDDCX_ADAPTER> {
    crate::registry::lock(&LIFECYCLE)
        .adapter()
        .map(|a| a as iddcx::IDDCX_ADAPTER)
}

/// The pin in force and the owner that set it. IddCx has one render adapter per adapter, so
/// this cannot be per owner.
static RENDER_PIN: Mutex<Option<(i64, u32)>> = Mutex::new(None);

/// Pin the GPU that renders the swap chains. On a hybrid PC Windows may otherwise render on
/// the integrated GPU, where the frame ring's textures (created on the selected GPU) cannot
/// open. Moving the pin flaps every live swap chain, so another owner's different GPU is
/// refused with `STATUS_ACCESS_DENIED` while the pinning owner still has a monitor. Busy
/// until the adapter exists, so the host retries instead of giving up on the pin.
pub fn set_render_adapter(owner: u32, luid_low: u32, luid_high: i32) -> NTSTATUS {
    let Some(adapter) = adapter() else {
        return crate::STATUS_DEVICE_BUSY;
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
