//! Windows Direct3D 11 video decoding isolation:
//! - Hooks FFmpeg's `AVD3D11VADeviceContext::lock`/`unlock` to `ID3D10Multithread::Enter`/`Leave`.
//! - Uses `ID3D11DeviceContext1::CopySubresourceRegion1(..., D3D11_COPY_DISCARD)` for non-shared
//!   static video textures, with fallback to `CopySubresourceRegion`.
//! - Creates a dedicated `D3D11_CREATE_DEVICE_VIDEO_SUPPORT` decode device on the renderer's DXGI
//!   adapter, exporting decoded frames via NT-handle shared single-slice textures and monitored
//!   `ID3D11Fence`s so FFmpeg never shares SDL3's immediate context.

use std::ffi::c_void;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::ThreadId;

use windows::core::Interface;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1,
};
use windows::Win32::Graphics::Direct3D10::ID3D10Multithread;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::{IDXGIAdapter, IDXGIDevice};

use super::ffi::{
    av_hwdevice_ctx_alloc, av_hwdevice_ctx_init, AVCodecContext, AVD3D11VADeviceContext,
    AVD3D11VAFramesContext, AVFrame, AVHWDeviceContext, AVHWDeviceType, AVHWFramesContext,
    AV_PIX_FMT_D3D11,
};
use super::{D3d11Removal, D3d11Surface, Pictures};

mod resources;
mod sync;
use resources::{
    create_shared_fence_pair, create_shared_texture_pair, preflight_format_and_srv,
    preflight_formats,
};
pub(super) use sync::device_removed_reason;
use sync::{choose_reuse_slot, fence_completed_ready, is_device_removed_code, SlotChoice};

pub(super) fn clone_device_ref(raw: *mut c_void) -> Option<ID3D11Device> {
    // SAFETY: Caller passes null or a borrowed `ID3D11Device` pointer on this thread.
    unsafe { ID3D11Device::from_raw_borrowed(&raw) }.cloned()
}

pub(crate) const HARD_SLOT_CAP: usize = 32;
const SHARED_DEVICE_ENV: &str = "NIGHTJAR_D3D11_SHARED_DEVICE";
static NEXT_BRIDGE_ID: AtomicU64 = AtomicU64::new(1);

/// RAII guard for `ID3D10Multithread::Enter` / `Leave`.
struct MtGuard<'a>(&'a ID3D10Multithread);

impl<'a> MtGuard<'a> {
    fn lock(mt: &'a ID3D10Multithread) -> Self {
        // SAFETY: `mt` is a live `ID3D10Multithread` with multithread protection enabled.
        unsafe { mt.Enter() };
        Self(mt)
    }
}

impl Drop for MtGuard<'_> {
    fn drop(&mut self) {
        // SAFETY: Paired with `Enter` in `MtGuard::lock`.
        unsafe { self.0.Leave() };
    }
}

/// Lock state stored in `AVD3D11VADeviceContext::lock_ctx` (freed in `d3d11_device_ctx_free`),
/// caching `ID3D11DeviceContext1` for the shared-device fallback copy path.
struct DeviceLock {
    mt: ID3D10Multithread,
    context1: Option<ID3D11DeviceContext1>,
}

unsafe extern "C" fn d3d11_mt_lock(ctx: *mut c_void) {
    // SAFETY: `ctx` is a boxed `DeviceLock` kept alive until `d3d11_device_ctx_free`.
    if !ctx.is_null() { unsafe { (*ctx.cast::<DeviceLock>()).mt.Enter() }; }
}

unsafe extern "C" fn d3d11_mt_unlock(ctx: *mut c_void) {
    // SAFETY: Paired with `d3d11_mt_lock` on the same live `DeviceLock`.
    if !ctx.is_null() { unsafe { (*ctx.cast::<DeviceLock>()).mt.Leave() }; }
}

unsafe extern "C" fn d3d11_device_ctx_free(ctx: *mut AVHWDeviceContext) {
    if ctx.is_null() { return; }
    // SAFETY: FFmpeg's `hwdevice_ctx_free` calls `device_uninit` strictly before `ctx->free`.
    unsafe {
        let d3d = &mut *((*ctx).hwctx.cast::<AVD3D11VADeviceContext>());
        let raw = std::mem::replace(&mut d3d.lock_ctx, ptr::null_mut());
        (d3d.lock, d3d.unlock) = (None, None);
        if !raw.is_null() { drop(Box::from_raw(raw.cast::<DeviceLock>())); }
    }
}

