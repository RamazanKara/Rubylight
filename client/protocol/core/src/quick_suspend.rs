//! Quick Suspend control-stream message (`0x554B`): pausing the running game's
//! process tree on the host and resuming it, in both directions.
//!
//! Frame, little-endian: version u8 ([`VERSION`]), kind u8 ([`Kind`]), flags
//! u16, request id u32, then a body by kind:
//!
//! - [`Kind::ClientRequest`] (client to host): op u8 ([`Op`]). 9 bytes.
//! - [`Kind::HostState`] (host to client): state u8 ([`State`]), reason u8
//!   ([`Reason`]), suspended process count u16, suspended since (Unix ms) u64.
//!   20 bytes. The host answers a request with the request's id, and pushes a
//!   change no request caused (another device resumed the game) with id 0.
//!
//! Trailing bytes are ignored so newer peers can append fields without
//! bumping the version. A client sends it only when the host announced the id
//! in `a=x-rl-control`.

/// Control-stream message type (`client/protocol/core/src/ids.rs`).
pub const QUICK_SUSPEND_MESSAGE_TYPE: u16 = 0x554b;
/// The serverinfo element of a host that takes it, while `quick_suspend` is on.
pub const SERVERINFO_FIELD: &str = "QuickSuspendVersion";
/// Its value: this frame version.
pub const SERVERINFO_VERSION: u8 = 1;
/// The frame version this crate reads and writes.
pub const VERSION: u8 = 1;
/// Size of the header every frame starts with.
pub const HEADER_BYTES: usize = 8;
/// Size of a v1 client request.
pub const REQUEST_BYTES: usize = HEADER_BYTES + 1;
/// Size of a v1 host state.
pub const STATE_BYTES: usize = HEADER_BYTES + 12;

/// Which side sent a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Kind {
    ClientRequest = 1,
    HostState = 2,
}

/// What a client asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Op {
    /// Only the current state.
    QueryState = 1,
    Suspend = 2,
    Resume = 3,
}

impl Op {
    pub fn from_u8(value: u8) -> Option<Self> {
        Some(match value {
            1 => Self::QueryState,
            2 => Self::Suspend,
            3 => Self::Resume,
            _ => return None,
        })
    }
}

/// The game's state as the host reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum State {
    /// Quick Suspend is off on the host or for this game, or nothing runs
    /// that it could pause ([`HostState::reason`] says which).
    Unavailable = 0,
    /// The game runs and can be suspended.
    Ready = 1,
    /// The game's processes are suspended.
    Suspended = 2,
    /// A resume is under way.
    Resuming = 3,
    /// The request was refused; [`HostState::reason`] says why and the game's
    /// state is unchanged.
    Refused = 4,
}

impl State {
    pub fn from_u8(value: u8) -> Option<Self> {
        Some(match value {
            0 => Self::Unavailable,
            1 => Self::Ready,
            2 => Self::Suspended,
            3 => Self::Resuming,
            4 => Self::Refused,
            _ => return None,
        })
    }
}

/// Why the host refused or cannot suspend.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Reason {
    None = 0,
    /// Quick Suspend is off for the host or this game.
    DisabledForApp = 1,
    /// The app has no process the host launched and tracks (Desktop, a
    /// detached command), or only a launcher runs.
    NoOwnedProcess = 2,
    /// An anti-cheat runs with the game.
    AntiCheatDetected = 3,
    /// Windows protects a process of the game.
    ProtectedProcess = 4,
    /// A system process, the host itself or a launcher would be paused.
    DenylistedProcess = 5,
    /// Guests watch or play.
    GuestsActive = 6,
    /// A multi-instance co-op session runs.
    CoopSession = 7,
    /// The device may not control this session.
    NotOwner = 8,
    /// A launch or teardown is under way.
    SessionBusy = 9,
    /// No game runs.
    NoSession = 10,
    /// Windows failed; what was suspended was resumed.
    IoOrOsError = 11,
}

impl Reason {
    /// A reason this build does not know reads as [`Reason::IoOrOsError`]: a
    /// refusal still shows as one.
    pub fn from_u8(value: u8) -> Self {
        match value {
            0 => Self::None,
            1 => Self::DisabledForApp,
            2 => Self::NoOwnedProcess,
            3 => Self::AntiCheatDetected,
            4 => Self::ProtectedProcess,
            5 => Self::DenylistedProcess,
            6 => Self::GuestsActive,
            7 => Self::CoopSession,
            8 => Self::NotOwner,
            9 => Self::SessionBusy,
            10 => Self::NoSession,
            _ => Self::IoOrOsError,
        }
    }

    /// The stable name the Native API and the console use for it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::DisabledForApp => "disabled_for_app",
            Self::NoOwnedProcess => "no_owned_process",
            Self::AntiCheatDetected => "anti_cheat_detected",
            Self::ProtectedProcess => "protected_process",
            Self::DenylistedProcess => "denylisted_process",
            Self::GuestsActive => "guests_active",
            Self::CoopSession => "coop_session",
            Self::NotOwner => "not_owner",
            Self::SessionBusy => "session_busy",
            Self::NoSession => "no_session",
            Self::IoOrOsError => "io_or_os_error",
        }
    }
}

/// A client's request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClientRequest {
    /// Reserved; a 7.0 host ignores it and a client sends 0.
    pub flags: u16,
    /// A nonzero id the host echoes in its answer.
    pub request_id: u32,
    pub op: Op,
}

