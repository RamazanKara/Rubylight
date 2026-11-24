//! Sharing the GPU renderer's Vulkan device with FFmpeg (`device_context`, `map_device_context`,
//! `frames_context`), Vulkan video profile/format probing, queue family discovery, and DRM render
//! node lookup.

use std::ffi::{c_char, c_int};
use std::ptr;

use ash::vk;
use sdl3_sys::everything::*;

use crate::media::ffi::{
    av_buffer_unref, av_hwdevice_ctx_alloc, av_hwdevice_ctx_init, av_pix_fmt_desc_get,
    avcodec_get_hw_frames_parameters, avutil_version, AVBufferRef, AVCodecContext, AVFrame,
    AVHWDeviceContext, AVHWFramesContext, AVPixelFormat, AV_HWDEVICE_TYPE_VULKAN,
    AV_PIX_FMT_NV12, AV_PIX_FMT_NV24, AV_PIX_FMT_P010LE, AV_PIX_FMT_P410LE, AV_PIX_FMT_RGBA,
    AV_PIX_FMT_VULKAN, AV_PIX_FMT_VUYA, AV_PIX_FMT_VUYX, AV_PIX_FMT_XV30LE,
};
use crate::media::{Codec, Pictures};
use crate::stream::pyrowave_sdk::zero_copy::{self, Chain, Features, SdlVulkan};

use super::abi::{
    ffmpeg_supported, DeviceContext60, DeviceContext61, FramesContext, QueueFamily, StructLayout,
};
use super::{
    codec_extension, codec_operation, note, Extensions, DECODE_FAMILIES, EXTENSIONS,
    EXTENSIONS_PROPERTY, MAP_ENVIRONMENT, MAP_EXTENSIONS,
};

/// What the device context's callbacks need, kept in its `user_opaque` until FFmpeg frees it.
pub(super) struct Opaque {
    /// The features the device was made with, which `device_features` points into.
    pub(super) chain: Box<Chain>,
    pub(super) extensions: Vec<*const c_char>,
    /// The SDL_Mutex SDL holds around its queue and around `vkDeviceWaitIdle` (`SDL_WaitForGPUIdle`,
    /// swapchain rebuilds, the device's end), which needs every queue of the device externally
    /// synchronized: the decode queues take it too. SDL's mutexes are recursive, and FFmpeg holds
    /// it only around a submit, inside its frame locks as the presenter's copy does.
    pub(super) sdl_lock: *mut SDL_Mutex,
}

/// The lock FFmpeg takes for a queue of `family`: SDL's for every family (`Opaque::sdl_lock`).
pub(super) fn lock_of(opaque: &Opaque, _family: u32) -> *mut SDL_Mutex {
    opaque.sdl_lock
}

unsafe extern "C" fn lock_queue(context: *mut AVHWDeviceContext, family: u32, _index: u32) {
    // SAFETY: Caller passes a valid `AVHWDeviceContext` whose `user_opaque` is a live `Opaque`.
    unsafe {
        let opaque = &*(*context).user_opaque.cast::<Opaque>();
        SDL_LockMutex(lock_of(opaque, family));
    }
}

unsafe extern "C" fn unlock_queue(context: *mut AVHWDeviceContext, family: u32, _index: u32) {
    // SAFETY: Caller passes a valid `AVHWDeviceContext` whose `user_opaque` is a live `Opaque`.
    unsafe {
        let opaque = &*(*context).user_opaque.cast::<Opaque>();
        SDL_UnlockMutex(lock_of(opaque, family));
    }
}

