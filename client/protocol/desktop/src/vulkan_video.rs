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

use std::ffi::{c_char, c_int, c_void, CStr};
use std::ptr;
use std::time::Duration;

use ash::vk::{self, Handle};
use sdl3_sys::everything::*;

use crate::media::ffi::{
    av_buffer_unref, av_frame_clone, av_frame_copy_props, av_frame_free, av_frame_unref,
    av_hwdevice_ctx_alloc, av_hwdevice_ctx_init, av_hwframe_map, av_hwframe_transfer_data, av_pix_fmt_desc_get,
    avcodec_get_hw_frames_parameters, avutil_version, AVBufferRef, AVCodecContext, AVFrame,
    AVHWDeviceContext, AVHWFramesContext, AVPixelFormat, AV_HWDEVICE_TYPE_VULKAN,
    AV_HWFRAME_MAP_READ, AV_PIX_FMT_NONE, AV_PIX_FMT_NV12, AV_PIX_FMT_NV24, AV_PIX_FMT_P010LE,
    AV_PIX_FMT_P410LE, AV_PIX_FMT_VULKAN,
};
use crate::media::{Codec, Pictures};
use crate::stream::pyrowave_sdk::zero_copy::{
    self, Chain, Features, SdlVulkan, TEXTURE_IMAGE_PROPERTY,
};
use crate::stream::stage_times::Stages;

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
    let listed = instance
        .enumerate_device_extension_properties(device)
        .unwrap_or_default();
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

/// Whether the libavutil of `version` (`avutil_version()`) has the `AVVulkanDeviceContext` and
/// frame structs as declared below: their queue families arrived in 59.34 (FFmpeg 7.1), and 61
/// drops the fixed queue fields before them. The layout is checked against 59.39 (FFmpeg 7.1) and
/// 60.26 (FFmpeg 8.1.2) (`the_structs_have_libavutils_layout`); FFmpeg moves no field within a
/// major version.
pub fn ffmpeg_supported(version: u32) -> Result<(), String> {
    let (major, minor, micro) = (version >> 16, (version >> 8) & 0xff, version & 0xff);
    if version < (59 << 16 | 34 << 8 | 100) {
        return Err(format!(
            "FFmpeg's libavutil {major}.{minor}.{micro} predates the Vulkan queue families this needs (59.34, FFmpeg 7.1)"
        ));
    }
    if major >= 61 {
        return Err(format!(
            "FFmpeg's libavutil {major}.{minor}.{micro} has Vulkan structs this build does not know (it knows 59 and 60)"
        ));
    }
    Ok(())
}

/// `AVVulkanDeviceQueueFamily` (libavutil/hwcontext_vulkan.h, 59.34 to 60).
#[repr(C)]
#[derive(Clone, Copy)]
struct QueueFamily {
    idx: c_int,
    num: c_int,
    flags: vk::QueueFlags,
    video_caps: vk::VideoCodecOperationFlagsKHR,
}

/// `AVVulkanDeviceContext` of libavutil 59.34 to 60 (`ffmpeg_supported`).
#[repr(C)]
struct DeviceContext {
    alloc: *const c_void,
    get_proc_addr: Option<vk::PFN_vkGetInstanceProcAddr>,
    inst: vk::Instance,
    phys_dev: vk::PhysicalDevice,
    act_dev: vk::Device,
    device_features: vk::PhysicalDeviceFeatures2<'static>,
    enabled_inst_extensions: *const *const c_char,
    nb_enabled_inst_extensions: c_int,
    enabled_dev_extensions: *const *const c_char,
    nb_enabled_dev_extensions: c_int,
    /// The deprecated fixed queue fields (`FF_API_VULKAN_FIXED_QUEUES`), left 0: `qf` says it all.
    fixed_queues: [c_int; 10],
    lock_queue: Option<unsafe extern "C" fn(*mut AVHWDeviceContext, u32, u32)>,
    unlock_queue: Option<unsafe extern "C" fn(*mut AVHWDeviceContext, u32, u32)>,
    qf: [QueueFamily; 64],
    nb_qf: c_int,
}

/// `AVVulkanFramesContext` of libavutil 59.34 to 60.
#[repr(C)]
struct FramesContext {
    tiling: vk::ImageTiling,
    usage: vk::ImageUsageFlags,
    create_pnext: *mut c_void,
    alloc_pnext: [*mut c_void; 8],
    flags: c_int,
    img_flags: vk::ImageCreateFlags,
    format: [vk::Format; 8],
    nb_layers: c_int,
    lock_frame: Option<unsafe extern "C" fn(*mut AVHWFramesContext, *mut VkFrame)>,
    unlock_frame: Option<unsafe extern "C" fn(*mut AVHWFramesContext, *mut VkFrame)>,
}

/// `AVVkFrame` of libavutil 59.34 to 60.
#[repr(C)]
struct VkFrame {
    img: [vk::Image; 8],
    tiling: vk::ImageTiling,
    mem: [vk::DeviceMemory; 8],
    size: [usize; 8],
    flags: vk::MemoryPropertyFlags,
    access: [vk::AccessFlags; 8],
    layout: [vk::ImageLayout; 8],
    sem: [vk::Semaphore; 8],
    sem_value: [u64; 8],
    internal: *mut c_void,
    offset: [isize; 8],
    queue_family: [u32; 8],
}

/// What the device context's callbacks need, kept in its `user_opaque` until FFmpeg frees it.
struct Opaque {
    /// The features the device was made with, which `device_features` points into.
    chain: Box<Chain>,
    extensions: Vec<*const c_char>,
    /// The SDL_Mutex SDL holds around its queue and around `vkDeviceWaitIdle` (`SDL_WaitForGPUIdle`,
    /// swapchain rebuilds, the device's end), which needs every queue of the device externally
    /// synchronized: the decode queues take it too. SDL's mutexes are recursive, and FFmpeg holds
    /// it only around a submit, inside its frame locks as the presenter's copy does.
    sdl_lock: *mut SDL_Mutex,
}

/// The lock FFmpeg takes for a queue of `family`: SDL's for every family (`Opaque::sdl_lock`).
fn lock_of(opaque: &Opaque, _family: u32) -> *mut SDL_Mutex {
    opaque.sdl_lock
}

unsafe extern "C" fn lock_queue(context: *mut AVHWDeviceContext, family: u32, _index: u32) {
    let opaque = &*(*context).user_opaque.cast::<Opaque>();
    SDL_LockMutex(lock_of(opaque, family));
}

unsafe extern "C" fn unlock_queue(context: *mut AVHWDeviceContext, family: u32, _index: u32) {
    let opaque = &*(*context).user_opaque.cast::<Opaque>();
    SDL_UnlockMutex(lock_of(opaque, family));
}

unsafe extern "C" fn free_opaque(context: *mut AVHWDeviceContext) {
    let opaque = std::mem::replace(&mut (*context).user_opaque, ptr::null_mut());
    if !opaque.is_null() {
        drop(Box::from_raw(opaque.cast::<Opaque>()));
    }
}

/// The renderer's Vulkan device as FFmpeg's device context for decoding `codec`, for
/// `AVCodecContext::hw_device_ctx`. `gpu` is the GPU renderer's SDL_GPUDevice; errors say why
/// its device cannot decode `codec`.
///
/// # Safety
/// `gpu` must be null or a live SDL GPU device that outlives the context and every frame made
/// with it.
pub unsafe fn device_context(
    gpu: *mut SDL_GPUDevice,
    codec: Codec,
    pictures: Pictures,
) -> Result<*mut AVBufferRef, String> {
    // Packed 4:4:4 pictures are plain 4:2:0 ones of twice the height: the presenter samples their
    // planes (`packed444::Reconstruction::for_planes`), so they decode like any other.
    ffmpeg_supported(avutil_version())?;
    if gpu.is_null() {
        return Err(
            "the renderer does not draw with SDL's GPU API, whose Vulkan device it would decode on"
                .into(),
        );
    }
    let sdl = SdlVulkan::of_device(gpu)?;
    sdl.require(Features::video())?;
    let props = SDL_GetGPUDeviceProperties(gpu);
    let extensions =
        Extensions(SDL_GetNumberProperty(props, EXTENSIONS_PROPERTY.as_ptr(), 0) as u32);
    if extensions.usable().is_empty() {
        return Err("the renderer's Vulkan device was created without the Vulkan video extensions (its devices lack them)".into());
    }
    if !extensions.decodes(codec) {
        return Err(format!(
            "the renderer's Vulkan device was created without {}",
            EXTENSIONS[codec_extension(codec)].to_string_lossy()
        ));
    }
    let decode_families = SDL_GetNumberProperty(props, DECODE_FAMILIES.as_ptr(), 0) as u32;
    let get_instance_proc_addr = zero_copy::get_instance_proc_addr()?;
    let instance = ash::Instance::load(
        &ash::StaticFn {
            get_instance_proc_addr,
        },
        sdl.instance,
    );
    let families = queue_families(&instance, sdl.physical_device);
    let listed = listed_families(&families, sdl.family, decode_families);
    if !listed
        .iter()
        .any(|family| family.video_caps.contains(codec_operation(codec)))
    {
        return Err(format!(
            "no queue of the renderer's Vulkan device decodes {codec:?}"
        ));
    }
    check_profile(
        get_instance_proc_addr,
        sdl.instance,
        sdl.physical_device,
        codec,
        pictures,
    )?;
    share(&sdl, extensions, &listed)
}

/// The renderer's device (`sdl`, made with `extensions`, its queue families `listed`) as FFmpeg's
/// Vulkan device context.
unsafe fn share(
    sdl: &SdlVulkan,
    extensions: Extensions,
    listed: &[QueueFamily],
) -> Result<*mut AVBufferRef, String> {
    let get_instance_proc_addr = zero_copy::get_instance_proc_addr()?;
    let mut chain = Chain::new();
    chain.enable(sdl.features);
    let names: Vec<*const c_char> = extensions
        .names()
        .iter()
        .map(|name| name.as_ptr())
        .collect();
    let opaque = Box::new(Opaque {
        chain,
        extensions: names,
        sdl_lock: sdl.lock,
    });

    let mut reference = av_hwdevice_ctx_alloc(AV_HWDEVICE_TYPE_VULKAN);
    if reference.is_null() {
        return Err("FFmpeg cannot allocate a Vulkan device context".into());
    }
    let context = (*reference).data.cast::<AVHWDeviceContext>();
    let vulkan = &mut *(*context).hwctx.cast::<DeviceContext>();
    vulkan.get_proc_addr = Some(get_instance_proc_addr);
    vulkan.inst = sdl.instance;
    vulkan.phys_dev = sdl.physical_device;
    vulkan.act_dev = sdl.device;
    vulkan.device_features = opaque.chain.pdf2;
    vulkan.enabled_dev_extensions = opaque.extensions.as_ptr();
    vulkan.nb_enabled_dev_extensions = opaque.extensions.len() as c_int;
    vulkan.lock_queue = Some(lock_queue);
    vulkan.unlock_queue = Some(unlock_queue);
    for (slot, family) in vulkan.qf.iter_mut().zip(listed) {
        *slot = *family;
    }
    vulkan.nb_qf = listed.len().min(vulkan.qf.len()) as c_int;
    (*context).user_opaque = Box::into_raw(opaque).cast();
    (*context).free = Some(free_opaque);
    let status = av_hwdevice_ctx_init(reference);
    if status < 0 {
        av_buffer_unref(&mut reference);
        return Err(format!(
            "FFmpeg cannot use the renderer's Vulkan device (error {status})"
        ));
    }
    Ok(reference)
}

