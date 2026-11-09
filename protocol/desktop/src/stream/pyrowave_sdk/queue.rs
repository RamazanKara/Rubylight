//! Which Vulkan queue the SDK decodes PyroWave on: the graphics queue, as always, or with the
//! setting "Decode PyroWave on a compute queue" (`Settings::pyrowave_async_compute`, off by
//! default) a separate compute queue where the GPU has one, so a decode can run beside the previous
//! frame's drawing and present.
//!
//! Ported from Nonary's moonlight-qt fork, which decoded on the compute family
//! (`pyrowave_device_set_queue_type` in pyrowaveplacebo.cpp, commit 5bb5902) and reverted it two
//! days later (e225f88) because async decodes delayed Wayland presentation; hence opt-in here.
//!
//! The queue is picked by Granite's rule, the one the SDK's own device runs (`compute_queue`): the
//! first family with compute but not graphics that has a queue left, else a second queue of a
//! compute family. On a device of the SDK's own (Windows, the Linux readback) Granite makes that
//! queue itself and `pyrowave_device_set_queue_type` moves the decodes onto it; on the renderer's
//! device (Linux, `zero_copy.rs`) the patched SDL makes it with the same rule when asked
//! (`zero_copy::COMPUTE_QUEUE_PROPERTY`) and the SDK is handed it beside the graphics queue. Either
//! way the decode is ordered before the planes are read by the timeline semaphore (or D3D11 fence)
//! it signals, as on the graphics queue.

use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};

use ash::vk;

use super::{result_name, Api, RawDevice, SUCCESS};

/// The setting of the stream being set up (`use_async_compute`).
static ASYNC_COMPUTE: AtomicBool = AtomicBool::new(false);

/// Takes the setting "Decode PyroWave on a compute queue" for the decoders made from now on
/// and, on Linux, for the GPU renderer's device.
pub fn use_async_compute(on: bool) {
    ASYNC_COMPUTE.store(on, Ordering::Relaxed);
}

/// The setting `use_async_compute` was last given.
pub fn async_compute() -> bool {
    ASYNC_COMPUTE.load(Ordering::Relaxed)
}

/// A queue family as `vkGetPhysicalDeviceQueueFamilyProperties` reports it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct QueueFamily {
    pub flags: vk::QueueFlags,
    pub count: u32,
}

impl std::fmt::Debug for QueueFamily {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "QueueFamily {{ flags: {:#x}, count: {} }}", self.flags.as_raw(), self.count)
    }
}

/// A queue: its family and its index in the family.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QueueSlot {
    pub family: u32,
    pub index: u32,
}

/// The family Granite gives a device of its own its graphics queue in (no surface): the first with
/// graphics and compute.
pub fn graphics_family(families: &[QueueFamily]) -> Option<u32> {
    let both = vk::QueueFlags::GRAPHICS | vk::QueueFlags::COMPUTE;
    families.iter().position(|family| family.count > 0 && family.flags.contains(both)).map(|index| index as u32)
}

/// The compute queue for the decodes, `used[i]` queues of family i being taken already (the
/// graphics queue, video decode queues; missing entries are 0): the first family with compute but
/// not graphics that has one left, else the first compute family that has one left (a second
/// queue beside the graphics one). Granite's rule (`Context::create_device`), which the SDL patch
/// follows too. None: there is no queue for it but the graphics one.
pub fn compute_queue(families: &[QueueFamily], used: &[u32]) -> Option<QueueSlot> {
    let taken = |family: usize| used.get(family).copied().unwrap_or(0);
    let pick = |separate: bool| {
        families.iter().enumerate().find_map(|(index, family)| {
            let fits = family.flags.contains(vk::QueueFlags::COMPUTE)
                && !(separate && family.flags.contains(vk::QueueFlags::GRAPHICS))
                && family.count > taken(index);
            fits.then(|| QueueSlot { family: index as u32, index: taken(index) })
        })
    };
    pick(true).or_else(|| pick(false))
}

/// Where the decodes run, and why on the graphics queue.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DecodeQueue {
    Graphics(String),
    Compute(QueueSlot),
}

impl DecodeQueue {
    /// The log line at stream start.
    pub fn line(&self) -> String {
        match self {
            Self::Graphics(why) => format!("PyroWave decodes on the graphics queue: {why}"),
            Self::Compute(slot) => format!("PyroWave decodes on a separate compute queue: family {}, queue {}", slot.family, slot.index),
        }
    }

    pub fn is_compute(&self) -> bool {
        matches!(self, Self::Compute(_))
    }
}

