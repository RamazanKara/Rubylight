// Copyright (c) 2026 Chase Payne
// SPDX-License-Identifier: MIT
//
// Ported from Nonary/libvirtualgamepad driver/src/driver.cpp.
// Adapted for Rubylight by RamazanKara (https://github.com/RamazanKara/Rubylight).

//! Device state coordinator, Configuration Manager child collision check, and IOCTL router.

use std::sync::{Arc, Condvar, Mutex, RwLock};

use rubylight_vgamepad_proto::lifecycle::{
    DeviceState, DriverStatus, OwnerToken, PendingSubmit, matches_cm_child_instance_list,
};
use rubylight_vgamepad_proto::protocol::{
    self, BatteryStateRequest, ControllerIdRequest, CreateControllerRequest, FeedbackEvent,
    InputStateRequest, MAX_CONTROLLERS, MotionStateRequest, QueryInfoRequest, QueryInfoResponse,
    SteamControllerAttributesRequest, TouchStateRequest,
};
use wdk_sys::{
    HANDLE, HID_XFER_PACKET, NTSTATUS, ULONG, VHFHANDLE, WDF_IO_TARGET_OPEN_PARAMS, WDFDEVICE,
    WDFIOTARGET, call_unsafe_wdf_function_binding,
};
use windows::Win32::Devices::DeviceAndDriverInstallation::{
    CM_GETIDLIST_FILTER_ENUMERATOR, CM_GETIDLIST_FILTER_PRESENT, CM_Get_Device_ID_List_SizeW,
    CM_Get_Device_ID_ListW, CR_BUFFER_SMALL, CR_SUCCESS,
};
use windows::core::{PCWSTR, w};

use crate::keepalive::{PidTimer, Sc26Keepalive, now_us};
use crate::vhf::{self, SlotTransfer};
use crate::wdf::Request;
use crate::{
    STATUS_DEVICE_NOT_READY, STATUS_INVALID_DEVICE_REQUEST, STATUS_INVALID_HANDLE, STATUS_SUCCESS,
    nt_success,
};

static DEVICES: Mutex<Vec<(usize, Arc<DeviceContext>)>> = Mutex::new(Vec::new());

pub fn register_device(device: WDFDEVICE, ctx: Arc<DeviceContext>) {
    let mut list = DEVICES.lock().unwrap();
    list.retain(|(d, _)| *d != device as usize);
    list.push((device as usize, ctx));
}

pub fn lookup_device(device: WDFDEVICE) -> Option<Arc<DeviceContext>> {
    if device.is_null() {
        return None;
    }
    let list = DEVICES.lock().unwrap();
    list.iter()
        .find(|(d, _)| *d == device as usize)
        .map(|(_, ctx)| Arc::clone(ctx))
}

pub fn unregister_device(device: WDFDEVICE) -> Option<Arc<DeviceContext>> {
    let mut list = DEVICES.lock().unwrap();
    let pos = list.iter().position(|(d, _)| *d == device as usize)?;
    Some(list.swap_remove(pos).1)
}

#[derive(Default)]
struct VhfTargetState {
    target: usize,
    file_handle: usize,
    open: bool,
}

pub struct DeviceContext {
    /// Outer lifecycle lock: read by fast submit IOCTLs, write by create/destroy/power/stop/cleanup.
    /// Never acquired from `EvtVhf*` callbacks (which `VhfDelete` waits on).
    lifetime_gate: RwLock<()>,
    state_lock: Mutex<DeviceState>,
    vhf_target: Mutex<VhfTargetState>,
    submits_cv: Condvar,
    transfer_slots: [Mutex<Box<SlotTransfer>>; MAX_CONTROLLERS as usize],
    pub pid_timer: PidTimer,
    pub sc26_keepalive: Sc26Keepalive,
}

impl DeviceContext {
    pub fn new(local_vhf_target: WDFIOTARGET) -> Arc<Self> {
        Arc::new(Self {
            lifetime_gate: RwLock::new(()),
            state_lock: Mutex::new(DeviceState::new()),
            vhf_target: Mutex::new(VhfTargetState {
                target: local_vhf_target as usize,
                file_handle: 0,
                open: false,
            }),
            submits_cv: Condvar::new(),
            transfer_slots: core::array::from_fn(|_| Mutex::new(SlotTransfer::new())),
            pid_timer: PidTimer::new(),
            sc26_keepalive: Sc26Keepalive::new(),
        })
    }

