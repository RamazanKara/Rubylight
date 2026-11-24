//! PyroWave decoding on the GPU through the SDK (libpyrowave-shared-1.dll on Windows,
//! libpyrowave-shared.so on Linux), route A of docs/pyrowave-decode.md: the SDK decodes the
//! entropy-coded records and runs the inverse wavelets on its own Vulkan device. On Windows the
//! three R8 or R16 planes are shared D3D11 textures and a shared `ID3D11Fence` orders the D3D11
//! side after the decode. On Linux the SDK decodes on the SDL GPU renderer's own Vulkan device
//! into images the presenter copies from on the GPU (`zero_copy.rs`), or, where that device cannot
//! be shared, on a device of its own with the planes read back into CPU buffers that the presenter
//! uploads (`unix.rs`).
//!
//! Ported from the Rubylight host's decode test harness (rust/windows/graphics/src/pyrowave/decode_tests.rs,
//! `Decoder`) and its `api!` loader (rust/windows/graphics/src/pyrowave.rs). The structs are hand-written
//! against the pinned `pyrowave.h` (API 1.0) with Vulkan types from `ash::vk`, and `tests.rs`
//! checks their sizes and offsets.
//!
//! Every failure is an error naming the missing piece; nothing falls back to the CPU decoder.
//! The stream's GPU decoder (`pyrowave.rs`, `Pyrowave::Gpu`) is the user of this module.
#![allow(dead_code)]

use std::collections::HashMap;
use std::ffi::{c_char, c_void};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use ash::vk;

#[cfg(windows)]
mod event;
#[cfg(windows)]
mod win32;
#[cfg(windows)]
pub use win32::*;

// Unix (Linux and macOS): Vulkan SDK decoder (shared device on Linux, readback on Linux/macOS).
#[cfg(unix)]
mod unix;
// Unix (Linux and macOS): re-export platform GpuDecoder, probe, and Vulkan diagnostics.
#[cfg(unix)]
pub use unix::*;
#[cfg(target_os = "macos")]
pub mod own_vulkan;
// Unix (Linux and macOS) plus tests on every platform so the deadline logic is tested on every host.
#[cfg(any(unix, test))]
mod watchdog;
// Unix (Linux and macOS) plus tests and test-support so renderer device sharing is tested on every host.
#[cfg(any(unix, test, feature = "test-support"))]
pub mod zero_copy;
pub mod queue;

#[cfg(test)]
pub(crate) mod tests;

#[cfg(windows)]
pub const DLL_NAME: &str = "libpyrowave-shared-1.dll";
#[cfg(target_os = "macos")]
pub const DLL_NAME: &str = "libpyrowave-shared.dylib";
#[cfg(target_os = "linux")]
pub const DLL_NAME: &str = "libpyrowave-shared.so";
/// The SDK's API major version this client is written against.
pub const ABI_MAJOR: u32 = 1;

// The C ABI of pyrowave.h, API 1.0. Opaque handles are pointers; enums are 32 bits.
pub type RawDevice = *mut c_void;
pub type RawDecoder = *mut c_void;
pub type RawSyncObject = *mut c_void;
pub type RawImage = *mut c_void;
pub type PyroResult = i32;

pub const SUCCESS: PyroResult = 0;
/// `VK_QUEUE_GLOBAL_PRIORITY_HIGH` (MEDIUM, 256, is Vulkan's default): the priority the decoder
/// has always asked for and was measured at.
const PRIORITY_HIGH: i32 = 512;
/// `VK_QUEUE_FAMILY_IGNORED`: acquiring from it discards the image's content (a decode overwrites it).
const QUEUE_FAMILY_IGNORED: u32 = u32::MAX;
/// `VK_QUEUE_FAMILY_EXTERNAL`: the planes are handed back to D3D11.
const QUEUE_FAMILY_EXTERNAL: u32 = u32::MAX - 1;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Luid {
    pub luid: [u8; 8],
}

#[repr(C)]
pub struct DecoderCreateInfo {
    pub device: RawDevice,
    pub width: i32,
    pub height: i32,
    /// `pyrowave_chroma_subsampling`: 0 is 4:2:0, 1 is 4:4:4.
    pub chroma: i32,
    pub fragment_path: bool,
}

#[repr(C)]
pub struct SyncObjectCreateInfo {
    pub device: RawDevice,
    pub external_handle: usize,
    pub handle_type: vk::ExternalSemaphoreHandleTypeFlags,
    pub semaphore_type: vk::SemaphoreType,
    pub import_flags: vk::SemaphoreImportFlags,
}

