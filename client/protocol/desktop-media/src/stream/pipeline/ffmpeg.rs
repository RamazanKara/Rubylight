//! FFmpeg and PyroWave frame feeding, in-flight timestamp tracking, hardware pool sizing,
//! failure escalation state and VAAPI/Vulkan zero-copy mapping helpers.

use std::collections::VecDeque;
use std::sync::atomic::Ordering;

use nightjar_core::sdp::{VIDEO_FORMAT_MASK_10BIT, VIDEO_FORMAT_MASK_AV1, VIDEO_FORMAT_MASK_H265, VIDEO_FORMAT_MASK_YUV444};
use nightjar_core::session::{self, DecodeStatus, VideoSetup};
use nightjar_core::video::Frame;
use crate::media::{self, Codec, Hardware, Picture, Received};
use super::{lock, say_cuda_fallback, Decoded, Decoder, GpuDevice};

/// The times of a frame sent to the FFmpeg decoder, for its picture.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct InFlight {
    pub(super) number: u32,
    pub(super) receive_us: u64,
    pub(super) enqueue_us: u64,
    pub(super) input_us: u64,
    pub(super) host_latency: u16,
}

/// More frames in flight than any decoder holds: the oldest are frames that made no picture.
pub(super) const IN_FLIGHT_LIMIT: usize = 32;

/// The times of the frame `number` from `in_flight`, dropping those of earlier frames, which
/// made no picture; none when it is not there (then the picture is taken for the latest frame's).
pub(super) fn take_in_flight(in_flight: &mut VecDeque<InFlight>, number: u32) -> Option<InFlight> {
    while let Some(front) = in_flight.front() {
        // Frame numbers wrap: an earlier frame is less than half the range behind.
        let behind = number.wrapping_sub(front.number) as i32;
        if behind < 0 {
            return None;
        }
        let front = in_flight.pop_front()?;
        if behind == 0 {
            return Some(front);
        }
    }
    None
}

/// What the FFmpeg part of `decode_frame` needs of a decoder, so that a fake one stands in for
/// it in the tests.
pub(super) trait PictureDecoder {
    type Picture: DecodedPicture;
    fn send(&mut self, data: &[u8], pts: i64) -> i32;
    fn receive(&mut self) -> Received<Self::Picture>;
    fn asynchronous(&self) -> bool;
}

pub(super) trait DecodedPicture {
    fn pts(&self) -> i64;
    fn corrupt(&self) -> bool;
}

impl PictureDecoder for media::VideoDecoder {
    type Picture = media::Frame;
    fn send(&mut self, data: &[u8], pts: i64) -> i32 {
        media::VideoDecoder::send(self, data, pts)
    }
    fn receive(&mut self) -> Received {
        media::VideoDecoder::receive(self)
    }
    fn asynchronous(&self) -> bool {
        media::VideoDecoder::asynchronous(self)
    }
}

impl DecodedPicture for media::Frame {
    fn pts(&self) -> i64 {
        self.info().pts
    }
    fn corrupt(&self) -> bool {
        media::Frame::corrupt(self)
    }
}

/// What one frame did in the FFmpeg decoder ([`feed`]).
#[derive(Debug, Default, PartialEq)]
pub(super) struct FedFrame {
    /// The decoder did not take the frame, after its ready pictures were read on EAGAIN.
    pub(super) refused: bool,
    /// `avcodec_receive_frame` failed.
    pub(super) failed: bool,
    /// Pictures that came out, this frame's or (asynchronous decoders) earlier ones'.
    pub(super) pictures: u32,
    /// Of them, pictures FFmpeg flagged as damaged.
    pub(super) corrupt: u32,
    /// This frame's own picture came out.
    pub(super) own: bool,
}

/// How often an asynchronous decoder is asked again for the picture of the frame just sent.
pub(super) const ASYNC_POLL_US: u64 = 250;

/// How long an asynchronous decoder is waited for: half a frame at `fps_millihz`, so the wait
/// never holds up the next frame, and no more than 8 ms.
pub(super) fn async_wait_us(fps_millihz: u32) -> u64 {
    (500_000_000 / u64::from(fps_millihz.max(1))).min(8_000)
}

