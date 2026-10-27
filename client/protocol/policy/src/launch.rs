//! Launching and leaving a stream: what a tap on a PC does, whether leaving quits the game, which
//! endings stop on a dialog, and how a PC offers Input-only.
//!
//! Android's `LaunchPolicy.java`, `StreamExitPolicy.java` and `InputOnlyPolicy.java`. [`Role`] and
//! [`Control`] mirror Android's `NvApp` enums (the desktop client's `AppRole` and `AppControl`), so
//! this crate stays free of the protocol crates.

use serde_json::Value;

use crate::trace::{Args, Subject};

/// What a launch starts on the host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Stream,
    RemoteMonitor,
    InputOnly,
}

/// Host control tiles; [`Control::None`] is an ordinary app.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    None,
    Resume,
    DisconnectMonitor,
    DisconnectInput,
    Terminate,
    Monitor,
    Input,
    RunningGame,
}

/// With quick resume on, a tap on a PC that runs a game goes straight back into that game;
/// otherwise, and for any other PC, the tap opens its library.
pub fn resumes_on_pc_tap(quick_resume: bool, game_running: bool) -> bool {
    quick_resume && game_running
}

/// Quit on leave is offered for a game stream this device may quit; actions and other roles end
/// differently.
pub fn offers_quit_on_leave(can_launch: bool, control: Control, role: Role) -> bool {
    can_launch && control == Control::None && role == Role::Stream
}

// moonlight-common-c's ML_ERROR_* termination codes.
const GRACEFUL_TERMINATION: i32 = 0;
const NO_VIDEO_TRAFFIC: i32 = -100;
const NO_VIDEO_FRAME: i32 = -101;
const UNEXPECTED_EARLY_TERMINATION: i32 = -102;
const PROTECTED_CONTENT: i32 = -103;
const FRAME_CONVERSION: i32 = -104;

/// Whether leaving a stream also quits its game. Only a leave the player asks for can quit; a lost
/// connection, an error or the host ending the stream never does, so the game is still there to
/// resume.
pub fn quits_on_leave(quit_on_leave_chosen: bool, role: Role, connected: bool) -> bool {
    quit_on_leave_chosen && connected && role == Role::Stream
}

/// Whether a stream that ended with `error_code` stops on a dialog the player has to read. That is
/// only an error that comes with advice: the port test says the network blocks the stream, no
/// video ever arrived, no complete frame, or the PC could not start or encode the stream. The host
/// ending the stream or going away, a lost connection, a timeout and a reason only the PC knows
/// close the stream with a message instead. A network grace period that may have run out and used
/// up automatic reconnecting also keep the dialog, for its advice and its Reconnect button.
pub fn termination_needs_action(
    error_code: i32,
    port_test_blocked: bool,
    network_grace_gave_up: bool,
    recovery_exhausted: bool,
) -> bool {
    if error_code == GRACEFUL_TERMINATION {
        return false;
    }
    port_test_blocked
        || network_grace_gave_up
        || recovery_exhausted
        || matches!(
            error_code,
            NO_VIDEO_TRAFFIC
                | NO_VIDEO_FRAME
                | UNEXPECTED_EARLY_TERMINATION
                | PROTECTED_CONTENT
                | FRAME_CONVERSION
        )
}

/// How this device offers Input-only on one PC.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputOnlyMode {
    /// The host's own Input-only tile; no video starts.
    Native,
    /// A small ordinary stream whose video and audio are discarded unseen.
    SmallStream,
    /// Not offered: the setting is off.
    Off,
}

/// Why a PC cannot take a small-stream Input-only session now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputOnlyBlocker {
    Pair,
    ViewPermission,
    InputPermission,
}

// The smallest stream every host accepts: 16:9, a frame rate GameStream offers, and the lowest
// bitrate the settings allow.
pub const SMALL_WIDTH: u32 = 640;
pub const SMALL_HEIGHT: u32 = 360;
pub const SMALL_FPS: u32 = 30;
pub const SMALL_BITRATE_KBPS: u32 = 500;
// Hosts add 20% forward error correction to video by default, and send stereo audio at the normal
// quality the core asks for at this bitrate.
const VIDEO_FEC_PERCENT: u32 = 20;
const AUDIO_KBPS: u32 = 96;

// Apollo permission bits, as serverinfo reports them.
pub const PERMISSION_INPUT: u64 = 0x0000_1F00;
pub const PERMISSION_VIEW: u64 = 1 << 25;
pub const PERMISSION_LAUNCH: u64 = 1 << 26;

/// `remote_session_tiles` is the host's `RemoteSessionTiles` feature; `host_lists_input_only`
/// says its app list has an Input-only tile.
pub fn input_only_mode(
    any_host_enabled: bool,
    remote_session_tiles: bool,
    host_lists_input_only: bool,
) -> InputOnlyMode {
    if host_lists_input_only || remote_session_tiles {
        InputOnlyMode::Native
    } else if any_host_enabled {
        InputOnlyMode::SmallStream
    } else {
        InputOnlyMode::Off
    }
}

