//! Stream DVR (instant replay), while the host lists [`crate::feature::STREAM_DVR`]:
//! `GET /dvr` (the DVR state), `POST /dvr/clip` (save the replay buffer's last seconds),
//! `POST /dvr/record` (start or stop a recording), `GET /dvr/clips` (saved clips, newest
//! first) and `DELETE /dvr/clips/{id}`. The state mirrors control message 0x554A
//! (`rubylight_protocol::stream_dvr`), whose reasons have the same names.
use serde::{Deserialize, Serialize};

/// The DVR state for the current stream.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DvrState {
    /// The feature is off on the host, or nothing streams ([`DvrStatus::reason`] says which).
    #[default]
    Off,
    /// The replay buffer is filling; clips can be saved.
    Armed,
    /// A recording runs; the replay buffer keeps running beside it.
    Recording,
    /// A clip is being written.
    Saving,
    /// This stream cannot be clipped ([`DvrStatus::reason`]).
    Unsupported,
    /// Writing failed ([`DvrStatus::reason`]); clears once the cause is gone.
    Error,
    #[serde(other)]
    Unknown,
}

/// Why a clip was refused or failed, or why the DVR is off or unsupported.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DvrReason {
    #[default]
    None,
    Disabled,
    Pyrowave,
    DiskFull,
    QuotaExceeded,
    NoKeyframeYet,
    Busy,
    PermissionDenied,
    AudioCodecUnsupported,
    IoError,
    NotStreaming,
    #[serde(other)]
    Unknown,
}

/// `GET /dvr`, and the `status` of `POST /dvr/record`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DvrStatus {
    pub state: DvrState,
    pub reason: DvrReason,
    /// Seconds of keyframe-backed video in the replay buffer now.
    pub ring_buffered_secs: u32,
    /// The replay buffer's configured length (`dvr_ring_seconds`).
    pub ring_capacity_secs: u32,
    pub recording: bool,
    /// 0 unless `recording`.
    pub recording_elapsed_secs: u32,
    /// Bytes the saved clips take.
    pub quota_used_bytes: u64,
    /// The clips quota (`dvr_quota_gb`) in bytes.
    pub quota_limit_bytes: u64,
    /// The clips folder exists and can be written.
    pub clips_dir_available: bool,
    /// The file format of new clips: `mp4`.
    pub container: String,
}

/// The body of `POST /dvr/clip`; `{}` saves the whole replay buffer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClipRequest {
    /// Seconds to save, clamped by the host to at least 5 and at most the buffer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_secs: Option<u16>,
}

/// What `POST /dvr/record` asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordAction {
    Start,
    Stop,
}

/// The body of `POST /dvr/record`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordRequest {
    pub action: RecordAction,
}

/// The answer of `POST /dvr/record`: the new state, and the finished recording on `stop`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RecordResult {
    pub status: DvrStatus,
    pub clip: Option<ClipSummary>,
}

/// The answer (200) of `POST /dvr/clip`: the saved clip.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClipSaved {
    pub clip: ClipSummary,
}

/// Whether a clip came from the replay buffer or a recording.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClipKind {
    #[default]
    Replay,
    Manual,
    #[serde(other)]
    Unknown,
}

/// One saved clip.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClipSummary {
    /// `[A-Za-z0-9_-]{8,64}`; names the file.
    pub id: String,
    /// The game that streamed, or `Desktop`.
    pub app_title: String,
    pub created_unix_ms: u64,
    pub duration_ms: u64,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    /// `h264`, `hevc` or `av1`.
    pub codec: String,
    pub hdr: bool,
    /// `mp4`.
    pub container: String,
    pub size_bytes: u64,
    pub kind: ClipKind,
    /// Kept from a recording the host could not finish (it stopped unexpectedly).
    pub recovered: bool,
}

/// `GET /dvr/clips?limit=50&cursor=...`, newest first.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClipList {
    pub clips: Vec<ClipSummary>,
    /// Pass as `cursor` for the next page; null on the last page.
    pub next_cursor: Option<String>,
    pub quota_used_bytes: u64,
    pub quota_limit_bytes: u64,
}
