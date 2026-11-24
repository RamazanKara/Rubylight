//! Vulkan video presenter plane images (`Planes`) and GPU-to-GPU image copies on SDL's Vulkan queue.

use std::ptr;

use ash::vk::{self, Handle};
use sdl3_sys::everything::*;

use crate::media::ffi::{av_frame_clone, AVFrame, AVHWFramesContext};
use crate::stream::pyrowave_sdk::zero_copy::{self, Features, SdlVulkan, TEXTURE_IMAGE_PROPERTY};
use crate::stream::stage_times::Stages;

use super::abi::{frame_fields, frame_now, Frame, FramesContext, VkFrame};
use super::device::{interleaved_sw_format, picture_sw_format, plane_format, sw_format};
#[cfg(target_os = "linux")]
use super::sample;
use super::sample_state::{acquire, after_copy, release, Held, HeldFrame, COPY_WAIT, SLOTS};
use super::vaapi_cuda::mapped_frames;

/// The presenter's two plane images on the renderer's device (R8 and R8G8, or R16 and R16G16 for
/// 10-bit, the chroma at half or full size), or the one image of a mapped 4:4:4 VAAPI picture
/// (`interleaved`), wrapped as SDL GPU textures for the plane shader, and the copies into them.
pub struct Planes {
    pub(super) device: ash::Device,
    pub(super) gpu: *mut SDL_GPUDevice,
    pub(super) queue: vk::Queue,
    pub(super) family: u32,
    pub(super) lock: *mut SDL_Mutex,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) ten_bit: bool,
    pub(super) chroma444: bool,
    /// One R8G8B8A8 or (10-bit) A2B10G10R10 image holding Y, Cb and Cr (`interleaved_444`).
    pub(super) interleaved: bool,
    images: Vec<(vk::Image, vk::DeviceMemory)>,
    textures: [*mut SDL_GPUTexture; 2],
    pub(super) pool: vk::CommandPool,
    slots: Vec<(vk::CommandBuffer, vk::Fence)>,
    next: usize,
    /// Mapped VAAPI pictures, each until its slot's copy is done.
    held: Held<HeldFrame>,
    /// The CPU time of each `copy`, waits for an earlier copy's slot included, summed up in the
    /// log when the planes go after a stream's worth of pictures.
    stages: Stages<1>,
    /// The barriers and submit infos of a copy, kept between copies: cleared and refilled, so a
    /// copy allocates none once they have grown.
    before: Vec<vk::ImageMemoryBarrier2<'static>>,
    after: Vec<vk::ImageMemoryBarrier2<'static>>,
    waits: Vec<vk::SemaphoreSubmitInfo<'static>>,
    signals: Vec<vk::SemaphoreSubmitInfo<'static>>,
    /// VA-API pictures sampled in place instead of copied (`Planes::sample`).
    #[cfg(target_os = "linux")]
    pub(super) sampling: Option<sample::Sampling>,
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
        // SAFETY: Caller guarantees `gpu` is the renderer's live SDL GPU device outliving the planes.
        unsafe { Self::make(gpu, width, height, ten_bit, chroma444, false) }
    }

    /// The one image for mapped 4:4:4 VAAPI pictures (`interleaved_444`) of `width` x `height`,
    /// wrapped as an SDL texture.
    ///
    /// # Safety
    /// As `new`.
    pub unsafe fn interleaved(
        gpu: *mut SDL_GPUDevice,
        width: u32,
        height: u32,
        ten_bit: bool,
    ) -> Result<Self, String> {
        // SAFETY: Caller guarantees `gpu` is the renderer's live SDL GPU device outliving the planes.
        unsafe { Self::make(gpu, width, height, ten_bit, true, true) }
    }

    unsafe fn make(
        gpu: *mut SDL_GPUDevice,
        width: u32,
        height: u32,
        ten_bit: bool,
        chroma444: bool,
        interleaved: bool,
    ) -> Result<Self, String> {
        // SAFETY: Caller guarantees `gpu` is the renderer's live SDL GPU device outliving the planes.
        unsafe {
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
                interleaved,
                images: Vec::new(),
                textures: [ptr::null_mut(); 2],
                pool: vk::CommandPool::null(),
                slots: Vec::new(),
                held: Held::new(),
                next: 0,
                stages: Stages::new(["copy"]),
                before: Vec::new(),
                after: Vec::new(),
                waits: Vec::new(),
                signals: Vec::new(),
                #[cfg(target_os = "linux")]
                sampling: None,
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
            for plane in 0..planes.plane_count() {
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
    }

    /// The luma texture and the chroma one, Cb and Cr interleaved, for the plane shader; the one
    /// texture twice for an interleaved picture.
    pub fn textures(&self) -> [*mut SDL_GPUTexture; 2] {
        if self.interleaved {
            [self.textures[0]; 2]
        } else {
            self.textures
        }
    }

    fn plane_count(&self) -> usize {
        if self.interleaved {
            1
        } else {
            2
        }
    }

    fn plane_size(&self, plane: usize) -> (u32, u32) {
        if plane == 0 || self.chroma444 {
            (self.width, self.height)
        } else {
            (self.width.div_ceil(2), self.height.div_ceil(2))
        }
    }

    fn vk_format(&self, plane: usize) -> vk::Format {
        plane_format(plane, self.ten_bit, self.interleaved).0
    }

    pub(super) fn sdl_format(&self, plane: usize) -> SDL_GPUTextureFormat {
        plane_format(plane, self.ten_bit, self.interleaved).1
    }

    /// Copies the two planes of `frame`, an NV12, P010, NV24 or P410 Vulkan picture of the planes'
    /// size, depth and chroma (or the one image of a mapped 4:4:4 VAAPI picture when
    /// `interleaved`), into the plane images, on SDL's queue ahead of SDL's next submission
    /// (`renderer` is flushed first, so nothing SDL recorded before reads the new picture). The
    /// picture is the decoder's again once the copy is done, which its semaphore tells FFmpeg.
    /// FFmpeg's images are shared by every queue family it was given (or made in SDL's alone), so
    /// no ownership moves; an imported one (a mapped VAAPI picture, `VaapiMap`) is acquired from
    /// outside Vulkan, released back to it after the copy (`release`), and kept until its copy is
    /// done; an image owned by another family is refused.
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
        // SAFETY: Caller guarantees `frame` is a live Vulkan AVFrame and `renderer` is the live renderer of `self`'s device.
        let copied = unsafe { self.copy_picture(renderer, frame) };
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
        // SAFETY: Caller guarantees `frame` and `renderer` satisfy `copy`'s safety contract.
        unsafe {
            let frames_ref = (*frame).hw_frames_ctx;
            let source = (*frame).data[0].cast::<VkFrame>();
            if frames_ref.is_null() || source.is_null() {
                return Err("the picture has no Vulkan frame".into());
            }
            // A copy overwrites the planes: the sampled picture's draws are done and it is given back first.
            #[cfg(target_os = "linux")]
            if !self.end_sampling() {
                return Err("the sampled VA-API pictures could not be given back".into());
            }
            let wanted = if self.interleaved {
                interleaved_sw_format(self.ten_bit)
            } else {
                picture_sw_format(self.ten_bit, self.chroma444)
            };
            if sw_format(frame) != wanted {
                return Err(format!(
                    "the picture's format {} is not the planes' format {wanted}",
                    sw_format(frame)
                ));
            }
            let now = frame_now(source);
            let sources = if now.img[1] == vk::Image::null() {
                1
            } else {
                2
            };
            if let Some(&family) = now.queue_family[..sources]
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
            self.held.drop_finished(|slot| device.get_fence_status(slots[slot].1) == Ok(true));
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
            let result = self.record_and_submit(commands, fence, &mut frame_fields(source));
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
    }

    /// Under the frame's lock: waits for the picture's semaphores, copies and signals them one
    /// higher, with the layout the copy leaves its images in.
    unsafe fn record_and_submit(
        &mut self,
        commands: vk::CommandBuffer,
        fence: vk::Fence,
        source: &mut Frame,
    ) -> Result<(), String> {
        // SAFETY: Caller holds the frame lock on `source` and owns `commands`/`fence`.
        unsafe {
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
            // Imported images (VA-API's surfaces) go back to VA-API after the copy.
            let mut external = [false; 2];
            for (index, flag) in external.iter_mut().enumerate().take(sources) {
                *flag = source.queue_family[index] == vk::QUEUE_FAMILY_EXTERNAL;
            }
            self.before.clear();
            self.before.extend((0..sources)
                .map(|index| {
                    let (old_layout, from, to) =
                        acquire(source.queue_family[index], source.layout[index], family);
                    let src_access = source.access.get(index);
                    let src_stage = if src_access.is_empty() {
                        vk::PipelineStageFlags2::COPY
                    } else {
                        vk::PipelineStageFlags2::ALL_COMMANDS
                    };
                    vk::ImageMemoryBarrier2::default()
                        .src_stage_mask(src_stage)
                        .src_access_mask(src_access)
                        .dst_stage_mask(vk::PipelineStageFlags2::COPY)
                        .dst_access_mask(vk::AccessFlags2::TRANSFER_READ)
                        .old_layout(old_layout)
                        .new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                        .src_queue_family_index(from)
                        .dst_queue_family_index(to)
                        .image(source.img[index])
                        .subresource_range(range(vk::ImageAspectFlags::COLOR))
                }));
            // The planes' old contents go; the copy waits for SDL's reads of them before it.
            self.before.extend(self.images.iter().map(|&(image, _)| {
                vk::ImageMemoryBarrier2::default()
                    .src_stage_mask(vk::PipelineStageFlags2::FRAGMENT_SHADER)
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
                &vk::DependencyInfo::default().image_memory_barriers(&self.before),
            );
            for plane in 0..self.plane_count() {
                let (width, height) = self.plane_size(plane);
                let (image, aspect) = if separate || self.interleaved {
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
            self.after.clear();
            self.after.extend(self
                .images
                .iter()
                .map(|&(image, _)| {
                    vk::ImageMemoryBarrier2::default()
                        .src_stage_mask(vk::PipelineStageFlags2::COPY)
                        .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
                        .dst_stage_mask(vk::PipelineStageFlags2::FRAGMENT_SHADER)
                        .dst_access_mask(vk::AccessFlags2::SHADER_SAMPLED_READ)
                        .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                        .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                        .image(image)
                        .subresource_range(range(vk::ImageAspectFlags::COLOR))
                }));
            self.after.extend(
                (0..sources)
                    .filter(|&index| external[index])
                    .map(|index| release(source.img[index], family)),
            );
            device.cmd_pipeline_barrier2(
                commands,
                &vk::DependencyInfo::default().image_memory_barriers(&self.after),
            );
            device
                .end_command_buffer(commands)
                .map_err(|e| format!("the copy cannot be recorded ({e})"))?;

            self.waits.clear();
            self.waits.extend((0..sources).map(|index| {
                vk::SemaphoreSubmitInfo::default()
                    .semaphore(source.sem[index])
                    .value(source.sem_value[index])
                    .stage_mask(vk::PipelineStageFlags2::COPY)
            }));
            self.signals.clear();
            self.signals.extend((0..sources).map(|index| {
                // External imports emit `release()` with `dst_stage_mask = NONE`; signal at
                // `ALL_COMMANDS` so the QFOT/layout release finishes before VA-API unblocks.
                let stage = if external[index] {
                    vk::PipelineStageFlags2::ALL_COMMANDS
                } else {
                    vk::PipelineStageFlags2::COPY
                };
                vk::SemaphoreSubmitInfo::default()
                    .semaphore(source.sem[index])
                    .value(source.sem_value[index] + 1)
                    .stage_mask(stage)
            }));
            let buffers = [vk::CommandBufferSubmitInfo::default().command_buffer(commands)];
            let submit = vk::SubmitInfo2::default()
                .wait_semaphore_infos(&self.waits)
                .command_buffer_infos(&buffers)
                .signal_semaphore_infos(&self.signals);
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
                let (layout, owner) = after_copy(source.queue_family[index]);
                source.layout[index] = layout;
                source.queue_family[index] = owner;
                source.access.set(index, vk::AccessFlags::empty());
            }
            Ok(())
        }
    }
}

impl Drop for Planes {
    fn drop(&mut self) {
        if self.stages.frames() >= crate::stream::stage_times::SUMMARY_FRAMES
            && let Some(summary) = self.stages.summary() {
                eprintln!("nightjar: Vulkan plane copy stages {summary} (CPU time)");
            }
        // SAFETY: Handles made in `new`; nothing is freed while the GPU may still use it.
        unsafe {
            #[cfg(target_os = "linux")]
            self.release_wrapped();
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
            #[cfg(target_os = "linux")]
            self.drop_wrapped(idle && copied);
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
