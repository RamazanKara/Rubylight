//! Virtual gamepad lifecycle, controller reports, and feedback.

use super::*;
use butterpollo_input_policy::{
    input_policy::{VHF_STEAM_CONTROLLER, VIGEM_DS4},
    steam_pad,
};

mod attach;
mod driver_host;
mod feedback;
mod slots;
mod submit;
mod touch;

use attach::profile_name;
#[cfg(test)]
use attach::{steam_controls_fallback, switch_fallback};
use feedback::FeedbackFailure;
#[cfg(test)]
use feedback::{FEEDBACK_DRAIN, poll_feedback};
pub use slots::open_interface;
#[cfg(test)]
use slots::reclaim;
use slots::{SLOTS, STRANDED, device_removed, plug_in_free_slot, reclaim_until_lost};
pub(super) use slots::{Slot, interface_present, request};
#[cfg(test)]
use submit::profile_button_word;
pub(super) use touch::GamepadTouchRequest;
use touch::{CCAP_DUAL_TOUCHPAD, gamepad_touch_request};

pub struct Gamepads {
    backend: gamepad_backend::Backend,
    pub(super) active: BTreeMap<u16, u16>,
    profile: u16,
    available: u32,
    profiles: BTreeMap<u16, u16>,
    arrivals: BTreeMap<u16, (u8, u16)>,
    states: BTreeMap<u16, (Event, butterpollo_input_policy::input_policy::BackButton)>,
    policy: butterpollo_input_policy::input_policy::Policy,
    started: std::time::Instant,
    pointers: BTreeMap<(u8, u32), u8>,
    unsupported_touchpads: BTreeSet<u8>,
    /// The last feedback report forwarded for each controller.
    last_feedback: BTreeMap<u16, (u16, Vec<u8>)>,
    feedback_failures: BTreeMap<u16, FeedbackFailure>,
    /// Feedback reports read from the driver since the thread last asked.
    feedback_drained: usize,
    /// When each pad the driver's device lost was last created again: a driver
    /// that stays gone is asked [`RECREATE_RETRY`] apart for that pad, from
    /// the input path and the feedback poll, not at every state and poll.
    recreated_at: BTreeMap<u16, Instant>,
    /// Controllers the client removed but whose pad the driver would not
    /// unplug. They stay in `active` and later states retry the unplug; a
    /// feedback poll must never create one again, or a pad nobody has
    /// reappears after sleep.
    dropped: BTreeSet<u16>,
    /// `gamepad_driver_host_priority` while a VHF pad is plugged.
    driver_host: Option<Box<dyn driver_host::Held>>,
    /// How a hold on it is taken; tests count instead of raising.
    driver_host_acquire: fn() -> Box<dyn driver_host::Held>,
    /// Steam Deck clients' controllers attached as real Steam Decks.
    pub(super) decks: BTreeMap<u16, steam_deck_pad::DeckPad>,
    /// Controllers a Steam Deck could not be attached for; they keep their
    /// VHF pad until they arrive again.
    deck_failed: BTreeSet<u16>,
    /// Whether Steam was running when each controller carrying on-screen Steam Controller
    /// controls arrived under Automatic. Asked once per connection: `ensure` plugs a pad again
    /// when its profile changes, and Steam starting later must not swap it. Forgotten when the
    /// controller arrives again or leaves.
    steam_at_arrival: BTreeMap<u16, bool>,
    /// How Steam is looked for; tests answer instead.
    steam_running: fn() -> bool,
    /// Controllers whose on-screen Steam Controller controls got another pad in an app started
    /// through Steam: the session's warning stays while any does.
    steam_fallbacks: BTreeSet<u16>,
    /// [`Gamepads::steam_at_arrival`] of the pads before, closed after the
    /// driver refused their handle: each controller's replayed arrival keeps
    /// its answer instead of asking again (#138).
    carried_steam: BTreeMap<u16, bool>,
    pub(super) dualsenses: BTreeMap<u16, dualsense_pad::DualSensePad>,
    dualsense_failed: BTreeSet<u16>,
    /// Controllers attached as Switch 2 Pro Controllers over usbip-win2.
    pub(super) switch2s: BTreeMap<u16, switch2_pad::Switch2Pad>,
    /// Controllers a Switch 2 Pro Controller could not be attached for; they
    /// keep their VHF pad until they arrive again.
    switch2_failed: BTreeSet<u16>,
    /// USB vendor and product of each controller whose client said which it
    /// is (0x55000080), sent before its arrival.
    identities: BTreeMap<u16, (u16, u16)>,
    warnings: std::sync::Arc<butterpollo_session::session::Warnings>,
    /// Each Steam Controller client's real firmware attributes, applied
    /// again whenever its pad is plugged anew.
    steam_attributes: BTreeMap<u16, steam_pad::SteamAttributes>,
    /// Haptic reports for Steam Controller pads whose client has no real pads:
    /// the gamepad thread synthesizes rumble from them.
    steam_haptics: Vec<steam_pad::SteamHaptic>,
    /// Whether the installed driver answered the attributes request as one
    /// that predates it; logged once.
    steam_attributes_unsupported: bool,
    /// The driver refused this handle ([`gamepad_backend::DriverLost`]): the
    /// gamepad thread closes these pads and opens the driver again (#138).
    lost: bool,
    /// Completed background USB/IP pad attaches waiting to be installed.
    pub(super) attaches:
        std::sync::Arc<std::sync::Mutex<std::collections::VecDeque<attach::CompletedAttach>>>,
    /// In-flight USB/IP pad attaches keyed by kind and controller ID -> generation.
    pub(super) attaching: BTreeMap<(attach::AttachKind, u16), u64>,
    /// Background USB/IP pad detaches still holding a [`Slot`], joined before reusing `id` or teardown.
    pub(super) detaching: Vec<(u16, std::thread::JoinHandle<()>)>,
    next_attach_gen: u64,
    /// Wakes the owning gamepad thread when a background attach finishes.
    wake: Option<std::sync::Arc<dyn Fn() + Send + Sync>>,
    /// Controllers whose async USB/IP attach completed with motion sensors and need a motion request.
    pending_motion_requests: Vec<(u8, u16)>,
    /// Test hook overriding USB/IP background plug without calling `usbip.exe`.
    #[cfg(test)]
    pub(super) test_attach_hook: Option<TestAttachHook>,
}

