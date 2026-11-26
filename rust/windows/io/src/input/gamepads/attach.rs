//! Virtual gamepad profile selection, USB/IP attachment, and fallback warnings for [`Gamepads`].

use super::*;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(in crate::input) enum AttachKind {
    Deck,
    Switch2,
    DualSense,
}

pub(super) enum AttachOutcome {
    SteamNotRunning,
    Deck(Result<steam_deck_pad::DeckPad>),
    Switch2(Result<switch2_pad::Switch2Pad>),
    DualSense(Result<dualsense_pad::DualSensePad>),
}

pub(in crate::input) struct CompletedAttach {
    kind: AttachKind,
    id: u16,
    generation: u64,
    outcome: AttachOutcome,
}

impl Gamepads {
    fn next_generation(&mut self) -> u64 {
        self.next_attach_gen += 1;
        self.next_attach_gen
    }

    fn spawn_attach(
        &mut self,
        kind: AttachKind,
        id: u16,
        work: impl FnOnce() -> AttachOutcome + Send + 'static,
    ) -> Result<()> {
        let generation = self.next_generation();
        self.attaching.insert((kind, id), generation);
        let attaches = std::sync::Arc::clone(&self.attaches);
        let wake = self.wake.clone();
        std::thread::Builder::new()
            .name("usbip-attach".into())
            .spawn(move || {
                let outcome = work();
                attaches.lock().unwrap().push_back(CompletedAttach {
                    kind,
                    id,
                    generation,
                    outcome,
                });
                if let Some(wake) = wake {
                    wake();
                }
            })
            .map(|_| ())
            .map_err(|error| {
                self.attaching.remove(&(kind, id));
                anyhow::anyhow!(error).context("could not spawn USB/IP attachment thread")
            })
    }

    /// Removes controller `id`'s temporary VHF pad before switching to a USB/IP pad.
    /// A failed non-driver-lost unplug strands the VHF slot rather than failing
    /// and leaving the controller stuck in an attach/detach loop.
    pub(super) fn retire_vhf_for_usbip(&mut self, id: u16) {
        let Some(global) = self.active.get(&id).copied() else {
            return;
        };
        self.active.remove(&id);
        self.recreated_at.remove(&id);
        self.dropped.remove(&id);
        self.profiles.remove(&id);
        self.states.remove(&id);
        self.last_feedback.remove(&id);
        self.feedback_failures.remove(&id);
        self.pointers.retain(|(pad, _), _| u16::from(*pad) != id);
        match self.backend.unplug(u32::from(global)) {
            Ok(()) => {
                SLOTS.lock().unwrap()[global as usize] = false;
            }
            Err(error) => {
                if gamepad_backend::driver_lost(&error) {
                    self.lost = true;
                    SLOTS.lock().unwrap()[global as usize] = false;
                } else {
                    STRANDED.lock().unwrap().insert(global);
                    tracing::warn!(
                        controller = id,
                        slot = global,
                        error = format!("{error:#}"),
                        "virtual controller removal failed while switching to USB/IP pad"
                    );
                }
            }
        }
    }

