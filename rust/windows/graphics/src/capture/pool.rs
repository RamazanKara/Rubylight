//! Capture pixel formats, CPU [`Image`] and immutable GPU [`GpuImage`] snapshots,
//! bounded [`GpuPool`] texture allocation/copying, and staging readback.

use super::{Crop, Device, bridge};
#[cfg(test)]
use anyhow::ensure;
use anyhow::{Context, Result, bail};
#[cfg(test)]
use std::time::Duration;
use std::time::Instant;
use windows::Win32::Graphics::{Direct3D11::*, Dxgi::Common::*};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pixel {
    Bgra8,
    RgbaF16,
    Rgba10Pq,
}

impl Pixel {
    /// Convert a DXGI texture format into a host [`Pixel`] when supported.
    pub fn try_from_dxgi(format: DXGI_FORMAT) -> Option<Self> {
        match format {
            DXGI_FORMAT_B8G8R8A8_UNORM => Some(Self::Bgra8),
            DXGI_FORMAT_R16G16B16A16_FLOAT => Some(Self::RgbaF16),
            DXGI_FORMAT_R10G10B10A2_UNORM => Some(Self::Rgba10Pq),
            _ => None,
        }
    }

    /// [`Self::try_from_dxgi`] from a raw DXGI / WinRT `DirectXPixelFormat` integer.
    pub fn try_from_dxgi_raw(format: i32) -> Option<Self> {
        Self::try_from_dxgi(DXGI_FORMAT(format))
    }
}
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub stride: usize,
    pub bytes: Vec<u8>,
    pub captured: Instant,
    pub pixel: Pixel,
}

