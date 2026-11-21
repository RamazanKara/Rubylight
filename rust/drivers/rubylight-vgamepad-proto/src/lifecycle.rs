// Copyright (c) 2026 Chase Payne
// SPDX-License-Identifier: MIT
//
// Ported from Nonary/libvirtualgamepad driver/src/driver.cpp.
// Adapted for Rubylight by RamazanKara (https://github.com/RamazanKara/Rubylight).

//! Portable controller slot state machine and VHF/IOCTL protocol router.
//!
//! Keeps all controller bookkeeping, admission rules, feedback coalescing/rings,
//! D0 exit loss semantics, and report serialization independent of WDF/Win32 types
//! so every lifecycle invariant can be tested with ordinary `cargo test`.

use crate::profiles::pid_ff::{self, PidEngine, PidRumble};
use crate::profiles::{
    self, ProfileDefinition, dualsense, dualshock4, generic, is_playstation, is_steam_controller,
    is_xbox, steam_controller, switch_pro, xbox_one, xbox_series,
};
use crate::protocol::{
    BatteryStateRequest, CONTROLLER_CONTAINER_BASE, ControllerIdRequest, CreateControllerRequest,
    FeedbackCounters, FeedbackEvent, FeedbackType, GenericRumbleRgbFeedback, InputStateRequest,
    MAX_CONTROLLERS, MotionStateRequest, PROTOCOL_VERSION, PlayStationOutputFeedback, Profile,
    QueryInfoResponse, RequestHeader, SteamControllerAttributesRequest, TouchEvent,
    TouchStateRequest, feature_mask, format_instance_id_utf16, saturate_counter,
};
use crate::report_pump::{MAX_REPORT_BYTES, ReportBuffer, ReportKind, ReportPump};
use crate::steam_haptic_queue::{HapticRing, SETTINGS_EVENTS_PER_WRITE, SettingsRing};

/// Windows NTSTATUS codes returned by the driver's IOCTL and VHF handlers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum DriverStatus {
    Success = 0x0000_0000u32 as i32,
    NotSupported = 0xC000_00BBu32 as i32,
    InvalidParameter = 0xC000_000Du32 as i32,
    InvalidDeviceRequest = 0xC000_0010u32 as i32,
    AccessDenied = 0xC000_0022u32 as i32,
    BufferTooSmall = 0xC000_0023u32 as i32,
    InvalidBufferSize = 0xC000_0206u32 as i32,
    DeviceNotReady = 0xC000_00A3u32 as i32,
    DevicePoweredOff = 0x8000_000Fu32 as i32,
    DeviceBusy = 0x8000_0011u32 as i32,
    Cancelled = 0xC000_0120u32 as i32,
    NoMoreEntries = 0x8000_001Au32 as i32,
    NotFound = 0xC000_0225u32 as i32,
    DeviceRemoved = 0xC000_02B6u32 as i32,
}

impl DriverStatus {
    pub const fn ntstatus(self) -> i32 {
        self as i32
    }

    pub const fn is_success(self) -> bool {
        (self as i32) >= 0
    }
}

pub const PID_TICK_MS: u32 = 10;
pub const SC26_TICK_MS: u32 = 4;
pub const SC26_RESEND_AFTER_US: u64 = 3_000;
pub const SC26_BATTERY_PERIOD_US: u64 = 3_500_000;

/// Opaque identity of a WDF file handle owning controller slots.
pub type OwnerToken = usize;

/// Opaque handle representing an active VHF child (e.g. `VHFHANDLE as usize`).
pub type VhfHandleToken = usize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SlotState {
    #[default]
    Empty,
    Starting,
    Active,
    Stopping,
    Lost,
}

/// Complete mutable state of a single controller slot (owned under `state_lock`).
#[derive(Clone, Debug)]
pub struct ControllerSlot {
    pub controller_id: u32,
    pub generation: u64,
    pub vhf: Option<VhfHandleToken>,
    pub owner: OwnerToken,
    pub selected_profile: Option<Profile>,
    pub state: SlotState,

    pub feedback_pending: bool,
    pub feedback: FeedbackEvent,
    pub haptic_queue: HapticRing,
    pub settings_queue: SettingsRing,
    pub submits_in_flight: u32,

    pub playstation_feedback: PlayStationOutputFeedback,
    pub force_feedback: bool,
    pub pid_rumble_valid: bool,
    pub pid_rumble: PidRumble,

    pub have_last_input: bool,
    pub last_input: InputStateRequest,
    pub ds4: dualshock4::Ds4State,
    pub ds5: dualsense::Ds5State,
    pub switch_pro: switch_pro::SwitchState,
    pub sc26: steam_controller::Sc26State,
    pub sc26_wire_sequence: u8,

    pub pump: ReportPump,
    pub pid: PidEngine,

    pub container_id: u128,
    pub instance_id_utf16: [u16; 32],
    pub instance_id_utf16_len: usize,
}

impl ControllerSlot {
    pub fn new(controller_id: u32) -> Self {
        Self {
            controller_id,
            generation: 0,
            vhf: None,
            owner: 0,
            selected_profile: None,
            state: SlotState::Empty,
            feedback_pending: false,
            feedback: FeedbackEvent::default(),
            haptic_queue: HapticRing::new(),
            settings_queue: SettingsRing::new(),
            submits_in_flight: 0,
            playstation_feedback: PlayStationOutputFeedback::default(),
            force_feedback: false,
            pid_rumble_valid: false,
            pid_rumble: PidRumble::default(),
            have_last_input: false,
            last_input: InputStateRequest::default(),
            ds4: dualshock4::Ds4State::default(),
            ds5: dualsense::Ds5State::default(),
            switch_pro: switch_pro::SwitchState::default(),
            sc26: steam_controller::Sc26State::default(),
            sc26_wire_sequence: 0,
            pump: ReportPump::new(),
            pid: PidEngine::new(),
            container_id: 0,
            instance_id_utf16: [0; 32],
            instance_id_utf16_len: 0,
        }
    }

    pub fn reset(&mut self) {
        let controller_id = self.controller_id;
        let generation = self.generation.wrapping_add(1);
        *self = Self::new(controller_id);
        self.generation = generation;
    }

    pub fn is_owned_by(&self, owner: OwnerToken) -> bool {
        owner != 0 && self.owner == owner
    }

    pub fn accepts_vhf_callbacks(&self, device_stopping: bool, device_powered: bool) -> bool {
        !device_stopping
            && device_powered
            && self.vhf.is_some()
            && matches!(self.state, SlotState::Starting | SlotState::Active)
    }

    pub fn inactive_status(&self, owner: OwnerToken) -> DriverStatus {
        if self.is_owned_by(owner) && self.state == SlotState::Lost {
            DriverStatus::DeviceRemoved
        } else {
            DriverStatus::DeviceNotReady
        }
    }

    pub fn counters(&self) -> FeedbackCounters {
        FeedbackCounters {
            haptic_evicted: saturate_counter(self.haptic_queue.evicted()),
            settings_evicted: saturate_counter(self.settings_queue.evicted()),
            transitions_dropped: saturate_counter(self.pump.dropped_transitions()),
            unknown_commands: saturate_counter(self.sc26.features.unknown_commands),
        }
    }
}

/// Action returned after a report is taken out of a slot's `ReportPump` under `state_lock`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PendingSubmit {
    pub controller_id: u32,
    pub vhf: VhfHandleToken,
    pub report_id: u8,
    pub length: u32,
    pub rearm_keepalive: bool,
}

/// Result of preparing a controller creation under `state_lock`.
#[derive(Clone, Copy, Debug)]
pub struct CreatePlan {
    pub controller_id: u32,
    pub profile: &'static ProfileDefinition,
    pub container_id: u128,
    pub instance_id_utf16: [u16; 32],
    pub instance_id_utf16_len: usize,
    pub register_feature_callbacks: bool,
}

/// Core device state protected by `state_lock` (inner lock).
#[derive(Clone, Debug)]
pub struct DeviceState {
    pub stopping: bool,
    pub powered: bool,
    pub sc26_timer_running: bool,
    pub sc26_timer_parked: bool,
    pub slots: [ControllerSlot; MAX_CONTROLLERS as usize],
}

