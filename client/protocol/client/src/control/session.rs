//! The control stream as a socket-free state machine: messages and times in, sealed
//! datagrams and [`ControlEvent`]s out. [`super::driver`] runs it over ENet.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use rubylight_protocol::control::ClockSyncRequest;
use rubylight_protocol::fec_report::{FecSummaryTally, FecTally, MIN_INTERVAL_MS};

use super::events::{self, ConnectionStatus, ControlEvent, HdrMetadata};
use super::messages::{ClientMessage, FrameFecStatus};
use super::negotiation::HostCapabilities;
use super::wire::{channel, ptype, Delivery, OpenError, Opener, Sealer};
use crate::crypto::Key;
use crate::video::FecBlock;

/// How often the periodic ping goes out.
pub const PERIODIC_PING_INTERVAL: Duration = Duration::from_millis(100);
const REFERENCE_FRAME_QUEUE_BOUND: usize = 20;
const FEC_STATUS_QUEUE_BOUND: usize = 8;
const CONN_IMMEDIATE_POOR_LOSS_RATE: u32 = 30;
const CONN_CONSECUTIVE_POOR_LOSS_RATE: u32 = 15;
const CONN_OKAY_LOSS_RATE: u32 = 5;
const CONN_STATUS_SAMPLE_PERIOD: Duration = Duration::from_millis(3000);
const FEC_SUMMARY_INTERVAL: Duration = Duration::from_millis(1000);
/// Queued datagrams past which arrival feedback is dropped rather than queued.
const TRANSPORT_FEEDBACK_BACKLOG: usize = 32;

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
    /// Send Foundation Sunshine's once-a-second 0x5510 FEC summary
    /// (`RtspOutcome::fec_summaries`).
    pub fec_summaries: bool,
}

#[derive(Clone, Copy, Debug)]
enum ReferenceFrameControl {
    Invalidate { first: u32, last: u32 },
    Acknowledge(u32),
}

/// Video FEC outcomes since the last 0x5539 report, timed from the first block counted.
#[derive(Default)]
struct FecReports {
    tally: FecTally,
    epoch: Option<Instant>,
    /// The tally's interval start, mirrored because the tally keeps it private: set by its first
    /// block and restarted by each report, which is when the tally's own start restarts.
    since: Option<Instant>,
    /// Whether the tally counts a block that could not be recovered since its last report.
    lost: bool,
}

impl FecReports {
    fn ms(&mut self, now: Instant) -> u64 {
        let epoch = *self.epoch.get_or_insert(now);
        now.saturating_duration_since(epoch).as_millis() as u64
    }
}