/// Sends a frame (`pts`, its number in the top 32 bits) to `decoder` and hands each picture that
/// comes out to `picture`. A decoder that says EAGAIN has its ready pictures read and gets the
/// frame again. An asynchronous decoder (V4L2 M2M) is asked again every [`ASYNC_POLL_US`]
/// (`poll` waits that long) until this frame's picture comes out, as moonlight-qt's decode loop
/// does, until `wait_us` after the send by `elapsed_us` (microseconds since the call), one
/// deadline for reading after EAGAIN and for the frame's own picture; what is still in flight
/// then comes out with a later frame.
pub(super) fn feed<D: PictureDecoder>(
    decoder: &mut D,
    data: &[u8],
    pts: i64,
    wait_us: u64,
    elapsed_us: impl Fn() -> u64,
    mut poll: impl FnMut(),
    mut picture: impl FnMut(D::Picture),
) -> FedFrame {
    let number = (pts >> 32) as u32;
    let asynchronous = decoder.asynchronous();
    let mut fed = FedFrame::default();
    let mut receive = |decoder: &mut D, fed: &mut FedFrame, done: &dyn Fn(&FedFrame) -> bool| {
        loop {
            match decoder.receive() {
                Received::Frame(out) => {
                    // Frame numbers wrap: this frame's picture or a later one.
                    fed.own |= (number.wrapping_sub((out.pts() >> 32) as u32) as i32) <= 0;
                    fed.corrupt += u32::from(out.corrupt());
                    fed.pictures += 1;
                    picture(out);
                }
                Received::None if asynchronous && !done(fed) && elapsed_us() < wait_us => poll(),
                Received::None => return,
                Received::Error => {
                    fed.failed = true;
                    return;
                }
            }
        }
    };
    let mut status = decoder.send(data, pts);
    if status == media::AGAIN {
        // Output first: what is ready goes on (one picture at least from an asynchronous
        // decoder, whose input queue is full), then the frame again.
        receive(decoder, &mut fed, &|fed| fed.pictures > 0);
        if fed.failed {
            return fed;
        }
        status = decoder.send(data, pts);
    }
    if status < 0 {
        fed.refused = true;
        return fed;
    }
    receive(decoder, &mut fed, &|fed| fed.own);
    fed
}

/// What a frame `fed` to a decoder costs the stream. A synchronous decoder (every one but V4L2
/// M2M) makes the picture of each frame it is sent, so once it has made one (`steady`), a frame
/// without a picture failed as much as a damaged picture did: the stream has no periodic key
/// frames, and without recovery the damage stays. An asynchronous one's pictures come later.
pub(super) fn frame_status(fed: &FedFrame, asynchronous: bool, steady: bool) -> DecodeStatus {
    let damaged = !asynchronous && (fed.corrupt > 0 || (steady && fed.pictures == 0));
    if fed.refused || fed.failed || damaged {
        DecodeStatus::NeedIdr
    } else {
        DecodeStatus::Ok
    }
}

/// Whether the FFmpeg decoder now decoding (`backend`, none for PyroWave) recovers with
/// reference frame invalidation: a launch asks for it only by backends that do
/// (`connect::takes_rfi`), so one a refusal brought in mid-stream (Vulkan video, V4L2 M2M,
/// NVDEC) has the rest of the stream recover with key frames.
pub(super) fn recovers_with_rfi(backend: Option<Hardware>) -> bool {
    backend.is_none_or(super::super::connect::takes_rfi)
}

/// FFmpeg took to software the Vulkan video decoder (`refused`) that stood in for a VAAPI decoder
/// whose pictures could not be mapped (`stands_in`, the stream format it did so for): VAAPI
/// decodes that stream, through memory.
pub(super) fn stood_in_for_vaapi(refused: Hardware, stands_in: Option<u32>, format: u32) -> bool {
    matches!(refused, Hardware::Vulkan(_)) && stands_in == Some(format)
}

/// The backends of `order` after `refused`, which FFmpeg took to software: none when it is not
/// in the order.
pub(in crate::stream) fn backends_after(order: Vec<Hardware>, refused: Hardware) -> Vec<Hardware> {
    match order.iter().position(|&backend| backend == refused) {
        Some(at) => order[at + 1..].to_vec(),
        None => Vec::new(),
    }
}