#[repr(C)]
pub struct ImageCreateInfo {
    pub device: RawDevice,
    pub external_handle: usize,
    pub handle_type: vk::ExternalMemoryHandleTypeFlags,
    /// A `VkImageCreateInfo`.
    pub image_create_info: *const c_void,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ImageView {
    pub image: vk::Image,
    pub width: u32,
    pub height: u32,
    pub image_format: vk::Format,
    pub view_format: vk::Format,
    pub mip_level: u32,
    pub layer: u32,
    pub aspect: vk::ImageAspectFlags,
    pub swizzle: vk::ComponentSwizzle,
    pub layout: vk::ImageLayout,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct GpuBuffers {
    pub planes: [ImageView; 3],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct GpuExternalReference {
    pub image: RawImage,
    pub queue_family_index: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SyncPoint {
    pub semaphore: vk::Semaphore,
    pub value: u64,
}

#[repr(C)]
pub struct GpuSyncOperation {
    pub images: *const GpuExternalReference,
    pub num_images: usize,
    pub sync: SyncPoint,
}

/// `pyrowave_cpu_buffer`: the CPU path of the decoder (`unix.rs`). The formats it takes are
/// `CPU_FORMAT_*`; the planes are written by the decoder.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct CpuBuffer {
    pub data: [*mut c_void; 3],
    pub row_stride_in_bytes: [usize; 3],
    pub plane_size_in_bytes: [usize; 3],
    pub width: i32,
    pub height: i32,
    pub format: i32,
}

/// `pyrowave_cpu_buffer_format`: separate planes, 8 bits per sample, or 16 bits holding the
/// picture as 16-bit UNORM (the SDK's default scaling, the same values as the R16 planes on Windows).
pub const CPU_FORMAT_YUV420P: i32 = 1;
pub const CPU_FORMAT_YUV444P: i32 = 2;
pub const CPU_FORMAT_YUV420P16: i32 = 5;
pub const CPU_FORMAT_YUV444P16: i32 = 6;

pub type MessageCallback = unsafe extern "C" fn(*mut c_void, *const c_char);

/// `pyrowave_device_create_queue_info`: a queue of the shared device the SDK uses instead of
/// asking Vulkan for one.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct DeviceQueueInfo {
    pub queue: vk::Queue,
    pub family_index: u32,
    pub index: u32,
}

/// `pyrowave_queue_lock_cb`: locks or unlocks every submission to the shared device's queues.
pub type QueueLockCallback = unsafe extern "C" fn(*mut c_void);

/// `pyrowave_device_create_info`: an existing Vulkan device for the SDK to share
/// (`pyrowave_create_device`). The create infos describe what the device was made with, and they
/// and everything they point to must outlive the SDK device.
#[repr(C)]
pub struct SharedDeviceCreateInfo {
    pub get_instance_proc_addr: vk::PFN_vkGetInstanceProcAddr,
    pub instance: vk::Instance,
    pub physical_device: vk::PhysicalDevice,
    pub device: vk::Device,
    pub instance_create_info: *const vk::InstanceCreateInfo<'static>,
    pub device_create_info: *const vk::DeviceCreateInfo<'static>,
    pub queue_info: *mut DeviceQueueInfo,
    pub queue_info_count: u32,
    pub queue_lock_callback: Option<QueueLockCallback>,
    pub queue_unlock_callback: Option<QueueLockCallback>,
    pub userdata: *mut c_void,
}

macro_rules! api {
    ($($name:ident: $ty:ty),* $(,)?) => {
        /// The SDK's entry points, resolved once from the DLL.
        pub struct Api {
            $(pub $name: $ty,)*
            _dll: libloading::Library,
        }
        impl Api {
            /// Resolves every entry point; the version was checked before.
            fn bind(dll: libloading::Library, path: &Path) -> Result<Self, String> {
                // SAFETY: Each symbol matches its API 1.0 C signature, and the library moves into
                // the Api, which keeps every copied pointer valid.
                unsafe {
                    $(let $name = *dll.get::<$ty>(concat!("pyrowave_", stringify!($name), "\0").as_bytes())
                        .map_err(|e| format!("{} lacks pyrowave_{}: {e}", path.display(), stringify!($name)))?;)*
                    Ok(Self { $($name,)* _dll: dll })
                }
            }
        }
    };
}
api! {
    create_device: unsafe extern "C" fn(*const SharedDeviceCreateInfo, *mut RawDevice) -> PyroResult,
    create_device_by_compat: unsafe extern "C" fn(u32, u32, *const [u8; 16], *const [u8; 16], *const Luid, i32, *mut RawDevice) -> PyroResult,
    device_confirm_interop_support: unsafe extern "C" fn(RawDevice) -> bool,
    device_get_vk_device_handles: unsafe extern "C" fn(RawDevice, *mut vk::Instance, *mut vk::PhysicalDevice, *mut vk::Device),
    device_report_performance_stats: unsafe extern "C" fn(RawDevice, MessageCallback, *mut c_void, bool),
    // `VkQueueFlagBits`: `VK_QUEUE_GRAPHICS_BIT` or `VK_QUEUE_COMPUTE_BIT` (`queue.rs`).
    device_set_queue_type: unsafe extern "C" fn(RawDevice, vk::QueueFlags) -> PyroResult,
    device_destroy: unsafe extern "C" fn(RawDevice),
    decoder_create: unsafe extern "C" fn(*const DecoderCreateInfo, *mut RawDecoder) -> PyroResult,
    decoder_destroy: unsafe extern "C" fn(RawDecoder),
    decoder_clear: unsafe extern "C" fn(RawDecoder),
    decoder_push_packet: unsafe extern "C" fn(RawDecoder, *const c_void, usize) -> PyroResult,
    decoder_decode_is_ready: unsafe extern "C" fn(RawDecoder, bool) -> bool,
    decoder_decode_gpu_buffer: unsafe extern "C" fn(RawDecoder, *const GpuSyncOperation, *const GpuSyncOperation, *const GpuBuffers) -> PyroResult,
    decoder_decode_cpu_buffer_async: unsafe extern "C" fn(RawDecoder, *const CpuBuffer, i32) -> PyroResult,
    decoder_decode_cpu_buffer_complete: unsafe extern "C" fn(RawDecoder, *const CpuBuffer, i32) -> PyroResult,
    sync_object_create: unsafe extern "C" fn(*const SyncObjectCreateInfo, *mut RawSyncObject) -> PyroResult,
    sync_object_get_semaphore: unsafe extern "C" fn(RawSyncObject) -> vk::Semaphore,
    sync_object_cpu_wait: unsafe extern "C" fn(RawSyncObject, u64, u64) -> PyroResult,
    sync_object_destroy: unsafe extern "C" fn(RawSyncObject),
    image_create: unsafe extern "C" fn(*const ImageCreateInfo, *mut RawImage) -> PyroResult,
    image_get_image_view: unsafe extern "C" fn(RawImage, vk::ImageAspectFlags, vk::ImageUsageFlags, *mut ImageView) -> PyroResult,
    image_destroy: unsafe extern "C" fn(RawImage),
}

// SAFETY: The function pointers are plain code addresses and the library handle is thread-safe;
// the SDK's objects are not, and GpuDecoder serializes them.
unsafe impl Send for Api {}
unsafe impl Sync for Api {}

/// The SDK names its results with these numbers (pyrowave_result).
pub fn result_name(code: PyroResult) -> &'static str {
    match code {
        0 => "success",
        1 => "timeout",
        -1 => "generic error",
        -2 => "invalid argument",
        -3 => "out of host memory",
        -4 => "out of device memory",
        -5 => "no Vulkan",
        -6 => "not implemented",
        -7 => "unsupported external handle",
        -8 => "failed external handle",
        _ => "unknown result",
    }
}

fn check(code: PyroResult, what: &str) -> Result<(), String> {
    if code == SUCCESS {
        Ok(())
    } else {
        Err(format!("{what}: PyroWave error {code} ({})", result_name(code)))
    }
}

/// The ABI rule: the SDK's major version must be the one this client's structs were written for.
pub fn check_abi(major: u32, minor: u32, patch: u32) -> Result<(), String> {
    if major == ABI_MAJOR {
        Ok(())
    } else {
        Err(format!("PyroWave SDK ABI {major}.{minor}.{patch} is not supported, this client needs {ABI_MAJOR}.x"))
    }
}

/// The DLL once loaded by `Api::shared`. It is never unloaded: the SDK starts threads and Vulkan
/// state that outlive any one decoder, and reloading it per probe or per stream is slow.
static SHARED: Mutex<Option<Arc<Api>>> = Mutex::new(None);
static SHARED_LOADS: AtomicUsize = AtomicUsize::new(0);

impl Api {
    /// The DLL next to the executable, loaded on the first call and kept for the life of the
    /// process. A failure is not remembered: the next call looks again.
    pub fn shared() -> Result<Arc<Self>, String> {
        let mut shared = SHARED.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(api) = shared.as_ref() {
            return Ok(api.clone());
        }
        let api = Self::load()?;
        SHARED_LOADS.fetch_add(1, Ordering::Relaxed);
        *shared = Some(api.clone());
        Ok(api)
    }

