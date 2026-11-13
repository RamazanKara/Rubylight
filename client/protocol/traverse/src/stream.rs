//! The reliable stream layer that carries the host's TCP channels (HTTP, HTTPS, RTSP) over a
//! punched UDP path, without IO. Small on purpose: these channels carry requests, replies and
//! the app list, not media.
//!
//! One sequence space carries every stream in order; each segment names its stream. A stream
//! opens with [`FLAG_OPEN`] and its target (which of the host's ports), ends with [`FLAG_FIN`]
//! or [`FLAG_RESET`]. The receiver acknowledges every segment with its next expected sequence
//! number, a bitmap of the 64 after it that it holds, and its free window. The sender
//! retransmits on a timeout from the measured round trip (RFC 6298), or after three
//! acknowledgements that skip a segment, and gives the path up after [`MAX_RETRIES`].
//!
//! | Segment | Layout |
//! |---|---|
//! | data | `01`, seq u32, stream u32, flags u8, target u8, payload |
//! | ack | `02`, next expected u32, received bitmap u64, window u16 |

use crate::Millis;
use std::collections::{BTreeMap, VecDeque};

pub type StreamId = u32;

/// The most payload a segment carries: it stays under 1200 bytes on the wire, which every
/// internet path passes unfragmented.
pub const MAX_PAYLOAD: usize = 1100;
/// The most segments in flight, and the most the receiver holds out of order.
pub const WINDOW: u32 = 256;
/// The most payload bytes a sender buffers before `write` takes less.
pub const SEND_BUFFER: usize = 512 * 1024;
pub const MAX_RETRIES: u32 = 10;
const INITIAL_RTO: Millis = 1_000;
const MIN_RTO: Millis = 200;
const MAX_RTO: Millis = 4_000;

const DATA: u8 = 1;
const ACK: u8 = 2;
const DATA_HEADER: usize = 11;
pub const FLAG_OPEN: u8 = 1;
pub const FLAG_FIN: u8 = 2;
pub const FLAG_RESET: u8 = 4;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// The peer opened a stream to `target`.
    Opened {
        stream: StreamId,
        target: u8,
    },
    Data {
        stream: StreamId,
        data: Vec<u8>,
    },
    /// The peer sends no more on this stream.
    Finished {
        stream: StreamId,
    },
    /// The peer abandoned the stream.
    Reset {
        stream: StreamId,
    },
    /// The peer stopped acknowledging: the path is gone.
    Dead,
}

struct Sent {
    bytes: Vec<u8>,
    payload: usize,
    first_sent: Millis,
    sent_at: Millis,
    retries: u32,
    sacked: bool,
    force: bool,
}

struct Segment {
    stream: StreamId,
    flags: u8,
    target: u8,
    payload: Vec<u8>,
}

pub struct Reliable {
    next_stream: StreamId,
    next_seq: u32,
    unsent: VecDeque<(u32, Vec<u8>, usize)>,
    in_flight: BTreeMap<u32, Sent>,
    buffered: usize,
    peer_window: u32,
    recv_next: u32,
    recv: BTreeMap<u32, Segment>,
    events: VecDeque<Event>,
    ack_due: bool,
    window_closed: bool,
    srtt: Option<Millis>,
    rttvar: Millis,
    rto: Millis,
    last_cum: u32,
    repeats: u32,
    dead: bool,
}

impl Reliable {
    /// `initiator`: the side that opens streams (the device) numbers them odd, the other even.
    pub fn new(initiator: bool) -> Self {
        Self {
            next_stream: if initiator { 1 } else { 2 },
            next_seq: 0,
            unsent: VecDeque::new(),
            in_flight: BTreeMap::new(),
            buffered: 0,
            peer_window: WINDOW,
            recv_next: 0,
            recv: BTreeMap::new(),
            events: VecDeque::new(),
            ack_due: false,
            window_closed: false,
            srtt: None,
            rttvar: 0,
            rto: INITIAL_RTO,
            last_cum: 0,
            repeats: 0,
            dead: false,
        }
    }

    fn queue(&mut self, stream: StreamId, flags: u8, target: u8, payload: &[u8]) {
        let seq = self.next_seq;
        self.next_seq = self.next_seq.wrapping_add(1);
        let mut bytes = Vec::with_capacity(DATA_HEADER + payload.len());
        bytes.push(DATA);
        bytes.extend_from_slice(&seq.to_be_bytes());
        bytes.extend_from_slice(&stream.to_be_bytes());
        bytes.push(flags);
        bytes.push(target);
        bytes.extend_from_slice(payload);
        self.buffered += payload.len();
        self.unsent.push_back((seq, bytes, payload.len()));
    }

