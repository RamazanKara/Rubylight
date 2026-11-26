//! Typed configuration enums, port layout, and resolution/KLID parsers.
use super::number_text;
use anyhow::{Result, bail};
use serde::Serialize;

/// Default UDP port for the DSU (cemuhook) motion server.
pub const DSU_DEFAULT_PORT: u16 = 26760;
/// Offset from the base port for the host's microphone receiver port.
pub const MIC_PORT_OFFSET: u16 = 13;
/// The setting that caps the picture every stream is encoded at.
pub const MAX_STREAM_RESOLUTION: &str = "max_stream_resolution";
/// The value that leaves every device the size it asks for: the default.
pub const MAX_STREAM_RESOLUTION_NATIVE: &str = "native";
/// The smallest and largest cap, the range the streamed display itself has
/// (`dd_manual_resolution`).
pub const MAX_STREAM_RESOLUTION_RANGE: (u32, u32, u32, u32) = (320, 200, 7680, 4320);
/// The most video FEC a device may ask for (`rlFecPercentage`) or reach
/// through its loss reports, in percent of the video packets: half of them.
pub const FEC_MAX_PERCENTAGE: u8 = 50;
/// The WebRTC ports the TV expects by default.
pub const WEBRTC_PORT: u16 = 8000;
pub const WEBRTC_MEDIA_PORT: u16 = 8001;

/// A resolution `WIDTHxHEIGHT`.
pub fn parse_resolution(text: &str) -> Option<(u32, u32)> {
    let (width, height) = text.trim().split_once(['x', 'X'])?;
    let width = width.trim().parse::<u32>().ok()?;
    let height = height.trim().parse::<u32>().ok()?;
    ((1..=16384).contains(&width) && (1..=16384).contains(&height)).then_some((width, height))
}

/// The cap `max_stream_resolution` sets: `None` for `native` (or an empty
/// value), which leaves each device the picture it asks for, and an error for
/// anything else but a `WIDTHxHEIGHT` within [`MAX_STREAM_RESOLUTION_RANGE`].
pub fn parse_max_stream_resolution(text: &str) -> Result<Option<(u32, u32)>> {
    let text = number_text(text);
    if text.is_empty() || text.eq_ignore_ascii_case(MAX_STREAM_RESOLUTION_NATIVE) {
        return Ok(None);
    }
    let (min_width, min_height, max_width, max_height) = MAX_STREAM_RESOLUTION_RANGE;
    match parse_resolution(text) {
        Some((width, height))
            if (min_width..=max_width).contains(&width)
                && (min_height..=max_height).contains(&height) =>
        {
            Ok(Some((width, height)))
        }
        _ => bail!(
            "{MAX_STREAM_RESOLUTION} must be {MAX_STREAM_RESOLUTION_NATIVE} or WIDTHxHEIGHT between {min_width}x{min_height} and {max_width}x{max_height}"
        ),
    }
}

/// Quick Suspend (`Config::quick_suspend`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuickSuspendMode {
    /// No game is ever paused.
    Off,
    /// A device or the console pauses the running game until it is resumed.
    Manual,
}

/// Updates (`Config::updates`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdatesMode {
    /// No automatic check; Check now still works.
    Off,
    /// Checks and tells the user about a new release.
    TellMe,
    /// As `TellMe`, and installs the release when nothing is streaming.
    Install,
}

/// Adaptive bitrate (`Config::bitrate_adaptation`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BitrateAdaptation {
    /// The stream keeps the bitrate it was set to.
    Off,
    /// Lowered on loss error correction cannot repair, then raised again.
    On,
    /// As `On`, and also lowered as network delay builds (congestion control).
    Delay,
}

/// Packet pacing (`Config::packet_pacing`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PacketPacing {
    /// Adaptive pacing for clients that report a Wi-Fi or cellular route on
    /// H.264, HEVC and AV1; every other stream keeps the route policy.
    Auto,
    /// Adaptive pacing for every client and codec, PyroWave included.
    Always,
    /// No adaptive pacing; the loss latch may still pace after video loss.
    Off,
    /// A fixed send rate in kbps that takes over pacing.
    Fixed(i64),
}

/// When a remote monitor is removed from Windows (`Config::remove_remote_monitor`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoveRemoteMonitor {
    /// Kept after its stream ends, for the device to resume where it was.
    Never,
    /// Removed when its stream ends.
    StreamEnd,
    /// Removed as soon as the device's connection drops, even while kept for resuming.
    Disconnect,
}

/// `keyboard_layout`: the layout the host types a device's keys with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyboardLayoutSetting {
    /// The layout the device reports (0x553E).
    Client,
    /// US English.
    Us,
    /// A Windows keyboard layout id (KLID), written as 8 hex digits.
    Klid(u32),
}

/// A KLID as Windows writes it: exactly 8 hex digits, such as `00000407`.
pub fn parse_klid(text: &str) -> Option<u32> {
    (text.len() == 8 && text.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| u32::from_str_radix(text, 16).ok())
        .flatten()
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct Ports {
    pub http: u16,
    pub https: u16,
    pub web: u16,
    /// The guest page and its socket (docs/browser-guests.md), the one port guests on the
    /// internet need forwarded besides their media; nothing of the console's answers on it.
    pub guest: u16,
    pub video: u16,
    pub control: u16,
    pub audio: u16,
    pub mic: u16,
    pub rtsp: u16,
}

impl Ports {
    pub fn from_base(n: u16) -> Self {
        Self {
            http: n,
            https: n - 5,
            web: n + 1,
            guest: n + 2,
            video: n + 9,
            control: n + 10,
            audio: n + 11,
            mic: n + MIC_PORT_OFFSET,
            rtsp: n + 21,
        }
    }
}
