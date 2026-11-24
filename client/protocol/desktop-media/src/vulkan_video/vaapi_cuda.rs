//! VA-API to Vulkan (`VaapiMap`) and NVDEC CUDA to Vulkan (`CudaMap`) picture mappers, plus
//! DMA-BUF layer validation and layout helpers.

use std::ffi::{c_int, c_void};
use std::ptr;

use ash::vk;
use sdl3_sys::everything::SDL_GPUDevice;

#[cfg(target_os = "linux")]
use crate::media::ffi::AV_PIX_FMT_P010LE;
use crate::media::ffi::{
    av_buffer_unref, av_frame_alloc, av_frame_copy_props, av_frame_free, av_frame_unref,
    av_hwframe_map, av_hwframe_transfer_data, AVBufferRef, AVFrame, AVHWDeviceContext,
    AVHWFramesContext, AVPixelFormat, AV_HWFRAME_MAP_READ, AV_PIX_FMT_DRM_PRIME, AV_PIX_FMT_NONE,
    AV_PIX_FMT_VULKAN,
};
use crate::stream::pyrowave_sdk::zero_copy;

use super::abi::{CudaDeviceContext, DrmFrameDescriptor, Frame, FramesContext};
use super::device::{
    interleaved_444, listed_families, made_with, map_device_context, mappable, queue_families,
    share,
};
#[cfg(target_os = "linux")]
use super::imports::SurfaceImports;
#[cfg(target_os = "linux")]
use super::note;
use super::{Extensions, INTEROP_EXTENSIONS};

