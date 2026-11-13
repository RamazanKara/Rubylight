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
pub(crate) mod tests;
