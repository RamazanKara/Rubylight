//! Start a stream from the command line, as game frontends (ES-DE, Pegasus, Playnite) do:
//!
//! `nightjar --host <name|uuid> [--app <name|uuid>] [--quit-on-exit]`
//!
//! The PC must be saved and paired. Without `--app` its app list opens; with it the app starts.
//! `--quit-on-exit` closes Nightjar when that stream ends, so the frontend gets its window back.
//! A PC or app that cannot be found, or a stream that cannot start, ends the process with an
//! error (exit code 1) instead of leaving the launcher open; a wrong command line exits with 64.
//! Cancelling the start closes Nightjar with exit code 0.
//!
//! moonlight-qt's form works too, for tools written for it (MoonDeck):
//! `nightjar [options] stream <host> <app>`, which closes Nightjar when the stream ends, with
//! moonlight-qt's stream options (`--resolution 1920x1080`, `--fps`, `--bitrate` in Kbps, `--hdr`,
//! `--no-vsync` and the others in [`StreamOptions`]). They change the settings of this run's
//! streams only and are never saved.
//!
//! `nightjar list <host> [--verbose]` prints the PC's apps, one name a line, as moonlight-qt's
//! `list` does (MoonDeck reads it), with its exit codes: 0, -1 for a PC that is not saved, not
//! paired or cannot be reached, and 1 when the app list cannot be read ([`run_list`]).
//!
//! `nightjar <pairing link>` is how the system opens a `rubylight:` link (see `link_channel`).

use std::ffi::OsString;
use std::time::{Duration, Instant};

use nightjar_core::http::{App, AppControl};

use crate::config::{AudioChannels, CodecPreference, DecoderPreference, FramePacing, MouseMode, Settings, WindowMode};

/// How long the PC gets to answer and list its apps.
const START_LIMIT: Duration = Duration::from_secs(60);
/// How long a launched stream gets to start: a backstop, the connection's own steps time out
/// sooner.
const LAUNCH_LIMIT: Duration = Duration::from_secs(180);

pub const USAGE: &str = "usage: nightjar [--kms | --no-kms] [--version | --host <name|uuid> [--app <name|uuid>] [--quit-on-exit] [stream options] | [stream options] stream <host> <app> | list <host> [--verbose] | <pairing link>]
stream options, as moonlight-qt's: --resolution <W>x<H>, --720, --1080, --1440, --4K, --fps <n>, --bitrate <Kbps>,
  --video-codec auto|H.264|HEVC|AV1, --video-decoder auto|software|hardware, --audio-config stereo|5.1-surround|7.1-surround,
  --display-mode fullscreen|windowed|borderless, --capture-system-keys never|fullscreen|always, and these with their --no- forms:
  --hdr, --vsync, --frame-pacing, --performance-overlay, --quit-after, --yuv444, --audio-on-host, --absolute-mouse,
  --game-optimization, --multi-controller, --swap-gamepad-buttons, --background-gamepad
  --kms, --no-kms: on Linux, draw straight to the display (DRM/KMS) or through the desktop, whatever Display output says";

/// Not the link itself: it holds a PIN and passphrase, and this goes to the log.
const LINK_NOT_ALONE: &str = "a pairing link is the only argument";

/// moonlight-qt's form needs both.
const STREAM_NEEDS: &str = "stream needs a host and an app: stream <host> <app>";

/// moonlight-qt's exit code for a PC it cannot find, reach or use (`QCoreApplication::exit(-1)`):
/// 255 on Linux.
pub const LIST_NO_PC: i32 = -1;
/// moonlight-qt's exit code for an app list the PC did not give.
pub const LIST_FAILED: i32 = 1;

/// moonlight-qt's `list <host>`: the PC's apps, one name a line.
#[derive(Debug, PartialEq, Eq)]
pub struct List {
    pub host: String,
    /// `--verbose`: moonlight-qt's progress lines before the list, on stdout.
    pub verbose: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Launch {
    pub host: String,
    pub app: Option<String>,
    pub quit_on_exit: bool,
    /// The game the PC runs for another device, joined by its id: the second screen of a first
    /// screen that joined it (`--second-screen-join`). The PC reports no running game to this
    /// device then, so `app` cannot find it.
    pub join: Option<i32>,
    /// moonlight-qt's stream options, for this run's streams from this PC.
    pub options: StreamOptions,
}

/// moonlight-qt's stream options (its `app/cli/commandlineparser.cpp`), as given: None leaves the
/// saved setting. They change the settings of one run's streams and are never saved. Options
/// Nightjar has no setting for (`--packet-size`, `--mouse-buttons-swap`, `--touchscreen-trackpad`,
/// `--mute-on-focus-loss`, `--reverse-scroll-direction`, `--keep-awake`) are taken and change
/// nothing, so a command line written for moonlight-qt still starts.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct StreamOptions {
    pub resolution: Option<(u32, u32)>,
    pub fps: Option<u32>,
    pub bitrate_kbps: Option<u32>,
    pub codec: Option<CodecPreference>,
    pub decoder: Option<DecoderPreference>,
    pub audio: Option<AudioChannels>,
    pub window_mode: Option<WindowMode>,
    /// `--capture-system-keys`: whether they are captured, and whether only in full screen.
    pub system_keys: Option<(bool, bool)>,
    pub hdr: Option<bool>,
    pub vsync: Option<bool>,
    pub frame_pacing: Option<bool>,
    pub overlay: Option<bool>,
    pub yuv444: Option<bool>,
    pub audio_on_host: Option<bool>,
    pub absolute_mouse: Option<bool>,
    pub game_optimization: Option<bool>,
    pub multi_controller: Option<bool>,
    pub swap_face_buttons: Option<bool>,
    pub background_gamepad: Option<bool>,
    /// `--quit-after`: leaving the stream quits the game on the PC; `--no-quit-after`: it does
    /// not, whatever the game's own choice says.
    pub quit_after: Option<bool>,
}

/// moonlight-qt's on/off options, each with its `--no-` form; the last one given wins.
const TOGGLES: [&str; 17] = [
    "hdr",
    "vsync",
    "frame-pacing",
    "performance-overlay",
    "yuv444",
    "audio-on-host",
    "absolute-mouse",
    "game-optimization",
    "multi-controller",
    "swap-gamepad-buttons",
    "background-gamepad",
    "quit-after",
    // Taken, and nothing changes: Nightjar has no such setting.
    "mouse-buttons-swap",
    "touchscreen-trackpad",
    "mute-on-focus-loss",
    "reverse-scroll-direction",
    "keep-awake",
];

/// moonlight-qt's options with a value; the last one given wins.
const VALUES: [&str; 9] = [
    "resolution",
    "fps",
    "bitrate",
    "packet-size",
    "display-mode",
    "audio-config",
    "capture-system-keys",
    "video-codec",
    "video-decoder",
];

impl StreamOptions {
    fn any(&self) -> bool {
        *self != Self::default()
    }

