//! One virtual Xbox pad per player of a Nucleus session, each at an XInput
//! index of its own, and each player's controller routed to their pad only.
//!
//! Slot k's pad is the gamepad thread's controller k. The profile gives
//! player k that pad's XInput index, and ProtoInput feeds it to instance k.
use crate::input::{GamepadThread, PadReport};
use anyhow::{Context, Result, bail, ensure};
use butterpollo_core::input::Input;
use butterpollo_core::nucleus::coop::{self, MAX_PLAYERS};
use butterpollo_core::session::Warnings;
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::{Duration, Instant};

/// Moonlight's controller type for an Xbox pad.
const XBOX: u8 = 1;
/// Moonlight's capability bits: analog triggers and rumble.
const CAPABILITIES: u16 = 0x01 | 0x02;
/// How long a pad may take to plug in and to reach XInput.
pub const PLUG_TIMEOUT: Duration = Duration::from_secs(5);
const POLL: Duration = Duration::from_millis(20);
/// How long a set of pads may take to unplug when it is stopped: a launch
/// that waits on a driver that stopped answering fails after this instead of
/// hanging, and so do the launches after it.
pub const RETIRE_TIMEOUT: Duration = Duration::from_secs(3);

/// What the stream shows when a player's keyboard or mouse input arrives.
pub const KEYBOARD_MOUSE_LIMIT: &str = "Nucleus Co-op streams take controller input only: keyboard, mouse, touch and pen cannot reach one player's game instance yet";
/// What the stream shows when a player uses a second controller.
pub const SECOND_CONTROLLER_LIMIT: &str = "Each Nucleus Co-op player has one controller: only this device's first controller reaches its game instance";
/// The stream's warning for a controller on the PC at a lower XInput index
/// than a player's pad ([`plug`]).
pub const CONTROLLER_AHEAD: &str = "nucleus_controller_ahead";

/// The session's virtual pads: the gamepad thread on the PC, a recorder in
/// tests.
pub trait Pads: Send + Sync {
    fn send(&self, event: Input);
    /// Whether every pad in `mask` is plugged in, waiting up to `timeout`.
    fn wait_ready(&self, mask: u16, timeout: Duration) -> bool;
    fn reports(&self) -> Vec<PadReport>;
    /// Unplug the pads earlier sessions still hold, and return the XInput
    /// indices they took. A session's pads stay until the host drops its
    /// last copy, which can be minutes after it ended; meanwhile they would
    /// hold the low indices a game reads first (#56).
    fn retire_earlier(&self) -> Result<Vec<u8>> {
        Ok(Vec::new())
    }
    /// Note the XInput indices this session's pads took, for [`free`] and
    /// a later session's [`Pads::retire_earlier`].
    fn took(&self, _indices: &[u8]) {}
    /// Show the stream a warning.
    fn warn(&self, _code: &str, _message: &str) {}
}

/// The session's pads on the PC; [`virtual_pads`].
pub struct SessionPads {
    thread: Arc<GamepadThread>,
    warnings: Arc<Warnings>,
}

/// Every session's pads the host still holds, and the XInput indices they
/// took. One Nucleus Co-op session runs at a time, so all but the newest
/// belong to sessions that ended.
struct Plugged {
    thread: Weak<GamepadThread>,
    indices: Vec<u8>,
    /// Pads the host dropped while their thread was stuck in the driver:
    /// kept until a later session finds them unplugged.
    abandoned: Option<Arc<GamepadThread>>,
}
static PLUGGED: Mutex<Vec<Plugged>> = Mutex::new(Vec::new());

/// A set stays listed until its pads are unplugged ([`SessionPads`]'s drop,
/// or a later session's [`Pads::retire_earlier`] for one whose thread was
/// stuck), so a set the host is dropping right now is still waited for.
fn plugged() -> std::sync::MutexGuard<'static, Vec<Plugged>> {
    PLUGGED.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Drop for SessionPads {
    fn drop(&mut self) {
        // The thread unplugs its pads as it ends.
        let stuck = self.thread.stop_within(RETIRE_TIMEOUT).err();
        let own = Arc::downgrade(&self.thread);
        let mut plugged = plugged();
        match stuck {
            Some(error) => {
                tracing::warn!("Nucleus Co-op: {error:#}");
                if let Some(set) = plugged.iter_mut().find(|p| p.thread.ptr_eq(&own)) {
                    set.abandoned = Some(self.thread.clone());
                }
            }
            None => plugged.retain(|p| !p.thread.ptr_eq(&own)),
        }
    }
}