#[cfg(test)]
pub(super) type TestAttachHook =
    std::sync::Arc<dyn Fn(attach::AttachKind, u16) -> Result<()> + Send + Sync>;

/// What pads closed after the driver refused their handle hand to the pads
/// opened next (#138).
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct PadMemory {
    /// Whether Steam was running at each controller's arrival.
    pub(super) steam_at_arrival: BTreeMap<u16, bool>,
    /// Controllers left without a Steam Controller, whose warning is up.
    pub(super) steam_fallbacks: BTreeSet<u16>,
}

/// How often each pad the driver's device lost is created again while the
/// driver keeps refusing, so a sleep that leaves the driver gone is not asked
/// at every controller state or 8 ms feedback poll.
const RECREATE_RETRY: Duration = Duration::from_secs(1);

fn retain_usb_pads<T: Send + 'static>(
    pads: &mut BTreeMap<u16, T>,
    active: u16,
    detaching: &mut Vec<(u16, std::thread::JoinHandle<()>)>,
) {
    let gone: Vec<u16> = pads
        .keys()
        .copied()
        .filter(|i| active & (1 << *i) == 0)
        .collect();
    for id in gone {
        if let Some(pad) = pads.remove(&id) {
            spawn_usb_detach(detaching, id, pad);
        }
    }
}