    /// Changes `settings` for one stream as moonlight-qt does: a resolution or frame rate given
    /// without a bitrate picks the bitrate for them.
    pub fn apply(&self, settings: &mut Settings) {
        fn set(value: Option<bool>, field: &mut bool) {
            if let Some(value) = value {
                *field = value;
            }
        }
        if let Some((width, height)) = self.resolution {
            (settings.width, settings.height) = (width, height);
        }
        if let Some(fps) = self.fps {
            settings.fps = fps;
        }
        match self.bitrate_kbps {
            Some(kbps) => settings.bitrate_kbps = kbps,
            None if self.resolution.is_some() || self.fps.is_some() => settings.bitrate_kbps = 0,
            None => {}
        }
        if let Some(codec) = self.codec {
            settings.codec = codec;
        }
        if let Some(decoder) = self.decoder {
            settings.decoder = decoder;
        }
        if let Some(audio) = self.audio {
            settings.audio = audio;
        }
        if let Some(mode) = self.window_mode {
            settings.window_mode = mode;
        }
        if let Some((capture, fullscreen_only)) = self.system_keys {
            settings.capture_system_keys = capture;
            settings.system_keys_fullscreen_only = fullscreen_only;
        }
        set(self.hdr, &mut settings.hdr);
        set(self.vsync, &mut settings.vsync);
        // moonlight-qt's frame pacing shows every frame in turn, one a refresh: Smoothest video.
        // A pacing already chosen that waits for the refresh is kept.
        match self.frame_pacing {
            Some(true) if settings.frame_pacing == FramePacing::Latency => settings.frame_pacing = FramePacing::Smoothness,
            Some(false) => settings.frame_pacing = FramePacing::Latency,
            _ => {}
        }
        set(self.overlay, &mut settings.overlay);
        if let Some(on) = self.yuv444 {
            // "Sharper colored text" is kept in both keys (`Settings::experimental_packed_444`).
            settings.experimental_packed_444 = on;
            settings.prefer_yuv444 = on;
        }
        set(self.audio_on_host, &mut settings.play_audio_on_host);
        if let Some(absolute) = self.absolute_mouse {
            settings.mouse_mode = if absolute { MouseMode::Absolute } else { MouseMode::Capture };
        }
        set(self.game_optimization, &mut settings.sops);
        set(self.multi_controller, &mut settings.multi_controller);
        set(self.swap_face_buttons, &mut settings.swap_face_buttons);
        set(self.background_gamepad, &mut settings.background_controllers);
    }

    fn toggle(&mut self, name: &str, on: bool) {
        let slot = match name {
            "hdr" => &mut self.hdr,
            "vsync" => &mut self.vsync,
            "frame-pacing" => &mut self.frame_pacing,
            "performance-overlay" => &mut self.overlay,
            "yuv444" => &mut self.yuv444,
            "audio-on-host" => &mut self.audio_on_host,
            "absolute-mouse" => &mut self.absolute_mouse,
            "game-optimization" => &mut self.game_optimization,
            "multi-controller" => &mut self.multi_controller,
            "swap-gamepad-buttons" => &mut self.swap_face_buttons,
            "background-gamepad" => &mut self.background_gamepad,
            "quit-after" => &mut self.quit_after,
            _ => return,
        };
        *slot = Some(on);
    }

    fn value(&mut self, name: &str, value: &str) -> Result<(), String> {
        let wrong = || format!("--{name} cannot be {value}");
        let number = |n: &str| n.parse::<u32>().ok().filter(|n| *n > 0).ok_or_else(wrong);
        // moonlight-qt compares its choices ignoring case.
        let choice = value.to_ascii_lowercase();
        match name {
            "resolution" => {
                let (w, h) = choice.split_once('x').ok_or_else(wrong)?;
                self.resolution = Some((number(w)?, number(h)?));
            }
            "fps" => self.fps = Some(number(value)?),
            "bitrate" => self.bitrate_kbps = Some(number(value)?),
            "packet-size" => {
                number(value)?;
            }
            "display-mode" => {
                self.window_mode = Some(match choice.as_str() {
                    "fullscreen" => WindowMode::Fullscreen,
                    "windowed" => WindowMode::Windowed,
                    "borderless" => WindowMode::Borderless,
                    _ => return Err(wrong()),
                })
            }
            "audio-config" => {
                self.audio = Some(match choice.as_str() {
                    "stereo" => AudioChannels::Stereo,
                    "5.1-surround" => AudioChannels::Surround51,
                    "7.1-surround" => AudioChannels::Surround71,
                    _ => return Err(wrong()),
                })
            }
            "capture-system-keys" => {
                self.system_keys = Some(match choice.as_str() {
                    "never" => (false, false),
                    "fullscreen" => (true, true),
                    "always" => (true, false),
                    _ => return Err(wrong()),
                })
            }
            "video-codec" => {
                self.codec = Some(match choice.as_str() {
                    "auto" => CodecPreference::Auto,
                    "h.264" => CodecPreference::H264,
                    "hevc" => CodecPreference::Hevc,
                    "av1" => CodecPreference::Av1,
                    _ => return Err(wrong()),
                })
            }
            "video-decoder" => {
                // Automatic decodes in hardware where this device can.
                self.decoder = Some(match choice.as_str() {
                    "auto" | "hardware" => DecoderPreference::Auto,
                    "software" => DecoderPreference::Software,
                    _ => return Err(wrong()),
                })
            }
            _ => return Err(format!("unknown argument --{name}")),
        }
        Ok(())
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Parsed {
    /// No arguments: the launcher as usual.
    Gui,
    Help,
    /// `--version`: print the version and exit, without a window (the packages' install checks).
    Version,
    Launch(Launch),
    /// moonlight-qt's `list <host>`: print the PC's apps and exit, without a window.
    List(List),
    /// A link the system opened with Nightjar, as it came: the window asks before it pairs, or
    /// refuses it. Text that is not valid is empty here, and refused the same way.
    Link(String),
}

/// Whether an argument is meant as a link. A browser or a QR reader may write the scheme in any
/// case, and any part after it may be wrong: that is for the window to refuse, with its own words.
pub(crate) fn is_link(arg: &str) -> bool {
    arg.get(.."rubylight:".len()).is_some_and(|scheme| scheme.eq_ignore_ascii_case("rubylight:"))
}

/// Reads the arguments after the program name. `--name value` and `--name=value` both work.
pub fn parse<I: IntoIterator<Item = OsString>>(args: I) -> Result<Parsed, String> {
    let mut host = None;
    let mut app = None;
    let mut quit_on_exit = false;
    let mut options = StreamOptions::default();
    // moonlight-qt's `stream <host> <app>` or `list <host>`, as its words come.
    let mut words: Vec<String> = Vec::new();
    // `list`'s own options.
    let mut list_flags: Vec<String> = Vec::new();
    let mut first = true;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        // Alone, whatever else the command line holds: a link with a quote in it must not be able
        // to break out of the system's quoting and add arguments of its own.
        if is_link(&arg.to_string_lossy()) {
            if !first || args.next().is_some() {
                return Err(LINK_NOT_ALONE.into());
            }
            return Ok(Parsed::Link(arg.into_string().unwrap_or_default()));
        }
        first = false;
        let arg = arg
            .into_string()
            .map_err(|a| format!("{} is not valid text", a.to_string_lossy()))?;
        let (flag, inline) = match arg.split_once('=') {
            Some((flag, value)) if flag.starts_with("--") => {
                (flag.to_owned(), Some(value.to_owned()))
            }
            _ => (arg, None),
        };
        let mut value_of = |inline: Option<String>| -> Result<String, String> {
            let value = match inline {
                Some(value) => value,
                None => args
                    .next()
                    .and_then(|v| v.into_string().ok())
                    .filter(|v| !v.starts_with("--"))
                    .ok_or_else(|| format!("{flag} needs a value"))?,
            };
            let value = value.trim().to_owned();
            if value.is_empty() {
                return Err(format!("{flag} needs a value"));
            }
            Ok(value)
        };
        let name = flag.strip_prefix("--").unwrap_or_default();
        let toggle = |n: &str| TOGGLES.contains(&n);
        match flag.as_str() {
            "--help" | "-h" if inline.is_none() => return Ok(Parsed::Help),
            "--version" if inline.is_none() => return Ok(Parsed::Version),
            "--quit-on-exit" if inline.is_none() => quit_on_exit = true,
            "--verbose" | "--csv" if inline.is_none() => list_flags.push(flag.clone()),
            "--host" | "--app" => {
                let value = value_of(inline)?;
                let slot = if flag == "--host" {
                    &mut host
                } else {
                    &mut app
                };
                if slot.replace(value).is_some() {
                    return Err(format!("{flag} is given twice"));
                }
            }
            "--720" | "--1080" | "--1440" | "--4K" | "--4k" if inline.is_none() => {
                options.resolution = Some(match name {
                    "720" => (1280, 720),
                    "1080" => (1920, 1080),
                    "1440" => (2560, 1440),
                    _ => (3840, 2160),
                });
            }
            _ if inline.is_none() && toggle(name) => options.toggle(name, true),
            _ if inline.is_none() && name.strip_prefix("no-").is_some_and(toggle) => {
                options.toggle(&name[3..], false)
            }
            _ if VALUES.contains(&name) => {
                let value = value_of(inline)?;
                options.value(name, &value)?;
            }
            // moonlight-qt finds its action anywhere among the words.
            _ if !flag.starts_with('-')
                && (!words.is_empty() || flag.eq_ignore_ascii_case("stream") || flag.eq_ignore_ascii_case("list")) =>
            {
                words.push(flag.clone());
            }
            _ => return Err(format!("unknown argument {flag}")),
        }
    }
    if words.first().is_some_and(|w| w.eq_ignore_ascii_case("list")) {
        if host.is_some() || app.is_some() || quit_on_exit || options.any() {
            return Err("list takes only a host and --verbose: list <host> [--verbose]".into());
        }
        if list_flags.iter().any(|f| f == "--csv") {
            return Err("list --csv is not available; list prints one app name a line".into());
        }
        // moonlight-qt reads the host and leaves any word after it.
        let Some(host) = words.get(1).map(|h| h.trim().to_owned()).filter(|h| !h.is_empty()) else {
            return Err("list needs a host: list <host>".into());
        };
        return Ok(Parsed::List(List { host, verbose: !list_flags.is_empty() }));
    }
    if let Some(flag) = list_flags.first() {
        return Err(format!("unknown argument {flag}"));
    }
    if !words.is_empty() {
        let [_, stream_host, stream_app] = <[String; 3]>::try_from(words).map_err(|_| STREAM_NEEDS.to_owned())?;
        let (stream_host, stream_app) = (stream_host.trim().to_owned(), stream_app.trim().to_owned());
        if stream_host.is_empty() || stream_app.is_empty() {
            return Err(STREAM_NEEDS.into());
        }
        if host.is_some() || app.is_some() {
            return Err("give the PC and app either with stream or with --host and --app".into());
        }
        // moonlight-qt closes when the stream it started from the command line ends.
        (host, app, quit_on_exit) = (Some(stream_host), Some(stream_app), true);
    }
    match host {
        Some(host) => Ok(Parsed::Launch(Launch {
            host,
            app,
            quit_on_exit,
            join: None,
            options,
        })),
        None if app.is_some() => Err("--app needs --host".into()),
        None if quit_on_exit => Err("--quit-on-exit needs --host".into()),
        None if options.any() => Err("stream options need --host or stream <host> <app>".into()),
        None => Ok(Parsed::Gui),
    }
}

/// Whether an error that ends Nightjar is shown in a box too. A frontend start only writes it to
/// stderr and the log: a box could open behind the fullscreen frontend and keep Nightjar open.
pub fn shows_error_box(launch: Option<&Launch>) -> bool {
    launch.is_none()
}

/// Whether this build has a console for the usage text: a Windows release build has none.
pub const HAS_CONSOLE: bool = !cfg!(all(windows, not(debug_assertions)));

/// Lets a build without a console print into the console it was started from (cmd, PowerShell),
/// for `--version`. Output redirected into a file or a pipe keeps going there.
pub fn use_parent_console() {
    #[cfg(windows)]
    if !HAS_CONSOLE {
        use windows::core::w;
        use windows::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE};
        use windows::Win32::Storage::FileSystem::{
            CreateFileW, GetFileType, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_TYPE_DISK, FILE_TYPE_PIPE, OPEN_EXISTING,
        };
        use windows::Win32::System::Console::{AttachConsole, GetStdHandle, SetStdHandle, ATTACH_PARENT_PROCESS, STD_OUTPUT_HANDLE};
        // SAFETY: plain console and file calls on handles of this process; a failure leaves stdout
        // as it was, and printing then shows nothing, as before.
        unsafe {
            if let Ok(handle) = GetStdHandle(STD_OUTPUT_HANDLE) {
                if !handle.is_invalid() && !handle.0.is_null() {
                    let kind = GetFileType(handle);
                    if kind == FILE_TYPE_DISK || kind == FILE_TYPE_PIPE {
                        return;
                    }
                }
            }
            // A console parent hands a window program its own console's handles, which are no use
            // to it until it attaches; then CONOUT$ is that console's screen.
            if AttachConsole(ATTACH_PARENT_PROCESS).is_ok() {
                if let Ok(out) = CreateFileW(w!("CONOUT$"), (GENERIC_READ | GENERIC_WRITE).0, FILE_SHARE_READ | FILE_SHARE_WRITE, None, OPEN_EXISTING, FILE_FLAGS_AND_ATTRIBUTES(0), None) {
                    let _ = SetStdHandle(STD_OUTPUT_HANDLE, out);
                }
            }
        }
    }
}