/// Hooks `AVD3D11VADeviceContext::lock`/`unlock` on `device_context` to `ID3D10Multithread` on
/// `device` and installs `AVHWDeviceContext::free` to release the COM references at teardown.
pub(super) unsafe fn attach_device_lock(device_context: *mut AVHWDeviceContext, device: &ID3D11Device) -> Result<(), String> {
    let mt: ID3D10Multithread = device.cast().map_err(|e| format!("D3D11 multithread protection: {e}"))?;
    // SAFETY: `device_context` is a freshly allocated D3D11VA `AVHWDeviceContext`.
    unsafe {
        let _ = mt.SetMultithreadProtected(true);
        let context1 = device.GetImmediateContext().ok().and_then(|c| c.cast::<ID3D11DeviceContext1>().ok());
        let d3d = &mut *((*device_context).hwctx.cast::<AVD3D11VADeviceContext>());
        d3d.lock_ctx = Box::into_raw(Box::new(DeviceLock { mt, context1 })).cast();
        d3d.lock = Some(d3d11_mt_lock);
        d3d.unlock = Some(d3d11_mt_unlock);
        (*device_context).free = Some(d3d11_device_ctx_free);
    }
    Ok(())
}

/// Copies `region` from `source[slice]` into non-shared static texture `target[0]`, preferring
/// `ID3D11DeviceContext1::CopySubresourceRegion1(..., D3D11_COPY_DISCARD)`.
pub(super) unsafe fn copy_subresource_discard(
    context: &ID3D11DeviceContext,
    context1: Option<&ID3D11DeviceContext1>,
    target: &ID3D11Texture2D,
    source: &ID3D11Texture2D,
    slice: u32,
    region: &D3D11_BOX,
) {
    // SAFETY: Caller guarantees `target` and `source` belong to `context`'s device and `region` fits.
    unsafe {
        let flags = D3D11_COPY_DISCARD.0 as u32;
        if let Some(ctx1) = context1 {
            ctx1.CopySubresourceRegion1(target, 0, 0, 0, 0, source, slice, Some(region), flags);
        } else if let Ok(ctx1) = context.cast::<ID3D11DeviceContext1>() {
            ctx1.CopySubresourceRegion1(target, 0, 0, 0, 0, source, slice, Some(region), flags);
        } else {
            context.CopySubresourceRegion(target, 0, 0, 0, 0, source, slice, Some(region));
        }
    }
}

/// Creates a dedicated `D3D11_CREATE_DEVICE_VIDEO_SUPPORT` device on `renderer_device`'s adapter.
pub(super) fn create_video_device_for(renderer_device: &ID3D11Device) -> Result<(ID3D11Device, ID3D11DeviceContext), String> {
    // SAFETY: `renderer_device` is a live COM device; creates an independent device on its adapter.
    unsafe {
        let dxgi: IDXGIDevice = renderer_device.cast().map_err(|e| format!("querying IDXGIDevice on renderer device: {e}"))?;
        let adapter: IDXGIAdapter = dxgi.GetAdapter().map_err(|e| format!("reading renderer DXGI adapter: {e}"))?;
        let (mut device, mut context) = (None, None);
        let levels = [D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0];
        D3D11CreateDevice(
            &adapter, D3D_DRIVER_TYPE_UNKNOWN, HMODULE::default(), D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
            Some(&levels), D3D11_SDK_VERSION, Some(&mut device), None, Some(&mut context),
        )
        .map_err(|e| format!("D3D11CreateDevice(VIDEO_SUPPORT) on renderer adapter: {e}"))?;
        let device = device.ok_or("D3D11CreateDevice returned no device")?;
        let context = context.ok_or("D3D11CreateDevice returned no immediate context")?;
        if let Ok(mt) = device.cast::<ID3D10Multithread>() { let _ = mt.SetMultithreadProtected(true); }
        Ok((device, context))
    }
}