impl Default for DeviceState {
    fn default() -> Self {
        Self::new()
    }
}

impl DeviceState {
    pub fn new() -> Self {
        Self {
            stopping: false,
            powered: false,
            sc26_timer_running: false,
            sc26_timer_parked: false,
            slots: core::array::from_fn(|i| ControllerSlot::new(i as u32)),
        }
    }

    pub fn fill_query_info(&self) -> QueryInfoResponse {
        QueryInfoResponse {
            header: RequestHeader::new(QueryInfoResponse::WIRE_SIZE as u32),
            minimum_protocol_version: PROTOCOL_VERSION,
            maximum_protocol_version: PROTOCOL_VERSION,
            available_profiles: profiles::available_profiles(),
            available_features: feature_mask::ALL,
            maximum_controllers: MAX_CONTROLLERS,
            counters: FeedbackCounters::default(),
        }
    }

    pub fn query_controller_info(
        &self,
        owner: OwnerToken,
        req: &ControllerIdRequest,
    ) -> Result<QueryInfoResponse, DriverStatus> {
        if req.controller_id >= MAX_CONTROLLERS {
            return Err(DriverStatus::InvalidParameter);
        }
        let slot = &self.slots[req.controller_id as usize];
        if slot.state == SlotState::Empty {
            return Err(DriverStatus::NotFound);
        }
        if !slot.is_owned_by(owner) {
            return Err(DriverStatus::AccessDenied);
        }
        let mut resp = self.fill_query_info();
        resp.counters = slot.counters();
        Ok(resp)
    }

    /// Pre-flight check outside `lifetime_gate` before calling OS Configuration Manager.
    pub fn check_create_preflight(
        &self,
        owner: OwnerToken,
        req: &CreateControllerRequest,
    ) -> Result<(&'static ProfileDefinition, u64), DriverStatus> {
        if req.controller_id >= MAX_CONTROLLERS || req.reserved != 0 {
            return Err(DriverStatus::InvalidParameter);
        }
        let Some(profile) = Profile::from_raw(req.requested_profile) else {
            return Err(DriverStatus::NotSupported);
        };
        let Some(def) = profiles::find_profile(profile) else {
            return Err(DriverStatus::NotSupported);
        };
        if self.stopping {
            return Err(DriverStatus::DeviceNotReady);
        }
        if !self.powered {
            return Err(DriverStatus::DevicePoweredOff);
        }
        let slot = &self.slots[req.controller_id as usize];
        let available = slot.state == SlotState::Empty
            || (slot.state == SlotState::Lost && slot.is_owned_by(owner));
        if !available {
            return Err(DriverStatus::DeviceBusy);
        }
        Ok((def, slot.generation))
    }

    /// Commits slot transition to `Starting` under `lifetime_gate + state_lock`.
    pub fn begin_create(
        &mut self,
        owner: OwnerToken,
        req: &CreateControllerRequest,
        expected_generation: u64,
        child_present: bool,
    ) -> Result<CreatePlan, DriverStatus> {
        let (def, _) = self.check_create_preflight(owner, req)?;
        let slot = &mut self.slots[req.controller_id as usize];
        if slot.generation != expected_generation || child_present {
            return Err(DriverStatus::DeviceBusy);
        }
        slot.reset();
        slot.owner = owner;
        slot.selected_profile = Some(def.id);
        slot.state = SlotState::Starting;
        slot.force_feedback = def.force_feedback;
        slot.ds4.features.address[0] = req.controller_id as u8;
        slot.ds5.features.address[0] = req.controller_id as u8;
        slot.sc26.stamp_slot_serial(req.controller_id);

        slot.container_id = CONTROLLER_CONTAINER_BASE | u128::from(req.controller_id as u8);
        slot.instance_id_utf16_len =
            format_instance_id_utf16(req.controller_id, &mut slot.instance_id_utf16);

        let register_feature_callbacks =
            def.force_feedback || is_playstation(def.id) || is_steam_controller(def.id);

        Ok(CreatePlan {
            controller_id: req.controller_id,
            profile: def,
            container_id: slot.container_id,
            instance_id_utf16: slot.instance_id_utf16,
            instance_id_utf16_len: slot.instance_id_utf16_len,
            register_feature_callbacks,
        })
    }

    pub fn release_starting_slot(&mut self, controller_id: u32, owner: OwnerToken) {
        let Some(slot) = self.slots.get_mut(controller_id as usize) else {
            return;
        };
        if slot.is_owned_by(owner)
            && matches!(slot.state, SlotState::Starting | SlotState::Stopping)
        {
            slot.reset();
        }
    }

    pub fn publish_vhf(&mut self, controller_id: u32, vhf: VhfHandleToken) {
        if let Some(slot) = self.slots.get_mut(controller_id as usize) {
            slot.vhf = Some(vhf);
        }
    }

    /// Finalizes `Starting -> Active` if still powered/unstopped; otherwise switches to
    /// `Stopping` and detaches `vhf` so caller can synchronously `VhfDelete`.
    pub fn commit_start(
        &mut self,
        controller_id: u32,
        owner: OwnerToken,
        start_succeeded: bool,
    ) -> bool {
        let slot = &mut self.slots[controller_id as usize];
        if start_succeeded
            && !self.stopping
            && self.powered
            && slot.is_owned_by(owner)
            && slot.state == SlotState::Starting
        {
            slot.state = SlotState::Active;
            true
        } else {
            slot.state = SlotState::Stopping;
            slot.feedback_pending = false;
            slot.vhf = None;
            false
        }
    }

    pub fn begin_destroy_checked(
        &mut self,
        owner: OwnerToken,
        controller_id: u32,
    ) -> Result<Option<VhfHandleToken>, DriverStatus> {
        if controller_id >= MAX_CONTROLLERS {
            return Err(DriverStatus::InvalidParameter);
        }
        let slot = &self.slots[controller_id as usize];
        if slot.state == SlotState::Empty {
            return Err(DriverStatus::NotFound);
        }
        if !slot.is_owned_by(owner) {
            return Err(DriverStatus::AccessDenied);
        }
        Ok(self.begin_destroy_owned(owner, controller_id))
    }

    /// Transitions an owned slot toward deletion. Returns `Some(vhf)` if the caller
    /// must wait for `submits_in_flight == 0` and call `VhfDelete` + `finish_destroy_owned`.
    pub fn begin_destroy_owned(
        &mut self,
        owner: OwnerToken,
        controller_id: u32,
    ) -> Option<VhfHandleToken> {
        let slot = self.slots.get_mut(controller_id as usize)?;
        if !slot.is_owned_by(owner) || slot.state == SlotState::Empty {
            return None;
        }
        match slot.state {
            SlotState::Starting => {
                slot.state = SlotState::Stopping;
                None
            }
            SlotState::Lost => {
                slot.reset();
                None
            }
            SlotState::Active | SlotState::Stopping => {
                slot.state = SlotState::Stopping;
                slot.feedback_pending = false;
                slot.haptic_queue.clear();
                slot.settings_queue.clear();
                slot.vhf.take()
            }
            SlotState::Empty => None,
        }
    }

    pub fn finish_destroy_owned(&mut self, controller_id: u32, owner: OwnerToken) {
        if let Some(slot) = self.slots.get_mut(controller_id as usize)
            && slot.is_owned_by(owner)
            && slot.state == SlotState::Stopping
        {
            slot.reset();
        }
    }

    pub fn set_sc_attributes(
        &mut self,
        owner: OwnerToken,
        req: &SteamControllerAttributesRequest,
    ) -> DriverStatus {
        if req.controller_id >= MAX_CONTROLLERS {
            return DriverStatus::InvalidParameter;
        }
        let slot = &mut self.slots[req.controller_id as usize];
        if slot.state == SlotState::Empty {
            return DriverStatus::NotFound;
        }
        if !slot.is_owned_by(owner) {
            return DriverStatus::AccessDenied;
        }
        if !slot.selected_profile.is_some_and(is_steam_controller) {
            return DriverStatus::NotSupported;
        }
        steam_controller::apply_attributes(req, &mut slot.sc26);
        DriverStatus::Success
    }

