//! The parts the session drives on its own threads: the video decoder, the audio renderer
//! and the connection listener. They hand decoded pictures, notices and control events to
//! the UI thread through [`Shared`].

use std::collections::VecDeque;
use std::sync::atomic::Ordering;
use std::sync::Arc;

mod audio;
mod audio_list;
mod ffmpeg;
mod shared;

pub use audio::{audio_renderer, passthrough_codecs, Audio};
pub(super) use audio::AudioDecoder;
pub use audio_list::{GuardedList, Lockable};
pub use ffmpeg::MAC_CPU_INSTEAD;
pub(super) use ffmpeg::{backends_after, map_to_vulkan};
pub use shared::{Counters, DecodeWindow, Decoded, Events, GpuDevice, Notice, Shared};

use ffmpeg::{
    codec_of, gpu_pyrowave, log_unavailable, mac_cpu_instead, pool_waiting, recovers_with_rfi,
    stood_in_for_vaapi, Escalation, Failing, InFlight,
};
use nightjar_core::sdp::{VIDEO_FORMAT_MASK_10BIT, VIDEO_FORMAT_MASK_PYROWAVE, VIDEO_FORMAT_MASK_YUV444};
use nightjar_core::session::{self, DecodeStatus, VideoDecoder, VideoSetup};
use nightjar_core::video::Frame;
use crate::media::{self, Hardware};
use crate::strings::trf;
use super::pyrowave::Failure;

pub fn lock<L: Lockable>(mutex: &L) -> L::Guard<'_> { mutex.lock_inner() }

/// A failure of PyroWave's GPU decoding, for the dialog that ends the stream: what happened in the
/// user's language, then the device's own words.
fn pyrowave_gpu_failed(error: &dyn std::fmt::Display) -> Notice {
    Notice::DecoderFailed(format!("{}: {error}", crate::strings::tr("PyroWave GPU decoding unavailable")))
}

/// Decodes with FFmpeg on the session's decoder thread.
pub struct Decoder {
    pub shared: Arc<Shared>,
    pub hardware: Hardware,
    pub decoder: Option<media::VideoDecoder>,
    pub software_threads: i32,
    setup: Option<VideoSetup>,
    pyrowave: Option<super::pyrowave::Pyrowave>,
    early: bool,
    /// Slices per frame for the beam race (0 off, `Settings::beam_race`).
    slices: u32,
    /// The renderer's GPU device when PyroWave is to be decoded on the GPU (`pyrowave_decoder = gpu`).
    gpu: Option<GpuDevice>,
    /// The presenter imports VAAPI pictures as DMA-BUFs (`crate::dmabuf`).
    dmabuf: bool,
    /// The pictures go straight onto a display plane (`kms_scanout`), which takes 10-bit too.
    kms_planes: bool,
    /// Keeps `hardware`'s device alive while this decoder may open on it.
    _device: media::DeviceRef,
    /// Measurement: where the first `PYROWAVE_DUMP_FRAMES` envelopes go (`NIGHTJAR_PYROWAVE_DUMP`),
    /// and how many were written.
    dump: Option<(std::path::PathBuf, u32)>,
    /// Frames that failed since the last picture.
    failing: Failing,
    /// The frame being submitted failed for data lost on the way (`Failure::Loss`).
    lost: bool,
    /// A PyroWave GPU decode failed on its device since the decoder was installed: the stream
    /// that follows opens its own instead of starting on one that failed.
    pyrowave_failed: bool,
    /// Pictures from a hardware pool that may wait for the presenter ([`pool_waiting`]).
    pool_waiting: usize,
    /// The times of the frames sent to the FFmpeg decoder whose pictures have not come out, in
    /// order: an asynchronous decoder's picture belongs to an earlier frame than the one sent.
    in_flight: VecDeque<InFlight>,
    /// The FFmpeg decoder made a picture since it was opened: from then on a frame without one
    /// is a decode failure (synchronous decoders).
    pictured: bool,
    /// The decoder was taken from the stream before (`decoder_cache`): its frames pool may not
    /// fit this stream even when the announced parameters match (a coded size or profile the key
    /// cannot see), so the first failed frame opens the context again at once.
    reused: bool,
    /// The renderer generation this session's decoder was made under (`decoder_cache::generation`).
    generation: u64,
    /// A packed full chroma stream's VAAPI pictures are mapped into Vulkan: a failed import in
    /// `receive` drops the map, and the doctor learns the pictures go through memory from then.
    packed_mapped: bool,
    /// The stream's VAAPI pictures are mapped into the GPU renderer's Vulkan images: a failed
    /// import in `receive` drops the map, and Vulkan video takes the stream over where it can
    /// (`vulkan_for_unmapped`).
    vaapi_mapped: bool,
    /// The stream format Vulkan video decodes in place of a VAAPI decoder whose pictures could
    /// not be mapped: should FFmpeg take it to software, VAAPI through memory is next, not software.
    stands_in: Option<u32>,
    /// The backends FFmpeg took to software for a stream format (`VideoSetup::format`), left
    /// out when the decoder is opened again for it.
    refused: Vec<(Hardware, u32)>,
    /// When the FFmpeg decoder was installed (session clock, us): pictures ready before that are
    /// another decoder's.
    installed_us: u64,
}

