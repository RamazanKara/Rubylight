//! Device-local Vulkan plane sets (`SharedImages`), layout transitions for sampling in place,
//! and decode/reuse timeline semaphores on the renderer's Vulkan device.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use ash::vk;
use sdl3_sys::everything::*;

use super::super::{GpuBuffers, GpuConfig, ImageView};
use super::{note, PlaneHandoff, SdlVulkan};

/// The queue families the plane images are shared by: none (EXCLUSIVE, SDL's family alone) unless
/// the decodes run on a queue of another family, then SDL's and that one (CONCURRENT).
pub(super) fn sharing(sdl_family: u32, decode_family: Option<u32>) -> Vec<u32> {
    match decode_family {
        Some(family) if family != sdl_family => vec![sdl_family, family],
        _ => Vec::new(),
    }
}

/// How long dropping the images waits for the GPU to be done with them before leaving them allocated.
const RELEASE_WAIT: Duration = Duration::from_secs(2);

/// The plane sets on the renderer's device: three device-local R8 or R16 images per set, in
/// GENERAL layout from creation on (the decoder writes them there, and the presenter's copy reads
/// them as compute-storage textures there), and the two timelines that order them. A presenter
/// that samples a set in place (`PlaneHandoff::Sample`) moves it to SHADER_READ_ONLY_OPTIMAL, the
/// layout SDL expects of a sampler texture, while it holds it (`sampled`), and back to GENERAL
/// before it gives it back (`returned`).
pub struct SharedImages {
    device: ash::Device,
    queue: vk::Queue,
    lock: *mut SDL_Mutex,
    family: u32,
    config: GpuConfig,
    handoff: PlaneHandoff,
    /// (image, memory) per plane, set by set.
    planes: Vec<(vk::Image, vk::DeviceMemory)>,
    /// Signalled by each decode once its planes are written.
    decode: vk::Semaphore,
    /// Signalled by the presenter once its last read of a set (its copy, or the last draw that
    /// sampled it) is done.
    reuse: vk::Semaphore,
    /// The last value a decode queued a signal of, and the last reuse value signalled.
    decoded: AtomicU64,
    reused: AtomicU64,
    /// With `PlaneHandoff::Sample`: the pool of the layout changes, and per set the change to
    /// SHADER_READ_ONLY_OPTIMAL and the one back to GENERAL, recorded once (simultaneous use).
    transitions_pool: vk::CommandPool,
    transitions: Vec<[vk::CommandBuffer; 2]>,
}

// SAFETY: Vulkan handles may be used from any thread; the queue only under SDL's lock.
unsafe impl Send for SharedImages {}
unsafe impl Sync for SharedImages {}

impl SharedImages {
    /// Creates `config.slots` plane sets on `sdl`'s device and moves them to GENERAL layout. With
    /// `decode_family` (the family of the compute queue the decodes run on) other than SDL's, the
    /// images are shared by both families (`sharing`).
    ///
    /// # Safety
    /// `sdl` must describe a live device that outlives the images.
    pub unsafe fn new(sdl: &SdlVulkan, get_instance_proc_addr: vk::PFN_vkGetInstanceProcAddr, config: GpuConfig, decode_family: Option<u32>) -> Result<Arc<Self>, String> {
        // SAFETY: Caller guarantees `sdl` describes a live device that outlives the images.
        unsafe { Self::with_handoff(sdl, get_instance_proc_addr, config, decode_family, PlaneHandoff::Copy) }
    }