/// Sets up FFmpeg's D3D11VA hardware device context on a dedicated video device (when possible and
/// not `probe_only`), returning the bridge and whether FFmpeg's decoder array should bind SRVs.
pub(super) unsafe fn open_device_context(
    context: &mut AVCodecContext,
    kind: AVHWDeviceType,
    device: *mut c_void,
    pictures: Pictures,
) -> Result<(Option<Arc<DecodeBridge>>, bool), String> {
    // SAFETY: `device` is null or a live renderer/probe `ID3D11Device`; `context` is freshly allocated.
    unsafe {
        let renderer_device =
            ID3D11Device::from_raw_borrowed(&device).ok_or("No D3D11 device.")?;
        if let Some(reason) = device_removed_reason(renderer_device) {
            return Err(format!("D3D11 renderer device removed ({reason})."));
        }
        let bridge = if pictures.probe_only {
            None
        } else {
            match DecodeBridge::new(renderer_device, pictures) {
                Ok(bridge) => {
                    eprintln!("nightjar: D3D11VA decodes on its own video device (shared textures + monitored fences)");
                    Some(bridge)
                }
                Err(why) => {
                    if let Some(reason) = device_removed_reason(renderer_device) {
                        return Err(format!("D3D11 renderer device removed ({reason})."));
                    }
                    eprintln!("nightjar: D3D11VA could not open a separate video device ({why}); sharing the renderer's D3D11 device with multithread lock");
                    None
                }
            }
        };
        let decode_device = bridge.as_ref().map_or(renderer_device, |b| b.decode_device());
        let reference = av_hwdevice_ctx_alloc(kind);
        if reference.is_null() {
            return Err("Could not allocate a D3D11 device context.".into());
        }
        context.hw_device_ctx = reference;
        let device_context = (*reference).data.cast::<AVHWDeviceContext>();
        let d3d = &mut *((*device_context).hwctx.cast::<AVD3D11VADeviceContext>());
        d3d.device = decode_device.clone().into_raw().cast();
        attach_device_lock(device_context, decode_device)?;
        let bind_textures = bridge.is_none()
            && !pictures.probe_only
            && crate::stream::direct::bind_decoder_textures(decode_device);
        let status = av_hwdevice_ctx_init(reference);
        if status < 0 {
            return Err(super::error(status));
        }
        Ok((bridge, bind_textures))
    }
}

/// Shared-device fallback copy into non-shared static texture `texture` using `D3D11_COPY_DISCARD`.
pub(super) unsafe fn copy_shared_device_frame(
    frame: &AVFrame,
    texture: *mut c_void,
    width: i32,
    height: i32,
) -> bool {
    // SAFETY: Caller guarantees `frame` is a live `AVFrame` and `texture` is a live `ID3D11Texture2D`.
    unsafe {
        if width < 2 || height < 2 || frame.format != AV_PIX_FMT_D3D11 || frame.hw_frames_ctx.is_null() {
            return false;
        }
        let frames = &*((*frame.hw_frames_ctx).data.cast::<AVHWFramesContext>());
        let d3d = &*((*frames.device_ctx).hwctx.cast::<AVD3D11VADeviceContext>());
        let (context_raw, source_raw) = (d3d.device_context.cast(), frame.data[0].cast());
        let Some(context) = ID3D11DeviceContext::from_raw_borrowed(&context_raw) else {
            return false;
        };
        let Some(source) = ID3D11Texture2D::from_raw_borrowed(&source_raw) else {
            return false;
        };
        let Some(target) = ID3D11Texture2D::from_raw_borrowed(&texture) else {
            return false;
        };
        let region = D3D11_BOX {
            left: 0,
            top: 0,
            front: 0,
            right: (width & !1) as u32,
            bottom: (height & !1) as u32,
            back: 1,
        };
        let cached_ctx1 = if d3d
            .lock
            .is_some_and(|lock| std::ptr::fn_addr_eq(lock, d3d11_mt_lock as unsafe extern "C" fn(*mut c_void)))
            && !d3d.lock_ctx.is_null()
        {
            (*d3d.lock_ctx.cast::<DeviceLock>()).context1.as_ref()
        } else { None };
        if let Some(lock) = d3d.lock { lock(d3d.lock_ctx); }
        copy_subresource_discard(context, cached_ctx1, target, source, frame.data[1] as usize as u32, &region);
        if let Some(unlock) = d3d.unlock { unlock(d3d.lock_ctx); }
        true
    }
}

/// Returns the presenter-visible D3D11 surface for a bridged or shared-device frame.
pub(super) unsafe fn surface_of(
    frame: &AVFrame,
    shared: Option<&SharedFrame>,
) -> Option<D3d11Surface> {
    if let Some(shared) = shared {
        return Some(D3d11Surface {
            texture: shared.render_texture_raw(),
            slice: 0,
            readable: true,
        });
    }
    // SAFETY: Caller guarantees `frame` is a live `AVFrame`.
    unsafe {
        if frame.format != AV_PIX_FMT_D3D11 || frame.hw_frames_ctx.is_null() || frame.data[0].is_null() {
            return None;
        }
        let frames = &*((*frame.hw_frames_ctx).data.cast::<AVHWFramesContext>());
        let d3d = &*frames.hwctx.cast::<AVD3D11VAFramesContext>();
        let readable = d3d.BindFlags & D3D11_BIND_SHADER_RESOURCE.0 as u32 != 0;
        Some(D3d11Surface {
            texture: frame.data[0].cast(),
            slice: frame.data[1] as usize as u32,
            readable,
        })
    }
}

