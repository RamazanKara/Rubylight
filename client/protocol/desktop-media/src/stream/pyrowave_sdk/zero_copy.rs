//! Linux GPU decoding without the plane readback: the SDK decodes on the SDL GPU renderer's own
//! Vulkan device, into images the presenter copies from on the GPU (`planes_gpu.rs`).
//!
//! SDL makes the renderer's device with only the features SDL itself uses, so the renderer asks
//! for the decoder's Vulkan 1.3 features when it is created (`RendererRequest`, called from
//! `sdl::create_renderer`). SDL refuses every device that lacks a requested feature, so `plan`
//! asks only when the devices SDL picks from all have the required ones, and only for the optional
//! ones they all have; the renderer marks its device with what it was made with. A stream then
//! shares that device with the SDK (`pyrowave_create_device`, `ShareInfo`), with SDL's queue mutex
//! as the SDK's queue lock, and decodes into `SharedImages`: three images per plane set, kept in
//! GENERAL layout. One timeline semaphore of ours orders the presenter's copy after the decode
//! (the decoder waits for it on the CPU before it hands the picture on, as the readback did), a
//! second the next decode into a set after the copy that read it (`Pool::after`).
//!
//! Whatever is missing (an older device, a renderer made without the features, a refused share)
//! falls back to the readback, with the reason in the log (`note`), never silently.
//! `NIGHTJAR_PYROWAVE_READBACK=1` forces the readback for A/B measurements; the renderer and its
//! device are the same either way.

use std::ffi::{CStr, OsStr};
use std::ptr;
use std::sync::Mutex;

use ash::vk;
#[cfg(test)]
use ash::vk::Handle;
use sdl3_sys::everything::*;

mod images;
mod share;

pub use images::SharedImages;
pub use share::{get_instance_proc_addr, SdlVulkan, ShareInfo};

#[cfg(test)]
use images::sharing;
#[cfg(test)]
use share::compute_of;

/// Set to 1, the GPU decoder reads its planes back even where it could share the renderer's device.
pub const FORCE_READBACK: &str = "NIGHTJAR_PYROWAVE_READBACK";

/// Whether `FORCE_READBACK`'s value asks for the readback: only "1" does.
pub fn readback_forced(value: Option<&OsStr>) -> bool {
    value.is_some_and(|value| value == "1")
}

/// Set to 0, the presenter copies each plane set out of the decoder's images, as before, instead
/// of sampling them in place: the way out should a driver mishandle the in-place layouts.
pub const SAMPLE_PLANES: &str = "NIGHTJAR_PYROWAVE_SAMPLE_PLANES";

/// How the presenter reads a plane set on the renderer's device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaneHandoff {
    /// It samples the decoder's images in place, holding the set while the picture is shown
    /// (`gpu_planes::Handover`), so the ring has a set more.
    Sample,
    /// It copies them into textures of its own and gives the set back behind the copy.
    Copy,
}

impl PlaneHandoff {
    /// What `SAMPLE_PLANES`'s value asks for: sampling in place, unless it is 0.
    pub fn of(value: Option<&OsStr>) -> Self {
        if value.is_some_and(|value| value == "0") { Self::Copy } else { Self::Sample }
    }

    pub fn from_environment() -> Self {
        Self::of(std::env::var_os(SAMPLE_PLANES).as_deref())
    }

    /// The plane sets the decoder makes for `slots` pictures in flight: one more while the
    /// presenter holds the shown picture's set, so the decoder never waits for it.
    pub fn sets(self, slots: usize) -> usize {
        match self {
            Self::Sample => slots + 1,
            Self::Copy => slots,
        }
    }

    /// How the log says it.
    pub fn line(self) -> String {
        match self {
            Self::Sample => "the presenter samples the decoder's plane images in place".into(),
            Self::Copy => format!("the presenter copies the decoder's plane images ({SAMPLE_PLANES}=0)"),
        }
    }
}

/// The last line `note` wrote.
static NOTED: Mutex<Option<String>> = Mutex::new(None);

/// Logs which way the GPU decoder's planes go and why, once until it changes.
pub fn note(line: &str) {
    let mut noted = NOTED.lock().unwrap_or_else(|e| e.into_inner());
    if noted.as_deref() != Some(line) {
        eprintln!("nightjar: {line}");
        *noted = Some(line.to_owned());
    }
}