    /// `new` for a presenter that reads the sets as `handoff` says. Sampling in place falls back
    /// to the copy, said in the log, when its layout changes cannot be recorded.
    ///
    /// # Safety
    /// As `new`.
    pub unsafe fn with_handoff(sdl: &SdlVulkan, get_instance_proc_addr: vk::PFN_vkGetInstanceProcAddr, config: GpuConfig, decode_family: Option<u32>, handoff: PlaneHandoff) -> Result<Arc<Self>, String> {
        // SAFETY: Caller guarantees `sdl` describes a live Vulkan device and `get_instance_proc_addr` is valid.
        unsafe {
            let families = sharing(sdl.family, decode_family);
            let instance = ash::Instance::load(&ash::StaticFn { get_instance_proc_addr }, sdl.instance);
            let memory = instance.get_physical_device_memory_properties(sdl.physical_device);
            let device = ash::Device::load(instance.fp_v1_0(), sdl.device);
            let mut images = Self {
                device, queue: sdl.queue, lock: sdl.lock, family: sdl.family, config, handoff: PlaneHandoff::Copy, planes: Vec::new(),
                decode: vk::Semaphore::null(), reuse: vk::Semaphore::null(), decoded: AtomicU64::new(0), reused: AtomicU64::new(0),
                transitions_pool: vk::CommandPool::null(), transitions: Vec::new(),
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
                        .sharing_mode(if families.is_empty() { vk::SharingMode::EXCLUSIVE } else { vk::SharingMode::CONCURRENT })
                        .queue_family_indices(&families).initial_layout(vk::ImageLayout::UNDEFINED), None)
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
            if handoff == PlaneHandoff::Sample {
                match images.record_transitions() {
                    Ok(()) => images.handoff = PlaneHandoff::Sample,
                    Err(e) => note(&format!("the presenter copies the PyroWave planes: their layout changes for sampling in place cannot be recorded ({e})")),
                }
            }
            Ok(Arc::new(images))
        }
    }

    /// How the presenter reads the sets.
    pub fn handoff(&self) -> PlaneHandoff {
        self.handoff
    }