/// Whether the usage text (`--help`, or a wrong command line) is shown in a box too: only by a
/// build without a console, where printing it shows nothing, and never for a command line that
/// names a PC (`--host`, or moonlight-qt's `stream` and `list`): that is a frontend's start, where a box
/// could open behind it and keep Nightjar open (see [`shows_error_box`]).
pub fn shows_usage_box<S: AsRef<std::ffi::OsStr>>(has_console: bool, args: &[S]) -> bool {
    let names_a_host = args.iter().any(|arg| {
        let arg = arg.as_ref().to_string_lossy();
        arg == "--host" || arg.starts_with("--host=") || arg.eq_ignore_ascii_case("stream") || arg.eq_ignore_ascii_case("list")
    });
    !has_console && !names_a_host
}

/// `list`'s output: the name of each app to stream, one a line, as moonlight-qt prints it. Host
/// actions (disconnect, terminate) are no apps to stream and are left out.
pub fn app_names(apps: &[App]) -> String {
    apps.iter().filter(|app| !app.is_control_action()).map(|app| format!("{}\n", app.name)).collect()
}

/// What `list` says and exits with when the app list could not be read.
fn list_error(error: &nightjar_core::http::Error, host: &str) -> (String, i32) {
    use nightjar_core::http::Error;
    match error {
        Error::Io(_) => (format!("Failed to connect to {host}"), LIST_NO_PC),
        Error::NotPaired | Error::Status { code: 401, .. } => (
            format!("Computer {host} has not been paired. Please open Nightjar to pair before retrieving games list."),
            LIST_NO_PC,
        ),
        other => (crate::hosts::describe(other), LIST_FAILED),
    }
}

/// Runs moonlight-qt's `list` against the saved PCs: the app names go to stdout, an error to
/// stderr, and the exit code is moonlight-qt's.
pub fn run_list(list: &List, store: &crate::config::Store) -> i32 {
    use std::io::Write;
    let records = store.hosts();
    let entries = records.iter().enumerate().map(|(index, record)| HostEntry {
        id: index as u64,
        unique_id: &record.unique_id,
        name: &record.name,
        alias: record.alias.as_deref().filter(|a| !a.is_empty()),
        paired: record.server_certificate.is_some(),
    });
    let record = match find_host(entries, &list.host) {
        Ok(index) => &records[index as usize],
        Err(why) => {
            eprintln!("{why}");
            return LIST_NO_PC;
        }
    };
    let identity = match store.identity() {
        Some(saved) => match nightjar_core::http::ClientIdentity::from_pem(saved.unique_id, &saved.certificate_pem, &saved.private_key_pem) {
            Ok(identity) => std::sync::Arc::new(identity),
            Err(e) => {
                eprintln!("this device's pairing key cannot be read ({e}); open Nightjar once");
                return LIST_NO_PC;
            }
        },
        None => {
            eprintln!("this device has no pairing key yet; pair the PC in Nightjar first");
            return LIST_NO_PC;
        }
    };
    let mut out = std::io::stdout();
    if list.verbose {
        let _ = out.write_all(b"Establishing connection to PC...\n");
    }
    let client = match crate::hosts::client_for(&identity, record, &record.address) {
        Ok(client) => client,
        Err(why) => {
            eprintln!("{why}");
            return LIST_NO_PC;
        }
    };
    if list.verbose {
        let _ = out.write_all(b"Loading app list...\n");
    }
    match client.app_list(&nightjar_core::http::HostCapabilities::UNKNOWN) {
        Ok(apps) => {
            let _ = out.write_all(app_names(&apps).as_bytes());
            let _ = out.flush();
            0
        }
        Err(e) => {
            let (why, code) = list_error(&e, &list.host);
            eprintln!("{why}");
            code
        }
    }
}

