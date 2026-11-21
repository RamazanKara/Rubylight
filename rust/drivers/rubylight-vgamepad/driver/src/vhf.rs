// Copyright (c) 2026 Chase Payne
// SPDX-License-Identifier: MIT
//
// Ported from Nonary/libvirtualgamepad driver/src/driver.cpp.
// Adapted for Rubylight by RamazanKara (https://github.com/RamazanKara/Rubylight).

//! Safe-boundary VHF (Virtual HID Framework) bindings and pinned per-slot report buffers.

use core::ffi::c_void;
use std::sync::{Arc, Weak};

use rubylight_vgamepad_proto::guid_fields;
use rubylight_vgamepad_proto::lifecycle::CreatePlan;
use rubylight_vgamepad_proto::report_pump::MAX_REPORT_BYTES;
use wdk_sys::{
    GUID, HANDLE, HID_XFER_PACKET, NTSTATUS, ULONG, USHORT, VHF_CONFIG, VHFHANDLE,
    VHFOPERATIONHANDLE, hid,
};

use crate::device::DeviceContext;
use crate::{STATUS_SUCCESS, STATUS_UNSUCCESSFUL, nt_success};

/// Heap-pinned transfer scratch buffer for a single controller slot's in-flight `VhfReadReportSubmit`.
pub struct SlotTransfer {
    packet: HID_XFER_PACKET,
    buffer: [u8; MAX_REPORT_BYTES],
}

// SAFETY: Accessed strictly one submit at a time (gated by `ReportPump.ready == false` and per-slot mutex during preparation).
unsafe impl Send for SlotTransfer {}

impl SlotTransfer {
    pub fn new() -> Box<Self> {
        Box::new(Self {
            packet: HID_XFER_PACKET::default(),
            buffer: [0; MAX_REPORT_BYTES],
        })
    }

    /// Populate the pinned packet and report buffer and return its stable packet pointer.
    pub fn prepare(&mut self, bytes: &[u8], len: usize, report_id: u8) -> *mut HID_XFER_PACKET {
        let copy_len = len.min(MAX_REPORT_BYTES).min(bytes.len());
        self.buffer[..copy_len].copy_from_slice(&bytes[..copy_len]);
        self.packet.reportBuffer = self.buffer.as_mut_ptr();
        self.packet.reportBufferLen = copy_len as ULONG;
        self.packet.reportId = report_id;
        &raw mut self.packet
    }
}

pub struct VhfSlotContext {
    pub controller_id: u32,
    pub device: Weak<DeviceContext>,
}

fn to_wdk_guid(raw: u128) -> GUID {
    let (data1, data2, data3, data4) = guid_fields(raw);
    GUID {
        Data1: data1,
        Data2: data2,
        Data3: data3,
        Data4: data4,
    }
}

/// Create and start a VHF virtual HID child device for `plan`.
///
/// Always called inside `lifetime_gate` and **outside** `state_lock`. Returns
/// `(vhf, VhfStart status)` once `VhfCreate` succeeds so the caller can commit or
/// drain-and-delete under `state_lock`.
pub fn create_and_start(
    file_handle: HANDLE,
    dev_ctx: &Arc<DeviceContext>,
    plan: &CreatePlan,
) -> Result<(VHFHANDLE, NTSTATUS), NTSTATUS> {
    let slot_ctx = Box::into_raw(Box::new(VhfSlotContext {
        controller_id: plan.controller_id,
        device: Arc::downgrade(dev_ctx),
    }));

    let mut cfg = VHF_CONFIG {
        Size: core::mem::size_of::<VHF_CONFIG>() as ULONG,
        VhfClientContext: slot_ctx.cast(),
        OperationContextSize: 0,
        FileHandle: file_handle,
        VendorID: plan.profile.vendor_id,
        ProductID: plan.profile.product_id,
        VersionNumber: plan.profile.version_number,
        ContainerID: to_wdk_guid(plan.container_id),
        InstanceIDLength: (plan.instance_id_utf16_len * 2) as USHORT,
        InstanceID: plan.instance_id_utf16.as_ptr().cast_mut(),
        ReportDescriptorLength: plan.profile.report_descriptor.len() as USHORT,
        ReportDescriptor: plan.profile.report_descriptor.as_ptr().cast_mut(),
        EvtVhfReadyForNextReadReport: Some(evt_vhf_ready_for_next_read_report),
        EvtVhfAsyncOperationGetFeature: plan
            .register_feature_callbacks
            .then_some(evt_vhf_get_feature),
        EvtVhfAsyncOperationSetFeature: plan
            .register_feature_callbacks
            .then_some(evt_vhf_set_feature),
        EvtVhfAsyncOperationWriteReport: Some(evt_vhf_write_report),
        EvtVhfAsyncOperationGetInputReport: Some(evt_vhf_get_input_report),
        EvtVhfCleanup: Some(evt_vhf_cleanup),
        HardwareIDsLength: (plan.profile.hardware_ids_utf16.len() * 2) as USHORT,
        HardwareIDs: plan.profile.hardware_ids_utf16.as_ptr().cast_mut(),
    };

    let mut vhf: VHFHANDLE = core::ptr::null_mut();
    // SAFETY: `cfg` points to static profile descriptors/IDs and `slot_ctx`; `vhf` receives the created handle.
    let create_st = unsafe { hid::VhfCreate(&mut cfg, &mut vhf) };
    if !nt_success(create_st) || vhf.is_null() {
        // `VhfCreate` failed before VHF adopted `slot_ctx`; free it now.
        // SAFETY: `slot_ctx` was allocated above with `Box::into_raw` and VHF did not accept it.
        unsafe { drop(Box::from_raw(slot_ctx)) };
        return Err(if nt_success(create_st) {
            STATUS_UNSUCCESSFUL
        } else {
            create_st
        });
    }

    // Publish handle BEFORE `VhfStart` so synchronous callbacks arriving during start are admitted.
    dev_ctx.publish_vhf(plan.controller_id, vhf);

    // SAFETY: `vhf` was successfully created by `VhfCreate` above.
    let start_st = unsafe { hid::VhfStart(vhf) };
    Ok((vhf, start_st))
}

