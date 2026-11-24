//! FFmpeg `libavutil` Vulkan, DRM PRIME, and CUDA C struct layouts (59.34–60.26 and 61) and
//! version-checked field accessors for `AVVulkanDeviceContext`, `AVVulkanFramesContext`, and
//! `AVVkFrame`.

use std::ffi::{c_char, c_int, c_void};

use ash::vk;

use crate::media::ffi::{avutil_version, AVHWDeviceContext, AVHWFramesContext};

/// The last libavutil 60 minor of an FFmpeg release (8.1): the development versions after it
/// changed the Vulkan structs step by step towards 61's.
const LAST_RELEASED_60: u32 = 26;

/// Whether the libavutil of `version` (`avutil_version()`) has the `AVVulkanDeviceContext` and
/// frame structs as declared below (`StructLayout`): their queue families arrived in 59.34
/// (FFmpeg 7.1); 61 (FFmpeg 9) drops the fixed queue fields before them, widens `AVVkFrame`'s
/// access flags and moves its internal part last; 62 drops the queue locks. The layouts are checked
/// against 59.39 (FFmpeg 7.1), 60.26 (FFmpeg 8.1.2) and 61.1 (FFmpeg 9.0.2)
/// (`the_structs_have_libavutils_layout`); FFmpeg moves no field within a released major version.
pub fn ffmpeg_supported(version: u32) -> Result<(), String> {
    let (major, minor, micro) = (version >> 16, (version >> 8) & 0xff, version & 0xff);
    if version < (59 << 16 | 34 << 8 | 100) {
        return Err(format!(
            "FFmpeg's libavutil {major}.{minor}.{micro} predates the Vulkan queue families this needs (59.34, FFmpeg 7.1)"
        ));
    }
    if major == 60 && minor > LAST_RELEASED_60 {
        return Err(format!(
            "FFmpeg's libavutil {major}.{minor}.{micro} is a development version between FFmpeg 8.1 and 9, whose Vulkan structs this build does not know (it knows 59.34 to 60.{LAST_RELEASED_60} and 61)"
        ));
    }
    if major >= 62 {
        return Err(format!(
            "FFmpeg's libavutil {major}.{minor}.{micro} has Vulkan structs this build does not know (it knows 59, 60 and 61)"
        ));
    }
    Ok(())
}

/// Which libavutil's `AVVulkanDeviceContext` and `AVVkFrame` the linked FFmpeg has.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum StructLayout {
    /// libavutil 59.34 to 60.26 (FFmpeg 7.1 to 8.1).
    V60,
    /// libavutil 61 (FFmpeg 9).
    V61,
}

impl StructLayout {
    /// Of a libavutil `ffmpeg_supported` takes.
    pub(super) fn of(version: u32) -> Self {
        if version >> 16 >= 61 {
            Self::V61
        } else {
            Self::V60
        }
    }

    /// The linked libavutil's.
    pub(super) fn linked() -> Self {
        // SAFETY: Returns a constant.
        Self::of(unsafe { avutil_version() })
    }

    /// `sizeof(AVVkFrame)`.
    pub(super) fn frame_size(self) -> usize {
        match self {
            Self::V60 => std::mem::size_of::<VkFrame60>(),
            Self::V61 => std::mem::size_of::<VkFrame61>(),
        }
    }

    /// The fields of `frame` this client writes.
    ///
    /// # Safety
    /// `frame` is a live AVVkFrame of this layout that nothing else reads or writes meanwhile
    /// (its frame lock is held, or it is no picture's yet), for as long as the view is used.
    pub(super) unsafe fn fields<'a>(self, frame: *mut VkFrame) -> Frame<'a> {
        // SAFETY: Caller guarantees `frame` is a live AVVkFrame of this layout with exclusive access.
        unsafe {
            match self {
                Self::V60 => {
                    let frame = &mut *frame.cast::<VkFrame60>();
                    Frame {
                        img: &mut frame.img,
                        tiling: &mut frame.tiling,
                        mem: &mut frame.mem,
                        size: &mut frame.size,
                        flags: &mut frame.flags,
                        access: Access::Bits(&mut frame.access),
                        layout: &mut frame.layout,
                        sem: &mut frame.sem,
                        sem_value: &mut frame.sem_value,
                        queue_family: &mut frame.queue_family,
                    }
                }
                Self::V61 => {
                    let frame = &mut *frame.cast::<VkFrame61>();
                    Frame {
                        img: &mut frame.img,
                        tiling: &mut frame.tiling,
                        mem: &mut frame.mem,
                        size: &mut frame.size,
                        flags: &mut frame.flags,
                        access: Access::Bits2(&mut frame.access),
                        layout: &mut frame.layout,
                        sem: &mut frame.sem,
                        sem_value: &mut frame.sem_value,
                        queue_family: &mut frame.queue_family,
                    }
                }
            }
        }
    }

