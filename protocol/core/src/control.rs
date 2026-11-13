//! Client-to-host control messages beyond the ones Moonlight defines.
//!
//! Each travels on the encrypted control stream, like the phase lock report
//! ([`crate::phase_lock::REPORT_MESSAGE_TYPE`]). A client sends them only to a host that
//! announced support (Rubylight 2.2.0 or later), and a host that does not know an id
//! ignores it, so old hosts and stock clients are unaffected. Every message starts with a
//! version byte; a newer client may append fields, which an older host ignores.

/// Control-stream message type of [`DisplayCaps`] (client to host).
pub const DISPLAY_CAPS_MESSAGE_TYPE: u16 = 0x5531;
/// Encoded size of [`DisplayCaps`].
pub const DISPLAY_CAPS_BYTES: usize = 16;
/// Control-stream message type of [`Reconfigure`] (client to host).
pub const RECONFIGURE_MESSAGE_TYPE: u16 = 0x5532;
/// Encoded size of [`Reconfigure`].
pub const RECONFIGURE_BYTES: usize = 12;
/// Control-stream message type of host timing, both ways: a client sends a
/// [`HostTimingRequest`], and the host answers with [`HostTiming`] reports.
pub const HOST_TIMING_MESSAGE_TYPE: u16 = 0x5536;
/// Encoded size of [`HostTimingRequest`].
pub const HOST_TIMING_REQUEST_BYTES: usize = 4;
/// Size of a host timing report's header, before its records.
pub const HOST_TIMING_HEADER_BYTES: usize = 4;
/// Encoded size of one [`HostTiming`] record.
pub const HOST_TIMING_RECORD_BYTES: usize = 20;
/// Most records one report carries (a 644-byte message).
pub const HOST_TIMING_MAX_RECORDS: usize = 32;
/// Bytes per record in a report's trailing [`HostTimingExtra`] block (header byte 2).
pub const HOST_TIMING_EXTRA_BYTES: usize = 16;
/// Most records an extended report carries: a 616-byte message. Nightjar's patched
/// moonlight-common-c hands a 0x5536 message to Java only up to 644 bytes (a v1 report of 32
/// records), which 17 extended records (4 + 17 x 36) stay under and 18 do not.
pub const HOST_TIMING_EXTENDED_MAX_RECORDS: usize = 17;
/// Control-stream message type of clock sync, both ways: a client sends a
/// [`ClockSyncRequest`], and the host answers each with a [`ClockSyncReply`].
pub const CLOCK_SYNC_MESSAGE_TYPE: u16 = 0x5537;
/// Encoded size of [`ClockSyncRequest`].
pub const CLOCK_SYNC_REQUEST_BYTES: usize = 16;
/// Encoded size of [`ClockSyncReply`].
pub const CLOCK_SYNC_REPLY_BYTES: usize = 32;
/// Control-stream message type of link notices, both ways: a client sends a
/// [`LinkNoticeRequest`], and the host then sends a [`LinkNotice`] when it changes the video
/// bitrate to follow the network.
pub const LINK_NOTICE_MESSAGE_TYPE: u16 = 0x5538;
/// Encoded size of [`LinkNoticeRequest`].
pub const LINK_NOTICE_REQUEST_BYTES: usize = 4;
/// Encoded size of [`LinkNotice`].
pub const LINK_NOTICE_BYTES: usize = 12;

use alloc::vec::Vec;

const VERSION: u8 = 1;

fn u16_at(data: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([data[at], data[at + 1]])
}

fn u32_at(data: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]])
}

fn u64_at(data: &[u8], at: usize) -> u64 {
    let mut word = [0; 8];
    word.copy_from_slice(&data[at..at + 8]);
    u64::from_le_bytes(word)
}

/// A client's clock sync probe (client to host). The host answers it at once with a
/// [`ClockSyncReply`] when its video clock runs; before the first frame it does not answer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ClockSyncRequest {
    /// Chosen by the client, echoed in the reply.
    pub sequence: u32,
    /// When the client sent it, in microseconds of the client's own clock.
    pub client_us: u64,
}

impl ClockSyncRequest {
    /// Little-endian wire form: version, reserved, sequence, client time.
    pub fn encode(&self) -> [u8; CLOCK_SYNC_REQUEST_BYTES] {
        let mut out = [0; CLOCK_SYNC_REQUEST_BYTES];
        out[0] = VERSION;
        out[4..8].copy_from_slice(&self.sequence.to_le_bytes());
        out[8..16].copy_from_slice(&self.client_us.to_le_bytes());
        out
    }

