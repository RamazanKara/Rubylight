//! The swap-chain worker: one thread per assignment that drains the IddCx swap chain (so the
//! virtual monitor stays a live display) and pushes each composed frame into the monitor's
//! frame ring.
//!
//! From punktfunk's pf-vdisplay `swap_chain_processor.rs` by Enrico Bühler (unom), MIT OR
//! Apache-2.0, with the encode pool replaced by [`OpenRing`].
//!
//! The OS presents the composed desktop through the swap chain and the driver must consume it
//! (acquire, then finished-processing) or the monitor stalls. Inside that window the worker
//! issues exactly one GPU copy into a ring texture, or drops the frame at the ring; it never
//! waits on the host there, because DWM waits on this window for the head.
//!
//! The swap-chain DDIs return an HRESULT-shaped `NTSTATUS`, classified by hand: `>= 0`
//! success, `E_PENDING` nothing composed yet, anything else an abandoned swap chain.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::thread::{self, JoinHandle};

use rubylight_vdisplay_proto::ring;
use wdk_sys::iddcx::{
    IDARG_IN_RELEASEANDACQUIREBUFFER2, IDARG_IN_SETREALTIMEGPUPRIORITY,
    IDARG_IN_SWAPCHAINSETDEVICE, IDARG_OUT_RELEASEANDACQUIREBUFFER2, IDDCX_SWAPCHAIN,
};
use wdk_sys::{HANDLE, NTSTATUS, WDFOBJECT, call_unsafe_wdf_function_binding};
use windows::Win32::Foundation::{HANDLE as WHANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::{IDXGIDevice, IDXGIResource};
use windows::Win32::System::Threading::{SetEvent, WaitForMultipleObjects, WaitForSingleObject};
use windows::core::Interface;

use crate::direct_3d_device::Direct3DDevice;
use crate::frame_ring::{Frame, OpenRing, Pushed, RingHandles, qpc_now};
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

/// Raise the swap chain's processing device to realtime GPU priority (default on), so a
/// GPU-saturating game does not starve the copy every frame depends on. `RLVD_NO_RT_GPU`
/// (any value, then a device restart) turns it off.
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
        // Borrow our reference; IddCx takes its own in SetDevice.
        let dxgi_device = match device.device.cast::<IDXGIDevice>() {
            Ok(d) => d,
            Err(e) => {
                dbglog!("[rlvd] swap-chain: ID3D11Device is not an IDXGIDevice: {e:?}");
                return;
            }
        };
        let set_device = IDARG_IN_SWAPCHAINSETDEVICE {
            pDevice: dxgi_device.as_raw().cast(),
        };
        // A failure means the OS already unassigned this swap chain; returning lets the
        // epilogue delete it so a fresh one is assigned.
        // SAFETY: the swap chain is live; `set_device` points at local storage.
        let hr = unsafe { wdk_iddcx::IddCxSwapChainSetDevice(swap_chain, &set_device) };
        if !hr_success(hr) {
            dbglog!(
                "[rlvd] swap-chain: SetDevice failed ({hr:#x}, target={target_id}); waiting for a fresh swap chain"
            );
            return;
        }
        if rt_gpu_enabled() {
            let rt = IDARG_IN_SETREALTIMEGPUPRIORITY {
                pDevice: dxgi_device.as_raw().cast(),
            };
            // SAFETY: the bound swap chain and its device, alive across the synchronous call.
            let hr = unsafe { wdk_iddcx::IddCxSetRealtimeGPUPriority(swap_chain, &rt) };
            dbglog!(
                "[rlvd] swap-chain: realtime GPU priority -> {hr:#x} (target={target_id}; RLVD_NO_RT_GPU disables)"
            );
        }
        drop(dxgi_device);

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
            rings.refresh(&owner, device);
            // Hand the strong count back before anything blocks.
            drop(owner);
            rings.beat();

            let mut in_args = IDARG_IN_RELEASEANDACQUIREBUFFER2::default();
            #[allow(clippy::cast_possible_truncation)]
            {
                in_args.Size = size_of::<IDARG_IN_RELEASEANDACQUIREBUFFER2>() as u32;
            }
            // Keep the GPU surface (MetaData.pSurface); no system-memory copy.
            in_args.AcquireSystemMemoryBuffer = 0;
            let mut buffer = IDARG_OUT_RELEASEANDACQUIREBUFFER2::default();
            // SAFETY: the swap chain is live; in/out point at local storage.
            let hr: NTSTATUS = unsafe {
                wdk_iddcx::IddCxSwapChainReleaseAndAcquireBuffer2(
                    swap_chain,
                    &mut in_args,
                    &mut buffer,
                )
            };

            if (hr as u32) == E_PENDING {
                let surface = WHANDLE(available_buffer_event.cast());
                // SAFETY: `surface` is the framework's event, live for this assignment; `w`
                // is this processor's wake event, closed only after this thread joins.
                let waited = unsafe {
                    match wake {
                        Some(w) => WaitForMultipleObjects(&[w, surface], false, IDLE_WAIT_MS),
                        None => WaitForSingleObject(surface, IDLE_WAIT_MS),
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
                let frame = Frame {
                    present_qpc: buffer.MetaData.PresentDisplayQPCTime,
                    acquire_qpc: qpc_now(),
                    frame_number: u64::from(buffer.MetaData.PresentationFrameNumber),
                };
                if !logged_frame {
                    dbglog!("[rlvd] swap-chain: first frame acquired (target={target_id})");
                    logged_frame = true;
                }
                let mut lost = false;
                // The acquire transfers one surface reference to the driver: adopt it and
                // release it before FinishedProcessingFrame, or every assign/unassign cycle
                // leaks the swap chain's surfaces. The queued copy survives the release.
                let raw = buffer.MetaData.pSurface as *mut core::ffi::c_void;
                if !raw.is_null() {
                    // SAFETY: `raw` is the live surface IddCx just handed over, carrying the
                    // acquire's reference, which `from_raw` adopts exactly once.
                    let res = unsafe { IDXGIResource::from_raw(raw) };
                    if let Ok(tex) = res.cast::<ID3D11Texture2D>()
                        && let Some(owner) = monitor.upgrade()
                    {
                        lost = rings.push(&owner, device, &tex, frame);
                    }
                }
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
/// instead.
#[derive(Default)]
struct Retained {
    texture: Option<(ID3D11Texture2D, D3D11_TEXTURE2D_DESC)>,
    /// The texture holds the newest image, with this `PresentationFrameNumber`.
    current: Option<u64>,
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
    ) -> Option<&ID3D11Texture2D> {
        if !self
            .texture
            .as_ref()
            .is_some_and(|(_, d)| (d.Width, d.Height, d.Format) == (width, height, format))
        {
            self.texture = None;
            self.current = None;
            let desc = D3D11_TEXTURE2D_DESC {
                Width: width,
                Height: height,
                MipLevels: 1,
                ArraySize: 1,
                Format: format,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: 0,
                CPUAccessFlags: 0,
                MiscFlags: 0,
            };
            let mut texture = None;
            // SAFETY: `desc` describes a plain single-sample texture; `texture` is a live
            // local.
            let made = unsafe {
                device
                    .device
                    .CreateTexture2D(&desc, None, Some(&mut texture))
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
        surface: &ID3D11Texture2D,
        desc: &D3D11_TEXTURE2D_DESC,
        frame_number: u64,
    ) {
        if desc.SampleDesc.Count != 1 || desc.ArraySize != 1 || desc.MipLevels > 1 {
            return;
        }
        let Some(texture) = self.texture(device, desc.Width, desc.Height, desc.Format) else {
            return;
        };
        // SAFETY: both live on this device with the same size, format and sample count.
        unsafe { device.device_context.CopyResource(texture, surface) };
        self.current = Some(frame_number);
    }

    /// Take the newest image back from a ring that is going away.
    fn take_from(&mut self, device: &Direct3DDevice, ring: &OpenRing, handles: &RingHandles) {
        let format = DXGI_FORMAT(handles.format as i32);
        let Some(texture) = self.texture(device, handles.width, handles.height, format) else {
            return;
        };
        self.current = ring.copy_newest(device, texture);
    }

    /// Publish the newest image into a freshly opened ring that fits it.
    fn seed(&self, device: &Direct3DDevice, ring: &mut OpenRing) {
        let (Some((texture, desc)), Some(frame_number)) = (&self.texture, self.current) else {
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
            frame_number,
        };
        let pushed = ring.seed(device, texture, desc, frame);
        dbglog!("[rlvd] frame ring seeded with the retained frame: {pushed:?}");
    }
}

impl RingState {
    /// Re-read the ring when its generation moved, and open it on `device` once per ring.
    fn refresh(&mut self, monitor: &Monitor, device: &Direct3DDevice) {
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
            && self.retained.current.is_none()
        {
            self.retained.take_from(device, open, old);
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
                self.retained.seed(device, &mut open);
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

    /// Push one acquired surface. Returns true when the device was lost under it.
    fn push(
        &mut self,
        monitor: &Monitor,
        device: &Direct3DDevice,
        surface: &ID3D11Texture2D,
        frame: Frame,
    ) -> bool {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: `surface` is live; `desc` is a live local.
        unsafe { surface.GetDesc(&mut desc) };
        // The host sizes its ring from these before it has one.
        let stats = &monitor.stats;
        stats.surface_size.store(
            u64::from(desc.Width) | (u64::from(desc.Height) << 32),
            Ordering::Release,
        );
        stats
            .surface_format
            .store(desc.Format.0 as u32, Ordering::Release);
        stats.acquired.fetch_add(1, Ordering::AcqRel);
        let Some(open) = &mut self.open else {
            if let Some(h) = &self.handles {
                // A ring that did not open still reports what the surface is.
                h.header()
                    .set_surface(desc.Width, desc.Height, desc.Format.0 as u32);
            }
            self.retained
                .keep(device, surface, &desc, frame.frame_number);
            return false;
        };
        match open.push(device, surface, &desc, frame) {
            Pushed::Published => {
                stats.pushed.fetch_add(1, Ordering::AcqRel);
                // The ring holds the newest image now; it is taken back if the ring goes.
                self.retained.current = None;
                false
            }
            Pushed::Dropped => {
                // Every texture is the host's; this image is newer than any of them.
                self.retained
                    .keep(device, surface, &desc, frame.frame_number);
                false
            }
            Pushed::Mismatch => {
                // The host rebuilds the ring at this surface's shape; it is seeded from this.
                self.retained
                    .keep(device, surface, &desc, frame.frame_number);
                false
            }
            Pushed::DeviceLost => {
                open.header().set_status(ring::STATUS_DEVICE_LOST, 0);
                true
            }
        }
    }
}
