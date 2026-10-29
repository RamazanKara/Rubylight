//! One virtual Xbox pad per player of a Nucleus session, each at an XInput
//! index of its own, and each player's controller routed to their pad only.
//!
//! Slot k's pad is the gamepad thread's controller k. The profile gives
//! player k that pad's XInput index, and ProtoInput feeds it to instance k.
use crate::input::{GamepadThread, PadReport};
use anyhow::{Context, Result, bail, ensure};
use butterpollo_core::input::Input;
use butterpollo_core::nucleus::coop::{self, MAX_PLAYERS};
use std::time::{Duration, Instant};

/// Moonlight's controller type for an Xbox pad.
const XBOX: u8 = 1;
/// Moonlight's capability bits: analog triggers and rumble.
const CAPABILITIES: u16 = 0x01 | 0x02;
/// How long a pad may take to plug in and to reach XInput.
pub const PLUG_TIMEOUT: Duration = Duration::from_secs(5);
const POLL: Duration = Duration::from_millis(20);

/// What the stream shows when a player's keyboard or mouse input arrives.
pub const KEYBOARD_MOUSE_LIMIT: &str = "Nucleus Co-op streams take controller input only: keyboard, mouse, touch and pen cannot reach one player's game instance yet";
/// What the stream shows when a player uses a second controller.
pub const SECOND_CONTROLLER_LIMIT: &str = "Each Nucleus Co-op player has one controller: only this device's first controller reaches its game instance";

/// The session's virtual pads: the gamepad thread on the PC, a recorder in
/// tests.
pub trait Pads: Send + Sync {
    fn send(&self, event: Input);
    /// Whether every pad in `mask` is plugged in, waiting up to `timeout`.
    fn wait_ready(&self, mask: u16, timeout: Duration) -> bool;
    fn reports(&self) -> Vec<PadReport>;
}
impl Pads for GamepadThread {
    fn send(&self, event: Input) {
        GamepadThread::send(self, event)
    }
    fn wait_ready(&self, mask: u16, timeout: Duration) -> bool {
        GamepadThread::wait_ready(self, mask, timeout)
    }
    fn reports(&self) -> Vec<PadReport> {
        GamepadThread::reports(self).collect()
    }
}

/// The session's pads on the PC: virtual Xbox Series pads, whatever the
/// clients' controllers are, since only Xbox pads reach XInput.
pub fn virtual_pads(
    policy: butterpollo_core::input_policy::Policy,
    warnings: std::sync::Arc<butterpollo_core::session::Warnings>,
) -> Result<GamepadThread> {
    GamepadThread::new(
        butterpollo_core::input_policy::VHF_XBOX_SERIES,
        policy,
        warnings,
    )
    .context("start the virtual controllers for Nucleus Co-op")
}

/// Which XInput user indices have a controller.
pub trait XInput: Send {
    fn connected(&mut self) -> Result<[bool; MAX_PLAYERS]>;
}

/// XInput on the PC.
pub struct System {
    library: libloading::Library,
}
impl System {
    pub fn open() -> Result<Self> {
        Ok(Self {
            library: crate::telemetry::system_library("xinput1_4.dll")
                .context("XInput is not available on this PC")?,
        })
    }

    /// The buttons XInput reports at user index `index` (`XINPUT_GAMEPAD`'s
    /// `wButtons`); None without a controller there. What a game that reads
    /// that index sees, before Nucleus Co-op's hooks.
    pub fn buttons(&self, index: u8) -> Result<Option<u16>> {
        type GetState = unsafe extern "system" fn(u32, *mut [u32; 4]) -> u32;
        // SAFETY: as in `connected`: the signature matches XInputGetState and
        // the library outlives the symbol.
        let get: libloading::Symbol<GetState> = unsafe { self.library.get(b"XInputGetState\0")? };
        let mut state = [0; 4];
        // SAFETY: the output has room for the whole XINPUT_STATE of this
        // synchronous call; XInput refuses an index outside 0..4.
        let result = unsafe { get(u32::from(index), &mut state) };
        Ok((result == 0).then(|| buttons_of(&state)))
    }
}

