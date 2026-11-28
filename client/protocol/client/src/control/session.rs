//! The control stream as a socket-free state machine: messages and times in, sealed
//! datagrams and [`ControlEvent`]s out. [`super::driver`] runs it over ENet.

use std::collections::VecDeque;
use std::time::Instant;

use rubylight_protocol::control::ClockSyncRequest;

use super::events::{self, ConnectionStatus, ControlEvent, HdrMetadata};
use super::messages::{ClientMessage, FrameFecStatus};
use super::negotiation::HostCapabilities;
use super::wire::{Delivery, OpenError, Opener, Sealer, channel, ptype};
use crate::crypto::Key;
#[cfg(test)]
use crate::video::FecBlock;

mod fec;
#[cfg(test)]
use fec::{FEC_STATUS_QUEUE_BOUND, REFERENCE_FRAME_QUEUE_BOUND};
use fec::{FecReports, FecSummaries};

/// How often the periodic ping goes out.
pub const PERIODIC_PING_INTERVAL: Duration = Duration::from_millis(100);
/// Queued datagrams past which arrival feedback is dropped rather than queued.
const TRANSPORT_FEEDBACK_BACKLOG: usize = 32;

use std::time::Duration;

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

/// The encrypted control stream of one connection.
pub struct ControlSession {
    caps: HostCapabilities,
    options: SessionOptions,
    sealer: Sealer,
    opener: Opener,
    /// Scratch buffer for encoding control message payloads before sealing.
    payload_buf: Vec<u8>,
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
    /// Speaker PCM (0x5602) for the controllers the input queue bound, when the ANNOUNCE asked.
    ps_speaker: Option<super::ps_speaker::Receiver>,
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
            payload_buf: Vec::new(),
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
            ps_speaker: None,
        }
    }

    pub fn capabilities(&self) -> &HostCapabilities {
        &self.caps
    }

    /// Where the host's Frame Warp messages go once a [`ClientMessage::FrameWarpRequest`]
    /// enables them.
    pub fn set_frame_warp(
        &mut self,
        frame_warp: Option<std::sync::Arc<crate::frame_warp::FrameWarp>>,
    ) {
        self.frame_warp = frame_warp;
    }

    /// Takes speaker PCM for the controllers bound in `owners` (the ANNOUNCE asked for it).
    pub fn set_ps_speaker(&mut self, owners: std::sync::Arc<super::ps_speaker::Owners>) {
        self.ps_speaker = Some(super::ps_speaker::Receiver::new(owners));
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
        if let Some(id) = message.announcement()
            && !self.caps.supports_control_message(id)
        {
            return Err(SendError::Unsupported);
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
            self.frame_warp_on = request.base.enabled;
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

    /// Queues an encoded input packet, sealing into the caller's existing `out` buffer so the
    /// input packet's allocation is reused as the outgoing ENet datagram.
    pub(super) fn send_input_into(
        &mut self,
        packet: &[u8],
        out: Vec<u8>,
        channel: u8,
        delivery: Delivery,
    ) -> Result<(), SendError> {
        if self.is_stopped() {
            return Err(SendError::Stopped);
        }
        if self.push_into(ptype::INPUT_DATA, packet, out, channel, delivery) {
            Ok(())
        } else {
            Err(SendError::Invalid)
        }
    }

    fn queue(&mut self, message: &ClientMessage) -> bool {
        let mut payload = std::mem::take(&mut self.payload_buf);
        message.encode_payload_into(&mut payload);
        let ok = self.push(
            message.message_type(),
            &payload,
            message.channel(),
            message.delivery(),
        );
        self.payload_buf = payload;
        ok
    }

    fn push(&mut self, message_type: u16, payload: &[u8], channel: u8, delivery: Delivery) -> bool {
        self.push_into(message_type, payload, Vec::new(), channel, delivery)
    }

    fn push_into(
        &mut self,
        message_type: u16,
        payload: &[u8],
        mut datagram: Vec<u8>,
        channel: u8,
        delivery: Delivery,
    ) -> bool {
        if !self.sealer.seal_into(message_type, payload, &mut datagram) {
            return false;
        }
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
        self.dispatch(
            now,
            message_type,
            &plain[super::wire::INNER_HEADER_BYTES..end],
        );
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
            ptype::PS_SPEAKER_PCM if extensions => self
                .ps_speaker
                .as_mut()
                .and_then(|speaker| speaker.admit(p))
                .map(ControlEvent::ControllerSpeaker),
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
            ptype::FRAME_WARP_MODEL
            | ptype::FRAME_WARP_TAGS
            | ptype::FRAME_WARP_MASK
            | ptype::FRAME_WARP_V2_META
                if extensions && self.frame_warp_on =>
            {
                self.frame_warp_message(message_type, p)
            }
            ptype::USB if extensions && self.caps.supports_control_message(ptype::USB) => {
                events::usb(p)
            }
            ptype::QUICK_SUSPEND
                if extensions && self.caps.supports_control_message(ptype::QUICK_SUSPEND) =>
            {
                events::quick_suspend(p)
            }
            ptype::CLIPBOARD_SYNC
                if extensions && self.caps.supports_control_message(ptype::CLIPBOARD_SYNC) =>
            {
                Some(events::clipboard_sync(p))
            }
            ptype::STREAM_DVR
                if extensions && self.caps.supports_control_message(ptype::STREAM_DVR) =>
            {
                events::stream_dvr(p)
            }
            ptype::LAUNCH_CURTAIN
                if extensions && self.caps.supports_control_message(ptype::LAUNCH_CURTAIN) =>
            {
                events::launch_curtain(p)
            }
            _ => None,
        };
        if let Some(event) = event {
            self.events.push(event);
        }
    }
}

#[cfg(test)]
mod tests;
