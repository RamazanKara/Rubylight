//! Capture from Rubylight's own virtual display driver (rust/drivers/rubylight-vdisplay).
//!
//! The driver copies every frame Windows composes for its monitor into textures this session
//! creates and shares with it, stamps the frame's present QPC beside it and sets an event
//! (`rubylight_vdisplay_proto::ring`). No WGC and no Desktop Duplication run: a frame is one
//! GPU copy in the driver.
//!
//! The default fence ring uses `CAP_FENCE_RING`: its textures and fences
//! are created on the capture GPU's D3D12 compute device. The compute conversion reads
//! the ring texture in place, waiting on the GPU for the driver's written fence, and the
//! compute queue signals the released fence once the conversion is done; no D3D11 work runs
//! for it. Without a compute queue (and with `idd_push_zero_copy = false`) the capture's D3D11 device waits
//! for the written fence, copies the texture into the owned pool as every other capture's
//! frames are, and signals the released fence after its copy.
//!
//! The keyed-mutex ring remains available with `idd_push_fence_ring = false`. It is also used,
//! with a warning, when fence-ring initialization fails or the installed driver does not
//! support the fence ring.
//!
//! It runs for every automatic capture of a display this host created, and for
//! `capture = idd-push`; physical displays, and `capture = wgc` or `ddx`, keep WGC or Desktop
//! Duplication.

use super::*;
use butterpollo_media_policy::WarningSink;
use butterpollo_media_policy::virtual_output::{PushedFrame, PushedFrames};
#[cfg(test)]
use rubylight_vdisplay_proto::control::{CAP_FENCE_RING, MonitorStatus};
use rubylight_vdisplay_proto::control::{
    CAP_SCOPED_CLEAR, SetFenceRingRequest, SetFrameRingRequest,
};
#[cfg(test)]
use rubylight_vdisplay_proto::ring::Header;
use rubylight_vdisplay_proto::ring::{self, Consumer, Counters, Kind, Layout, SlotMeta};
use std::os::windows::io::AsRawHandle;
#[cfg(test)]
use std::os::windows::io::OwnedHandle;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use windows::Win32::Foundation::HANDLE;

mod vdd;
use vdd::Remote;
pub(super) use vdd::wanted_for;
pub use vdd::{VddBridge, VddControl, VddOwned};

mod fences;
mod frame_ring;
mod transport;
use fences::Fences;
pub(crate) use fences::Lent;
pub(super) use frame_ring::wanted;
use frame_ring::*;

pub struct Session {
    pub(super) gpu: Device,
    control: Box<dyn VddControl>,
    monitor: VddOwned,
    remote: Arc<Remote>,
    adapter: (u32, i32),
    ring: Ring,
    /// Where a fence ring's textures and fences live: the capture GPU's compute device.
    /// `None` when the driver takes only keyed-mutex rings.
    compute: Option<Arc<crate::compute::Compute>>,
    consumer: Consumer,
    pool: GpuPool,
    /// Lends keyed-mutex ring textures to the compute conversion instead of copying them.
    /// A fence ring lends without it.
    lender: Option<Arc<Mutex<crate::compute::Returns>>>,
    /// Whether `pool.compute` (`Handoff`) should be lazily created if copying is needed.
    wants_handoff: bool,
    /// The compute conversion reads ring textures in place (`idd_push_zero_copy`).
    zero_copy: bool,
    frame: Arc<crate::timing::Signal>,
    staging: Option<ID3D11Texture2D>,
    started: bool,
    reshapes: u32,
    opened: Instant,
    next_check: Instant,
    counted: (Instant, Counters),
    /// Released after `Drop` cleared the ring.
    _exclusive: Exclusive,
}