/// Synchronously delete a VHF handle. Must be called outside `state_lock` after `submits_in_flight == 0`.
pub fn delete_sync(vhf: VHFHANDLE) {
    // SAFETY: Caller detached `vhf` from active use, drained in-flight submits, and holds no lock that `EvtVhfCleanup` takes.
    unsafe { hid::VhfDelete(vhf, wdk_sys::TRUE as u8) };
}

/// Submit a prepared report packet to VHF. Must be called outside `state_lock`.
///
/// # Safety
/// `packet` must point to the slot's pinned `SlotTransfer.packet` with valid buffer bytes for the
/// duration of this call until VHF fires `EvtVhfReadyForNextReadReport`.
pub unsafe fn submit_read_report(vhf: VHFHANDLE, packet: *mut HID_XFER_PACKET) -> NTSTATUS {
    // SAFETY: Caller guarantees `vhf` is live (`submits_in_flight > 0`) and `packet` is valid.
    unsafe { hid::VhfReadReportSubmit(vhf, packet) }
}

fn complete_async_op(op: VHFOPERATIONHANDLE, status: NTSTATUS) {
    if !op.is_null() {
        // SAFETY: `op` is the live non-null async operation handle from the current VHF callback.
        let _ = unsafe { hid::VhfAsyncOperationComplete(op, status) };
    }
}

fn borrow_slot_ctx<'a>(ctx: *mut c_void) -> Option<&'a VhfSlotContext> {
    if ctx.is_null() {
        return None;
    }
    // SAFETY: `ctx` is a live `Box<VhfSlotContext>` owned by VHF until `evt_vhf_cleanup`.
    Some(unsafe { &*ctx.cast::<VhfSlotContext>() })
}

unsafe extern "C" fn evt_vhf_ready_for_next_read_report(vhf_client_context: *mut c_void) {
    let Some(slot_ctx) = borrow_slot_ctx(vhf_client_context) else {
        return;
    };
    if let Some(dev) = slot_ctx.device.upgrade() {
        dev.handle_vhf_ready(slot_ctx.controller_id);
    }
}

unsafe extern "C" fn evt_vhf_get_input_report(
    vhf_client_context: *mut c_void,
    vhf_operation_handle: VHFOPERATIONHANDLE,
    _vhf_operation_context: *mut c_void,
    hid_transfer_packet: *mut HID_XFER_PACKET,
) {
    let Some(slot_ctx) = borrow_slot_ctx(vhf_client_context) else {
        complete_async_op(vhf_operation_handle, STATUS_INVALID_DEVICE_REQUEST);
        return;
    };
    let Some(dev) = slot_ctx.device.upgrade() else {
        complete_async_op(vhf_operation_handle, STATUS_INVALID_DEVICE_REQUEST);
        return;
    };
    if hid_transfer_packet.is_null() {
        complete_async_op(vhf_operation_handle, crate::STATUS_INVALID_PARAMETER);
        return;
    }
    // SAFETY: VHF provides a valid mutable `HID_XFER_PACKET` for the duration of the sync completion.
    let pkt = unsafe { &mut *hid_transfer_packet };
    let buf_len = pkt.reportBufferLen as usize;
    if pkt.reportBuffer.is_null() || buf_len == 0 {
        complete_async_op(vhf_operation_handle, STATUS_INVALID_DEVICE_REQUEST);
        return;
    }
    // SAFETY: `pkt.reportBuffer` is writable for `buf_len` bytes per VHF's transfer packet contract.
    let out = unsafe { core::slice::from_raw_parts_mut(pkt.reportBuffer, buf_len) };
    let st = match dev.handle_vhf_get_input_report(slot_ctx.controller_id, pkt.reportId, out) {
        Ok(_written) => STATUS_SUCCESS,
        Err(status) => status,
    };
    complete_async_op(vhf_operation_handle, st);
}

