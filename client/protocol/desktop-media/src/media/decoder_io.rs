//! `VideoDecoder` packet submission, zero-copy packet lending, picture reception, hardware hand-off
//! (DMA-BUF export, VAAPI/CUDA Vulkan mapping, D3D11 bridge export), and Linux decode stage timing.

use std::ffi::{c_int, c_void};
use std::ptr::{self, NonNull};

use nightjar_core::video::{FrameBytes, FRAME_PADDING};

#[cfg(target_os = "linux")]
use crate::stream::stage_times::Stages;

use super::decoder::{Received, VideoDecoder, AGAIN};
use super::ffi::*;
use super::frame::Frame;
use super::hardware::Hardware;

/// The size of the pooled packet buffers for a packet of `needed` bytes with its padding: a
/// quarter more, in whole 64 KiB, so a stream whose frames grow slowly does not remake the pool
/// for each one.
pub(super) fn packet_buffer_size(needed: usize) -> usize {
    (needed + needed / 4).next_multiple_of(64 * 1024)
}

unsafe extern "C" {
    /// libavutil/buffer.h (FFmpeg 5 and newer take the size as `size_t`); not in the bindings.
    fn av_buffer_create(
        data: *mut u8,
        size: usize,
        free: Option<unsafe extern "C" fn(*mut c_void, *mut u8)>,
        opaque: *mut c_void,
        flags: c_int,
    ) -> *mut AVBufferRef;
}

/// `AV_BUFFER_FLAG_READONLY`: FFmpeg copies a packet of such a buffer before it would write to it.
const BUFFER_READ_ONLY: c_int = 1;

// A frame's own padding covers what FFmpeg may read past a packet.
const _: () = assert!(FRAME_PADDING >= AV_INPUT_BUFFER_PADDING_SIZE as usize);

/// The free callback of a buffer `lend_packet` made: lets go of its reference to the frame's
/// bytes, on whichever thread FFmpeg releases the last packet holding it (possibly after the
/// decoder and the stream are gone; the reference owns the bytes).
unsafe extern "C" fn release_lent(opaque: *mut c_void, _data: *mut u8) {
    // SAFETY: the reference `lend_packet` turned into this opaque pointer, released once.
    drop(unsafe { FrameBytes::from_raw(opaque.cast_const()) });
}

/// An FFmpeg buffer over a frame's `bytes` that holds a reference to them, for a packet of `data`:
/// when `data` is a non-empty part of those bytes followed there by at least `padding` zero bytes
/// (the frame's padding, after any zeros `access_unit` trimmed). None otherwise, and the packet
/// is copied instead.
pub(super) fn lend_packet(bytes: &FrameBytes, data: &[u8], padding: usize) -> Option<*mut AVBufferRef> {
    let padded = bytes.padded();
    if data.is_empty() {
        return None;
    }
    let start = (data.as_ptr() as usize).checked_sub(padded.as_ptr() as usize)?;
    let end = start.checked_add(data.len())?;
    let zeros = padded.get(end..end.checked_add(padding)?)?;
    if zeros.iter().any(|&byte| byte != 0) {
        return None;
    }
    let raw = bytes.clone().into_raw();
    // SAFETY: the buffer spans the frame's bytes and padding, which the reference given as its
    // opaque pointer keeps alive and unwritten until `release_lent` lets it go. FFmpeg calls that
    // only for a buffer it made; on failure the reference is taken back here.
    let buffer = unsafe {
        av_buffer_create(padded.as_ptr().cast_mut(), padded.len(), Some(release_lent), raw.cast_mut(), BUFFER_READ_ONLY)
    };
    if buffer.is_null() {
        // SAFETY: from into_raw above; FFmpeg did not take it.
        drop(unsafe { FrameBytes::from_raw(raw) });
        return None;
    }
    Some(buffer)
}

/// An Annex B access unit without the zero bytes after its last NAL unit (trailing_zero_8bits,
/// here the last shard's padding, which no NAL unit ends with). FFmpeg's HEVC decoder reads a
/// Dolby Vision RPU, the access unit's last NAL unit, up to the packet's end, and fails on every
/// frame whose RPU the padding follows (FFmpeg 8.1: "Error parsing DOVI NAL unit"). All zeros stay
/// as they are: an empty packet would start draining the decoder.
pub(super) fn access_unit(data: &[u8]) -> &[u8] {
    let end = data.iter().rposition(|&byte| byte != 0).map_or(data.len(), |last| last + 1);
    &data[..end]
}