/// Whether VAAPI pictures can be mapped into Vulkan images on the renderer's device, or why not:
/// `switched_on` is `MAP_ENVIRONMENT`, `renderer` the extensions the renderer's device was made
/// with (or why it has none of ours), `same_gpu` whether VAAPI opened on the renderer's GPU
/// (`media::VideoDecoder::open_for` opens it on the render node of `render_minor`).
pub fn map_plan(
    switched_on: bool,
    renderer: Result<Extensions, String>,
    same_gpu: Result<(), String>,
) -> Result<(), String> {
    if !switched_on {
        return Err(format!("{MAP_ENVIRONMENT}=0"));
    }
    let made_with = renderer?;
    let missing: Vec<_> = Extensions(MAP_EXTENSIONS & !made_with.0)
        .names()
        .iter()
        .map(|name| name.to_string_lossy())
        .collect();
    if !missing.is_empty() {
        return Err(format!(
            "the renderer's Vulkan device was created without {}",
            missing.join(", ")
        ));
    }
    same_gpu.map_err(|why| format!("VAAPI does not decode on the renderer's GPU: {why}"))
}

/// The renderer's Vulkan device as FFmpeg's device context for mapping VAAPI pictures into it
/// (`VaapiMap`), if `map_plan` says they are (`switched_on`, `same_gpu`): its own queue family
/// only, the DMA-BUF import extensions enabled.
///
/// # Safety
/// As `device_context`.
pub unsafe fn map_device_context(
    gpu: *mut SDL_GPUDevice,
    switched_on: bool,
    same_gpu: Result<(), String>,
) -> Result<*mut AVBufferRef, String> {
    let renderer = made_with(gpu);
    map_plan(switched_on, renderer.as_ref().map(|&(_, extensions)| extensions).map_err(Clone::clone), same_gpu)?;
    let (sdl, extensions) = renderer?;
    let instance = ash::Instance::load(
        &ash::StaticFn {
            get_instance_proc_addr: zero_copy::get_instance_proc_addr()?,
        },
        sdl.instance,
    );
    let families = queue_families(&instance, sdl.physical_device);
    share(&sdl, extensions, &listed_families(&families, sdl.family, 0))
}

/// The renderer's Vulkan device and the extensions of ours it was made with, if FFmpeg can share it.
unsafe fn made_with(gpu: *mut SDL_GPUDevice) -> Result<(SdlVulkan, Extensions), String> {
    ffmpeg_supported(avutil_version())?;
    let sdl = SdlVulkan::of_device(gpu)?;
    // The presenter's copy submits with vkQueueSubmit2 and waits on timeline semaphores.
    sdl.require(Features::video())?;
    let props = SDL_GetGPUDeviceProperties(gpu);
    Ok((sdl, Extensions(SDL_GetNumberProperty(props, EXTENSIONS_PROPERTY.as_ptr(), 0) as u32)))
}

/// The DRM render node minor (`/dev/dri/renderD<minor>`) of the renderer's GPU, for opening VAAPI
/// on the same GPU, or why it is not known (`VK_EXT_physical_device_drm`).
///
/// # Safety
/// `gpu` must be null or a live SDL GPU device.
pub unsafe fn render_minor(gpu: *mut SDL_GPUDevice) -> Result<u32, String> {
    let sdl = SdlVulkan::of_device(gpu)?;
    let instance = ash::Instance::load(
        &ash::StaticFn {
            get_instance_proc_addr: zero_copy::get_instance_proc_addr()?,
        },
        sdl.instance,
    );
    let listed = instance
        .enumerate_device_extension_properties(sdl.physical_device)
        .unwrap_or_default();
    if !listed
        .iter()
        .any(|extension| extension.extension_name_as_c_str() == Ok(c"VK_EXT_physical_device_drm"))
    {
        return Err("the renderer's Vulkan driver does not name its DRM node (no VK_EXT_physical_device_drm)".into());
    }
    let mut drm = vk::PhysicalDeviceDrmPropertiesEXT::default();
    let mut properties = vk::PhysicalDeviceProperties2::default().push_next(&mut drm);
    instance.get_physical_device_properties2(sdl.physical_device, &mut properties);
    render_node_minor(drm.has_render != vk::FALSE, drm.render_minor)
}

/// `render_minor`'s answer from the driver's `VkPhysicalDeviceDrmPropertiesEXT`.
fn render_node_minor(has_render: bool, minor: i64) -> Result<u32, String> {
    if !has_render {
        return Err("the renderer's GPU has no DRM render node".into());
    }
    u32::try_from(minor).map_err(|_| format!("the renderer's GPU reports render node minor {minor}"))
}

/// VAAPI pictures mapped into Vulkan images of the renderer's device, for the presenter's GPU copy
/// (`Planes::copy`) instead of the trip through memory: FFmpeg exports each surface as DMA-BUFs and
/// imports them (`av_hwframe_map`, VAAPI to DRM PRIME to Vulkan, after `vaSyncSurface`, so the
/// decode is done before the copy reads it). The mapped frame holds its VAAPI surface until it is
/// freed, and freeing it waits for the image's timeline semaphore, which the copy signals
/// (hwcontext_vulkan's `vulkan_frame_free`); `Planes` keeps it until its copy is done (`Held`).
pub struct VaapiMap {
    device: *mut AVBufferRef,
    /// The frames context the mapped frames belong to, for the VAAPI pictures of `shape`.
    frames: *mut AVBufferRef,
    shape: (c_int, c_int, AVPixelFormat),
}

// SAFETY: Owned FFmpeg references, used from the decoder thread only.
unsafe impl Send for VaapiMap {}

impl VaapiMap {
    /// # Safety
    /// As `map_device_context`.
    pub unsafe fn new(
        gpu: *mut SDL_GPUDevice,
        switched_on: bool,
        same_gpu: Result<(), String>,
    ) -> Result<Self, String> {
        Ok(Self {
            device: map_device_context(gpu, switched_on, same_gpu)?,
            frames: ptr::null_mut(),
            shape: (0, 0, AV_PIX_FMT_NONE),
        })
    }

    /// Maps the VAAPI picture `source` into `target`, an empty frame, as a Vulkan picture.
    ///
    /// # Safety
    /// `source` must be a live VAAPI frame and `target` an empty one.
    pub unsafe fn map(&mut self, source: *const AVFrame, target: *mut AVFrame) -> Result<(), String> {
        unsafe extern "C" {
            fn av_hwframe_ctx_alloc(device: *mut AVBufferRef) -> *mut AVBufferRef;
            fn av_hwframe_ctx_init(reference: *mut AVBufferRef) -> c_int;
            fn av_buffer_ref(reference: *const AVBufferRef) -> *mut AVBufferRef;
        }
        let vaapi_ref = (*source).hw_frames_ctx;
        if vaapi_ref.is_null() {
            return Err("the VAAPI picture has no frames context".into());
        }
        let vaapi = &*(*vaapi_ref).data.cast::<AVHWFramesContext>();
        let shape = (vaapi.width, vaapi.height, vaapi.sw_format);
        if !mappable(shape.2) {
            return Err(format!("VAAPI pictures of format {} are not two planes the presenter copies", shape.2));
        }
        if self.frames.is_null() || self.shape != shape {
            av_buffer_unref(&mut self.frames);
            let mut frames = av_hwframe_ctx_alloc(self.device);
            if frames.is_null() {
                return Err("FFmpeg cannot allocate a Vulkan frames context".into());
            }
            let context = &mut *(*frames).data.cast::<AVHWFramesContext>();
            context.format = AV_PIX_FMT_VULKAN;
            context.sw_format = vaapi.sw_format;
            context.width = vaapi.width;
            context.height = vaapi.height;
            // Mapped frames only: no images of its own up front.
            context.initial_pool_size = 0;
            // What `Planes::copy` knows the mapped pictures by (`mapped_frames`).
            context.user_opaque = MAPPED.as_ptr().cast_mut().cast();
            let status = av_hwframe_ctx_init(frames);
            if status < 0 {
                av_buffer_unref(&mut frames);
                return Err(format!(
                    "FFmpeg cannot make Vulkan frames for {}x{} pictures of format {} (error {status})",
                    shape.0, shape.1, shape.2
                ));
            }
            self.frames = frames;
            self.shape = shape;
        }
        (*target).format = AV_PIX_FMT_VULKAN;
        (*target).hw_frames_ctx = av_buffer_ref(self.frames);
        if (*target).hw_frames_ctx.is_null() {
            return Err("FFmpeg cannot reference the Vulkan frames context".into());
        }
        let status = av_hwframe_map(target, source, AV_HWFRAME_MAP_READ as c_int);
        if status < 0 {
            av_frame_unref(target);
            return Err(format!("FFmpeg cannot map the VAAPI picture into a Vulkan image (error {status}); the driver may refuse its DRM format modifier"));
        }
        let status = av_frame_copy_props(target, source);
        if status < 0 {
            av_frame_unref(target);
            return Err(format!("the mapped picture cannot take the VAAPI picture's properties (error {status})"));
        }
        Ok(())
    }
}

impl Drop for VaapiMap {
    fn drop(&mut self) {
        // SAFETY: Our references; mapped frames hold their own.
        unsafe {
            av_buffer_unref(&mut self.frames);
            av_buffer_unref(&mut self.device);
        }
    }
}

/// Whether NVDEC's pictures can be copied into Vulkan images of the renderer's device by CUDA
/// (`CudaMap`), or why not: `renderer` is the extensions the renderer's device was made with (or
/// why it has none of ours).
pub fn cuda_plan(renderer: Result<Extensions, String>) -> Result<(), String> {
    let made_with = renderer?;
    let missing: Vec<_> = Extensions(INTEROP_EXTENSIONS & !made_with.0)
        .names()
        .iter()
        .map(|name| name.to_string_lossy())
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "the renderer's Vulkan device was created without {}",
            missing.join(", ")
        ))
    }
}

/// `AV_VK_FRAME_FLAG_DISABLE_MULTIPLANE`: one image per plane, as CUDA imports no multi-plane image.
const DISABLE_MULTIPLANE: c_int = 1 << 2;

/// NVDEC pictures copied on the GPU into Vulkan images of the renderer's device, for the
/// presenter's GPU copy (`Planes::copy`) instead of the trip through memory. Ported from mpv's
/// hwdec_cuda_vk.c as FFmpeg ships the same steps (hwcontext_vulkan's
/// `vulkan_transfer_data_from_cuda`, reached through `av_hwframe_transfer_data`): images with
/// exportable memory (OPAQUE_FD) and timeline semaphores, one image per plane, whose memory and
/// semaphores CUDA imports once per image (`cuImportExternalMemory`,
/// `cuExternalMemoryGetMappedMipmappedArray`, `cuImportExternalSemaphore`); each picture is a
/// `cuMemcpy2DAsync` per plane on the decoder's CUDA stream, after a wait on the image's
/// semaphore and before its signal, between a release of the image to `VK_QUEUE_FAMILY_EXTERNAL`
/// and its acquire back. FFmpeg loads libcuda at run time (ffnvcodec), so nothing links to it.
pub struct CudaMap {
    device: *mut AVBufferRef,
    /// The frames context the copies go into, for NVDEC pictures of `shape`.
    frames: *mut AVBufferRef,
    shape: (c_int, c_int, AVPixelFormat),
}