// The patched SDL's device properties (vendor/sdl3-src/patches/vulkan-interop.patch).
const INSTANCE: &CStr = c"SDL.gpu.device.vulkan.instance";
const PHYSICAL_DEVICE: &CStr = c"SDL.gpu.device.vulkan.physical_device";
const DEVICE: &CStr = c"SDL.gpu.device.vulkan.device";
const QUEUE: &CStr = c"SDL.gpu.device.vulkan.queue";
const QUEUE_FAMILY: &CStr = c"SDL.gpu.device.vulkan.queue_family_index";
const QUEUE_LOCK: &CStr = c"SDL.gpu.device.vulkan.queue_lock";
/// Asks SDL for one more queue for PyroWave's decodes at the device's creation (`queue.rs`).
pub const COMPUTE_QUEUE_PROPERTY: &CStr = c"SDL.gpu.device.create.vulkan.compute_queue";
/// That queue, its family (-1: asked for, but there is none) and its index in the family.
const COMPUTE_QUEUE: &CStr = c"SDL.gpu.device.vulkan.compute_queue";
const COMPUTE_FAMILY: &CStr = c"SDL.gpu.device.vulkan.compute_queue_family_index";
const COMPUTE_INDEX: &CStr = c"SDL.gpu.device.vulkan.compute_queue_index";
/// Ours: the `Features` the renderer's device was created with, set by `RendererRequest::mark`.
pub const FEATURES_PROPERTY: &CStr = c"nightjar.pyrowave.vulkan_features";
/// Wraps a VkImage as an SDL GPU texture (the same patch).
pub const TEXTURE_IMAGE_PROPERTY: &CStr = c"SDL.gpu.texture.create.vulkan.image";

/// The subgroup operations the decoder's shaders use (pyrowave.h).
const SUBGROUP_OPS: vk::SubgroupFeatureFlags = vk::SubgroupFeatureFlags::from_raw(
    vk::SubgroupFeatureFlags::BASIC.as_raw()
        | vk::SubgroupFeatureFlags::VOTE.as_raw()
        | vk::SubgroupFeatureFlags::ARITHMETIC.as_raw()
        | vk::SubgroupFeatureFlags::BALLOT.as_raw()
        | vk::SubgroupFeatureFlags::SHUFFLE.as_raw()
        | vk::SubgroupFeatureFlags::SHUFFLE_RELATIVE.as_raw(),
);

/// The feature structs Vulkan 1.3 reports and enables, linked in place (so boxed or inside a box).
#[derive(Default)]
pub struct Chain {
    pub pdf2: vk::PhysicalDeviceFeatures2<'static>,
    pub v11: vk::PhysicalDeviceVulkan11Features<'static>,
    pub v12: vk::PhysicalDeviceVulkan12Features<'static>,
    pub v13: vk::PhysicalDeviceVulkan13Features<'static>,
}

impl Chain {
    pub fn new() -> Box<Self> {
        let mut chain = Box::<Self>::default();
        chain.link();
        chain
    }

    /// Points each struct at the next; again after the chain moved.
    pub fn link(&mut self) {
        self.pdf2.p_next = (&raw mut self.v11).cast();
        self.v11.p_next = (&raw mut self.v12).cast();
        self.v12.p_next = (&raw mut self.v13).cast();
        self.v13.p_next = ptr::null_mut();
    }

    /// Turns on `features` (others stay as they are).
    pub fn enable(&mut self, features: Features) {
        for (bit, feature) in FEATURES.iter().enumerate() {
            if features.0 & (1 << bit) != 0 {
                *(feature.field)(self) = vk::TRUE;
            }
        }
    }

    /// The decoder's features that are on.
    pub fn features(&mut self) -> Features {
        Features(FEATURES.iter().enumerate().filter(|(_, feature)| *(feature.field)(self) != vk::FALSE).fold(0, |bits, (bit, _)| bits | 1 << bit))
    }
}

type Field = fn(&mut Chain) -> &mut vk::Bool32;

struct Feature {
    name: &'static str,
    /// The decoder or the sharing needs it; the others it uses where the device has them, as the
    /// SDK's own device does.
    required: bool,
    field: Field,
}

