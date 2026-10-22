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
mod tests {
    use super::*;

    #[test]
    fn display_caps_round_trip_and_reject_nonsense() {
        // A Galaxy Z Fold 7 inner screen: 2600 nits peak, 0.0005 nits black.
        let caps = DisplayCaps {
            hdr: true,
            max_centinits: 260_000,
            max_average_centinits: 120_000,
            min_decimillinits: 5,
        };
        let bytes = caps.encode();
        assert_eq!(DisplayCaps::decode(&bytes), Some(caps));
        assert_eq!(caps.max_nits(), Some(2600));
        assert_eq!(caps.max_average_nits(), Some(1200));
        let mut longer = bytes.to_vec();
        longer.extend_from_slice(&[0xff; 4]);
        assert_eq!(
            DisplayCaps::decode(&longer),
            Some(caps),
            "newer clients may append fields"
        );
        assert_eq!(DisplayCaps::decode(&bytes[..DISPLAY_CAPS_BYTES - 1]), None);
        let mut version = bytes;
        version[0] = 2;
        assert_eq!(DisplayCaps::decode(&version), None);
        let unknown = DisplayCaps {
            hdr: false,
            ..DisplayCaps::default()
        };
        assert_eq!(DisplayCaps::decode(&unknown.encode()), Some(unknown));
        assert_eq!(unknown.max_nits(), None);
        for wild in [
            DisplayCaps {
                max_centinits: 5_000_000,
                ..caps
            },
            DisplayCaps {
                max_average_centinits: 300_000,
                ..caps
            },
            DisplayCaps {
                min_decimillinits: 20_000,
                ..caps
            },
        ] {
            assert_eq!(DisplayCaps::decode(&wild.encode()), None, "{wild:?}");
        }
    }

    #[test]
    fn reconfigure_round_trips_and_rejects_nonsense() {
        let request = Reconfigure {
            width: 2176,
            height: 1812,
            fps_millihz: 120_000,
        };
        let bytes = request.encode();
        assert_eq!(Reconfigure::decode(&bytes), Some(request));
        assert_eq!(Reconfigure::decode(&bytes[..RECONFIGURE_BYTES - 1]), None);
        for wild in [
            Reconfigure {
                width: 2177,
                ..request
            },
            Reconfigure {
                height: 100,
                ..request
            },
            Reconfigure {
                width: 10_000,
                ..request
            },
            Reconfigure {
                fps_millihz: 1_000,
                ..request
            },
            Reconfigure {
                fps_millihz: 1_000_000,
                ..request
            },
        ] {
            assert_eq!(Reconfigure::decode(&wild.encode()), None, "{wild:?}");
        }
    }

    #[test]
    fn clock_sync_round_trips_and_rejects_nonsense() {
        let request = ClockSyncRequest {
            sequence: 7,
            client_us: 0x0102_0304_0506_0708,
        };
        let bytes = request.encode();
        assert_eq!(&bytes[..8], &[1, 0, 0, 0, 7, 0, 0, 0]);
        assert_eq!(ClockSyncRequest::decode(&bytes), Some(request));
        assert_eq!(ClockSyncRequest::decode(&bytes[..15]), None);
        let mut version = bytes;
        version[0] = 2;
        assert_eq!(ClockSyncRequest::decode(&version), None);

        let reply = ClockSyncReply {
            sequence: 7,
            client_us: request.client_us,
            host_receive_us: 5_000_000,
            host_send_us: 5_000_040,
        };
        let bytes = reply.encode();
        assert_eq!(ClockSyncReply::decode(&bytes), Some(reply));
        let mut longer = bytes.to_vec();
        longer.extend_from_slice(&[0xff; 8]);
        assert_eq!(ClockSyncReply::decode(&longer), Some(reply), "newer hosts may append");
        assert_eq!(ClockSyncReply::decode(&bytes[..31]), None);
        let backwards = ClockSyncReply {
            host_send_us: 4_999_999,
            ..reply
        };
        assert_eq!(ClockSyncReply::decode(&backwards.encode()), None);
    }

