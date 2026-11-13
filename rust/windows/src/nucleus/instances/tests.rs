use super::*;

pub(crate) const SCREEN: Rect = Rect {
    x: 0,
    y: 0,
    width: 2560,
    height: 1440,
};

/// A desktop that is only data: tests change it between passes.
#[derive(Default)]
pub(crate) struct Fake {
    pub processes: Vec<Process>,
    pub paths: BTreeMap<u32, String>,
    pub windows: Vec<Window>,
    pub placed: Vec<(u64, Rect)>,
    /// The windows whose frames were taken away, in order.
    pub unframed: Vec<u64>,
    /// Frames come back as soon as they are taken away, as a game that
    /// sets its own window style does.
    pub keep_frames: bool,
    pub unthrottled: BTreeSet<u32>,
    pub labels: Vec<(u32, String, u64)>,
    pub buttons: Vec<(u32, String, u64)>,
    pub clicks: Vec<u64>,
    pub covered: bool,
    /// Message boxes and the process showing each; [`CSRSS`]'s are those
    /// Windows shows for any program.
    pub boxes: Vec<(u32, MessageBox)>,
    /// Listing message boxes fails.
    pub unlisted: bool,
    /// Forms that disable their owner, and the process showing each.
    pub forms: Vec<(u32, MessageBox)>,
    pub closed: Vec<u64>,
    /// Whether Windows forces Mandatory ASLR on each process; one not
    /// listed can't be read.
    pub aslr: BTreeMap<u32, bool>,
    /// Placing a window moves it there, as a cooperative game does.
    pub obey: bool,
    /// The windows brought to the front, in order.
    pub fronted: Vec<u64>,
    /// Someone uses the PC: no window is brought to the front.
    pub pc_busy: bool,
}
impl Desktop for Fake {
    fn processes(&mut self) -> Result<Vec<Process>> {
        Ok(self.processes.clone())
    }
    fn image_path(&mut self, pid: u32) -> Option<String> {
        self.paths.get(&pid).cloned()
    }
    fn windows(&mut self) -> Result<Vec<Window>> {
        Ok(self.windows.clone())
    }
    fn monitors(&mut self) -> Result<Vec<Rect>> {
        Ok(vec![SCREEN])
    }
    fn virtual_screen(&mut self) -> Rect {
        SCREEN
    }
    fn place(&mut self, window: u64, to: Rect) -> Result<()> {
        self.placed.push((window, to));
        if self.obey
            && let Some(w) = self.windows.iter_mut().find(|w| w.id == window)
        {
            w.rect = to;
            w.visible |= w.minimized;
            w.minimized = false;
        }
        Ok(())
    }
    fn unframe(&mut self, window: u64) -> Result<()> {
        self.unframed.push(window);
        if !self.keep_frames
            && let Some(w) = self.windows.iter_mut().find(|w| w.id == window)
        {
            w.framed = false;
        }
        Ok(())
    }
    fn unthrottle(&mut self, processes: &BTreeSet<u32>) {
        self.unthrottled = processes.clone();
    }
    fn label(&mut self, processes: &BTreeSet<u32>, text: &str) -> Result<Option<u64>> {
        let mut found = self
            .labels
            .iter()
            .filter(|(pid, t, _)| processes.contains(pid) && t == text);
        let first = found.next().map(|(_, _, id)| *id);
        anyhow::ensure!(found.next().is_none(), "two labels");
        Ok(first)
    }
    fn button(&mut self, processes: &BTreeSet<u32>, name: &str) -> Result<Option<u64>> {
        Ok(self
            .buttons
            .iter()
            .find(|(pid, n, _)| processes.contains(pid) && n == name)
            .map(|(_, _, id)| *id))
    }
    fn click(&mut self, label: u64) -> Result<()> {
        anyhow::ensure!(!self.covered, "covered");
        self.clicks.push(label);
        Ok(())
    }
    fn to_front(&mut self, window: u64) -> bool {
        self.fronted.push(window);
        true
    }
    fn pc_idle(&mut self) -> bool {
        !self.pc_busy
    }
    fn message_boxes(&mut self, processes: &BTreeSet<u32>) -> Result<Vec<MessageBox>> {
        anyhow::ensure!(!self.unlisted, "access denied");
        Ok(self
            .boxes
            .iter()
            .filter(|(pid, _)| processes.contains(pid) || *pid == CSRSS)
            .map(|(_, shown)| shown.clone())
            .collect())
    }
    fn close_forms(&mut self, processes: &BTreeSet<u32>) -> Result<Vec<MessageBox>> {
        let (closed, kept) = std::mem::take(&mut self.forms)
            .into_iter()
            .partition::<Vec<_>, _>(|(pid, _)| processes.contains(pid));
        self.forms = kept;
        let closed: Vec<MessageBox> = closed.into_iter().map(|(_, form)| form).collect();
        self.closed.extend(closed.iter().map(|form| form.window));
        Ok(closed)
    }
    fn owner(&mut self, window: u64) -> Result<u32> {
        let (pid, _) = self
            .boxes
            .iter()
            .find(|(_, shown)| shown.window == window)
            .ok_or_else(|| anyhow::anyhow!("no such window"))?;
        Ok(*pid)
    }
    fn forced_aslr(&mut self, pid: u32) -> Result<bool> {
        let forced = self.aslr.get(&pid).copied();
        forced.ok_or_else(|| anyhow::anyhow!("access denied"))
    }
}

