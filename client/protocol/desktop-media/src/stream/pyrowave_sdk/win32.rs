//! Windows side of the PyroWave GPU SDK decoder: Vulkan decode bound to shared Direct3D 11
//! textures and timeline fences, plus adapter probing and Vulkan requirement diagnostics.
//!
//! Ported from the Rubylight host's decode test harness (rust/windows/graphics/src/pyrowave/decode_tests.rs,
//! `Decoder`).

use std::ffi::{c_char, c_void, CStr};
use std::path::Path;
use std::ptr;
use std::sync::Arc;
use std::time::Duration;

use ash::vk;
use windows::core::{Interface, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, GENERIC_ALL, HMODULE};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_11_0};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_R16_UNORM, DXGI_FORMAT_R8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::{IDXGIDevice, IDXGIResource1, DXGI_SHARED_RESOURCE_READ, DXGI_SHARED_RESOURCE_WRITE};

use super::*;

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
    /// The queue the decodes run on (`queue.rs`).
    queue: queue::DecodeQueue,
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
    /// With the SDK next to the executable (loaded once per process, see `Api::shared`), decoding
    /// on the queue the setting asks for (`queue::async_compute`); which one and why goes to
    /// the log.
    pub fn new(d3d: &ID3D11Device, config: GpuConfig) -> Result<Self, String> {
        let decoder = Self::with_queue(Api::shared()?, d3d, config, queue::async_compute())?;
        eprintln!("nightjar: {}", decoder.queue.line());
        Ok(decoder)
    }

    /// `with_queue` on the graphics queue.
    pub fn with_api(api: Arc<Api>, d3d: &ID3D11Device, config: GpuConfig) -> Result<Self, String> {
        Self::with_queue(api, d3d, config, false)
    }

    /// The queue the decodes run on.
    pub fn decode_queue(&self) -> &queue::DecodeQueue {
        &self.queue
    }

    /// Creates the SDK device on the D3D11 adapter's LUID, the decoder (compute path), `config.slots`
    /// plane sets and the two shared fences, the decodes on a separate compute queue with
    /// `async_compute` where the device has one (`queue::own_device_queue`). Errors name the step
    /// that failed.
    pub fn with_queue(api: Arc<Api>, d3d: &ID3D11Device, config: GpuConfig, async_compute: bool) -> Result<Self, String> {
        if config.slots == 0 || config.width == 0 || config.height == 0 || (!config.chroma444 && (!config.width.is_multiple_of(2) || !config.height.is_multiple_of(2))) {
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
            queue: queue::DecodeQueue::Graphics(queue::SETTING_OFF.into()),
        };
        // SAFETY: `luid` and the out-pointers outlive each call, and `s` owns every handle made so
        // Drop releases them on any early return.
        unsafe {
            let result = (s.api.create_device_by_compat)(0, 0, ptr::null(), ptr::null(), &luid, PRIORITY_HIGH, &mut s.device);
            if result != SUCCESS {
                let why = diagnose_vulkan(&luid);
                return Err(format!("no PyroWave Vulkan device for the display adapter (PyroWave error {result}, {}): {why}", result_name(result)));
            }
            if !(s.api.device_confirm_interop_support)(s.device) {
                return Err("the Vulkan driver cannot share D3D11 textures and fences with the PyroWave device (external memory and semaphore interop are missing)".into());
            }
            // Before the decoder, so nothing of it is made for the other queue. The planes are
            // acquired from QUEUE_FAMILY_IGNORED and released to QUEUE_FAMILY_EXTERNAL by the decode
            // itself, and the decode fence orders D3D11 after it, on either queue.
            let (api, device) = (s.api.clone(), s.device);
            s.queue = queue::apply(&api, device, queue::own_device_queue(async_compute, || queue::sdk_families(&api, device)));
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
        // SAFETY: caller guarantees `self.device` is a live SDK device and `device5` is its D3D11.5 interface.
        unsafe {
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
        // SAFETY: `ImageView` is a C handle struct; caller guarantees `self.device` and `d3d` are live.
        unsafe {
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
        self.submit(slot, after, QUEUE_FAMILY_IGNORED)
    }

    /// Like `decode`, for a set that already holds an earlier decode of the same frame (a slice,
    /// which the presenter may still be drawing). The planes are acquired from `QUEUE_FAMILY_EXTERNAL`,
    /// so the SDK records GENERAL to GENERAL and the driver keeps the contents; with IGNORED it
    /// records UNDEFINED to GENERAL, which lets the driver discard them. It pairs with the release
    /// the previous decode of the set made.
    pub fn decode_again(&mut self, slot: usize, after: u64) -> Result<u64, String> {
        self.submit(slot, after, QUEUE_FAMILY_EXTERNAL)
    }

    fn submit(&mut self, slot: usize, after: u64, acquire_family: u32) -> Result<u64, String> {
        let slot = self.slots.get(slot).ok_or_else(|| format!("plane set {slot} does not exist"))?;
        let acquire_images = slot.images.map(|image| GpuExternalReference { image, queue_family_index: acquire_family });
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

/// Ok once `fence` has reached `value`, an error if it has not within `deadline`. A fence whose
/// device was removed reads as `u64::MAX`, which is an error too, not "done".
pub fn fence_ready_within(fence: &ID3D11Fence, value: u64, deadline: Duration) -> Result<(), String> {
    event::wait_fence_within(fence, value, deadline)
}

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
    // SAFETY: caller guarantees `instance` is a live Vulkan instance.
    unsafe {
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
}
