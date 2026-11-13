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
    cell::Cell,
    collections::{HashSet, VecDeque},
    ffi::c_void,
    ptr,
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant},
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
/// `pyrowave_encoder_set_frame_context`: which of the SDK's readbacks the
/// next encode writes and the next readout reads, for two frames in flight.
type SetFrameContext =
    unsafe extern "C" fn(p::pyrowave_encoder, std::ffi::c_int) -> p::pyrowave_result;
macro_rules! api {
    ($($name:ident:$ty:ty),* $(,)?) => {
        struct Api { $($name:$ty,)* set_frame_context: Option<SetFrameContext>, _dll:libloading::Library }
        impl Api {
            fn load() -> Result<Arc<Self>> {
                let path=std::env::current_exe()?.parent().context("executable directory unavailable")?.join("libpyrowave-shared-1.dll");
                // SAFETY: `dll` moves into the Api, keeping every copied symbol loaded; each
                // matches the 1.0 C header, and the version is checked below.
                unsafe {
                    let dll=libloading::Library::new(&path).with_context(||format!("loading {}",path.display()))?;
                    $(let $name=*dll.get::<$ty>(concat!("pyrowave_",stringify!($name),"\0").as_bytes())?;)*
                    // Optional: without it, one frame is in flight at a time.
                    let set_frame_context=dll.get::<SetFrameContext>(b"pyrowave_encoder_set_frame_context\0").ok().map(|symbol| *symbol);
                    let api=Self { $($name,)* set_frame_context, _dll:dll };
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
    /// Whether Vulkan has signalled `value`, waiting at most `timeout`.
    fn reached(&self, semaphore: p::VkSemaphore, value: u64, timeout: Duration) -> Result<bool> {
        let info = p::VkSemaphoreWaitInfo {
            sType: p::VkStructureType_VK_STRUCTURE_TYPE_SEMAPHORE_WAIT_INFO,
            pNext: ptr::null(),
            flags: 0,
            semaphoreCount: 1,
            pSemaphores: &semaphore,
            pValues: &value,
        };
        let timeout = u64::try_from(timeout.as_nanos()).unwrap_or(u64::MAX);
        // SAFETY: The imported semaphore belongs to this device; its owner and
        // the two borrowed locals remain alive for this bounded CPU wait.
        let result = unsafe { self.wait.unwrap()(self.device, &info, timeout) };
        reached(result)
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
/// [`completion_result`] of a wait that may end before the value: `false`
/// when it timed out.
fn reached(result: p::VkResult) -> Result<bool> {
    if result == p::VkResult_VK_TIMEOUT {
        return Ok(false);
    }
    completion_result(result).map(|()| true)
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
/// Lets one line through per period, for the numbers of a warning whose text must stay fixed so
/// `Warnings` keeps it to one log line (a text that changes with every frame never dedups).
#[derive(Default)]
struct Notice {
    last: Option<Instant>,
}
impl Notice {
    fn due(&mut self, now: Instant, period: Duration) -> bool {
        if self.last.is_some_and(|at| now.duration_since(at) < period) {
            return false;
        }
        self.last = Some(now);
        true
    }
}
/// The text of the warning for a frame sent in index order though row order was asked for.
fn row_order_dropped_message(packet_size: usize) -> String {
    format!(
        "PyroWave row order was dropped for frames that do not fit the wire budget at a packet size of {packet_size}. Raise the bitrate or turn the client's beam racing off."
    )
}
/// The text of the warning for a frame over the transport limit.
fn frame_size_message(limit: usize) -> String {
    format!(
        "PyroWave frames were dropped: over the transport limit of {limit} bytes. Lower bitrate or resolution."
    )
}
/// Frames in flight at most: the SDK keeps a bitstream readback for each of
/// its frame contexts (`PYROWAVE_MAX_FRAME_CONTEXTS` in pyrowave.h).
const MAX_DEPTH: usize = 2;
/// How long a frame's GPU work may take before it counts as unconfirmed.
const COMPLETION_TIMEOUT: Duration = Duration::from_secs(2);
/// Whether a frame submitted at `submitted` is done, as `reached` reports it
/// waiting at most the given time: at once unless `block`, otherwise for what
/// is left of [`COMPLETION_TIMEOUT`]. A frame not done in that time is an
/// error, as is any failure of the wait.
fn settle(
    reached: impl FnOnce(Duration) -> Result<bool>,
    submitted: Instant,
    now: Instant,
    block: bool,
) -> Result<bool> {
    let left = COMPLETION_TIMEOUT.saturating_sub(now.saturating_duration_since(submitted));
    if reached(if block { left } else { Duration::ZERO })? {
        return Ok(true);
    }
    if block || left.is_zero() {
        bail!("PyroWave GPU completion timed out after 2 seconds");
    }
    Ok(false)
}
/// What a frame's readout needs from its submission.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Plan {
    /// The bitstream budget the encode was given.
    budget: usize,
    kbps: u32,
    interval: Duration,
}
/// A frame on the GPU.
struct Flight<L> {
    /// Pins the capture and its conversion inputs until Vulkan confirms the
    /// frame complete.
    lease: L,
    /// The planes, fence and SDK frame context it uses.
    slot: usize,
    /// The slot fence value its encode signals when done.
    release: u64,
    /// The encode call's start, before the colour conversion.
    begin: Instant,
    submitted: Instant,
    captured: Instant,
    plan: Plan,
}
/// A frame Vulkan confirmed complete, its lease returned.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Completed {
    slot: usize,
    begin: Instant,
    captured: Instant,
    plan: Plan,
}
/// Frames on the GPU, oldest first. Only the oldest completes, so frames
/// leave in the order they were submitted.
struct Flights<L> {
    queue: VecDeque<Flight<L>>,
    /// A completion could not be confirmed: every lease and resource stays.
    unconfirmed: bool,
}
impl<L> Default for Flights<L> {
    fn default() -> Self {
        Self {
            queue: VecDeque::with_capacity(MAX_DEPTH),
            unconfirmed: false,
        }
    }
}
impl<L> Flights<L> {
    fn len(&self) -> usize {
        self.queue.len()
    }
    fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }
    fn front(&self) -> Option<&Flight<L>> {
        self.queue.front()
    }
    /// The lowest of `depth` slots no frame in flight uses.
    fn free_slot(&self, depth: usize) -> Option<usize> {
        (0..depth).find(|slot| self.queue.iter().all(|flight| flight.slot != *slot))
    }
    fn push(&mut self, flight: Flight<L>) {
        self.queue.push_back(flight);
    }
    /// The oldest frame, once `done` confirms it complete; its lease goes
    /// back then. A frame `done` fails stays in flight, unconfirmed.
    fn complete(
        &mut self,
        done: impl FnOnce(&Flight<L>) -> Result<bool>,
    ) -> Result<Option<Completed>> {
        let Some(oldest) = self.queue.front() else {
            return Ok(None);
        };
        match done(oldest) {
            Ok(true) => {}
            Ok(false) => return Ok(None),
            Err(error) => {
                self.unconfirmed = true;
                return Err(error);
            }
        }
        let flight = self.queue.pop_front().expect("the oldest frame");
        let completed = Completed {
            slot: flight.slot,
            begin: flight.begin,
            captured: flight.captured,
            plan: flight.plan,
        };
        drop(flight.lease);
        Ok(Some(completed))
    }
    /// Every frame in order, each confirmed by `done`, which waits for it.
    fn settle_all(&mut self, mut done: impl FnMut(&Flight<L>) -> Result<bool>) -> Result<()> {
        while !self.is_empty() {
            if self.complete(&mut done)?.is_none() {
                self.unconfirmed = true;
                bail!("PyroWave GPU completion was not confirmed");
            }
        }
        Ok(())
    }
    /// The device that converted the frames is lost: its queued work is dead
    /// and the leases are free. Vulkan reads only the slots' own planes.
    fn abandon(&mut self) {
        self.queue.clear();
    }
    /// Keeps every lease alive until the process exits.
    fn retain(&mut self) {
        for flight in self.queue.drain(..) {
            std::mem::forget(flight);
        }
    }
}
/// One frame's conversion target: its colour planes, their Vulkan images and
/// the fence between the conversion and the encode. Each frame in flight has
/// its own; fields drop in this order.
struct Slot {
    compute: Option<ComputePlanes>,
    /// The planes as Vulkan images; the first one's fence orders the frame.
    interop: Vec<Interop>,
    converter: PlanarConverter,
}
impl Slot {
    fn new(
        api: &Arc<Api>,
        d3d: &Device,
        device: p::pyrowave_device,
        config: &Negotiated,
        source: (u32, u32, Pixel),
        use_compute: bool,
        warnings: &butterpollo_core::session::Warnings,
    ) -> Result<Self> {
        let converter = PlanarConverter::new(d3d, config, source)?;
        let mut interop = Vec::with_capacity(converter.textures.len());
        for texture in &converter.textures {
            interop.push(Interop::new(api.clone(), d3d, device, texture.clone())?);
        }
        let mut compute = None;
        if use_compute {
            match ComputePlanes::new(d3d, config, &converter.textures, &interop[0].fence) {
                Ok(planes) => compute = Some(planes),
                Err(error) => {
                    warnings.set("pyrowave_conversion", format!("PyroWave compute conversion unavailable ({error:#}); converting on the graphics queue. Game rendering may delay frames; update the AMD driver or lower GPU load."))
                }
            }
        }
        Ok(Self {
            compute,
            interop,
            converter,
        })
    }
    fn semaphore(&self) -> p::VkSemaphore {
        let i = &self.interop[0];
        // SAFETY: `sync` is the live sync object `Interop::new` imported on this device.
        unsafe { (i.api.sync_object_get_semaphore)(i.sync) }
    }
}
pub struct Encoder {
    warnings: Arc<butterpollo_core::session::Warnings>,
    api: Arc<Api>,
    device: p::pyrowave_device,
    encoder: p::pyrowave_encoder,
    completion: Option<CompletionWait>,
    /// The capture lease of the frame being submitted, from before its
    /// conversion until it is in flight. It remains set on every failed
    /// conversion or submission.
    submitting: Option<GpuImage>,
    /// Frames on the GPU, oldest first, each pinning its capture lease until
    /// Vulkan confirms it complete.
    flights: Flights<GpuImage>,
    /// Frames in flight at most: two with `pyrowave_encode_overlap`, the next
    /// one's conversion overlapping the last one's encode; one without, which
    /// waits for each frame in its encode call.
    depth: usize,
    lost: Option<crate::device_loss::DeviceLost>,
    adapter: [u8; 8],
    d3d: Device,
    /// Whether to convert on the compute queue (AMD, `gpu_compute_conversion`).
    use_compute: bool,
    source: Option<(u32, u32, Pixel)>,
    /// One conversion target per frame in flight, made when first needed.
    slots: Vec<Slot>,
    config: Negotiated,
    critical_fec: bool,
    /// Parity for each frame's critical block, as a percentage; 0 without.
    critical_fec_percentage: u32,
    interval: pyrowave::Interval,
    /// The next picture repeats the last one encoded.
    pub(crate) repeat: bool,
    pub(crate) luminance: [f32; 2],
    /// The couch menu's panel, drawn by the conversion while set.
    pub(crate) overlay: Option<crate::overlay::Overlay>,
    packets: Vec<p::pyrowave_packet>,
    /// When the last frame was submitted, after its conversion.
    submitted: Option<Instant>,
    /// Set once the oldest frame in flight is done, with two in flight.
    output_event: Option<crate::timing::Signal>,
    /// The slot and fence value the event was last armed for.
    armed: Cell<Option<(usize, u64)>>,
    row_order_notice: Notice,
    size_notice: Notice,
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
        let overlap = tuning.pyrowave_encode_overlap();
        if overlap && api.set_frame_context.is_none() {
            tracing::warn!(
                "the PyroWave runtime has no frame contexts; encoding one frame at a time"
            );
        }
        let depth = if overlap && api.set_frame_context.is_some() {
            MAX_DEPTH
        } else {
            1
        };
        tracing::info!(frames_in_flight = depth, "PyroWave encode overlap");
        let mut s = Self {
            warnings,
            api,
            device: ptr::null_mut(),
            encoder: ptr::null_mut(),
            completion: None,
            submitting: None,
            flights: Flights::default(),
            depth,
            lost: None,
            adapter: luid.luid,
            use_compute: crate::compute::enabled(tuning),
            d3d,
            source: None,
            slots: Vec::with_capacity(depth),
            config: config.clone(),
            critical_fec,
            critical_fec_percentage,
            interval: interval(config),
            repeat: false,
            luminance: [100., 1.],
            overlay: None,
            packets: vec![],
            submitted: None,
            output_event: if depth > 1 {
                crate::timing::Signal::new().ok()
            } else {
                None
            },
            armed: Cell::new(None),
            row_order_notice: Notice::default(),
            size_notice: Notice::default(),
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
        let compute = self.slots.first()?.compute.as_ref()?.converter.compute();
        crate::device_loss::DeviceLost::d3d12(&compute.device)
    }
    /// A D3D device is lost: its queued work is dead, so the capture leases
    /// are free and the adapter is not quarantined; the host rebuilds the
    /// encoder on a new device. Vulkan reads only the slots' own planes, which
    /// stay until Drop has idled it. A frame whose completion was already
    /// unconfirmed keeps everything for the quarantine.
    fn device_lost(&mut self, loss: crate::device_loss::DeviceLost) {
        self.lost = Some(loss);
        if !self.flights.unconfirmed {
            self.submitting = None;
            self.flights.abandon();
        }
    }
    /// A conversion failed before anything reached Vulkan. When the device it
    /// ran on is lost, see [`Self::device_lost`]. Any other failure keeps the
    /// lease and quarantines.
    fn conversion_failed(&mut self, error: anyhow::Error) -> anyhow::Error {
        match self.device_removed() {
            Some(loss) => {
                self.device_lost(loss);
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
    /// A CPU image, such as the codec probe's: its frame comes back from this
    /// call, with any frame still in flight.
    pub fn encode(&mut self, image: &Image, idr: bool, kbps: u32) -> Result<Vec<Encoded>> {
        anyhow::ensure!(
            self.overlay.is_none(),
            "couch menu: needs wgc_direct_encoder_input"
        );
        let mut frames = self.encode_gpu(&GpuImage::upload(&self.d3d, image)?, idr, kbps)?;
        frames.extend(self.finish()?);
        Ok(frames)
    }
    /// Submits `image` and returns the frames done by now, oldest first. With
    /// one frame in flight that is this one; with two, usually the one before
    /// it, and this one comes from [`Self::poll`].
    pub fn encode_gpu(&mut self, image: &GpuImage, _idr: bool, kbps: u32) -> Result<Vec<Encoded>> {
        let result = self.encode_gpu_inner(image, kbps);
        self.settled(result)
    }
    /// The frames done by now, oldest first.
    pub fn poll(&mut self) -> Result<Vec<Encoded>> {
        let result = self.collect(false);
        self.settled(result)
    }
    /// Waits for every frame in flight and returns them, oldest first.
    pub fn finish(&mut self) -> Result<Vec<Encoded>> {
        let result = self.collect(true);
        self.settled(result)
    }
    pub fn pending(&self) -> bool {
        !self.flights.is_empty()
    }
    /// Frames submitted and not yet returned.
    pub fn backlog(&self) -> usize {
        self.flights.len()
    }
    pub fn last_submitted(&self) -> Option<Instant> {
        self.submitted
    }
    /// An event set once the oldest frame in flight is done, armed by this
    /// call; `None` when nothing is in flight or there is no event, and the
    /// caller keeps its output poll, which stays the bound of any wait.
    pub fn output_event(&self) -> Option<&crate::timing::Signal> {
        let event = self.output_event.as_ref()?;
        match self.arm_output(event) {
            Ok(armed) => armed.then_some(event),
            Err(error) => {
                self.warnings.event(
                    "pyrowave_output_event",
                    format!("PyroWave could not arm its output event ({error:#}); polling for output instead. Report this with the host log."),
                    butterpollo_core::session::EVENT_PERIOD,
                );
                None
            }
        }
    }
    fn arm_output(&self, event: &crate::timing::Signal) -> Result<bool> {
        let Some(oldest) = self.flights.front() else {
            return Ok(false);
        };
        let fence = &self.slots[oldest.slot].interop[0].fence;
        let wanted = (oldest.slot, oldest.release);
        // SAFETY: The fence is the slot's live shared fence, which outlives the
        // frames in flight; `event` is an open event this encoder keeps until
        // Drop or until it arms it again.
        unsafe {
            if fence.GetCompletedValue() >= oldest.release {
                event.set()?;
            } else if self.armed.get() != Some(wanted) {
                event.reset()?;
                fence.SetEventOnCompletion(oldest.release, event.raw())?;
                self.armed.set(Some(wanted));
            } else if event.is_set() {
                event.reset()?;
                // Done meanwhile: its notification may have come before the reset.
                if fence.GetCompletedValue() >= oldest.release {
                    event.set()?;
                }
            }
        }
        Ok(true)
    }
    /// A submission whose completion is not confirmed, or a frame whose
    /// conversion or submission failed, still holds its resources.
    fn unsettled(&self) -> bool {
        self.submitting.is_some() || self.flights.unconfirmed
    }
    /// Quarantines the adapter when `result` failed with GPU work unconfirmed.
    fn settled<T>(&self, result: Result<T>) -> Result<T> {
        if result.is_err() && self.unsettled() {
            self.quarantine();
        }
        result
    }
    fn quarantine(&self) {
        quarantined_adapters().block(self.adapter);
        for compute in self.slots.iter().filter_map(|slot| slot.compute.as_ref()) {
            compute.converter.compute().retire();
        }
        self.warnings.set("pyrowave_gpu_quarantine", "PyroWave stopped after GPU work could not be confirmed complete. Its resources are retained to protect capture; restart Rubylight before using PyroWave on this adapter again, or use HEVC/AV1.");
    }
    /// The frames done by now, oldest first; with `block`, every frame in
    /// flight, waiting for each.
    fn collect(&mut self, block: bool) -> Result<Vec<Encoded>> {
        let mut frames = Vec::new();
        while let Some(frame) = self.complete(block)? {
            frames.extend(frame);
        }
        Ok(frames)
    }
    /// The oldest frame in flight, read out once Vulkan confirms it complete:
    /// `None` while it is not (or nothing is in flight), `Some(None)` when it
    /// is dropped over the transport limit.
    fn complete(&mut self, block: bool) -> Result<Option<Option<Encoded>>> {
        let completion = self
            .completion
            .as_ref()
            .context("PyroWave completion wait")?;
        let slots = &self.slots;
        let completed = self.flights.complete(|flight| {
            settle(
                |timeout| {
                    completion.reached(slots[flight.slot].semaphore(), flight.release, timeout)
                },
                flight.submitted,
                Instant::now(),
                block,
            )
        });
        let completed = match completed {
            Ok(Some(completed)) => completed,
            Ok(None) => return Ok(None),
            Err(error) => {
                // A Vulkan loss (VK_ERROR_DEVICE_LOST) keeps the lease and
                // quarantines: nothing proves the driver stopped reading the
                // imported capture texture, unlike a D3D loss below.
                if let Some(loss) = crate::device_loss::DeviceLost::from_error(&error) {
                    self.lost = Some(loss);
                }
                return Err(error);
            }
        };
        // The lease is free whatever the wait proved: Vulkan never reads the
        // capture texture, only the encoder-owned planes of the slot. Ask now
        // whether a device died under the frame: a lost compute device
        // signals its fences to the maximum, which can satisfy that wait
        // before Vulkan has run the encode (Drop idles Vulkan before it
        // destroys anything). The output is garbage, and the host rebuilds on
        // a new device; the adapter is not quarantined. On the graphics path
        // (compute planes present, frame not shareable) the D3D11 conversion
        // may still be reading the capture when the lease is returned;
        // capture can then tear a buffer that is discarded anyway. This is
        // the second GetDeviceRemovedReason pair per frame (the first is the
        // entry check). Both stay: the entry check keeps a dead device from
        // being handed new work, this one is the only check that sees a loss
        // during the frame. Each is a runtime flag read on two devices, not a
        // GPU round trip; not measured.
        if let Some(loss) = self.device_removed() {
            self.device_lost(loss);
            return Err(loss.into());
        }
        self.read_out(completed).map(Some)
    }
    fn encode_gpu_inner(&mut self, image: &GpuImage, kbps: u32) -> Result<Vec<Encoded>> {
        // Refuse a device already known or found lost before it gets new work.
        if let Some(loss) = self.device_removed() {
            self.device_lost(loss);
            return Err(loss.into());
        }
        quarantined_adapters().check(self.adapter)?;
        anyhow::ensure!(
            !self.unsettled(),
            "PyroWave encoder has an unconfirmed GPU submission; rebuild it before encoding again"
        );
        if !self.accepts_gpu_device(image) {
            bail!("capture device changed");
        }
        let begin = Instant::now();
        let mut frames = Vec::new();
        let source = (image.width, image.height, image.pixel);
        if self.source != Some(source) {
            // The frames in flight still read the planes about to go.
            frames.extend(self.collect(true)?);
            self.slots.clear();
            self.armed.set(None);
            self.warnings.clear("pyrowave_conversion");
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
            frames.extend(self.collect(false)?);
            return Ok(frames);
        }
        // A caller past the backlog waits for the oldest frame here.
        while self.flights.len() >= self.depth {
            match self.complete(true)? {
                Some(frame) => frames.extend(frame),
                None => break,
            }
        }
        let slot = self
            .flights
            .free_slot(self.depth)
            .context("no free PyroWave frame slot")?;
        if slot == self.slots.len() {
            self.slots.push(Slot::new(
                &self.api,
                &self.d3d,
                self.device,
                &self.config,
                source,
                self.use_compute,
                &self.warnings,
            )?);
        }
        self.submitting = Some(image.clone());
        let release = self.submit(slot, image, budget)?;
        let submitted = Instant::now();
        self.submitted = Some(submitted);
        // SDK 502a3b52 submits the metadata/bitstream host copies and their
        // COPY→HOST barrier before this same-queue release signal. Granite
        // signals it at ALL_COMMANDS, so this covers the output writes as
        // well as the input reads; the getters of the readout submit no GPU
        // work.
        #[cfg(test)]
        if let Some(fault) = self.hooks.after_submit.take() {
            fault();
        }
        let lease = self.submitting.take().context("PyroWave capture lease")?;
        self.flights.push(Flight {
            lease,
            slot,
            release,
            begin,
            submitted,
            captured: image.captured,
            plan: Plan {
                budget,
                kbps,
                interval,
            },
        });
        // One frame in flight waits for it here, as the encode always did.
        frames.extend(self.collect(self.depth == 1)?);
        Ok(frames)
    }
    /// Converts `image` into `slot`'s planes and submits their encode;
    /// returns the fence value the encode signals when done.
    fn submit(&mut self, slot: usize, image: &GpuImage, budget: usize) -> Result<u64> {
        #[cfg(test)]
        if let Some(fault) = self.hooks.before_conversion.take() {
            fault();
        }
        let Slot {
            compute,
            interop,
            converter,
        } = &mut self.slots[slot];
        let images: [p::pyrowave_gpu_external_reference; 3] =
            std::array::from_fn(|plane| p::pyrowave_gpu_external_reference {
                image: interop[plane].image,
                queue_family_index: 0xfffffffe,
            });
        let views = [interop[0].view, interop[1].view, interop[2].view];
        let i = &mut interop[0];
        let after = i.counter;
        i.counter = i
            .counter
            .checked_add(1)
            .context("PyroWave fence exhausted")?;
        let compute = compute
            .as_mut()
            .filter(|_| crate::compute::shareable(&image.texture));
        let converted = if let Some(compute) = compute {
            compute.converter.set_overlay(self.overlay.as_ref());
            compute.convert(image, &self.config, self.luminance, after, i.counter)
        } else {
            converter.set_overlay(self.overlay.as_ref());
            let converted = converter.convert(image, self.luminance);
            // SAFETY: `i.context` and `i.fence` belong to the live D3D11 device
            // the conversion recorded on; Signal and Flush only queue work.
            converted.and_then(|()| unsafe {
                i.context.Signal(&i.fence, i.counter)?;
                // Submit the conversion and the signal now, not whenever
                // D3D11 next flushes: the encode below waits for that signal.
                i.context.Flush();
                Ok(())
            })
        };
        if let Err(error) = converted {
            return Err(self.conversion_failed(error));
        }
        let Slot { interop, .. } = &mut self.slots[slot];
        let i = &mut interop[0];
        // SAFETY: `i.sync` is the slot's live sync object on `self.device`.
        let semaphore = unsafe { (self.api.sync_object_get_semaphore)(i.sync) };
        let acquire = p::pyrowave_gpu_sync_operation {
            images: images.as_ptr(),
            num_images: 3,
            sync: p::pyrowave_sync_point {
                semaphore,
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
        let buffers = p::pyrowave_gpu_buffers { planes: views };
        // SAFETY: `images`, `acquire`, `release` and `buffers` are live locals
        // naming the slot's images and views made on `self.device`; the call
        // records and submits before it returns.
        unsafe {
            if self.depth > 1 {
                let set = self
                    .api
                    .set_frame_context
                    .context("PyroWave frame contexts unavailable")?;
                check(set(self.encoder, slot as std::ffi::c_int))?;
            }
            check((self.api.encoder_encode_gpu)(
                self.encoder,
                &acquire,
                &release,
                &buffers,
                &p::pyrowave_rate_control {
                    maximum_bitstream_size: budget,
                },
            ))?;
        }
        Ok(release.sync.value)
    }
    /// A completed frame's bitstream, framed for the client.
    fn read_out(&mut self, frame: Completed) -> Result<Option<Encoded>> {
        let Plan {
            budget,
            kbps,
            interval,
        } = frame.plan;
        // SAFETY: The getters read the frame context's host-visible readback
        // of an encode Vulkan confirmed complete; `bytes` and `self.packets`
        // are sized as passed.
        let (mut bytes, written) = unsafe {
            if self.depth > 1 {
                let set = self
                    .api
                    .set_frame_context
                    .context("PyroWave frame contexts unavailable")?;
                check(set(self.encoder, frame.slot as std::ffi::c_int))?;
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
            // The codec writes every byte the packets cover; framing below
            // refers to these bytes instead of copying them again.
            let mut bytes = crate::frame_pool::take_filled(size + 8);
            self.packets
                .resize_with(count, || p::pyrowave_packet { offset: 0, size: 0 });
            let mut written = 0;
            check((self.api.encoder_packetize_with_padding)(
                self.encoder,
                self.packets.as_mut_ptr(),
                boundary,
                0,
                &mut written,
                bytes.as_mut_ptr().cast(),
                bytes.len(),
            ))?;
            if written == 0 || written > count {
                bail!("invalid PyroWave packetization");
            }
            (bytes, written)
        };
        let limit = pyrowave::max_frame_bytes(self.config.packet_size, self.critical_fec);
        let framing = if self.config.pyrowave_records {
            if written != 1 {
                bail!("record frame must be one codec packet");
            }
            let (offset, size) = (self.packets[0].offset, self.packets[0].size);
            let end = offset.checked_add(size).context("packet overflow")?;
            anyhow::ensure!(end <= bytes.len(), "invalid codec record range");
            // The record frame is the codec packet itself.
            if offset > 0 {
                bytes.copy_within(offset..end, 0);
            }
            bytes.truncate(size);
            let payload = pyrowave::aligned_payload(self.config.packet_size);
            let framing =
                pyrowave::Framing::records(&bytes, payload, limit, self.config.pyrowave_row_order)?;
            let chosen = if framing.len() <= budget {
                Some(framing)
            } else {
                let layout = pyrowave::layout(framing.view(&bytes)?, payload)?;
                let minimum = self.config.min_fec.max(pyrowave::MIN_PARITY_SHARDS);
                let packets = (framing.len() + 8).div_ceil(self.config.packet_size - 16)
                    + pyrowave::critical_parity_bound(
                        layout.critical_shards,
                        self.critical_fec_percentage as usize,
                        minimum,
                    );
                let allowance = (f64::from(kbps) * 125. * interval.as_secs_f64()) as usize;
                if packets * (self.config.packet_size + pyrowave::WIRE_OVERHEAD) <= allowance {
                    Some(framing)
                } else {
                    if self.config.pyrowave_row_order {
                        self.warnings.event(
                            "pyrowave_row_order",
                            row_order_dropped_message(self.config.packet_size),
                            butterpollo_core::session::EVENT_PERIOD,
                        );
                        if self
                            .row_order_notice
                            .due(Instant::now(), butterpollo_core::session::EVENT_PERIOD)
                        {
                            tracing::warn!(
                                "PyroWave row order dropped: {} bytes framed ({} unframed) against a budget of {budget} bytes and a wire allowance of {allowance} bytes",
                                framing.len(),
                                bytes.len()
                            );
                        }
                    }
                    // The codec's packet as it is.
                    None
                }
            };
            // Framing that leaves the packet whole sends it as it is.
            chosen.filter(|framing| !framing.is_whole(&bytes))
        } else {
            Some(pyrowave::Framing::container(
                &bytes,
                self.packets[..written].iter().map(|p| (p.offset, p.size)),
            )?)
        };
        let encoded = Encoded {
            bytes,
            framing,
            unit: crate::encoder::Unit::Whole,
            idr: true,
            after_invalidation: false,
            frame_cap_bytes: None,
            reencoded: false,
            frame_cap_target: None,
            latency: Some(frame.begin.elapsed()),
            presentation: Some(frame.captured),
        };
        if encoded.wire_len() > limit {
            self.warnings.event(
                "pyrowave_frame_size",
                frame_size_message(limit),
                butterpollo_core::session::EVENT_PERIOD,
            );
            if self
                .size_notice
                .due(Instant::now(), butterpollo_core::session::EVENT_PERIOD)
            {
                tracing::warn!(
                    "PyroWave frame of {} bytes dropped over the transport limit of {limit}",
                    encoded.wire_len()
                );
            }
            // Its buffer goes back to the pool as it drops.
            return Ok(None);
        }
        Ok(Some(encoded))
    }
    /// Encodes `image` and waits for it, for tests that check each frame.
    #[cfg(test)]
    pub(crate) fn encode_now(
        &mut self,
        image: &GpuImage,
        idr: bool,
        kbps: u32,
    ) -> Result<Vec<Encoded>> {
        let mut frames = self.encode_gpu(image, idr, kbps)?;
        frames.extend(self.finish()?);
        Ok(frames)
    }
    /// Whether the first slot converts on the compute queue.
    #[cfg(test)]
    pub(crate) fn converts_on_compute(&self) -> bool {
        self.slots
            .first()
            .is_some_and(|slot| slot.compute.is_some())
    }
    /// The compute converter of the first slot.
    #[cfg(test)]
    pub(crate) fn compute(&self) -> Option<&Arc<crate::compute::Compute>> {
        Some(self.slots.first()?.compute.as_ref()?.converter.compute())
    }
    /// The Vulkan images of the first slot's planes.
    #[cfg(test)]
    pub(crate) fn plane_images(&self) -> Vec<usize> {
        self.slots.first().map_or_else(Vec::new, |slot| {
            slot.interop.iter().map(|i| i.view.image as usize).collect()
        })
    }
    /// Capture leases the encoder holds.
    #[cfg(test)]
    pub(crate) fn leases(&self) -> usize {
        self.flights.len() + usize::from(self.submitting.is_some())
    }
}
impl Drop for Encoder {
    fn drop(&mut self) {
        // The frames still in flight, each within its own completion bound,
        // as at the end of a stream or a rebuild.
        let confirmed = !self.unsettled()
            && match &self.completion {
                Some(completion) => {
                    let slots = &self.slots;
                    self.flights
                        .settle_all(|flight| {
                            settle(
                                |timeout| {
                                    completion.reached(
                                        slots[flight.slot].semaphore(),
                                        flight.release,
                                        timeout,
                                    )
                                },
                                flight.submitted,
                                Instant::now(),
                                true,
                            )
                        })
                        .is_ok()
                }
                None => self.flights.is_empty(),
            };
        if !confirmed {
            // Keep both APIs, all imported images/fences, conversion commands,
            // and the capture leases alive. No queued wait is added to capture.
            self.quarantine();
            tracing::warn!("retaining PyroWave resources after an unconfirmed GPU submission");
            retain_pending(&mut self.submitting);
            self.flights.retain();
            retain_pending(&mut self.completion);
            std::mem::forget(std::mem::take(&mut self.slots));
            std::mem::forget(self.api.clone());
            std::mem::forget(self.d3d.clone());
            return;
        }
        // SAFETY: `encoder` and `device` were created in `new_device_reported`; the slots' images
        // go before the device. After a device loss the frames in flight are already gone while
        // Vulkan may still be running or queued, because the lost fence satisfies the completion
        // wait early. `pyrowave_encoder_destroy` goes first: pyrowave.h (SDK 502a3b52,
        // "Implementation ensures GPU is idle before destroying objects") idles the GPU there,
        // before the images and the device are destroyed.
        unsafe {
            if !self.encoder.is_null() {
                (self.api.encoder_destroy)(self.encoder);
            }
            self.slots.clear();
            if !self.device.is_null() {
                (self.api.device_destroy)(self.device);
            }
        }
    }
}

#[cfg(test)]
mod tests;