    pub fn decode(data: &[u8]) -> Option<Self> {
        if data.len() < CLOCK_SYNC_REQUEST_BYTES || data[0] != VERSION {
            return None;
        }
        Some(Self {
            sequence: u32_at(data, 4),
            client_us: u64_at(data, 8),
        })
    }
}

/// The host's answer to a [`ClockSyncRequest`] (host to client).
///
/// Host times are microseconds on the video stream's RTP clock: a frame stamped at
/// `host_us` carries the RTP timestamp `host_us * 90 / 1000` (modulo 2^32). The client
/// maps frame timestamps to its own clock with these, NTP style (see
/// [`crate::present_clock::ClockOffset`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ClockSyncReply {
    pub sequence: u32,
    /// The request's client time, unchanged.
    pub client_us: u64,
    /// When the host received the request.
    pub host_receive_us: u64,
    /// When the host sent this reply.
    pub host_send_us: u64,
}

impl ClockSyncReply {
    /// Little-endian wire form: version, reserved, sequence, client, receive, send.
    pub fn encode(&self) -> [u8; CLOCK_SYNC_REPLY_BYTES] {
        let mut out = [0; CLOCK_SYNC_REPLY_BYTES];
        out[0] = VERSION;
        out[4..8].copy_from_slice(&self.sequence.to_le_bytes());
        out[8..16].copy_from_slice(&self.client_us.to_le_bytes());
        out[16..24].copy_from_slice(&self.host_receive_us.to_le_bytes());
        out[24..32].copy_from_slice(&self.host_send_us.to_le_bytes());
        out
    }

    /// Parses a reply; a send time before the receive time rejects it.
    pub fn decode(data: &[u8]) -> Option<Self> {
        if data.len() < CLOCK_SYNC_REPLY_BYTES || data[0] != VERSION {
            return None;
        }
        let reply = Self {
            sequence: u32_at(data, 4),
            client_us: u64_at(data, 8),
            host_receive_us: u64_at(data, 16),
            host_send_us: u64_at(data, 24),
        };
        (reply.host_send_us >= reply.host_receive_us).then_some(reply)
    }
}

/// The client display's HDR luminance, sent at stream start and when the display changes
/// (for example a foldable switching screens), so the host tone-maps for the real panel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DisplayCaps {
    /// The display can show HDR (HDR10 or HLG) right now.
    pub hdr: bool,
    /// Peak luminance, in hundredths of a nit (1000 nits = 100_000).
    pub max_centinits: u32,
    /// Maximum frame-average luminance, in hundredths of a nit.
    pub max_average_centinits: u32,
    /// Minimum luminance, in ten-thousandths of a nit (0.0005 nits = 5).
    pub min_decimillinits: u32,
}

impl DisplayCaps {
    const HDR_FLAG: u8 = 1;

    /// Little-endian wire form: version, flags, reserved, max, max average, min.
    pub fn encode(&self) -> [u8; DISPLAY_CAPS_BYTES] {
        let mut out = [0; DISPLAY_CAPS_BYTES];
        out[0] = VERSION;
        out[1] = if self.hdr { Self::HDR_FLAG } else { 0 };
        out[4..8].copy_from_slice(&self.max_centinits.to_le_bytes());
        out[8..12].copy_from_slice(&self.max_average_centinits.to_le_bytes());
        out[12..16].copy_from_slice(&self.min_decimillinits.to_le_bytes());
        out
    }

    /// Parses the message; unknown versions and implausible values are rejected.
    pub fn decode(data: &[u8]) -> Option<Self> {
        if data.len() < DISPLAY_CAPS_BYTES || data[0] != VERSION {
            return None;
        }
        let caps = Self {
            hdr: data[1] & Self::HDR_FLAG != 0,
            max_centinits: u32_at(data, 4),
            max_average_centinits: u32_at(data, 8),
            min_decimillinits: u32_at(data, 12),
        };
        // Phones report 0 when they do not know a value; otherwise 1 to 10 000 nits peak,
        // an average no higher than the peak, and a black level below 1 nit.
        let known = |value: u32| value != 0;
        let plausible = (!known(caps.max_centinits)
            || (100..=1_000_000).contains(&caps.max_centinits))
            && (!known(caps.max_average_centinits)
                || !known(caps.max_centinits)
                || caps.max_average_centinits <= caps.max_centinits)
            && caps.min_decimillinits < 10_000;
        plausible.then_some(caps)
    }

    /// Peak luminance in whole nits, or `None` when the client did not know it.
    pub fn max_nits(&self) -> Option<u32> {
        (self.max_centinits != 0).then(|| self.max_centinits.div_ceil(100))
    }