/// The XInput indices ended sessions' pads hold: free once
/// [`Pads::retire_earlier`] unplugs them, as it does before a session plugs
/// its own.
fn held() -> Vec<u8> {
    plugged().iter().flat_map(|p| p.indices.clone()).collect()
}

impl Pads for SessionPads {
    fn send(&self, event: Input) {
        self.thread.send(event)
    }
    fn wait_ready(&self, mask: u16, timeout: Duration) -> bool {
        self.thread.wait_ready(mask, timeout)
    }
    fn reports(&self) -> Vec<PadReport> {
        self.thread.reports().collect()
    }
    fn retire_earlier(&self) -> Result<Vec<u8>> {
        let earlier: Vec<_> = {
            let plugged = plugged();
            let own = Arc::downgrade(&self.thread);
            plugged
                .iter()
                .filter(|p| !p.thread.ptr_eq(&own))
                .map(|p| (p.thread.clone(), p.indices.clone()))
                .collect()
        };
        // Each thread unplugs its pads as it ends; all are told at once and
        // share one deadline. A set the host is dropping meanwhile is waited
        // for there; [`plug`] waits for its indices either way.
        let threads: Vec<_> = earlier.iter().map(|(t, _)| t.upgrade()).collect();
        for thread in threads.iter().flatten() {
            thread.request_stop();
        }
        let deadline = Instant::now() + RETIRE_TIMEOUT;
        let mut indices = Vec::new();
        for ((weak, held), thread) in earlier.into_iter().zip(threads) {
            if let Some(thread) = thread {
                thread
                    .stop_within(deadline.saturating_duration_since(Instant::now()))
                    .context(
                        "an earlier Nucleus Co-op session's controllers are still plugged in",
                    )?;
            }
            plugged().retain(|p| !(p.abandoned.is_some() && p.thread.ptr_eq(&weak)));
            tracing::info!(
                xinput = ?held,
                "Nucleus Co-op: an earlier session's controllers unplugged"
            );
            indices.extend(held);
        }
        Ok(indices)
    }
    fn took(&self, indices: &[u8]) {
        let own = Arc::downgrade(&self.thread);
        if let Some(set) = plugged().iter_mut().find(|p| p.thread.ptr_eq(&own)) {
            set.indices = indices.to_vec();
        }
    }
    fn warn(&self, code: &str, message: &str) {
        self.warnings.set(code, message);
    }
}

/// The session's pads on the PC: [`Policy::session_pad`], whatever the
/// clients' controllers are.
///
/// [`Policy::session_pad`]: butterpollo_core::input_policy::Policy::session_pad
pub fn virtual_pads(
    policy: butterpollo_core::input_policy::Policy,
    warnings: Arc<Warnings>,
) -> Result<SessionPads> {
    let thread = GamepadThread::new(policy.session_pad, policy, warnings.clone())
        .context("start the virtual controllers for Nucleus Co-op")?;
    Ok(session_pads(thread, warnings))
}