/// The process that shows the boxes Windows raises for a program.
pub(crate) const CSRSS: u32 = 600;

#[test]
fn a_message_box_is_quoted_on_one_short_line() {
    let shown = |caption: &str, text: &str| MessageBox {
        window: 1,
        caption: caption.into(),
        text: text.into(),
    };
    assert_eq!(
        shown(
            "OneDrive must be disabled",
            "Your Documents folder\r\n  is in OneDrive."
        )
        .summary(),
        "OneDrive must be disabled: Your Documents folder is in OneDrive."
    );
    assert_eq!(shown("", "Text only").summary(), "Text only");
    assert_eq!(shown("Caption only", " ").summary(), "Caption only");
    let long = shown("Error", &"é".repeat(400)).summary();
    assert_eq!(long.chars().count(), 303);
    assert!(long.ends_with("é..."), "{long}");
}

pub(crate) fn fake(processes: Vec<Process>, windows: Vec<Window>) -> Fake {
    Fake {
        processes,
        windows,
        ..Default::default()
    }
}
pub(crate) fn process(pid: u32, parent: u32, started: u64, name: &str) -> Process {
    Process {
        pid,
        parent,
        started,
        name: name.into(),
    }
}
pub(crate) fn window(id: u64, pid: u32, rect: Rect) -> Window {
    Window {
        id,
        pid,
        rect,
        visible: true,
        minimized: false,
        owned: false,
        tool: false,
        framed: false,
    }
}
pub(crate) const ON_SCREEN: Rect = Rect {
    x: 100,
    y: 100,
    width: 1280,
    height: 720,
};
const SIZES: [(u32, u32); 2] = [(1920, 1080), (1280, 720)];

fn keeper(players: usize) -> Keeper {
    Keeper::new(players, &["Game.exe".into()], r"C:\Nucleus\content", 100)
}

#[test]
fn instance_folders_name_their_player() {
    assert_eq!(
        instance_number(r"C:\Nucleus\content\BL2\Instance1\Binaries\Game.exe"),
        Some(1)
    );
    assert_eq!(
        instance_number(r"c:/n/content/x/instance12/game.exe"),
        Some(12)
    );
    assert_eq!(instance_number(r"D:\Games\BL2\Game.exe"), None);
    assert_eq!(instance_number(r"D:\Instances\Game.exe"), None);
    assert_eq!(instance_number(r"D:\x\Instance2.exe"), None);
}