impl Session {
    pub(super) fn new(
        name: &str,
        hdr: bool,
        config: &butterpollo_config::config::Config,
        warnings: Arc<dyn WarningSink>,
        vdd: Option<&dyn VddBridge>,
    ) -> Result<Self> {
        let vdd = vdd.context("Rubylight virtual display driver bridge is not configured")?;
        let monitor = vdd.owned_display(name)?.with_context(|| {
            format!("{name} is not a display this host created with the Rubylight display driver")
        })?;
        let exclusive = Exclusive::claim(monitor.session_id).with_context(|| {
            format!("another stream with different capture settings already captures {name} from the Rubylight display driver")
        })?;
        let control = vdd.open_control()?;
        let status = control.query(monitor.session_id)?;
        let render = (status.render_luid_low, status.render_luid_high);
        // Open on the GPU the display renders on, resolved as its render
        // adapter was, not on whichever adapter Windows lists it under.
        let luid = super::luid_resolve::capture_luid(
            render,
            config.get("adapter_name", ""),
            config.get("adapter_pnp_id", ""),
            super::gpus,
        )
        .inspect_err(|error| warnings.set(super::luid_resolve::WARNING, &format!("{error:#}")))?;
        warnings.clear(super::luid_resolve::WARNING);
        let gpu = match luid {
            Some(luid) => Device::for_display_on(name, luid)?,
            None => Device::for_display(name, "", "")?,
        };
        // SAFETY: the device is live; GetAdapter and GetDesc return owned values.
        let adapter = unsafe {
            let desc = gpu.device.cast::<IDXGIDevice>()?.GetAdapter()?.GetDesc()?;
            (desc.AdapterLuid.LowPart, desc.AdapterLuid.HighPart)
        };
        if render != (0, 0) && render != adapter {
            bail!(
                "the Rubylight display renders on GPU {:08x}:{:08x}, but capture opened on {:08x}:{:08x}",
                render.1,
                render.0,
                adapter.1,
                adapter.0
            );
        }
        if let Some(failures) = FAILURES
            .lock()
            .unwrap()
            .excluded(FenceScope::new(&monitor, adapter))
        {
            bail!(
                "its frame ring failed {failures} times before delivering a frame; capturing with WGC until the display is recreated, the driver restarts or it renders on another GPU"
            );
        }
        if let Err(error) = crate::gpu_priority::configure(&gpu, config) {
            tracing::debug!(%error, "GPU priority remains at the process default");
        }
        let remote = Arc::new(Remote::open(monitor.wudf_pid)?);
        let pool = GpuPool {
            warnings: warnings.clone(),
            ..Default::default()
        };
        // Defer `Handoff` (its command queue and shared fences) until a copy path
        // actually runs: default zero-copy rings never copy through `pool`.
        let compute_device = if crate::compute::enabled(config) {
            match crate::compute::Compute::for_device(&gpu.device) {
                Ok(compute) => Some(compute),
                Err(error) => {
                    warnings.set("capture_compute", &format!("Compute capture copies unavailable ({error:#}); copying on the graphics queue. Lower game GPU load or update the graphics driver if capture stutters."));
                    None
                }
            }
        } else {
            None
        };
        let wants_handoff = compute_device.is_some();
        let zero_copy = wants_handoff && config.boolean("idd_push_zero_copy", true);
        let requested = config.idd_push_fence_ring();
        let scope = FenceScope::new(&monitor, adapter);
        let rejected = {
            let mut failures = FENCE_FAILURES.lock().unwrap();
            if !requested {
                failures.clear(monitor.session_id);
            }
            failures.excluded(scope)
        };
        let RingChoice { kind, warning } =
            choose_ring(control.caps(), requested, rejected, zero_copy);
        warnings.clear("capture_fence_ring");
        if let Some(warning) = warning {
            tracing::warn!(output = %name, "{warning}");
            warnings.set("capture_fence_ring", warning);
        }
        // Before the first frame the driver may not know the surface yet: guess from the
        // display's colour state, and rebuild once it reports the real one.
        let native_hdr = hdr
            || gpu
                .color_description()?
                .is_some_and(|d| d.color_space == DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020);
        let (width, height, format) = ring_shape(&status, native_hdr);
        let layout = Layout {
            kind,
            slot_count: ring::DEFAULT_SLOTS as u32,
            width,
            height,
            format,
            generation: next_generation(status.ring_generation),
        };
        let frame = Arc::new(crate::timing::Signal::new()?);
        let ring = Self::make_ring(
            &gpu,
            compute_device,
            layout,
            zero_copy,
            &warnings,
            scope,
            Some(remote.clone()),
        )?;
        let compute = ring.fences.as_ref().map(|fences| fences.compute.clone());
        let now = Instant::now();
        let mut session = Self {
            gpu,
            control,
            monitor,
            remote,
            adapter,
            consumer: Consumer::new(layout.generation, layout.slot_count),
            ring,
            compute,
            pool,
            lender: None,
            wants_handoff,
            zero_copy,
            frame,
            staging: None,
            started: false,
            reshapes: 0,
            opened: now,
            next_check: now,
            counted: (now, Counters::default()),
            _exclusive: exclusive,
        };
        if let Err(error) = session.deliver() {
            if session.ring.fences.is_none() {
                return Err(error);
            }
            session.use_keyed_ring(&format!("initialization failed: {error:#}"), true)?;
        }
        tracing::info!(
            output = %name,
            width,
            height,
            format,
            ring = ?session.ring.layout.kind,
            target = session.monitor.target,
            compute = session.pool.compute.is_some()
                || session.compute.is_some()
                || session.lender.is_some(),
            in_place = session.zero_copy,
            gpu = %session.gpu.display.adapter,
            capture_luid = ?session.adapter,
            render_luid = ?render,
            "capturing from the Rubylight display driver's frame push"
        );
        Ok(session)
    }