/// An immutable, owned GPU snapshot. The capture pool never overwrites a
/// texture until the latest-frame slot and every encoder have released it.
#[derive(Clone)]
pub struct GpuImage {
    pub(crate) cursor: Option<crate::cursor::Cursor>,
    pub width: u32,
    pub height: u32,
    pub pixel: Pixel,
    /// Capture timestamp used by pacing and encoders. Future, missing or
    /// more than two-second-old stamps fall back to the conversion time.
    pub captured: Instant,
    /// Unclamped WGC SystemRelativeTime, for diagnostics only. This is not
    /// an application Present time and can be later than host acquisition.
    pub wgc_stamp: Option<Instant>,
    /// When the host submitted its owned copy, before GPU completion.
    pub acquired: Instant,
    /// When the capture published the picture to the stream threads; until
    /// then, its acquisition. Only timing reports read this.
    pub published: Instant,
    pub gpu: Device,
    pub texture: std::sync::Arc<ID3D11Texture2D>,
    /// When the GPU write into `texture` (a compute-queue copy, or the
    /// display driver's copy into a fence ring) completes. D3D12 queues wait
    /// for this; D3D11 work on `gpu` is ordered after it once
    /// [`Self::order_d3d11`] ran.
    pub(crate) ready: Option<crate::compute::Ready>,
    /// A shared texture read in place rather than copied (a WGC helper's or
    /// the display driver's); it goes back with the last clone of this image.
    pub(super) lease: Option<std::sync::Arc<bridge::Lease>>,
    /// Whether the picture changed the foreground window, from WGC's dirty
    /// regions: `None` when the capture cannot tell (`Freshness::observe_focus`).
    pub focus: Option<bool>,
    /// This picture repeats the previous one: constant-rate capture
    /// (`capture = wgcc`) published it again because nothing was composed for
    /// the stream's slot. Its stamps are the publication's, not a composition's.
    pub repeat: bool,
    /// The capture publication that first carried this content, when the
    /// capture publishes repeats (`capture = wgcc`): a repeat keeps it, so an
    /// enhancement reuses the picture it made from the same content. 0 when
    /// the capture does not repeat.
    pub origin: u64,
}
/// A shown pointer Desktop Duplication supplied beside a picture.
pub struct PointerShape<'a> {
    pub width: u32,
    pub height: u32,
    /// The point that clicks, from the shape's top left.
    pub hotspot: [i32; 2],
    /// Logic alpha (0 transparent, 128 inverts the screen by the pixel's color, 255
    /// opaque) for monochrome and masked shapes; straight alpha otherwise.
    pub logic: bool,
    /// `width * height` RGBA pixels, rows top to bottom; a shape seen again shares them.
    pub pixels: &'a std::sync::Arc<[u8]>,
}
impl GpuImage {
    /// The pointer drawn into this picture, when the capture supplies it apart from the
    /// desktop (Desktop Duplication) and it is shown.
    pub fn pointer(&self) -> Option<PointerShape<'_>> {
        self.cursor.as_ref().map(|cursor| PointerShape {
            width: cursor.width,
            height: cursor.height,
            hotspot: cursor.hotspot,
            logic: cursor.logic,
            pixels: &cursor.pixels,
        })
    }
    /// Wrap a pooled or rendered texture with no lease or detached cursor and
    /// current timestamps. Callers deriving from an existing capture follow
    /// this with [`Self::inherit`].
    pub(crate) fn from_texture(
        gpu: Device,
        texture: std::sync::Arc<ID3D11Texture2D>,
        width: u32,
        height: u32,
        pixel: Pixel,
        ready: Option<crate::compute::Ready>,
    ) -> Self {
        let now = Instant::now();
        Self {
            cursor: None,
            width,
            height,
            pixel,
            captured: now,
            wgc_stamp: None,
            acquired: now,
            published: now,
            gpu,
            texture,
            ready,
            lease: None,
            focus: None,
            repeat: false,
            origin: 0,
        }
    }
    /// Take `source`'s timing, pointer, focus and content identity for a
    /// picture made from it (the NVIDIA image enhancements).
    pub(crate) fn inherit(&mut self, source: &GpuImage) {
        self.captured = source.captured;
        self.wgc_stamp = source.wgc_stamp;
        self.acquired = source.acquired;
        self.published = source.published;
        self.cursor = source.cursor.clone();
        self.focus = source.focus;
        self.repeat = source.repeat;
        self.origin = source.origin;
    }
    /// Leave the pointer out of this picture, for a device that draws it itself. Only a
    /// pointer the capture supplies apart can be left out.
    pub fn leave_pointer_out(&mut self) {
        self.cursor = None;
    }
    /// Order D3D11 work on `gpu` submitted after this call after the write
    /// `ready` waits for. Every D3D11 reader of `texture` calls it first; only
    /// a fence ring's texture lent in place needs it, which the compute
    /// conversion otherwise reads without D3D11.
    pub(crate) fn order_d3d11(&self) -> Result<()> {
        match &self.lease {
            Some(lease) => lease.order_d3d11(),
            None => Ok(()),
        }
    }
    pub fn readback(&self, staging: &mut Option<ID3D11Texture2D>) -> Result<Image> {
        // CPU readback needs the completed pixels anyway. Bound the producer wait before
        // queuing D3D11 work: Map otherwise blocks behind a missing external fence signal,
        // preventing capture recovery. GPU-only readers keep their asynchronous waits.
        if let Some(ready) = &self.ready {
            ready
                .wait()
                .inspect_err(|_| {
                    if let Some(lease) = &self.lease {
                        lease.reject();
                    }
                })
                .context("wait for captured pixels before CPU readback")?;
        }
        self.order_d3d11()?;
        let mut image = read_texture(&self.gpu, &self.texture, staging)?;
        image.captured = self.captured;
        if let Some(cursor) = &self.cursor {
            cursor.blend(&mut image);
        }
        Ok(image)
    }
    pub fn upload(gpu: &Device, image: &Image) -> Result<Self> {
        let format = match image.pixel {
            Pixel::Bgra8 => DXGI_FORMAT_B8G8R8A8_UNORM,
            Pixel::RgbaF16 => DXGI_FORMAT_R16G16B16A16_FLOAT,
            Pixel::Rgba10Pq => DXGI_FORMAT_R10G10B10A2_UNORM,
        };
        let bytes = if image.pixel == Pixel::RgbaF16 { 8 } else { 4 };
        if image.width == 0
            || image.height == 0
            || image.stride < image.width as usize * bytes
            || image.bytes.len() < image.stride * image.height as usize
        {
            bail!("invalid GPU upload image");
        }
        // SAFETY: `image.bytes` holds `stride * height` bytes with `stride` covering a row (checked
        // above), and CreateTexture2D copies them during the call.
        unsafe {
            let desc = D3D11_TEXTURE2D_DESC {
                Width: image.width,
                Height: image.height,
                MipLevels: 1,
                ArraySize: 1,
                Format: format,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
                // Shareable with the D3D12 compute converter.
                MiscFlags: (D3D11_RESOURCE_MISC_SHARED_NTHANDLE.0 | D3D11_RESOURCE_MISC_SHARED.0)
                    as u32,
                ..Default::default()
            };
            let data = D3D11_SUBRESOURCE_DATA {
                pSysMem: image.bytes.as_ptr().cast(),
                SysMemPitch: image.stride.try_into()?,
                SysMemSlicePitch: 0,
            };
            let mut texture = None;
            gpu.device
                .CreateTexture2D(&desc, Some(&data), Some(&mut texture))?;
            let mut uploaded = Self::from_texture(
                gpu.clone(),
                std::sync::Arc::new(texture.unwrap()),
                image.width,
                image.height,
                image.pixel,
                None,
            );
            uploaded.captured = image.captured;
            Ok(uploaded)
        }
    }
}

