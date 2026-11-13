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
        error.starts_with("an earlier Nucleus Co-op session's controllers are still plugged in")
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