/// A saved PC as `find_host` sees it.
pub struct HostEntry<'a> {
    pub id: u64,
    pub unique_id: &'a str,
    pub name: &'a str,
    pub alias: Option<&'a str>,
    pub paired: bool,
}

/// The saved PC with this unique id, or else the one with this name (its own or the one given
/// on this device), ignoring case. Unknown, ambiguous and unpaired PCs are errors.
pub fn find_host<'a>(
    hosts: impl IntoIterator<Item = HostEntry<'a>>,
    wanted: &str,
) -> Result<u64, String> {
    let hosts: Vec<HostEntry> = hosts.into_iter().collect();
    let found = match hosts
        .iter()
        .find(|h| !h.unique_id.is_empty() && h.unique_id.eq_ignore_ascii_case(wanted))
    {
        Some(host) => host,
        None => {
            let named: Vec<&HostEntry> = hosts
                .iter()
                .filter(|h| {
                    h.name.eq_ignore_ascii_case(wanted)
                        || h.alias.is_some_and(|a| a.eq_ignore_ascii_case(wanted))
                })
                .collect();
            match named.as_slice() {
                [] => return Err(format!("no saved PC is named {wanted} or has that UUID")),
                [host] => *host,
                _ => {
                    return Err(format!(
                        "several saved PCs are named {wanted}; give its UUID instead"
                    ))
                }
            }
        }
    };
    if !found.paired {
        return Err(format!(
            "the PC {wanted} is not paired; pair it in Nightjar first"
        ));
    }
    Ok(found.id)
}

/// The app with this UUID, or else the one with this name, ignoring case. Host actions
/// (disconnect, terminate) are no apps to stream.
pub fn find_app<'a>(apps: &'a [App], wanted: &str) -> Result<&'a App, String> {
    let found = match apps
        .iter()
        .find(|a| !a.uuid.is_empty() && a.uuid.eq_ignore_ascii_case(wanted))
    {
        Some(app) => app,
        None => {
            let named: Vec<&App> = apps
                .iter()
                .filter(|a| a.name.eq_ignore_ascii_case(wanted))
                .collect();
            match named.as_slice() {
                [] => {
                    return Err(format!(
                        "the PC has no app named {wanted} or with that UUID"
                    ))
                }
                [app] => *app,
                _ => {
                    return Err(format!(
                        "the PC has several apps named {wanted}; give its UUID instead"
                    ))
                }
            }
        }
    };
    if found.is_control_action() {
        return Err(format!(
            "{} is an action on the PC, not an app to stream",
            found.name
        ));
    }
    Ok(found)
}

/// Whether starting `app` resumes it, by the rules of a click on its poster (`ui.rs`). Another
/// app running is an error: the PC refuses a plain launch next to it unless it asks the user
/// itself.
fn resume(host: &HostSeen, apps: &[App], app: &App) -> Result<bool, String> {
    if app.control != AppControl::None {
        return Ok(false);
    }
    let Some((id, uuid)) = &host.running else {
        return Ok(false);
    };
    if app.matches_running_app(*id, uuid.as_deref()) {
        return Ok(true);
    }
    if host.confirms_replacement {
        return Ok(false);
    }
    let running = apps
        .iter()
        .find(|a| a.matches_running_app(*id, uuid.as_deref()))
        .map_or("another app", |a| a.name.as_str());
    Err(format!(
        "the PC is running {running}; quit it first or start that app instead"
    ))
}

pub enum AppsSeen {
    NotLoaded,
    Loading,
    Loaded(Vec<App>),
    Failed(String),
}

pub struct HostSeen {
    pub answered: bool,
    pub paired: bool,
    /// Why the PC could not be reached, when its last check failed.
    pub offline: Option<String>,
    /// The app the PC runs, as it reports it: its id and UUID.
    pub running: Option<(i32, Option<String>)>,
    pub confirms_replacement: bool,
    pub apps: AppsSeen,
}

pub struct Seen {
    pub identity: bool,
    /// None once the PC is no longer saved.
    pub host: Option<HostSeen>,
    /// The launcher's error message, if one is up.
    pub message: Option<String>,
    /// A launch is under way: the network check, the connection or the stream.
    pub busy: bool,
    /// The launcher asks the user to confirm closing the app the PC runs.
    pub confirming: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    Wait,
    OpenApps(u64),
    LoadApps(u64),
    /// Host, app, and whether to resume it.
    Launch(u64, i32, bool),
    Fail(String),
    /// The user cancelled the start: close without an error.
    Close,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Starting,
    /// The launch was asked for; waiting for the stream.
    Launched,
    Streaming,
    /// Nothing more to drive (the app list is open, or the stream ended without quitting).
    Idle,
}

/// Drives one command-line start through the launcher, one step per frame.
pub struct CliLaunch {
    pub launch: Launch,
    pub host: u64,
    phase: Phase,
    /// The app list was asked for and has not started loading since.
    apps_asked: bool,
    /// When the current phase began.
    started: Instant,
    /// The user cancelled the launch.
    cancelled: bool,
    /// Why the start failed; the process then exits with an error.
    pub failed: Option<String>,
}

impl CliLaunch {
    pub fn new(launch: Launch, hosts: &crate::hosts::Hosts) -> Result<Self, String> {
        let entries = hosts.list.iter().map(|h| HostEntry {
            id: h.id,
            unique_id: &h.record.unique_id,
            name: &h.record.name,
            alias: h.record.alias.as_deref().filter(|a| !a.is_empty()),
            paired: h.record.server_certificate.is_some(),
        });
        let host = find_host(entries, &launch.host)?;
        Ok(Self::resolved(launch, host))
    }

    fn resolved(launch: Launch, host: u64) -> Self {
        Self {
            launch,
            host,
            phase: Phase::Starting,
            apps_asked: false,
            started: Instant::now(),
            cancelled: false,
            failed: None,
        }
    }

    pub fn host_seen(&self, hosts: &crate::hosts::Hosts) -> Option<HostSeen> {
        use crate::hosts::{Apps, Reach};
        let host = hosts.get(self.host)?;
        Some(HostSeen {
            answered: host.info.is_some(),
            paired: host.paired(),
            offline: (host.reach == Reach::Offline).then(|| host.error.clone().unwrap_or_default()),
            running: host.running_app().map(|id| {
                let uuid = host.info.as_ref().and_then(|i| i.current_game_uuid.clone());
                (id, uuid)
            }),
            confirms_replacement: host.confirms_replacement(),
            apps: match &host.apps {
                Apps::NotLoaded => AppsSeen::NotLoaded,
                Apps::Loading => AppsSeen::Loading,
                Apps::Loaded(apps) => AppsSeen::Loaded(apps.clone()),
                Apps::Failed(e) => AppsSeen::Failed(e.clone()),
            },
        })
    }

