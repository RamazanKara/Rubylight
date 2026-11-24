//! Pure copy-slot tracking (`Held`, `HeldFrame`), external queue-family ownership barriers, and
//! in-place VA-API sampling eligibility and lifetime state machine (`SampleGpu`, `Sampled`).

use std::time::Duration;

use ash::vk;

use crate::media::ffi::{av_frame_free, AVFrame};

/// How long the presenter waits for its copy of the picture before last to finish.
pub(super) const COPY_WAIT: Duration = Duration::from_secs(1);
/// Command buffers the copies take turns with.
pub(super) const SLOTS: usize = 3;

/// How many mapped VAAPI pictures the presenter keeps beyond the one it shows, each until its
/// copy is done (`Held`, one per copy slot): a VAAPI pool that maps gets this many more surfaces.
pub const MAPPED_HELD: i32 = SLOTS as i32;

/// Items kept per copy slot until that slot's copy is done: mapped VAAPI pictures, whose freeing
/// would otherwise wait on the presenter's thread for the copy just submitted, and which give
/// their VAAPI surface back to the decoder only once the GPU no longer reads it.
pub(super) struct Held<T> {
    items: Vec<(usize, T)>,
}

impl<T> Held<T> {
    pub(super) fn new() -> Self {
        Self { items: Vec::new() }
    }

    /// Keeps `item` for `slot`'s copy; what the slot held before comes back (its copy was waited
    /// for before the slot was used again).
    pub(super) fn hold(&mut self, slot: usize, item: T) -> Option<T> {
        let before = self.take(slot);
        self.items.push((slot, item));
        before
    }

    /// What `slot` held.
    pub(super) fn take(&mut self, slot: usize) -> Option<T> {
        let at = self.items.iter().position(|&(held, _)| held == slot)?;
        Some(self.items.swap_remove(at).1)
    }

    /// Drops entries whose slots are `done` in place without allocating.
    pub(super) fn drop_finished(&mut self, done: impl Fn(usize) -> bool) {
        self.items.retain_mut(|&mut (slot, _)| !done(slot));
    }

    /// What the slots whose copies are `done` held.
    #[cfg(test)]
    pub(super) fn release(&mut self, done: impl Fn(usize) -> bool) -> Vec<T> {
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
    pub(super) fn drain(&mut self) -> Vec<T> {
        self.items.drain(..).map(|(_, item)| item).collect()
    }
}

/// A reference to a mapped picture, freed (unmapped) when dropped.
pub(super) struct HeldFrame(pub(super) *mut AVFrame);

impl Drop for HeldFrame {
    fn drop(&mut self) {
        // SAFETY: Our own reference (`av_frame_clone`).
        unsafe { av_frame_free(&mut self.0) };
    }
}

/// The old layout and the queue families of a picture image's barrier before the copy: an
/// imported image (`VK_QUEUE_FAMILY_EXTERNAL`, FFmpeg's DMA-BUF import or a kept one of
/// `VaapiMap`) is acquired from outside Vulkan by the renderer's `family`, from GENERAL when it
/// was left UNDEFINED (whose contents a transition may drop), and released back after the copy
/// (`release`); FFmpeg's own images are shared and need no transfer.
pub(super) fn acquire(owner: u32, layout: vk::ImageLayout, family: u32) -> (vk::ImageLayout, u32, u32) {
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

/// The barrier after the copy that gives an acquired imported `image` back to VA-API: from the
/// renderer's `family` to `VK_QUEUE_FAMILY_EXTERNAL`, from the copy's TRANSFER_SRC_OPTIMAL to
/// GENERAL, after the copy's reads (a release's destination scope is the external owner's).
/// Every use of an image shared with another API sits between such an acquire and release; a
/// kept import is acquired again for its surface's next picture.
pub(super) fn release(image: vk::Image, family: u32) -> vk::ImageMemoryBarrier2<'static> {
    vk::ImageMemoryBarrier2::default()
        .src_stage_mask(vk::PipelineStageFlags2::COPY)
        .src_access_mask(vk::AccessFlags2::TRANSFER_READ)
        .dst_stage_mask(vk::PipelineStageFlags2::NONE)
        .dst_access_mask(vk::AccessFlags2::NONE)
        .old_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
        .new_layout(vk::ImageLayout::GENERAL)
        .src_queue_family_index(family)
        .dst_queue_family_index(vk::QUEUE_FAMILY_EXTERNAL)
        .image(image)
        .subresource_range(vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            base_mip_level: 0,
            level_count: 1,
            base_array_layer: 0,
            layer_count: 1,
        })
}

