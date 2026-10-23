//! PyroWave decoding on the GPU through the SDK (libpyrowave-shared-1.dll), route A of
//! docs/pyrowave-decode.md: the SDK decodes the entropy-coded records and runs the inverse
//! wavelets on its own Vulkan device, into three R8 or R16 planes that are shared D3D11
//! textures, and a shared `ID3D11Fence` orders the D3D11 side after it.
//!
//! Ported from the Rubylight host's decode test harness (rust/windows/src/pyrowave/decode_tests.rs,
//! `Decoder`) and its `api!` loader (rust/windows/src/pyrowave.rs). The structs are hand-written
//! against the pinned `pyrowave.h` (API 1.0) with Vulkan types from `ash::vk`, and `tests.rs`
//! checks their sizes and offsets.
//!
//! Every failure is an error naming the missing piece; nothing falls back to the CPU decoder.
//! The stream's GPU decoder (`pyrowave.rs`, `Pyrowave::Gpu`) is the user of this module.
#![allow(dead_code)]

use std::collections::HashMap;
use std::ffi::{c_char, c_void, CStr};
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ash::vk;
use windows::core::{Interface, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, GENERIC_ALL, HMODULE, WAIT_OBJECT_0};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_11_0};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_R16_UNORM, DXGI_FORMAT_R8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::{IDXGIDevice, IDXGIResource1, DXGI_SHARED_RESOURCE_READ, DXGI_SHARED_RESOURCE_WRITE};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

#[cfg(test)]
mod tests;

pub const DLL_NAME: &str = "libpyrowave-shared-1.dll";
/// The SDK's API major version this client is written against.
pub const ABI_MAJOR: u32 = 1;

// The C ABI of pyrowave.h, API 1.0. Opaque handles are pointers; enums are 32 bits.
pub type RawDevice = *mut c_void;
pub type RawDecoder = *mut c_void;
pub type RawSyncObject = *mut c_void;
pub type RawImage = *mut c_void;
pub type PyroResult = i32;

pub const SUCCESS: PyroResult = 0;
/// `VK_QUEUE_GLOBAL_PRIORITY_MEDIUM`, the normal default.
const PRIORITY_MEDIUM: i32 = 512;
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

pub type MessageCallback = unsafe extern "C" fn(*mut c_void, *const c_char);

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
    create_device_by_compat: unsafe extern "C" fn(u32, u32, *const [u8; 16], *const [u8; 16], *const Luid, i32, *mut RawDevice) -> PyroResult,
    device_confirm_interop_support: unsafe extern "C" fn(RawDevice) -> bool,
    device_get_vk_device_handles: unsafe extern "C" fn(RawDevice, *mut vk::Instance, *mut vk::PhysicalDevice, *mut vk::Device),
    device_report_performance_stats: unsafe extern "C" fn(RawDevice, MessageCallback, *mut c_void, bool),
    device_destroy: unsafe extern "C" fn(RawDevice),
    decoder_create: unsafe extern "C" fn(*const DecoderCreateInfo, *mut RawDecoder) -> PyroResult,
    decoder_destroy: unsafe extern "C" fn(RawDecoder),
    decoder_clear: unsafe extern "C" fn(RawDecoder),
    decoder_push_packet: unsafe extern "C" fn(RawDecoder, *const c_void, usize) -> PyroResult,
    decoder_decode_is_ready: unsafe extern "C" fn(RawDecoder, bool) -> bool,
    decoder_decode_gpu_buffer: unsafe extern "C" fn(RawDecoder, *const GpuSyncOperation, *const GpuSyncOperation, *const GpuBuffers) -> PyroResult,
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
        let exe = std::env::current_exe().map_err(|e| format!("cannot locate the executable: {e}"))?;
        Self::load_from(&exe.parent().map(Path::to_path_buf).unwrap_or_default().join(DLL_NAME))
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

/// One set of decoder output planes: D3D11 textures and their Vulkan imports.
pub struct Slot {
    pub textures: [ID3D11Texture2D; 3],
    images: [RawImage; 3],
    buffers: GpuBuffers,
}