// SAFETY: Owned FFmpeg references, used from the decoder thread only.
unsafe impl Send for CudaMap {}

impl CudaMap {
    /// # Safety
    /// As `device_context`.
    pub unsafe fn new(gpu: *mut SDL_GPUDevice) -> Result<Self, String> {
        let renderer = made_with(gpu);
        cuda_plan(renderer.as_ref().map(|&(_, extensions)| extensions).map_err(Clone::clone))?;
        let (sdl, extensions) = renderer?;
        let instance = ash::Instance::load(
            &ash::StaticFn {
                get_instance_proc_addr: zero_copy::get_instance_proc_addr()?,
            },
            sdl.instance,
        );
        let families = queue_families(&instance, sdl.physical_device);
        Ok(Self {
            device: share(&sdl, extensions, &listed_families(&families, sdl.family, 0))?,
            frames: ptr::null_mut(),
            shape: (0, 0, AV_PIX_FMT_NONE),
        })
    }

    /// Copies the NVDEC picture `source` into `target`, an empty frame, as a Vulkan picture.
    ///
    /// # Safety
    /// `source` must be a live CUDA frame and `target` an empty one.
    pub unsafe fn copy(&mut self, source: *const AVFrame, target: *mut AVFrame) -> Result<(), String> {
        unsafe extern "C" {
            fn av_hwframe_ctx_alloc(device: *mut AVBufferRef) -> *mut AVBufferRef;
            fn av_hwframe_ctx_init(reference: *mut AVBufferRef) -> c_int;
            fn av_hwframe_get_buffer(frames: *mut AVBufferRef, frame: *mut AVFrame, flags: c_int) -> c_int;
        }
        let cuda_ref = (*source).hw_frames_ctx;
        if cuda_ref.is_null() {
            return Err("the NVDEC picture has no frames context".into());
        }
        let cuda = &*(*cuda_ref).data.cast::<AVHWFramesContext>();
        let shape = (cuda.width, cuda.height, cuda.sw_format);
        if !mappable(shape.2) {
            return Err(format!("NVDEC pictures of format {} are not two planes the presenter copies", shape.2));
        }
        if self.frames.is_null() || self.shape != shape {
            av_buffer_unref(&mut self.frames);
            let mut frames = av_hwframe_ctx_alloc(self.device);
            if frames.is_null() {
                return Err("FFmpeg cannot allocate a Vulkan frames context".into());
            }
            interop_frames(&mut *(*frames).data.cast::<AVHWFramesContext>(), shape);
            let status = av_hwframe_ctx_init(frames);
            if status < 0 {
                av_buffer_unref(&mut frames);
                return Err(format!(
                    "FFmpeg cannot make exportable Vulkan frames for {}x{} pictures of format {} (error {status})",
                    shape.0, shape.1, shape.2
                ));
            }
            self.frames = frames;
            self.shape = shape;
        }
        let status = av_hwframe_get_buffer(self.frames, target, 0);
        if status < 0 {
            return Err(format!("FFmpeg has no Vulkan image for the NVDEC picture (error {status})"));
        }
        let status = av_hwframe_transfer_data(target, source, 0);
        if status < 0 {
            av_frame_unref(target);
            return Err(format!("CUDA cannot copy the NVDEC picture into a Vulkan image (error {status}); NVDEC may decode on another GPU than the renderer's"));
        }
        // The images are the frames context's size, the decoder's coded one; the picture is its own.
        (*target).width = (*source).width;
        (*target).height = (*source).height;
        let status = av_frame_copy_props(target, source);
        if status < 0 {
            av_frame_unref(target);
            return Err(format!("the copied picture cannot take the NVDEC picture's properties (error {status})"));
        }
        Ok(())
    }
}

impl Drop for CudaMap {
    fn drop(&mut self) {
        // SAFETY: Our references; copied frames hold their own.
        unsafe {
            av_buffer_unref(&mut self.frames);
            av_buffer_unref(&mut self.device);
        }
    }
}

/// Sets up the Vulkan frames context `context` for CUDA's copies of NVDEC pictures of `shape`:
/// the picture's software format and coded size, a pool that grows as needed, and one image per
/// plane (`DISABLE_MULTIPLANE`). FFmpeg makes the memory and semaphores exportable itself when
/// the device has the interop extensions.
///
/// # Safety
/// `context` must be a newly allocated Vulkan frames context.
unsafe fn interop_frames(context: &mut AVHWFramesContext, shape: (c_int, c_int, AVPixelFormat)) {
    context.format = AV_PIX_FMT_VULKAN;
    context.sw_format = shape.2;
    context.width = shape.0;
    context.height = shape.1;
    context.initial_pool_size = 0;
    (*context.hwctx.cast::<FramesContext>()).flags |= DISABLE_MULTIPLANE;
}

/// The mark of `VaapiMap`'s frames contexts (`AVHWFramesContext::user_opaque`).
static MAPPED: [u8; 1] = [0];

/// Whether pictures of this frames context are mapped VAAPI pictures (`VaapiMap`), which the copy
/// keeps until it is done (`Held`) whatever FFmpeg did to the image's queue family and layout.
fn mapped_frames(context: &AVHWFramesContext) -> bool {
    ptr::eq(context.user_opaque.cast_const().cast::<u8>(), MAPPED.as_ptr())
}

/// The render node VAAPI opens on for the renderer `gpu`, when `map_plan` says its pictures will
/// be mapped into the renderer's device (`switched_on`, the extensions the device was made with);
/// otherwise why not, and VAAPI opens on the default device, as the probes did.
///
/// # Safety
/// `gpu` must be null or a live SDL GPU device.
pub unsafe fn map_node(gpu: *mut SDL_GPUDevice, switched_on: bool) -> Result<u32, String> {
    map_plan(switched_on, made_with(gpu).map(|(_, extensions)| extensions), Ok(()))?;
    render_minor(gpu)
}

/// Whether VAAPI pictures of this format are mapped: the 4:2:0 two-plane ones the presenter copies
/// (`Planes`); 4:4:4 VAAPI surfaces (AYUV, Y410) are one packed plane.
fn mappable(sw_format: AVPixelFormat) -> bool {
    [AV_PIX_FMT_NV12, AV_PIX_FMT_P010LE].contains(&sw_format)
}

/// How many mapped VAAPI pictures the presenter keeps beyond the one it shows, each until its
/// copy is done (`Held`, one per copy slot): a VAAPI pool that maps gets this many more surfaces.
pub const MAPPED_HELD: i32 = SLOTS as i32;

/// Items kept per copy slot until that slot's copy is done: mapped VAAPI pictures, whose freeing
/// would otherwise wait on the presenter's thread for the copy just submitted, and which give
/// their VAAPI surface back to the decoder only once the GPU no longer reads it.
struct Held<T> {
    items: Vec<(usize, T)>,
}

impl<T> Held<T> {
    fn new() -> Self {
        Self { items: Vec::new() }
    }

    /// Keeps `item` for `slot`'s copy; what the slot held before comes back (its copy was waited
    /// for before the slot was used again).
    fn hold(&mut self, slot: usize, item: T) -> Option<T> {
        let before = self.take(slot);
        self.items.push((slot, item));
        before
    }

    /// What `slot` held.
    fn take(&mut self, slot: usize) -> Option<T> {
        let at = self.items.iter().position(|&(held, _)| held == slot)?;
        Some(self.items.swap_remove(at).1)
    }

    /// What the slots whose copies are `done` held.
    fn release(&mut self, done: impl Fn(usize) -> bool) -> Vec<T> {
        let mut released = Vec::new();
        let mut index = 0;
        while index < self.items.len() {
            if done(self.items[index].0) {
                released.push(self.items.swap_remove(index).1);
            } else {
                index += 1;
            }
        }
        released
    }

    /// Everything still held, for after the device is idle.
    fn drain(&mut self) -> Vec<T> {
        self.items.drain(..).map(|(_, item)| item).collect()
    }
}

/// A reference to a mapped picture, freed (unmapped) when dropped.
struct HeldFrame(*mut AVFrame);

impl Drop for HeldFrame {
    fn drop(&mut self) {
        // SAFETY: Our own reference (`av_frame_clone`).
        unsafe { av_frame_free(&mut self.0) };
    }
}

/// The old layout and the queue families of a picture image's barrier before the copy: an
/// imported image (`VK_QUEUE_FAMILY_EXTERNAL`, FFmpeg's DMA-BUF import) is acquired from outside
/// Vulkan by the renderer's `family`, from GENERAL when FFmpeg left it UNDEFINED (whose contents
/// a transition may drop); FFmpeg's own images are shared and need no transfer.
fn acquire(owner: u32, layout: vk::ImageLayout, family: u32) -> (vk::ImageLayout, u32, u32) {
    if owner == vk::QUEUE_FAMILY_EXTERNAL {
        let layout = if layout == vk::ImageLayout::UNDEFINED {
            vk::ImageLayout::GENERAL
        } else {
            layout
        };
        (layout, vk::QUEUE_FAMILY_EXTERNAL, family)
    } else {
        (layout, vk::QUEUE_FAMILY_IGNORED, vk::QUEUE_FAMILY_IGNORED)
    }
}

/// The two-plane format FFmpeg's Vulkan pictures of that depth and chroma are in (NV12, P010,
/// NV24 or P410), the one `Planes` copies.
fn picture_format(ten_bit: bool, chroma444: bool) -> vk::Format {
    match (ten_bit, chroma444) {
        (false, false) => vk::Format::G8_B8R8_2PLANE_420_UNORM,
        (true, false) => vk::Format::G10X6_B10X6R10X6_2PLANE_420_UNORM_3PACK16,
        (false, true) => vk::Format::G8_B8R8_2PLANE_444_UNORM,
        (true, true) => vk::Format::G10X6_B10X6R10X6_2PLANE_444_UNORM_3PACK16,
    }
}

/// `picture_format` as FFmpeg's software format: NV12, P010, NV24 or P410.
fn picture_sw_format(ten_bit: bool, chroma444: bool) -> AVPixelFormat {
    match (ten_bit, chroma444) {
        (false, false) => AV_PIX_FMT_NV12,
        (true, false) => AV_PIX_FMT_P010LE,
        (false, true) => AV_PIX_FMT_NV24,
        (true, true) => AV_PIX_FMT_P410LE,
    }
}

/// "8-bit 4:2:0" and the like, for messages.
fn describe(ten_bit: bool, chroma444: bool) -> String {
    format!(
        "{}-bit {}",
        if ten_bit { 10 } else { 8 },
        if chroma444 { "4:4:4" } else { "4:2:0" }
    )
}