/// Runs `f` synchronized with either the cross-device monitored fence bridge or FFmpeg's lock.
pub(super) unsafe fn with_lock(
    frame: &AVFrame,
    shared: Option<&SharedFrame>,
    f: &mut dyn FnMut(),
) {
    if let Some(shared) = shared {
        if let Err(why) = shared.wait_on_renderer() {
            if shared.lease.bridge.check_removed() == D3d11Removal::None
                || shared.lease.bridge.take_removal_log_once()
            {
                eprintln!("nightjar: D3D11 decode bridge wait failed: {why}");
            }
            return;
        }
        f();
        shared.signal_read_on_renderer();
        return;
    }
    // SAFETY: Caller guarantees `frame` is a live `AVFrame`.
    unsafe { with_ffmpeg_lock(frame, f) };
}

/// Runs `f` under FFmpeg's `ID3D10Multithread` device lock only when decoding shares SDL3's
/// renderer device (for `CreateShaderResourceView`, which needs no cross-device fence signal when
/// the texture already lives on the renderer device via `DecodeBridge`).
pub(super) unsafe fn with_shared_device_lock(
    frame: &AVFrame,
    shared: Option<&SharedFrame>,
    f: &mut dyn FnMut(),
) {
    if shared.is_some() {
        return f();
    }
    // SAFETY: Caller guarantees `frame` is a live `AVFrame`.
    unsafe { with_ffmpeg_lock(frame, f) };
}

unsafe fn with_ffmpeg_lock(frame: &AVFrame, f: &mut dyn FnMut()) {
    // SAFETY: Caller guarantees `frame` is a live `AVFrame`.
    unsafe {
        if frame.hw_frames_ctx.is_null() {
            return f();
        }
        let frames = &*((*frame.hw_frames_ctx).data.cast::<AVHWFramesContext>());
        let d3d = &*((*frames.device_ctx).hwctx.cast::<AVD3D11VADeviceContext>());
        match (d3d.lock, d3d.unlock) {
            (Some(lock), Some(unlock)) => {
                lock(d3d.lock_ctx);
                f();
                unlock(d3d.lock_ctx);
            }
            _ => f(),
        }
    }
}

/// Dedicated `D3D11_CREATE_DEVICE_VIDEO_SUPPORT` device for off-thread decoder probes so
/// `color_probe` never touches SDL3's shared renderer immediate context.
pub struct ProbeVideoDevice(ID3D11Device);

impl ProbeVideoDevice {
    pub fn for_renderer(renderer_device: *mut c_void) -> Result<Self, String> {
        // SAFETY: `renderer_device` is null or a live `ID3D11Device` kept alive by the caller.
        let renderer = unsafe { ID3D11Device::from_raw_borrowed(&renderer_device) }
            .ok_or("No D3D11 renderer device.")?;
        let (device, _context) = create_video_device_for(renderer)?;
        Ok(Self(device))
    }

    pub fn as_raw(&self) -> *mut c_void {
        self.0.as_raw()
    }
}

struct SharedSlot {
    decode_tex: ID3D11Texture2D,
    render_tex: ID3D11Texture2D,
    reuse_value: AtomicU64,
    waited_value: AtomicU64,
    in_use: AtomicBool,
}

struct Pool {
    format: DXGI_FORMAT,
    width: u32,
    height: u32,
    slots: Vec<Arc<SharedSlot>>,
}

#[derive(Default)]
struct PoolState {
    pool: Option<Pool>,
}

/// Cross-device bridge between a dedicated `D3D11_CREATE_DEVICE_VIDEO_SUPPORT` decode device and
/// SDL3's Direct3D 11 renderer device using NT-handle shared textures and monitored `ID3D11Fence`s.
pub(crate) struct DecodeBridge {
    id: u64,
    decode_device: ID3D11Device,
    decode_context: ID3D11DeviceContext,
    decode_queue: ID3D11DeviceContext4,
    decode_mt: ID3D10Multithread,
    decode_fence_dec: ID3D11Fence,
    reuse_fence_dec: ID3D11Fence,

    render_device: ID3D11Device,
    render_device1: ID3D11Device1,
    render_context: ID3D11DeviceContext,
    render_context1: Option<ID3D11DeviceContext1>,
    render_queue: ID3D11DeviceContext4,
    decode_fence_ren: ID3D11Fence,
    reuse_fence_ren: ID3D11Fence,