impl Decoder {
    pub fn new(shared: Arc<Shared>, hardware: Hardware) -> Self {
        // Slice threads only (`thread_type`): a 4K host stream carries four slices (the host's
        // automatic slice delivery), and a timed 4K software decode gained nothing beyond four
        // threads, so the cap is the stream's slice count, not this machine's cores. More threads
        // also cannot help a one-slice 1080p stream, and frame threading would add frames of latency.
        let threads = std::thread::available_parallelism().map_or(2, |n| n.get().min(4)) as i32;
        Self {
            shared,
            hardware,
            decoder: None,
            software_threads: threads,
            setup: None,
            pyrowave: None,
            early: false,
            slices: 0,
            gpu: None,
            dmabuf: false,
            kms_planes: false,
            _device: media::DeviceRef::default(),
            dump: std::env::var_os("NIGHTJAR_PYROWAVE_DUMP").filter(|dir| !dir.is_empty()).map(|dir| (dir.into(), 0)),
            failing: Failing::default(),
            lost: false,
            pyrowave_failed: false,
            pool_waiting: media::EXTRA_HW_FRAMES as usize - media::HELD_BEYOND_WAITING,
            in_flight: VecDeque::new(),
            pictured: false,
            reused: false,
            generation: 0,
            packed_mapped: false,
            vaapi_mapped: false,
            stands_in: None,
            refused: Vec::new(),
            installed_us: 0,
        }
    }

    /// A frame failed with no picture since the last one: after [`FAILING_FOR_US`] of that the
    /// FFmpeg decoder is opened again once on its device, and when that gives no picture either,
    /// made anew with a new hardware device of its own once (not on the renderer's device, which
    /// a rebuild would not change). When it still fails, a hardware decoder that made no
    /// picture since gives way to the next decoder; otherwise, or when PyroWave fails, the stream
    /// ends saying why instead of freezing.
    fn frame_failed(&mut self, now_us: u64) {
        let frames = self.failing.frames + 1;
        let for_ms = self.failing.since_us.map_or(0, |since| now_us.saturating_sub(since) / 1000);
        let rebuildable = self.decoder.as_ref().is_some_and(|decoder| match decoder.hardware {
            Hardware::Vaapi | Hardware::Cuda | Hardware::V4l2m2m => true,
            #[cfg(target_os = "macos")]
            Hardware::VideoToolbox => true,
            Hardware::D3d11(_) if decoder.owns_d3d11_device() => true,
            _ => false,
        });
        match self.failing.failed(now_us, self.decoder.is_some(), rebuildable) {
            Escalation::Wait => {}
            Escalation::Reopen => {
                eprintln!("nightjar: the video decoder failed {frames} frames in a row over {for_ms} ms with no picture; opening it again on its device");
                let setup = self.setup.expect("decoder setup precedes frames");
                // The codec context is what a failed decoder needs anew; its hardware device (a
                // VA-API, Vulkan, D3D11VA or VideoToolbox device costs what the start did) is kept.
                if !self.reopen_on_device(now_us) {
                    eprintln!("nightjar: opening it anew");
                    self.rebuild(&setup);
                }
            }
            Escalation::Rebuild => {
                // The reopened context made no picture: the decoder's own hardware device (VA-API,
                // VideoToolbox, V4L2) may be wedged (`avcodec_open2` does not touch it, FFmpeg sets
                // it up at the first frame), and a decoder with a new one is the last try before
                // the stream ends.
                eprintln!("nightjar: the video decoder failed {frames} frames in a row over {for_ms} ms after it was opened again; making it anew with a new hardware device");
                let setup = self.setup.expect("decoder setup precedes frames");
                self.rebuild(&setup);
            }
            Escalation::GiveUp if self.never_pictured_hardware().is_some() => {
                // A hardware decoder that made no picture since it was opened again (V4L2 M2M on
                // an Odin 2 Portal that cannot set up its capture buffers, issue 77): the next
                // decoder, software after the last, takes over instead of the stream ending.
                let backend = self.never_pictured_hardware().expect("checked above");
                eprintln!("nightjar: {} failed {frames} frames in a row over {for_ms} ms after it was opened again, without a picture; trying the next decoder", backend.label());
                self.replace_refused(backend);
                // The next decoder gets its own second and its own reopen.
                self.failing = Failing::default();
            }
            Escalation::GiveUp => {
                eprintln!("nightjar: the video decoder failed {frames} frames in a row over {for_ms} ms with no picture; ending the stream");
                self.shared.notify(Notice::DecoderFailed(trf("The video decoder stopped working: no picture for {} s.", &[&for_ms.div_ceil(1000)])));
            }
        }
    }