/// Why the decodes stay on the graphics queue with the setting off.
pub const SETTING_OFF: &str = "the setting \"Decode PyroWave on a compute queue\" is off";

/// The queue on a device of the SDK's own, whose queue `families` are read only with the setting on.
pub fn own_device_queue(setting: bool, families: impl FnOnce() -> Result<Vec<QueueFamily>, String>) -> DecodeQueue {
    if !setting {
        return DecodeQueue::Graphics(SETTING_OFF.into());
    }
    let families = match families() {
        Ok(families) => families,
        Err(why) => return DecodeQueue::Graphics(format!("the queue families of the SDK's Vulkan device cannot be read ({why})")),
    };
    let Some(graphics) = graphics_family(&families) else {
        return DecodeQueue::Graphics("the SDK's Vulkan device has no graphics and compute queue family".into());
    };
    let mut used = vec![0; families.len()];
    used[graphics as usize] = 1;
    match compute_queue(&families, &used) {
        Some(slot) => DecodeQueue::Compute(slot),
        None => DecodeQueue::Graphics("the GPU has no compute queue apart from the graphics one".into()),
    }
}

/// The compute queue the renderer's device was made with for the decodes (the patched SDL's).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RendererCompute {
    /// The device was made without the setting on, so SDL was not asked for one.
    NotAsked,
    /// Asked for, but the GPU has no queue for it but SDL's.
    None,
    Queue(vk::Queue, QueueSlot),
}

/// The queue on the renderer's device (`compute`, what it was made with).
pub fn shared_device_queue(setting: bool, compute: RendererCompute) -> DecodeQueue {
    match (setting, compute) {
        (false, _) => DecodeQueue::Graphics(SETTING_OFF.into()),
        (true, RendererCompute::NotAsked) => DecodeQueue::Graphics(
            "the renderer's Vulkan device was made while the setting was off, without a compute queue; it gets one when Nightjar starts again".into(),
        ),
        (true, RendererCompute::None) => DecodeQueue::Graphics("the GPU has no compute queue apart from the renderer's".into()),
        (true, RendererCompute::Queue(_, slot)) => DecodeQueue::Compute(slot),
    }
}

/// The queue families of the SDK's own Vulkan device.
///
/// # Safety
/// `device` must be a live SDK device of `api`.
pub unsafe fn sdk_families(api: &Api, device: RawDevice) -> Result<Vec<QueueFamily>, String> {
    let (mut instance, mut physical) = (vk::Instance::null(), vk::PhysicalDevice::null());
    (api.device_get_vk_device_handles)(device, &mut instance, &mut physical, ptr::null_mut());
    if instance == vk::Instance::null() || physical == vk::PhysicalDevice::null() {
        return Err("the SDK names no Vulkan instance or physical device".into());
    }
    // The same Vulkan loader the SDK loaded, so its instance is one of this loader's.
    let entry = ash::Entry::load().map_err(|e| format!("the Vulkan loader cannot be loaded ({e})"))?;
    let instance = ash::Instance::load(entry.static_fn(), instance);
    Ok(instance
        .get_physical_device_queue_family_properties(physical)
        .iter()
        .map(|family| QueueFamily { flags: family.queue_flags, count: family.queue_count })
        .collect())
}