    decode_counter: AtomicU64,
    reuse_counter: AtomicU64,
    signaled_reuse: AtomicU64,
    flushed_reuse: AtomicU64,
    pending_flush: AtomicU64,
    decode_removed: AtomicBool,
    render_removed: AtomicBool,
    removal_logged: AtomicBool,
    render_thread: OnceLock<ThreadId>,

    pool: Mutex<PoolState>,
    max_slots: usize,
    #[cfg(test)]
    drop_flushes: AtomicU64,
}

fn shared_device_forced() -> bool {
    std::env::var(SHARED_DEVICE_ENV)
        .ok()
        .is_some_and(|v| v.trim() == "1")
}

impl DecodeBridge {
    pub(super) fn new(renderer_device: &ID3D11Device, pictures: Pictures) -> Result<Arc<Self>, String> {
        if shared_device_forced() {
            return Err(format!("{SHARED_DEVICE_ENV}=1"));
        }
        let (decode_device, decode_context) = create_video_device_for(renderer_device)?;
        Self::new_with_decode_device(renderer_device, decode_device, decode_context, pictures)
    }

    fn new_with_decode_device(
        renderer_device: &ID3D11Device,
        decode_device: ID3D11Device,
        decode_context: ID3D11DeviceContext,
        pictures: Pictures,
    ) -> Result<Arc<Self>, String> {
        let decode_mt: ID3D10Multithread = decode_device.cast().map_err(|e| format!("decode ID3D10Multithread: {e}"))?;
        let decode_dev5: ID3D11Device5 = decode_device.cast().map_err(|e| format!("decode ID3D11Device5: {e}"))?;
        let decode_queue: ID3D11DeviceContext4 = decode_context.cast().map_err(|e| format!("decode ID3D11DeviceContext4: {e}"))?;
        let render_device1: ID3D11Device1 = renderer_device.cast().map_err(|e| format!("renderer ID3D11Device1: {e}"))?;
        let render_dev5: ID3D11Device5 = renderer_device.cast().map_err(|e| format!("renderer ID3D11Device5: {e}"))?;
        // SAFETY: `renderer_device` is a live COM device.
        let render_context = unsafe { renderer_device.GetImmediateContext() }
            .map_err(|e| format!("renderer immediate context: {e}"))?;
        let render_context1 = render_context.cast::<ID3D11DeviceContext1>().ok();
        let render_queue: ID3D11DeviceContext4 = render_context.cast().map_err(|e| format!("renderer ID3D11DeviceContext4: {e}"))?;
        let (decode_fence_dec, decode_fence_ren) = create_shared_fence_pair(&decode_dev5, &render_dev5, "decode")?;
        let (reuse_fence_dec, reuse_fence_ren) = create_shared_fence_pair(&decode_dev5, &render_dev5, "reuse")?;

        let candidates = preflight_formats(pictures);
        let decoder_supported: Vec<DXGI_FORMAT> = candidates
            .iter()
            .copied()
            .filter(|&fmt| unsafe { decode_device.CheckFormatSupport(fmt) }.is_ok_and(|f| f & D3D11_FORMAT_SUPPORT_DECODER_OUTPUT.0 as u32 != 0))
            .collect();
        if !decoder_supported.is_empty() {
            for format in decoder_supported {
                preflight_format_and_srv(&decode_device, renderer_device, &render_device1, format)
                    .map_err(|why| format!("shared texture pre-flight failed: {why}"))?;
            }
        } else {
            let mut last_error = String::new();
            let mut any_ok = false;
            for &format in candidates {
                match preflight_format_and_srv(&decode_device, renderer_device, &render_device1, format) {
                    Ok(()) => { any_ok = true; break; }
                    Err(why) => last_error = why,
                }
            }
            if !any_ok {
                return Err(format!("shared texture pre-flight failed: {last_error}"));
            }
        }

        let max_slots = (pictures.waiting + 4).clamp(4, 16);
        Ok(Arc::new(Self {
            id: NEXT_BRIDGE_ID.fetch_add(1, Ordering::Relaxed),
            decode_device, decode_context, decode_queue, decode_mt, decode_fence_dec, reuse_fence_dec,
            render_device: renderer_device.clone(), render_device1, render_context, render_context1,
            render_queue, decode_fence_ren, reuse_fence_ren,
            decode_counter: AtomicU64::new(0), reuse_counter: AtomicU64::new(0),
            signaled_reuse: AtomicU64::new(0), flushed_reuse: AtomicU64::new(0), pending_flush: AtomicU64::new(0),
            decode_removed: AtomicBool::new(false), render_removed: AtomicBool::new(false),
            removal_logged: AtomicBool::new(false),
            render_thread: OnceLock::new(), pool: Mutex::new(PoolState::default()), max_slots,
            #[cfg(test)] drop_flushes: AtomicU64::new(0),
        }))
    }

