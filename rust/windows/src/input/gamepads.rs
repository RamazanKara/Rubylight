//! Virtual gamepad lifecycle, controller reports, and feedback.

use super::*;

pub fn open_interface(guid: GUID) -> Result<HANDLE> {
    // SAFETY: The sized SetupAPI structures and aligned detail buffer remain live through each call; the terminated device path is consumed before the device set is destroyed.
    unsafe {
        let set = SetupDiGetClassDevsW(
            Some(&guid),
            PCWSTR::null(),
            None,
            DIGCF_DEVICEINTERFACE | DIGCF_PRESENT,
        )?;
        let result = (|| -> Result<HANDLE> {
            let mut interface = SP_DEVICE_INTERFACE_DATA {
                cbSize: size_of::<SP_DEVICE_INTERFACE_DATA>() as u32,
                ..Default::default()
            };
            SetupDiEnumDeviceInterfaces(set, None, &guid, 0, &mut interface)?;
            let mut size = 0;
            let _ =
                SetupDiGetDeviceInterfaceDetailW(set, &interface, None, 0, Some(&mut size), None);
            if !(8..=65536).contains(&size) {
                bail!("invalid device interface path length");
            }
            let mut data = vec![0u64; (size as usize).div_ceil(8)];
            let detail = data.as_mut_ptr() as *mut SP_DEVICE_INTERFACE_DETAIL_DATA_W;
            (*detail).cbSize = size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;
            SetupDiGetDeviceInterfaceDetailW(set, &interface, Some(detail), size, None, None)?;
            Ok(CreateFileW(
                PCWSTR((*detail).DevicePath.as_ptr()),
                GENERIC_READ.0 | GENERIC_WRITE.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                None,
            )?)
        })();
        SetupDiDestroyDeviceInfoList(set)?;
        result
    }
}
pub(super) fn request(size: u32, id: Option<u32>) -> Vec<u8> {
    let mut b = size.to_le_bytes().to_vec();
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&0u16.to_le_bytes());
    if let Some(id) = id {
        b.extend_from_slice(&id.to_le_bytes());
    }
    b
}
pub struct Gamepads {
    backend: gamepad_backend::Backend,
    pub(super) active: BTreeMap<u16, u16>,
    profile: u16,
    available: u32,
    profiles: BTreeMap<u16, u16>,
    arrivals: BTreeMap<u16, (u8, u16)>,
    states: BTreeMap<u16, (Event, butterpollo_core::input_policy::BackButton)>,
    policy: butterpollo_core::input_policy::Policy,
    started: std::time::Instant,
    pointers: BTreeMap<(u8, u32), u8>,
    unsupported_touchpads: BTreeSet<u8>,
    /// The last feedback report forwarded for each controller.
    last_feedback: BTreeMap<u16, (u16, Vec<u8>)>,
    feedback_failures: BTreeMap<u16, FeedbackFailure>,
    /// Steam Deck clients' controllers attached as real Steam Decks.
    pub(super) decks: BTreeMap<u16, steam_deck_pad::DeckPad>,
    /// Controllers a Steam Deck could not be attached for; they keep their
    /// VHF pad until they arrive again.
    deck_failed: BTreeSet<u16>,
    pub(super) dualsenses: BTreeMap<u16, dualsense_pad::DualSensePad>,
    dualsense_failed: BTreeSet<u16>,
    warnings: std::sync::Arc<butterpollo_core::session::Warnings>,
}
#[derive(Default)]
struct FeedbackFailure {
    unreported: u64,
    reported: Option<Instant>,
}
static SLOTS: std::sync::Mutex<[bool; 16]> = std::sync::Mutex::new([false; 16]);
/// Reserve a host-wide identity until its USB attachment is fully detached.
pub(super) struct Slot(u16);
/// Driver slots whose pad would not unplug when its session's gamepads closed. They stay
/// reserved in [`SLOTS`], so no other pad is plugged over one the driver may still hold,
/// until a later unplug of them succeeds.
static STRANDED: std::sync::Mutex<BTreeSet<u16>> = std::sync::Mutex::new(BTreeSet::new());
/// Unplugs the stranded slots that will now, freeing them in `slots`.
fn reclaim(slots: &mut [bool; 16], mut unplug: impl FnMut(u32) -> Result<()>) {
    STRANDED
        .lock()
        .unwrap()
        .retain(|&global| match unplug(u32::from(global)) {
            Ok(()) => {
                tracing::info!(
                    slot = global,
                    "a virtual controller left by an earlier session was removed"
                );
                slots[global as usize] = false;
                false
            }
            Err(error) => {
                tracing::debug!(
                    slot = global,
                    error = format!("{error:#}"),
                    "a virtual controller left by an earlier session still will not unplug"
                );
                true
            }
        });
}
impl Slot {
    pub(super) fn reserve() -> Result<Self> {
        let mut slots = SLOTS.lock().unwrap();
        let id = slots
            .iter()
            .position(|used| !used)
            .context("no free controller slots")?;
        slots[id] = true;
        Ok(Self(id as u16))
    }
    pub(super) fn id(&self) -> u16 {
        self.0
    }
}
impl Drop for Slot {
    fn drop(&mut self) {
        SLOTS.lock().unwrap()[self.0 as usize] = false;
    }
}
impl Gamepads {
    pub(super) fn set_warnings(
        &mut self,
        warnings: std::sync::Arc<butterpollo_core::session::Warnings>,
    ) {
        self.warnings = warnings;
    }
    pub fn open(profile: u16) -> Result<Self> {
        Self::open_options(
            profile,
            butterpollo_core::input_policy::Policy::resolve(&Default::default())?,
            Default::default(),
        )
    }
    pub(super) fn open_options(
        profile: u16,
        policy: butterpollo_core::input_policy::Policy,
        warnings: std::sync::Arc<butterpollo_core::session::Warnings>,
    ) -> Result<Self> {
        let (backend, available) = gamepad_backend::Backend::open(profile)?;
        Ok(Self {
            backend,
            active: BTreeMap::new(),
            profile: if profile == VHF_AUTO { 0 } else { profile },
            available,
            profiles: BTreeMap::new(),
            arrivals: BTreeMap::new(),
            states: BTreeMap::new(),
            policy,
            started: std::time::Instant::now(),
            pointers: BTreeMap::new(),
            unsupported_touchpads: BTreeSet::new(),
            last_feedback: BTreeMap::new(),
            feedback_failures: BTreeMap::new(),
            decks: BTreeMap::new(),
            deck_failed: BTreeSet::new(),
            dualsenses: BTreeMap::new(),
            dualsense_failed: BTreeSet::new(),
            warnings,
        })
    }
    /// Attaches a real Steam Deck controller for a Steam Deck client when
    /// the `steam_deck_controller` setting and the host allow it. Returns
    /// whether controller `id` is one.
    fn ensure_deck(&mut self, id: u16) -> Result<bool> {
        use butterpollo_core::input_policy::{CTYPE_STEAM, DeckPad};
        if self.profile == butterpollo_core::input_policy::USBIP_DUALSENSE {
            return Ok(false);
        }
        if self.decks.contains_key(&id) {
            return Ok(true);
        }
        let steam = self
            .arrivals
            .get(&id)
            .is_some_and(|(kind, _)| *kind == CTYPE_STEAM);
        if !steam || self.deck_failed.contains(&id) {
            return Ok(false);
        }
        let mode = self.policy.steam_deck;
        let exe = match mode {
            DeckPad::VirtualPad => None,
            _ => steam_deck_pad::usbip_exe(),
        };
        let Some(exe) = exe else {
            if mode == DeckPad::SteamDeck {
                self.warnings.set("input_steam_deck", "Steam Deck controller unavailable: usbip-win2 is not installed, so the Deck's controls arrive as a virtual pad. Install usbip-win2 on the host to pass the Deck through as a Steam Deck, then reconnect.");
            }
            self.deck_failed.insert(id);
            return Ok(false);
        };
        if mode == DeckPad::Auto && !steam_deck_pad::steam_running() {
            tracing::info!(
                controller = id,
                "Steam is not running; the Steam Deck's controls use a virtual pad"
            );
            self.deck_failed.insert(id);
            return Ok(false);
        }
        let deck = match Slot::reserve().and_then(|slot| steam_deck_pad::DeckPad::plug(&exe, slot))
        {
            Ok(deck) => deck,
            Err(error) => {
                self.warnings.set("input_steam_deck", format!("Steam Deck controller unavailable ({error:#}); the Deck's controls arrive as a virtual pad. Check that usbip-win2 is installed and working, then reconnect."));
                self.deck_failed.insert(id);
                return Ok(false);
            }
        };
        self.warnings.clear("input_steam_deck");
        tracing::info!(
            controller = id,
            port = deck.port(),
            "Steam Deck controller attached through usbip-win2"
        );
        // A VHF pad plugged before the arrival made way for it.
        if let Some(global) = self.active.remove(&id) {
            self.backend.unplug(u32::from(global))?;
            self.profiles.remove(&id);
            self.states.remove(&id);
            self.last_feedback.remove(&id);
            self.feedback_failures.remove(&id);
            self.pointers.retain(|(pad, _), _| u16::from(*pad) != id);
            SLOTS.lock().unwrap()[global as usize] = false;
        }
        self.decks.insert(id, deck);
        Ok(true)
    }
    fn ensure(&mut self, id: u16) -> Result<()> {
        if id >= 16 {
            bail!("controller ID out of range");
        }
        if self.ensure_dualsense(id)? {
            return Ok(());
        }
        let (kind, capabilities) = self.arrivals.get(&id).copied().unwrap_or_default();
        let profile = self
            .policy
            .controller_profile(self.profile, kind, capabilities, self.available)
            .ok_or_else(|| anyhow::anyhow!("gamepad driver has no supported controller profile"))?;
        if let Some(global) = self.active.get(&id).copied() {
            if self.profiles.get(&id) == Some(&profile) {
                return Ok(());
            }
            // Some clients send arrival capabilities after their first state.
            self.backend.unplug(u32::from(global))?;
            self.active.remove(&id);
            self.profiles.remove(&id);
            self.last_feedback.remove(&id);
            self.feedback_failures.remove(&id);
            self.pointers.retain(|(pad, _), _| u16::from(*pad) != id);
            SLOTS.lock().unwrap()[global as usize] = false;
        }
        let mut slots = SLOTS.lock().unwrap();
        reclaim(&mut slots, |global| self.backend.unplug(global));
        let mut failed = None;
        for global in 0..16u16 {
            if slots[global as usize] {
                continue;
            }
            if let Err(error) = self.backend.plug(u32::from(global), profile) {
                failed = Some(error);
                continue;
            }
            slots[global as usize] = true;
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
            return Ok(());
        }
        // The driver's own reason, not slot exhaustion, when it refused every free slot.
        match failed {
            Some(error) => Err(error.context(format!(
                "the gamepad driver could not plug a {} controller",
                profile_name(profile)
            ))),
            None => bail!("no free virtual controller slots"),
        }
    }
    fn ensure_dualsense(&mut self, id: u16) -> Result<bool> {
        use butterpollo_core::{
            haptics,
            input_policy::{USBIP_DUALSENSE, VHF_DUALSENSE},
        };
        if self.dualsenses.contains_key(&id) {
            return Ok(true);
        }
        let forced = self.profile == USBIP_DUALSENSE;
        if self.dualsense_failed.contains(&id) {
            if forced {
                bail!("DualSense USB attachment failed; reconnect after checking usbip-win2");
            }
            return Ok(false);
        }
        let (kind, caps) = self.arrivals.get(&id).copied().unwrap_or_default();
        let selected = self
            .policy
            .controller_profile(self.profile, kind, caps, self.available);
        if !forced && (caps & haptics::CAPABILITY == 0 || selected != Some(VHF_DUALSENSE)) {
            return Ok(false);
        }
        let attached = (|| -> Result<_> {
            let exe = steam_deck_pad::usbip_exe().context("usbip-win2 is not installed")?;
            dualsense_pad::DualSensePad::plug(&exe, Slot::reserve()?, id, caps)
        })();
        let pad = match attached {
            Ok(pad) => pad,
            Err(error) => {
                self.dualsense_failed.insert(id);
                self.warnings.set("input_dualsense", format!("DualSense waveform haptics unavailable ({error:#}). Install usbip-win2 on the host and reconnect. Automatic controller selection keeps ordinary rumble and adaptive triggers through the VHF driver."));
                if forced {
                    return Err(error);
                }
                return Ok(false);
            }
        };
        // Arrival may follow the first state; replace the temporary VHF pad.
        if let Some(global) = self.active.get(&id).copied() {
            self.backend.unplug(u32::from(global))?;
            self.active.remove(&id);
            SLOTS.lock().unwrap()[global as usize] = false;
            self.profiles.remove(&id);
            self.last_feedback.remove(&id);
            self.feedback_failures.remove(&id);
            self.pointers.retain(|(pad, _), _| u16::from(*pad) != id);
        }
        self.dualsenses.insert(id, pad);
        self.warnings.clear("input_dualsense");
        tracing::info!(
            controller = id,
            "Composite DualSense HID and audio attached through usbip-win2"
        );
        Ok(true)
    }
    pub fn apply(&mut self, event: &Event) -> Result<()> {
        match event {
            Event::Controller {
                id,
                active,
                buttons,
                left_trigger,
                right_trigger,
                sticks,
            } => {
                if *id >= 16 {
                    bail!("controller ID out of range");
                }
                self.decks.retain(|i, _| active & (1 << *i) != 0);
                self.dualsenses.retain(|i, _| active & (1 << *i) != 0);
                self.arrivals.retain(|i, _| active & (1 << *i) != 0);
                self.dualsense_failed.retain(|i| active & (1 << *i) != 0);
                self.states.retain(|i, _| active & (1 << *i) != 0);
                for (i, global) in self.active.clone() {
                    if active & (1 << i) == 0 {
                        // A pad that would not unplug stays tracked and is
                        // tried again with the next state; this state still
                        // goes to the live pad.
                        if let Err(error) = self.backend.unplug(u32::from(global)) {
                            tracing::debug!(
                                controller = i,
                                error = format!("{error:#}"),
                                "virtual controller removal failed"
                            );
                            continue;
                        }
                        self.active.remove(&i);
                        self.profiles.remove(&i);
                        self.arrivals.remove(&i);
                        self.states.remove(&i);
                        self.last_feedback.remove(&i);
                        self.feedback_failures.remove(&i);
                        self.pointers.retain(|(id, _), _| u16::from(*id) != i);
                        self.unsupported_touchpads.remove(&(i as u8));
                        SLOTS.lock().unwrap()[global as usize] = false;
                    }
                }
                if active & (1 << id) == 0 {
                    return Ok(());
                }
                if let Some(deck) = self.decks.get(id) {
                    deck.apply(event);
                    return Ok(());
                }
                self.ensure(*id)?;
                // No virtual pad has back grips: press what each is mapped to.
                let (buttons, left_trigger, right_trigger) =
                    self.policy
                        .map_back_grips(*buttons, *left_trigger, *right_trigger);
                let mapped = Event::Controller {
                    id: *id,
                    active: *active,
                    buttons,
                    left_trigger,
                    right_trigger,
                    sticks: *sticks,
                };
                let state = self
                    .states
                    .entry(*id)
                    .or_insert_with(|| (mapped.clone(), Default::default()));
                state.0 = mapped;
                let buttons = state.1.update(
                    buttons,
                    self.started.elapsed(),
                    self.policy.back_button_timeout,
                );
                self.submit(*id, buttons, left_trigger, right_trigger, sticks)?;
            }
            Event::Arrival {
                id,
                kind,
                capabilities,
                ..
            } => {
                let id = u16::from(*id);
                if self
                    .arrivals
                    .get(&id)
                    .is_some_and(|old| *old != (*kind, *capabilities))
                {
                    self.decks.remove(&id);
                    self.dualsenses.remove(&id);
                }
                self.arrivals.insert(id, (*kind, *capabilities));
                self.dualsense_failed.remove(&id);
                if let Some(pad) = self.dualsenses.get(&id) {
                    pad.capabilities(*capabilities);
                }
                // A controller that arrives again may try a Steam Deck again.
                self.deck_failed.remove(&id);
                if !self.ensure_deck(id)? {
                    self.ensure(id)?;
                }
            }
            Event::Motion { id, kind, xyz } => {
                if let Some(pad) = self.dualsenses.get(&u16::from(*id)) {
                    pad.apply(event);
                    return Ok(());
                }
                if let Some(deck) = self.decks.get(&u16::from(*id)) {
                    deck.apply(event);
                    return Ok(());
                }
                // Only arrival and state packets create a pad: motion travels on
                // another channel and can arrive after the pad was removed.
                if !self.motion_supported(u16::from(*id)) {
                    return Ok(());
                }
                self.backend
                    .motion(self.slot(u16::from(*id))?, *kind, xyz)?;
            }
            Event::ControllerTouch { id, .. } if self.dualsenses.contains_key(&u16::from(*id)) => {
                self.dualsenses[&u16::from(*id)].apply(event);
            }
            Event::ControllerTouch { id, .. } if self.decks.contains_key(&u16::from(*id)) => {
                self.decks[&u16::from(*id)].apply(event);
            }
            Event::ControllerTouch { id, touchpad, .. } => {
                // The driver exposes one PlayStation touch surface with two
                // contacts. A client that announced two touchpads gets them
                // side by side on it; a second touchpad that was not announced
                // is never reinterpreted as the first.
                let dual = self
                    .arrivals
                    .get(&u16::from(*id))
                    .is_some_and(|(_, capabilities)| capabilities & CCAP_DUAL_TOUCHPAD != 0);
                if *touchpad > u8::from(dual) {
                    if self.unsupported_touchpads.insert(*id) {
                        tracing::warn!(
                            controller = id,
                            touchpad,
                            dual,
                            "controller touchpad ignored: the virtual gamepad driver has one touch surface, shared by two touchpads only when the client announced both"
                        );
                    }
                    return Ok(());
                }
                let (Some(profile), Some(global)) = (
                    self.profiles.get(&u16::from(*id)).copied(),
                    self.active.get(&u16::from(*id)).copied(),
                ) else {
                    return Ok(());
                };
                if let Some(b) = gamepad_touch_request(&self.pointers, profile, global, event, dual)
                {
                    self.backend.touch(&b)?;
                    b.submitted(&mut self.pointers);
                }
            }
            Event::Battery { id, state, percent } => {
                if let Some(pad) = self.dualsenses.get(&u16::from(*id)) {
                    pad.apply(event);
                    return Ok(());
                }
                // Only the PlayStation and Switch profiles have a battery.
                if !matches!(self.profiles.get(&u16::from(*id)).copied(), Some(5..=7)) {
                    return Ok(());
                }
                self.backend
                    .battery(self.slot(u16::from(*id))?, *state, *percent)?;
            }
            _ => {}
        }
        Ok(())
    }
    pub(super) fn release_all(&mut self) -> Result<()> {
        let ids: BTreeSet<_> = self
            .active
            .keys()
            .chain(self.decks.keys())
            .chain(self.dualsenses.keys())
            .copied()
            .collect();
        let active = ids.iter().fold(0, |mask, id| mask | (1 << id));
        // A VHF pad that never got a state or a touch holds nothing. The
        // driver refused the neutral report to one a launch's controller
        // preparation had plugged a millisecond earlier (0x80070016). It
        // stays in `active`, which would otherwise unplug it.
        let untouched = |id: &u16| {
            self.active.contains_key(id)
                && !self.states.contains_key(id)
                && !self.pointers.keys().any(|(pad, _)| u16::from(*pad) == *id)
        };
        let ids: Vec<_> = ids.into_iter().filter(|id| !untouched(id)).collect();
        let mut failure = None;
        for id in ids {
            for event in [
                Event::Controller {
                    id,
                    active,
                    buttons: 0,
                    left_trigger: 0,
                    right_trigger: 0,
                    sticks: [0; 4],
                },
                Event::ControllerTouch {
                    id: id as u8,
                    event: 7,
                    touchpad: 0,
                    pointer: 0,
                    x: 0.,
                    y: 0.,
                    pressure: 0.,
                },
                Event::ControllerTouch {
                    id: id as u8,
                    event: 7,
                    touchpad: 1,
                    pointer: 0,
                    x: 0.,
                    y: 0.,
                    pressure: 0.,
                },
            ] {
                if matches!(event, Event::ControllerTouch { touchpad: 1, .. })
                    && !self.decks.contains_key(&id)
                    && !self
                        .arrivals
                        .get(&id)
                        .is_some_and(|(_, caps)| caps & CCAP_DUAL_TOUCHPAD != 0)
                {
                    continue;
                }
                if let Err(error) = self.apply(&event) {
                    failure.get_or_insert(error);
                }
            }
        }
        for pad in self.dualsenses.values() {
            pad.clear_pcm();
        }
        failure.map_or(Ok(()), Err)
    }
    /// New feedback (rumble, lights, trigger effects) for each controller.
    /// A controller with nothing pending or a failed poll does not hide the
    /// others' feedback, and a repeated report is not sent again.
    pub fn feedback(&mut self) -> Vec<(u16, u16, Vec<u8>)> {
        let mut reports = poll_feedback(
            &self.active,
            &mut self.last_feedback,
            &mut self.feedback_failures,
            self.backend.name(),
            Instant::now(),
            |slot| self.backend.feedback(slot),
        );
        for (id, deck) in &mut self.decks {
            if let Some((kind, data)) = deck.feedback() {
                reports.push((*id, kind, data));
            }
        }
        for (id, pad) in &self.dualsenses {
            reports.extend(
                pad.feedback()
                    .into_iter()
                    .map(|(kind, data)| (*id, kind, data)),
            );
        }
        reports
    }
    pub(super) fn haptics(&self) -> Vec<butterpollo_core::haptics::Frame> {
        self.dualsenses
            .values()
            .flat_map(|pad| pad.haptics())
            .collect()
    }
    pub(super) fn speaker(&self) -> Vec<butterpollo_core::haptics::Frame> {
        self.dualsenses.values().flat_map(|p| p.speaker()).collect()
    }
    pub(super) fn audio(&mut self, id: u16, kind: u16, payload: &[u8]) {
        if let Some(pad) = self.dualsenses.get(&id) {
            pad.audio(kind, payload);
        }
    }
    pub(super) fn audio_ready(&self) -> u16 {
        // DualSensePad::plug returns only after HID/audio enumeration succeeds.
        self.dualsenses.keys().fold(0, |mask, id| mask | (1 << id))
    }
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
        self.backend
            .submit(self.slot(id)?, buttons, left, right, sticks)
    }
    /// The driver slot of a plugged-in pad. A pad whose re-plug failed has a
    /// state but no slot, and must not take the host down (panic = abort).
    fn slot(&self, id: u16) -> Result<u32> {
        self.active
            .get(&id)
            .map(|global| u32::from(*global))
            .ok_or_else(|| anyhow::anyhow!("controller {id} is not plugged in"))
    }
    pub(super) fn refresh(&mut self) -> Result<()> {
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
        for (id, buttons, left, right, sticks) in updates {
            self.submit(id, buttons, left, right, &sticks)?;
        }
        Ok(())
    }
    pub fn motion_supported(&self, id: u16) -> bool {
        self.decks.contains_key(&id)
            || self.dualsenses.contains_key(&id)
            || matches!(self.profiles.get(&id).copied(), Some(5..=7))
    }
}
fn poll_feedback(
    active: &BTreeMap<u16, u16>,
    last: &mut BTreeMap<u16, (u16, Vec<u8>)>,
    failures: &mut BTreeMap<u16, FeedbackFailure>,
    backend: &str,
    now: Instant,
    mut poll: impl FnMut(u32) -> Result<Option<(u16, Vec<u8>)>>,
) -> Vec<(u16, u16, Vec<u8>)> {
    let mut output = vec![];
    let due = |failure: &FeedbackFailure| {
        failure
            .reported
            .is_none_or(|at| now.duration_since(at) >= Duration::from_secs(5))
    };
    for (&id, &global) in active {
        let report = match poll(u32::from(global)) {
            Ok(report) => {
                // Report the failures the interval held back once the pad
                // recovers, under the same interval, so none go unlogged.
                if let Some(failure) = failures.get_mut(&id)
                    && failure.unreported > 0
                    && due(failure)
                {
                    tracing::warn!(
                        controller = id,
                        slot = global,
                        backend,
                        failures = failure.unreported,
                        "controller feedback polls recovered after failing"
                    );
                    failure.unreported = 0;
                    failure.reported = Some(now);
                }
                let Some(report) = report else { continue };
                report
            }
            Err(error) => {
                let failure = failures.entry(id).or_default();
                failure.unreported += 1;
                if due(failure) {
                    tracing::warn!(controller = id, slot = global, backend, failures = failure.unreported, error = %format!("{error:#}"), "controller feedback poll failed");
                    failure.unreported = 0;
                    failure.reported = Some(now);
                }
                continue;
            }
        };
        if last.get(&id) != Some(&report) {
            last.insert(id, report.clone());
            output.push((id, report.0, report.1));
        }
    }
    last.retain(|id, _| active.contains_key(id));
    // Keep the warning interval across successful polls too: an intermittent
    // failure must not flood the log. Unplugging starts a fresh pad lifetime.
    failures.retain(|id, _| active.contains_key(id));
    output
}
fn profile_name(profile: u16) -> &'static str {
    match profile {
        3 => "vhf_xbox_one",
        4 => "vhf_xbox",
        5 => "vhf_ds4",
        6 => "vhf_ds5",
        7 => "vhf_switch",
        _ => "vhf",
    }
}
/// The session warning for a Nintendo controller (Moonlight type 3) that automatic
/// selection made another pad because the driver has no Switch Pro profile.
fn switch_fallback(configured: u16, kind: u8, capabilities: u16, profile: u16) -> Option<String> {
    use butterpollo_core::input_policy::{VHF_DS4, VHF_DUALSENSE, VHF_SWITCH};
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
// The driver has one touch surface and two contact slots. Keep this mapping
// independent of device IO so unsupported surfaces cannot change slot state.
pub(super) struct GamepadTouchRequest {
    pub(super) packet: Vec<u8>,
    id: u8,
    pointer: u32,
    slot: u8,
    event: u8,
}
impl GamepadTouchRequest {
    fn submitted(self, pointers: &mut BTreeMap<(u8, u32), u8>) {
        if self.event == 5 {
            pointers.retain(|(pad, _), _| *pad != self.id);
        } else if matches!(self.event, 2 | 4) {
            pointers.remove(&(self.id, self.pointer));
        } else {
            pointers.insert((self.id, self.pointer), self.slot);
        }
    }
}
/// Moonlight's `LI_CCAP_DUAL_TOUCHPAD` controller capability.
const CCAP_DUAL_TOUCHPAD: u16 = 0x100;

/// Maps a controller touch onto the driver's single touch surface. With
/// `dual`, touchpad 0 covers the left half of that surface and touchpad 1 the
/// right half, and a finger is told apart by its touchpad as well as its id.
fn gamepad_touch_request(
    pointers: &BTreeMap<(u8, u32), u8>,
    profile: u16,
    global: u16,
    input: &Event,
    dual: bool,
) -> Option<GamepadTouchRequest> {
    let Event::ControllerTouch {
        id,
        event,
        touchpad,
        pointer,
        x,
        y,
        pressure,
    } = input
    else {
        return None;
    };
    if *touchpad > u8::from(dual) || !matches!(profile, 5 | 6) {
        return None;
    }
    let event = match event {
        0..=4 => *event,
        6 => 4,
        7 => 5,
        _ => return None,
    };
    // Finger ids are small; the top bit tells the second touchpad's apart.
    let pointer = (*pointer & 0x7fff_ffff) | (u32::from(*touchpad) << 31);
    let key = (*id, pointer);
    let slot = if event == 5 {
        0
    } else if let Some(slot) = pointers.get(&key) {
        *slot
    } else if matches!(event, 0 | 1) {
        (0..2).find(|slot| {
            !pointers
                .iter()
                .any(|((pad, _), used)| pad == id && used == slot)
        })?
    } else {
        return None;
    };
    let mut b = request(22, Some(u32::from(global)));
    b.extend_from_slice(&[slot, event]);
    let x = if dual {
        x.clamp(0., 1.) / 2. + f32::from(*touchpad) / 2.
    } else {
        *x
    };
    for f in [x, *y, *pressure] {
        b.extend_from_slice(&((f.clamp(0., 1.) * 65535.) as u16).to_le_bytes());
    }
    b.extend_from_slice(&[0, 0]);
    Some(GamepadTouchRequest {
        packet: b,
        id: *id,
        pointer,
        slot,
        event,
    })
}
impl Drop for Gamepads {
    fn drop(&mut self) {
        for (i, global) in self.active.clone() {
            match self.backend.unplug(u32::from(global)) {
                Ok(()) => SLOTS.lock().unwrap()[global as usize] = false,
                // Its slot stays reserved and is tried again before the next plug.
                Err(error) => {
                    tracing::warn!(
                        controller = i,
                        slot = global,
                        error = format!("{error:#}"),
                        "virtual controller removal failed as its session ended"
                    );
                    STRANDED.lock().unwrap().insert(global);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