/// The SDK's Vulkan decoder bound to a D3D11 device through shared textures and two shared fences:
/// the decode fence (Vulkan signals, D3D11 waits) and the reuse fence (D3D11 signals, Vulkan waits).
///
/// `Send` but not `Sync` (the SDK's entry points are not thread safe, and the raw handles make it
/// `!Sync`): move it between threads, never share it. The textures and fences it hands out are
/// COM objects. A presenter that keeps drawing from them on another thread must hold its own
/// AddRef'd clones (`planes(..).clone()`, `fence().clone()`), because dropping the decoder releases
/// its references and the SDK's Vulkan side, but the presenter's clones keep the D3D11 objects alive.
pub struct GpuDecoder {
    api: Arc<Api>,
    config: GpuConfig,
    device: RawDevice,
    decoder: RawDecoder,
    sync: RawSyncObject,
    fence: Option<ID3D11Fence>,
    reuse_sync: RawSyncObject,
    reuse_fence: Option<ID3D11Fence>,
    slots: Vec<Slot>,
    counter: u64,
}

// SAFETY: Every raw handle is owned by this value, and the SDK may be called from any one thread
// at a time, which `&mut self` guarantees.
unsafe impl Send for GpuDecoder {}

fn luid_of(device: &ID3D11Device) -> Result<Luid, String> {
    // SAFETY: Plain DXGI queries on a live device.
    let describe = || -> windows::core::Result<_> { unsafe { device.cast::<IDXGIDevice>()?.GetAdapter()?.GetDesc() } };
    let desc = describe().map_err(|e| format!("cannot read the D3D11 adapter: {e}"))?;
    let mut luid = Luid::default();
    luid.luid[..4].copy_from_slice(&desc.AdapterLuid.LowPart.to_le_bytes());
    luid.luid[4..].copy_from_slice(&desc.AdapterLuid.HighPart.to_le_bytes());
    Ok(luid)
}

impl GpuDecoder {
    /// With the SDK next to the executable (loaded once per process, see `Api::shared`).
    pub fn new(d3d: &ID3D11Device, config: GpuConfig) -> Result<Self, String> {
        Self::with_api(Api::shared()?, d3d, config)
    }

    /// Creates the SDK device on the D3D11 adapter's LUID, the decoder (compute path), `config.slots`
    /// plane sets and the two shared fences. Errors name the step that failed.
    pub fn with_api(api: Arc<Api>, d3d: &ID3D11Device, config: GpuConfig) -> Result<Self, String> {
        if config.slots == 0 || config.width == 0 || config.height == 0 || (!config.chroma444 && (config.width % 2 != 0 || config.height % 2 != 0)) {
            return Err(format!("PyroWave GPU decoding: bad picture {}x{} with {} plane sets", config.width, config.height, config.slots));
        }
        // The D3D11 side first: its failures are the cheap ones to name.
        let device5 = d3d.cast::<ID3D11Device5>().map_err(|e| format!("the D3D11 device lacks ID3D11Device5 (shared fences need Windows 10 1703 or newer): {e}"))?;
        let luid = luid_of(d3d)?;
        let mut s = Self {
            api,
            config,
            device: ptr::null_mut(),
            decoder: ptr::null_mut(),
            sync: ptr::null_mut(),
            fence: None,
            reuse_sync: ptr::null_mut(),
            reuse_fence: None,
            slots: Vec::new(),
            counter: 0,
        };
        // SAFETY: `luid` and the out-pointers outlive each call, and `s` owns every handle made so
        // Drop releases them on any early return.
        unsafe {
            let result = (s.api.create_device_by_compat)(0, 0, ptr::null(), ptr::null(), &luid, PRIORITY_MEDIUM, &mut s.device);
            if result != SUCCESS {
                let why = diagnose_vulkan(&luid);
                return Err(format!("no PyroWave Vulkan device for the display adapter (PyroWave error {result}, {}): {why}", result_name(result)));
            }
            if !(s.api.device_confirm_interop_support)(s.device) {
                return Err("the Vulkan driver cannot share D3D11 textures and fences with the PyroWave device (external memory and semaphore interop are missing)".into());
            }
            let result = (s.api.decoder_create)(
                &DecoderCreateInfo { device: s.device, width: config.width as i32, height: config.height as i32, chroma: i32::from(config.chroma444), fragment_path: false },
                &mut s.decoder,
            );
            if result != SUCCESS {
                return Err(decoder_refused(result, &diagnose_vulkan(&luid)));
            }
            for _ in 0..config.slots {
                let slot = s.create_slot(d3d)?;
                s.slots.push(slot);
            }
            let (fence, sync) = s.shared_fence(&device5, "decode")?;
            s.fence = Some(fence);
            s.sync = sync;
            let (fence, sync) = s.shared_fence(&device5, "reuse")?;
            s.reuse_fence = Some(fence);
            s.reuse_sync = sync;
        }
        Ok(s)
    }