    /// Maximum frame-average luminance in whole nits, or `None` when unknown.
    pub fn max_average_nits(&self) -> Option<u32> {
        (self.max_average_centinits != 0).then(|| self.max_average_centinits.div_ceil(100))
    }
}

/// A request to change the stream's resolution or frame rate without reconnecting, for
/// example when a foldable opens or the window is resized. The host answers with a
/// keyframe at the new size; until then the client keeps decoding the old one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Reconfigure {
    pub width: u16,
    pub height: u16,
    /// Frame rate in thousandths of a frame per second (120 fps = 120_000).
    pub fps_millihz: u32,
}

impl Reconfigure {
    /// Little-endian wire form: version, reserved, width, height, reserved, fps.
    pub fn encode(&self) -> [u8; RECONFIGURE_BYTES] {
        let mut out = [0; RECONFIGURE_BYTES];
        out[0] = VERSION;
        out[2..4].copy_from_slice(&self.width.to_le_bytes());
        out[4..6].copy_from_slice(&self.height.to_le_bytes());
        out[8..12].copy_from_slice(&self.fps_millihz.to_le_bytes());
        out
    }

    /// Parses the message; unknown versions, odd or tiny sizes and rates outside
    /// 10-500 fps are rejected.
    pub fn decode(data: &[u8]) -> Option<Self> {
        if data.len() < RECONFIGURE_BYTES || data[0] != VERSION {
            return None;
        }
        let request = Self {
            width: u16_at(data, 2),
            height: u16_at(data, 4),
            fps_millihz: u32_at(data, 8),
        };
        let plausible = (256..=8192).contains(&request.width)
            && (256..=8192).contains(&request.height)
            && request.width % 2 == 0
            && request.height % 2 == 0
            && (10_000..=500_000).contains(&request.fps_millihz);
        plausible.then_some(request)
    }
}

/// A client's request for [`HostTiming`] reports (client to host), sent once the stream
/// starts. A host that does not know the id ignores it and the client keeps Moonlight's
/// per-frame host latency.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HostTimingRequest {
    pub enabled: bool,
}

impl HostTimingRequest {
    const ENABLED_FLAG: u8 = 1;

    /// Little-endian wire form: version, flags, reserved.
    pub fn encode(&self) -> [u8; HOST_TIMING_REQUEST_BYTES] {
        let mut out = [0; HOST_TIMING_REQUEST_BYTES];
        out[0] = VERSION;
        out[1] = if self.enabled { Self::ENABLED_FLAG } else { 0 };
        out
    }

    pub fn decode(data: &[u8]) -> Option<Self> {
        if data.len() < HOST_TIMING_REQUEST_BYTES || data[0] != VERSION {
            return None;
        }
        Some(Self {
            enabled: data[1] & Self::ENABLED_FLAG != 0,
        })
    }
}

/// A client's request for [`LinkNotice`]s (client to host), sent once the stream starts. A
/// host that does not know the id ignores it, and a host never sends a notice to a client that
/// did not ask, so stock Moonlight clients never see the message.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LinkNoticeRequest {
    pub enabled: bool,
}

impl LinkNoticeRequest {
    const ENABLED_FLAG: u8 = 1;

    /// Little-endian wire form: version, flags, reserved.
    pub fn encode(&self) -> [u8; LINK_NOTICE_REQUEST_BYTES] {
        let mut out = [0; LINK_NOTICE_REQUEST_BYTES];
        out[0] = VERSION;
        out[1] = if self.enabled { Self::ENABLED_FLAG } else { 0 };
        out
    }

    pub fn decode(data: &[u8]) -> Option<Self> {
        if data.len() < LINK_NOTICE_REQUEST_BYTES || data[0] != VERSION {
            return None;
        }
        Some(Self {
            enabled: data[1] & Self::ENABLED_FLAG != 0,
        })
    }
}

/// Where the host's video bitrate stands against the one the client asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkState {
    /// At the requested bitrate. The client clears its notice.
    Full,
    /// Below it, and probing back up as the link allows.
    Limited,
    /// At the lowest bitrate the host uses, and the network still loses video.
    Floor,
}

impl LinkState {
    fn wire(self) -> u8 {
        match self {
            Self::Full => 0,
            Self::Limited => 1,
            Self::Floor => 2,
        }
    }

    fn from_wire(byte: u8) -> Option<Self> {
        match byte {
            0 => Some(Self::Full),
            1 => Some(Self::Limited),
            2 => Some(Self::Floor),
            _ => None,
        }
    }
}

/// Why the host holds the bitrate down.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkReason {
    /// Nothing: the bitrate is at the requested one.
    None,
    /// The network between the host and this device lost video at the requested bitrate.
    Network,
}