    /// Applies any USB/IP pads whose background attachment finished since the last tick.
    pub(super) fn finish_attaches(&mut self) -> Result<()> {
        use butterpollo_input_policy::input_policy::{
            USBIP_DUALSENSE, USBIP_SWITCH2, VHF_DUALSENSE,
        };
        let completed: Vec<_> = self.attaches.lock().unwrap().drain(..).collect();
        let mut forced_error = None;
        for item in completed {
            let current = self.attaching.get(&(item.kind, item.id)).copied();
            if current != Some(item.generation) {
                match item.outcome {
                    AttachOutcome::Deck(Ok(pad)) => {
                        std::thread::spawn(move || drop(pad));
                    }
                    AttachOutcome::Switch2(Ok(pad)) => {
                        std::thread::spawn(move || drop(pad));
                    }
                    AttachOutcome::DualSense(Ok(pad)) => {
                        std::thread::spawn(move || drop(pad));
                    }
                    _ => {}
                }
                continue;
            }
            self.attaching.remove(&(item.kind, item.id));
            let id = item.id;
            match item.outcome {
                AttachOutcome::SteamNotRunning => {
                    tracing::info!(
                        controller = id,
                        "Steam is not running; the Steam Deck's controls use a virtual pad"
                    );
                    self.deck_failed.insert(id);
                }
                AttachOutcome::Deck(Ok(deck)) => {
                    self.warnings.clear("input_steam_deck");
                    tracing::info!(
                        controller = id,
                        port = deck.port(),
                        "Steam Deck controller attached through usbip-win2"
                    );
                    self.retire_vhf_for_usbip(id);
                    self.forget_steam_fallback(id);
                    self.decks.insert(id, deck);
                }
                AttachOutcome::Deck(Err(error)) => {
                    self.warnings.set("input_steam_deck", format!("Steam Deck controller unavailable ({error:#}); the Deck's controls arrive as a virtual pad. Check that usbip-win2 is installed and working, then reconnect."));
                    self.deck_failed.insert(id);
                }
                AttachOutcome::Switch2(Ok(pad)) => {
                    self.warnings.clear("input_switch2");
                    tracing::info!(
                        controller = id,
                        port = pad.port(),
                        "Switch 2 Pro Controller attached through usbip-win2"
                    );
                    self.retire_vhf_for_usbip(id);
                    self.forget_steam_fallback(id);
                    self.switch2s.insert(id, pad);
                }
                AttachOutcome::Switch2(Err(error)) => {
                    let forced = self.profile == USBIP_SWITCH2;
                    self.switch2_failed.insert(id);
                    let instead = if forced {
                        "the controller does not work until it does"
                    } else {
                        "the controller arrives as a Switch Pro controller instead"
                    };
                    tracing::warn!(
                        controller = id,
                        error = format!("{error:#}"),
                        "Switch 2 Pro Controller attachment failed"
                    );
                    self.warnings.set("input_switch2", format!("Switch 2 Pro Controller unavailable ({error:#}); {instead}. Install usbip-win2 on the host and check that it works, then reconnect."));
                    if forced {
                        forced_error.get_or_insert(error);
                    }
                }
                AttachOutcome::DualSense(Ok(pad)) => {
                    self.retire_vhf_for_usbip(id);
                    if let Some((kind, caps)) = self.arrivals.get(&id).copied() {
                        pad.capabilities(caps);
                        self.note_steam_controls(id, kind, caps, VHF_DUALSENSE);
                    }
                    self.dualsenses.insert(id, pad);
                    self.warnings.clear("input_dualsense");
                    tracing::info!(
                        controller = id,
                        "Composite DualSense HID and audio attached through usbip-win2"
                    );
                }
                AttachOutcome::DualSense(Err(error)) => {
                    let forced = self.profile == USBIP_DUALSENSE;
                    self.dualsense_failed.insert(id);
                    self.warnings.set("input_dualsense", format!("DualSense waveform haptics unavailable ({error:#}). Install usbip-win2 on the host and reconnect. Automatic controller selection keeps ordinary rumble and adaptive triggers through the VHF driver."));
                    if forced {
                        forced_error.get_or_insert(error);
                    }
                }
            }
        }
        forced_error.map_or(Ok(()), Err)
    }