/// VAAPI pictures mapped into Vulkan images of the renderer's device, for the presenter's GPU copy
/// (`Planes::copy`) instead of the trip through memory. A two-plane (NV12, P010) surface's
/// DMA-BUFs are imported once, on its first picture, and kept for the surface's later pictures
/// while the decoder's pool lasts (`SurfaceImports`, after `vaSyncSurface`, so the decode is done
/// before the copy reads it); `NIGHTJAR_DMABUF_CACHE=0`, a 4:4:4 surface, or an import of ours
/// that failed once, and FFmpeg exports and imports every picture anew instead (`av_hwframe_map`,
/// VAAPI to DRM PRIME to Vulkan). Either way the mapped frame holds its VAAPI surface until it is
/// freed, and freeing it waits for the image's timeline semaphore, which the copy signals
/// (hwcontext_vulkan's `vulkan_frame_free`, or `Lent`); `Planes` keeps it until its copy is done
/// (`Held`).
pub struct VaapiMap {
    device: *mut AVBufferRef,
    /// The frames context the mapped frames belong to, for the VAAPI pictures of `shape`.
    frames: *mut AVBufferRef,
    shape: (c_int, c_int, AVPixelFormat),
    /// The per-surface imports, while they work.
    #[cfg(target_os = "linux")]
    imports: Option<SurfaceImports>,
    /// Whether the per-surface imports were used at all, for the count at the end.
    cached: bool,
    /// Pictures mapped, and of them the ones whose images came from the per-surface cache.
    pictures: (u64, u64),
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
        // SAFETY: Caller guarantees `gpu` is null or a live SDL GPU device outliving this map.
        unsafe {
            let device = map_device_context(gpu, switched_on, same_gpu)?;
            #[cfg(target_os = "linux")]
            let imports = SurfaceImports::start(gpu);
            Ok(Self {
                device,
                frames: ptr::null_mut(),
                shape: (0, 0, AV_PIX_FMT_NONE),
                #[cfg(target_os = "linux")]
                cached: imports.is_some(),
                #[cfg(not(target_os = "linux"))]
                cached: false,
                #[cfg(target_os = "linux")]
                imports,
                pictures: (0, 0),
            })
        }
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
        // SAFETY: Caller guarantees `source` is a live VAAPI frame and `target` is an empty AVFrame.
        unsafe {
            let vaapi_ref = (*source).hw_frames_ctx;
            if vaapi_ref.is_null() {
                return Err("the VAAPI picture has no frames context".into());
            }
            let vaapi = &*(*vaapi_ref).data.cast::<AVHWFramesContext>();
            let shape = (vaapi.width, vaapi.height, vaapi.sw_format);
            let interleaved = interleaved_444(shape.2);
            if !mappable(shape.2) && interleaved.is_none() {
                return Err(format!(
                    "VAAPI pictures of format {} are neither two planes nor one 4:4:4 plane the presenter copies",
                    shape.2
                ));
            }
            if self.frames.is_null() || self.shape != shape {
                av_buffer_unref(&mut self.frames);
                let mut frames = av_hwframe_ctx_alloc(self.device);
                if frames.is_null() {
                    return Err("FFmpeg cannot allocate a Vulkan frames context".into());
                }
                let context = &mut *(*frames).data.cast::<AVHWFramesContext>();
                context.format = AV_PIX_FMT_VULKAN;
                // A 4:4:4 surface as the single-plane format its Vulkan image is in.
                context.sw_format = interleaved.map_or(vaapi.sw_format, |(format, _)| format);
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
            // Whether the surface's kept import served the picture, when one of ours mapped it.
            #[allow(unused_mut)]
            let mut cached = None;
            #[cfg(target_os = "linux")]
            if let Some(imports) = self.imports.as_mut() {
                let ten_bit = interleaved.map_or(shape.2 == AV_PIX_FMT_P010LE, |(_, ten)| ten);
                match imports.map(source, target, self.frames, ten_bit, interleaved.is_some()) {
                    Ok(hit) => cached = Some(hit),
                    Err(why) => {
                        // `target` is empty again; FFmpeg maps this picture and the rest.
                        note(&format!("VAAPI pictures are imported into Vulkan anew for every picture from now on: {why}"));
                        self.imports = None;
                    }
                }
            }
            if cached.is_none() {
                (*target).format = AV_PIX_FMT_VULKAN;
                (*target).hw_frames_ctx = av_buffer_ref(self.frames);
                if (*target).hw_frames_ctx.is_null() {
                    return Err("FFmpeg cannot reference the Vulkan frames context".into());
                }
                if interleaved.is_some() {
                    if let Err(why) = map_interleaved(target, source) {
                        av_frame_unref(target);
                        return Err(why);
                    }
                } else {
                    let status = av_hwframe_map(target, source, AV_HWFRAME_MAP_READ as c_int);
                    if status < 0 {
                        av_frame_unref(target);
                        return Err(format!("FFmpeg cannot map the VAAPI picture into a Vulkan image (error {status}); the driver may refuse its DRM format modifier"));
                    }
                }
            }
            let status = av_frame_copy_props(target, source);
            if status < 0 {
                av_frame_unref(target);
                return Err(format!("the mapped picture cannot take the VAAPI picture's properties (error {status})"));
            }
            let hit = cached == Some(true);
            self.pictures.0 += 1;
            self.pictures.1 += u64::from(hit);
            #[cfg(test)]
            {
                use std::sync::atomic::Ordering::Relaxed;
                MAPPED_PICTURES.0.fetch_add(1, Relaxed);
                MAPPED_PICTURES.1.fetch_add(u64::from(hit), Relaxed);
            }
            Ok(())
        }
    }
}

impl Drop for VaapiMap {
    fn drop(&mut self) {
        if self.cached && self.pictures.0 > 0 {
            eprintln!(
                "nightjar: VAAPI pictures mapped into Vulkan: {} of {} from the per-surface cache",
                self.pictures.1, self.pictures.0
            );
        }
        // The cache's imports go once no mapped frame uses them (`CachedImport`).
        #[cfg(target_os = "linux")]
        drop(self.imports.take());
        // SAFETY: Our references; mapped frames hold their own.
        unsafe {
            av_buffer_unref(&mut self.frames);
            av_buffer_unref(&mut self.device);
        }
    }
}

/// Pictures `VaapiMap` mapped in this test process, and of them the ones whose images came from
/// the per-surface cache (`stream::vulkan_tests_unix`).
#[cfg(test)]
pub(crate) static MAPPED_PICTURES: (std::sync::atomic::AtomicU64, std::sync::atomic::AtomicU64) =
    (std::sync::atomic::AtomicU64::new(0), std::sync::atomic::AtomicU64::new(0));

/// `MAPPED_PICTURES` as (mapped, from the cache).
#[cfg(test)]
pub(crate) fn mapped_pictures() -> (u64, u64) {
    use std::sync::atomic::Ordering::Relaxed;
    (MAPPED_PICTURES.0.load(Relaxed), MAPPED_PICTURES.1.load(Relaxed))
}