    /// What `frame` holds now, read without a reference to it.
    ///
    /// # Safety
    /// `frame` is a live AVVkFrame of this layout.
    pub(super) unsafe fn read(self, frame: *const VkFrame) -> FrameNow {
        // SAFETY: Caller guarantees `frame` is a live AVVkFrame of this layout.
        unsafe {
            match self {
                Self::V60 => {
                    let frame = frame.cast::<VkFrame60>();
                    FrameNow {
                        img: (*frame).img,
                        mem: (*frame).mem,
                        access: (*frame).access.map(|flags| vk::AccessFlags2::from_raw(u64::from(flags.as_raw()))),
                        layout: (*frame).layout,
                        sem: (*frame).sem,
                        sem_value: (*frame).sem_value,
                        queue_family: (*frame).queue_family,
                        internal: (*frame).internal,
                    }
                }
                Self::V61 => {
                    let frame = frame.cast::<VkFrame61>();
                    FrameNow {
                        img: (*frame).img,
                        mem: (*frame).mem,
                        access: (*frame).access,
                        layout: (*frame).layout,
                        sem: (*frame).sem,
                        sem_value: (*frame).sem_value,
                        queue_family: (*frame).queue_family,
                        internal: (*frame).internal,
                    }
                }
            }
        }
    }
}

/// The linked libavutil's fields of `frame` (`StructLayout::fields`).
///
/// # Safety
/// As `StructLayout::fields`.
pub(super) unsafe fn frame_fields<'a>(frame: *mut VkFrame) -> Frame<'a> {
    // SAFETY: Caller satisfies `StructLayout::fields` for the linked libavutil.
    unsafe { StructLayout::linked().fields(frame) }
}

/// What `frame` holds now (`StructLayout::read`).
///
/// # Safety
/// `frame` is a live AVVkFrame of the linked libavutil.
pub(super) unsafe fn frame_now(frame: *const VkFrame) -> FrameNow {
    // SAFETY: Caller guarantees `frame` is a live AVVkFrame of the linked libavutil.
    unsafe { StructLayout::linked().read(frame) }
}

/// `AVVulkanDeviceQueueFamily` (libavutil/hwcontext_vulkan.h, 59.34 to 61).
#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct QueueFamily {
    pub(super) idx: c_int,
    pub(super) num: c_int,
    pub(super) flags: vk::QueueFlags,
    pub(super) video_caps: vk::VideoCodecOperationFlagsKHR,
}

/// `AVVulkanDeviceContext` of libavutil 59.34 to 60.26 (`StructLayout::V60`).
#[repr(C)]
pub(super) struct DeviceContext60 {
    pub(super) alloc: *const c_void,
    pub(super) get_proc_addr: Option<vk::PFN_vkGetInstanceProcAddr>,
    pub(super) inst: vk::Instance,
    pub(super) phys_dev: vk::PhysicalDevice,
    pub(super) act_dev: vk::Device,
    pub(super) device_features: vk::PhysicalDeviceFeatures2<'static>,
    pub(super) enabled_inst_extensions: *const *const c_char,
    pub(super) nb_enabled_inst_extensions: c_int,
    pub(super) enabled_dev_extensions: *const *const c_char,
    pub(super) nb_enabled_dev_extensions: c_int,
    /// The deprecated fixed queue fields (`FF_API_VULKAN_FIXED_QUEUES`), left 0: `qf` says it all.
    pub(super) fixed_queues: [c_int; 10],
    pub(super) lock_queue: Option<unsafe extern "C" fn(*mut AVHWDeviceContext, u32, u32)>,
    pub(super) unlock_queue: Option<unsafe extern "C" fn(*mut AVHWDeviceContext, u32, u32)>,
    pub(super) qf: [QueueFamily; 64],
    pub(super) nb_qf: c_int,
}