/// Whether a picture of `format` from a decoder opened for the hardware format `hardware` (none
/// for software and V4L2 M2M decoders) says FFmpeg went to software.
pub(super) fn refused_by(hardware: AVPixelFormat, format: AVPixelFormat) -> bool {
    hardware != AV_PIX_FMT_NONE && format != hardware
}

/// A refused decoder's name: its codec, with the backend that refused it.
pub(super) fn refused_name(name: &str, hardware: Hardware) -> String {
    let codec = name.rsplit_once(' ').map_or(name, |(codec, _)| codec);
    format!("{codec} (software, {} refused)", hardware.label())
}

impl VideoDecoder {
    /// Whether FFmpeg took this hardware decoder to software: it found no hardware format for the
    /// stream (`select_format`; AV1, which has no software path here, then fails every frame), or
    /// a picture came out that is not the backend's.
    pub fn refused(&self) -> bool {
        self.refused || self._format.software.get()
    }

    /// The decoder's new name, once, when it is no longer the backend it opened with (a hardware
    /// decoder FFmpeg took to software), for the overlay and the log.
    pub fn renamed(&mut self) -> Option<&str> {
        if !self.refused() || std::mem::replace(&mut self.told, true) {
            return None;
        }
        self.name = refused_name(&self.name, self.hardware);
        eprintln!("nightjar: FFmpeg cannot decode this stream with {} after all: {}", self.hardware.label(), self.name);
        Some(&self.name)
    }

    /// Has the decoder act as one FFmpeg took to software, for the tests of what follows.
    #[cfg(test)]
    pub fn refuse(&mut self) {
        self.refused = true;
    }

    /// Has the decoder act as one whose CUDA copy just failed, for the tests of what follows.
    // Linux and macOS (unix): test hook for Unix CUDA copy fallback path.
    #[cfg(all(test, unix))]
    pub fn fail_cuda_copy(&mut self, why: &str) {
        let mut map = Some(());
        copy_or_drop_map(&mut map, &mut self.cuda_fallback, |_| Err(why.to_owned()));
    }

    /// Whether pictures come out of it on their own time, not the frame they were sent with: the
    /// V4L2 M2M decoders answer "no picture yet" while the driver still takes input.
    pub fn asynchronous(&self) -> bool {
        self.hardware == Hardware::V4l2m2m
    }

    /// Whether its pictures hold a surface of the decoder's fixed pool until they are dropped
    /// (D3D11VA, VAAPI, Vulkan video): only [`EXTRA_HW_FRAMES`] are there beyond what the decoder
    /// itself needs. NVDEC's are copied out, and a refused decoder's are in memory.
    pub fn pooled(&self) -> bool {
        match self.hardware {
            Hardware::D3d11(_) | Hardware::Vaapi | Hardware::Vulkan(_) => !self.refused(),
            _ => false,
        }
    }

    /// Whether VAAPI or NVDEC pictures still go to the GPU renderer as Vulkan images
    /// (`map_to_vulkan`): a failed import or copy in `receive` drops the map for the rest of the
    /// stream.
    // Linux and macOS (unix): Vulkan interop maps exist on Unix.
    #[cfg(unix)]
    pub fn vulkan_mapped(&self) -> bool {
        self.vulkan_map.is_some() || self.cuda_map.is_some()
    }

    // Non-Unix platforms (Windows): VAAPI/CUDA Vulkan maps are never enabled.
    #[cfg(not(unix))]
    pub fn vulkan_mapped(&self) -> bool {
        false
    }

    /// Why NVDEC pictures went through memory to the GPU renderer instead of CUDA's copy into its
    /// Vulkan images, once: the stream says so in a toast.
    // Linux and macOS (unix): CUDA-to-Vulkan fallback diagnostic reason.
    #[cfg(unix)]
    pub fn take_cuda_fallback(&mut self) -> Option<String> {
        self.cuda_fallback.take()
    }

    // Non-Unix platforms (Windows): CUDA-to-Vulkan mapping is not used.
    #[cfg(not(unix))]
    pub fn take_cuda_fallback(&mut self) -> Option<String> {
        None
    }

