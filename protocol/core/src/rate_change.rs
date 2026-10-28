//! Mid-stream bitrate and frame-rate changes (control message 0x553C), for a client that
//! launched with `rlRateChange=1`.
//!
//! The client sends a [`RateChangeRequest`]; the host answers each with a
//! [`RateChangeResult`] carrying the same serial and the stream's bitrate and rate after it.
//! A bitrate goes to the encoder on its next frame, without a keyframe; a rate is applied
//! between frames like a 0x5532 reconfigure, so its result follows once the encoder runs at
//! it, or did not. The host clamps either to what the device and the encoder allow, and says
//! so per field.
//!
//! So results can come out of request order: one for a rate carries the bitrate the host set
//! when the request came, and a bitrate-only request sent after it is answered first. A client
//! takes a result's bitrate only when no result for a later serial came before it.
//!
//! Request, little-endian: version u8, reserved u8, reserved u16, serial u32, bitrate_kbps
//! u32, fps_millihz u32; 0 leaves that value unchanged. Result: version u8, reserved u8 x3,
//! serial u32, bitrate_kbps u32, fps_millihz u32, then a status and a reason byte for the
//! bitrate and for the rate.

/// Control-stream message type of both directions.
pub const RATE_CHANGE_MESSAGE_TYPE: u16 = 0x553C;
/// Encoded size of a [`RateChangeRequest`].
pub const RATE_CHANGE_REQUEST_BYTES: usize = 16;
/// Encoded size of a [`RateChangeResult`].
pub const RATE_CHANGE_RESULT_BYTES: usize = 20;
/// Launch parameter a client sends with `1` to take rate changes; the host then lists 0x553C
/// in DESCRIBE's `a=x-rl-control:` line.
pub const LAUNCH_PARAMETER: &str = "rlRateChange";
/// The lowest and highest rate a client may ask for, as for 0x5532.
pub const MIN_FPS_MILLIHZ: u32 = 10_000;
pub const MAX_FPS_MILLIHZ: u32 = 500_000;

const VERSION: u8 = 1;

/// Client to host: a new bitrate, rate, or both; 0 keeps the current value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RateChangeRequest {
    /// Echoed in the result, so a client can match it to its request.
    pub serial: u32,
    pub bitrate_kbps: u32,
    pub fps_millihz: u32,
}

impl RateChangeRequest {
    pub fn encode(&self) -> [u8; RATE_CHANGE_REQUEST_BYTES] {
        let mut out = [0; RATE_CHANGE_REQUEST_BYTES];
        out[0] = VERSION;
        out[4..8].copy_from_slice(&self.serial.to_le_bytes());
        out[8..12].copy_from_slice(&self.bitrate_kbps.to_le_bytes());
        out[12..16].copy_from_slice(&self.fps_millihz.to_le_bytes());
        out
    }

    /// Parses a request; version 0 and a short payload reject it. The values are checked by
    /// [`Self::valid`], so the host can answer an invalid one with its serial.
    pub fn decode(data: &[u8]) -> Option<Self> {
        if data.len() < RATE_CHANGE_REQUEST_BYTES || data[0] == 0 {
            return None;
        }
        Some(Self {
            serial: u32_at(data, 4),
            bitrate_kbps: u32_at(data, 8),
            fps_millihz: u32_at(data, 12),
        })
    }

    /// Asks for something, and a rate it asks for is within
    /// [`MIN_FPS_MILLIHZ`]-[`MAX_FPS_MILLIHZ`].
    pub fn valid(&self) -> bool {
        (self.bitrate_kbps != 0 || self.fps_millihz != 0)
            && (self.fps_millihz == 0
                || (MIN_FPS_MILLIHZ..=MAX_FPS_MILLIHZ).contains(&self.fps_millihz))
    }
}

/// What became of one requested value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// Not asked for: the value stays.
    Unchanged = 0,
    /// The stream runs at the asked value.
    Applied = 1,
    /// The stream runs at a lower value than asked; the reason says what limits it.
    Clamped = 2,
    /// The stream keeps the value it had.
    Refused = 3,
}

impl Status {
    fn from_wire(value: u8) -> Option<Self> {
        Some(match value {
            0 => Self::Unchanged,
            1 => Self::Applied,
            2 => Self::Clamped,
            3 => Self::Refused,
            _ => return None,
        })
    }
}

/// Why a value was clamped or refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    None = 0,
    /// Both values 0, or a rate outside the protocol's limits.
    Invalid = 1,
    /// The host does not change this stream's rate (`stream_reconfigure` is off).
    Disabled = 2,
    /// The stream's codec or geometry is fixed at connection (PyroWave, packed full chroma).
    Codec = 3,
    /// The device's or host's limit (`max_bitrate`, the 500 Mbps cap).
    HostLimit = 4,
    /// The encoder's measured throughput at the stream's size.
    EncoderLimit = 5,
    /// The encoder could not run at the rate; the stream went back to the previous one.
    EncoderFailed = 6,
    /// A later request, or a 0x5532 reconfigure, replaced this one before it applied.
    Superseded = 7,
    /// Another reason, such as the host failing to load the device's configuration, or one
    /// this version does not know.
    Other = 255,
}

impl Reason {
    fn from_wire(value: u8) -> Self {
        match value {
            0 => Self::None,
            1 => Self::Invalid,
            2 => Self::Disabled,
            3 => Self::Codec,
            4 => Self::HostLimit,
            5 => Self::EncoderLimit,
            6 => Self::EncoderFailed,
            7 => Self::Superseded,
            _ => Self::Other,
        }
    }
}

/// The status and reason of one value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Outcome {
    pub status: Status,
    pub reason: Reason,
}

