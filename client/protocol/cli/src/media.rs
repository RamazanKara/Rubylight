//! Video decoder, audio renderer, and session event callbacks for `nightjar-cli`.

use std::fs::File;
use std::io::Write;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use nightjar_core::control::{ControlEvent, HdrMetadata};
use nightjar_core::roaming::RoamEvent;
use nightjar_core::sdp::*;
use nightjar_core::session::{
    self, AudioRenderer, AudioSetup, DecodeStatus, Listener, Stage, VideoDecoder, VideoSetup,
};
use nightjar_core::timing_record as record;
use nightjar_core::video::Frame;
use rubylight_protocol::control::{LinkNotice, LinkReason, LinkState};

use crate::decode::{self, Received};
use crate::harness::*;

pub(crate) struct Decoder {
    pub(crate) state: Arc<State>,
    pub(crate) decoder: Option<decode::VideoDecoder>,
    pub(crate) timing_csv: Option<File>,
    pub(crate) bitstream: Option<File>,
    pub(crate) frame_warp: Option<WarpLog>,
}

impl Decoder {
    fn barcode_sample_y(&self, height: i32, row: i32) -> i32 {
        let o = &self.state.options;
        if o.barcode_bottom {
            height
                - (o.barcode_bottom_margin + f64::from(128 - 20 - row * 24) * o.barcode_scale)
                    as i32
        } else {
            (f64::from(20 + row * 24) * o.barcode_scale) as i32
        }
    }

    /// The motion probe's barcode: sequence and performance-counter ticks.
    fn picture_timestamp(&self, d: &decode::VideoDecoder) -> Option<(u32, u64)> {
        let o = &self.state.options;
        let s = o.barcode_scale;
        let (width, height) = (d.width(), d.height());
        if f64::from(width) < 640.0 * s || f64::from(height) < 128.0 * s {
            return None;
        }
        let mut words = [0u32; 4];
        for (row, word) in words.iter_mut().enumerate() {
            let y = self.barcode_sample_y(height, row as i32);
            let black = d.luma((o.barcode_left + 12.0 * s) as i32, y);
            let white = d.luma((o.barcode_left + 36.0 * s) as i32, y);
            if white <= black + 32 {
                return None;
            }
            let threshold = (black + white) / 2;
            for bit in 0..32 {
                let x = (o.barcode_left + f64::from(72 + bit * 16) * s) as i32;
                if d.luma(x, y) > threshold {
                    *word |= 1 << bit;
                }
            }
        }
        if words[3] != 0xB17E_2212 || words[0] == 0 {
            return None;
        }
        Some((words[0], (u64::from(words[2]) << 32) | u64::from(words[1])))
    }

    /// Checks one decoded picture; returns its barcode sequence, ticks and age.
    fn check_picture(&mut self) -> (u32, u64, f64) {
        let state = self.state.clone();
        let o = &state.options;
        let c = &state.counters;
        let d = self.decoder.as_mut().expect("open decoder");
        if o.idr_probe_count != 0 {
            lock(&state.probe).decoded(d.pts(), session::now_us());
        }
        // AMD AV1 surfaces can include alignment padding outside the stream.
        if o.requested_format & VIDEO_FORMAT_MASK_AV1 != 0
            && (o.requested_width..=o.requested_width + 64).contains(&d.width())
            && (o.requested_height..=o.requested_height + 16).contains(&d.height())
        {
            d.crop(o.requested_width, o.requested_height);
        }
        let d = self.decoder.as_ref().expect("open decoder");
        if d.width() != o.requested_width || d.height() != o.requested_height {
            c.fail();
        }
        let (mut low, mut high) = (65535u32, 0u32);
        for y in 1..8 {
            for x in 1..12 {
                let value = d.luma(d.width() * x / 12, d.height() * y / 8);
                low = low.min(value);
                high = high.max(value);
            }
        }
        let depth = d.depth();
        if depth > 0 && high > low + (16u32 << (depth - 8).max(0)) {
            c.detailed_frames.fetch_add(1, Ordering::SeqCst);
        }
        if o.requested_hdr && (depth < 10 || d.primaries() != 9 || d.transfer() != 16) {
            c.fail();
        }
        let decoded = c.decoded_frames.fetch_add(1, Ordering::SeqCst) + 1;
        if decoded == 1 {
            println!(
                "DECODED {}x{} pixel_format={} primaries={} transfer={}",
                d.width(),
                d.height(),
                d.format(),
                d.primaries(),
                d.transfer()
            );
        }
        if let Some(path) = &o.frame_dump
            && decoded == o.frame_dump_at
        {
            d.dump(path);
            println!(
                "FRAME_DUMP frame={} format={}",
                o.frame_dump_at,
                d.format_name()
            );
        }
        let Some((sequence, ticks)) = self.picture_timestamp(d) else {
            return (0, 0, -1.0);
        };
        match picture_age(o.host.as_deref(), ticks) {
            // A signature alone must not turn unrelated content into a latency sample.
            Some(age) if !(0.0..=3000.0).contains(&age) => (0, ticks, -1.0),
            Some(age) => (sequence, ticks, age),
            None => (sequence, ticks, -1.0),
        }
    }
}

