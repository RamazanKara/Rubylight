//! Windows Graphics Capture of a display or a window: frame pool, session, QPC timestamps and size and colour checks.

use super::*;

pub(super) const WGC_FRAME_POOL_CAPACITY: usize = 2;

/// Select only within the current pool's capacity so a busy producer cannot
/// keep capture draining forever. Each acquired frame is validated, including
/// ones that would be skipped, so resize/access errors still trigger recovery.
pub(super) fn newest_capture_frame<T>(
    limit: usize,
    mut next: impl FnMut() -> Result<Option<T>>,
    mut validate: impl FnMut(&T) -> Result<()>,
    mut close: impl FnMut(T) -> Result<()>,
) -> Result<Option<T>> {
    let mut newest = None;
    for _ in 0..limit {
        let candidate = match next() {
            Ok(Some(frame)) => frame,
            Ok(None) => break,
            Err(error) => {
                if let Some(frame) = newest {
                    let _ = close(frame);
                }
                return Err(error);
            }
        };
        if let Err(error) = validate(&candidate) {
            let _ = close(candidate);
            if let Some(frame) = newest {
                let _ = close(frame);
            }
            return Err(error);
        }
        if let Some(previous) = newest.replace(candidate)
            && let Err(error) = close(previous)
        {
            if let Some(frame) = newest.take() {
                let _ = close(frame);
            }
            return Err(error);
        }
    }
    Ok(newest)
}