/// The features the SDK's decoder and Granite use (pyrowave.h and the SDK's shaders), and the one
/// only FFmpeg's Vulkan video adds (`Features::video`), one bit each in `Features`.
const FEATURES: [Feature; 14] = [
    Feature { name: "subgroupSizeControl", required: true, field: |c| &mut c.v13.subgroup_size_control },
    Feature { name: "computeFullSubgroups", required: true, field: |c| &mut c.v13.compute_full_subgroups },
    Feature { name: "synchronization2", required: true, field: |c| &mut c.v13.synchronization2 },
    Feature { name: "timelineSemaphore", required: true, field: |c| &mut c.v12.timeline_semaphore },
    Feature { name: "shaderStorageImageWriteWithoutFormat", required: false, field: |c| &mut c.pdf2.features.shader_storage_image_write_without_format },
    Feature { name: "shaderStorageImageExtendedFormats", required: false, field: |c| &mut c.pdf2.features.shader_storage_image_extended_formats },
    Feature { name: "shaderInt16", required: false, field: |c| &mut c.pdf2.features.shader_int16 },
    Feature { name: "storageBuffer16BitAccess", required: false, field: |c| &mut c.v11.storage_buffer16_bit_access },
    Feature { name: "uniformAndStorageBuffer16BitAccess", required: false, field: |c| &mut c.v11.uniform_and_storage_buffer16_bit_access },
    Feature { name: "storageBuffer8BitAccess", required: false, field: |c| &mut c.v12.storage_buffer8_bit_access },
    Feature { name: "uniformAndStorageBuffer8BitAccess", required: false, field: |c| &mut c.v12.uniform_and_storage_buffer8_bit_access },
    Feature { name: "shaderFloat16", required: false, field: |c| &mut c.v12.shader_float16 },
    Feature { name: "shaderInt8", required: false, field: |c| &mut c.v12.shader_int8 },
    Feature { name: "samplerYcbcrConversion", required: false, field: |c| &mut c.v11.sampler_ycbcr_conversion },
];

/// A set of the decoder's features, bit n for `FEATURES[n]`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Features(pub u32);

impl Features {
    pub fn required() -> Self {
        Self(FEATURES.iter().enumerate().filter(|(_, feature)| feature.required).fold(0, |bits, (bit, _)| bits | 1 << bit))
    }

    pub fn all() -> Self {
        Self((1 << FEATURES.len()) - 1)
    }

    /// The ones FFmpeg's Vulkan video needs (`crate::vulkan_video`): it submits with
    /// vkQueueSubmit2, orders its pictures with timeline semaphores and (FFmpeg 7.1) makes a
    /// Y'CbCr conversion for its picture views.
    pub fn video() -> Self {
        Self(FEATURES.iter().enumerate().filter(|(_, feature)| matches!(feature.name, "synchronization2" | "timelineSemaphore" | "samplerYcbcrConversion")).fold(0, |bits, (bit, _)| bits | 1 << bit))
    }

    pub fn and(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }

    pub fn without(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub fn names(self) -> Vec<&'static str> {
        FEATURES.iter().enumerate().filter(|(bit, _)| self.0 & (1 << bit) != 0).map(|(_, feature)| feature.name).collect()
    }
}

/// What a Vulkan device offers the decoder.
#[derive(Clone)]
pub struct DeviceCaps {
    pub name: String,
    pub kind: vk::PhysicalDeviceType,
    pub api: u32,
    pub subgroup_ops: vk::SubgroupFeatureFlags,
    pub supported: Features,
    /// Its Vulkan video extensions, for FFmpeg's Vulkan video decoding (`crate::vulkan_video`).
    pub video: crate::vulkan_video::Extensions,
}

/// SDL's order of preference among Vulkan devices (the GPU renderer asks for performance).
fn rank(kind: vk::PhysicalDeviceType) -> u32 {
    match kind {
        vk::PhysicalDeviceType::DISCRETE_GPU => 4,
        vk::PhysicalDeviceType::INTEGRATED_GPU => 3,
        vk::PhysicalDeviceType::VIRTUAL_GPU => 2,
        vk::PhysicalDeviceType::CPU => 1,
        _ => 0,
    }
}

