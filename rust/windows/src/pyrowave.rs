//! Rust-owned D3D12/Vulkan PyroWave encoder using the stable 2.0 bitstream.
//!
//! Each frame is converted into three plane textures on the capture GPU's D3D12 compute queue
//! ([`crate::compute::Converter::convert_planes`]) and encoded on PyroWave's Vulkan queue. The
//! planes are D3D12 shared resources imported into Vulkan, and one shared D3D12 fence, imported
//! as a Vulkan timeline semaphore, orders the two queues: the compute queue waits for the
//! encode's release value before it writes the planes and signals the next value when done,
//! which the encode waits for before it reads them.

use crate::{
    capture::{Device, GpuImage, Image, Pixel},
    encoder::Encoded,
    pyro_abi as p,
};
use anyhow::{Context, Result, bail};
use butterpollo_core::{
    pyrowave::{self, PACKET_BOUNDARY},
    rtsp::Negotiated,
};
use std::{
    ffi::c_void,
    os::windows::io::{IntoRawHandle, OwnedHandle},
    ptr,
    sync::Arc,
    time::Instant,
};
use windows::Win32::{
    Foundation::{CloseHandle, HANDLE},
    Graphics::{
        Direct3D12::{D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS, ID3D12Resource},
        Dxgi::Common::*,
    },
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
pub fn available() -> bool {
    Api::load().is_ok()
}

/// Pass `handle` to a PyroWave import, which takes ownership of it when it succeeds.
///
/// # Safety
///
/// `import` must import the raw handle it is given and own it only on success.
unsafe fn import(
    handle: OwnedHandle,
    import: impl FnOnce(usize) -> p::pyrowave_result,
) -> Result<()> {
    let raw = handle.into_raw_handle();
    let result = import(raw as usize);
    if result != 0 {
        // SAFETY: the import failed, so the handle is still this function's to close.
        let _ = unsafe { CloseHandle(HANDLE(raw)) };
    }
    check(result)
}

/// A D3D12 texture on the capture GPU's compute device imported into PyroWave's Vulkan device.
struct Plane {
    api: Arc<Api>,
    image: p::pyrowave_image,
    view: p::pyrowave_image_view,
    texture: ID3D12Resource,
}
impl Plane {
    /// A `width` x `height` plane in `format` (R8 or R16 UNORM) that the compute queue writes
    /// (`usage` for Vulkan's side).
    fn new(
        api: Arc<Api>,
        pyro: p::pyrowave_device,
        compute: &crate::compute::Compute,
        (width, height): (u32, u32),
        format: DXGI_FORMAT,
        usage: p::VkImageUsageFlagBits,
    ) -> Result<Self> {
        let (texture, handle) = compute.share_texture(
            width,
            height,
            format,
            D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
        )?;
        // SAFETY: the create-infos outlive their calls, `s` owns the image so Drop frees it on
        // error, and the import takes the shared handle only on success.
        unsafe {
            let mut s = Self {
                api,
                image: ptr::null_mut(),
                view: std::mem::zeroed(),
                texture,
            };
            let info = p::VkImageCreateInfo {
                sType: p::VkStructureType_VK_STRUCTURE_TYPE_IMAGE_CREATE_INFO,
                imageType: p::VkImageType_VK_IMAGE_TYPE_2D,
                format: if format == DXGI_FORMAT_R16_UNORM {
                    p::VkFormat_VK_FORMAT_R16_UNORM
                } else {
                    p::VkFormat_VK_FORMAT_R8_UNORM
                },
                extent: p::VkExtent3D {
                    width,
                    height,
                    depth: 1,
                },
                mipLevels: 1,
                arrayLayers: 1,
                samples: p::VkSampleCountFlagBits_VK_SAMPLE_COUNT_1_BIT,
                tiling: p::VkImageTiling_VK_IMAGE_TILING_OPTIMAL,
                usage,
                sharingMode: p::VkSharingMode_VK_SHARING_MODE_EXCLUSIVE,
                ..std::mem::zeroed()
            };
            let (create, out) = (s.api.image_create, &mut s.image);
            import(handle, |handle| {
                create(
                    &p::pyrowave_image_create_info {
                        device: pyro,
                        external_handle: handle,
                        handle_type: p::VkExternalMemoryHandleTypeFlagBits_VK_EXTERNAL_MEMORY_HANDLE_TYPE_D3D12_RESOURCE_BIT,
                        image_create_info: &info,
                    },
                    out,
                )
            })
            .context("importing a D3D12 plane into Vulkan")?;
            check((s.api.image_get_image_view)(
                s.image,
                p::VkImageAspectFlagBits_VK_IMAGE_ASPECT_COLOR_BIT,
                if usage & p::VkImageUsageFlagBits_VK_IMAGE_USAGE_STORAGE_BIT != 0 {
                    p::VkImageUsageFlagBits_VK_IMAGE_USAGE_STORAGE_BIT
                } else {
                    p::VkImageUsageFlagBits_VK_IMAGE_USAGE_SAMPLED_BIT
                },
                &mut s.view,
            ))?;
            Ok(s)
        }
    }
}
impl Drop for Plane {
    fn drop(&mut self) {
        // SAFETY: `image` was created on the Encoder's device, which is destroyed after the
        // planes.
        unsafe {
            if !self.image.is_null() {
                (self.api.image_destroy)(self.image);
            }
        }
    }
}
/// A shared D3D12 fence on the compute device and the Vulkan timeline semaphore imported
/// from it.
struct Timeline {
    api: Arc<Api>,
    sync: p::pyrowave_sync_object,
    fence: windows::Win32::Graphics::Direct3D12::ID3D12Fence,
}
impl Timeline {
    fn new(
        api: Arc<Api>,
        pyro: p::pyrowave_device,
        compute: &crate::compute::Compute,
    ) -> Result<Self> {
        let (fence, handle) = compute.share_fence()?;
        let mut s = Self {
            api,
            sync: ptr::null_mut(),
            fence,
        };
        let (create, out) = (s.api.sync_object_create, &mut s.sync);
        // SAFETY: the create-info outlives the call, `s` owns the sync object so Drop frees it,
        // and the import takes the shared handle only on success.
        unsafe {
            import(handle, |handle| {
                create(
                    &p::pyrowave_sync_object_create_info {
                        device: pyro,
                        external_handle: handle,
                        handle_type: p::VkExternalSemaphoreHandleTypeFlagBits_VK_EXTERNAL_SEMAPHORE_HANDLE_TYPE_D3D12_FENCE_BIT,
                        semaphore_type: p::VkSemaphoreType_VK_SEMAPHORE_TYPE_TIMELINE,
                        import_flags: 0,
                    },
                    out,
                )
            })
            .context("importing the D3D12 fence into Vulkan")?;
        }
        Ok(s)
    }
}
impl Drop for Timeline {
    fn drop(&mut self) {
        // SAFETY: `sync` was created on the Encoder's device, which is destroyed after it.
        unsafe {
            if !self.sync.is_null() {
                (self.api.sync_object_destroy)(self.sync);
            }
        }
    }
}
/// The planes for one source format and the conversion into them on the compute queue.
/// Beside a game using the whole GPU, a graphics-queue conversion of a 1080p HDR 4:4:4 frame
/// waited 5.1 ms, against 0.15 ms idle, nearly all of the encode's 5.7 ms; PyroWave's own GPU
/// work took 0.2 ms either way.
struct Planes {
    // Vulkan's side goes first; the converter then waits for the compute queue before the
    // D3D12 textures it writes go with `target`.
    planes: [Plane; 3],
    timeline: Timeline,
    converter: crate::compute::Converter,
    target: crate::compute::Planes,
    /// The timeline's last value: the encode's release, or before the first frame 0.
    counter: u64,
}
impl Planes {
    fn new(
        api: &Arc<Api>,
        pyro: p::pyrowave_device,
        d3d: &Device,
        config: &Negotiated,
    ) -> Result<Self> {
        let compute = &d3d.compute;
        let format = if config.ten_bit() {
            DXGI_FORMAT_R16_UNORM
        } else {
            DXGI_FORMAT_R8_UNORM
        };
        let divisor = if config.yuv444 { 1 } else { 2 };
        let usage = p::VkImageUsageFlagBits_VK_IMAGE_USAGE_SAMPLED_BIT
            | p::VkImageUsageFlagBits_VK_IMAGE_USAGE_TRANSFER_SRC_BIT
            | p::VkImageUsageFlagBits_VK_IMAGE_USAGE_TRANSFER_DST_BIT;
        let [y, u, v] = [1, divisor, divisor].map(|divisor| {
            Plane::new(
                api.clone(),
                pyro,
                compute,
                (config.width / divisor, config.height / divisor),
                format,
                usage,
            )
        });
        let planes = [y?, u?, v?];
        let timeline = Timeline::new(api.clone(), pyro, compute)?;
        let converter = crate::compute::Converter::new(
            compute.clone(),
            config.width,
            config.height,
            config.ten_bit(),
        )?;
        let target = crate::compute::Planes {
            planes: planes.each_ref().map(|plane| plane.texture.clone()),
            format,
            fence: timeline.fence.clone(),
            after: 0,
            done: 0,
        };
        Ok(Self {
            planes,
            timeline,
            converter,
            target,
            counter: 0,
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
        self.converter.values =
            crate::compute::constants(config, (image.width, image.height, image.pixel), luminance);
        self.converter.values[11] = u32::from(config.yuv444);
        self.target.after = after;
        self.target.done = done;
        self.converter.convert_planes(image, &self.target)
    }
}
impl Drop for Planes {
    fn drop(&mut self) {
        // A conversion may wait on the GPU for a release the encode never submitted after an
        // error. Release it from here so the queue drains before the textures go; nothing
        // uses the timeline after this.
        // SAFETY: the fence is live; a CPU signal only sets its value.
        unsafe {
            if self.timeline.fence.GetCompletedValue() < self.counter {
                let _ = self.timeline.fence.Signal(self.counter);
            }
        }
    }
}
fn interval(config: &Negotiated, critical_fec: bool) -> pyrowave::Interval {
    pyrowave::Interval::new(
        butterpollo_core::framegen::Rate(config.fps_millihz()).period(),
        critical_fec,
    )
}

pub struct Encoder {
    warnings: Arc<butterpollo_core::session::Warnings>,
    api: Arc<Api>,
    device: p::pyrowave_device,
    encoder: p::pyrowave_encoder,
    d3d: Device,
    /// The planes for the current source format, made at its first frame.
    planes: Option<Planes>,
    source: Option<(u32, u32, Pixel)>,
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
        let (low, high) = d3d.luid();
        let mut luid = p::pyrowave_luid { luid: [0; 8] };
        luid.luid[..4].copy_from_slice(&low.to_le_bytes());
        luid.luid[4..].copy_from_slice(&high.to_le_bytes());
        let critical_fec_percentage = if config.pyrowave_records {
            tuning
                .integer("pyrowave_critical_fec_percentage", 20)
                .clamp(0, 255) as u32
        } else {
            0
        };
        let critical_fec = critical_fec_percentage > 0;
        let mut s = Self {
            warnings,
            api,
            device: ptr::null_mut(),
            encoder: ptr::null_mut(),
            d3d,
            planes: None,
            source: None,
            config: config.clone(),
            critical_fec,
            critical_fec_percentage,
            interval: interval(config, critical_fec),
            repeat: false,
            luminance: [100., 1.],
            bitstream: vec![],
            packets: vec![],
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
                bail!("Vulkan device cannot import D3D12 textures and fences");
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
        }
        Ok(s)
    }
    pub fn accepts_gpu_device(&self, image: &GpuImage) -> bool {
        self.d3d.same(&image.gpu)
    }
    /// The next encode gets a first frame's budget, one stream period.
    #[cfg(test)]
    pub(crate) fn restart_interval(&mut self) {
        self.interval = interval(&self.config, self.critical_fec);
    }
    pub fn encode(&mut self, image: &Image, idr: bool, kbps: u32) -> Result<Vec<Encoded>> {
        self.encode_gpu(&GpuImage::upload(&self.d3d, image)?, idr, kbps)
    }
    pub fn encode_gpu(&mut self, image: &GpuImage, _idr: bool, kbps: u32) -> Result<Vec<Encoded>> {
        if !self.accepts_gpu_device(image) {
            bail!("capture device changed");
        }
        let begin = Instant::now();
        let source = (image.width, image.height, image.pixel);
        if self.source != Some(source) || self.planes.is_none() {
            self.planes = None;
            self.source = None;
            self.planes = Some(Planes::new(
                &self.api,
                self.device,
                &self.d3d,
                &self.config,
            )?);
            self.source = Some(source);
        }
        let interval = self.interval.next(begin, std::mem::take(&mut self.repeat));
        let budget = pyrowave::budget(
            kbps,
            interval,
            self.config.packet_size,
            self.config.pyrowave_records,
            self.critical_fec_percentage,
        );
        if budget < 16 {
            return Ok(vec![]);
        }
        let planes = self.planes.as_mut().context("PyroWave planes missing")?;
        // SAFETY: `planes` holds the three images and views made on `self.device`; `images`
        // outlives the synchronous encode and the output buffers are sized as passed.
        unsafe {
            let images: Vec<_> = planes
                .planes
                .iter()
                .map(|plane| p::pyrowave_gpu_external_reference {
                    image: plane.image,
                    queue_family_index: 0xfffffffe,
                })
                .collect();
            let after = planes.counter;
            let done = after.checked_add(1).context("PyroWave fence exhausted")?;
            let release = done.checked_add(1).context("PyroWave fence exhausted")?;
            planes.counter = release;
            planes.convert(image, &self.config, self.luminance, after, done)?;
            let acquire = p::pyrowave_gpu_sync_operation {
                images: images.as_ptr(),
                num_images: 3,
                sync: p::pyrowave_sync_point {
                    semaphore: (self.api.sync_object_get_semaphore)(planes.timeline.sync),
                    value: done,
                },
            };
            let release = p::pyrowave_gpu_sync_operation {
                sync: p::pyrowave_sync_point {
                    value: release,
                    ..acquire.sync
                },
                ..acquire
            };
            let buffers = p::pyrowave_gpu_buffers {
                planes: planes.planes.each_ref().map(|plane| plane.view),
            };
            // The compute queue waits for the release before its next conversion.
            check((self.api.encoder_encode_gpu)(
                self.encoder,
                &acquire,
                &release,
                &buffers,
                &p::pyrowave_rate_control {
                    maximum_bitstream_size: budget,
                },
            ))?;
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
                pyrowave::record_frame(
                    raw,
                    pyrowave::aligned_payload(self.config.packet_size),
                    limit,
                )?
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
                return Ok(vec![]);
            }
            Ok(vec![Encoded {
                bytes,
                idr: true,
                after_invalidation: false,
                latency: Some(begin.elapsed()),
                presentation: Some(image.captured),
            }])
        }
    }
}
impl Drop for Encoder {
    fn drop(&mut self) {
        // SAFETY: `encoder` and `device` were created in `new_device_reported`; the planes' Vulkan
        // images and semaphore go before the device.
        unsafe {
            if !self.encoder.is_null() {
                (self.api.encoder_destroy)(self.encoder);
            }
            self.planes = None;
            if !self.device.is_null() {
                (self.api.device_destroy)(self.device);
            }
        }
    }
}