/// Why this PC cannot take a small-stream Input-only session now, or `None` when it can.
/// `permission` is `None` for hosts that report no permissions; they allow everything.
pub fn small_stream_blocker(paired: bool, permission: Option<u64>) -> Option<InputOnlyBlocker> {
    let has = |mask: u64| match permission {
        None => true,
        Some(bits) => bits & mask != 0,
    };
    if !paired {
        Some(InputOnlyBlocker::Pair)
    } else if !has(PERMISSION_VIEW | PERMISSION_LAUNCH) {
        Some(InputOnlyBlocker::ViewPermission)
    } else if !has(PERMISSION_INPUT) {
        Some(InputOnlyBlocker::InputPermission)
    } else {
        None
    }
}

/// Whether a small-stream session for this game leaves the host's running game alone.
pub fn keeps_running_game(host_running_game: bool, this_game_running: bool) -> bool {
    !host_running_game || this_game_running
}

/// What the small stream costs on the network at most, in kbit/s.
pub const fn estimated_kbps() -> u32 {
    SMALL_BITRATE_KBPS * (100 + VIDEO_FEC_PERCENT) / 100 + AUDIO_KBPS
}

/// [`estimated_kbps`] over an hour, in megabytes.
pub const fn estimated_megabytes_per_hour() -> u32 {
    estimated_kbps() * 3600 / 8 / 1000
}

fn role(value: Option<&str>) -> Result<Role, String> {
    match value {
        Some("stream") => Ok(Role::Stream),
        Some("remote_monitor") => Ok(Role::RemoteMonitor),
        Some("input_only") => Ok(Role::InputOnly),
        _ => Err(format!("unknown role {value:?}")),
    }
}

fn control(value: Option<&str>) -> Result<Control, String> {
    match value {
        Some("none") => Ok(Control::None),
        Some("resume") => Ok(Control::Resume),
        Some("disconnect_monitor") => Ok(Control::DisconnectMonitor),
        Some("disconnect_input") => Ok(Control::DisconnectInput),
        Some("terminate") => Ok(Control::Terminate),
        Some("monitor") => Ok(Control::Monitor),
        Some("input") => Ok(Control::Input),
        Some("running_game") => Ok(Control::RunningGame),
        _ => Err(format!("unknown control {value:?}")),
    }
}

fn flag(args: &Value, i: usize) -> Result<bool, String> {
    args[i]
        .as_bool()
        .ok_or_else(|| format!("argument {i} is not a boolean: {}", args[i]))
}

/// The traced Java class. All three are static only, so their traces have `"new": null`.
#[derive(Clone, Copy)]
pub(crate) enum Traced {
    Launch,
    Exit,
    InputOnly,
}

pub(crate) fn traced(policy: Traced, args: &Value) -> Result<Box<dyn Subject>, String> {
    if !args.is_null() {
        return Err("static policy: new must be null".into());
    }
    Ok(Box::new(policy))
}

impl Subject for Traced {
    fn call(&mut self, method: &str, args: &Value) -> Result<Value, String> {
        match (*self, method) {
            (Traced::Launch, "resumes_on_pc_tap") => {
                Args::new(args, 2)?;
                Ok(resumes_on_pc_tap(flag(args, 0)?, flag(args, 1)?).into())
            }
            (Traced::Launch, "offers_quit_on_leave") => {
                let a = Args::new(args, 3)?;
                let control = control(a.text(1)?)?;
                Ok(offers_quit_on_leave(flag(args, 0)?, control, role(a.text(2)?)?).into())
            }
            (Traced::Exit, "quits_on_leave") => {
                let a = Args::new(args, 3)?;
                Ok(quits_on_leave(flag(args, 0)?, role(a.text(1)?)?, flag(args, 2)?).into())
            }
            (Traced::Exit, "termination_needs_action") => {
                let a = Args::new(args, 4)?;
                let (blocked, grace, exhausted) = (flag(args, 1)?, flag(args, 2)?, flag(args, 3)?);
                Ok(termination_needs_action(a.int(0)?, blocked, grace, exhausted).into())
            }
            (Traced::InputOnly, "mode") => {
                Args::new(args, 3)?;
                let mode = input_only_mode(flag(args, 0)?, flag(args, 1)?, flag(args, 2)?);
                Ok(match mode {
                    InputOnlyMode::Native => "native",
                    InputOnlyMode::SmallStream => "small_stream",
                    InputOnlyMode::Off => "off",
                }
                .into())
            }
            (Traced::InputOnly, "small_stream_blocker") => {
                let a = Args::new(args, 2)?;
                // Java's absent permissions are -1.
                let permission = match a.int::<i64>(1)? {
                    -1 => None,
                    _ => Some(a.int(1)?),
                };
                Ok(match small_stream_blocker(flag(args, 0)?, permission) {
                    Some(InputOnlyBlocker::Pair) => "pair".into(),
                    Some(InputOnlyBlocker::ViewPermission) => "view_permission".into(),
                    Some(InputOnlyBlocker::InputPermission) => "input_permission".into(),
                    None => Value::Null,
                })
            }
            (Traced::InputOnly, "keeps_running_game") => {
                Args::new(args, 2)?;
                Ok(keeps_running_game(flag(args, 0)?, flag(args, 1)?).into())
            }
            (Traced::InputOnly, "estimated_kbps") => {
                Args::new(args, 0).map(|_| estimated_kbps().into())
            }
            (Traced::InputOnly, "estimated_megabytes_per_hour") => {
                Args::new(args, 0).map(|_| estimated_megabytes_per_hour().into())
            }
            _ => Err(format!("unknown method {method}")),
        }
    }
}
