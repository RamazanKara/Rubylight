//! Frame Warp's passes over captures (docs/frame-warp.md) on the capture
//! GPU's compute queue, for every capture format and either encoder: a luma
//! thumbnail of each new capture for the host's motion measurement and,
//! while the camera turns, which pixels stay exactly as they were, scored per
//! HUD mask cell (`frame_warp_*_cs` in `shaders/color.hlsl`). A capture's
//! list goes in after its frame went to the encoder, and nothing waits for
//! one on the CPU: [`FrameWarpGpu::poll`] hands back the passes that have
//! finished, usually a frame later.
use super::*;
use crate::capture::{GpuImage, Pixel};

/// The largest thumbnail: `butterpollo_core::frame_warp::THUMBNAIL_WIDTH`
/// by the tallest `thumbnail_height`.
pub const THUMBNAIL_EDGE: u32 = 256;
const THUMBNAIL_BYTES: u64 = THUMBNAIL_EDGE as u64 * THUMBNAIL_EDGE as u64;
/// HUD cells across or down at most
/// (`rubylight_protocol::frame_warp::MAX_MASK_EDGE`).
pub const MAX_GRID_EDGE: u32 = 2048;
/// The score from which a cell is HUD: it stayed through about 24 frames of
/// turning, and every change halves it.
pub const HUD_SCORE: u32 = 24;
/// `cbuffer FrameWarp` (b4).
const CONSTANTS: u32 = 16;

/// What to run over one capture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pass {
    /// The thumbnail's width, a multiple of 4, and height, each at most
    /// [`THUMBNAIL_EDGE`].
    pub thumbnail: (u32, u32),
    /// The HUD work while the camera turns; `None` while it stands still.
    pub hud: Option<Hud>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hud {
    /// The mask cells across and down ([`hud_grid`]).
    pub grid: (u32, u32),
    /// Copy the mask bits back for [`FrameWarpGpu::poll`]; ignored while one
    /// copy is still being read.
    pub read: bool,
}
impl Pass {
    fn check(&self) -> Result<()> {
        let (width, height) = self.thumbnail;
        anyhow::ensure!(
            width % 4 == 0
                && (4..=THUMBNAIL_EDGE).contains(&width)
                && (1..=THUMBNAIL_EDGE).contains(&height),
            "Frame Warp thumbnail of {width}x{height}"
        );
        if let Some(hud) = &self.hud {
            anyhow::ensure!(
                (1..=MAX_GRID_EDGE).contains(&hud.grid.0)
                    && (1..=MAX_GRID_EDGE).contains(&hud.grid.1),
                "Frame Warp HUD grid of {}x{}",
                hud.grid.0,
                hud.grid.1
            );
        }
        Ok(())
    }
}

/// The HUD cells of a mask `width` cells wide over a `source` picture: as many
/// rows as keep the cells' shape the picture's, and no more cells than pixels.
/// `None` for no mask.
pub fn hud_grid(width: u16, source: (u32, u32)) -> Option<(u32, u32)> {
    if width == 0 || source.0 == 0 || source.1 == 0 {
        return None;
    }
    let across = u32::from(width).min(source.0).min(MAX_GRID_EDGE);
    let down =
        (u64::from(across) * u64::from(source.1) + u64::from(source.0) / 2) / u64::from(source.0);
    Some((across, (down as u32).clamp(1, source.1.min(MAX_GRID_EDGE))))
}