/// `AVVulkanDeviceContext` of libavutil 61 (`StructLayout::V61`): without the fixed queue fields,
/// and with the flags FFmpeg makes its own device's queues with.
#[repr(C)]
pub(super) struct DeviceContext61 {
    pub(super) alloc: *const c_void,
    pub(super) get_proc_addr: Option<vk::PFN_vkGetInstanceProcAddr>,
    pub(super) inst: vk::Instance,
    pub(super) phys_dev: vk::PhysicalDevice,
    pub(super) act_dev: vk::Device,
    pub(super) device_features: vk::PhysicalDeviceFeatures2<'static>,
    pub(super) enabled_inst_extensions: *const *const c_char,
    pub(super) nb_enabled_inst_extensions: c_int,
    pub(super) enabled_dev_extensions: *const *const c_char,
    pub(super) nb_enabled_dev_extensions: c_int,
    /// Deprecated in 61 and still called for every submit (`FF_API_VULKAN_SYNC_QUEUES`, until 62).
    pub(super) lock_queue: Option<unsafe extern "C" fn(*mut AVHWDeviceContext, u32, u32)>,
    pub(super) unlock_queue: Option<unsafe extern "C" fn(*mut AVHWDeviceContext, u32, u32)>,
    pub(super) qf: [QueueFamily; 64],
    pub(super) nb_qf: c_int,
    /// Left 0: the renderer's queues are not internally synchronized, FFmpeg takes `lock_queue`.
    pub(super) queue_flags: vk::DeviceQueueCreateFlags,
}

/// `AVVulkanFramesContext` of libavutil 59.34 to 61.
#[repr(C)]
pub(super) struct FramesContext {
    pub(super) tiling: vk::ImageTiling,
    pub(super) usage: vk::ImageUsageFlags,
    pub(super) create_pnext: *mut c_void,
    pub(super) alloc_pnext: [*mut c_void; 8],
    pub(super) flags: c_int,
    pub(super) img_flags: vk::ImageCreateFlags,
    pub(super) format: [vk::Format; 8],
    pub(super) nb_layers: c_int,
    pub(super) lock_frame: Option<unsafe extern "C" fn(*mut AVHWFramesContext, *mut VkFrame)>,
    pub(super) unlock_frame: Option<unsafe extern "C" fn(*mut AVHWFramesContext, *mut VkFrame)>,
}

/// An `AVVkFrame`, only ever behind a pointer: its fields are where the linked libavutil has them
/// (`VkFrame60`, `VkFrame61`), reached through `frame_fields` and `frame_now`.
#[repr(C)]
pub(super) struct VkFrame {
    _libavutils: [u8; 0],
}

/// `AVVkFrame` of libavutil 59.34 to 60.26 (`StructLayout::V60`).
#[repr(C)]
pub(super) struct VkFrame60 {
    pub(super) img: [vk::Image; 8],
    pub(super) tiling: vk::ImageTiling,
    pub(super) mem: [vk::DeviceMemory; 8],
    pub(super) size: [usize; 8],
    pub(super) flags: vk::MemoryPropertyFlags,
    pub(super) access: [vk::AccessFlags; 8],
    pub(super) layout: [vk::ImageLayout; 8],
    pub(super) sem: [vk::Semaphore; 8],
    pub(super) sem_value: [u64; 8],
    pub(super) internal: *mut c_void,
    pub(super) offset: [isize; 8],
    pub(super) queue_family: [u32; 8],
}

