//! `DriverEntry` and device add: the IddCx client config, the device, the control interface.
//!
//! From punktfunk's pf-vdisplay `entry.rs` by Enrico Bühler (unom), MIT OR Apache-2.0.

use wdk_iddcx::nt_success;
use wdk_sys::{
    GUID, NTSTATUS, PCUNICODE_STRING, PDRIVER_OBJECT, PWDFDEVICE_INIT, ULONG, WDF_DRIVER_CONFIG,
    WDF_NO_HANDLE, WDF_NO_OBJECT_ATTRIBUTES, WDF_PNPPOWER_EVENT_CALLBACKS, WDFDEVICE, WDFDRIVER,
    call_unsafe_wdf_function_binding, iddcx,
};

use crate::callbacks;

#[unsafe(export_name = "DriverEntry")]
pub unsafe extern "system" fn driver_entry(
    driver: PDRIVER_OBJECT,
    registry_path: PCUNICODE_STRING,
) -> NTSTATUS {
    dbglog!(
        "[rlvd] DriverEntry (driver {}, protocol {})",
        env!("CARGO_PKG_VERSION"),
        rubylight_vdisplay_proto::PROTOCOL_VERSION
    );
    let mut config = WDF_DRIVER_CONFIG {
        Size: core::mem::size_of::<WDF_DRIVER_CONFIG>() as ULONG,
        EvtDriverDeviceAdd: Some(driver_add),
        ..Default::default()
    };
    // SAFETY: `driver` and `registry_path` come from the loader; `config` is valid.
    let st = unsafe {
        call_unsafe_wdf_function_binding!(
            WdfDriverCreate,
            driver,
            registry_path,
            WDF_NO_OBJECT_ATTRIBUTES,
            &mut config,
            WDF_NO_HANDLE.cast::<WDFDRIVER>()
        )
    };
    dbglog!("[rlvd] WdfDriverCreate -> {st:#x}");
    st
}