    #[test]
    fn host_timing_request_round_trips() {
        for enabled in [true, false] {
            let request = HostTimingRequest { enabled };
            let bytes = request.encode();
            assert_eq!(bytes, [1, u8::from(enabled), 0, 0]);
            assert_eq!(HostTimingRequest::decode(&bytes), Some(request));
        }
        assert_eq!(HostTimingRequest::decode(&[1, 1, 0]), None);
        assert_eq!(HostTimingRequest::decode(&[2, 1, 0, 0]), None);
    }

    #[test]
    fn host_timing_reports_round_trip_and_reject_nonsense() {
        let records: Vec<HostTiming> = (0..3)
            .map(|i| HostTiming {
                frame: 1000 + i,
                host_us: 3_200,
                queue_us: 400,
                encode_us: 2_500,
                send_us: 120,
            })
            .collect();
        let bytes = HostTiming::encode_report(&records);
        assert_eq!(
            bytes.len(),
            HOST_TIMING_HEADER_BYTES + 3 * HOST_TIMING_RECORD_BYTES
        );
        assert_eq!(&bytes[..8], &[1, 3, 0, 0, 0xe8, 0x03, 0, 0]);
        assert_eq!(HostTiming::decode_report(&bytes), Some(records.clone()));
        assert_eq!(records[0].residual_us(), 180);
        let mut longer = bytes.clone();
        longer.extend_from_slice(&[0xff; 7]);
        assert_eq!(
            HostTiming::decode_report(&longer),
            Some(records.clone()),
            "newer hosts may append bytes"
        );
        assert_eq!(HostTiming::decode_report(&bytes[..bytes.len() - 1]), None);
        let mut version = bytes.clone();
        version[0] = 2;
        assert_eq!(HostTiming::decode_report(&version), None);
        assert_eq!(HostTiming::decode_report(&[1, 0, 0, 0]), None);
        let mut too_many = bytes.clone();
        too_many[1] = (HOST_TIMING_MAX_RECORDS + 1) as u8;
        assert_eq!(HostTiming::decode_report(&too_many), None);
        let over = HostTiming {
            send_us: 900,
            ..records[0]
        };
        assert_eq!(
            HostTiming::decode_report(&HostTiming::encode_report(&[over])),
            None
        );
        let many = alloc::vec![records[0]; HOST_TIMING_MAX_RECORDS + 5];
        let capped = HostTiming::encode_report(&many);
        assert_eq!(usize::from(capped[1]), HOST_TIMING_MAX_RECORDS);
        assert_eq!(
            HostTiming::decode_report(&capped).map(|r| r.len()),
            Some(HOST_TIMING_MAX_RECORDS)
        );
    }

    fn extended_sample(count: u32) -> Vec<(HostTiming, HostTimingExtra)> {
        (0..count)
            .map(|i| {
                (
                    HostTiming {
                        frame: 1000 + i,
                        host_us: 3_200,
                        queue_us: 400,
                        encode_us: 2_500,
                        send_us: 120,
                    },
                    HostTimingExtra {
                        presented_at_us: 5_000_000_000 + u64::from(i) * 16_667,
                        capture_us: 150,
                        convert_us: 200,
                    },
                )
            })
            .collect()
    }

    #[test]
    fn host_timing_extended_report_round_trips() {
        let records = extended_sample(3);
        let bytes = HostTiming::encode_report_with(&records);
        assert_eq!(
            bytes.len(),
            HOST_TIMING_HEADER_BYTES + 3 * (HOST_TIMING_RECORD_BYTES + HOST_TIMING_EXTRA_BYTES)
        );
        assert_eq!(&bytes[..4], &[1, 3, 16, 0]);
        let decoded = HostTiming::decode_report_with(&bytes).unwrap();
        let expected: Vec<_> = records.iter().map(|(r, x)| (*r, Some(*x))).collect();
        assert_eq!(decoded, expected);
    }

    #[test]
    fn host_timing_old_decoder_reads_extended_report() {
        let records = extended_sample(HOST_TIMING_EXTENDED_MAX_RECORDS as u32);
        let bytes = HostTiming::encode_report_with(&records);
        let base: Vec<HostTiming> = records.iter().map(|(r, _)| *r).collect();
        assert_eq!(HostTiming::decode_report(&bytes), Some(base.clone()));
        assert_eq!(
            HostTiming::decode_report(&HostTiming::encode_report(&base)),
            Some(base)
        );
    }