/// The depth and chroma (10-bit, 4:4:4) of FFmpeg's software format `format`, if it is a 4:2:0
/// or 4:4:4 YUV format of 8 or 10 bits.
fn depth_and_chroma(format: AVPixelFormat) -> Option<(bool, bool)> {
    // SAFETY: FFmpeg's static descriptor table, or null.
    let descriptor = unsafe { av_pix_fmt_desc_get(format) };
    if descriptor.is_null() {
        return None;
    }
    // SAFETY: Checked non-null above.
    let descriptor = unsafe { &*descriptor };
    let ten_bit = match descriptor.comp[0].depth {
        8 => false,
        10 => true,
        _ => return None,
    };
    let chroma444 = match (descriptor.log2_chroma_w, descriptor.log2_chroma_h) {
        (1, 1) => false,
        (0, 0) if descriptor.nb_components >= 3 => true,
        _ => return None,
    };
    Some((ten_bit, chroma444))
}

/// Makes the decoder's frames context (`AVCodecContext::hw_frames_ctx`, in `get_format`) in the
/// two-plane format `check_profile` approved for `pictures`, the one `Planes` copies. Left to
/// itself, FFmpeg picks the driver's format nearest the stream's, a three-plane one where the
/// driver lists it. Errors, and the decoder falls back, when the stream is not of that depth and
/// chroma or FFmpeg cannot make the context.
///
/// # Safety
/// `context` must be a live decoder context in `get_format`, with a `device_context` device.
pub unsafe fn frames_context(
    context: *mut AVCodecContext,
    pictures: Pictures,
) -> Result<(), String> {
    unsafe extern "C" {
        fn av_hwframe_ctx_init(reference: *mut AVBufferRef) -> c_int;
    }
    let context = &mut *context;
    let stream = context.sw_pix_fmt;
    let wanted = (pictures.ten_bit, pictures.chroma444);
    match depth_and_chroma(stream) {
        Some(found) if found == wanted => {}
        Some((ten_bit, chroma444)) => {
            return Err(format!(
                "the stream's pictures are {}, not the {} the device was checked for",
                describe(ten_bit, chroma444),
                describe(wanted.0, wanted.1)
            ))
        }
        None => return Err(format!(
            "the stream's pictures are in FFmpeg format {stream}, not 8- or 10-bit 4:2:0 or 4:4:4"
        )),
    }
    av_buffer_unref(&mut context.hw_frames_ctx);
    let status = avcodec_get_hw_frames_parameters(
        context,
        context.hw_device_ctx,
        AV_PIX_FMT_VULKAN,
        &mut context.hw_frames_ctx,
    );
    if status < 0 {
        return Err(format!(
            "FFmpeg cannot describe the Vulkan frames (error {status})"
        ));
    }
    let frames = &mut *(*context.hw_frames_ctx).data.cast::<AVHWFramesContext>();
    let vulkan = &mut *frames.hwctx.cast::<FramesContext>();
    let (chosen, format) = (frames.sw_format, picture_sw_format(wanted.0, wanted.1));
    if chosen != format {
        note(&format!("FFmpeg would decode into format {chosen}; the frames are made in {format}, the two-plane format the presenter copies"));
    }
    frames.sw_format = format;
    vulkan.format = [vk::Format::UNDEFINED; 8];
    vulkan.format[0] = picture_format(wanted.0, wanted.1);
    // ff_decode_get_hw_frames_ctx adds these 3 to the minimum FFmpeg asks for.
    if frames.initial_pool_size != 0 {
        frames.initial_pool_size += 3;
    }
    let status = av_hwframe_ctx_init(context.hw_frames_ctx);
    if status < 0 {
        av_buffer_unref(&mut context.hw_frames_ctx);
        return Err(format!(
            "FFmpeg cannot make Vulkan frames in format {format} (error {status})"
        ));
    }
    Ok(())
}

/// The codec profile FFmpeg asks the driver for, for pictures of that depth and chroma: H.264
/// High (High 4:4:4 Predictive), HEVC Main or Main 10 (Range Extensions), AV1 Main (High).
fn profiles(
    pictures: Pictures,
) -> (
    vk::native::StdVideoH264ProfileIdc,
    vk::native::StdVideoH265ProfileIdc,
    vk::native::StdVideoAV1Profile,
) {
    use vk::native::*;
    if pictures.chroma444 {
        return (
            StdVideoH264ProfileIdc_STD_VIDEO_H264_PROFILE_IDC_HIGH_444_PREDICTIVE,
            StdVideoH265ProfileIdc_STD_VIDEO_H265_PROFILE_IDC_FORMAT_RANGE_EXTENSIONS,
            StdVideoAV1Profile_STD_VIDEO_AV1_PROFILE_HIGH,
        );
    }
    let h265 = if pictures.ten_bit {
        StdVideoH265ProfileIdc_STD_VIDEO_H265_PROFILE_IDC_MAIN_10
    } else {
        StdVideoH265ProfileIdc_STD_VIDEO_H265_PROFILE_IDC_MAIN
    };
    (
        StdVideoH264ProfileIdc_STD_VIDEO_H264_PROFILE_IDC_HIGH,
        h265,
        StdVideoAV1Profile_STD_VIDEO_AV1_PROFILE_MAIN,
    )
}

/// Whether the device decodes `codec` pictures of that depth and chroma into the two-plane format
/// `Planes` copies, asked of the driver before FFmpeg is: FFmpeg finds out only at the first
/// picture and then decodes in software (4:4:4 and 10-bit are what drivers refuse).
///
/// # Safety
/// `physical_device` must be a physical device of `instance`, made with `get_instance_proc_addr`.
unsafe fn check_profile(
    get_instance_proc_addr: vk::PFN_vkGetInstanceProcAddr,
    instance: vk::Instance,
    physical_device: vk::PhysicalDevice,
    codec: Codec,
    pictures: Pictures,
) -> Result<(), String> {
    let fns = ash::khr::video_queue::InstanceFn::load(|name| {
        std::mem::transmute(get_instance_proc_addr(instance, name.as_ptr()))
    });
    let what = format!(
        "{codec:?} {}-bit {}",
        if pictures.ten_bit { 10 } else { 8 },
        if pictures.chroma444 { "4:4:4" } else { "4:2:0" }
    );
    let (h264_idc, h265_idc, av1_profile) = profiles(pictures);
    let mut h264 = vk::VideoDecodeH264ProfileInfoKHR::default()
        .std_profile_idc(h264_idc)
        .picture_layout(vk::VideoDecodeH264PictureLayoutFlagsKHR::PROGRESSIVE);
    let mut h265 = vk::VideoDecodeH265ProfileInfoKHR::default().std_profile_idc(h265_idc);
    let mut av1 = vk::VideoDecodeAV1ProfileInfoKHR::default().std_profile(av1_profile);
    let depth = if pictures.ten_bit {
        vk::VideoComponentBitDepthFlagsKHR::TYPE_10
    } else {
        vk::VideoComponentBitDepthFlagsKHR::TYPE_8
    };
    let profile = vk::VideoProfileInfoKHR::default()
        .video_codec_operation(codec_operation(codec))
        .chroma_subsampling(if pictures.chroma444 {
            vk::VideoChromaSubsamplingFlagsKHR::TYPE_444
        } else {
            vk::VideoChromaSubsamplingFlagsKHR::TYPE_420
        })
        .luma_bit_depth(depth)
        .chroma_bit_depth(depth);
    let profile = match codec {
        Codec::H264 => profile.push_next(&mut h264),
        Codec::Hevc => profile.push_next(&mut h265),
        Codec::Av1 => profile.push_next(&mut av1),
    };
    let mut h264_caps = vk::VideoDecodeH264CapabilitiesKHR::default();
    let mut h265_caps = vk::VideoDecodeH265CapabilitiesKHR::default();
    let mut av1_caps = vk::VideoDecodeAV1CapabilitiesKHR::default();
    let mut decode_caps = vk::VideoDecodeCapabilitiesKHR::default();
    let caps = vk::VideoCapabilitiesKHR::default().push_next(&mut decode_caps);
    let mut caps = match codec {
        Codec::H264 => caps.push_next(&mut h264_caps),
        Codec::Hevc => caps.push_next(&mut h265_caps),
        Codec::Av1 => caps.push_next(&mut av1_caps),
    };
    let result =
        (fns.get_physical_device_video_capabilities_khr)(physical_device, &profile, &mut caps);
    if result != vk::Result::SUCCESS {
        return Err(format!(
            "the renderer's Vulkan device does not decode {what} ({result})"
        ));
    }
    let listed = [profile];
    let mut list = vk::VideoProfileListInfoKHR::default().profiles(&listed);
    let info = vk::PhysicalDeviceVideoFormatInfoKHR::default()
        .image_usage(vk::ImageUsageFlags::VIDEO_DECODE_DST_KHR)
        .push_next(&mut list);
    let get_formats = fns.get_physical_device_video_format_properties_khr;
    let mut count = 0;
    let mut result = get_formats(physical_device, &info, &mut count, ptr::null_mut());
    let mut formats = vec![vk::VideoFormatPropertiesKHR::default(); count as usize];
    if result == vk::Result::SUCCESS {
        result = get_formats(physical_device, &info, &mut count, formats.as_mut_ptr());
    }
    if result != vk::Result::SUCCESS && result != vk::Result::INCOMPLETE {
        return Err(format!(
            "the renderer's Vulkan device lists no picture formats for {what} ({result})"
        ));
    }
    formats.truncate(count as usize);
    let wanted = picture_format(pictures.ten_bit, pictures.chroma444);
    if !formats.iter().any(|format| format.format == wanted) {
        let listed: Vec<_> = formats
            .iter()
            .map(|format| format.format.as_raw())
            .collect();
        return Err(format!("the renderer's Vulkan device decodes {what} into VkFormats {listed:?}, not {}, the two-plane format the presenter copies", wanted.as_raw()));
    }
    Ok(())
}

/// Each queue family's flags and video codec operations.
unsafe fn queue_families(
    instance: &ash::Instance,
    device: vk::PhysicalDevice,
) -> Vec<(vk::QueueFlags, vk::VideoCodecOperationFlagsKHR)> {
    let count = instance.get_physical_device_queue_family_properties2_len(device);
    let mut video = vec![vk::QueueFamilyVideoPropertiesKHR::default(); count];
    let mut families: Vec<_> = video
        .iter_mut()
        .map(|video| vk::QueueFamilyProperties2::default().push_next(video))
        .collect();
    instance.get_physical_device_queue_family_properties2(device, &mut families);
    let flags: Vec<_> = families
        .iter()
        .map(|family| family.queue_family_properties.queue_flags)
        .collect();
    drop(families);
    flags
        .into_iter()
        .zip(video.iter().map(|video| video.video_codec_operations))
        .collect()
}