/// Where the HUD pass keeps its words in its buffer: a code per pixel, then
/// per cell the changed pixels and the score, then the mask bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Layout {
    codes: u32,
    counts: u32,
    scores: u32,
    bits: u32,
    /// Mask words per row.
    stride: u32,
    words: u64,
}
fn layout(source: (u32, u32), grid: (u32, u32)) -> Result<Layout> {
    let pixels = u64::from(source.0) * u64::from(source.1);
    let cells = u64::from(grid.0) * u64::from(grid.1);
    let stride = grid.0.div_ceil(32);
    let bits = pixels + 2 * cells;
    let words = bits + u64::from(stride) * u64::from(grid.1);
    anyhow::ensure!(
        words <= u64::from(u32::MAX),
        "Frame Warp HUD buffer for {}x{}",
        source.0,
        source.1
    );
    Ok(Layout {
        codes: 0,
        counts: pixels as u32,
        scores: (pixels + cells) as u32,
        bits: bits as u32,
        stride,
        words,
    })
}
/// `cbuffer FrameWarp`'s values.
fn constants(
    source: (u32, u32),
    pixel: Pixel,
    pass: &Pass,
    mode: u32,
    layout: Option<&Layout>,
) -> [u32; CONSTANTS as usize] {
    let grid = pass.hud.map_or((0, 0), |hud| hud.grid);
    let pixel = match pixel {
        Pixel::Bgra8 => 0,
        Pixel::RgbaF16 => 1,
        Pixel::Rgba10Pq => 2,
    };
    let layout = layout.copied().unwrap_or(Layout {
        codes: 0,
        counts: 0,
        scores: 0,
        bits: 0,
        stride: 0,
        words: 0,
    });
    [
        source.0,
        source.1,
        pass.thumbnail.0,
        pass.thumbnail.1,
        grid.0,
        grid.1,
        pixel,
        mode,
        0,
        layout.codes,
        layout.counts,
        layout.scores,
        layout.bits,
        layout.stride,
        HUD_SCORE,
        0,
    ]
}
/// `fwMode` of the score pass: clear the counts only, score, or start over.
const CLEAR: u32 = 0;
const SCORE: u32 = 1;
const RESET: u32 = 2;

/// A finished thumbnail: `width` x `height` luma bytes, rows top to bottom.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThumbnailReadout {
    /// What [`FrameWarpGpu::submit`] was given for the capture.
    pub at: u64,
    pub width: usize,
    pub height: usize,
    pub luma: Vec<u8>,
}
/// A finished HUD mask: `height` rows of `width.div_ceil(32)` words, bit `i`
/// of word `w` being column `32 * w + i`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MaskReadout {
    pub width: u32,
    pub height: u32,
    pub words: Vec<u32>,
}

struct Slot {
    allocator: ID3D12CommandAllocator,
    list: ID3D12GraphicsCommandList,
    /// Persistently mapped READBACK copy of the thumbnail.
    readback: ID3D12Resource,
    mapped: *const u8,
    /// The fence value of the slot's last list, 0 before its first.
    fence: u64,
    /// The thumbnail in `readback` once the list has run, not yet polled.
    thumbnail: Option<(u64, (u32, u32))>,
    /// The capture and its D3D12 view of it, kept until the list has run.
    capture: Option<(GpuImage, ID3D12Resource)>,
}
struct HudBuffer {
    buffer: ID3D12Resource,
    source: (u32, u32),
    grid: (u32, u32),
    layout: Layout,
}
struct MaskRead {
    fence: u64,
    readback: ID3D12Resource,
    grid: (u32, u32),
    stride: u32,
}

/// The passes' GPU objects on one compute queue.
pub struct FrameWarpGpu {
    compute: Arc<Compute>,
    /// The capture device these passes were made for ([`FrameWarpGpu::for_image`]).
    device: Option<windows::Win32::Graphics::Direct3D11::ID3D11Device>,
    root: ID3D12RootSignature,
    thumb: ID3D12PipelineState,
    hud: ID3D12PipelineState,
    score: ID3D12PipelineState,
    bits: ID3D12PipelineState,
    /// One capture view per slot.
    heap: ID3D12DescriptorHeap,
    increment: u32,
    /// The thumbnail's words, in COMMON between lists.
    thumbnail: ID3D12Resource,
    slots: Vec<Slot>,
    /// The HUD's words for the current capture size and grid, in COMMON
    /// between lists.
    hud_buffer: Option<HudBuffer>,
    /// The last list ran a HUD pass, so the codes are the previous capture's.
    primed: bool,
    mask_read: Option<MaskRead>,
    /// Buffers replaced while a list may still use them, with that fence value.
    retired: Vec<(u64, ID3D12Resource)>,
    /// The fence value of the last list submitted.
    last: u64,
}
// SAFETY: The D3D12 objects are free-threaded; the mapped pointers are only
// read through `&mut self`, by whichever thread owns the passes.
unsafe impl Send for FrameWarpGpu {}