impl VideoDecoder for Decoder {
    fn setup(&mut self, setup: &VideoSetup) -> Result<(), i32> {
        let o = &self.state.options;
        println!(
            "VIDEO format={} {}x{}@{}",
            setup.format, setup.width, setup.height, setup.fps
        );
        if setup.format != o.requested_format {
            eprintln!(
                "Codec fallback: requested={} negotiated={}",
                o.requested_format, setup.format
            );
            return Err(-1);
        }
        let codec = if setup.format & VIDEO_FORMAT_MASK_H264 != 0 {
            0
        } else if setup.format & VIDEO_FORMAT_MASK_H265 != 0 {
            1
        } else {
            2
        };
        let (decoder, info, error) = decode::VideoDecoder::open(
            codec,
            o.decoder_threads,
            o.hardware.as_deref(),
            o.hardware_device.as_deref(),
        );
        if let Some(hardware) = &o.hardware
            && !info.hardware_format.is_empty()
        {
            println!(
                "HARDWARE_DECODER type={hardware} format={}",
                info.hardware_format
            );
        }
        if !info.name.is_empty() {
            println!(
                "DECODER codec={} active_thread_type={} delay={}",
                info.name, info.active_thread_type, info.delay
            );
        }
        self.decoder = decoder;
        if self.decoder.is_none() {
            return Err(if error < 0 { error } else { -1 });
        }
        Ok(())
    }

    fn submit(&mut self, frame: &Frame) -> DecodeStatus {
        let state = self.state.clone();
        let o = &state.options;
        let c = &state.counters;
        let decode_started = clock::ms();
        if o.idr_probe_count != 0 {
            lock(&state.probe).arrived(frame);
        }
        let hardware = o.hardware.is_some();
        let arrival_ms = if hardware {
            decode_started
                - (session::now_us() as f64 / 1000.0 - frame.enqueue_time_us as f64 / 1000.0)
        } else {
            decode_started
        };
        let data = frame.data();
        if data.is_empty() || data.len() > 32 * 1024 * 1024 {
            c.frames.fetch_add(1, Ordering::SeqCst);
            c.fail();
            return DecodeStatus::Ok;
        }
        if let Some(path) = &o.first_frame_dump
            && c.frames.load(Ordering::SeqCst) == 0
        {
            let written = File::create(path).and_then(|mut f| f.write_all(data));
            if written.is_err() {
                c.fail();
            }
        }
        if self.bitstream.is_none()
            && let Some(path) = &o.bitstream
        {
            self.bitstream = File::create(path).ok();
        }
        if let Some(file) = &mut self.bitstream {
            let _ = file.write_all(data).and_then(|_| file.flush());
        }
        let (mut sequence, mut ticks, mut age) = (0u32, 0u64, -1.0f64);
        let decoder = self.decoder.as_mut().expect("open decoder");
        if decoder.send(data, i64::from(frame.number)) < 0 {
            c.fail();
        }
        let mut received = decoder.receive();
        while received == Received::Frame || received == Received::Readback {
            if received == Received::Readback {
                c.fail();
            } else {
                (sequence, ticks, age) = self.check_picture();
                // The picture's own frame number, which the decoder carries as its timestamp.
                let picture = self
                    .decoder
                    .as_ref()
                    .and_then(|d| u32::try_from(d.pts()).ok())
                    .unwrap_or(frame.number);
                if let Some(log) = &mut self.frame_warp {
                    if log.row(picture, sequence).is_ok() {
                        c.frame_warp_rows.fetch_add(1, Ordering::SeqCst);
                    } else {
                        c.fail();
                    }
                }
            }
            received = self.decoder.as_mut().expect("open decoder").receive();
        }
        if let Received::Error(_) = received {
            c.fail();
        }
        let decode_ms = clock::ms() - decode_started;
        let host_ms = f64::from(frame.host_processing_latency) / 10.0;
        lock(&state.measurements).push(
            MAX_MEASUREMENTS,
            host_ms,
            decode_ms,
            arrival_ms,
            frame.enqueue_time_us as f64 / 1000.0,
            age,
            sequence,
        );
        if let Some(csv) = &mut self.timing_csv {
            let line = format!(
                "{},{:.6},{:.3},{:.6},{},{},{:.6},{},{},{},{},{}",
                frame.number,
                arrival_ms,
                host_ms,
                decode_ms,
                sequence,
                ticks,
                age,
                frame.receive_time_us,
                frame.enqueue_time_us,
                frame.presentation_time_us,
                frame.frame_type as i32,
                data.len()
            );
            match &state.record {
                // The host columns wait for the frame's report, which follows its last packet.
                Some(record) => {
                    let mut record = lock(record);
                    let row = record::PendingRow {
                        number: frame.number,
                        line,
                        rtp_timestamp: frame.rtp_timestamp,
                        clock: record.clock(frame.enqueue_time_us),
                        due_us: session::now_us() + record::ROW_DELAY_US,
                    };
                    record.push(row);
                    for row in record.take_due(session::now_us()) {
                        let _ = writeln!(csv, "{row}");
                    }
                }
                None => {
                    let _ = writeln!(csv, "{line}");
                }
            }
        }
        let frames = c.frames.fetch_add(1, Ordering::SeqCst) + 1;
        if frames < 4 {
            println!(
                "FRAME {} bytes={} type={}",
                frame.number,
                data.len(),
                frame.frame_type as i32
            );
        }
        DecodeStatus::Ok
    }

