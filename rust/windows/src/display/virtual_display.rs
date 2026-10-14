//! Virtual display driver protocol, creation, and lease heartbeats.

use super::modes::{
    ModeOutcome, current_timing, format_timing, mode_outcome, offered_rates, supported_modes,
};
use super::topology::activate_target;
use super::*;

const NAMESPACE: [u8; 16] = [
    0x84, 0x42, 0x86, 0xa2, 0xfe, 0x77, 0x36, 0x43, 0xa8, 0x28, 0, 0xfe, 0xec, 0x89, 0xeb, 0xac,
];
const LEASE_TIMEOUT_MS: u32 = 10_000;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DriverProtocol {
    Legacy35,
    Secure36,
}
impl DriverProtocol {
    fn parse(version: &[u8]) -> Result<Self> {
        if version.len() != 24 || version[..16] != NAMESPACE {
            bail!("invalid virtual display protocol response");
        }
        let major = u16::from_le_bytes(version[16..18].try_into().unwrap());
        let minor = u16::from_le_bytes(version[18..20].try_into().unwrap());
        if major != 3 || minor < 5 {
            bail!("unsupported virtual display protocol {major}.{minor}; requires 3.5+");
        }
        Ok(if minor == 5 {
            Self::Legacy35
        } else {
            Self::Secure36
        })
    }
    fn create_function(self) -> u32 {
        match self {
            Self::Legacy35 => 0x901,
            Self::Secure36 => 0x90c,
        }
    }
    fn name(self) -> &'static str {
        match self {
            Self::Legacy35 => "3.5",
            Self::Secure36 => "3.6+",
        }
    }
}
struct Driver {
    handle: HANDLE,
    protocol: DriverProtocol,
}
impl Driver {
    fn open() -> Result<Self> {
        let mut driver = Self {
            handle: crate::input::open_interface(GUID::from_u128(
                0x5f894d6c_3a69_48a2_86ef_e4c671932d63,
            ))?,
            protocol: DriverProtocol::Legacy35,
        };
        let version = driver.ioctl(0x900, 0, &[], 24)?;
        driver.protocol = DriverProtocol::parse(&version)?;
        Ok(driver)
    }
    fn ioctl(&self, function: u32, access: u32, input: &[u8], size: usize) -> Result<Vec<u8>> {
        // SAFETY: The owned device handle and the input/output buffers remain valid for this synchronous IOCTL with their exact lengths.
        unsafe {
            let mut output = vec![0; size];
            let mut bytes = 0;
            DeviceIoControl(
                self.handle,
                (0x22 << 16) | (access << 14) | (function << 2),
                if input.is_empty() {
                    None
                } else {
                    Some(input.as_ptr().cast())
                },
                input.len() as u32,
                if size == 0 {
                    None
                } else {
                    Some(output.as_mut_ptr().cast())
                },
                size as u32,
                Some(&mut bytes),
                None,
            )?;
            if bytes as usize > size {
                bail!("invalid virtual display response length");
            }
            output.truncate(bytes as usize);
            Ok(output)
        }
    }
}
impl Drop for Driver {
    fn drop(&mut self) {
        // SAFETY: Driver owns this device handle and closes it exactly once on drop.
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}
/// Windows 11 (build 22000) or later. Vibepollo keeps hosts on Windows 10 on
/// the physical display unless configured otherwise: virtual-display capture
/// depends on Windows 11 capture features.
pub fn windows_11() -> bool {
    static BUILD: std::sync::OnceLock<Option<u32>> = std::sync::OnceLock::new();
    BUILD
        // SAFETY: The registry names are terminated and the output size matches the live UTF-16 buffer.
        .get_or_init(|| unsafe {
            use windows::Win32::System::Registry::*;
            let mut text = [0u16; 32];
            let mut size = (text.len() * 2) as u32;
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                windows::core::w!("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion"),
                windows::core::w!("CurrentBuildNumber"),
                RRF_RT_REG_SZ,
                None,
                Some(text.as_mut_ptr().cast()),
                Some(&mut size),
            )
            .ok()
            .ok()?;
            wide(&text).trim().parse().ok()
        })
        .is_none_or(|build| build >= 22000)
}
pub fn virtual_display_available() -> bool {
    Driver::open().is_ok()
}
pub fn virtual_display_status() -> serde_json::Value {
    match Driver::open() {
        Ok(driver) => {
            serde_json::json!({"capable":true,"ready":true,"reason":"","protocol":driver.protocol.name()})
        }
        Err(error) => {
            serde_json::json!({"capable":false,"ready":false,"reason":driver_problem(&error),"protocol":"3.5+"})
        }
    }
}
/// What the console says when the driver cannot be opened.
fn driver_problem(error: &anyhow::Error) -> String {
    use windows::Win32::Foundation::{ERROR_ACCESS_DENIED, ERROR_NO_MORE_ITEMS};
    let code = error
        .downcast_ref::<windows::core::Error>()
        .map(|e| e.code());
    if code == Some(ERROR_NO_MORE_ITEMS.to_hresult()) {
        "The virtual display driver is not installed, or Windows must restart to finish installing it. The Rubylight service sets it up again a minute after it starts; if it stays missing, run the Rubylight installer.".into()
    } else if code == Some(ERROR_ACCESS_DENIED.to_hresult()) {
        "Only the Rubylight service may open the virtual display driver, and this host runs outside it. Install Rubylight with its setup and let the service run the host.".into()
    } else {
        format!("{error:#}")
    }
}
fn display_label(name: &str) -> [u8; 32] {
    let mut label = [0u8; 32];
    let name: Vec<_> = name
        .bytes()
        .filter(|c| (0x20..=0x7e).contains(c))
        .take(31)
        .collect();
    let end = name.iter().rposition(|c| *c != b' ').map_or(0, |n| n + 1);
    if end == 0 {
        label[..b"Butterpollo".len()].copy_from_slice(b"Butterpollo");
    } else {
        label[..end].copy_from_slice(&name[..end]);
    }
    label
}
#[derive(Clone, Debug)]
pub struct VirtualOptions {
    pub label: String,
    pub peak_nits: u32,
    /// The GPU to render the display on; None leaves the choice to Windows.
    pub render_adapter: Option<RenderAdapter>,
}
impl Default for VirtualOptions {
    fn default() -> Self {
        Self {
            label: "Butterpollo".into(),
            peak_nits: 1000,
            render_adapter: None,
        }
    }
}
/// The GPU that renders the driver's displays: IddCx's preferred render
/// adapter. Windows otherwise picks one, on a hybrid PC often the integrated
/// GPU, and only that GPU can then duplicate the display.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderAdapter {
    pub name: String,
    pub luid: (u32, i32),
}
/// The GPU the Graphics card setting names, for virtual displays to render on
/// as Apollo and Vibepollo do. Automatic leaves the choice to Windows.
pub fn configured_render_adapter(
    config: &butterpollo_core::config::Config,
) -> Option<RenderAdapter> {
    let (name, pnp_id) = (
        config.get("adapter_name", ""),
        config.get("adapter_pnp_id", ""),
    );
    if name.is_empty() && pnp_id.is_empty() {
        return None;
    }
    let selected = crate::capture::gpus().and_then(|gpus| {
        Ok(
            crate::capture::select_gpu(&gpus, name, pnp_id)?.map(|gpu| RenderAdapter {
                name: gpu.name.clone(),
                luid: gpu.luid,
            }),
        )
    });
    selected
        .inspect_err(|error| {
        tracing::warn!(error = %format!("{error:#}"), "the selected GPU was not found; Windows picks the GPU that renders the virtual display")
    })
    .ok()
    .flatten()
}
/// libvirtualdisplay's SetRenderAdapter IOCTL: function 0x90b, write access.
const SET_RENDER_ADAPTER: (u32, u32) = (0x90b, 2);
/// libvirtualdisplay's SetRenderAdapterRequest: namespace, LUID, flags, reserved.
fn render_adapter_request(luid: (u32, i32)) -> Vec<u8> {
    let mut request = NAMESPACE.to_vec();
    request.extend_from_slice(&luid.0.to_le_bytes());
    request.extend_from_slice(&luid.1.to_le_bytes());
    request.extend_from_slice(&0u32.to_le_bytes()); // Flags.
    request.extend_from_slice(&0u32.to_le_bytes()); // Reserved.
    request
}
/// The IOCTL to send before a display is created or recreated, if any.
fn render_adapter_call(options: &VirtualOptions) -> Option<(u32, u32, Vec<u8>)> {
    let adapter = options.render_adapter.as_ref()?;
    let (function, access) = SET_RENDER_ADAPTER;
    Some((function, access, render_adapter_request(adapter.luid)))
}
/// Best effort: a driver that refuses leaves the display on the GPU Windows
/// picks, and capture then says it encodes there.
fn apply_render_adapter(driver: &Driver, options: &VirtualOptions) {
    let (Some(adapter), Some((function, access, request))) =
        (&options.render_adapter, render_adapter_call(options))
    else {
        return;
    };
    match retry_busy(
        || driver.ioctl(function, access, &request, 0),
        Duration::from_secs(1),
    ) {
        Ok(_) => tracing::info!(gpu = %adapter.name, "virtual display renders on the selected GPU"),
        Err(error) => {
            tracing::warn!(gpu = %adapter.name, error = %format!("{error:#}"), "the virtual display driver did not accept the selected GPU as its render adapter; Windows picks the GPU that renders the virtual display, and Desktop Duplication then captures and encodes there")
        }
    }
}
fn temporary_request(
    lease: u64,
    id: u64,
    mode: (u32, u32, u32),
    options: &VirtualOptions,
    protocol: DriverProtocol,
    capability: &[u8; 32],
) -> Vec<u8> {
    let mut request = NAMESPACE.to_vec();
    request.extend_from_slice(&lease.to_le_bytes());
    request.extend_from_slice(&id.to_le_bytes());
    for value in [mode.0, mode.1, 600, 340, mode.2, LEASE_TIMEOUT_MS] {
        request.extend_from_slice(&value.to_le_bytes());
    }
    request.extend_from_slice(&display_label(&options.label));
    request.extend_from_slice(&0u32.to_le_bytes()); // Retain Windows identity across sessions.
    if protocol == DriverProtocol::Secure36 {
        request.extend_from_slice(&options.peak_nits.clamp(400, 2000).to_le_bytes());
        request.extend_from_slice(capability);
    } else {
        request.extend_from_slice(&0u32.to_le_bytes()); // Legacy reserved field.
    }
    request
}
fn permanent_request(count: u32) -> Result<Vec<u8>> {
    if count > 4 {
        bail!("permanent virtual display count must be between 0 and 4");
    }
    let mut request = NAMESPACE.to_vec();
    for value in [count, 0, 1920, 1080, 600, 340, 60000] {
        request.extend_from_slice(&value.to_le_bytes());
    }
    request.extend_from_slice(&display_label("Butterpollo"));
    Ok(request)
}
fn permanent_response(bytes: &[u8]) -> Result<u32> {
    if bytes.len() != 80 || bytes[..16] != NAMESPACE {
        bail!("invalid permanent display response");
    }
    let count = u32::from_le_bytes(bytes[16..20].try_into().unwrap());
    let max = u32::from_le_bytes(bytes[20..24].try_into().unwrap());
    if count > max || count > 4 {
        bail!("invalid permanent display count");
    }
    Ok(count)
}
/// Persistent driver setting, applied only when the administrator explicitly
/// configured this key. It is independent of temporary streaming leases.
pub fn configure_permanent(config: &butterpollo_core::config::Config) -> Result<()> {
    let Some(count) = configured_permanent_count(config)? else {
        return Ok(());
    };
    if permanent_display_count()? == count {
        return Ok(());
    }
    set_permanent_display_count(count)
}
fn configured_permanent_count(config: &butterpollo_core::config::Config) -> Result<Option<u32>> {
    // Config reads dd_vdd_static_monitor_count, the key's older name, as this.
    let Some(value) = config.values.get("dd_virtual_display_permanent_count") else {
        return Ok(None);
    };
    let count = butterpollo_core::config::parse_integer(value)
        .and_then(|count| u32::try_from(count).ok())
        .context("invalid permanent virtual display count")?;
    Ok(Some(count))
}
/// The driver's persistent display count.
pub fn permanent_display_count() -> Result<u32> {
    permanent_response(&Driver::open()?.ioctl(0x907, 1, &[], 80)?)
}
pub fn set_permanent_display_count(count: u32) -> Result<()> {
    let request = permanent_request(count)?;
    let driver = Driver::open()?;
    let result = driver.ioctl(0x906, 3, &request, 80);
    // The driver may persist the count and report a registry-write failure.
    // Confirm runtime state before treating that failure as fatal.
    let after = match result {
        Ok(bytes) => permanent_response(&bytes)?,
        Err(error) => match driver
            .ioctl(0x907, 1, &[], 80)
            .and_then(|bytes| permanent_response(&bytes))
        {
            Ok(actual) if actual == count => actual,
            _ => return Err(error),
        },
    };
    if after != count {
        bail!("driver did not apply the permanent display count");
    }
    Ok(())
}
pub struct VirtualDisplay {
    driver: Driver,
    pub name: String,
    lease: u64,
    id: u64,
    pub(super) last_feed: Instant,
    mode: (u32, u32, u32),
    options: VirtualOptions,
    pub(super) generation: u64,
    capability: [u8; 32],
    startup_protection: Option<hotplug::Protection>,
    pub(super) resolved_target: Option<Monitor>,
    /// Windows left this display off and the host switched it on.
    pub switched_on: bool,
}
// SAFETY: Driver IOCTLs use a thread-safe Windows device handle; shared access is
// serialized by the enclosing mutex, including feed and final teardown.
unsafe impl Send for VirtualDisplay {}
pub(super) type DisplayLease = std::sync::Arc<std::sync::Mutex<VirtualDisplay>>;
static DISPLAYS: std::sync::Mutex<
    std::collections::BTreeMap<String, std::sync::Weak<std::sync::Mutex<VirtualDisplay>>>,