    pub(super) fn decode_device(&self) -> &ID3D11Device { &self.decode_device }

    pub(super) fn mark_renderer_flushed(&self) {
        let current = self.signaled_reuse.load(Ordering::Acquire);
        if current > 0 { self.flushed_reuse.fetch_max(current, Ordering::AcqRel); }
    }

    pub(super) fn is_device_removed(&self) -> bool {
        self.decode_removed.load(Ordering::Acquire) || self.render_removed.load(Ordering::Acquire)
    }

    pub(super) fn check_removed(&self) -> D3d11Removal {
        if !self.render_removed.load(Ordering::Acquire) && device_removed_reason(&self.render_device).is_some() {
            self.render_removed.store(true, Ordering::Release);
        }
        if !self.decode_removed.load(Ordering::Acquire) && device_removed_reason(&self.decode_device).is_some() {
            self.decode_removed.store(true, Ordering::Release);
        }
        if self.render_removed.load(Ordering::Acquire) { D3d11Removal::RendererLost }
        else if self.decode_removed.load(Ordering::Acquire) { D3d11Removal::DecodeOnly }
        else { D3d11Removal::None }
    }

    pub(super) fn take_removal_log_once(&self) -> bool {
        !self.removal_logged.swap(true, Ordering::AcqRel)
    }

    /// Flushes any off-thread `SlotLease::drop` reuse signal deferred to the presenting thread.
    fn flush_pending_on_renderer(&self) {
        let _ = self.render_thread.set(std::thread::current().id());
        let want = self.pending_flush.load(Ordering::Acquire);
        if want > 0 && want > self.flushed_reuse.load(Ordering::Acquire) {
            // SAFETY: Called on the presenting thread that owns `render_context`.
            unsafe {
                let completed = self.reuse_fence_ren.GetCompletedValue();
                if completed == u64::MAX {
                    self.render_removed.store(true, Ordering::Release);
                    return;
                }
                if completed < want { self.render_context.Flush(); }
            }
            self.flushed_reuse.fetch_max(want, Ordering::AcqRel);
        }
    }

    fn device_error(&self, what: &str, detail: impl std::fmt::Display) -> String {
        if let Some(e) = device_removed_reason(&self.decode_device) {
            self.decode_removed.store(true, Ordering::Release);
            return format!("{what}: decode device removed ({e})");
        }
        if let Some(e) = device_removed_reason(&self.render_device) {
            self.render_removed.store(true, Ordering::Release);
            return format!("{what}: renderer device removed ({e})");
        }
        format!("{what}: {detail}")
    }

    fn note_fence_error(&self, decode_side: bool, what: &str, err: &windows::core::Error) -> String {
        if is_device_removed_code(err.code().0) {
            if decode_side { self.decode_removed.store(true, Ordering::Release); }
            else { self.render_removed.store(true, Ordering::Release); }
        }
        self.device_error(what, err)
    }

    fn acquire_slot(&self, state: &mut PoolState, format: DXGI_FORMAT, width: u32, height: u32) -> Result<Arc<SharedSlot>, String> {
        if state.pool.as_ref().is_none_or(|p| p.format != format || p.width != width || p.height != height) {
            state.pool = Some(Pool { format, width, height, slots: Vec::with_capacity(self.max_slots) });
        }
        let pool = state.pool.as_mut().expect("initialized above");
        // SAFETY: Reading the completed value of a live monitored fence is thread-safe.
        let completed = unsafe { self.reuse_fence_dec.GetCompletedValue() };
        if completed == u64::MAX {
            self.decode_removed.store(true, Ordering::Release);
            return Err(self.device_error("shared reuse fence reported device removal", "u64::MAX"));
        }
        let flushed = self.flushed_reuse.load(Ordering::Acquire);
        let snapshot: Vec<_> = pool.slots.iter()
            .map(|s| (s.in_use.load(Ordering::Acquire), s.reuse_value.load(Ordering::Acquire)))
            .collect();
        match choose_reuse_slot(&snapshot, completed, flushed, self.max_slots, HARD_SLOT_CAP) {
            SlotChoice::Reuse(idx) => {
                let slot = pool.slots[idx].clone();
                slot.in_use.store(true, Ordering::Release);
                Ok(slot)
            }
            SlotChoice::Alloc => self.alloc_slot(pool, format, width, height),
            SlotChoice::Exhausted => Err("all shared D3D11 decode slots are held or pending renderer flush".into()),
        }
    }