    fn cleanup(&mut self) {
        self.decoder = None;
        if let Some(csv) = &mut self.timing_csv {
            if let Some(record) = &self.state.record {
                for row in lock(record).take_due(u64::MAX) {
                    let _ = writeln!(csv, "{row}");
                }
            }
            let _ = csv.flush();
        }
        if let Some(log) = &mut self.frame_warp
            && log.csv.flush().is_err()
        {
            self.state.counters.fail();
        }
    }
}

pub(crate) struct Audio {
    pub(crate) state: Arc<State>,
    pub(crate) decoder: Option<decode::OpusDecoder>,
    pub(crate) channels: usize,
    pub(crate) csv: Option<File>,
    pub(crate) pcm: Vec<f32>,
}

impl AudioRenderer for Audio {
    fn init(&mut self, setup: &AudioSetup) -> Result<(), i32> {
        let opus = &setup.opus;
        println!(
            "AUDIO channels={} samples={}",
            opus.channel_count, setup.samples_per_frame
        );
        self.channels = usize::from(opus.channel_count);
        // The decoder writes up to 5760 samples for every channel, and 7.1.4 has twelve.
        self.pcm.resize(5760 * self.channels.max(8), 0.0);
        self.decoder = Some(decode::OpusDecoder::new(
            i32::from(opus.channel_count),
            i32::from(opus.streams),
            i32::from(opus.coupled_streams),
            &opus.mapping,
        )?);
        Ok(())
    }

    fn play(&mut self, packet: Option<&[u8]>) {
        let Some(decoder) = self.decoder.as_mut() else {
            return;
        };
        let count = decoder.decode(packet, &mut self.pcm, 5760);
        let c = &self.state.counters;
        let (mut minimum, mut maximum) = (f64::INFINITY, 0.0f64);
        if count > 0 {
            c.audio_packets.fetch_add(1, Ordering::SeqCst);
            let samples = &self.pcm[..count as usize * self.channels];
            let mut signal = lock(&self.state.audio);
            (minimum, maximum) = signal.add(samples, self.channels, self.state.options.audio_tone);
        } else {
            c.fail();
        }
        if let Some(csv) = &mut self.csv {
            let _ = writeln!(
                csv,
                "{:.6},{},{:.9},{:.9}",
                clock::ms(),
                count,
                if minimum.is_finite() {
                    minimum.sqrt()
                } else {
                    -1.0
                },
                maximum.sqrt()
            );
        }
    }

    fn cleanup(&mut self) {
        self.decoder = None;
        if let Some(csv) = &mut self.csv {
            let _ = csv.flush();
        }
    }
}

pub(crate) struct Events {
    pub(crate) state: Arc<State>,
}

