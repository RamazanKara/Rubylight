//! Stream DVR (instant replay), control message 0x554A in both directions: the client asks
//! the host to save the last seconds of the stream as a clip or to start and stop a
//! recording, and the host answers with its DVR state and each clip's result.
//!
//! A host that can keep clips lists [`SERVERINFO_FIELD`] `1` in serverinfo and `0x554A` in
//! the stream's `a=x-rl-control` line; whether the replay buffer runs for this stream (the
//! feature is off by default, a PyroWave stream cannot be clipped) travels in
//! [`HostState`]. A client sends nothing to a host that does not list the id.
//!
//! Frame, little-endian: version u8 ([`VERSION`]), kind u8 ([`KIND_REQUEST`],
//! [`KIND_STATE`], [`KIND_CLIP`]), flags u16 (0, ignored), request id u32 (chosen by the
//! client, echoed by the host, 0 for a state the host sends on its own), then the body of
//! the kind. Bytes after the body are ignored; a short, unknown or bad frame decodes to
//! `None`.

use alloc::string::String;
use alloc::vec::Vec;

/// Control-stream message type of both directions.
pub const DVR_STATE_MESSAGE_TYPE: u16 = 0x554A;
/// The serverinfo element of a host that keeps clips.
pub const SERVERINFO_FIELD: &str = "StreamDvrVersion";
/// The [`SERVERINFO_FIELD`] value this crate speaks.
pub const SERVERINFO_VERSION: u32 = 1;
/// The frame version this crate reads and writes.
pub const VERSION: u8 = 1;
/// Size of the frame header before the body.
pub const HEADER_BYTES: usize = 8;
/// Most UTF-8 bytes in a clip id.
pub const MAX_CLIP_ID_BYTES: usize = 64;
/// The shortest clip a host saves; shorter requests are raised to it.
pub const MIN_CLIP_SECS: u16 = 5;

/// Client to host: [`Request`].
pub const KIND_REQUEST: u8 = 1;
/// Host to client: [`HostState`].
pub const KIND_STATE: u8 = 2;
/// Host to client: [`ClipResult`].
pub const KIND_CLIP: u8 = 3;

const REQUEST_BODY: usize = 3;
const STATE_BODY: usize = 26;
const CLIP_BODY: usize = 14;

/// What the client asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Op {
    /// Answer with a [`HostState`].
    QueryState = 1,
    /// Save the replay buffer's last seconds as a clip; a [`ClipResult`] follows.
    SaveClip = 2,
    /// Start a recording; a [`HostState`] follows.
    StartRecord = 3,
    /// Stop the recording; a [`ClipResult`] for the recording follows.
    StopRecord = 4,
}

impl Op {
    pub fn from_u8(value: u8) -> Option<Self> {
        Some(match value {
            1 => Self::QueryState,
            2 => Self::SaveClip,
            3 => Self::StartRecord,
            4 => Self::StopRecord,
            _ => return None,
        })
    }
}

/// The host's DVR state for this stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum State {
    /// The feature is off on the host, or nothing streams.
    Off = 0,
    /// The replay buffer is filling; clips can be saved.
    Armed = 1,
    /// A recording runs (the replay buffer keeps running beside it).
    Recording = 2,
    /// A clip is being written.
    Saving = 3,
    /// This stream cannot be clipped; [`Reason`] says why.
    Unsupported = 4,
    /// Writing failed; [`Reason`] says why. Clears once the cause is gone.
    Error = 5,
}

impl State {
    pub fn from_u8(value: u8) -> Option<Self> {
        Some(match value {
            0 => Self::Off,
            1 => Self::Armed,
            2 => Self::Recording,
            3 => Self::Saving,
            4 => Self::Unsupported,
            5 => Self::Error,
            _ => return None,
        })
    }
}