#[test]
fn instances_take_their_folders_slot_and_keep_their_children() {
    let mut keeper = keeper(2);
    // Nucleus (10) started instance 1 first, then instance 0; each game
    // started a helper of the same name (a crash reporter), and the PC
    // user's own copy of the game (5, older than Nucleus) runs too.
    let mut desktop = Fake {
        processes: vec![
            process(5, 1, 50, "Game.exe"),
            process(10, 1, 100, "NucleusCoop.exe"),
            process(20, 10, 110, "Game.exe"),
            process(21, 20, 111, "Game.exe"),
            process(30, 10, 120, "game.exe"),
        ],
        paths: BTreeMap::from([
            (5, r"D:\Games\Game.exe".into()),
            (20, r"C:\Nucleus\content\G\Instance1\Game.exe".into()),
            (21, r"C:\Nucleus\content\G\Instance1\Game.exe".into()),
            (30, r"C:\Nucleus\content\G\Instance0\Game.exe".into()),
        ]),
        windows: vec![
            window(1, 5, ON_SCREEN),
            window(2, 21, ON_SCREEN),
            window(3, 30, ON_SCREEN),
        ],
        ..Default::default()
    };
    let job = BTreeSet::from([10, 20, 21, 30]);
    let pass = keeper.pass(&mut desktop, &job, &SIZES).unwrap();
    assert_eq!(pass.windows, [Some(3), Some(2)]);
    assert_eq!(
        pass.roots,
        [
            Some(Identity {
                pid: 30,
                started: 120
            }),
            Some(Identity {
                pid: 20,
                started: 110
            })
        ]
    );
    assert_eq!(desktop.unthrottled, BTreeSet::from([20, 21, 30]));
    // The session stops these, never the PC user's own copy.
    assert_eq!(
        keeper.processes(),
        BTreeMap::from([(20, 110), (21, 111), (30, 120)])
    );
    // Parked beside the desktop at each player's stream size; the PC
    // user's window is left alone.
    assert_eq!(
        desktop.placed,
        [
            (
                3,
                Rect {
                    x: 2624,
                    y: 0,
                    width: 1920,
                    height: 1080
                }
            ),
            (
                2,
                Rect {
                    x: 2560 + 64 + 1920 + 64,
                    y: 0,
                    width: 1280,
                    height: 720
                }
            )
        ]
    );
}

#[test]
fn without_instance_folders_instances_fill_slots_in_start_order() {
    let mut keeper = keeper(2);
    let mut desktop = fake(
        vec![
            process(10, 1, 100, "NucleusCoop.exe"),
            process(31, 10, 130, "Game.exe"),
            process(30, 10, 120, "Game.exe"),
        ],
        vec![window(4, 31, ON_SCREEN), window(3, 30, ON_SCREEN)],
    );
    let job = BTreeSet::from([10, 30, 31]);
    let pass = keeper.pass(&mut desktop, &job, &SIZES).unwrap();
    assert_eq!(pass.windows, [Some(3), Some(4)]);
    // Outside the job and not below Nucleus' content folder: not an instance.
    let mut keeper = self::keeper(1);
    desktop.processes = vec![process(40, 1, 140, "Game.exe")];
    desktop.windows = vec![window(5, 40, ON_SCREEN)];
    let pass = keeper.pass(&mut desktop, &BTreeSet::new(), &SIZES).unwrap();
    assert_eq!(pass.windows, [None]);
}

#[test]
fn a_closed_window_is_replaced_and_an_exited_instance_reported() {
    let mut keeper = keeper(1);
    let mut desktop = Fake {
        obey: true,
        ..fake(
            vec![process(20, 10, 110, "Game.exe")],
            vec![window(2, 20, ON_SCREEN)],
        )
    };
    let job = BTreeSet::from([20]);
    assert_eq!(
        keeper.pass(&mut desktop, &job, &SIZES).unwrap().windows,
        [Some(2)]
    );
    // The game recreates its window (a mode change): the new one is followed and parked.
    desktop.windows = vec![window(7, 20, ON_SCREEN)];
    assert_eq!(
        keeper.pass(&mut desktop, &job, &SIZES).unwrap().windows,
        [Some(7)]
    );
    assert_eq!(desktop.placed.last().unwrap().0, 7);
    // No window for a while (loading) is not an exit.
    desktop.windows.clear();
    let pass = keeper.pass(&mut desktop, &job, &SIZES).unwrap();
    assert_eq!((pass.windows, pass.gone), (vec![None], vec![]));
    desktop.processes.clear();
    let pass = keeper.pass(&mut desktop, &job, &SIZES).unwrap();
    assert_eq!(pass.gone, [0]);
    assert_eq!(pass.windows, [None]);
    // Reported once.
    assert!(
        keeper
            .pass(&mut desktop, &job, &SIZES)
            .unwrap()
            .gone
            .is_empty()
    );
}