    /// Hands VAAPI pictures to the GPU renderer, the SDL GPU device `gpu`, mapped into Vulkan
    /// images on its device instead of through memory (nightjar#80: 2.5 GB/s read back and
    /// uploaded at 4K100 10-bit), unless `NIGHTJAR_VAAPI_VULKAN_MAP` is 0. Says which in the log,
    /// with the reason when they keep going through memory, and returns whether they are mapped
    /// (`vulkan_mapped` after a failed import says they no longer are).
    /// An NVDEC decoder's pictures are copied by CUDA into Vulkan images of that device instead
    /// (`vulkan_video::CudaMap`), where its device was made with the interop extensions.
    /// Only a VAAPI or NVDEC decoder on Linux (and macOS's Unix tests, which build the Vulkan rig).
    // Linux and macOS (unix): Linux production path plus macOS Unix test builds.
    #[cfg(all(unix, any(target_os = "linux", test)))]
    pub fn map_to_vulkan(&mut self, gpu: *mut c_void) -> bool {
        if self.hardware == Hardware::Cuda {
            // SAFETY: As below; the codec context's device is the CUDA one this decoder opened.
            return match unsafe { crate::vulkan_video::CudaMap::new(gpu.cast(), (*self.context).hw_device_ctx) } {
                Ok(map) => {
                    eprintln!("nightjar: NVDEC pictures go to the GPU renderer as Vulkan images, copied by CUDA on the GPU");
                    self.cuda_map = Some(map);
                    true
                }
                Err(why) => {
                    eprintln!("nightjar: NVDEC pictures go through memory to the GPU renderer: {why}");
                    self.cuda_fallback = Some(why);
                    false
                }
            };
        }
        if self.hardware != Hardware::Vaapi {
            return false;
        }
        let switched_on = crate::vulkan_video::map_switched_on(std::env::var_os(crate::vulkan_video::MAP_ENVIRONMENT).as_deref());
        // SAFETY: The renderer keeps its device alive until the decoder has stopped.
        match unsafe { crate::vulkan_video::VaapiMap::new(gpu.cast(), switched_on, self.vaapi_on_renderer.clone()) } {
            Ok(map) => {
                eprintln!("nightjar: VAAPI pictures go to the GPU renderer as mapped Vulkan images");
                self.vulkan_map = Some(map);
                true
            }
            Err(why) => {
                eprintln!("nightjar: VAAPI pictures go through memory to the GPU renderer: {why}");
                false
            }
        }
    }