    pub fn poll_feedback(
        &mut self,
        owner: OwnerToken,
        req: &ControllerIdRequest,
    ) -> Result<FeedbackEvent, DriverStatus> {
        if req.controller_id >= MAX_CONTROLLERS {
            return Err(DriverStatus::InvalidParameter);
        }
        let slot = &mut self.slots[req.controller_id as usize];
        if slot.state == SlotState::Empty {
            return Err(DriverStatus::NotFound);
        }
        if !slot.is_owned_by(owner) {
            return Err(DriverStatus::AccessDenied);
        }
        if slot.state != SlotState::Active {
            return Err(slot.inactive_status(owner));
        }
        if let Some(ev) = slot.settings_queue.pop() {
            return Ok(ev);
        }
        if let Some(ev) = slot.haptic_queue.pop() {
            return Ok(ev);
        }
        if !slot.feedback_pending {
            return Err(DriverStatus::NoMoreEntries);
        }
        slot.feedback_pending = false;
        Ok(slot.feedback)
    }

    fn take_and_stamp(
        slot: &mut ControllerSlot,
        now_us: u64,
    ) -> Option<(u8, u32, VhfHandleToken, bool)> {
        let taken = slot.pump.take()?;
        let is_sc26_input = taken.report_id == steam_controller::INPUT_REPORT_ID
            && taken.length > 1
            && slot.selected_profile.is_some_and(is_steam_controller);
        if is_sc26_input {
            taken.data[1] = slot.sc26_wire_sequence;
            slot.sc26_wire_sequence = slot.sc26_wire_sequence.wrapping_add(1);
            slot.sc26.last_report_us = now_us;
        }
        let vhf = slot.vhf?;
        slot.submits_in_flight += 1;
        Some((taken.report_id, taken.length, vhf, is_sc26_input))
    }

    pub fn submit_finish(&mut self, controller_id: u32) {
        if let Some(slot) = self.slots.get_mut(controller_id as usize)
            && slot.submits_in_flight > 0
        {
            slot.submits_in_flight -= 1;
        }
    }

    /// Enqueues an encoded report on `controller_id` if callbacks are admitted and takes
    /// the next ready report if VHF is currently ready.
    pub fn pump_enqueue_and_take(
        &mut self,
        controller_id: u32,
        data: Option<(&[u8], u8, ReportKind)>,
        now_us: u64,
        rearm_keepalive: bool,
    ) -> Result<Option<PendingSubmit>, DriverStatus> {
        let stopping = self.stopping;
        let powered = self.powered;
        let slot = &mut self.slots[controller_id as usize];
        if !slot.accepts_vhf_callbacks(stopping, powered) {
            return Err(DriverStatus::DeviceNotReady);
        }
        if let Some((bytes, report_id, kind)) = data {
            let _ = slot.pump.enqueue(bytes, report_id, kind);
        }
        if let Some((report_id, length, vhf, is_sc26)) = Self::take_and_stamp(slot, now_us) {
            Ok(Some(PendingSubmit {
                controller_id,
                vhf,
                report_id,
                length,
                rearm_keepalive: rearm_keepalive && is_sc26,
            }))
        } else {
            Ok(None)
        }
    }

    pub fn on_vhf_ready_for_next_report(
        &mut self,
        controller_id: u32,
        now_us: u64,
    ) -> (Option<PendingSubmit>, bool) {
        let stopping = self.stopping;
        let powered = self.powered;
        let timer_parked = self.sc26_timer_parked;
        let slot = &mut self.slots[controller_id as usize];
        if !slot.accepts_vhf_callbacks(stopping, powered) {
            return (None, false);
        }
        slot.pump.set_ready();
        let pending =
            Self::take_and_stamp(slot, now_us).map(|(report_id, length, vhf, is_sc26)| {
                PendingSubmit {
                    controller_id,
                    vhf,
                    report_id,
                    length,
                    rearm_keepalive: is_sc26,
                }
            });
        let unpark_keepalive = timer_parked
            && slot.have_last_input
            && slot.selected_profile.is_some_and(is_steam_controller);
        if unpark_keepalive {
            self.sc26_timer_parked = false;
        }
        (pending, unpark_keepalive)
    }

    /// Handles `IOCTL_SUBMIT_INPUT_STATE`. Returns `(Option<PendingSubmit>, start_sc26_keepalive)`.
    pub fn submit_input_state(
        &mut self,
        owner: OwnerToken,
        req: &InputStateRequest,
        now_us: u64,
        has_sc26_timer: bool,
    ) -> Result<(Option<PendingSubmit>, bool), DriverStatus> {
        if req.controller_id >= MAX_CONTROLLERS || req.reserved != 0 {
            return Err(DriverStatus::InvalidParameter);
        }
        let stopping = self.stopping;
        let slot = &mut self.slots[req.controller_id as usize];
        if slot.state == SlotState::Empty
            || !slot.is_owned_by(owner)
            || stopping
            || slot.state != SlotState::Active
            || slot.vhf.is_none()
        {
            return Err(slot.inactive_status(owner));
        }

        let Some(profile) = slot.selected_profile else {
            return Err(DriverStatus::DeviceNotReady);
        };

        let mut start_keepalive = false;
        let mut buf = [0u8; MAX_REPORT_BYTES];
        let (len, report_id, kind) = match profile {
            Profile::DualShock4 => {
                slot.last_input = *req;
                slot.have_last_input = true;
                let kind = slot
                    .pump
                    .classify(req.buttons, req.left_trigger, req.right_trigger);
                let r = dualshock4::encode_input(req, Some(&mut slot.ds4), now_us);
                buf[..r.len()].copy_from_slice(&r);
                (r.len(), dualshock4::INPUT_REPORT_ID, kind)
            }
            Profile::DualSense => {
                slot.last_input = *req;
                slot.have_last_input = true;
                let kind = slot
                    .pump
                    .classify(req.buttons, req.left_trigger, req.right_trigger);
                let r = dualsense::encode_input(req, Some(&mut slot.ds5), now_us);
                buf[..r.len()].copy_from_slice(&r);
                (r.len(), dualsense::INPUT_REPORT_ID, kind)
            }
            Profile::SteamController => {
                slot.last_input = *req;
                slot.have_last_input = true;
                steam_controller::sc26_tick(&mut slot.sc26, now_us);
                let r = steam_controller::encode_input(req, &mut slot.sc26);
                let enc_btns = u32::from_le_bytes([r[2], r[3], r[4], r[5]]);
                let cls_btns = steam_controller::classify_buttons(&slot.sc26.device, enc_btns);
                let kind = slot
                    .pump
                    .classify(cls_btns, req.left_trigger, req.right_trigger);
                if has_sc26_timer && !self.sc26_timer_running {
                    self.sc26_timer_running = true;
                    start_keepalive = true;
                }
                buf[..r.len()].copy_from_slice(&r);
                (r.len(), steam_controller::INPUT_REPORT_ID, kind)
            }
            Profile::SwitchPro => {
                slot.last_input = *req;
                slot.have_last_input = true;
                let kind = slot
                    .pump
                    .classify(req.buttons, req.left_trigger, req.right_trigger);
                let r = switch_pro::encode_input(req, Some(&mut slot.switch_pro));
                buf[..r.len()].copy_from_slice(&r);
                (r.len(), switch_pro::INPUT_REPORT_ID, kind)
            }
            Profile::XboxOne => {
                slot.last_input = *req;
                slot.have_last_input = true;
                let kind = slot
                    .pump
                    .classify(req.buttons, req.left_trigger, req.right_trigger);
                let r = xbox_one::encode_input(req);
                buf[..r.len()].copy_from_slice(&r);
                (r.len(), xbox_one::INPUT_REPORT_ID, kind)
            }
            Profile::XboxSeries => {
                slot.last_input = *req;
                slot.have_last_input = true;
                let kind = slot
                    .pump
                    .classify(req.buttons, req.left_trigger, req.right_trigger);
                let r = xbox_series::encode_input(req);
                buf[..r.len()].copy_from_slice(&r);
                (r.len(), xbox_series::INPUT_REPORT_ID, kind)
            }
            Profile::GenericHid | Profile::GenericPid => {
                slot.last_input = *req;
                slot.have_last_input = true;
                let kind = slot
                    .pump
                    .classify(req.buttons, req.left_trigger, req.right_trigger);
                let r = generic::encode_generic_input(req);
                buf[..r.len()].copy_from_slice(&r);
                (r.len(), generic::GENERIC_INPUT_REPORT_ID, kind)
            }
            Profile::Xbox360 => return Err(DriverStatus::NotSupported),
        };

        let submit = self.pump_enqueue_and_take(
            req.controller_id,
            Some((&buf[..len], report_id, kind)),
            now_us,
            true,
        )?;
        Ok((submit, start_keepalive))
    }