#[test]
fn a_launcher_that_exits_leaves_the_game_as_the_instance() {
    let mut keeper = Keeper::new(
        1,
        &["Launcher.exe".into(), "Game.exe".into()],
        r"C:\N\content",
        100,
    );
    let mut desktop = fake(
        vec![
            process(20, 10, 110, "Launcher.exe"),
            process(21, 20, 111, "Game.exe"),
        ],
        vec![window(2, 21, ON_SCREEN)],
    );
    let job = BTreeSet::from([20, 21]);
    let pass = keeper.pass(&mut desktop, &job, &SIZES).unwrap();
    assert_eq!(
        pass.roots,
        [Some(Identity {
            pid: 20,
            started: 110
        })]
    );
    desktop.processes.remove(0);
    let pass = keeper.pass(&mut desktop, &job, &SIZES).unwrap();
    assert!(pass.gone.is_empty());
    assert_eq!(
        pass.roots,
        [Some(Identity {
            pid: 21,
            started: 111
        })]
    );
    assert_eq!(pass.windows, [Some(2)]);
}

#[test]
fn a_reused_pid_is_not_the_instance() {
    let mut keeper = keeper(1);
    let mut desktop = fake(vec![process(20, 10, 110, "Game.exe")], Vec::new());
    let job = BTreeSet::from([20]);
    keeper.pass(&mut desktop, &job, &SIZES).unwrap();
    // The instance exited and Windows gave its pid to an unrelated process.
    desktop.processes = vec![process(20, 3, 500, "Other.exe")];
    assert_eq!(keeper.pass(&mut desktop, &job, &SIZES).unwrap().gone, [0]);
}

/// #56: a borderless fullscreen game kept returning to the PC's monitor,
/// and was left there with the PC's keyboard and mouse after 20 tries.
#[test]
fn a_window_that_keeps_returning_to_a_monitor_is_put_back_less_often_never_left() {
    let mut keeper = keeper(1);
    let mut desktop = fake(
        vec![process(20, 10, 110, "Game.exe")],
        vec![window(2, 20, ON_SCREEN)],
    );
    let job = BTreeSet::from([20]);
    let mut stuck = Vec::new();
    for _ in 0..TRIES + 2 * BACKOFF {
        stuck.extend(keeper.pass(&mut desktop, &job, &SIZES).unwrap().stuck);
    }
    assert!(stuck.is_empty(), "{stuck:?}");
    assert_eq!(desktop.placed.len(), (TRIES + 2) as usize);

    // One that keeps its own size beside the desktop, off the monitors
    // (Sekiro), is left there: it is out of the PC's reach.
    let mut keeper = self::keeper(1);
    let parked = window::parking(SCREEN, 0, SIZES[0]).unwrap();
    desktop.windows = vec![window(
        2,
        20,
        Rect {
            width: 1940,
            ..parked
        },
    )];
    desktop.placed.clear();
    for _ in 0..TRIES + 2 * BACKOFF {
        stuck.extend(keeper.pass(&mut desktop, &job, &SIZES).unwrap().stuck);
    }
    assert_eq!(stuck, [0]);
    assert_eq!(desktop.placed.len(), TRIES as usize);
    // Until it reaches a monitor again.
    desktop.windows = vec![window(2, 20, ON_SCREEN)];
    keeper.pass(&mut desktop, &job, &SIZES).unwrap();
    assert_eq!(desktop.placed.len(), TRIES as usize + 1);

    // A window that stays where it was parked is moved once.
    let mut keeper = self::keeper(1);
    desktop.obey = true;
    desktop.placed.clear();
    for _ in 0..3 {
        keeper.pass(&mut desktop, &job, &SIZES).unwrap();
    }
    assert_eq!(desktop.placed.len(), 1);
}