    pub(super) fn frame_signal(&self) -> &crate::timing::Signal {
        &self.frame
    }
    pub(super) fn pending_frame(&self) -> bool {
        self.consumer
            .wanted(self.ring.section.header().latest())
            .is_some()
    }

    pub(super) fn next_frame(&mut self) -> Result<Option<Image>> {
        self.next_gpu(0)?
            .map(|image| image.readback(&mut self.staging))
            .transpose()
    }

    pub(super) fn next_gpu(&mut self, origin: u64) -> Result<Option<std::sync::Arc<GpuImage>>> {
        let now = Instant::now();
        if now >= self.next_check {
            self.next_check = now + Duration::from_millis(100);
            if let Err(error) = self.check() {
                if !self.started && crate::device_loss::DeviceLost::from_error(&error).is_none() {
                    let failures = FAILURES
                        .lock()
                        .unwrap()
                        .failed(FenceScope::new(&self.monitor, self.adapter));
                    tracing::warn!(error = %format!("{error:#}"), failures, "the Rubylight frame ring failed before its first frame");
                }
                return Err(error);
            }
        }
        let Some((frame, mut image)) = self.take_newest()? else {
            return Ok(None);
        };
        (image.captured, image.wgc_stamp) =
            qpc_timestamps(frame.presented().min(i64::MAX as u64) as i64);
        image.published = Instant::now();
        image.origin = origin;
        // This exact, immutable host timestamp also appears in the stream's claim
        // trace, joining driver identity to the selected image without changing
        // frame ownership or carrying diagnostic metadata through every backend.
        tracing::trace!(
            target: "idd_push",
            seq = frame.seq,
            frame = frame.frame_number,
            acquired_qpc = frame.acquired,
            scanout_qpc = frame.scanout,
            host_acquired = ?image.acquired,
            "image"
        );
        // The picture leaves the capture wrapped: the stream threads own it
        // through the same reference, without a clone of it.
        Ok(Some(std::sync::Arc::new(image)))
    }

    /// Record `meta`'s frame as consumed (taken from the ring at QPC `taken`).
    fn finish(&mut self, meta: &SlotMeta, taken: u64) -> PushedFrame {
        self.consumer.consumed(meta.seq);
        self.ring
            .section
            .header()
            .set_consumed(self.consumer.last());
        if !self.started {
            FAILURES.lock().unwrap().delivered(self.monitor.session_id);
        }
        self.started = true;
        let frequency = qpc_frequency();
        tracing::trace!(
            target: "idd_push",
            seq = meta.seq,
            frame = meta.frame_number,
            scanout_ahead_us = qpc_micros(meta.acquire_qpc, meta.present_qpc, frequency),
            driver_us = qpc_micros(meta.acquire_qpc, meta.publish_qpc, frequency),
            handoff_us = qpc_micros(meta.publish_qpc, taken, frequency),
            "frame"
        );
        PushedFrame {
            seq: meta.seq,
            frame_number: meta.frame_number,
            acquired: meta.acquire_qpc,
            scanout: meta.present_qpc,
        }
    }