    fn check_state_update_target(
        &self,
        owner: OwnerToken,
        controller_id: u32,
    ) -> Result<Profile, DriverStatus> {
        if controller_id >= MAX_CONTROLLERS {
            return Err(DriverStatus::InvalidParameter);
        }
        let slot = &self.slots[controller_id as usize];
        if slot.state == SlotState::Empty {
            return Err(DriverStatus::NotFound);
        }
        if !slot.is_owned_by(owner) {
            return Err(DriverStatus::AccessDenied);
        }
        if self.stopping || slot.state != SlotState::Active || slot.vhf.is_none() {
            return Err(slot.inactive_status(owner));
        }
        let Some(profile) = slot.selected_profile else {
            return Err(DriverStatus::DeviceNotReady);
        };
        if !is_playstation(profile)
            && profile != Profile::SwitchPro
            && !is_steam_controller(profile)
        {
            return Err(DriverStatus::NotSupported);
        }
        Ok(profile)
    }

    fn rebuild_profile_report(
        &mut self,
        controller_id: u32,
        kind: ReportKind,
        now_us: u64,
    ) -> Result<Option<PendingSubmit>, DriverStatus> {
        let slot = &mut self.slots[controller_id as usize];
        if self.stopping || slot.state != SlotState::Active || slot.vhf.is_none() {
            return Err(DriverStatus::DeviceNotReady);
        }
        if !slot.have_last_input {
            return Ok(None);
        }
        let profile = slot.selected_profile.ok_or(DriverStatus::DeviceNotReady)?;
        let mut buf = [0u8; MAX_REPORT_BYTES];
        let (len, report_id) = match profile {
            Profile::DualShock4 => {
                let r = dualshock4::encode_input(&slot.last_input, Some(&mut slot.ds4), now_us);
                buf[..r.len()].copy_from_slice(&r);
                (r.len(), dualshock4::INPUT_REPORT_ID)
            }
            Profile::DualSense => {
                let r = dualsense::encode_input(&slot.last_input, Some(&mut slot.ds5), now_us);
                buf[..r.len()].copy_from_slice(&r);
                (r.len(), dualsense::INPUT_REPORT_ID)
            }
            Profile::SwitchPro => {
                let r = switch_pro::encode_input(&slot.last_input, Some(&mut slot.switch_pro));
                buf[..r.len()].copy_from_slice(&r);
                (r.len(), switch_pro::INPUT_REPORT_ID)
            }
            Profile::SteamController => {
                steam_controller::sc26_tick(&mut slot.sc26, now_us);
                let r = steam_controller::encode_input(&slot.last_input, &mut slot.sc26);
                buf[..r.len()].copy_from_slice(&r);
                (r.len(), steam_controller::INPUT_REPORT_ID)
            }
            _ => return Ok(None),
        };
        self.pump_enqueue_and_take(
            controller_id,
            Some((&buf[..len], report_id, kind)),
            now_us,
            true,
        )
    }

    pub fn submit_touch_state(
        &mut self,
        owner: OwnerToken,
        req: &TouchStateRequest,
        now_us: u64,
    ) -> Result<Option<PendingSubmit>, DriverStatus> {
        if req.reserved != 0 {
            return Err(DriverStatus::InvalidParameter);
        }
        let profile = self.check_state_update_target(owner, req.controller_id)?;
        if profile == Profile::SwitchPro {
            return Err(DriverStatus::NotSupported);
        }
        let slot = &mut self.slots[req.controller_id as usize];
        let applied = match profile {
            Profile::DualShock4 => dualshock4::apply_touch(req, &mut slot.ds4),
            Profile::DualSense => dualsense::apply_touch(req, &mut slot.ds5),
            Profile::SteamController => steam_controller::apply_touch(req, &mut slot.sc26),
            _ => return Err(DriverStatus::NotSupported),
        };
        if !applied {
            return Err(DriverStatus::InvalidParameter);
        }
        let kind = match TouchEvent::from_raw(req.event_type) {
            Some(TouchEvent::Move | TouchEvent::Hover) => ReportKind::Continuous,
            _ => ReportKind::Transition,
        };
        self.rebuild_profile_report(req.controller_id, kind, now_us)
    }

    pub fn submit_motion_state(
        &mut self,
        owner: OwnerToken,
        req: &MotionStateRequest,
        now_us: u64,
        has_sc26_timer: bool,
    ) -> Result<Option<PendingSubmit>, DriverStatus> {
        if req.reserved0 != [0; 3] {
            return Err(DriverStatus::InvalidParameter);
        }
        let profile = self.check_state_update_target(owner, req.controller_id)?;
        let slot = &mut self.slots[req.controller_id as usize];
        let applied = match profile {
            Profile::DualShock4 => dualshock4::apply_motion(req, &mut slot.ds4),
            Profile::DualSense => dualsense::apply_motion(req, &mut slot.ds5),
            Profile::SteamController => steam_controller::apply_motion(req, &mut slot.sc26, now_us),
            Profile::SwitchPro => switch_pro::apply_motion(req, &mut slot.switch_pro),
            _ => return Err(DriverStatus::NotSupported),
        };
        if !applied {
            return Err(DriverStatus::InvalidParameter);
        }
        if profile == Profile::SteamController && has_sc26_timer && self.sc26_timer_running {
            return Ok(None);
        }
        self.rebuild_profile_report(req.controller_id, ReportKind::Continuous, now_us)
    }

    pub fn submit_battery_state(
        &mut self,
        owner: OwnerToken,
        req: &BatteryStateRequest,
        now_us: u64,
    ) -> Result<Option<PendingSubmit>, DriverStatus> {
        if req.reserved != 0 {
            return Err(DriverStatus::InvalidParameter);
        }
        let profile = self.check_state_update_target(owner, req.controller_id)?;
        let slot = &mut self.slots[req.controller_id as usize];
        let applied = match profile {
            Profile::DualShock4 => dualshock4::apply_battery(req, &mut slot.ds4),
            Profile::DualSense => dualsense::apply_battery(req, &mut slot.ds5),
            Profile::SteamController => steam_controller::apply_battery(req, &mut slot.sc26),
            Profile::SwitchPro => switch_pro::apply_battery(req, &mut slot.switch_pro),
            _ => return Err(DriverStatus::NotSupported),
        };
        if !applied {
            return Err(DriverStatus::InvalidParameter);
        }
        if profile == Profile::SteamController {
            let ready = slot.pump.ready();
            slot.sc26.last_battery_us = if ready { now_us } else { 0 };
            if !ready {
                return Ok(None);
            }
            let battery_report = steam_controller::encode_battery(&slot.sc26);
            return self.pump_enqueue_and_take(
                req.controller_id,
                Some((
                    &battery_report,
                    steam_controller::BATTERY_REPORT_ID,
                    ReportKind::Aside,
                )),
                now_us,
                true,
            );
        }
        self.rebuild_profile_report(req.controller_id, ReportKind::Continuous, now_us)
    }

