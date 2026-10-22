//! Per-picture host timing for clients that ask for it (control message 0x5536), measured
//! as punktfunk measures its host time: from the picture's present stamp to its last packet
//! handed to the socket, with the queue, encode and send stages beside it.
//!
//! Moonlight's frame header carries one host latency, written before the first packet, so it
//! cannot hold the send. A client that sends a [`HostTimingRequest`] gets the whole span
//! after each picture is out instead. The stream threads [`HostTimings::record`] each sent
//! picture; the control loop sends the pending ones as reports.
use crate::performance::Stamps;
pub use rubylight_protocol::control::{
    HOST_TIMING_EXTENDED_MAX_RECORDS, HostTiming, HostTimingExtra, HostTimingRequest,
};
use std::collections::VecDeque;
use std::time::Instant;

pub use rubylight_protocol::control::HOST_TIMING_MESSAGE_TYPE;

#[derive(Default)]
pub struct HostTimings {
    enabled: bool,
    pending: VecDeque<(HostTiming, HostTimingExtra)>,
}

impl HostTimings {
    /// Records kept while the control loop is behind: about a second at 240 fps. The oldest
    /// go first; the client misses those samples, nothing else.
    const LIMIT: usize = 256;

    /// Applies a client's request. Returns whether reports are on, or `None` for a
    /// malformed request, which changes nothing.
    pub fn on_request(&mut self, payload: &[u8]) -> Option<bool> {
        let request = HostTimingRequest::decode(payload)?;
        self.enabled = request.enabled;
        if !self.enabled {
            self.pending.clear();
        }
        Some(self.enabled)
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Notes a new picture whose last packet was just handed to the socket as video frame
    /// `frame`; `epoch` is the media clock's start. Nothing is kept unless the client asked.
    pub fn record(&mut self, frame: u32, stamps: &Stamps, epoch: Instant) {
        if !self.enabled {
            return;
        }
        let Some(timing) = timing(frame, stamps, epoch) else {
            return;
        };
        if self.pending.len() == Self::LIMIT {
            self.pending.pop_front();
        }
        self.pending.push_back(timing);
    }

    /// The next report of up to [`HOST_TIMING_EXTENDED_MAX_RECORDS`] pending records, oldest
    /// first, with the absolute and split stage times a client can place on the media clock.
    pub fn take_report(&mut self) -> Option<Vec<u8>> {
        if self.pending.is_empty() {
            return None;
        }
        let count = self.pending.len().min(HOST_TIMING_EXTENDED_MAX_RECORDS);
        let records: Vec<_> = self.pending.drain(..count).collect();
        Some(HostTiming::encode_report_with(&records))
    }
}

/// One picture's record: present to sent, and its queue (capture, hold, claim and convert),
/// encode and send stages. With them, the capture present on the media clock (`epoch` is its
/// start) and the capture and convert stages split out of the queue. `None` past the u32
/// microseconds the wire holds (71 minutes).
pub fn timing(
    frame: u32,
    stamps: &Stamps,
    epoch: Instant,
) -> Option<(HostTiming, HostTimingExtra)> {
    let [
        capture,
        hold,
        claim,
        convert,
        encode,
        _queue,
        _packetize,
        _send_queue,
        send,
    ] = stamps.stages();
    let host_us = u32::try_from(stamps.total()).ok()?;
    let queue_us = u32::try_from(capture + hold + claim + convert).ok()?;
    let timing = HostTiming {
        frame,
        host_us,
        queue_us,
        encode_us: u32::try_from(encode).ok()?,
        send_us: u32::try_from(send).ok()?,
    };
    let extra = HostTimingExtra {
        presented_at_us: u64::try_from(
            stamps
                .presented
                .saturating_duration_since(epoch)
                .as_micros(),
        )
        .ok()?,
        capture_us: u32::try_from(capture).ok()?,
        convert_us: u32::try_from(convert).ok()?,
    };
    Some((timing, extra))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn stamps(at: Instant) -> Stamps {
        let ms = |tenths: u64| at + Duration::from_micros(tenths * 100);
        Stamps {
            presented: at,
            acquired: ms(2),
            published: ms(3),
            claimed: ms(4),
            submitted: ms(6),
            encoded: ms(31),
            packetized: ms(32),
            prepared: ms(33),
            first_sent: ms(33),
            last_sent: ms(35),
        }
    }

    /// A media clock that started 12.345 ms before `at`.
    fn epoch(at: Instant) -> Instant {
        at.checked_sub(Duration::from_micros(12_345)).unwrap()
    }

    #[test]
    fn a_record_splits_present_to_sent_like_punktfunk() {
        let at = Instant::now();
        let (record, _) = timing(41, &stamps(at), epoch(at)).unwrap();
        assert_eq!(
            record,
            HostTiming {
                frame: 41,
                host_us: 3_500,
                queue_us: 600,
                encode_us: 2_500,
                send_us: 200,
            }
        );
        // The sender's wait and packetizing are what the named stages leave.
        assert_eq!(record.residual_us(), 200);
        let report = HostTiming::encode_report(&[record]);
        assert_eq!(HostTiming::decode_report(&report), Some(vec![record]));
    }

    #[test]
    fn the_extra_holds_the_present_on_the_media_clock_and_the_capture_and_convert_stages() {
        let at = Instant::now();
        let (record, extra) = timing(41, &stamps(at), epoch(at)).unwrap();
        assert_eq!(
            extra,
            HostTimingExtra {
                presented_at_us: 12_345,
                // Present to acquired, and claimed to submitted.
                capture_us: 200,
                convert_us: 200,
            }
        );
        assert!(extra.fits(&record));
        // A present before the media clock started counts as its start.
        let early = at.checked_sub(Duration::from_secs(1)).unwrap();
        let (_, extra) = timing(1, &stamps(at), early + Duration::from_secs(2)).unwrap();
        assert_eq!(extra.presented_at_us, 0);
    }

    #[test]
    fn a_present_stamp_after_its_acquire_counts_from_the_acquire() {
        let at = Instant::now();
        let mut late = stamps(at);
        late.presented = at + Duration::from_millis(7);
        let (record, extra) = timing(1, &late, epoch(at)).unwrap();
        assert_eq!(record.host_us, late.total() as u32);
        assert!(record.queue_us + record.encode_us + record.send_us <= record.host_us);
        assert_eq!(extra.capture_us, 0, "no negative stage");
        assert!(extra.fits(&record));
    }

    #[test]
    fn records_are_kept_only_for_a_client_that_asked() {
        let mut timings = HostTimings::default();
        let at = Instant::now();
        timings.record(1, &stamps(at), epoch(at));
        assert_eq!(timings.take_report(), None);
        assert_eq!(timings.on_request(&[1, 1]), None, "short request");
        assert!(!timings.enabled());
        assert_eq!(
            timings.on_request(&HostTimingRequest { enabled: true }.encode()),
            Some(true)
        );
        for frame in 0..40 {
            timings.record(frame, &stamps(at), epoch(at));
        }
        let first = HostTiming::decode_report(&timings.take_report().unwrap()).unwrap();
        assert_eq!(first.len(), HOST_TIMING_EXTENDED_MAX_RECORDS);
        assert_eq!(first[0].frame, 0);
        let rest = HostTiming::decode_report(&timings.take_report().unwrap()).unwrap();
        assert_eq!(
            rest.iter().map(|r| r.frame).collect::<Vec<_>>(),
            (24..40).collect::<Vec<_>>()
        );
        assert_eq!(timings.take_report(), None);
        timings.record(50, &stamps(at), epoch(at));
        assert_eq!(
            timings.on_request(&HostTimingRequest { enabled: false }.encode()),
            Some(false)
        );
        assert_eq!(
            timings.take_report(),
            None,
            "turning reports off drops what waits"
        );
    }

    #[test]
    fn a_report_of_24_extended_records_decodes_in_the_old_decoder() {
        let mut timings = HostTimings::default();
        timings.on_request(&HostTimingRequest { enabled: true }.encode());
        let at = Instant::now();
        for frame in 0..HOST_TIMING_EXTENDED_MAX_RECORDS as u32 {
            timings.record(frame, &stamps(at), epoch(at));
        }
        let report = timings.take_report().unwrap();
        assert_eq!(report.len(), 868, "one ENet packet");
        // `decode_report` is the v1 decoder an older client runs.
        let old = HostTiming::decode_report(&report).unwrap();
        assert_eq!(old.len(), HOST_TIMING_EXTENDED_MAX_RECORDS);
        let new = HostTiming::decode_report_with(&report).unwrap();
        assert!(new.iter().all(|(_, extra)| extra.is_some()));
        assert_eq!(
            old,
            new.iter().map(|(record, _)| *record).collect::<Vec<_>>()
        );
        assert_eq!(new[0].1.unwrap().presented_at_us, 12_345);
    }

    #[test]
    fn a_stalled_control_loop_keeps_the_newest_records() {
        let mut timings = HostTimings::default();
        timings.on_request(&HostTimingRequest { enabled: true }.encode());
        let at = Instant::now();
        for frame in 0..(HostTimings::LIMIT as u32 + 10) {
            timings.record(frame, &stamps(at), epoch(at));
        }
        let first = HostTiming::decode_report(&timings.take_report().unwrap()).unwrap();
        assert_eq!(first[0].frame, 10);
    }
}