    /// How this decoder's pictures reach the presenter, for the `video path:` line.
    #[cfg(target_os = "linux")]
    pub fn route(&self) -> &'static str {
        match self.hardware {
            Hardware::Vaapi if self.vulkan_map.is_some() => "mapped into the renderer's Vulkan images",
            Hardware::Vaapi if self.dmabuf.is_some() => "imported as DMA-BUFs",
            Hardware::Vulkan(_) => "Vulkan images on the renderer's device",
            Hardware::D3d11(_) => "D3D11 textures on the renderer's device",
            Hardware::Cuda if self.cuda_map.is_some() => "copied by CUDA into the renderer's Vulkan images",
            Hardware::Cuda => "copied out of CUDA memory",
            Hardware::None => "decoded in memory",
            _ => "through memory",
        }
    }

    /// Whether this decoder owns a dedicated Direct3D 11 video device separate from SDL3's
    /// renderer device (so rebuilding the decoder gives it a fresh decode device).
    pub fn owns_d3d11_device(&self) -> bool {
        #[cfg(windows)]
        {
            self.d3d11_bridge.is_some()
        }
        #[cfg(not(windows))]
        false
    }

    /// Hands VAAPI pictures to the presenter as DMA-BUFs instead of CPU mappings, and says whether
    /// it does: only a VAAPI decoder on Linux.
    pub fn export_dmabuf(&mut self) -> bool {
        // Linux and macOS (unix): enables DMA-BUF export on VAAPI decoders.
        #[cfg(unix)]
        if self.hardware == Hardware::Vaapi {
            self.dmabuf = Some(Default::default());
            return true;
        }
        false
    }

    pub fn send(&mut self, data: &[u8], pts: i64) -> i32 {
        #[cfg(target_os = "linux")]
        let started = std::time::Instant::now();
        let status = self.send_packet(data, pts);
        #[cfg(target_os = "linux")]
        if let Some(stages) = self.stages.as_mut() {
            stages.add(0, started);
        }
        status
    }

    /// Lends the decoder the bytes of the frame about to be sent, or takes them back (`None`, after
    /// the frame): a `send` whose data lies in them hands FFmpeg a reference to them instead of a
    /// copy. FFmpeg may keep that reference past the call; the bytes stay valid until it lets go.
    pub fn lend(&mut self, bytes: Option<FrameBytes>) {
        self.lent = bytes;
    }

    /// Packets that went to FFmpeg by reference (`lend`) rather than copied.
    #[cfg(test)]
    pub fn lent_packets(&self) -> u64 {
        self.lent_packets
    }

    fn send_packet(&mut self, data: &[u8], pts: i64) -> i32 {
        let data = if self.trims_padding { access_unit(data) } else { data };
        let Ok(length) = i32::try_from(data.len()) else {
            return -1;
        };
        let padding = AV_INPUT_BUFFER_PADDING_SIZE as usize;
        if let Some(buffer) = self.lent.as_ref().and_then(|bytes| lend_packet(bytes, data, padding)) {
            unsafe {
                av_packet_unref(self.packet);
                let packet = &mut *self.packet;
                packet.buf = buffer;
                packet.data = data.as_ptr().cast_mut();
                packet.size = length;
                packet.pts = pts;
                let status = avcodec_send_packet(self.context, self.packet);
                // FFmpeg took its own reference if it kept the packet: ours goes now, so the
                // frame's bytes return to the pool when FFmpeg is done with them.
                av_packet_unref(self.packet);
                self.lent_packets += 1;
                return status;
            }
        }
        unsafe {
            av_packet_unref(self.packet);
            let needed = data.len() + padding;
            if needed > self.pool_size {
                // Buffers the decoder still holds go back to the old pool, which frees itself
                // once they are all back.
                av_buffer_pool_uninit(&mut self.pool);
                self.pool_size = 0;
                let size = packet_buffer_size(needed);
                self.pool = av_buffer_pool_init(size, None);
                if self.pool.is_null() {
                    return -12; // AVERROR(ENOMEM)
                }
                self.pool_size = size;
            }
            let buffer = av_buffer_pool_get(self.pool);
            if buffer.is_null() {
                return -12;
            }
            let packet = &mut *self.packet;
            packet.buf = buffer;
            packet.data = (*buffer).data;
            packet.size = length;
            ptr::copy_nonoverlapping(data.as_ptr(), packet.data, data.len());
            // The buffer can hold an earlier, longer frame, and the decoder reads into the padding.
            ptr::write_bytes(packet.data.add(data.len()), 0, padding);
            packet.pts = pts;
            avcodec_send_packet(self.context, self.packet)
        }
    }

    /// Says the input has ended (an empty packet), so the decoder hands out what it still holds:
    /// a V4L2 M2M decoder keeps a lone frame until more input or this comes.
    pub fn drain(&mut self) -> i32 {
        unsafe { avcodec_send_packet(self.context, ptr::null()) }
    }

    pub fn receive(&mut self) -> Received {
        #[cfg(target_os = "linux")]
        let started = std::time::Instant::now();
        let mut decoded = None;
        let received = self.receive_picture(&mut decoded);
        #[cfg(target_os = "linux")]
        if let Some(stages) = self.stages.as_mut() {
            time_receive(stages, &received, started, decoded);
        }
        received
    }

    /// `receive`, setting `decoded` once FFmpeg has handed over a picture.
    fn receive_picture(&mut self, decoded: &mut Option<std::time::Instant>) -> Received {
        unsafe {
            #[allow(unused_mut)]
            let mut frame = match self.spare.take() {
                Some(frame) => frame,
                None => {
                    let Some(raw) = NonNull::new(av_frame_alloc()) else {
                        return Received::Error;
                    };
                    Frame::new(raw)
                }
            };
            let raw = frame.raw;
            let status = avcodec_receive_frame(self.context, raw.as_ptr());
            if status < 0 {
                // avcodec_receive_frame unreferences the frame first, so it is empty again.
                self.spare = Some(frame);
                return if status == AGAIN || status == -541478725 {
                    Received::None
                } else {
                    Received::Error
                };
            }
            *decoded = Some(std::time::Instant::now());
            let format = (*raw.as_ptr()).format;
            self.refused |= refused_by(self._format.hardware, format);
            if format == AV_PIX_FMT_VAAPI {
                // Linux and macOS (unix): export VAAPI surface as DMA-BUF when OpenGL import is active.
                #[cfg(unix)]
                if let Some(decline) = self.dmabuf.as_ref().filter(|d| !d.load(std::sync::atomic::Ordering::Relaxed)) {
                    match crate::dmabuf::export(raw.as_ptr(), decline) {
                        Ok(export) => {
                            frame.dmabuf = Some(Box::new(export));
                            return Received::Frame(frame);
                        }
                        Err(error) => {
                            eprintln!("nightjar: VAAPI pictures cannot be exported as DMA-BUFs ({error}); mapping them to memory instead");
                            decline.store(true, std::sync::atomic::Ordering::Relaxed);
                        }
                    }
                }
                // Linux and macOS (unix): map VAAPI surface into Vulkan images for the GPU renderer.
                #[cfg(unix)]
                if let Some(map) = self.vulkan_map.as_mut() {
                    let Some(target) = NonNull::new(av_frame_alloc()) else {
                        return Received::Error;
                    };
                    let mapped = Frame::new(target);
                    match map.map(raw.as_ptr(), target.as_ptr()) {
                        Ok(()) => {
                            // The mapped picture holds the surface; the decoded frame is empty
                            // again for the next `receive`.
                            av_frame_unref(raw.as_ptr());
                            self.spare = Some(frame);
                            return Received::Frame(mapped);
                        }
                        Err(why) => {
                            eprintln!("nightjar: VAAPI pictures go through memory to the GPU renderer from now on: {why}");
                            self.vulkan_map = None;
                        }
                    }
                }
                let mapped = frame.map_to_memory();
                av_frame_unref(raw.as_ptr());
                self.spare = Some(frame);
                return mapped.map_or(Received::Error, Received::Frame);
            }
            if format == AV_PIX_FMT_CUDA {
                // Linux and macOS (unix): copy CUDA picture into Vulkan images via interop when active.
                #[cfg(unix)]
                if self.cuda_map.is_some() {
                    let Some(target) = NonNull::new(av_frame_alloc()) else {
                        return Received::Error;
                    };
                    let copied = Frame::new(target);
                    let source = raw.as_ptr();
                    if copy_or_drop_map(&mut self.cuda_map, &mut self.cuda_fallback, |map| map.copy(source, target.as_ptr())) {
                        // The copy is queued on the decoder's CUDA stream, ahead of any later
                        // use of the CUDA picture; the decoded frame is empty again.
                        av_frame_unref(raw.as_ptr());
                        self.spare = Some(frame);
                        return Received::Frame(copied);
                    }
                }
                // NVDEC's pictures are in CUDA memory, which only CUDA itself can read: copied out.
                let Some(memory) = NonNull::new(av_frame_alloc()) else {
                    av_frame_unref(raw.as_ptr());
                    self.spare = Some(frame);
                    return Received::Error;
                };
                let memory = Frame::new(memory);
                let ok = av_hwframe_transfer_data(memory.raw.as_ptr(), raw.as_ptr(), 0) >= 0
                    && av_frame_copy_props(memory.raw.as_ptr(), raw.as_ptr()) >= 0;
                av_frame_unref(raw.as_ptr());
                self.spare = Some(frame);
                return if ok { Received::Frame(memory) } else { Received::Error };
            }
            #[cfg(windows)]
            if format == AV_PIX_FMT_D3D11 && let Some(bridge) = &self.d3d11_bridge {
                match bridge.export(raw.as_ref()) {
                    Ok(shared) => { frame.shared = Some(shared); return Received::Frame(frame); }
                    Err(why) => { eprintln!("nightjar: D3D11 decode bridge export failed: {why}"); return Received::Error; }
                }
            }
            Received::Frame(frame)
        }
    }
}