    /// Handles `EvtVhfAsyncOperationGetInputReport`. Returns written slice length on success.
    pub fn on_get_input_report(
        &mut self,
        controller_id: u32,
        requested_report_id: u8,
        out: &mut [u8],
        now_us: u64,
    ) -> Result<usize, DriverStatus> {
        let stopping = self.stopping;
        let powered = self.powered;
        let slot = &mut self.slots[controller_id as usize];
        if !slot.accepts_vhf_callbacks(stopping, powered) {
            return Err(DriverStatus::DeviceNotReady);
        }
        let profile = slot.selected_profile.ok_or(DriverStatus::DeviceNotReady)?;
        let mut buf = [0u8; MAX_REPORT_BYTES];
        let (length, report_id) = match profile {
            Profile::DualShock4 => {
                let r = dualshock4::encode_input(&slot.last_input, Some(&mut slot.ds4), now_us);
                buf[..r.len()].copy_from_slice(&r);
                (r.len(), dualshock4::INPUT_REPORT_ID)
            }
            Profile::DualSense => {
                let r = dualsense::encode_input(&slot.last_input, Some(&mut slot.ds5), now_us);
                buf[..r.len()].copy_from_slice(&r);
                (r.len(), dualsense::INPUT_REPORT_ID)
            }
            Profile::SwitchPro => {
                let r = switch_pro::encode_input(&slot.last_input, Some(&mut slot.switch_pro));
                buf[..r.len()].copy_from_slice(&r);
                (r.len(), switch_pro::INPUT_REPORT_ID)
            }
            Profile::XboxSeries => {
                let r = xbox_series::encode_input(&slot.last_input);
                buf[..r.len()].copy_from_slice(&r);
                (r.len(), xbox_series::INPUT_REPORT_ID)
            }
            Profile::XboxOne => {
                let r = xbox_one::encode_input(&slot.last_input);
                buf[..r.len()].copy_from_slice(&r);
                (r.len(), xbox_one::INPUT_REPORT_ID)
            }
            Profile::SteamController => {
                if requested_report_id == steam_controller::BATTERY_REPORT_ID {
                    let r = steam_controller::encode_battery(&slot.sc26);
                    buf[..r.len()].copy_from_slice(&r);
                    (r.len(), steam_controller::BATTERY_REPORT_ID)
                } else {
                    steam_controller::sc26_tick(&mut slot.sc26, now_us);
                    let mut r = steam_controller::encode_input(&slot.last_input, &mut slot.sc26);
                    r[1] = slot.sc26_wire_sequence;
                    buf[..r.len()].copy_from_slice(&r);
                    (r.len(), steam_controller::INPUT_REPORT_ID)
                }
            }
            Profile::GenericHid | Profile::GenericPid => {
                let r = generic::encode_generic_input(&slot.last_input);
                buf[..r.len()].copy_from_slice(&r);
                (r.len(), generic::GENERIC_INPUT_REPORT_ID)
            }
            Profile::Xbox360 => return Err(DriverStatus::InvalidDeviceRequest),
        };
        if requested_report_id != 0 && requested_report_id != report_id {
            return Err(DriverStatus::InvalidDeviceRequest);
        }
        if out.len() < length {
            return Err(DriverStatus::BufferTooSmall);
        }
        out[..length].copy_from_slice(&buf[..length]);
        Ok(length)
    }

    /// Handles `EvtVhfAsyncOperationGetFeature`. Returns written slice length on success.
    pub fn on_get_feature(
        &self,
        controller_id: u32,
        report_id: u8,
        out: &mut [u8],
    ) -> Result<usize, DriverStatus> {
        let slot = &self.slots[controller_id as usize];
        if !slot.accepts_vhf_callbacks(self.stopping, self.powered) {
            return Err(DriverStatus::DeviceNotReady);
        }
        let Some(profile) = slot.selected_profile else {
            return Err(DriverStatus::InvalidDeviceRequest);
        };
        if is_steam_controller(profile) {
            let written = steam_controller::get_feature(report_id, out, &slot.sc26);
            return if written != 0 {
                Ok(written)
            } else if report_id == steam_controller::FEATURES_REPORT_ID {
                Err(DriverStatus::BufferTooSmall)
            } else {
                Err(DriverStatus::InvalidDeviceRequest)
            };
        }
        if is_playstation(profile) {
            let written = if profile == Profile::DualShock4 {
                dualshock4::get_feature(report_id, out, &slot.ds4.features)
            } else {
                dualsense::get_feature(report_id, out, &slot.ds5.features)
            };
            return if written != 0 {
                Ok(written)
            } else {
                Err(DriverStatus::InvalidDeviceRequest)
            };
        }
        if slot.force_feedback {
            let (bytes, len): ([u8; 5], usize) = match report_id {
                pid_ff::PID_BLOCK_LOAD_REPORT_ID => (slot.pid.block_load(), 5),
                pid_ff::PID_POOL_REPORT_ID => (slot.pid.pool(), 5),
                pid_ff::PID_STATE_REPORT_ID => {
                    let s = slot.pid.state_report();
                    ([s[0], s[1], s[2], 0, 0], 3)
                }
                _ => return Err(DriverStatus::InvalidDeviceRequest),
            };
            if out.len() < len {
                return Err(DriverStatus::BufferTooSmall);
            }
            out[..len].copy_from_slice(&bytes[..len]);
            return Ok(len);
        }
        Err(DriverStatus::InvalidDeviceRequest)
    }

    /// Handles `EvtVhfAsyncOperationSetFeature`.
    pub fn on_set_feature(
        &mut self,
        controller_id: u32,
        report_id: u8,
        data: &[u8],
    ) -> DriverStatus {
        let stopping = self.stopping;
        let powered = self.powered;
        let slot = &mut self.slots[controller_id as usize];
        if !slot.accepts_vhf_callbacks(stopping, powered) {
            return DriverStatus::DeviceNotReady;
        }
        let Some(profile) = slot.selected_profile else {
            return DriverStatus::InvalidDeviceRequest;
        };
        if is_steam_controller(profile) {
            let accepted = steam_controller::set_feature(report_id, data, &mut slot.sc26);
            if accepted && report_id == steam_controller::FEATURES_REPORT_ID {
                let mut events = [FeedbackEvent::default(); SETTINGS_EVENTS_PER_WRITE];
                let count = steam_controller::apply_settings(data, slot.controller_id, &mut events);
                for ev in &events[..count] {
                    slot.settings_queue.enqueue(*ev);
                }
            }
            return if accepted {
                DriverStatus::Success
            } else {
                DriverStatus::InvalidParameter
            };
        }
        if is_playstation(profile) {
            let accepted = if profile == Profile::DualShock4 {
                dualshock4::set_feature(report_id, data, &mut slot.ds4.features)
            } else {
                dualsense::set_feature(report_id, data, &mut slot.ds5.features)
            };
            return if accepted {
                DriverStatus::Success
            } else {
                DriverStatus::InvalidDeviceRequest
            };
        }
        if slot.force_feedback && report_id == pid_ff::PID_CREATE_NEW_EFFECT_REPORT_ID {
            if data.len() < 4 || data[0] != report_id {
                return DriverStatus::InvalidParameter;
            }
            let _ = slot.pid.create_new_effect(data);
            return DriverStatus::Success;
        }
        DriverStatus::InvalidDeviceRequest
    }