    /// Open the dedicated local WDF I/O target by file handle if not already open.
    /// Callers must hold `lifetime_gate.write()`.
    pub fn ensure_vhf_target_open(&self) -> Result<HANDLE, NTSTATUS> {
        let (target, existing) = {
            let guard = self.vhf_target.lock().unwrap();
            if guard.file_handle != 0 {
                return Ok(guard.file_handle as HANDLE);
            }
            (guard.target as WDFIOTARGET, guard.open)
        };
        if existing || target.is_null() || self.state_lock.lock().unwrap().stopping {
            return Err(STATUS_DEVICE_NOT_READY);
        }

        let mut open_params = WDF_IO_TARGET_OPEN_PARAMS {
            Size: core::mem::size_of::<WDF_IO_TARGET_OPEN_PARAMS>() as ULONG,
            Type: wdk_sys::_WDF_IO_TARGET_OPEN_TYPE::WdfIoTargetOpenLocalTargetByFile,
            ..Default::default()
        };
        // SAFETY: `target` is this device's dedicated local I/O target; `open_params` is initialized on stack.
        let status =
            unsafe { call_unsafe_wdf_function_binding!(WdfIoTargetOpen, target, &mut open_params) };
        if !nt_success(status) {
            return Err(status);
        }

        // SAFETY: `target` was just opened synchronously via `WdfIoTargetOpen`.
        let file_handle: HANDLE =
            unsafe { call_unsafe_wdf_function_binding!(WdfIoTargetWdmGetTargetFileHandle, target) };
        if file_handle.is_null() {
            // SAFETY: Closing target because WDM file handle could not be retrieved.
            unsafe { call_unsafe_wdf_function_binding!(WdfIoTargetClose, target) };
            return Err(STATUS_DEVICE_NOT_READY);
        }

        let mut guard = self.vhf_target.lock().unwrap();
        guard.file_handle = file_handle as usize;
        guard.open = true;
        Ok(file_handle)
    }

    pub fn on_prepare_hardware(&self) {
        let _gate = self.lifetime_gate.write().unwrap();
        {
            let mut state = self.state_lock.lock().unwrap();
            state.stopping = false;
        }
        // Best effort on start; IOCTL_CREATE_CONTROLLER retries opening if VHF wasn't ready yet.
        let _ = self.ensure_vhf_target_open();
    }

    /// Copy the just-taken `pump.in_flight` bytes into the slot's pinned `SlotTransfer` while still holding `state_lock`.
    fn prepare_transfer_locked(
        &self,
        state: &DeviceState,
        pending: &PendingSubmit,
    ) -> *mut HID_XFER_PACKET {
        let slot = &state.slots[pending.controller_id as usize];
        let mut xfer = self.transfer_slots[pending.controller_id as usize]
            .lock()
            .unwrap();
        xfer.prepare(
            &slot.pump.in_flight.data,
            pending.length as usize,
            pending.report_id,
        )
    }

    /// Prepare transfer bytes and an optional SC26 keepalive re-arm deadline while holding `state_lock`.
    fn prepare_submit_locked(
        &self,
        state: &DeviceState,
        pending: Option<PendingSubmit>,
        now: u64,
        force_rearm: bool,
    ) -> (Option<(PendingSubmit, *mut HID_XFER_PACKET)>, Option<u64>) {
        let prepared = pending.map(|p| {
            let pkt = self.prepare_transfer_locked(state, &p);
            (p, pkt)
        });
        let should_rearm = state.sc26_timer_running
            && (force_rearm || prepared.as_ref().is_some_and(|(p, _)| p.rearm_keepalive));
        let arm_delay = should_rearm.then(|| state.sc26_next_deadline_us(now));
        (prepared, arm_delay)
    }

    pub fn any_pid_playing(&self) -> bool {
        self.state_lock.lock().unwrap().any_pid_playing()
    }

    pub fn clear_sc26_timer_running(&self) {
        let mut state = self.state_lock.lock().unwrap();
        state.sc26_timer_running = false;
        state.sc26_timer_parked = false;
    }

