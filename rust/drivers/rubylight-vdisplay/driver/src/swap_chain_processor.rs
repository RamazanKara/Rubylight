//! The swap-chain worker: one thread per assignment that drains the IddCx swap chain (so the
//! virtual monitor stays a live display) and pushes each composed frame into the monitor's
//! frame ring.
//!
//! From punktfunk's pf-vdisplay `swap_chain_processor.rs` by Enrico Bühler (unom), MIT OR
//! Apache-2.0, with the encode pool replaced by [`OpenRing`] and D3D11 by IddCx 1.11's D3D12
//! swap chain.
//!
//! The OS presents the composed desktop through the swap chain and the driver must consume it
//! (acquire, then finished-processing) or the monitor stalls. Inside that window the worker
//! submits exactly one GPU copy into a ring texture on its [`Copier`] queue, or drops the
//! frame at the ring; it never waits on the host or the GPU there, because DWM waits on this
//! window for the head. Every acquire names that queue, so the OS orders its writes to the
//! surface before the copy.
//!
//! The swap-chain DDIs return an HRESULT-shaped `NTSTATUS`, classified by hand: `>= 0`
//! success, `E_PENDING` nothing composed yet, anything else an abandoned swap chain.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::thread::{self, JoinHandle};

use rubylight_vdisplay_proto::ring;
use wdk_sys::iddcx::{
    IDARG_IN_RELEASEANDACQUIREBUFFER2, IDARG_IN_SWAPCHAINSETDEVICE2,
    IDARG_IN_SWAPCHAINSETDEVICE2__bindgen_ty_1, IDARG_OUT_RELEASEANDACQUIREBUFFER2,
    IDDCX_METADATA2_VALID_FLAGS, IDDCX_SWAPCHAIN, IDDCX_SWAPCHAIN_DEVICE_TYPE,
};
use wdk_sys::{HANDLE, NTSTATUS, WDFOBJECT, call_unsafe_wdf_function_binding};
use windows::Win32::Foundation::{HANDLE as WHANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::Graphics::Direct3D12::{
    D3D12_HEAP_FLAG_NONE, D3D12_HEAP_PROPERTIES, D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_DESC,
    D3D12_RESOURCE_DIMENSION_TEXTURE2D, D3D12_RESOURCE_FLAG_NONE, D3D12_RESOURCE_STATE_COMMON,
    D3D12_TEXTURE_LAYOUT_UNKNOWN, ID3D12Resource,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::IDXGIResource;
use windows::Win32::System::Threading::{SetEvent, WaitForMultipleObjects, WaitForSingleObject};
use windows::core::Interface;

use crate::copier::Copier;
use crate::direct_3d_device::Direct3DDevice;
use crate::frame_retry::PendingFrame;
use crate::frame_ring::{Frame, OpenRing, Pushed, RingHandles, qpc_now, shape};
use crate::monitor::Monitor;
use crate::worker::{Mmcss, OwnedHandle, Sendable};

/// `E_PENDING`: the swap chain is valid but DWM composed nothing new.
const E_PENDING: u32 = 0x8000_000A;
/// Idle wait. Rings and stops arrive on the wake event; the timeout only keeps the ring's
/// heartbeat ticking over a desktop that composes nothing, so the host can tell an idle
/// desktop from a stalled worker.
const IDLE_WAIT_MS: u32 = 125;

fn hr_success(hr: NTSTATUS) -> bool {
    hr >= 0
}

/// Run the copy queue at global realtime priority (default on), so a GPU-saturating game does
/// not starve the copy every frame depends on. `RLVD_NO_RT_GPU` (any value, then a device
/// restart) keeps it at high priority.
fn rt_gpu_enabled() -> bool {
    use std::sync::OnceLock;
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| crate::log::knob("RLVD_NO_RT_GPU").is_none())
}

pub struct SwapChainProcessor {
    terminate: Arc<AtomicBool>,
    /// Auto-reset event that ends the worker's idle wait. It closes as a field, after `Drop`
    /// joined the worker, so [`Self::wake`] never signals a closed handle.
    wake: Option<OwnedHandle>,
    thread: Option<JoinHandle<()>>,
}

impl SwapChainProcessor {
    pub fn new() -> Self {
        let wake = OwnedHandle::event(false);
        if wake.is_none() {
            dbglog!("[rlvd] swap-chain: wake event creation failed; timeout-only idle wait");
        }
        Self {
            terminate: Arc::new(AtomicBool::new(false)),
            wake,
            thread: None,
        }
    }

    /// End the worker's idle wait so it re-reads the ring. Never blocks.
    pub fn wake(&self) {
        if let Some(h) = &self.wake {
            // SAFETY: our own event, closed only after `Drop` joined the worker.
            let _ = unsafe { SetEvent(h.as_raw()) };
        }
    }

    /// Start the worker for a fresh assignment. It owns `swap_chain` and deletes it before
    /// returning; `monitor` is a weak link, so the worker never keeps its monitor alive.
    pub fn run(
        &mut self,
        swap_chain: IDDCX_SWAPCHAIN,
        device: Arc<Direct3DDevice>,
        available_buffer_event: HANDLE,
        monitor: Weak<Monitor>,
    ) {
        let wake = self.wake.as_ref().map(|h| Sendable(h.as_raw()));
        let available_buffer_event = Sendable(available_buffer_event);
        let swap_chain = Sendable(swap_chain);
        let terminate = self.terminate.clone();
        let target_id = monitor.upgrade().map_or(0, |m| m.target_id());
        // A worker that never starts still owns the swap chain and must delete it.
        let sc_raw = swap_chain.0;
        let spawned = thread::Builder::new()
            .name("rlvd-swapchain".into())
            .spawn(move || {
                // Rebind the whole wrappers so the closure captures them, not their `!Send`
                // fields.
                let swap_chain = swap_chain;
                let wake = wake;
                let available_buffer_event = available_buffer_event;
                let _mmcss = Mmcss::distribution("swap-chain");
                Self::run_core(
                    swap_chain.0,
                    &device,
                    (wake.map(|w| w.0), available_buffer_event.0),
                    &terminate,
                    &monitor,
                    target_id,
                );
                dbglog!(
                    "[rlvd] swap-chain worker done (target={target_id}); deleting the swap chain"
                );
                // The swap chain goes before the device `Arc` it references.
                // SAFETY: the live swap chain this worker owns; the drain loop has exited.
                unsafe {
                    call_unsafe_wdf_function_binding!(WdfObjectDelete, swap_chain.0 as WDFOBJECT);
                }
            });
        match spawned {
            Ok(join_handle) => self.thread = Some(join_handle),
            Err(e) => {
                dbglog!("[rlvd] swap-chain worker did not start: {e}; deleting the swap chain");
                // SAFETY: the worker never ran, so this is the only reference to the live
                // swap chain IddCx handed over.
                unsafe {
                    call_unsafe_wdf_function_binding!(WdfObjectDelete, sc_raw as WDFOBJECT);
                }
            }
        }
    }

    fn run_core(
        swap_chain: IDDCX_SWAPCHAIN,
        device: &Arc<Direct3DDevice>,
        events: (Option<WHANDLE>, HANDLE),
        terminate: &AtomicBool,
        monitor: &Weak<Monitor>,
        target_id: u32,
    ) {
        let (wake, available_buffer_event) = events;
        let mut copier = match Copier::new(&device.device, rt_gpu_enabled()) {
            Ok(c) => c,
            Err(e) => {
                dbglog!(
                    "[rlvd] swap-chain: copy queue creation failed: {e:?} (target={target_id})"
                );
                return;
            }
        };
        // Borrow our reference; IddCx takes its own in SetDevice2.
        let set_device = IDARG_IN_SWAPCHAINSETDEVICE2 {
            Type: IDDCX_SWAPCHAIN_DEVICE_TYPE::IDDCX_SWAPCHAIN_DEVICE_TYPE_D3D12,
            Device: IDARG_IN_SWAPCHAINSETDEVICE2__bindgen_ty_1 {
                pD3d12Device: device.device.as_raw().cast(),
            },
        };
        // A failure means the OS already unassigned this swap chain; returning lets the
        // epilogue delete it so a fresh one is assigned.
        // SAFETY: the swap chain is live; `set_device` points at local storage.
        let hr = unsafe { wdk_iddcx::IddCxSwapChainSetDevice2(swap_chain, &set_device) };
        if !hr_success(hr) {
            dbglog!(
                "[rlvd] swap-chain: SetDevice2 failed ({hr:#x}, target={target_id}); waiting for a fresh swap chain"
            );
            return;
        }

        let mut rings = RingState::default();
        let mut logged_frame = false;
        loop {
            // Checked every pass: during a continuous burst the success branch never reaches
            // a wait, and an unassigned worker would otherwise pin its device.
            if terminate.load(Ordering::Relaxed) {
                break;
            }
            if device.is_removed() {
                dbglog!(
                    "[rlvd] swap-chain: device epoch {} removed (target={target_id}); exiting for reassignment",
                    device.epoch()
                );
                break;
            }
            let Some(owner) = monitor.upgrade() else {
                break;
            };
            rings.refresh(&owner, device, &mut copier);
            // Hand the strong count back before anything blocks.
            drop(owner);
            rings.beat();

            let mut in_args = IDARG_IN_RELEASEANDACQUIREBUFFER2::default();
            #[allow(clippy::cast_possible_truncation)]
            {
                in_args.Size = size_of::<IDARG_IN_RELEASEANDACQUIREBUFFER2>() as u32;
            }
            // Keep the GPU surface; no system-memory copy.
            in_args.AcquireSystemMemoryBuffer = 0;
            // The queue that reads the surface: the OS orders its writes before it.
            in_args.pD3D12CommandQueue = copier.queue().as_raw().cast();
            let mut buffer = IDARG_OUT_RELEASEANDACQUIREBUFFER2::default();
            // SAFETY: the swap chain is live; in/out point at local storage, and the queue
            // outlives every acquire of this worker.
            let hr: NTSTATUS = unsafe {
                wdk_iddcx::IddCxSwapChainReleaseAndAcquireBuffer2(
                    swap_chain,
                    &mut in_args,
                    &mut buffer,
                )
            };

            if (hr as u32) == E_PENDING {
                // A final desktop update may have arrived while all ring slots were busy.
                // Retry only the owned retained copy, after giving a newer acquisition
                // priority. No IddCx surface remains in use here.
                let Some(owner) = monitor.upgrade() else {
                    break;
                };
                if rings.retry(&owner, device, &mut copier) {
                    break;
                }
                let wait_ms = rings.wait_ms();
                drop(owner);
                let surface = WHANDLE(available_buffer_event.cast());
                // SAFETY: `surface` is the framework's event, live for this assignment; `w`
                // is this processor's wake event, closed only after this thread joins.
                let waited = unsafe {
                    match wake {
                        Some(w) => WaitForMultipleObjects(&[w, surface], false, wait_ms),
                        None => WaitForSingleObject(surface, wait_ms),
                    }
                };
                if waited == WAIT_OBJECT_0
                    || waited == WAIT_TIMEOUT
                    || waited.0 == WAIT_OBJECT_0.0 + 1
                {
                    continue;
                }
                dbglog!(
                    "[rlvd] swap-chain: idle wait -> {:#x} (target={target_id}); exiting",
                    waited.0
                );
                break;
            } else if hr_success(hr) {
                let meta = &buffer.MetaData;
                let frame = Frame {
                    present_qpc: meta.PresentDisplayQPCTime,
                    acquire_qpc: qpc_now(),
                    frame_number: u64::from(meta.PresentationFrameNumber),
                };
                // The acquire transfers one reference per surface it hands over: adopt each
                // and release it before FinishedProcessingFrame, or every assign/unassign
                // cycle leaks the swap chain's surfaces. The submitted copy survives the
                // release.
                let dxgi = meta.pSurface as *mut core::ffi::c_void;
                // SAFETY: a non-null `pSurface` is a live surface IddCx just handed over,
                // carrying the acquire's reference, which `from_raw` adopts exactly once.
                let dxgi = (!dxgi.is_null()).then(|| unsafe { IDXGIResource::from_raw(dxgi) });
                let d3d12 = IDDCX_METADATA2_VALID_FLAGS::IDDCX_METADATA2_VALID_FLAGS_D3D12_SURFACE;
                let raw = meta.pD3D12Surface as *mut core::ffi::c_void;
                let surface = (meta.ValidFlags & d3d12 != 0 && !raw.is_null())
                    // SAFETY: as above, for the D3D12 surface the flag says is valid.
                    .then(|| unsafe { ID3D12Resource::from_raw(raw) });
                if !logged_frame {
                    dbglog!(
                        "[rlvd] swap-chain: first frame acquired (target={target_id}, D3D12 surface {}, DXGI surface {})",
                        surface.is_some(),
                        dxgi.is_some()
                    );
                    logged_frame = true;
                }
                drop(dxgi);
                let mut lost = false;
                if let Some(surface) = &surface
                    && let Some(owner) = monitor.upgrade()
                {
                    lost = rings.push(&owner, device, &mut copier, surface, frame);
                }
                drop(surface);
                // SAFETY: the swap chain is live.
                let hr = unsafe { wdk_iddcx::IddCxSwapChainFinishedProcessingFrame(swap_chain) };
                if !hr_success(hr) || lost {
                    break;
                }
            } else {
                // Abandoned (e.g. DXGI_ERROR_ACCESS_LOST): the OS assigns a new one.
                break;
            }
        }
    }
}

impl Drop for SwapChainProcessor {
    fn drop(&mut self) {
        if let Some(handle) = self.thread.take() {
            // Flag first, then wake: the worker re-checks the flag at the loop top.
            self.terminate.store(true, Ordering::Relaxed);
            self.wake();
            let _ = handle.join();
        }
    }
}

/// The worker's view of the monitor's ring: the generation it last read and the ring opened
/// on this worker's device.
#[derive(Default)]
struct RingState {
    seen: Option<u32>,
    handles: Option<Arc<RingHandles>>,
    open: Option<OpenRing>,
    retained: Retained,
}

/// The newest composed image no ring holds. Windows stops presenting a desktop that does not
/// change, so a ring attached to a quiet monitor (a reconnect, or one rebuilt for the real
/// surface) would wait for the next change for its first frame; it is seeded from this
/// instead. A busy current ring also retries this copy when the swap chain becomes idle.
#[derive(Default)]
struct Retained {
    texture: Option<(ID3D12Resource, D3D12_RESOURCE_DESC)>,
    /// Metadata for this owned image, and whether the current ring still needs it.
    pending: PendingFrame,
}

impl Retained {
    /// A texture of `width` x `height` in `format` on `device`, made on first use or when
    /// the shape changes.
    fn texture(
        &mut self,
        device: &Direct3DDevice,
        width: u32,
        height: u32,
        format: DXGI_FORMAT,
    ) -> Option<&ID3D12Resource> {
        if !self
            .texture
            .as_ref()
            .is_some_and(|(_, d)| shape(d) == (width, height, format.0 as u32))
        {
            self.texture = None;
            self.pending.clear();
            let desc = D3D12_RESOURCE_DESC {
                Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
                Alignment: 0,
                Width: u64::from(width),
                Height: height,
                DepthOrArraySize: 1,
                MipLevels: 1,
                Format: format,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
                Flags: D3D12_RESOURCE_FLAG_NONE,
            };
            let heap = D3D12_HEAP_PROPERTIES {
                Type: D3D12_HEAP_TYPE_DEFAULT,
                ..Default::default()
            };
            let mut texture: Option<ID3D12Resource> = None;
            // SAFETY: `desc` describes a plain single-sample 2D texture on a default heap of
            // the live device; `texture` is a live local.
            let made = unsafe {
                device.device.CreateCommittedResource(
                    &heap,
                    D3D12_HEAP_FLAG_NONE,
                    &desc,
                    D3D12_RESOURCE_STATE_COMMON,
                    None,
                    &mut texture,
                )
            };
            if let Err(e) = made {
                dbglog!("[rlvd] swap-chain: the retained frame texture failed: {e:?}");
                return None;
            }
            self.texture = texture.map(|t| (t, desc));
        }
        self.texture.as_ref().map(|(t, _)| t)
    }

    /// Keep `surface` as the newest image.
    fn keep(
        &mut self,
        device: &Direct3DDevice,
        copier: &mut Copier,
        surface: &ID3D12Resource,
        desc: &D3D12_RESOURCE_DESC,
        frame: Frame,
    ) {
        // An unretainable newer image must not leave an older image eligible for retry.
        self.pending.clear();
        if desc.SampleDesc.Count != 1 || desc.DepthOrArraySize != 1 || desc.MipLevels > 1 {
            return;
        }
        let (width, height, _) = shape(desc);
        let Some(texture) = self.texture(device, width, height, desc.Format) else {
            return;
        };
        // Same size, format and sample count, on this device. The copy is queued before
        // FinishedProcessingFrame; later retries read only our owned texture, never the IddCx
        // surface after its next release/acquire.
        match copier.copy(&[(texture, surface)], None) {
            Ok(()) => self.pending.set(frame),
            Err(e) => dbglog!("[rlvd] swap-chain: keeping the retained frame failed: {e:?}"),
        }
    }

    /// Take the newest image back from a ring that is going away.
    fn take_from(
        &mut self,
        device: &Direct3DDevice,
        copier: &mut Copier,
        ring: &OpenRing,
        handles: &RingHandles,
    ) {
        self.pending.clear();
        let format = DXGI_FORMAT(handles.format as i32);
        let Some(texture) = self.texture(device, handles.width, handles.height, format) else {
            return;
        };
        if let Some(frame) = ring.copy_newest(copier, texture) {
            self.pending.set(frame);
        }
    }

    /// Publish the newest image into a freshly opened ring that fits it.
    fn seed(&mut self, device: &Direct3DDevice, copier: &mut Copier, ring: &mut OpenRing) {
        let (Some((texture, desc)), Some(current)) = (&self.texture, self.pending.current()) else {
            return;
        };
        if !ring.fits(desc) {
            return;
        }
        // The image is what the monitor still shows, so it is stamped as of now: the host
        // measures a frame's age from these, and the original present predates this ring.
        let now = qpc_now();
        let frame = Frame {
            present_qpc: now,
            acquire_qpc: now,
            frame_number: current.frame_number,
        };
        self.pending.set(frame);
        let pushed = self
            .pending
            .retry(|frame| ring.seed(device, copier, texture, desc, frame));
        dbglog!("[rlvd] frame ring seeded with the retained frame: {pushed:?}");
    }

    fn fits(&self, ring: &OpenRing) -> bool {
        self.pending.is_pending()
            && self
                .texture
                .as_ref()
                .is_some_and(|(_, desc)| ring.fits(desc))
    }

    fn retry(
        &mut self,
        device: &Direct3DDevice,
        copier: &mut Copier,
        ring: &mut OpenRing,
    ) -> Option<Pushed> {
        if !self.fits(ring) {
            return None;
        }
        let (texture, desc) = self.texture.as_ref()?;
        self.pending
            .retry(|frame| ring.retry(device, copier, texture, desc, frame))
    }
}

impl RingState {
    /// Re-read the ring when its generation moved, and open it on `device` once per ring.
    fn refresh(&mut self, monitor: &Monitor, device: &Direct3DDevice, copier: &mut Copier) {
        if self.seen == Some(monitor.ring_gen.load(Ordering::Acquire)) {
            return;
        }
        let (generation, handles) = monitor.ring();
        self.seen = Some(generation);
        if let (Some(open), Some(h)) = (&self.open, &handles)
            && open.is(h)
        {
            return;
        }
        if let (Some(open), Some(old)) = (&self.open, &self.handles)
            && self.retained.pending.current().is_none()
        {
            self.retained.take_from(device, copier, open, old);
        }
        self.open = None;
        self.handles = handles;
        let Some(h) = &self.handles else {
            return;
        };
        match OpenRing::open(h.clone(), device) {
            Ok(mut open) => {
                dbglog!(
                    "[rlvd] frame ring {} open: {} x {}x{} format {} (target={})",
                    h.generation,
                    h.slot_count,
                    h.width,
                    h.height,
                    h.format,
                    monitor.target_id()
                );
                monitor.stats.ring_open.store(true, Ordering::Release);
                self.retained.seed(device, copier, &mut open);
                self.open = Some(open);
            }
            Err((status, detail)) => {
                dbglog!(
                    "[rlvd] frame ring {} did not open: status {status} detail {detail:#x}",
                    h.generation
                );
                h.header().set_status(status, detail);
            }
        }
    }

    /// Stamp the drain loop alive into the header.
    fn beat(&self) {
        if let Some(h) = &self.handles {
            h.header().beat(qpc_now());
        }
    }

    /// Try the saved final image once without another swap-chain acquisition. Return true
    /// only for device loss, just as `push` does; slot ownership is never waited for here.
    fn retry(&mut self, monitor: &Monitor, device: &Direct3DDevice, copier: &mut Copier) -> bool {
        let Some(open) = &mut self.open else {
            return false;
        };
        match self.retained.retry(device, copier, open) {
            Some(Pushed::Published) => {
                monitor.stats.pushed.fetch_add(1, Ordering::AcqRel);
                false
            }
            Some(Pushed::DeviceLost) => true,
            _ => false,
        }
    }

    fn wait_ms(&self) -> u32 {
        if self
            .open
            .as_ref()
            .is_some_and(|open| self.retained.fits(open))
        {
            self.retained.pending.wait_ms()
        } else {
            IDLE_WAIT_MS
        }
    }

    /// Push one acquired surface. Returns true when the device was lost under it.
    fn push(
        &mut self,
        monitor: &Monitor,
        device: &Direct3DDevice,
        copier: &mut Copier,
        surface: &ID3D12Resource,
        frame: Frame,
    ) -> bool {
        // SAFETY: a plain query of the live surface.
        let desc = unsafe { surface.GetDesc() };
        let (width, height, format) = shape(&desc);
        // The host sizes its ring from these before it has one.
        let stats = &monitor.stats;
        stats.surface_size.store(
            u64::from(width) | (u64::from(height) << 32),
            Ordering::Release,
        );
        stats.surface_format.store(format, Ordering::Release);
        stats.acquired.fetch_add(1, Ordering::AcqRel);
        let Some(open) = &mut self.open else {
            if let Some(h) = &self.handles {
                // A ring that did not open still reports what the surface is.
                h.header().set_surface(width, height, format);
            }
            self.retained.keep(device, copier, surface, &desc, frame);
            return false;
        };
        match open.push(device, copier, surface, &desc, frame) {
            Pushed::Published => {
                stats.pushed.fetch_add(1, Ordering::AcqRel);
                // The ring holds the newest image now; it is taken back if the ring goes.
                self.retained.pending.clear();
                false
            }
            Pushed::Dropped | Pushed::Mismatch => {
                // Every texture is the host's, the copy queue was busy, or the host rebuilds
                // the ring at this surface's shape: this image is newer than any slot, and a
                // rebuilt ring is seeded from it.
                self.retained.keep(device, copier, surface, &desc, frame);
                false
            }
            Pushed::DeviceLost => {
                open.header().set_status(ring::STATUS_DEVICE_LOST, 0);
                true
            }
        }
    }
}