> = std::sync::Mutex::new(std::collections::BTreeMap::new());
pub(super) fn display_lease(
    id: &str,
    width: u32,
    height: u32,
    fps: u32,
    options: &VirtualOptions,
) -> Result<DisplayLease> {
    let mut displays = DISPLAYS.lock().unwrap();
    displays.retain(|_, lease| lease.strong_count() != 0);
    if let Some(display) = displays.get(id).and_then(std::sync::Weak::upgrade) {
        if display.lock().unwrap().mode != (width, height, fps) {
            bail!("shared virtual display already uses a different mode");
        }
        return Ok(display);
    }
    let display = std::sync::Arc::new(std::sync::Mutex::new(VirtualDisplay::create_options(
        id,
        width,
        height,
        fps,
        options.clone(),
    )?));
    displays.insert(id.into(), std::sync::Arc::downgrade(&display));
    Ok(display)
}
fn lease_exists(result: Result<()>) -> Result<bool> {
    match result {
        Ok(()) => Ok(true),
        // The driver reports STATUS_NOT_FOUND for an expired/missing lease.
        // BUSY and other I/O failures say nothing about monitor ownership.
        Err(error)
            if error
                .downcast_ref::<windows::core::Error>()
                .is_some_and(|e| {
                    e.code() == windows::core::HRESULT::from_win32(ERROR_NOT_FOUND.0)
                }) =>
        {
            Ok(false)
        }
        Err(error) => Err(error),
    }
}
fn retry_busy<T>(mut attempt: impl FnMut() -> Result<T>, timeout: Duration) -> Result<T> {
    let deadline = Instant::now() + timeout;
    loop {
        match attempt() {
            Err(error)
                if error
                    .downcast_ref::<windows::core::Error>()
                    .is_some_and(|e| e.code() == ERROR_BUSY.to_hresult())
                    && Instant::now() < deadline =>
            {
                tracing::debug!(%error, "virtual display driver busy; retrying");
                std::thread::sleep(
                    Duration::from_millis(100)
                        .min(deadline.saturating_duration_since(Instant::now())),
                );
            }
            result => return result,
        }
    }
}
fn wait_for_monitor(
    mut query: impl FnMut() -> Result<Option<Monitor>>,
    mut recover: impl FnMut() -> Result<()>,
    timeout: Duration,
) -> Result<Monitor> {
    let deadline = Instant::now() + timeout;
    let mut next_recovery = Instant::now();
    loop {
        if let Some(monitor) = query()? {
            return Ok(monitor);
        }
        anyhow::ensure!(
            Instant::now() < deadline,
            "owned virtual display unavailable after startup protection"
        );
        if Instant::now() >= next_recovery {
            recover()?;
            next_recovery = Instant::now() + Duration::from_secs(1);
        }
        std::thread::sleep(
            Duration::from_millis(50).min(deadline.saturating_duration_since(Instant::now())),
        );
    }
}
/// What a lease heartbeat does about the owned display.
#[derive(Debug, PartialEq)]
enum Heartbeat {
    /// Leased and on the desktop.
    Present,
    /// Leased and connected, but Windows switched it off.
    SwitchOn,
    /// The driver no longer knows the lease.
    Recreate,
}
/// Only the driver's missing-lease answer permits creating the display again.
/// A failed renewal or topology query is retried at the next heartbeat: the
/// driver refuses to create a display that still exists (ERROR_BUSY), and a
/// lease that stays unrenewed expires and is then reported missing.
fn heartbeat(
    renewed: Result<bool>,
    active: impl FnOnce() -> Result<bool>,
    connected: impl FnOnce() -> Result<bool>,
) -> Result<Heartbeat> {
    if !renewed? {
        return Ok(Heartbeat::Recreate);
    }
    if active()? {
        return Ok(Heartbeat::Present);
    }
    anyhow::ensure!(
        connected()?,
        "owned virtual display is temporarily absent from Windows topology"
    );
    Ok(Heartbeat::SwitchOn)
}
impl VirtualDisplay {
    pub fn create(stable_id: &str, width: u32, height: u32, fps: u32) -> Result<Self> {
        Self::create_rate(
            stable_id,
            width,
            height,
            fps.checked_mul(1000).context("refresh overflow")?,
        )
    }
    pub fn create_rate(stable_id: &str, width: u32, height: u32, fps: u32) -> Result<Self> {
        Self::create_options(stable_id, width, height, fps, VirtualOptions::default())
    }
    pub fn create_options(
        stable_id: &str,
        width: u32,
        height: u32,
        fps: u32,
        options: VirtualOptions,
    ) -> Result<Self> {
        if !(320..=7680).contains(&width)
            || !(200..=4320).contains(&height)
            || !(1000..=1_000_000).contains(&fps)
        {
            bail!("virtual display mode is outside the driver limits");
        }
        let driver = Driver::open()?;
        let lease = (u64::from_le_bytes(butterpollo_core::crypto::random())
            & 0x1fff_ffff_ffff_ffff)
            | 0x6000_0000_0000_0000;
        let id = butterpollo_core::display_policy::virtual_display_id(stable_id);
        let capability = butterpollo_core::crypto::random::<32>();
        let request = temporary_request(
            lease,
            id,
            (width, height, fps),
            &options,
            driver.protocol,
            &capability,
        );
        apply_render_adapter(&driver, &options);
        // A previous host's lease can survive a session handover until its
        // removal completes or its heartbeat expires. Keep the same identity.
        let (result, startup_protection) = retry_busy(
            || {
                let protection = hotplug::Protection::capture()?;
                let result = driver.ioctl(driver.protocol.create_function(), 3, &request, 56)?;
                Ok((result, protection))
            },
            Duration::from_millis(u64::from(LEASE_TIMEOUT_MS)),
        )?;
        let mut display = Self {
            driver,
            name: String::new(),
            lease,
            id,
            last_feed: Instant::now(),
            mode: (width, height, fps),
            options,
            generation: 0,
            capability,
            startup_protection: Some(startup_protection),
            resolved_target: None,
            switched_on: false,
        };
        display.resolve(&result)?;
        display.check_hotplug("created")?;
        Ok(display)
    }
    fn resolve(&mut self, result: &[u8]) -> Result<()> {
        if result.len() != 56
            || result[..16] != NAMESPACE
            || result[16..24] != self.lease.to_le_bytes()
            || result[24..32] != self.id.to_le_bytes()
        {
            bail!("invalid virtual display identity response");
        }
        let luid = LUID {
            LowPart: u32::from_le_bytes(result[32..36].try_into().unwrap()),
            HighPart: i32::from_le_bytes(result[36..40].try_into().unwrap()),
        };
        let target = u32::from_le_bytes(result[40..44].try_into().unwrap());
        let deadline = Instant::now() + Duration::from_secs(10);
        // Windows decides whether a display it has just connected joins the
        // desktop. A layout it saved for the same displays (duplicate, or one
        // screen only) can leave the new virtual display connected but off.
        // Vibepollo's display helper switches it on after a short grace; so
        // do we, without changing the other displays.
        let mut connected_since = None;
        let mut next_check = Instant::now();
        while Instant::now() < deadline {
            if let Ok(monitors) = monitors()
                && let Some(m) = monitors
                    .into_iter()
                    // Windows can report the active route before publishing its
                    // monitor interface. An empty path cannot identify our display.
                    .find(|m| {
                        m.adapter == luid && m.target == target && !m.monitor_device_path.is_empty()
                    })
            {
                self.name = m.display_name.clone();
                self.resolved_target = Some(m);
                self.last_feed = Instant::now();
                return Ok(());
            }
            if Instant::now() >= next_check {
                next_check = Instant::now() + Duration::from_millis(250);
                let connected = Topology::query_all().is_ok_and(|all| {
                    all.paths.iter().any(|p| {
                        p.targetInfo.adapterId == luid
                            && p.targetInfo.id == target
                            && p.targetInfo.targetAvailable.as_bool()
                    })
                });
                if connected {
                    let since = *connected_since.get_or_insert_with(Instant::now);
                    if since.elapsed() >= Duration::from_secs(1) {
                        next_check = Instant::now() + Duration::from_secs(1);
                        match activate_target(luid, target) {
                            Ok(Some(kept_timings)) => {
                                self.switched_on = true;
                                tracing::info!(
                                    kept_timings,
                                    "Windows left the new virtual display switched off; switched it on beside the current displays"
                                )
                            }
                            Ok(None) => {}
                            Err(error) => tracing::warn!(
                                error = format!("{error:#}"),
                                "could not switch on the new virtual display"
                            ),
                        }
                    }
                }
            }
            if self.last_feed.elapsed() >= Duration::from_secs(1) {
                self.renew()?;
                self.last_feed = Instant::now();
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        if connected_since.is_some() {
            bail!("virtual display was connected but Windows kept it switched off")
        }
        bail!("virtual display did not become active before the deadline")
    }
    pub(super) fn owns_monitor(&self, monitor: &Monitor) -> bool {
        self.resolved_target.as_ref().is_some_and(|owned| {
            owned.adapter == monitor.adapter
                && owned.target == monitor.target
                && owned
                    .monitor_device_path
                    .eq_ignore_ascii_case(&monitor.monitor_device_path)
        })
    }
    fn hotplug_monitor(&self) -> Result<&Monitor> {
        // Resolve stores the target returned by our driver request. Never infer
        // ownership from a GDI display name that Windows could later reuse.
        self.resolved_target
            .as_ref()
            .context("owned virtual display unavailable during startup protection")
    }
    fn refresh_name(&mut self) -> Result<()> {
        self.name = self.current_monitor()?.display_name;
        Ok(())
    }
    pub(super) fn current_monitor(&mut self) -> Result<Monitor> {
        let owned = self.hotplug_monitor()?.clone();
        wait_for_monitor(
            || {
                Ok(monitors()?.into_iter().find(|m| {
                    m.adapter == owned.adapter
                        && m.target == owned.target
                        && m.monitor_device_path
                            .eq_ignore_ascii_case(&owned.monitor_device_path)
                }))
            },
            || {
                // Windows can switch the owned target off during startup,
                // before the stream starts feeding its lease.
                self.renew()?;
                self.last_feed = Instant::now();
                // SAFETY: Topology::shows supplies initialized device-name
                // structures with matching header types and sizes.
                if Topology::query_all()?.shows(&owned, |header| unsafe {
                    DisplayConfigGetDeviceInfo(header)
                })? && let Some(kept_timings) = activate_target(owned.adapter, owned.target)?
                {
                    self.switched_on = true;
                    tracing::info!(
                        kept_timings,
                        "owned virtual display reactivated during startup"
                    );
                }
                Ok(())
            },
            Duration::from_secs(10),
        )
    }
    fn check_hotplug(&mut self, stage: &str) -> Result<()> {
        if let Some(protection) = &self.startup_protection {
            if let Err(error) = protection.check(self.hotplug_monitor()?, stage) {
                // Keeping another display off is a courtesy; a stream that
                // cannot start is worse than one that leaves it on.
                tracing::warn!(
                    error = format!("{error:#}"),
                    stage,
                    "could not keep inactive displays off; streaming without startup protection"
                );
                self.startup_protection = None;
            }
            self.refresh_name()?;
        }
        Ok(())
    }
    /// Finish only after virtual HDR settings have settled. Afterwards the
    /// user is free to change displays without a stream overriding them.
    pub(super) fn finish_hotplug(&mut self, stage: &str) -> Result<()> {
        if self.startup_protection.is_some() {
            let owned = self.hotplug_monitor()?.clone();
            let protection = self.startup_protection.as_mut().unwrap();
            if let Err(error) = protection.settle(&owned, stage) {
                // The guard covers startup only and is a courtesy to the other
                // displays: end it and keep streaming rather than fail.
                tracing::warn!(
                    error = format!("{error:#}"),
                    stage,
                    "virtual display startup protection ended before the layout settled"
                );
            }
            self.refresh_name()?;
            self.startup_protection = None;
        }
        self.apply_mode(stage);
        Ok(())
    }
    /// Set the stream's mode on the display. The driver offers it, but
    /// Windows picks the mode it saved for this display identity, or the
    /// EDID's preferred timing, which the driver caps at 60 Hz for 4K. It
    /// does so on arrival, when it switches the display back on and when a
    /// layout is applied. A refused mode is logged, not retried: the stream
    /// continues at the display's mode and its card reports the difference.
    pub(super) fn apply_mode(&self, stage: &str) {
        let (width, height, rate) = self.mode;
        let result = Topology::set_mode_rate(
            &self.name,
            width,
            height,
            butterpollo_core::framegen::Rate(rate),
        );
        let actual = current_timing(&self.name);
        let requested = format_timing(self.mode);
        match mode_outcome(self.mode, actual.as_ref().ok().copied()) {
            // Applied tolerates rational rates: log what Windows shows.
            ModeOutcome::Applied => tracing::info!(
                output = %self.name,
                stage,
                requested = %requested,
                actual = %actual.as_ref().map_or(requested.clone(), |t| format_timing(*t)),
                "virtual display mode applied"
            ),
            outcome => {
                let actual_text = match outcome {
                    ModeOutcome::Kept(actual) => format_timing(actual),
                    _ => "unknown".into(),
                };
                tracing::warn!(
                    output = %self.name,
                    stage,
                    requested = %requested,
                    actual = %actual_text,
                    offered_hz = ?offered_rates(&supported_modes(&self.name), width, height),
                    error = ?result.err().or(actual.err()).map(|e| format!("{e:#}")),
                    "virtual display mode not applied; streaming at the mode Windows chose"
                )
            }
        }
    }
    fn renew(&mut self) -> Result<()> {
        let mut request = NAMESPACE.to_vec();
        request.extend_from_slice(&self.lease.to_le_bytes());
        request.extend_from_slice(&LEASE_TIMEOUT_MS.to_le_bytes());
        request.extend_from_slice(&0u32.to_le_bytes());
        self.driver.ioctl(0x903, 3, &request, 0)?;
        Ok(())
    }
    pub fn feed(&mut self) -> Result<()> {
        if self.last_feed.elapsed() >= Duration::from_secs(1) {
            self.last_feed = Instant::now();
            let renewed = lease_exists(self.renew());
            if renewed.is_err() {
                // Reopen a stale transport for the next heartbeat, without
                // mistaking its error for permission to create another monitor.
                if let Ok(driver) = Driver::open() {
                    self.driver = driver;
                }
            }
            // SAFETY: Topology::shows supplies a live source- or target-name structure with a matching header type and size.
            let device_info = |header: &mut DISPLAYCONFIG_DEVICE_INFO_HEADER| unsafe {
                DisplayConfigGetDeviceInfo(header)
            };
            match heartbeat(
                renewed,
                || Topology::query()?.shows(self.hotplug_monitor()?, device_info),
                || Topology::query_all()?.shows(self.hotplug_monitor()?, device_info),
            )? {
                Heartbeat::Present => return Ok(()),
                Heartbeat::SwitchOn => {
                    // A live lease can be switched off by a Windows layout
                    // recall. Activate that same target; creating its ID again
                    // returns BUSY.
                    let owned = self.hotplug_monitor()?;
                    activate_target(owned.adapter, owned.target)?
                        .context("owned virtual display has no connected route yet")?;
                    // Publish the change before the name lookup, which can lag
                    // the switch. Once the display is on, later heartbeats find
                    // it present, so a failed lookup must not leave the guard
                    // unaware: it reapplies the stream's layout and HDR and
                    // takes the display's new desktop name itself.
                    self.generation = self.generation.wrapping_add(1);
                    self.refresh_name()?;
                    tracing::info!(output=%self.name, "owned virtual display reactivated");
                    return Ok(());
                }
                Heartbeat::Recreate => {}
            }
            // Only a confirmed missing lease permits creation. Retain the
            // stream's descriptor and identity, never a recalled desktop mode.
            self.driver = Driver::open()?;
            // A restarted driver has forgotten the render adapter.
            apply_render_adapter(&self.driver, &self.options);
            let request = temporary_request(
                self.lease,
                self.id,
                self.mode,
                &self.options,
                self.driver.protocol,
                &self.capability,
            );
            let protection = hotplug::Protection::capture()?;
            let response =
                self.driver
                    .ioctl(self.driver.protocol.create_function(), 3, &request, 56)?;
            self.startup_protection = Some(protection);
            self.resolve(&response)?;
            // Retain a resolved recovery even if protection needs a retry, so
            // Guard::feed still completes HDR/startup validation next time.
            self.generation = self.generation.wrapping_add(1);
            self.check_hotplug("recreated")?;
            tracing::info!(output=%self.name, "owned virtual display recovered");
        }
        Ok(())
    }
}
impl Drop for VirtualDisplay {
    fn drop(&mut self) {
        let mut request = NAMESPACE.to_vec();
        request.extend_from_slice(&self.lease.to_le_bytes());
        request.extend_from_slice(&self.id.to_le_bytes());
        if let Err(e) = lease_exists(retry_busy(
            || self.driver.ioctl(0x902, 3, &request, 0).map(|_| ()),
            Duration::from_secs(1),
        )) {
            tracing::warn!(error=%e,"virtual display removal failed; lease expiration will recover it");
        }
        request.truncate(24);
        request.extend_from_slice(&0u64.to_le_bytes());
        // Removing the last display also removes its lease in the driver.
        if let Err(e) = lease_exists(retry_busy(
            || self.driver.ioctl(0x904, 3, &request, 0).map(|_| ()),
            Duration::from_secs(1),
        )) {
            tracing::warn!(error=%e,"virtual display lease release failed; lease expiration will recover it");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::modes::timing_matches;
    use super::*;
    #[test]
    fn a_busy_driver_request_recovers_within_the_same_call() {
        let mut attempts = 0;
        let result = retry_busy(
            || {
                attempts += 1;
                if attempts < 3 {
                    Err(windows::core::Error::from_hresult(ERROR_BUSY.to_hresult()).into())
                } else {
                    Ok(vec![17, 23])
                }
            },
            Duration::from_secs(1),
        )
        .unwrap();
        assert_eq!(result, [17, 23]);
        assert_eq!(attempts, 3);
    }
    #[test]
    fn driver_retries_stop_at_the_deadline_and_preserve_other_errors() {
        for code in [
            ERROR_BUSY,
            ERROR_ACCESS_DENIED,
            ERROR_NOT_FOUND,
            ERROR_GEN_FAILURE,
        ] {
            let mut attempts = 0;
            let timeout = if code == ERROR_BUSY {
                Duration::ZERO
            } else {
                Duration::from_secs(1)
            };
            let error = retry_busy::<()>(
                || {
                    attempts += 1;
                    Err(windows::core::Error::from_hresult(code.to_hresult()).into())
                },
                timeout,
            )
            .unwrap_err();
            assert_eq!(attempts, 1);
            assert_eq!(
                error.downcast_ref::<windows::core::Error>().unwrap().code(),
                code.to_hresult()
            );
        }
    }
    #[test]
    fn an_owned_monitor_can_reappear_during_session_handover() {
        let mut attempts = 0;
        let found = wait_for_monitor(
            || {
                attempts += 1;
                Ok((attempts == 3).then(|| super::super::monitor("owned", 4, false)))
            },
            || Ok(()),
            Duration::from_secs(1),
        )
        .unwrap();
        assert_eq!(found.device_id, "owned");
        assert_eq!(attempts, 3);
    }
    #[test]
    fn startup_recovers_a_connected_monitor_instead_of_only_polling_it() {
        let renewals = std::cell::Cell::new(0);
        let found = wait_for_monitor(
            || Ok((renewals.get() == 2).then(|| super::super::monitor("owned", 4, false))),
            || {
                renewals.set(renewals.get() + 1);
                Ok(())
            },
            Duration::from_secs(2),
        )
        .unwrap();
        assert_eq!(found.device_id, "owned");
        assert_eq!(renewals.get(), 2);
    }
    #[test]
    fn startup_does_not_reactivate_a_present_monitor_or_hide_a_lease_error() {
        assert!(
            wait_for_monitor(
                || Ok(Some(super::super::monitor("owned", 4, false))),
                || panic!("an active display needs no recovery"),
                Duration::ZERO,
            )
            .is_ok()
        );
        let error = wait_for_monitor(
            || Ok(None),
            || Err(windows::core::Error::from_hresult(ERROR_ACCESS_DENIED.to_hresult()).into()),
            Duration::from_secs(1),
        )
        .err()
        .unwrap();
        assert_eq!(
            error.downcast_ref::<windows::core::Error>().unwrap().code(),
            ERROR_ACCESS_DENIED.to_hresult()
        );
    }
    #[test]
    fn an_absent_owned_monitor_or_failed_query_does_not_wait_forever() {
        let mut attempts = 0;
        assert!(
            wait_for_monitor(
                || {
                    attempts += 1;
                    Ok(None)
                },
                || panic!("expired lookup must not change the topology"),
                Duration::ZERO,
            )
            .is_err()
        );
        assert_eq!(attempts, 1);
        let error = wait_for_monitor(
            || Err(windows::core::Error::from_hresult(ERROR_ACCESS_DENIED.to_hresult()).into()),
            || panic!("failed query must not change the topology"),
            Duration::from_secs(1),
        )
        .err()
        .unwrap();
        assert_eq!(
            error.downcast_ref::<windows::core::Error>().unwrap().code(),
            ERROR_ACCESS_DENIED.to_hresult()
        );
    }
    #[test]
    fn permanent_display_count_accepts_the_legacy_key_and_prefers_the_current_key() {
        use butterpollo_core::config::Config;
        for (text, expected) in [
            ("", None),
            ("dd_vdd_static_monitor_count=2", Some(2)),
            (
                "dd_virtual_display_permanent_count=0\ndd_vdd_static_monitor_count=2",
                Some(0),
            ),
        ] {
            assert_eq!(
                configured_permanent_count(&Config::parse(text).unwrap()).unwrap(),
                expected
            );
        }
        assert!(
            configured_permanent_count(
                &Config::parse("dd_virtual_display_permanent_count=invalid").unwrap()
            )
            .is_err()
        );
    }
    #[test]
    fn permanent_display_count_accepts_quoted_numbers_like_other_config_integers() {
        for key in [
            "dd_virtual_display_permanent_count",
            "dd_vdd_static_monitor_count",
        ] {
            for (value, expected) in [
                ("\"2\"", 2),
                ("\" 2 \"", 2),
                ("0x2", 2),
                ("\"0X2\"", 2),
                ("\"0\"", 0),
                ("\"4294967295\"", u32::MAX),
            ] {
                let config =
                    butterpollo_core::config::Config::parse(&format!("{key}={value}")).unwrap();
                assert_eq!(
                    configured_permanent_count(&config).unwrap(),
                    Some(expected),
                    "{key}={value}"
                );
            }
            for value in [
                "-1",
                "\"-1\"",
                "4294967296",
                "\"4294967296\"",
                "\"invalid\"",
            ] {
                let config =
                    butterpollo_core::config::Config::parse(&format!("{key}={value}")).unwrap();
                assert!(
                    configured_permanent_count(&config).is_err(),
                    "{key}={value}"
                );
            }
        }
    }
    #[test]
    fn temporary_monitor_metadata_preserves_the_owned_identity_and_sanitizes_labels() {
        let options = VirtualOptions {
            label: "Living Room\0\n😀   ".into(),
            peak_nits: 1500,
            ..Default::default()
        };
        let request = temporary_request(
            17,
            23,
            (1920, 1080, 59940),
            &options,
            DriverProtocol::Secure36,
            &[42; 32],
        );
        assert_eq!(request.len(), 128);
        assert_eq!(&request[16..24], &17u64.to_le_bytes());
        assert_eq!(&request[24..32], &23u64.to_le_bytes());
        assert_eq!(&request[48..52], &59940u32.to_le_bytes());
        assert_eq!(&request[56..88], &display_label("Living Room"));
        assert_eq!(&request[88..92], &0u32.to_le_bytes());
        assert_eq!(&request[92..96], &1500u32.to_le_bytes());
        assert_eq!(&display_label("😀")[..11], b"Butterpollo");
        assert_eq!(display_label(&"A".repeat(40))[31], 0);
    }
    #[test]
    fn only_a_missing_driver_lease_allows_recreation() {
        assert!(lease_exists(Ok(())).unwrap());
        for code in [
            ERROR_BUSY,
            ERROR_RETRY,
            ERROR_GEN_FAILURE,
            ERROR_ACCESS_DENIED,
            ERROR_INVALID_HANDLE,
        ] {
            let error =
                windows::core::Error::from_hresult(windows::core::HRESULT::from_win32(code.0));
            assert!(lease_exists(Err(error.into())).is_err(), "{code:?}");
            assert!(
                lease_exists(Ok(())).unwrap(),
                "the next heartbeat can succeed"
            );
        }
        let error = windows::core::Error::from_hresult(windows::core::HRESULT::from_win32(
            ERROR_NOT_FOUND.0,
        ));
        assert!(!lease_exists(Err(error.into())).unwrap());
    }
    #[test]
    fn heartbeat_recreates_only_a_missing_lease_and_switches_on_a_display_windows_turned_off() {
        let failure = |code: WIN32_ERROR| -> anyhow::Error {
            windows::core::Error::from_hresult(windows::core::HRESULT::from_win32(code.0)).into()
        };
        let busy = || lease_exists(Err(failure(ERROR_BUSY)));
        let missing = || lease_exists(Err(failure(ERROR_NOT_FOUND)));
        // The log this guards against: the lease renewed, Windows had switched
        // the display off, and a duplicate create returned ERROR_BUSY.
        assert_eq!(
            heartbeat(
                Ok(true),
                || Ok(true),
                || panic!("present needs no full query")
            )
            .unwrap(),
            Heartbeat::Present
        );
        assert_eq!(
            heartbeat(Ok(true), || Ok(false), || Ok(true)).unwrap(),
            Heartbeat::SwitchOn
        );
        // Not yet shown anywhere, or a query failed: retry, never create.
        assert!(heartbeat(Ok(true), || Ok(false), || Ok(false)).is_err());
        assert!(
            heartbeat(
                Ok(true),
                || Err(failure(ERROR_BUSY)),
                || panic!("no full query")
            )
            .is_err()
        );
        assert!(heartbeat(Ok(true), || Ok(false), || Err(failure(ERROR_BUSY))).is_err());
        // A renewal failure other than a missing lease is retried as well,
        // without touching the topology.
        assert!(
            heartbeat(
                busy(),
                || panic!("no topology query"),
                || panic!("no topology query")
            )
            .is_err()
        );
        // A lease the driver no longer knows is a real loss: recreate at once,
        // whatever Windows still lists.
        assert_eq!(
            heartbeat(
                missing(),
                || panic!("no topology query"),
                || panic!("no topology query")
            )
            .unwrap(),
            Heartbeat::Recreate
        );
    }
    #[test]
    #[ignore = "creates a virtual display and briefly switches it off"]
    fn native_virtual_display_left_off_by_windows_is_switched_on_without_retiming_others()
    -> Result<()> {
        let mut display = VirtualDisplay::create(
            &format!("activation-test-{}", std::process::id()),
            1280,
            720,
            60,
        )?;
        let owned = display
            .resolved_target
            .clone()
            .context("virtual display unresolved")?;
        let ours = |m: &Monitor| m.adapter == owned.adapter && m.target == owned.target;
        // Device, width, height, refresh and desktop position.
        type Timing = (String, u32, u32, u32, i32, i32);
        let timings = || -> Result<Vec<Timing>> {
            monitors()?
                .iter()
                .filter(|m| !ours(m))
                .map(|m| {
                    let mode = mode(&m.display_name)?;
                    // SAFETY: mode() returns a display DEVMODEW with its display position union field initialized.
                    let position = unsafe { mode.Anonymous1.Anonymous2.dmPosition };
                    Ok((
                        m.device_id.clone(),
                        mode.dmPelsWidth,
                        mode.dmPelsHeight,
                        mode.dmDisplayFrequency,
                        position.x,
                        position.y,
                    ))
                })
                .collect()
        };
        let before = timings()?;
        // What Windows does when a saved layout leaves the new display off.
        let active = Topology::query()?;
        let off: Vec<_> = active
            .paths
            .iter()
            .filter(|p| {
                !(p.targetInfo.adapterId == owned.adapter && p.targetInfo.id == owned.target)
            })
            .copied()
            .collect();
        // SAFETY: The retained paths and queried modes remain valid for this synchronous display configuration call.
        check(unsafe {
            SetDisplayConfig(
                Some(&off),
                Some(&active.modes),
                SDC_APPLY | SDC_USE_SUPPLIED_DISPLAY_CONFIG | SDC_ALLOW_CHANGES,
            )
        })?;
        anyhow::ensure!(
            !monitors()?.iter().any(ours),
            "virtual display stayed on after switching it off"
        );
        let recovered = display.current_monitor()?;
        assert!(display.owns_monitor(&recovered));
        assert!(display.switched_on);
        assert_eq!(display.generation, 0);
        assert_eq!(timings()?, before);
        // Exercise the established stream's heartbeat separately from startup.
        // SAFETY: The same retained paths and queried modes remain valid here.
        check(unsafe {
            SetDisplayConfig(
                Some(&off),
                Some(&active.modes),
                SDC_APPLY | SDC_USE_SUPPLIED_DISPLAY_CONFIG | SDC_ALLOW_CHANGES,
            )
        })?;
        display.last_feed = Instant::now() - Duration::from_secs(2);
        display.feed()?;
        anyhow::ensure!(monitors()?.iter().any(ours), "virtual display stayed off");
        assert_eq!(display.generation, 1);
        assert!(display.owns_monitor(&owned));
        assert_eq!(timings()?, before);
        println!(
            "switched on {} beside {} displays",
            display.name,
            before.len()
        );
        Ok(())
    }
    #[test]
    #[ignore = "creates and expires owned 4K virtual displays and changes their mode and HDR"]
    fn native_virtual_display_recovery_restores_the_requested_mode_and_mode_list() -> Result<()> {
        let _ = tracing_subscriber::fmt()
            .with_test_writer()
            .with_ansi(false)
            .try_init();
        let before = Snapshot::capture()?;
        let result = (|| -> Result<()> {
            for rate in [116_000, 1_000_000] {
                let mut display = VirtualDisplay::create_rate(
                    &format!("mode-recovery-test-{}-{rate}", std::process::id()),
                    3840,
                    2160,
                    rate,
                )
                .with_context(|| format!("create 4K{}", rate / 1000))?;
                display
                    .finish_hotplug("test startup")
                    .with_context(|| format!("settle 4K{}", rate / 1000))?;
                let supported = supported_modes(&display.name);
                anyhow::ensure!(supported.contains(&(3840, 2160, rate / 1000)));
                anyhow::ensure!(supported.contains(&(1920, 1080, 60)));
                // The driver's EDID prefers 4K60; creation must set the
                // stream's rate rather than leave Windows' choice.
                let created = current_timing(&display.name)?;
                anyhow::ensure!(
                    timing_matches((3840, 2160, rate), created),
                    "created at {}",
                    format_timing(created)
                );
                Topology::set_mode_rate(
                    &display.name,
                    2560,
                    1440,
                    butterpollo_core::framegen::Rate(60_000),
                )?;
                // A healthy heartbeat must allow a game's own mode change.
                display.last_feed = Instant::now() - Duration::from_secs(2);
                display.feed()?;
                anyhow::ensure!(mode(&display.name)?.dmPelsWidth == 2560);
                anyhow::ensure!(display.generation == 0);

                let mut request = NAMESPACE.to_vec();
                request.extend_from_slice(&display.lease.to_le_bytes());
                request.extend_from_slice(&0u64.to_le_bytes());
                display.driver.ioctl(0x904, 3, &request, 0)?;
                let deadline = Instant::now() + Duration::from_secs(20);
                while display.generation == 0 {
                    let recovered = display.feed();
                    if Instant::now() >= deadline {
                        recovered?;
                        bail!("owned virtual display did not recover");
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                let owned = display.hotplug_monitor()?.clone();
                set_hdr(&owned, true)?;
                display
                    .finish_hotplug("test recovery after HDR")
                    .with_context(|| format!("finish recovered 4K{}", rate / 1000))?;
                let actual = current_timing(&display.name)?;
                anyhow::ensure!(
                    timing_matches((3840, 2160, rate), actual),
                    "recovered at {}",
                    format_timing(actual)
                );
                let recovered = supported_modes(&display.name);
                anyhow::ensure!(
                    supported.iter().all(|mode| recovered.contains(mode)),
                    "recovery restricted the mode list"
                );
                println!(
                    "recovered {} at 3840x2160@{} with {} modes after a 2560x1440@60 recall",
                    display.name,
                    rate / 1000,
                    recovered.len()
                );
                drop(display);
                let deadline = Instant::now() + Duration::from_secs(10);
                while Topology::query_all()?.paths.iter().any(|path| {
                    path.targetInfo.adapterId == owned.adapter
                        && path.targetInfo.id == owned.target
                        && path.targetInfo.targetAvailable.as_bool()
                }) {
                    anyhow::ensure!(Instant::now() < deadline, "owned display did not depart");
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
            Ok(())
        })();
        before.restore()?;
        result?;
        anyhow::ensure!(
            serde_json::to_value(Snapshot::capture()?)? == serde_json::to_value(before)?,
            "the pre-test layout was not restored"
        );
        Ok(())
    }
    #[test]
    fn legacy_and_secure_driver_requests_keep_identity_and_capability() -> Result<()> {
        let mut version = NAMESPACE.to_vec();
        version.extend_from_slice(&[3, 0, 5, 0, 0, 0, 0, 0]);
        assert_eq!(DriverProtocol::parse(&version)?, DriverProtocol::Legacy35);
        let options = VirtualOptions::default();
        let legacy = temporary_request(
            17,
            23,
            (1968, 2184, 120000),
            &options,
            DriverProtocol::Legacy35,
            &[42; 32],
        );
        assert_eq!(legacy.len(), 96);
        assert_eq!(&legacy[88..96], &[0; 8]);
        assert_eq!(DriverProtocol::Legacy35.create_function(), 0x901);
        version[18] = 6;
        assert_eq!(DriverProtocol::parse(&version)?, DriverProtocol::Secure36);
        let first = temporary_request(
            17,
            23,
            (1968, 2184, 120000),
            &options,
            DriverProtocol::Secure36,
            &[42; 32],
        );
        let recovered = temporary_request(
            17,
            23,
            (1968, 2184, 120000),
            &options,
            DriverProtocol::Secure36,
            &[42; 32],
        );
        assert_eq!(&first[..88], &legacy[..88]);
        assert_eq!(&first[96..], &[42; 32]);
        assert_eq!(first, recovered);
        version[18] = 4;
        assert!(DriverProtocol::parse(&version).is_err());
        version[16] = 4;
        assert!(DriverProtocol::parse(&version).is_err());
        Ok(())
    }
    #[test]
    fn a_vrr_display_requests_1000_hz_in_the_drivers_millihertz_field() {
        // libvirtualdisplay's CreateTemporaryDisplayRequest: physical size in
        // millimetres at 40/44, refresh_rate_millihz at 48 (no rational and
        // no VRR flag), lease timeout in ms at 52, flags at 88.
        let field =
            |request: &[u8], at: usize| u32::from_le_bytes(request[at..at + 4].try_into().unwrap());
        for (rate, hz) in [(1_000_000, 1000), (116_000, 116), (59_940, 59)] {
            let request = temporary_request(
                1,
                2,
                (3840, 2160, rate),
                &VirtualOptions::default(),
                DriverProtocol::Secure36,
                &[0; 32],
            );
            assert_eq!(
                [32, 36, 40, 44, 48, 52].map(|at| field(&request, at)),
                [3840, 2160, 600, 340, rate, 10000]
            );
            assert_eq!(field(&request, 88), 0);
            assert_eq!(field(&request, 48) / 1000, hz);
        }
        // Windows takes the mode as a reduced rational.
        assert_eq!(
            butterpollo_core::framegen::Rate(1_000_000).rational(),
            (1000, 1)
        );
        assert_eq!(
            butterpollo_core::framegen::Rate(116_000).rational(),
            (116, 1)
        );
        assert_eq!(
            butterpollo_core::framegen::Rate(59_940).rational(),
            (2997, 50)
        );
    }
    #[test]
    fn a_selected_gpu_renders_the_virtual_display_and_automatic_leaves_it_to_windows() {
        // Issue #28: an Intel iGPU rendered the virtual display while the
        // selected Radeon R7 M370 was asked to duplicate it.
        assert!(render_adapter_call(&VirtualOptions::default()).is_none());
        use butterpollo_core::config::Config;
        assert_eq!(configured_render_adapter(&Config::default()), None);
        // A GPU that is gone leaves the choice to Windows, never another GPU.
        let missing = Config::parse("adapter_name = No Such GPU\n").unwrap();
        assert_eq!(configured_render_adapter(&missing), None);
        let options = VirtualOptions {
            render_adapter: Some(RenderAdapter {
                name: "AMD Radeon R7 M370".into(),
                luid: (0x0001_2345, -2),
            }),
            ..Default::default()
        };
        let (function, access, request) = render_adapter_call(&options).unwrap();
        // libvirtualdisplay's kIoctlSetRenderAdapter.
        assert_eq!((0x22 << 16) | (access << 14) | (function << 2), 0x0022_A42C);
        // SetRenderAdapterRequest: namespace, LUID low and high, flags, reserved.
        assert_eq!(request.len(), 32);
        assert_eq!(&request[..16], &NAMESPACE);
        assert_eq!(&request[16..20], &0x0001_2345u32.to_le_bytes());
        assert_eq!(&request[20..24], &(-2i32).to_le_bytes());
        assert_eq!(&request[24..], &[0; 8]);
    }
    #[test]
    fn permanent_monitor_payload_matches_driver_v3_contract() {
        let request = permanent_request(4).unwrap();
        assert_eq!(request.len(), 76); // SDK PermanentDisplayCountRequest.
        assert_eq!(&request[..16], &NAMESPACE);
        assert_eq!(&request[16..20], &[4, 0, 0, 0]);
        assert_eq!(&request[24..32], &[128, 7, 0, 0, 56, 4, 0, 0]); // 1920 x 1080.
        assert_eq!(&request[40..44], &[96, 234, 0, 0]); // 60000 millihertz.
        assert!(permanent_request(5).is_err());
        let mut response = NAMESPACE.to_vec();
        response.extend_from_slice(&[4, 0, 0, 0, 4, 0, 0, 0]);
        response.resize(80, 0); // SDK PermanentDisplayCountResult.
        assert_eq!(permanent_response(&response).unwrap(), 4);
        response[16] = 5;
        assert!(permanent_response(&response).is_err());
        response[0] ^= 1;
        assert!(permanent_response(&response).is_err());
    }
}