    /// Records each set's two layout changes for sampling in place. Nothing is submitted, so on
    /// an error the pool (and the buffers in it) can go at once.
    unsafe fn record_transitions(&mut self) -> Result<(), vk::Result> {
        // SAFETY: Caller guarantees `self.device` is live and handles are owned by `self`.
        unsafe {
            let device = &self.device;
            self.transitions_pool = device.create_command_pool(&vk::CommandPoolCreateInfo::default().queue_family_index(self.family), None)?;
            let sets = self.config.slots;
            let buffers = match device.allocate_command_buffers(&vk::CommandBufferAllocateInfo::default()
                .command_pool(self.transitions_pool).level(vk::CommandBufferLevel::PRIMARY).command_buffer_count(2 * sets as u32))
            {
                Ok(buffers) => buffers,
                Err(e) => { self.drop_transitions(); return Err(e); }
            };
            let range = vk::ImageSubresourceRange { aspect_mask: vk::ImageAspectFlags::COLOR, base_mip_level: 0, level_count: 1, base_array_layer: 0, layer_count: 1 };
            let decode_stages = vk::PipelineStageFlags::COMPUTE_SHADER | vk::PipelineStageFlags::TRANSFER;
            let decode_writes = vk::AccessFlags::SHADER_WRITE | vk::AccessFlags::TRANSFER_WRITE;
            for set in 0..sets {
                let commands = [buffers[2 * set], buffers[2 * set + 1]];
                for (index, &buffer) in commands.iter().enumerate() {
                    let to_sampled = index == 0;
                    let barriers: Vec<_> = (0..3).map(|plane| {
                        let barrier = vk::ImageMemoryBarrier::default()
                            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED).dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                            .image(self.image(set, plane)).subresource_range(range);
                        if to_sampled {
                            // Semaphore wait at `FRAGMENT_SHADER` makes the decode's writes
                            // available; chain the layout transition off that wait before reads.
                            barrier.src_access_mask(vk::AccessFlags::empty()).dst_access_mask(vk::AccessFlags::SHADER_READ)
                                .old_layout(vk::ImageLayout::GENERAL).new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                        } else {
                            // After the draws' reads (an execution dependency suffices), before the
                            // next decode's reads and writes, which wait for the reuse signal behind it.
                            barrier.src_access_mask(vk::AccessFlags::empty()).dst_access_mask(vk::AccessFlags::SHADER_READ | decode_writes)
                                .old_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL).new_layout(vk::ImageLayout::GENERAL)
                        }
                    }).collect();
                    let (from, to) = if to_sampled {
                        (vk::PipelineStageFlags::FRAGMENT_SHADER, vk::PipelineStageFlags::FRAGMENT_SHADER)
                    } else {
                        (vk::PipelineStageFlags::FRAGMENT_SHADER | vk::PipelineStageFlags::COMPUTE_SHADER, decode_stages)
                    };
                    let recorded = device.begin_command_buffer(buffer, &vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::SIMULTANEOUS_USE))
                        .and_then(|()| {
                            device.cmd_pipeline_barrier(buffer, from, to, vk::DependencyFlags::empty(), &[], &[], &barriers);
                            device.end_command_buffer(buffer)
                        });
                    if let Err(e) = recorded {
                        self.drop_transitions();
                        return Err(e);
                    }
                }
                self.transitions.push(commands);
            }
            Ok(())
        }
    }

    /// Frees the recorded layout changes (never submitted, or done).
    unsafe fn drop_transitions(&mut self) {
        if self.transitions_pool != vk::CommandPool::null() {
            // SAFETY: Caller guarantees no recorded command buffer is still pending on the GPU.
            unsafe { self.device.destroy_command_pool(self.transitions_pool, None) };
        }
        self.transitions_pool = vk::CommandPool::null();
        self.transitions.clear();
    }

    /// Makes `set` readable by the draws SDL submits after this: waits on `decode_value` on the
    /// decode timeline at `FRAGMENT_SHADER` and moves its images to `SHADER_READ_ONLY_OPTIMAL`.
    pub fn sampled(&self, set: usize, decode_value: u64) -> Result<(), String> {
        let commands = self.transitions.get(set).ok_or_else(|| format!("plane set {set} has no layout change for sampling in place"))?[0];
        let semaphores = [self.decode];
        let values = [decode_value];
        let stages = [vk::PipelineStageFlags::FRAGMENT_SHADER];
        let buffers = [commands];
        let mut timeline = vk::TimelineSemaphoreSubmitInfo::default().wait_semaphore_values(&values);
        let mut submit = vk::SubmitInfo::default().command_buffers(&buffers);
        if decode_value > 0 {
            submit = submit.wait_semaphores(&semaphores).wait_dst_stage_mask(&stages).push_next(&mut timeline);
        }
        // SAFETY: The queue is used under SDL's lock; the structs outlive the call.
        unsafe {
            SDL_LockMutex(self.lock);
            let result = self.device.queue_submit(self.queue, &[submit], vk::Fence::null());
            SDL_UnlockMutex(self.lock);
            result.map_err(|e| format!("the plane set's change for sampling cannot be submitted ({e})"))
        }
    }

    /// Moves `set` back to GENERAL behind everything SDL submitted so far (the last draw that
    /// sampled it among it), and signals the next reuse value behind that; returns the value, which
    /// the next decode into the set waits for.
    pub fn returned(&self, set: usize) -> Result<u64, String> {
        let commands = self.transitions.get(set).ok_or_else(|| format!("plane set {set} has no layout change back for the decoder"))?[1];
        self.signal_reuse_after(&[commands])
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
        self.signal_reuse_after(&[])
    }

    /// `signal_reuse` behind `commands`, submitted in the same batch.
    fn signal_reuse_after(&self, commands: &[vk::CommandBuffer]) -> Result<u64, String> {
        // SAFETY: The queue is used under SDL's lock; the structs outlive the call.
        unsafe {
            SDL_LockMutex(self.lock);
            let value = self.reused.load(Ordering::Acquire) + 1;
            let semaphores = [self.reuse];
            let values = [value];
            let mut timeline = vk::TimelineSemaphoreSubmitInfo::default().signal_semaphore_values(&values);
            let submit = vk::SubmitInfo::default().command_buffers(commands).signal_semaphores(&semaphores).push_next(&mut timeline);
            let result = self.device.queue_submit(self.queue, &[submit], vk::Fence::null());
            if result.is_ok() { self.reused.store(value, Ordering::Release); }
            SDL_UnlockMutex(self.lock);
            result.map(|()| value).map_err(|e| format!("the plane copy's signal cannot be submitted ({e})"))
        }
    }

    /// Moves every image from UNDEFINED to GENERAL with a command buffer of ours, and waits for it.
    unsafe fn to_general(&self) -> Result<(), String> {
        // SAFETY: Caller guarantees `self.device`, `self.queue`, and `self.planes` are valid and unused.
        unsafe {
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
            // Every layout change was submitted before a reuse value that was waited for above.
            self.drop_transitions();
            for &(image, memory) in &self.planes { self.device.destroy_image(image, None); if memory != vk::DeviceMemory::null() { self.device.free_memory(memory, None); } }
            for semaphore in [self.decode, self.reuse] { if semaphore != vk::Semaphore::null() { self.device.destroy_semaphore(semaphore, None); } }
        }
    }
}