/// Why a clip was refused or failed, or why the DVR is off or unsupported. A value from a
/// newer peer reads as [`Reason::Unknown`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Reason {
    None = 0,
    /// The feature is off in the host's settings.
    Disabled = 1,
    /// PyroWave streams cannot be remuxed into a clip.
    Pyrowave = 2,
    /// The clips folder's drive is nearly full.
    DiskFull = 3,
    /// Saved clips use the whole quota and none can be removed.
    QuotaExceeded = 4,
    /// No keyframe is buffered yet; the host asked for one.
    NoKeyframeYet = 5,
    /// Another clip is being written.
    Busy = 6,
    /// This device may not save clips on this stream (a guest, or no Launch permission).
    PermissionDenied = 7,
    /// The stream's audio is passthrough (PCM, AC3, E-AC3), not Opus.
    AudioCodecUnsupported = 8,
    /// The clips folder could not be written.
    IoError = 9,
    /// Nothing streams.
    NotStreaming = 10,
    Unknown = 11,
}

impl Reason {
    pub fn from_u8(value: u8) -> Self {
        match value {
            0 => Self::None,
            1 => Self::Disabled,
            2 => Self::Pyrowave,
            3 => Self::DiskFull,
            4 => Self::QuotaExceeded,
            5 => Self::NoKeyframeYet,
            6 => Self::Busy,
            7 => Self::PermissionDenied,
            8 => Self::AudioCodecUnsupported,
            9 => Self::IoError,
            10 => Self::NotStreaming,
            _ => Self::Unknown,
        }
    }

    /// The stable lowercase name the native API, logs and the console use.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Disabled => "disabled",
            Self::Pyrowave => "pyrowave",
            Self::DiskFull => "disk_full",
            Self::QuotaExceeded => "quota_exceeded",
            Self::NoKeyframeYet => "no_keyframe_yet",
            Self::Busy => "busy",
            Self::PermissionDenied => "permission_denied",
            Self::AudioCodecUnsupported => "audio_codec_unsupported",
            Self::IoError => "io_error",
            Self::NotStreaming => "not_streaming",
            Self::Unknown => "unknown",
        }
    }
}

/// How a clip request ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Outcome {
    Saved = 0,
    Refused = 1,
    Failed = 2,
}

impl Outcome {
    pub fn from_u8(value: u8) -> Option<Self> {
        Some(match value {
            0 => Self::Saved,
            1 => Self::Refused,
            2 => Self::Failed,
            _ => return None,
        })
    }
}

/// Client to host. Body: op u8, clip seconds u16 (0 = the whole replay buffer; the host
/// clamps to at least [`MIN_CLIP_SECS`] and at most its buffer).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Request {
    pub request_id: u32,
    pub op: Op,
    pub duration_secs: u16,
}

/// Host to client, after a [`Op::QueryState`] or [`Op::StartRecord`] and whenever the state
/// changes. Body: state u8, reason u8, buffered seconds u16, buffer capacity seconds u16,
/// recording seconds u32, quota used u64, quota limit u64 (bytes).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HostState {
    pub request_id: u32,
    pub state: State,
    pub reason: Reason,
    /// Seconds of keyframe-backed video in the replay buffer now.
    pub ring_buffered_secs: u16,
    /// The replay buffer's configured length.
    pub ring_capacity_secs: u16,
    /// 0 unless [`State::Recording`].
    pub recording_elapsed_secs: u32,
    pub quota_used_bytes: u64,
    pub quota_limit_bytes: u64,
}

/// Host to client, once a [`Op::SaveClip`] or [`Op::StopRecord`] ends. Body: outcome u8,
/// reason u8, clip seconds u16, file bytes u64, clip id length u16, clip id UTF-8.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClipResult {
    pub request_id: u32,
    pub outcome: Outcome,
    pub reason: Reason,
    pub duration_secs: u16,
    pub file_bytes: u64,
    /// The saved clip's id (empty unless saved), at most [`MAX_CLIP_ID_BYTES`].
    pub clip_id: String,
}

/// One 0x554A frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Message {
    Request(Request),
    State(HostState),
    Clip(ClipResult),
}