    /// Drops the decoder, its device with it, and makes it anew; the stream ends saying why when
    /// that fails.
    fn rebuild(&mut self, setup: &VideoSetup) {
        self.decoder = None;
        if self.setup(setup).is_err() || self.decoder.is_none() {
            self.failing.given_up = true;
            self.shared.notify(Notice::DecoderFailed(crate::strings::tr("The video decoder stopped working and could not be opened again.").to_owned()));
        }
    }

    /// Opens the stream's decoder context again on the device it has; false when it cannot (the
    /// caller then tries a new decoder or lets the stream end). Drops any half-open context on Err.
    fn reopen_on_device(&mut self, now_us: u64) -> bool {
        let reopened = self.decoder.as_mut().map_or_else(|| Err("no decoder to open again".to_owned()), media::VideoDecoder::reopen);
        match reopened {
            Ok(()) => {
                self.in_flight.clear();
                self.pictured = false;
                self.installed_us = now_us;
                true
            }
            Err(error) => {
                eprintln!("nightjar: the decoder could not be opened again on its device ({error})");
                self.decoder = None;
                false
            }
        }
    }

    /// The stream's hardware decoder when it has made no picture since it was opened.
    fn never_pictured_hardware(&self) -> Option<Hardware> {
        self.decoder.as_ref().map(|decoder| decoder.hardware).filter(|&backend| backend != Hardware::None && !self.pictured)
    }

    /// Counts a submitted frame towards [`Failing`]: a picture starts over, and a frame that failed
    /// in the decoder counts. One that only lost too much of its data on the way does not.
    fn after_frame(&mut self, decoded: bool, need_idr: bool, now_us: u64) {
        if decoded {
            self.failing.decoded();
            self.reused = false;
        } else if need_idr && !self.lost {
            if self.reused {
                // The frames pool of the last stream may not fit this one even though its
                // announced parameters matched (a coded size or profile the key cannot see):
                // one frame is too much to wait, so the context is opened again at once.
                self.reused = false;
                eprintln!("nightjar: the first picture on the decoder of the last stream failed; opening its context again on the same device");
                if !self.reopen_on_device(now_us) {
                    let setup = self.setup.expect("decoder setup precedes frames");
                    self.rebuild(&setup);
                }
                return;
            }
            self.frame_failed(now_us);
        }
    }

    /// What a frame the PyroWave decoder could not turn into a picture costs the stream.
    fn failed(&mut self, failure: Failure) -> DecodeStatus {
        let counters = &self.shared.counters;
        match failure {
            Failure::Frame(error) => {
                eprintln!("nightjar: PyroWave: {error}");
                counters.decode_errors.fetch_add(1, Ordering::Relaxed);
                DecodeStatus::NeedIdr
            }
            Failure::Loss(error) => {
                eprintln!("nightjar: PyroWave: {error}");
                counters.decode_errors.fetch_add(1, Ordering::Relaxed);
                self.lost = true;
                DecodeStatus::NeedIdr
            }
            Failure::NoPlanes => {
                if counters.replaced.fetch_add(1, Ordering::Relaxed) == 0 { eprintln!("nightjar: PyroWave: every GPU plane set is waiting to be shown, dropping the frame (counted, not repeated)"); }
                DecodeStatus::Ok
            }
            Failure::Device(error) => {
                eprintln!("nightjar: PyroWave GPU decode failed: {error}");
                counters.decode_errors.fetch_add(1, Ordering::Relaxed);
                self.pyrowave_failed = true;
                self.shared.notify(pyrowave_gpu_failed(&error));
                DecodeStatus::NeedIdr
            }
        }
    }

