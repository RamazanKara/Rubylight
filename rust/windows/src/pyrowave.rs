//! Rust-owned D3D11/Vulkan PyroWave encoder using the stable 2.0 bitstream.

use crate::{
    capture::{Device, GpuImage, Image, Pixel},
    encoder::Encoded,
    gpu_color::PlanarConverter,
    pyro_abi as p,
};
use anyhow::{Context, Result, bail};
use butterpollo_core::{
    pyrowave::{self, PACKET_BOUNDARY},
    rtsp::Negotiated,
};
use std::{
    collections::HashSet,
    ffi::c_void,
    ptr,
    sync::{Arc, Mutex, OnceLock},
    time::Instant,
};
use windows::{
    Win32::{
        Foundation::*,
        Graphics::{
            Direct3D11::*,
            Dxgi::{
                Common::*, DXGI_ERROR_DEVICE_REMOVED, DXGI_SHARED_RESOURCE_READ,
                DXGI_SHARED_RESOURCE_WRITE, IDXGIDevice, IDXGIResource1,
            },
        },
    },
    core::{Interface, PCWSTR},
};
#[cfg(test)]
mod decode_tests;
macro_rules! api {
    ($($name:ident:$ty:ty),* $(,)?) => {
        struct Api { $($name:$ty,)* _dll:libloading::Library }
        impl Api {
            fn load() -> Result<Arc<Self>> {
                let path=std::env::current_exe()?.parent().context("executable directory unavailable")?.join("libpyrowave-shared-1.dll");
                // SAFETY: `dll` moves into the Api, keeping every copied symbol loaded; each
                // matches the 1.0 C header, and the version is checked below.
                unsafe {
                    let dll=libloading::Library::new(&path).with_context(||format!("loading {}",path.display()))?;
                    $(let $name=*dll.get::<$ty>(concat!("pyrowave_",stringify!($name),"\0").as_bytes())?;)*
                    let api=Self { $($name,)* _dll:dll };
                    let (mut major,mut minor,mut patch)=(0,0,0);
                    (api.get_api_version)(&mut major,&mut minor,&mut patch);
                    if major!=1 { bail!("unsupported PyroWave ABI {major}.{minor}.{patch}; expected 1.x"); }
                    Ok(Arc::new(api))
                }
            }
        }
    };
}
api! {
    get_api_version:unsafe extern "C" fn(*mut u32,*mut u32,*mut u32),
    create_device_by_compat:unsafe extern "C" fn(u32,u32,*const p::pyrowave_uuid,*const p::pyrowave_uuid,*const p::pyrowave_luid,p::VkQueueGlobalPriority,*mut p::pyrowave_device)->p::pyrowave_result,
    device_get_global_priority:unsafe extern "C" fn(p::pyrowave_device)->p::VkQueueGlobalPriority,
    device_set_queue_type:unsafe extern "C" fn(p::pyrowave_device,p::VkQueueFlagBits)->p::pyrowave_result,
    device_confirm_interop_support:unsafe extern "C" fn(p::pyrowave_device)->bool,
    device_get_vk_device_handles:unsafe extern "C" fn(p::pyrowave_device,*mut p::VkInstance,*mut p::VkPhysicalDevice,*mut p::VkDevice),
    device_destroy:unsafe extern "C" fn(p::pyrowave_device),
    encoder_create:unsafe extern "C" fn(*const p::pyrowave_encoder_create_info,*mut p::pyrowave_encoder)->p::pyrowave_result,
    encoder_destroy:unsafe extern "C" fn(p::pyrowave_encoder),
    encoder_encode_gpu:unsafe extern "C" fn(p::pyrowave_encoder,*const p::pyrowave_gpu_sync_operation,*const p::pyrowave_gpu_sync_operation,*const p::pyrowave_gpu_buffers,*const p::pyrowave_rate_control)->p::pyrowave_result,
    encoder_compute_num_packets_with_padding:unsafe extern "C" fn(p::pyrowave_encoder,usize,usize,*mut usize)->p::pyrowave_result,
    encoder_get_mapped_raw_bitstream:unsafe extern "C" fn(p::pyrowave_encoder,*mut *const c_void,*mut usize,*mut *const c_void,*mut usize)->p::pyrowave_result,
    encoder_packetize_with_padding:unsafe extern "C" fn(p::pyrowave_encoder,*mut p::pyrowave_packet,usize,usize,*mut usize,*mut c_void,usize)->p::pyrowave_result,
    image_create:unsafe extern "C" fn(*const p::pyrowave_image_create_info,*mut p::pyrowave_image)->p::pyrowave_result,
    image_get_image_view:unsafe extern "C" fn(p::pyrowave_image,p::VkImageAspectFlagBits,p::VkImageUsageFlagBits,*mut p::pyrowave_image_view)->p::pyrowave_result,
    image_destroy:unsafe extern "C" fn(p::pyrowave_image),
    sync_object_create:unsafe extern "C" fn(*const p::pyrowave_sync_object_create_info,*mut p::pyrowave_sync_object)->p::pyrowave_result,
    sync_object_get_semaphore:unsafe extern "C" fn(p::pyrowave_sync_object)->p::VkSemaphore,
    sync_object_destroy:unsafe extern "C" fn(p::pyrowave_sync_object),
}
fn check(code: p::pyrowave_result) -> Result<()> {
    if code == 0 {
        Ok(())
    } else {
        bail!("PyroWave runtime error {code}")
    }
}
/// The runtime's ordinary fence waits discard VkResult. Wait for the external
/// release ourselves, before its unchecked mapping/packetization can read stale
/// memory. This also replaces a D3D11 GPU wait which could wedge capture forever
/// when Vulkan failed to signal the shared fence.
struct CompletionWait {
    device: p::VkDevice,
    wait: p::PFN_vkWaitSemaphores,
    _loader: libloading::Library,
}
impl CompletionWait {
    fn new(api: &Api, device: p::pyrowave_device) -> Result<Self> {
        // SAFETY: Both entry points use the generated Vulkan ABI. The loader and
        // PyroWave device outlive every call through this function pointer.
        unsafe {
            let loader = libloading::Library::new("vulkan-1.dll")?;
            let get = loader.get::<p::PFN_vkGetDeviceProcAddr>(b"vkGetDeviceProcAddr\0")?;
            let mut vk_device = ptr::null_mut();
            (api.device_get_vk_device_handles)(
                device,
                ptr::null_mut(),
                ptr::null_mut(),
                &mut vk_device,
            );
            anyhow::ensure!(!vk_device.is_null(), "PyroWave Vulkan device unavailable");
            let symbol = get.context("Vulkan device dispatch unavailable")?(
                vk_device,
                c"vkWaitSemaphores".as_ptr(),
            );
            anyhow::ensure!(
                symbol.is_some(),
                "Vulkan timeline semaphore wait unavailable"
            );
            let wait =
                std::mem::transmute::<p::PFN_vkVoidFunction, p::PFN_vkWaitSemaphores>(symbol);
            Ok(Self {
                device: vk_device,
                wait,
                _loader: loader,
            })
        }
    }
    fn wait(&self, semaphore: p::VkSemaphore, value: u64) -> Result<()> {
        let info = p::VkSemaphoreWaitInfo {
            sType: p::VkStructureType_VK_STRUCTURE_TYPE_SEMAPHORE_WAIT_INFO,
            pNext: ptr::null(),
            flags: 0,
            semaphoreCount: 1,
            pSemaphores: &semaphore,
            pValues: &value,
        };
        // SAFETY: The imported semaphore belongs to this device; its owner and
        // the two borrowed locals remain alive for this bounded CPU wait.
        let result = unsafe { self.wait.unwrap()(self.device, &info, 2_000_000_000) };
        completion_result(result)
    }
}
fn completion_result(result: p::VkResult) -> Result<()> {
    match result {
        p::VkResult_VK_SUCCESS => Ok(()),
        p::VkResult_VK_ERROR_DEVICE_LOST => Err(anyhow::Error::new(
            crate::device_loss::DeviceLost(DXGI_ERROR_DEVICE_REMOVED.0),
        )
        .context("PyroWave Vulkan device lost (VK_ERROR_DEVICE_LOST)")),
        p::VkResult_VK_TIMEOUT => bail!("PyroWave GPU completion timed out after 2 seconds"),
        code => bail!("PyroWave GPU completion failed with Vulkan result {code}"),
    }
}
fn finish_submission<T>(pending: &mut Option<T>, completed: Result<()>) -> Result<()> {
    completed?;
    *pending = None;
    Ok(())
}
/// Unconfirmed submissions can still access their resources. Do not run their
/// destructors on timeout: retaining them until process exit is safer than a
/// use-after-free or an unbounded runtime/device-idle wait during recovery.
fn retain_pending<T>(pending: &mut Option<T>) {
    if let Some(pending) = pending.take() {
        std::mem::forget(pending);
    }
}
/// Adapter identity survives device recreation. A timeout must not make the
/// recovery loop retain another full encoder on every attempt. Existing
/// concurrent encoders also check this before submitting their next frame.
#[derive(Default)]
struct QuarantinedAdapters(Mutex<HashSet<[u8; 8]>>);
impl QuarantinedAdapters {
    fn check(&self, adapter: [u8; 8]) -> Result<()> {
        anyhow::ensure!(
            !self.0.lock().unwrap().contains(&adapter),
            "PyroWave is unavailable on this adapter after an unconfirmed GPU submission. Restart Rubylight to release the retained GPU resources, or use HEVC/AV1."
        );
        Ok(())
    }
    fn block(&self, adapter: [u8; 8]) {
        self.0.lock().unwrap().insert(adapter);
    }
}
fn quarantined_adapters() -> &'static QuarantinedAdapters {
    static ADAPTERS: OnceLock<QuarantinedAdapters> = OnceLock::new();
    ADAPTERS.get_or_init(Default::default)
}
pub fn available() -> bool {
    Api::load().is_ok()
}
struct Interop {
    api: Arc<Api>,
    image: p::pyrowave_image,
    sync: p::pyrowave_sync_object,
    view: p::pyrowave_image_view,
    _texture: Arc<ID3D11Texture2D>,
    fence: ID3D11Fence,
    context: ID3D11DeviceContext4,
    counter: u64,
}
impl Interop {
    fn new(
        api: Arc<Api>,
        device: &Device,
        pyro: p::pyrowave_device,
        texture: Arc<ID3D11Texture2D>,
    ) -> Result<Self> {
        // SAFETY: `texture` and `device` share one live D3D11 device, the create-infos outlive
        // their calls, and `s` owns each handle so Drop frees it on error.
        unsafe {
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            texture.GetDesc(&mut desc);
            let mut fence = None;
            device.device.cast::<ID3D11Device5>()?.CreateFence(
                0,
                D3D11_FENCE_FLAG_SHARED,
                &mut fence,
            )?;
            let mut s = Self {
                api,
                image: ptr::null_mut(),
                sync: ptr::null_mut(),
                view: std::mem::zeroed(),
                _texture: texture,
                fence: fence.unwrap(),
                context: device.context.cast()?,
                counter: 0,
            };
            let handle = s._texture.cast::<IDXGIResource1>()?.CreateSharedHandle(
                None,
                DXGI_SHARED_RESOURCE_READ.0 | DXGI_SHARED_RESOURCE_WRITE.0,
                PCWSTR::null(),
            )?;
            let image = p::VkImageCreateInfo {
                sType: p::VkStructureType_VK_STRUCTURE_TYPE_IMAGE_CREATE_INFO,
                imageType: p::VkImageType_VK_IMAGE_TYPE_2D,
                format: if desc.Format == DXGI_FORMAT_R16_UNORM {
                    p::VkFormat_VK_FORMAT_R16_UNORM
                } else {
                    p::VkFormat_VK_FORMAT_R8_UNORM
                },
                extent: p::VkExtent3D {
                    width: desc.Width,
                    height: desc.Height,
                    depth: 1,
                },
                mipLevels: 1,
                arrayLayers: 1,
                samples: p::VkSampleCountFlagBits_VK_SAMPLE_COUNT_1_BIT,
                tiling: p::VkImageTiling_VK_IMAGE_TILING_OPTIMAL,
                usage: p::VkImageUsageFlagBits_VK_IMAGE_USAGE_SAMPLED_BIT
                    | p::VkImageUsageFlagBits_VK_IMAGE_USAGE_TRANSFER_SRC_BIT
                    | p::VkImageUsageFlagBits_VK_IMAGE_USAGE_TRANSFER_DST_BIT,
                sharingMode: p::VkSharingMode_VK_SHARING_MODE_EXCLUSIVE,
                ..std::mem::zeroed()
            };
            let info=p::pyrowave_image_create_info { device:pyro,external_handle:handle.0 as usize,handle_type:p::VkExternalMemoryHandleTypeFlagBits_VK_EXTERNAL_MEMORY_HANDLE_TYPE_D3D11_TEXTURE_BIT,image_create_info:&image };
            let result = (s.api.image_create)(&info, &mut s.image);
            if result != 0 {
                let _ = CloseHandle(handle);
                check(result)?;
            }
            check((s.api.image_get_image_view)(
                s.image,
                p::VkImageAspectFlagBits_VK_IMAGE_ASPECT_COLOR_BIT,
                p::VkImageUsageFlagBits_VK_IMAGE_USAGE_SAMPLED_BIT,
                &mut s.view,
            ))?;
            let handle = s
                .fence
                .CreateSharedHandle(None, GENERIC_ALL.0, PCWSTR::null())?;
            let info=p::pyrowave_sync_object_create_info { device:pyro,external_handle:handle.0 as usize,handle_type:p::VkExternalSemaphoreHandleTypeFlagBits_VK_EXTERNAL_SEMAPHORE_HANDLE_TYPE_D3D12_FENCE_BIT,semaphore_type:p::VkSemaphoreType_VK_SEMAPHORE_TYPE_TIMELINE,import_flags:0 };
            let result = (s.api.sync_object_create)(&info, &mut s.sync);
            if result != 0 {
                let _ = CloseHandle(handle);
                check(result)?;
            }
            Ok(s)
        }
    }
}
impl Drop for Interop {
    fn drop(&mut self) {
        // SAFETY: `sync` and `image` were created on the Encoder's device, which is destroyed after
        // `interop`.
        unsafe {
            if !self.sync.is_null() {
                (self.api.sync_object_destroy)(self.sync);
            }
            if !self.image.is_null() {
                (self.api.image_destroy)(self.image);
            }
        }
    }
}
/// The colour conversion on the GPU's compute queue. Beside a game using the
/// whole GPU, the D3D11 conversion of a 1080p HDR 4:4:4 frame waited 5.1 ms
/// on the graphics queue, against 0.15 ms idle, nearly all of the encode's
/// 5.7 ms; PyroWave's own GPU work took 0.2 ms either way.
struct ComputePlanes {
    converter: crate::compute::Converter,
    target: crate::compute::Planes,
    textures: [Arc<ID3D11Texture2D>; 3],
}
impl ComputePlanes {
    fn new(
        d3d: &Device,
        config: &Negotiated,
        textures: &[Arc<ID3D11Texture2D>; 3],
        fence: &ID3D11Fence,
    ) -> Result<Self> {
        let compute = crate::compute::Compute::for_device(&d3d.device)?;
        let converter = crate::compute::Converter::new(
            compute.clone(),
            config.width,
            config.height,
            config.ten_bit(),
        )?;
        let planes = [
            compute.open(&textures[0])?,
            compute.open(&textures[1])?,
            compute.open(&textures[2])?,
        ];
        // SAFETY: `fence` is a live shared D3D11 fence and `compute.device` is a D3D12 device on
        // the same adapter; `opened` is a live local and the handle is closed once opened.
        let fence = unsafe {
            let handle = fence.CreateSharedHandle(None, GENERIC_ALL.0, PCWSTR::null())?;
            let mut opened: Option<windows::Win32::Graphics::Direct3D12::ID3D12Fence> = None;
            let result = compute.device.OpenSharedHandle(handle, &mut opened);
            let _ = CloseHandle(handle);
            result.context("opening the PyroWave fence on the compute queue")?;
            opened.context("no shared fence")?
        };
        Ok(Self {
            converter,
            textures: textures.clone(),
            target: crate::compute::Planes {
                planes,
                format: if config.ten_bit() {
                    DXGI_FORMAT_R16_UNORM
                } else {
                    DXGI_FORMAT_R8_UNORM
                },
                fence,
                after: 0,
                done: 0,
            },
        })
    }
    /// Convert once the encoder has signalled `after`; signals `done`.
    fn convert(
        &mut self,
        image: &GpuImage,
        config: &Negotiated,
        luminance: [f32; 2],
        after: u64,
        done: u64,
    ) -> Result<()> {
        let texture = self.converter.compute().open(&image.texture)?;
        let pointer = image
            .cursor
            .as_ref()
            .map(|cursor| self.converter.pointer(cursor))
            .transpose()?;
        self.converter.values = crate::gpu_color::constants(
            config,
            (image.width, image.height, image.pixel),
            luminance,
            image.cursor.as_ref(),
        );
        self.converter.values[11] = u32::from(config.yuv444);
        self.target.after = after;
        self.target.done = done;
        self.converter.convert_planes(
            &texture,
            crate::compute::format(image.pixel),
            pointer.as_ref(),
            image.ready.as_ref(),
            &self.target,
            Some(image),
        )
    }
}
impl Drop for ComputePlanes {
    fn drop(&mut self) {
        for texture in &self.textures {
            self.converter.compute().forget(texture);
        }
    }
}
fn interval(config: &Negotiated) -> pyrowave::Interval {
    // A pause must not buy a larger burst when capture resumes, including for
    // legacy clients without record framing or critical FEC.
    pyrowave::Interval::new(
        butterpollo_core::framegen::Rate(config.fps_millihz()).period(),
        true,
    )
}
pub struct Encoder {
    warnings: Arc<butterpollo_core::session::Warnings>,
    api: Arc<Api>,
    device: p::pyrowave_device,
    encoder: p::pyrowave_encoder,
    completion: Option<CompletionWait>,
    /// Pins the capture lease and conversion inputs until Vulkan releases them.
    /// It remains set on every failed conversion, submission or completion.
    pending: Option<GpuImage>,
    lost: Option<crate::device_loss::DeviceLost>,
    adapter: [u8; 8],
    d3d: Device,
    converter: Option<PlanarConverter>,
    /// Whether to convert on the compute queue (AMD, `gpu_compute_conversion`).
    use_compute: bool,
    compute: Option<ComputePlanes>,
    source: Option<(u32, u32, Pixel)>,
    interop: Vec<Interop>,
    config: Negotiated,
    critical_fec: bool,
    /// Parity for each frame's critical block, as a percentage; 0 without.
    critical_fec_percentage: u32,
    interval: pyrowave::Interval,
    /// The next picture repeats the last one encoded.
    pub(crate) repeat: bool,
    pub(crate) luminance: [f32; 2],
    bitstream: Vec<u8>,
    packets: Vec<p::pyrowave_packet>,
    #[cfg(test)]
    pub(crate) hooks: TestHooks,
}
/// Runs a test's fault at an exact point of the next frame; each fires once.
#[cfg(test)]
#[derive(Default)]
pub(crate) struct TestHooks {
    /// After the entry checks, before the conversion is recorded.
    pub(crate) before_conversion: Option<Box<dyn FnOnce() + Send>>,
    /// After the encode is submitted, before its completion is awaited.
    pub(crate) after_submit: Option<Box<dyn FnOnce() + Send>>,
}
impl Encoder {
    pub fn new(config: &Negotiated, display: &str) -> Result<Self> {
        Self::new_device(config, Device::new(display)?, &Default::default())
    }
    pub fn new_device(
        config: &Negotiated,
        d3d: Device,
        tuning: &butterpollo_core::config::Config,
    ) -> Result<Self> {
        Self::new_device_reported(config, d3d, tuning, Default::default())
    }
    pub fn new_device_reported(
        config: &Negotiated,
        d3d: Device,
        tuning: &butterpollo_core::config::Config,
        warnings: Arc<butterpollo_core::session::Warnings>,
    ) -> Result<Self> {
        let api = Api::load()?;
        // SAFETY: `d3d.device` is a live D3D11 device; GetAdapter and GetDesc only read it.
        let desc = unsafe { d3d.device.cast::<IDXGIDevice>()?.GetAdapter()?.GetDesc()? };
        let mut luid = p::pyrowave_luid { luid: [0; 8] };
        luid.luid[..4].copy_from_slice(&desc.AdapterLuid.LowPart.to_le_bytes());
        luid.luid[4..].copy_from_slice(&desc.AdapterLuid.HighPart.to_le_bytes());
        quarantined_adapters().check(luid.luid)?;
        let critical_fec_percentage = tuning
            .integer("pyrowave_critical_fec_percentage", 20)
            .clamp(0, 255) as u32;
        let critical_fec = config.pyrowave_records && critical_fec_percentage > 0;
        let mut s = Self {
            warnings,
            api,
            device: ptr::null_mut(),
            encoder: ptr::null_mut(),
            completion: None,
            pending: None,
            lost: None,
            adapter: luid.luid,
            use_compute: crate::compute::enabled(tuning),
            d3d,
            converter: None,
            compute: None,
            source: None,
            interop: vec![],
            config: config.clone(),
            critical_fec,
            critical_fec_percentage,
            interval: interval(config),
            repeat: false,
            luminance: [100., 1.],
            bitstream: vec![],
            packets: vec![],
            #[cfg(test)]
            hooks: Default::default(),
        };
        // SAFETY: `luid` and `info` are live locals, and `s` owns the device and encoder, which
        // Drop destroys.
        unsafe {
            // The encode queue runs ahead of the game's work, as the D3D12 copy and
            // conversion queue does: high priority, realtime only when
            // compute_queue_realtime asks for it. A driver that refuses the
            // priority gets the default one.
            let wanted = if crate::compute::realtime() {
                p::VkQueueGlobalPriority_VK_QUEUE_GLOBAL_PRIORITY_REALTIME
            } else {
                p::VkQueueGlobalPriority_VK_QUEUE_GLOBAL_PRIORITY_HIGH
            };
            let mut result = p::pyrowave_result_PYROWAVE_ERROR_GENERIC;
            for priority in [
                wanted,
                p::VkQueueGlobalPriority_VK_QUEUE_GLOBAL_PRIORITY_MEDIUM,
            ] {
                result = (s.api.create_device_by_compat)(
                    0,
                    0,
                    ptr::null(),
                    ptr::null(),
                    &luid,
                    priority,
                    &mut s.device,
                );
                if result == p::pyrowave_result_PYROWAVE_SUCCESS {
                    break;
                }
            }
            check(result)?;
            let granted = (s.api.device_get_global_priority)(s.device);
            tracing::info!(
                requested = wanted,
                granted,
                "PyroWave encode queue priority"
            );
            if !(s.api.device_confirm_interop_support)(s.device) {
                bail!("Vulkan device cannot import D3D11 textures and fences");
            }
            if let Err(error) = check((s.api.device_set_queue_type)(
                s.device,
                p::VkQueueFlagBits_VK_QUEUE_COMPUTE_BIT,
            )) {
                s.warnings.set("pyrowave_queue", format!("PyroWave compute queue selection failed ({error:#}); using the runtime's default queue. Game rendering may delay encoding; update the AMD driver or lower game GPU load."));
            }
            let info = p::pyrowave_encoder_create_info {
                device: s.device,
                width: config.width as i32,
                height: config.height as i32,
                chroma: u32::from(config.yuv444),
            };
            check((s.api.encoder_create)(&info, &mut s.encoder))?;
            s.completion = Some(CompletionWait::new(&s.api, s.device)?);
        }
        Ok(s)
    }
    pub fn accepts_gpu_device(&self, image: &GpuImage) -> bool {
        self.d3d.device.as_raw() == image.gpu.device.as_raw()
    }
    pub(crate) fn device_removed(&self) -> Option<crate::device_loss::DeviceLost> {
        self.lost
            .or_else(|| crate::device_loss::DeviceLost::d3d11(&self.d3d.device))
            .or_else(|| self.compute_device_removed())
    }
    /// The D3D12 device that converts on the compute queue, when it is lost.
    fn compute_device_removed(&self) -> Option<crate::device_loss::DeviceLost> {
        let compute = self.compute.as_ref()?.converter.compute();
        crate::device_loss::DeviceLost::d3d12(&compute.device)
    }
    /// A conversion failed before anything reached Vulkan. When the device it
    /// ran on is lost its queued work is dead, so the capture lease is free and
    /// the adapter is not quarantined: the host rebuilds the encoder on a new
    /// device. Any other failure keeps the lease and quarantines.
    fn conversion_failed(&mut self, error: anyhow::Error) -> anyhow::Error {
        match self.device_removed() {
            Some(loss) => {
                self.lost = Some(loss);
                self.pending = None;
                error.context(loss)
            }
            None => error,
        }
    }
    /// The next encode gets a first frame's budget, one stream period.
    #[cfg(test)]
    pub(crate) fn restart_interval(&mut self) {
        self.interval = interval(&self.config);
    }
    pub fn encode(&mut self, image: &Image, idr: bool, kbps: u32) -> Result<Vec<Encoded>> {
        self.encode_gpu(&GpuImage::upload(&self.d3d, image)?, idr, kbps)
    }
    pub fn encode_gpu(&mut self, image: &GpuImage, _idr: bool, kbps: u32) -> Result<Vec<Encoded>> {
        let result = self.encode_gpu_inner(image, kbps);
        if result.is_err() && self.pending.is_some() {
            self.quarantine();
        }
        result
    }
    fn quarantine(&self) {
        quarantined_adapters().block(self.adapter);
        if let Some(compute) = &self.compute {
            compute.converter.compute().retire();
        }
        self.warnings.set("pyrowave_gpu_quarantine", "PyroWave stopped after GPU work could not be confirmed complete. Its resources are retained to protect capture; restart Rubylight before using PyroWave on this adapter again, or use HEVC/AV1.");
    }
    fn encode_gpu_inner(&mut self, image: &GpuImage, kbps: u32) -> Result<Vec<Encoded>> {
        // Refuse a device already known or found lost before it gets new work.
        if let Some(loss) = self.device_removed() {
            return Err(loss.into());
        }
        quarantined_adapters().check(self.adapter)?;
        anyhow::ensure!(
            self.pending.is_none(),
            "PyroWave encoder has an unconfirmed GPU submission; rebuild it before encoding again"
        );
        if !self.accepts_gpu_device(image) {
            bail!("capture device changed");
        }
        let begin = Instant::now();
        let source = (image.width, image.height, image.pixel);
        if self.source != Some(source) {
            self.compute = None;
            self.interop.clear();
            self.converter = None;
            let converter = PlanarConverter::new(&self.d3d, &self.config, source)?;
            for texture in &converter.textures {
                self.interop.push(Interop::new(
                    self.api.clone(),
                    &self.d3d,
                    self.device,
                    texture.clone(),
                )?);
            }
            self.warnings.clear("pyrowave_conversion");
            if self.use_compute {
                match ComputePlanes::new(
                    &self.d3d,
                    &self.config,
                    &converter.textures,
                    &self.interop[0].fence,
                ) {
                    Ok(planes) => self.compute = Some(planes),
                    Err(error) => {
                        self.warnings.set("pyrowave_conversion", format!("PyroWave compute conversion unavailable ({error:#}); converting on the graphics queue. Game rendering may delay frames; update the AMD driver or lower GPU load."))
                    }
                }
            }
            self.converter = Some(converter);
            self.source = Some(source);
        }
        let interval = self.interval.next(begin, std::mem::take(&mut self.repeat));
        let budget = pyrowave::budget_with_minimum(
            kbps,
            interval,
            self.config.packet_size,
            self.config.pyrowave_records,
            self.critical_fec_percentage,
            self.config.min_fec,
        );
        if budget < 16 {
            return Ok(vec![]);
        }
        self.pending = Some(image.clone());
        // SAFETY: `interop` holds the three images and views made on `self.device`; `images`
        // outlives the synchronous encode and the output buffers are sized as passed.
        unsafe {
            let images: Vec<_> = self
                .interop
                .iter()
                .map(|i| p::pyrowave_gpu_external_reference {
                    image: i.image,
                    queue_family_index: 0xfffffffe,
                })
                .collect();
            #[cfg(test)]
            if let Some(fault) = self.hooks.before_conversion.take() {
                fault();
            }
            let i = &mut self.interop[0];
            let after = i.counter;
            i.counter = i
                .counter
                .checked_add(1)
                .context("PyroWave fence exhausted")?;
            let compute = self
                .compute
                .as_mut()
                .filter(|_| crate::compute::shareable(&image.texture));
            let converted = if let Some(compute) = compute {
                compute.convert(image, &self.config, self.luminance, after, i.counter)
            } else {
                self.converter
                    .as_mut()
                    .unwrap()
                    .convert(image, self.luminance)
                    .and_then(|()| Ok(i.context.Signal(&i.fence, i.counter)?))
                    .inspect(|()| {
                        // Submit the conversion and the signal now, not whenever
                        // D3D11 next flushes: the encode below waits for that signal.
                        i.context.Flush();
                    })
            };
            if let Err(error) = converted {
                return Err(self.conversion_failed(error));
            }
            let acquire = p::pyrowave_gpu_sync_operation {
                images: images.as_ptr(),
                num_images: 3,
                sync: p::pyrowave_sync_point {
                    semaphore: (self.api.sync_object_get_semaphore)(i.sync),
                    value: i.counter,
                },
            };
            i.counter = i
                .counter
                .checked_add(1)
                .context("PyroWave fence exhausted")?;
            let release = p::pyrowave_gpu_sync_operation {
                sync: p::pyrowave_sync_point {
                    value: i.counter,
                    ..acquire.sync
                },
                ..acquire
            };
            let buffers = p::pyrowave_gpu_buffers {
                planes: [
                    self.interop[0].view,
                    self.interop[1].view,
                    self.interop[2].view,
                ],
            };
            check((self.api.encoder_encode_gpu)(
                self.encoder,
                &acquire,
                &release,
                &buffers,
                &p::pyrowave_rate_control {
                    maximum_bitstream_size: budget,
                },
            ))?;
            // SDK 502a3b52 submits the metadata/bitstream host copies and their
            // COPY→HOST barrier before this same-queue release signal. Granite
            // signals it at ALL_COMMANDS, so this covers the output writes as
            // well as the input reads; the getters below submit no GPU work.
            #[cfg(test)]
            if let Some(fault) = self.hooks.after_submit.take() {
                fault();
            }
            let completed = self
                .completion
                .as_ref()
                .unwrap()
                .wait(release.sync.semaphore, release.sync.value);
            if let Err(error) = &completed {
                // A Vulkan loss (VK_ERROR_DEVICE_LOST) keeps the lease and
                // quarantines: nothing proves the driver stopped reading the
                // imported capture texture, unlike the D3D12 case below.
                self.lost = crate::device_loss::DeviceLost::from_error(error);
            }
            finish_submission(&mut self.pending, completed)?;
            // The lease is free whatever the wait proved: Vulkan never reads the
            // capture texture, only the encoder-owned planes in `interop`. Ask
            // now whether a device died under the frame: a lost compute device
            // signals its fences to the maximum, which can satisfy that wait
            // before Vulkan has run the encode (Drop idles Vulkan before it
            // destroys anything). The output is garbage, and the host rebuilds
            // on a new device; the adapter is not quarantined. On the graphics
            // path (compute planes present, frame not shareable) the D3D11
            // conversion may still be reading the capture when the lease is
            // returned; capture can then tear a buffer that is discarded anyway.
            // This is the second GetDeviceRemovedReason pair per frame (the
            // first is the entry check). Both stay: the entry check keeps a
            // dead device from being handed new work, this one is the only
            // check that sees a loss during the frame. Each is a runtime flag
            // read on two devices, not a GPU round trip; not measured.
            if let Some(loss) = self.device_removed() {
                self.lost = Some(loss);
                return Err(loss.into());
            }
            let (mut raw, mut meta) = (ptr::null(), ptr::null());
            let (mut size, mut meta_size) = (0, 0);
            check((self.api.encoder_get_mapped_raw_bitstream)(
                self.encoder,
                &mut raw,
                &mut size,
                &mut meta,
                &mut meta_size,
            ))?;
            if size > 64 * 1024 * 1024 {
                bail!("PyroWave bitstream exceeds the limit");
            }
            let boundary = if self.config.pyrowave_records {
                u32::MAX as usize
            } else {
                PACKET_BOUNDARY
            };
            let mut count = 0;
            check((self.api.encoder_compute_num_packets_with_padding)(
                self.encoder,
                boundary,
                0,
                &mut count,
            ))?;
            if count == 0 || count > 65535 {
                bail!("invalid PyroWave packet count");
            }
            self.bitstream.resize(size + 8, 0);
            self.packets
                .resize_with(count, || p::pyrowave_packet { offset: 0, size: 0 });
            let mut written = 0;
            check((self.api.encoder_packetize_with_padding)(
                self.encoder,
                self.packets.as_mut_ptr(),
                boundary,
                0,
                &mut written,
                self.bitstream.as_mut_ptr().cast(),
                self.bitstream.len(),
            ))?;
            if written == 0 || written > count {
                bail!("invalid PyroWave packetization");
            }
            let limit = pyrowave::max_frame_bytes(self.config.packet_size, self.critical_fec);
            let bytes = if self.config.pyrowave_records {
                if written != 1 {
                    bail!("record frame must be one codec packet");
                }
                let packet = &self.packets[0];
                let end = packet
                    .offset
                    .checked_add(packet.size)
                    .context("packet overflow")?;
                let raw = self
                    .bitstream
                    .get(packet.offset..end)
                    .context("invalid codec record range")?;
                let framed = pyrowave::record_frame_with(
                    raw,
                    pyrowave::aligned_payload(self.config.packet_size),
                    limit,
                    self.config.pyrowave_row_order,
                )?;
                if framed.len() <= budget {
                    framed
                } else {
                    let layout = pyrowave::layout(
                        &framed,
                        pyrowave::aligned_payload(self.config.packet_size),
                    )?;
                    let minimum = self.config.min_fec.max(pyrowave::MIN_PARITY_SHARDS);
                    let packets = (framed.len() + 8).div_ceil(self.config.packet_size - 16)
                        + pyrowave::critical_parity_bound(
                            layout.critical_shards,
                            self.critical_fec_percentage as usize,
                            minimum,
                        );
                    let allowance = (f64::from(kbps) * 125. * interval.as_secs_f64()) as usize;
                    if packets * (self.config.packet_size + pyrowave::WIRE_OVERHEAD) <= allowance {
                        framed
                    } else {
                        if self.config.pyrowave_row_order {
                            self.warnings.event("pyrowave_row_order", format!("PyroWave row order was dropped for frames that do not fit the wire budget: {} bytes framed ({} unframed, packet size {}) against a budget of {budget} bytes and a wire allowance of {allowance} bytes. Raise the bitrate or turn the client's beam racing off.", framed.len(), raw.len(), self.config.packet_size), butterpollo_core::session::EVENT_PERIOD);
                        }
                        raw.to_vec()
                    }
                }
            } else {
                pyrowave::container(
                    &self.bitstream,
                    &self.packets[..written]
                        .iter()
                        .map(|p| (p.offset, p.size))
                        .collect::<Vec<_>>(),
                )?
            };
            if bytes.len() > limit {
                self.warnings.event("pyrowave_frame_size", format!("PyroWave frame was dropped: {} bytes exceeds the transport limit of {limit}. Lower bitrate or resolution.", bytes.len()), butterpollo_core::session::EVENT_PERIOD);
                return Ok(vec![]);
            }
            Ok(vec![Encoded {
                bytes,
                unit: crate::encoder::Unit::Whole,
                idr: true,
                after_invalidation: false,
                frame_cap_bytes: None,
                latency: Some(begin.elapsed()),
                presentation: Some(image.captured),
            }])
        }
    }
}
impl Drop for Encoder {
    fn drop(&mut self) {
        if self.pending.is_some() {
            // Keep both APIs, all imported images/fences, conversion commands,
            // and the capture lease alive. No queued wait is added to capture.
            self.quarantine();
            tracing::warn!("retaining PyroWave resources after an unconfirmed GPU submission");
            retain_pending(&mut self.pending);
            retain_pending(&mut self.completion);
            retain_pending(&mut self.compute);
            retain_pending(&mut self.converter);
            std::mem::forget(std::mem::take(&mut self.interop));
            std::mem::forget(self.api.clone());
            std::mem::forget(self.d3d.clone());
            return;
        }
        // SAFETY: `encoder` and `device` were created in `new_device_reported`; the interop images
        // go before the device. After a device loss `pending` is already clear while Vulkan may
        // still be running or queued, because the lost fence satisfies the completion wait early.
        // `pyrowave_encoder_destroy` goes first: pyrowave.h (SDK 502a3b52, "Implementation
        // ensures GPU is idle before destroying objects") idles the GPU there, before the
        // images and the device are destroyed.
        unsafe {
            if !self.encoder.is_null() {
                (self.api.encoder_destroy)(self.encoder);
            }
            self.compute = None;
            self.interop.clear();
            self.converter = None;
            if !self.device.is_null() {
                (self.api.device_destroy)(self.device);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, rc::Rc, time::Duration};

    struct Resource(Rc<Cell<usize>>);
    impl Drop for Resource {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }

    #[test]
    fn only_confirmed_vulkan_completion_releases_the_capture_lease() {
        for code in [
            p::VkResult_VK_SUCCESS,
            p::VkResult_VK_TIMEOUT,
            p::VkResult_VK_ERROR_DEVICE_LOST,
            p::VkResult_VK_ERROR_OUT_OF_DEVICE_MEMORY,
        ] {
            let dropped = Rc::new(Cell::new(0));
            let mut pending = Some(Resource(dropped.clone()));
            let result = finish_submission(&mut pending, completion_result(code));
            if code == p::VkResult_VK_SUCCESS {
                assert!(result.is_ok());
                assert!(pending.is_none());
                assert_eq!(dropped.get(), 1);
            } else {
                assert!(result.is_err());
                assert!(pending.is_some());
                assert_eq!(dropped.get(), 0);
                // Quarantining must not run the capture lease's destructor.
                retain_pending(&mut pending);
                assert!(pending.is_none());
                assert_eq!(dropped.get(), 0);
            }
        }
    }

    #[test]
    fn vulkan_device_loss_reaches_the_existing_recovery_classifier() {
        let error = completion_result(p::VkResult_VK_ERROR_DEVICE_LOST)
            .unwrap_err()
            .context("encoding a legacy frame");
        assert_eq!(
            crate::device_loss::DeviceLost::from_error(&error),
            Some(crate::device_loss::DeviceLost(DXGI_ERROR_DEVICE_REMOVED.0))
        );
        assert!(format!("{error:#}").contains("VK_ERROR_DEVICE_LOST"));
        assert!(
            crate::device_loss::DeviceLost::from_error(
                &completion_result(p::VkResult_VK_TIMEOUT).unwrap_err()
            )
            .is_none()
        );
    }

    #[test]
    fn quarantine_survives_recreation_and_stops_concurrent_admission_on_the_same_adapter() {
        let adapters = QuarantinedAdapters::default();
        let failed_adapter = [1, 2, 3, 4, 5, 6, 7, 8];
        let other_adapter = [8, 7, 6, 5, 4, 3, 2, 1];
        // Two streams may already be open when one fails; creation and every
        // subsequent frame must both consult the same adapter identity.
        assert!(adapters.check(failed_adapter).is_ok());
        assert!(adapters.check(failed_adapter).is_ok());
        adapters.block(failed_adapter);
        std::thread::scope(|scope| {
            for _ in 0..4 {
                let adapters = &adapters;
                scope.spawn(move || {
                    assert!(adapters.check(failed_adapter).is_err());
                    assert!(adapters.check(other_adapter).is_ok());
                });
            }
        });
        assert_eq!(adapters.0.lock().unwrap().len(), 1);
    }

    #[test]
    fn legacy_and_record_frames_do_not_accumulate_a_pause_budget() {
        for records in [false, true] {
            let config = Negotiated {
                fps: 60,
                pyrowave_records: records,
                ..Default::default()
            };
            let mut interval = interval(&config);
            let now = Instant::now();
            let period = butterpollo_core::framegen::Rate(config.fps_millihz()).period();
            assert_eq!(interval.next(now, false), period);
            assert_eq!(interval.next(now + Duration::from_secs(5), true), period);
            assert_eq!(
                interval.next(
                    now + Duration::from_secs(5) + Duration::from_millis(1),
                    false
                ),
                period
            );
        }
    }
}