/// DRM fourccs of the layers a two-plane VAAPI surface exports as (`VA_EXPORT_SURFACE_SEPARATE_LAYERS`):
/// NV12 as R8 and GR88, P010 as R16 and GR1616 (red the low half: Cb, green Cr).
pub(super) const DRM_FORMAT_R16: u32 = fourcc(b"R16 ");
pub(super) const DRM_FORMAT_GR1616: u32 = fourcc(b"GR32");
/// `DRM_FORMAT_MOD_INVALID`: the driver named no layout.
pub(super) const DRM_FORMAT_MOD_INVALID: u64 = 0x00ff_ffff_ffff_ffff;

/// The Vulkan format a one-plane DMA-BUF layer of `drm_format` imports as, if it is one of a
/// two-plane surface's (`import_layers`); the same as hwcontext_vulkan's table.
pub(super) fn layer_format(drm_format: u32) -> Option<vk::Format> {
    match drm_format {
        crate::dmabuf::DRM_FORMAT_R8 => Some(vk::Format::R8_UNORM),
        crate::dmabuf::DRM_FORMAT_GR88 => Some(vk::Format::R8G8_UNORM),
        DRM_FORMAT_R16 => Some(vk::Format::R16_UNORM),
        DRM_FORMAT_GR1616 => Some(vk::Format::R16G16_UNORM),
        _ => None,
    }
}

/// The Vulkan formats of the two layers of an exported NV12 (or `ten_bit`, P010) surface, the
/// planes `Planes` copies, or why the export is not that: two one-plane layers, luma then chroma,
/// each in an object the export lists.
pub(super) fn import_layers(descriptor: &crate::dmabuf::Descriptor, ten_bit: bool) -> Result<[vk::Format; 2], String> {
    let wanted = if ten_bit {
        [DRM_FORMAT_R16, DRM_FORMAT_GR1616]
    } else {
        [crate::dmabuf::DRM_FORMAT_R8, crate::dmabuf::DRM_FORMAT_GR88]
    };
    if descriptor.num_layers != 2 || descriptor.num_objects == 0 || descriptor.num_objects > 4 {
        return Err(format!(
            "the export has {} layers in {} objects, not a {}-bit surface's two planes",
            descriptor.num_layers,
            descriptor.num_objects,
            if ten_bit { 10 } else { 8 }
        ));
    }
    let mut formats = [vk::Format::UNDEFINED; 2];
    for (index, (layer, wanted)) in descriptor.layers.iter().zip(wanted).enumerate() {
        if layer.drm_format != wanted || layer.num_planes != 1 {
            return Err(format!(
                "layer {index} is DRM format {:#010x} with {} planes, not {wanted:#010x} with one",
                layer.drm_format, layer.num_planes
            ));
        }
        if layer.object_index[0] >= descriptor.num_objects {
            return Err(format!("layer {index} names object {} of {}", layer.object_index[0], descriptor.num_objects));
        }
        formats[index] = layer_format(layer.drm_format).expect("one of the wanted formats");
    }
    Ok(formats)
}

/// The Vulkan format of a single-layer 4:4:4 VAAPI surface (`VUYA`/`VUYX` as R8G8B8A8, `XV30` as
/// A2R10G10B10), matching `map_interleaved` and FFmpeg's hwcontext_vulkan table.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(super) fn import_interleaved_layer(descriptor: &crate::dmabuf::Descriptor, ten_bit: bool) -> Result<vk::Format, String> {
    let (wanted, vk_format) = if ten_bit {
        (fourcc(b"XV30"), vk::Format::A2R10G10B10_UNORM_PACK32)
    } else {
        (fourcc(b"XYUV"), vk::Format::R8G8B8A8_UNORM)
    };
    if descriptor.num_layers != 1 || descriptor.num_objects == 0 || descriptor.num_objects > 4 {
        return Err(format!(
            "the export has {} layers in {} objects, not a {}-bit 4:4:4 surface's single plane",
            descriptor.num_layers,
            descriptor.num_objects,
            if ten_bit { 10 } else { 8 }
        ));
    }
    let layer = &descriptor.layers[0];
    if drm_alias(layer.drm_format) != wanted || layer.num_planes != 1 {
        return Err(format!(
            "layer 0 is DRM format {:#010x} with {} planes, not {wanted:#010x} with one",
            layer.drm_format, layer.num_planes
        ));
    }
    if layer.object_index[0] >= descriptor.num_objects {
        return Err(format!("layer 0 names object {} of {}", layer.object_index[0], descriptor.num_objects));
    }
    Ok(vk_format)
}