    fn alloc_slot(&self, pool: &mut Pool, format: DXGI_FORMAT, width: u32, height: u32) -> Result<Arc<SharedSlot>, String> {
        let (decode_tex, render_tex) = create_shared_texture_pair(&self.decode_device, &self.render_device1, format, width, height)?;
        let slot = Arc::new(SharedSlot {
            decode_tex, render_tex, reuse_value: AtomicU64::new(0),
            waited_value: AtomicU64::new(0), in_use: AtomicBool::new(true),
        });
        pool.slots.push(slot.clone());
        Ok(slot)
    }

    /// Copies the decoded slice from `raw_frame` on the decode device into a shared NT-handle
    /// texture, signals the monitored decode fence and flushes the decode queue.
    pub(super) unsafe fn export(self: &Arc<Self>, raw_frame: &AVFrame) -> Result<SharedFrame, String> {
        if self.is_device_removed() {
            return Err(self.device_error("D3D11 decode bridge stopped", "device removed"));
        }
        let source_raw = raw_frame.data[0].cast();
        // SAFETY: `raw_frame` is a live `AV_PIX_FMT_D3D11` frame belonging to this decode device.
        let source = unsafe { ID3D11Texture2D::from_raw_borrowed(&source_raw) }.ok_or("decoded D3D11 frame has no source texture")?;
        let slice = raw_frame.data[1] as usize as u32;
        // Read `GetDesc` directly (cheap user-mode copy) instead of caching by raw pointer,
        // avoiding stale descriptions if FFmpeg recycles freed texture/frames addresses.
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: `GetDesc` is a user-mode read on a live texture.
        unsafe { source.GetDesc(&mut desc) };
        let slot = {
            let mut state = self.pool.lock().unwrap_or_else(|e| e.into_inner());
            self.acquire_slot(&mut state, desc.Format, desc.Width, desc.Height)?
        };
        let lease = Arc::new(SlotLease { bridge: self.clone(), slot });
        let need_reuse = lease.slot.reuse_value.load(Ordering::Acquire);
        let decode_value = self.decode_counter.fetch_add(1, Ordering::AcqRel) + 1;
        let region = if raw_frame.width >= 2 && raw_frame.height >= 2 {
            let (w, h) = (raw_frame.width as u32, raw_frame.height as u32);
            let (cw, ch) = if matches!(desc.Format, DXGI_FORMAT_NV12 | DXGI_FORMAT_P010) {
                (((w + 1) & !1).min(desc.Width), ((h + 1) & !1).min(desc.Height))
            } else { (w.min(desc.Width), h.min(desc.Height)) };
            (cw < desc.Width || ch < desc.Height).then_some(D3D11_BOX { left: 0, top: 0, front: 0, right: cw, bottom: ch, back: 1 })
        } else { None };
        {
            let _lock = MtGuard::lock(&self.decode_mt);
            // SAFETY: Serialized via `ID3D10Multithread`; textures and fences belong to `decode_device`.
            unsafe {
                if need_reuse > 0
                    && !fence_completed_ready(self.reuse_fence_dec.GetCompletedValue(), need_reuse)
                        .map_err(|e| {
                            self.decode_removed.store(true, Ordering::Release);
                            self.device_error("shared reuse fence", e)
                        })?
                {
                    if need_reuse > self.flushed_reuse.load(Ordering::Acquire) {
                        return Err("shared reuse fence signal pending renderer flush".into());
                    }
                    self.decode_queue.Wait(&self.reuse_fence_dec, need_reuse)
                        .map_err(|e| self.note_fence_error(true, "waiting on shared reuse fence", &e))?;
                }
                self.decode_context.CopySubresourceRegion(&lease.slot.decode_tex, 0, 0, 0, 0, source, slice, region.as_ref().map(ptr::from_ref));
                self.decode_queue.Signal(&self.decode_fence_dec, decode_value)
                    .map_err(|e| self.note_fence_error(true, "signaling shared decode fence", &e))?;
                self.decode_context.Flush();
            }
        }
        Ok(SharedFrame { lease, decode_value })
    }
}

struct SlotLease { bridge: Arc<DecodeBridge>, slot: Arc<SharedSlot> }