    /// PyroWave decodes on the GPU with the SDK, into planes on this device.
    pub fn with_gpu_decoder(mut self, device: Option<GpuDevice>) -> Self {
        self.gpu = device;
        self
    }

    /// PyroWave decodes the blocks of a frame while they arrive (`Settings::early_decode`).
    pub fn with_early_decode(mut self, early: bool) -> Self {
        self.early = early;
        self
    }

    /// PyroWave decodes a row-ordered frame in `slices` slices while it arrives (0 off).
    pub fn with_beam_race(mut self, slices: u32) -> Self {
        self.slices = slices;
        self
    }

    /// VAAPI pictures go to the presenter as DMA-BUFs, which it imports into its textures.
    pub fn with_dmabuf(mut self, dmabuf: bool) -> Self {
        self.dmabuf = dmabuf;
        self
    }

    /// VAAPI pictures go straight onto a display plane as DMA-BUFs (`kms_scanout`).
    pub fn with_kms_planes(mut self, kms_planes: bool) -> Self {
        self.kms_planes = kms_planes;
        self
    }

    pub fn with_device(mut self, device: media::DeviceRef) -> Self {
        self._device = device;
        self
    }

    /// The presenter could not show this decoder's pictures (`Shared::present_refused`): a
    /// hardware decoder gives way as one FFmpeg refused does, to the next backend or software,
    /// and software ends the stream saying why. Pictures ready before this decoder was installed
    /// say nothing about it. Returns whether it acted.
    fn presenter_refused(&mut self) -> bool {
        let ready_us = self.shared.present_refused.swap(0, Ordering::Relaxed);
        if ready_us == 0 || ready_us < self.installed_us { return false; }
        let Some(backend) = self.decoder.as_ref().map(|decoder| decoder.hardware) else { return false };
        if backend == Hardware::None {
            eprintln!("nightjar: the software decoder's pictures cannot be shown either; ending the stream");
            self.shared.notify(Notice::DecoderFailed(crate::strings::tr(PICTURES_UNSHOWN).to_owned()));
        } else {
            eprintln!("nightjar: {}'s pictures cannot be shown; trying the next decoder", backend.label());
            self.replace_refused(backend);
        }
        true
    }

    /// FFmpeg took the `refused` backend to software (`media::VideoDecoder::refused`): says so,
    /// and opens the next backend of the order that opens, or the software decoder after them,
    /// so the stream never decodes in software under a hardware decoder's name, nor fails every
    /// frame (AV1) while another decoder could decode it.
    fn replace_refused(&mut self, refused: Hardware) {
        let setup = self.setup.expect("decoder setup precedes frames");
        self.refused.push((refused, setup.format));
        let codec = codec_of(&setup);
        let mut rest: Vec<_> = backends_after(self.hardware.order(), refused).into_iter().filter(|&backend| !self.refused.contains(&(backend, setup.format))).collect();
        // Vulkan video stood in for VAAPI, whose pictures could not be mapped: VAAPI decodes the
        // stream, through memory, before the decoders after both and before software.
        if stood_in_for_vaapi(refused, self.stands_in.take(), setup.format) && !self.refused.contains(&(Hardware::Vaapi, setup.format)) {
            rest.insert(0, Hardware::Vaapi);
        }
        let hardware = if rest.is_empty() {
            Err("no other hardware decoder".to_owned())
        } else {
            media::VideoDecoder::open_any(codec, self.pictures(&setup), rest, setup.chroma_packing != 0, log_unavailable)
        };
        let (opened, now) = match hardware {
            Ok(decoder) => {
                let now = trf("{} cannot decode this stream. Now decoding with {}.", &[&refused.label(), &decoder.hardware.label()]);
                (Ok(decoder), now)
            }
            Err(error) if setup.chroma_packing != 0 => {
                // The software decoder cannot unpack the layout (setup refuses it the same way).
                eprintln!("nightjar: after {} refused the packed 4:4:4 stream: {error}; no software decoder can show it", refused.label());
                self.decoder = None;
                self.shared.notify(Notice::DecoderFailed(crate::strings::tr(PACKED_NEEDS_HARDWARE).to_owned()));
                return;
            }
            Err(error) => {
                eprintln!("nightjar: after {} refused the stream: {error}", refused.label());
                let now = trf("{} cannot decode this stream. Now decoding in software.", &[&refused.label()]);
                (media::VideoDecoder::open(codec, Hardware::None, self.software_threads, false), now)
            }
        };
        self.decoder = None;
        match opened {
            Ok(decoder) => {
                self.shared.notify(Notice::DecoderFallback(now));
                self.install(decoder, &setup);
            }
            Err(error) => {
                self.shared.notify(Notice::DecoderFailed(trf("{} cannot decode this stream, and no other decoder opens: {}", &[&refused.label(), &error])));
            }
        }
    }
}

