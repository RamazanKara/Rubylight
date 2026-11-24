//! Shared Vulkan device handles read from SDL GPU properties (`SdlVulkan`) and the create-info
//! payload passed to `pyrowave_create_device` (`ShareInfo`).

use std::ptr;

use ash::vk::{self, Handle};
use sdl3_sys::everything::*;

use super::super::queue::{QueueSlot, RendererCompute};
use super::super::{DeviceQueueInfo, SharedDeviceCreateInfo};
use super::{
    driver_of, Chain, Features, COMPUTE_FAMILY, COMPUTE_INDEX, COMPUTE_QUEUE, DEVICE, FEATURES_PROPERTY, INSTANCE,
    PHYSICAL_DEVICE, QUEUE, QUEUE_FAMILY, QUEUE_LOCK,
};

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
    /// The queue SDL made for PyroWave's decodes beside its own (`COMPUTE_QUEUE_PROPERTY`).
    pub compute: RendererCompute,
}

impl SdlVulkan {
    /// # Safety
    /// `device` must be null or a live SDL GPU device.
    pub unsafe fn of_device(device: *mut SDL_GPUDevice) -> Result<Self, String> {
        if device.is_null() {
            return Err("the renderer has no SDL GPU device".into());
        }
        // SAFETY: `device` is non-null and live per caller's guarantee.
        unsafe { Self::from_properties(SDL_GetGPUDeviceProperties(device), &driver_of(device)) }
    }

    /// Reads the device of `props`, the properties of an SDL GPU device whose driver is `driver`.
    ///
    /// # Safety
    /// `props` must be a live properties group.
    pub unsafe fn from_properties(props: SDL_PropertiesID, driver: &str) -> Result<Self, String> {
        if driver != "vulkan" {
            return Err(format!("the renderer draws with {driver:?}, not Vulkan"));
        }
        // SAFETY: Caller guarantees `props` is a live SDL properties group.
        unsafe {
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
                compute: compute_of(props),
            })
        }
    }

    /// Whether the device was created with `needed` (PyroWave's `Features::required`, or
    /// `Features::video`); the error names the ones it lacks.
    pub fn require(&self, needed: Features) -> Result<(), String> {
        let missing = needed.without(self.features);
        if missing.is_empty() { Ok(()) } else { Err(format!("the renderer's Vulkan device was created without {}", missing.names().join(", "))) }
    }
}

/// The compute queue the device of `props` was made with for PyroWave (`COMPUTE_QUEUE_PROPERTY`).
///
/// # Safety
/// `props` must be a live properties group.
pub(super) unsafe fn compute_of(props: SDL_PropertiesID) -> RendererCompute {
    // SAFETY: Caller guarantees `props` is a live SDL properties group.
    unsafe {
        if !SDL_HasProperty(props, COMPUTE_FAMILY.as_ptr()) {
            return RendererCompute::NotAsked;
        }
        let family = SDL_GetNumberProperty(props, COMPUTE_FAMILY.as_ptr(), -1);
        let index = SDL_GetNumberProperty(props, COMPUTE_INDEX.as_ptr(), 0);
        let queue = SDL_GetPointerProperty(props, COMPUTE_QUEUE.as_ptr(), ptr::null_mut());
        // The patch takes the first free queue of a family, beside at most one of its own (index 1).
        match (u32::try_from(family), queue.is_null(), index) {
            (Ok(family), false, 0..=1) => RendererCompute::Queue(vk::Queue::from_raw(queue as u64), QueueSlot { family, index: index as u32 }),
            _ => RendererCompute::None,
        }
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
    // SAFETY: Caller passes a valid `SDL_Mutex` pointer from `SdlVulkan::lock`.
    unsafe { SDL_LockMutex(mutex.cast()) };
}

unsafe extern "C" fn unlock_queue(mutex: *mut std::ffi::c_void) {
    // SAFETY: Caller passes a valid `SDL_Mutex` pointer from `SdlVulkan::lock`.
    unsafe { SDL_UnlockMutex(mutex.cast()) };
}

/// What `pyrowave_create_device` is told about the renderer's device: Vulkan 1.3 without
/// extensions of ours, its queue (and the compute queue SDL made for the decodes, when they run
/// there), and exactly the features it was made with (Granite treats the chain as the features it
/// may use, and picks its queues from the queue create infos: with the compute queue listed, its
/// compute and transfer queues are that one). Boxed, as it points into itself; it must outlive
/// the SDK device.
pub struct ShareInfo {
    app: vk::ApplicationInfo<'static>,
    instance: vk::InstanceCreateInfo<'static>,
    priorities: [f32; 2],
    queue_infos: [vk::DeviceQueueCreateInfo<'static>; 2],
    chain: Chain,
    device: vk::DeviceCreateInfo<'static>,
    queues: [DeviceQueueInfo; 2],
    pub create: SharedDeviceCreateInfo,
}

impl ShareInfo {
    /// With `compute`, the compute queue (and where it is) the decodes run on.
    pub fn new(sdl: &SdlVulkan, get_instance_proc_addr: vk::PFN_vkGetInstanceProcAddr, compute: Option<(vk::Queue, QueueSlot)>) -> Box<Self> {
        let graphics = DeviceQueueInfo { queue: sdl.queue, family_index: sdl.family, index: 0 };
        let mut info = Box::new(Self {
            app: vk::ApplicationInfo::default().api_version(vk::API_VERSION_1_3),
            instance: vk::InstanceCreateInfo::default(),
            priorities: [1.0; 2],
            queue_infos: [vk::DeviceQueueCreateInfo { queue_family_index: sdl.family, queue_count: 1, ..Default::default() }; 2],
            chain: Chain::default(),
            device: vk::DeviceCreateInfo::default(),
            queues: [graphics; 2],
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
        let mut families = 1;
        if let Some((queue, slot)) = compute {
            info.queues[1] = DeviceQueueInfo { queue, family_index: slot.family, index: slot.index };
            info.create.queue_info_count = 2;
            if slot.family == sdl.family {
                // A second queue of SDL's family: its create info has both.
                info.queue_infos[0].queue_count = slot.index + 1;
            } else {
                info.queue_infos[1] = vk::DeviceQueueCreateInfo { queue_family_index: slot.family, queue_count: slot.index + 1, ..Default::default() };
                families = 2;
            }
        }
        info.chain.link();
        info.chain.enable(sdl.features);
        info.instance.p_application_info = &raw const info.app;
        let priorities = (&raw const info.priorities).cast::<f32>();
        for queue_info in &mut info.queue_infos {
            queue_info.p_queue_priorities = priorities;
        }
        info.device.queue_create_info_count = families;
        info.device.p_queue_create_infos = info.queue_infos.as_ptr();
        info.device.p_next = (&raw const info.chain.pdf2).cast();
        info.create.instance_create_info = &raw const info.instance;
        info.create.device_create_info = &raw const info.device;
        info.create.queue_info = info.queues.as_mut_ptr();
        info
    }
}