    /// Submit `pkt_ptr` to VHF outside `state_lock`, then release `submits_in_flight` and notify waiters.
    pub fn finish_submit(&self, pending: PendingSubmit, pkt_ptr: *mut HID_XFER_PACKET) {
        // SAFETY: `pending.vhf` stays live until `submits_in_flight` drops to zero; `pkt_ptr` stays valid while ready == false.
        let _ = unsafe { vhf::submit_read_report(pending.vhf as VHFHANDLE, pkt_ptr) };
        let mut state = self.state_lock.lock().unwrap();
        state.submit_finish(pending.controller_id);
        if state.slots[pending.controller_id as usize].submits_in_flight == 0 {
            self.submits_cv.notify_all();
        }
    }

    fn finish_and_arm(
        &self,
        prepared: Option<(PendingSubmit, *mut HID_XFER_PACKET)>,
        arm_delay: Option<u64>,
    ) {
        if let Some((pending, pkt)) = prepared {
            self.finish_submit(pending, pkt);
        }
        if let Some(delay_us) = arm_delay {
            self.sc26_keepalive.arm(delay_us);
        }
    }

    pub fn publish_vhf(&self, controller_id: u32, vhf: VHFHANDLE) {
        let mut state = self.state_lock.lock().unwrap();
        state.publish_vhf(controller_id, vhf as usize);
    }

    fn drain_and_delete_vhf(&self, controller_id: u32, vhf: VHFHANDLE) {
        {
            let mut state = self.state_lock.lock().unwrap();
            while state.slots[controller_id as usize].submits_in_flight > 0 {
                state = self.submits_cv.wait(state).unwrap();
            }
        }
        vhf::delete_sync(vhf);
    }

    pub fn on_d0_entry(&self) {
        let _gate = self.lifetime_gate.write().unwrap();
        let mut state = self.state_lock.lock().unwrap();
        state.stopping = false;
        state.powered = true;
        dbglog!("device entered D0");
    }

    pub fn on_d0_exit(&self) {
        dbglog!("device exiting D0");
        {
            let mut state = self.state_lock.lock().unwrap();
            state.powered = false;
        }
        let _gate = self.lifetime_gate.write().unwrap();
        self.pid_timer.stop(true);
        self.sc26_keepalive.stop_and_join();

        let retired = {
            let mut state = self.state_lock.lock().unwrap();
            state.begin_power_down()
        };
        for (i, token) in retired.iter().enumerate() {
            if let Some(vhf) = *token {
                self.drain_and_delete_vhf(i as u32, vhf as VHFHANDLE);
            }
        }
        let mut state = self.state_lock.lock().unwrap();
        state.finish_power_down(&retired);
    }

    pub fn on_release_or_cleanup(&self, label: &str, forget_target: bool) {
        dbglog!("device {label}");
        {
            let mut state = self.state_lock.lock().unwrap();
            state.stopping = true;
            state.powered = false;
        }
        let _gate = self.lifetime_gate.write().unwrap();
        self.pid_timer.stop(true);
        self.sc26_keepalive.stop_and_join();

        let handles = {
            let mut state = self.state_lock.lock().unwrap();
            state.begin_stop_all()
        };
        for (i, token) in handles.iter().enumerate() {
            if let Some(vhf) = *token {
                self.drain_and_delete_vhf(i as u32, vhf as VHFHANDLE);
                let mut state = self.state_lock.lock().unwrap();
                state.finish_stop_slot(i as u32);
            }
        }
        let close_target = {
            let mut guard = self.vhf_target.lock().unwrap();
            guard.file_handle = 0;
            let to_close = if !forget_target && guard.open {
                guard.target as WDFIOTARGET
            } else {
                core::ptr::null_mut()
            };
            guard.open = false;
            if forget_target {
                guard.target = 0;
            }
            to_close
        };
        if !close_target.is_null() {
            // SAFETY: All VHF child handles were synchronously deleted before closing the target.
            unsafe { call_unsafe_wdf_function_binding!(WdfIoTargetClose, close_target) };
        }
    }