impl LinkReason {
    fn wire(self) -> u8 {
        match self {
            Self::None => 0,
            Self::Network => 1,
        }
    }

    fn from_wire(byte: u8) -> Option<Self> {
        match byte {
            0 => Some(Self::None),
            1 => Some(Self::Network),
            _ => None,
        }
    }
}

/// The host lowered, raised or restored the video bitrate to follow the network (host to
/// client). Sent only to a client that sent a [`LinkNoticeRequest`], on each change and at
/// most once a second; the newest wins when changes come faster.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LinkNotice {
    pub state: LinkState,
    pub reason: LinkReason,
    /// The video bitrate the client asked for, as the encoder is given it, in kbps.
    pub ceiling_kbps: u32,
    /// The encoder bitrate now, in kbps.
    pub current_kbps: u32,
}

impl LinkNotice {
    /// Whether the host holds the bitrate below the requested one.
    pub fn limited(&self) -> bool {
        self.state != LinkState::Full
    }

    /// Little-endian wire form: version, state, reason, reserved, ceiling, current.
    pub fn encode(&self) -> [u8; LINK_NOTICE_BYTES] {
        let mut out = [0; LINK_NOTICE_BYTES];
        out[0] = VERSION;
        out[1] = self.state.wire();
        out[2] = self.reason.wire();
        out[4..8].copy_from_slice(&self.ceiling_kbps.to_le_bytes());
        out[8..12].copy_from_slice(&self.current_kbps.to_le_bytes());
        out
    }

    /// Parses a notice. An unknown version, state or reason, a zero bitrate, a limited state
    /// without a reason or a full one with a reason rejects it; bytes past the 12 are ignored.
    pub fn decode(data: &[u8]) -> Option<Self> {
        if data.len() < LINK_NOTICE_BYTES || data[0] != VERSION {
            return None;
        }
        let notice = Self {
            state: LinkState::from_wire(data[1])?,
            reason: LinkReason::from_wire(data[2])?,
            ceiling_kbps: u32_at(data, 4),
            current_kbps: u32_at(data, 8),
        };
        let consistent = notice.ceiling_kbps != 0
            && notice.current_kbps != 0
            && (notice.state == LinkState::Full) == (notice.reason == LinkReason::None);
        consistent.then_some(notice)
    }
}

/// Absolute and split stage times for one [`HostTiming`] record, appended after the v1
/// records of a report. They let a client place every host stage on the host media clock:
/// capture present is `presented_at_us`, acquired is that plus `capture_us`, submit is
/// `presented_at_us + queue_us`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HostTimingExtra {
    /// The capture present stamp, in microseconds on the host's media clock (the clock of
    /// [`ClockSyncReply`] and of the RTP timestamps).
    pub presented_at_us: u64,
    /// Capture present to the picture acquired.
    pub capture_us: u32,
    /// The stream thread's claim to the encoder taking the picture (the conversion).
    pub convert_us: u32,
}

impl HostTimingExtra {
    /// Whether the two stages fit inside the record's `queue_us`.
    pub fn fits(&self, timing: &HostTiming) -> bool {
        u64::from(self.capture_us) + u64::from(self.convert_us) <= u64::from(timing.queue_us)
    }
}

/// Where one new picture's time went on the host, sent after its last packet left the
/// socket, as punktfunk's host timing is. All in microseconds from the same anchor: the
/// picture's present stamp (the display driver's acquire on a Rubylight display), or the
/// stream thread's claim of a picture without one.
///
/// `host_us = queue_us + encode_us + residual + send_us`; the residual (waiting for the
/// sender and packetizing) is what the named stages leave.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HostTiming {
    /// The frame number in the video stream's packets (Moonlight's frame index).
    pub frame: u32,
    /// Present to the frame's last packet handed to the socket.
    pub host_us: u32,
    /// Present to the encoder taking the picture: capture, the wait for the claim and
    /// the conversion.
    pub queue_us: u32,
    /// The encoder taking the picture to the encoded frame in the host's hands.
    pub encode_us: u32,
    /// The first packet to the last one handed to the socket.
    pub send_us: u32,
}

impl HostTiming {
    /// What `host_us` holds besides the named stages.
    pub fn residual_us(&self) -> u32 {
        self.host_us
            .saturating_sub(self.queue_us)
            .saturating_sub(self.encode_us)
            .saturating_sub(self.send_us)
    }

