//! The control stream as a socket-free state machine: messages and times in, sealed
//! datagrams and [`ControlEvent`]s out. [`super::driver`] runs it over ENet.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use rubylight_protocol::control::ClockSyncRequest;

use super::events::{self, ConnectionStatus, ControlEvent, HdrMetadata};
use super::messages::{ClientMessage, FrameFecStatus};
use super::negotiation::HostCapabilities;
use super::wire::{channel, ptype, Delivery, OpenError, Opener, Sealer};
use crate::crypto::Key;

/// How often the periodic ping goes out.
pub const PERIODIC_PING_INTERVAL: Duration = Duration::from_millis(100);
const REFERENCE_FRAME_QUEUE_BOUND: usize = 20;
const FEC_STATUS_QUEUE_BOUND: usize = 8;
const CONN_IMMEDIATE_POOR_LOSS_RATE: u32 = 30;
const CONN_CONSECUTIVE_POOR_LOSS_RATE: u32 = 15;
const CONN_OKAY_LOSS_RATE: u32 = 5;
const CONN_STATUS_SAMPLE_PERIOD: Duration = Duration::from_millis(3000);

/// A sealed datagram for ENet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outgoing {
    pub channel: u8,
    pub delivery: Delivery,
    pub datagram: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SendError {
    /// The host did not announce the message, is not a Sunshine host, or the session does
    /// not use the feature (RFI for LTR acknowledgements).
    Unsupported,
    /// The payload breaks the message's rules.
    Invalid,
    /// The stream has ended or is ending.
    Stopped,
}

impl std::fmt::Display for SendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Unsupported => "the host does not support this control message",
            Self::Invalid => "invalid control message",
            Self::Stopped => "the control stream has stopped",
        })
    }
}

impl std::error::Error for SendError {}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SessionOptions {
    /// Report lost frames with reference frame invalidation instead of IDR requests, and
    /// acknowledge long-term reference frames to Sunshine hosts.
    pub reference_frame_invalidation: bool,
}

#[derive(Clone, Copy, Debug)]
enum ReferenceFrameControl {
    Invalidate { first: u32, last: u32 },
    Acknowledge(u32),
}

/// The encrypted control stream of one connection.
pub struct ControlSession {
    caps: HostCapabilities,
    options: SessionOptions,
    sealer: Sealer,
    opener: Opener,
    started: bool,
    stopped: bool,
    terminated: Option<i32>,
    transmit: VecDeque<Outgoing>,
    events: Vec<ControlEvent>,

    idr_requested: bool,
    reference_frames: VecDeque<ReferenceFrameControl>,
    fec_statuses: VecDeque<FrameFecStatus>,
    next_ping: Option<Instant>,

    last_good_frame: u32,
    last_seen_frame: u32,
    first_frame_at: Option<Instant>,
    interval_start: Option<Instant>,
    interval_good_frames: u32,
    interval_total_frames: u32,
    last_interval_loss: u32,
    connection_status: ConnectionStatus,

    hdr_enabled: bool,
    hdr_metadata: HdrMetadata,
    cursor_shapes: bool,
    clock_sync_sequence: u32,
}

impl ControlSession {
    /// A session for a host on the encrypted control stream (GFE 3.22 and every Sunshine
    /// host). Older GFE generations are not supported.
    pub fn new(caps: HostCapabilities, key: &Key, options: SessionOptions) -> Self {
        let v2 = caps.control_v2();
        Self {
            caps,
            options,
            sealer: Sealer::new(key, v2),
            opener: Opener::new(key, v2),
            started: false,
            stopped: false,
            terminated: None,
            transmit: VecDeque::new(),
            events: Vec::new(),
            idr_requested: false,
            reference_frames: VecDeque::new(),
            fec_statuses: VecDeque::new(),
            next_ping: None,
            last_good_frame: 0,
            last_seen_frame: 0,
            first_frame_at: None,
            interval_start: None,
            interval_good_frames: 0,
            interval_total_frames: 0,
            last_interval_loss: 0,
            connection_status: ConnectionStatus::Okay,
            hdr_enabled: false,
            hdr_metadata: HdrMetadata::default(),
            cursor_shapes: false,
            clock_sync_sequence: 0,
        }
    }

