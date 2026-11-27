//! Controller state submission, device-loss recreation, back-button refresh,
//! and profile-specific button mapping for [`Gamepads`].

use super::*;

impl Gamepads {
    fn submit(
        &mut self,
        id: u16,
        buttons: u32,
        left: u8,
        right: u8,
        sticks: &[i16; 4],
    ) -> Result<()> {
        if let Some(pad) = self.dualsenses.get(&id) {
            pad.apply(&Event::Controller {
                id,
                active: 1 << id,
                buttons,
                left_trigger: left,
                right_trigger: right,
                sticks: *sticks,
            });
            return Ok(());
        }
        let buttons = profile_button_word(self.profiles.get(&id).copied(), buttons);
        let buttons = self.profile_buttons(id, buttons);
        self.backend
            .submit(self.slot(id)?, buttons, left, right, sticks)
    }
    /// Sends a state, creating the pad again when the driver reports its
    /// device removed, as the VHF driver does after the machine's sleep. A
    /// driver that stays gone is asked once a second for this pad.
    pub(super) fn submit_state(
        &mut self,
        id: u16,
        buttons: u32,
        left: u8,
        right: u8,
        sticks: &[i16; 4],
    ) -> Result<()> {
        let Err(error) = self.submit(id, buttons, left, right, sticks) else {
            return Ok(());
        };
        if !device_removed(&error) {
            return Err(error);
        }
        if !self.recreate_due(id, Instant::now()) {
            return Err(error);
        }
        self.recreate(id)?;
        self.submit(id, buttons, left, right, sticks)
    }
    /// Whether controller `id`'s pad may be created again at `now`: a driver
    /// that stays gone is asked [`RECREATE_RETRY`] apart for that pad, from
    /// the input path and the feedback poll alike.
    pub(super) fn recreate_due(&mut self, id: u16, now: Instant) -> bool {
        if self
            .recreated_at
            .get(&id)
            .is_some_and(|at| now.saturating_duration_since(*at) < RECREATE_RETRY)
        {
            return false;
        }
        self.recreated_at.insert(id, now);
        true
    }
    /// Forgets a pad the driver no longer holds, freeing its slot for another
    /// controller.
    pub(super) fn forget_pad(&mut self, id: u16, global: u16) {
        self.active.remove(&id);
        self.profiles.remove(&id);
        self.arrivals.remove(&id);
        self.steam_at_arrival.remove(&id);
        self.forget_steam_fallback(id);
        self.states.remove(&id);
        self.last_feedback.remove(&id);
        self.feedback_failures.remove(&id);
        self.recreated_at.remove(&id);
        self.dropped.remove(&id);
        self.pointers.retain(|(pad, _), _| u16::from(*pad) != id);
        self.unsupported_touchpads.remove(&(id as u8));
        SLOTS.lock().unwrap()[global as usize] = false;
    }
    /// Creates controller `id`'s pad again in the slot it owns. The VHF
    /// driver retires every HID child when Windows leaves D0 (sleep,
    /// hibernate) and then refuses input for the slot until its owner creates
    /// it again (`libvirtualgamepad` 0.1.0-beta.10's `protocol.h`), so a
    /// controller that was plugged when the PC slept plays again without
    /// reconnecting.
    ///
    /// This is that driver's contract. A ViGEmBus pad goes through the same
    /// `plug`, but was not seen to lose its target across resume: it is an
    /// emulated device, not one of this driver's HID children, so
    /// `ERROR_DEVICE_REMOVED` is not expected from it (a submit that carries
    /// it would still be retried here). The USB/IP pads (Steam Deck,
    /// DualSense, Switch 2 over usbip-win2) are real USB devices on the vhci
    /// bus, which Windows re-enumerates itself: they hold no driver slot to
    /// create again, so a lost attachment is outside this path.
    pub(super) fn recreate(&mut self, id: u16) -> Result<()> {
        let Some(&global) = self.active.get(&id) else {
            return Ok(());
        };
        let Some(&profile) = self.profiles.get(&id) else {
            return Ok(());
        };
        self.backend.plug(u32::from(global), profile)?;
        // Whatever the retired pad had pending belongs to a device Windows
        // has already removed.
        self.last_feedback.remove(&id);
        self.feedback_failures.remove(&id);
        if profile == VHF_STEAM_CONTROLLER {
            // The new slot starts from the driver's captured firmware details.
            self.send_steam_attributes(id);
        }
        tracing::info!(
            controller = id,
            slot = global,
            profile = profile_name(profile),
            "virtual controller re-created after the gamepad driver's device was removed"
        );
        Ok(())
    }
    /// The buttons as the pad's profile takes them: a Steam Controller's
    /// own, and on a ViGEmBus DualShock 4 Back also clicking the touchpad
    /// (Moonlight's touchpad button) unless `ds4_back_as_touchpad_click` is off.
    pub(super) fn profile_buttons(&self, id: u16, buttons: u32) -> u32 {
        match self.profiles.get(&id).copied() {
            Some(VHF_STEAM_CONTROLLER) => {
                let (_, capabilities) = self.arrivals.get(&id).copied().unwrap_or_default();
                steam_pad::buttons(buttons, capabilities)
            }
            Some(VIGEM_DS4) if self.policy.ds4_back_as_touchpad_click && buttons & 0x20 != 0 => {
                buttons | 0x10_0000
            }
            _ => buttons,
        }
    }
    /// The driver slot of a plugged-in pad. A pad whose re-plug failed has a
    /// state but no slot, and must not take the host down (panic = abort).
    pub(super) fn slot(&self, id: u16) -> Result<u32> {
        self.active
            .get(&id)
            .map(|global| u32::from(*global))
            .ok_or_else(|| anyhow::anyhow!("controller {id} is not plugged in"))
    }
    pub(in crate::input) fn refresh(&mut self) -> Result<()> {
        let attached = self.finish_attaches();
        let keepalive = self.backend.refresh();
        let mut updates = Vec::new();
        for (id, (event, back)) in &mut self.states {
            if !self.active.contains_key(id) && !self.dualsenses.contains_key(id) {
                continue;
            }
            if let Some(buttons) =
                back.poll(self.started.elapsed(), self.policy.back_button_timeout)
                && let Event::Controller {
                    left_trigger,
                    right_trigger,
                    sticks,
                    ..
                } = event
            {
                updates.push((*id, buttons, *left_trigger, *right_trigger, *sticks));
            }
        }
        // One pad's failure must not skip the others' back-button update: try
        // every pad and report the last error, as the input and feedback paths
        // treat each pad on its own.
        let mut last_error = None;
        for (id, buttons, left, right, sticks) in updates {
            if let Err(error) = self.submit_state(id, buttons, left, right, &sticks) {
                last_error = Some(error);
            }
        }
        let back = match last_error {
            Some(error) => Err(error),
            None => keepalive,
        };
        match back {
            Err(error) if gamepad_backend::driver_lost(&error) => Err(error),
            Err(error) => attached.and(Err(error)),
            Ok(()) => attached,
        }
    }
    /// Controllers whose client should be asked for motion, with their arrival
    /// capabilities: USB/IP pads whose async attachment just completed and any
    /// ViGEmBus DualShock 4 whose gyro went silent, when the client announced motion sensors.
    pub(in crate::input) fn motion_requests(&mut self) -> Vec<(u8, u16)> {
        let mut requests = std::mem::take(&mut self.pending_motion_requests);
        let slots = self.backend.motion_requests();
        if slots.is_empty() {
            return requests;
        }
        for (&id, global) in &self.active {
            if slots.contains(&u32::from(*global)) {
                let (_, capabilities) = self.arrivals.get(&id).copied().unwrap_or_default();
                if capabilities & 0x30 != 0
                    && !requests.iter().any(|(req_id, _)| *req_id == id as u8)
                {
                    requests.push((id as u8, capabilities));
                }
            }
        }
        requests
    }
    pub fn motion_supported(&self, id: u16) -> bool {
        self.decks.contains_key(&id)
            || self.dualsenses.contains_key(&id)
            || self.switch2s.contains_key(&id)
            || matches!(
                self.profiles.get(&id).copied(),
                Some(5..=7 | VHF_STEAM_CONTROLLER | VIGEM_DS4)
            )
    }
}

/// The button word a pad's profile gets. Only the Switch 2 Pro Controller has a
/// C button, and its flag (0x40_0000) is also a Steam Controller's left grip
/// touch (`rubylight_protocol::steam_controller::LEFT_GRIP_TOUCH_FLAG`): the
/// Steam Controller's pad must get the bit, or its left grip never registers
/// (nightjar#125).
pub(super) fn profile_button_word(profile: Option<u16>, buttons: u32) -> u32 {
    if profile == Some(VHF_STEAM_CONTROLLER) {
        buttons
    } else {
        buttons & !butterpollo_input_policy::switch2::BUTTON_C
    }
}