    /// What to do next, given what the launcher can see now. Each action is asked for once.
    pub fn next(&mut self, seen: &Seen) -> Step {
        match self.phase {
            Phase::Streaming | Phase::Idle => return Step::Wait,
            Phase::Launched => return self.launched(seen),
            Phase::Starting => {}
        }
        // An error up now stops the start. None is up when the launch goes out, so every
        // message in `Launched` is about the launch.
        if let Some(message) = &seen.message {
            return Step::Fail(message.clone());
        }
        let Some(host) = &seen.host else {
            return Step::Fail(format!("the PC {} is no longer saved", self.launch.host));
        };
        if !seen.identity {
            return self.in_time(Step::Wait);
        }
        let Some(wanted) = self.launch.app.clone() else {
            self.phase = Phase::Idle;
            return Step::OpenApps(self.host);
        };
        if let Some(why) = &host.offline {
            return Step::Fail(format!(
                "the PC {} cannot be reached: {why}",
                self.launch.host
            ));
        }
        if !host.answered {
            return self.in_time(Step::Wait);
        }
        if !host.paired {
            return Step::Fail(format!(
                "the PC {} is not paired; pair it in Nightjar first",
                self.launch.host
            ));
        }
        match &host.apps {
            AppsSeen::NotLoaded if !self.apps_asked => {
                self.apps_asked = true;
                Step::LoadApps(self.host)
            }
            AppsSeen::NotLoaded => self.in_time(Step::Wait),
            AppsSeen::Loading => {
                // A list that drops back to unloaded later is asked for again.
                self.apps_asked = false;
                self.in_time(Step::Wait)
            }
            AppsSeen::Failed(e) => Step::Fail(format!("could not load the apps: {e}")),
            AppsSeen::Loaded(apps) => {
                let found = match self.launch.join {
                    Some(id) => Ok((id, true)),
                    None => find_app(apps, &wanted).and_then(|app| Ok((app.id, resume(host, apps, app)?))),
                };
                match found {
                    Ok((id, resume)) => {
                        self.phase = Phase::Launched;
                        self.started = Instant::now();
                        Step::Launch(self.host, id, resume)
                    }
                    Err(why) => Step::Fail(why),
                }
            }
        }
    }

    /// After the launch was asked for: the stream starts, or the launch ends without one.
    fn launched(&mut self, seen: &Seen) -> Step {
        if let Some(message) = &seen.message {
            return Step::Fail(message.clone());
        }
        if seen.confirming {
            // The user takes their time; the launch that follows the answer gets the full limit.
            self.started = Instant::now();
            return Step::Wait;
        }
        if !seen.busy {
            // The launcher dropped the launch without saying why, or the user cancelled it.
            return if self.cancelled {
                Step::Close
            } else {
                Step::Fail("the stream did not start".into())
            };
        }
        if self.started.elapsed() > LAUNCH_LIMIT {
            return Step::Fail("the stream did not start in time".into());
        }
        Step::Wait
    }

    fn in_time(&self, step: Step) -> Step {
        if self.started.elapsed() > START_LIMIT {
            Step::Fail(format!(
                "the PC {} did not answer in time",
                self.launch.host
            ))
        } else {
            step
        }
    }

    pub fn streaming_began(&mut self) {
        self.phase = Phase::Streaming;
    }

    /// The user cancelled the launch: Nightjar closes once it has stopped.
    pub fn cancel(&mut self) {
        if self.phase == Phase::Launched {
            self.cancelled = true;
        }
    }

    /// The stream is over (no reconnect follows). True when Nightjar should close now;
    /// `message` is the error the launcher shows for it, if any.
    pub fn stream_ended(&mut self, message: Option<String>) -> bool {
        if self.phase != Phase::Streaming {
            return false;
        }
        self.phase = Phase::Idle;
        if !self.launch.quit_on_exit {
            return false;
        }
        if let Some(message) = message {
            self.fail(&message);
        }
        true
    }