    pub fn capabilities(&self) -> &HostCapabilities {
        &self.caps
    }

    /// Queues Start A and Start B; the periodic ping starts with the next [`Self::poll`].
    pub fn start(&mut self, now: Instant) {
        if self.started {
            return;
        }
        self.started = true;
        self.queue(&ClientMessage::StartA);
        self.queue(&ClientMessage::StartB);
        self.next_ping = Some(now);
    }

    /// Sends no more messages, as teardown closes sending.
    pub fn stop(&mut self) {
        self.stopped = true;
    }

    pub fn is_stopped(&self) -> bool {
        self.stopped || self.terminated.is_some()
    }

    /// The termination code, once the stream has ended.
    pub fn terminated(&self) -> Option<i32> {
        self.terminated
    }

    /// Checks and queues a message.
    pub fn send(&mut self, message: ClientMessage) -> Result<(), SendError> {
        if self.is_stopped() {
            return Err(SendError::Stopped);
        }
        if !message.is_valid() {
            return Err(SendError::Invalid);
        }
        if message.sunshine_only() && !self.caps.sunshine_extensions() {
            return Err(SendError::Unsupported);
        }
        if let Some(id) = message.announcement() {
            if !self.caps.supports_control_message(id) {
                return Err(SendError::Unsupported);
            }
        }
        if matches!(message, ClientMessage::LtrFrameAck { .. })
            && !self.options.reference_frame_invalidation
        {
            return Err(SendError::Unsupported);
        }
        if let ClientMessage::CursorShapeRequest { enabled } = message {
            // Shapes are accepted from just before an enable goes out, so the host's first
            // answer is not dropped, and refused from just before a disable.
            self.cursor_shapes = enabled;
        }
        if !self.queue(&message) {
            if matches!(message, ClientMessage::CursorShapeRequest { .. }) {
                self.cursor_shapes = false;
            }
            return Err(SendError::Invalid);
        }
        Ok(())
    }

    /// Queues a clock sync probe stamped `client_us` and returns its sequence number.
    pub fn send_clock_sync_probe(&mut self, client_us: u64) -> Result<u32, SendError> {
        let sequence = self.clock_sync_sequence.wrapping_add(1);
        self.send(ClientMessage::ClockSync(ClockSyncRequest {
            sequence,
            client_us,
        }))?;
        self.clock_sync_sequence = sequence;
        Ok(sequence)
    }

    /// Queues an encoded input packet (see [`crate::input`]).
    pub fn send_input(
        &mut self,
        packet: &[u8],
        channel: u8,
        delivery: Delivery,
    ) -> Result<(), SendError> {
        if self.is_stopped() {
            return Err(SendError::Stopped);
        }
        if self.push(ptype::INPUT_DATA, packet, channel, delivery) {
            Ok(())
        } else {
            Err(SendError::Invalid)
        }
    }

    fn queue(&mut self, message: &ClientMessage) -> bool {
        self.push(
            message.message_type(),
            &message.payload(),
            message.channel(),
            message.delivery(),
        )
    }

    fn push(&mut self, message_type: u16, payload: &[u8], channel: u8, delivery: Delivery) -> bool {
        let Some(datagram) = self.sealer.seal(message_type, payload) else {
            return false;
        };
        // GFE takes only reliable messages, and only on channel 0.
        let (channel, delivery) = if self.caps.is_sunshine() {
            (channel, delivery)
        } else {
            (channel::GENERIC, Delivery::Reliable)
        };
        self.transmit.push_back(Outgoing {
            channel,
            delivery,
            datagram,
        });
        true
    }

    /// The decoder needs an IDR frame; pending invalidations are dropped.
    pub fn request_idr_frame(&mut self) {
        self.reference_frames.clear();
        self.idr_requested = true;
    }