/// Foundation Sunshine's 0x5510 summaries, ported from V+'s ControlStream.c: blocks are counted
/// at their first final status, and every second on the ping's tick, the first one at once.
#[derive(Default)]
struct FecSummaries {
    tally: FecSummaryTally,
    last: Option<Instant>,
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
    fec_reports: FecReports,
    fec_summaries: FecSummaries,
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
    /// Frame Warp's shared state, fed the host's model, tags and mask while requested.
    frame_warp: Option<std::sync::Arc<crate::frame_warp::FrameWarp>>,
    frame_warp_on: bool,
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
            fec_reports: FecReports::default(),
            fec_summaries: FecSummaries::default(),
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
            frame_warp: None,
            frame_warp_on: false,
        }
    }

    pub fn capabilities(&self) -> &HostCapabilities {
        &self.caps
    }

    /// Where the host's Frame Warp messages go once a [`ClientMessage::FrameWarpRequest`]
    /// enables them.
    pub fn set_frame_warp(&mut self, frame_warp: Option<std::sync::Arc<crate::frame_warp::FrameWarp>>) {
        self.frame_warp = frame_warp;
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
        if let ClientMessage::FrameWarpRequest(request) = message {
            // As for cursor shapes: accepted from just before an enable goes out.
            self.frame_warp_on = request.enabled;
        }
        if !self.queue(&message) {
            if matches!(message, ClientMessage::CursorShapeRequest { .. }) {
                self.cursor_shapes = false;
            }
            if matches!(message, ClientMessage::FrameWarpRequest(_)) {
                self.frame_warp_on = false;
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

    /// Queues a batch of video arrival feedback (0x5547) for a host that listed it. Dropped
    /// without that, after stop, and while the control stream holds a backlog: by then the
    /// report is stale and would only add to it.
    pub fn transport_feedback(
        &mut self,
        report: rubylight_protocol::transport_feedback::TransportFeedback,
    ) {
        if self.transmit.len() >= TRANSPORT_FEEDBACK_BACKLOG {
            return;
        }
        let _ = self.send(ClientMessage::TransportFeedback(report));
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
        // V+ counts the block toward its 0x5510 summary from the same final status.
        if self.options.fec_summaries {
            self.fec_summaries.tally.block(
                status.frame_index,
                status.multi_fec_block_index,
                status.total_data_packets,
                status.received_data_packets,
            );
        }
        if self.caps.is_sunshine() && self.fec_statuses.len() < FEC_STATUS_QUEUE_BOUND {
            self.fec_statuses.push_back(status);
        }
    }

    /// Counts a finished video FEC block toward the next 0x5539 report, for a host that
    /// announced it.
    pub fn fec_block(&mut self, block: FecBlock, now: Instant) {
        // A block that completed without a final status (no recovery needed) counts toward the
        // 0x5510 summary whole; a recovered or given-up one already counted at its status.
        if self.options.fec_summaries && !block.unrecovered {
            let data = u16::try_from(block.data_shards).unwrap_or(0);
            self.fec_summaries
                .tally
                .block(block.frame, block.block_index, data, data);
        }
        if !self.started
            || !self.caps.sunshine_extensions()
            || !self.caps.supports_control_message(ptype::FEC_REPORT)
        {
            return;
        }
        let now_ms = self.fec_reports.ms(now);
        self.fec_reports.tally.block(block, now_ms);
        self.fec_reports.since.get_or_insert(now);
        self.fec_reports.lost |= block.unrecovered;
    }

    /// Queues the FEC report once its interval is a quarter second old if it counts a block
    /// that could not be recovered (see [`FecTally::take_lost`]), otherwise on the ping's tick
    /// (`nominal`) once it is a second old (see [`FecTally::take`]).
    fn flush_fec_report(&mut self, now: Instant, nominal: bool) {
        if self.fec_reports.epoch.is_none() {
            return;
        }
        let now_ms = self.fec_reports.ms(now);
        let tally = &mut self.fec_reports.tally;
        let report = match tally.take_lost(now_ms) {
            None if nominal => tally.take(now_ms),
            report => report,
        };
        if let Some(report) = report {
            self.fec_reports.since = Some(now);
            self.fec_reports.lost = false;
            self.queue(&ClientMessage::FecReport(report));
        }
    }

    /// When the lost-block report falls due early (see [`FecTally::take_lost`]), if it is still
    /// ahead of `now`. Only the wake-up time is computed here; the report itself is still taken
    /// by `flush_fec_report`.
    fn lost_report_due(&self, now: Instant) -> Option<Instant> {
        let reports = &self.fec_reports;
        if !reports.lost {
            return None;
        }
        let due = reports.since? + Duration::from_millis(u64::from(MIN_INTERVAL_MS));
        (due > now).then_some(due)
    }

    /// Queues the 0x5510 summary once a second (see [`FecSummaryTally::take`]).
    fn flush_fec_summary(&mut self, now: Instant) {
        if !self.options.fec_summaries
            || self
                .fec_summaries
                .last
                .is_some_and(|last| now.saturating_duration_since(last) < FEC_SUMMARY_INTERVAL)
        {
            return;
        }
        self.fec_summaries.last = Some(now);
        if let Some(summary) = self.fec_summaries.tally.take() {
            self.queue(&ClientMessage::FecSummary(summary));
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
            // V+ sends its summary ahead of the per-frame statuses.
            self.flush_fec_summary(now);
            while let Some(status) = self.fec_statuses.pop_front() {
                self.queue(&ClientMessage::FrameFecStatus(status));
            }
            self.flush_fec_report(now, true);
            self.queue(&ClientMessage::PeriodicPing);
        } else {
            self.flush_fec_report(now, false);
        }
        match (self.next_ping, self.lost_report_due(now)) {
            (Some(ping), Some(report)) => Some(ping.min(report)),
            (ping, report) => ping.or(report),
        }
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
        let (message_type, end) = self.opener.open_into(datagram)?;
        let plain = std::mem::take(&mut self.opener.scratch);
        self.dispatch(now, message_type, &plain[super::wire::INNER_HEADER_BYTES..end]);
        self.opener.scratch = plain;
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
            ptype::STEAM_HAPTIC if extensions => Some(events::steam_haptic(p)),
            ptype::STEAM_SETTINGS if extensions => Some(events::steam_settings(p)),
            ptype::CONTROLLER_HAPTICS if extensions => events::controller_haptics(p),
            ptype::CONTROLLER_SPEAKER if extensions => events::controller_speaker(p),
            ptype::CONTROLLER_AUDIO_CONTROL if extensions => events::controller_audio_control(p),
            ptype::HOST_TIMING if extensions => events::host_timing(p),
            ptype::CLOCK_SYNC if extensions => events::clock_sync(p, now),
            ptype::LINK_NOTICE if extensions => events::link_notice(p),
            ptype::TEXT_INPUT if extensions => events::text_input(p),
            ptype::PIPELINE_RATES if extensions => events::pipeline_rates(p),
            ptype::RATE_CHANGE
                if extensions && self.caps.supports_control_message(ptype::RATE_CHANGE) =>
            {
                events::rate_change(p)
            }
            ptype::CURSOR_SHAPE if extensions && self.cursor_shapes => {
                Some(events::cursor_shape(p))
            }
            ptype::FRAME_WARP_MODEL | ptype::FRAME_WARP_TAGS | ptype::FRAME_WARP_MASK
                if extensions && self.frame_warp_on =>
            {
                self.frame_warp_message(message_type, p)
            }
            ptype::USB if extensions && self.caps.supports_control_message(ptype::USB) => {
                events::usb(p)
            }
            ptype::CLIPBOARD_SYNC
                if extensions && self.caps.supports_control_message(ptype::CLIPBOARD_SYNC) =>
            {
                Some(events::clipboard_sync(p))
            }
            _ => None,
        };
        if let Some(event) = event {
            self.events.push(event);
        }
    }

    /// Hands a Frame Warp message to the shared state; a model also becomes an event for the
    /// statistics. Messages that break their format are dropped.
    fn frame_warp_message(&self, message_type: u16, p: &[u8]) -> Option<ControlEvent> {
        use rubylight_protocol::frame_warp::{Mask, Model, Tags};
        let warp = self.frame_warp.as_ref();
        match message_type {
            ptype::FRAME_WARP_MODEL => {
                let model = Model::decode(p)?;
                if let Some(warp) = warp {
                    warp.on_model(model);
                }
                Some(ControlEvent::FrameWarpModel(model))
            }
            ptype::FRAME_WARP_TAGS => {
                let tags = Tags::decode(p)?;
                if let Some(warp) = warp {
                    warp.on_tags(&tags);
                }
                None
            }
            _ => {
                let mask = Mask::decode(p)?;
                if let Some(warp) = warp {
                    warp.on_mask(mask);
                }
                None
            }
        }
    }
}

#[cfg(test)]
mod tests;