/// The layout and owner a picture image of `owner` is left with after its copy, for the frame:
/// an imported one released to VA-API (`release`) in GENERAL; any other in TRANSFER_SRC_OPTIMAL,
/// its owner kept.
pub(super) fn after_copy(owner: u32) -> (vk::ImageLayout, u32) {
    if owner == vk::QUEUE_FAMILY_EXTERNAL {
        (vk::ImageLayout::GENERAL, vk::QUEUE_FAMILY_EXTERNAL)
    } else {
        (vk::ImageLayout::TRANSFER_SRC_OPTIMAL, owner)
    }
}

/// Set to 0, decoded VA-API and Vulkan video pictures are copied into the presenter's planes
/// (`Planes::copy`) as before, instead of being sampled where the decoder wrote them.
pub const SAMPLE_ENVIRONMENT: &str = "NIGHTJAR_VIDEO_SAMPLE_PICTURES";

/// What `SAMPLE_ENVIRONMENT`'s value asks for: sampling, unless it is 0.
pub fn sample_switched_on(value: Option<&std::ffi::OsStr>) -> bool {
    value.is_none_or(|value| value != "0")
}

/// How a decoded Linux video picture reaches the renderer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    /// Sampled where the decoder wrote it, held until the draws that read it are fenced (`Sampled`).
    Sample,
    /// Copied into the presenter's planes (`Planes::copy`); the reason is logged once.
    Copy(&'static str),
}

/// What the route depends on, per stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Eligibility {
    /// `SAMPLE_ENVIRONMENT` does not turn sampling off.
    pub switched_on: bool,
    /// The driver samples the picture's format with its DRM modifier: `ImportDevice::check_importable`
    /// with SAMPLED usage passes.
    pub sampleable: bool,
    /// The VA-API pool has a surface beyond the ones the presenter holds (`SAMPLED_HELD`).
    pub spare_surface: bool,
    /// The consumer needs a copy that outlives the picture (the packed 4:4:4 rebuild reads its planes
    /// after the copy and keeps no picture).
    pub stable_copy: bool,
}

/// The first reason that rules sampling out, in the order they are checked; `Sample` when none does.
pub fn route(eligibility: Eligibility) -> Route {
    if !eligibility.switched_on {
        Route::Copy("NIGHTJAR_VIDEO_SAMPLE_PICTURES=0")
    } else if eligibility.stable_copy {
        Route::Copy("the consumer needs a copy that outlives the picture")
    } else if !eligibility.sampleable {
        Route::Copy("the driver does not sample this DMA-BUF format and modifier")
    } else if !eligibility.spare_surface {
        Route::Copy("the VA-API pool has no surface beyond the ones the presenter holds")
    } else {
        Route::Sample
    }
}

/// Surfaces a VA-API pool keeps beyond the decoder's when pictures are sampled: the shown picture
/// and the ones whose release is not fenced yet (at most `SLOTS` of them), so one more than
/// `MAPPED_HELD`.
pub const SAMPLED_HELD: i32 = MAPPED_HELD + 1;

/// The surfaces a VAAPI pool that maps keeps beyond the decoder's: `SAMPLED_HELD` when pictures are
/// sampled in place (`SAMPLE_ENVIRONMENT`), else `MAPPED_HELD`.
pub fn mapped_held() -> i32 {
    if sample_switched_on(std::env::var_os(SAMPLE_ENVIRONMENT).as_deref()) { SAMPLED_HELD } else { MAPPED_HELD }
}

/// How the log says the route.
pub fn describe_route(route: Route) -> String {
    match route {
        Route::Sample => "VA-API and Vulkan video pictures are sampled in place".into(),
        Route::Copy(why) => format!("VA-API and Vulkan video pictures are copied into the presenter's planes: {why}"),
    }
}