fn header(out: &mut Vec<u8>, kind: u8, request_id: u32) {
    out.push(VERSION);
    out.push(kind);
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&request_id.to_le_bytes());
}

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn u64_at(b: &[u8], at: usize) -> u64 {
    let mut bytes = [0; 8];
    bytes.copy_from_slice(&b[at..at + 8]);
    u64::from_le_bytes(bytes)
}

impl Message {
    /// The frame on the wire, or `None` when a clip id is over [`MAX_CLIP_ID_BYTES`].
    pub fn encode(&self) -> Option<Vec<u8>> {
        let mut out = Vec::with_capacity(HEADER_BYTES + STATE_BODY);
        match self {
            Self::Request(r) => {
                header(&mut out, KIND_REQUEST, r.request_id);
                out.push(r.op as u8);
                out.extend_from_slice(&r.duration_secs.to_le_bytes());
            }
            Self::State(s) => {
                header(&mut out, KIND_STATE, s.request_id);
                out.push(s.state as u8);
                out.push(s.reason as u8);
                out.extend_from_slice(&s.ring_buffered_secs.to_le_bytes());
                out.extend_from_slice(&s.ring_capacity_secs.to_le_bytes());
                out.extend_from_slice(&s.recording_elapsed_secs.to_le_bytes());
                out.extend_from_slice(&s.quota_used_bytes.to_le_bytes());
                out.extend_from_slice(&s.quota_limit_bytes.to_le_bytes());
            }
            Self::Clip(c) => {
                if c.clip_id.len() > MAX_CLIP_ID_BYTES {
                    return None;
                }
                header(&mut out, KIND_CLIP, c.request_id);
                out.push(c.outcome as u8);
                out.push(c.reason as u8);
                out.extend_from_slice(&c.duration_secs.to_le_bytes());
                out.extend_from_slice(&c.file_bytes.to_le_bytes());
                out.extend_from_slice(&(c.clip_id.len() as u16).to_le_bytes());
                out.extend_from_slice(c.clip_id.as_bytes());
            }
        }
        Some(out)
    }

    /// A frame from the wire; `None` for a short frame, another version, an unknown kind,
    /// op, state or outcome, or a clip id that is too long or not UTF-8.
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < HEADER_BYTES || bytes[0] != VERSION {
            return None;
        }
        let request_id = u32_at(bytes, 4);
        let body = &bytes[HEADER_BYTES..];
        Some(match bytes[1] {
            KIND_REQUEST if body.len() >= REQUEST_BODY => Self::Request(Request {
                request_id,
                op: Op::from_u8(body[0])?,
                duration_secs: u16_at(body, 1),
            }),
            KIND_STATE if body.len() >= STATE_BODY => Self::State(HostState {
                request_id,
                state: State::from_u8(body[0])?,
                reason: Reason::from_u8(body[1]),
                ring_buffered_secs: u16_at(body, 2),
                ring_capacity_secs: u16_at(body, 4),
                recording_elapsed_secs: u32_at(body, 6),
                quota_used_bytes: u64_at(body, 10),
                quota_limit_bytes: u64_at(body, 18),
            }),
            KIND_CLIP if body.len() >= CLIP_BODY => {
                let len = usize::from(u16_at(body, 12));
                if len > MAX_CLIP_ID_BYTES || body.len() < CLIP_BODY + len {
                    return None;
                }
                let clip_id = core::str::from_utf8(&body[CLIP_BODY..CLIP_BODY + len]).ok()?;
                Self::Clip(ClipResult {
                    request_id,
                    outcome: Outcome::from_u8(body[0])?,
                    reason: Reason::from_u8(body[1]),
                    duration_secs: u16_at(body, 2),
                    file_bytes: u64_at(body, 4),
                    clip_id: String::from(clip_id),
                })
            }
            _ => return None,
        })
    }
}

