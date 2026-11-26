//! Display and GPU adapter enumeration, hybrid-GPU pinning, COM guard,
//! and MMCSS thread priority helpers.

use super::{WindowTarget, window};
use crate::text::from_wide;
use anyhow::{Context, Result, bail};
use butterpollo_media_policy::encoder_gpu;
use serde::Serialize;
use windows::{
    Win32::{Graphics::Dxgi::*, System::Com::*},
    core::PCWSTR,
};

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
pub(super) fn select_display<'a>(
    choices: &'a [Display],
    name: &str,
) -> Result<Option<&'a Display>> {
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
/// display. A game window target always counts as present: a closed window,
/// a missing display or a malformed target is the capture's to report, never
/// a reason to show the primary display in the window's place.
pub fn display_present(name: &str) -> bool {
    name.starts_with(window::PREFIX)
        || displays().is_ok_and(|choices| select_display(&choices, name).is_ok_and(|d| d.is_some()))
}
/// Refuses a window target whose window closed or is minimised (no frames
/// would come), that another process than the one named owns (a reused
/// handle), or that shows on another display than the one named (or on none
/// of that name), so a stream of it fails with the reason instead of
/// freezing, showing another window or using the wrong GPU and colour space.
pub fn check_window(target: &WindowTarget) -> Result<()> {
    window::check(target.handle())?;
    if let Some(process) = target.process {
        window::owned_by(target.handle(), window::owner_of(target.handle()), process)?;
    }
    let shown_on = window::display_of(target.handle())?;
    window::on_display(target.handle(), &shown_on, &target.display)
}
/// Whether a window's capture draws the pointer: an app's window streamed in
/// place of a display (`capture_window = window`) takes the device's mouse.
/// A co-op player's window does not; its stream's settings never ask for it.
pub(crate) fn window_cursor(config: &butterpollo_config::config::Config) -> bool {
    butterpollo_media_policy::app_window::settings(config).is_some()
}
/// Whether a window's capture keeps only its client area
/// (`capture_window_frame = client`), without the title bar and borders.
pub(crate) fn window_client(config: &butterpollo_config::config::Config) -> bool {
    butterpollo_media_policy::app_window::settings(config).is_some_and(|settings| {
        settings.frame == butterpollo_media_policy::app_window::Frame::Client
    })
}

