//! `GET /audio` and `POST /audio/settings`: the stream's audio processing.
//!
//! The settings reuse the Radeon panel's [`Setting`] and [`Change`].
use serde::{Deserialize, Serialize};

use crate::radeon::{Change, Setting};

/// `Setting::id` of night mode (a `toggle`).
pub const NIGHT_MODE: &str = "audio.night_mode";
/// `Setting::id` of headphone surround (a `toggle`).
pub const HEADPHONE_SURROUND: &str = "audio.headphone_surround";
/// `Setting::group` of both.
pub const GROUP: &str = "sound";

/// `Setting::reason` values: a note on a setting this stream doesn't apply.
/// The owner can still write it, so it stays `configurable` with its `value`;
/// only [`reason::NO_SESSION`] comes with `temporarily_unavailable` and no
/// `value`. Clients show reasons they don't know by `message`.
pub mod reason {
    /// The caller has no streaming or suspended session, so nothing is
    /// writable: `temporarily_unavailable`, without a `value`.
    pub const NO_SESSION: &str = "no_session";
    /// The client asked for 5.1 or 7.1 and renders its own speakers.
    pub const CLIENT_SURROUND: &str = "client_surround";
    /// Capture uses Game only audio, which is stereo.
    pub const GAME_ONLY_AUDIO: &str = "game_only_audio";
    /// The PC's playback device is stereo.
    pub const STEREO_ENDPOINT: &str = "stereo_endpoint";
    /// Audio isn't being streamed.
    pub const MUTED: &str = "muted";
    /// Turned on mid-stream without 8-channel capture: it applies from the
    /// next stream.
    pub const NEXT_STREAM: &str = "next_stream";
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Audio {
    pub revision: u64,
    pub settings: Vec<Setting>,
}

/// `POST /audio/settings`. A request: unknown fields are rejected.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AudioWrite {
    pub changes: Vec<Change>,
}
