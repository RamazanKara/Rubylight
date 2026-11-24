//! Linux hardware decoding with FFmpeg's Vulkan video on the GPU renderer's own Vulkan device,
//! for GPUs without VAAPI (NVIDIA's driver among them), and the presenter's GPU copy of its
//! pictures.
//!
//! Ported from moonlight-qt's plvk.cpp as Moonlight V+ ships it: the renderer's device is handed
//! to FFmpeg as an `AVVulkanDeviceContext` (instance, physical and logical device, the features
//! and device extensions it was made with, every queue family it has queues in, the decode
//! families with their codec operations), FFmpeg's `lock_queue` / `unlock_queue` take SDL's queue
//! lock for every queue (SDL waits for the whole device under it, and Vulkan wants every queue
//! left alone for that wait), and a picture is used only between the frames context's `lock_frame` and
//! `unlock_frame`, after a wait on its timeline semaphore at `sem_value`, which the use signals
//! one higher and leaves in the frame with the layout it left the image in (libplacebo's
//! `pl_vulkan_release_ex` / `pl_vulkan_hold_ex`, which plvk.cpp reaches through
//! `pl_map_avframe_ex`). Where plvk.cpp makes the device with libplacebo (`pl_vulkan_create`
//! with the video extensions and every queue) and wraps the planes for it (`pl_vulkan_wrap`),
//! the patched SDL makes the renderer's device with the video extensions the renderer asks for
//! (`zero_copy::RendererRequest`) and one more queue in each video decode family
//! (`vendor/sdl3-src/patches/vulkan-interop.patch`), and the presenter copies each picture's two
//! planes on the GPU into images of its own that the plane shader draws (`Planes`,
//! `stream::planes_gpu`): SDL can only wrap single-plane color images, the copy hands the picture
//! back to the decoder at once, and the plane shader's colour handling (range, matrix, 10-bit
//! SDR, HDR10, HLG) is the one software-decoded pictures get.
//!
//! What cannot be done is said, never skipped silently: the decoder tries the next backend with
//! the reason in the log (`device_context`, which asks the driver for the stream's profile first),
//! and a picture the presenter cannot copy is dropped with the reason in the log and on the
//! overlay.

use std::ffi::CStr;
#[cfg(target_os = "linux")]
use std::ffi::{c_int, c_void};
#[cfg(any(target_os = "linux", test))]
use std::ptr;

#[cfg(any(target_os = "linux", test))]
use ash::vk::Handle;
use ash::vk;
#[cfg(any(target_os = "linux", test))]
use sdl3_sys::everything::*;

#[cfg(target_os = "linux")]
use crate::media::ffi::{av_frame_clone, av_frame_unref, AVBufferRef, AVFrame};
#[cfg(any(target_os = "linux", test))]
use crate::media::ffi::{AVHWFramesContext, AV_PIX_FMT_VULKAN};
#[cfg(test)]
use crate::media::ffi::{
    av_buffer_unref, AV_PIX_FMT_NV12, AV_PIX_FMT_NV24, AV_PIX_FMT_P010LE, AV_PIX_FMT_RGBA,
    AV_PIX_FMT_VUYA, AV_PIX_FMT_VUYX, AV_PIX_FMT_XV30LE,
};
use crate::media::Codec;
#[cfg(test)]
use crate::media::Pictures;
#[cfg(target_os = "linux")]
use crate::stream::pyrowave_sdk::zero_copy::{self, SdlVulkan, TEXTURE_IMAGE_PROPERTY};
#[cfg(test)]
use crate::stream::pyrowave_sdk::zero_copy::Chain;

mod abi;
mod device;
/// `VaapiMap`'s imports of a decoder's surfaces, one per surface, kept while the pool lasts.
#[cfg(target_os = "linux")]
mod imports;
mod planes;
#[cfg(target_os = "linux")]
mod sample;
mod sample_state;
mod vaapi_cuda;

pub use abi::ffmpeg_supported;
pub use device::{
    device_context, frames_context, interleaved_444, map_device_context, map_node, map_plan,
    render_minor, sw_format,
};
pub use planes::Planes;
pub use sample_state::{
    describe_route, mapped_held, route, sample_switched_on, Eligibility, Route, SampleGpu, Sampled,
    MAPPED_HELD, SAMPLED_HELD, SAMPLE_ENVIRONMENT,
};
pub use vaapi_cuda::{cuda_plan, same_gpu, CudaMap, VaapiMap};
// Unix (Linux and macOS): `stream::vulkan_tests_unix` reads these test counters.
#[cfg(test)]
#[cfg_attr(not(unix), allow(unused_imports))]
pub(crate) use vaapi_cuda::{mapped_pictures, MAPPED_PICTURES};