/// The features the renderer should ask for, or why it should not ask: SDL picks a device of the
/// best kind present, so every device of that kind needs Vulkan 1.3, the decoder's subgroup
/// operations and the required features. The optional features asked for are the ones they all
/// have, so no device SDL could pick is refused for them.
pub fn plan(devices: &[DeviceCaps]) -> Result<Features, String> {
    let best = devices.iter().map(|device| rank(device.kind)).max().ok_or("Vulkan lists no devices")?;
    let mut features = Features::all();
    for device in devices.iter().filter(|device| rank(device.kind) == best) {
        let name = &device.name;
        if device.api < vk::API_VERSION_1_3 {
            return Err(format!("{name} supports Vulkan {}.{}, sharing it needs 1.3", vk::api_version_major(device.api), vk::api_version_minor(device.api)));
        }
        let ops = SUBGROUP_OPS & !device.subgroup_ops;
        if !ops.is_empty() {
            return Err(format!("{name} lacks subgroup operations (missing bits {:#x} of VkSubgroupFeatureFlags)", ops.as_raw()));
        }
        let missing = Features::required().without(device.supported);
        if !missing.is_empty() {
            return Err(format!("{name} lacks {}", missing.names().join(", ")));
        }
        features = features.and(device.supported);
    }
    Ok(features)
}

/// The Vulkan video extensions the renderer should ask for, or why it should not ask, apart from
/// PyroWave's `plan`: every device SDL could pick needs Vulkan 1.3 and `Features::video`, and the
/// extensions asked for are the ones they all have, so none is refused for them, if they decode a
/// codec; the DMA-BUF import extensions VAAPI's pictures are mapped with, if they all have
/// them (`crate::vulkan_video::VaapiMap`); and the CUDA interop ones NVDEC's pictures are copied
/// into Vulkan images with, if they all have them (`crate::vulkan_video::CudaMap`), with or
/// without the others.
pub fn plan_video(devices: &[DeviceCaps]) -> Result<crate::vulkan_video::Extensions, String> {
    plan_video_for(devices, crate::vulkan_video::Extensions::wanted(true, true))
}

/// `plan_video` for the `wanted` extensions (`Extensions::wanted`): the video ones that decode a
/// codec and the import ones, those that are wanted.
pub fn plan_video_for(devices: &[DeviceCaps], wanted: crate::vulkan_video::Extensions) -> Result<crate::vulkan_video::Extensions, String> {
    let best = devices.iter().map(|device| rank(device.kind)).max().ok_or("Vulkan lists no devices")?;
    let mut video = crate::vulkan_video::Extensions(u32::MAX);
    for device in devices.iter().filter(|device| rank(device.kind) == best) {
        let name = &device.name;
        if device.api < vk::API_VERSION_1_3 {
            return Err(format!("{name} supports Vulkan {}.{}, FFmpeg's Vulkan video needs 1.3", vk::api_version_major(device.api), vk::api_version_minor(device.api)));
        }
        let missing = Features::video().without(device.supported);
        if !missing.is_empty() {
            return Err(format!("{name} lacks {}", missing.names().join(", ")));
        }
        video = video.and(device.video);
    }
    let video = video.and(wanted);
    let (usable, mapping, interop) = (video.usable(), video.mapping(), video.interop());
    // Interop alone is enough: NVDEC's pictures are copied into the device by CUDA (`CudaMap`),
    // and with no video or import extensions neither Vulkan video (`device_context` checks
    // `usable`) nor VAAPI's mapping (`map_plan` checks `mapping`) is tried on it.
    if usable.is_empty() && mapping.is_empty() && interop.is_empty() {
        return Err(if wanted.mapping().is_empty() {
            "the Vulkan devices SDL picks from lack the video decoding extensions and the CUDA interop ones".into()
        } else {
            "the Vulkan devices SDL picks from lack the video decoding extensions, the DMA-BUF import ones and the CUDA interop ones".into()
        });
    }
    Ok(crate::vulkan_video::Extensions(usable.0 | mapping.0 | interop.0))
}

/// The video extensions the GPU renderer's request would ask for (`plan_video` of the devices the
/// loader lists), or why Vulkan video cannot be used: a plan without the video decoding extensions
/// (the CUDA interop ones alone) is no Vulkan video.
pub fn video_devices() -> Result<crate::vulkan_video::Extensions, String> {
    vulkan_video_of(plan_video_for(&query_devices()?, crate::vulkan_video::Extensions::wanted(true, false)))
}

/// `video_devices` of a plan: only one with the video decoding extensions decodes with Vulkan video.
fn vulkan_video_of(planned: Result<crate::vulkan_video::Extensions, String>) -> Result<crate::vulkan_video::Extensions, String> {
    let planned = planned?;
    if planned.usable().is_empty() {
        return Err("the Vulkan devices SDL picks from lack the video decoding extensions".into());
    }
    Ok(planned)
}