    /// Opens a stream to the peer's `target`.
    pub fn open(&mut self, target: u8) -> StreamId {
        let stream = self.next_stream;
        self.next_stream = self.next_stream.wrapping_add(2);
        self.queue(stream, FLAG_OPEN, target, &[]);
        stream
    }

    /// Queues as much of `data` as the send buffer takes, and returns how much that was.
    pub fn write(&mut self, stream: StreamId, data: &[u8]) -> usize {
        if self.dead {
            return 0;
        }
        let taken = data.len().min(self.capacity());
        for chunk in data[..taken].chunks(MAX_PAYLOAD) {
            self.queue(stream, 0, 0, chunk);
        }
        taken
    }

    pub fn finish(&mut self, stream: StreamId) {
        if !self.dead {
            self.queue(stream, FLAG_FIN, 0, &[]);
        }
    }

    pub fn reset(&mut self, stream: StreamId) {
        if !self.dead {
            self.queue(stream, FLAG_RESET, 0, &[]);
        }
    }

    /// How many more bytes `write` takes now.
    pub fn capacity(&self) -> usize {
        SEND_BUFFER.saturating_sub(self.buffered)
    }

    pub fn is_dead(&self) -> bool {
        self.dead
    }

    /// Nothing waits to be sent or acknowledged.
    pub fn is_idle(&self) -> bool {
        self.unsent.is_empty() && self.in_flight.is_empty()
    }

    fn backoff(&self, retries: u32) -> Millis {
        (self.rto << retries.min(5)).min(MAX_RTO)
    }

    /// When `poll_transmit` next has something without new input.
    pub fn next_timeout(&self) -> Option<Millis> {
        if self.dead {
            return None;
        }
        self.in_flight
            .values()
            .filter(|sent| !sent.sacked)
            .map(|sent| {
                if sent.force {
                    0
                } else {
                    sent.sent_at + self.backoff(sent.retries)
                }
            })
            .min()
    }

    /// The next segment to send, if any: an acknowledgement, a retransmission, then new data.
    pub fn poll_transmit(&mut self, now: Millis) -> Option<Vec<u8>> {
        if self.dead {
            return None;
        }
        if self.ack_due {
            self.ack_due = false;
            return Some(self.ack());
        }
        let rto = self.rto;
        let mut died = false;
        let mut resend = None;
        for sent in self.in_flight.values_mut() {
            let due = sent.sent_at + (rto << sent.retries.min(5)).min(MAX_RTO);
            if sent.sacked || !(sent.force || now >= due) {
                continue;
            }
            if sent.retries >= MAX_RETRIES {
                died = true;
                break;
            }
            sent.retries += 1;
            sent.sent_at = now;
            sent.force = false;
            resend = Some(sent.bytes.clone());
            break;
        }
        if died {
            self.dead = true;
            self.events.push_back(Event::Dead);
            return None;
        }
        if resend.is_some() {
            return resend;
        }
        // A closed window still lets one segment through, so a lost window update can't stall.
        if (self.in_flight.len() as u32) < self.peer_window.clamp(1, WINDOW) {
            if let Some((seq, bytes, payload)) = self.unsent.pop_front() {
                let copy = bytes.clone();
                self.in_flight.insert(
                    seq,
                    Sent {
                        bytes,
                        payload,
                        first_sent: now,
                        sent_at: now,
                        retries: 0,
                        sacked: false,
                        force: false,
                    },
                );
                return Some(copy);
            }
        }
        None
    }

    fn window(&self) -> u32 {
        WINDOW.saturating_sub(self.recv.len() as u32 + (self.events.len() as u32).min(WINDOW))
    }

    fn ack(&mut self) -> Vec<u8> {
        let mut sack = 0u64;
        for (&seq, _) in self.recv.range(self.recv_next.wrapping_add(1)..) {
            let offset = seq.wrapping_sub(self.recv_next).wrapping_sub(1);
            if offset < 64 {
                sack |= 1 << offset;
            }
        }
        let window = self.window();
        self.window_closed = window == 0;
        let mut out = Vec::with_capacity(15);
        out.push(ACK);
        out.extend_from_slice(&self.recv_next.to_be_bytes());
        out.extend_from_slice(&sack.to_be_bytes());
        out.extend_from_slice(&(window.min(u16::MAX as u32) as u16).to_be_bytes());
        out
    }