    /// The newest frame of a fence ring. Read in place, the slot is the image's until its
    /// last clone drops, and the compute conversion waits for the driver's copy on the GPU;
    /// copied, the capture's D3D11 device waits for that copy, copies and frees the slot
    /// behind its copy at once.
    fn take_fenced(&mut self, fences: &Arc<Fences>) -> Result<Option<(PushedFrame, GpuImage)>> {
        if !self.zero_copy && !self.pool.has_capacity() {
            return Ok(None);
        }
        let header = self.ring.section.header();
        let Some((index, seq)) = self.consumer.claim(&header) else {
            // Nothing new, or the driver is refilling that slot with a newer frame; its
            // publish wakes the next poll.
            return Ok(None);
        };
        let taken = qpc_now().max(0) as u64;
        let meta = header.slot(index);
        let slot = &self.ring.slots[index];
        if self.zero_copy {
            let compute_read = Arc::new(AtomicBool::new(false));
            let lent = Lent {
                fences: fences.clone(),
                slot: index,
                seq,
                d3d11: Mutex::new(false),
                compute_read: compute_read.clone(),
            };
            let ready =
                crate::compute::Ready::on(&fences.compute, fences.written.clone(), u64::from(seq))
                    .with_compute_read(compute_read);
            let image = lend_fenced_slot(&self.gpu, slot, index, lent, ready);
            return Ok(Some((self.finish(&meta, taken), image)));
        }
        let copied = fences
            .order_d3d11(seq)
            .and_then(|()| self.pool.copy(&self.gpu, &slot.texture, None));
        // The copy is queued on this device (or ordered before it by the compute handoff),
        // so freeing now cannot let the driver overwrite pixels still being read.
        let freed = fences.free(index, seq, false, false);
        let image = copied?;
        freed?;
        let Some(image) = image else {
            return Ok(None);
        };
        Ok(Some((self.finish(&meta, taken), image)))
    }

    /// The newest frame of the default keyed-mutex ring.
    fn take_keyed(&mut self) -> Result<Option<(PushedFrame, GpuImage)>> {
        // Leave the newest frame published while every snapshot is held: the driver keeps
        // the texture updated, and it is taken once a snapshot frees. A lent texture needs
        // no snapshot.
        if self.lender.is_none() && !self.pool.has_capacity() {
            return Ok(None);
        }
        let header = self.ring.section.header();
        let Some(index) = self.consumer.wanted(header.latest()) else {
            return Ok(None);
        };
        let slot = &self.ring.slots[index];
        let mutex = slot.mutex.as_ref().context("a keyed-mutex ring's slot")?;
        if !acquire(mutex, ring::KEY_HOST)? {
            // The driver is refilling it with a newer frame; its publish wakes the next poll.
            return Ok(None);
        }
        let taken = qpc_now().max(0) as u64;
        let meta = header.slot(index);
        let image = if !self.consumer.fresh(&meta) {
            Ok(None)
        } else if let Some(lender) = &self.lender {
            match lend(&self.gpu, slot, mutex, index, lender.clone()) {
                // The mutex goes back with the image's last clone.
                Ok(image) => return Ok(Some((self.finish(&meta, taken), image))),
                Err(error) => Err(error),
            }
        } else {
            self.pool.copy(&self.gpu, &slot.texture, None)
        };
        // A copy is queued on this device (or ordered before it by the compute handoff), so
        // releasing now cannot let the driver overwrite pixels still being read.
        // SAFETY: this process took the mutex with KEY_HOST above; the context is this
        // device's and multithread-protected.
        unsafe {
            mutex.ReleaseSync(ring::KEY_DRIVER)?;
            self.gpu.context.Flush();
        }
        let Some(image) = image? else {
            return Ok(None);
        };
        Ok(Some((self.finish(&meta, taken), image)))
    }
}

impl PushedFrames for Session {
    type Image = GpuImage;

    fn take_newest(&mut self) -> Result<Option<(PushedFrame, GpuImage)>> {
        match self.ring.fences.clone() {
            Some(fences) => self.take_fenced(&fences),
            None => self.take_keyed(),
        }
    }
}

#[cfg(test)]
fn pixel_of(texture: &ID3D11Texture2D) -> Result<(u32, u32, Pixel)> {
    let mut desc = D3D11_TEXTURE2D_DESC::default();
    // SAFETY: `texture` is a live texture and `desc` a live local.
    unsafe { texture.GetDesc(&mut desc) };
    Ok((desc.Width, desc.Height, ring_pixel(desc.Format)?))
}