/// `wButtons` of an `XINPUT_STATE` read as four `u32`s: after the packet
/// number, the first two bytes of `XINPUT_GAMEPAD`.
fn buttons_of(state: &[u32; 4]) -> u16 {
    (state[1] & 0xffff) as u16
}
impl XInput for System {
    fn connected(&mut self) -> Result<[bool; MAX_PLAYERS]> {
        type GetState = unsafe extern "system" fn(u32, *mut [u32; 4]) -> u32;
        // SAFETY: the signature matches XInputGetState: [u32; 4] has XINPUT_STATE's 16-byte size
        // and 4-byte alignment, and the library outlives the symbol.
        let get: libloading::Symbol<GetState> = unsafe { self.library.get(b"XInputGetState\0")? };
        let mut connected = [false; MAX_PLAYERS];
        for (index, slot) in connected.iter_mut().enumerate() {
            // SAFETY: the index is in XInput's 0..4 range and the output has room for the whole
            // XINPUT_STATE of this synchronous call.
            *slot = unsafe { get(index as u32, &mut [0; 4]) } == 0;
        }
        Ok(connected)
    }
}

/// How many XInput indices the PC's own controllers leave free.
pub fn free(xinput: &mut dyn XInput) -> Result<usize> {
    Ok(xinput.connected()?.iter().filter(|c| !**c).count())
}

/// Plug one pad per player, in slot order, and learn the XInput index each
/// took. Plugged one at a time, so each new index belongs to the pad just
/// plugged; a controller connected meanwhile on the PC makes it ambiguous.
pub fn plug(
    pads: &dyn Pads,
    xinput: &mut dyn XInput,
    players: usize,
    timeout: Duration,
) -> Result<Vec<u8>> {
    ensure!(
        (1..=MAX_PLAYERS).contains(&players),
        "a Nucleus Co-op session has 1 to {MAX_PLAYERS} players, not {players}"
    );
    let mut indices = Vec::with_capacity(players);
    for slot in 0..players {
        let before = xinput.connected()?;
        pads.send(Input::Arrival {
            id: slot as u8,
            kind: XBOX,
            capabilities: CAPABILITIES,
            buttons: 0,
        });
        if !pads.wait_ready(1 << slot, timeout) {
            bail!(
                "Player {}'s virtual controller did not connect: check the virtual gamepad driver",
                slot + 1
            );
        }
        let deadline = Instant::now() + timeout;
        let index = loop {
            let after = xinput.connected()?;
            if let Some(index) = coop::new_pad(before, after) {
                break index;
            }
            let added = (0..MAX_PLAYERS).filter(|&i| after[i] && !before[i]).count();
            ensure!(
                added < 2,
                "another controller connected to the PC while Player {}'s virtual controller did; start the stream again",
                slot + 1
            );
            ensure!(
                Instant::now() < deadline,
                "Player {}'s virtual controller did not reach XInput: Nucleus Co-op needs Xbox controllers, so check the virtual gamepad driver's Xbox profile",
                slot + 1
            );
            std::thread::sleep(POLL);
        };
        tracing::info!(slot, xinput = index, "Nucleus Co-op player controller");
        indices.push(index);
    }
    Ok(indices)
}

/// Why a player's input event did not reach a pad.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dropped {
    /// The slot's pad is fixed for the session: arrivals, sensors, battery
    /// and haptics settings of the client's controller do not change it.
    Ignored,
    /// Keyboard, mouse, touch or pen: [`KEYBOARD_MOUSE_LIMIT`].
    KeyboardMouse,
    /// A controller other than the client's first: [`SECOND_CONTROLLER_LIMIT`].
    SecondController,
}
impl Dropped {
    /// What the stream tells the player, when it tells them anything.
    pub fn limit(self) -> Option<&'static str> {
        match self {
            Self::Ignored => None,
            Self::KeyboardMouse => Some(KEYBOARD_MOUSE_LIMIT),
            Self::SecondController => Some(SECOND_CONTROLLER_LIMIT),
        }
    }
}

/// Slot `slot`'s pad state from a client's controller `event`, in a session
/// of `players` pads. The client's first controller (the lowest it reports
/// connected) plays; the pad keeps every slot's pad plugged, since the
/// thread unplugs pads missing from `active`.
pub fn route(event: Input, slot: usize, players: usize) -> Result<Input, Dropped> {
    let all = ((1u32 << players) - 1) as u16;
    match event {
        Input::Controller {
            id,
            active,
            buttons,
            left_trigger,
            right_trigger,
            sticks,
        } => {
            // A client whose controllers all left sends one last state: the
            // slot's pad is released, never unplugged.
            let released = active == 0;
            if !released && id != active.trailing_zeros() as u16 {
                return Err(Dropped::SecondController);
            }
            Ok(Input::Controller {
                id: slot as u16,
                active: all,
                buttons: if released { 0 } else { buttons },
                left_trigger: if released { 0 } else { left_trigger },
                right_trigger: if released { 0 } else { right_trigger },
                sticks: if released { [0; 4] } else { sticks },
            })
        }
        Input::Relative { .. }
        | Input::Absolute { .. }
        | Input::MouseButton { .. }
        | Input::Scroll { .. }
        | Input::Keyboard { .. }
        | Input::Text(_)
        | Input::Touch { .. }
        | Input::Pen { .. } => Err(Dropped::KeyboardMouse),
        _ => Err(Dropped::Ignored),
    }
}