/// Where layer `index` of a checked export (`import_layers`) is: its object, that object's DRM
/// format modifier, and the plane's explicit layout for `VkImageDrmFormatModifierExplicitCreateInfoEXT`
/// (offset and row pitch in the object; the sizes 0, as the specification asks).
pub(super) fn layer_layout(descriptor: &crate::dmabuf::Descriptor, index: usize) -> (usize, u64, vk::SubresourceLayout) {
    let layer = &descriptor.layers[index];
    let object = layer.object_index[0] as usize;
    let layout = vk::SubresourceLayout {
        offset: u64::from(layer.offset[0]),
        size: 0,
        row_pitch: u64::from(layer.pitch[0]),
        array_pitch: 0,
        depth_pitch: 0,
    };
    (object, descriptor.objects[object].drm_format_modifier, layout)
}

/// The size of layer `index` of a 4:2:0 picture of `width` x `height`: chroma at half size,
/// rounded up, as FFmpeg sizes its planes and `Planes` its chroma image.
pub(super) fn layer_extent(index: usize, width: u32, height: u32) -> (u32, u32) {
    if index == 0 {
        (width, height)
    } else {
        (width.div_ceil(2), height.div_ceil(2))
    }
}

/// The memory type an imported DMA-BUF is allocated in: one the file descriptor can be imported as
/// (`vkGetMemoryFdPropertiesKHR`) and the image can be bound to, device-local first; none when no
/// type is both.
pub(super) fn import_memory_type(fd_types: u32, image_types: u32, types: &[vk::MemoryPropertyFlags]) -> Option<u32> {
    let usable = fd_types & image_types;
    let candidates = || (0..types.len().min(32)).filter(move |&index| usable & (1 << index) != 0);
    candidates()
        .find(|&index| types[index].contains(vk::MemoryPropertyFlags::DEVICE_LOCAL))
        .or_else(|| candidates().next())
        .map(|index| index as u32)
}

/// Readies a kept import's frame for its surface's next picture: owned by VA-API again
/// (`VK_QUEUE_FAMILY_EXTERNAL`, in GENERAL, nothing to make available), as the copy's release left
/// it (`after_copy`), whatever an earlier picture of it did not get to. The semaphores keep their
/// values: the copy waits for the last one and signals one more.
pub(super) fn ready_for_reuse(frame: &mut Frame) {
    for index in 0..frame.img.len() {
        if frame.img[index] != vk::Image::null() {
            frame.layout[index] = vk::ImageLayout::GENERAL;
            frame.queue_family[index] = vk::QUEUE_FAMILY_EXTERNAL;
            frame.access.set(index, vk::AccessFlags::empty());
        }
    }
}

pub(super) const fn fourcc(code: &[u8; 4]) -> u32 {
    u32::from_le_bytes(*code)
}

/// The DRM format FFmpeg's DMA-BUF import (hwcontext_vulkan's table) knows a 4:4:4 VAAPI surface's
/// layer by: AYUV as XYUV8888 and Y410 as XVYU2101010, the same bytes with the alpha ignored,
/// which import as R8G8B8A8 and A2R10G10B10. Other formats stay.
pub(super) fn drm_alias(format: u32) -> u32 {
    match format {
        f if f == fourcc(b"AYUV") => fourcc(b"XYUV"),
        f if f == fourcc(b"Y410") => fourcc(b"XV30"),
        f => f,
    }
}

