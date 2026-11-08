//! Dual screen mode, as on Android (`DualScreen.java`): with a second monitor and a PC that keeps a
//! device's second stream beside its first (`Feature::SecondScreen`), a stream starts a second one
//! of the same game, full screen on the other monitor.
//!
//! The second stream is a second Nightjar process, started from the first with the command line
//! of a frontend start (`--host`, `--app`) and `--second-screen=<x>,<y>`, a point on its monitor.
//! A process of its own because a stream owns its window, renderer, decoder, audio and input, and
//! every one of them is one per process here. It reads the same settings, PCs and identity, so the
//! PC sees the same paired device, and every request it makes carries `rlSecondScreen=1`
//! (`HostClient::with_second_screen`). It writes none of those files, keeps its log in
//! `logs/second-screen`, plays no sound, takes no controllers, microphone, USB devices or
//! clipboard, and ends when the first closes its standard input: the first stream's end, or the
//! first process gone for any reason. The first line on that input hands it the first's open PIN
//! lock (`PinStore::hand_over`), so the second screen does not ask for the PIN again.

use std::ffi::OsString;
use std::io::{BufRead, IsTerminal as _, Read as _, Write as _};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc::{channel, Receiver};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use nightjar_core::http::{App, AppControl};

use crate::config::{HostRecord, Settings, WindowMode};
use crate::window_place::Rect;

/// `--second-screen=<x>,<y>`: this process is the second screen's, its window on the monitor
/// that holds the point.
const FLAG: &str = "--second-screen=";

/// `--second-screen-join=<app id>`: the first screen joined the game the PC runs for another
/// device, and the second joins it too, by the id the first streams.
const JOIN_FLAG: &str = "--second-screen-join=";

/// How long the second screen gets to end its stream after the first closed its input.
const END_WAIT: Duration = Duration::from_secs(10);

/// How long the second screen waits for the first's line on its input as it starts. The first
/// writes it right after the start, so this only bounds a first that broke.
pub const HAND_OVER_WAIT: Duration = Duration::from_secs(5);

/// The longest line the first writes: a PIN lock token, 64 hex digits.
const LINE_MAX: u64 = 256;

/// The point of the second screen's monitor, in this process only.
static THIS: OnceLock<(i32, i32)> = OnceLock::new();

/// Marks this process as the second screen's, before anything reads [`this_process`].
pub fn mark(point: (i32, i32)) {
    let _ = THIS.set(point);
}

/// Whether this process streams the second screen.
pub fn this_process() -> bool {
    THIS.get().is_some()
}

/// The point on the second screen's monitor, in this process only.
pub fn point() -> Option<(i32, i32)> {
    THIS.get().copied()
}

/// The point `--second-screen` gave, if any, or why it is no point.
pub type Flag = Result<Option<(i32, i32)>, String>;

/// Takes `--second-screen=<x>,<y>` out of the command line.
pub fn split_flag(mut args: Vec<OsString>) -> (Vec<OsString>, Flag) {
    let mut found: Flag = Ok(None);
    args.retain(|arg| {
        let Some(value) = arg.to_str().and_then(|a| a.strip_prefix(FLAG)) else { return true };
        let point = value.split_once(',').and_then(|(x, y)| Some((x.trim().parse().ok()?, y.trim().parse().ok()?)));
        found = match (&found, point) {
            (Ok(None), Some(point)) => Ok(Some(point)),
            (Ok(Some(_)), _) => Err("--second-screen is given twice".into()),
            (Err(e), _) => Err(e.clone()),
            (_, None) => Err(format!("--second-screen needs a point such as {FLAG}1920,0")),
        };
        false
    });
    (args, found)
}

/// Takes `--second-screen-join=<app id>` out of the command line.
pub fn split_join(mut args: Vec<OsString>) -> (Vec<OsString>, Result<Option<i32>, String>) {
    let mut found: Result<Option<i32>, String> = Ok(None);
    args.retain(|arg| {
        let Some(value) = arg.to_str().and_then(|a| a.strip_prefix(JOIN_FLAG)) else { return true };
        found = match (&found, value.trim().parse::<i32>()) {
            (Ok(None), Ok(id)) => Ok(Some(id)),
            (Ok(Some(_)), _) => Err("--second-screen-join is given twice".into()),
            (Err(e), _) => Err(e.clone()),
            (_, Err(_)) => Err(format!("--second-screen-join needs an app id such as {JOIN_FLAG}12345")),
        };
        false
    });
    (args, found)
}