/// The toast for NVDEC pictures that go through memory to the GPU renderer because CUDA's copy into
/// its Vulkan images could not be set up or failed; the reason, in English, is in the log.
const CUDA_FALLBACK_TOAST: &str = crate::strings::en("NVDEC pictures can’t be copied into the renderer’s images on the graphics card, so each one goes through memory, which takes longer. The log says why.");

/// The end of a stream whose packed 4:4:4 ("Sharper colored text") has no hardware decoder.
const PACKED_NEEDS_HARDWARE: &str = crate::strings::en("Sharper colored text needs a hardware decoder. Choose one under Decoder in Settings, or turn off Sharper colored text, then start the stream again.");

/// The end of a stream no decoder opens for: the setup's failure, which the failed start shows.
fn no_decoder(shared: &Shared, error: &dyn std::fmt::Display) {
    eprintln!("nightjar: no video decoder opens for the stream: {error}");
    shared.notify(Notice::DecoderFailed(crate::strings::tr("No video decoder on this device can play this stream. Choose another Video codec in Settings, then start the stream again.").to_owned()));
}

/// The end of a stream whose pictures the presenter could not show from any decoder, software last.
const PICTURES_UNSHOWN: &str = crate::strings::en("This stream’s picture cannot be shown with any decoder on this device. Choose another Video codec or Decoder in Settings, then start the stream again.");

/// Says once in a toast when `decoder`'s NVDEC pictures went through memory instead of CUDA's copy.
fn say_cuda_fallback(shared: &Shared, decoder: &mut media::VideoDecoder) {
    if decoder.take_cuda_fallback().is_some() {
        shared.notify(Notice::DecoderFallback(crate::strings::tr(CUDA_FALLBACK_TOAST).to_owned()));
    }
}

