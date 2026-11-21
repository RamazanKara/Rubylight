// Copyright (c) 2026 Chase Payne
// SPDX-License-Identifier: MIT
//
// Ported from Nonary/libvirtualgamepad driver/src/driver.cpp.
// Adapted for Rubylight by RamazanKara (https://github.com/RamazanKara/Rubylight).

//! WDF entry point, device creation, PnP/Power callbacks, file create/close, and IOCTL dispatch.

use rubylight_vgamepad_proto::guid_fields;
use rubylight_vgamepad_proto::protocol::DEVICE_INTERFACE_GUID;
use wdk_sys::{
    GUID, NTSTATUS, PCUNICODE_STRING, PDRIVER_OBJECT, PWDFDEVICE_INIT, ULONG, WDF_DRIVER_CONFIG,
    WDF_FILEOBJECT_CONFIG, WDF_IO_QUEUE_CONFIG, WDF_NO_HANDLE, WDF_NO_OBJECT_ATTRIBUTES,
    WDF_PNPPOWER_EVENT_CALLBACKS, WDF_POWER_DEVICE_STATE, WDFCMRESLIST, WDFDEVICE, WDFDRIVER,
    WDFFILEOBJECT, WDFIOTARGET, WDFOBJECT, WDFQUEUE, WDFREQUEST, call_unsafe_wdf_function_binding,
};

use crate::device::{DeviceContext, lookup_device, register_device, unregister_device};
use crate::wdf::Request;
use crate::{STATUS_SUCCESS, nt_success};

fn wdf_object_attributes_init() -> wdk_sys::WDF_OBJECT_ATTRIBUTES {
    wdk_sys::WDF_OBJECT_ATTRIBUTES {
        Size: core::mem::size_of::<wdk_sys::WDF_OBJECT_ATTRIBUTES>() as ULONG,
        ExecutionLevel: wdk_sys::_WDF_EXECUTION_LEVEL::WdfExecutionLevelInheritFromParent,
        SynchronizationScope:
            wdk_sys::_WDF_SYNCHRONIZATION_SCOPE::WdfSynchronizationScopeInheritFromParent,
        ..Default::default()
    }
}

/// UMDF driver entry point called by `WdfDriverStubUm`.
///
/// # Safety
/// Called by the WDF reflector with valid framework `driver` and `registry_path` pointers.
#[unsafe(export_name = "DriverEntry")]
pub unsafe extern "system" fn driver_entry(
    driver: PDRIVER_OBJECT,
    registry_path: PCUNICODE_STRING,
) -> NTSTATUS {
    let mut cfg = WDF_DRIVER_CONFIG {
        Size: core::mem::size_of::<WDF_DRIVER_CONFIG>() as ULONG,
        EvtDriverDeviceAdd: Some(evt_driver_device_add),
        ..Default::default()
    };
    let mut h_driver: WDFDRIVER = WDF_NO_HANDLE as WDFDRIVER;
    // SAFETY: Framework passed valid `driver` and `registry_path`; `cfg` is initialized on stack.
    let status = unsafe {
        call_unsafe_wdf_function_binding!(
            WdfDriverCreate,
            driver,
            registry_path,
            WDF_NO_OBJECT_ATTRIBUTES,
            &mut cfg,
            &mut h_driver
        )
    };
    dbglog!("DriverEntry WdfDriverCreate -> {status:#x}");
    status
}