    /// Attaches a real Steam Deck controller for a Steam Deck client when
    /// the `steam_deck_controller` setting and the host allow it. Returns
    /// whether controller `id` is one.
    pub(super) fn ensure_deck(&mut self, id: u16) -> Result<bool> {
        use butterpollo_input_policy::input_policy::{
            CTYPE_STEAM, DeckPad, USBIP_DUALSENSE, USBIP_SWITCH2, VIGEM_X360,
        };
        // A pad chosen by name is the one every client gets.
        if matches!(
            self.profile,
            USBIP_DUALSENSE | USBIP_SWITCH2 | VIGEM_X360 | VIGEM_DS4 | VHF_STEAM_CONTROLLER
        ) {
            return Ok(false);
        }
        if self.decks.contains_key(&id) {
            return Ok(true);
        }
        let steam = self
            .arrivals
            .get(&id)
            .is_some_and(|(kind, _)| *kind == CTYPE_STEAM);
        if !steam
            || self.deck_failed.contains(&id)
            || self.switch2s.contains_key(&id)
            || self.attaching.contains_key(&(AttachKind::Switch2, id))
            || self.attaching.contains_key(&(AttachKind::Deck, id))
        {
            return Ok(false);
        }
        let mode = self.policy.steam_deck;
        if mode == DeckPad::VirtualPad {
            self.deck_failed.insert(id);
            return Ok(false);
        }
        #[cfg(test)]
        if let Some(hook) = &self.test_attach_hook {
            let hook = std::sync::Arc::clone(hook);
            if let Err(error) = self.spawn_attach(AttachKind::Deck, id, move || {
                AttachOutcome::Deck(
                    hook(AttachKind::Deck, id)
                        .and_then(|()| anyhow::bail!("simulated Deck attach hook")),
                )
            }) {
                self.warnings.set("input_steam_deck", format!("Steam Deck controller unavailable ({error:#}); the Deck's controls arrive as a virtual pad."));
                self.deck_failed.insert(id);
            }
            return Ok(false);
        }
        let Some(exe) = steam_deck_pad::usbip_exe() else {
            if mode == DeckPad::SteamDeck {
                self.warnings.set("input_steam_deck", "Steam Deck controller unavailable: usbip-win2 is not installed, so the Deck's controls arrive as a virtual pad. Install usbip-win2 on the host to pass the Deck through as a Steam Deck, then reconnect.");
            }
            self.deck_failed.insert(id);
            return Ok(false);
        };
        if let Err(error) = self.spawn_attach(AttachKind::Deck, id, move || {
            if mode == DeckPad::Auto && !steam_deck_pad::steam_running() {
                return AttachOutcome::SteamNotRunning;
            }
            AttachOutcome::Deck(
                Slot::reserve().and_then(|slot| steam_deck_pad::DeckPad::plug(&exe, slot)),
            )
        }) {
            self.warnings.set("input_steam_deck", format!("Steam Deck controller unavailable ({error:#}); the Deck's controls arrive as a virtual pad. Check that usbip-win2 is installed and working, then reconnect."));
            self.deck_failed.insert(id);
        }
        Ok(false)
    }
    /// Attaches a Switch 2 Pro Controller over usbip-win2 for a client's
    /// Switch 2 Pro Controller, or for every controller when `gamepad =
    /// usbip_switch2` chose it. Returns whether controller `id` is one.
    pub(super) fn ensure_switch2(&mut self, id: u16) -> Result<bool> {
        use butterpollo_input_policy::input_policy::{USBIP_SWITCH2, wants_switch2};
        if self.switch2s.contains_key(&id) {
            return Ok(true);
        }
        if !wants_switch2(self.profile, self.identities.get(&id).copied()) {
            return Ok(false);
        }
        let forced = self.profile == USBIP_SWITCH2;
        if self.switch2_failed.contains(&id) {
            if forced {
                bail!(
                    "Switch 2 Pro Controller USB attachment failed; reconnect after checking usbip-win2"
                );
            }
            return Ok(false);
        }
        if self.attaching.contains_key(&(AttachKind::Switch2, id)) {
            return Ok(forced);
        }
        #[cfg(test)]
        if let Some(hook) = &self.test_attach_hook {
            let hook = std::sync::Arc::clone(hook);
            if let Err(error) = self.spawn_attach(AttachKind::Switch2, id, move || {
                AttachOutcome::Switch2(
                    hook(AttachKind::Switch2, id)
                        .and_then(|()| anyhow::bail!("simulated Switch2 attach hook")),
                )
            }) {
                self.switch2_failed.insert(id);
                if forced {
                    return Err(error);
                }
            }
            return Ok(forced);
        }
        let Some(exe) = steam_deck_pad::usbip_exe() else {
            let error = anyhow::anyhow!("usbip-win2 is not installed");
            self.switch2_failed.insert(id);
            let instead = if forced {
                "the controller does not work until it does"
            } else {
                "the controller arrives as a Switch Pro controller instead"
            };
            tracing::warn!(
                controller = id,
                error = format!("{error:#}"),
                "Switch 2 Pro Controller attachment failed"
            );
            self.warnings.set("input_switch2", format!("Switch 2 Pro Controller unavailable ({error:#}); {instead}. Install usbip-win2 on the host and check that it works, then reconnect."));
            if forced {
                return Err(error);
            }
            return Ok(false);
        };
        if let Err(error) = self.spawn_attach(AttachKind::Switch2, id, move || {
            AttachOutcome::Switch2(
                Slot::reserve().and_then(|slot| switch2_pad::Switch2Pad::plug(&exe, slot)),
            )
        }) {
            self.switch2_failed.insert(id);
            if forced {
                return Err(error);
            }
        }
        Ok(forced)
    }
    /// Whether Steam was running when controller `id` connected, for the policy's choice of
    /// pad. Asked once per connection, and only for on-screen Steam Controller controls under
    /// Automatic, in an app started through Steam, on a driver that has the Steam Controller,
    /// so the host's processes are not listed for every pad.
    fn steam_at_arrival(&mut self, id: u16, kind: u8, capabilities: u16) -> bool {
        if self.profile != 0
            || !self.policy.steam_app
            || self.available & (1 << (VHF_STEAM_CONTROLLER - 1)) == 0
            || !butterpollo_input_policy::input_policy::steam_controls(kind, capabilities)
        {
            return false;
        }
        let running = self.steam_running;
        *self.steam_at_arrival.entry(id).or_insert_with(running)
    }
    /// Once controller `id`'s arrival has its pad (`profile`; a USB/IP DualSense counts as the
    /// DualSense): warns when its on-screen Steam Controller controls got another pad than the
    /// Steam Controller in an app started through Steam, and clears the warning once no
    /// controller is left so. Logs why, once per arrival.
    pub(super) fn note_steam_controls(
        &mut self,
        id: u16,
        kind: u8,
        capabilities: u16,
        profile: u16,
    ) {
        let fallback = steam_controls_fallback(
            self.profile,
            kind,
            capabilities,
            self.policy.steam_app,
            self.available,
            profile,
        );
        if let Some(warning) = fallback {
            tracing::info!(
                controller = id,
                driver_has_steam_controller =
                    self.available & (1 << (VHF_STEAM_CONTROLLER - 1)) != 0,
                // Asked only when the driver has the Steam Controller.
                steam_running = ?self.steam_at_arrival.get(&id),
                profile = profile_name(profile),
                "on-screen Steam Controller controls got no Steam Controller"
            );
            self.steam_fallbacks.insert(id);
            self.warnings.set("input_steam_controls", warning);
            return;
        }
        if self.profile == 0
            && !self.policy.steam_app
            && butterpollo_input_policy::input_policy::steam_controls(kind, capabilities)
        {
            tracing::info!(
                controller = id,
                profile = profile_name(profile),
                "on-screen Steam Controller controls get no Steam Controller: the stream's app does not start through Steam"
            );
        }
        self.forget_steam_fallback(id);
    }
    /// Controller `id` no longer has on-screen Steam Controller controls without their pad.
    pub(super) fn forget_steam_fallback(&mut self, id: u16) {
        if self.steam_fallbacks.remove(&id) && self.steam_fallbacks.is_empty() {
            self.warnings.clear("input_steam_controls");
        }
    }
    /// The controllers whose pad is a Steam Controller, a bit each: their back grips are the
    /// pad's own, whether `gamepad` or Automatic chose it.
    pub(in crate::input) fn steam_pads(&self) -> u16 {
        self.profiles
            .iter()
            .filter(|(_, profile)| **profile == VHF_STEAM_CONTROLLER)
            .fold(0, |mask, (id, _)| mask | (1 << id))
    }
    pub(super) fn ensure(&mut self, id: u16) -> Result<()> {
        if id >= 16 {
            bail!("controller ID out of range");
        }
        if self.ensure_dualsense(id)? {
            return Ok(());
        }
        let (kind, capabilities) = self.arrivals.get(&id).copied().unwrap_or_default();
        let steam = self.steam_at_arrival(id, kind, capabilities);
        let profile = self
            .policy
            .controller_profile(self.profile, kind, capabilities, self.available, steam)
            .ok_or_else(|| anyhow::anyhow!("gamepad driver has no supported controller profile"))?;
        if let Some(global) = self.active.get(&id).copied() {
            if self.profiles.get(&id) == Some(&profile) {
                return Ok(());
            }
            // Some clients send arrival capabilities after their first state.
            self.backend.unplug(u32::from(global))?;
            self.active.remove(&id);
            self.recreated_at.remove(&id);
            self.dropped.remove(&id);
            self.profiles.remove(&id);
            self.last_feedback.remove(&id);
            self.feedback_failures.remove(&id);
            self.pointers.retain(|(pad, _), _| u16::from(*pad) != id);
            SLOTS.lock().unwrap()[global as usize] = false;
        }
        if let Some(error) = reclaim_until_lost(&mut SLOTS.lock().unwrap(), |global| {
            self.backend.unplug(global)
        }) {
            return Err(error);
        }
        let every_slot = self.backend.every_slot_differs();
        let plugged = plug_in_free_slot(&SLOTS, every_slot, |global| {
            self.backend.plug(u32::from(global), profile)
        });
        // The driver's own reason, not slot exhaustion, when it refused.
        let global = match plugged {
            Ok(Some(global)) => global,
            Ok(None) => bail!("no free virtual controller slots"),
            Err(error) => {
                return Err(error.context(format!(
                    "the gamepad driver could not plug a {} controller",
                    profile_name(profile)
                )));
            }
        };
        self.active.insert(id, global);
        self.profiles.insert(id, profile);
        if let Some(warning) = switch_fallback(self.profile, kind, capabilities, profile) {
            self.warnings.set("input_switch", warning);
        }
        tracing::info!(
            controller = id,
            client_type = kind,
            capabilities = format!("{capabilities:#x}"),
            backend = self.backend.name(),
            profile = profile_name(profile),
            "virtual controller connected"
        );
        if profile == VHF_STEAM_CONTROLLER {
            tracing::warn!(
                controller = id,
                "virtual Steam Controller connected: never start a controller firmware update from Steam while streaming, it would target the virtual controller, not the real one"
            );
            self.send_steam_attributes(id);
        }
        Ok(())
    }
    /// Hands the client's real firmware attributes to its virtual Steam
    /// Controller, so Steam judges the real firmware, not the driver's
    /// captured defaults. A client without a real one gets a firmware build
    /// time Steam does not offer to update, since no update can reach a
    /// virtual controller. A driver that predates the request keeps its defaults.
    pub(super) fn send_steam_attributes(&mut self, id: u16) {
        let Some(global) = self.active.get(&id).copied() else {
            return;
        };
        if self.profiles.get(&id) != Some(&VHF_STEAM_CONTROLLER) {
            return;
        }
        let real = self.steam_attributes.get(&id).copied();
        let attributes = real.unwrap_or_else(|| steam_pad::virtual_attributes(id as u8));
        match self
            .backend
            .steam_attributes(u32::from(global), &attributes)
        {
            Ok(()) => tracing::info!(
                controller = id,
                firmware_build_time = format!("{:#x}", attributes.firmware_build_time),
                bootloader_build_time = format!("{:#x}", attributes.bootloader_build_time),
                board_revision = attributes.board_revision,
                fields = format!("{:#x}", attributes.valid_mask),
                transport = attributes.transport,
                source = if real.is_some() {
                    "the client's controller"
                } else {
                    "the host: no real Steam Controller, so Steam offers no firmware update"
                },
                "virtual Steam Controller firmware attributes set"
            ),
            Err(error)
                if error
                    .downcast_ref::<windows::core::Error>()
                    .is_some_and(|error| error.code() == ERROR_INVALID_FUNCTION.to_hresult()) =>
            {
                if !std::mem::replace(&mut self.steam_attributes_unsupported, true) {
                    tracing::info!(
                        controller = id,
                        "the installed gamepad driver predates Steam Controller attributes; the virtual controller keeps its defaults"
                    );
                }
            }
            Err(error) => {
                self.lost |= gamepad_backend::driver_lost(&error);
                tracing::warn!(
                    controller = id,
                    error = format!("{error:#}"),
                    "the gamepad driver refused the Steam Controller's attributes"
                );
            }
        }
    }
    fn ensure_dualsense(&mut self, id: u16) -> Result<bool> {
        use butterpollo_input_policy::{
            input_policy::{USBIP_DUALSENSE, VHF_DUALSENSE},
            ps_speaker::wants_usb_dualsense,
        };
        if self.dualsenses.contains_key(&id) {
            return Ok(true);
        }
        if self.switch2s.contains_key(&id)
            || self.decks.contains_key(&id)
            || self.attaching.contains_key(&(AttachKind::Switch2, id))
            || self.attaching.contains_key(&(AttachKind::Deck, id))
        {
            return Ok(false);
        }
        let forced = self.profile == USBIP_DUALSENSE;
        if self.dualsense_failed.contains(&id) {
            if forced {
                bail!("DualSense USB attachment failed; reconnect after checking usbip-win2");
            }
            return Ok(false);
        }
        let (kind, caps) = self.arrivals.get(&id).copied().unwrap_or_default();
        let steam = self.steam_at_arrival(id, kind, caps);
        let selected =
            self.policy
                .controller_profile(self.profile, kind, caps, self.available, steam);
        if !forced && (!wants_usb_dualsense(kind, caps) || selected != Some(VHF_DUALSENSE)) {
            return Ok(false);
        }
        if self.attaching.contains_key(&(AttachKind::DualSense, id)) {
            return Ok(forced);
        }
        #[cfg(test)]
        if let Some(hook) = &self.test_attach_hook {
            let hook = std::sync::Arc::clone(hook);
            if let Err(error) = self.spawn_attach(AttachKind::DualSense, id, move || {
                AttachOutcome::DualSense(
                    hook(AttachKind::DualSense, id)
                        .and_then(|()| anyhow::bail!("simulated DualSense attach hook")),
                )
            }) {
                self.dualsense_failed.insert(id);
                if forced {
                    return Err(error);
                }
            }
            return Ok(forced);
        }
        let Some(exe) = steam_deck_pad::usbip_exe() else {
            let error = anyhow::anyhow!("usbip-win2 is not installed");
            self.dualsense_failed.insert(id);
            self.warnings.set("input_dualsense", format!("DualSense waveform haptics unavailable ({error:#}). Install usbip-win2 on the host and reconnect. Automatic controller selection keeps ordinary rumble and adaptive triggers through the VHF driver."));
            if forced {
                return Err(error);
            }
            return Ok(false);
        };
        let gain = self.policy.dualsense_haptics_gain;
        if let Err(error) = self.spawn_attach(AttachKind::DualSense, id, move || {
            AttachOutcome::DualSense(
                Slot::reserve()
                    .and_then(|slot| dualsense_pad::DualSensePad::plug(&exe, slot, id, caps, gain)),
            )
        }) {
            self.dualsense_failed.insert(id);
            if forced {
                return Err(error);
            }
        }
        Ok(forced)
    }
}

