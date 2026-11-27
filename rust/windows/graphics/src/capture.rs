//! Windows screen and window capture backends (WGC, WGC user-session helper,
//! Desktop Duplication, Rubylight virtual display driver push, and AMD Direct Capture),
//! plus GPU/display enumeration and bounded snapshot pooling.

use crate::text::from_wide;
use anyhow::{Context, Result, bail, ensure};
use butterpollo_media_policy::capture_policy::notification_wait_interval;
use butterpollo_media_policy::encoder_gpu;
use std::time::{Duration, Instant};
#[cfg(test)]
use windows::Win32::System::Com::CoFreeUnusedLibrariesEx;
use windows::{
    Graphics::{
        Capture::{
            Direct3D11CaptureFrame, Direct3D11CaptureFramePool, GraphicsCaptureDirtyRegionMode,
            GraphicsCaptureItem, GraphicsCaptureSession,
        },
        DirectX::{Direct3D11::IDirect3DDevice, DirectXPixelFormat},
    },
    Win32::{
        Foundation::HMODULE,
        Graphics::{
            Direct3D11::*,
            Dxgi::{Common::*, *},
        },
        System::WinRT::{
            Direct3D11::{CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess},
            Graphics::Capture::IGraphicsCaptureItemInterop,
        },
    },
    core::{Interface, PCWSTR},
};

mod adapters;
mod amd_direct;
mod app_window;
mod bridge;
mod device;
mod duplication;
mod env;
mod focus;
pub mod idd_push;
mod luid_resolve;
mod output;
mod pool;
mod wgc;
mod window;

pub use adapters::{
    ComGuard, Display, Gpu, Priority, automatic_switch, check_window, display_present, displays,
    enable_dpi_awareness, gpu_adapters, gpus, panel_driver, primary_display, select_gpu,
    stream_adapter, stream_gpu, stream_gpu_vendor,
};
use adapters::{
    adapter_pnp_id, hardware_adapter, headless_candidate, preference_orders, probe_skips_display,
    require_encoding_duplication, select_display,
};
pub(crate) use adapters::{display_rect, window_client, window_cursor};
#[cfg(test)]
use adapters::{headless_gpu, next_switch, orders_by_luid, pinned_choice};
pub use amd_direct::Presented;
pub(crate) use app_window::Crop;
#[cfg(test)]
pub(crate) use app_window::client_box;
pub use app_window::{
    app_window_target, app_windows, foreground_window, target_rect as window_target_rect,
};
pub use bridge::START_TIMEOUT as WGC_START_TIMEOUT;
pub use bridge::run_worker_with as run_wgc_worker_with;
pub use device::Device;
use device::{GpuMismatch, duplication_device};
pub use duplication::Duplication;
#[cfg(test)]
use duplication::duplication_upright;
pub use env::CaptureEnv;
pub use luid_resolve::{missing_gpu_text, render_gpu};
pub(crate) use output::{current_refresh_hz, published_refresh_hz};
pub(crate) use pool::GpuPool;
#[cfg(test)]
use pool::{GPU_SNAPSHOT_CAPACITY, snapshot_capacity};
pub use pool::{GpuImage, Image, Pixel, PointerShape, read_texture};
use pool::{acquire_snapshot_frame, snapshot_publication_deadline};
pub use wgc::*;
pub use window::WindowTarget;

pub struct ClaimGrid {
    pub anchor: Instant,
    pub period: Duration,
}
fn nanos(at: Instant, origin: Instant) -> i64 {
    if at >= origin {
        at.duration_since(origin).as_nanos().min(i64::MAX as u128) as i64
    } else {
        -(origin.duration_since(at).as_nanos().min(i64::MAX as u128) as i64)
    }
}

/// Whether WGC can capture the input desktop without crossing into Winlogon.
pub fn wgc_desktop_available() -> bool {
    bridge::desktop_available()
}