// Re-exports for existing `imports`, `sample`, `tests`, and `sample_tests` submodules (`use super::*`).
#[cfg(target_os = "linux")]
use abi::{frame_fields, frame_now, FrameNow};
#[cfg(any(target_os = "linux", test))]
use abi::{FramesContext, StructLayout, VkFrame};
#[cfg(test)]
use abi::{
    DeviceContext60, DeviceContext61, DrmFrameDescriptor, DrmLayer, QueueFamily, VkFrame60,
    VkFrame61,
};
#[cfg(any(target_os = "linux", test))]
use device::picture_sw_format;
#[cfg(test)]
use device::{
    depth_and_chroma, interleaved_sw_format, listed_families, lock_of, mappable, plane_format,
    render_node_minor, Opaque,
};
#[cfg(target_os = "linux")]
use sample_state::{HeldFrame, COPY_WAIT, SLOTS};
#[cfg(any(target_os = "linux", test))]
use sample_state::acquire;
#[cfg(test)]
use sample_state::{after_copy, release, Held};
#[cfg(target_os = "linux")]
use vaapi_cuda::DRM_FORMAT_MOD_INVALID;
#[cfg(any(target_os = "linux", test))]
use vaapi_cuda::{
    import_interleaved_layer, import_layers, import_memory_type, layer_extent, layer_layout,
    mapped_frames, ready_for_reuse,
};
#[cfg(test)]
use vaapi_cuda::{
    cuda_uuid, drm_alias, fourcc, interop_frames, layer_format, DISABLE_MULTIPLANE,
    DRM_FORMAT_GR1616, DRM_FORMAT_R16, MAPPED,
};

/// Set to 1, Vulkan video is tried before VAAPI and the GPU renderer is used for it (tests and
/// A/B runs on machines with VAAPI); set to 0, it is not tried.
pub const ENVIRONMENT: &str = "NIGHTJAR_VULKAN_VIDEO";

/// Set to 1, a stream on the GPU renderer tries Vulkan video before VAAPI; set to 0, VAAPI first
/// (A/B runs). Otherwise the Debug setting "Vulkan video first" decides, off by default: VAAPI's
/// pictures are mapped into the renderer's Vulkan images without a copy too (`VaapiMap`), and
/// VAAPI keeps reference frame invalidation.
pub const GPU_FIRST_ENVIRONMENT: &str = "NIGHTJAR_VULKAN_VIDEO_GPU_FIRST";

/// Vulkan video first on the GPU renderer: what `GPU_FIRST_ENVIRONMENT` says when it is set and
/// readable (`env_flag`: 1, true, yes, on, 0, false, no, off), else `setting`.
pub fn gpu_first(value: Option<bool>, setting: bool) -> bool {
    value.unwrap_or(setting)
}

/// What `ENVIRONMENT` asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    /// After VAAPI, for machines without it.
    Default,
    First,
    Off,
}

impl Choice {
    pub fn of(value: Option<&std::ffi::OsStr>) -> Self {
        match value.and_then(|value| value.to_str()) {
            Some("1") => Self::First,
            Some("0") => Self::Off,
            _ => Self::Default,
        }
    }

    pub fn from_environment() -> Self {
        Self::of(std::env::var_os(ENVIRONMENT).as_deref())
    }
}

/// Set to 0, VAAPI pictures on the GPU renderer go through memory as before (A/B runs);
/// otherwise they are mapped into Vulkan images on the renderer's device (`VaapiMap`).
pub const MAP_ENVIRONMENT: &str = "NIGHTJAR_VAAPI_VULKAN_MAP";

/// Where a VAAPI stream goes whose pictures cannot be mapped into the GPU renderer's Vulkan images
/// (`VaapiMap` refused at its start, or an import failed on a picture): unset, to Vulkan video on
/// the renderer's device where it decodes the stream, instead of each picture being read back and
/// uploaded; 0 keeps them going through memory as before (A/B runs); 1 also takes
/// `MAP_ENVIRONMENT=0` for a refusal (test runs: that switch alone asks for memory).
pub const UNMAPPED_ENVIRONMENT: &str = "NIGHTJAR_UNMAPPED_VULKAN_VIDEO";

/// What `MAP_ENVIRONMENT` asks for: the mapping, unless 0.
pub fn map_switched_on(value: Option<&std::ffi::OsStr>) -> bool {
    value.and_then(|value| value.to_str()) != Some("0")
}

/// The device extensions Vulkan video decoding uses (the video ones of plvk.cpp's
/// `k_OptionalDeviceExtensions`), bit n for `EXTENSIONS[n]`: the first two are needed for any
/// codec, then one per codec; then the three FFmpeg imports VAAPI's DMA-BUFs with (`VaapiMap`,
/// hwcontext_vulkan's DRM mapping); then the semaphore export that, with the first of those,
/// FFmpeg hands its images and semaphores to CUDA with (`CudaMap`).
const EXTENSIONS: [&CStr; 9] = [
    c"VK_KHR_video_queue",
    c"VK_KHR_video_decode_queue",
    c"VK_KHR_video_decode_h264",
    c"VK_KHR_video_decode_h265",
    c"VK_KHR_video_decode_av1",
    c"VK_KHR_external_memory_fd",
    c"VK_EXT_external_memory_dma_buf",
    c"VK_EXT_image_drm_format_modifier",
    c"VK_KHR_external_semaphore_fd",
];
const QUEUE_EXTENSIONS: u32 = 0b11;
const VIDEO_EXTENSIONS: u32 = 0b1_1111;
const MAP_EXTENSIONS: u32 = 0b111 << 5;
/// VK_KHR_external_memory_fd and VK_KHR_external_semaphore_fd: hwcontext_vulkan's CUDA transfer.
const INTEROP_EXTENSIONS: u32 = 1 << 5 | 1 << 8;