/// The host's state report.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HostState {
    pub flags: u16,
    /// The request this answers, or 0 for a change no request caused.
    pub request_id: u32,
    pub state: State,
    pub reason: Reason,
    /// How many processes are suspended (0 unless [`State::Suspended`]).
    pub suspended_count: u16,
    /// When the suspend started, Unix ms (0 unless [`State::Suspended`]).
    pub suspended_since_unix_ms: u64,
}

/// A decoded frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Message {
    Request(ClientRequest),
    State(HostState),
}

fn header(out: &mut [u8], kind: Kind, flags: u16, request_id: u32) {
    out[0] = VERSION;
    out[1] = kind as u8;
    out[2..4].copy_from_slice(&flags.to_le_bytes());
    out[4..8].copy_from_slice(&request_id.to_le_bytes());
}

impl ClientRequest {
    pub fn encode(&self) -> [u8; REQUEST_BYTES] {
        let mut out = [0u8; REQUEST_BYTES];
        header(&mut out, Kind::ClientRequest, self.flags, self.request_id);
        out[8] = self.op as u8;
        out
    }
}

impl HostState {
    pub fn encode(&self) -> [u8; STATE_BYTES] {
        let mut out = [0u8; STATE_BYTES];
        header(&mut out, Kind::HostState, self.flags, self.request_id);
        out[8] = self.state as u8;
        out[9] = self.reason as u8;
        out[10..12].copy_from_slice(&self.suspended_count.to_le_bytes());
        out[12..20].copy_from_slice(&self.suspended_since_unix_ms.to_le_bytes());
        out
    }
}

impl Message {
    /// Decodes a v1 frame, ignoring trailing bytes; None for a short frame,
    /// another version, an unknown kind, op or state.
    pub fn decode(data: &[u8]) -> Option<Self> {
        if data.len() < HEADER_BYTES || data[0] != VERSION {
            return None;
        }
        let flags = u16::from_le_bytes([data[2], data[3]]);
        let request_id = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
        match data[1] {
            1 if data.len() >= REQUEST_BYTES => Some(Self::Request(ClientRequest {
                flags,
                request_id,
                op: Op::from_u8(data[8])?,
            })),
            2 if data.len() >= STATE_BYTES => {
                let mut since = [0u8; 8];
                since.copy_from_slice(&data[12..20]);
                Some(Self::State(HostState {
                    flags,
                    request_id,
                    state: State::from_u8(data[8])?,
                    reason: Reason::from_u8(data[9]),
                    suspended_count: u16::from_le_bytes([data[10], data[11]]),
                    suspended_since_unix_ms: u64::from_le_bytes(since),
                }))
            }
            _ => None,
        }
    }

    /// The frame's bytes.
    pub fn encode(&self) -> ([u8; STATE_BYTES], usize) {
        let mut out = [0u8; STATE_BYTES];
        match self {
            Self::Request(request) => {
                out[..REQUEST_BYTES].copy_from_slice(&request.encode());
                (out, REQUEST_BYTES)
            }
            Self::State(state) => (state.encode(), STATE_BYTES),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> HostState {
        HostState {
            flags: 0,
            request_id: 7,
            state: State::Suspended,
            reason: Reason::None,
            suspended_count: 3,
            suspended_since_unix_ms: 1_763_772_200_000,
        }
    }

    #[test]
    fn requests_and_states_round_trip_with_trailing_bytes() {
        let request = ClientRequest {
            flags: 0,
            request_id: 42,
            op: Op::Suspend,
        };
        let wire = request.encode();
        assert_eq!(wire, [1, 1, 0, 0, 42, 0, 0, 0, 2]);
        assert_eq!(Message::decode(&wire), Some(Message::Request(request)));
        let wire = state().encode();
        assert_eq!(&wire[..12], &[1, 2, 0, 0, 7, 0, 0, 0, 2, 0, 3, 0]);
        let mut extended = wire.to_vec();
        extended.extend_from_slice(&[0xaa, 0xbb]);
        assert_eq!(Message::decode(&extended), Some(Message::State(state())));
        let (bytes, len) = Message::State(state()).encode();
        assert_eq!(&bytes[..len], &wire);
    }

    #[test]
    fn short_unknown_or_newer_frames_are_rejected() {
        let wire = state().encode();
        assert_eq!(Message::decode(&wire[..STATE_BYTES - 1]), None);
        let mut newer = wire;
        newer[0] = VERSION + 1;
        assert_eq!(Message::decode(&newer), None);
        let mut kind = wire;
        kind[1] = 3;
        assert_eq!(Message::decode(&kind), None);
        let mut op = [1, 1, 0, 0, 1, 0, 0, 0, 9];
        assert_eq!(Message::decode(&op), None);
        op[8] = 1;
        assert!(Message::decode(&op).is_some());
    }

    #[test]
    fn an_unknown_reason_still_reads_as_a_refusal() {
        let mut wire = state().encode();
        wire[8] = State::Refused as u8;
        wire[9] = 200;
        let Some(Message::State(state)) = Message::decode(&wire) else {
            panic!("a state");
        };
        assert_eq!(state.reason, Reason::IoOrOsError);
        assert_eq!(Reason::AntiCheatDetected.as_str(), "anti_cheat_detected");
        for value in 0..=11 {
            assert_eq!(Reason::from_u8(value) as u8, value);
        }
    }
}