fn in_place_cached(
    gpu: &Device,
    texture: Arc<ID3D11Texture2D>,
    (width, height, pixel): (u32, u32, Pixel),
    ready: crate::compute::Ready,
    lease: bridge::Lease,
) -> GpuImage {
    let now = Instant::now();
    GpuImage {
        cursor: None,
        width,
        height,
        pixel,
        captured: now,
        wgc_stamp: None,
        acquired: now,
        published: now,
        gpu: gpu.clone(),
        texture,
        ready: Some(ready),
        lease: Some(Arc::new(lease)),
        focus: None,
        repeat: false,
        origin: 0,
    }
}

/// An image of fence-ring `slot`, read in place using its cached immutable shape.
fn lend_fenced_slot(
    gpu: &Device,
    slot: &Slot,
    index: usize,
    lent: Lent,
    ready: crate::compute::Ready,
) -> GpuImage {
    in_place_cached(
        gpu,
        slot.texture.clone(),
        slot.shape,
        ready,
        bridge::Lease::fenced(index, lent),
    )
}

/// An image of fence-ring texture `index`, read in place (for tests that pass a raw texture).
#[cfg(test)]
fn lend_fenced(
    gpu: &Device,
    texture: &ID3D11Texture2D,
    index: usize,
    lent: Lent,
    ready: crate::compute::Ready,
) -> Result<GpuImage> {
    let shape = pixel_of(texture)?;
    Ok(in_place_cached(
        gpu,
        Arc::new(texture.clone()),
        shape,
        ready,
        bridge::Lease::fenced(index, lent),
    ))
}

/// An image of keyed-mutex ring texture `index`, read in place: this process holds its keyed
/// mutex (taken with KEY_HOST) until the image's lease hands it back with KEY_DRIVER.
fn lend(
    gpu: &Device,
    slot: &Slot,
    mutex: &IDXGIKeyedMutex,
    index: usize,
    returns: Arc<Mutex<crate::compute::Returns>>,
) -> Result<GpuImage> {
    let (ready, compute_read) = returns.lock().unwrap().acquired()?;
    Ok(in_place_cached(
        gpu,
        slot.texture.clone(),
        slot.shape,
        ready,
        bridge::Lease::ring(
            index,
            crate::compute::KeyedLease::new(mutex.clone(), returns, compute_read, 0),
        ),
    ))
}

impl Session {
    /// Synchronously clear the active ring in the display driver (waiting for
    /// `rlvd-swapchain` to drain its D3D11 GPU work and drop its shared handles)
    /// and then CPU-wait up to a bounded budget on the latest driver `written`
    /// fence before releasing D3D12 shared textures and fences (#148).
    pub(super) fn clear_ring(&self) {
        if self.remote.has_exited() {
            tracing::debug!("Rubylight display driver exited; skipping frame ring clear wait");
            if let Some(fences) = &self.ring.fences {
                let _ = fences.wait_written_within(Duration::ZERO);
            }
            return;
        }
        let started = Instant::now();
        let cleared = if self.control.caps() & CAP_SCOPED_CLEAR != 0 {
            self.control
                .clear_frame_ring_generation(self.monitor.session_id, self.ring.layout.generation)
        } else {
            self.control.clear_frame_ring(self.monitor.session_id)
        };
        let wait_budget = match cleared {
            Ok(()) => Duration::from_secs(2)
                .saturating_sub(started.elapsed())
                .min(Duration::from_millis(250)),
            Err(error) => {
                tracing::debug!(
                    error = %format!("{error:#}"),
                    "Rubylight frame ring was already gone"
                );
                Duration::ZERO
            }
        };
        if let Some(fences) = &self.ring.fences
            && !fences.wait_written_within(wait_budget)
        {
            fences.mark_clear_timed_out();
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // A direct reader error can close the capture before its next periodic check.
        // Preserve the rejection so that reopening cannot immediately retry this ring.
        if self.ring.fences.as_ref().is_some_and(|fences| {
            fences.failed.load(Ordering::Acquire) || fences.compute.is_stuck()
        }) {
            FENCE_FAILURES
                .lock()
                .unwrap()
                .failed(FenceScope::new(&self.monitor, self.adapter));
        }
        // Wait for the driver to drain D3D11 and release its duplicates before
        // `self.ring` and `self.compute` drop. Only this ring generation is
        // cleared so one registered since (another process's, say) stays.
        self.clear_ring();
    }
}

// SAFETY: the D3D11 objects live on a multithread-protected device, the section view is used
// only through atomics, and every handle is owned; the session moves between threads with
// the capture, never shared.
unsafe impl Send for Session {}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod fence_tests;
