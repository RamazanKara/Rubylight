//! Video decoder hardware selection, device reference lifetime management, pool sizing, and
//! platform backend ordering.

use std::ffi::c_void;

#[cfg(windows)]
use super::d3d11::ProbeVideoDevice;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    H264,
    Hevc,
    Av1,
}

/// What a stream's pictures are, for a backend that checks it can decode and hand them on before
/// it opens (Vulkan video): their depth and chroma, and whether they carry packed 4:4:4 chroma.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pictures {
    pub ten_bit: bool,
    pub chroma444: bool,
    pub packed: bool,
    /// How many pictures the stream may keep waiting to be shown (`pipeline::pool_waiting`): a
    /// hardware pool gets these and three more beyond FFmpeg's minimum ([`pool_extra`]).
    pub waiting: usize,
    /// Where VAAPI opens. Linux only.
    pub vaapi_node: VaapiNode,
    /// The size the stream was negotiated for (`setup.width`/`height`), given to a V4L2 M2M
    /// decoder before it opens: FFmpeg copies it into its queues, which with no size counts the
    /// driver's first format event as a resolution change and reinitialises the buffers there
    /// (Nightjar issue 77 on Qualcomm Iris). 0 leaves the size to the driver, as before.
    pub width: u32,
    pub height: u32,
    /// Decoder probe (`color_probe`) that already created its own `D3D11_CREATE_DEVICE_VIDEO_SUPPORT`
    /// device and never presents decoded surfaces to SDL's renderer.
    pub probe_only: bool,
}

/// The device VAAPI opens on (`Pictures::vaapi_node`, [`vaapi_node`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub enum VaapiNode {
    /// FFmpeg's default device, the first render node.
    #[default]
    Default,
    /// The GPU renderer's DRM render node (`/dev/dri/renderD<minor>`) when its device can take
    /// VAAPI's pictures mapped (`crate::vulkan_video::VaapiMap`): VAAPI decodes on its GPU, and
    /// its pool gets the surfaces the presenter keeps for its copies.
    Mapped(u32),
    /// The render node of the GPU the OpenGL renderer's EGL display runs on, when that is not the
    /// default device (`crate::dmabuf::display_vaapi_node`): its EGL imports the pictures VAAPI
    /// decodes there, and refuses another GPU's tiled ones.
    Display(u32),
}

impl VaapiNode {
    /// The render node's minor; none for the default device.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))] // VAAPI is Unix's; Windows reads it in tests only.
    pub fn minor(self) -> Option<u32> {
        match self {
            Self::Default => None,
            Self::Mapped(minor) | Self::Display(minor) => Some(minor),
        }
    }
}

/// Guard holding a dedicated `D3D11_CREATE_DEVICE_VIDEO_SUPPORT` probe device on Windows so
/// off-thread probes never touch SDL3's immediate context.
pub struct ProbeHardware {
    pub hardware: Hardware,
    #[cfg(windows)]
    _device: Option<ProbeVideoDevice>,
}