pub struct Wgc {
    pub(super) gpu: Device,
    pool: Direct3D11CaptureFramePool,
    pub(super) session: GraphicsCaptureSession,
    pub(super) notifications: Option<(std::sync::Arc<crate::timing::Signal>, i64)>,
    staging: Option<ID3D11Texture2D>,
    pub(super) owned: GpuPool,
    pub(super) grid: Option<std::sync::Arc<std::sync::Mutex<ClaimGrid>>>,
    intervals: butterpollo_core::capture_policy::Intervals,
    origin: Instant,
    last_publish: Instant,
    pub(super) held: Option<(Direct3D11CaptureFrame, Instant, Instant, Option<Instant>)>,
    size: (i32, i32),
    color_space: Option<DXGI_COLOR_SPACE_TYPE>,
    color_check: Instant,
    pub(super) drain_to_newest: bool,
    /// The captured display or window. When a display leaves the desktop,
    /// even for a moment (a layout change switching it off and on), or a
    /// window closes, its capture item closes and WGC delivers no more
    /// frames, without an error.
    item: GraphicsCaptureItem,
    source: Source,
    format: DirectXPixelFormat,
    high_rate: bool,
    /// A window's new size restarts the capture at it. The helper
    /// cannot: its shared textures keep the first size, so it fails and the
    /// host reopens it at the new size.
    pub(super) follow_resize: bool,
    closed: std::sync::Arc<std::sync::atomic::AtomicBool>,
    closed_token: Option<i64>,
    /// Whether the compositions since the last published picture changed
    /// the foreground window (the frame pool's closures borrow `self`).
    focus: std::sync::Mutex<focus::Focus>,
}
/// Handles kept as numbers so the capture can move threads.
#[derive(Clone, Copy, Debug)]
enum Source {
    /// A display's HMONITOR.
    Monitor(usize),
    /// A game window's HWND.
    Window(usize),
}
impl Source {
    fn gone(self) -> &'static str {
        match self {
            Self::Monitor(_) => "the captured display left the desktop",
            Self::Window(_) => "the game window closed",
        }
    }
}
pub(crate) fn qpc_frequency() -> i64 {
    *std::sync::OnceLock::get_or_init(&QPC_FREQUENCY, || {
        let mut frequency = 0;
        // SAFETY: QueryPerformanceFrequency writes only to the live local `frequency`.
        let _ = unsafe {
            windows::Win32::System::Performance::QueryPerformanceFrequency(&mut frequency)
        };
        frequency.max(1)
    })
}
static QPC_FREQUENCY: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
pub(super) fn qpc_timestamps(ticks: i64) -> (Instant, Option<Instant>) {
    let now = Instant::now();
    let mut current = 0;
    if ticks <= 0
        // SAFETY: QueryPerformanceCounter writes only to the live local `current`.
        || unsafe { windows::Win32::System::Performance::QueryPerformanceCounter(&mut current) }
            .is_err()
    {
        return (now, None);
    }
    qpc_timestamps_at(ticks, current, qpc_frequency(), now)
}
pub(super) fn qpc_timestamps_at(
    ticks: i64,
    current: i64,
    frequency: i64,
    now: Instant,
) -> (Instant, Option<Instant>) {
    if ticks <= 0 || current <= 0 || frequency <= 0 {
        return (now, None);
    }
    let delta = Duration::from_secs_f64(ticks.abs_diff(current) as f64 / frequency as f64);
    let stamp = if ticks > current {
        now.checked_add(delta)
    } else {
        now.checked_sub(delta)
    };
    // Keep the historical clock for pacing; diagnostics retain the raw stamp.
    let captured = if ticks > current || delta > Duration::from_secs(2) {
        now
    } else {
        stamp.unwrap_or(now)
    };
    (captured, stamp)
}
pub(super) fn wgc_qpc(ticks: i64, frequency: i64) -> i64 {
    (i128::from(ticks) * i128::from(frequency) / 10_000_000).clamp(0, i128::from(i64::MAX)) as i64
}
fn wgc_presentation(frame: &Direct3D11CaptureFrame) -> (Instant, Option<Instant>) {
    let ticks = frame.SystemRelativeTime().map(|t| t.Duration).unwrap_or(0);
    qpc_timestamps(wgc_qpc(ticks, qpc_frequency()))
}
fn pin_capture_runtime() -> Result<()> {
    use std::sync::OnceLock;
    use windows::Win32::System::LibraryLoader::{
        GET_MODULE_HANDLE_EX_FLAG_PIN, GetModuleHandleExW,
    };
    static PINNED: OnceLock<windows::core::Result<()>> = OnceLock::new();
    // Closing a free-threaded pool does not join all of GraphicsCapture's workers.
    // COM teardown can unload the DLL while one is still returning through it:
    // https://github.com/robmikh/Win32CaptureSample/issues/99
    // Keep only the system runtime loaded for the process lifetime; sessions,
    // frame pools and their GPU resources still close and release normally.
    PINNED
        // SAFETY: The module name is a static NUL-terminated literal, and `module` is a live local.
        .get_or_init(|| unsafe {
            let mut module = HMODULE::default();
            GetModuleHandleExW(
                GET_MODULE_HANDLE_EX_FLAG_PIN,
                windows::core::w!("GraphicsCapture.dll"),
                &mut module,
            )
        })
        .clone()
        .context("keep the Windows capture runtime loaded during asynchronous shutdown")
}
/// A frame pool of `size` and its session on `item`, configured but not started.
fn open_session(
    gpu: &Device,
    item: &GraphicsCaptureItem,
    format: DirectXPixelFormat,
    size: windows::Graphics::SizeInt32,
    window: bool,
    high_rate: bool,
) -> Result<(Direct3D11CaptureFramePool, GraphicsCaptureSession)> {
    use butterpollo_core::os_gate::{Feature, missing};
    let build = crate::display::windows_build();
    let dxgi: IDXGIDevice = gpu.device.cast()?;
    // SAFETY: `dxgi` is the live device the capture runs on.
    let winrt: IDirect3DDevice = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi)? }.cast()?;
    let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
        &winrt,
        format,
        WGC_FRAME_POOL_CAPACITY as i32,
        size,
    )
    .context("Windows Graphics Capture frame pool")?;
    let session = pool
        .CreateCaptureSession(item)
        .context("Windows Graphics Capture session")?;
    // Each switch below needs a Windows build (see `os_gate`); older
    // builds keep their fixed behaviour and the stream says so. A
    // game window's stream takes no mouse, so the PC's pointer is not
    // drawn into it.
    if let Some(reason) = missing(build, Feature::WgcCursorSwitch) {
        tracing::info!("{reason}");
    } else {
        session
            .SetIsCursorCaptureEnabled(!window)
            .context("Windows Graphics Capture cursor capture")?;
    }
    if let Some(reason) = missing(build, Feature::WgcBorderSwitch) {
        tracing::info!("{reason}");
    } else if let Err(error) = session.SetIsBorderRequired(false) {
        // Windows asks the user for borderless access or refuses it
        // for some sessions; the capture itself still works.
        tracing::warn!(%error, "WGC border could not be removed; Windows draws a yellow border around the captured display or window");
    }
    // Frames keep their whole picture and only report what changed, which
    // tells another window's updates from the focus window's (`focus`).
    // ReportOnly is the default; set where it exists, as a guard.
    if !window
        && let Err(error) = session.SetDirtyRegionMode(GraphicsCaptureDirtyRegionMode::ReportOnly)
    {
        tracing::debug!(%error, "WGC dirty region mode unavailable");
    }
    // Leaving this property untouched is not equivalent to zero:
    // Windows build 26200 reports a 16 ms default, which can deliver
    // only 50-57 updates/sec when quantized to compositor ticks.
    // Explicitly remove that throttle at every stream rate. The old
    // 1 ms workaround remains available for diagnostic comparisons.
    let interval = windows::Foundation::TimeSpan {
        Duration: if high_rate { 10_000 } else { 0 },
    };
    // Always tried, whatever the build: a Windows servicing update can
    // bring the property to an older build. Only the log wording
    // depends on the build.
    match session.SetMinUpdateInterval(interval) {
        Ok(()) => tracing::info!(
            interval_us = interval.Duration / 10,
            "WGC minimum update interval configured"
        ),
        Err(error) => match missing(build, Feature::WgcUpdateInterval) {
            Some(reason) => tracing::info!(%error, "{reason}"),
            None => {
                tracing::warn!(%error, "WGC minimum update interval unavailable; using the Windows default, which may limit capture fps. Update Windows or disable high-rate capture")
            }
        },
    }
    Ok((pool, session))
}