/// The second screen's command line: the PC and the game by UUID or name, as a frontend gives them,
/// and the game's id when the first joined it.
pub fn args(host: &str, app: &str, point: (i32, i32), join: Option<i32>) -> Vec<OsString> {
    ["--host", host, "--app", app].into_iter().map(OsString::from)
        .chain([OsString::from(format!("{FLAG}{},{}", point.0, point.1))])
        .chain(join.map(|id| OsString::from(format!("{JOIN_FLAG}{id}"))))
        .collect()
}

/// The PC and the game for the second screen's command line, each by UUID, else by name. A control
/// tile (Resume) names the game the PC runs, which is the one the first screen plays.
pub fn target(host: &HostRecord, app: &App, apps: &[App], running: Option<(i32, Option<&str>)>) -> Result<(String, String), String> {
    let pc = if host.unique_id.is_empty() { &host.name } else { &host.unique_id };
    let game = if app.control == AppControl::None {
        Some(app)
    } else {
        running.and_then(|(id, uuid)| apps.iter().find(|a| a.control == AppControl::None && a.matches_running_app(id, uuid)))
    };
    let game = game.ok_or_else(|| format!("{} names no game of the PC's list", app.name))?;
    let game = if game.uuid.is_empty() { &game.name } else { &game.uuid };
    if pc.is_empty() || game.is_empty() {
        return Err("the PC or the game has neither a UUID nor a name".into());
    }
    Ok((pc.clone(), game.clone()))
}

/// The running game `target` looks for: the one the PC reports, else the one the first joined
/// (another device's, which the PC does not report to this one).
pub fn joined_running(running: Option<(i32, Option<&str>)>, join: Option<i32>) -> Option<(i32, Option<&str>)> {
    running.or(join.map(|id| (id, None)))
}

/// The monitor for the second screen: the first one that is not `main`'s, as its centre.
pub fn pick_display<Id: PartialEq>(displays: &[(Id, Rect)], main: Id) -> Option<(i32, i32)> {
    displays.iter().find(|(id, rect)| *id != main && rect.w > 0 && rect.h > 0)
        .map(|(_, r)| (r.x + r.w / 2, r.y + r.h / 2))
}

/// What the second screen's stream gives up to the first: sound, the controllers, the microphone,
/// USB devices and the clipboard stay with the first. Its size is the Second screen resolution
/// (0x0: its monitor's own), whatever the first's is, and it fills its monitor.
pub fn stream_settings(settings: &mut Settings) {
    settings.width = settings.second_screen_width;
    settings.height = settings.second_screen_height;
    settings.dual_screen = false;
    settings.mute_audio = true;
    settings.microphone = false;
    settings.controllers = false;
    settings.controller_audio = false;
    settings.usb_forwarding = false;
    settings.clipboard_sync = false;
    settings.window_mode = WindowMode::Borderless;
}

/// The second screen's process, seen from the first's.
pub struct Companion {
    child: Child,
    /// Closed to end the second screen.
    input: Option<ChildStdin>,
}

impl Companion {
    /// `hand_over` is the PIN lock's token for the second screen, empty without a lock; it goes
    /// on the process's input, which no other process reads.
    pub fn start(host: &str, app: &str, point: (i32, i32), join: Option<i32>, hand_over: &str) -> std::io::Result<Self> {
        let mut child = Command::new(std::env::current_exe()?)
            .args(args(host, app, point, join))
            // A start the test harness asked for is the first's; the second has its own command line.
            .env_remove("NIGHTJAR_AUTOSTREAM")
            .stdin(Stdio::piped())
            // Not our stderr, which is the log pipe: the second screen keeps a log of its own.
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        let mut input = child.stdin.take();
        // A process that cannot take the line is ended by its input closing, as `self` drops.
        if let Some(pipe) = input.as_mut() {
            pipe.write_all(format!("{hand_over}\n").as_bytes()).and_then(|()| pipe.flush())?;
        }
        Ok(Self { child, input })
    }

    /// How the second screen's process ended, once it has.
    pub fn exited(&mut self) -> Option<std::io::Result<ExitStatus>> {
        self.child.try_wait().transpose()
    }

    /// Ends the second screen: its input closes, which it takes for the first's end, and a process
    /// still there after `END_WAIT` is killed. Waits on a thread of its own.
    pub fn end(mut self) {
        drop(self.input.take());
        let spawned = std::thread::Builder::new().name("second screen end".into()).spawn(move || {
            let deadline = Instant::now() + END_WAIT;
            loop {
                match self.child.try_wait() {
                    Ok(Some(_)) => return,
                    Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(100)),
                    Ok(None) => {
                        eprintln!("nightjar: second screen: still running {} s after the first screen's end, stopping it", END_WAIT.as_secs());
                        if let Err(e) = self.child.kill() {
                            eprintln!("nightjar: second screen: could not be stopped: {e}");
                        }
                        let _ = self.child.wait();
                        return;
                    }
                    Err(e) => {
                        eprintln!("nightjar: second screen: its end cannot be seen: {e}");
                        return;
                    }
                }
            }
        });
        if let Err(e) = spawned {
            eprintln!("nightjar: second screen: no thread to wait for its end ({e}); its input is closed");
        }
    }
}