pub(super) fn profile_name(profile: u16) -> &'static str {
    match profile {
        butterpollo_input_policy::input_policy::VIGEM_X360 => "vigem_x360",
        VIGEM_DS4 => "vigem_ds4",
        3 => "vhf_xbox_one",
        4 => "vhf_xbox",
        5 => "vhf_ds4",
        6 => "vhf_ds5",
        7 => "vhf_switch",
        VHF_STEAM_CONTROLLER => "vhf_steam",
        butterpollo_input_policy::input_policy::USBIP_SWITCH2 => "usbip_switch2",
        _ => "vhf",
    }
}

/// The session warning for a Nintendo controller (Moonlight type 3) that automatic
/// selection made another pad because the driver has no Switch Pro profile.
pub(super) fn switch_fallback(
    configured: u16,
    kind: u8,
    capabilities: u16,
    profile: u16,
) -> Option<String> {
    use butterpollo_input_policy::input_policy::{VHF_DS4, VHF_DUALSENSE, VHF_SWITCH};
    if configured != 0 || kind != 3 || profile == VHF_SWITCH {
        return None;
    }
    let pad = match profile {
        VHF_DUALSENSE => "a DualSense",
        VHF_DS4 => "a DualShock 4",
        _ => "an Xbox controller",
    };
    let gyro = if capabilities & 0x30 == 0 {
        ""
    } else if matches!(profile, VHF_DUALSENSE | VHF_DS4) {
        ", which keeps its gyro"
    } else {
        ", without its gyro"
    };
    Some(format!(
        "The gamepad driver has no Switch Pro profile, so the Nintendo controller plays as {pad}{gyro}. Run Rubylight setup again to update the gamepad driver, then reconnect."
    ))
}