unsafe extern "C" fn evt_vhf_get_feature(
    vhf_client_context: *mut c_void,
    vhf_operation_handle: VHFOPERATIONHANDLE,
    _vhf_operation_context: *mut c_void,
    hid_transfer_packet: *mut HID_XFER_PACKET,
) {
    let Some(slot_ctx) = borrow_slot_ctx(vhf_client_context) else {
        complete_async_op(vhf_operation_handle, STATUS_INVALID_DEVICE_REQUEST);
        return;
    };
    let Some(dev) = slot_ctx.device.upgrade() else {
        complete_async_op(vhf_operation_handle, STATUS_INVALID_DEVICE_REQUEST);
        return;
    };
    if hid_transfer_packet.is_null() {
        complete_async_op(vhf_operation_handle, crate::STATUS_INVALID_PARAMETER);
        return;
    }
    // SAFETY: VHF provides a valid mutable `HID_XFER_PACKET` for this operation.
    let pkt = unsafe { &mut *hid_transfer_packet };
    let buf_len = pkt.reportBufferLen as usize;
    if pkt.reportBuffer.is_null() || buf_len == 0 {
        complete_async_op(vhf_operation_handle, STATUS_INVALID_DEVICE_REQUEST);
        return;
    }
    // SAFETY: `pkt.reportBuffer` is writable for `buf_len` bytes.
    let out = unsafe { core::slice::from_raw_parts_mut(pkt.reportBuffer, buf_len) };
    let st = match dev.handle_vhf_get_feature(slot_ctx.controller_id, pkt.reportId, out) {
        Ok(_written) => STATUS_SUCCESS,
        Err(status) => status,
    };
    complete_async_op(vhf_operation_handle, st);
}

unsafe extern "C" fn evt_vhf_set_feature(
    vhf_client_context: *mut c_void,
    vhf_operation_handle: VHFOPERATIONHANDLE,
    _vhf_operation_context: *mut c_void,
    hid_transfer_packet: *mut HID_XFER_PACKET,
) {
    let Some(slot_ctx) = borrow_slot_ctx(vhf_client_context) else {
        complete_async_op(vhf_operation_handle, STATUS_INVALID_DEVICE_REQUEST);
        return;
    };
    let Some(dev) = slot_ctx.device.upgrade() else {
        complete_async_op(vhf_operation_handle, STATUS_INVALID_DEVICE_REQUEST);
        return;
    };
    if hid_transfer_packet.is_null() {
        complete_async_op(vhf_operation_handle, crate::STATUS_INVALID_PARAMETER);
        return;
    }
    // SAFETY: VHF provides a valid `HID_XFER_PACKET` for this operation.
    let pkt = unsafe { &*hid_transfer_packet };
    let data = if pkt.reportBuffer.is_null() || pkt.reportBufferLen == 0 {
        &[][..]
    } else {
        // SAFETY: `pkt.reportBuffer` is readable for `pkt.reportBufferLen` bytes.
        unsafe { core::slice::from_raw_parts(pkt.reportBuffer, pkt.reportBufferLen as usize) }
    };
    let st = dev.handle_vhf_set_feature(slot_ctx.controller_id, pkt.reportId, data);
    complete_async_op(vhf_operation_handle, st);
}

unsafe extern "C" fn evt_vhf_write_report(
    vhf_client_context: *mut c_void,
    vhf_operation_handle: VHFOPERATIONHANDLE,
    _vhf_operation_context: *mut c_void,
    hid_transfer_packet: *mut HID_XFER_PACKET,
) {
    let Some(slot_ctx) = borrow_slot_ctx(vhf_client_context) else {
        complete_async_op(vhf_operation_handle, STATUS_INVALID_DEVICE_REQUEST);
        return;
    };
    let Some(dev) = slot_ctx.device.upgrade() else {
        complete_async_op(vhf_operation_handle, STATUS_INVALID_DEVICE_REQUEST);
        return;
    };
    if hid_transfer_packet.is_null() {
        complete_async_op(vhf_operation_handle, crate::STATUS_INVALID_PARAMETER);
        return;
    }
    // SAFETY: VHF provides a valid `HID_XFER_PACKET` for this operation.
    let pkt = unsafe { &*hid_transfer_packet };
    let data = if pkt.reportBuffer.is_null() || pkt.reportBufferLen == 0 {
        &[][..]
    } else {
        // SAFETY: `pkt.reportBuffer` is readable for `pkt.reportBufferLen` bytes.
        unsafe { core::slice::from_raw_parts(pkt.reportBuffer, pkt.reportBufferLen as usize) }
    };
    let (st, follow_up) = dev.handle_vhf_write_report(slot_ctx.controller_id, pkt.reportId, data);
    complete_async_op(vhf_operation_handle, st);
    if let Some((pending, pkt_ptr)) = follow_up {
        dev.finish_submit(pending, pkt_ptr);
    }
}

unsafe extern "C" fn evt_vhf_cleanup(vhf_client_context: *mut c_void) {
    if vhf_client_context.is_null() {
        return;
    }
    // SAFETY: `vhf_client_context` was allocated with `Box::into_raw` in `create_and_start` and VHF invokes `EvtVhfCleanup` once.
    unsafe { drop(Box::from_raw(vhf_client_context.cast::<VhfSlotContext>())) };
}

use crate::STATUS_INVALID_DEVICE_REQUEST;