    /// Handles `EvtVhfAsyncOperationWriteReport`.
    /// Returns `(status, optional_read_submit, arm_pid_timer)`.
    pub fn on_write_report(
        &mut self,
        controller_id: u32,
        report_id: u8,
        data: &[u8],
        now_us: u64,
    ) -> (DriverStatus, Option<PendingSubmit>, bool) {
        let stopping = self.stopping;
        let powered = self.powered;
        let slot = &mut self.slots[controller_id as usize];
        if !slot.accepts_vhf_callbacks(stopping, powered) {
            return (DriverStatus::DeviceNotReady, None, false);
        }
        let Some(profile) = slot.selected_profile else {
            return (DriverStatus::InvalidDeviceRequest, None, false);
        };

        if is_steam_controller(profile) {
            if let Some(ev) = steam_controller::apply_output(data, slot.controller_id) {
                let _ = slot.haptic_queue.enqueue(ev);
                let mut ack = [0u8; 6];
                let ack_len = steam_controller::encode_haptic_ack(data, &mut ack);
                let submit = if ack_len != 0 {
                    self.pump_enqueue_and_take(
                        controller_id,
                        Some((
                            &ack[..ack_len],
                            steam_controller::HAPTIC_ACK_REPORT_ID,
                            ReportKind::Aside,
                        )),
                        now_us,
                        true,
                    )
                    .ok()
                    .flatten()
                } else {
                    None
                };
                return (DriverStatus::Success, submit, false);
            } else if !data.is_empty()
                && data[0] > steam_controller::HAPTIC_SCRIPT_ID
                && steam_controller::is_output_report(data[0])
            {
                return (DriverStatus::Success, None, false);
            }
            return (DriverStatus::InvalidParameter, None, false);
        }

        if profile == Profile::SwitchPro {
            let mut rumble = PlayStationOutputFeedback::default();
            if switch_pro::decode_rumble(data, &mut rumble) {
                slot.feedback =
                    dualshock4::encode_playstation_feedback(slot.controller_id, &rumble);
                slot.feedback_pending = true;
            }
            let mut queued_reply = false;
            if report_id == switch_pro::USB_COMMAND_ID {
                let mut reply = [0u8; switch_pro::REPORT_SIZE];
                if switch_pro::handle_usb_command(data, Some(&mut slot.switch_pro), &mut reply) != 0
                {
                    queued_reply = slot.pump.enqueue(&reply, reply[0], ReportKind::Priority);
                }
            } else if report_id == switch_pro::RUMBLE_SUBCOMMAND_ID {
                queued_reply = switch_pro::queue_subcommand_reply(
                    data,
                    &slot.last_input,
                    &mut slot.switch_pro,
                    &mut slot.pump,
                );
            }
            let submit = if queued_reply {
                self.pump_enqueue_and_take(controller_id, None, now_us, true)
                    .ok()
                    .flatten()
            } else {
                None
            };
            return (DriverStatus::Success, submit, false);
        }

        if is_playstation(profile) {
            let mut feedback = slot.playstation_feedback;
            let decoded = if profile == Profile::DualShock4 {
                dualshock4::apply_output(data, &mut feedback)
            } else {
                dualsense::apply_output(data, &mut feedback)
            };
            if !decoded {
                return (DriverStatus::InvalidParameter, None, false);
            }
            slot.playstation_feedback = feedback;
            slot.feedback = dualshock4::encode_playstation_feedback(slot.controller_id, &feedback);
            slot.feedback_pending = true;
            return (DriverStatus::Success, None, false);
        }

        if is_xbox(profile) {
            if report_id != xbox_series::OUTPUT_REPORT_ID {
                return (DriverStatus::InvalidDeviceRequest, None, false);
            }
            let Some(rumble) = xbox_series::decode_output(data) else {
                return (DriverStatus::InvalidDeviceRequest, None, false);
            };
            slot.feedback = xbox_series::encode_feedback(slot.controller_id, &rumble);
            slot.feedback_pending = true;
            return (DriverStatus::Success, None, false);
        }

        if slot.force_feedback && report_id != generic::GENERIC_OUTPUT_REPORT_ID {
            let ok = match report_id {
                pid_ff::PID_SET_EFFECT_REPORT_ID => slot.pid.set_effect(data),
                pid_ff::PID_SET_ENVELOPE_REPORT_ID => slot.pid.set_envelope(data),
                pid_ff::PID_SET_CONDITION_REPORT_ID => slot.pid.set_condition(data),
                pid_ff::PID_SET_PERIODIC_REPORT_ID => slot.pid.set_periodic(data),
                pid_ff::PID_SET_CONSTANT_FORCE_REPORT_ID => slot.pid.set_constant_force(data),
                pid_ff::PID_SET_RAMP_FORCE_REPORT_ID => slot.pid.set_ramp_force(data),
                pid_ff::PID_EFFECT_OPERATION_REPORT_ID => slot.pid.effect_operation(data),
                pid_ff::PID_BLOCK_FREE_REPORT_ID => slot.pid.block_free(data),
                pid_ff::PID_DEVICE_CONTROL_REPORT_ID => slot.pid.device_control(data),
                pid_ff::PID_DEVICE_GAIN_REPORT_ID => slot.pid.device_gain(data),
                _ => return (DriverStatus::InvalidDeviceRequest, None, false),
            };
            if !ok {
                return (DriverStatus::InvalidParameter, None, false);
            }
            Self::publish_pid_rumble(slot);
            let arm = slot.pid.needs_tick();
            return (DriverStatus::Success, None, arm);
        }

        if report_id == generic::GENERIC_OUTPUT_REPORT_ID
            && matches!(profile, Profile::GenericHid | Profile::GenericPid)
        {
            if let Some(ev) = generic::encode_generic_feedback(slot.controller_id, data) {
                slot.feedback = ev;
                slot.feedback_pending = true;
                return (DriverStatus::Success, None, false);
            }
            return (DriverStatus::InvalidParameter, None, false);
        }

        (DriverStatus::InvalidDeviceRequest, None, false)
    }

    fn publish_pid_rumble(slot: &mut ControllerSlot) {
        let rumble = slot.pid.rumble();
        if slot.pid_rumble_valid && slot.pid_rumble == rumble {
            return;
        }
        slot.pid_rumble = rumble;
        slot.pid_rumble_valid = true;
        let payload = GenericRumbleRgbFeedback {
            low_frequency: rumble.low_frequency,
            high_frequency: rumble.high_frequency,
            red: 0,
            green: 0,
            blue: 0,
            reserved: 0,
        };
        slot.feedback = FeedbackEvent::new(
            slot.controller_id,
            FeedbackType::GenericRumble,
            &payload.to_bytes(),
        );
        slot.feedback_pending = true;
    }

    pub fn pid_tick(&mut self) -> bool {
        let mut still_playing = false;
        for slot in &mut self.slots {
            if slot.state != SlotState::Active || !slot.force_feedback {
                continue;
            }
            slot.pid.advance(PID_TICK_MS);
            Self::publish_pid_rumble(slot);
            still_playing = still_playing || slot.pid.needs_tick();
        }
        still_playing && !self.stopping
    }

    pub fn any_pid_playing(&self) -> bool {
        !self.stopping
            && self.slots.iter().any(|slot| {
                slot.state == SlotState::Active && slot.force_feedback && slot.pid.needs_tick()
            })
    }

    /// Collects SC2026 keep-alive report submissions under `state_lock`.
    /// Returns `(submits, count, any_active, any_ready)`.
    pub fn sc26_keepalive_collect(
        &mut self,
        now_us: u64,
    ) -> (
        [Option<PendingSubmit>; MAX_CONTROLLERS as usize],
        usize,
        bool,
        bool,
    ) {
        let mut pending = [None; MAX_CONTROLLERS as usize];
        let mut count = 0usize;
        let mut any_active = false;
        let mut any_ready = false;

        if !self.stopping {
            for idx in 0..MAX_CONTROLLERS as usize {
                let slot = &mut self.slots[idx];
                if slot.state != SlotState::Active
                    || !slot.selected_profile.is_some_and(is_steam_controller)
                    || slot.vhf.is_none()
                    || !slot.have_last_input
                {
                    continue;
                }
                any_active = true;
                if !slot.pump.ready() {
                    slot.sc26.last_report_us = now_us;
                    continue;
                }
                any_ready = true;
                let battery_due = now_us >= slot.sc26.last_battery_us
                    && now_us - slot.sc26.last_battery_us >= SC26_BATTERY_PERIOD_US;
                if battery_due {
                    let battery = steam_controller::encode_battery(&slot.sc26);
                    let _ = slot.pump.enqueue(
                        &battery,
                        steam_controller::BATTERY_REPORT_ID,
                        ReportKind::Aside,
                    );
                    slot.sc26.last_battery_us = now_us;
                }
                let resend_due = now_us >= slot.sc26.last_report_us
                    && now_us - slot.sc26.last_report_us >= SC26_RESEND_AFTER_US;
                if !resend_due && !battery_due {
                    continue;
                }
                if resend_due {
                    steam_controller::sc26_tick(&mut slot.sc26, now_us);
                    let report = steam_controller::encode_input(&slot.last_input, &mut slot.sc26);
                    let _ = slot.pump.enqueue(
                        &report,
                        steam_controller::INPUT_REPORT_ID,
                        ReportKind::Continuous,
                    );
                }
                if let Some((report_id, length, vhf, is_sc26)) = Self::take_and_stamp(slot, now_us)
                {
                    pending[count] = Some(PendingSubmit {
                        controller_id: idx as u32,
                        vhf,
                        report_id,
                        length,
                        rearm_keepalive: is_sc26,
                    });
                    count += 1;
                }
            }
        }

        if !any_active {
            self.sc26_timer_running = false;
            self.sc26_timer_parked = false;
        } else {
            self.sc26_timer_parked = !any_ready;
        }

        (pending, count, any_active, any_ready)
    }