/// The first line of `input`, without its line end: the first's hand-over. None at the input's end.
fn first_line(input: &mut impl BufRead) -> std::io::Result<Option<String>> {
    let mut line = String::new();
    if input.by_ref().take(LINE_MAX).read_line(&mut line)? == 0 {
        return Ok(None);
    }
    Ok(Some(line.trim_end_matches(['\r', '\n']).to_owned()))
}

/// In the second screen's process: the returned channel gets the first line of this process's
/// input, the first's hand-over of the PIN lock; `quit` runs when the first closes that input, or
/// it has none (a second screen started by hand has no first to follow). A console's input hands
/// over nothing.
pub fn follow_first(quit: impl FnOnce() + Send + 'static) -> Receiver<String> {
    let (line, hand_over) = channel();
    let spawned = std::thread::Builder::new().name("second screen input".into()).spawn(move || {
        let mut stdin = std::io::stdin().lock();
        if !stdin.is_terminal() {
            match first_line(&mut stdin) {
                Ok(Some(first)) => {
                    let _ = line.send(first);
                }
                Ok(None) => {}
                Err(e) => eprintln!("nightjar: second screen: the first screen's line cannot be read ({e})"),
            }
        }
        drop(line);
        let mut sink = [0u8; 64];
        loop {
            match stdin.read(&mut sink) {
                Ok(0) => break,
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => {
                    eprintln!("nightjar: second screen: the first screen's end cannot be read ({e})");
                    break;
                }
            }
        }
        eprintln!("nightjar: second screen: the first screen's stream is over");
        quit();
    });
    if let Err(e) = spawned {
        eprintln!("nightjar: second screen: cannot follow the first screen ({e}); ending");
        std::process::exit(1);
    }
    hand_over
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    #[test]
    fn the_second_screens_command_line_is_a_frontend_start_with_its_monitor() {
        let args = args("PC-UUID", "Game, the sequel", (2560, -40), None);
        let (rest, join) = split_join(args);
        assert_eq!(join, Ok(None));
        let (rest, point) = split_flag(rest);
        assert_eq!(point, Ok(Some((2560, -40))));
        match crate::cli::parse(rest) {
            Ok(crate::cli::Parsed::Launch(launch)) => {
                assert_eq!(launch.host, "PC-UUID");
                assert_eq!(launch.app.as_deref(), Some("Game, the sequel"));
            }
            other => panic!("{other:?}"),
        }
    }

    /// A first screen that joined another device's game hands the second the game's id: the PC
    /// reports no running game to this device, so neither a Resume tile nor a name finds it.
    #[test]
    fn a_joined_game_goes_to_the_second_screen_by_its_id() {
        let (rest, join) = split_join(args("PC-UUID", "GAME-UUID", (0, 0), Some(77)));
        assert_eq!(join, Ok(Some(77)));
        let (rest, point) = split_flag(rest);
        assert_eq!(point, Ok(Some((0, 0))));
        assert!(matches!(crate::cli::parse(rest), Ok(crate::cli::Parsed::Launch(_))));
        for bad in [&["--second-screen-join=x"][..], &["--second-screen-join="], &["--second-screen-join=1", "--second-screen-join=2"]] {
            let (rest, join) = split_join(os(bad));
            assert!(join.is_err() && rest.is_empty(), "{bad:?}");
        }
        // The first screen's Resume tile with nothing reported running: the joined id names the game.
        let pc = HostRecord { unique_id: "PC-UUID".into(), ..HostRecord::default() };
        let resume = app("Resume", 9, "RESUME-UUID", AppControl::Resume);
        let game = app("Game", 77, "GAME-UUID", AppControl::None);
        assert_eq!(target(&pc, &resume, &[resume.clone(), game], joined_running(None, Some(77))), Ok(("PC-UUID".into(), "GAME-UUID".into())));
        assert_eq!(joined_running(Some((3, Some("X"))), Some(77)), Some((3, Some("X"))), "what the PC reports wins");
    }

    #[test]
    fn a_command_line_without_the_flag_is_left_as_it_is() {
        let args = os(&["--host", "pc", "--quit-on-exit"]);
        assert_eq!(split_flag(args.clone()), (args, Ok(None)));
    }

    #[test]
    fn a_flag_without_a_point_or_twice_is_an_error() {
        for bad in [&["--second-screen=1920"][..], &["--second-screen=x,0"], &["--second-screen="],
            &["--second-screen=0,0", "--second-screen=1,1"]] {
            let (rest, point) = split_flag(os(bad));
            assert!(point.is_err(), "{bad:?}");
            assert!(rest.is_empty(), "{bad:?}");
        }
    }

    fn app(name: &str, id: i32, uuid: &str, control: AppControl) -> App {
        App { name: name.into(), id, hdr_supported: false, uuid: uuid.into(), host_index: None, art_version: String::new(), control, monitor_resume: false }
    }

    #[test]
    fn the_second_screen_asks_for_the_first_screens_pc_and_game() {
        let pc = HostRecord { unique_id: "PC-UUID".into(), name: "Desk".into(), ..HostRecord::default() };
        let game = app("Game", 7, "GAME-UUID", AppControl::None);
        assert_eq!(target(&pc, &game, &[], None), Ok(("PC-UUID".into(), "GAME-UUID".into())));
        let unnamed = HostRecord { name: "Desk".into(), ..HostRecord::default() };
        let old = app("Old game", 3, "", AppControl::None);
        assert_eq!(target(&unnamed, &old, &[], None), Ok(("Desk".into(), "Old game".into())));
        // Resume streams the game the PC runs: the second screen asks for that game.
        let resume = app("Resume", 9, "RESUME-UUID", AppControl::Resume);
        let list = [resume.clone(), game.clone(), old.clone()];
        assert_eq!(target(&pc, &resume, &list, Some((7, Some("GAME-UUID")))), Ok(("PC-UUID".into(), "GAME-UUID".into())));
        assert_eq!(target(&pc, &resume, &list, Some((3, None))), Ok(("PC-UUID".into(), "Old game".into())));
        assert!(target(&pc, &resume, &list, None).is_err(), "nothing running");
        assert!(target(&HostRecord::default(), &game, &[], None).is_err());
    }

    /// The first line on the second screen's input is the PIN lock's hand-over; what follows only
    /// keeps the input open.
    #[test]
    fn the_first_line_on_the_input_is_the_hand_over() {
        let token = "ab".repeat(32);
        let mut input = std::io::Cursor::new(format!("{token}\r\nmore"));
        assert_eq!(first_line(&mut input).unwrap(), Some(token));
        let mut rest = String::new();
        input.read_to_string(&mut rest).unwrap();
        assert_eq!(rest, "more", "the rest stays for the end's watch");
        assert_eq!(first_line(&mut std::io::Cursor::new("\n")).unwrap(), Some(String::new()), "no lock: an empty line");
        assert_eq!(first_line(&mut std::io::Cursor::new("")).unwrap(), None, "closed at once");
        let long = "1".repeat(1000);
        assert_eq!(first_line(&mut std::io::Cursor::new(long)).unwrap().map(|l| l.len()), Some(LINE_MAX as usize), "a line is bounded");
    }

    #[test]
    fn the_second_screen_is_another_monitor_than_the_first_streams_on() {
        let left = Rect { x: -1920, y: 0, w: 1920, h: 1080 };
        let main = Rect { x: 0, y: 0, w: 2560, h: 1440 };
        assert_eq!(pick_display(&[(1, main), (2, left)], 1), Some((-960, 540)));
        assert_eq!(pick_display(&[(1, main), (2, left)], 2), Some((1280, 720)));
        assert_eq!(pick_display(&[(1, main)], 1), None);
        assert_eq!(pick_display(&[(1, main), (2, Rect::default())], 1), None);
        assert_eq!(pick_display(&[], 1), None);
    }

    #[test]
    fn the_second_screen_gives_sound_input_and_devices_to_the_first() {
        let mut settings = Settings {
            width: 3840,
            height: 2160,
            second_screen_width: 1280,
            second_screen_height: 720,
            dual_screen: true,
            mute_audio: false,
            microphone: true,
            controllers: true,
            usb_forwarding: true,
            clipboard_sync: true,
            window_mode: WindowMode::Windowed,
            ..Settings::default()
        };
        stream_settings(&mut settings);
        assert_eq!((settings.width, settings.height), (1280, 720));
        assert!(!settings.dual_screen, "a second screen starts no third");
        assert!(settings.mute_audio && !settings.microphone && !settings.controllers);
        assert!(!settings.usb_forwarding && !settings.clipboard_sync);
        assert_eq!(settings.window_mode, WindowMode::Borderless);
        let mut native = Settings { second_screen_width: 0, second_screen_height: 0, ..Settings::default() };
        stream_settings(&mut native);
        assert_eq!((native.width, native.height), (0, 0), "its monitor's own size");
    }
}