impl VideoDecoder for Decoder {
    fn setup(&mut self, setup: &VideoSetup) -> Result<(), i32> {
        self.setup = Some(*setup);
        self.generation = super::decoder_cache::generation();
        self.shared.fps_millihz.store(setup.fps.max(1) as u32 * 1000, Ordering::Relaxed);
        self.pool_waiting = pool_waiting(self.shared.host_times.load(Ordering::Relaxed), self.shared.queue_limit,
            setup.fps.max(1) as u32);
        if setup.format & VIDEO_FORMAT_MASK_PYROWAVE != 0 {
            let (width, height) = (setup.width as usize, setup.height as usize);
            let chroma444 = setup.format & VIDEO_FORMAT_MASK_YUV444 != 0;
            let ten_bit = setup.format & VIDEO_FORMAT_MASK_10BIT != 0;
            let slots = {
                // The presenter holds the shown picture's plane set while it draws the planes into the back buffer.
                #[cfg(windows)]
                { super::direct_planes::plane_sets(self.shared.queue_limit) }
                #[cfg(not(windows))]
                { self.shared.queue_limit + 2 }
            };
            let key = super::decoder_cache::PyroWaveKey {
                width, height, chroma444, ten_bit,
                gpu: self.gpu.is_some(),
                device: self.gpu.map_or(0, |device| device.0 as usize),
                slots,
            };
            super::decoder_cache::only_pyrowave();
            let (created, reused) = match super::decoder_cache::take_pyrowave(key) {
                Some((mut decoder, name)) => {
                    decoder.flush();
                    eprintln!("nightjar: reusing the PyroWave decoder of the last stream for {width}x{height} ({}), flushed", if chroma444 { "4:4:4" } else { "4:2:0" });
                    (Ok((decoder, name)), true)
                }
                None => (match self.gpu {
                    Some(device) => gpu_pyrowave(device, setup, slots)
                        .map(|decoder| (decoder, if cfg!(target_os = "macos") { "PyroWave (GPU decode, SDK, KosmicKrisp, planes read back)" } else { "PyroWave (GPU decode, SDK)" }.to_owned()))
                        .or_else(|error| mac_cpu_instead(&error, width, height, chroma444).map(|(decoder, name)| (decoder, name.to_owned())).ok_or(error))
                        .map_err(|error| pyrowave_gpu_failed(&error)),
                    None => super::pyrowave::Pyrowave::cpu(width, height, chroma444)
                        .map(|decoder| (decoder, "PyroWave (Rust, GPU wavelets)".to_owned()))
                        .map_err(|error| Notice::DecoderFallback(error.into())),
                }, false),
            };
            return match created {
                Ok((mut decoder, name)) => {
                    if self.slices > 0
                        && let Err(error) = decoder.set_beam_race(self.slices) { eprintln!("nightjar: beam race: {error}"); }
                    self.pyrowave = Some(decoder);
                    if !reused && name == MAC_CPU_INSTEAD {
                        self.shared.notify(Notice::DecoderFallback(crate::strings::tr("PyroWave GPU decoding did not start on this Mac, so the processor decodes this stream.").to_owned()));
                    }
                    *lock(&self.shared.decoder_name) = name;
                    Ok(())
                }
                Err(notice) => { self.shared.notify(notice); Err(-1) }
            };
        }
        if setup.chroma_packing != 0 && self.hardware == Hardware::None {
            eprintln!("nightjar: packed 4:4:4 needs a hardware decoder, and the stream has none");
            self.shared.notify(Notice::DecoderFailed(crate::strings::tr(PACKED_NEEDS_HARDWARE).to_owned()));
            return Err(-1);
        }
        let codec = codec_of(setup);
        let pictures = self.pictures(setup);
        let order = self.hardware.order();
        let request = super::decoder_cache::MediaKey {
            codec,
            requested: self.hardware,
            opened: self.hardware,
            format: setup.format,
            width: setup.width,
            height: setup.height,
            chroma_packing: setup.chroma_packing,
            pictures,
            threads: self.software_threads,
        };
        super::decoder_cache::only_media();
        if let Some(mut decoder) = super::decoder_cache::take_media(request, &order) {
            decoder.flush();
            eprintln!("nightjar: reusing the {} decoder of the last stream for this one, flushed", decoder.name);
            self.reused = true;
            self.install(decoder, setup);
            return Ok(());
        }
        let opened = match self.hardware {
            Hardware::None => {
                media::VideoDecoder::open(codec, Hardware::None, self.software_threads, false)
            }
            _ => {
                // Backends FFmpeg took to software for this stream before are not tried again.
                let any = !order.is_empty();
                let order: Vec<_> = order.into_iter().filter(|&backend| !self.refused.contains(&(backend, setup.format))).collect();
                if any && order.is_empty() && setup.chroma_packing == 0 {
                    // Every hardware decoder gave way to software this stream, which was said then.
                    return match media::VideoDecoder::open(codec, Hardware::None, self.software_threads, false) {
                        Ok(decoder) => { self.install(decoder, setup); Ok(()) }
                        Err(e) => { no_decoder(&self.shared, &e); Err(-1) }
                    };
                }
                media::VideoDecoder::open_any(codec, pictures, order, setup.chroma_packing != 0, log_unavailable).or_else(|e| {
                    if setup.chroma_packing != 0 {
                        return Err(e);
                    }
                    eprintln!("nightjar: hardware decoding unavailable: {e}; decoding in software");
                    self.shared.notify(Notice::DecoderFallback(crate::strings::tr("Hardware decoding is unavailable for this stream. Now decoding in software.").to_owned()));
                    media::VideoDecoder::open(codec, Hardware::None, self.software_threads, false)
                })
            }
        };
        match opened {
            Ok(decoder) => {
                self.install(decoder, setup);
                Ok(())
            }
            Err(e) => {
                no_decoder(&self.shared, &e);
                Err(-1)
            }
        }
    }

    fn wants_early_chunks(&self) -> bool {
        self.early
    }

    fn recovers_with_rfi(&self) -> bool {
        recovers_with_rfi(self.decoder.as_ref().map(|decoder| decoder.hardware))
    }