    pub fn fail(&mut self, why: &str) {
        eprintln!("nightjar: --host start failed: {why}");
        self.failed = Some(why.to_owned());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Result<Parsed, String> {
        parse(list.iter().map(OsString::from))
    }

    fn launch(host: &str, app: Option<&str>, quit_on_exit: bool) -> Parsed {
        Parsed::Launch(Launch {
            host: host.into(),
            app: app.map(Into::into),
            quit_on_exit,
            join: None,
            options: StreamOptions::default(),
        })
    }

    #[test]
    fn no_arguments_open_the_launcher() {
        assert_eq!(args(&[]), Ok(Parsed::Gui));
    }

    #[test]
    fn a_pairing_link_is_an_argument_of_its_own() {
        let link = "rubylight://pair?v=1&host=pc&port=1&otp=4821&pass=secret&fp=00";
        assert_eq!(args(&[link]), Ok(Parsed::Link(link.into())));
        // Whatever follows the scheme is the window's to refuse, never a usage error.
        for odd in ["RUBYLIGHT://pair?v=9", "rubylight:", "rubylight://other"] {
            assert_eq!(args(&[odd]), Ok(Parsed::Link(odd.into())));
        }
        let huge = format!("rubylight://pair?{}", "a".repeat(1024 * 1024));
        assert_eq!(args(&[huge.as_str()]), Ok(Parsed::Link(huge.clone())));
        // Any other positional argument stays the error it was.
        assert_eq!(args(&["ruby"]), Err("unknown argument ruby".into()));
        assert_eq!(args(&["rubylightning"]), Err("unknown argument rubylightning".into()));
    }

    #[test]
    fn a_link_cannot_add_arguments_and_errors_do_not_repeat_it() {
        let link = "rubylight://pair?otp=4821&pass=secret";
        for line in [
            vec![link, "--host", "evil"],
            vec![link, "--check-pyrowave"],
            vec![link, link],
            vec!["--host", "pc", link],
            vec!["--quit-on-exit", link],
        ] {
            let error = args(&line).unwrap_err();
            assert_eq!(error, LINK_NOT_ALONE, "{line:?}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_link_that_is_not_text_is_refused_without_being_repeated() {
        use std::os::unix::ffi::OsStringExt;
        let parsed = parse([OsString::from_vec(b"rubylight://pair?otp=4821\xff".to_vec())]);
        assert_eq!(parsed, Ok(Parsed::Link(String::new())));
    }

    #[test]
    fn host_app_and_quit_parse_in_any_order_and_both_forms() {
        assert_eq!(
            args(&["--host", "Gaming PC"]),
            Ok(launch("Gaming PC", None, false))
        );
        assert_eq!(
            args(&[
                "--quit-on-exit",
                "--app",
                "Steam Big Picture",
                "--host",
                "pc"
            ]),
            Ok(launch("pc", Some("Steam Big Picture"), true))
        );
        assert_eq!(
            args(&["--host=pc", "--app=Desktop"]),
            Ok(launch("pc", Some("Desktop"), false))
        );
        assert_eq!(args(&["--host", " pc "]), Ok(launch("pc", None, false)));
        assert_eq!(args(&["--help"]), Ok(Parsed::Help));
        assert_eq!(args(&["--version"]), Ok(Parsed::Version));
    }

    #[test]
    fn wrong_command_lines_are_errors() {
        assert!(args(&["--host"]).is_err());
        assert!(args(&["--host", "--app", "x"]).is_err());
        assert!(args(&["--host="]).is_err());
        assert!(args(&["--host", "a", "--host", "b"]).is_err());
        assert!(args(&["--app", "Desktop"]).is_err());
        assert!(args(&["--quit-on-exit"]).is_err());
        assert!(args(&["--host", "pc", "--fullscreen"]).is_err());
        assert!(args(&["--quit-on-exit=yes", "--host", "pc"]).is_err());
        assert!(args(&["pc"]).is_err());
        assert!(args(&["--version=1"]).is_err());
    }

    fn options_of(list: &[&str]) -> StreamOptions {
        match args(list) {
            Ok(Parsed::Launch(launch)) => launch.options,
            other => panic!("{list:?}: {other:?}"),
        }
    }

    /// MoonDeck's command line (its `moonlightproxy.py`): options first, then `stream`.
    #[test]
    fn moondecks_moonlight_qt_command_line_starts_the_stream_with_its_options() {
        let parsed = args(&[
            "--audio-config", "7.1-surround", "--resolution", "2560x1440", "--fps", "120", "--hdr", "--bitrate", "60000",
            "--no-quit-after", "--performance-overlay", "--no-vsync", "--frame-pacing", "--video-codec", "HEVC",
            "stream", "Gaming PC", "Steam Big Picture",
        ]);
        let launch = match parsed {
            Ok(Parsed::Launch(launch)) => launch,
            other => panic!("{other:?}"),
        };
        // moonlight-qt closes once its command-line stream ends.
        assert_eq!((launch.host.as_str(), launch.app.as_deref(), launch.quit_on_exit), ("Gaming PC", Some("Steam Big Picture"), true));
        let wanted = StreamOptions {
            resolution: Some((2560, 1440)),
            fps: Some(120),
            bitrate_kbps: Some(60000),
            codec: Some(CodecPreference::Hevc),
            audio: Some(AudioChannels::Surround71),
            hdr: Some(true),
            vsync: Some(false),
            frame_pacing: Some(true),
            overlay: Some(true),
            quit_after: Some(false),
            ..Default::default()
        };
        assert_eq!(launch.options, wanted);
    }

    #[test]
    fn moonlight_qt_options_follow_its_spelling_and_the_last_one_wins() {
        assert_eq!(options_of(&["stream", "pc", "app", "--hdr", "--no-hdr"]).hdr, Some(false));
        assert_eq!(options_of(&["--no-hdr", "--hdr", "--host", "pc"]).hdr, Some(true));
        assert_eq!(options_of(&["--1080", "stream", "pc", "app"]).resolution, Some((1920, 1080)));
        assert_eq!(options_of(&["--resolution=1280X800", "--4K", "stream", "pc", "app"]).resolution, Some((3840, 2160)));
        assert_eq!(options_of(&["--video-codec", "h.264", "stream", "pc", "app"]).codec, Some(CodecPreference::H264));
        assert_eq!(options_of(&["--video-codec=AV1", "stream", "pc", "app"]).codec, Some(CodecPreference::Av1));
        assert_eq!(options_of(&["--video-codec", "auto", "stream", "pc", "app"]).codec, Some(CodecPreference::Auto));
        assert_eq!(options_of(&["--video-decoder", "hardware", "stream", "pc", "app"]).decoder, Some(DecoderPreference::Auto));
        assert_eq!(options_of(&["--display-mode", "borderless", "stream", "pc", "app"]).window_mode, Some(WindowMode::Borderless));
        assert_eq!(options_of(&["--capture-system-keys", "fullscreen", "stream", "pc", "app"]).system_keys, Some((true, true)));
        // The action is found anywhere among the words, in any case, as moonlight-qt finds it.
        assert_eq!(args(&["STREAM", "pc", "Desktop"]), Ok(Parsed::Launch(launch_of("pc", "Desktop"))));
        // moonlight-qt's options Nightjar has no setting for are taken, so its command lines start.
        for unused in ["--packet-size=1392", "--keep-awake", "--no-mouse-buttons-swap", "--touchscreen-trackpad", "--mute-on-focus-loss", "--reverse-scroll-direction"] {
            assert_eq!(options_of(&[unused, "stream", "pc", "app"]), StreamOptions::default(), "{unused}");
        }
    }

    #[test]
    fn moonlight_qts_list_names_a_host_and_takes_only_verbose() {
        // MoonDeck's call (its moonlightproxy.py `get_apps`).
        assert_eq!(args(&["list", "Gaming PC"]), Ok(Parsed::List(List { host: "Gaming PC".into(), verbose: false })));
        assert_eq!(args(&["--verbose", "LIST", "pc"]), Ok(Parsed::List(List { host: "pc".into(), verbose: true })));
        assert_eq!(args(&["list", "pc", "extra"]), Ok(Parsed::List(List { host: "pc".into(), verbose: false })));
        for wrong in [
            &["list"][..],
            &["list", " "],
            &["list", "pc", "--csv"],
            &["list", "pc", "--hdr"],
            &["--host", "pc", "list", "pc"],
            &["list", "pc", "--quit-on-exit"],
            &["--verbose", "stream", "pc", "app"],
            &["--verbose", "--host", "pc"],
            &["--verbose=1", "list", "pc"],
        ] {
            assert!(args(wrong).is_err(), "{wrong:?}");
        }
        assert!(!shows_usage_box(false, &["list", "pc", "--bogus"]));
    }

    #[test]
    fn list_prints_one_app_name_a_line_without_host_actions() {
        let terminate = App { control: AppControl::Terminate, ..app(3, "E16C", "Terminate") };
        let apps = vec![app(1, "", "Desktop"), app(2, "5E1F-77", "Steam Big Picture"), terminate];
        assert_eq!(app_names(&apps), "Desktop\nSteam Big Picture\n");
        assert_eq!(app_names(&[]), "");
    }

    #[test]
    fn list_errors_carry_moonlight_qts_exit_codes() {
        use nightjar_core::http::Error;
        let refused = Error::Io(std::io::Error::from(std::io::ErrorKind::ConnectionRefused));
        assert_eq!(list_error(&refused, "pc"), ("Failed to connect to pc".to_owned(), LIST_NO_PC));
        assert_eq!(list_error(&Error::NotPaired, "pc").1, LIST_NO_PC);
        assert!(list_error(&Error::NotPaired, "pc").0.starts_with("Computer pc has not been paired."));
        assert_eq!(list_error(&Error::CertificateMismatch, "pc").1, LIST_FAILED);
    }

    fn launch_of(host: &str, app: &str) -> Launch {
        Launch { host: host.into(), app: Some(app.into()), quit_on_exit: true, join: None, options: StreamOptions::default() }
    }

    #[test]
    fn wrong_moonlight_qt_command_lines_are_errors() {
        for wrong in [
            &["stream"][..],
            &["stream", "pc"],
            &["stream", "pc", "app", "more"],
            &["stream", " ", "app"],
            &["--host", "pc", "stream", "pc", "app"],
            &["--fps", "sixty", "stream", "pc", "app"],
            &["--fps", "0", "stream", "pc", "app"],
            &["--bitrate", "stream", "pc", "app"],
            &["--resolution", "1080p", "stream", "pc", "app"],
            &["--resolution", "0x1080", "stream", "pc", "app"],
            &["--video-codec", "VP9", "stream", "pc", "app"],
            &["--audio-config", "quad", "stream", "pc", "app"],
            &["--hdr=yes", "stream", "pc", "app"],
            &["--no-fps", "stream", "pc", "app"],
            &["--fps", "60"],
            &["--hdr"],
        ] {
            assert!(args(wrong).is_err(), "{wrong:?}");
        }
        // A link still stands alone after moonlight-qt's words or options.
        let link = "rubylight://pair?otp=4821&pass=secret";
        assert_eq!(args(&["stream", "pc", link]), Err(LINK_NOT_ALONE.into()));
        assert_eq!(args(&["--hdr", link]), Err(LINK_NOT_ALONE.into()));
    }

    #[test]
    fn moonlight_qt_options_change_one_streams_settings_as_moonlight_qt_does() {
        let saved = Settings { width: 1920, height: 1080, fps: 60, bitrate_kbps: 40000, frame_pacing: FramePacing::Latency, ..Default::default() };
        let mut settings = saved.clone();
        StreamOptions::default().apply(&mut settings);
        assert_eq!(settings, saved);
        // A resolution or rate without a bitrate gets the bitrate for them.
        let mut settings = saved.clone();
        StreamOptions { fps: Some(120), ..Default::default() }.apply(&mut settings);
        assert_eq!((settings.fps, settings.bitrate_kbps), (120, 0));
        let mut settings = saved.clone();
        let options = StreamOptions {
            resolution: Some((2560, 1600)),
            bitrate_kbps: Some(80000),
            hdr: Some(true),
            vsync: Some(false),
            frame_pacing: Some(true),
            overlay: Some(true),
            yuv444: Some(true),
            absolute_mouse: Some(true),
            system_keys: Some((true, false)),
            ..Default::default()
        };
        options.apply(&mut settings);
        assert_eq!((settings.width, settings.height, settings.bitrate_kbps), (2560, 1600, 80000));
        assert!(settings.hdr && !settings.vsync && settings.overlay);
        assert_eq!(settings.frame_pacing, FramePacing::Smoothness);
        assert!(settings.experimental_packed_444 && settings.prefer_yuv444);
        assert_eq!(settings.mouse_mode, MouseMode::Absolute);
        assert!(settings.capture_system_keys && !settings.system_keys_fullscreen_only);
        // A pacing already chosen that waits for the refresh stays; off is the lowest latency.
        let mut settings = Settings { frame_pacing: FramePacing::CapFps, ..saved.clone() };
        StreamOptions { frame_pacing: Some(true), ..Default::default() }.apply(&mut settings);
        assert_eq!(settings.frame_pacing, FramePacing::CapFps);
        StreamOptions { frame_pacing: Some(false), ..Default::default() }.apply(&mut settings);
        assert_eq!(settings.frame_pacing, FramePacing::Latency);
    }

    fn entry<'a>(
        id: u64,
        unique_id: &'a str,
        name: &'a str,
        alias: Option<&'a str>,
        paired: bool,
    ) -> HostEntry<'a> {
        HostEntry {
            id,
            unique_id,
            name,
            alias,
            paired,
        }
    }

    #[test]
    fn hosts_match_by_uuid_then_name_or_alias() {
        let hosts = || {
            vec![
                entry(1, "AAAA-1111", "DESKTOP-1", Some("Living room"), true),
                entry(2, "BBBB-2222", "aaaa-1111", None, true),
                entry(3, "", "Office", None, true),
            ]
        };
        assert_eq!(
            find_host(hosts(), "aaaa-1111"),
            Ok(1),
            "a UUID wins over a name"
        );
        assert_eq!(find_host(hosts(), "desktop-1"), Ok(1));
        assert_eq!(find_host(hosts(), "LIVING ROOM"), Ok(1));
        assert_eq!(find_host(hosts(), "office"), Ok(3));
        assert!(
            find_host(hosts(), "").is_err(),
            "an empty UUID matches nothing"
        );
        assert!(find_host(hosts(), "Garage").unwrap_err().contains("Garage"));
    }

    #[test]
    fn ambiguous_and_unpaired_hosts_are_errors() {
        let twins = vec![
            entry(1, "a", "PC", None, true),
            entry(2, "b", "pc", None, true),
        ];
        assert!(find_host(twins, "pc").unwrap_err().contains("UUID"));
        let unpaired = vec![entry(1, "a", "PC", None, false)];
        assert!(find_host(unpaired, "PC")
            .unwrap_err()
            .contains("not paired"));
    }

    fn app(id: i32, uuid: &str, name: &str) -> App {
        App {
            name: name.into(),
            id,
            hdr_supported: false,
            uuid: uuid.into(),
            host_index: None,
            art_version: String::new(),
            control: AppControl::None,
            monitor_resume: false,
        }
    }

    fn found(apps: &[App], wanted: &str) -> Result<i32, String> {
        find_app(apps, wanted).map(|a| a.id)
    }

    #[test]
    fn apps_match_by_uuid_then_name() {
        let apps = vec![
            app(1, "", "Desktop"),
            app(2, "5E1F-77", "Steam Big Picture"),
            app(3, "x", "Steam Big Picture"),
        ];
        assert_eq!(found(&apps, "desktop"), Ok(1));
        assert_eq!(found(&apps, "5e1f-77"), Ok(2));
        assert!(found(&apps, "steam big picture")
            .unwrap_err()
            .contains("UUID"));
        assert!(found(&apps, "Cyberpunk").unwrap_err().contains("Cyberpunk"));
    }

    #[test]
    fn host_actions_are_no_apps_to_stream() {
        let terminate = App {
            control: AppControl::Terminate,
            ..app(9, "E16C", "Terminate")
        };
        let apps = vec![app(1, "", "Desktop"), terminate];
        assert!(found(&apps, "terminate")
            .unwrap_err()
            .contains("not an app to stream"));
        assert!(found(&apps, "e16c").is_err());
    }

    fn cli(app: Option<&str>, quit_on_exit: bool) -> CliLaunch {
        CliLaunch::resolved(
            Launch {
                host: "pc".into(),
                app: app.map(Into::into),
                quit_on_exit,
                join: None,
                options: StreamOptions::default(),
            },
            7,
        )
    }

    fn seen(answered: bool, apps: AppsSeen) -> Seen {
        Seen {
            identity: true,
            host: Some(HostSeen {
                answered,
                paired: true,
                offline: None,
                running: None,
                confirms_replacement: false,
                apps,
            }),
            message: None,
            busy: false,
            confirming: false,
        }
    }

    /// The PC as `seen` with the apps loaded and `running` running.
    fn running(apps: Vec<App>, running: Option<(i32, Option<&str>)>, confirms: bool) -> Seen {
        let mut seen = seen(true, AppsSeen::Loaded(apps));
        let host = seen.host.as_mut().unwrap();
        host.running = running.map(|(id, uuid)| (id, uuid.map(Into::into)));
        host.confirms_replacement = confirms;
        seen
    }

    #[test]
    fn an_app_is_loaded_and_launched_once() {
        let mut c = cli(Some("desktop"), false);
        assert_eq!(
            c.next(&Seen {
                identity: false,
                ..seen(false, AppsSeen::NotLoaded)
            }),
            Step::Wait
        );
        assert_eq!(c.next(&seen(false, AppsSeen::NotLoaded)), Step::Wait);
        assert_eq!(c.next(&seen(true, AppsSeen::NotLoaded)), Step::LoadApps(7));
        assert_eq!(c.next(&seen(true, AppsSeen::NotLoaded)), Step::Wait);
        assert_eq!(c.next(&seen(true, AppsSeen::Loading)), Step::Wait);
        let apps = || AppsSeen::Loaded(vec![app(1, "", "Steam"), app(5, "", "Desktop")]);
        assert_eq!(c.next(&seen(true, apps())), Step::Launch(7, 5, false));
        let busy = |s: Seen| Seen { busy: true, ..s };
        assert_eq!(c.next(&busy(seen(true, apps()))), Step::Wait);
    }

    /// The second screen of a first that joined another device's game joins it by its id: the PC
    /// reports no running game to this device, and the name may name no listed app.
    #[test]
    fn a_join_resumes_the_game_by_its_id() {
        let mut c = cli(Some("Running game"), true);
        c.launch.join = Some(9);
        let apps = || AppsSeen::Loaded(vec![app(1, "", "Steam"), app(5, "", "Desktop")]);
        assert_eq!(c.next(&seen(true, apps())), Step::Launch(7, 9, true));
    }

    #[test]
    fn a_list_that_drops_back_to_unloaded_is_asked_for_again() {
        let mut c = cli(Some("desktop"), false);
        assert_eq!(c.next(&seen(true, AppsSeen::NotLoaded)), Step::LoadApps(7));
        assert_eq!(c.next(&seen(true, AppsSeen::Loading)), Step::Wait);
        assert_eq!(c.next(&seen(true, AppsSeen::NotLoaded)), Step::LoadApps(7));
        assert_eq!(c.next(&seen(true, AppsSeen::NotLoaded)), Step::Wait);
    }

    #[test]
    fn the_running_app_is_resumed_and_another_one_is_an_error() {
        let apps = || vec![app(1, "", "Steam"), app(5, "D-5", "Desktop")];
        let mut c = cli(Some("desktop"), false);
        assert_eq!(
            c.next(&running(apps(), Some((5, None)), false)),
            Step::Launch(7, 5, true)
        );
        // A UUID decides when both sides have one.
        let mut c = cli(Some("desktop"), false);
        assert_eq!(
            c.next(&running(apps(), Some((1, Some("D-5"))), false)),
            Step::Launch(7, 5, true)
        );
        let mut c = cli(Some("desktop"), false);
        assert_eq!(
            c.next(&running(apps(), Some((1, None)), false)),
            Step::Fail("the PC is running Steam; quit it first or start that app instead".into())
        );
        // A PC that asks its user before replacing the running app gets a plain launch.
        let mut c = cli(Some("desktop"), false);
        assert_eq!(
            c.next(&running(apps(), Some((1, None)), true)),
            Step::Launch(7, 5, false)
        );
        // Resume and the other special tiles are never resumed themselves.
        let resume_tile = App {
            control: AppControl::Resume,
            ..app(8, "", "Resume")
        };
        let mut c = cli(Some("resume"), false);
        assert_eq!(
            c.next(&running(vec![resume_tile], Some((1, None)), false)),
            Step::Launch(7, 8, false)
        );
    }

    #[test]
    fn a_launch_the_launcher_drops_ends_the_start() {
        let apps = || AppsSeen::Loaded(vec![app(1, "", "Steam")]);
        let launch =
            |c: &mut CliLaunch| assert_eq!(c.next(&seen(true, apps())), Step::Launch(7, 1, false));
        // Dropped without a message (the app vanished, no identity): an error.
        let mut c = cli(Some("Steam"), false);
        launch(&mut c);
        assert_eq!(
            c.next(&seen(true, apps())),
            Step::Fail("the stream did not start".into())
        );
        // Cancelled by the user: closed without an error.
        let mut c = cli(Some("Steam"), false);
        launch(&mut c);
        c.cancel();
        assert_eq!(c.next(&seen(true, apps())), Step::Close);
        // Still connecting after the backstop.
        let mut c = cli(Some("Steam"), false);
        launch(&mut c);
        c.started = Instant::now() - LAUNCH_LIMIT - Duration::from_secs(1);
        let busy = Seen {
            busy: true,
            ..seen(true, apps())
        };
        assert_eq!(
            c.next(&busy),
            Step::Fail("the stream did not start in time".into())
        );
    }

    #[test]
    fn the_start_waits_for_the_replace_prompt_and_its_cancel_closes() {
        let apps = || vec![app(1, "", "Steam"), app(5, "", "Desktop")];
        let refused = || running(apps(), Some((1, None)), true);
        let confirming = || Seen { confirming: true, ..refused() };
        for confirm in [true, false] {
            let mut c = cli(Some("desktop"), false);
            assert_eq!(c.next(&refused()), Step::Launch(7, 5, false));
            // The PC refused the launch and the launcher asks, however long the user takes.
            c.started = Instant::now() - LAUNCH_LIMIT - Duration::from_secs(1);
            assert_eq!(c.next(&confirming()), Step::Wait);
            if confirm {
                // The launch with the answer gets the full limit.
                assert_eq!(c.next(&Seen { busy: true, ..refused() }), Step::Wait);
                c.streaming_began();
                assert_eq!(c.next(&refused()), Step::Wait);
            } else {
                // Cancelling the prompt cancels the start, without an error.
                c.cancel();
                assert_eq!(c.next(&refused()), Step::Close);
                assert!(c.failed.is_none());
            }
        }
    }

    #[test]
    fn a_message_or_an_unreachable_pc_fails_the_start_at_once() {
        let mut c = cli(Some("Steam"), false);
        let message = Seen {
            message: Some("Nightjar's identity: could not be read".into()),
            ..seen(false, AppsSeen::NotLoaded)
        };
        assert_eq!(
            c.next(&message),
            Step::Fail("Nightjar's identity: could not be read".into())
        );
        let mut c = cli(Some("Steam"), false);
        let mut offline = seen(false, AppsSeen::NotLoaded);
        offline.host.as_mut().unwrap().offline = Some("connection refused".into());
        assert_eq!(
            c.next(&offline),
            Step::Fail("the PC pc cannot be reached: connection refused".into())
        );
    }

    #[test]
    fn only_a_start_without_a_frontend_shows_an_error_box() {
        assert!(shows_error_box(None));
        let launch = Launch {
            host: "pc".into(),
            app: None,
            quit_on_exit: true,
            join: None,
            options: StreamOptions::default(),
        };
        assert!(!shows_error_box(Some(&launch)));
    }

    #[test]
    fn the_usage_is_boxed_only_without_a_console_and_never_for_a_frontend_start() {
        // A console prints it; a box on top would only be in the way.
        assert!(!shows_usage_box(true, &["--help"]));
        assert!(!shows_usage_box(true, &["--bogus"]));
        // Without one the box is the only place it shows.
        assert!(shows_usage_box(false, &["--help"]));
        assert!(shows_usage_box(false, &["--app", "Desktop"]));
        // A frontend's wrong command line ends at once, in stderr.
        assert!(!shows_usage_box(false, &["--host", "pc", "--bogus"]));
        assert!(!shows_usage_box(false, &["--bogus", "--host=pc"]));
        assert!(shows_usage_box(false, &["--hostname"]));
        assert!(!shows_usage_box(false, &["--bogus", "stream", "pc", "app"]));
    }

    #[test]
    fn without_an_app_the_app_list_opens_once() {
        let mut c = cli(None, false);
        assert_eq!(c.next(&seen(false, AppsSeen::NotLoaded)), Step::OpenApps(7));
        assert_eq!(c.next(&seen(true, AppsSeen::NotLoaded)), Step::Wait);
    }

    #[test]
    fn a_missing_app_a_failed_list_a_silent_pc_or_a_failed_start_are_errors() {
        let mut c = cli(Some("Cyberpunk"), false);
        assert!(matches!(
            c.next(&seen(true, AppsSeen::Loaded(vec![app(1, "", "Steam")]))),
            Step::Fail(_)
        ));
        let mut c = cli(Some("Steam"), false);
        assert!(matches!(
            c.next(&seen(true, AppsSeen::Failed("timeout".into()))),
            Step::Fail(_)
        ));
        let mut c = cli(Some("Steam"), false);
        assert!(matches!(
            c.next(&Seen {
                host: None,
                ..seen(true, AppsSeen::NotLoaded)
            }),
            Step::Fail(_)
        ));
        let mut c = cli(Some("Steam"), false);
        c.started = Instant::now() - START_LIMIT - Duration::from_secs(1);
        assert!(matches!(
            c.next(&seen(false, AppsSeen::NotLoaded)),
            Step::Fail(_)
        ));
        let mut c = cli(Some("Steam"), false);
        assert_eq!(
            c.next(&seen(true, AppsSeen::Loaded(vec![app(1, "", "Steam")]))),
            Step::Launch(7, 1, false)
        );
        let failed = Seen {
            message: Some("The stream could not start: no video".into()),
            ..seen(true, AppsSeen::NotLoaded)
        };
        assert_eq!(
            c.next(&failed),
            Step::Fail("The stream could not start: no video".into())
        );
    }

    #[test]
    fn quit_on_exit_closes_after_the_stream_and_keeps_its_error() {
        let mut c = cli(Some("Steam"), true);
        assert!(!c.stream_ended(None), "no stream ran yet");
        c.streaming_began();
        assert!(c.stream_ended(None));
        assert_eq!(c.failed, None);
        let mut c = cli(Some("Steam"), true);
        c.streaming_began();
        assert!(c.stream_ended(Some("The stream ended: host error".into())));
        assert_eq!(c.failed.as_deref(), Some("The stream ended: host error"));
        let mut c = cli(Some("Steam"), false);
        c.streaming_began();
        assert!(
            !c.stream_ended(None),
            "without --quit-on-exit the launcher stays"
        );
    }
}