/// `thread` as a session's pads, listed for later sessions.
fn session_pads(thread: GamepadThread, warnings: Arc<Warnings>) -> SessionPads {
    let thread = Arc::new(thread);
    plugged().push(Plugged {
        thread: Arc::downgrade(&thread),
        indices: Vec::new(),
        abandoned: None,
    });
    SessionPads { thread, warnings }
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

/// How many XInput indices the PC's own controllers leave free; those
/// ended sessions' pads hold count as free ([`Pads::retire_earlier`]).
pub fn free(xinput: &mut dyn XInput) -> Result<usize> {
    Ok(free_of(xinput.connected()?, &held()))
}

fn free_of(connected: [bool; MAX_PLAYERS], held: &[u8]) -> usize {
    (0..MAX_PLAYERS)
        .filter(|&i| !connected[i] || held.contains(&(i as u8)))
        .count()
}

/// How long XInput may take to show controllers [`released`] unplugged gone.
const RELEASE_SETTLE: Duration = Duration::from_secs(1);

/// Run `release`, which unplugs other streams' controllers and returns how
/// many pads they had plugged in, then wait until XInput shows that many
/// fewer controllers, so the session's [`free`] and [`plug`] don't see one
/// still leaving. Pads that never reached XInput (PlayStation and Switch
/// profiles) end the wait after [`RELEASE_SETTLE`].
pub fn released(xinput: &mut dyn XInput, release: impl FnOnce() -> Result<u32>) -> Result<()> {
    let count = |connected: [bool; MAX_PLAYERS]| connected.iter().filter(|c| **c).count();
    let before = count(xinput.connected()?);
    let plugged = release()? as usize;
    if plugged == 0 {
        return Ok(());
    }
    let wanted = before.saturating_sub(plugged);
    let deadline = Instant::now() + RELEASE_SETTLE;
    loop {
        let now = count(xinput.connected()?);
        if now <= wanted {
            return Ok(());
        }
        if Instant::now() >= deadline {
            tracing::info!(
                before,
                now,
                plugged,
                "Nucleus Co-op: some released controllers were not XInput controllers"
            );
            return Ok(());
        }
        std::thread::sleep(POLL);
    }
}

/// Plug one pad per player, in slot order, and learn the XInput index each
/// took. Plugged one at a time, so each new index belongs to the pad just
/// plugged; a controller connected meanwhile on the PC makes it ambiguous.
/// Earlier sessions' pads go first, so the players take the lowest indices
/// the PC's own controllers leave; a controller of the PC's ahead of a
/// player's is a warning on the stream ([`CONTROLLER_AHEAD`]).
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
    let retired = pads.retire_earlier()?;
    let deadline = Instant::now() + timeout;
    loop {
        let connected = xinput.connected()?;
        let Some(&index) = retired
            .iter()
            .find(|&&i| connected.get(usize::from(i)) == Some(&true))
        else {
            break;
        };
        ensure!(
            Instant::now() < deadline,
            "an earlier Nucleus Co-op session's virtual controller still holds XInput slot {}: start the stream again",
            index + 1
        );
        std::thread::sleep(POLL);
    }
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
        // Also those of a start that fails later, should its pads outlive it.
        pads.took(&indices);
    }
    if let Some(message) = ahead(xinput.connected()?, &indices) {
        tracing::warn!("Nucleus Co-op: {message}");
        pads.warn(CONTROLLER_AHEAD, &message);
    }
    Ok(indices)
}