    fn chunk(&mut self, first: bool, flags: u32, payload: &[u8]) {
        if let Some(decoder) = &mut self.pyrowave {
            let start = session::now_us();
            decoder.chunk(first, flags, payload);
            self.shared.push_slices(decoder.take_slices());
            self.shared.counters.early_us_total.fetch_add(session::now_us() - start, Ordering::Relaxed);
        }
    }

    /// PyroWave frames decode on their own, so one that a newer frame is already queued behind
    /// can go undecoded: a decoder slower than the stream then shows fewer frames, each of them
    /// fresh, where it would show every frame late.
    fn intra_only(&self) -> bool {
        self.pyrowave.as_ref().is_some_and(|decoder| decoder.skips_frames())
    }

    fn skip(&mut self, frame: &Frame) {
        // The frame arrived: the host's timing and the bitrate count it as they count a decoded one.
        self.shared.counters.count_host_latency(frame.host_processing_latency);
        self.shared.counters.video_bytes.fetch_add(frame.data().len() as u64, Ordering::Relaxed);
        if let Some(decoder) = &mut self.pyrowave { decoder.skip(frame.data()); }
    }

    fn submit(&mut self, frame: &Frame) -> DecodeStatus {
        let pictures = self.shared.counters.decoded.load(Ordering::Relaxed);
        self.lost = false;
        let status = self.decode_frame(frame);
        let decoded = self.shared.counters.decoded.load(Ordering::Relaxed) != pictures;
        self.after_frame(decoded, status == DecodeStatus::NeedIdr, session::now_us());
        status
    }

    fn cleanup(&mut self) {
        lock(&self.shared.frames).clear();
        self.in_flight.clear();
        if let Some(summary) = self.pyrowave.as_ref().and_then(|decoder| decoder.stage_summary()) {
            eprintln!("nightjar: PyroWave decode stages {summary}");
        }
        for line in self.pyrowave.as_ref().map(super::pyrowave::Pyrowave::gpu_report).unwrap_or_default() {
            eprintln!("nightjar: {line}");
        }
        // A reconnect follows this stream, and no decoder failure ended it: keep what is safe to
        // keep for the next session (`decoder_cache`), which flushes it before its first frame.
        // A decoder that failed since its last picture is not kept, even below the retry's
        // threshold: a stream can end into a reconnect while frames fail.
        let keep = self.shared.keep_decoder.load(Ordering::Relaxed) && self.failing == Failing::default();
        if let (true, Some(setup)) = (keep, self.setup) {
            if let Some(decoder) = self.decoder.take() {
                // V4L2 M2M's driver queues outlive avcodec_flush_buffers: a reused decoder would
                // hand the new stream a capture buffer of the last one.
                if !decoder.refused() && !decoder.asynchronous() {
                    let key = super::decoder_cache::MediaKey {
                        codec: codec_of(&setup),
                        requested: self.hardware,
                        opened: decoder.hardware,
                        format: setup.format,
                        width: setup.width,
                        height: setup.height,
                        chroma_packing: setup.chroma_packing,
                        pictures: self.pictures(&setup),
                        threads: self.software_threads,
                    };
                    eprintln!("nightjar: keeping the {} decoder for the stream that follows", decoder.name);
                    super::decoder_cache::park_media(key, decoder, self.generation);
                }
            }
            if !self.pyrowave_failed
                && let Some(decoder) = self.pyrowave.take().filter(super::pyrowave::Pyrowave::reusable) {
                    let key = super::decoder_cache::PyroWaveKey {
                        width: setup.width as usize,
                        height: setup.height as usize,
                        chroma444: setup.format & VIDEO_FORMAT_MASK_YUV444 != 0,
                        ten_bit: setup.format & VIDEO_FORMAT_MASK_10BIT != 0,
                        gpu: self.gpu.is_some(),
                        device: self.gpu.map_or(0, |device| device.0 as usize),
                        slots: {
                            #[cfg(windows)]
                            { super::direct_planes::plane_sets(self.shared.queue_limit) }
                            #[cfg(not(windows))]
                            { self.shared.queue_limit + 2 }
                        },
                    };
                    let name = lock(&self.shared.decoder_name).clone();
                    eprintln!("nightjar: keeping the PyroWave decoder for the stream that follows");
                    super::decoder_cache::park_pyrowave(key, decoder, name, self.generation);
                }
        }
        self.decoder = None;
        self.pyrowave = None;
    }
}

#[cfg(test)]
mod tests;