/// The session warning for a controller carrying on-screen Steam Controller controls that
/// automatic selection gave another pad (`profile`) although the stream's app starts through
/// Steam (`steam_app`): the driver has no Steam Controller, or Steam was not running when the
/// controller connected. Any other app keeps its Xbox-type pad without a word.
pub(super) fn steam_controls_fallback(
    configured: u16,
    kind: u8,
    capabilities: u16,
    steam_app: bool,
    available: u32,
    profile: u16,
) -> Option<String> {
    use butterpollo_input_policy::input_policy::{VHF_DS4, VHF_DUALSENSE, steam_controls};
    if configured != 0
        || !steam_app
        || profile == VHF_STEAM_CONTROLLER
        || !steam_controls(kind, capabilities)
    {
        return None;
    }
    let pad = match profile {
        VHF_DUALSENSE => "a DualSense, which has a touchpad but no back buttons or grips",
        VHF_DS4 => "a DualShock 4, which has a touchpad but no back buttons or grips",
        _ => "an Xbox controller, on which the trackpads, back buttons and grip touch do nothing",
    };
    Some(if available & (1 << (VHF_STEAM_CONTROLLER - 1)) == 0 {
        format!(
            "Steam Controller unavailable: the gamepad driver has no Steam Controller profile, so the controller with the on-screen Steam Controller controls plays as {pad}. Run Rubylight setup again to update the gamepad driver, then reconnect."
        )
    } else {
        format!(
            "Steam Controller unavailable: Steam was not running when the controller with the on-screen Steam Controller controls connected, so it plays as {pad}. Start Steam, then reconnect."
        )
    })
}