    /// A shared D3D11 fence and the SDK's timeline semaphore imported from it.
    ///
    /// # Safety
    /// `self.device` must be a live SDK device.
    unsafe fn shared_fence(&self, device5: &ID3D11Device5, name: &str) -> Result<(ID3D11Fence, RawSyncObject), String> {
        let mut fence: Option<ID3D11Fence> = None;
        device5.CreateFence(0, D3D11_FENCE_FLAG_SHARED, &mut fence).map_err(|e| format!("creating the shared D3D11 {name} fence: {e}"))?;
        let fence = fence.ok_or_else(|| format!("creating the shared D3D11 {name} fence: no fence returned"))?;
        let handle = fence.CreateSharedHandle(None, GENERIC_ALL.0, PCWSTR::null()).map_err(|e| format!("sharing the D3D11 {name} fence: {e}"))?;
        let mut sync = ptr::null_mut();
        let result = (self.api.sync_object_create)(
            &SyncObjectCreateInfo {
                device: self.device,
                external_handle: handle.0 as usize,
                handle_type: vk::ExternalSemaphoreHandleTypeFlags::D3D12_FENCE,
                semaphore_type: vk::SemaphoreType::TIMELINE,
                import_flags: vk::SemaphoreImportFlags::empty(),
            },
            &mut sync,
        );
        if result != SUCCESS {
            // The SDK takes the handle only when the import succeeds.
            let _ = CloseHandle(handle);
            return Err(check(result, &format!("importing the D3D11 {name} fence as a Vulkan timeline semaphore")).unwrap_err());
        }
        Ok((fence, sync))
    }