pub(crate) fn hdr_metadata_valid(m: &HdrMetadata) -> bool {
    let chromaticity = |(x, y): (u16, u16)| {
        let (x, y) = (u32::from(x), u32::from(y));
        y > 0 && x <= 50000 && y <= 50000 && x + y <= 50000
    };
    m.max_display_luminance != 0
        && chromaticity(m.white_point)
        && u32::from(m.min_display_luminance) <= u32::from(m.max_display_luminance) * 10000
        && m.max_full_frame_luminance <= m.max_display_luminance
        && m.display_primaries.iter().all(|&p| chromaticity(p))
}

impl Listener for Events {
    fn stage_starting(&self, stage: Stage) {
        println!("STAGE {}", stage.name());
    }

    fn stage_failed(&self, stage: Stage, error: i32) {
        println!("FAILED {} error={error}", stage.name());
    }

    fn connection_terminated(&self, error: i32) {
        println!("TERMINATED error={error}");
        self.state.counters.ended.store(true, Ordering::SeqCst);
    }

    fn log(&self, message: &str) {
        println!("{message}");
    }

    fn control_event(&self, event: ControlEvent) {
        let (enabled, metadata) = match event {
            ControlEvent::HdrMode { enabled, metadata } => (enabled, metadata),
            ControlEvent::HostTiming(records) => {
                if let Some(record) = &self.state.record {
                    lock(record).add_reports(records);
                }
                return;
            }
            ControlEvent::LinkNotice(notice) => {
                println!("{}", link_notice_line(&notice));
                return;
            }
            ControlEvent::Roam(event) => {
                if matches!(event, RoamEvent::Roamed { .. }) {
                    self.state.counters.roamed.fetch_add(1, Ordering::SeqCst);
                }
                println!("ROAM_EVENT {event:?}");
                return;
            }
            ControlEvent::ClockSync { reply, received } => {
                if let Some(record) = &self.state.record {
                    lock(record)
                        .offset
                        .add(&reply, record::local_us_at(received));
                }
                return;
            }
            _ => return,
        };
        let c = &self.state.counters;
        let valid = metadata.as_ref().is_some_and(hdr_metadata_valid);
        let notification = c.hdr_notifications.fetch_add(1, Ordering::SeqCst) + 1;
        if enabled {
            c.hdr_enabled_notifications.fetch_add(1, Ordering::SeqCst);
        } else {
            c.hdr_disabled_notifications.fetch_add(1, Ordering::SeqCst);
        }
        let expected = self.state.options.expected_hdr_control;
        if expected >= 0 && i32::from(enabled) != expected {
            c.hdr_control_mismatches.fetch_add(1, Ordering::SeqCst);
        }
        if enabled && !valid {
            c.hdr_invalid_metadata.fetch_add(1, Ordering::SeqCst);
        }
        match metadata {
            Some(m) => println!(
                "HDR_CONTROL {{\"notification\":{notification},\"enabled\":{enabled},\"metadata_available\":true,\"metadata_valid\":{valid},\
\"primaries_xy_50000\":[[{},{}],[{},{}],[{},{}]],\"white_xy_50000\":[{},{}],\
\"maximum_nits\":{},\"minimum_10000th_nit\":{},\"max_cll_nits\":{},\"max_fall_nits\":{},\"full_frame_nits\":{}}}",
                m.display_primaries[0].0,
                m.display_primaries[0].1,
                m.display_primaries[1].0,
                m.display_primaries[1].1,
                m.display_primaries[2].0,
                m.display_primaries[2].1,
                m.white_point.0,
                m.white_point.1,
                m.max_display_luminance,
                m.min_display_luminance,
                m.max_content_light_level,
                m.max_frame_average_light_level,
                m.max_full_frame_luminance
            ),
            None => println!(
                "HDR_CONTROL {{\"notification\":{notification},\"enabled\":{enabled},\"metadata_available\":false,\"metadata_valid\":false}}"
            ),
        }
    }
}

/// The line printed for each link notice the host sends (0x5538), for the link-speed harness.
pub(crate) fn link_notice_line(notice: &LinkNotice) -> String {
    let state = match notice.state {
        LinkState::Full => "full",
        LinkState::Limited => "limited",
        LinkState::Floor => "floor",
    };
    let reason = match notice.reason {
        LinkReason::None => "none",
        LinkReason::Network => "network",
    };
    format!(
        "LINK_NOTICE {{\"state\":\"{state}\",\"reason\":\"{reason}\",\"ceiling_kbps\":{},\"current_kbps\":{}}}",
        notice.ceiling_kbps, notice.current_kbps
    )
}