impl ProbeHardware {
    pub fn isolate(hardware: Hardware) -> Self {
        #[cfg(windows)]
        if let Hardware::D3d11(raw) = hardware {
            match ProbeVideoDevice::for_renderer(raw) {
                Ok(dev) => return Self { hardware: Hardware::D3d11(dev.as_raw()), _device: Some(dev) },
                Err(why) => eprintln!("nightjar: decoder probe: could not open a dedicated D3D11 video device ({why}); probing on the renderer's device"),
            }
        }
        Self { hardware, #[cfg(windows)] _device: None }
    }
}

/// A reference held on the renderer's Direct3D 11 device ([`Hardware::D3d11`]'s, and PyroWave's
/// GPU decoder's), so the device outlives the renderer's use of it while a launch, a decoder or a
/// benchmark still uses it: SDL makes its Direct3D 11 device again after a driver reset, and a
/// decoder opened on the old pointer would use freed memory. Elsewhere it holds nothing.
#[derive(Clone, Default)]
pub struct DeviceRef(#[cfg(windows)] Option<windows::Win32::Graphics::Direct3D11::ID3D11Device>);

impl DeviceRef {
    /// Takes a reference on `hardware`'s device, read from the live renderer on the main thread.
    pub fn of(hardware: Hardware) -> Self {
        match hardware {
            Hardware::D3d11(raw) => Self::of_device(raw),
            _ => Self::default(),
        }
    }

    /// Takes a reference on the Direct3D 11 device at `raw`, just read from the live renderer on
    /// the main thread; none for null.
    pub fn of_device(raw: *mut c_void) -> Self {
        #[cfg(windows)]
        {
            use windows::core::Interface;
            // SAFETY: `raw` is null or the live renderer's device, just read from it.
            Self(unsafe { windows::Win32::Graphics::Direct3D11::ID3D11Device::from_raw_borrowed(&raw) }.cloned())
        }
        #[cfg(not(windows))]
        {
            let _ = raw;
            Self::default()
        }
    }

    /// Why the held device was removed (a driver reset, a hang, a removed adapter), as Direct3D
    /// names it; None while it works or when none is held.
    pub fn removed_reason(&self) -> Option<String> {
        #[cfg(windows)]
        {
            // SAFETY: a device this holds a reference on; the call only reads its state.
            let removed = self.0.as_ref().and_then(|device| unsafe { device.GetDeviceRemovedReason() }.err());
            removed.map(|e| format!("{:#010X}: {}", e.code().0 as u32, e.message()))
        }
        #[cfg(not(windows))]
        None
    }

    /// Whether the held Direct3D 11 device is already in a removed or reset state.
    pub fn is_removed(&self) -> bool { self.removed_reason().is_some() }

    /// Whether this holds a Direct3D 11 device reference.
    #[cfg(windows)]
    pub fn is_some(&self) -> bool { self.0.is_some() }

    /// Has the held device destroy what its users already let go of. Direct3D defers that until
    /// its immediate context is flushed, and a swap chain kept that way stays on its window, which
    /// then takes no new one. A decoder sharing the device's immediate context made it
    /// multithread-protected (`open_for`), so the calls wait for the decoder's.
    #[cfg(windows)]
    pub fn flush_released(&self) {
        if let Some(device) = &self.0 {
            // SAFETY: a device this holds a reference on; its context is used on this thread alone
            // or serialized by the device's multithread protection.
            unsafe {
                if let Ok(context) = device.GetImmediateContext() {
                    context.ClearState();
                    context.Flush();
                }
            }
        }
    }

    /// Whether this holds the device at `raw` (null: whether it holds none). A held device is never
    /// freed, so a device made later cannot take its address. Always true off Windows.
    pub fn holds(&self, raw: *mut c_void) -> bool {
        #[cfg(windows)]
        {
            use windows::core::Interface;
            self.0.as_ref().map_or(std::ptr::null_mut(), Interface::as_raw) == raw
        }
        #[cfg(not(windows))]
        {
            let _ = raw;
            true
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hardware {
    None,
    D3d11(*mut c_void),
    /// The whole `order` where no single backend is given: on Linux with the GPU renderer's SDL GPU
    /// device for Vulkan video (null when the renderer is another kind) and the render node VAAPI
    /// opens on for the OpenGL renderer (`crate::dmabuf::renderer_vaapi_node`, none for the default
    /// device), and on Windows without a Direct3D 11 renderer.
    Auto(*mut c_void, Option<u32>),
    Vaapi,
    #[cfg(target_os = "macos")]
    VideoToolbox,
    /// FFmpeg's Vulkan video on the GPU renderer's own Vulkan device, this SDL GPU device (or
    /// null, which does not open), its pictures drawn without a copy to memory. Linux only.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    Vulkan(*mut c_void),
    /// NVDEC, with CUDA frames copied into the GPU renderer's Vulkan images (`map_to_vulkan`), else
    /// to memory for the presenter.
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    Cuda,
    /// The V4L2 memory-to-memory decoders of ARM handhelds and boards (H.264 and HEVC).
    V4l2m2m,
}

// The renderer keeps the device alive until the decoder has stopped; FFmpeg owns a COM reference.
unsafe impl Send for Hardware {}

/// The Debug settings "Vulkan video first" (`Settings::vulkan_video_first`) and "Vulkan video
/// recovery" (`Settings::vulkan_rfi`) of the stream being set up, both off by default; their
/// environment variables win over them for test runs.
static VULKAN_VIDEO_FIRST: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static VULKAN_RFI: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// The Debug setting "VideoToolbox recovery" (`Settings::videotoolbox_rfi`) of the stream being
/// set up, off by default; `NIGHTJAR_VIDEOTOOLBOX_RFI` wins over it.
static VIDEOTOOLBOX_RFI: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Takes the Vulkan video Debug settings of the stream about to start, before any decoder order
/// or capability is worked out for it.
pub fn use_vulkan_video_settings(first: bool, rfi: bool) {
    VULKAN_VIDEO_FIRST.store(first, std::sync::atomic::Ordering::Relaxed);
    VULKAN_RFI.store(rfi, std::sync::atomic::Ordering::Relaxed);
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn vulkan_video_first_setting() -> bool {
    VULKAN_VIDEO_FIRST.load(std::sync::atomic::Ordering::Relaxed)
}

/// The "Vulkan video recovery" setting ([`use_vulkan_video_settings`]).
pub fn vulkan_rfi_setting() -> bool {
    VULKAN_RFI.load(std::sync::atomic::Ordering::Relaxed)
}

/// Takes the "VideoToolbox recovery" setting of the stream about to start, before its decoder
/// capabilities are worked out.
pub fn use_videotoolbox_rfi_setting(rfi: bool) {
    VIDEOTOOLBOX_RFI.store(rfi, std::sync::atomic::Ordering::Relaxed);
}

/// The "VideoToolbox recovery" setting ([`use_videotoolbox_rfi_setting`]).
pub fn videotoolbox_rfi_setting() -> bool {
    VIDEOTOOLBOX_RFI.load(std::sync::atomic::Ordering::Relaxed)
}

impl Hardware {
    /// The backends to try, in turn, for this choice. On Linux that is moonlight-qt's and Moonlight
    /// V+'s order: the first-tier hwaccels (VAAPI, then Vulkan video for GPUs without VAAPI), then
    /// the hardware decoders outside FFmpeg's hwaccels (V4L2 M2M), then the second tier (NVDEC);
    /// software decoding comes after them all. VAAPI stays first with the GPU renderer's device
    /// too: its pictures are mapped into the renderer's Vulkan images without a copy, and it keeps
    /// reference frame invalidation. The Debug setting "Vulkan video first"
    /// ([`use_vulkan_video_settings`]) or `NIGHTJAR_VULKAN_VIDEO_GPU_FIRST=1` puts Vulkan video
    /// before VAAPI there, VAAPI next where it does not open or FFmpeg refuses it for the stream;
    /// `NIGHTJAR_VULKAN_VIDEO` puts Vulkan first whatever the renderer or leaves it out. A VA
    /// driver that wraps another API (`crate::linux_video::vaapi_plan`: nvidia-vaapi-driver, the
    /// VDPAU backend) leaves VAAPI out, so NVIDIA decodes with Vulkan video, then NVDEC. The order,
    /// the devices and why go to the log once.
    pub fn order(self) -> Vec<Self> {
        match self {
            Self::None => Vec::new(),
            #[cfg(target_os = "linux")]
            Self::Auto(gpu, _) => {
                use crate::linux_video;
                let vaapi = vaapi_device(vaapi_node(self).minor());
                let plan = linux_video::vaapi_plan(
                    vaapi.as_ref().map(|identity| identity.vendor.as_str()).map_err(String::as_str),
                    crate::stream::env_flag(linux_video::FORCE_VAAPI_ENVIRONMENT) == Some(true),
                );
                if !gpu.is_null() {
                    // SAFETY: The renderer keeps its device alive while streams and probes use it.
                    match unsafe { linux_video::vulkan_identity(gpu.cast()) } {
                        Ok(identity) => linux_video::note(&identity.line()),
                        Err(why) => linux_video::note(&format!("vulkan: the renderer's device is not described: {why}")),
                    }
                }
                let (order, why) = Self::linux_order(
                    gpu,
                    crate::vulkan_video::Choice::from_environment(),
                    crate::vulkan_video::gpu_first(
                        crate::stream::env_flag(crate::vulkan_video::GPU_FIRST_ENVIRONMENT),
                        vulkan_video_first_setting(),
                    ),
                    &plan,
                );
                let labels: Vec<_> = order.iter().map(|backend| backend.label()).collect();
                linux_video::note(&linux_video::order_line(&labels, &why));
                order
            }
            // Vulkan video is Linux's; the rest of the order is tried as before it.
            #[cfg(target_os = "macos")]
            Self::Auto(..) => vec![Self::VideoToolbox],
            #[cfg(not(any(target_os = "linux", target_os = "macos")))]
            Self::Auto(..) => vec![Self::Vaapi, Self::V4l2m2m, Self::Cuda],
            other => vec![other],
        }
    }

    /// The Linux order for the GPU renderer's device `gpu` (null for another renderer), what
    /// `NIGHTJAR_VULKAN_VIDEO` asks for, "Vulkan video first" and the VA driver's plan, and why.
    #[cfg(any(target_os = "linux", test))]
    pub(super) fn linux_order(
        gpu: *mut c_void,
        choice: crate::vulkan_video::Choice,
        gpu_first: bool,
        vaapi: &crate::linux_video::VaapiPlan,
    ) -> (Vec<Self>, String) {
        use crate::vulkan_video::Choice;
        let (mut order, why) = match choice {
            Choice::Default if gpu_first && !gpu.is_null() => {
                (vec![Self::Vulkan(gpu), Self::Vaapi, Self::V4l2m2m, Self::Cuda], "Vulkan video first, the Debug setting or NIGHTJAR_VULKAN_VIDEO_GPU_FIRST=1")
            }
            Choice::Default if gpu.is_null() => {
                (vec![Self::Vaapi, Self::Vulkan(gpu), Self::V4l2m2m, Self::Cuda], "VA-API first; Vulkan video needs the GPU renderer")
            }
            Choice::Default => (vec![Self::Vaapi, Self::Vulkan(gpu), Self::V4l2m2m, Self::Cuda], "VA-API first, its pictures mapped into the renderer's Vulkan images"),
            Choice::First => (vec![Self::Vulkan(gpu), Self::Vaapi, Self::V4l2m2m, Self::Cuda], "NIGHTJAR_VULKAN_VIDEO=1"),
            Choice::Off => (vec![Self::Vaapi, Self::V4l2m2m, Self::Cuda], "NIGHTJAR_VULKAN_VIDEO=0"),
        };
        if vaapi.skip {
            order.retain(|&backend| backend != Self::Vaapi);
        }
        let why = match (&vaapi.why, vaapi.skip) {
            (Some(note), true) => note.clone(),
            (Some(note), false) => format!("{why}; {note}"),
            (None, _) => why.to_owned(),
        };
        (order, why)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::None => "software",
            Self::D3d11(_) => "D3D11VA",
            Self::Auto(..) => "hardware",
            Self::Vaapi => "VAAPI",
            #[cfg(target_os = "macos")]
            Self::VideoToolbox => "VideoToolbox",
            Self::Vulkan(_) => "Vulkan",
            Self::Cuda => "NVDEC",
            Self::V4l2m2m => "V4L2 M2M",
        }
    }
}

/// The VA driver on render node `node` (FFmpeg's default device for none), or why VAAPI does not
/// open there: AMD and Intel GPUs open, NVIDIA's driver does not without a VA driver of its own
/// (Vulkan video is used then, `crate::vulkan_video`). Asked once per node; the `vaapi:` line
/// goes to the log then.
#[cfg(target_os = "linux")]
pub fn vaapi_device(node: Option<u32>) -> Result<crate::linux_video::VaapiIdentity, String> {
    type Known = Vec<(Option<u32>, Result<crate::linux_video::VaapiIdentity, String>)>;
    static KNOWN: std::sync::Mutex<Known> = std::sync::Mutex::new(Vec::new());
    let mut known = KNOWN.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, identity)) = known.iter().find(|(asked, _)| *asked == node) {
        return identity.clone();
    }
    let identity = crate::linux_video::vaapi_identity(node);
    match &identity {
        Ok(identity) => crate::linux_video::note(&identity.line()),
        Err(error) => {
            let node = node.map_or_else(|| "the default device".to_owned(), |minor| format!("renderD{minor}"));
            crate::linux_video::note(&format!("vaapi: does not open on {node} ({error})"));
        }
    }
    known.push((node, identity.clone()));
    identity
}

/// Opens the first backend of `order` that opens; `failed` hears about each one that did not, and
/// the error names them all.
pub(super) fn first_opened<T>(
    order: Vec<Hardware>,
    mut open: impl FnMut(Hardware) -> Result<T, String>,
    mut failed: impl FnMut(Hardware, &str),
) -> Result<T, String> {
    let mut errors = Vec::new();
    for backend in order {
        match open(backend) {
            Ok(opened) => return Ok(opened),
            Err(error) => {
                failed(backend, &error);
                errors.push(format!("{}: {error}", backend.label()));
            }
        }
    }
    if errors.is_empty() {
        return Err("no hardware decoder".into());
    }
    Err(errors.join("; "))
}

/// Surfaces a hardware decoder's pool has at least beyond FFmpeg's minimum, for pictures waiting
/// to be shown, the one on the screen, the one being drawn and one kept after a refused present.
pub const EXTRA_HW_FRAMES: i32 = 6;

/// Pictures holding pool surfaces outside the presenter's queue and the pacer: the shown one, the
/// drawn one and the one kept for the next present when a non-blocking present was refused
/// (`skipped_picture`).
pub const HELD_BEYOND_WAITING: usize = 3;

/// The pool's surfaces beyond FFmpeg's minimum for `waiting` pictures waiting to be shown, plus
/// the [`HELD_BEYOND_WAITING`] ones.
pub fn pool_extra(waiting: usize) -> i32 {
    (waiting.min(64) as i32 + HELD_BEYOND_WAITING as i32).max(EXTRA_HW_FRAMES)
}

/// The device VAAPI opens on for `hardware` (`Pictures::vaapi_node`): the GPU renderer's render
/// node when its device will take VAAPI's pictures mapped (`crate::vulkan_video::map_node`), the
/// OpenGL renderer's when it is not the default device (`Hardware::Auto`'s node), else the default
/// device. The probes and the stream ask the same, so they decode on one GPU; why not the
/// renderer's goes to the log.
pub fn vaapi_node(hardware: Hardware) -> VaapiNode {
    #[cfg(target_os = "linux")]
    if let Hardware::Auto(gpu, display) = hardware {
        if !gpu.is_null() {
            let switched_on = crate::vulkan_video::map_switched_on(std::env::var_os(crate::vulkan_video::MAP_ENVIRONMENT).as_deref());
            // SAFETY: The renderer keeps its device alive while streams and probes use it.
            match unsafe { crate::vulkan_video::map_node(gpu.cast(), switched_on) } {
                Ok(minor) => return VaapiNode::Mapped(minor),
                Err(why) => crate::vulkan_video::note(&format!("VAAPI opens on the default device: {why}")),
            }
        }
        return display.map_or(VaapiNode::Default, VaapiNode::Display);
    }
    let _ = hardware;
    VaapiNode::Default
}

/// The surfaces a VAAPI pool gets beyond [`pool_extra`] when its pictures will be mapped for the
/// GPU renderer (`VaapiNode::Mapped`): those the presenter keeps until their copies are done.
pub(super) fn mapped_extra(hardware: Hardware, pictures: Pictures) -> i32 {
    // Linux and macOS (unix): VAAPI mapped-surface pool reserve queries vulkan_video::mapped_held.
    #[cfg(unix)]
    if hardware == Hardware::Vaapi && matches!(pictures.vaapi_node, VaapiNode::Mapped(_)) {
        return crate::vulkan_video::mapped_held();
    }
    let _ = (hardware, pictures);
    0
}

/// The device path of DRM render node `minor`, for VAAPI.
// Linux and macOS (unix): DRM render node paths are constructed on Unix (and exercised in tests).
#[cfg(any(unix, test))]
pub(super) fn vaapi_node_path(minor: u32) -> std::ffi::CString {
    std::ffi::CString::new(format!("/dev/dri/renderD{minor}")).expect("no NUL in a path of digits")
}