pub enum Capture {
    Wgc(Box<Wgc>),
    WgcWorker(Box<bridge::Session>),
    Dxgi(Box<Duplication>),
    /// Frames pushed by Rubylight's own virtual display driver.
    IddPush(Box<idd_push::Session>),
    /// AMD Direct Capture: the Radeon driver's presented surface, which
    /// AMD Fluid Motion Frames' generated frames reach.
    AmdDirect(Box<amd_direct::Session>),
    /// Holds no device: the place of a lost capture while a new one is made.
    Closed,
}
/// The desktop pointer a lost capture last saw, for the capture replacing it.
pub struct Pointer(crate::cursor::Carried);

/// Whether an in-process WGC capture of display `current` can change to a target on `next`
/// (a window there when `window`) on its own device ([`Capture::switch_on_device`]): the same
/// display, outside the WGC helper, and a target that opens with WGC (a window always does; the
/// display does unless `kind` names another backend or the display driver pushes its frames).
fn switches_in_place(
    current: &str,
    next: &str,
    window: bool,
    kind: &str,
    helper: bool,
    push: bool,
) -> bool {
    let same_display = !current.is_empty() && current.eq_ignore_ascii_case(next);
    let opens_with_wgc = window || !(push || matches!(kind, "ddx" | "dxgi" | "amd"));
    same_display && opens_with_wgc && !helper
}

fn open_stream_capture<T>(
    kind: &str,
    warnings: &dyn butterpollo_media_policy::WarningSink,
    mut open: impl FnMut(&str) -> Result<T>,
) -> Result<T> {
    match open(kind) {
        Ok(capture) => {
            if matches!(kind, "wgc" | "ddx" | "dxgi" | "amd") {
                warnings.clear("capture_backend");
            }
            Ok(capture)
        }
        Err(error) if crate::device_loss::DeviceLost::from_error(&error).is_some() => Err(error),
        Err(error) if kind == "amd" => {
            // A display on another GPU, an old driver or a driver without
            // the capture component: keep the stream on WGC and say so.
            let capture = open_stream_capture("wgc", warnings, open).with_context(|| {
                format!("AMD Direct Capture failed ({error:#}); the WGC fallback also failed")
            })?;
            tracing::warn!(error = %format!("{error:#}"), "AMD Direct Capture unavailable; capturing with WGC");
            let error = format!("{error:#}");
            let advice = amd_direct::fallback_advice(&error);
            warnings.set("capture_backend", &format!("Capturing with Windows Graphics Capture: AMD Direct Capture could not start ({error}). {advice}"));
            Ok(capture)
        }
        Err(error) if kind == "wgc" => {
            // WGC can be unavailable under SYSTEM or on a secure desktop.
            // Apply the same fallback at startup and after a capture restart.
            let capture = open("ddx").with_context(|| {
                format!("WGC failed ({error:#}); Desktop Duplication fallback also failed")
            })?;
            warnings.set("capture_backend", &format!("Capturing with Desktop Duplication: WGC unavailable ({error:#}). Capture timing and HDR may differ. Unlock the desktop or dismiss the UAC prompt; if this persists, check the WGC helper and graphics driver."));
            Ok(capture)
        }
        Err(error) => Err(error),
    }
}