/// A stream's decoder, for whether Vulkan video takes it over ([`vulkan_for_unmapped`]).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Unmapped {
    /// The stream's decoder is VAAPI's.
    pub vaapi: bool,
    /// Its pictures reach the presenter without crossing memory (mapped, or as DMA-BUFs).
    pub zero_copy: bool,
    /// Packed full chroma, which Vulkan video leaves to the next decoder.
    pub packed: bool,
    /// `NIGHTJAR_VAAPI_VULKAN_MAP` is not 0.
    pub map_switched_on: bool,
    /// `NIGHTJAR_UNMAPPED_VULKAN_VIDEO`, where it is set.
    pub switch: Option<bool>,
}

/// The Vulkan video backend to open in place of a VAAPI decoder whose pictures would be read back
/// and uploaded, one by one (nightjar#80: 2.5 GB/s at 4K100 10-bit; on an Iris Plus 655 a 4K
/// 10-bit picture took 40 ms to decode and map and 9.6 ms to draw that way, against 1.3 ms and
/// 2.4 ms with Vulkan video, rust/PERFORMANCE.md, November 12):
/// the GPU renderer's, where the stream's `order` has it after VAAPI (so not where it was tried
/// first, nor without the GPU renderer, whose device is null) and FFmpeg did not take it to
/// software for this stream before (`refused`). Vulkan video decodes into images of the renderer's
/// device, so nothing is copied; it asks for a key frame where VAAPI invalidates reference frames,
/// unless "Vulkan video recovery" is on. `NIGHTJAR_VAAPI_VULKAN_MAP=0` alone asks for memory.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(super) fn vulkan_for_unmapped(now: Unmapped, order: Vec<Hardware>, refused: impl Fn(Hardware) -> bool) -> Option<Hardware> {
    let asked_for_memory = now.switch == Some(false) || (!now.map_switched_on && now.switch != Some(true));
    if !now.vaapi || now.zero_copy || now.packed || asked_for_memory {
        return None;
    }
    backends_after(order, Hardware::Vaapi).into_iter().find(|&backend| matches!(backend, Hardware::Vulkan(gpu) if !gpu.is_null()) && !refused(backend))
}

/// How long frames may fail with no picture between before the decoder is taken for broken.
/// After a GPU reset or a lost device a hardware decoder fails every frame, and each failed
/// frame only asks for the next IDR, so the picture would freeze without a word.
pub(super) const FAILING_FOR_US: u64 = 1_000_000;
/// And how many frames, so that one bad frame and the next one after a pause are not enough.
pub(super) const FAILING_FRAMES: u32 = 3;

/// The frames that failed since the decoder last made a picture.
#[derive(Debug, Default, PartialEq)]
pub(super) struct Failing {
    pub(super) since_us: Option<u64>,
    pub(super) frames: u32,
    /// The decoder was opened again in this run of failures.
    pub(super) reopened: bool,
    /// And, when that gave no picture either, made anew on a new device.
    pub(super) rebuilt: bool,
    /// The stream was told to end; nothing more is said.
    pub(super) given_up: bool,
}

/// What a decoder that failed again has to do.
#[derive(Debug, PartialEq)]
pub(super) enum Escalation {
    /// Ask for an IDR as usual.
    Wait,
    /// Open the decoder again (once), on the device it has.
    Reopen,
    /// Make the decoder anew, with a new hardware device of its own (once, after the reopen gave
    /// no picture): opening a codec context does not touch a hardware device, so only this tells
    /// a wedged one. Not for a backend on the renderer's own device (D3D11VA, Vulkan video): a
    /// new decoder would take the same one.
    Rebuild,
    /// End the stream: the decoder failed after it was opened again, or cannot be.
    GiveUp,
}

impl Failing {
    pub(super) fn decoded(&mut self) {
        *self = Self::default();
    }

    pub(super) fn failed(&mut self, now_us: u64, reopenable: bool, rebuildable: bool) -> Escalation {
        if self.given_up {
            return Escalation::Wait;
        }
        let since = *self.since_us.get_or_insert(now_us);
        self.frames += 1;
        if self.frames < FAILING_FRAMES || now_us.saturating_sub(since) < FAILING_FOR_US {
            return Escalation::Wait;
        }
        if reopenable && !self.reopened {
            *self = Self { reopened: true, ..Self::default() };
            return Escalation::Reopen;
        }
        if reopenable && rebuildable && !self.rebuilt {
            *self = Self { reopened: true, rebuilt: true, ..Self::default() };
            return Escalation::Rebuild;
        }
        self.given_up = true;
        Escalation::GiveUp
    }
}