/// What the player should know when a controller that is not the session's
/// holds a lower XInput index than a player's pad: a game that reads XInput
/// past Nucleus Co-op's hooks takes the first controller, that one.
fn ahead(connected: [bool; MAX_PLAYERS], indices: &[u8]) -> Option<String> {
    let first = indices.iter().copied().min()?;
    let other = (0..first).find(|&i| connected[usize::from(i)])?;
    Some(format!(
        "A controller on the PC holds XInput slot {} ahead of the players' virtual controllers: if the game doesn't respond to your controller, disconnect the PC's controllers or end the PC's other streams, then start again",
        other + 1
    ))
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
        | Input::Pen { .. }
        | Input::Touchpad { .. } => Err(Dropped::KeyboardMouse),
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
        /// XInput indices an earlier session's pads hold until retired.
        pub earlier: Mutex<Vec<u8>>,
        /// Pads of an earlier session that never unplug.
        pub stuck: bool,
        /// An earlier session's pad thread that never ends.
        pub hung: bool,
        pub took: Mutex<Vec<u8>>,
        pub warned: Mutex<Vec<(String, String)>>,
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
        fn retire_earlier(&self) -> Result<Vec<u8>> {
            ensure!(!self.hung, "the virtual controllers did not unplug");
            let mut earlier = self.earlier.lock().unwrap();
            Ok(if self.stuck {
                earlier.clone()
            } else {
                std::mem::take(&mut earlier)
            })
        }
        fn took(&self, indices: &[u8]) {
            *self.took.lock().unwrap() = indices.to_vec();
        }
        fn warn(&self, code: &str, message: &str) {
            self.warned
                .lock()
                .unwrap()
                .push((code.to_string(), message.to_string()));
        }
    }

    /// XInput that shows `pads`' arrivals at the first free index, behind
    /// the PC's own controllers in `taken` and an earlier session's pads.
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
            for &index in self.pads.earlier.lock().unwrap().iter() {
                connected[usize::from(index)] = true;
            }
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
        assert_eq!(*pads.took.lock().unwrap(), vec![0, 2, 3]);
        assert!(pads.warned.lock().unwrap().is_empty());
    }

    /// #56: the pads of the sessions before still held XInput 0 and 1, so
    /// the player's pad took 2 and the game read a pad nobody held.
    #[test]
    fn earlier_sessions_pads_go_before_the_players_take_the_first_index() {
        let pads = Recorder {
            earlier: Mutex::new(vec![0, 1]),
            ..Default::default()
        };
        let mut xinput = Fake {
            pads: &pads,
            taken: [false; 4],
            hidden: false,
            intruder: false,
        };
        assert_eq!(free_of(xinput.connected().unwrap(), &[]), 2);
        assert_eq!(free_of(xinput.connected().unwrap(), &[0, 1]), 4);
        assert_eq!(plug(&pads, &mut xinput, 1, PLUG_TIMEOUT).unwrap(), vec![0]);
        assert!(pads.earlier.lock().unwrap().is_empty());
        assert_eq!(*pads.took.lock().unwrap(), vec![0]);
        assert!(pads.warned.lock().unwrap().is_empty());

        // Ones that stay plugged fail the start, never shift the player.
        let stuck = Recorder {
            earlier: Mutex::new(vec![1]),
            stuck: true,
            ..Default::default()
        };
        let mut xinput = Fake {
            pads: &stuck,
            taken: [false; 4],
            hidden: false,
            intruder: false,
        };
        let error = plug(&stuck, &mut xinput, 1, Duration::from_millis(50)).unwrap_err();
        assert!(
            error.to_string().contains("still holds XInput slot 2"),
            "{error}"
        );
        assert!(stuck.sent.lock().unwrap().is_empty());

        // Their thread stuck in the driver fails the start too, at once.
        let hung = Recorder {
            earlier: Mutex::new(vec![0]),
            hung: true,
            ..Default::default()
        };
        let mut xinput = Fake {
            pads: &hung,
            taken: [false; 4],
            hidden: false,
            intruder: false,
        };
        assert!(plug(&hung, &mut xinput, 1, PLUG_TIMEOUT).is_err());
        assert!(hung.sent.lock().unwrap().is_empty());
    }

    /// XInput that shows `gone` controllers fewer after `reads` reads.
    struct Leaving {
        connected: [bool; MAX_PLAYERS],
        reads: usize,
    }
    impl XInput for Leaving {
        fn connected(&mut self) -> Result<[bool; MAX_PLAYERS]> {
            self.reads = self.reads.saturating_sub(1);
            if self.reads == 0 {
                self.connected[0] = false;
            }
            Ok(self.connected)
        }
    }

    /// #56: the desktop stream's parked pad held XInput 0; the session waits
    /// until it is gone before counting and plugging.
    #[test]
    fn released_controllers_are_waited_for_until_xinput_shows_them_gone() {
        let mut xinput = Leaving {
            connected: [true, true, false, false],
            reads: 4,
        };
        let started = Instant::now();
        released(&mut xinput, || Ok(1)).unwrap();
        assert_eq!(xinput.connected, [false, true, false, false]);
        assert!(started.elapsed() < RELEASE_SETTLE);
        // Nothing plugged in: no wait. One that never reached XInput: the
        // wait ends after the bound.
        let mut xinput = Leaving {
            connected: [false, true, false, false],
            reads: usize::MAX,
        };
        released(&mut xinput, || Ok(0)).unwrap();
        let started = Instant::now();
        released(&mut xinput, || Ok(1)).unwrap();
        assert!(started.elapsed() >= RELEASE_SETTLE);
        assert!(released(&mut xinput, || anyhow::bail!("stuck")).is_err());
    }

    /// The real sets: an earlier session's pad thread stuck in the driver
    /// fails the next launch after [`RETIRE_TIMEOUT`], the launches after it
    /// at once (never blocked by it), and none once it ended.
    #[test]
    fn a_stuck_earlier_set_is_waited_for_once_and_retired_once_it_ends() {
        let abandoned = || plugged().iter().filter(|p| p.abandoned.is_some()).count();
        let (stuck, gate, log) = crate::input::fake_pads();
        let earlier = session_pads(stuck, Default::default());
        earlier.send(Input::Arrival {
            id: 0,
            kind: XBOX,
            capabilities: CAPABILITIES,
            buttons: 0,
        });
        gate.wait_entered(1);
        let (thread, next_gate, _) = crate::input::fake_pads();
        next_gate.open();
        let next = session_pads(thread, Default::default());

        let started = Instant::now();
        let error = format!("{:#}", next.retire_earlier().unwrap_err());
        assert!(
            error
                .starts_with("an earlier Nucleus Co-op session's controllers are still plugged in")
                && error.ends_with("restart Rubylight and start again"),
            "{error}"
        );
        assert!(started.elapsed() >= RETIRE_TIMEOUT - Duration::from_millis(100));
        let started = Instant::now();
        assert!(next.retire_earlier().is_err());
        // The host drops the earlier session meanwhile: not waited for, still
        // listed.
        drop(earlier);
        assert!(next.retire_earlier().is_err());
        assert!(started.elapsed() < Duration::from_secs(2));
        assert_eq!(abandoned(), 1);

        gate.open();
        gate.wait_until(|| next.retire_earlier().is_ok());
        assert!(log.lock().unwrap().iter().any(|e| e == "unplugged"));
        assert_eq!(abandoned(), 0);
    }

    #[test]
    fn a_pc_controller_ahead_of_the_players_is_a_warning() {
        let pads = Recorder::default();
        let mut xinput = Fake {
            pads: &pads,
            taken: [true, false, false, false],
            hidden: false,
            intruder: false,
        };
        assert_eq!(
            plug(&pads, &mut xinput, 2, PLUG_TIMEOUT).unwrap(),
            vec![1, 2]
        );
        let warned = pads.warned.lock().unwrap();
        assert_eq!(warned.len(), 1, "{warned:?}");
        assert_eq!(warned[0].0, CONTROLLER_AHEAD);
        assert!(
            warned[0].1.contains("XInput slot 1 ahead"),
            "{}",
            warned[0].1
        );
        // Controllers behind the players' are no matter.
        assert_eq!(ahead([true, true, true, false], &[0, 1]), None);
        assert_eq!(ahead([false, true, false, false], &[1]), None);
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
        let pads = virtual_pads(policy.clone(), warnings.clone())?;
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
        // #56: the next session's pads take the same indices, the first
        // set's unplugged before them while the host still holds it.
        let next = virtual_pads(policy, warnings.clone())?;
        ensure!(
            free(&mut xinput)? >= players,
            "the first set's indices count as free"
        );
        let again = plug(&next, &mut xinput, players, PLUG_TIMEOUT)?;
        ensure!(
            again == indices,
            "the next set took {again:?}, not {indices:?}"
        );
        drop(next);
        drop(pads);
        let deadline = Instant::now() + PLUG_TIMEOUT;
        while xinput.connected()? != before {
            ensure!(Instant::now() < deadline, "the pads did not unplug");
            std::thread::sleep(POLL);
        }
        Ok(())
    }

    /// #74: `gamepad = vigem_x360` holds in a co-op or background stream too.
    #[test]
    #[ignore = "plugs a pad on the installed ViGEmBus; needs an idle host"]
    fn a_session_with_the_vigem_pad_plugs_one_on_this_pc() -> Result<()> {
        let config = butterpollo_core::config::Config::parse("gamepad=vigem_x360\n")?;
        let policy = butterpollo_core::input_policy::Policy::resolve(&config)?;
        let warnings = std::sync::Arc::new(butterpollo_core::session::Warnings::default());
        let pads = virtual_pads(policy, warnings)?;
        let mut xinput = System::open()?;
        let before = xinput.connected()?;
        let indices = plug(&pads, &mut xinput, 1, PLUG_TIMEOUT)?;
        ensure!(indices.len() == 1, "{indices:?}");
        let listed = std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                r"Get-PnpDevice -PresentOnly -InstanceId 'USB\VID_045E&PID_028E*' | ForEach-Object InstanceId",
            ])
            .output()?;
        let listed = String::from_utf8_lossy(&listed.stdout);
        ensure!(listed.contains(r"USB\VID_045E&PID_028E"), "{listed}");
        drop(pads);
        let deadline = Instant::now() + PLUG_TIMEOUT;
        while xinput.connected()? != before {
            ensure!(Instant::now() < deadline, "the pad did not unplug");
            std::thread::sleep(POLL);
        }
        Ok(())
    }
}
