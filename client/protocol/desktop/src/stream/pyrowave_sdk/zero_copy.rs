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
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ash::vk::{self, Handle};
use sdl3_sys::everything::*;

use super::{DeviceQueueInfo, GpuBuffers, GpuConfig, ImageView, SharedDeviceCreateInfo};

/// Set to 1, the GPU decoder reads its planes back even where it could share the renderer's device.
pub const FORCE_READBACK: &str = "NIGHTJAR_PYROWAVE_READBACK";

/// Whether `FORCE_READBACK`'s value asks for the readback: only "1" does.
pub fn readback_forced(value: Option<&OsStr>) -> bool {
    value.is_some_and(|value| value == "1")
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
/// codec.
pub fn plan_video(devices: &[DeviceCaps]) -> Result<crate::vulkan_video::Extensions, String> {
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
    let usable = video.usable();
    if usable.is_empty() {
        return Err("the Vulkan devices SDL picks from lack the video decoding extensions".into());
    }
    Ok(usable)
}

/// The video extensions the GPU renderer's request would ask for (`plan_video` of the devices the
/// loader lists), or why Vulkan video cannot be used.
pub fn video_devices() -> Result<crate::vulkan_video::Extensions, String> {
    plan_video(&query_devices()?)
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

unsafe fn caps_of(instance: &ash::Instance, device: vk::PhysicalDevice) -> DeviceCaps {
    let mut subgroup = vk::PhysicalDeviceSubgroupProperties::default();
    let mut properties = vk::PhysicalDeviceProperties2::default().push_next(&mut subgroup);
    instance.get_physical_device_properties2(device, &mut properties);
    let properties = properties.properties;
    let mut chain = Chain::new();
    instance.get_physical_device_features2(device, &mut chain.pdf2);
    DeviceCaps {
        name: properties.device_name_as_c_str().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default(),
        kind: properties.device_type,
        api: properties.api_version,
        subgroup_ops: subgroup.supported_operations,
        supported: chain.features(),
        video: crate::vulkan_video::extensions_of(instance, device),
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
    /// The request for the devices the loader lists, or why the renderer should not make it. Either
    /// half is asked for without the other, the reason for a missing one in the log.
    pub fn new() -> Result<Box<Self>, String> {
        let devices = query_devices()?;
        let (features, video) = Self::halves(plan(&devices), plan_video(&devices))?;
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

    /// The features and video extensions to ask for from PyroWave's `plan` and `plan_video`:
    /// both halves' features (`plan_video` checked every candidate has Vulkan video's); without
    /// PyroWave's, only the ones Vulkan video needs; without video, no extensions. Each missing
    /// half is logged; an error when both are.
    fn halves(pyrowave: Result<Features, String>, video: Result<crate::vulkan_video::Extensions, String>) -> Result<(Features, crate::vulkan_video::Extensions), String> {
        match (pyrowave, video) {
            (Ok(features), Ok(video)) => Ok((Features(features.0 | Features::video().0), video)),
            (Ok(features), Err(why)) => {
                crate::vulkan_video::note(&format!("the GPU renderer is made without Vulkan video: {why}"));
                Ok((features, Default::default()))
            }
            (Err(why), Ok(video)) => {
                note(&format!("the GPU renderer is made without PyroWave's Vulkan features, so PyroWave GPU decoding will read its planes back: {why}"));
                Ok((Features::video(), video))
            }
            (Err(pyrowave), Err(video)) => Err(format!("{pyrowave}; and for Vulkan video, {video}")),
        }
    }

    pub fn features(&self) -> Features {
        self.features
    }

    /// The Vulkan video extensions asked for; with any, the renderer also asks SDL for its decode
    /// queues (`crate::vulkan_video::DECODE_QUEUES_PROPERTY`).
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
        let device: *mut SDL_GPUDevice = crate::sdl::renderer_pointer(renderer, SDL_PROP_RENDERER_GPU_DEVICE_POINTER).cast();
        if device.is_null() || driver_of(device) != "vulkan" {
            return false;
        }
        let props = SDL_GetGPUDeviceProperties(device);
        SDL_SetNumberProperty(props, FEATURES_PROPERTY.as_ptr(), i64::from(self.features.0))
            && SDL_SetNumberProperty(props, crate::vulkan_video::EXTENSIONS_PROPERTY.as_ptr(), i64::from(self.video.0))
    }
}

unsafe fn driver_of(device: *mut SDL_GPUDevice) -> String {
    let driver = SDL_GetGPUDeviceDriver(device);
    if driver.is_null() { String::new() } else { CStr::from_ptr(driver).to_string_lossy().into_owned() }
}

/// The renderer's Vulkan device as the patched SDL shares it, and the features it was made with.
#[derive(Clone, Copy, Debug)]
pub struct SdlVulkan {
    pub instance: vk::Instance,
    pub physical_device: vk::PhysicalDevice,
    pub device: vk::Device,
    pub queue: vk::Queue,
    pub family: u32,
    /// The SDL_Mutex SDL holds around every use of `queue`.
    pub lock: *mut SDL_Mutex,
    pub features: Features,
}

impl SdlVulkan {
    /// # Safety
    /// `device` must be null or a live SDL GPU device.
    pub unsafe fn of_device(device: *mut SDL_GPUDevice) -> Result<Self, String> {
        if device.is_null() {
            return Err("the renderer has no SDL GPU device".into());
        }
        Self::from_properties(SDL_GetGPUDeviceProperties(device), &driver_of(device))
    }

    /// Reads the device of `props`, the properties of an SDL GPU device whose driver is `driver`.
    ///
    /// # Safety
    /// `props` must be a live properties group.
    pub unsafe fn from_properties(props: SDL_PropertiesID, driver: &str) -> Result<Self, String> {
        if driver != "vulkan" {
            return Err(format!("the renderer draws with {driver:?}, not Vulkan"));
        }
        let handles = [INSTANCE, PHYSICAL_DEVICE, DEVICE, QUEUE, QUEUE_LOCK].map(|name| SDL_GetPointerProperty(props, name.as_ptr(), ptr::null_mut()));
        if handles.iter().any(|handle| handle.is_null()) || !SDL_HasProperty(props, QUEUE_FAMILY.as_ptr()) {
            return Err("this SDL does not share its Vulkan device (the Nightjar SDL patch is missing)".into());
        }
        if !SDL_HasProperty(props, FEATURES_PROPERTY.as_ptr()) {
            return Err("the renderer's Vulkan device was created without the decoders' features".into());
        }
        let features = Features(SDL_GetNumberProperty(props, FEATURES_PROPERTY.as_ptr(), 0) as u32);
        Ok(Self {
            instance: vk::Instance::from_raw(handles[0] as u64),
            physical_device: vk::PhysicalDevice::from_raw(handles[1] as u64),
            device: vk::Device::from_raw(handles[2] as u64),
            queue: vk::Queue::from_raw(handles[3] as u64),
            family: SDL_GetNumberProperty(props, QUEUE_FAMILY.as_ptr(), 0) as u32,
            lock: handles[4].cast(),
            features,
        })
    }

    /// Whether the device was created with `needed` (PyroWave's `Features::required`, or
    /// `Features::video`); the error names the ones it lacks.
    pub fn require(&self, needed: Features) -> Result<(), String> {
        let missing = needed.without(self.features);
        if missing.is_empty() { Ok(()) } else { Err(format!("the renderer's Vulkan device was created without {}", missing.names().join(", "))) }
    }
}

/// SDL's `vkGetInstanceProcAddr`, from the Vulkan loader the GPU device was made with.
pub fn get_instance_proc_addr() -> Result<vk::PFN_vkGetInstanceProcAddr, String> {
    // SAFETY: A plain SDL query; both function types are vkGetInstanceProcAddr's C signature.
    unsafe {
        SDL_Vulkan_GetVkGetInstanceProcAddr()
            .map(|function| std::mem::transmute::<_, vk::PFN_vkGetInstanceProcAddr>(function))
            .ok_or_else(|| format!("SDL has no Vulkan loader ({})", crate::sdl::error()))
    }
}

unsafe extern "C" fn lock_queue(mutex: *mut std::ffi::c_void) {
    SDL_LockMutex(mutex.cast());
}

unsafe extern "C" fn unlock_queue(mutex: *mut std::ffi::c_void) {
    SDL_UnlockMutex(mutex.cast());
}

/// What `pyrowave_create_device` is told about the renderer's device: Vulkan 1.3 without
/// extensions of ours, its one queue, and exactly the features it was made with (Granite treats
/// the chain as the features it may use). Boxed, as it points into itself; it must outlive the SDK
/// device.
pub struct ShareInfo {
    app: vk::ApplicationInfo<'static>,
    instance: vk::InstanceCreateInfo<'static>,
    priority: f32,
    queue: vk::DeviceQueueCreateInfo<'static>,
    chain: Chain,
    device: vk::DeviceCreateInfo<'static>,
    queues: DeviceQueueInfo,
    pub create: SharedDeviceCreateInfo,
}

impl ShareInfo {
    pub fn new(sdl: &SdlVulkan, get_instance_proc_addr: vk::PFN_vkGetInstanceProcAddr) -> Box<Self> {
        let mut info = Box::new(Self {
            app: vk::ApplicationInfo::default().api_version(vk::API_VERSION_1_3),
            instance: vk::InstanceCreateInfo::default(),
            priority: 1.0,
            queue: vk::DeviceQueueCreateInfo::default().queue_family_index(sdl.family),
            chain: Chain::default(),
            device: vk::DeviceCreateInfo::default(),
            queues: DeviceQueueInfo { queue: sdl.queue, family_index: sdl.family, index: 0 },
            create: SharedDeviceCreateInfo {
                get_instance_proc_addr,
                instance: sdl.instance,
                physical_device: sdl.physical_device,
                device: sdl.device,
                instance_create_info: ptr::null(),
                device_create_info: ptr::null(),
                queue_info: ptr::null_mut(),
                queue_info_count: 1,
                queue_lock_callback: Some(lock_queue),
                queue_unlock_callback: Some(unlock_queue),
                userdata: sdl.lock.cast(),
            },
        });
        info.chain.link();
        info.chain.enable(sdl.features);
        info.instance.p_application_info = &raw const info.app;
        info.queue.queue_count = 1;
        info.queue.p_queue_priorities = &raw const info.priority;
        info.device.queue_create_info_count = 1;
        info.device.p_queue_create_infos = &raw const info.queue;
        info.device.p_next = (&raw const info.chain.pdf2).cast();
        info.create.instance_create_info = &raw const info.instance;
        info.create.device_create_info = &raw const info.device;
        info.create.queue_info = &raw mut info.queues;
        info
    }
}

/// How long dropping the images waits for the GPU to be done with them before leaving them allocated.
const RELEASE_WAIT: Duration = Duration::from_secs(2);

/// The plane sets on the renderer's device: three device-local R8 or R16 images per set, in
/// GENERAL layout from creation on (the decoder writes them there, and the presenter's SDL
/// textures expect it of a compute-storage texture), and the two timelines that order them.
pub struct SharedImages {
    device: ash::Device,
    queue: vk::Queue,
    lock: *mut SDL_Mutex,
    family: u32,
    config: GpuConfig,
    /// (image, memory) per plane, set by set.
    planes: Vec<(vk::Image, vk::DeviceMemory)>,
    /// Signalled by each decode once its planes are written.
    decode: vk::Semaphore,
    /// Signalled by the presenter once its copy out of a set is done.
    reuse: vk::Semaphore,
    /// The last value a decode queued a signal of, and the last reuse value signalled.
    decoded: AtomicU64,
    reused: AtomicU64,
}

// SAFETY: Vulkan handles may be used from any thread; the queue only under SDL's lock.
unsafe impl Send for SharedImages {}
unsafe impl Sync for SharedImages {}

impl SharedImages {
    /// Creates `config.slots` plane sets on `sdl`'s device and moves them to GENERAL layout.
    ///
    /// # Safety
    /// `sdl` must describe a live device that outlives the images.
    pub unsafe fn new(sdl: &SdlVulkan, get_instance_proc_addr: vk::PFN_vkGetInstanceProcAddr, config: GpuConfig) -> Result<Arc<Self>, String> {
        let instance = ash::Instance::load(&ash::StaticFn { get_instance_proc_addr }, sdl.instance);
        let memory = instance.get_physical_device_memory_properties(sdl.physical_device);
        let device = ash::Device::load(instance.fp_v1_0(), sdl.device);
        let mut images = Self {
            device, queue: sdl.queue, lock: sdl.lock, family: sdl.family, config, planes: Vec::new(),
            decode: vk::Semaphore::null(), reuse: vk::Semaphore::null(), decoded: AtomicU64::new(0), reused: AtomicU64::new(0),
        };
        for semaphore in [&mut images.decode, &mut images.reuse] {
            let mut timeline = vk::SemaphoreTypeCreateInfo::default().semaphore_type(vk::SemaphoreType::TIMELINE);
            *semaphore = images.device.create_semaphore(&vk::SemaphoreCreateInfo::default().push_next(&mut timeline), None)
                .map_err(|e| format!("a timeline semaphore cannot be created ({e})"))?;
        }
        for _ in 0..config.slots {
            for plane in 0..3 {
                let (width, height) = config.plane_size(plane);
                let image = images.device.create_image(&vk::ImageCreateInfo::default()
                    .image_type(vk::ImageType::TYPE_2D).format(images.format()).extent(vk::Extent3D { width, height, depth: 1 })
                    .mip_levels(1).array_layers(1).samples(vk::SampleCountFlags::TYPE_1).tiling(vk::ImageTiling::OPTIMAL)
                    .usage(vk::ImageUsageFlags::STORAGE | vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_SRC | vk::ImageUsageFlags::TRANSFER_DST)
                    .sharing_mode(vk::SharingMode::EXCLUSIVE).initial_layout(vk::ImageLayout::UNDEFINED), None)
                    .map_err(|e| format!("a {width}x{height} plane image cannot be created ({e})"))?;
                images.planes.push((image, vk::DeviceMemory::null()));
                let needs = images.device.get_image_memory_requirements(image);
                let kinds = &memory.memory_types[..memory.memory_type_count as usize];
                let usable = |index: &usize| needs.memory_type_bits & (1 << index) != 0;
                let kind = (0..kinds.len()).filter(usable).find(|&index| kinds[index].property_flags.contains(vk::MemoryPropertyFlags::DEVICE_LOCAL))
                    .or_else(|| (0..kinds.len()).find(usable))
                    .ok_or("no memory type holds the plane images")?;
                let allocated = images.device.allocate_memory(&vk::MemoryAllocateInfo::default().allocation_size(needs.size).memory_type_index(kind as u32), None)
                    .map_err(|e| format!("{} bytes of plane memory cannot be allocated ({e})", needs.size))?;
                images.planes.last_mut().unwrap().1 = allocated;
                images.device.bind_image_memory(image, allocated, 0).map_err(|e| format!("the plane memory cannot be bound ({e})"))?;
            }
        }
        if let Err(e) = images.to_general() {
            // The transition may still be pending on a hung queue: nothing it uses may be freed.
            std::mem::forget(images);
            return Err(e);
        }
        Ok(Arc::new(images))
    }

    fn format(&self) -> vk::Format {
        if self.config.ten_bit { vk::Format::R16_UNORM } else { vk::Format::R8_UNORM }
    }

    pub fn config(&self) -> GpuConfig {
        self.config
    }

    pub fn image(&self, set: usize, plane: usize) -> vk::Image {
        self.planes[set * 3 + plane].0
    }

    /// The decode timeline and the reuse timeline.
    pub fn semaphores(&self) -> (vk::Semaphore, vk::Semaphore) {
        (self.decode, self.reuse)
    }

    /// Set `set` as the decoder's output planes.
    pub fn views(&self, set: usize) -> GpuBuffers {
        let format = self.format();
        GpuBuffers {
            planes: [0, 1, 2].map(|plane| {
                let (width, height) = self.config.plane_size(plane);
                ImageView {
                    image: self.image(set, plane), width, height, image_format: format, view_format: format, mip_level: 0, layer: 0,
                    aspect: vk::ImageAspectFlags::COLOR, swizzle: vk::ComponentSwizzle::IDENTITY, layout: vk::ImageLayout::GENERAL,
                }
            }),
        }
    }

    /// A decode queued its signal of `value` on the decode timeline.
    pub fn decode_queued(&self, value: u64) {
        self.decoded.fetch_max(value, Ordering::AcqRel);
    }

    /// Waits up to `timeout` for the decode that signals `value`.
    pub fn wait_decode(&self, value: u64, timeout: Duration) -> Result<(), String> {
        let semaphores = [self.decode];
        let values = [value];
        // SAFETY: The semaphore lives as long as self.
        match unsafe { self.device.wait_semaphores(&vk::SemaphoreWaitInfo::default().semaphores(&semaphores).values(&values), timeout.as_nanos() as u64) } {
            Ok(()) => Ok(()),
            Err(vk::Result::TIMEOUT) => Err(format!("the PyroWave decode did not finish within {timeout:?}: the Vulkan device may be hung or lost")),
            Err(e) => Err(format!("waiting for the PyroWave decode failed ({e})")),
        }
    }

    /// Signals the next value on the reuse timeline once everything queued so far is done, the
    /// presenter's copy out of a set among it, and returns the value. Under SDL's queue lock, so
    /// it follows SDL's last submission and the values reach the queue in order.
    pub fn signal_reuse(&self) -> Result<u64, String> {
        // SAFETY: The queue is used under SDL's lock; the structs outlive the call.
        unsafe {
            SDL_LockMutex(self.lock);
            let value = self.reused.load(Ordering::Acquire) + 1;
            let semaphores = [self.reuse];
            let values = [value];
            let mut timeline = vk::TimelineSemaphoreSubmitInfo::default().signal_semaphore_values(&values);
            let submit = vk::SubmitInfo::default().signal_semaphores(&semaphores).push_next(&mut timeline);
            let result = self.device.queue_submit(self.queue, &[submit], vk::Fence::null());
            if result.is_ok() { self.reused.store(value, Ordering::Release); }
            SDL_UnlockMutex(self.lock);
            result.map(|()| value).map_err(|e| format!("the plane copy's signal cannot be submitted ({e})"))
        }
    }

    /// Moves every image from UNDEFINED to GENERAL with a command buffer of ours, and waits for it.
    unsafe fn to_general(&self) -> Result<(), String> {
        let device = &self.device;
        let pool = device.create_command_pool(&vk::CommandPoolCreateInfo::default().queue_family_index(self.family), None)
            .map_err(|e| format!("a command pool cannot be created ({e})"))?;
        let fence = match device.create_fence(&vk::FenceCreateInfo::default(), None) {
            Ok(fence) => fence,
            Err(e) => { device.destroy_command_pool(pool, None); return Err(format!("a fence cannot be created ({e})")); }
        };
        let mut submitted = false;
        let mut run = || -> Result<(), vk::Result> {
            let commands = device.allocate_command_buffers(&vk::CommandBufferAllocateInfo::default().command_pool(pool).level(vk::CommandBufferLevel::PRIMARY).command_buffer_count(1))?[0];
            device.begin_command_buffer(commands, &vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT))?;
            let barriers: Vec<_> = self.planes.iter().map(|&(image, _)| vk::ImageMemoryBarrier::default()
                .dst_access_mask(vk::AccessFlags::SHADER_READ | vk::AccessFlags::SHADER_WRITE | vk::AccessFlags::TRANSFER_READ)
                .old_layout(vk::ImageLayout::UNDEFINED).new_layout(vk::ImageLayout::GENERAL)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED).dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .image(image).subresource_range(vk::ImageSubresourceRange { aspect_mask: vk::ImageAspectFlags::COLOR, base_mip_level: 0, level_count: 1, base_array_layer: 0, layer_count: 1 }))
                .collect();
            device.cmd_pipeline_barrier(commands, vk::PipelineStageFlags::TOP_OF_PIPE, vk::PipelineStageFlags::COMPUTE_SHADER | vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(), &[], &[], &barriers);
            device.end_command_buffer(commands)?;
            let buffers = [commands];
            SDL_LockMutex(self.lock);
            let queued = device.queue_submit(self.queue, &[vk::SubmitInfo::default().command_buffers(&buffers)], fence);
            SDL_UnlockMutex(self.lock);
            submitted = queued.is_ok();
            queued?;
            device.wait_for_fences(&[fence], true, RELEASE_WAIT.as_nanos() as u64)
        };
        let result = run();
        if result.is_ok() || !submitted {
            // Done, or never submitted. A submission still pending keeps both.
            device.destroy_fence(fence, None);
            device.destroy_command_pool(pool, None);
        }
        result.map_err(|e| format!("the plane images cannot be prepared ({e})"))
    }
}

impl Drop for SharedImages {
    fn drop(&mut self) {
        // SAFETY: Handles made in `new`; nothing is freed while the GPU may still use it.
        unsafe {
            if self.decode != vk::Semaphore::null() && self.reuse != vk::Semaphore::null() {
                let semaphores = [self.decode, self.reuse];
                let values = [self.decoded.load(Ordering::Acquire), self.reused.load(Ordering::Acquire)];
                let waited = self.device.wait_semaphores(&vk::SemaphoreWaitInfo::default().semaphores(&semaphores).values(&values), RELEASE_WAIT.as_nanos() as u64);
                if let Err(e) = waited {
                    eprintln!("nightjar: the GPU did not finish with the PyroWave plane images ({e}), so they stay allocated");
                    return;
                }
            }
            for &(image, memory) in &self.planes {
                self.device.destroy_image(image, None);
                if memory != vk::DeviceMemory::null() { self.device.free_memory(memory, None); }
            }
            for semaphore in [self.decode, self.reuse] {
                if semaphore != vk::Semaphore::null() { self.device.destroy_semaphore(semaphore, None); }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps(name: &str, kind: vk::PhysicalDeviceType, supported: Features) -> DeviceCaps {
        DeviceCaps { name: name.into(), kind, api: vk::API_VERSION_1_3, subgroup_ops: SUBGROUP_OPS, supported, video: Default::default() }
    }

    fn with(names: &[&str]) -> Features {
        Features(FEATURES.iter().enumerate().filter(|(_, feature)| names.contains(&feature.name)).fold(0, |bits, (bit, _)| bits | 1 << bit))
    }

    #[test]
    fn only_four_features_are_required() {
        assert_eq!(Features::required().names(), ["subgroupSizeControl", "computeFullSubgroups", "synchronization2", "timelineSemaphore"]);
        assert_eq!(Features::all().names().len(), FEATURES.len());
    }

    #[test]
    fn a_device_with_everything_gets_everything_asked_for() {
        assert_eq!(plan(&[caps("RX 7900 XT", vk::PhysicalDeviceType::DISCRETE_GPU, Features::all())]), Ok(Features::all()));
    }

    #[test]
    fn the_optional_features_asked_for_are_the_ones_every_candidate_has() {
        let a = Features::required().0 | with(&["shaderFloat16", "shaderInt16"]).0;
        let b = Features::required().0 | with(&["shaderInt16", "storageBuffer8BitAccess"]).0;
        let planned = plan(&[caps("a", vk::PhysicalDeviceType::DISCRETE_GPU, Features(a)), caps("b", vk::PhysicalDeviceType::DISCRETE_GPU, Features(b))]).unwrap();
        assert_eq!(planned, Features(Features::required().0 | with(&["shaderInt16"]).0));
    }

    #[test]
    fn the_video_extensions_asked_for_are_the_ones_every_candidate_has_and_only_ones_that_decode() {
        use crate::vulkan_video::Extensions;
        let discrete = vk::PhysicalDeviceType::DISCRETE_GPU;
        let with_video = |name, kind, video| DeviceCaps { video: Extensions(video), ..caps(name, kind, Features::all()) };
        let rtx = with_video("RTX 4070", discrete, 0b11111);
        assert_eq!(plan_video(&[rtx.clone(), with_video("GTX 1660", discrete, 0b01111)]), Ok(Extensions(0b01111)), "no AV1 for either");
        // SDL picks the discrete GPU, so the integrated one's lack of video does not matter.
        assert_eq!(plan_video(&[rtx.clone(), with_video("iGPU", vk::PhysicalDeviceType::INTEGRATED_GPU, 0)]), Ok(Extensions(0b11111)));
        let none = Err("the Vulkan devices SDL picks from lack the video decoding extensions".to_string());
        assert_eq!(plan_video(&[with_video("queues only", discrete, 0b00011)]), none, "queues without a codec");
        assert_eq!(plan_video(&[with_video("a", discrete, 0b11111), with_video("b", discrete, 0b11100)]), none, "b has no queues");
        assert_eq!(plan_video(&[]), Err("Vulkan lists no devices".into()));
        // Apart from PyroWave: a device without its subgroup operations and features decodes video
        // with synchronization2 and timeline semaphores alone, and not without them.
        let narrow = DeviceCaps { subgroup_ops: vk::SubgroupFeatureFlags::BASIC, supported: Features::video(), ..rtx.clone() };
        assert!(plan(std::slice::from_ref(&narrow)).is_err());
        assert_eq!(plan_video(&[narrow]), Ok(Extensions(0b11111)));
        let error = plan_video(&[DeviceCaps { supported: with(&["synchronization2"]), ..rtx.clone() }]).unwrap_err();
        assert!(error.contains("RTX 4070 lacks timelineSemaphore"), "{error}");
        let error = plan_video(&[DeviceCaps { api: vk::make_api_version(0, 1, 2, 0), ..rtx }]).unwrap_err();
        assert!(error.contains("supports Vulkan 1.2, FFmpeg's Vulkan video needs 1.3"), "{error}");
    }

    #[test]
    fn each_half_of_the_request_is_asked_for_without_the_other() {
        use crate::vulkan_video::Extensions;
        assert_eq!(Features::video().names(), ["synchronization2", "timelineSemaphore", "samplerYcbcrConversion"]);
        let both = RendererRequest::halves(Ok(Features::all()), Ok(Extensions(0b111)));
        assert_eq!(both, Ok((Features::all(), Extensions(0b111))));
        // A device whose PyroWave plan has only the required features still gets video's for FFmpeg.
        let both = RendererRequest::halves(Ok(Features::required()), Ok(Extensions(0b111)));
        assert_eq!(both, Ok((Features(Features::required().0 | Features::video().0), Extensions(0b111))));
        assert_eq!(Features::video().without(Features::required()).names(), ["samplerYcbcrConversion"]);
        assert_eq!(RendererRequest::halves(Ok(Features::all()), Err("no video".into())), Ok((Features::all(), Extensions(0))));
        assert_eq!(RendererRequest::halves(Err("no subgroups".into()), Ok(Extensions(0b111))), Ok((Features::video(), Extensions(0b111))));
        assert_eq!(RendererRequest::halves(Err("a".into()), Err("b".into())), Err("a; and for Vulkan video, b".into()));
    }

    #[test]
    fn only_devices_of_the_kind_sdl_picks_count() {
        // llvmpipe beside a GPU: SDL never picks the CPU device, so what it lacks does not matter.
        let cpu = DeviceCaps { api: vk::API_VERSION_1_1, ..caps("llvmpipe", vk::PhysicalDeviceType::CPU, Features::default()) };
        let gpu = caps("Iris Plus 655", vk::PhysicalDeviceType::INTEGRATED_GPU, Features::all());
        assert_eq!(plan(&[cpu.clone(), gpu]), Ok(Features::all()));
        // Alone, the CPU device is the one SDL picks, and it cannot share.
        let error = plan(&[cpu]).unwrap_err();
        assert!(error.contains("llvmpipe") && error.contains("1.1"), "{error}");
        // A discrete GPU beats the integrated one, so its gaps decide.
        let weak = caps("dGPU", vk::PhysicalDeviceType::DISCRETE_GPU, with(&["subgroupSizeControl", "synchronization2"]));
        let error = plan(&[caps("iGPU", vk::PhysicalDeviceType::INTEGRATED_GPU, Features::all()), weak]).unwrap_err();
        assert!(error.contains("dGPU lacks computeFullSubgroups, timelineSemaphore"), "{error}");
    }

    #[test]
    fn a_device_without_the_subgroup_operations_or_vulkan_1_3_is_named_with_what_it_lacks() {
        let old = DeviceCaps { api: vk::make_api_version(0, 1, 2, 0), ..caps("old", vk::PhysicalDeviceType::DISCRETE_GPU, Features::all()) };
        assert!(plan(&[old]).unwrap_err().contains("old supports Vulkan 1.2, sharing it needs 1.3"));
        let narrow = DeviceCaps { subgroup_ops: SUBGROUP_OPS & !vk::SubgroupFeatureFlags::SHUFFLE_RELATIVE, ..caps("narrow", vk::PhysicalDeviceType::DISCRETE_GPU, Features::all()) };
        let error = plan(&[narrow]).unwrap_err();
        assert!(error.contains("narrow lacks subgroup operations") && error.contains("0x20"), "{error}");
        assert_eq!(plan(&[]), Err("Vulkan lists no devices".into()));
    }

    #[test]
    fn features_go_into_the_chain_and_come_back_out() {
        let mut chain = Chain::new();
        assert_eq!(chain.features(), Features::default());
        let features = with(&["timelineSemaphore", "shaderInt16", "storageBuffer16BitAccess", "computeFullSubgroups"]);
        chain.enable(features);
        assert_eq!(chain.features(), features);
        assert_eq!((chain.v12.timeline_semaphore, chain.pdf2.features.shader_int16, chain.v11.storage_buffer16_bit_access, chain.v13.compute_full_subgroups), (vk::TRUE, vk::TRUE, vk::TRUE, vk::TRUE));
        assert_eq!(chain.v12.shader_float16, vk::FALSE);
        // Linked in place, PDF2 first, as Vulkan and Granite read it.
        assert_eq!(chain.pdf2.p_next, (&raw mut chain.v11).cast());
        assert_eq!(chain.v12.p_next, (&raw mut chain.v13).cast());
        assert!(chain.v13.p_next.is_null());
    }

    #[test]
    fn only_one_forces_the_readback() {
        assert!(readback_forced(Some(OsStr::new("1"))));
        for value in [None, Some(""), Some("0"), Some("true"), Some("11")] {
            assert!(!readback_forced(value.map(OsStr::new)), "{value:?}");
        }
    }

    /// The renderer's device as SDL shares it: every handle, the queue lock and our feature mark,
    /// or the reason the stream reads back instead.
    #[test]
    fn the_shared_device_needs_vulkan_the_patch_and_the_features_mark() {
        // SAFETY: Properties groups made and destroyed here (SDL's properties need no SDL_Init).
        unsafe {
            let props = SDL_CreateProperties();
            assert!(props.0 != 0, "{}", crate::sdl::error());
            let mut fake = [0u8; 5];
            let error = SdlVulkan::from_properties(props, "vulkan").unwrap_err();
            assert!(error.contains("SDL patch is missing"), "{error}");
            for (name, handle) in [INSTANCE, PHYSICAL_DEVICE, DEVICE, QUEUE, QUEUE_LOCK].into_iter().zip(fake.iter_mut()) {
                SDL_SetPointerProperty(props, name.as_ptr(), (handle as *mut u8).cast());
            }
            SDL_SetNumberProperty(props, QUEUE_FAMILY.as_ptr(), 2);
            let error = SdlVulkan::from_properties(props, "vulkan").unwrap_err();
            assert!(error.contains("created without the decoders' features"), "{error}");
            // Each decoder checks the features it needs.
            SDL_SetNumberProperty(props, FEATURES_PROPERTY.as_ptr(), i64::from(with(&["timelineSemaphore"]).0));
            let sdl = SdlVulkan::from_properties(props, "vulkan").unwrap();
            let error = sdl.require(Features::required()).unwrap_err();
            assert!(error.contains("without subgroupSizeControl, computeFullSubgroups, synchronization2"), "{error}");
            assert_eq!(sdl.require(Features::video()), Err("the renderer's Vulkan device was created without synchronization2, samplerYcbcrConversion".into()));
            SDL_SetNumberProperty(props, FEATURES_PROPERTY.as_ptr(), i64::from(Features::video().0));
            let sdl = SdlVulkan::from_properties(props, "vulkan").unwrap();
            assert_eq!(sdl.require(Features::video()), Ok(()));
            assert!(sdl.require(Features::required()).is_err());
            SDL_SetNumberProperty(props, FEATURES_PROPERTY.as_ptr(), i64::from(Features::all().0));
            let error = SdlVulkan::from_properties(props, "direct3d12").unwrap_err();
            assert!(error.contains("not Vulkan"), "{error}");
            let sdl = SdlVulkan::from_properties(props, "vulkan").unwrap();
            assert_eq!((sdl.family, sdl.features, sdl.device.as_raw()), (2, Features::all(), (&raw const fake[2]) as u64));
            assert_eq!(sdl.lock, (&raw mut fake[4]).cast());
            SDL_DestroyProperties(props);
        }
    }

    #[test]
    fn the_share_info_points_into_itself_and_claims_exactly_the_device_features() {
        let sdl = SdlVulkan {
            instance: vk::Instance::from_raw(1), physical_device: vk::PhysicalDevice::from_raw(2), device: vk::Device::from_raw(3),
            queue: vk::Queue::from_raw(4), family: 5, lock: ptr::null_mut(), features: Features(Features::required().0 | with(&["shaderInt8"]).0),
        };
        unsafe extern "system" fn loader(_: vk::Instance, _: *const std::ffi::c_char) -> vk::PFN_vkVoidFunction { None }
        let info = ShareInfo::new(&sdl, loader);
        // SAFETY: Every pointer is into the box, alive here.
        unsafe {
            let device = &*info.create.device_create_info;
            let queue = &*device.p_queue_create_infos;
            assert_eq!((device.queue_create_info_count, queue.queue_family_index, queue.queue_count, *queue.p_queue_priorities), (1, 5, 1, 1.0));
            let pdf2 = &*device.p_next.cast::<vk::PhysicalDeviceFeatures2>();
            assert_eq!(pdf2.s_type.as_raw(), vk::StructureType::PHYSICAL_DEVICE_FEATURES_2.as_raw());
            let v11 = &*pdf2.p_next.cast::<vk::PhysicalDeviceVulkan11Features>();
            let v12 = &*v11.p_next.cast::<vk::PhysicalDeviceVulkan12Features>();
            assert_eq!((v12.shader_int8, v12.timeline_semaphore, v12.shader_float16), (vk::TRUE, vk::TRUE, vk::FALSE));
            let app = &*(*info.create.instance_create_info).p_application_info;
            assert_eq!((app.api_version, (*info.create.instance_create_info).enabled_extension_count), (vk::API_VERSION_1_3, 0));
            let queues = &*info.create.queue_info;
            assert_eq!((info.create.queue_info_count, queues.queue.as_raw(), queues.family_index, queues.index), (1, 4, 5, 0));
            assert!(info.create.queue_lock_callback.is_some() && info.create.queue_unlock_callback.is_some());
        }
    }
}