impl Drop for SlotLease {
    fn drop(&mut self) {
        let val = self.slot.reuse_value.load(Ordering::Acquire);
        if val > 0 && val > self.bridge.flushed_reuse.load(Ordering::Acquire) {
            // SAFETY: `GetCompletedValue` is thread-safe; `Flush` runs only on the presenting thread
            // so `render_device` needs no `SetMultithreadProtected(true)` lock.
            unsafe {
                let completed = self.bridge.reuse_fence_ren.GetCompletedValue();
                if completed == u64::MAX {
                    self.bridge.render_removed.store(true, Ordering::Release);
                } else if completed < val {
                    if self.bridge.render_thread.get() == Some(&std::thread::current().id()) {
                        self.bridge.render_context.Flush();
                        #[cfg(test)]
                        self.bridge.drop_flushes.fetch_add(1, Ordering::Relaxed);
                        let flushed = val.max(self.bridge.signaled_reuse.load(Ordering::Acquire));
                        self.bridge.flushed_reuse.fetch_max(flushed, Ordering::AcqRel);
                    } else {
                        self.bridge.pending_flush.fetch_max(val, Ordering::AcqRel);
                    }
                }
            }
        }
        self.slot.in_use.store(false, Ordering::Release);
    }
}

/// A decoded picture exported into a renderer-visible NT-handle shared texture with monitored fence synchronization.
#[derive(Clone)]
pub(super) struct SharedFrame { lease: Arc<SlotLease>, decode_value: u64 }

impl SharedFrame {
    pub(super) fn bridge_id(&self) -> u64 { self.lease.bridge.id }
    pub(super) fn render_texture_raw(&self) -> *mut c_void { self.lease.slot.render_tex.as_raw() }
    pub(super) fn mark_renderer_flushed(&self) { self.lease.bridge.mark_renderer_flushed(); }

    pub(super) fn wait_on_renderer(&self) -> Result<(), String> {
        let bridge = &self.lease.bridge;
        if bridge.is_device_removed() {
            return Err(bridge.device_error("D3D11 decode bridge stopped", "device removed"));
        }
        bridge.flush_pending_on_renderer();
        if self.lease.slot.waited_value.load(Ordering::Acquire) >= self.decode_value { return Ok(()); }
        // SAFETY: `GetCompletedValue` is thread-safe; `Wait` runs on the presenting thread.
        unsafe {
            if fence_completed_ready(bridge.decode_fence_ren.GetCompletedValue(), self.decode_value)
                .map_err(|e| {
                    bridge.render_removed.store(true, Ordering::Release);
                    bridge.device_error("shared decode fence", e)
                })?
            {
                self.lease.slot.waited_value.fetch_max(self.decode_value, Ordering::AcqRel);
                return Ok(());
            }
            bridge.render_queue.Wait(&bridge.decode_fence_ren, self.decode_value)
                .map_err(|e| bridge.note_fence_error(false, "queuing shared decode fence wait on renderer", &e))?;
        }
        self.lease.slot.waited_value.fetch_max(self.decode_value, Ordering::AcqRel);
        Ok(())
    }

    pub(super) fn signal_read_on_renderer(&self) {
        let bridge = &self.lease.bridge;
        if bridge.is_device_removed() { return; }
        bridge.flush_pending_on_renderer();
        let val = bridge.reuse_counter.fetch_add(1, Ordering::AcqRel) + 1;
        // SAFETY: Runs on the presenting thread's immediate context after submitting work reading `render_tex`.
        match unsafe { bridge.render_queue.Signal(&bridge.reuse_fence_ren, val) } {
            Ok(()) => {
                bridge.signaled_reuse.fetch_max(val, Ordering::AcqRel);
                self.lease.slot.reuse_value.fetch_max(val, Ordering::AcqRel);
            }
            Err(e) => {
                let _ = bridge.note_fence_error(false, "signaling shared reuse fence", &e);
            }
        }
    }

    pub(super) unsafe fn copy_to_renderer_texture(&self, target: *mut c_void, width: i32, height: i32) -> bool {
        if width < 2 || height < 2 || self.wait_on_renderer().is_err() { return false; }
        // SAFETY: Caller guarantees `target` is a live `ID3D11Texture2D` on the renderer device.
        let Some(target) = (unsafe { ID3D11Texture2D::from_raw_borrowed(&target) }) else { return false };
        let region = D3D11_BOX { left: 0, top: 0, front: 0, right: (width & !1) as u32, bottom: (height & !1) as u32, back: 1 };
        let bridge = &self.lease.bridge;
        // SAFETY: Both textures live on `bridge.render_device`, ordered after decode by `wait_on_renderer`.
        unsafe { copy_subresource_discard(&bridge.render_context, bridge.render_context1.as_ref(), target, &self.lease.slot.render_tex, 0, &region); }
        self.signal_read_on_renderer();
        true
    }
}

#[cfg(test)]
mod tests;