    pub fn on_file_cleanup(&self, owner: OwnerToken) {
        if owner == 0 {
            return;
        }
        let _gate = self.lifetime_gate.write().unwrap();
        let mut tokens = [None; MAX_CONTROLLERS as usize];
        {
            let mut state = self.state_lock.lock().unwrap();
            for (i, slot) in tokens.iter_mut().enumerate() {
                *slot = state.begin_destroy_owned(owner, i as u32);
            }
        }
        for (i, token) in tokens.iter().enumerate() {
            if let Some(vhf) = *token {
                self.drain_and_delete_vhf(i as u32, vhf as VHFHANDLE);
                let mut state = self.state_lock.lock().unwrap();
                state.finish_destroy_owned(i as u32, owner);
            }
        }
    }

    pub fn handle_vhf_ready(&self, controller_id: u32) {
        let now = now_us();
        let (prepared, arm_delay) = {
            let mut state = self.state_lock.lock().unwrap();
            let (pending, unpark) = state.on_vhf_ready_for_next_report(controller_id, now);
            self.prepare_submit_locked(&state, pending, now, unpark)
        };
        self.finish_and_arm(prepared, arm_delay);
    }

    pub fn handle_vhf_get_input_report(
        &self,
        controller_id: u32,
        report_id: u8,
        out: &mut [u8],
    ) -> Result<usize, NTSTATUS> {
        let now = now_us();
        let mut state = self.state_lock.lock().unwrap();
        state
            .on_get_input_report(controller_id, report_id, out, now)
            .map_err(DriverStatus::ntstatus)
    }

    pub fn handle_vhf_get_feature(
        &self,
        controller_id: u32,
        report_id: u8,
        out: &mut [u8],
    ) -> Result<usize, NTSTATUS> {
        let state = self.state_lock.lock().unwrap();
        state
            .on_get_feature(controller_id, report_id, out)
            .map_err(DriverStatus::ntstatus)
    }

    pub fn handle_vhf_set_feature(
        &self,
        controller_id: u32,
        report_id: u8,
        data: &[u8],
    ) -> NTSTATUS {
        let mut state = self.state_lock.lock().unwrap();
        state
            .on_set_feature(controller_id, report_id, data)
            .ntstatus()
    }

    pub fn handle_vhf_write_report(
        &self,
        controller_id: u32,
        report_id: u8,
        data: &[u8],
    ) -> (NTSTATUS, Option<(PendingSubmit, *mut HID_XFER_PACKET)>) {
        let now = now_us();
        let (status, prepared, arm_delay, arm_pid) = {
            let mut state = self.state_lock.lock().unwrap();
            let (ds, pending, arm_pid) = state.on_write_report(controller_id, report_id, data, now);
            let (prepared, arm_delay) = self.prepare_submit_locked(&state, pending, now, false);
            (ds.ntstatus(), prepared, arm_delay, arm_pid)
        };
        if arm_pid {
            self.pid_timer.start();
        }
        if let Some(delay_us) = arm_delay {
            self.sc26_keepalive.arm(delay_us);
        }
        (status, prepared)
    }

    pub fn on_pid_tick(&self) -> bool {
        let mut state = self.state_lock.lock().unwrap();
        state.pid_tick()
    }

    pub fn on_sc26_keepalive_tick(&self) -> bool {
        let now = now_us();
        let (items, count, any_active, arm_delay) = {
            let mut state = self.state_lock.lock().unwrap();
            let (pending, count, any_active, any_ready) = state.sc26_keepalive_collect(now);
            let mut prepared: [Option<(PendingSubmit, *mut HID_XFER_PACKET)>;
                MAX_CONTROLLERS as usize] = [None; MAX_CONTROLLERS as usize];
            for (dst, src) in prepared[..count].iter_mut().zip(&pending[..count]) {
                if let Some(p) = src {
                    let pkt = self.prepare_transfer_locked(&state, p);
                    *dst = Some((*p, pkt));
                }
            }
            let arm_delay = (any_active && any_ready).then(|| state.sc26_next_deadline_us(now));
            (prepared, count, any_active, arm_delay)
        };
        for item in &items[..count] {
            if let Some((p, pkt)) = *item {
                self.finish_submit(p, pkt);
            }
        }
        if let Some(delay_us) = arm_delay {
            self.sc26_keepalive.arm(delay_us);
        }
        any_active
    }