    /// How many times `shared` has loaded the DLL (at most once per process).
    pub fn shared_loads() -> usize {
        SHARED_LOADS.load(Ordering::Relaxed)
    }

    /// The DLL next to the executable.
    pub fn load() -> Result<Arc<Self>, String> {
        Self::load_from(&dll_path().ok_or("cannot locate the executable")?)
    }

    /// The DLL at `path`, after checking that it is a PyroWave SDK of a supported ABI.
    pub fn load_from(path: &Path) -> Result<Arc<Self>, String> {
        if !path.exists() {
            return Err(format!("{DLL_NAME} not found next to the executable ({})", path.display()));
        }
        // SAFETY: Loading runs the DLL's initializers; this is the SDK the package ships.
        let dll = unsafe { libloading::Library::new(path) }.map_err(|e| format!("cannot load {}: {e}", path.display()))?;
        // SAFETY: pyrowave_get_api_version is the SDK's one stable entry point (void, three out-pointers).
        let version = unsafe {
            dll.get::<unsafe extern "C" fn(*mut u32, *mut u32, *mut u32)>(b"pyrowave_get_api_version\0")
                .map(|f| *f)
                .map_err(|_| format!("{} is not a PyroWave SDK: pyrowave_get_api_version is missing", path.display()))?
        };
        let (mut major, mut minor, mut patch) = (0, 0, 0);
        // SAFETY: The three out-pointers are live locals.
        unsafe { version(&mut major, &mut minor, &mut patch) };
        check_abi(major, minor, patch)?;
        Self::bind(dll, path).map(Arc::new)
    }
}

/// A pass and its GPU milliseconds per frame from one line of the SDK's performance report
/// (`performance_stats`), which reads "iDWT: 0.051 ms per frame" for a pass and has memory heap
/// lines besides.
pub fn pass_ms(line: &str) -> Option<(&str, f64)> {
    let (name, rest) = line.split_once(": ")?;
    Some((name.trim(), rest.strip_suffix(" ms per frame")?.trim().parse().ok()?))
}

/// The SDK's GPU time per frame, from its timestamps around each pass: the passes' milliseconds
/// and their sum, or None when the report has no pass line. Unlike the stream's "gpu" stage, which
/// is the wall time from the submit to the signal, this leaves out the time the decode waited for
/// the queue (other work on it, or the presenter's copy the decode waits for), so the two apart
/// tell a slow GPU from a busy queue.
pub fn pass_summary(lines: &[String]) -> Option<String> {
    let passes: Vec<(&str, f64)> = lines.iter().filter_map(|line| pass_ms(line)).collect();
    if passes.is_empty() { return None; }
    let sum: f64 = passes.iter().map(|(_, ms)| ms).sum();
    let each: Vec<String> = passes.iter().map(|(name, ms)| format!("{name} {ms:.3}")).collect();
    Some(format!("(SDK GPU timestamps, ms per frame): {sum:.2} in all; {}", each.join(", ")))
}

/// What the stream needs of the decoder, fixed at setup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GpuConfig {
    pub width: u32,
    pub height: u32,
    pub chroma444: bool,
    /// R16 planes holding the picture as 16-bit UNORM (the SDK's default scaling) instead of R8.
    pub ten_bit: bool,
    /// Plane sets in the ring; a set is free again once D3D11 has finished reading it.
    pub slots: usize,
}