/// Maps the 4:4:4 VAAPI picture `source` into `target`, a Vulkan frame of `VaapiMap`'s frames
/// context, through DRM PRIME as hwcontext_vulkan's own VAAPI map does (FFmpeg's export syncs the
/// surface first), with each layer named as the import knows it (`drm_alias`). The mapped frame
/// keeps the DRM frame, and that the VAAPI picture, until it is freed.
///
/// # Safety
/// `source` must be a live VAAPI frame and `target` set up for `VaapiMap`'s frames context.
unsafe fn map_interleaved(target: *mut AVFrame, source: *const AVFrame) -> Result<(), String> {
    // SAFETY: Caller guarantees `source` is a live VAAPI frame and `target` is configured for `VaapiMap`'s frames context.
    unsafe {
        let mut drm = av_frame_alloc();
        if drm.is_null() {
            return Err("FFmpeg cannot allocate a DRM PRIME frame".into());
        }
        (*drm).format = AV_PIX_FMT_DRM_PRIME;
        let status = av_hwframe_map(drm, source, AV_HWFRAME_MAP_READ as c_int);
        if status < 0 {
            av_frame_free(&mut drm);
            return Err(format!("FFmpeg cannot export the 4:4:4 VAAPI picture as DMA-BUFs (error {status})"));
        }
        let descriptor = &mut *(*drm).data[0].cast::<DrmFrameDescriptor>();
        let layers = descriptor.nb_layers.clamp(0, 4) as usize;
        for layer in &mut descriptor.layers[..layers] {
            layer.format = drm_alias(layer.format);
        }
        let status = av_hwframe_map(target, drm, AV_HWFRAME_MAP_READ as c_int);
        av_frame_free(&mut drm);
        if status < 0 {
            return Err(format!("FFmpeg cannot map the 4:4:4 VAAPI picture's DMA-BUFs into a Vulkan image (error {status}); the driver may refuse its format or modifier"));
        }
        Ok(())
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

/// A device UUID as CUDA and Vulkan print it (8-4-4-4-12 hex digits).
fn uuid_text(uuid: [u8; 16]) -> String {
    let hex: String = uuid.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &hex[..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..])
}

/// Whether CUDA's device (`cuda`, `cuDeviceGetUuid`) is the renderer's Vulkan device (`vulkan`,
/// `VkPhysicalDeviceIDProperties::deviceUUID`); NVIDIA's drivers give one GPU the same UUID in both.
pub fn same_gpu(vulkan: [u8; 16], cuda: [u8; 16]) -> Result<(), String> {
    if vulkan == cuda {
        Ok(())
    } else {
        Err(format!(
            "NVDEC decodes on another GPU than the renderer's (CUDA device {}, the renderer's Vulkan device {}), so CUDA cannot copy its pictures into the renderer's images",
            uuid_text(cuda),
            uuid_text(vulkan)
        ))
    }
}

/// The UUID of the GPU the CUDA device context `cuda` runs on, asked of libcuda (which FFmpeg
/// already loaded for it) inside that context.
///
/// # Safety
/// `cuda` must be null or a live `AV_HWDEVICE_TYPE_CUDA` device context.
pub(super) unsafe fn cuda_uuid(cuda: *mut AVBufferRef) -> Result<[u8; 16], String> {
    type Call1 = unsafe extern "C" fn(*mut c_void) -> c_int;
    type PopCall = unsafe extern "C" fn(*mut *mut c_void) -> c_int;
    type DeviceCall = unsafe extern "C" fn(*mut c_int) -> c_int;
    type UuidCall = unsafe extern "C" fn(*mut [u8; 16], c_int) -> c_int;
    // SAFETY: Caller guarantees `cuda` is null or a live `AV_HWDEVICE_TYPE_CUDA` device context.
    unsafe {
        if cuda.is_null() || (*cuda).data.is_null() {
            return Err("the NVDEC decoder has no CUDA device context".into());
        }
        let context = &*(*(*cuda).data.cast::<AVHWDeviceContext>()).hwctx.cast::<CudaDeviceContext>();
        let library = libloading::Library::new("libcuda.so.1").map_err(|e| format!("libcuda.so.1 cannot be loaded ({e})"))?;
        let symbol = |name: &[u8]| {
            library.get::<*mut c_void>(name).map(|symbol| *symbol).map_err(|e| format!("libcuda lacks {} ({e})", String::from_utf8_lossy(&name[..name.len() - 1])))
        };
        let push = std::mem::transmute::<*mut c_void, Call1>(symbol(b"cuCtxPushCurrent_v2\0")?);
        let pop = std::mem::transmute::<*mut c_void, PopCall>(symbol(b"cuCtxPopCurrent_v2\0")?);
        let device_of = std::mem::transmute::<*mut c_void, DeviceCall>(symbol(b"cuCtxGetDevice\0")?);
        let uuid_of = std::mem::transmute::<*mut c_void, UuidCall>(symbol(b"cuDeviceGetUuid_v2\0").or_else(|_| symbol(b"cuDeviceGetUuid\0"))?);
        let status = push(context.cuda_ctx);
        if status != 0 {
            return Err(format!("the NVDEC decoder's CUDA context cannot be made current (CUDA error {status})"));
        }
        let (mut device, mut uuid) = (0, [0u8; 16]);
        let mut status = device_of(&mut device);
        if status == 0 {
            status = uuid_of(&mut uuid, device);
        }
        let mut popped = ptr::null_mut();
        pop(&mut popped);
        if status != 0 {
            return Err(format!("CUDA does not name the GPU NVDEC decodes on (CUDA error {status})"));
        }
        Ok(uuid)
    }
}

/// `AV_VK_FRAME_FLAG_DISABLE_MULTIPLANE`: one image per plane, as CUDA imports no multi-plane image.
pub(super) const DISABLE_MULTIPLANE: c_int = 1 << 2;

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
    /// For the NVDEC decoder whose CUDA device context is `cuda` (`AVCodecContext::hw_device_ctx`):
    /// refused up front, with both GPUs named, when CUDA runs on another GPU than the renderer's,
    /// where every copy would fail.
    ///
    /// # Safety
    /// As `device_context`, and `cuda` must be a live `AV_HWDEVICE_TYPE_CUDA` device context.
    pub unsafe fn new(gpu: *mut SDL_GPUDevice, cuda: *mut AVBufferRef) -> Result<Self, String> {
        // SAFETY: Caller guarantees `gpu` is live and outlives the context and `cuda` is a live CUDA device context.
        unsafe {
            let renderer = made_with(gpu);
            cuda_plan(renderer.as_ref().map(|&(_, extensions)| extensions).map_err(Clone::clone))?;
            let (sdl, extensions) = renderer?;
            let instance = ash::Instance::load(
                &ash::StaticFn {
                    get_instance_proc_addr: zero_copy::get_instance_proc_addr()?,
                },
                sdl.instance,
            );
            let mut ids = vk::PhysicalDeviceIDProperties::default();
            let mut properties = vk::PhysicalDeviceProperties2::default().push_next(&mut ids);
            instance.get_physical_device_properties2(sdl.physical_device, &mut properties);
            same_gpu(ids.device_uuid, cuda_uuid(cuda)?)?;
            let families = queue_families(&instance, sdl.physical_device);
            Ok(Self {
                device: share(&sdl, extensions, &listed_families(&families, sdl.family, 0))?,
                frames: ptr::null_mut(),
                shape: (0, 0, AV_PIX_FMT_NONE),
            })
        }
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
        // SAFETY: Caller guarantees `source` is a live CUDA frame and `target` is an empty AVFrame.
        unsafe {
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
pub(super) unsafe fn interop_frames(context: &mut AVHWFramesContext, shape: (c_int, c_int, AVPixelFormat)) {
    context.format = AV_PIX_FMT_VULKAN;
    context.sw_format = shape.2;
    context.width = shape.0;
    context.height = shape.1;
    context.initial_pool_size = 0;
    // SAFETY: Caller guarantees `context` is a newly allocated Vulkan frames context whose `hwctx` is a `FramesContext`.
    unsafe { (*context.hwctx.cast::<FramesContext>()).flags |= DISABLE_MULTIPLANE };
}

/// The mark of `VaapiMap`'s frames contexts (`AVHWFramesContext::user_opaque`).
pub(super) static MAPPED: [u8; 1] = [0];

/// Whether pictures of this frames context are mapped VAAPI pictures (`VaapiMap`), which the copy
/// keeps until it is done (`Held`) whatever FFmpeg did to the image's queue family and layout.
pub(super) fn mapped_frames(context: &AVHWFramesContext) -> bool {
    ptr::eq(context.user_opaque.cast_const().cast::<u8>(), MAPPED.as_ptr())
}