/// A picture's copy with `map` (CUDA's into Vulkan images): true when it was copied. A failed copy
/// drops the map for the rest of the stream, logs why and keeps it in `fallback` for the stream's
/// toast, and the picture goes through memory; without a map nothing is copied.
// Linux and macOS (unix): CUDA map fallback helper used on Unix and unit tests.
#[cfg(any(unix, test))]
pub(super) fn copy_or_drop_map<M>(map: &mut Option<M>, fallback: &mut Option<String>, copy: impl FnOnce(&mut M) -> Result<(), String>) -> bool {
    let Some(mapper) = map.as_mut() else { return false };
    match copy(mapper) {
        Ok(()) => true,
        Err(why) => {
            eprintln!("nightjar: NVDEC pictures go through memory to the GPU renderer from now on: {why}");
            *map = None;
            *fallback = Some(why);
            false
        }
    }
}

/// Times a `receive` that `started` and got `received`, FFmpeg handing over its picture at
/// `decoded`: a picture adds FFmpeg's receive and the hand-off, a poll that finds none (EAGAIN)
/// adds nothing to the next picture, and a failed receive or hand-off drops the picture's stages.
#[cfg(target_os = "linux")]
pub(super) fn time_receive(stages: &mut Stages<3>, received: &Received, started: std::time::Instant, decoded: Option<std::time::Instant>) {
    match (received, decoded) {
        (Received::Frame(_), Some(at)) => {
            stages.add_between(1, started, at);
            stages.add(2, at);
            stages.commit();
        }
        (Received::None, _) => {}
        _ => stages.reset(),
    }
}

/// The decode stages' summary once they cover a stream's worth of pictures, not a probe's.
#[cfg(target_os = "linux")]
pub(super) fn stage_summary(stages: &Stages<3>) -> Option<String> {
    (stages.frames() >= crate::stream::stage_times::SUMMARY_FRAMES).then(|| stages.summary()).flatten()
}