/// The families FFmpeg is told about: SDL's, with its one queue, then each decode family SDL
/// made a queue in (bit n of `decode_families` for family n).
fn listed_families(
    families: &[(vk::QueueFlags, vk::VideoCodecOperationFlagsKHR)],
    sdl_family: u32,
    decode_families: u32,
) -> Vec<QueueFamily> {
    families
        .iter()
        .enumerate()
        .filter(|&(index, _)| {
            index as u32 == sdl_family || (index < 32 && decode_families & (1 << index) != 0)
        })
        .map(|(index, &(flags, video_caps))| {
            let decodes = flags.contains(vk::QueueFlags::VIDEO_DECODE_KHR);
            QueueFamily {
                idx: index as c_int,
                num: 1,
                flags,
                video_caps: if decodes {
                    video_caps
                } else {
                    vk::VideoCodecOperationFlagsKHR::empty()
                },
            }
        })
        .collect()
}

/// The software format of a Vulkan picture's frames context: NV12, P010 or another.
///
/// # Safety
/// `frame` must be a live AVFrame of format `AV_PIX_FMT_VULKAN`.
pub unsafe fn sw_format(frame: *const AVFrame) -> crate::media::ffi::AVPixelFormat {
    let frames = (*frame).hw_frames_ctx;
    if frames.is_null() {
        return crate::media::ffi::AV_PIX_FMT_NONE;
    }
    (*(*frames).data.cast::<AVHWFramesContext>()).sw_format
}

/// How long the presenter waits for its copy of the picture before last to finish.
const COPY_WAIT: Duration = Duration::from_secs(1);
/// Command buffers the copies take turns with.
const SLOTS: usize = 3;

/// The presenter's two plane images on the renderer's device (R8 and R8G8, or R16 and R16G16 for
/// 10-bit, the chroma at half or full size), wrapped as SDL GPU textures for the plane shader, and
/// the copies into them.
pub struct Planes {
    device: ash::Device,
    gpu: *mut SDL_GPUDevice,
    queue: vk::Queue,
    family: u32,
    lock: *mut SDL_Mutex,
    width: u32,
    height: u32,
    ten_bit: bool,
    chroma444: bool,
    images: Vec<(vk::Image, vk::DeviceMemory)>,
    textures: [*mut SDL_GPUTexture; 2],
    pool: vk::CommandPool,
    slots: Vec<(vk::CommandBuffer, vk::Fence)>,
    next: usize,
    /// Mapped VAAPI pictures, each until its slot's copy is done.
    held: Held<HeldFrame>,
    /// The CPU time of each `copy`, waits for an earlier copy's slot included, summed up in the
    /// log when the planes go after a stream's worth of pictures.
    stages: Stages<1>,
}

impl Planes {
    /// Plane images for `width` x `height` pictures on `gpu`'s device, wrapped as SDL textures.
    ///
    /// # Safety
    /// `gpu` must be the renderer's live SDL GPU device, which outlives the planes.
    pub unsafe fn new(
        gpu: *mut SDL_GPUDevice,
        width: u32,
        height: u32,
        ten_bit: bool,
        chroma444: bool,
    ) -> Result<Self, String> {
        let sdl = SdlVulkan::of_device(gpu)?;
        sdl.require(Features::video())?;
        let get_instance_proc_addr = zero_copy::get_instance_proc_addr()?;
        let instance = ash::Instance::load(
            &ash::StaticFn {
                get_instance_proc_addr,
            },
            sdl.instance,
        );
        let memory = instance.get_physical_device_memory_properties(sdl.physical_device);
        let device = ash::Device::load(instance.fp_v1_0(), sdl.device);
        let mut planes = Self {
            device,
            gpu,
            queue: sdl.queue,
            family: sdl.family,
            lock: sdl.lock,
            width,
            height,
            ten_bit,
            chroma444,
            images: Vec::new(),
            textures: [ptr::null_mut(); 2],
            pool: vk::CommandPool::null(),
            slots: Vec::new(),
            held: Held::new(),
            next: 0,
            stages: Stages::new(["copy"]),
        };
        planes.pool = planes
            .device
            .create_command_pool(
                &vk::CommandPoolCreateInfo::default()
                    .queue_family_index(sdl.family)
                    .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER),
                None,
            )
            .map_err(|e| format!("a command pool cannot be created ({e})"))?;
        let buffers = planes
            .device
            .allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(planes.pool)
                    .level(vk::CommandBufferLevel::PRIMARY)
                    .command_buffer_count(SLOTS as u32),
            )
            .map_err(|e| format!("command buffers cannot be allocated ({e})"))?;
        for buffer in buffers {
            let fence = planes
                .device
                .create_fence(
                    &vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED),
                    None,
                )
                .map_err(|e| format!("a fence cannot be created ({e})"))?;
            planes.slots.push((buffer, fence));
        }
        for plane in 0..2 {
            let (w, h) = planes.plane_size(plane);
            let image = planes
                .device
                .create_image(
                    &vk::ImageCreateInfo::default()
                        .image_type(vk::ImageType::TYPE_2D)
                        .format(planes.vk_format(plane))
                        .extent(vk::Extent3D {
                            width: w,
                            height: h,
                            depth: 1,
                        })
                        .mip_levels(1)
                        .array_layers(1)
                        .samples(vk::SampleCountFlags::TYPE_1)
                        .tiling(vk::ImageTiling::OPTIMAL)
                        .usage(
                            vk::ImageUsageFlags::SAMPLED
                                | vk::ImageUsageFlags::TRANSFER_SRC
                                | vk::ImageUsageFlags::TRANSFER_DST,
                        )
                        .sharing_mode(vk::SharingMode::EXCLUSIVE)
                        .initial_layout(vk::ImageLayout::UNDEFINED),
                    None,
                )
                .map_err(|e| format!("a {w}x{h} plane image cannot be created ({e})"))?;
            planes.images.push((image, vk::DeviceMemory::null()));
            let needs = planes.device.get_image_memory_requirements(image);
            let kinds = &memory.memory_types[..memory.memory_type_count as usize];
            let usable = |index: &usize| needs.memory_type_bits & (1 << index) != 0;
            let kind = (0..kinds.len())
                .filter(usable)
                .find(|&index| {
                    kinds[index]
                        .property_flags
                        .contains(vk::MemoryPropertyFlags::DEVICE_LOCAL)
                })
                .or_else(|| (0..kinds.len()).find(usable))
                .ok_or("no memory type holds the plane images")?;
            let allocated = planes
                .device
                .allocate_memory(
                    &vk::MemoryAllocateInfo::default()
                        .allocation_size(needs.size)
                        .memory_type_index(kind as u32),
                    None,
                )
                .map_err(|e| {
                    format!(
                        "{} bytes of plane memory cannot be allocated ({e})",
                        needs.size
                    )
                })?;
            planes.images.last_mut().expect("pushed above").1 = allocated;
            planes
                .device
                .bind_image_memory(image, allocated, 0)
                .map_err(|e| format!("the plane memory cannot be bound ({e})"))?;
            let props = SDL_CreateProperties();
            SDL_SetNumberProperty(
                props,
                TEXTURE_IMAGE_PROPERTY.as_ptr(),
                image.as_raw() as i64,
            );
            // A sampler texture: SDL expects the image in SHADER_READ_ONLY_OPTIMAL, where the
            // copy leaves it.
            planes.textures[plane] = SDL_CreateGPUTexture(
                gpu,
                &SDL_GPUTextureCreateInfo {
                    r#type: SDL_GPU_TEXTURETYPE_2D,
                    format: planes.sdl_format(plane),
                    usage: SDL_GPU_TEXTUREUSAGE_SAMPLER,
                    width: w,
                    height: h,
                    layer_count_or_depth: 1,
                    num_levels: 1,
                    sample_count: SDL_GPU_SAMPLECOUNT_1,
                    props,
                },
            );
            SDL_DestroyProperties(props);
            if planes.textures[plane].is_null() {
                return Err(format!(
                    "the plane images cannot be wrapped as textures: {}",
                    crate::sdl::error()
                ));
            }
        }
        Ok(planes)
    }

    /// The luma texture and the chroma one, Cb and Cr interleaved, for the plane shader.
    pub fn textures(&self) -> [*mut SDL_GPUTexture; 2] {
        self.textures
    }

    fn plane_size(&self, plane: usize) -> (u32, u32) {
        if plane == 0 || self.chroma444 {
            (self.width, self.height)
        } else {
            (self.width.div_ceil(2), self.height.div_ceil(2))
        }
    }

    fn vk_format(&self, plane: usize) -> vk::Format {
        match (plane, self.ten_bit) {
            (0, false) => vk::Format::R8_UNORM,
            (0, true) => vk::Format::R16_UNORM,
            (_, false) => vk::Format::R8G8_UNORM,
            (_, true) => vk::Format::R16G16_UNORM,
        }
    }

    fn sdl_format(&self, plane: usize) -> SDL_GPUTextureFormat {
        match (plane, self.ten_bit) {
            (0, false) => SDL_GPU_TEXTUREFORMAT_R8_UNORM,
            (0, true) => SDL_GPU_TEXTUREFORMAT_R16_UNORM,
            (_, false) => SDL_GPU_TEXTUREFORMAT_R8G8_UNORM,
            (_, true) => SDL_GPU_TEXTUREFORMAT_R16G16_UNORM,
        }
    }

    /// Copies the two planes of `frame`, an NV12, P010, NV24 or P410 Vulkan picture of the planes'
    /// size, depth and chroma, into the plane images, on SDL's queue ahead of SDL's next submission
    /// (`renderer` is flushed first, so nothing SDL recorded before reads the new picture). The
    /// picture is the decoder's again once the copy is done, which its semaphore tells FFmpeg.
    /// FFmpeg's images are shared by every queue family it was given (or made in SDL's alone), so
    /// no ownership moves; an imported one (a mapped VAAPI picture, `VaapiMap`) is acquired from
    /// outside Vulkan and kept until its copy is done; an image owned by another family is refused.
    ///
    /// # Safety
    /// `frame` must be a live Vulkan AVFrame made with `device_context` on this device, and
    /// `renderer` the live renderer of the planes' device, on its thread.
    pub unsafe fn copy(
        &mut self,
        renderer: *mut SDL_Renderer,
        frame: *const AVFrame,
    ) -> Result<(), String> {
        let started = std::time::Instant::now();
        let copied = self.copy_picture(renderer, frame);
        self.stages.add(0, started);
        if copied.is_ok() {
            self.stages.commit();
        } else {
            self.stages.reset();
        }
        copied
    }

    /// `copy`, untimed.
    unsafe fn copy_picture(
        &mut self,
        renderer: *mut SDL_Renderer,
        frame: *const AVFrame,
    ) -> Result<(), String> {
        let frames_ref = (*frame).hw_frames_ctx;
        let source = (*frame).data[0].cast::<VkFrame>();
        if frames_ref.is_null() || source.is_null() {
            return Err("the picture has no Vulkan frame".into());
        }
        let wanted = picture_sw_format(self.ten_bit, self.chroma444);
        if sw_format(frame) != wanted {
            return Err(format!(
                "the picture's format {} is not the planes' format {wanted}",
                sw_format(frame)
            ));
        }
        let sources = if (*source).img[1] == vk::Image::null() {
            1
        } else {
            2
        };
        if let Some(&family) = (&(*source).queue_family)[..sources]
            .iter()
            .find(|&&family| ![vk::QUEUE_FAMILY_IGNORED, vk::QUEUE_FAMILY_EXTERNAL, self.family].contains(&family))
        {
            return Err(format!("the picture is owned by queue family {family}, not shared with the renderer's ({})", self.family));
        }
        let frames = (*frames_ref).data.cast::<AVHWFramesContext>();
        let vulkan_frames = &*(*frames).hwctx.cast::<FramesContext>();
        let (Some(lock_frame), Some(unlock_frame)) =
            (vulkan_frames.lock_frame, vulkan_frames.unlock_frame)
        else {
            return Err("FFmpeg's Vulkan frames context has no frame lock".into());
        };
        if !SDL_FlushRenderer(renderer) {
            return Err(format!(
                "the renderer cannot be flushed before the copy: {}",
                crate::sdl::error()
            ));
        }
        // Mapped pictures whose copies are done go back to the decoder.
        let device = &self.device;
        let slots = &self.slots;
        drop(self.held.release(|slot| device.get_fence_status(slots[slot].1) == Ok(true)));
        let (commands, fence) = self.slots[self.next];
        match self.device.wait_for_fences(&[fence], true, COPY_WAIT.as_nanos() as u64) {
            Ok(()) => {}
            Err(vk::Result::TIMEOUT) => {
                return Err(format!("an earlier picture's copy did not finish within {COPY_WAIT:?}: the Vulkan device may be hung or lost"))
            }
            Err(e) => return Err(format!("waiting for an earlier picture's copy failed ({e})")),
        }
        self.device
            .reset_command_buffer(commands, vk::CommandBufferResetFlags::empty())
            .map_err(|e| format!("the copy's command buffer cannot be reset ({e})"))?;
        drop(self.held.take(self.next));
        let imported = mapped_frames(&*(*frames_ref).data.cast::<AVHWFramesContext>());
        lock_frame(frames, source);
        let result = self.record_and_submit(commands, fence, &mut *source);
        unlock_frame(frames, source);
        if result.is_ok() {
            if imported {
                let clone = av_frame_clone(frame);
                if clone.is_null() {
                    // Freed with the presenter's reference instead, which waits for the copy.
                    eprintln!("nightjar: a mapped VAAPI picture cannot be kept for its copy (out of memory); it is freed after the copy instead");
                } else {
                    drop(self.held.hold(self.next, HeldFrame(clone)));
                }
            }
            self.next = (self.next + 1) % self.slots.len();
        }
        result
    }

    /// Under the frame's lock: waits for the picture's semaphores, copies and signals them one
    /// higher, with the layout the copy leaves its images in.
    unsafe fn record_and_submit(
        &mut self,
        commands: vk::CommandBuffer,
        fence: vk::Fence,
        source: &mut VkFrame,
    ) -> Result<(), String> {
        let device = &self.device;
        // One multi-plane image, or one image per plane (`AV_VK_FRAME_FLAG_DISABLE_MULTIPLANE`).
        let separate = source.img[1] != vk::Image::null();
        let sources = if separate { 2 } else { 1 };
        let range = |aspect| vk::ImageSubresourceRange {
            aspect_mask: aspect,
            base_mip_level: 0,
            level_count: 1,
            base_array_layer: 0,
            layer_count: 1,
        };
        device
            .begin_command_buffer(
                commands,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )
            .map_err(|e| format!("the copy cannot be recorded ({e})"))?;
        let family = self.family;
        let mut before: Vec<vk::ImageMemoryBarrier2> = (0..sources)
            .map(|index| {
                let (old_layout, from, to) =
                    acquire(source.queue_family[index], source.layout[index], family);
                vk::ImageMemoryBarrier2::default()
                    .src_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
                    .src_access_mask(vk::AccessFlags2::from_raw(u64::from(
                        source.access[index].as_raw(),
                    )))
                    .dst_stage_mask(vk::PipelineStageFlags2::COPY)
                    .dst_access_mask(vk::AccessFlags2::TRANSFER_READ)
                    .old_layout(old_layout)
                    .new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                    .src_queue_family_index(from)
                    .dst_queue_family_index(to)
                    .image(source.img[index])
                    .subresource_range(range(vk::ImageAspectFlags::COLOR))
            })
            .collect();
        // The planes' old contents go; the copy waits for SDL's reads of them before it.
        before.extend(self.images.iter().map(|&(image, _)| {
            vk::ImageMemoryBarrier2::default()
                .src_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
                .dst_stage_mask(vk::PipelineStageFlags2::COPY)
                .dst_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
                .old_layout(vk::ImageLayout::UNDEFINED)
                .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .image(image)
                .subresource_range(range(vk::ImageAspectFlags::COLOR))
        }));
        device.cmd_pipeline_barrier2(
            commands,
            &vk::DependencyInfo::default().image_memory_barriers(&before),
        );
        for plane in 0..2 {
            let (width, height) = self.plane_size(plane);
            let (image, aspect) = if separate {
                (source.img[plane], vk::ImageAspectFlags::COLOR)
            } else {
                (
                    source.img[0],
                    if plane == 0 {
                        vk::ImageAspectFlags::PLANE_0
                    } else {
                        vk::ImageAspectFlags::PLANE_1
                    },
                )
            };
            let layers = |aspect_mask| vk::ImageSubresourceLayers {
                aspect_mask,
                mip_level: 0,
                base_array_layer: 0,
                layer_count: 1,
            };
            let region = vk::ImageCopy {
                src_subresource: layers(aspect),
                src_offset: vk::Offset3D::default(),
                dst_subresource: layers(vk::ImageAspectFlags::COLOR),
                dst_offset: vk::Offset3D::default(),
                extent: vk::Extent3D {
                    width,
                    height,
                    depth: 1,
                },
            };
            device.cmd_copy_image(
                commands,
                image,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                self.images[plane].0,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &[region],
            );
        }
        let after: Vec<vk::ImageMemoryBarrier2> = self
            .images
            .iter()
            .map(|&(image, _)| {
                vk::ImageMemoryBarrier2::default()
                    .src_stage_mask(vk::PipelineStageFlags2::COPY)
                    .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
                    .dst_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
                    .dst_access_mask(vk::AccessFlags2::SHADER_SAMPLED_READ)
                    .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                    .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .image(image)
                    .subresource_range(range(vk::ImageAspectFlags::COLOR))
            })
            .collect();
        device.cmd_pipeline_barrier2(
            commands,
            &vk::DependencyInfo::default().image_memory_barriers(&after),
        );
        device
            .end_command_buffer(commands)
            .map_err(|e| format!("the copy cannot be recorded ({e})"))?;

        let waits: Vec<vk::SemaphoreSubmitInfo> = (0..sources)
            .map(|index| {
                vk::SemaphoreSubmitInfo::default()
                    .semaphore(source.sem[index])
                    .value(source.sem_value[index])
                    .stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
            })
            .collect();
        let signals: Vec<vk::SemaphoreSubmitInfo> = (0..sources)
            .map(|index| {
                vk::SemaphoreSubmitInfo::default()
                    .semaphore(source.sem[index])
                    .value(source.sem_value[index] + 1)
                    .stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
            })
            .collect();
        let buffers = [vk::CommandBufferSubmitInfo::default().command_buffer(commands)];
        let submit = vk::SubmitInfo2::default()
            .wait_semaphore_infos(&waits)
            .command_buffer_infos(&buffers)
            .signal_semaphore_infos(&signals);
        device
            .reset_fences(&[fence])
            .map_err(|e| format!("the copy's fence cannot be reset ({e})"))?;
        SDL_LockMutex(self.lock);
        let submitted = device.queue_submit2(self.queue, &[submit], fence);
        SDL_UnlockMutex(self.lock);
        if let Err(e) = submitted {
            // Never submitted: the fence stays unsignalled, so the slot's next wait would time
            // out; signal it with an empty submission instead of leaving it.
            SDL_LockMutex(self.lock);
            let _ = device.queue_submit2(self.queue, &[], fence);
            SDL_UnlockMutex(self.lock);
            return Err(format!("the copy cannot be submitted ({e})"));
        }
        for index in 0..sources {
            source.sem_value[index] += 1;
            source.layout[index] = vk::ImageLayout::TRANSFER_SRC_OPTIMAL;
            source.access[index] = vk::AccessFlags::empty();
            if source.queue_family[index] == vk::QUEUE_FAMILY_EXTERNAL {
                source.queue_family[index] = family;
            }
        }
        Ok(())
    }
}