    pub fn dispatch_ioctl(self: &Arc<Self>, request: Request, ioctl_code: u32) {
        let owner = request.file_object_key();
        if owner == 0 {
            request.complete(STATUS_INVALID_HANDLE);
            return;
        }
        match ioctl_code {
            protocol::IOCTL_QUERY_INFO => self.ioctl_query_info(request, owner),
            protocol::IOCTL_CREATE_CONTROLLER => self.ioctl_create_controller(request, owner),
            protocol::IOCTL_DESTROY_CONTROLLER => self.ioctl_destroy_controller(request, owner),
            protocol::IOCTL_SUBMIT_INPUT_STATE => self.ioctl_submit_input(request, owner),
            protocol::IOCTL_POLL_FEEDBACK => self.ioctl_poll_feedback(request, owner),
            protocol::IOCTL_SUBMIT_TOUCH_STATE => self.ioctl_submit_touch(request, owner),
            protocol::IOCTL_SUBMIT_MOTION_STATE => self.ioctl_submit_motion(request, owner),
            protocol::IOCTL_SUBMIT_BATTERY_STATE => self.ioctl_submit_battery(request, owner),
            protocol::IOCTL_SET_STEAM_CONTROLLER_ATTRIBUTES => {
                self.ioctl_set_sc_attributes(request, owner)
            }
            _ => request.complete(STATUS_INVALID_DEVICE_REQUEST),
        }
    }

    fn ioctl_query_info(&self, request: Request, owner: OwnerToken) {
        if let Err(st) = request.check_output_len(QueryInfoResponse::WIRE_SIZE) {
            request.complete(st);
            return;
        }
        let (bytes, len) = match request.input_bytes(ControllerIdRequest::WIRE_SIZE) {
            Ok(v) => v,
            Err(st) => {
                request.complete(st);
                return;
            }
        };
        let resp = if len == QueryInfoRequest::WIRE_SIZE {
            let Some(_req) = QueryInfoRequest::from_bytes(&bytes) else {
                request.complete(DriverStatus::InvalidBufferSize.ntstatus());
                return;
            };
            let state = self.state_lock.lock().unwrap();
            Ok(state.fill_query_info())
        } else if len == ControllerIdRequest::WIRE_SIZE {
            let Some(req) = ControllerIdRequest::from_bytes(&bytes) else {
                request.complete(DriverStatus::InvalidBufferSize.ntstatus());
                return;
            };
            let state = self.state_lock.lock().unwrap();
            state.query_controller_info(owner, &req)
        } else {
            request.complete(DriverStatus::InvalidBufferSize.ntstatus());
            return;
        };

        match resp {
            Ok(info) => match request.copy_to_output(&info.to_bytes()) {
                Ok(written) => request.complete_with_info(STATUS_SUCCESS, written),
                Err(st) => request.complete(st),
            },
            Err(ds) => request.complete(ds.ntstatus()),
        }
    }

    fn ioctl_create_controller(self: &Arc<Self>, request: Request, owner: OwnerToken) {
        let req = match read_exact(
            &request,
            CreateControllerRequest::WIRE_SIZE,
            CreateControllerRequest::from_bytes,
        ) {
            Ok(r) => r,
            Err(st) => {
                request.complete(st);
                return;
            }
        };

        let expected_generation = {
            let state = self.state_lock.lock().unwrap();
            match state.check_create_preflight(owner, &req) {
                Ok((_, gen_num)) => gen_num,
                Err(ds) => {
                    request.complete(ds.ntstatus());
                    return;
                }
            }
        };

        // Run Configuration Manager device enumeration outside `lifetime_gate` and `state_lock`.
        let child_present = cm_child_exists_for_slot(req.controller_id);

        let _gate = self.lifetime_gate.write().unwrap();
        let file_handle = match self.ensure_vhf_target_open() {
            Ok(h) => h,
            Err(st) => {
                request.complete(st);
                return;
            }
        };
        let plan = {
            let mut state = self.state_lock.lock().unwrap();
            match state.begin_create(owner, &req, expected_generation, child_present) {
                Ok(p) => p,
                Err(ds) => {
                    request.complete(ds.ntstatus());
                    return;
                }
            }
        };

        match vhf::create_and_start(file_handle, self, &plan) {
            Ok((vhf, start_st)) => {
                let committed = {
                    let mut state = self.state_lock.lock().unwrap();
                    state.commit_start(req.controller_id, owner, nt_success(start_st))
                };
                if committed {
                    dbglog!(
                        "created controller slot {} profile {:?}",
                        req.controller_id,
                        plan.profile.id
                    );
                    request.complete(STATUS_SUCCESS);
                } else {
                    self.drain_and_delete_vhf(req.controller_id, vhf);
                    let mut state = self.state_lock.lock().unwrap();
                    state.release_starting_slot(req.controller_id, owner);
                    request.complete(if nt_success(start_st) {
                        DriverStatus::Cancelled.ntstatus()
                    } else {
                        start_st
                    });
                }
            }
            Err(st) => {
                let mut state = self.state_lock.lock().unwrap();
                state.release_starting_slot(req.controller_id, owner);
                dbglog!(
                    "create_and_start failed for slot {}: {st:#x}",
                    req.controller_id
                );
                request.complete(st);
            }
        }
    }