    /// One report of up to [`HOST_TIMING_MAX_RECORDS`] records; the rest are left out.
    /// Little-endian: version, count, reserved, then each record's five words.
    pub fn encode_report(records: &[Self]) -> Vec<u8> {
        let records = &records[..records.len().min(HOST_TIMING_MAX_RECORDS)];
        let mut out =
            Vec::with_capacity(HOST_TIMING_HEADER_BYTES + records.len() * HOST_TIMING_RECORD_BYTES);
        out.extend_from_slice(&[VERSION, records.len() as u8, 0, 0]);
        for r in records {
            for word in [r.frame, r.host_us, r.queue_us, r.encode_us, r.send_us] {
                out.extend_from_slice(&word.to_le_bytes());
            }
        }
        out
    }

    /// An extended report: the v1 records, then one [`HostTimingExtra`] per record, with
    /// header byte 2 set to [`HOST_TIMING_EXTRA_BYTES`]. At most
    /// [`HOST_TIMING_EXTENDED_MAX_RECORDS`] records, so it stays one ENet packet. Old
    /// decoders read the base records and ignore the rest.
    pub fn encode_report_with(records: &[(Self, HostTimingExtra)]) -> Vec<u8> {
        let records = &records[..records.len().min(HOST_TIMING_EXTENDED_MAX_RECORDS)];
        let mut out = Vec::with_capacity(
            HOST_TIMING_HEADER_BYTES
                + records.len() * (HOST_TIMING_RECORD_BYTES + HOST_TIMING_EXTRA_BYTES),
        );
        out.extend_from_slice(&[
            VERSION,
            records.len() as u8,
            HOST_TIMING_EXTRA_BYTES as u8,
            0,
        ]);
        for (r, _) in records {
            for word in [r.frame, r.host_us, r.queue_us, r.encode_us, r.send_us] {
                out.extend_from_slice(&word.to_le_bytes());
            }
        }
        for (_, x) in records {
            out.extend_from_slice(&x.presented_at_us.to_le_bytes());
            out.extend_from_slice(&x.capture_us.to_le_bytes());
            out.extend_from_slice(&x.convert_us.to_le_bytes());
        }
        out
    }

    /// Parses a report with its extension. The base records are validated as in
    /// [`Self::decode_report`]. A record's extra is `None` when the report has none, when it
    /// is cut short or over the extended record limit, or when `capture_us + convert_us`
    /// exceeds that record's `queue_us`; the base record is kept either way. Bytes of an
    /// entry past [`HOST_TIMING_EXTRA_BYTES`] are skipped.
    pub fn decode_report_with(data: &[u8]) -> Option<Vec<(Self, Option<HostTimingExtra>)>> {
        let records = Self::decode_report(data)?;
        let count = records.len();
        let extra = usize::from(data[2]);
        let base_end = HOST_TIMING_HEADER_BYTES + count * HOST_TIMING_RECORD_BYTES;
        let usable = extra >= HOST_TIMING_EXTRA_BYTES
            && count <= HOST_TIMING_EXTENDED_MAX_RECORDS
            && data.len() >= base_end + count * extra;
        Some(
            records
                .into_iter()
                .enumerate()
                .map(|(i, r)| {
                    let x = usable
                        .then(|| {
                            let at = base_end + i * extra;
                            HostTimingExtra {
                                presented_at_us: u64_at(data, at),
                                capture_us: u32_at(data, at + 8),
                                convert_us: u32_at(data, at + 12),
                            }
                        })
                        .filter(|x| x.fits(&r));
                    (r, x)
                })
                .collect(),
        )
    }

    /// Parses a report. An unknown version, no records, a count over the limit or past the
    /// data, or a record whose stages add up to more than its total rejects it whole.
    pub fn decode_report(data: &[u8]) -> Option<Vec<Self>> {
        if data.len() < HOST_TIMING_HEADER_BYTES || data[0] != VERSION {
            return None;
        }
        let count = usize::from(data[1]);
        if count == 0
            || count > HOST_TIMING_MAX_RECORDS
            || data.len() < HOST_TIMING_HEADER_BYTES + count * HOST_TIMING_RECORD_BYTES
        {
            return None;
        }
        let records: Vec<Self> = (0..count)
            .map(|i| {
                let at = HOST_TIMING_HEADER_BYTES + i * HOST_TIMING_RECORD_BYTES;
                Self {
                    frame: u32_at(data, at),
                    host_us: u32_at(data, at + 4),
                    queue_us: u32_at(data, at + 8),
                    encode_us: u32_at(data, at + 12),
                    send_us: u32_at(data, at + 16),
                }
            })
            .collect();
        records
            .iter()
            .all(|r| {
                u64::from(r.queue_us) + u64::from(r.encode_us) + u64::from(r.send_us)
                    <= u64::from(r.host_us)
            })
            .then_some(records)
    }
}

#[cfg(test)]
mod tests;