    pub fn sc26_next_deadline_us(&self, now_us: u64) -> u64 {
        let tick_us = u64::from(SC26_TICK_MS) * 1000;
        let mut delay_us = tick_us;
        for slot in &self.slots {
            if slot.state != SlotState::Active
                || !slot.selected_profile.is_some_and(is_steam_controller)
                || slot.vhf.is_none()
                || !slot.have_last_input
            {
                continue;
            }
            let elapsed = now_us.saturating_sub(slot.sc26.last_report_us);
            let remaining = if elapsed >= tick_us {
                1
            } else {
                tick_us - elapsed
            };
            if remaining < delay_us {
                delay_us = remaining;
            }
        }
        delay_us
    }

    /// Retires active VHF handles on D0 exit, preserving `(owner, selected_profile)` as `Lost`.
    pub fn begin_power_down(&mut self) -> [Option<VhfHandleToken>; MAX_CONTROLLERS as usize] {
        self.powered = false;
        self.sc26_timer_running = false;
        self.sc26_timer_parked = false;
        let mut handles = [None; MAX_CONTROLLERS as usize];
        for (i, slot) in self.slots.iter_mut().enumerate() {
            if slot.state == SlotState::Active && slot.vhf.is_some() {
                slot.state = SlotState::Stopping;
                slot.feedback_pending = false;
                handles[i] = slot.vhf.take();
            }
        }
        handles
    }

    pub fn finish_power_down(
        &mut self,
        retired: &[Option<VhfHandleToken>; MAX_CONTROLLERS as usize],
    ) {
        for (i, slot) in self.slots.iter_mut().enumerate() {
            if retired[i].is_some() && slot.state == SlotState::Stopping {
                let owner = slot.owner;
                let profile = slot.selected_profile;
                slot.reset();
                slot.owner = owner;
                slot.selected_profile = profile;
                slot.state = SlotState::Lost;
            }
        }
    }

    pub fn begin_stop_all(&mut self) -> [Option<VhfHandleToken>; MAX_CONTROLLERS as usize] {
        self.stopping = true;
        self.powered = false;
        self.sc26_timer_running = false;
        self.sc26_timer_parked = false;
        let mut handles = [None; MAX_CONTROLLERS as usize];
        for (i, slot) in self.slots.iter_mut().enumerate() {
            slot.feedback_pending = false;
            slot.haptic_queue.clear();
            slot.settings_queue.clear();
            match slot.state {
                SlotState::Active | SlotState::Starting => {
                    slot.state = SlotState::Stopping;
                    handles[i] = slot.vhf.take();
                }
                SlotState::Stopping => {
                    handles[i] = slot.vhf.take();
                }
                SlotState::Lost => {
                    slot.reset();
                }
                SlotState::Empty => {}
            }
        }
        handles
    }

    pub fn finish_stop_slot(&mut self, controller_id: u32) {
        if let Some(slot) = self.slots.get_mut(controller_id as usize) {
            slot.reset();
        }
    }
}

/// Checks if `instance_id` (e.g. `"RubylightGamepad0"`) appears as an `&<instance>` suffix
/// in a double-null-terminated `CM_Get_Device_ID_ListW("HID", ...)` buffer.
pub fn matches_cm_child_instance_list(utf16_multi_sz: &[u16], target_instance_id: &str) -> bool {
    let len = utf16_multi_sz.len();
    if len == 0 || utf16_multi_sz[len - 1] != 0 || (len > 1 && utf16_multi_sz[len - 2] != 0) {
        // Malformed CM buffer must fail closed (`present = true`).
        return true;
    }
    let mut i = 0usize;
    while i < len && utf16_multi_sz[i] != 0 {
        let start = i;
        while i < len && utf16_multi_sz[i] != 0 {
            i += 1;
        }
        let item = &utf16_multi_sz[start..i];
        if let Some(amp_pos) = item.iter().rposition(|&c| c == u16::from(b'&')) {
            let suffix = &item[amp_pos + 1..];
            if ascii_eq_ignore_case(suffix, target_instance_id.as_bytes()) {
                return true;
            }
        }
        i += 1;
    }
    false
}

fn ascii_eq_ignore_case(utf16: &[u16], ascii: &[u8]) -> bool {
    if utf16.len() != ascii.len() {
        return false;
    }
    utf16
        .iter()
        .zip(ascii.iter())
        .all(|(&w, &a)| w <= 0x7F && (w as u8).eq_ignore_ascii_case(&a))
}