/// Pictures from a hardware decoder's pool that may wait for the presenter, which the pool gets
/// spare surfaces for ([`media::pool_extra`]). Pacing by host times, the pacer holds every
/// picture until due: the clock raises its hold up to `max_hold_us` (50 ms) on a lateness spike
/// and lowers it over about half a second, 13 pictures at 240 FPS, and the presenter's queue
/// holds up to `queue_limit` more. Otherwise a picture waits a frame at most.
pub(super) fn pool_waiting(host_times: bool, queue_limit: usize, fps: u32) -> usize {
    let usual = media::EXTRA_HW_FRAMES as usize - media::HELD_BEYOND_WAITING;
    if !host_times {
        return usual;
    }
    let frame_us = (1_000_000 / i64::from(fps.max(1))).max(1);
    let max_hold_us = rubylight_protocol::present_clock::Settings::default().max_hold_us.max(0);
    let held = (max_hold_us + frame_us - 1) / frame_us + 1;
    (held as usize + queue_limit).max(usual)
}

/// How many real PyroWave envelopes `NIGHTJAR_PYROWAVE_DUMP` keeps.
pub(super) const PYROWAVE_DUMP_FRAMES: u32 = 120;

/// Whether a stream's VAAPI pictures go to the presenter as DMA-BUFs: it imports them, and they
/// are what its import takes (SDL's NV12 texture: 8-bit 4:2:0, not packed). A display plane
/// (`kms_planes`) takes them as they are, P010 included.
pub(super) fn exports_dmabuf(presenter_imports: bool, kms_planes: bool, setup: &VideoSetup) -> bool {
    let ten_bit = setup.format & nightjar_core::sdp::VIDEO_FORMAT_MASK_10BIT != 0;
    (presenter_imports || kms_planes)
        && (kms_planes || !ten_bit)
        && setup.format & VIDEO_FORMAT_MASK_YUV444 == 0
        && setup.chroma_packing == 0
}

/// A packed full chroma stream's coded size where its pictures are rebuilt from memory: on Linux
/// they never go as DMA-BUFs (`exports_dmabuf`), so unless they reach the GPU renderer as Vulkan
/// images (`on_gpu`: mapped VAAPI pictures or Vulkan video's), every picture, twice the stream's
/// height, is copied through the CPU (#80: 3840x4320 at 100 fps fell behind).
pub(super) fn packed_from_memory(setup: &VideoSetup, on_gpu: bool) -> Option<(i32, i32)> {
    // Linux/macOS: packed 4:4:4 pictures go through memory when not decoded on the GPU.
    (cfg!(unix) && setup.chroma_packing != 0 && !on_gpu).then_some((setup.width, setup.height))
}

/// The coded size a packed stream's pictures go through memory at once a picture's Vulkan import
/// failed: the decoder dropped the map it had at install (`was_mapped`, not `still_mapped`).
pub(super) fn packed_map_dropped(setup: &VideoSetup, was_mapped: bool, still_mapped: bool) -> Option<(i32, i32)> {
    if was_mapped && !still_mapped { packed_from_memory(setup, false) } else { None }
}

/// Maps `decoder`'s VAAPI pictures into the GPU renderer's Vulkan images when it draws with SDL's
/// GPU API (`media::VideoDecoder::map_to_vulkan`); whether they are.
pub(in crate::stream) fn map_to_vulkan(hardware: Hardware, decoder: &mut media::VideoDecoder) -> bool {
    #[cfg(target_os = "linux")]
    if let Hardware::Auto(gpu, _) = hardware {
        return !gpu.is_null() && decoder.map_to_vulkan(gpu);
    }
    let _ = (hardware, decoder);
    false
}

pub(super) fn codec_of(setup: &VideoSetup) -> Codec {
    if setup.format & VIDEO_FORMAT_MASK_AV1 != 0 {
        Codec::Av1
    } else if setup.format & VIDEO_FORMAT_MASK_H265 != 0 {
        Codec::Hevc
    } else {
        Codec::H264
    }
}