    /// One plane set: three shared textures, imported and bound for storage writes.
    ///
    /// # Safety
    /// `self.device` must be a live SDK device.
    unsafe fn create_slot(&self, d3d: &ID3D11Device) -> Result<Slot, String> {
        let config = self.config;
        let (format, vk_format) = if config.ten_bit { (DXGI_FORMAT_R16_UNORM, vk::Format::R16_UNORM) } else { (DXGI_FORMAT_R8_UNORM, vk::Format::R8_UNORM) };
        let plane_format = if config.ten_bit { "R16 (10-bit)" } else { "R8" };
        let mut textures: Vec<ID3D11Texture2D> = Vec::with_capacity(3);
        let mut images: Vec<RawImage> = Vec::with_capacity(3);
        let mut views = [std::mem::zeroed::<ImageView>(); 3];
        let built = (|| -> Result<(), String> {
            for (plane, view) in views.iter_mut().enumerate() {
                let (width, height) = config.plane_size(plane);
                let mut texture = None;
                d3d.CreateTexture2D(
                    &D3D11_TEXTURE2D_DESC {
                        Width: width,
                        Height: height,
                        MipLevels: 1,
                        ArraySize: 1,
                        Format: format,
                        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                        Usage: D3D11_USAGE_DEFAULT,
                        BindFlags: (D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_UNORDERED_ACCESS.0) as u32,
                        MiscFlags: (D3D11_RESOURCE_MISC_SHARED.0 | D3D11_RESOURCE_MISC_SHARED_NTHANDLE.0) as u32,
                        ..Default::default()
                    },
                    None,
                    Some(&mut texture),
                )
                .map_err(|e| format!("creating a {width}x{height} shared {plane_format} plane texture: {e}"))?;
                let texture = texture.ok_or("creating a plane texture: none returned")?;
                let handle = texture
                    .cast::<IDXGIResource1>()
                    .and_then(|resource| resource.CreateSharedHandle(None, DXGI_SHARED_RESOURCE_READ.0 | DXGI_SHARED_RESOURCE_WRITE.0, PCWSTR::null()))
                    .map_err(|e| format!("sharing a {plane_format} plane texture: {e}"))?;
                let info = vk::ImageCreateInfo::default()
                    .image_type(vk::ImageType::TYPE_2D)
                    .format(vk_format)
                    .extent(vk::Extent3D { width, height, depth: 1 })
                    .mip_levels(1)
                    .array_layers(1)
                    .samples(vk::SampleCountFlags::TYPE_1)
                    .tiling(vk::ImageTiling::OPTIMAL)
                    .usage(vk::ImageUsageFlags::STORAGE | vk::ImageUsageFlags::TRANSFER_SRC)
                    .sharing_mode(vk::SharingMode::EXCLUSIVE);
                let mut image = ptr::null_mut();
                let result = (self.api.image_create)(
                    &ImageCreateInfo {
                        device: self.device,
                        external_handle: handle.0 as usize,
                        handle_type: vk::ExternalMemoryHandleTypeFlags::D3D11_TEXTURE,
                        image_create_info: (&raw const info).cast(),
                    },
                    &mut image,
                );
                if result != SUCCESS {
                    let _ = CloseHandle(handle);
                    return check(result, &format!("importing a {width}x{height} {plane_format} D3D11 plane texture into Vulkan"));
                }
                images.push(image);
                textures.push(texture);
                check(
                    (self.api.image_get_image_view)(image, vk::ImageAspectFlags::COLOR, vk::ImageUsageFlags::STORAGE, view),
                    "creating a plane view",
                )?;
            }
            Ok(())
        })();
        if let Err(error) = built {
            for image in images {
                (self.api.image_destroy)(image);
            }
            return Err(error);
        }
        let textures: [ID3D11Texture2D; 3] = textures.try_into().map_err(|_| "plane count")?;
        Ok(Slot { textures, images: images.try_into().map_err(|_| "plane count")?, buffers: GpuBuffers { planes: views } })
    }

    pub fn config(&self) -> GpuConfig {
        self.config
    }

    pub fn slot_count(&self) -> usize {
        self.slots.len()
    }

    /// The textures decoding into `slot` writes, for the presenter's shader resource views.
    pub fn planes(&self, slot: usize) -> &[ID3D11Texture2D; 3] {
        &self.slots[slot].textures
    }

    /// The decode fence: Vulkan signals the value `decode` returned when the planes are final, and
    /// the presenter makes its queue wait for it with `ID3D11DeviceContext4::Wait`.
    ///
    /// `Wait` has no timeout: if the Vulkan device hangs or is lost the fence never reaches the
    /// value, the D3D11 queue stalls behind it and the display driver ends in a TDR. So the
    /// presenter must call `ready_within` for the value first, and queue the `Wait` only when it
    /// returns `Ok`; its `Err` is the loud failure to report instead.
    pub fn fence(&self) -> &ID3D11Fence {
        self.fence.as_ref().expect("a constructed decoder has its fence")
    }

    /// The reuse fence: the presenter `Signal`s it (`ID3D11DeviceContext4::Signal`) after the draw
    /// that last read a plane set, with a value that only grows, and passes that value as `after` to
    /// the next `decode` into the set.
    pub fn reuse_fence(&self) -> &ID3D11Fence {
        self.reuse_fence.as_ref().expect("a constructed decoder has its reuse fence")
    }

    /// Ok once the decode fence has reached `value` (the GPU work is done and `Wait` will not
    /// block); an error naming the stall if it has not within `deadline`, or if the device is gone.
    pub fn ready_within(&self, value: u64, deadline: Duration) -> Result<(), String> {
        fence_ready_within(self.fence(), value, deadline)
    }

    /// Throws away the packets queued for the frame in progress.
    pub fn clear(&mut self) {
        // SAFETY: `decoder` is live while self is.
        unsafe { (self.api.decoder_clear)(self.decoder) }
    }

    /// Queues one record (or container packet) of the frame. CPU only: it copies into SDK staging.
    pub fn push_packet(&mut self, data: &[u8]) -> Result<(), String> {
        // SAFETY: `data` is a live slice for the call, which copies it.
        check(unsafe { (self.api.decoder_push_packet)(self.decoder, data.as_ptr().cast(), data.len()) }, "pushing a PyroWave packet")
    }