impl Drop for Planes {
    fn drop(&mut self) {
        if self.stages.frames() >= crate::stream::stage_times::SUMMARY_FRAMES {
            if let Some(summary) = self.stages.summary() {
                eprintln!("nightjar: Vulkan plane copy stages {summary} (CPU time)");
            }
        }
        // SAFETY: Handles made in `new`; nothing is freed while the GPU may still use it.
        unsafe {
            for texture in self
                .textures
                .into_iter()
                .filter(|texture| !texture.is_null())
            {
                SDL_ReleaseGPUTexture(self.gpu, texture);
            }
            let fences: Vec<vk::Fence> = self.slots.iter().map(|&(_, fence)| fence).collect();
            // SDL destroys a texture's views once its last command is done, and the images must
            // outlive them; the copies must be done too.
            let idle = SDL_WaitForGPUIdle(self.gpu);
            let copied = fences.is_empty()
                || self
                    .device
                    .wait_for_fences(&fences, true, COPY_WAIT.as_nanos() as u64)
                    .is_ok();
            if !idle || !copied {
                eprintln!(
                    "nightjar: the GPU did not finish with the Vulkan video planes ({}), so their images stay allocated",
                    crate::sdl::error()
                );
                return;
            }
            // The copies are done: the mapped pictures go back.
            drop(self.held.drain());
            for &(image, memory) in &self.images {
                self.device.destroy_image(image, None);
                if memory != vk::DeviceMemory::null() {
                    self.device.free_memory(memory, None);
                }
            }
            for fence in fences {
                self.device.destroy_fence(fence, None);
            }
            if self.pool != vk::CommandPool::null() {
                self.device.destroy_command_pool(self.pool, None);
            }
        }
    }
}

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
mod tests {
    use super::*;

    #[test]
    fn the_environment_puts_vulkan_video_first_or_off_and_anything_else_leaves_it_after_vaapi() {
        assert_eq!(Choice::of(Some("1".as_ref())), Choice::First);
        assert_eq!(Choice::of(Some("0".as_ref())), Choice::Off);
        for other in [None, Some("yes"), Some(""), Some("2")] {
            assert_eq!(
                Choice::of(other.map(AsRef::as_ref)),
                Choice::Default,
                "{other:?}"
            );
        }
    }