/// Kept for `ReportBuffer` import cleanliness across crates.
pub const fn max_report_bytes() -> usize {
    let _ = size_of::<ReportBuffer>();
    MAX_REPORT_BYTES
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{SteamHapticFeedback, sc_attr_flags};

    #[test]
    fn lifecycle_create_power_loss_recreate_and_counters() {
        let mut dev = DeviceState::new();
        dev.powered = true;
        let owner: OwnerToken = 101;
        let stranger: OwnerToken = 202;

        // Reserved profiles fail closed.
        for bad_prof in [Profile::GenericHid, Profile::GenericPid, Profile::Xbox360] {
            let req = CreateControllerRequest::new(0, bad_prof as u16);
            assert_eq!(
                dev.check_create_preflight(owner, &req),
                Err(DriverStatus::NotSupported)
            );
        }

        // Create DualSense on slot 0.
        let req = CreateControllerRequest::new(0, Profile::DualSense as u16);
        let (_, generation) = dev.check_create_preflight(owner, &req).unwrap();
        let plan = dev.begin_create(owner, &req, generation, false).unwrap();
        assert!(plan.register_feature_callbacks);
        dev.publish_vhf(0, 0xABCD);
        assert!(dev.commit_start(0, owner, true));
        assert_eq!(dev.slots[0].state, SlotState::Active);

        // Stranger cannot touch slot 0; query_controller_info rejects stranger.
        let id_req = ControllerIdRequest::new(0);
        assert_eq!(
            dev.query_controller_info(stranger, &id_req),
            Err(DriverStatus::AccessDenied)
        );
        assert_eq!(
            dev.poll_feedback(stranger, &id_req),
            Err(DriverStatus::AccessDenied)
        );
        assert_eq!(
            dev.poll_feedback(owner, &id_req),
            Err(DriverStatus::NoMoreEntries)
        );

        // D0 exit retires handle and marks slot Lost while preserving owner.
        let retired = dev.begin_power_down();
        assert_eq!(retired[0], Some(0xABCD));
        dev.finish_power_down(&retired);
        assert_eq!(dev.slots[0].state, SlotState::Lost);
        assert!(!dev.powered);
        assert_eq!(
            dev.poll_feedback(owner, &id_req),
            Err(DriverStatus::DeviceRemoved)
        );
        assert_eq!(
            dev.check_create_preflight(owner, &req),
            Err(DriverStatus::DevicePoweredOff)
        );

        // D0 entry restores power; stranger still blocked, owner recreates.
        dev.powered = true;
        assert_eq!(
            dev.check_create_preflight(stranger, &req),
            Err(DriverStatus::DeviceBusy)
        );
        let (_, gen2) = dev.check_create_preflight(owner, &req).unwrap();
        assert!(dev.begin_create(owner, &req, gen2, false).is_ok());
        dev.publish_vhf(0, 0xBEEF);
        assert!(dev.commit_start(0, owner, true));

        // Cleanup via owner destroy.
        let vhf = dev.begin_destroy_checked(owner, 0).unwrap();
        assert_eq!(vhf, Some(0xBEEF));
        dev.finish_destroy_owned(0, owner);
        assert_eq!(dev.slots[0].state, SlotState::Empty);
        assert_eq!(
            dev.begin_destroy_checked(owner, 0),
            Err(DriverStatus::NotFound)
        );
    }

    #[test]
    fn steam_controller_settings_haptics_and_attributes_priority() {
        let mut dev = DeviceState::new();
        dev.powered = true;
        let owner = 55;
        let req = CreateControllerRequest::new(2, Profile::SteamController as u16);
        let (_, generation) = dev.check_create_preflight(owner, &req).unwrap();
        let _ = dev.begin_create(owner, &req, generation, false).unwrap();
        dev.publish_vhf(2, 0x2222);
        assert!(dev.commit_start(2, owner, true));

        // Forwarded real hardware attributes update feature replies, fail on non-SC26.
        let mut attr = SteamControllerAttributesRequest {
            controller_id: 2,
            flags: sc_attr_flags::FIRMWARE_BUILD_TIME | sc_attr_flags::SERIALS,
            firmware_build_time: 0x1122_3344,
            ..Default::default()
        };
        attr.unit_serial[..13].copy_from_slice(b"FXA9999999999");
        assert_eq!(dev.set_sc_attributes(owner, &attr), DriverStatus::Success);

        let mut get_buf = [0u8; 64];
        let mut get_unit = [0u8; 64];
        get_unit[0] = steam_controller::FEATURES_REPORT_ID;
        get_unit[1] = steam_controller::CMD_GET_STRING_ATTRIBUTE;
        get_unit[2] = 0x15;
        get_unit[3] = steam_controller::STRING_ATTR_UNIT_SERIAL;
        assert_eq!(
            dev.on_set_feature(2, steam_controller::FEATURES_REPORT_ID, &get_unit),
            DriverStatus::Success
        );
        assert_eq!(
            dev.on_get_feature(2, steam_controller::FEATURES_REPORT_ID, &mut get_buf)
                .unwrap(),
            64
        );
        assert_eq!(&get_buf[4..17], b"FXA9999999999");

        get_unit[3] = steam_controller::STRING_ATTR_BOARD_SERIAL;
        assert_eq!(
            dev.on_set_feature(2, steam_controller::FEATURES_REPORT_ID, &get_unit),
            DriverStatus::Success
        );
        assert_eq!(
            dev.on_get_feature(2, steam_controller::FEATURES_REPORT_ID, &mut get_buf)
                .unwrap(),
            64
        );
        assert_eq!(&get_buf[4..17], b"LVGSC26BOARD0");

        // Enqueue haptic + settings; settings must drain ahead of haptic.
        let haptic = [steam_controller::HAPTIC_PULSE_ID, 1, 0, 0, 0, 0, 0, 0];
        let (st, ack_sub, _) =
            dev.on_write_report(2, steam_controller::HAPTIC_PULSE_ID, &haptic, 100);
        assert_eq!(st, DriverStatus::Success);
        assert!(ack_sub.is_none()); // Not ready yet -> queued as Aside in pump

        let settings = [
            steam_controller::FEATURES_REPORT_ID,
            steam_controller::CMD_SET_SETTINGS_VALUES,
            3,
            9,
            0,
            0,
        ];
        assert_eq!(
            dev.on_set_feature(2, steam_controller::FEATURES_REPORT_ID, &settings),
            DriverStatus::Success
        );

        let id_req = ControllerIdRequest::new(2);
        let ev1 = dev.poll_feedback(owner, &id_req).unwrap();
        assert_eq!(ev1.feedback_type, FeedbackType::SteamSettings as u16);
        let ev2 = dev.poll_feedback(owner, &id_req).unwrap();
        assert_eq!(ev2.feedback_type, FeedbackType::SteamHaptic as u16);
        let parsed = SteamHapticFeedback::from_bytes(&ev2.payload).unwrap();
        assert_eq!(parsed.length, 8);
    }

    #[test]
    fn cm_instance_list_fails_closed_on_malformed_and_matches_case_insensitively() {
        assert!(matches_cm_child_instance_list(&[], "RubylightGamepad0"));
        assert!(matches_cm_child_instance_list(
            &[b'a' as u16],
            "RubylightGamepad0"
        ));
        let mut list = [0u16; 48];
        for (dst, src) in list
            .iter_mut()
            .zip("HID\\VID_054C&PID_0CE6\\0&0000&RUBYLIGHTGAMEPAD3\0\0".encode_utf16())
        {
            *dst = src;
        }
        assert!(matches_cm_child_instance_list(&list, "RubylightGamepad3"));
        assert!(!matches_cm_child_instance_list(&list, "RubylightGamepad0"));
        assert!(!matches_cm_child_instance_list(&list, "RubylightGamepad30"));
    }

    #[test]
    fn ntstatus_codes_and_keepalive_parking_and_pid_playing_match_contract() {
        assert_eq!(DriverStatus::Success.ntstatus(), 0x0000_0000u32 as i32);
        assert!(DriverStatus::Success.is_success());
        assert_eq!(
            DriverStatus::InvalidBufferSize.ntstatus(),
            0xC000_0206u32 as i32
        );
        assert_eq!(
            DriverStatus::DevicePoweredOff.ntstatus(),
            0x8000_000Fu32 as i32
        );
        assert_eq!(DriverStatus::DeviceBusy.ntstatus(), 0x8000_0011u32 as i32);
        assert_eq!(
            DriverStatus::NoMoreEntries.ntstatus(),
            0x8000_001Au32 as i32
        );
        for non_success in [
            DriverStatus::NotSupported,
            DriverStatus::InvalidParameter,
            DriverStatus::InvalidDeviceRequest,
            DriverStatus::AccessDenied,
            DriverStatus::BufferTooSmall,
            DriverStatus::InvalidBufferSize,
            DriverStatus::DeviceNotReady,
            DriverStatus::DevicePoweredOff,
            DriverStatus::DeviceBusy,
            DriverStatus::Cancelled,
            DriverStatus::NoMoreEntries,
            DriverStatus::NotFound,
            DriverStatus::DeviceRemoved,
        ] {
            assert!(!non_success.is_success(), "{non_success:?}");
        }

        // Steam Controller parks when nobody reads, unparks once on ready, and clears on destroy.
        let mut dev = DeviceState::new();
        dev.powered = true;
        let owner = 99;
        let create = CreateControllerRequest::new(0, Profile::SteamController as u16);
        let (_, gen_num) = dev.check_create_preflight(owner, &create).unwrap();
        let _ = dev.begin_create(owner, &create, gen_num, false).unwrap();
        dev.publish_vhf(0, 0xCAFE);
        assert!(dev.commit_start(0, owner, true));

        let input = InputStateRequest {
            controller_id: 0,
            ..Default::default()
        };
        let (pending, start) = dev.submit_input_state(owner, &input, 10_000, true).unwrap();
        assert!(pending.is_none());
        assert!(start);
        assert!(dev.sc26_timer_running);

        let (_, count, any_active, any_ready) = dev.sc26_keepalive_collect(20_000);
        assert_eq!(count, 0);
        assert!(any_active);
        assert!(!any_ready);
        assert!(dev.sc26_timer_parked);

        let (ready_pending, unpark) = dev.on_vhf_ready_for_next_report(0, 21_000);
        assert!(ready_pending.is_some());
        assert!(unpark);
        assert!(!dev.sc26_timer_parked);

        let (_, unpark_again) = dev.on_vhf_ready_for_next_report(0, 22_000);
        assert!(!unpark_again);

        assert!(!dev.any_pid_playing());
        dev.slots[0].force_feedback = true;
        assert!(dev.slots[0].pid.create_new_effect(&[
            pid_ff::PID_CREATE_NEW_EFFECT_REPORT_ID,
            1,
            0,
            0
        ]));
        assert!(dev.slots[0].pid.effect_operation(&[
            pid_ff::PID_EFFECT_OPERATION_REPORT_ID,
            1,
            1,
            1
        ]));
        assert!(dev.any_pid_playing());
        dev.stopping = true;
        assert!(!dev.any_pid_playing());
    }
}
