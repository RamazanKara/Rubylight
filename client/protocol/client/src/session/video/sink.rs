//! Direct-submit video decoder state, receiver sink adapter and FEC status conversion.

use std::sync::atomic::Ordering;

use super::queue::StagedChunks;
use super::refresh::KeyFrameRequests;
use super::{lock, VideoShared};
use super::super::{now_us, DecodeStatus, VideoDecoder};
use crate::control::FrameFecStatus;
use crate::video::{Frame, FrameType, VideoReceiver, VideoSignal, VideoSink};

/// The decoder when it runs on the receive thread (`CAPABILITY_DIRECT_SUBMIT`): the frames the
/// receiver completed, submitted once its lock is released, and the feedback that waits for
/// the next lock.
pub(super) struct Direct {
    pub(super) decoder: Box<dyn VideoDecoder>,
    pub(super) ready: Vec<Frame>,
    decoded_key_frame: bool,
    need_refresh: bool,
    refresh: KeyFrameRequests,
    pub(super) done: Vec<Frame>,
}

impl Direct {
    pub(super) fn new(decoder: Box<dyn VideoDecoder>, refresh: KeyFrameRequests) -> Self {
        Self {
            decoder,
            ready: Vec::new(),
            decoded_key_frame: false,
            need_refresh: false,
            refresh,
            done: Vec::new(),
        }
    }

    /// Submits the ready frames; runs without the receiver's lock, since the decoder may wait
    /// for an input buffer.
    pub(super) fn submit(&mut self, shared: &VideoShared) {
        for frame in self.ready.drain(..) {
            shared.counters.submitted.fetch_add(1, Ordering::Relaxed);
            let need_idr = match self.decoder.submit(&frame) {
                DecodeStatus::Ok => {
                    if frame.frame_type == FrameType::Idr {
                        self.decoded_key_frame = true;
                        self.refresh.key_frame_decoded();
                    }
                    false
                }
                DecodeStatus::NeedIdr => {
                    shared.counters.need_idr.fetch_add(1, Ordering::Relaxed);
                    true
                }
            };
            self.need_refresh |= self.refresh.request(need_idr, now_us);
            self.done.push(frame);
        }
    }

    /// Sends a held key frame request once its window has passed, for the receive thread
    /// between datagrams: without further frames nothing else would. Returns whether it did.
    pub(super) fn send_held(&mut self, shared: &VideoShared, signals: &mut Vec<VideoSignal>) -> bool {
        if !self.refresh.request(false, now_us) {
            return false;
        }
        let mut sink = Sink {
            shared,
            direct: None,
            staged: None,
            signals,
        };
        lock(&shared.receiver).request_decoder_refresh(&mut sink);
        true
    }

    /// Hands the submissions' feedback and frames back to the receiver.
    pub(super) fn settle(
        &mut self,
        receiver: &mut VideoReceiver,
        shared: &VideoShared,
        signals: &mut Vec<VideoSignal>,
    ) {
        if self.done.is_empty() {
            return;
        }
        if std::mem::take(&mut self.decoded_key_frame) {
            receiver.frame_decoded(FrameType::Idr);
        }
        if !self.decoder.recovers_with_rfi() {
            receiver.recover_with_idr_only();
        }
        if std::mem::take(&mut self.need_refresh) {
            let mut sink = Sink {
                shared,
                direct: None,
                staged: None,
                signals,
            };
            receiver.request_decoder_refresh(&mut sink);
        }
        for frame in self.done.drain(..) {
            receiver.recycle(frame);
        }
    }
}

pub(super) struct Sink<'a> {
    pub(super) shared: &'a VideoShared,
    pub(super) direct: Option<&'a mut Direct>,
    /// Where the receive thread's early chunks wait for their frame or the end of the receive
    /// call ([`super::receive_batch`]). Without, each chunk is queued at once.
    pub(super) staged: Option<&'a mut StagedChunks>,
    pub(super) signals: &'a mut Vec<VideoSignal>,
}

impl VideoSink for Sink<'_> {
    fn frame(&mut self, frame: Frame) -> Result<(), Frame> {
        let key_frame = frame.frame_type == FrameType::Idr;
        if key_frame {
            self.shared.key_frame.store(true, Ordering::Relaxed);
        }
        if let Some(direct) = self.direct.as_deref_mut() {
            direct.ready.push(frame);
            return Ok(());
        }
        let result = match self.staged.as_deref_mut() {
            Some(staged) => {
                let (dropped, result) = self.shared.queue.offer_behind(staged, frame);
                self.shared.counters.count_chunks_dropped(dropped);
                result
            }
            None => self.shared.queue.offer(frame),
        };
        if result.is_err() {
            self.shared
                .counters
                .queue_overflows
                .fetch_add(1, Ordering::Relaxed);
        }
        result
    }

    fn flush_frames(&mut self) -> usize {
        self.shared
            .queue
            .flush_staged_into(self.staged.as_deref_mut(), |frame| {
                self.shared.returns.give(frame);
            })
    }

    fn signal(&mut self, signal: VideoSignal) {
        self.signals.push(signal);
    }

    fn chunk(&mut self, first: bool, flags: u32, payload: &[u8]) {
        // PyroWave never submits directly, so a direct decoder has asked for no chunks.
        if self.direct.is_some() {
            return;
        }
        match self.staged.as_deref_mut() {
            Some(staged) => staged.stage(&self.shared.queue, first, flags, payload),
            None => {
                if !self.shared.queue.offer_chunk(first, flags, payload) {
                    self.shared.counters.count_chunks_dropped(1);
                }
            }
        }
    }
}

pub(super) fn fec_status(status: crate::video::FecStatus) -> FrameFecStatus {
    FrameFecStatus {
        frame_index: status.frame_index,
        highest_received_sequence: status.highest_received_sequence,
        next_contiguous_sequence: status.next_contiguous_sequence,
        missing_before_highest_received: status.missing_before_highest,
        total_data_packets: status.total_data_packets,
        total_parity_packets: status.total_parity_packets,
        received_data_packets: status.received_data_packets,
        received_parity_packets: status.received_parity_packets,
        fec_percentage: status.fec_percentage,
        multi_fec_block_index: status.block_index,
        multi_fec_block_count: status.block_count,
    }
}