/// The desktop rectangle of `name` as [left, top, right, bottom], the primary
/// display for an empty name; `None` when it is not on the desktop.
pub(crate) fn display_rect(name: &str) -> Option<[i32; 4]> {
    let choices = displays().ok()?;
    let display = select_display(&choices, name).ok()??;
    Some([
        display.x,
        display.y,
        display.x.saturating_add(display.width as i32),
        display.y.saturating_add(display.height as i32),
    ])
}
/// The primary display's name, which capture opens when the stream's display
/// did not come back.
pub fn primary_display() -> Option<String> {
    let choices = displays().ok()?;
    select_display(&choices, "")
        .ok()
        .flatten()
        .map(|d| d.display_name.clone())
}
pub struct ComGuard;
#[derive(serde::Serialize)]
pub struct Gpu {
    pub name: String,
    pub vendor: u32,
    pub dedicated_memory: u64,
    /// The PCI device ID (not in the console's GPU list).
    #[serde(skip)]
    pub device: u32,
    pub luid: (u32, i32),
    pub pnp_id: Option<String>,
    /// Where Windows lists it when asked for the highest-performance GPU and
    /// for the lowest-power one ([`preference_orders`]); not in the console's
    /// GPU list.
    #[serde(skip)]
    pub preference: Option<(u32, u32)>,
}
impl Gpu {
    fn ranked(&self) -> encoder_gpu::Ranked {
        encoder_gpu::Ranked {
            vendor: self.vendor,
            dedicated_memory: self.dedicated_memory,
            preference: self.preference,
        }
    }
}
/// An adapter LUID with its place in Windows' high-performance and
/// minimum-power orders.
pub(super) type Ranking = ((u32, i32), (u32, u32));
/// Where Windows lists each adapter, by LUID, when asked for the
/// highest-performance GPU and when asked for the lowest-power one: external
/// GPUs, then discrete, then integrated, and the reverse, as
/// `IDXGIFactory6::EnumAdapterByGpuPreference` documents. Empty where Windows
/// has no such order (before Windows 10 1803). Listing creates no device.
pub(super) fn preference_orders() -> Vec<Ranking> {
    // SAFETY: The factory and adapters are live COM objects returned by these calls, and
    // GetDesc1 returns an owned struct.
    unsafe {
        let Ok(factory) = CreateDXGIFactory1::<IDXGIFactory6>() else {
            return Vec::new();
        };
        let list = |preference| {
            let mut luids = Vec::new();
            for index in 0..32 {
                let Ok(adapter) =
                    factory.EnumAdapterByGpuPreference::<IDXGIAdapter1>(index, preference)
                else {
                    break;
                };
                if let Ok(desc) = adapter.GetDesc1() {
                    luids.push((desc.AdapterLuid.LowPart, desc.AdapterLuid.HighPart));
                }
            }
            luids
        };
        let fast = list(DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE);
        let frugal = list(DXGI_GPU_PREFERENCE_MINIMUM_POWER);
        orders_by_luid(&fast, &frugal)
    }
}
/// Each LUID of both lists with its position in each.
pub(super) fn orders_by_luid(fast: &[(u32, i32)], frugal: &[(u32, i32)]) -> Vec<Ranking> {
    fast.iter()
        .enumerate()
        .filter_map(|(position, luid)| {
            let other = frugal.iter().position(|l| l == luid)?;
            Some((*luid, (position as u32, other as u32)))
        })
        .collect()
}
fn preference_of(orders: &[Ranking], luid: (u32, i32)) -> Option<(u32, u32)> {
    orders
        .iter()
        .find(|(l, _)| *l == luid)
        .map(|(_, order)| *order)
}
pub(super) fn hardware_adapter(desc: &DXGI_ADAPTER_DESC1) -> bool {
    // Windows can expose Basic Render Driver as the primary adapter without
    // DXGI_ADAPTER_FLAG_SOFTWARE when the display driver is unavailable.
    desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 == 0
        && (desc.VendorId, desc.DeviceId) != (0x1414, 0x8c)
}
/// Which of `candidates` (hardware adapters, in DXGI order) the probe opens
/// when no display gives one: [`encoder_gpu::headless`] over their vendors,
/// dedicated video memory and Windows' preference `orders`, as the stream pins
/// with no GPU chosen. Its encoders give the codecs `/serverinfo` reports.
pub(super) fn headless_candidate(candidates: &[DXGI_ADAPTER_DESC1], orders: &[Ranking]) -> usize {
    let adapters: Vec<encoder_gpu::Ranked> = candidates
        .iter()
        .map(|desc| encoder_gpu::Ranked {
            vendor: desc.VendorId,
            dedicated_memory: desc.DedicatedVideoMemory as u64,
            preference: preference_of(
                orders,
                (desc.AdapterLuid.LowPart, desc.AdapterLuid.HighPart),
            ),
        })
        .collect();
    encoder_gpu::headless(&adapters)
}
pub(super) fn adapter_pnp_id(luid: windows::Win32::Foundation::LUID) -> Option<String> {
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
                    device: info.DeviceId,
                    luid: (info.AdapterLuid.LowPart, info.AdapterLuid.HighPart),
                    pnp_id: adapter_pnp_id(info.AdapterLuid),
                    preference: None,
                });
            }
        }
        // Only a PC with several GPUs needs Windows' ranking of them.
        if result.len() > 1 {
            let orders = preference_orders();
            for gpu in &mut result {
                gpu.preference = preference_of(&orders, gpu.luid);
            }
        }
        Ok(result)
    }
}
/// The hardware GPUs as the encoder GPU check reads them.
pub fn gpu_adapters(gpus: &[Gpu]) -> Vec<encoder_gpu::Adapter> {
    gpus.iter()
        .map(|gpu| encoder_gpu::Adapter {
            name: gpu.name.clone(),
            vendor: gpu.vendor,
        })
        .collect()
}
/// The GPU choice a stream that encodes on the capture device opens it with.
/// A chosen GPU always stays. With none chosen on a PC with several GPUs it is
/// the one [`encoder_gpu::pinned`] ranks first (discrete over integrated, then
/// the most video memory), unless the display is already on an AMD or NVIDIA
/// GPU (`display_vendor`), so that WGC captures and AMF or NVENC encodes there
/// whichever GPU drives the monitor, as the probe already assumes.
pub(super) fn pinned_choice(
    gpus: &[Gpu],
    display_vendor: Option<u32>,
    adapter_name: &str,
    pnp_id: &str,
) -> (String, String) {
    if adapter_name.is_empty() && pnp_id.is_empty() {
        let adapters: Vec<encoder_gpu::Ranked> = gpus.iter().map(Gpu::ranked).collect();
        if let Some(gpu) = encoder_gpu::pinned(&adapters, display_vendor).map(|index| &gpus[index])
        {
            log_automatic_choice(gpu, gpus.len());
            return (gpu.name.clone(), gpu.pnp_id.clone().unwrap_or_default());
        }
    }
    (adapter_name.to_string(), pnp_id.to_string())
}
/// Logs the GPU Automatic pins a stream to, once per change: on a PC with
/// several GPUs the console log then says which one streams and why.
fn log_automatic_choice(gpu: &Gpu, count: usize) {
    static LAST: std::sync::Mutex<Option<(u32, i32)>> = std::sync::Mutex::new(None);
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    if last.replace(gpu.luid) != Some(gpu.luid) {
        tracing::info!(
            gpu = %gpu.name,
            gpus = count,
            windows_rank = ?gpu.preference,
            "Graphics card is Automatic: streaming on the AMD or NVIDIA GPU Windows ranks first for performance (then the one with the most video memory); choose another under Graphics card to override"
        );
    }
}
/// `pinned_choice` for this PC's GPUs and the GPU of `display` (None for a
/// display not created yet, such as a virtual one); the choice as given when
/// they cannot be listed.
pub fn stream_adapter(display: Option<&str>, adapter_name: &str, pnp_id: &str) -> (String, String) {
    match gpus() {
        Ok(gpus) => {
            let vendor = display.and_then(display_vendor);
            pinned_choice(&gpus, vendor, adapter_name, pnp_id)
        }
        Err(error) => {
            tracing::debug!(%error, "the GPUs could not be listed; the GPU choice stays as configured");
            (adapter_name.to_string(), pnp_id.to_string())
        }
    }
}
/// The GPU a stream of `display` captures and encodes on, by name and LUID:
/// the one the Graphics card setting names or [`stream_adapter`] pins,
/// otherwise the one that drives the display (the primary when it is gone),
/// and with no display connected the one [`headless_gpu`] names.
pub fn stream_gpu(display: &str, adapter_name: &str, pnp_id: &str) -> Result<(String, (u32, i32))> {
    let gpus = gpus()?;
    let (name, pnp_id) = stream_adapter(Some(display), adapter_name, pnp_id);
    if let Some(gpu) = select_gpu(&gpus, &name, &pnp_id)? {
        return Ok((gpu.name.clone(), gpu.luid));
    }
    let choices = displays()?;
    let Some(shown) =
        select_display(&choices, display).or_else(|_| select_display(&choices, ""))?
    else {
        let gpu = headless_gpu(&gpus)?;
        return Ok((gpu.name.clone(), gpu.luid));
    };
    // SAFETY: The factory and adapter are live COM objects, and GetDesc1 returns an owned
    // descriptor.
    let desc = unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
        factory.EnumAdapters1(shown.adapter_index)?.GetDesc1()?
    };
    Ok((
        from_wide(&desc.Description),
        (desc.AdapterLuid.LowPart, desc.AdapterLuid.HighPart),
    ))
}
/// The PCI vendor of [`stream_gpu`]'s GPU for the configured display and adapter, None when it
/// can't be found.
pub fn stream_gpu_vendor(config: &butterpollo_config::config::Config) -> Option<u32> {
    let [display, adapter_name, pnp_id] =
        ["output_name", "adapter_name", "adapter_pnp_id"].map(|key| config.get(key, ""));
    let (_, luid) = stream_gpu(display, adapter_name, pnp_id)
        .inspect_err(|error| tracing::debug!(%error, "the stream's GPU is unknown"))
        .ok()?;
    gpus()
        .ok()?
        .into_iter()
        .find(|gpu| gpu.luid == luid)
        .map(|gpu| gpu.vendor)
}