unsafe extern "C" fn evt_driver_device_add(
    _driver: WDFDRIVER,
    device_init: PWDFDEVICE_INIT,
) -> NTSTATUS {
    // This INF installs vhf.sys (VhfMode=2) as function driver and WUDFRd as UpperFilter.
    // SAFETY: `device_init` is live during `EvtDriverDeviceAdd`.
    unsafe { call_unsafe_wdf_function_binding!(WdfFdoInitSetFilter, device_init) };

    let mut pnp = WDF_PNPPOWER_EVENT_CALLBACKS {
        Size: core::mem::size_of::<WDF_PNPPOWER_EVENT_CALLBACKS>() as ULONG,
        EvtDevicePrepareHardware: Some(evt_device_prepare_hardware),
        EvtDeviceReleaseHardware: Some(evt_device_release_hardware),
        EvtDeviceD0Entry: Some(evt_device_d0_entry),
        EvtDeviceD0Exit: Some(evt_device_d0_exit),
        ..Default::default()
    };
    // SAFETY: `device_init` and `pnp` are valid locals.
    unsafe {
        call_unsafe_wdf_function_binding!(
            WdfDeviceInitSetPnpPowerEventCallbacks,
            device_init,
            &mut pnp
        )
    };

    let mut file_cfg = WDF_FILEOBJECT_CONFIG {
        Size: core::mem::size_of::<WDF_FILEOBJECT_CONFIG>() as ULONG,
        EvtDeviceFileCreate: Some(evt_file_create),
        EvtFileClose: Some(evt_file_close),
        AutoForwardCleanupClose: wdk_sys::_WDF_TRI_STATE::WdfFalse,
        FileObjectClass: wdk_sys::_WDF_FILEOBJECT_CLASS::WdfFileObjectWdfCannotUseFsContexts,
        ..Default::default()
    };
    let mut file_attr = wdf_object_attributes_init();
    file_attr.ExecutionLevel = wdk_sys::_WDF_EXECUTION_LEVEL::WdfExecutionLevelPassive;
    file_attr.SynchronizationScope =
        wdk_sys::_WDF_SYNCHRONIZATION_SCOPE::WdfSynchronizationScopeNone;
    // SAFETY: `device_init`, `file_cfg`, and `file_attr` are valid initialized locals.
    unsafe {
        call_unsafe_wdf_function_binding!(
            WdfDeviceInitSetFileObjectConfig,
            device_init,
            &mut file_cfg,
            &mut file_attr
        )
    };

    let mut dev_attr = wdf_object_attributes_init();
    dev_attr.ExecutionLevel = wdk_sys::_WDF_EXECUTION_LEVEL::WdfExecutionLevelPassive;
    dev_attr.SynchronizationScope =
        wdk_sys::_WDF_SYNCHRONIZATION_SCOPE::WdfSynchronizationScopeNone;
    dev_attr.EvtCleanupCallback = Some(evt_device_cleanup);
    let mut device: WDFDEVICE = core::ptr::null_mut();
    // SAFETY: `device_init` ownership transfers to WDF on success; `device` receives the handle.
    let status = unsafe {
        call_unsafe_wdf_function_binding!(
            WdfDeviceCreate,
            &mut { device_init },
            &mut dev_attr,
            &mut device
        )
    };
    if !nt_success(status) {
        dbglog!("WdfDeviceCreate failed: {status:#x}");
        return status;
    }

    let mut target_attr = wdf_object_attributes_init();
    target_attr.ParentObject = device.cast();
    target_attr.EvtCleanupCallback = Some(evt_vhf_target_cleanup);
    let mut local_vhf_target: WDFIOTARGET = core::ptr::null_mut();
    // SAFETY: `device` is live and `target_attr` parents the local I/O target to `device`.
    let target_st = unsafe {
        call_unsafe_wdf_function_binding!(
            WdfIoTargetCreate,
            device,
            &mut target_attr,
            &mut local_vhf_target
        )
    };
    if !nt_success(target_st) {
        dbglog!("WdfIoTargetCreate failed: {target_st:#x}");
        return target_st;
    }

    let ctx = DeviceContext::new(local_vhf_target);
    let timer_st = ctx.pid_timer.create(device);
    if !nt_success(timer_st) {
        // Non-fatal: PID force feedback still delivers immediate reports without timer shaping.
        dbglog!("PidTimer create failed cleanly (optional): {timer_st:#x}");
    }
    register_device(device, ctx);

    let (d1, d2, d3, d4) = guid_fields(DEVICE_INTERFACE_GUID);
    let guid = GUID {
        Data1: d1,
        Data2: d2,
        Data3: d3,
        Data4: d4,
    };
    // SAFETY: `device` and `guid` are valid; no reference string is used.
    let iface_st = unsafe {
        call_unsafe_wdf_function_binding!(
            WdfDeviceCreateDeviceInterface,
            device,
            &guid,
            core::ptr::null()
        )
    };
    if !nt_success(iface_st) {
        dbglog!("WdfDeviceCreateDeviceInterface failed: {iface_st:#x}");
        return iface_st;
    }

    let mut queue_cfg = WDF_IO_QUEUE_CONFIG {
        Size: core::mem::size_of::<WDF_IO_QUEUE_CONFIG>() as ULONG,
        DispatchType: wdk_sys::_WDF_IO_QUEUE_DISPATCH_TYPE::WdfIoQueueDispatchSequential,
        PowerManaged: wdk_sys::_WDF_TRI_STATE::WdfFalse,
        DefaultQueue: wdk_sys::TRUE as u8,
        EvtIoDeviceControl: Some(evt_io_device_control),
        ..Default::default()
    };
    let mut queue: WDFQUEUE = core::ptr::null_mut();
    // SAFETY: `device` and `queue_cfg` are live initialized locals; `queue` receives the handle.
    let queue_st = unsafe {
        call_unsafe_wdf_function_binding!(
            WdfIoQueueCreate,
            device,
            &mut queue_cfg,
            WDF_NO_OBJECT_ATTRIBUTES,
            &mut queue
        )
    };
    dbglog!("EvtDriverDeviceAdd complete -> {queue_st:#x}");
    queue_st
}