impl GpuConfig {
    pub fn plane_size(&self, plane: usize) -> (u32, u32) {
        if plane == 0 || self.chroma444 {
            (self.width, self.height)
        } else {
            (self.width / 2, self.height / 2)
        }
    }
}

/// The error for a decoder the SDK would not create, with the Vulkan requirement the adapter misses.
fn decoder_refused(result: PyroResult, why: &str) -> String {
    format!(
        "creating the PyroWave decoder: PyroWave error {result} ({}); it needs Vulkan 1.3 with subgroup size control and the subgroup operations: {why}",
        result_name(result)
    )
}

/// What a probe found, kept per adapter and plane format: the driver does not change under a
/// running process, and a probe builds a Vulkan device.
#[derive(Default)]
struct ProbeCache(Mutex<HashMap<(Luid, bool), Result<(), String>>>);

impl ProbeCache {
    fn get_or_probe(&self, luid: Luid, ten_bit: bool, probe: impl FnOnce() -> Result<(), String>) -> Result<(), String> {
        let mut found = self.0.lock().unwrap_or_else(|e| e.into_inner());
        found.entry((luid, ten_bit)).or_insert_with(probe).clone()
    }
}

static PROBES: std::sync::LazyLock<ProbeCache> = std::sync::LazyLock::new(ProbeCache::default);

/// Where the DLL is looked for: the executable's folder.
pub fn dll_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    // In Nightjar.app the libraries sit in Contents/Frameworks, beside the executable's folder.
    #[cfg(target_os = "macos")]
    if let Some(frameworks) = exe.parent().and_then(Path::parent).map(|contents| contents.join("Frameworks").join(DLL_NAME))
        && frameworks.is_file()
    {
        return Some(frameworks);
    }
    Some(exe.parent()?.join(DLL_NAME))
}