/// The clip length a host saves for `requested` seconds (0 = the whole buffer) with a
/// buffer of `capacity` seconds.
pub fn clamp_clip_secs(requested: u16, capacity: u16) -> u16 {
    let capacity = capacity.max(MIN_CLIP_SECS);
    if requested == 0 {
        capacity
    } else {
        requested.clamp(MIN_CLIP_SECS, capacity)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn state() -> HostState {
        HostState {
            request_id: 7,
            state: State::Armed,
            reason: Reason::None,
            ring_buffered_secs: 42,
            ring_capacity_secs: 60,
            recording_elapsed_secs: 0,
            quota_used_bytes: 4_200_000_000,
            quota_limit_bytes: 20 << 30,
        }
    }

    #[test]
    fn request_layout_is_fixed() {
        let wire = Message::Request(Request {
            request_id: 0x0102_0304,
            op: Op::SaveClip,
            duration_secs: 60,
        })
        .encode()
        .unwrap();
        assert_eq!(wire, [1, 1, 0, 0, 4, 3, 2, 1, 2, 60, 0]);
    }

    #[test]
    fn every_kind_round_trips() {
        let messages = [
            Message::Request(Request {
                request_id: 1,
                op: Op::StopRecord,
                duration_secs: 0,
            }),
            Message::State(state()),
            Message::Clip(ClipResult {
                request_id: 9,
                outcome: Outcome::Saved,
                reason: Reason::None,
                duration_secs: 61,
                file_bytes: 84_000_000,
                clip_id: "20261126-153000-ab12".into(),
            }),
        ];
        for message in messages {
            let wire = message.encode().unwrap();
            assert_eq!(Message::decode(&wire), Some(message));
        }
        assert_eq!(
            Message::State(state()).encode().unwrap().len(),
            HEADER_BYTES + STATE_BODY
        );
    }

    #[test]
    fn bad_frames_are_refused_and_trailing_bytes_ignored() {
        let mut wire = Message::State(state()).encode().unwrap();
        assert_eq!(Message::decode(&wire[..wire.len() - 1]), None);
        wire.extend_from_slice(&[9, 9]);
        assert_eq!(Message::decode(&wire), Some(Message::State(state())));
        wire[0] = 2;
        assert_eq!(Message::decode(&wire), None);
        assert_eq!(Message::decode(&[1, 4, 0, 0, 0, 0, 0, 0, 0]), None);
        assert_eq!(Message::decode(&[1, 1, 0, 0, 0, 0, 0, 0, 9, 0, 0]), None);
        let long = Message::Clip(ClipResult {
            request_id: 0,
            outcome: Outcome::Saved,
            reason: Reason::None,
            duration_secs: 0,
            file_bytes: 0,
            clip_id: "x".repeat(MAX_CLIP_ID_BYTES + 1),
        });
        assert_eq!(long.encode(), None);
        let mut clip = vec![1, 3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        clip.extend_from_slice(&[0; 8]);
        clip.extend_from_slice(&[1, 0, 0xFF]);
        assert_eq!(Message::decode(&clip), None, "clip id must be UTF-8");
    }

    #[test]
    fn unknown_reasons_read_as_unknown() {
        let mut wire = Message::State(state()).encode().unwrap();
        wire[HEADER_BYTES + 1] = 200;
        let Some(Message::State(decoded)) = Message::decode(&wire) else {
            panic!("state decodes");
        };
        assert_eq!(decoded.reason, Reason::Unknown);
        assert_eq!(Reason::Pyrowave.as_str(), "pyrowave");
        assert_eq!(Reason::from_u8(Reason::IoError as u8), Reason::IoError);
    }

    #[test]
    fn clip_seconds_clamp_to_the_buffer() {
        assert_eq!(clamp_clip_secs(0, 60), 60);
        assert_eq!(clamp_clip_secs(1, 60), MIN_CLIP_SECS);
        assert_eq!(clamp_clip_secs(500, 90), 90);
        assert_eq!(clamp_clip_secs(30, 60), 30);
        assert_eq!(clamp_clip_secs(0, 0), MIN_CLIP_SECS);
    }
}