/// Each backend that does not open is logged as the next one is tried.
pub(super) fn log_unavailable(backend: Hardware, error: &str) {
    eprintln!("nightjar: {} decoding is unavailable: {error}", backend.label());
}

/// The name the CPU decoder takes on a Mac whose GPU decoder did not start for this stream.
pub const MAC_CPU_INSTEAD: &str = "PyroWave (Rust, GPU wavelets; the GPU decoder did not start)";

/// macOS: the CPU decoder in place of a GPU decoder that did not start (the bundled driver or the
/// SDK refused this stream's picture), said in the log; elsewhere none, and the stream ends with
/// the GPU decoder's error as before.
pub(super) fn mac_cpu_instead(error: &str, width: usize, height: usize, chroma444: bool) -> Option<(super::super::pyrowave::Pyrowave, &'static str)> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    eprintln!("nightjar: PyroWave GPU decoding did not start ({error}); the processor decodes this stream");
    super::super::pyrowave::Pyrowave::cpu(width, height, chroma444).ok().map(|decoder| (decoder, MAC_CPU_INSTEAD))
}

/// The SDK's decoder (on the renderer's adapter on Windows), with a plane set per picture that can be waiting.
pub(super) fn gpu_pyrowave(device: GpuDevice, setup: &VideoSetup, slots: usize) -> Result<super::super::pyrowave::Pyrowave, String> {
    #[cfg(windows)]
    {
        use windows::core::Interface;
        // SAFETY: The renderer owns the device for the whole stream; this only borrows it to AddRef.
        let device = unsafe { windows::Win32::Graphics::Direct3D11::ID3D11Device::from_raw_borrowed(&device.0) }
            .ok_or("the renderer has no Direct3D 11 device")?;
        let ten_bit = setup.format & nightjar_core::sdp::VIDEO_FORMAT_MASK_10BIT != 0;
        let chroma444 = setup.format & VIDEO_FORMAT_MASK_YUV444 != 0;
        super::super::pyrowave::GpuStream::new(device, setup.width as usize, setup.height as usize, chroma444, ten_bit, slots)
            .map(|stream| super::super::pyrowave::Pyrowave::Gpu(Box::new(stream)))
    }
    // The SDK decodes on a Vulkan device of its own on the bundled driver and reads its planes back
    // for the Metal renderer (`GpuStream::new`); the renderer's device is not Vulkan.
    #[cfg(target_os = "macos")]
    {
        let _ = device;
        let ten_bit = setup.format & nightjar_core::sdp::VIDEO_FORMAT_MASK_10BIT != 0;
        let chroma444 = setup.format & VIDEO_FORMAT_MASK_YUV444 != 0;
        super::super::pyrowave::GpuStream::new(setup.width as usize, setup.height as usize, chroma444, ten_bit, slots)
            .map(|stream| super::super::pyrowave::Pyrowave::Gpu(Box::new(stream)))
    }
    #[cfg(target_os = "linux")]
    {
        // The SDK decodes on the renderer's Vulkan device where it can share it, else on its own
        // with the planes crossing through CPU memory (`GpuDecoder::for_renderer`).
        let ten_bit = setup.format & nightjar_core::sdp::VIDEO_FORMAT_MASK_10BIT != 0;
        let chroma444 = setup.format & VIDEO_FORMAT_MASK_YUV444 != 0;
        super::super::pyrowave::GpuStream::new(device.0, setup.width as usize, setup.height as usize, chroma444, ten_bit, slots)
            .map(|stream| super::super::pyrowave::Pyrowave::Gpu(Box::new(stream)))
    }
}