fn spawn_usb_detach<T: Send + 'static>(
    detaching: &mut Vec<(u16, std::thread::JoinHandle<()>)>,
    id: u16,
    pad: T,
) {
    detaching.retain(|(_, h)| !h.is_finished());
    // If thread creation fails, dropping `Err` drops the closure and `pad` inline.
    if let Ok(handle) = std::thread::Builder::new()
        .name("usbip-detach".into())
        .spawn(move || drop(pad))
    {
        detaching.push((id, handle));
    }
}

impl Gamepads {
    pub(super) fn detach_usb_pad<T: Send + 'static>(&mut self, id: u16, pad: T) {
        spawn_usb_detach(&mut self.detaching, id, pad);
    }
    pub(super) fn wait_detach(&mut self, target: Option<u16>) {
        let mut keep = Vec::new();
        for (id, handle) in self.detaching.drain(..) {
            if target.is_none_or(|wanted| wanted == id) {
                let _ = handle.join();
            } else if !handle.is_finished() {
                keep.push((id, handle));
            } else {
                let _ = handle.join();
            }
        }
        self.detaching = keep;
    }
    pub(super) fn wait_detach_before_plug(&mut self, id: u16) {
        self.wait_detach(Some(id));
        if !self.detaching.is_empty() && SLOTS.lock().unwrap().iter().all(|&used| used) {
            self.wait_detach(None);
        }
    }
    pub(super) fn set_warnings(
        &mut self,
        warnings: std::sync::Arc<butterpollo_session::session::Warnings>,
    ) {
        self.warnings = warnings;
    }
    pub(super) fn set_policy(
        &mut self,
        policy: butterpollo_input_policy::input_policy::Policy,
    ) -> Result<()> {
        let was_steam = self.policy.steam_app;
        self.policy = policy;
        if !was_steam && self.policy.steam_app {
            self.steam_at_arrival.clear();
        }
        self.follow_driver_host();
        let ids: Vec<u16> = self.active.keys().copied().collect();
        let mut failure = None;
        for id in ids {
            if self.dropped.contains(&id) {
                continue;
            }
            if let Err(error) = self.ensure(id) {
                failure.get_or_insert(error);
                continue;
            }
            if let Some(&(kind, capabilities)) = self.arrivals.get(&id) {
                let pad = if self.dualsenses.contains_key(&id) {
                    Some(butterpollo_input_policy::input_policy::VHF_DUALSENSE)
                } else {
                    self.profiles.get(&id).copied()
                };
                match pad {
                    Some(profile) => self.note_steam_controls(id, kind, capabilities, profile),
                    None => self.forget_steam_fallback(id),
                }
            }
        }
        self.note(failure.map_or(Ok(()), Err))
    }
    pub fn open(profile: u16) -> Result<Self> {
        Self::open_options(
            profile,
            butterpollo_input_policy::input_policy::Policy::resolve(&Default::default())?,
            Default::default(),
        )
    }
    pub(super) fn open_options(
        profile: u16,
        policy: butterpollo_input_policy::input_policy::Policy,
        warnings: std::sync::Arc<butterpollo_session::session::Warnings>,
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
            feedback_drained: 0,
            recreated_at: BTreeMap::new(),
            dropped: BTreeSet::new(),
            driver_host: None,
            driver_host_acquire: driver_host::acquire,
            decks: BTreeMap::new(),
            deck_failed: BTreeSet::new(),
            steam_at_arrival: BTreeMap::new(),
            steam_running: steam_deck_pad::steam_running,
            steam_fallbacks: BTreeSet::new(),
            carried_steam: BTreeMap::new(),
            dualsenses: BTreeMap::new(),
            dualsense_failed: BTreeSet::new(),
            switch2s: BTreeMap::new(),
            switch2_failed: BTreeSet::new(),
            identities: BTreeMap::new(),
            warnings,
            steam_attributes: BTreeMap::new(),
            steam_haptics: Vec::new(),
            steam_attributes_unsupported: false,
            lost: false,
            attaches: Default::default(),
            attaching: BTreeMap::new(),
            detaching: Vec::new(),
            next_attach_gen: 0,
            wake: None,
            pending_motion_requests: Vec::new(),
            #[cfg(test)]
            test_attach_hook: None,
        })
    }
    pub fn apply(&mut self, event: &Event) -> Result<()> {
        self.finish_attaches()?;
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
                retain_usb_pads(&mut self.decks, *active, &mut self.detaching);
                retain_usb_pads(&mut self.dualsenses, *active, &mut self.detaching);
                retain_usb_pads(&mut self.switch2s, *active, &mut self.detaching);
                self.switch2_failed.retain(|i| active & (1 << *i) != 0);
                self.attaching.retain(|(_, i), _| active & (1 << *i) != 0);
                self.pending_motion_requests
                    .retain(|(i, _)| active & (1 << *i) != 0);
                self.arrivals.retain(|i, _| active & (1 << *i) != 0);
                self.steam_at_arrival.retain(|i, _| active & (1 << *i) != 0);
                self.carried_steam.retain(|i, _| active & (1 << *i) != 0);
                // A USB/IP pad that leaves is never forgotten through `forget_pad`.
                if self.steam_fallbacks.iter().any(|i| active & (1 << *i) == 0) {
                    self.steam_fallbacks.retain(|i| active & (1 << *i) != 0);
                    if self.steam_fallbacks.is_empty() {
                        self.warnings.clear("input_steam_controls");
                    }
                }
                self.steam_attributes.retain(|i, _| active & (1 << *i) != 0);
                self.dualsense_failed.retain(|i| active & (1 << *i) != 0);
                self.states.retain(|i, _| active & (1 << *i) != 0);
                // A pad this state turns back on is not on its way out: only
                // the client's removal had deferred its unplug.
                self.dropped.retain(|i| active & (1 << *i) == 0);
                // Every state passes here: find the pads that left without
                // copying the map, which only a removal needs to change.
                let gone = self.active.keys().fold(0u16, |mask, i| mask | (1 << i)) & !active;
                for i in (0..16u16).filter(|i| gone & (1 << i) != 0) {
                    let global = self.active[&i];
                    if let Err(error) = self.backend.unplug(u32::from(global)) {
                        if gamepad_backend::driver_lost(&error) {
                            self.lost = true;
                            return Err(error);
                        }
                        self.dropped.insert(i);
                        tracing::debug!(
                            controller = i,
                            error = format!("{error:#}"),
                            "virtual controller removal failed"
                        );
                        continue;
                    }
                    self.forget_pad(i, global);
                }
                if active & (1 << id) == 0 {
                    return Ok(());
                }
                if let Some(deck) = self.decks.get(id) {
                    deck.apply(event);
                    return Ok(());
                }
                // It has grip buttons of its own: no back grip mapping.
                if self.ensure_switch2(*id)? {
                    if let Some(pad) = self.switch2s.get(id) {
                        pad.apply(event);
                    }
                    return Ok(());
                }
                self.ensure(*id)?;
                // Only the Steam Controller has back grips: on any other pad,
                // press what each is mapped to.
                let (buttons, left_trigger, right_trigger) =
                    if self.profiles.get(id) == Some(&VHF_STEAM_CONTROLLER) {
                        (*buttons, *left_trigger, *right_trigger)
                    } else {
                        self.policy
                            .map_back_grips(*buttons, *left_trigger, *right_trigger)
                    };
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
                self.submit_state(*id, buttons, left_trigger, right_trigger, sticks)?;
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
                    if let Some(pad) = self.decks.remove(&id) {
                        self.detach_usb_pad(id, pad);
                    }
                    if let Some(pad) = self.dualsenses.remove(&id) {
                        self.detach_usb_pad(id, pad);
                    }
                    self.attaching.retain(|(k, i), _| {
                        *i != id
                            || !matches!(
                                k,
                                attach::AttachKind::Deck | attach::AttachKind::DualSense
                            )
                    });
                }
                // A controller that is no longer a Switch 2 Pro Controller
                // (another one in its place) gets its own pad.
                if self.switch2s.contains_key(&id)
                    && !butterpollo_input_policy::input_policy::wants_switch2(
                        self.profile,
                        self.identities.get(&id).copied(),
                    )
                {
                    if let Some(pad) = self.switch2s.remove(&id) {
                        self.detach_usb_pad(id, pad);
                    }
                    self.attaching.remove(&(attach::AttachKind::Switch2, id));
                }
                self.pending_motion_requests
                    .retain(|(i, _)| u16::from(*i) != id);
                self.arrivals.insert(id, (*kind, *capabilities));
                // A Steam Controller sends its attributes after each arrival.
                self.steam_attributes.remove(&id);
                self.dualsense_failed.remove(&id);
                if let Some(pad) = self.dualsenses.get(&id) {
                    pad.capabilities(*capabilities);
                }
                // A controller that arrives again may try a Steam Deck or a
                // Switch 2 Pro Controller again, and asks again whether Steam runs.
                self.deck_failed.remove(&id);
                self.switch2_failed.remove(&id);
                // An arrival replayed on pads opened after the driver refused
                // the last ones' handle keeps their answer: Steam starting
                // meanwhile must not make the pad a Steam Controller (#138).
                match self.carried_steam.remove(&id) {
                    Some(running) => self.steam_at_arrival.insert(id, running),
                    None => self.steam_at_arrival.remove(&id),
                };
                // The client says this controller is here, whatever a
                // removal whose unplug failed left behind.
                self.dropped.remove(&id);
                if !self.ensure_switch2(id)? && !self.ensure_deck(id)? {
                    self.ensure(id)?;
                }
                // Whichever branch gave it its pad (a USB/IP DualSense returns from `ensure`
                // early), and also when the pad stayed as it was.
                let pad = if self.dualsenses.contains_key(&id) {
                    Some(butterpollo_input_policy::input_policy::VHF_DUALSENSE)
                } else {
                    self.profiles.get(&id).copied()
                };
                match pad {
                    Some(profile) => self.note_steam_controls(id, *kind, *capabilities, profile),
                    None => self.forget_steam_fallback(id),
                }
            }
            Event::Motion { id, kind, xyz } => {
                if let Some(pad) = self.dualsenses.get(&u16::from(*id)) {
                    pad.apply(event);
                    return Ok(());
                }
                if let Some(pad) = self.switch2s.get(&u16::from(*id)) {
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
                if let Some(pad) = self.switch2s.get(&u16::from(*id)) {
                    pad.apply(event);
                    return Ok(());
                }
                // Only the PlayStation, Switch and Steam Controller profiles
                // have a battery.
                if !matches!(
                    self.profiles.get(&u16::from(*id)).copied(),
                    Some(5..=7 | VHF_STEAM_CONTROLLER | VIGEM_DS4)
                ) {
                    return Ok(());
                }
                self.backend
                    .battery(self.slot(u16::from(*id))?, *state, *percent)?;
            }
            Event::SteamAttributes(attributes) => {
                use steam_pad::LI_CCAP_STEAM_ATTRIBUTES;
                let id = u16::from(attributes.controller);
                // Only from a Steam Controller that announced them.
                if !self.arrivals.get(&id).is_some_and(|(kind, capabilities)| {
                    *kind == butterpollo_input_policy::input_policy::CTYPE_STEAM
                        && capabilities & LI_CCAP_STEAM_ATTRIBUTES != 0
                }) {
                    return Ok(());
                }
                self.steam_attributes.insert(id, *attributes);
                self.send_steam_attributes(id);
            }
            Event::ControllerIdentity(identity) => {
                let id = u16::from(identity.controller);
                let new = (identity.vendor, identity.product);
                // Another controller in this slot: the arrival that follows
                // chooses its pad again.
                if self
                    .identities
                    .insert(id, new)
                    .is_some_and(|old| old != new)
                {
                    if let Some(pad) = self.switch2s.remove(&id) {
                        self.detach_usb_pad(id, pad);
                    }
                    self.attaching.remove(&(attach::AttachKind::Switch2, id));
                }
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
            .chain(self.switch2s.keys())
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
    /// What the pads opened after these must keep (#138).
    pub(super) fn memory(&self) -> PadMemory {
        // Answers carried from the pads before whose arrival was not replayed
        // yet stay; the pads' own answer wins.
        let mut steam_at_arrival = self.carried_steam.clone();
        steam_at_arrival.extend(&self.steam_at_arrival);
        PadMemory {
            steam_at_arrival,
            steam_fallbacks: self.steam_fallbacks.clone(),
        }
    }
    /// Takes over what the pads before kept: their Steam answers for the
    /// replayed arrivals, and the controllers whose warning is still up, so
    /// it clears when they leave.
    pub(super) fn remember(&mut self, memory: PadMemory) {
        self.carried_steam = memory.steam_at_arrival;
        self.steam_fallbacks.extend(memory.steam_fallbacks);
    }
    /// Marks the pads lost when `result` is the driver refusing their handle.
    pub(super) fn note(&mut self, result: Result<()>) -> Result<()> {
        self.lost |= result.as_ref().is_err_and(gamepad_backend::driver_lost);
        result
    }
    /// Whether the driver refused this handle, so the pads must be closed.
    pub(super) fn lost(&self) -> bool {
        self.lost
    }
    /// Whether the VHF driver's device is still there once it refused the
    /// handle: the driver lost the host's open, or its device failed.
    pub(super) fn driver_present(&self) -> bool {
        interface_present(gamepad_backend::VHF_INTERFACE)
    }
    /// Hold the gamepad driver's WUDFHost raised while a VHF pad is plugged,
    /// with `gamepad_driver_host_priority` on.
    pub(super) fn follow_driver_host(&mut self) {
        let wanted = self.policy.driver_host_priority
            && self.backend.hosted_by_wudfhost()
            && !self.active.is_empty();
        if wanted != self.driver_host.is_some() {
            self.driver_host = wanted.then(self.driver_host_acquire);
        }
    }
    /// A raised WUDFHost that restarted is raised again in its new process.
    pub(super) fn check_driver_host(&self) {
        if let Some(held) = &self.driver_host {
            held.check();
        }
    }
    /// The events of the plugged pads' pending feedback requests.
    pub(super) fn wait_handles(&self, handles: &mut Vec<HANDLE>) {
        for &global in self.active.values() {
            handles.extend(self.backend.wait_handle(u32::from(global)));
        }
    }
    /// Whether a plugged pad's feedback request has completed.
    pub(super) fn feedback_ready(&self) -> bool {
        self.active
            .values()
            .any(|&global| self.backend.feedback_ready(u32::from(global)))
    }
    /// Feedback reports read from the driver since the last call.
    pub(super) fn take_feedback_drained(&mut self) -> usize {
        std::mem::take(&mut self.feedback_drained)
    }
    /// Haptic reports Steam wrote to a virtual Steam Controller whose client
    /// has no real pads, since the last call.
    pub(super) fn take_steam_haptics(&mut self) -> Vec<steam_pad::SteamHaptic> {
        std::mem::take(&mut self.steam_haptics)
    }
    pub(super) fn haptics(&self) -> Vec<butterpollo_input_policy::haptics::Frame> {
        self.dualsenses
            .values()
            .flat_map(|pad| pad.haptics())
            .collect()
    }
    pub(super) fn speaker(&self) -> Vec<butterpollo_input_policy::haptics::Frame> {
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
    pub(super) fn set_wake(&mut self, wake: std::sync::Arc<dyn Fn() + Send + Sync>) {
        self.wake = Some(wake);
    }
    pub(super) fn attach_ready(&self) -> bool {
        !self.attaches.lock().unwrap().is_empty()
    }
}

#[cfg(test)]
mod recovery_tests;
#[cfg(test)]
mod tests;