impl Capture {
    /// The pointer to hand to the capture that replaces this one.
    pub fn pointer(&self) -> Option<Pointer> {
        match self {
            Self::Dxgi(duplication) => Some(Pointer(duplication.cursor.carry())),
            _ => None,
        }
    }
    /// Draw `pointer` until Desktop Duplication reports the pointer itself,
    /// which it does for a shape only when the shape changes.
    pub fn resume_pointer(&mut self, pointer: Pointer) -> Result<()> {
        if let Self::Dxgi(duplication) = self {
            let Duplication { gpu, cursor, .. } = &mut **duplication;
            cursor.resume(gpu, pointer.0)?;
        }
        Ok(())
    }
    /// AMD Direct Capture's last present (index and flip time) and the
    /// presents it skipped; `None` for other captures.
    pub fn amd_presented(&self) -> Option<(Option<Presented>, u64)> {
        match self {
            Self::AmdDirect(session) => Some((session.last, session.skipped)),
            _ => None,
        }
    }
    pub fn backend(&self) -> &'static str {
        match self {
            Self::Wgc(_) | Self::WgcWorker(_) => "wgc",
            Self::Dxgi(_) => "ddx",
            Self::IddPush(_) => "idd-push",
            Self::AmdDirect(_) => "amd",
            Self::Closed => "closed",
        }
    }
    pub fn set_claim_grid(
        &mut self,
        grid: std::sync::Arc<std::sync::Mutex<ClaimGrid>>,
        enabled: bool,
    ) {
        if let Self::Wgc(wgc) = self {
            wgc.grid = enabled.then_some(grid);
        } else if let Self::WgcWorker(worker) = self {
            worker.set_claim_grid(enabled.then_some(grid));
        }
    }
    pub fn publication_deadline(&self) -> Option<Instant> {
        match self {
            Self::Wgc(wgc) => snapshot_publication_deadline(
                wgc.owned.has_capacity(),
                wgc.held.as_ref().map(|(_, deadline, _, _)| *deadline),
            ),
            Self::WgcWorker(worker) => worker.publication_deadline(),
            _ => None,
        }
    }
    /// Set when the capture helper or display driver announces a frame. It
    /// interrupts both short active polls and the longer static-source wait.
    pub fn frame_signal(&self) -> Option<&crate::timing::Signal> {
        match self {
            Self::WgcWorker(worker) => Some(worker.frame_signal()),
            Self::IddPush(session) => Some(session.frame_signal()),
            _ => None,
        }
    }
    /// Avoid empty polls on a static source, but never sleep past a pending GPU
    /// handoff: a frame announcement can precede keyed-mutex readiness.
    pub fn wait_interval(&self, idle: Duration, poll: Duration) -> Duration {
        let pending = match self {
            Self::WgcWorker(worker) => worker.pending_frame(),
            Self::IddPush(session) => session.pending_frame(),
            _ => return poll,
        };
        notification_wait_interval(idle, pending, poll)
    }
    /// Opt in to local WGC notifications for capture probes. Normal local WGC
    /// keeps polling; the helper uses notifications with short active polls
    /// because pure event waits measured slower detection under GPU load.
    pub fn enable_frame_notifications(&mut self) -> Result<()> {
        match self {
            Self::Wgc(wgc) => wgc.enable_notifications(),
            _ => bail!("frame notifications require Windows Graphics Capture"),
        }
    }
    /// Wait on an enabled WGC notification or the probe's deadline.
    pub fn wait_until(&self, timer: &crate::timing::Timer, deadline: Instant) -> Result<()> {
        if let Self::Wgc(wgc) = self
            && let Some((arrived, _)) = &wgc.notifications
        {
            timer.until_or_signal(deadline, arrived)?;
        } else {
            timer.until(deadline);
        }
        Ok(())
    }
    /// Keep a stream usable when WGC cannot open. Explicit capture probes use
    /// `new_options` instead, so a WGC benchmark cannot silently measure DDX.
    pub fn open_for_stream_reported(
        name: &str,
        kind: &str,
        hdr: bool,
        config: &butterpollo_config::config::Config,
        warnings: std::sync::Arc<dyn butterpollo_media_policy::WarningSink>,
    ) -> Result<Self> {
        Self::open_for_stream_with_env(name, kind, hdr, config, warnings, &CaptureEnv::default())
    }
    /// [`Self::open_for_stream_reported`] with host-supplied VDD and user-session launcher bridges.
    pub fn open_for_stream_with_env(
        name: &str,
        kind: &str,
        hdr: bool,
        config: &butterpollo_config::config::Config,
        warnings: std::sync::Arc<dyn butterpollo_media_policy::WarningSink>,
        env: &CaptureEnv,
    ) -> Result<Self> {
        warnings.clear("capture_compute");
        warnings.clear("capture_helper_compute");
        warnings.clear("capture_adapter");
        warnings.clear("capture_idd_push");
        // A game window opens with WGC or fails: no frame push, no Desktop
        // Duplication of the display it is not on.
        if WindowTarget::parse(name)?.is_some() {
            return Self::new_options_with_env(name, kind, hdr, config, warnings, env);
        }
        // Rubylight's display driver pushes its frames itself; without it, or when it
        // fails, the stream keeps the usual WGC/DDX path.
        if let Some(result) = env::try_idd_push(name, kind, hdr, config, &warnings, env) {
            return result;
        }
        let kind = if kind == "idd-push" { "wgc" } else { kind };
        open_stream_capture(kind, &*warnings, |backend| {
            Self::new_options_with_env(name, backend, hdr, config, warnings.clone(), env)
        })
    }
    pub fn switch_on_device(
        &mut self,
        name: &str,
        kind: &str,
        hdr: bool,
        config: &butterpollo_config::config::Config,
        warnings: std::sync::Arc<dyn butterpollo_media_policy::WarningSink>,
    ) -> Option<Result<Self>> {
        self.switch_on_device_with_vdd(name, kind, hdr, config, warnings, None)
    }
    pub fn switch_on_device_with_vdd(
        &mut self,
        name: &str,
        kind: &str,
        hdr: bool,
        config: &butterpollo_config::config::Config,
        warnings: std::sync::Arc<dyn butterpollo_media_policy::WarningSink>,
        vdd: Option<&dyn idd_push::VddBridge>,
    ) -> Option<Result<Self>> {
        env::switch_on_device_with_vdd(self, name, kind, hdr, config, warnings, vdd)
    }
    pub fn new(name: &str, kind: &str) -> Result<Self> {
        Self::new_format(name, kind, false)
    }
    pub fn new_format(name: &str, kind: &str, hdr: bool) -> Result<Self> {
        Self::new_options(name, kind, hdr, &Default::default())
    }
    pub fn new_options(
        name: &str,
        kind: &str,
        hdr: bool,
        config: &butterpollo_config::config::Config,
    ) -> Result<Self> {
        Self::new_options_with_env(
            name,
            kind,
            hdr,
            config,
            butterpollo_media_policy::recorded_warnings(),
            &CaptureEnv::default(),
        )
    }
    fn new_options_with_env(
        name: &str,
        kind: &str,
        hdr: bool,
        config: &butterpollo_config::config::Config,
        warnings: std::sync::Arc<dyn butterpollo_media_policy::WarningSink>,
        env: &CaptureEnv,
    ) -> Result<Self> {
        let target = WindowTarget::parse(name)?;
        if let Some(target) = &target {
            window::refuse_backend(kind)?;
            window::require_capture(crate::process_info::windows_build())?;
            // Refused before a helper starts; the capture checks again.
            window::check(target.handle())?;
        }
        let window = target.as_ref().map(WindowTarget::handle);
        let client = target.as_ref().is_some_and(|target| target.client);
        let name = target
            .as_ref()
            .map_or(name, |target| target.display.as_str());
        // A window target is WGC whatever else the setting would allow.
        let kind = if window.is_some() { "wgc" } else { kind };
        if kind == "idd-push" {
            return Ok(Self::IddPush(Box::new(idd_push::Session::new(
                name,
                hdr,
                config,
                warnings,
                env.vdd.as_deref(),
            )?)));
        }
        if kind == "amd" {
            // The driver's own surface carries the desktop's format; `hdr`
            // follows the display, as Desktop Duplication's does.
            let mut session = amd_direct::Session::new(name)?;
            if let Err(error) = crate::gpu_priority::configure(&session.gpu, config) {
                tracing::debug!(%error, "GPU priority remains at the process default");
            }
            session.owned.warnings = warnings.clone();
            if crate::compute::enabled(config) {
                // Keep pooled textures NT-handle shared even if AMF's source
                // cannot be opened by D3D12 `Handoff`, so downstream AMF and
                // PyroWave conversion still runs on D3D12 compute.
                session.owned.shared = true;
                match crate::compute::Compute::for_device(&session.gpu.device)
                    .and_then(|compute| crate::compute::Handoff::new(compute, &session.gpu))
                {
                    Ok(compute) => session.owned.compute = Some(compute),
                    Err(error) => {
                        warnings.set("capture_compute", &format!("AMD Direct Capture compute copies unavailable ({error:#}); copying on the graphics queue. Lower game GPU load or update the graphics driver if capture stutters."));
                    }
                }
            }
            return Ok(Self::AmdDirect(Box::new(session)));
        }
        if kind == "wgc"
            && (crate::process_info::is_system() || config.boolean("wgc_user_helper", false))
        {
            // The helper parses the target without its #client suffix, so its
            // crop comes from the forwarded config. Make the target's suffix
            // authoritative there too, as it is in-process.
            let mut helper_config = config.clone();
            helper_config.values.insert(
                butterpollo_media_policy::app_window::FRAME.into(),
                if client { "client" } else { "full" }.into(),
            );
            return Ok(Self::WgcWorker(Box::new(bridge::Session::new(
                name,
                window,
                hdr,
                &helper_config,
                warnings,
                env.user_launcher.as_deref(),
            )?)));
        }
        // WGC captures a display on any GPU, so it opens on the encoding one;
        // Desktop Duplication needs the GPU that drives the display and is
        // refused when that is neither AMD nor NVIDIA.
        let (adapter_name, pnp_id) = if matches!(kind, "ddx" | "dxgi") {
            (
                config.get("adapter_name", "").to_string(),
                config.get("adapter_pnp_id", "").to_string(),
            )
        } else {
            stream_adapter(
                Some(name),
                config.get("adapter_name", ""),
                config.get("adapter_pnp_id", ""),
            )
        };
        let gpu = Device::for_display(name, &adapter_name, &pnp_id)?;
        if let Err(error) = crate::gpu_priority::configure(&gpu, config) {
            tracing::debug!(%error, "GPU priority remains at the process default");
        }
        // Desktop Duplication copies each frame out of the shared desktop
        // surface; on a compute queue that copy keeps pace beside a game.
        let duplication = |gpu: Device| -> Result<Duplication> {
            let gpu = duplication_device(
                gpu,
                GpuMismatch::of,
                || {
                    let owner = Device::for_display(name, "", "")?;
                    if let Err(error) = crate::gpu_priority::configure(&owner, config) {
                        tracing::debug!(%error, "GPU priority remains at the process default");
                    }
                    Ok(owner)
                },
                &warnings,
            )?;
            // A display on another vendor's GPU captures there and could only
            // be encoded there.
            if !encoder_gpu::encodes(gpu.vendor) {
                require_encoding_duplication(&gpu.display.adapter, gpu.vendor, &gpus()?)?;
            }
            let mut duplication = Duplication::new_device(gpu, hdr)?;
            duplication.owned.warnings = warnings.clone();
            if crate::compute::enabled(config) {
                match crate::compute::Compute::for_device(&duplication.gpu.device)
                    .and_then(|compute| crate::compute::Handoff::new(compute, &duplication.gpu))
                {
                    Ok(compute) => duplication.owned.compute = Some(compute),
                    Err(error) => {
                        warnings.set("capture_compute", &format!("Compute capture copies unavailable ({error:#}); copying on the graphics queue. Lower game GPU load or update the graphics driver if capture stutters."))
                    }
                }
            }
            Ok(duplication)
        };
        let wgc = |gpu: Device| -> Result<Wgc> {
            let mut capture = Wgc::new_device(
                gpu,
                hdr,
                config.boolean("wgc_high_rate_capture", false),
                window,
                window_cursor(config),
                client,
            )?;
            capture.owned.warnings = warnings.clone();
            capture.drain_to_newest = config.boolean("wgc_drain_to_newest", false);
            // The pool's textures can be shared with D3D12 on supported AMD
            // and NVIDIA drivers. The same fenced handoff used by DDX keeps the
            // copy and the AMF or NVENC conversion off a busy graphics queue. Keep an independent
            // switch for comparisons and drivers that need the D3D11 path.
            if config.boolean("wgc_compute_copy", true) && crate::compute::enabled(config) {
                match crate::compute::Compute::for_device(&capture.gpu.device)
                    .and_then(|compute| crate::compute::Handoff::new(compute, &capture.gpu))
                {
                    Ok(compute) => capture.owned.compute = Some(compute),
                    Err(error) => {
                        warnings.set("capture_compute", &format!("WGC compute copies unavailable ({error:#}); copying on the graphics queue. Lower game GPU load or update the graphics driver if capture stutters."))
                    }
                }
            }
            Ok(capture)
        };
        match kind {
            "wgc" => Ok(Self::Wgc(Box::new(wgc(gpu)?))),
            "ddx" | "dxgi" => Ok(Self::Dxgi(Box::new(duplication(gpu)?))),
            _ => open_stream_capture("wgc", &warnings, |backend| {
                if backend == "wgc" {
                    wgc(gpu.clone()).map(|capture| Self::Wgc(Box::new(capture)))
                } else {
                    duplication(gpu.clone()).map(|d| Self::Dxgi(Box::new(d)))
                }
            }),
        }
    }
    pub fn next_frame(&mut self) -> Result<Option<Image>> {
        match self {
            Self::Wgc(w) => w.next_frame(),
            Self::WgcWorker(w) => w.next_frame(),
            Self::Dxgi(d) => d.next(Duration::from_millis(1)),
            Self::IddPush(s) => s.next_frame(),
            Self::AmdDirect(s) => s.next_frame(),
            Self::Closed => bail!("capture is closed"),
        }
    }
    /// The newest picture, wrapped for the stream threads. `origin` is the
    /// publication identity the caller stamps into it when the capture
    /// publishes repeats (`capture = wgcc`), which a repeat keeps; 0 when the
    /// capture does not repeat. The picture's `published` is the moment the
    /// capture handed it over, and the pool's textures stay alive through the
    /// returned reference for as long as an encoder reads them.
    pub fn next_gpu(&mut self, origin: u64) -> Result<Option<std::sync::Arc<GpuImage>>> {
        let result = match self {
            Self::Wgc(w) => w.next_gpu(origin),
            Self::WgcWorker(w) => w.next_gpu(origin),
            Self::Dxgi(d) => d.next_gpu(origin),
            Self::IddPush(s) => s.next_gpu(origin),
            Self::AmdDirect(s) => s.next_gpu(origin),
            Self::Closed => bail!("capture is closed"),
        };
        result.map_err(|error| match self.device_removed() {
            Some(loss) => error.context(loss),
            None => error,
        })
    }
    pub fn device_removed(&self) -> Option<crate::device_loss::DeviceLost> {
        let gpu = match self {
            Self::Wgc(w) => &w.gpu,
            Self::WgcWorker(w) => &w.gpu,
            Self::Dxgi(d) => &d.gpu,
            Self::IddPush(s) => &s.gpu,
            Self::AmdDirect(s) => &s.gpu,
            Self::Closed => return None,
        };
        crate::device_loss::DeviceLost::d3d11(&gpu.device)
    }
}

#[cfg(test)]
mod tests;