/// Constants (b4), the capture's view (t0) and the words (u8).
fn root_signature(device: &ID3D12Device) -> Result<ID3D12RootSignature> {
    // SAFETY: `srv` and `parameters` outlive the serialize call that reads them, and the slice
    // covers exactly the blob's GetBufferSize bytes while `blob` is alive.
    unsafe {
        let srv = D3D12_DESCRIPTOR_RANGE {
            RangeType: D3D12_DESCRIPTOR_RANGE_TYPE_SRV,
            NumDescriptors: 1,
            BaseShaderRegister: 0,
            RegisterSpace: 0,
            OffsetInDescriptorsFromTableStart: 0,
        };
        let parameters = [
            D3D12_ROOT_PARAMETER {
                ParameterType: D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS,
                Anonymous: D3D12_ROOT_PARAMETER_0 {
                    Constants: D3D12_ROOT_CONSTANTS {
                        ShaderRegister: 4,
                        RegisterSpace: 0,
                        Num32BitValues: CONSTANTS,
                    },
                },
                ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
            },
            D3D12_ROOT_PARAMETER {
                ParameterType: D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE,
                Anonymous: D3D12_ROOT_PARAMETER_0 {
                    DescriptorTable: D3D12_ROOT_DESCRIPTOR_TABLE {
                        NumDescriptorRanges: 1,
                        pDescriptorRanges: &srv,
                    },
                },
                ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
            },
            D3D12_ROOT_PARAMETER {
                ParameterType: D3D12_ROOT_PARAMETER_TYPE_UAV,
                Anonymous: D3D12_ROOT_PARAMETER_0 {
                    Descriptor: D3D12_ROOT_DESCRIPTOR {
                        ShaderRegister: 8,
                        RegisterSpace: 0,
                    },
                },
                ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
            },
        ];
        let mut blob = None;
        D3D12SerializeRootSignature(
            &D3D12_ROOT_SIGNATURE_DESC {
                NumParameters: parameters.len() as u32,
                pParameters: parameters.as_ptr(),
                ..Default::default()
            },
            D3D_ROOT_SIGNATURE_VERSION_1,
            &mut blob,
            None,
        )?;
        let blob = blob.context("no Frame Warp root signature")?;
        Ok(device.CreateRootSignature(
            0,
            std::slice::from_raw_parts(blob.GetBufferPointer().cast::<u8>(), blob.GetBufferSize()),
        )?)
    }
}