pub(crate) struct GpuPool {
    pub(crate) warnings: std::sync::Arc<dyn butterpollo_media_policy::WarningSink>,
    pub(super) textures: Vec<std::sync::Arc<ID3D11Texture2D>>,
    /// Width, height and format of every texture in `textures`.
    pub(super) shape: Option<(u32, u32, DXGI_FORMAT)>,
    /// Copies on a compute queue instead of the D3D11 graphics queue,
    /// where they wait behind a game's rendering.
    pub(crate) compute: Option<crate::compute::Handoff>,
    /// Keep allocated textures NT-handle shared (`SHARED_NTHANDLE | SHARED`)
    /// even when copying on D3D11, so downstream compute conversion stays on D3D12.
    pub(crate) shared: bool,
    /// Add `D3D11_BIND_RENDER_TARGET` alongside `D3D11_BIND_SHADER_RESOURCE`.
    pub(super) render_target: bool,
    /// Shared D3D11->D3D12 fence signalling D3D11 copies into `shared` textures.
    pub(super) d3d11_signal: Option<Option<crate::compute::D3d11Signal>>,
}
impl Default for GpuPool {
    fn default() -> Self {
        Self {
            warnings: butterpollo_media_policy::recorded_warnings(),
            textures: Vec::new(),
            shape: None,
            compute: None,
            shared: false,
            render_target: false,
            d3d11_signal: None,
        }
    }
}
pub(super) const GPU_SNAPSHOT_CAPACITY: usize = 8;

pub(super) fn snapshot_capacity<T>(textures: &[std::sync::Arc<T>]) -> bool {
    textures.len() < GPU_SNAPSHOT_CAPACITY
        || textures
            .iter()
            .any(|texture| std::sync::Arc::strong_count(texture) == 1)
}

/// Do not consume the last desktop update until it can be snapshotted. A
/// static desktop may never send another frame after consumers release space.
pub(super) fn acquire_snapshot_frame<T>(
    available: bool,
    acquire: impl FnOnce() -> Result<Option<T>>,
) -> Result<Option<T>> {
    if available { acquire() } else { Ok(None) }
}