impl Decoder {
    /// Writes a PyroWave envelope as `frame-<w>x<h>-<420|444>-<n>.bin`, which
    /// `cpu_costs_of_a_pyrowave_frame` reads back through `NIGHTJAR_PYROWAVE_FRAMES`.
    fn dump_envelope(&mut self, setup: &VideoSetup, data: &[u8]) {
        let Some((dir, written)) = &mut self.dump else { return };
        if *written >= PYROWAVE_DUMP_FRAMES { return; }
        let path = dir.join(super::super::pyrowave::dump_name(setup.width as usize, setup.height as usize, setup.format & VIDEO_FORMAT_MASK_YUV444 != 0, *written));
        let result = std::fs::create_dir_all(&*dir).and_then(|_| std::fs::write(&path, data));
        match result {
            Ok(()) => *written += 1,
            Err(e) => { eprintln!("nightjar: PyroWave dump to {} failed: {e}", path.display()); *written = PYROWAVE_DUMP_FRAMES; }
        }
        if *written == PYROWAVE_DUMP_FRAMES { eprintln!("nightjar: PyroWave dump: {PYROWAVE_DUMP_FRAMES} envelopes in {}", dir.display()); }
    }

    pub(super) fn pictures(&self, setup: &VideoSetup) -> media::Pictures {
        media::Pictures {
            ten_bit: setup.format & VIDEO_FORMAT_MASK_10BIT != 0,
            chroma444: setup.format & VIDEO_FORMAT_MASK_YUV444 != 0,
            packed: setup.chroma_packing != 0,
            waiting: self.pool_waiting,
            // The device the probes ask, the packed one too: the GPU renderer's when it maps pictures.
            vaapi_node: media::vaapi_node(self.hardware),
            width: setup.width.max(0) as u32,
            height: setup.height.max(0) as u32,
            probe_only: false,
        }
    }

    /// Vulkan video opened for a stream whose VAAPI pictures cannot be mapped into the GPU
    /// renderer's Vulkan images (`vulkan_for_unmapped`), or None: VAAPI keeps the stream then, its
    /// pictures through memory. `zero_copy`: they reach the presenter without crossing memory.
    #[cfg(target_os = "linux")]
    fn vulkan_instead(&mut self, vaapi: bool, zero_copy: bool, setup: &VideoSetup) -> Option<media::VideoDecoder> {
        // The order is asked for only where it can matter: it reads the VA driver's name again.
        if !vaapi || zero_copy || !matches!(self.hardware, Hardware::Auto(gpu, _) if !gpu.is_null()) {
            return None;
        }
        let now = Unmapped {
            vaapi,
            zero_copy,
            packed: setup.chroma_packing != 0,
            map_switched_on: crate::vulkan_video::map_switched_on(std::env::var_os(crate::vulkan_video::MAP_ENVIRONMENT).as_deref()),
            switch: super::super::env_flag(crate::vulkan_video::UNMAPPED_ENVIRONMENT),
        };
        let vulkan = vulkan_for_unmapped(now, self.hardware.order(), |backend| self.refused.contains(&(backend, setup.format)))?;
        match media::VideoDecoder::open_any(codec_of(setup), self.pictures(setup), vec![vulkan], false, log_unavailable) {
            Ok(decoder) => {
                eprintln!("nightjar: VAAPI pictures cannot be mapped into the renderer's Vulkan images: Vulkan video decodes the stream on the renderer's device instead, so no picture is copied through memory");
                self.stands_in = Some(setup.format);
                Some(decoder)
            }
            Err(_) => {
                eprintln!("nightjar: Vulkan video does not decode this stream either: VAAPI keeps it, each picture copied through memory");
                None
            }
        }
    }

    /// Only Linux maps VAAPI pictures and decodes with Vulkan video.
    #[cfg(not(target_os = "linux"))]
    fn vulkan_instead(&mut self, _vaapi: bool, _zero_copy: bool, _setup: &VideoSetup) -> Option<media::VideoDecoder> {
        None
    }