/// #56: a fullscreen game minimised itself when it lost the focus, and
/// the stream lost its window.
#[test]
fn a_window_that_minimises_itself_is_kept_and_restored() {
    let mut keeper = keeper(1);
    let mut desktop = Fake {
        obey: true,
        ..fake(
            vec![process(20, 10, 110, "Game.exe")],
            vec![window(2, 20, ON_SCREEN)],
        )
    };
    let job = BTreeSet::from([20]);
    keeper.pass(&mut desktop, &job, &SIZES).unwrap();
    let parked = desktop.windows[0].rect;
    assert_eq!(desktop.placed.len(), 1);
    desktop.windows[0] = Window {
        visible: false,
        minimized: true,
        ..window(
            2,
            20,
            Rect {
                x: -32000,
                y: -32000,
                width: 160,
                height: 28,
            },
        )
    };
    let pass = keeper.pass(&mut desktop, &job, &SIZES).unwrap();
    assert_eq!(pass.windows, [Some(2)]);
    assert_eq!(desktop.placed.last(), Some(&(2, parked)));
    assert!(desktop.windows[0].visible && !desktop.windows[0].minimized);
    // Minimised again, but in place: still restored.
    desktop.windows[0].visible = false;
    desktop.windows[0].minimized = true;
    keeper.pass(&mut desktop, &job, &SIZES).unwrap();
    assert_eq!(desktop.placed.len(), 3);
    // Minimised beside another window of the instance (a launcher that
    // minimised itself when the game opened): the other is the window.
    desktop.windows[0].visible = false;
    desktop.windows[0].minimized = true;
    desktop.windows.push(window(3, 20, ON_SCREEN));
    assert_eq!(
        keeper.pass(&mut desktop, &job, &SIZES).unwrap().windows,
        [Some(3)]
    );
    assert_eq!(desktop.placed.last().unwrap().0, 3);
    // A minimised window it never followed is no instance's window yet.
    let mut keeper = self::keeper(1);
    desktop.windows.truncate(1);
    assert_eq!(
        keeper.pass(&mut desktop, &job, &SIZES).unwrap().windows,
        [None]
    );
}

/// #56: Split Fiction started in a window with a title bar, which its
/// stream showed above the game.
#[test]
fn a_framed_window_loses_its_frame_then_takes_its_place() {
    let mut keeper = keeper(1);
    let parked = window::parking(SCREEN, 0, SIZES[0]).unwrap();
    let mut desktop = Fake {
        obey: true,
        ..fake(
            vec![process(20, 10, 110, "Game.exe")],
            vec![Window {
                framed: true,
                ..window(2, 20, ON_SCREEN)
            }],
        )
    };
    let job = BTreeSet::from([20]);
    keeper.pass(&mut desktop, &job, &SIZES).unwrap();
    assert_eq!(desktop.unframed, [2]);
    assert_eq!(desktop.placed, [(2, parked)]);
    assert!(!desktop.windows[0].framed);
    // Settled: left alone.
    keeper.pass(&mut desktop, &job, &SIZES).unwrap();
    assert_eq!((desktop.unframed.len(), desktop.placed.len()), (1, 1));
    // Already in its place, a frame alone is taken away too.
    desktop.windows[0].framed = true;
    keeper.pass(&mut desktop, &job, &SIZES).unwrap();
    assert_eq!(desktop.unframed, [2, 2]);
    // A game that puts its frame back each time is left framed beside
    // the desktop after its tries, out of the PC's reach.
    let mut keeper = self::keeper(1);
    desktop.keep_frames = true;
    desktop.windows[0].framed = true;
    desktop.unframed.clear();
    let mut stuck = Vec::new();
    for _ in 0..TRIES + 2 * BACKOFF {
        stuck.extend(keeper.pass(&mut desktop, &job, &SIZES).unwrap().stuck);
    }
    assert_eq!(stuck, [0]);
    assert_eq!(desktop.unframed.len(), TRIES as usize);
}

#[test]
fn a_stream_too_large_to_park_is_an_error() {
    let mut keeper = keeper(1);
    let mut desktop = fake(
        vec![process(20, 10, 110, "Game.exe")],
        vec![window(2, 20, ON_SCREEN)],
    );
    let error = keeper
        .pass(&mut desktop, &BTreeSet::from([20]), &[(40000, 1080)])
        .unwrap_err()
        .to_string();
    assert!(error.contains("does not fit"), "{error}");
}
