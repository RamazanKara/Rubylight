use crate::text::from_wide;
use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::time::{Duration, Instant};
use windows::{
    Win32::{
        Graphics::{
            Direct3D12::ID3D12Resource,
            Dxgi::{Common::*, *},
        },
        System::Com::*,
    },
    core::{Interface, PCWSTR},
};

mod output;

#[derive(Clone, Debug, Serialize)]
pub struct Display {
    pub device_id: String,
    pub display_name: String,
    pub friendly_name: String,
    pub width: u32,
    pub height: u32,
    pub x: i32,
    pub y: i32,
    pub primary: bool,
    pub adapter: String,
    #[serde(skip)]
    pub adapter_index: u32,
    #[serde(skip)]
    pub output_index: u32,
}
pub fn enable_dpi_awareness() {
    // SAFETY: SetProcessDpiAwarenessContext takes a predefined context constant and no pointers.
    unsafe {
        // A failed call means the embedding process already set its DPI context.
        let _ = windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(
            windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        );
    }
}
pub fn displays() -> Result<Vec<Display>> {
    // SAFETY: The factory, adapters and outputs are live COM objects returned by these calls, and
    // each GetDesc returns an owned struct.
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
        let mut result = vec![];
        for a in 0..32 {
            let Ok(adapter) = factory.EnumAdapters1(a) else {
                break;
            };
            let ad = adapter.GetDesc1()?;
            for o in 0..32 {
                let Ok(output) = adapter.EnumOutputs(o) else {
                    break;
                };
                let d = output.GetDesc()?;
                if !d.AttachedToDesktop.as_bool() {
                    continue;
                }
                let name = from_wide(&d.DeviceName);
                let r = d.DesktopCoordinates;
                result.push(Display {
                    device_id: name.clone(),
                    display_name: name,
                    friendly_name: from_wide(&ad.Description),
                    width: (r.right - r.left) as u32,
                    height: (r.bottom - r.top) as u32,
                    x: r.left,
                    y: r.top,
                    primary: r.left == 0 && r.top == 0,
                    adapter: from_wide(&ad.Description),
                    adapter_index: a,
                    output_index: o,
                });
            }
        }
        Ok(result)
    }
}
fn select_display<'a>(choices: &'a [Display], name: &str) -> Result<Option<&'a Display>> {
    if name.is_empty() {
        return Ok(choices
            .iter()
            .find(|d| d.primary)
            .or_else(|| choices.first()));
    }
    // A selected VDD can disappear during a mode change. Capture must wait
    // for it to return instead of reopening on the user's physical desktop.
    choices
        .iter()
        .find(|d| d.display_name == name || d.device_id == name)
        .map(Some)
        .with_context(|| format!("selected capture display is unavailable: {name}"))
}
/// Whether capture can open this display now. An empty name is the primary
/// display.
pub fn display_present(name: &str) -> bool {
    displays().is_ok_and(|choices| select_display(&choices, name).is_ok_and(|d| d.is_some()))
}
pub struct ComGuard;
#[derive(serde::Serialize)]
pub struct Gpu {
    pub name: String,
    pub vendor: u32,
    pub dedicated_memory: u64,
    pub luid: (u32, i32),
    pub pnp_id: Option<String>,
}
fn hardware_adapter(desc: &DXGI_ADAPTER_DESC1) -> bool {
    // Windows can expose Basic Render Driver as the primary adapter without
    // DXGI_ADAPTER_FLAG_SOFTWARE when the display driver is unavailable.
    desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 == 0
        && (desc.VendorId, desc.DeviceId) != (0x1414, 0x8c)
}
fn adapter_pnp_id(luid: windows::Win32::Foundation::LUID) -> Option<String> {
    use windows::Win32::Devices::Display::*;
    let mut info = DISPLAYCONFIG_ADAPTER_NAME::default();
    info.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_ADAPTER_NAME;
    info.header.size = std::mem::size_of_val(&info) as u32;
    info.header.adapterId = luid;
    // SAFETY: `info.header` names the request type and the full size of `info`, so the call writes
    // only within `info`.
    if unsafe { DisplayConfigGetDeviceInfo(&mut info.header) } != 0 {
        return None;
    }
    let path = from_wide(&info.adapterDevicePath);
    let path = path.strip_prefix(r"\\?\")?;
    let mut parts = path.split('#');
    let (bus, hardware, instance) = (parts.next()?, parts.next()?, parts.next()?);
    if bus.is_empty() || hardware.is_empty() || instance.is_empty() {
        return None;
    }
    Some(format!("{bus}\\{hardware}\\{instance}"))
}
/// The GPU the Graphics card setting names, matched as capture and the
/// encoders match it: by the stored PnP identity when there is one, otherwise
/// by name. None on Automatic.
pub fn select_gpu<'a>(
    gpus: &'a [Gpu],
    adapter_name: &str,
    pnp_id: &str,
) -> Result<Option<&'a Gpu>> {
    if adapter_name.is_empty() && pnp_id.is_empty() {
        return Ok(None);
    }
    let mut matches = gpus.iter().filter(|gpu| {
        if pnp_id.is_empty() {
            gpu.name == adapter_name
        } else {
            gpu.pnp_id
                .as_deref()
                .is_some_and(|id| id.eq_ignore_ascii_case(pnp_id))
        }
    });
    let gpu = matches.next();
    if gpu.is_none() || (!pnp_id.is_empty() && matches.next().is_some()) {
        bail!("configured GPU adapter was not resolved uniquely: {adapter_name} {pnp_id}");
    }
    Ok(gpu)
}
pub fn gpus() -> Result<Vec<Gpu>> {
    // SAFETY: The factory and adapters are live COM objects returned by these calls, and GetDesc1
    // returns an owned struct.
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
        let mut result = Vec::new();
        for index in 0..32 {
            let Ok(adapter) = factory.EnumAdapters1(index) else {
                break;
            };
            let info = adapter.GetDesc1()?;
            if hardware_adapter(&info) {
                result.push(Gpu {
                    name: from_wide(&info.Description),
                    vendor: info.VendorId,
                    dedicated_memory: info.DedicatedVideoMemory as u64,
                    luid: (info.AdapterLuid.LowPart, info.AdapterLuid.HighPart),
                    pnp_id: adapter_pnp_id(info.AdapterLuid),
                });
            }
        }
        Ok(result)
    }
}
impl ComGuard {
    pub fn new() -> Result<Self> {
        // SAFETY: CoInitializeEx takes no pointers here, and only a successful call yields the
        // guard whose Drop balances it.
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
        }
        Ok(Self)
    }
}
impl Drop for ComGuard {
    fn drop(&mut self) {
        // SAFETY: CoUninitialize takes no arguments, and `new` returns a guard only after
        // CoInitializeEx succeeded.
        unsafe {
            CoUninitialize();
        }
    }
}
pub struct Priority {
    handle: windows::Win32::Foundation::HANDLE,
}
impl Default for Priority {
    fn default() -> Self {
        Self::new()
    }
}