    /// Takes a segment from the peer.
    pub fn handle(&mut self, segment: &[u8], now: Millis) {
        if self.dead || segment.is_empty() {
            return;
        }
        match segment[0] {
            DATA if segment.len() >= DATA_HEADER => {
                let seq = u32::from_be_bytes(segment[1..5].try_into().unwrap());
                self.ack_due = true;
                let ahead = seq.wrapping_sub(self.recv_next);
                if ahead >= WINDOW {
                    // Already delivered (a retransmission whose ack was lost), or too far ahead.
                    return;
                }
                self.recv.entry(seq).or_insert_with(|| Segment {
                    stream: u32::from_be_bytes(segment[5..9].try_into().unwrap()),
                    flags: segment[9],
                    target: segment[10],
                    payload: segment[DATA_HEADER..].to_vec(),
                });
                while let Some(segment) = self.recv.remove(&self.recv_next) {
                    self.recv_next = self.recv_next.wrapping_add(1);
                    self.deliver(segment);
                }
            }
            ACK if segment.len() >= 15 => {
                let cum = u32::from_be_bytes(segment[1..5].try_into().unwrap());
                let sack = u64::from_be_bytes(segment[5..13].try_into().unwrap());
                self.peer_window = u16::from_be_bytes([segment[13], segment[14]]) as u32;
                self.on_ack(cum, sack, now);
            }
            _ => {}
        }
    }

    fn on_ack(&mut self, cum: u32, sack: u64, now: Millis) {
        let acked: Vec<u32> = self
            .in_flight
            .keys()
            .copied()
            .filter(|&seq| seq < cum)
            .collect();
        let progressed = !acked.is_empty();
        for seq in acked {
            if let Some(sent) = self.in_flight.remove(&seq) {
                self.buffered -= sent.payload;
                // Karn: only a segment sent once times the round trip.
                if sent.retries == 0 {
                    self.sample(now.saturating_sub(sent.first_sent));
                }
            }
        }
        for offset in 0..64 {
            if sack & (1 << offset) != 0 {
                if let Some(sent) = self.in_flight.get_mut(&cum.wrapping_add(1 + offset)) {
                    sent.sacked = true;
                }
            }
        }
        if progressed || cum != self.last_cum {
            self.last_cum = cum;
            self.repeats = 0;
        } else if sack != 0 {
            self.repeats += 1;
            if self.repeats == 3 {
                if let Some(sent) = self.in_flight.get_mut(&cum) {
                    sent.force = true;
                }
            }
        }
    }

    fn sample(&mut self, rtt: Millis) {
        match self.srtt {
            None => {
                self.srtt = Some(rtt);
                self.rttvar = rtt / 2;
            }
            Some(srtt) => {
                self.rttvar = (3 * self.rttvar + srtt.abs_diff(rtt)) / 4;
                self.srtt = Some((7 * srtt + rtt) / 8);
            }
        }
        self.rto = (self.srtt.unwrap() + (4 * self.rttvar).max(10)).clamp(MIN_RTO, MAX_RTO);
    }

    fn deliver(&mut self, segment: Segment) {
        let stream = segment.stream;
        if segment.flags & FLAG_OPEN != 0 {
            self.events.push_back(Event::Opened {
                stream,
                target: segment.target,
            });
        }
        if !segment.payload.is_empty() {
            self.events.push_back(Event::Data {
                stream,
                data: segment.payload,
            });
        }
        if segment.flags & FLAG_FIN != 0 {
            self.events.push_back(Event::Finished { stream });
        }
        if segment.flags & FLAG_RESET != 0 {
            self.events.push_back(Event::Reset { stream });
        }
    }