/// The client's controller a pad's feedback is for: slot k's pad is
/// controller k; the player's own is their first, 0.
pub fn feedback(report: PadReport, players: usize) -> Option<(usize, PadReport)> {
    match report {
        PadReport::Feedback { id, kind, data } if usize::from(id) < players => {
            Some((usize::from(id), PadReport::Feedback { id: 0, kind, data }))
        }
        _ => None,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn the_buttons_come_after_the_packet_number() {
        // Packet 7, A and B held, a left trigger of 0x40.
        let state = [7, 0x0040_3000, 0, 0];
        assert_eq!(buttons_of(&state), 0x3000);
    }

    /// Pads that record what they are sent; a plugged pad reaches XInput at
    /// the next free index.
    #[derive(Default)]
    pub(crate) struct Recorder {
        pub sent: Mutex<Vec<Input>>,
        /// Arrivals that never become ready.
        pub dead: bool,
        pub reports: Mutex<Vec<PadReport>>,
    }
    impl Pads for Recorder {
        fn send(&self, event: Input) {
            self.sent.lock().unwrap().push(event);
        }
        fn wait_ready(&self, _mask: u16, _timeout: Duration) -> bool {
            !self.dead
        }
        fn reports(&self) -> Vec<PadReport> {
            std::mem::take(&mut self.reports.lock().unwrap())
        }
    }

    /// XInput that shows `pads`' arrivals at the first free index, behind
    /// the PC's own controllers in `taken`.
    pub(crate) struct Fake<'a> {
        pub pads: &'a Recorder,
        pub taken: [bool; MAX_PLAYERS],
        /// Pads that never reach XInput (a DualShock profile).
        pub hidden: bool,
        /// A controller plugged into the PC with each pad.
        pub intruder: bool,
    }
    impl XInput for Fake<'_> {
        fn connected(&mut self) -> Result<[bool; MAX_PLAYERS]> {
            let mut connected = self.taken;
            if self.hidden {
                return Ok(connected);
            }
            let arrivals = self
                .pads
                .sent
                .lock()
                .unwrap()
                .iter()
                .filter(|e| matches!(e, Input::Arrival { .. }))
                .count()
                * if self.intruder { 2 } else { 1 };
            for slot in connected.iter_mut().filter(|c| !**c).take(arrivals) {
                *slot = true;
            }
            Ok(connected)
        }
    }

    #[test]
    fn each_player_gets_the_next_free_xinput_index() {
        let pads = Recorder::default();
        let mut xinput = Fake {
            pads: &pads,
            taken: [false, true, false, false],
            hidden: false,
            intruder: false,
        };
        assert_eq!(free(&mut xinput).unwrap(), 3);
        // The PC's own pad holds index 1.
        assert_eq!(
            plug(&pads, &mut xinput, 3, PLUG_TIMEOUT).unwrap(),
            vec![0, 2, 3]
        );
        let sent = pads.sent.lock().unwrap();
        assert_eq!(sent.len(), 3);
        assert!(
            matches!(
                sent[2],
                Input::Arrival {
                    id: 2,
                    kind: XBOX,
                    ..
                }
            ),
            "{sent:?}"
        );
    }

    #[test]
    fn a_pad_that_does_not_reach_xinput_fails_loudly() {
        let pads = Recorder::default();
        let mut xinput = Fake {
            pads: &pads,
            taken: [false; 4],
            hidden: true,
            intruder: false,
        };
        let error = plug(&pads, &mut xinput, 1, Duration::from_millis(50)).unwrap_err();
        assert!(
            error
                .to_string()
                .starts_with("Player 1's virtual controller did not reach XInput"),
            "{error}"
        );
        let dead = Recorder {
            dead: true,
            ..Default::default()
        };
        let mut xinput = Fake {
            pads: &dead,
            taken: [false; 4],
            hidden: false,
            intruder: false,
        };
        let error = plug(&dead, &mut xinput, 2, PLUG_TIMEOUT).unwrap_err();
        assert!(error.to_string().contains("did not connect"), "{error}");
        let pads = Recorder::default();
        let mut xinput = Fake {
            pads: &pads,
            taken: [false; 4],
            hidden: false,
            intruder: true,
        };
        let error = plug(&pads, &mut xinput, 1, PLUG_TIMEOUT).unwrap_err();
        assert!(
            error
                .to_string()
                .starts_with("another controller connected"),
            "{error}"
        );
        assert!(plug(&pads, &mut xinput, 0, PLUG_TIMEOUT).is_err());
        assert!(plug(&pads, &mut xinput, 5, PLUG_TIMEOUT).is_err());
    }

    fn state(id: u16, active: u16, buttons: u32) -> Input {
        Input::Controller {
            id,
            active,
            buttons,
            left_trigger: 10,
            right_trigger: 20,
            sticks: [1, 2, 3, 4],
        }
    }

    #[test]
    fn a_player_s_first_controller_drives_their_slot_s_pad_only() {
        // Player 3's client calls its controller 0; it becomes pad 2, and
        // every slot's pad stays plugged.
        assert_eq!(
            route(state(0, 0b1, 0x1000), 2, 3),
            Ok(state(2, 0b111, 0x1000))
        );
        // A client whose first controller is its second slot.
        assert_eq!(route(state(1, 0b110, 0x10), 0, 2), Ok(state(0, 0b11, 0x10)));
        assert_eq!(
            route(state(2, 0b110, 0x10), 0, 2),
            Err(Dropped::SecondController)
        );
        // The last controller left: the slot's pad is released, not unplugged.
        assert_eq!(
            route(state(1, 0, 0x1000), 1, 2),
            Ok(Input::Controller {
                id: 1,
                active: 0b11,
                buttons: 0,
                left_trigger: 0,
                right_trigger: 0,
                sticks: [0; 4],
            })
        );
        let arrival = Input::Arrival {
            id: 0,
            kind: 2,
            capabilities: 0,
            buttons: 0,
        };
        assert_eq!(route(arrival, 0, 1), Err(Dropped::Ignored));
        assert_eq!(route(Input::Haptics(false), 0, 1), Err(Dropped::Ignored));
    }

    #[test]
    fn keyboard_and_mouse_are_refused_with_the_limit() {
        for event in [
            Input::Relative { x: 1, y: 1 },
            Input::Keyboard {
                key: 0x41,
                modifiers: 0,
                flags: 0,
                down: true,
            },
            Input::Text("a".into()),
            Input::MouseButton {
                button: 1,
                down: true,
            },
        ] {
            let dropped = route(event, 0, 1).unwrap_err();
            assert_eq!(dropped, Dropped::KeyboardMouse);
            assert_eq!(dropped.limit(), Some(KEYBOARD_MOUSE_LIMIT));
        }
        assert_eq!(Dropped::Ignored.limit(), None);
    }

    #[test]
    fn a_pad_s_rumble_goes_to_its_player_s_controller() {
        let rumble = |id| PadReport::Feedback {
            id,
            kind: 1,
            data: vec![1, 2],
        };
        assert_eq!(feedback(rumble(1), 2), Some((1, rumble(0))));
        assert_eq!(feedback(rumble(2), 2), None);
        let motion = PadReport::Motion {
            id: 0,
            capabilities: 0,
        };
        assert_eq!(feedback(motion, 1), None);
    }

    /// The real pads (hw-queue.md, "Nucleus Co-op pads"): each player's pad
    /// takes an XInput index the PC's own controllers leave free, and all
    /// of them are gone again once the pads are dropped.
    #[test]
    #[ignore = "plugs virtual Xbox pads: requires the virtual gamepad driver and an idle PC"]
    fn virtual_pads_take_free_xinput_indices_on_this_pc() -> Result<()> {
        let players = std::env::var("RUBYLIGHT_NUCLEUS_PLAYERS").map_or(Ok(2), |p| p.parse())?;
        let policy = butterpollo_core::input_policy::Policy::resolve(&Default::default())?;
        let warnings = std::sync::Arc::new(butterpollo_core::session::Warnings::default());
        let pads = virtual_pads(policy, warnings.clone())?;
        let mut xinput = System::open()?;
        let before = xinput.connected()?;
        let started = Instant::now();
        let indices = plug(&pads, &mut xinput, players, PLUG_TIMEOUT)?;
        println!(
            "before {before:?}, indices {indices:?} in {:?}",
            started.elapsed()
        );
        for warning in warnings.snapshot() {
            println!("warning {}: {}", warning.code, warning.message);
        }
        for &index in &indices {
            ensure!(!before[usize::from(index)], "index {index} was the PC's");
        }
        drop(pads);
        let deadline = Instant::now() + PLUG_TIMEOUT;
        while xinput.connected()? != before {
            ensure!(Instant::now() < deadline, "the pads did not unplug");
            std::thread::sleep(POLL);
        }
        Ok(())
    }
}