impl Priority {
    pub fn new() -> Self {
        let priority = Self::games();
        // SAFETY: SetThreadPriority acts on the current thread's pseudo-handle and takes no
        // pointers.
        unsafe {
            use windows::Win32::System::Threading::*;
            let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_ABOVE_NORMAL);
        }
        priority
    }
    /// For the input thread: the MMCSS boost alone. The thread priority that
    /// new() sets afterwards cancels the boost (14 instead of 18), and input
    /// then waited milliseconds, not microseconds, behind busy time-critical
    /// threads. Without MMCSS the thread is raised as new() raises it.
    pub fn input() -> Self {
        let priority = Self::games();
        if priority.handle.is_invalid() {
            // SAFETY: SetThreadPriority acts on the current thread's pseudo-handle and takes no
            // pointers.
            unsafe {
                use windows::Win32::System::Threading::*;
                let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_ABOVE_NORMAL);
            }
        }
        priority
    }
    /// Register the thread with MMCSS's "Games" task at high priority.
    fn games() -> Self {
        // SAFETY: `name` is a NUL-terminated UTF-16 buffer alive for the call, `index` is a live
        // local, and the handle is checked before use.
        unsafe {
            use windows::Win32::System::Threading::*;
            let name: Vec<u16> = "Games\0".encode_utf16().collect();
            let mut index = 0;
            let handle = windows::Win32::System::Threading::AvSetMmThreadCharacteristicsW(
                PCWSTR(name.as_ptr()),
                &mut index,
            )
            .unwrap_or_default();
            if !handle.is_invalid() {
                let _ = AvSetMmThreadPriority(handle, AVRT_PRIORITY_HIGH);
            }
            Self { handle }
        }
    }
}
impl Drop for Priority {
    fn drop(&mut self) {
        if !self.handle.is_invalid() {
            // SAFETY: `handle` is the valid MMCSS handle `games` registered on this thread (HANDLE
            // keeps `Priority` on it), reverted only here.
            unsafe {
                let _ =
                    windows::Win32::System::Threading::AvRevertMmThreadCharacteristics(self.handle);
            }
        }
    }
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pixel {
    Bgra8,
    RgbaF16,
    Rgba10Pq,
}
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub stride: usize,
    pub bytes: Vec<u8>,
    pub captured: Instant,
    pub pixel: Pixel,
}
#[derive(Clone)]
pub struct Device {
    /// The GPU's D3D12 device with the compute queue that copies and converts its frames.
    pub compute: std::sync::Arc<crate::compute::Compute>,
    /// The monitor the device was opened for; absent on a host without one,
    /// where the device can still encode.
    pub output: Option<IDXGIOutput1>,
    pub display: std::sync::Arc<Display>,
    // Capture and all encoder clones observe one throttled colour snapshot.
    color: Option<std::sync::Arc<std::sync::Mutex<output::Color>>>,
}
impl Device {
    /// Read the selected output's luminance; conversion produces Rec.2020/D65.
    pub fn hdr_metadata(&self) -> butterpollo_core::hdr::Metadata {
        self.color
            .as_ref()
            .map(|color| color.lock().unwrap().metadata(Instant::now()))
            .unwrap_or_default()
    }
    fn color_description(&self) -> Result<Option<output::Description>> {
        self.color
            .as_ref()
            .map_or(Ok(None), |color| color.lock().unwrap().poll(Instant::now()))
    }
    /// Whether `other` runs on the same D3D12 device, so its textures can be read here.
    pub fn same(&self, other: &Device) -> bool {
        self.compute.device.as_raw() == other.compute.device.as_raw()
    }
    /// The adapter's LUID as (low, high).
    pub fn luid(&self) -> (u32, i32) {
        let luid = self.compute.luid();
        (luid.LowPart, luid.HighPart)
    }
    pub fn new(name: &str) -> Result<Self> {
        Self::new_adapter(name, "", "")
    }
    /// A device for work that needs a GPU, not a particular picture: encoder
    /// probes and tools. A display that is gone is replaced by the primary.
    pub fn new_adapter(name: &str, adapter_name: &str, pnp_id: &str) -> Result<Self> {
        let choices = displays()?;
        let display = match select_display(&choices, name) {
            Ok(display) => display,
            Err(error) => {
                tracing::debug!(%error, "using the primary display's GPU");
                select_display(&choices, "")?
            }
        };
        if let Some(display) = display {
            // SAFETY: The factory and adapter are live COM objects, and GetDesc1
            // returns an owned descriptor.
            let desc = unsafe {
                let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
                factory.EnumAdapters1(display.adapter_index)?.GetDesc1()?
            };
            // A software desktop must not hide a hardware encoder from the
            // probe that runs before the stream creates its virtual display.
            if !hardware_adapter(&desc) {
                return Self::without_display(adapter_name, pnp_id);
            }
        }
        Self::with_display(display.cloned(), adapter_name, pnp_id)
    }
    /// Capture's device: the named display and never another one, so that a
    /// stream cannot silently show a different desktop. An empty name is the
    /// primary display.
    pub fn for_display(name: &str, adapter_name: &str, pnp_id: &str) -> Result<Self> {
        let choices = displays()?;
        Self::with_display(
            select_display(&choices, name)?.cloned(),
            adapter_name,
            pnp_id,
        )
    }
    fn with_display(display: Option<Display>, adapter_name: &str, pnp_id: &str) -> Result<Self> {
        // SAFETY: The factory, adapters and outputs are live COM objects returned by these calls,
        // and each GetDesc1 returns an owned struct.
        unsafe {
            let Some(mut display) = display else {
                return Self::without_display(adapter_name, pnp_id);
            };
            let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
            let adapter = factory.EnumAdapters1(display.adapter_index)?;
            let output = Some(adapter.EnumOutputs(display.output_index)?.cast()?);
            let adapter = if adapter_name.is_empty() && pnp_id.is_empty() {
                adapter
            } else {
                let mut matches = Vec::new();
                for index in 0..32 {
                    let Ok(candidate) = factory.EnumAdapters1(index) else {
                        break;
                    };
                    let desc = candidate.GetDesc1()?;
                    if !hardware_adapter(&desc) {
                        continue;
                    }
                    let matched = if !pnp_id.is_empty() {
                        adapter_pnp_id(desc.AdapterLuid)
                            .is_some_and(|id| id.eq_ignore_ascii_case(pnp_id))
                    } else {
                        from_wide(&desc.Description) == adapter_name
                    };
                    if matched {
                        matches.push(candidate);
                        if pnp_id.is_empty() {
                            break;
                        }
                    }
                }
                if matches.len() != 1 {
                    bail!(
                        "configured GPU adapter was not resolved uniquely: {adapter_name} {pnp_id}"
                    );
                }
                matches.remove(0)
            };
            display.adapter = from_wide(&adapter.GetDesc1()?.Description);
            Self::create(&adapter, output, display)
        }
    }
    /// A device on the configured GPU, or the first hardware GPU, when no
    /// monitor is connected: the host can still probe its encoders, as
    /// Vibepollo does on headless hosts.
    fn without_display(adapter_name: &str, pnp_id: &str) -> Result<Self> {
        // SAFETY: The factory and adapters are live COM objects returned by these calls, and
        // GetDesc1 returns an owned struct.
        unsafe {
            let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
            for index in 0..32 {
                let Ok(adapter) = factory.EnumAdapters1(index) else {
                    break;
                };
                let desc = adapter.GetDesc1()?;
                let description = from_wide(&desc.Description);
                if !hardware_adapter(&desc)
                    || (!pnp_id.is_empty()
                        && !adapter_pnp_id(desc.AdapterLuid)
                            .is_some_and(|id| id.eq_ignore_ascii_case(pnp_id)))
                    || (pnp_id.is_empty()
                        && !adapter_name.is_empty()
                        && description != adapter_name)
                {
                    continue;
                }
                let display = Display {
                    device_id: String::new(),
                    display_name: String::new(),
                    friendly_name: String::new(),
                    width: 0,
                    height: 0,
                    x: 0,
                    y: 0,
                    primary: false,
                    adapter: description,
                    adapter_index: index,
                    output_index: 0,
                };
                return Self::create(&adapter, None, display);
            }
            bail!("no desktop display or hardware GPU")
        }
    }
    fn create(
        adapter: &IDXGIAdapter1,
        output: Option<IDXGIOutput1>,
        display: Display,
    ) -> Result<Self> {
        // SAFETY: `adapter` is a live DXGI adapter; GetDesc1 returns an owned struct.
        let luid = unsafe { adapter.GetDesc1()? }.AdapterLuid;
        let compute = crate::compute::Compute::for_adapter(luid)?;
        let color = output
            .as_ref()
            .map(|output| output::Color::new(output, &display.display_name))
            .transpose()?
            .map(|color| std::sync::Arc::new(std::sync::Mutex::new(color)));
        Ok(Self {
            compute,
            output,
            display: std::sync::Arc::new(display),
            color,
        })
    }
}
/// An immutable, owned GPU snapshot. The capture pool never overwrites a
/// texture until the latest-frame slot and every encoder have released it.
#[derive(Clone)]
pub struct GpuImage {
    pub width: u32,
    pub height: u32,
    pub pixel: Pixel,
    /// Capture timestamp used by pacing and encoders. Future, missing or
    /// more than two-second-old stamps fall back to the conversion time.
    pub captured: Instant,
    /// The unclamped present time the display driver stamped, for
    /// diagnostics only. It can be later than host acquisition.
    pub present_stamp: Option<Instant>,
    /// When the host submitted its owned copy, before GPU completion.
    pub acquired: Instant,
    /// When the capture published the picture to the stream threads; until
    /// then, its acquisition. Only timing reports read this.
    pub published: Instant,
    pub gpu: Device,
    /// A D3D12 texture on `gpu`'s device, in the COMMON state between uses.
    pub texture: std::sync::Arc<ID3D12Resource>,
    /// When the GPU write into `texture` (a compute-queue copy, an upload, or the
    /// display driver's copy into a fence ring) completes. Every reader's queue waits for it.
    pub(crate) ready: Option<crate::compute::Ready>,
    /// A display driver frame ring slot read in place rather than copied;
    /// it goes back with the last clone of this image.
    _lease: Option<std::sync::Arc<idd_push::Lent>>,
}
impl GpuImage {
    pub fn readback(&self, staging: &mut Option<crate::compute::Staging>) -> Result<Image> {
        let mut image = read_texture(&self.gpu, &self.texture, self.ready.as_ref(), staging)?;
        image.captured = self.captured;
        Ok(image)
    }
    /// A CPU image on `gpu`, uploaded through its compute queue. The upload has finished
    /// when this returns.
    pub fn upload(gpu: &Device, image: &Image) -> Result<Self> {
        let bytes = if image.pixel == Pixel::RgbaF16 { 8 } else { 4 };
        if image.width == 0
            || image.height == 0
            || image.stride < image.width as usize * bytes
            || image.bytes.len() < image.stride * image.height as usize
        {
            bail!("invalid GPU upload image");
        }
        let (texture, value) = crate::compute::upload(
            &gpu.compute,
            (image.width, image.height),
            crate::compute::format(image.pixel),
            windows::Win32::Graphics::Direct3D12::D3D12_RESOURCE_FLAG_NONE,
            &image.bytes,
            image.stride,
        )?;
        Ok(Self {
            width: image.width,
            height: image.height,
            pixel: image.pixel,
            captured: image.captured,
            present_stamp: None,
            acquired: Instant::now(),
            published: Instant::now(),
            gpu: gpu.clone(),
            texture: std::sync::Arc::new(texture),
            ready: Some(crate::compute::Ready::on(
                &gpu.compute,
                gpu.compute.fence.clone(),
                value,
            )),
            _lease: None,
        })
    }
}