    #[test]
    fn host_timing_report_without_extension_has_no_extras() {
        let records = extended_sample(2);
        let base: Vec<HostTiming> = records.iter().map(|(r, _)| *r).collect();
        let decoded = HostTiming::decode_report_with(&HostTiming::encode_report(&base)).unwrap();
        assert_eq!(decoded.len(), 2);
        assert!(decoded.iter().all(|(_, x)| x.is_none()));
        assert_eq!(decoded[1].0, base[1]);
    }

    #[test]
    fn host_timing_extension_over_queue_is_dropped_and_base_kept() {
        let mut records = extended_sample(2);
        records[1].1.convert_us = 251;
        let bytes = HostTiming::encode_report_with(&records);
        let decoded = HostTiming::decode_report_with(&bytes).unwrap();
        assert_eq!(decoded[0], (records[0].0, Some(records[0].1)));
        assert_eq!(decoded[1], (records[1].0, None));
        records[1].1.convert_us = 250;
        let ok = HostTiming::decode_report_with(&HostTiming::encode_report_with(&records));
        assert_eq!(ok.unwrap()[1].1, Some(records[1].1));
    }

    #[test]
    fn host_timing_extended_report_is_capped_at_616_bytes() {
        let many = extended_sample(HOST_TIMING_EXTENDED_MAX_RECORDS as u32 + 8);
        let bytes = HostTiming::encode_report_with(&many);
        assert_eq!(bytes.len(), 616);
        // Under the 644 bytes of a full v1 report, which shipped clients still accept.
        assert!(
            bytes.len()
                <= HOST_TIMING_HEADER_BYTES + HOST_TIMING_MAX_RECORDS * HOST_TIMING_RECORD_BYTES
        );
        assert_eq!(usize::from(bytes[1]), HOST_TIMING_EXTENDED_MAX_RECORDS);
        let decoded = HostTiming::decode_report_with(&bytes).unwrap();
        assert_eq!(decoded.len(), HOST_TIMING_EXTENDED_MAX_RECORDS);
        assert!(decoded.iter().all(|(_, x)| x.is_some()));
        // More records than the extended limit: the base records are kept and any
        // appended extras are not trusted.
        let base: Vec<HostTiming> = many.iter().map(|(r, _)| *r).collect();
        let mut wide = HostTiming::encode_report(&base);
        wide[2] = 16;
        wide.extend_from_slice(&alloc::vec![0; base.len() * 16]);
        let decoded = HostTiming::decode_report_with(&wide).unwrap();
        assert_eq!(decoded.len(), base.len());
        assert!(decoded.iter().all(|(_, x)| x.is_none()));
    }

    #[test]
    fn host_timing_extension_skips_bytes_past_the_known_fields() {
        let records = extended_sample(2);
        let bytes = HostTiming::encode_report_with(&records);
        let base_end = HOST_TIMING_HEADER_BYTES + 2 * HOST_TIMING_RECORD_BYTES;
        // Entries of 24 bytes: the 16 known bytes, then 8 newer ones.
        let mut wider = bytes[..base_end].to_vec();
        wider[2] = 24;
        for i in 0..2 {
            let at = base_end + i * 16;
            wider.extend_from_slice(&bytes[at..at + 16]);
            wider.extend_from_slice(&[0xee; 8]);
        }
        let decoded = HostTiming::decode_report_with(&wider).unwrap();
        for (i, (r, x)) in decoded.iter().enumerate() {
            assert_eq!(*r, records[i].0);
            assert_eq!(*x, Some(records[i].1));
        }
        // Declared extra bytes missing: base kept, no extras.
        let decoded = HostTiming::decode_report_with(&bytes[..bytes.len() - 1]).unwrap();
        assert!(decoded.iter().all(|(_, x)| x.is_none()));
        // Trailing bytes after the entries are ignored.
        let mut longer = bytes.clone();
        longer.extend_from_slice(&[0xff; 5]);
        assert_eq!(
            HostTiming::decode_report_with(&longer),
            HostTiming::decode_report_with(&bytes)
        );
    }
}