    #[test]
    fn video_extensions_decode_a_codec_only_with_both_queue_extensions() {
        let all = Extensions::among(EXTENSIONS);
        assert_eq!(all, Extensions(0b1_1111_1111));
        assert_eq!(all.usable(), Extensions(0b1_1111), "the import extensions are not video ones");
        assert!([Codec::H264, Codec::Hevc, Codec::Av1]
            .into_iter()
            .all(|codec| all.decodes(codec)));
        let hevc = Extensions::among([
            c"VK_KHR_video_queue",
            c"VK_KHR_video_decode_queue",
            c"VK_KHR_video_decode_h265",
            c"VK_KHR_swapchain",
        ]);
        assert!(
            hevc.decodes(Codec::Hevc) && !hevc.decodes(Codec::H264) && !hevc.decodes(Codec::Av1)
        );
        assert_eq!(
            hevc.names(),
            [
                c"VK_KHR_video_queue",
                c"VK_KHR_video_decode_queue",
                c"VK_KHR_video_decode_h265"
            ]
        );
        let no_queue = Extensions::among([c"VK_KHR_video_queue", c"VK_KHR_video_decode_h264"]);
        assert!(no_queue.usable().is_empty() && !no_queue.decodes(Codec::H264));
        let no_codec = Extensions::among([c"VK_KHR_video_queue", c"VK_KHR_video_decode_queue"]);
        assert!(
            no_codec.usable().is_empty(),
            "queues without a codec decode nothing"
        );
    }

    const IMPORT: [&CStr; 3] = [
        c"VK_KHR_external_memory_fd",
        c"VK_EXT_external_memory_dma_buf",
        c"VK_EXT_image_drm_format_modifier",
    ];

    /// VAAPI pictures went through memory to the GPU renderer (2.5 GB/s at 4K100 10-bit,
    /// nightjar#80): they are mapped where the renderer's device imports DMA-BUFs and VAAPI
    /// decodes on its GPU, and every other case says why it keeps the readback.
    #[test]
    fn vaapi_pictures_are_mapped_only_with_the_import_extensions_on_the_renderers_gpu() {
        let import = Extensions::among(IMPORT);
        assert_eq!(import.mapping(), import);
        assert!(import.usable().is_empty(), "importing decodes nothing");
        assert_eq!(map_plan(true, Ok(import), Ok(())), Ok(()));
        let video_and_import = Extensions::among(EXTENSIONS);
        assert_eq!(map_plan(true, Ok(video_and_import), Ok(())), Ok(()));
        assert_eq!(map_plan(false, Ok(import), Ok(())), Err("NIGHTJAR_VAAPI_VULKAN_MAP=0".into()));
        let no_modifier = Extensions::among(IMPORT[..2].iter().copied());
        assert!(no_modifier.mapping().is_empty());
        let why = map_plan(true, Ok(no_modifier), Ok(())).unwrap_err();
        assert!(why.contains("VK_EXT_image_drm_format_modifier") && !why.contains("dma_buf"), "{why}");
        let why = map_plan(true, Ok(Extensions::among(EXTENSIONS[..5].iter().copied())), Ok(())).unwrap_err();
        assert!(IMPORT.iter().all(|name| why.contains(name.to_str().unwrap())), "{why}");
        let why = map_plan(true, Ok(import), Err("VAAPI does not open on /dev/dri/renderD129".into())).unwrap_err();
        assert!(why.starts_with("VAAPI does not decode on the renderer's GPU") && why.contains("renderD129"), "{why}");
        let why = map_plan(true, Err("the renderer draws with \"opengl\", not Vulkan".into()), Ok(())).unwrap_err();
        assert!(why.contains("opengl"), "{why}");
        // The switch and what the renderer asks for.
        assert!(map_switched_on(None) && map_switched_on(Some("1".as_ref())) && !map_switched_on(Some("0".as_ref())));
        assert_eq!(Extensions::wanted(true, true), Extensions::among(EXTENSIONS));
        assert_eq!(Extensions::wanted(false, true), Extensions(import.0 | 1 << 8), "and the semaphore export for CUDA");
        assert_eq!(Extensions::wanted(true, false).mapping(), Extensions(0));
        // The render node VAAPI opens on.
        assert_eq!(render_node_minor(true, 129), Ok(129));
        assert!(render_node_minor(false, 129).is_err());
        assert!(render_node_minor(true, -1).is_err());
    }

    /// A mapped picture freed right after its copy was submitted would block the presenter until
    /// the copy finished and give its VAAPI surface back while the GPU may still read it.
    #[test]
    fn mapped_pictures_are_kept_until_their_slots_copy_is_done() {
        let mut held = Held::new();
        assert_eq!(held.hold(0, "frame 1"), None);
        assert_eq!(held.hold(1, "frame 2"), None);
        assert!(held.release(|_| false).is_empty(), "nothing goes while copies run");
        assert_eq!(held.release(|slot| slot == 0), ["frame 1"]);
        assert_eq!(held.hold(0, "frame 3"), None, "slot 0's earlier picture went when its copy was done");
        // A slot used again after its fence was waited for gives back what it held.
        assert_eq!(held.take(1), Some("frame 2"));
        assert_eq!(held.hold(0, "frame 4"), Some("frame 3"));
        assert_eq!(held.drain(), ["frame 4"]);
        assert!(held.release(|_| true).is_empty());
    }

    /// Mapped pictures were kept only while their image still said `VK_QUEUE_FAMILY_EXTERNAL`: one
    /// FFmpeg had moved itself was freed at once, blocking on its copy. They are known by their
    /// frames context instead.
    #[test]
    fn mapped_pictures_are_known_by_their_frames_context() {
        // SAFETY: A plain C struct, all-zero is valid.
        let mut context: AVHWFramesContext = unsafe { std::mem::zeroed() };
        assert!(!mapped_frames(&context), "a decoder's frames context");
        context.user_opaque = MAPPED.as_ptr().cast_mut().cast();
        assert!(mapped_frames(&context));
        let other = [0u8];
        context.user_opaque = other.as_ptr().cast_mut().cast();
        assert!(!mapped_frames(&context), "another user's mark");
    }

    /// NVDEC's pictures crossed system memory on their way to the GPU renderer: CUDA copies them
    /// into exportable Vulkan images where the renderer's device has both interop extensions,
    /// and every other case says why they keep the trip through memory.
    #[test]
    fn nvdec_pictures_are_copied_by_cuda_only_with_both_interop_extensions() {
        let interop = Extensions::among([c"VK_KHR_external_memory_fd", c"VK_KHR_external_semaphore_fd"]);
        assert_eq!(interop.interop(), interop);
        assert!(interop.usable().is_empty() && interop.mapping().is_empty(), "interop decodes and maps nothing");
        assert_eq!(cuda_plan(Ok(interop)), Ok(()));
        assert_eq!(cuda_plan(Ok(Extensions::among(EXTENSIONS))), Ok(()));
        // The DMA-BUF import extensions have the memory export, not the semaphore one.
        let import = Extensions::among(IMPORT);
        assert!(import.interop().is_empty());
        let why = cuda_plan(Ok(import)).unwrap_err();
        assert!(why.contains("VK_KHR_external_semaphore_fd") && !why.contains("memory_fd"), "{why}");
        let why = cuda_plan(Ok(Extensions::among(EXTENSIONS[..5].iter().copied()))).unwrap_err();
        assert!(why.contains("VK_KHR_external_memory_fd, VK_KHR_external_semaphore_fd"), "{why}");
        let why = cuda_plan(Err("the renderer draws with \"opengl\", not Vulkan".into())).unwrap_err();
        assert!(why.contains("opengl"), "{why}");
        // Asked for whatever the switches say.
        for (video, map) in [(false, false), (true, false), (false, true), (true, true)] {
            assert_eq!(Extensions::wanted(video, map).interop(), interop, "{video} {map}");
        }
        // Only the two-plane 4:2:0 pictures `Planes` copies: NVDEC's 4:4:4 ones are three planes.
        assert!(mappable(AV_PIX_FMT_NV12) && mappable(AV_PIX_FMT_P010LE));
        assert!(!mappable(crate::media::ffi::AV_PIX_FMT_YUV444P) && !mappable(crate::media::ffi::AV_PIX_FMT_YUV444P16LE));
    }

    /// CUDA imports no multi-plane image, and the decoder's coded size is the copy's.
    #[test]
    fn the_frames_cuda_copies_into_have_one_image_per_plane() {
        // SAFETY: Plain C structs, all-zero is valid; `vulkan` outlives the context's use.
        let mut vulkan: FramesContext = unsafe { std::mem::zeroed() };
        let mut context: AVHWFramesContext = unsafe { std::mem::zeroed() };
        context.hwctx = (&raw mut vulkan).cast();
        context.initial_pool_size = 20;
        unsafe { interop_frames(&mut context, (1920, 1088, AV_PIX_FMT_P010LE)) };
        assert_eq!((context.format, context.sw_format, context.width, context.height), (AV_PIX_FMT_VULKAN, AV_PIX_FMT_P010LE, 1920, 1088));
        assert_eq!(context.initial_pool_size, 0, "the pool grows as pictures are held");
        assert_eq!(vulkan.flags, DISABLE_MULTIPLANE);
        assert_eq!(DISABLE_MULTIPLANE, 4, "AV_VK_FRAME_FLAG_DISABLE_MULTIPLANE");
    }

    /// Release builds' FFmpeg had no libdrm, so `av_hwframe_map` was ENOSYS and every shipped
    /// Linux build read VAAPI pictures back.
    #[test]
    fn the_shipped_ffmpeg_is_built_with_libdrm_for_the_mapping() {
        let script = include_str!("../ffmpeg-decode-only.sh");
        let linux = script.split("platform=(").nth(1).expect("the Linux options");
        let linux = &linux[..linux.find(')').expect("their end")];
        assert!(linux.split_whitespace().any(|option| option == "--enable-libdrm"), "{linux}");
        assert!(script.contains("does not link libdrm"), "the build checks libavutil links it");
    }

    /// An imported image is FFmpeg's `VK_QUEUE_FAMILY_EXTERNAL` in UNDEFINED layout: the copy
    /// acquires it from outside Vulkan without dropping its contents; decoder images move no family.
    #[test]
    fn an_imported_picture_is_acquired_from_outside_vulkan_and_a_decoded_one_is_not() {
        let (layout, from, to) = acquire(vk::QUEUE_FAMILY_EXTERNAL, vk::ImageLayout::UNDEFINED, 0);
        assert_eq!((layout.as_raw(), from, to), (vk::ImageLayout::GENERAL.as_raw(), vk::QUEUE_FAMILY_EXTERNAL, 0));
        let (layout, from, to) = acquire(vk::QUEUE_FAMILY_IGNORED, vk::ImageLayout::VIDEO_DECODE_DPB_KHR, 0);
        assert_eq!((layout.as_raw(), from, to), (vk::ImageLayout::VIDEO_DECODE_DPB_KHR.as_raw(), vk::QUEUE_FAMILY_IGNORED, vk::QUEUE_FAMILY_IGNORED));
        assert_eq!(acquire(2, vk::ImageLayout::GENERAL, 2).1, vk::QUEUE_FAMILY_IGNORED);
    }

    #[test]
    fn only_libavutil_59_34_to_60_has_the_structs_declared_here() {
        let version = |major: u32, minor: u32, micro: u32| major << 16 | minor << 8 | micro;
        assert!(ffmpeg_supported(version(59, 39, 100)).is_ok(), "FFmpeg 7.1");
        assert!(ffmpeg_supported(version(60, 26, 103)).is_ok(), "FFmpeg 8.1");
        let old = ffmpeg_supported(version(59, 8, 100)).unwrap_err();
        assert!(
            old.contains("59.8.100") && old.contains("FFmpeg 7.1"),
            "{old}"
        );
        assert!(
            ffmpeg_supported(version(58, 29, 100)).is_err(),
            "FFmpeg 6.1"
        );
        let new = ffmpeg_supported(version(61, 0, 100)).unwrap_err();
        assert!(new.contains("61.0.100"), "{new}");
    }