/// Textures the host owns that frames are copied into, on a compute queue.
pub(crate) struct GpuPool {
    textures: Vec<std::sync::Arc<ID3D12Resource>>,
    pub(crate) copier: crate::compute::Copier,
}
const GPU_SNAPSHOT_CAPACITY: usize = 8;

fn snapshot_capacity<T>(textures: &[std::sync::Arc<T>]) -> bool {
    textures.len() < GPU_SNAPSHOT_CAPACITY
        || textures
            .iter()
            .any(|texture| std::sync::Arc::strong_count(texture) == 1)
}

impl GpuPool {
    pub(crate) fn new(compute: std::sync::Arc<crate::compute::Compute>) -> Result<Self> {
        Ok(Self {
            textures: Vec::new(),
            copier: crate::compute::Copier::new(compute)?,
        })
    }
    fn has_capacity(&self) -> bool {
        snapshot_capacity(&self.textures)
    }

    /// A copy of `source` (on `gpu`'s device) once `after` is reached, or None when every
    /// pool texture is still held.
    pub(crate) fn copy(
        &mut self,
        gpu: &Device,
        source: &ID3D12Resource,
        after: Option<&crate::compute::Ready>,
    ) -> Result<Option<GpuImage>> {
        use windows::Win32::Graphics::Direct3D12::{
            D3D12_HEAP_FLAG_NONE, D3D12_RESOURCE_FLAG_NONE,
        };
        // SAFETY: `source` is a live resource; GetDesc only reads it.
        let desc = unsafe { source.GetDesc() };
        let pixel = crate::compute::pixel(desc.Format)?;
        let width = u32::try_from(desc.Width)?;
        let copier = &mut self.copier;
        self.textures.retain(|texture| {
            // SAFETY: The pool's textures are live resources; GetDesc only reads them.
            let old = unsafe { texture.GetDesc() };
            let keep =
                old.Width == desc.Width && old.Height == desc.Height && old.Format == desc.Format;
            if !keep {
                copier.forget(texture);
            }
            keep
        });
        let free = self
            .textures
            .iter()
            .find(|texture| std::sync::Arc::strong_count(texture) == 1)
            .cloned();
        let texture = if let Some(texture) = free {
            texture
        } else {
            // Capture retains only the newest native update while consumers
            // own every snapshot, instead of growing a frame queue.
            if self.textures.len() >= GPU_SNAPSHOT_CAPACITY {
                return Ok(None);
            }
            let texture = std::sync::Arc::new(gpu.compute.texture(
                width,
                desc.Height,
                desc.Format,
                D3D12_RESOURCE_FLAG_NONE,
                D3D12_HEAP_FLAG_NONE,
            )?);
            self.textures.push(texture.clone());
            texture
        };
        let ready = self.copier.copy(&texture, source, after).map_err(|error| {
            match crate::device_loss::DeviceLost::d3d12(&gpu.compute.device) {
                Some(loss) => error.context(loss),
                None => error,
            }
        })?;
        Ok(Some(GpuImage {
            width,
            height: desc.Height,
            pixel,
            captured: Instant::now(),
            present_stamp: None,
            acquired: Instant::now(),
            published: Instant::now(),
            gpu: gpu.clone(),
            texture,
            ready: Some(ready),
            _lease: None,
        }))
    }
}
/// `texture`'s pixels once `ready` is reached, read on `gpu`'s compute queue.
pub fn read_texture(
    gpu: &Device,
    texture: &ID3D12Resource,
    ready: Option<&crate::compute::Ready>,
    staging: &mut Option<crate::compute::Staging>,
) -> Result<Image> {
    // SAFETY: `texture` is a live resource; GetDesc only reads it.
    let desc = unsafe { texture.GetDesc() };
    let pixel = crate::compute::pixel(desc.Format)?;
    let bytes = crate::compute::read_planes(&gpu.compute, texture, ready, staging)?
        .into_iter()
        .next()
        .context("a texture without planes")?;
    Ok(Image {
        width: u32::try_from(desc.Width)?,
        height: desc.Height,
        stride: bytes.len() / desc.Height as usize,
        bytes,
        captured: Instant::now(),
        pixel,
    })
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
fn qpc_timestamps(ticks: i64) -> (Instant, Option<Instant>) {
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
fn qpc_timestamps_at(
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
mod idd_push;

/// A stream's capture: the frames Rubylight's display driver pushes for its
/// virtual display.
pub enum Capture {
    IddPush(Box<idd_push::Session>),
    /// Holds no device: the place of a lost capture while a new one is made.
    Closed,
}

impl Capture {
    pub fn backend(&self) -> &'static str {
        match self {
            Self::IddPush(_) => "idd-push",
            Self::Closed => "closed",
        }
    }
    /// Set when the driver publishes a frame. A wait for the next poll that
    /// also ends on it sees the frame at once instead of up to a poll
    /// interval later; the polling itself stays.
    pub fn frame_signal(&self) -> Option<&crate::timing::Signal> {
        match self {
            Self::IddPush(session) => Some(session.frame_signal()),
            Self::Closed => None,
        }
    }
    /// The capture of `name`, a virtual display this host created.
    pub fn open(name: &str, hdr: bool, config: &butterpollo_core::config::Config) -> Result<Self> {
        Ok(Self::IddPush(Box::new(idd_push::Session::new(
            name, hdr, config,
        )?)))
    }
    pub fn next_frame(&mut self) -> Result<Option<Image>> {
        match self {
            Self::IddPush(s) => s.next_frame(),
            Self::Closed => bail!("capture is closed"),
        }
    }
    pub fn next_gpu(&mut self) -> Result<Option<GpuImage>> {
        let result = match self {
            Self::IddPush(s) => s.next_gpu(),
            Self::Closed => bail!("capture is closed"),
        };
        result.map_err(|error| match self.device_removed() {
            Some(loss) => error.context(loss),
            None => error,
        })
    }
    pub fn device_removed(&self) -> Option<crate::device_loss::DeviceLost> {
        match self {
            Self::IddPush(s) => crate::device_loss::DeviceLost::d3d12(&s.gpu.compute.device),
            Self::Closed => None,
        }
    }
}

#[cfg(test)]
mod tests;