/// The GPU work a sampled picture takes, in the order it is queued on the renderer's queue.
pub trait SampleGpu {
    /// Submits what the renderer recorded (`SDL_FlushRenderer`), so every draw so far is ahead of
    /// what is queued after it.
    fn flush(&mut self) -> Result<(), String>;
    /// Blocks until the work last queued on `slot` has finished.
    fn wait(&mut self, slot: usize) -> Result<(), String>;
    /// Whether the work last queued on `slot` has finished.
    fn finished(&self, slot: usize) -> bool;
    /// Queues the barrier that hands the picture of `slot` to the renderer for sampling, after its decode.
    fn acquire(&mut self, slot: usize) -> Result<(), String>;
    /// Queues the barrier that gives the picture of `slot` back to the decoder behind the draws the
    /// last flush submitted, and signals `slot`'s fence once it is done.
    fn release(&mut self, slot: usize) -> Result<(), String>;
    /// Queues the acquire of `acquire_slot` and release of `release_slot` (signaling
    /// `release_slot`'s fence) in one submission. Defaults to calling `acquire` then `release`.
    fn swap(&mut self, acquire_slot: usize, release_slot: usize) -> Result<(), String> {
        self.acquire(acquire_slot)?;
        self.release(release_slot)
    }
}

/// The pictures the presenter samples in place. The shown one is held until the next picture is
/// shown; then its release is queued after the draws that read it (behind the flush), and the
/// picture is handed back to the decoder only once that release's fence has finished. Each slot
/// is waited for before its next use, so at most `slots` pictures are held at once.
pub struct Sampled<P> {
    slots: usize,
    next: usize,
    /// The picture shown, and its slot.
    shown: Option<(P, usize)>,
    /// Released pictures whose fence has not been seen finished yet, and their slots.
    returning: Vec<(P, usize)>,
}

impl<P> Sampled<P> {
    pub fn new(slots: usize) -> Self {
        Self { slots, next: 0, shown: None, returning: Vec::new() }
    }

    /// Pictures held: the shown one and those not yet given back.
    pub fn held(&self) -> usize {
        self.returning.len() + usize::from(self.shown.is_some())
    }

    /// Shows `picture`: flushes the draws of the picture shown so far, samples `picture` on the next
    /// slot and queues the release of the one it replaces (combined in `swap`), then drops released
    /// pictures whose fence has finished in place. If `picture` cannot be sampled it comes back
    /// unshown with the error; a failed release leaves the replaced picture unreturned (the device
    /// is lost) and the caller stops sampling.
    pub fn show(&mut self, gpu: &mut impl SampleGpu, picture: P) -> Result<(), (P, String)> {
        if let Err(error) = gpu.flush() {
            return Err((picture, error));
        }
        let slot = self.next;
        if let Err(error) = gpu.wait(slot) {
            return Err((picture, error));
        }
        if let Some((old, old_slot)) = self.shown.take() {
            let swapped = gpu.swap(slot, old_slot);
            self.returning.push((old, old_slot));
            if let Err(error) = swapped {
                return Err((picture, error));
            }
        } else if let Err(error) = gpu.acquire(slot) {
            return Err((picture, error));
        }
        self.shown = Some((picture, slot));
        self.next = (slot + 1) % self.slots;
        self.collect(&*gpu);
        Ok(())
    }

    /// Ends the stream: releases the shown picture behind the last draws, then waits for every
    /// picture to come back and drops them in place.
    pub fn finish(&mut self, gpu: &mut impl SampleGpu) -> Result<(), String> {
        if let Some((old, old_slot)) = self.shown.take() {
            let released = gpu.flush().and_then(|()| gpu.release(old_slot));
            self.returning.push((old, old_slot));
            released?;
        }
        for &(_, slot) in &self.returning {
            gpu.wait(slot)?;
        }
        self.returning.clear();
        Ok(())
    }

    /// Drops released pictures whose fence has finished in place without allocating.
    fn collect(&mut self, gpu: &impl SampleGpu) {
        self.returning.retain_mut(|&mut (_, slot)| !gpu.finished(slot));
    }
}