    /// Whether every block of the frame has been pushed (or, with `allow_partial`, enough of them).
    pub fn is_ready(&self, allow_partial: bool) -> bool {
        // SAFETY: `decoder` is live while self is.
        unsafe { (self.api.decoder_decode_is_ready)(self.decoder, allow_partial) }
    }

    /// Submits the decode of the pushed records into `slot` and returns the fence value that the
    /// GPU signals when the planes are final. Blocks, if at all, only for staging uploads.
    ///
    /// The decode overwrites the set, so the GPU waits, before it writes, until the reuse fence
    /// reaches `after`: the value the presenter signals after the last draw that read `slot`'s
    /// previous picture (0 when nothing read it). The D3D11 queue can run frames behind the CPU,
    /// so this wait is what keeps a late draw from sampling a half-written picture. The `Signal`
    /// must already be queued (and flushed) on the D3D11 side, or the decode waits for ever.
    /// Missing blocks decode as zero, so a partial frame blurs and never corrupts.
    pub fn decode(&mut self, slot: usize, after: u64) -> Result<u64, String> {
        let slot = self.slots.get(slot).ok_or_else(|| format!("plane set {slot} does not exist"))?;
        let acquire_images = slot.images.map(|image| GpuExternalReference { image, queue_family_index: QUEUE_FAMILY_IGNORED });
        let release_images = slot.images.map(|image| GpuExternalReference { image, queue_family_index: QUEUE_FAMILY_EXTERNAL });
        // SAFETY: Both operations and the buffers are live locals for the call; the semaphores are
        // the imported fences'. A null semaphore means no wait.
        let value = self.counter + 1;
        let result = unsafe {
            let wait = if after == 0 { std::mem::zeroed() } else { SyncPoint { semaphore: (self.api.sync_object_get_semaphore)(self.reuse_sync), value: after } };
            let acquire = GpuSyncOperation { images: acquire_images.as_ptr(), num_images: 3, sync: wait };
            let release = GpuSyncOperation {
                images: release_images.as_ptr(),
                num_images: 3,
                sync: SyncPoint { semaphore: (self.api.sync_object_get_semaphore)(self.sync), value },
            };
            (self.api.decoder_decode_gpu_buffer)(self.decoder, &acquire, &release, &slot.buffers)
        };
        check(result, "submitting the PyroWave decode")?;
        self.counter = value;
        Ok(value)
    }

    /// Waits on the CPU until the GPU has signalled `value`; an error if it did not within `timeout`.
    pub fn wait(&self, value: u64, timeout: std::time::Duration) -> Result<(), String> {
        // SAFETY: `sync` is live while self is.
        let result = unsafe { (self.api.sync_object_cpu_wait)(self.sync, value, timeout.as_nanos().min(u128::from(u64::MAX)) as u64) };
        match result {
            SUCCESS => Ok(()),
            1 => Err(format!("the PyroWave decode did not finish within {timeout:?}")),
            code => check(code, "waiting for the PyroWave decode"),
        }
    }

    /// The SDK's GPU timestamps since the last reset, as the lines it reports.
    pub fn performance_stats(&self, reset: bool) -> Vec<String> {
        unsafe extern "C" fn collect(userdata: *mut c_void, message: *const c_char) {
            if !message.is_null() {
                // SAFETY: `userdata` is the Vec below and `message` a NUL-terminated string.
                unsafe { (*userdata.cast::<Vec<String>>()).push(CStr::from_ptr(message).to_string_lossy().into_owned()) };
            }
        }
        let mut lines: Vec<String> = Vec::new();
        // SAFETY: The callback runs inside this call, with `lines` alive.
        unsafe { (self.api.device_report_performance_stats)(self.device, collect, (&raw mut lines).cast(), reset) };
        lines
    }
}