    /// The video stream lost frames `first..=last`.
    pub fn frame_loss(&mut self, first: u32, last: u32) {
        if !self.options.reference_frame_invalidation || first > last {
            self.request_idr_frame();
            return;
        }
        if self.reference_frames.len() >= REFERENCE_FRAME_QUEUE_BOUND {
            self.request_idr_frame();
            return;
        }
        self.reference_frames
            .push_back(ReferenceFrameControl::Invalidate { first, last });
    }

    /// The video stream completed a frame; long-term reference frames are acknowledged to
    /// Sunshine hosts when RFI is on.
    pub fn frame_complete(&mut self, frame: u32, long_term_reference: bool) {
        self.last_good_frame = frame;
        self.interval_good_frames = self.interval_good_frames.wrapping_add(1);
        if long_term_reference
            && self.caps.is_sunshine()
            && self.options.reference_frame_invalidation
        {
            if self.reference_frames.len() >= REFERENCE_FRAME_QUEUE_BOUND {
                self.request_idr_frame();
            } else {
                self.reference_frames
                    .push_back(ReferenceFrameControl::Acknowledge(frame));
            }
        }
    }

    /// The video stream saw (part of) frame `frame`; drives the connection status.
    pub fn frame_seen(&mut self, frame: u32, now: Instant) {
        let Some(first_frame_at) = self.first_frame_at else {
            self.last_seen_frame = frame;
            self.first_frame_at = Some(now);
            return;
        };
        // The network and host settle during the first sample period.
        if now.duration_since(first_frame_at) < CONN_STATUS_SAMPLE_PERIOD {
            self.last_seen_frame = frame;
            return;
        }
        let interval_over = match self.interval_start {
            Some(start) => now.duration_since(start) >= CONN_STATUS_SAMPLE_PERIOD,
            None => true,
        };
        if interval_over {
            if self.interval_total_frames != 0 {
                let good = self.interval_good_frames.min(self.interval_total_frames);
                let loss = 100 - good * 100 / self.interval_total_frames;
                if self.connection_status != ConnectionStatus::Poor
                    && (loss >= CONN_IMMEDIATE_POOR_LOSS_RATE
                        || (loss >= CONN_CONSECUTIVE_POOR_LOSS_RATE
                            && self.last_interval_loss >= CONN_CONSECUTIVE_POOR_LOSS_RATE))
                {
                    self.connection_status = ConnectionStatus::Poor;
                    self.events
                        .push(ControlEvent::ConnectionStatus(ConnectionStatus::Poor));
                } else if loss <= CONN_OKAY_LOSS_RATE
                    && self.connection_status != ConnectionStatus::Okay
                {
                    self.connection_status = ConnectionStatus::Okay;
                    self.events
                        .push(ControlEvent::ConnectionStatus(ConnectionStatus::Okay));
                }
                self.last_interval_loss = loss;
            }
            self.interval_start = Some(now);
            self.interval_good_frames = 0;
            self.interval_total_frames = 0;
        }
        self.interval_total_frames = self
            .interval_total_frames
            .wrapping_add(frame.wrapping_sub(self.last_seen_frame));
        self.last_seen_frame = frame;
    }

    /// Queues a per-frame FEC report for the next ping (Sunshine only, best effort).
    pub fn frame_fec_status(&mut self, status: FrameFecStatus) {
        if self.caps.is_sunshine() && self.fec_statuses.len() < FEC_STATUS_QUEUE_BOUND {
            self.fec_statuses.push_back(status);
        }
    }

    pub fn connection_status(&self) -> ConnectionStatus {
        self.connection_status
    }

    pub fn last_good_frame(&self) -> u32 {
        self.last_good_frame
    }

    pub fn hdr_enabled(&self) -> bool {
        self.hdr_enabled
    }