    fn ioctl_destroy_controller(&self, request: Request, owner: OwnerToken) {
        let req = match read_exact(
            &request,
            ControllerIdRequest::WIRE_SIZE,
            ControllerIdRequest::from_bytes,
        ) {
            Ok(r) => r,
            Err(st) => {
                request.complete(st);
                return;
            }
        };

        let _gate = self.lifetime_gate.write().unwrap();
        let vhf = {
            let mut state = self.state_lock.lock().unwrap();
            match state.begin_destroy_checked(owner, req.controller_id) {
                Ok(token) => token,
                Err(ds) => {
                    request.complete(ds.ntstatus());
                    return;
                }
            }
        };
        if let Some(vhf) = vhf {
            self.drain_and_delete_vhf(req.controller_id, vhf as VHFHANDLE);
            let mut state = self.state_lock.lock().unwrap();
            state.finish_destroy_owned(req.controller_id, owner);
        }
        request.complete(STATUS_SUCCESS);
    }

    fn ioctl_submit_input(self: &Arc<Self>, request: Request, owner: OwnerToken) {
        let req = match read_exact(
            &request,
            InputStateRequest::WIRE_SIZE,
            InputStateRequest::from_bytes,
        ) {
            Ok(r) => r,
            Err(st) => {
                request.complete(st);
                return;
            }
        };
        let _gate = self.lifetime_gate.read().unwrap();
        let now = now_us();
        let has_sc26_timer = self.sc26_keepalive.has_timer();
        let prepared = {
            let mut state = self.state_lock.lock().unwrap();
            match state.submit_input_state(owner, &req, now, has_sc26_timer) {
                Ok((pending, start_keepalive)) => {
                    let (pkt, arm_delay) = self.prepare_submit_locked(&state, pending, now, false);
                    Ok((pkt, start_keepalive, arm_delay))
                }
                Err(ds) => Err(ds.ntstatus()),
            }
        };
        match prepared {
            Ok((pkt, start_keepalive, arm_delay)) => {
                if let Some((p, pkt_ptr)) = pkt {
                    self.finish_submit(p, pkt_ptr);
                }
                if start_keepalive {
                    self.sc26_keepalive.ensure_started(self);
                } else if let Some(delay_us) = arm_delay {
                    self.sc26_keepalive.arm(delay_us);
                }
                request.complete(STATUS_SUCCESS);
            }
            Err(st) => request.complete(st),
        }
    }

    fn ioctl_poll_feedback(&self, request: Request, owner: OwnerToken) {
        if let Err(st) = request.check_output_len(FeedbackEvent::WIRE_SIZE) {
            request.complete(st);
            return;
        }
        let req = match read_exact(
            &request,
            ControllerIdRequest::WIRE_SIZE,
            ControllerIdRequest::from_bytes,
        ) {
            Ok(r) => r,
            Err(st) => {
                request.complete(st);
                return;
            }
        };
        let res = {
            let mut state = self.state_lock.lock().unwrap();
            state.poll_feedback(owner, &req)
        };
        match res {
            Ok(ev) => {
                let bytes: [u8; FeedbackEvent::WIRE_SIZE] = ev.to_bytes();
                match request.copy_to_output(&bytes) {
                    Ok(written) => request.complete_with_info(STATUS_SUCCESS, written),
                    Err(st) => request.complete(st),
                }
            }
            Err(ds) => request.complete(ds.ntstatus()),
        }
    }