impl FrameWarpGpu {
    /// The passes on `compute`, the queue of the GPU the captures are on
    /// ([`Compute::for_device`]).
    pub fn new(compute: Arc<Compute>) -> Result<Self> {
        compute.ensure_active()?;
        let device = &compute.device;
        let root = root_signature(device)?;
        let thumb = pipeline(device, &root, b"frame_warp_thumb_cs\0")?;
        let hud = pipeline(device, &root, b"frame_warp_hud_cs\0")?;
        let score = pipeline(device, &root, b"frame_warp_score_cs\0")?;
        let bits = pipeline(device, &root, b"frame_warp_bits_cs\0")?;
        let thumbnail = focus_buffer(
            device,
            D3D12_HEAP_TYPE_DEFAULT,
            THUMBNAIL_BYTES,
            D3D12_RESOURCE_STATE_COMMON,
        )?;
        // SAFETY: Calls on the live device; each list is closed until a pass records it, and a
        // READBACK buffer stays mapped for its life (Drop unmaps it).
        unsafe {
            let heap: ID3D12DescriptorHeap =
                device.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                    Type: D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
                    NumDescriptors: SLOTS as u32,
                    Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
                    NodeMask: 0,
                })?;
            let increment =
                device.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV);
            let mut slots = Vec::with_capacity(SLOTS);
            for _ in 0..SLOTS {
                let allocator: ID3D12CommandAllocator =
                    device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_COMPUTE)?;
                let list: ID3D12GraphicsCommandList = device.CreateCommandList(
                    0,
                    D3D12_COMMAND_LIST_TYPE_COMPUTE,
                    &allocator,
                    None,
                )?;
                list.Close()?;
                let readback = focus_buffer(
                    device,
                    D3D12_HEAP_TYPE_READBACK,
                    THUMBNAIL_BYTES,
                    D3D12_RESOURCE_STATE_COPY_DEST,
                )?;
                let mut mapped = std::ptr::null_mut();
                readback.Map(0, None, Some(&mut mapped))?;
                slots.push(Slot {
                    allocator,
                    list,
                    readback,
                    mapped: mapped.cast_const().cast::<u8>(),
                    fence: 0,
                    thumbnail: None,
                    capture: None,
                });
            }
            Ok(Self {
                compute,
                device: None,
                root,
                thumb,
                hud,
                score,
                bits,
                heap,
                increment,
                thumbnail,
                slots,
                hud_buffer: None,
                primed: false,
                mask_read: None,
                retired: Vec::new(),
                last: 0,
            })
        }
    }

    /// The passes on the compute queue of the GPU `image` was captured on.
    pub fn for_image(image: &GpuImage) -> Result<Self> {
        let mut passes = Self::new(Compute::for_device(&image.gpu.device)?)?;
        passes.device = Some(image.gpu.device.clone());
        Ok(passes)
    }

    /// Whether `image` is on the capture device these passes were made for.
    pub fn accepts(&self, image: &GpuImage) -> bool {
        self.device
            .as_ref()
            .is_some_and(|device| *device == image.gpu.device)
    }

    /// The queue the passes run on.
    pub fn compute(&self) -> &Arc<Compute> {
        &self.compute
    }

    /// Whether the passes can read `image` without a CPU wait: its texture
    /// can be opened here, its copy finishes on this device, and its pool
    /// takes this queue's fence as the read.
    pub fn can_read(&self, image: &GpuImage) -> bool {
        let device = self.compute.device.as_raw() as usize;
        shareable(&image.texture)
            && image.ready.as_ref().is_none_or(|ready| {
                ready.device == device
                    && (ready.readers.is_none()
                        || ready.reader_fence.as_ref() == Some(&self.compute.fence))
            })
    }

    /// Run `pass` over `image` once its capture copy is done; its thumbnail
    /// comes back from [`FrameWarpGpu::poll`] with `at`. `Ok(None)` when the
    /// capture is skipped: it cannot be read without a CPU wait
    /// ([`FrameWarpGpu::can_read`]) or every slot is still busy. Otherwise
    /// whether a mask copy started.
    pub fn submit(&mut self, image: &GpuImage, pass: &Pass, at: u64) -> Result<Option<bool>> {
        pass.check()?;
        if !self.can_read(image) {
            return Ok(None);
        }
        self.release_retired();
        let compute = self.compute.clone();
        let Some(index) = (0..SLOTS).find(|&slot| {
            let slot = &self.slots[slot];
            slot.thumbnail.is_none() && (slot.fence == 0 || compute.completed(slot.fence))
        }) else {
            return Ok(None);
        };
        let source = (image.width, image.height);
        anyhow::ensure!(
            source.0 > 0 && source.1 > 0,
            "Frame Warp over an empty capture"
        );
        let mode = match pass.hud {
            Some(hud) => Some(self.prepare_hud(source, hud.grid)?),
            None => None,
        };
        let read = pass.hud.is_some_and(|hud| hud.read) && self.mask_read.is_none();
        let mask_readback = match (read, self.hud_buffer.as_ref()) {
            (true, Some(hud)) => Some(focus_buffer(
                &compute.device,
                D3D12_HEAP_TYPE_READBACK,
                u64::from(hud.layout.stride) * u64::from(hud.grid.1) * 4,
                D3D12_RESOURCE_STATE_COPY_DEST,
            )?),
            _ => None,
        };
        let opened = compute.open(&image.texture)?;
        let hud = mode.zip(self.hud_buffer.as_ref());
        let values = constants(
            source,
            image.pixel,
            pass,
            mode.unwrap_or(CLEAR),
            hud.map(|(_, hud)| &hud.layout),
        );
        let (thumb_width, thumb_height) = pass.thumbnail;
        let thumb_bytes = u64::from(thumb_width) * u64::from(thumb_height);
        let slot = &self.slots[index];
        // SAFETY: The slot's last list has run (checked above), so its view, allocator and list
        // may be reused; every view, barrier and copy names a live resource on this device, the
        // copies stay inside both buffers, and the words go back to COMMON at the end.
        unsafe {
            let cpu = self.heap.GetCPUDescriptorHandleForHeapStart();
            let gpu = self.heap.GetGPUDescriptorHandleForHeapStart();
            compute.device.CreateShaderResourceView(
                &opened,
                Some(&texture_srv(format(image.pixel))),
                D3D12_CPU_DESCRIPTOR_HANDLE {
                    ptr: cpu.ptr + index * self.increment as usize,
                },
            );
            slot.allocator.Reset()?;
            slot.list.Reset(&slot.allocator, None)?;
            let list = &slot.list;
            list.SetComputeRootSignature(&self.root);
            list.SetDescriptorHeaps(&[Some(self.heap.clone())]);
            list.SetComputeRoot32BitConstants(0, CONSTANTS, values.as_ptr().cast(), 0);
            list.SetComputeRootDescriptorTable(
                1,
                D3D12_GPU_DESCRIPTOR_HANDLE {
                    ptr: gpu.ptr + (index * self.increment as usize) as u64,
                },
            );
            let barrier = |resource: &ID3D12Resource, before, after| {
                let mut barriers = [transition(resource, before, after)];
                list.ResourceBarrier(&barriers);
                drop_barriers(&mut barriers);
            };
            barrier(
                &self.thumbnail,
                D3D12_RESOURCE_STATE_COMMON,
                D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
            );
            list.SetComputeRootUnorderedAccessView(2, self.thumbnail.GetGPUVirtualAddress());
            list.SetPipelineState(&self.thumb);
            list.Dispatch((thumb_width / 4).div_ceil(8), thumb_height.div_ceil(8), 1);
            barrier(
                &self.thumbnail,
                D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                D3D12_RESOURCE_STATE_COPY_SOURCE,
            );
            list.CopyBufferRegion(&slot.readback, 0, &self.thumbnail, 0, thumb_bytes);
            barrier(
                &self.thumbnail,
                D3D12_RESOURCE_STATE_COPY_SOURCE,
                D3D12_RESOURCE_STATE_COMMON,
            );
            if let Some((_, hud)) = hud {
                barrier(
                    &hud.buffer,
                    D3D12_RESOURCE_STATE_COMMON,
                    D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                );
                list.SetComputeRootUnorderedAccessView(2, hud.buffer.GetGPUVirtualAddress());
                list.SetPipelineState(&self.hud);
                list.Dispatch(source.0.div_ceil(16), source.1.div_ceil(16), 1);
                list.ResourceBarrier(&[uav_barrier()]);
                list.SetPipelineState(&self.score);
                list.Dispatch(hud.grid.0.div_ceil(8), hud.grid.1.div_ceil(8), 1);
                if let Some(readback) = &mask_readback {
                    list.ResourceBarrier(&[uav_barrier()]);
                    list.SetPipelineState(&self.bits);
                    list.Dispatch(hud.layout.stride.div_ceil(8), hud.grid.1.div_ceil(8), 1);
                    barrier(
                        &hud.buffer,
                        D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                        D3D12_RESOURCE_STATE_COPY_SOURCE,
                    );
                    list.CopyBufferRegion(
                        readback,
                        0,
                        &hud.buffer,
                        u64::from(hud.layout.bits) * 4,
                        u64::from(hud.layout.stride) * u64::from(hud.grid.1) * 4,
                    );
                    barrier(
                        &hud.buffer,
                        D3D12_RESOURCE_STATE_COPY_SOURCE,
                        D3D12_RESOURCE_STATE_COMMON,
                    );
                } else {
                    barrier(
                        &hud.buffer,
                        D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                        D3D12_RESOURCE_STATE_COMMON,
                    );
                }
            }
            list.Close()?;
        }
        let grid = hud.map(|(_, hud)| (hud.grid, hud.layout.stride));
        let ready = image.ready.as_ref();
        if let Some(ready) = ready {
            // SAFETY: `can_read` checked that the fence belongs to this queue's device, and
            // `image` holds it for the call.
            unsafe {
                compute
                    .queue
                    .Wait(&ready.fence, ready.value)
                    .inspect_err(|_| compute.retire())?;
            }
        }
        let fence = compute.execute(&self.slots[index].list)?;
        self.last = fence;
        let slot = &mut self.slots[index];
        slot.fence = fence;
        slot.thumbnail = Some((at, pass.thumbnail));
        slot.capture = Some((image.clone(), opened));
        self.primed = pass.hud.is_some();
        let started = match (mask_readback, grid) {
            (Some(readback), Some((grid, stride))) => {
                self.mask_read = Some(MaskRead {
                    fence,
                    readback,
                    grid,
                    stride,
                });
                true
            }
            _ => false,
        };
        // The capture's pool must not overwrite the texture before the passes have read it.
        if let Some(ready) = ready {
            ready.read_by(&compute, fence)?;
        }
        Ok(Some(started))
    }

    /// The HUD buffer for `source` and `grid`, and the score pass's mode: a
    /// new size starts over, and the first pass after the camera stood still
    /// only takes the capture's codes.
    fn prepare_hud(&mut self, source: (u32, u32), grid: (u32, u32)) -> Result<u32> {
        if self
            .hud_buffer
            .as_ref()
            .is_some_and(|hud| hud.source == source && hud.grid == grid)
        {
            return Ok(if self.primed { SCORE } else { CLEAR });
        }
        let layout = layout(source, grid)?;
        let buffer = focus_buffer(
            &self.compute.device,
            D3D12_HEAP_TYPE_DEFAULT,
            layout.words * 4,
            D3D12_RESOURCE_STATE_COMMON,
        )?;
        if let Some(old) = self.hud_buffer.replace(HudBuffer {
            buffer,
            source,
            grid,
            layout,
        }) {
            self.retired.push((self.last, old.buffer));
        }
        Ok(RESET)
    }

    fn release_retired(&mut self) {
        let compute = &self.compute;
        self.retired.retain(|(fence, _)| !compute.completed(*fence));
    }

    /// The thumbnails whose passes finished, oldest first, and the mask
    /// copy once it finished. Only asks the fence: never waits.
    pub fn poll(&mut self) -> Result<(Vec<ThumbnailReadout>, Option<MaskReadout>)> {
        let mut finished = Vec::new();
        for slot in &mut self.slots {
            if slot.fence == 0 || !self.compute.completed(slot.fence) {
                continue;
            }
            slot.capture = None;
            if let Some((at, (width, height))) = slot.thumbnail.take() {
                let (width, height) = (width as usize, height as usize);
                // SAFETY: The slot's copy has completed and the next one needs `&mut self`
                // (`submit`), so the mapped bytes stay unchanged while they are read.
                let luma =
                    unsafe { std::slice::from_raw_parts(slot.mapped, width * height) }.to_vec();
                finished.push((
                    slot.fence,
                    ThumbnailReadout {
                        at,
                        width,
                        height,
                        luma,
                    },
                ));
            }
        }
        finished.sort_by_key(|(fence, _)| *fence);
        let thumbnails = finished
            .into_iter()
            .map(|(_, thumbnail)| thumbnail)
            .collect();
        let mut mask = None;
        if let Some(read) = self
            .mask_read
            .take_if(|read| self.compute.completed(read.fence))
        {
            let count = read.stride as usize * read.grid.1 as usize;
            // SAFETY: The copy into `readback` has completed, nothing writes it again, and the
            // slice stays inside the mapped buffer until Unmap.
            let words = unsafe {
                let mut mapped = std::ptr::null_mut();
                read.readback.Map(0, None, Some(&mut mapped))?;
                let words = std::slice::from_raw_parts(mapped.cast::<u32>(), count).to_vec();
                read.readback.Unmap(0, None);
                words
            };
            mask = Some(MaskReadout {
                width: read.grid.0,
                height: read.grid.1,
                words,
            });
        }
        self.release_retired();
        Ok((thumbnails, mask))
    }
}