pub(super) fn snapshot_publication_deadline(
    available: bool,
    deadline: Option<Instant>,
) -> Option<Instant> {
    // An expired publication deadline cannot be served while every snapshot is
    // owned. Let the capture worker wait for its normal poll instead of spinning.
    if available { deadline } else { None }
}

impl GpuPool {
    /// A pool whose textures carry `SHARED_NTHANDLE | SHARED` even without a
    /// D3D12 copy handoff, so downstream encoders keep D3D12 compute conversion.
    #[allow(dead_code)]
    pub(crate) fn shared() -> Self {
        Self {
            shared: true,
            ..Self::default()
        }
    }
    /// [`Self::shared`] with `D3D11_BIND_RENDER_TARGET` so a GPU pass can
    /// render directly into a pooled texture without an extra copy.
    #[allow(dead_code)]
    pub(crate) fn shared_rtv() -> Self {
        Self {
            shared: true,
            render_target: true,
            ..Self::default()
        }
    }
    /// Obtain a free pooled texture of `width` x `height` in `format`, or
    /// `None` when the bounded pool is full. Replaces the pool on shape change.
    pub(crate) fn acquire_texture(
        &mut self,
        gpu: &Device,
        width: u32,
        height: u32,
        format: DXGI_FORMAT,
    ) -> Result<Option<std::sync::Arc<ID3D11Texture2D>>> {
        self.acquire_texture_on(&gpu.device, width, height, format)
    }