    /// The host's HDR metadata while HDR is on (Sunshine hosts).
    pub fn hdr_metadata(&self) -> Option<HdrMetadata> {
        (self.caps.is_sunshine() && self.hdr_enabled).then_some(self.hdr_metadata)
    }

    pub fn cursor_shapes_requested(&self) -> bool {
        self.cursor_shapes
    }

    /// Queues what is due: an IDR request or the batched RFI range and LTR acknowledgements,
    /// then the FEC reports and the periodic ping. Returns when to call again.
    pub fn poll(&mut self, now: Instant) -> Option<Instant> {
        if !self.started || self.is_stopped() {
            return None;
        }
        if self.idr_requested {
            self.idr_requested = false;
            self.reference_frames.clear();
            self.queue(&ClientMessage::RequestIdrFrame);
        } else if !self.reference_frames.is_empty() {
            let mut range: Option<(u32, u32)> = None;
            while let Some(control) = self.reference_frames.pop_front() {
                match control {
                    ReferenceFrameControl::Invalidate { first, last } => {
                        range = Some(match range {
                            Some((start, end)) => (start, end.max(last)),
                            None => (first, last),
                        });
                    }
                    ReferenceFrameControl::Acknowledge(frame) => {
                        self.queue(&ClientMessage::LtrFrameAck { frame });
                    }
                }
            }
            if let Some((first, last)) = range {
                self.queue(&ClientMessage::InvalidateReferenceFrames { first, last });
            }
        }
        let next_ping = self.next_ping.get_or_insert(now);
        if now >= *next_ping {
            *next_ping = now + PERIODIC_PING_INTERVAL;
            while let Some(status) = self.fec_statuses.pop_front() {
                self.queue(&ClientMessage::FrameFecStatus(status));
            }
            self.queue(&ClientMessage::PeriodicPing);
        }
        self.next_ping
    }

    /// The next datagram to send.
    pub fn poll_transmit(&mut self) -> Option<Outgoing> {
        self.transmit.pop_front()
    }

    /// Whether more datagrams wait after the one just taken.
    pub fn has_transmit(&self) -> bool {
        !self.transmit.is_empty()
    }

    /// The events since the last call, with stale rumble, LED and HDR updates collapsed.
    pub fn take_events(&mut self) -> Vec<ControlEvent> {
        let mut events = std::mem::take(&mut self.events);
        events::coalesce(&mut events);
        events
    }

    /// The host disconnected (or ENet timed the peer out) without a termination message.
    pub fn disconnected(&mut self) {
        self.terminate(events::termination::DISCONNECTED);
    }

    fn terminate(&mut self, error: i32) {
        if self.terminated.is_none() {
            self.terminated = Some(error);
            self.events.push(ControlEvent::Terminated { error });
        }
    }

    /// Opens and dispatches one datagram from the host.
    pub fn handle_datagram(&mut self, now: Instant, datagram: &[u8]) -> Result<(), OpenError> {
        if self.terminated.is_some() {
            return Ok(());
        }
        let (message_type, payload) = self.opener.open(datagram)?;
        self.dispatch(now, message_type, &payload);
        Ok(())
    }