    pub fn poll_event(&mut self) -> Option<Event> {
        let event = self.events.pop_front();
        // The window reopens: tell the sender.
        if event.is_some() && self.window_closed && self.window() > 0 {
            self.ack_due = true;
        }
        event
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs two ends over a link that drops what `lose` says, delivering after `delay` ms.
    fn run(
        a: &mut Reliable,
        b: &mut Reliable,
        mut lose: impl FnMut(usize) -> bool,
        delay: Millis,
        until: Millis,
    ) -> (Vec<Event>, Vec<Event>) {
        let mut in_air: Vec<(Millis, bool, Vec<u8>)> = Vec::new();
        let (mut events_a, mut events_b) = (Vec::new(), Vec::new());
        let mut sent = 0;
        for now in 0..until {
            for (from_a, end) in [(true, &mut *a), (false, &mut *b)] {
                while let Some(segment) = end.poll_transmit(now) {
                    sent += 1;
                    if !lose(sent) {
                        in_air.push((now + delay, from_a, segment));
                    }
                }
            }
            let (due, later): (Vec<_>, Vec<_>) =
                in_air.into_iter().partition(|(at, _, _)| *at <= now);
            in_air = later;
            for (_, from_a, segment) in due {
                if from_a {
                    b.handle(&segment, now);
                } else {
                    a.handle(&segment, now);
                }
            }
            while let Some(event) = a.poll_event() {
                events_a.push(event);
            }
            while let Some(event) = b.poll_event() {
                events_b.push(event);
            }
        }
        (events_a, events_b)
    }

    fn received(events: &[Event], stream: StreamId) -> Vec<u8> {
        events
            .iter()
            .filter_map(|event| match event {
                Event::Data { stream: s, data } if *s == stream => Some(data.clone()),
                _ => None,
            })
            .flatten()
            .collect()
    }

    #[test]
    fn streams_open_carry_data_and_finish_in_order() {
        let (mut client, mut host) = (Reliable::new(true), Reliable::new(false));
        let stream = client.open(1);
        assert_eq!(stream, 1);
        assert_eq!(client.write(stream, b"GET /serverinfo"), 15);
        client.finish(stream);
        let (_, at_host) = run(&mut client, &mut host, |_| false, 5, 100);
        assert_eq!(
            at_host[0],
            Event::Opened {
                stream: 1,
                target: 1
            }
        );
        assert_eq!(received(&at_host, 1), b"GET /serverinfo");
        assert_eq!(at_host.last(), Some(&Event::Finished { stream: 1 }));
        assert!(client.is_idle());
    }

    #[test]
    fn a_lossy_link_delivers_everything_once_and_in_order() {
        let (mut client, mut host) = (Reliable::new(true), Reliable::new(false));
        let stream = client.open(2);
        let data: Vec<u8> = (0..200_000u32).map(|n| (n % 251) as u8).collect();
        let mut written = 0;
        let mut seed = 12345u32;
        // About one datagram in five lost, both ways.
        let lose = move |_| {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
            (seed >> 16) % 5 == 0
        };
        written += client.write(stream, &data[written..]);
        assert_eq!(written, data.len());
        client.finish(stream);
        let (_, at_host) = run(&mut client, &mut host, lose, 20, 60_000);
        assert_eq!(received(&at_host, stream), data);
        assert_eq!(
            at_host
                .iter()
                .filter(|event| matches!(event, Event::Finished { .. }))
                .count(),
            1
        );
        assert!(!client.is_dead());
        assert!(client.is_idle());
    }

    #[test]
    fn the_send_buffer_bounds_what_write_takes() {
        let mut client = Reliable::new(true);
        let stream = client.open(1);
        let big = vec![0u8; SEND_BUFFER + 10];
        assert_eq!(client.write(stream, &big), SEND_BUFFER);
        assert_eq!(client.capacity(), 0);
        assert_eq!(client.write(stream, b"x"), 0);
    }

    #[test]
    fn a_silent_peer_kills_the_path() {
        let mut client = Reliable::new(true);
        client.open(1);
        let mut events = Vec::new();
        for now in 0..120_000 {
            while client.poll_transmit(now).is_some() {}
            while let Some(event) = client.poll_event() {
                events.push(event);
            }
        }
        assert_eq!(events, vec![Event::Dead]);
        assert!(client.is_dead());
        assert_eq!(client.write(1, b"x"), 0);
    }

    #[test]
    fn several_streams_interleave() {
        let (mut client, mut host) = (Reliable::new(true), Reliable::new(false));
        let a = client.open(0);
        let b = client.open(2);
        client.write(a, b"one");
        client.write(b, b"two");
        client.reset(a);
        host.open(0);
        let (at_client, at_host) = run(&mut client, &mut host, |n| n % 3 == 0, 10, 10_000);
        assert_eq!(received(&at_host, a), b"one");
        assert_eq!(received(&at_host, b), b"two");
        assert!(at_host.contains(&Event::Reset { stream: a }));
        assert_eq!(
            at_client,
            vec![Event::Opened {
                stream: 2,
                target: 0
            }]
        );
    }
}