/// The Vulkan devices the loader lists, as `plan` reads them.
fn query_devices() -> Result<Vec<DeviceCaps>, String> {
    // SAFETY: Plain Vulkan calls on handles made here; the instance is destroyed before returning.
    unsafe {
        let entry = ash::Entry::load().map_err(|e| format!("libvulkan.so.1 cannot be loaded ({e})"))?;
        let version = entry.try_enumerate_instance_version().ok().flatten().unwrap_or(vk::API_VERSION_1_0);
        if version < vk::API_VERSION_1_3 {
            return Err(format!("the Vulkan loader supports {}.{}, sharing the renderer's device needs 1.3", vk::api_version_major(version), vk::api_version_minor(version)));
        }
        let app = vk::ApplicationInfo::default().api_version(vk::API_VERSION_1_3);
        let instance = entry
            .create_instance(&vk::InstanceCreateInfo::default().application_info(&app), None)
            .map_err(|e| format!("the Vulkan loader cannot create an instance ({e})"))?;
        let devices = instance.enumerate_physical_devices().map_err(|e| format!("Vulkan lists no devices ({e})"));
        let caps = devices.map(|devices| devices.into_iter().map(|device| caps_of(&instance, device)).collect());
        instance.destroy_instance(None);
        caps
    }
}

/// # Safety
/// `instance` and `device` must be valid Vulkan handles from `ash`.
pub unsafe fn caps_of(instance: &ash::Instance, device: vk::PhysicalDevice) -> DeviceCaps {
    let mut subgroup = vk::PhysicalDeviceSubgroupProperties::default();
    let mut properties = vk::PhysicalDeviceProperties2::default().push_next(&mut subgroup);
    let mut chain = Chain::new();
    // SAFETY: Caller guarantees `instance` and `device` are valid Vulkan handles.
    unsafe {
        instance.get_physical_device_properties2(device, &mut properties);
        instance.get_physical_device_features2(device, &mut chain.pdf2);
    }
    let properties = properties.properties;
    DeviceCaps {
        name: properties.device_name_as_c_str().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default(),
        kind: properties.device_type,
        api: properties.api_version,
        subgroup_ops: subgroup.supported_operations,
        supported: chain.features(),
        // SAFETY: Caller guarantees `instance` and `device` are valid Vulkan handles.
        video: unsafe { crate::vulkan_video::extensions_of(instance, device) },
    }
}

/// The Vulkan options the renderer is created with so the decoders can share its device: PyroWave's
/// features, and the Vulkan video extensions with SDL's extra decode queues for FFmpeg
/// (`crate::vulkan_video`).
pub struct RendererRequest {
    features: Features,
    video: crate::vulkan_video::Extensions,
    chain: Box<Chain>,
    extensions: Vec<*const std::ffi::c_char>,
    options: SDL_GPUVulkanOptions,
}

impl RendererRequest {
    /// The request for the devices the loader lists, with PyroWave's half when `pyrowave` and
    /// Vulkan video's when `video`, or why the renderer should not make it. Either half is asked
    /// for without the other, the reason for a missing one in the log.
    pub fn new(pyrowave: bool, video: bool) -> Result<Box<Self>, String> {
        let devices = query_devices()?;
        // The video extensions unless NIGHTJAR_VULKAN_VIDEO=0, the import ones unless VAAPI's
        // pictures are not to be mapped.
        let wanted = crate::vulkan_video::Extensions::wanted(
            crate::vulkan_video::Choice::from_environment() != crate::vulkan_video::Choice::Off,
            crate::vulkan_video::map_switched_on(std::env::var_os(crate::vulkan_video::MAP_ENVIRONMENT).as_deref()),
        );
        let (features, video) = Self::halves(pyrowave.then(|| plan(&devices)), video.then(|| plan_video_for(&devices, wanted)))?;
        let mut chain = Chain::new();
        chain.enable(features);
        let extensions = video.names().iter().map(|name| name.as_ptr()).collect();
        let mut request = Box::new(Self { features, video, chain, extensions, options: SDL_GPUVulkanOptions::default() });
        request.options.vulkan_api_version = vk::API_VERSION_1_3;
        // SDL reads the Vulkan 1.0 features from their own pointer and the 1.1 to 1.3 structs from the list.
        request.options.feature_list = (&raw mut request.chain.v11).cast();
        request.options.vulkan_10_physical_device_features = (&raw mut request.chain.pdf2.features).cast();
        request.options.device_extension_count = request.extensions.len() as u32;
        request.options.device_extension_names = request.extensions.as_mut_ptr();
        Ok(request)
    }

