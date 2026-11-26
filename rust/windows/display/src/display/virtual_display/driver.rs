//! The display driver as the host sees it: whether Windows and the driver can
//! serve virtual displays, the status the console shows, the render adapter
//! displays are created on, and the driver's lease hooks.
use super::*;

/// The Windows build number from the registry, or None when it cannot be read
/// (callers then treat the build as capable; see `butterpollo_media_policy::os_gate`).
pub fn windows_build() -> Option<u32> {
    static BUILD: std::sync::OnceLock<Option<u32>> = std::sync::OnceLock::new();
    *BUILD
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
}
/// Whether this Windows ships the IddCx the virtual display driver requires
/// (1.10, Windows 11 23H2 and later). Below that the host defaults to the
/// physical display, as Vibepollo does on Windows 10, because the driver
/// cannot bind. The first call logs why, once.
pub fn virtual_display_os() -> bool {
    use butterpollo_media_policy::os_gate::{Feature, missing, virtual_display};
    static LOGGED: std::sync::Once = std::sync::Once::new();
    let build = windows_build();
    if let Some(reason) = missing(build, Feature::VirtualDisplay) {
        LOGGED.call_once(|| tracing::info!("{reason}"));
    }
    virtual_display(build)
}
pub fn virtual_display_available() -> bool {
    Control::open().is_ok()
}
/// Whether the driver counts as there for the service's repair watch: its
/// interface opens, or its device is installed and switched off, which the user
/// may have done on purpose.
pub fn virtual_display_installed() -> bool {
    virtual_display_available() || super::driver_recovery::disabled()
}
/// Launch/resume may repair a confirmed failed devnode before choosing its display.
/// Console and server-info inspection must keep using the read-only availability query.
pub fn ensure_virtual_display_available() -> bool {
    Control::open_recovering().is_ok()
}
pub fn virtual_display_status() -> serde_json::Value {
    match Control::open() {
        Ok(control) => serde_json::json!({
            "capable": true,
            "ready": true,
            "reason": "",
            "protocol": "rubylight",
            "driver_version": format!(
                "{}.{}.{}",
                control.info.driver_version >> 16,
                (control.info.driver_version >> 8) & 0xff,
                control.info.driver_version & 0xff
            ),
        }),
        Err(error) => {
            serde_json::json!({"capable":false,"ready":false,"reason":driver_problem(&error),"protocol":"rubylight"})
        }
    }
}
/// What the console says when the driver cannot be opened.
fn driver_problem(error: &anyhow::Error) -> String {
    let recorded = crate::desktop::registry_string(
        windows::Win32::System::Registry::HKEY_LOCAL_MACHINE,
        "SOFTWARE\\Rubylight\\VirtualDisplay",
        "Problem",
    );
    problem_text(error, recorded)
}
/// Why the driver is missing as setup recorded it (a Smart App Control, S mode or App Control
/// policy refusing the driver signed on the PC, or a failed install) comes first: it says what
/// the user can do. Otherwise the error explains it.
pub(super) fn problem_text(error: &anyhow::Error, recorded: Option<String>) -> String {
    use windows::Win32::Foundation::{ERROR_ACCESS_DENIED, ERROR_NO_MORE_ITEMS};
    if let Some(recorded) = recorded.filter(|text| !text.trim().is_empty()) {
        return recorded;
    }
    let code = error
        .downcast_ref::<windows::core::Error>()
        .map(|e| e.code());
    if code == Some(ERROR_NO_MORE_ITEMS.to_hresult()) {
        "Rubylight's display driver is not installed, or Windows must restart to finish installing it. The Rubylight service sets it up again a minute after it finds it missing, unless a stream is running; to do it now, run \"butterpollo-setup.exe --repair-drivers --now\" from the Rubylight folder in an administrator shell.".into()
    } else if code == Some(ERROR_ACCESS_DENIED.to_hresult()) {
        "Rubylight's display driver refused this process. Install Rubylight with its setup and let the service run the host.".into()
    } else {
        format!("{error:#}")
    }
}
#[derive(Clone, Debug)]
pub struct VirtualOptions {
    /// The peak brightness the display's EDID advertises for HDR.
    pub peak_nits: u32,
    /// The GPU to render the display on; None leaves the choice to Windows.
    pub render_adapter: Option<RenderAdapter>,
    /// Finds `render_adapter` again after a driver restart or a GPU reset.
    pub render_resolver: Option<RenderResolver>,
    /// The stream joins a running game's display and takes it as it is
    /// ([`super::joinable_mode`]).
    pub joins: bool,
}
impl Default for VirtualOptions {
    fn default() -> Self {
        Self {
            peak_nits: 1000,
            render_adapter: None,
            render_resolver: None,
            joins: false,
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
/// as Apollo and Vibepollo do. Automatic leaves the choice to Windows on a PC
/// with one GPU; with several it is the AMD or NVIDIA GPU the stream pins to
/// (discrete over integrated, then the most video memory), which the probe
/// already counts on. `select` resolves it as `idd-push` capture does.
pub fn configured_render_adapter_with(
    config: &butterpollo_config::config::Config,
    select: impl FnOnce(&str, &str) -> Result<Option<RenderAdapter>>,
) -> Option<RenderAdapter> {
    select(
        config.get("adapter_name", ""),
        config.get("adapter_pnp_id", ""),
    )
    .inspect_err(|error| {
        tracing::warn!(error = %format!("{error:#}"), "the selected GPU was not found; Windows picks the GPU that renders the virtual display")
    })
    .ok()
    .flatten()
}
/// Finds the GPU that renders the driver's displays again from the Graphics
/// card setting it was resolved from: after a driver restart or a GPU reset
/// Windows may list that GPU under another LUID, and the frozen one would
/// name no GPU.
#[derive(Clone)]
pub struct RenderResolver(std::sync::Arc<Resolve>);
type Resolve = dyn Fn() -> Result<Option<RenderAdapter>> + Send + Sync;
impl RenderResolver {
    pub fn new(
        resolve: impl Fn() -> Result<Option<RenderAdapter>> + Send + Sync + 'static,
    ) -> Self {
        Self(std::sync::Arc::new(resolve))
    }
}
impl std::fmt::Debug for RenderResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RenderResolver")
    }
}
/// The render adapter a display asks for after a driver restart or a GPU
/// reset: the one `resolver` finds now, under its live LUID, never the frozen
/// one. A chosen GPU that is missing leaves none and is logged (the resolver
/// sets the console warning): Rubylight never moves the display to another
/// GPU on its own, and capture then fails with the same reason. Without a
/// resolver the previous adapter stays.
pub(super) fn reresolve_render_adapter(
    previous: Option<RenderAdapter>,
    resolver: Option<&RenderResolver>,
) -> Option<RenderAdapter> {
    let Some(resolver) = resolver else {
        return previous;
    };
    match (resolver.0)() {
        Ok(adapter) => {
            if let (Some(old), Some(new)) = (&previous, &adapter)
                && old.luid != new.luid
            {
                tracing::info!(gpu = %new.name, old = ?old.luid, new = ?new.luid, "the virtual display's GPU is listed under a new LUID");
            }
            adapter
        }
        Err(error) => {
            tracing::warn!(error = %format!("{error:#}"), "the virtual display's GPU is missing; Windows picks the GPU that renders it until the chosen one returns");
            None
        }
    }
}
/// Best effort: a driver that refuses leaves the display on the GPU Windows picks, and
/// capture then says it encodes there.
pub(super) fn apply_render_adapter(control: &Control, options: &VirtualOptions) {
    let Some(adapter) = &options.render_adapter else {
        return;
    };
    match retry_busy(
        || control.set_render_adapter(adapter.luid),
        Duration::from_secs(1),
    ) {
        Ok(()) => {
            tracing::info!(gpu = %adapter.name, "virtual display renders on the selected GPU")
        }
        Err(error) => {
            tracing::warn!(gpu = %adapter.name, error = %format!("{error:#}"), "the display driver did not accept the selected GPU; Windows picks the GPU that renders the virtual display")
        }
    }
}
/// A monitor belongs to this process and departs when it stops sending heartbeats; the
/// display id is the driver's session id.
impl DisplayProvider for Control {
    fn renew(&self, id: u64) -> Result<()> {
        self.drain_log();
        self.heartbeat(id)
    }
    fn offer_mode(&self, id: u64, mode: Mode) {
        let mode = (mode.width, mode.height, mode.refresh_mhz);
        if let Err(error) = self.update_modes(id, mode) {
            tracing::warn!(error = %format!("{error:#}"), mode = %format_timing(mode), "the display driver did not offer the mode");
        }
    }
    fn remove(&self, id: u64) {
        if let Err(e) = lease_exists(retry_busy(
            || Control::remove(self, id),
            Duration::from_secs(1),
        )) {
            tracing::warn!(error=%e,"virtual display removal failed; the driver departs it when this host stops");
        }
        // Do not unregister by bare session id here: a concurrent reconnect for the same
        // session registers its own `registration` token, which `VirtualDisplay`
        // releases via `unregister_registration` instead.
        self.drain_log();
    }
    fn pushes_frames(&self) -> bool {
        true
    }
}
