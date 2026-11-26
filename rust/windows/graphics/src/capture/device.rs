//! Capture D3D11 [`Device`] wrapper, output colour-space snapshot, and
//! hybrid-GPU Desktop Duplication adapter-mismatch recovery.

use super::{
    Display, adapter_pnp_id, displays, gpus, hardware_adapter, headless_candidate, output,
    preference_orders, probe_skips_display, select_display,
};
use crate::text::from_wide;
use anyhow::{Context, Result, bail};
use butterpollo_media_policy::encoder_gpu;
use windows::{
    Win32::{
        Foundation::HMODULE,
        Graphics::{
            Direct3D::D3D_DRIVER_TYPE_UNKNOWN,
            Direct3D11::*,
            Dxgi::{Common::*, *},
        },
    },
    core::Interface,
};

#[derive(Clone)]
pub struct Device {
    pub device: ID3D11Device,
    pub context: ID3D11DeviceContext,
    /// The monitor the device was opened for; absent on a host without one,
    /// where the device can still encode.
    pub output: Option<IDXGIOutput1>,
    pub display: std::sync::Arc<Display>,
    /// The PCI vendor of the device's adapter. Streams encode only on AMD and NVIDIA.
    pub vendor: u32,
    // Capture and all encoder clones read one watched colour snapshot.
    color: Option<std::sync::Arc<output::Color>>,
}
impl Device {
    /// Forget the display's colour, so a test picture counts as an HDR desktop's.
    #[cfg(test)]
    pub(crate) fn forget_color(&mut self) {
        self.color = None;
    }
    /// The D3D11 device's identity, to tell a recreated device from this one.
    pub fn key(&self) -> usize {
        self.device.as_raw() as usize
    }
    /// The monitor this device captures.
    pub fn output(&self) -> Result<&IDXGIOutput1> {
        self.output
            .as_ref()
            .context("no monitor is attached to this GPU")
    }
    /// Read the selected output's luminance; conversion produces Rec.2020/D65.
    pub fn hdr_metadata(&self) -> butterpollo_media_policy::hdr::Metadata {
        self.color
            .as_ref()
            .map(|color| color.read().1)
            .unwrap_or_default()
    }
    /// Read the selected output's Windows "SDR content brightness" in nits
    /// (`DISPLAYCONFIG_SDR_WHITE_LEVEL`), defaulting to `100.0` when unavailable.
    pub fn sdr_white_nits(&self) -> f32 {
        self.color.as_ref().map_or(100.0, |color| color.read().3)
    }
    /// Whether Windows composes the captured output as HDR10 (BT.2020 PQ) right now. The pixel
    /// format of a frame does not say: Desktop Duplication hands over FP16 for an SDR output
    /// under advanced colour or beside a virtual display, and WGC's pool is FP16 whenever HDR
    /// was requested. `None` when the output's colour space cannot be read.
    pub fn display_in_hdr(&self) -> Option<bool> {
        output::in_hdr(self.color_description().ok().flatten())
    }
    /// Raw DXGI colour space (`DXGI_OUTPUT_DESC1::ColorSpace`) from the latest check.
    pub fn color_space(&self) -> Option<DXGI_COLOR_SPACE_TYPE> {
        self.color_description()
            .ok()
            .flatten()
            .map(|desc| desc.color_space)
    }
    pub(super) fn color_is_current(&self) -> bool {
        self.color.as_ref().is_none_or(|color| color.is_current())
    }
    pub(super) fn refresh_color(&self) {
        if let Some(color) = &self.color {
            let _ = color.refresh_now();
        }
    }
    pub(super) fn color_description(&self) -> Result<Option<output::Description>> {
        self.color
            .as_ref()
            .map_or(Ok(None), |color| color.read().0.map_err(anyhow::Error::msg))
    }
    /// [`Self::new_adapter`] with no GPU chosen. With no display, or one on an
    /// Intel or software adapter, that is the GPU a stream would encode on,
    /// which on a PC with AMD and NVIDIA cards is the one with the most video
    /// memory: the AMF tests and examples that open `Device::new("")` then get
    /// an NVIDIA device there and fail in AMF.
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
            // With no GPU chosen, a monitor on an Intel card must not hide an
            // AMD or NVIDIA card that can encode. The stream then captures and
            // encodes on that card too (`stream_adapter`), so the probe
            // promises what it delivers.
            if gpus().is_ok_and(|all| {
                let vendors: Vec<u32> = all.iter().map(|gpu| gpu.vendor).collect();
                probe_skips_display(
                    desc.VendorId,
                    !adapter_name.is_empty() || !pnp_id.is_empty(),
                    &vendors,
                )
            }) {
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
    /// Capture's device for the named display on the GPU with `luid`, as
    /// `idd-push` opens it on the GPU that renders Rubylight's virtual display
    /// whichever adapter Windows lists the display under.
    pub fn for_display_on(name: &str, luid: (u32, i32)) -> Result<Self> {
        let choices = displays()?;
        let mut display = select_display(&choices, name)?
            .cloned()
            .with_context(|| format!("{name} is not on the desktop"))?;
        // SAFETY: The factory, adapters and output are live COM objects returned by these
        // calls, and GetDesc1 returns an owned struct.
        unsafe {
            let factory: IDXGIFactory4 = CreateDXGIFactory1()?;
            let output = Some(
                factory
                    .EnumAdapters1(display.adapter_index)?
                    .EnumOutputs(display.output_index)?
                    .cast()?,
            );
            let adapter: IDXGIAdapter1 = factory
                .EnumAdapterByLuid(windows::Win32::Foundation::LUID {
                    LowPart: luid.0,
                    HighPart: luid.1,
                })
                .with_context(|| format!("the GPU {:08x}:{:08x} is not listed", luid.1, luid.0))?;
            display.adapter = from_wide(&adapter.GetDesc1()?.Description);
            Self::create(&adapter, output, display)
        }
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
    /// A device on the configured GPU, or with none chosen the one a stream
    /// would encode on ([`encoder_gpu::headless`]), when no monitor gives one:
    /// the host can still probe its encoders, as Vibepollo does on headless
    /// hosts, and reports the codecs of the GPU that encodes.
    pub(super) fn without_display(adapter_name: &str, pnp_id: &str) -> Result<Self> {
        // SAFETY: The factory and adapters are live COM objects returned by these calls, and
        // GetDesc1 returns an owned struct.
        unsafe {
            let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
            let mut candidates = Vec::new();
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
                candidates.push((index, adapter, desc, description));
            }
            if candidates.is_empty() {
                if !adapter_name.is_empty() || !pnp_id.is_empty() {
                    bail!("the configured GPU adapter was not found: {adapter_name} {pnp_id}");
                }
                bail!("{}", encoder_gpu::Refusal::NoGpu);
            }
            // With no GPU chosen, the adapter a stream pins to (on a PC with
            // several GPUs the discrete AMD or NVIDIA one), else AMD first,
            // then NVIDIA. Neither leaves the first one, which the encoder
            // refuses.
            let descs: Vec<DXGI_ADAPTER_DESC1> = candidates.iter().map(|c| c.2).collect();
            let orders = if descs.len() > 1 {
                preference_orders()
            } else {
                Vec::new()
            };
            let (index, adapter, _, description) =
                candidates.remove(headless_candidate(&descs, &orders));
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
            Self::create(&adapter, None, display)
        }
    }
    fn create(
        adapter: &IDXGIAdapter1,
        output: Option<IDXGIOutput1>,
        display: Display,
    ) -> Result<Self> {
        // SAFETY: `adapter` is a live DXGI adapter, the out-parameters are live locals, and they
        // are unwrapped only after D3D11CreateDevice succeeded.
        unsafe {
            let mut device = None;
            let mut context = None;
            D3D11CreateDevice(
                adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )?;
            let context = context.unwrap();
            let multithread: ID3D11Multithread = context.cast()?;
            let _ = multithread.SetMultithreadProtected(true);
            let color = output
                .as_ref()
                .map(|output| output::Color::open(output, &display.display_name))
                .transpose()?
                .map(|color| {
                    let color = std::sync::Arc::new(color);
                    color.publish();
                    color
                });
            Ok(Self {
                device: device.unwrap(),
                context,
                output,
                display: std::sync::Arc::new(display),
                vendor: adapter.GetDesc1()?.VendorId,
                color,
            })
        }
    }
}
/// A capture device opened on another GPU than the one that renders its
/// display, as the Graphics card setting can ask on a hybrid PC.
#[derive(Debug)]
pub(super) struct GpuMismatch {
    pub(super) display: String,
    pub(super) owner: String,
    pub(super) owner_vendor: u32,
    pub(super) selected: String,
}
impl GpuMismatch {
    /// None when the device is on the display's GPU, or the GPUs cannot be
    /// compared.
    pub(super) fn of(gpu: &Device) -> Option<Self> {
        let output = gpu.output.as_ref()?;
        // SAFETY: The device and output are live COM objects; GetAdapter, GetParent and GetDesc
        // return owned values.
        let (device, owner) = unsafe {
            (
                gpu.device
                    .cast::<IDXGIDevice>()
                    .and_then(|d| d.GetAdapter())
                    .and_then(|a| a.GetDesc())
                    .ok()?,
                output
                    .GetParent::<IDXGIAdapter>()
                    .and_then(|a| a.GetDesc())
                    .ok()?,
            )
        };
        ((device.AdapterLuid.LowPart, device.AdapterLuid.HighPart)
            != (owner.AdapterLuid.LowPart, owner.AdapterLuid.HighPart))
            .then(|| Self {
                display: gpu.display.display_name.clone(),
                owner: from_wide(&owner.Description),
                owner_vendor: owner.VendorId,
                selected: from_wide(&device.Description),
            })
    }
    pub(super) fn refusal(&self) -> String {
        format!(
            "Desktop Duplication captures {} only on the GPU it is connected to ({}), not on {}; use WGC capture to encode on another GPU",
            self.display, self.owner, self.selected
        )
    }
    fn fallback(&self) -> String {
        format!(
            "Capturing and encoding on {owner}, not on the selected {selected}: Desktop Duplication captures {display} only on {owner}, the GPU that renders it. WGC capture can encode on another GPU; otherwise select Automatic or {owner} under Encoders > Graphics card.",
            owner = self.owner,
            selected = self.selected,
            display = self.display
        )
    }
}
/// Desktop Duplication duplicates a display only on the GPU that renders it.
/// A device on the selected GPU is replaced by one on that GPU: the encoder
/// follows the captured frames, so the stream encodes there and says so
/// instead of ending without a picture. Rubylight encodes only on AMD and
/// NVIDIA, so a display driven by another vendor's GPU fails with the refusal,
/// which suggests WGC capture, instead of moving there.
pub(super) fn duplication_device<D>(
    selected: D,
    mismatch: impl FnOnce(&D) -> Option<GpuMismatch>,
    owner: impl FnOnce() -> Result<D>,
    warnings: &dyn butterpollo_media_policy::WarningSink,
) -> Result<D> {
    let Some(mismatch) = mismatch(&selected) else {
        return Ok(selected);
    };
    drop(selected);
    if !encoder_gpu::encodes(mismatch.owner_vendor) {
        bail!("{}", mismatch.refusal());
    }
    let device = owner().with_context(|| mismatch.refusal())?;
    let message = mismatch.fallback();
    tracing::warn!("{message}");
    warnings.set("capture_adapter", &message);
    Ok(device)
}