impl Outcome {
    pub const UNCHANGED: Self = Self {
        status: Status::Unchanged,
        reason: Reason::None,
    };
    pub const APPLIED: Self = Self {
        status: Status::Applied,
        reason: Reason::None,
    };
    pub const fn clamped(reason: Reason) -> Self {
        Self {
            status: Status::Clamped,
            reason,
        }
    }
    pub const fn refused(reason: Reason) -> Self {
        Self {
            status: Status::Refused,
            reason,
        }
    }
}

/// Host to client: what became of the request with `serial`, and the stream's bitrate and
/// rate after it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RateChangeResult {
    pub serial: u32,
    pub bitrate_kbps: u32,
    pub fps_millihz: u32,
    pub bitrate: Outcome,
    pub fps: Outcome,
}

impl RateChangeResult {
    pub fn encode(&self) -> [u8; RATE_CHANGE_RESULT_BYTES] {
        let mut out = [0; RATE_CHANGE_RESULT_BYTES];
        out[0] = VERSION;
        out[4..8].copy_from_slice(&self.serial.to_le_bytes());
        out[8..12].copy_from_slice(&self.bitrate_kbps.to_le_bytes());
        out[12..16].copy_from_slice(&self.fps_millihz.to_le_bytes());
        out[16] = self.bitrate.status as u8;
        out[17] = self.bitrate.reason as u8;
        out[18] = self.fps.status as u8;
        out[19] = self.fps.reason as u8;
        out
    }

    /// Parses a result; version 0, a short payload and an unknown status reject it. An
    /// unknown reason is [`Reason::Other`]. Bytes past the 20 are ignored.
    pub fn decode(data: &[u8]) -> Option<Self> {
        if data.len() < RATE_CHANGE_RESULT_BYTES || data[0] == 0 {
            return None;
        }
        let outcome = |at: usize| {
            Some(Outcome {
                status: Status::from_wire(data[at])?,
                reason: Reason::from_wire(data[at + 1]),
            })
        };
        Some(Self {
            serial: u32_at(data, 4),
            bitrate_kbps: u32_at(data, 8),
            fps_millihz: u32_at(data, 12),
            bitrate: outcome(16)?,
            fps: outcome(18)?,
        })
    }
}

fn u32_at(data: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]])
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    #[test]
    fn requests_round_trip_and_check_their_values() {
        let request = RateChangeRequest {
            serial: 7,
            bitrate_kbps: 40_000,
            fps_millihz: 120_000,
        };
        let wire = request.encode();
        assert_eq!(
            wire,
            [1, 0, 0, 0, 7, 0, 0, 0, 0x40, 0x9C, 0, 0, 0xC0, 0xD4, 0x01, 0]
        );
        assert_eq!(RateChangeRequest::decode(&wire), Some(request));
        assert!(request.valid());
        let mut longer = wire.to_vec();
        longer[0] = 2;
        longer.extend_from_slice(&[9; 4]);
        assert_eq!(RateChangeRequest::decode(&longer), Some(request));
        assert_eq!(RateChangeRequest::decode(&wire[..15]), None);
        let mut zero = wire;
        zero[0] = 0;
        assert_eq!(RateChangeRequest::decode(&zero), None);

        for (bitrate_kbps, fps_millihz, valid) in [
            (40_000, 0, true),
            (0, MIN_FPS_MILLIHZ, true),
            (0, MAX_FPS_MILLIHZ, true),
            (0, 0, false),
            (40_000, MIN_FPS_MILLIHZ - 1, false),
            (0, MAX_FPS_MILLIHZ + 1, false),
        ] {
            let request = RateChangeRequest {
                serial: 1,
                bitrate_kbps,
                fps_millihz,
            };
            assert_eq!(
                RateChangeRequest::decode(&request.encode()).map(|r| r.valid()),
                Some(valid),
                "{bitrate_kbps} {fps_millihz}"
            );
        }
    }

    #[test]
    fn results_round_trip_with_both_outcomes() {
        let result = RateChangeResult {
            serial: 0xA0B0_C0D0,
            bitrate_kbps: 150_000,
            fps_millihz: 60_000,
            bitrate: Outcome::clamped(Reason::HostLimit),
            fps: Outcome::refused(Reason::Codec),
        };
        let wire = result.encode();
        assert_eq!(wire[..8], [1, 0, 0, 0, 0xD0, 0xC0, 0xB0, 0xA0]);
        assert_eq!(wire[16..], [2, 4, 3, 3]);
        assert_eq!(RateChangeResult::decode(&wire), Some(result));
        let mut longer: Vec<u8> = wire.to_vec();
        longer[0] = 3;
        longer.push(1);
        assert_eq!(RateChangeResult::decode(&longer), Some(result));
        assert_eq!(RateChangeResult::decode(&wire[..19]), None);
        let mut zero = wire;
        zero[0] = 0;
        assert_eq!(RateChangeResult::decode(&zero), None);
        let mut status = wire;
        status[18] = 4;
        assert_eq!(RateChangeResult::decode(&status), None);
        let mut reason = wire;
        reason[19] = 99;
        assert_eq!(
            RateChangeResult::decode(&reason).map(|r| r.fps),
            Some(Outcome::refused(Reason::Other))
        );
        for outcome in [
            Outcome::UNCHANGED,
            Outcome::APPLIED,
            Outcome::clamped(Reason::EncoderLimit),
            Outcome::refused(Reason::Invalid),
            Outcome::refused(Reason::Disabled),
            Outcome::refused(Reason::EncoderFailed),
            Outcome::refused(Reason::Superseded),
        ] {
            let result = RateChangeResult {
                bitrate: outcome,
                fps: outcome,
                ..result
            };
            assert_eq!(RateChangeResult::decode(&result.encode()), Some(result));
        }
    }
}