    fn acquire_texture_on(
        &mut self,
        device: &ID3D11Device,
        width: u32,
        height: u32,
        format: DXGI_FORMAT,
    ) -> Result<Option<std::sync::Arc<ID3D11Texture2D>>> {
        let shape = Some((width, height, format));
        if self.shape != shape {
            if let Some(handoff) = &mut self.compute {
                for texture in &self.textures {
                    handoff.forget(texture);
                }
            }
            if let Some(Some(signal)) = &self.d3d11_signal {
                for texture in &self.textures {
                    signal.forget(texture);
                }
            }
            self.textures.clear();
            self.shape = shape;
        }
        if let Some(texture) = self
            .textures
            .iter()
            .find(|texture| std::sync::Arc::strong_count(texture) == 1)
            .cloned()
        {
            return Ok(Some(texture));
        }
        if self.textures.len() >= GPU_SNAPSHOT_CAPACITY {
            return Ok(None);
        }
        let desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: format,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: (D3D11_BIND_SHADER_RESOURCE.0
                | if self.render_target {
                    D3D11_BIND_RENDER_TARGET.0
                } else {
                    0
                }) as u32,
            CPUAccessFlags: 0,
            MiscFlags: if self.compute.is_some() || self.shared {
                (D3D11_RESOURCE_MISC_SHARED_NTHANDLE.0 | D3D11_RESOURCE_MISC_SHARED.0) as u32
            } else {
                0
            },
        };
        let mut texture = None;
        // SAFETY: `device` is live and `desc` describes a valid single-sample 2D texture.
        unsafe {
            device.CreateTexture2D(&desc, None, Some(&mut texture))?;
        }
        let texture = std::sync::Arc::new(texture.unwrap());
        self.textures.push(texture.clone());
        Ok(Some(texture))
    }
    pub(super) fn has_capacity(&self) -> bool {
        snapshot_capacity(&self.textures)
    }

    pub(super) fn desktop_snapshot(
        &mut self,
        gpu: &Device,
        source: &ID3D11Texture2D,
        cached: &mut Option<std::sync::Arc<GpuImage>>,
        desktop_updated: bool,
    ) -> Result<Option<GpuImage>> {
        // DXGI supplies a zero LastPresentTime for pointer-only updates. The
        // owned bitmap is still valid; only the detached cursor snapshot changes.
        if !desktop_updated && let Some(image) = cached.as_ref() {
            return Ok(Some((**image).clone()));
        }
        // A missed desktop copy must invalidate the cache. Otherwise a later
        // pointer-only frame could publish pixels from before the missed update.
        *cached = None;
        self.copy(gpu, source, None)
    }
    /// Copy `source` into a pool texture, cropped to `crop` when given: the
    /// picture keeps only that rectangle, and the image reports its size.
    pub(crate) fn copy(
        &mut self,
        gpu: &Device,
        source: &ID3D11Texture2D,
        crop: Option<Crop>,
    ) -> Result<Option<GpuImage>> {
        // SAFETY: `source` and the pool's textures are live on `gpu.device` with matching size and
        // format, and that device's context is multithread-protected. `GetDesc` is read directly
        // without retaining any strong COM reference to capture-owned surfaces past this call.
        unsafe {
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            source.GetDesc(&mut desc);
            let pixel = Pixel::try_from_dxgi(desc.Format).ok_or_else(|| {
                anyhow::anyhow!("unsupported GPU capture format {:?}", desc.Format)
            })?;
            // A crop outside the source would copy only garbage: keep the
            // whole frame instead, as WGC's own size checks do.
            let crop = crop.filter(|crop| {
                crop.width() > 0
                    && crop.height() > 0
                    && crop.right <= desc.Width
                    && crop.bottom <= desc.Height
            });
            let (width, height) = crop.map_or((desc.Width, desc.Height), |crop| {
                (crop.width(), crop.height())
            });
            let Some(texture) = self.acquire_texture(gpu, width, height, desc.Format)? else {
                // Capture retains only the newest native update while consumers
                // own every snapshot, instead of growing a frame queue.
                return Ok(None);
            };
            let ready = match self
                .compute
                .as_mut()
                .map(|handoff| handoff.copy(texture.as_ref(), source, crop))
            {
                Some(Ok(ready)) => Some(ready),
                Some(Err(error)) => {
                    if let Some(loss) = crate::device_loss::DeviceLost::d3d11(&gpu.device)
                        .or_else(|| {
                            self.compute
                                .as_ref()
                                .and_then(|handoff| handoff.device_removed())
                        })
                        .or_else(|| crate::device_loss::DeviceLost::from_error(&error))
                    {
                        return Err(error.context(loss));
                    }
                    // Replace the shared textures and copy this same frame on
                    // D3D11. Waiting for another update can strand a stream on
                    // a static desktop. Consumers see an unshared texture unless
                    // `self.shared` keeps D3D12 conversion active.
                    self.warnings.set("capture_compute", &format!("Compute capture copy failed ({error:#}); copying on the graphics queue. A busy game can delay capture; lower GPU load or update the graphics driver."));
                    self.compute = None;
                    self.textures.clear();
                    return self.copy(gpu, source, crop);
                }
                None => {
                    copy_d3d11_surface(&gpu.context, texture.as_ref(), source, crop);
                    if self.shared {
                        if let Some(ready) = self.signal_shared_d3d11(gpu) {
                            Some(ready)
                        } else {
                            if let Some(loss) = crate::device_loss::DeviceLost::d3d11(&gpu.device) {
                                return Err(anyhow::Error::new(loss));
                            }
                            // Never hand a `SHARED_NTHANDLE` texture with `ready: None` to
                            // downstream D3D12 compute readers: downgrade to a non-shared
                            // D3D11 pool and re-copy into a plain texture instead.
                            self.downgrade_shared_d3d11();
                            return self.copy(gpu, source, crop);
                        }
                    } else {
                        None
                    }
                }
            };
            Ok(Some(GpuImage::from_texture(
                gpu.clone(),
                texture,
                width,
                height,
                pixel,
                ready,
            )))
        }
    }

    #[cfg(test)]
    fn copy_d3d11_on(
        &mut self,
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        source: &ID3D11Texture2D,
        crop: Option<Crop>,
    ) -> Result<Option<(std::sync::Arc<ID3D11Texture2D>, u32, u32, Pixel)>> {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: `source` is a live D3D11 texture on `device`.
        unsafe { source.GetDesc(&mut desc) };
        let pixel = Pixel::try_from_dxgi(desc.Format)
            .ok_or_else(|| anyhow::anyhow!("unsupported GPU capture format {:?}", desc.Format))?;
        let crop = crop.filter(|crop| {
            crop.width() > 0
                && crop.height() > 0
                && crop.right <= desc.Width
                && crop.bottom <= desc.Height
        });
        let (width, height) = crop.map_or((desc.Width, desc.Height), |crop| {
            (crop.width(), crop.height())
        });
        let Some(texture) = self.acquire_texture_on(device, width, height, desc.Format)? else {
            return Ok(None);
        };
        // SAFETY: `source` and `texture` live on `device` with compatible dimensions and format.
        unsafe { copy_d3d11_surface(context, texture.as_ref(), source, crop) };
        Ok(Some((texture, width, height, pixel)))
    }

    fn downgrade_shared_d3d11(&mut self) {
        tracing::warn!(
            "shared D3D11 fence unavailable; downgrading capture pool to non-shared D3D11 textures"
        );
        self.warnings.set(
            "capture_compute",
            "Shared D3D11 fence unavailable; copying on the graphics queue. A busy game can delay capture; lower GPU load or update the graphics driver.",
        );
        self.shared = false;
        self.d3d11_signal = None;
        self.textures.clear();
    }

    /// Signal the shared D3D11->D3D12 fence after a D3D11 copy into a shared
    /// pool texture, retrying creation once if the cached `Compute` was retired.
    fn signal_shared_d3d11(&mut self, gpu: &Device) -> Option<crate::compute::Ready> {
        let mut current = self.d3d11_signal.take().flatten();
        let ready = resolve_shared_signal(
            &mut current,
            |signal| signal.signal(),
            || {
                if crate::device_loss::DeviceLost::d3d11(&gpu.device).is_some() {
                    return None;
                }
                crate::compute::Compute::for_device(&gpu.device)
                    .and_then(|compute| crate::compute::D3d11Signal::new(compute, gpu))
                    .ok()
            },
        );
        self.d3d11_signal = Some(current);
        ready
    }
}