    #[test]
    fn the_structs_have_libavutils_layout() {
        use std::mem::{offset_of, size_of};
        // Each field's offset (or size) here, then offsetof and sizeof with gcc on x86-64 in the
        // hwcontext_vulkan.h of FFmpeg 7.1 (libavutil 59.39) and of FFmpeg 8.1.2 (60.26), which
        // the release ships: 60 still has the fixed queue fields (FF_API_VULKAN_FIXED_QUEUES
        // lasts until 61) and lock_queue (FF_API_VULKAN_SYNC_QUEUES until 62).
        let fields = [
            (
                "AVVulkanDeviceContext.get_proc_addr",
                offset_of!(DeviceContext, get_proc_addr),
                8,
                8,
            ),
            (
                "AVVulkanDeviceContext.act_dev",
                offset_of!(DeviceContext, act_dev),
                32,
                32,
            ),
            (
                "AVVulkanDeviceContext.device_features",
                offset_of!(DeviceContext, device_features),
                40,
                40,
            ),
            (
                "AVVulkanDeviceContext.enabled_dev_extensions",
                offset_of!(DeviceContext, enabled_dev_extensions),
                296,
                296,
            ),
            (
                "AVVulkanDeviceContext.nb_enabled_dev_extensions",
                offset_of!(DeviceContext, nb_enabled_dev_extensions),
                304,
                304,
            ),
            (
                "AVVulkanDeviceContext.lock_queue",
                offset_of!(DeviceContext, lock_queue),
                352,
                352,
            ),
            (
                "AVVulkanDeviceContext.unlock_queue",
                offset_of!(DeviceContext, unlock_queue),
                360,
                360,
            ),
            (
                "AVVulkanDeviceContext.qf",
                offset_of!(DeviceContext, qf),
                368,
                368,
            ),
            (
                "AVVulkanDeviceContext.nb_qf",
                offset_of!(DeviceContext, nb_qf),
                1392,
                1392,
            ),
            (
                "sizeof AVVulkanDeviceQueueFamily",
                size_of::<QueueFamily>(),
                16,
                16,
            ),
            (
                "AVVulkanFramesContext.usage",
                offset_of!(FramesContext, usage),
                4,
                4,
            ),
            (
                "AVVulkanFramesContext.img_flags",
                offset_of!(FramesContext, img_flags),
                84,
                84,
            ),
            (
                "AVVulkanFramesContext.format",
                offset_of!(FramesContext, format),
                88,
                88,
            ),
            (
                "AVVulkanFramesContext.nb_layers",
                offset_of!(FramesContext, nb_layers),
                120,
                120,
            ),
            (
                "AVVulkanFramesContext.lock_frame",
                offset_of!(FramesContext, lock_frame),
                128,
                128,
            ),
            (
                "AVVulkanFramesContext.unlock_frame",
                offset_of!(FramesContext, unlock_frame),
                136,
                136,
            ),
            ("AVVkFrame.access", offset_of!(VkFrame, access), 204, 204),
            ("AVVkFrame.layout", offset_of!(VkFrame, layout), 236, 236),
            ("AVVkFrame.sem", offset_of!(VkFrame, sem), 272, 272),
            (
                "AVVkFrame.sem_value",
                offset_of!(VkFrame, sem_value),
                336,
                336,
            ),
            ("AVVkFrame.offset", offset_of!(VkFrame, offset), 408, 408),
            (
                "AVVkFrame.queue_family",
                offset_of!(VkFrame, queue_family),
                472,
                472,
            ),
            ("sizeof AVVkFrame", size_of::<VkFrame>(), 504, 504),
        ];
        for (name, ours, v59, v60) in fields {
            assert_eq!(ours, v59, "{name} in libavutil 59.39");
            assert_eq!(ours, v60, "{name} in libavutil 60.26");
        }
        assert_eq!(size_of::<DeviceContext>(), 1400);
        assert_eq!(size_of::<FramesContext>(), 144);
    }

    #[test]
    fn ffmpeg_hears_of_sdls_family_and_the_decode_families_sdl_made_a_queue_in() {
        let graphics =
            vk::QueueFlags::GRAPHICS | vk::QueueFlags::COMPUTE | vk::QueueFlags::TRANSFER;
        let decode = vk::QueueFlags::VIDEO_DECODE_KHR | vk::QueueFlags::TRANSFER;
        let h264_hevc = vk::VideoCodecOperationFlagsKHR::DECODE_H264
            | vk::VideoCodecOperationFlagsKHR::DECODE_H265;
        let families = [
            (graphics, vk::VideoCodecOperationFlagsKHR::empty()),
            (
                vk::QueueFlags::COMPUTE,
                vk::VideoCodecOperationFlagsKHR::empty(),
            ),
            (decode, h264_hevc),
            (
                vk::QueueFlags::VIDEO_ENCODE_KHR,
                vk::VideoCodecOperationFlagsKHR::ENCODE_H264,
            ),
        ];
        let listed = listed_families(&families, 0, 1 << 2);
        // The flags compare as raw bits: ash's flag types print only with its debug feature.
        let summary: Vec<_> = listed
            .iter()
            .map(|family| {
                (
                    family.idx,
                    family.num,
                    family.flags.as_raw(),
                    family.video_caps.as_raw(),
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                (0, 1, graphics.as_raw(), 0),
                (2, 1, decode.as_raw(), h264_hevc.as_raw())
            ]
        );
        assert_eq!(
            listed_families(&families, 0, 0).len(),
            1,
            "without SDL's decode queue only SDL's family"
        );
    }

    /// The frames are made in the two-plane format of the stream's depth and chroma; other streams
    /// (4:2:2, 12-bit, grey) and a stream unlike the one the device was checked for are refused
    /// before FFmpeg makes anything.
    #[test]
    fn the_frames_are_made_in_the_two_plane_format_of_the_checked_pictures() {
        use crate::media::ffi::*;
        for (ten_bit, chroma444) in [(false, false), (true, false), (false, true), (true, true)] {
            let format = picture_sw_format(ten_bit, chroma444);
            assert_eq!(
                depth_and_chroma(format),
                Some((ten_bit, chroma444)),
                "{format}"
            );
            // SAFETY: FFmpeg's static descriptor table.
            let planes = unsafe {
                let descriptor = &*av_pix_fmt_desc_get(format);
                (0..descriptor.nb_components as usize)
                    .map(|c| descriptor.comp[c].plane)
                    .max()
            };
            assert_eq!(planes, Some(1), "{format} has two planes");
        }
        let streams = [
            (AV_PIX_FMT_YUV420P, Some((false, false))),
            (AV_PIX_FMT_YUV420P10LE, Some((true, false))),
            (AV_PIX_FMT_YUV444P, Some((false, true))),
            (AV_PIX_FMT_YUV444P10LE, Some((true, true))),
            (AV_PIX_FMT_YUV422P, None),
            (AV_PIX_FMT_YUV420P12LE, None),
            (AV_PIX_FMT_GRAY8, None),
        ];
        for (stream, expected) in streams {
            assert_eq!(depth_and_chroma(stream), expected, "{stream}");
        }
        // SAFETY: A context allocated and freed here, with no device: refused before FFmpeg uses it.
        unsafe {
            let mut context = avcodec_alloc_context3(ptr::null());
            assert!(!context.is_null());
            let eight = Pictures::default();
            (*context).sw_pix_fmt = AV_PIX_FMT_YUV420P10LE;
            let error = frames_context(context, eight).unwrap_err();
            assert!(
                error.contains("10-bit 4:2:0, not the 8-bit 4:2:0"),
                "{error}"
            );
            (*context).sw_pix_fmt = AV_PIX_FMT_YUV422P;
            let error = frames_context(context, eight).unwrap_err();
            assert!(error.contains("not 8- or 10-bit 4:2:0 or 4:4:4"), "{error}");
            assert!((*context).hw_frames_ctx.is_null());
            avcodec_free_context(&mut context);
        }
    }

    /// SDL waits for its whole device under its queue lock, so FFmpeg's decode queues take that lock
    /// too, not one of their own (`stream::vulkan_tests_unix` runs it against a device wait).
    #[test]
    fn every_queue_ffmpeg_submits_to_takes_sdls_queue_lock() {
        let mut fake = 0u8;
        let opaque = Opaque {
            chain: Chain::new(),
            extensions: Vec::new(),
            sdl_lock: (&raw mut fake).cast(),
        };
        for family in [0, 1, 3, 31] {
            assert_eq!(lock_of(&opaque, family), opaque.sdl_lock, "family {family}");
        }
    }

    /// FFmpeg takes the shared renderer's device for every codec its video extensions decode. Needs
    /// a Vulkan device with video decoding (ANV only with `ANV_VIDEO_DECODE=1`) and SDL's offscreen
    /// driver: `ANV_VIDEO_DECODE=1 cargo test -p nightjar-desktop -- --ignored --test-threads=1 vulkan_video`.
    #[test]
    #[ignore = "needs a Vulkan device with video decoding and SDL's offscreen driver"]
    fn ffmpeg_takes_the_shared_renderers_device() {
        // SAFETY: Plain SDL calls on objects made here; each device context is freed before the renderer.
        unsafe {
            SDL_SetHint(SDL_HINT_VIDEO_DRIVER, c"offscreen".as_ptr());
            assert!(SDL_Init(SDL_INIT_VIDEO), "{}", crate::sdl::error());
            let window = SDL_CreateWindow(c"Vulkan video".as_ptr(), 640, 360, SDL_WINDOW_HIDDEN);
            let renderer = crate::sdl::create_renderer_for(window, true, true, true);
            assert!(!renderer.is_null(), "{}", crate::sdl::error());
            let gpu: *mut SDL_GPUDevice =
                crate::sdl::renderer_pointer(renderer, SDL_PROP_RENDERER_GPU_DEVICE_POINTER).cast();
            let props = SDL_GetGPUDeviceProperties(gpu);
            assert_ne!(
                SDL_GetNumberProperty(props, DECODE_FAMILIES.as_ptr(), 0),
                0,
                "SDL made no decode queue"
            );
            let extensions =
                Extensions(SDL_GetNumberProperty(props, EXTENSIONS_PROPERTY.as_ptr(), 0) as u32);
            let codecs: Vec<_> = [Codec::H264, Codec::Hevc, Codec::Av1]
                .into_iter()
                .filter(|&codec| extensions.decodes(codec))
                .collect();
            assert!(
                !codecs.is_empty(),
                "the renderer has no codec's video extensions"
            );
            for codec in codecs {
                let mut context = device_context(gpu, codec, Pictures::default())
                    .unwrap_or_else(|e| panic!("{codec:?}: {e}"));
                av_buffer_unref(&mut context);
            }
            SDL_DestroyRenderer(renderer);
            SDL_DestroyWindow(window);
            SDL_Quit();
        }
    }
}