    /// The features and video extensions to ask for from PyroWave's `plan` and `plan_video` (None:
    /// that half is not wanted): both halves' features (`plan_video` checked every candidate has
    /// Vulkan video's); without PyroWave's, only the ones Vulkan video needs; without video, no
    /// extensions, PyroWave's plan alone. A wanted half that is missing is logged; an error when
    /// no half is left.
    fn halves(pyrowave: Option<Result<Features, String>>, video: Option<Result<crate::vulkan_video::Extensions, String>>) -> Result<(Features, crate::vulkan_video::Extensions), String> {
        match (pyrowave, video) {
            (Some(Ok(features)), Some(Ok(video))) => Ok((Features(features.0 | Features::video().0), video)),
            (Some(Ok(features)), video) => {
                if let Some(Err(why)) = video {
                    crate::vulkan_video::note(&format!("the GPU renderer is made without Vulkan video: {why}"));
                }
                Ok((features, Default::default()))
            }
            (pyrowave, Some(Ok(video))) => {
                if let Some(Err(why)) = pyrowave {
                    note(&format!("the GPU renderer is made without PyroWave's Vulkan features, so PyroWave GPU decoding will read its planes back: {why}"));
                }
                Ok((Features::video(), video))
            }
            (Some(Err(pyrowave)), Some(Err(video))) => Err(format!("{pyrowave}; and for Vulkan video, {video}")),
            (Some(Err(pyrowave)), None) => Err(pyrowave),
            (None, Some(Err(video))) => Err(format!("for Vulkan video, {video}")),
            (None, None) => Err("neither PyroWave nor Vulkan video decodes on the renderer's device".into()),
        }
    }

    pub fn features(&self) -> Features {
        self.features
    }

    /// Whether the request has PyroWave's half (its required features), so the SDK may decode on
    /// the device and a compute queue for it is worth asking SDL for.
    pub fn pyrowave(&self) -> bool {
        has_pyrowave(self.features)
    }

    /// The Vulkan video and DMA-BUF import extensions asked for; with video ones, the renderer
    /// also asks SDL for its decode queues (`crate::vulkan_video::DECODE_QUEUES_PROPERTY`).
    pub fn video(&self) -> crate::vulkan_video::Extensions {
        self.video
    }

    /// For `SDL_PROP_GPU_DEVICE_CREATE_VULKAN_OPTIONS_POINTER`; valid while the request lives.
    pub fn options(&mut self) -> *mut SDL_GPUVulkanOptions {
        &raw mut self.options
    }

    /// Records on the renderer's GPU device that it was created with this request, for
    /// `SdlVulkan`. False if the renderer has no SDL GPU device on Vulkan.
    ///
    /// # Safety
    /// `renderer` must be a live renderer created with `options`.
    pub unsafe fn mark(&self, renderer: *mut SDL_Renderer) -> bool {
        // SAFETY: Caller guarantees `renderer` is a live SDL renderer created with `options`.
        unsafe {
            let device: *mut SDL_GPUDevice = crate::sdl::renderer_pointer(renderer, SDL_PROP_RENDERER_GPU_DEVICE_POINTER).cast();
            if device.is_null() || driver_of(device) != "vulkan" {
                return false;
            }
            let props = SDL_GetGPUDeviceProperties(device);
            SDL_SetNumberProperty(props, FEATURES_PROPERTY.as_ptr(), i64::from(self.features.0))
                && SDL_SetNumberProperty(props, crate::vulkan_video::EXTENSIONS_PROPERTY.as_ptr(), i64::from(self.video.0))
        }
    }
}

/// Whether `features` hold PyroWave's required ones (`RendererRequest::pyrowave`).
fn has_pyrowave(features: Features) -> bool {
    Features::required().without(features).is_empty()
}

unsafe fn driver_of(device: *mut SDL_GPUDevice) -> String {
    // SAFETY: Caller guarantees `device` is a live SDL GPU device.
    unsafe {
        let driver = SDL_GetGPUDeviceDriver(device);
        if driver.is_null() { String::new() } else { CStr::from_ptr(driver).to_string_lossy().into_owned() }
    }
}

#[cfg(test)]
mod tests;