/// `AVVkFrame` of libavutil 61 (`StructLayout::V61`): the access flags are `VkAccessFlagBits2`,
/// and the internal part comes last.
#[repr(C)]
pub(super) struct VkFrame61 {
    pub(super) img: [vk::Image; 8],
    pub(super) tiling: vk::ImageTiling,
    pub(super) mem: [vk::DeviceMemory; 8],
    pub(super) size: [usize; 8],
    pub(super) flags: vk::MemoryPropertyFlags,
    pub(super) access: [vk::AccessFlags2; 8],
    pub(super) layout: [vk::ImageLayout; 8],
    pub(super) sem: [vk::Semaphore; 8],
    pub(super) sem_value: [u64; 8],
    pub(super) offset: [isize; 8],
    pub(super) queue_family: [u32; 8],
    pub(super) internal: *mut c_void,
}

/// An `AVVkFrame`'s access flags, 32 bits wide until libavutil 61.
pub(super) enum Access<'a> {
    Bits(&'a mut [vk::AccessFlags; 8]),
    Bits2(&'a mut [vk::AccessFlags2; 8]),
}

impl Access<'_> {
    pub(super) fn get(&self, index: usize) -> vk::AccessFlags2 {
        match self {
            Self::Bits(flags) => vk::AccessFlags2::from_raw(u64::from(flags[index].as_raw())),
            Self::Bits2(flags) => flags[index],
        }
    }

    /// `flags` are of the 32 bits both widths have.
    pub(super) fn set(&mut self, index: usize, flags: vk::AccessFlags) {
        match self {
            Self::Bits(all) => all[index] = flags,
            Self::Bits2(all) => all[index] = vk::AccessFlags2::from_raw(u64::from(flags.as_raw())),
        }
    }
}

/// The fields of an `AVVkFrame` this client writes, wherever the linked libavutil has them
/// (`StructLayout::fields`).
pub(super) struct Frame<'a> {
    pub(super) img: &'a mut [vk::Image; 8],
    pub(super) tiling: &'a mut vk::ImageTiling,
    pub(super) mem: &'a mut [vk::DeviceMemory; 8],
    pub(super) size: &'a mut [usize; 8],
    pub(super) flags: &'a mut vk::MemoryPropertyFlags,
    pub(super) access: Access<'a>,
    pub(super) layout: &'a mut [vk::ImageLayout; 8],
    pub(super) sem: &'a mut [vk::Semaphore; 8],
    pub(super) sem_value: &'a mut [u64; 8],
    pub(super) queue_family: &'a mut [u32; 8],
}

/// What an `AVVkFrame` held when it was read (`StructLayout::read`).
pub(super) struct FrameNow {
    pub(super) img: [vk::Image; 8],
    pub(super) mem: [vk::DeviceMemory; 8],
    pub(super) access: [vk::AccessFlags2; 8],
    pub(super) layout: [vk::ImageLayout; 8],
    pub(super) sem: [vk::Semaphore; 8],
    pub(super) sem_value: [u64; 8],
    pub(super) queue_family: [u32; 8],
    pub(super) internal: *mut c_void,
}

/// FFmpeg's `AVDRMFrameDescriptor` (libavutil/hwcontext_drm.h): a DRM PRIME frame's `data[0]`.
/// Only the layer formats are read; the rest is there for the layout.
#[allow(dead_code)]
#[repr(C)]
pub(super) struct DrmFrameDescriptor {
    pub(super) nb_objects: c_int,
    pub(super) objects: [DrmObject; 4],
    pub(super) nb_layers: c_int,
    pub(super) layers: [DrmLayer; 4],
}

#[allow(dead_code)]
#[repr(C)]
pub(super) struct DrmObject {
    pub(super) fd: c_int,
    pub(super) size: usize,
    pub(super) format_modifier: u64,
}

#[allow(dead_code)]
#[repr(C)]
pub(super) struct DrmLayer {
    pub(super) format: u32,
    pub(super) nb_planes: c_int,
    pub(super) planes: [DrmPlane; 4],
}

#[allow(dead_code)]
#[repr(C)]
pub(super) struct DrmPlane {
    pub(super) object_index: c_int,
    pub(super) offset: isize,
    pub(super) pitch: isize,
}

/// `AVCUDADeviceContext` (libavutil/hwcontext_cuda.h): the CUDA context NVDEC decodes in.
#[repr(C)]
pub(super) struct CudaDeviceContext {
    pub(super) cuda_ctx: *mut c_void,
    pub(super) stream: *mut c_void,
    pub(super) internal: *mut c_void,
}