impl Drop for GpuDecoder {
    fn drop(&mut self) {
        // SAFETY: Each non-null handle was made on `self.device`, which goes last; the SDK idles the
        // GPU before destroying the decoder. The D3D11 textures and fence go with the fields after
        // this; D3D11 frees them at its next flush, which the owner of the immediate context does
        // (a Flush here would race that thread).
        unsafe {
            if !self.decoder.is_null() {
                (self.api.decoder_destroy)(self.decoder);
            }
            for slot in &self.slots {
                for image in slot.images {
                    (self.api.image_destroy)(image);
                }
            }
            for sync in [self.sync, self.reuse_sync] {
                if !sync.is_null() {
                    (self.api.sync_object_destroy)(sync);
                }
            }
            if !self.device.is_null() {
                (self.api.device_destroy)(self.device);
            }
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

/// Ok once `fence` has reached `value`, an error if it has not within `deadline`. A fence whose
/// device was removed reads as `u64::MAX`, which is an error too, not "done".
pub fn fence_ready_within(fence: &ID3D11Fence, value: u64, deadline: Duration) -> Result<(), String> {
    // SAFETY: Plain fence calls; the event is closed on every path.
    unsafe {
        let mut completed = fence.GetCompletedValue();
        if completed < value {
            let event = CreateEventW(None, false, false, PCWSTR::null()).map_err(|e| format!("cannot wait for the PyroWave decode: {e}"))?;
            let armed = fence.SetEventOnCompletion(value, event);
            let waited = armed.is_ok().then(|| WaitForSingleObject(event, deadline.as_millis().min(u128::from(u32::MAX - 1)) as u32));
            let _ = CloseHandle(event);
            armed.map_err(|e| format!("cannot wait for the PyroWave decode: {e}"))?;
            completed = fence.GetCompletedValue();
            if waited != Some(WAIT_OBJECT_0) && completed < value {
                return Err(format!(
                    "the PyroWave decode (fence value {value}) was not done within {deadline:?} and the fence stands at {completed}: the Vulkan device may be hung or lost, so the D3D11 queue must not wait on it"
                ));
            }
        }
        if completed == u64::MAX {
            return Err(format!("the D3D11 device holding the PyroWave fence was removed while waiting for value {value}"));
        }
    }
    Ok(())
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

/// The decoder is offered only if this succeeds, with the DLL next to the executable and the
/// D3D11 device the stream renders with (`None`: the default hardware adapter, for settings screens
/// that have no device yet). It builds a minimal decoder, so every requirement is exercised for
/// real: the DLL and its ABI, `ID3D11Device5`, a Vulkan device for the adapter's LUID with the
/// subgroup features, D3D11 interop, plane import (R16 planes with `ten_bit`, R8 without) and both
/// shared fences. The error names the first piece that is missing.
///
/// The DLL loads once per process and the result is remembered per adapter and plane format, so
/// asking again (every settings screen, every stream start) costs nothing. A 10-bit stream needs
/// `probe(.., true)` to succeed; a failure there is as loud as any other probe failure.
pub fn probe(device: Option<&ID3D11Device>, ten_bit: bool) -> Result<(), String> {
    let api = Api::shared()?;
    let own;
    let device = match device {
        Some(device) => device,
        None => {
            own = default_device()?;
            &own
        }
    };
    let luid = luid_of(device)?;
    PROBES.get_or_probe(luid, ten_bit, || probe_once(api, device, ten_bit))
}

/// `probe` with the DLL at `dll`, neither kept nor remembered.
pub fn probe_in(dll: &Path, device: Option<&ID3D11Device>, ten_bit: bool) -> Result<(), String> {
    let api = Api::load_from(dll)?;
    let own;
    let device = match device {
        Some(device) => device,
        None => {
            own = default_device()?;
            &own
        }
    };
    probe_once(api, device, ten_bit)
}

fn probe_once(api: Arc<Api>, device: &ID3D11Device, ten_bit: bool) -> Result<(), String> {
    GpuDecoder::with_api(api, device, GpuConfig { width: 128, height: 128, chroma444: false, ten_bit, slots: 1 }).map(drop)
}

/// A hardware D3D11 device on the default adapter.
pub fn default_device() -> Result<ID3D11Device, String> {
    let mut device = None;
    // SAFETY: The out-pointer is a live local.
    unsafe {
        D3D11CreateDevice(None, D3D_DRIVER_TYPE_HARDWARE, HMODULE::default(), D3D11_CREATE_DEVICE_FLAG(0), Some(&[D3D_FEATURE_LEVEL_11_0]), D3D11_SDK_VERSION, Some(&mut device), None, None)
    }
    .map_err(|e| format!("cannot create a D3D11 device: {e}"))?;
    device.ok_or_else(|| "cannot create a D3D11 device: none returned".to_owned())
}

/// Why the SDK may have refused a Vulkan device for this adapter, read from the driver directly:
/// the first requirement of the decoder that the adapter's Vulkan device does not meet.
pub fn diagnose_vulkan(luid: &Luid) -> String {
    // SAFETY: Plain Vulkan calls on handles made here; the instance is destroyed before returning.
    unsafe {
        let entry = match ash::Entry::load() {
            Ok(entry) => entry,
            Err(e) => return format!("vulkan-1.dll cannot be loaded, so there is no Vulkan driver ({e})"),
        };
        let app = vk::ApplicationInfo::default().api_version(vk::make_api_version(0, 1, 3, 0));
        let instance = match entry.create_instance(&vk::InstanceCreateInfo::default().application_info(&app), None) {
            Ok(instance) => instance,
            Err(e) => return format!("the Vulkan loader cannot create an instance ({e})"),
        };
        let found = describe_adapter(&instance, luid);
        instance.destroy_instance(None);
        found
    }
}

unsafe fn describe_adapter(instance: &ash::Instance, luid: &Luid) -> String {
    let devices = match instance.enumerate_physical_devices() {
        Ok(devices) => devices,
        Err(e) => return format!("Vulkan lists no devices ({e})"),
    };
    let mut names = Vec::new();
    for device in devices {
        let mut id = vk::PhysicalDeviceIDProperties::default();
        let mut subgroup = vk::PhysicalDeviceSubgroupProperties::default();
        let mut properties = vk::PhysicalDeviceProperties2::default().push_next(&mut id).push_next(&mut subgroup);
        instance.get_physical_device_properties2(device, &mut properties);
        let properties = properties.properties;
        let name = properties.device_name_as_c_str().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if id.device_luid_valid == vk::FALSE || id.device_luid != luid.luid {
            names.push(name);
            continue;
        }
        let version = properties.api_version;
        if version < vk::make_api_version(0, 1, 3, 0) {
            return format!("{name} supports Vulkan {}.{}, the decoder needs 1.3", vk::api_version_major(version), vk::api_version_minor(version));
        }
        let needed = vk::SubgroupFeatureFlags::BASIC
            | vk::SubgroupFeatureFlags::VOTE
            | vk::SubgroupFeatureFlags::ARITHMETIC
            | vk::SubgroupFeatureFlags::BALLOT
            | vk::SubgroupFeatureFlags::SHUFFLE
            | vk::SubgroupFeatureFlags::SHUFFLE_RELATIVE;
        let missing = needed & !subgroup.supported_operations;
        if !missing.is_empty() {
            return format!("{name} lacks subgroup operations (missing bits {:#x} of VkSubgroupFeatureFlags)", missing.as_raw());
        }
        let mut v13 = vk::PhysicalDeviceVulkan13Features::default();
        let mut features = vk::PhysicalDeviceFeatures2::default().push_next(&mut v13);
        instance.get_physical_device_features2(device, &mut features);
        if v13.subgroup_size_control == vk::FALSE {
            return format!("{name} lacks subgroup size control");
        }
        let extensions = instance.enumerate_device_extension_properties(device).unwrap_or_default();
        for required in [c"VK_KHR_external_memory_win32", c"VK_KHR_external_semaphore_win32"] {
            if !extensions.iter().any(|e| e.extension_name_as_c_str().is_ok_and(|n| n == required)) {
                return format!("{name} lacks {}", required.to_string_lossy());
            }
        }
        return format!("{name} meets the decoder's Vulkan requirements, but the SDK still refused it");
    }
    format!("Vulkan lists no device with this adapter's LUID (it lists {})", if names.is_empty() { "none".to_owned() } else { names.join(", ") })
}

/// Where the DLL is looked for: the executable's folder.
pub fn dll_path() -> Option<PathBuf> {
    Some(std::env::current_exe().ok()?.parent()?.join(DLL_NAME))
}