impl Wgc {
    pub fn new(name: &str) -> Result<Self> {
        Self::new_format(name, false)
    }
    pub fn new_format(name: &str, hdr: bool) -> Result<Self> {
        Self::new_device(Device::new(name)?, hdr, false, None)
    }
    /// Captures `window` (an HWND) instead of the display when given; the
    /// display still chooses the GPU and colour space.
    pub(super) fn new_device(
        gpu: Device,
        hdr: bool,
        high_rate: bool,
        window: Option<usize>,
    ) -> Result<Self> {
        // SAFETY: `output` and `gpu.device` are live, `d.Monitor` is that output's monitor, and
        // every call returns owned COM or WinRT objects.
        unsafe {
            let output = gpu.output()?.clone();
            let d = output.GetDesc()?;
            let color_space = gpu.color_description()?.map(|desc| desc.color_space);
            let native_hdr = color_space
                .map(|space| space == DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020)
                .unwrap_or(hdr);
            // Say which Windows build lacks WGC or display capture, and what to
            // use instead, before the generic COM error would hide it.
            use butterpollo_core::os_gate::{Feature, missing};
            let build = crate::display::windows_build();
            if window.is_some() {
                window::require_capture(build)?;
            } else {
                for feature in [Feature::Wgc, Feature::WgcMonitor] {
                    if let Some(reason) = missing(build, feature) {
                        bail!("{reason} Select Desktop Duplication in the capture settings.");
                    }
                }
            }
            let interop: IGraphicsCaptureItemInterop =
                windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()
                    .context("Windows Graphics Capture is unavailable")?;
            pin_capture_runtime()?;
            let (item, source): (GraphicsCaptureItem, _) = match window {
                Some(hwnd) => (
                    window::check(hwnd).and_then(|()| {
                        interop
                            .CreateForWindow(windows::Win32::Foundation::HWND(hwnd as *mut _))
                            .with_context(|| {
                                format!(
                                    "Windows Graphics Capture cannot capture the game window {hwnd:#x}"
                                )
                            })
                    })?,
                    Source::Window(hwnd),
                ),
                None => (
                    interop
                        .CreateForMonitor(d.Monitor)
                        .context("Windows Graphics Capture cannot capture this display")?,
                    Source::Monitor(d.Monitor.0 as usize),
                ),
            };
            let closed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let flag = closed.clone();
            let closed_token = item
                .Closed(&windows::Foundation::TypedEventHandler::new(move |_, _| {
                    flag.store(true, std::sync::atomic::Ordering::Release);
                    Ok(())
                }))
                .inspect_err(|error| tracing::warn!(%error, "WGC cannot report the display or window leaving; a capture that stops is found by its monitor or window check"))
                .ok();
            let size = item.Size()?;
            let format = if native_hdr {
                DirectXPixelFormat::R16G16B16A16Float
            } else {
                DirectXPixelFormat::B8G8R8A8UIntNormalized
            };
            let (pool, session) =
                open_session(&gpu, &item, format, size, window.is_some(), high_rate)?;
            let capture = Self {
                gpu,
                pool,
                session,
                notifications: None,
                staging: None,
                owned: GpuPool::default(),
                grid: None,
                intervals: Default::default(),
                origin: Instant::now(),
                last_publish: Instant::now(),
                held: None,
                size: (size.Width, size.Height),
                color_space,
                color_check: Instant::now() + Duration::from_secs(1),
                drain_to_newest: false,
                item,
                source,
                format,
                high_rate,
                follow_resize: true,
                closed,
                closed_token,
                focus: std::sync::Mutex::new(focus::Focus::new(match source {
                    Source::Monitor(_) => {
                        let r = d.DesktopCoordinates;
                        Some([r.left, r.top, r.right, r.bottom])
                    }
                    Source::Window(_) => None,
                })),
            };
            // Own the pool/session before starting so failure closes them
            // just like normal capture teardown.
            capture
                .session
                .StartCapture()
                .context("Windows Graphics Capture did not start")?;
            Ok(capture)
        }
    }
    pub(super) fn enable_notifications(&mut self) -> Result<()> {
        if self.notifications.is_none() {
            self.notify(std::sync::Arc::new(crate::timing::Signal::new()?))?;
        }
        Ok(())
    }
    fn notify(&mut self, arrived: std::sync::Arc<crate::timing::Signal>) -> Result<()> {
        let wake = arrived.clone();
        // The callback never takes the D3D11 lock or touches capture frames.
        let token = self
            .pool
            .FrameArrived(&windows::Foundation::TypedEventHandler::new(move |_, _| {
                let _ = wake.set();
                Ok(())
            }))?;
        self.notifications = Some((arrived.clone(), token));
        // Registration may follow the first frame; make the caller check
        // the pool before waiting for a subsequent notification.
        arrived.set()?;
        Ok(())
    }
    pub fn next_frame(&mut self) -> Result<Option<Image>> {
        self.check_color_space()?;
        let Some(frame) = self.next_native_frame()? else {
            return Ok(None);
        };
        let result = (|| -> Result<Image> {
            let surface = frame.Surface()?;
            let access: IDirect3DDxgiInterfaceAccess = surface.cast()?;
            // SAFETY: `access` is the surface of `frame`, which is closed only after this closure.
            let texture: ID3D11Texture2D = unsafe { access.GetInterface()? };
            read_texture(&self.gpu, &texture, &mut self.staging)
        })();
        // A CPU image carries no focus label.
        self.take_focus();
        frame.Close()?;
        result.map(Some)
    }
    pub fn next_gpu(&mut self) -> Result<Option<GpuImage>> {
        self.check_color_space()?;
        let now = Instant::now();
        if let Some(frame) = self.next_native_frame()? {
            let composition = self.intervals.observe(nanos(now, self.origin));
            if let Some((previous, _, _, _)) = self.held.take()
                && let Err(error) = previous.Close()
            {
                let _ = frame.Close();
                return Err(error.into());
            }
            let deadline = self.grid.as_ref().and_then(|grid| {
                let grid = grid.lock().unwrap();
                butterpollo_core::capture_policy::publication_deadline(
                    grid.period.as_nanos().min(i64::MAX as u128) as i64,
                    nanos(now, grid.anchor),
                    composition,
                    nanos(self.last_publish, grid.anchor),
                )
                .and_then(|deadline| {
                    if deadline >= 0 {
                        grid.anchor
                            .checked_add(Duration::from_nanos(deadline as u64))
                    } else {
                        grid.anchor
                            .checked_sub(Duration::from_nanos(deadline.unsigned_abs()))
                    }
                })
            });
            let (presented, stamp) = wgc_presentation(&frame);
            self.held = Some((frame, deadline.unwrap_or(now), presented, stamp));
        }
        if self
            .held
            .as_ref()
            .is_none_or(|(_, deadline, _, _)| *deadline > now)
        {
            return Ok(None);
        }
        let Some((frame, _, captured, stamp)) =
            acquire_snapshot_frame(self.owned.has_capacity(), || Ok(self.held.take()))?
        else {
            return Ok(None);
        };
        let result = (|| {
            let surface = frame.Surface()?;
            let access: IDirect3DDxgiInterfaceAccess = surface.cast()?;
            // SAFETY: `access` is the surface of `frame`, which is closed only after this closure.
            let texture: ID3D11Texture2D = unsafe { access.GetInterface()? };
            let mut image = self.owned.copy(&self.gpu, &texture)?;
            if let Some(image) = image.as_mut() {
                image.captured = captured;
                image.wgc_stamp = stamp;
                image.focus = self.take_focus();
            }
            Ok(image)
        })();
        frame.Close()?;
        self.last_publish = now;
        result
    }
    pub(super) fn next_native_frame(&mut self) -> Result<Option<Direct3D11CaptureFrame>> {
        if self.closed.load(std::sync::atomic::Ordering::Acquire) {
            bail!("{}", self.source.gone());
        }
        let newest = newest_capture_frame(
            if self.drain_to_newest {
                WGC_FRAME_POOL_CAPACITY
            } else {
                1
            },
            || {
                let frame = self.try_frame()?;
                if let Some(frame) = &frame {
                    self.focus.lock().unwrap().observe(frame);
                }
                Ok(frame)
            },
            |frame| self.check_size(frame),
            |frame| frame.Close().map_err(Into::into),
        )?;
        if newest.is_none() {
            self.focus.lock().unwrap().refresh(Instant::now());
        }
        match (self.source, newest) {
            (Source::Window(_), Some(frame)) => self.window_frame(frame),
            (_, newest) => Ok(newest),
        }
    }
    /// A window's size changes with the game; the newest frame tells.
    fn window_frame(
        &mut self,
        frame: Direct3D11CaptureFrame,
    ) -> Result<Option<Direct3D11CaptureFrame>> {
        let sizes = (|| -> Result<_> {
            let content = frame.ContentSize()?;
            let surface = frame.Surface()?.Description()?;
            Ok((
                (content.Width, content.Height),
                (surface.Width, surface.Height),
            ))
        })();
        let action = match sizes {
            Ok((content, surface)) => window::frame(self.size, content, surface),
            Err(error) => {
                let _ = frame.Close();
                return Err(error);
            }
        };
        if action == window::Frame::Use {
            return Ok(Some(frame));
        }
        frame.Close()?;
        if let window::Frame::Resize(size) = action {
            ensure!(
                self.follow_resize,
                "the game window was resized from {}x{} to {}x{}; reopening its capture",
                self.size.0,
                self.size.1,
                size.0,
                size.1
            );
            self.restart_at(size)?;
        }
        Ok(None)
    }
    /// Restarts the capture at the window's new size. Recreating the frame
    /// pool in place is not enough: WGC sends a window's next frame only when
    /// the window changes again, so a game that stopped drawing (paused,
    /// unfocused) would stream its old-size picture until then. A started
    /// session always sends a first frame.
    fn restart_at(&mut self, size: (i32, i32)) -> Result<()> {
        // Its frame came from the old buffers and shows the old size.
        if let Some((frame, _, _, _)) = self.held.take() {
            let _ = frame.Close();
        }
        let (pool, session) = open_session(
            &self.gpu,
            &self.item,
            self.format,
            windows::Graphics::SizeInt32 {
                Width: size.0,
                Height: size.1,
            },
            true,
            self.high_rate,
        )
        .context("Windows Graphics Capture could not follow the game window's new size")?;
        // Revoked before the old pool closes, as in `close`.
        let notifications = self.notifications.take();
        if let Some((_, token)) = &notifications {
            let _ = self.pool.RemoveFrameArrived(*token);
        }
        let _ = self.session.Close();
        let _ = self.pool.Close();
        self.pool = pool;
        self.session = session;
        if let Some((arrived, _)) = notifications {
            self.notify(arrived)?;
        }
        self.session
            .StartCapture()
            .context("Windows Graphics Capture did not restart at the game window's new size")?;
        tracing::info!(
            from = ?self.size,
            to = ?size,
            "game window resized; WGC capture restarted at the new size"
        );
        self.size = size;
        Ok(())
    }
    /// Whether the compositions since the last call changed the foreground
    /// window (`Freshness::observe_focus`).
    pub(super) fn take_focus(&self) -> Option<bool> {
        self.focus.lock().unwrap().take()
    }
    pub(super) fn try_frame(&self) -> Result<Option<Direct3D11CaptureFrame>> {
        // Reset before checking the pool: a notification racing the check stays
        // set, and a frame queued before the reset is found by TryGetNextFrame.
        // Resetting after an empty result would lose an arriving frame's wakeup.
        if let Some((arrived, _)) = &self.notifications {
            arrived.reset()?;
        }
        // An empty pool returns a successful HRESULT and a null interface. The
        // generated binding requires a non-null frame, so preserve the HRESULT
        // and optional output separately instead of swallowing every error.
        // SAFETY: `pool` is live, the call matches the TryGetNextFrame vtable slot, and a non-null
        // `frame` is a reference it transferred to us.
        unsafe {
            let mut frame = std::ptr::null_mut();
            (self.pool.vtable().TryGetNextFrame)(self.pool.as_raw(), &mut frame)
                .ok()
                .context("Windows capture frame pool failed; reconnect required")?;
            Ok(if frame.is_null() {
                None
            } else {
                // A successful call transfers this frame reference to us.
                Some(Direct3D11CaptureFrame::from_raw(frame))
            })
        }
    }
    pub(super) fn check_size(&self, frame: &Direct3D11CaptureFrame) -> Result<()> {
        if let Source::Window(_) = self.source {
            // Followed by `window_frame` instead.
            return Ok(());
        }
        let size = frame.ContentSize()?;
        if (size.Width, size.Height) != self.size {
            bail!("capture output dimensions changed");
        }
        Ok(())
    }
    pub(super) fn check_color_space(&mut self) -> Result<()> {
        if Instant::now() < self.color_check {
            return Ok(());
        }
        self.color_check = Instant::now() + Duration::from_secs(1);
        if let Some(loss) = crate::device_loss::DeviceLost::d3d11(&self.gpu.device) {
            return Err(loss.into());
        }
        match self.source {
            Source::Monitor(monitor) => {
                // A display that returned has a new monitor handle; the old one's
                // capture stays silent, so reopen on the display that is there now.
                let mut info = windows::Win32::Graphics::Gdi::MONITORINFO {
                    cbSize: size_of::<windows::Win32::Graphics::Gdi::MONITORINFO>() as u32,
                    ..Default::default()
                };
                // SAFETY: `info` is a live local whose size is set; a stale handle only fails the
                // call.
                if !unsafe {
                    windows::Win32::Graphics::Gdi::GetMonitorInfoW(
                        windows::Win32::Graphics::Gdi::HMONITOR(monitor as *mut _),
                        &mut info,
                    )
                }
                .as_bool()
                {
                    bail!("{}", self.source.gone());
                }
            }
            // A minimised window is not composed: no frames, no error.
            Source::Window(hwnd) => window::check(hwnd)?,
        }
        let current = self.gpu.color_description()?;
        if output::mode_changed(self.color_space, current) {
            bail!("capture output color space changed");
        }
        Ok(())
    }
    pub(super) fn close(&mut self) {
        // Windows can fail-fast instead of returning RO_E_CLOSED when an event
        // is revoked after pool.Close(). Always revoke first and only once.
        // An in-flight callback owns its own Arc to the event until it returns.
        if let Some((_, token)) = self.notifications.take() {
            let _ = self.pool.RemoveFrameArrived(token);
        }
        if let Some(token) = self.closed_token.take() {
            let _ = self.item.RemoveClosed(token);
        }
        if let Some((frame, _, _, _)) = self.held.take() {
            let _ = frame.Close();
        }
        let _ = self.session.Close();
        let _ = self.pool.Close();
    }
}

impl Drop for Wgc {
    fn drop(&mut self) {
        self.close();
    }
}