/// Signal an existing shared fence or recreate it once if signalling failed
/// (for example after a retired compute queue). Returns `None` if no working
/// shared fence can be signalled, so the caller must downgrade off shared
/// textures rather than emit an unsynchronized `SHARED_NTHANDLE` image.
fn resolve_shared_signal<S, R, E: std::fmt::Display>(
    slot: &mut Option<S>,
    mut signal: impl FnMut(&mut S) -> std::result::Result<R, E>,
    mut create: impl FnMut() -> Option<S>,
) -> Option<R> {
    if let Some(existing) = slot.as_mut() {
        match signal(existing) {
            Ok(ready) => return Some(ready),
            Err(error) => {
                tracing::debug!(%error, "shared D3D11 fence signal failed; recreating signal");
                *slot = None;
            }
        }
    }
    let mut fresh = create()?;
    match signal(&mut fresh) {
        Ok(ready) => {
            *slot = Some(fresh);
            Some(ready)
        }
        Err(error) => {
            tracing::debug!(%error, "recreated shared D3D11 fence signal failed");
            None
        }
    }
}
pub fn read_texture(
    gpu: &Device,
    texture: &ID3D11Texture2D,
    staging: &mut Option<ID3D11Texture2D>,
) -> Result<Image> {
    // SAFETY: `stage` matches `texture`'s size and format, and each row read stays inside the
    // `RowPitch * Height` mapped bytes until Unmap below.
    unsafe {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        texture.GetDesc(&mut desc);
        let pixel = Pixel::try_from_dxgi(desc.Format).ok_or_else(|| {
            anyhow::anyhow!("unsupported capture texture format {:?}", desc.Format)
        })?;
        let mut needs = true;
        if let Some(old) = staging {
            let mut d = D3D11_TEXTURE2D_DESC::default();
            old.GetDesc(&mut d);
            needs = d.Width != desc.Width || d.Height != desc.Height || d.Format != desc.Format;
        }
        if needs {
            desc.Usage = D3D11_USAGE_STAGING;
            desc.BindFlags = 0;
            desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
            desc.MiscFlags = 0;
            let mut created = None;
            gpu.device
                .CreateTexture2D(&desc, None, Some(&mut created))?;
            *staging = created;
        }
        let stage = staging.as_ref().unwrap();
        gpu.context.CopyResource(stage, texture);
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        gpu.context
            .Map(stage, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
        let stride = (desc.Width as usize) * if pixel == Pixel::RgbaF16 { 8 } else { 4 };
        let mut bytes = vec![0; stride * desc.Height as usize];
        for y in 0..desc.Height as usize {
            std::ptr::copy_nonoverlapping(
                (mapped.pData as *const u8).add(y * mapped.RowPitch as usize),
                bytes.as_mut_ptr().add(y * stride),
                stride,
            );
        }
        gpu.context.Unmap(stage, 0);
        Ok(Image {
            width: desc.Width,
            height: desc.Height,
            stride,
            bytes,
            captured: Instant::now(),
            pixel,
        })
    }
}

#[cfg(test)]
impl GpuImage {
    /// [`GpuImage::upload`] once D3D11 has finished copying the picture. The
    /// upload carries no fence for the D3D12 compute queue to wait on, and a
    /// large picture is still copying when a pass reads it (captures carry
    /// `ready`, so streams are not affected); a test that opens the texture on
    /// the compute device uses this.
    pub(crate) fn upload_settled(gpu: &Device, image: &Image) -> Result<Self> {
        use windows::core::BOOL;
        let source = Self::upload(gpu, image)?;
        let desc = D3D11_QUERY_DESC {
            Query: D3D11_QUERY_EVENT,
            MiscFlags: 0,
        };
        let mut query = None;
        // SAFETY: `desc` and `query` are live locals; the event query writes one
        // BOOL into the live local `done`, and only once it has completed.
        unsafe {
            gpu.device.CreateQuery(&desc, Some(&mut query))?;
            let query = query.context("an event query")?;
            gpu.context.End(&query);
            gpu.context.Flush();
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                let mut done = BOOL(0);
                // Still copying is a success with `done` unset; a failure (the device is gone)
                // ends the wait with its own error.
                gpu.context
                    .GetData(
                        &query,
                        Some((&raw mut done).cast()),
                        std::mem::size_of::<BOOL>() as u32,
                        0,
                    )
                    .context("the upload's event query")?;
                if done.as_bool() {
                    break;
                }
                ensure!(Instant::now() < deadline, "the upload did not finish");
                std::thread::yield_now();
            }
        }
        Ok(source)
    }
}

/// Copies `source` (or its `crop` subrectangle) into `destination` on `context`.
///
/// # Safety
/// `destination` and `source` must belong to `context`'s D3D11 device with matching format
/// and bounds covering `crop` (or identical dimensions when `crop` is `None`).
unsafe fn copy_d3d11_surface(
    context: &ID3D11DeviceContext,
    destination: &ID3D11Texture2D,
    source: &ID3D11Texture2D,
    crop: Option<Crop>,
) {
    // SAFETY: Guaranteed by caller.
    unsafe {
        if let Some(crop) = crop {
            let box_ = crop.d3d11();
            context.CopySubresourceRegion(destination, 0, 0, 0, 0, source, 0, Some(&box_));
        } else {
            context.CopyResource(destination, source);
        }
    }
}

#[cfg(test)]
mod tests;