/// Moves the SDK device's decodes to its compute queue when `queue` says so, before the decoder is
/// made. What the decodes then run on: an SDK that refuses keeps them on the graphics queue, and
/// says why.
///
/// # Safety
/// `device` must be a live SDK device of `api` without a decoder yet.
pub unsafe fn apply(api: &Api, device: RawDevice, queue: DecodeQueue) -> DecodeQueue {
    if !queue.is_compute() {
        return queue;
    }
    let result = (api.device_set_queue_type)(device, vk::QueueFlags::COMPUTE);
    if result == SUCCESS {
        queue
    } else {
        DecodeQueue::Graphics(format!("the PyroWave SDK refused the compute queue (PyroWave error {result}, {})", result_name(result)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GRAPHICS: vk::QueueFlags = vk::QueueFlags::from_raw(vk::QueueFlags::GRAPHICS.as_raw() | vk::QueueFlags::COMPUTE.as_raw() | vk::QueueFlags::TRANSFER.as_raw());
    const COMPUTE: vk::QueueFlags = vk::QueueFlags::from_raw(vk::QueueFlags::COMPUTE.as_raw() | vk::QueueFlags::TRANSFER.as_raw());

    fn family(flags: vk::QueueFlags, count: u32) -> QueueFamily {
        QueueFamily { flags, count }
    }

    /// RADV and AMD's driver: graphics, then a compute family; the decodes go to the compute family.
    #[test]
    fn a_family_with_compute_but_not_graphics_is_picked_first() {
        let amd = [family(GRAPHICS, 1), family(COMPUTE, 4), family(vk::QueueFlags::TRANSFER, 2)];
        assert_eq!(graphics_family(&amd), Some(0));
        assert_eq!(compute_queue(&amd, &[1]), Some(QueueSlot { family: 1, index: 0 }));
        // NVIDIA: 16 graphics queues, then transfer, then compute; still the separate family.
        let nvidia = [family(GRAPHICS, 16), family(vk::QueueFlags::TRANSFER, 2), family(COMPUTE, 8), family(vk::QueueFlags::VIDEO_DECODE_KHR, 4)];
        assert_eq!(compute_queue(&nvidia, &[1]), Some(QueueSlot { family: 2, index: 0 }));
        // A taken queue of that family moves the pick to the next index.
        assert_eq!(compute_queue(&amd, &[1, 1]), Some(QueueSlot { family: 1, index: 1 }));
    }

    /// Without a separate family, a second queue of the graphics family; with one queue only, none.
    #[test]
    fn a_second_graphics_queue_serves_when_no_family_is_compute_only() {
        let intel = [family(GRAPHICS, 2), family(vk::QueueFlags::VIDEO_DECODE_KHR, 1)];
        assert_eq!(compute_queue(&intel, &[1]), Some(QueueSlot { family: 0, index: 1 }));
        let single = [family(GRAPHICS, 1), family(vk::QueueFlags::TRANSFER, 1)];
        assert_eq!(compute_queue(&single, &[1]), None);
        // A compute family with no queue left counts as none.
        assert_eq!(compute_queue(&[family(GRAPHICS, 1), family(COMPUTE, 1)], &[1, 1]), None);
        assert_eq!(compute_queue(&[], &[]), None);
        assert_eq!(graphics_family(&[family(COMPUTE, 1)]), None);
        assert_eq!(graphics_family(&[family(COMPUTE, 1), family(GRAPHICS, 1)]), Some(1));
    }

    /// The setting off leaves the decodes where they always were and reads nothing.
    #[test]
    fn the_setting_off_keeps_the_graphics_queue_and_asks_nothing() {
        let queue = own_device_queue(false, || panic!("the families are read only with the setting on"));
        assert_eq!(queue, DecodeQueue::Graphics(SETTING_OFF.into()));
        assert_eq!(queue.line(), "PyroWave decodes on the graphics queue: the setting \"Decode PyroWave on a compute queue\" is off");
        let renderer = RendererCompute::Queue(vk::Queue::null(), QueueSlot { family: 1, index: 0 });
        assert_eq!(shared_device_queue(false, renderer), DecodeQueue::Graphics(SETTING_OFF.into()));
    }

    /// With the setting on, each way the decodes stay on the graphics queue is named.
    #[test]
    fn with_the_setting_on_the_log_names_the_queue_or_why_there_is_none() {
        let amd = || Ok(vec![family(GRAPHICS, 1), family(COMPUTE, 4)]);
        let queue = own_device_queue(true, amd);
        assert_eq!(queue.line(), "PyroWave decodes on a separate compute queue: family 1, queue 0");
        assert!(queue.is_compute());
        let none = own_device_queue(true, || Ok(vec![family(GRAPHICS, 1)]));
        assert_eq!(none.line(), "PyroWave decodes on the graphics queue: the GPU has no compute queue apart from the graphics one");
        let unread = own_device_queue(true, || Err("no loader".into()));
        assert!(unread.line().contains("cannot be read (no loader)"), "{}", unread.line());
        let odd = own_device_queue(true, || Ok(vec![family(COMPUTE, 2)]));
        assert!(odd.line().contains("no graphics and compute queue family"), "{}", odd.line());

        let slot = QueueSlot { family: 2, index: 0 };
        assert_eq!(shared_device_queue(true, RendererCompute::Queue(vk::Queue::null(), slot)), DecodeQueue::Compute(slot));
        assert!(shared_device_queue(true, RendererCompute::NotAsked).line().contains("made while the setting was off"));
        assert!(shared_device_queue(true, RendererCompute::None).line().contains("no compute queue apart from the renderer's"));
    }

    #[test]
    fn the_setting_is_off_until_a_stream_turns_it_on() {
        assert!(!crate::config::Settings::default().pyrowave_async_compute);
    }
}