    /// Makes `decoder` the stream's FFmpeg decoder.
    pub(super) fn install(&mut self, mut decoder: media::VideoDecoder, setup: &VideoSetup) {
        let exported = exports_dmabuf(self.dmabuf, self.kms_planes, setup) && decoder.export_dmabuf();
        let mut mapped = if exported {
            eprintln!("nightjar: VAAPI pictures go to the renderer as DMA-BUFs");
            false
        } else {
            map_to_vulkan(self.hardware, &mut decoder)
        };
        if let Some(vulkan) = self.vulkan_instead(decoder.hardware == Hardware::Vaapi, exported || mapped, setup) {
            decoder = vulkan;
            mapped = false;
        }
        self.vaapi_mapped = mapped && decoder.hardware == Hardware::Vaapi;
        say_cuda_fallback(&self.shared, &mut decoder);
        // The overlay's decoder name, in the log for runs nobody watches.
        eprintln!("nightjar: video decoder {}", decoder.name);
        #[cfg(target_os = "linux")]
        eprintln!("nightjar: video path: {}, pictures {}", decoder.hardware.label(), decoder.route());
        *lock(&self.shared.decoder_name) = decoder.name.clone();
        let from_memory = packed_from_memory(setup, mapped || matches!(decoder.hardware, Hardware::Vulkan(_)));
        if let Some((width, height)) = from_memory {
            eprintln!("nightjar: full chroma (packed 4:4:4): each {width}x{height} picture is copied through memory and rebuilt on the GPU");
        } else if setup.chroma_packing != 0 {
            eprintln!("nightjar: full chroma (packed 4:4:4): pictures are rebuilt on the GPU from its Vulkan images");
        }
        *lock(&self.shared.packed_from_memory) = from_memory;
        self.packed_mapped = setup.chroma_packing != 0 && mapped;
        self.decoder = Some(decoder);
        self.in_flight.clear();
        self.pictured = false;
        self.installed_us = session::now_us();
    }

