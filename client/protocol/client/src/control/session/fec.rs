//! Control-stream FEC reporting, frame-loss accounting, and Frame Warp dispatch helpers for [`ControlSession`].

use std::time::{Duration, Instant};

use rubylight_protocol::fec_report::{FecSummaryTally, FecTally, MIN_INTERVAL_MS};

use super::{ControlSession, ReferenceFrameControl, TRANSPORT_FEEDBACK_BACKLOG};
use crate::control::events::{ConnectionStatus, ControlEvent};
use crate::control::messages::{ClientMessage, FrameFecStatus};
use crate::control::wire::ptype;
use crate::video::FecBlock;

pub(super) const REFERENCE_FRAME_QUEUE_BOUND: usize = 20;
pub(super) const FEC_STATUS_QUEUE_BOUND: usize = 8;
const CONN_IMMEDIATE_POOR_LOSS_RATE: u32 = 30;
const CONN_CONSECUTIVE_POOR_LOSS_RATE: u32 = 15;
const CONN_OKAY_LOSS_RATE: u32 = 5;
const CONN_STATUS_SAMPLE_PERIOD: Duration = Duration::from_millis(3000);
const FEC_SUMMARY_INTERVAL: Duration = Duration::from_millis(1000);

/// Video FEC outcomes since the last 0x5539 report, timed from the first block counted.
#[derive(Default)]
pub(super) struct FecReports {
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
pub(super) struct FecSummaries {
    tally: FecSummaryTally,
    last: Option<Instant>,
}

impl ControlSession {
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
    pub(super) fn flush_fec_report(&mut self, now: Instant, nominal: bool) {
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
    pub(super) fn lost_report_due(&self, now: Instant) -> Option<Instant> {
        let reports = &self.fec_reports;
        if !reports.lost {
            return None;
        }
        let due = reports.since? + Duration::from_millis(u64::from(MIN_INTERVAL_MS));
        (due > now).then_some(due)
    }

    /// Queues the 0x5510 summary once a second (see [`FecSummaryTally::take`]).
    pub(super) fn flush_fec_summary(&mut self, now: Instant) {
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

    /// Hands a Frame Warp message to the shared state; a model also becomes an event for the
    /// statistics. Messages that break their format are dropped.
    pub(super) fn frame_warp_message(&self, message_type: u16, p: &[u8]) -> Option<ControlEvent> {
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
            ptype::FRAME_WARP_V2_META => {
                let meta = rubylight_protocol::frame_warp_v2::FrameWarpV2Meta::decode(p)?;
                if let Some(warp) = warp {
                    warp.on_meta(meta);
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