/// Whose driver settings the panel controls, for the configured stream GPU.
pub fn panel_driver(config: &butterpollo_config::config::Config) -> crate::radeon::Driver {
    let vendors: Vec<u32> = gpus()
        .map(|gpus| gpus.iter().map(|gpu| gpu.vendor).collect())
        .unwrap_or_default();
    crate::radeon::Driver::for_stream(stream_gpu_vendor(config), &vendors)
}
/// The GPU a stream encodes on while no display is connected, as on a
/// headless host or one whose monitor is off: the GPU
/// [`encoder_gpu::pinned`] chooses, else the only AMD or NVIDIA GPU. With
/// none at all, nothing streams.
pub(super) fn headless_gpu(gpus: &[Gpu]) -> Result<&Gpu> {
    let adapters: Vec<encoder_gpu::Ranked> = gpus.iter().map(Gpu::ranked).collect();
    if let Some(index) = encoder_gpu::pinned(&adapters, None) {
        return Ok(&gpus[index]);
    }
    let mut encoding = gpus.iter().filter(|gpu| encoder_gpu::encodes(gpu.vendor));
    match (encoding.next(), encoding.next()) {
        (Some(gpu), None) => Ok(gpu),
        (Some(_), Some(_)) => bail!(
            "no display is connected, and of this PC's AMD and NVIDIA GPUs none is chosen: choose one in the Graphics card setting"
        ),
        (None, _) => bail!("no display is connected, and this PC has no AMD or NVIDIA GPU"),
    }
}
/// The PCI vendor of the GPU that drives `name` (the primary if it is gone).
pub(super) fn display_vendor(name: &str) -> Option<u32> {
    let choices = displays().ok()?;
    let display = select_display(&choices, name)
        .or_else(|_| select_display(&choices, ""))
        .ok()??;
    // SAFETY: The factory and adapter are live COM objects, and GetDesc1 returns an owned
    // descriptor.
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1().ok()?;
        Some(
            factory
                .EnumAdapters1(display.adapter_index)
                .ok()?
                .GetDesc1()
                .ok()?
                .VendorId,
        )
    }
}
/// Whether the probe's device skips the display's adapter for an encoding
/// one: no GPU is chosen, the display is on an adapter that is neither AMD nor
/// NVIDIA, and an AMD or NVIDIA one exists.
pub(super) fn probe_skips_display(display_vendor: u32, chosen: bool, vendors: &[u32]) -> bool {
    !chosen
        && !encoder_gpu::encodes(display_vendor)
        && vendors.iter().any(|v| encoder_gpu::encodes(*v))
}
/// Whether capture on `gpu` can be encoded, for Desktop Duplication, which can
/// capture a display only on the GPU that drives it: the refusal otherwise.
pub(super) fn require_encoding_duplication(name: &str, vendor: u32, all: &[Gpu]) -> Result<()> {
    let adapter = encoder_gpu::Adapter {
        name: name.to_string(),
        vendor,
    };
    encoder_gpu::check_adapter(&adapter, &gpu_adapters(all))?;
    Ok(())
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
    /// The thread priority to put back on drop, for a borrowed (pool) thread.
    restore: Option<i32>,
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
    /// [`Self::new`] on a thread the caller only borrows, such as a Tokio
    /// blocking-pool thread: dropping it also puts the thread priority back.
    pub fn scoped() -> Self {
        // SAFETY: GetThreadPriority reads the current thread's pseudo-handle.
        let previous = unsafe {
            windows::Win32::System::Threading::GetThreadPriority(
                windows::Win32::System::Threading::GetCurrentThread(),
            )
        };
        let mut priority = Self::new();
        // THREAD_PRIORITY_ERROR_RETURN: nothing known to restore.
        priority.restore = (previous != 0x7fff_ffff).then_some(previous);
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
            Self {
                handle,
                restore: None,
            }
        }
    }
}
impl Drop for Priority {
    fn drop(&mut self) {
        if let Some(previous) = self.restore {
            // SAFETY: SetThreadPriority acts on the current thread's pseudo-handle and takes no
            // pointers; callers hold the guard on the stack of the thread it raised.
            unsafe {
                use windows::Win32::System::Threading::*;
                let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY(previous));
            }
        }
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