    /// Decodes `frame`, PyroWave or FFmpeg, and queues its picture for the presenter.
    pub(super) fn decode_frame(&mut self, frame: &Frame) -> DecodeStatus {
        self.shared.counters.count_host_latency(frame.host_processing_latency);
        self.shared.counters.video_bytes.fetch_add(frame.data().len() as u64, Ordering::Relaxed);
        if self.pyrowave.is_some() {
            let start = session::now_us();
            let setup = self.setup.expect("decoder setup precedes frames");
            if self.dump.is_some() { self.dump_envelope(&setup, frame.data()); }
            let Some(decoder) = self.pyrowave.as_mut() else { return DecodeStatus::Ok };
            let shared = &self.shared;
            // The presenter's queue and the pacer hold plane sets: when none is free the oldest picture goes.
            let mut evict = || shared.evict_oldest();
            let result = decoder.frame(frame.data(), &mut evict);
            // Counted for frames that failed too, as before the GPU decoder.
            let early = self.shared.count_source(decoder.source());
            let output = match result {
                Ok(output) => output,
                Err(failure) => return self.failed(failure),
            };
            let ready_us = session::now_us();
            let counters = &self.shared.counters;
            counters.decoded.fetch_add(1, Ordering::Relaxed);
            counters.decode_us_total.fetch_add(ready_us - start, Ordering::Relaxed);
            let mut timing = lock(&self.shared.decode_window);
            timing.samples += 1;
            timing.queue_us += start.saturating_sub(frame.enqueue_time_us);
            if output.loss_percent >= 0.0 {
                timing.record_loss_milli += (output.loss_percent * 1000.0) as u64;
                timing.record_samples += 1;
            }
            drop(timing);
            self.shared.count_race(&output.race);
            let decoded = Decoded { picture: output.picture, setup, rtp: frame.rtp_timestamp, number: frame.number,
                receive_us: frame.receive_time_us,
                enqueue_us: frame.enqueue_time_us, input_us: start, ready_us, host_latency: frame.host_processing_latency,
                early, first_block_us: output.first_block_us, race: output.race, bands: output.bands, race_seq: output.race_seq, visible: None };
            self.shared.queue(decoded);
            return DecodeStatus::Ok;
        }
        // The next decoder starts from a key frame.
        if self.presenter_refused() { return DecodeStatus::NeedIdr; }
        let Some(decoder) = self.decoder.as_mut() else {
            return DecodeStatus::Ok;
        };
        let started = session::now_us();
        let pts = (i64::from(frame.number) << 32) | i64::from(frame.rtp_timestamp);
        let sent = InFlight {
            number: frame.number,
            receive_us: frame.receive_time_us,
            enqueue_us: frame.enqueue_time_us,
            input_us: started,
            host_latency: frame.host_processing_latency,
        };
        if self.in_flight.len() >= IN_FLIGHT_LIMIT {
            self.in_flight.pop_front();
        }
        self.in_flight.push_back(sent);
        let setup = self.setup.expect("decoder setup precedes frames");
        let asynchronous = decoder.asynchronous();
        // Pictures before the first of a refused decoder are its pool's; the first is in memory
        // and goes to the queue's pool limit too, which only evicts sooner.
        let pooled = decoder.pooled();
        let fps_millihz = self.shared.fps_millihz.load(Ordering::Relaxed).max(1);
        let (shared, in_flight, pool_waiting) = (&self.shared, &mut self.in_flight, self.pool_waiting);
        let sent_at = std::time::Instant::now();
        // FFmpeg takes the frame's bytes by reference, not copied; ours goes back after the frame,
        // so they return to the receiver's pool once FFmpeg lets go too.
        decoder.lend(Some(frame.shared_data()));
        let fed = feed(decoder, frame.data(), pts, async_wait_us(fps_millihz),
            || sent_at.elapsed().as_micros() as u64,
            || std::thread::sleep(std::time::Duration::from_micros(ASYNC_POLL_US)),
            |picture| {
                let ready = session::now_us();
                // An asynchronous decoder's picture is an earlier frame's: its own times, by pts.
                let pts = picture.info().pts;
                let own = take_in_flight(in_flight, (pts >> 32) as u32).unwrap_or(sent);
                let counters = &shared.counters;
                counters.decoded.fetch_add(1, Ordering::Relaxed);
                counters.decode_us_total.fetch_add(ready.saturating_sub(own.input_us), Ordering::Relaxed);
                let mut timing = lock(&shared.decode_window);
                if timing.samples == 0 { timing.first_us = ready; }
                timing.last_us = ready;
                timing.samples += 1;
                timing.queue_us += own.input_us.saturating_sub(own.enqueue_us);
                let budget_us = 2_000_000_000 / u64::from(fps_millihz);
                timing.late += u32::from(ready.saturating_sub(own.enqueue_us) >= budget_us);
                drop(timing);
                let decoded = Decoded {
                    picture: Picture::Frame(picture),
                    setup,
                    rtp: pts as u32,
                    number: own.number,
                    receive_us: own.receive_us,
                    enqueue_us: own.enqueue_us,
                    input_us: own.input_us,
                    ready_us: ready,
                    host_latency: own.host_latency,
                    early: 0,
                    first_block_us: 0,
                    race: Default::default(),
                    bands: Vec::new(),
                    race_seq: 0,
                    visible: None,
                };
                if pooled { shared.queue_pooled(decoded, pool_waiting) } else { shared.queue(decoded) }
            });
        decoder.lend(None);
        say_cuda_fallback(&self.shared, decoder);
        if let Some((width, height)) = packed_map_dropped(&setup, self.packed_mapped, decoder.vulkan_mapped()) {
            eprintln!("nightjar: full chroma (packed 4:4:4): the Vulkan map failed, so each {width}x{height} picture is copied through memory from now on");
            *lock(&self.shared.packed_from_memory) = Some((width, height));
            self.packed_mapped = false;
        }
        let status = frame_status(&fed, asynchronous, self.pictured);
        self.pictured |= fed.pictures > 0;
        if status == DecodeStatus::NeedIdr {
            self.shared.counters.decode_errors.fetch_add(1, Ordering::Relaxed);
            if !(fed.refused || fed.failed) {
                eprintln!("nightjar: frame {} {}; asking for a key frame", frame.number,
                    if fed.corrupt > 0 { "decoded damaged" } else { "made no picture" });
            }
        }
        // FFmpeg took a hardware decoder to software: said, and the next decoder opened, which
        // starts from a key frame.
        let backend = decoder.hardware;
        let unmapped = self.vaapi_mapped && !decoder.vulkan_mapped();
        if let Some(name) = decoder.renamed().map(str::to_owned) {
            *lock(&self.shared.decoder_name) = name;
            self.replace_refused(backend);
            return DecodeStatus::NeedIdr;
        }
        // A picture's import failed and the map is gone: Vulkan video takes the stream over where
        // it can, from a key frame, before every picture goes through memory.
        if unmapped {
            self.vaapi_mapped = false;
            if let Some(vulkan) = self.vulkan_instead(true, false, &setup) {
                self.decoder = None;
                self.install(vulkan, &setup);
                return DecodeStatus::NeedIdr;
            }
        }
        status
    }
}