extern "C" fn driver_add(_driver: WDFDRIVER, mut init: PWDFDEVICE_INIT) -> NTSTATUS {
    dbglog!("[rlvd] driver_add");
    // The adapter is created on the first D0 entry.
    let mut pnp = WDF_PNPPOWER_EVENT_CALLBACKS {
        Size: core::mem::size_of::<WDF_PNPPOWER_EVENT_CALLBACKS>() as ULONG,
        EvtDeviceD0Entry: Some(callbacks::device_d0_entry),
        EvtDeviceD0Exit: Some(callbacks::device_d0_exit),
        EvtDeviceReleaseHardware: Some(callbacks::device_release_hardware),
        ..Default::default()
    };
    // SAFETY: `init` is the framework's device-init; `pnp` is valid for the call.
    unsafe {
        call_unsafe_wdf_function_binding!(WdfDeviceInitSetPnpPowerEventCallbacks, init, &mut pnp);
    }

    // A control handle's close is the owner-gone signal, so a crashed host's monitors depart
    // at once instead of after the watchdog window.
    let mut files = wdk_sys::WDF_FILEOBJECT_CONFIG {
        Size: core::mem::size_of::<wdk_sys::WDF_FILEOBJECT_CONFIG>() as ULONG,
        EvtFileCleanup: Some(crate::watchdog::evt_file_cleanup),
        AutoForwardCleanupClose: wdk_sys::_WDF_TRI_STATE::WdfUseDefault,
        FileObjectClass: wdk_sys::_WDF_FILEOBJECT_CLASS::WdfFileObjectWdfCannotUseFsContexts,
        ..Default::default()
    };
    // SAFETY: `init` is the framework's device-init; `files` is valid for the call.
    unsafe {
        call_unsafe_wdf_function_binding!(
            WdfDeviceInitSetFileObjectConfig,
            init,
            &mut files,
            WDF_NO_OBJECT_ATTRIBUTES
        );
    }

    let cfg = iddcx::IDD_CX_CLIENT_CONFIG {
        Size: core::mem::size_of::<iddcx::IDD_CX_CLIENT_CONFIG>() as u32,
        EvtIddCxAdapterInitFinished: Some(callbacks::adapter_init_finished),
        EvtIddCxParseMonitorDescription: Some(callbacks::parse_monitor_description),
        EvtIddCxMonitorGetDefaultDescriptionModes: Some(callbacks::monitor_get_default_modes),
        EvtIddCxMonitorQueryTargetModes: Some(callbacks::monitor_query_modes),
        EvtIddCxAdapterCommitModes: Some(callbacks::adapter_commit_modes),
        // The `*2` mode DDIs and the HDR callbacks, obligated by CAN_PROCESS_FP16. The OS
        // prefers them on IddCx 1.10 and falls back to the v1 set down-level.
        EvtIddCxParseMonitorDescription2: Some(callbacks::parse_monitor_description2),
        EvtIddCxMonitorQueryTargetModes2: Some(callbacks::monitor_query_modes2),
        EvtIddCxAdapterCommitModes2: Some(callbacks::adapter_commit_modes2),
        EvtIddCxAdapterQueryTargetInfo: Some(callbacks::query_target_info),
        EvtIddCxMonitorSetDefaultHdrMetaData: Some(callbacks::set_default_hdr_metadata),
        EvtIddCxMonitorSetGammaRamp: Some(callbacks::set_gamma_ramp),
        EvtIddCxMonitorAssignSwapChain: Some(callbacks::assign_swap_chain),
        EvtIddCxMonitorUnassignSwapChain: Some(callbacks::unassign_swap_chain),
        EvtIddCxDeviceIoControl: Some(callbacks::device_io_control),
        ..Default::default()
    };
    // SAFETY: `init` is the framework's device-init; `cfg` is fully populated and sized.
    let status = unsafe { wdk_iddcx::IddCxDeviceInitConfig(init, &cfg) };
    dbglog!("[rlvd] IddCxDeviceInitConfig -> {status:#x}");
    if !nt_success(status) {
        return status;
    }

    let mut device: WDFDEVICE = core::ptr::null_mut();
    // The cleanup callback stops every monitor's swap-chain worker on device removal.
    let mut dev_attr = wdk_sys::WDF_OBJECT_ATTRIBUTES {
        Size: core::mem::size_of::<wdk_sys::WDF_OBJECT_ATTRIBUTES>() as u32,
        ExecutionLevel: wdk_sys::_WDF_EXECUTION_LEVEL::WdfExecutionLevelInheritFromParent,
        SynchronizationScope:
            wdk_sys::_WDF_SYNCHRONIZATION_SCOPE::WdfSynchronizationScopeInheritFromParent,
        EvtCleanupCallback: Some(callbacks::device_cleanup),
        ..Default::default()
    };
    // SAFETY: `init` was configured above; `dev_attr` is a valid attributes block.
    let status = unsafe {
        call_unsafe_wdf_function_binding!(WdfDeviceCreate, &mut init, &mut dev_attr, &mut device)
    };
    dbglog!("[rlvd] WdfDeviceCreate -> {status:#x}");
    if !nt_success(status) {
        return status;
    }

    // SAFETY: `device` is the just-created WDFDEVICE.
    let status = unsafe { wdk_iddcx::IddCxDeviceInitialize(device) };
    dbglog!("[rlvd] IddCxDeviceInitialize -> {status:#x}");
    if !nt_success(status) {
        return status;
    }

    // The watchdog timer exists before the interface does, so no IOCTL beats it.
    let status = crate::watchdog::create(device);
    if !nt_success(status) {
        return status;
    }

    // Rubylight's own control interface: the host opens this GUID and sends the
    // rubylight-vdisplay-proto IOCTLs, which arrive in EvtIddCxDeviceIoControl.
    let (d1, d2, d3, d4) = rubylight_vdisplay_proto::interface_guid_fields();
    let guid = GUID {
        Data1: d1,
        Data2: d2,
        Data3: d3,
        Data4: d4,
    };
    // SAFETY: `device` is the just-created WDFDEVICE; `guid` lives for the call.
    let status = unsafe {
        call_unsafe_wdf_function_binding!(
            WdfDeviceCreateDeviceInterface,
            device,
            &guid,
            core::ptr::null()
        )
    };
    dbglog!("[rlvd] WdfDeviceCreateDeviceInterface -> {status:#x}");
    status
}