impl Drop for FrameWarpGpu {
    fn drop(&mut self) {
        let mut work = RetainedWork::new(&self.compute, &self.compute.queue);
        work.keep(&self.root);
        work.keep(&self.thumb);
        work.keep(&self.hud);
        work.keep(&self.score);
        work.keep(&self.bits);
        work.keep(&self.heap);
        work.keep(&self.thumbnail);
        if let Some(hud) = &self.hud_buffer {
            work.keep(&hud.buffer);
        }
        if let Some(read) = &self.mask_read {
            work.keep(&read.readback);
        }
        for (_, buffer) in &self.retired {
            work.keep(buffer);
        }
        for slot in &mut self.slots {
            work.keep(&slot.allocator);
            work.keep(&slot.list);
            work.keep(&slot.readback);
            if let Some((capture, opened)) = slot.capture.take() {
                work.keep(&opened);
                work.captures.push(capture);
            }
        }
        work.finish(&self.compute);
        for slot in &self.slots {
            // SAFETY: Each buffer was mapped once in `new`; nothing reads it after this. Unmap
            // ends CPU access only; retained GPU work still owns the resource.
            unsafe { slot.readback.Unmap(0, None) };
        }
    }
}

#[cfg(test)]
#[path = "compute_frame_warp_tests.rs"]
mod tests;