unsafe extern "C" fn evt_device_prepare_hardware(
    device: WDFDEVICE,
    _resources_raw: WDFCMRESLIST,
    _resources_translated: WDFCMRESLIST,
) -> NTSTATUS {
    if let Some(ctx) = lookup_device(device) {
        ctx.on_prepare_hardware();
    }
    STATUS_SUCCESS
}

unsafe extern "C" fn evt_device_d0_entry(
    device: WDFDEVICE,
    _previous_state: WDF_POWER_DEVICE_STATE,
) -> NTSTATUS {
    if let Some(ctx) = lookup_device(device) {
        ctx.on_d0_entry();
    }
    STATUS_SUCCESS
}

unsafe extern "C" fn evt_device_d0_exit(
    device: WDFDEVICE,
    _target_state: WDF_POWER_DEVICE_STATE,
) -> NTSTATUS {
    if let Some(ctx) = lookup_device(device) {
        ctx.on_d0_exit();
    }
    STATUS_SUCCESS
}

unsafe extern "C" fn evt_device_release_hardware(
    device: WDFDEVICE,
    _resources: WDFCMRESLIST,
) -> NTSTATUS {
    if let Some(ctx) = lookup_device(device) {
        ctx.on_release_or_cleanup("ReleaseHardware", false);
    }
    STATUS_SUCCESS
}

unsafe extern "C" fn evt_vhf_target_cleanup(object: WDFOBJECT) {
    let target: WDFIOTARGET = object.cast();
    // SAFETY: `target` is valid during its `EvtCleanupCallback` and parented to the device.
    let device: WDFDEVICE =
        unsafe { call_unsafe_wdf_function_binding!(WdfIoTargetGetDevice, target) };
    if let Some(ctx) = lookup_device(device) {
        ctx.on_release_or_cleanup("TargetCleanup", true);
    }
}

unsafe extern "C" fn evt_device_cleanup(object: WDFOBJECT) {
    let device: WDFDEVICE = object.cast();
    if let Some(ctx) = unregister_device(device) {
        ctx.on_release_or_cleanup("Cleanup", true);
    }
}

unsafe extern "C" fn evt_file_create(
    _device: WDFDEVICE,
    request: WDFREQUEST,
    _file_object: WDFFILEOBJECT,
) {
    // SAFETY: `request` is the live framework create request.
    unsafe { call_unsafe_wdf_function_binding!(WdfRequestComplete, request, STATUS_SUCCESS) };
}

unsafe extern "C" fn evt_file_close(file_object: WDFFILEOBJECT) {
    if file_object.is_null() {
        return;
    }
    // SAFETY: `file_object` is valid during `EvtFileClose`.
    let device = unsafe { call_unsafe_wdf_function_binding!(WdfFileObjectGetDevice, file_object) };
    if let Some(ctx) = lookup_device(device) {
        ctx.on_file_cleanup(file_object as usize);
    }
}

unsafe extern "C" fn evt_io_device_control(
    queue: WDFQUEUE,
    request: WDFREQUEST,
    _output_buffer_length: usize,
    _input_buffer_length: usize,
    io_control_code: ULONG,
) {
    // SAFETY: `queue` is live inside `EvtIoDeviceControl`.
    let device = unsafe { call_unsafe_wdf_function_binding!(WdfIoQueueGetDevice, queue) };
    // SAFETY: `request` is the live, incomplete request passed to this callback.
    let req = unsafe { Request::new(request) };
    let Some(ctx) = lookup_device(device) else {
        req.complete(crate::STATUS_UNSUCCESSFUL);
        return;
    };
    ctx.dispatch_ioctl(req, io_control_code);
}