    fn ioctl_state_update<T>(
        &self,
        request: Request,
        owner: OwnerToken,
        wire_size: usize,
        decode: fn(&[u8]) -> Option<T>,
        apply: impl FnOnce(
            &mut DeviceState,
            OwnerToken,
            &T,
            u64,
            bool,
        ) -> Result<Option<PendingSubmit>, DriverStatus>,
    ) {
        let req = match read_exact(&request, wire_size, decode) {
            Ok(r) => r,
            Err(st) => {
                request.complete(st);
                return;
            }
        };
        let _gate = self.lifetime_gate.read().unwrap();
        let now = now_us();
        let has_sc26_timer = self.sc26_keepalive.has_timer();
        let prepared = {
            let mut state = self.state_lock.lock().unwrap();
            match apply(&mut state, owner, &req, now, has_sc26_timer) {
                Ok(pending) => Ok(self.prepare_submit_locked(&state, pending, now, false)),
                Err(ds) => Err(ds.ntstatus()),
            }
        };
        match prepared {
            Ok((pkt, arm_delay)) => {
                self.finish_and_arm(pkt, arm_delay);
                request.complete(STATUS_SUCCESS);
            }
            Err(st) => request.complete(st),
        }
    }

    fn ioctl_submit_touch(&self, request: Request, owner: OwnerToken) {
        self.ioctl_state_update(
            request,
            owner,
            TouchStateRequest::WIRE_SIZE,
            TouchStateRequest::from_bytes,
            |state, owner, req, now, _| state.submit_touch_state(owner, req, now),
        );
    }

    fn ioctl_submit_motion(&self, request: Request, owner: OwnerToken) {
        self.ioctl_state_update(
            request,
            owner,
            MotionStateRequest::WIRE_SIZE,
            MotionStateRequest::from_bytes,
            |state, owner, req, now, has_sc26_timer| {
                state.submit_motion_state(owner, req, now, has_sc26_timer)
            },
        );
    }

    fn ioctl_submit_battery(&self, request: Request, owner: OwnerToken) {
        self.ioctl_state_update(
            request,
            owner,
            BatteryStateRequest::WIRE_SIZE,
            BatteryStateRequest::from_bytes,
            |state, owner, req, now, _| state.submit_battery_state(owner, req, now),
        );
    }

    fn ioctl_set_sc_attributes(&self, request: Request, owner: OwnerToken) {
        let req = match read_exact(
            &request,
            SteamControllerAttributesRequest::WIRE_SIZE,
            SteamControllerAttributesRequest::from_bytes,
        ) {
            Ok(r) => r,
            Err(st) => {
                request.complete(st);
                return;
            }
        };
        let status = {
            let mut state = self.state_lock.lock().unwrap();
            state.set_sc_attributes(owner, &req).ntstatus()
        };
        request.complete(status);
    }
}

fn read_exact<T>(
    request: &Request,
    size: usize,
    decode: impl FnOnce(&[u8]) -> Option<T>,
) -> Result<T, NTSTATUS> {
    let (bytes, len) = request.input_bytes(size)?;
    if len != size {
        return Err(DriverStatus::InvalidBufferSize.ntstatus());
    }
    decode(&bytes).ok_or_else(|| DriverStatus::InvalidBufferSize.ntstatus())
}

fn cm_child_exists_for_slot(controller_id: u32) -> bool {
    let target = format!("{}{controller_id}", protocol::INSTANCE_ID_PREFIX);
    let flags = CM_GETIDLIST_FILTER_ENUMERATOR | CM_GETIDLIST_FILTER_PRESENT;
    let filter = PCWSTR(w!("HID").as_ptr());

    for _ in 0..4 {
        let mut needed: u32 = 0;
        // SAFETY: `needed` is a valid stack out parameter; `filter` points to static `"HID\0"`.
        let st = unsafe { CM_Get_Device_ID_List_SizeW(&mut needed, filter, flags) };
        if st != CR_SUCCESS || needed == 0 || needed > 1024 * 1024 {
            return true;
        }
        let mut buf = vec![0u16; needed as usize];
        // SAFETY: `buf` has capacity `needed` UTF-16 code units; `filter` is `"HID\0"`.
        let list_st = unsafe { CM_Get_Device_ID_ListW(filter, &mut buf, flags) };
        if list_st == CR_SUCCESS {
            return matches_cm_child_instance_list(&buf, &target);
        }
        if list_st != CR_BUFFER_SMALL {
            return true;
        }
    }
    true
}