unsafe extern "C" fn free_opaque(context: *mut AVHWDeviceContext) {
    // SAFETY: Caller passes a valid `AVHWDeviceContext` tearing down `user_opaque` made with `Box::into_raw`.
    unsafe {
        let opaque = std::mem::replace(&mut (*context).user_opaque, ptr::null_mut());
        if !opaque.is_null() {
            drop(Box::from_raw(opaque.cast::<Opaque>()));
        }
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
    // SAFETY: Caller guarantees `gpu` is null or a live SDL GPU device outliving the context.
    unsafe {
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
}

/// The renderer's device (`sdl`, made with `extensions`, its queue families `listed`) as FFmpeg's
/// Vulkan device context.
pub(super) unsafe fn share(
    sdl: &SdlVulkan,
    extensions: Extensions,
    listed: &[QueueFamily],
) -> Result<*mut AVBufferRef, String> {
    // SAFETY: Caller guarantees `sdl` describes a live Vulkan device that outlives the FFmpeg context.
    unsafe {
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
        // The same fields, where the linked libavutil has them.
        macro_rules! fill {
            ($context:ty) => {{
                let vulkan = &mut *(*context).hwctx.cast::<$context>();
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
            }};
        }
        match StructLayout::linked() {
            StructLayout::V60 => fill!(DeviceContext60),
            StructLayout::V61 => fill!(DeviceContext61),
        }
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
    // SAFETY: Caller guarantees `gpu` is null or a live SDL GPU device outliving the context.
    unsafe {
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
}

/// The renderer's Vulkan device and the extensions of ours it was made with, if FFmpeg can share it.
pub(super) unsafe fn made_with(gpu: *mut SDL_GPUDevice) -> Result<(SdlVulkan, Extensions), String> {
    // SAFETY: Caller guarantees `gpu` is null or a live SDL GPU device.
    unsafe {
        ffmpeg_supported(avutil_version())?;
        let sdl = SdlVulkan::of_device(gpu)?;
        // The presenter's copy submits with vkQueueSubmit2 and waits on timeline semaphores.
        sdl.require(Features::video())?;
        let props = SDL_GetGPUDeviceProperties(gpu);
        Ok((sdl, Extensions(SDL_GetNumberProperty(props, EXTENSIONS_PROPERTY.as_ptr(), 0) as u32)))
    }
}

/// The DRM render node minor (`/dev/dri/renderD<minor>`) of the renderer's GPU, for opening VAAPI
/// on the same GPU, or why it is not known (`VK_EXT_physical_device_drm`).
///
/// # Safety
/// `gpu` must be null or a live SDL GPU device.
pub unsafe fn render_minor(gpu: *mut SDL_GPUDevice) -> Result<u32, String> {
    // SAFETY: Caller guarantees `gpu` is null or a live SDL GPU device.
    unsafe {
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
}

/// `render_minor`'s answer from the driver's `VkPhysicalDeviceDrmPropertiesEXT`.
pub(super) fn render_node_minor(has_render: bool, minor: i64) -> Result<u32, String> {
    if !has_render {
        return Err("the renderer's GPU has no DRM render node".into());
    }
    u32::try_from(minor).map_err(|_| format!("the renderer's GPU reports render node minor {minor}"))
}

/// The render node VAAPI opens on for the renderer `gpu`, when `map_plan` says its pictures will
/// be mapped into the renderer's device (`switched_on`, the extensions the device was made with);
/// otherwise why not, and VAAPI opens on the default device, as the probes did.
///
/// # Safety
/// `gpu` must be null or a live SDL GPU device.
pub unsafe fn map_node(gpu: *mut SDL_GPUDevice, switched_on: bool) -> Result<u32, String> {
    // SAFETY: Caller guarantees `gpu` is null or a live SDL GPU device.
    unsafe {
        map_plan(switched_on, made_with(gpu).map(|(_, extensions)| extensions), Ok(()))?;
        render_minor(gpu)
    }
}

/// Whether VAAPI or NVDEC pictures of this format are the 4:2:0 two-plane ones the presenter copies
/// (`Planes`); 4:4:4 VAAPI surfaces are one interleaved plane (`interleaved_444`).
pub(super) fn mappable(sw_format: AVPixelFormat) -> bool {
    [AV_PIX_FMT_NV12, AV_PIX_FMT_P010LE].contains(&sw_format)
}

/// The single-plane format a 4:4:4 VAAPI surface of this software format is mapped as, and whether
/// it is 10-bit: AYUV and XYUV (FFmpeg's VUYA and VUYX, bytes V, U, Y and A) as RGBA, and Y410
/// (XV30, U, Y and V in 10 bits each from the lowest) as itself; its Vulkan image is R8G8B8A8 or
/// A2R10G10B10, copied into one image of `Planes::interleaved`. Y412 and Y416 hold 12 and 16 bits,
/// more than a stream has.
pub fn interleaved_444(sw_format: AVPixelFormat) -> Option<(AVPixelFormat, bool)> {
    match sw_format {
        AV_PIX_FMT_VUYA | AV_PIX_FMT_VUYX => Some((interleaved_sw_format(false), false)),
        AV_PIX_FMT_XV30LE => Some((interleaved_sw_format(true), true)),
        _ => None,
    }
}

/// The frames context format of a mapped 4:4:4 VAAPI picture of that depth (`interleaved_444`).
pub(super) fn interleaved_sw_format(ten_bit: bool) -> AVPixelFormat {
    if ten_bit {
        AV_PIX_FMT_XV30LE
    } else {
        AV_PIX_FMT_RGBA
    }
}

/// The two-plane format FFmpeg's Vulkan pictures of that depth and chroma are in (NV12, P010,
/// NV24 or P410), the one `Planes` copies.
pub(super) fn picture_format(ten_bit: bool, chroma444: bool) -> vk::Format {
    match (ten_bit, chroma444) {
        (false, false) => vk::Format::G8_B8R8_2PLANE_420_UNORM,
        (true, false) => vk::Format::G10X6_B10X6R10X6_2PLANE_420_UNORM_3PACK16,
        (false, true) => vk::Format::G8_B8R8_2PLANE_444_UNORM,
        (true, true) => vk::Format::G10X6_B10X6R10X6_2PLANE_444_UNORM_3PACK16,
    }
}

/// `picture_format` as FFmpeg's software format: NV12, P010, NV24 or P410.
pub(super) fn picture_sw_format(ten_bit: bool, chroma444: bool) -> AVPixelFormat {
    match (ten_bit, chroma444) {
        (false, false) => AV_PIX_FMT_NV12,
        (true, false) => AV_PIX_FMT_P010LE,
        (false, true) => AV_PIX_FMT_NV24,
        (true, true) => AV_PIX_FMT_P410LE,
    }
}

/// The Vulkan and SDL formats of `Planes`' image `plane`: R8 and R8G8, or R16 and R16G16 for
/// 10-bit; one R8G8B8A8 or A2B10G10R10 image when `interleaved` (a mapped 4:4:4 VAAPI picture's
/// R8G8B8A8 or A2R10G10B10 copies into it bit for bit, the shader swaps the 10-bit channels).
pub(super) fn plane_format(plane: usize, ten_bit: bool, interleaved: bool) -> (vk::Format, SDL_GPUTextureFormat) {
    match (interleaved, plane, ten_bit) {
        (true, _, false) => (vk::Format::R8G8B8A8_UNORM, SDL_GPU_TEXTUREFORMAT_R8G8B8A8_UNORM),
        (true, _, true) => (vk::Format::A2B10G10R10_UNORM_PACK32, SDL_GPU_TEXTUREFORMAT_R10G10B10A2_UNORM),
        (false, 0, false) => (vk::Format::R8_UNORM, SDL_GPU_TEXTUREFORMAT_R8_UNORM),
        (false, 0, true) => (vk::Format::R16_UNORM, SDL_GPU_TEXTUREFORMAT_R16_UNORM),
        (false, _, false) => (vk::Format::R8G8_UNORM, SDL_GPU_TEXTUREFORMAT_R8G8_UNORM),
        (false, _, true) => (vk::Format::R16G16_UNORM, SDL_GPU_TEXTUREFORMAT_R16G16_UNORM),
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
pub(super) fn depth_and_chroma(format: AVPixelFormat) -> Option<(bool, bool)> {
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
    // SAFETY: Caller guarantees `context` is a live `AVCodecContext` in `get_format` with a `device_context` device.
    unsafe {
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
    // SAFETY: Caller guarantees `physical_device` belongs to `instance`, made with `get_instance_proc_addr`.
    unsafe {
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
}

/// Each queue family's flags and video codec operations.
pub(super) unsafe fn queue_families(
    instance: &ash::Instance,
    device: vk::PhysicalDevice,
) -> Vec<(vk::QueueFlags, vk::VideoCodecOperationFlagsKHR)> {
    // SAFETY: Caller guarantees `device` is a physical device of `instance`.
    unsafe {
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
}

/// The families FFmpeg is told about: SDL's, with its one queue, then each decode family SDL
/// made a queue in (bit n of `decode_families` for family n).
pub(super) fn listed_families(
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
    // SAFETY: Caller guarantees `frame` is a live AVFrame of format `AV_PIX_FMT_VULKAN`.
    unsafe {
        let frames = (*frame).hw_frames_ctx;
        if frames.is_null() {
            return crate::media::ffi::AV_PIX_FMT_NONE;
        }
        (*(*frames).data.cast::<AVHWFramesContext>()).sw_format
    }
}