/// A set of `EXTENSIONS`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Extensions(pub u32);

impl Extensions {
    /// The ones among `names`.
    pub fn among<'a>(names: impl IntoIterator<Item = &'a CStr>) -> Self {
        Self(names.into_iter().fold(0, |bits, name| {
            bits | EXTENSIONS
                .iter()
                .position(|known| *known == name)
                .map_or(0, |bit| 1 << bit)
        }))
    }

    pub fn and(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }

    /// The video extensions of the set when they decode at least one codec, else none.
    pub fn usable(self) -> Self {
        let video = self.0 & VIDEO_EXTENSIONS;
        if video & QUEUE_EXTENSIONS == QUEUE_EXTENSIONS && video & !QUEUE_EXTENSIONS != 0 {
            Self(video)
        } else {
            Self(0)
        }
    }

    /// The set's DMA-BUF import extensions when it has all three (`VaapiMap`), else none.
    pub fn mapping(self) -> Self {
        if self.0 & MAP_EXTENSIONS == MAP_EXTENSIONS {
            Self(MAP_EXTENSIONS)
        } else {
            Self(0)
        }
    }

    /// The set's CUDA interop extensions when it has both (`CudaMap`), else none.
    pub fn interop(self) -> Self {
        if self.0 & INTEROP_EXTENSIONS == INTEROP_EXTENSIONS {
            Self(INTEROP_EXTENSIONS)
        } else {
            Self(0)
        }
    }

    /// The video extensions when `video`, the import ones when `map`, and always the CUDA interop
    /// ones: what the renderer asks for with `NIGHTJAR_VULKAN_VIDEO` and `MAP_ENVIRONMENT` as they
    /// are.
    pub fn wanted(video: bool, map: bool) -> Self {
        Self(if video { VIDEO_EXTENSIONS } else { 0 } | if map { MAP_EXTENSIONS } else { 0 } | INTEROP_EXTENSIONS)
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub fn names(self) -> Vec<&'static CStr> {
        EXTENSIONS
            .iter()
            .enumerate()
            .filter(|(bit, _)| self.0 & (1 << bit) != 0)
            .map(|(_, name)| *name)
            .collect()
    }

    /// Whether the set decodes `codec`.
    pub fn decodes(self, codec: Codec) -> bool {
        let bit = 1 << codec_extension(codec);
        self.usable().0 & bit != 0
    }
}

fn codec_extension(codec: Codec) -> usize {
    match codec {
        Codec::H264 => 2,
        Codec::Hevc => 3,
        Codec::Av1 => 4,
    }
}

fn codec_operation(codec: Codec) -> vk::VideoCodecOperationFlagsKHR {
    match codec {
        Codec::H264 => vk::VideoCodecOperationFlagsKHR::DECODE_H264,
        Codec::Hevc => vk::VideoCodecOperationFlagsKHR::DECODE_H265,
        Codec::Av1 => vk::VideoCodecOperationFlagsKHR::DECODE_AV1,
    }
}

/// The video extensions `device` has.
///
/// # Safety
/// `device` must be a physical device of `instance`.
pub unsafe fn extensions_of(instance: &ash::Instance, device: vk::PhysicalDevice) -> Extensions {
    // SAFETY: Caller guarantees `device` is a physical device of `instance`.
    let listed = unsafe {
        instance
            .enumerate_device_extension_properties(device)
            .unwrap_or_default()
    };
    Extensions::among(
        listed
            .iter()
            .filter_map(|extension| extension.extension_name_as_c_str().ok()),
    )
}

/// Set by `zero_copy::RendererRequest::mark`: the `Extensions` the renderer's device was made with.
pub const EXTENSIONS_PROPERTY: &CStr = c"nightjar.vulkan_video.extensions";
// The patched SDL's (vulkan-interop.patch).
pub const DECODE_QUEUES_PROPERTY: &CStr = c"SDL.gpu.device.create.vulkan.video_decode_queues";
const DECODE_FAMILIES: &CStr = c"SDL.gpu.device.vulkan.video_decode_queue_families";

/// Logs a line about Vulkan video pictures once until it changes, like `zero_copy::note`.
pub fn note(line: &str) {
    static NOTED: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
    let mut noted = NOTED.lock().unwrap_or_else(|e| e.into_inner());
    if noted.as_deref() != Some(line) {
        eprintln!("nightjar: {line}");
        *noted = Some(line.to_owned());
    }
}

#[cfg(test)]
mod sample_tests;

#[cfg(test)]
mod tests;