    fn dispatch(&mut self, now: Instant, message_type: u16, p: &[u8]) {
        let extensions = self.caps.sunshine_extensions();
        let event = match message_type {
            ptype::HDR_MODE => {
                let Some((enabled, metadata)) = events::hdr_mode(p, self.caps.is_sunshine()) else {
                    return;
                };
                self.hdr_enabled = enabled;
                if let Some(metadata) = metadata {
                    self.hdr_metadata = metadata;
                }
                Some(ControlEvent::HdrMode {
                    enabled,
                    metadata: metadata.filter(|_| enabled),
                })
            }
            ptype::TERMINATION => {
                if let Some(error) = events::termination_error(p, self.last_seen_frame != 0) {
                    self.terminate(error);
                }
                None
            }
            ptype::RUMBLE => events::rumble(p),
            ptype::RUMBLE_TRIGGERS => events::rumble_triggers(p),
            ptype::SET_MOTION_EVENT => events::motion_event(p),
            ptype::SET_RGB_LED => events::controller_led(p),
            ptype::ADAPTIVE_TRIGGERS => events::adaptive_triggers(p),
            ptype::CONTROLLER_HAPTICS if extensions => events::controller_haptics(p),
            ptype::CONTROLLER_SPEAKER if extensions => events::controller_speaker(p),
            ptype::CONTROLLER_AUDIO_CONTROL if extensions => events::controller_audio_control(p),
            ptype::HOST_TIMING if extensions => events::host_timing(p),
            ptype::CLOCK_SYNC if extensions => events::clock_sync(p, now),
            ptype::CURSOR_SHAPE if extensions && self.cursor_shapes => {
                Some(events::cursor_shape(p))
            }
            _ => None,
        };
        if let Some(event) = event {
            self.events.push(event);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::negotiation::encryption;
    use butterpollo_core::packet;
    use rubylight_protocol::control::{
        ClockSyncReply, DisplayCaps, HostTiming, HostTimingExtra, HOST_TIMING_EXTENDED_MAX_RECORDS,
    };

    const KEY: Key = *b"0011223344556677";

    fn rubylight() -> HostCapabilities {
        HostCapabilities {
            app_version: [7, 1, 431, -1],
            feature_flags: 3,
            encryption_enabled: encryption::CONTROL_V2,
            control_messages: vec![
                0x5530, 0x5531, 0x5532, 0x5534, 0x5535, 0x5536, 0x5537, 0x5540,
            ],
        }
    }

    fn session(options: SessionOptions) -> ControlSession {
        ControlSession::new(rubylight(), &KEY, options)
    }

    /// Opens everything queued as the host would.
    fn host_reads(session: &mut ControlSession) -> Vec<(u8, Delivery, u16, Vec<u8>)> {
        std::iter::from_fn(|| session.poll_transmit())
            .map(|out| {
                let (_, kind, payload) =
                    packet::decrypt_control(&KEY, &out.datagram, true).unwrap();
                (out.channel, out.delivery, kind, payload)
            })
            .collect()
    }

    fn from_host(seq: u32, kind: u16, payload: &[u8]) -> Vec<u8> {
        packet::encrypted_control(&KEY, seq, kind, payload, true).unwrap()
    }

    #[test]
    fn start_sends_start_messages_then_pings() {
        let mut s = session(SessionOptions::default());
        let t0 = Instant::now();
        assert_eq!(s.poll(t0), None, "nothing before start");
        s.start(t0);
        assert_eq!(s.poll(t0), Some(t0 + PERIODIC_PING_INTERVAL));
        let sent = host_reads(&mut s);
        let kinds: Vec<u16> = sent.iter().map(|m| m.2).collect();
        assert_eq!(kinds, [0x0302, 0x0307, 0x0200]);
        assert_eq!(
            sent[2],
            (0, Delivery::Reliable, 0x0200, vec![4, 0, 0, 0, 0, 0, 0, 0])
        );
        assert_eq!(
            s.poll(t0 + Duration::from_millis(50)),
            Some(t0 + PERIODIC_PING_INTERVAL)
        );
        assert!(host_reads(&mut s).is_empty());
        s.poll(t0 + PERIODIC_PING_INTERVAL);
        assert_eq!(host_reads(&mut s).len(), 1);
    }

    #[test]
    fn rfi_batches_ranges_and_idr_supersedes_them() {
        let mut s = session(SessionOptions {
            reference_frame_invalidation: true,
        });
        let t0 = Instant::now();
        s.start(t0);
        s.poll(t0);
        host_reads(&mut s);

        s.frame_loss(10, 12);
        s.frame_complete(13, true);
        s.frame_loss(14, 15);
        s.poll(t0);
        let sent = host_reads(&mut s);
        assert_eq!(sent.len(), 2);
        assert_eq!((sent[0].0, sent[0].2), (channel::URGENT, 0x0350));
        assert_eq!(&sent[0].3[..4], &13u32.to_le_bytes());
        assert_eq!(sent[1].2, 0x0301);
        assert_eq!(&sent[1].3[..4], &10u32.to_le_bytes());
        assert_eq!(&sent[1].3[8..12], &15u32.to_le_bytes());

        s.frame_loss(20, 21);
        s.request_idr_frame();
        s.poll(t0);
        let sent = host_reads(&mut s);
        assert_eq!(sent.iter().map(|m| m.2).collect::<Vec<_>>(), [0x0302]);

        for frame in 0..REFERENCE_FRAME_QUEUE_BOUND as u32 + 1 {
            s.frame_loss(frame, frame);
        }
        s.poll(t0);
        assert_eq!(
            host_reads(&mut s).iter().map(|m| m.2).collect::<Vec<_>>(),
            [0x0302]
        );
    }

    #[test]
    fn without_rfi_loss_requests_idr_and_ltr_acks_are_refused() {
        let mut s = session(SessionOptions::default());
        let t0 = Instant::now();
        s.start(t0);
        s.poll(t0);
        host_reads(&mut s);
        s.frame_loss(3, 4);
        s.frame_complete(5, true);
        s.poll(t0);
        assert_eq!(
            host_reads(&mut s).iter().map(|m| m.2).collect::<Vec<_>>(),
            [0x0302]
        );
        assert_eq!(
            s.send(ClientMessage::LtrFrameAck { frame: 1 }),
            Err(SendError::Unsupported)
        );
    }

    #[test]
    fn fec_reports_ride_with_the_ping_and_are_bounded() {
        let mut s = session(SessionOptions::default());
        let t0 = Instant::now();
        s.start(t0);
        for frame in 0..10 {
            s.frame_fec_status(FrameFecStatus {
                frame_index: frame,
                multi_fec_block_count: 1,
                ..Default::default()
            });
        }
        s.poll(t0);
        let sent = host_reads(&mut s);
        let fec: Vec<_> = sent.iter().filter(|m| m.2 == 0x5502).collect();
        assert_eq!(fec.len(), FEC_STATUS_QUEUE_BOUND);
        assert!(fec.iter().all(|m| m.1 == Delivery::Unsequenced));
        assert_eq!(sent.last().unwrap().2, 0x0200);
    }

    #[test]
    fn rubylight_messages_need_announcement() {
        let mut s = session(SessionOptions::default());
        let caps = DisplayCaps {
            hdr: true,
            max_centinits: 100_000,
            max_average_centinits: 50_000,
            min_decimillinits: 5,
        };
        s.send(ClientMessage::DisplayCaps(caps)).unwrap();
        let sent = host_reads(&mut s);
        assert_eq!(sent[0].2, 0x5531);
        assert_eq!(DisplayCaps::decode(&sent[0].3), Some(caps));

        let mut bare = ControlSession::new(
            HostCapabilities {
                control_messages: vec![],
                ..rubylight()
            },
            &KEY,
            SessionOptions::default(),
        );
        assert_eq!(
            bare.send(ClientMessage::DisplayCaps(caps)),
            Err(SendError::Unsupported)
        );
        assert_eq!(
            bare.send(ClientMessage::CursorShapeRequest { enabled: true }),
            Err(SendError::Unsupported)
        );
        assert!(!bare.cursor_shapes_requested());

        let mut gfe = ControlSession::new(
            HostCapabilities {
                app_version: [7, 1, 431, 0],
                ..rubylight()
            },
            &KEY,
            SessionOptions::default(),
        );
        assert_eq!(
            gfe.send(ClientMessage::ServerCommand(vec![1])),
            Err(SendError::Unsupported)
        );
        gfe.send(ClientMessage::RequestIdrFrame).unwrap();
        let out = gfe.poll_transmit().unwrap();
        assert_eq!((out.channel, out.delivery), (0, Delivery::Reliable));

        let seq = s.send_clock_sync_probe(42).unwrap();
        assert_eq!(seq, 1);
        let sent = host_reads(&mut s);
        assert_eq!(sent[0].1, Delivery::Unsequenced);
        assert_eq!(
            ClockSyncRequest::decode(&sent[0].3),
            Some(ClockSyncRequest {
                sequence: 1,
                client_us: 42
            })
        );
    }

    #[test]
    fn host_messages_become_events() {
        let mut s = session(SessionOptions::default());
        let t0 = Instant::now();
        s.start(t0);
        let mut seq = 0;
        let mut feed = |s: &mut ControlSession, kind, payload: &[u8]| {
            seq += 1;
            s.handle_datagram(t0, &from_host(seq, kind, payload))
                .unwrap();
        };
        feed(&mut s, 0x010b, &[0, 0, 0, 0, 1, 0, 2, 0, 3, 0]);
        feed(&mut s, 0x5500, &[1, 0, 4, 0, 5, 0]);
        let mut hdr = vec![1];
        hdr.extend_from_slice(&[0x10, 0x27].repeat(12));
        feed(&mut s, 0x010e, &hdr);
        let timing = HostTiming {
            frame: 5,
            host_us: 100,
            queue_us: 10,
            encode_us: 50,
            send_us: 20,
        };
        feed(&mut s, 0x5536, &HostTiming::encode_report(&[timing]));
        let reply = ClockSyncReply {
            sequence: 1,
            client_us: 42,
            host_receive_us: 1000,
            host_send_us: 1010,
        };
        feed(&mut s, 0x5537, &reply.encode());
        // Not requested yet: dropped.
        feed(&mut s, 0x5541, &[1, 0, 0, 0, 0, 0, 0, 0]);
        s.send(ClientMessage::CursorShapeRequest { enabled: true })
            .unwrap();
        feed(&mut s, 0x5541, &[1, 0, 0, 0, 0, 0, 0, 0]);
        let pcm = crate::control::haptics::encode(0, 9, &[1; 8]).unwrap();
        feed(&mut s, 0x5601, &pcm);
        feed(&mut s, 0x5533, &pcm);

        let events = s.take_events();
        assert_eq!(
            events[0],
            ControlEvent::Rumble {
                controller: 1,
                low_frequency: 2,
                high_frequency: 3
            }
        );
        assert_eq!(
            events[1],
            ControlEvent::RumbleTriggers {
                controller: 1,
                left: 4,
                right: 5
            }
        );
        let ControlEvent::HdrMode {
            enabled: true,
            metadata: Some(metadata),
        } = events[2]
        else {
            panic!("{:?}", events[2]);
        };
        assert_eq!(metadata.max_display_luminance, 10_000);
        assert!(s.hdr_enabled() && s.hdr_metadata().is_some());
        assert_eq!(events[3], ControlEvent::HostTiming(vec![(timing, None)]));
        assert_eq!(
            events[4],
            ControlEvent::ClockSync {
                reply,
                received: t0
            }
        );
        assert_eq!(
            events[5],
            ControlEvent::CursorShape(events::CursorShape::Hide)
        );
        assert!(matches!(&events[6], ControlEvent::ControllerHaptics(p) if p.sequence == 9));
        assert!(matches!(&events[7], ControlEvent::ControllerSpeaker(p) if p.frames() == 2));
        assert_eq!(events.len(), 8);
    }

    #[test]
    fn extended_host_timing_reports_keep_their_extension() {
        let mut s = session(SessionOptions::default());
        let t0 = Instant::now();
        s.start(t0);
        let record = |frame| HostTiming {
            frame,
            host_us: 900,
            queue_us: 300,
            encode_us: 200,
            send_us: 100,
        };
        let extra = HostTimingExtra {
            presented_at_us: 5_000_000_123,
            capture_us: 100,
            convert_us: 150,
        };
        let full = vec![(record(7), extra); HOST_TIMING_EXTENDED_MAX_RECORDS];
        let report = HostTiming::encode_report_with(&full);
        assert_eq!(report.len(), 616);
        let mut seq = 0;
        let mut feed = |s: &mut ControlSession, payload: &[u8]| {
            seq += 1;
            s.handle_datagram(t0, &from_host(seq, 0x5536, payload))
                .unwrap();
        };
        feed(&mut s, &report);
        // A v1 report has no extension; one with capture and convert past queue loses it.
        feed(&mut s, &HostTiming::encode_report(&[record(8)]));
        let bad = HostTimingExtra {
            capture_us: 200,
            convert_us: 200,
            ..extra
        };
        feed(&mut s, &HostTiming::encode_report_with(&[(record(9), bad)]));
        // The widest report the decoder takes is 32 records with 16 extra bytes each (1156 bytes),
        // so a host's record cap can rise; past 17 records the extension is not used.
        let mut widest = HostTiming::encode_report(&[record(10); 32]);
        widest[2] = 16;
        widest.resize(4 + 32 * 36, 0);
        feed(&mut s, &widest);

        let events = s.take_events();
        let ControlEvent::HostTiming(records) = &events[0] else {
            panic!("{:?}", events[0]);
        };
        assert_eq!(records.len(), 17);
        assert!(records.iter().all(|r| *r == (record(7), Some(extra))));
        assert_eq!(events[1], ControlEvent::HostTiming(vec![(record(8), None)]));
        assert_eq!(events[2], ControlEvent::HostTiming(vec![(record(9), None)]));
        // Over 17 records the base records stay and the extension is dropped.
        let ControlEvent::HostTiming(records) = &events[3] else {
            panic!("{:?}", events[3]);
        };
        assert_eq!(records.len(), 32);
        assert!(records.iter().all(|r| *r == (record(10), None)));
    }

    #[test]
    fn termination_ends_the_session_once() {
        let mut s = session(SessionOptions::default());
        let t0 = Instant::now();
        s.start(t0);
        s.frame_seen(1, t0);
        s.handle_datagram(t0, &from_host(1, 0x0109, &0x8003_0023u32.to_be_bytes()))
            .unwrap();
        assert_eq!(s.terminated(), Some(0));
        s.disconnected();
        assert_eq!(s.take_events(), [ControlEvent::Terminated { error: 0 }]);
        assert_eq!(
            s.send(ClientMessage::RequestIdrFrame),
            Err(SendError::Stopped)
        );
        assert_eq!(s.poll(t0), None);

        let mut early = session(SessionOptions::default());
        early
            .handle_datagram(t0, &from_host(1, 0x0109, &0x8003_0023u32.to_be_bytes()))
            .unwrap();
        assert_eq!(early.terminated(), Some(-102));
        let mut silent = session(SessionOptions::default());
        silent.disconnected();
        assert_eq!(
            silent.take_events(),
            [ControlEvent::Terminated { error: -1 }]
        );
    }

    #[test]
    fn connection_status_follows_frame_loss() {
        let mut s = session(SessionOptions::default());
        let t0 = Instant::now();
        let ms = |n| t0 + Duration::from_millis(n);
        s.frame_seen(1, t0);
        s.frame_seen(2, ms(3100));
        // 40 % loss over the next interval.
        let mut frame = 2;
        for i in 0..100u32 {
            frame += 1;
            if i % 5 >= 2 {
                s.frame_complete(frame, false);
            }
            s.frame_seen(frame, ms(3100 + u64::from(i) * 30));
        }
        s.frame_seen(frame + 1, ms(6200));
        assert_eq!(
            s.take_events(),
            [ControlEvent::ConnectionStatus(ConnectionStatus::Poor)]
        );
        for i in 0..100u32 {
            frame += 1;
            s.frame_complete(frame, false);
            s.frame_seen(frame, ms(6200 + u64::from(i) * 30));
        }
        s.frame_seen(frame + 1, ms(9300));
        assert_eq!(
            s.take_events(),
            [ControlEvent::ConnectionStatus(ConnectionStatus::Okay)]
        );
    }
}
