//! The couch menu's pass on the compute queue (docs/couch-menu-design.md
//! 6.2): while an overlay is set, each conversion runs its entry's menu
//! variant over the panel's blocks after the closed dispatch, the colours
//! read from the panel image. Closed, nothing here is recorded.
use super::*;
use crate::overlay::Overlay;

/// Textures the panel image rotates through.
const RING: usize = 3;
/// The panel image's descriptor among each slot's views.
pub(super) const VIEW: usize = 5;

struct Image {
    texture: ID3D12Resource,
    size: [u32; 2],
    /// The last conversion reading it, `u64::MAX` while it is submitted.
    read: u64,
}
struct Upload {
    buffer: ID3D12Resource,
    bytes: u64,
    /// The conversion that copies from it.
    fence: u64,
}
#[derive(Default)]
pub(super) struct Menu {
    overlay: Option<Overlay>,
    /// Made on the first open, for the entry it was made for.
    pipeline: Option<(&'static [u8], ID3D12RootSignature, ID3D12PipelineState)>,
    ring: Vec<Image>,
    /// The ring texture holding the panel image, and its revision.
    current: Option<(usize, u64)>,
    upload: Option<Upload>,
    /// Images uploaded and passes recorded, for the tests.
    uploads: u64,
    passes: u64,
}
impl Menu {
    pub(super) fn retain(&self, work: &mut RetainedWork) {
        if let Some((_, root, pipeline)) = &self.pipeline {
            work.keep(root);
            work.keep(pipeline);
        }
        for image in &self.ring {
            work.keep(&image.texture);
        }
        if let Some(upload) = &self.upload {
            work.keep(&upload.buffer);
        }
    }
}
/// One conversion's menu pass, recorded around its closed dispatch.
pub(super) struct Pass {
    root: ID3D12RootSignature,
    pipeline: ID3D12PipelineState,
    index: usize,
    /// The panel revision the texture holds once this pass has run.
    revision: u64,
    pub(super) texture: ID3D12Resource,
    /// A new panel image to copy first: the upload buffer, the image size,
    /// and whether the texture is new (still `COPY_DEST`).
    copy: Option<(ID3D12Resource, [u32; 2], bool)>,
    groups: [u32; 2],
}
#[derive(Debug, PartialEq, Eq)]
enum Pick {
    Reuse(usize),
    Push,
    Replace(usize),
}
/// Where a new panel image of `size` goes, given each ring texture's size
/// and last reader: a texture of that size no conversion still reads, else
/// a new texture. A replaced one stays alive in the slots that read it, so
/// no texture is written before its readers' fences.
fn pick(
    ring: &[([u32; 2], u64)],
    current: Option<usize>,
    size: [u32; 2],
    completed: impl Fn(u64) -> bool,
) -> Pick {
    if let Some(index) = ring
        .iter()
        .position(|&(made, read)| made == size && completed(read))
    {
        return Pick::Reuse(index);
    }
    if ring.len() < RING {
        return Pick::Push;
    }
    Pick::Replace(current.map_or(0, |current| (current + 1) % ring.len()))
}
/// The conversion's root (b0, t0-t1, the UAVs) followed by `cbuffer Menu`
/// (b2) and the panel image (t2): parameters 3 and 4.
fn menu_root(device: &ID3D12Device, first: u32, uavs: u32) -> Result<ID3D12RootSignature> {
    let image = D3D12_DESCRIPTOR_RANGE {
        RangeType: D3D12_DESCRIPTOR_RANGE_TYPE_SRV,
        NumDescriptors: 1,
        BaseShaderRegister: 2,
        RegisterSpace: 0,
        OffsetInDescriptorsFromTableStart: 0,
    };
    root_signature_with(
        device,
        first,
        uavs,
        &[
            D3D12_ROOT_PARAMETER {
                ParameterType: D3D12_ROOT_PARAMETER_TYPE_CBV,
                Anonymous: D3D12_ROOT_PARAMETER_0 {
                    Descriptor: D3D12_ROOT_DESCRIPTOR {
                        ShaderRegister: 2,
                        RegisterSpace: 0,
                    },
                },
                ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
            },
            D3D12_ROOT_PARAMETER {
                ParameterType: D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE,
                Anonymous: D3D12_ROOT_PARAMETER_0 {
                    DescriptorTable: D3D12_ROOT_DESCRIPTOR_TABLE {
                        NumDescriptorRanges: 1,
                        pDescriptorRanges: &image,
                    },
                },
                ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
            },
        ],
    )
}
impl Converter {
    /// Draw `overlay` over the next conversions, or stop with `None`.
    pub fn set_overlay(&mut self, overlay: Option<&Overlay>) {
        if self.menu.overlay.as_ref() != overlay {
            self.menu.overlay = overlay.cloned();
        }
    }
    /// Panel images uploaded and menu passes recorded so far.
    #[cfg(test)]
    pub(crate) fn menu_counts(&self) -> (u64, u64) {
        (self.menu.uploads, self.menu.passes)
    }
    /// Prepare `slot`'s menu pass with `entry`, whose root has `uavs` views
    /// from `u{first}` and whose threads cover `block`-sized target blocks:
    /// its constants, its panel image (uploaded if the revision is new) and
    /// view. `None` while closed. The slot's last conversion has finished.
    pub(super) fn menu_pass(
        &mut self,
        slot: usize,
        entry: &'static [u8],
        first: u32,
        uavs: u32,
        block: [u32; 2],
    ) -> Result<Option<Pass>> {
        let Some(overlay) = self.menu.overlay.clone() else {
            return Ok(None);
        };
        overlay.validate([self.values[2], self.values[3]])?;
        let device = self.compute.device.clone();
        if self
            .menu
            .pipeline
            .as_ref()
            .is_none_or(|(made, ..)| *made != entry)
        {
            let started = std::time::Instant::now();
            let root = menu_root(&device, first, uavs)?;
            let pipeline = pipeline(&device, &root, entry)?;
            tracing::info!(
                elapsed_ms = started.elapsed().as_secs_f64() * 1e3,
                "couch menu: compute pass made"
            );
            self.menu.pipeline = Some((entry, root, pipeline));
        }
        let constants = overlay.constants(block);
        // SAFETY: The slot is idle, and its 48 menu bytes fit the second 256-byte region,
        // `(SLOTS + slot) * 256`, of the mapped SLOTS * 512 bytes.
        unsafe {
            std::ptr::copy_nonoverlapping(
                constants.as_ptr().cast::<u8>(),
                self.mapped.add((SLOTS + slot) * 256),
                std::mem::size_of_val(&constants),
            );
        }
        let size = overlay.image_size();
        let (index, copy) = match self.menu.current {
            Some((index, revision)) if revision == overlay.revision => (index, None),
            current => {
                let ring: Vec<_> = self.menu.ring.iter().map(|i| (i.size, i.read)).collect();
                let picked = pick(&ring, current.map(|(index, _)| index), size, |value| {
                    self.compute.completed(value)
                });
                let (index, fresh) = match picked {
                    Pick::Reuse(index) => (index, false),
                    Pick::Push => {
                        self.menu.ring.push(menu_texture(&device, size)?);
                        (self.menu.ring.len() - 1, true)
                    }
                    Pick::Replace(index) => {
                        self.menu.ring[index] = menu_texture(&device, size)?;
                        (index, true)
                    }
                };
                let buffer = self.menu_upload(&overlay)?;
                self.menu.uploads += 1;
                (index, Some((buffer, size, fresh)))
            }
        };
        let texture = self.menu.ring[index].texture.clone();
        // SAFETY: The slot is idle, so its view may be rewritten, and `texture` is live.
        unsafe {
            device.CreateShaderResourceView(
                &texture,
                Some(&D3D12_SHADER_RESOURCE_VIEW_DESC {
                    Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                    ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2D,
                    Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
                    Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
                        Texture2D: D3D12_TEX2D_SRV {
                            MostDetailedMip: 0,
                            MipLevels: 1,
                            PlaneSlice: 0,
                            ResourceMinLODClamp: 0.,
                        },
                    },
                }),
                D3D12_CPU_DESCRIPTOR_HANDLE {
                    ptr: self.heap.GetCPUDescriptorHandleForHeapStart().ptr
                        + (slot * VIEWS + VIEW) * self.increment as usize,
                },
            );
        }
        let (_, root, pipeline) = self.menu.pipeline.as_ref().expect("made above");
        self.menu.ring[index].read = u64::MAX;
        self.menu.passes += 1;
        Ok(Some(Pass {
            root: root.clone(),
            pipeline: pipeline.clone(),
            index,
            revision: overlay.revision,
            texture,
            copy,
            groups: [constants[6].div_ceil(8), constants[7].div_ceil(8)],
        }))
    }
    /// The panel image in the upload buffer, once its last copy has run.
    fn menu_upload(&mut self, overlay: &Overlay) -> Result<ID3D12Resource> {
        let [width, height] = overlay.image_size();
        let pitch = (width * 4).div_ceil(256) * 256;
        let bytes = u64::from(pitch) * u64::from(height);
        if let Some(upload) = &self.menu.upload {
            self.compute.wait(upload.fence)?;
        }
        if self.menu.upload.as_ref().is_none_or(|u| u.bytes < bytes) {
            let mut buffer: Option<ID3D12Resource> = None;
            // SAFETY: The device is live and the descriptions outlive the call.
            unsafe {
                self.compute.device.CreateCommittedResource(
                    &D3D12_HEAP_PROPERTIES {
                        Type: D3D12_HEAP_TYPE_UPLOAD,
                        ..Default::default()
                    },
                    D3D12_HEAP_FLAG_NONE,
                    &buffer_desc(bytes),
                    D3D12_RESOURCE_STATE_GENERIC_READ,
                    None,
                    &mut buffer,
                )?;
            }
            self.menu.upload = Some(Upload {
                buffer: buffer.context("no couch menu upload buffer")?,
                bytes,
                fence: 0,
            });
        }
        let upload = self.menu.upload.as_mut().expect("made above");
        let row = width as usize * 4;
        // SAFETY: The buffer's last copy has run (waited above), it holds `pitch * height` bytes,
        // and `validate` checked that `rgba` holds `row * height`.
        unsafe {
            let mut mapped = std::ptr::null_mut();
            upload.buffer.Map(0, None, Some(&mut mapped))?;
            for y in 0..height as usize {
                std::ptr::copy_nonoverlapping(
                    overlay.rgba.as_ptr().add(y * row),
                    mapped.cast::<u8>().add(y * pitch as usize),
                    row,
                );
            }
            upload.buffer.Unmap(0, None);
        }
        upload.fence = u64::MAX;
        Ok(upload.buffer.clone())
    }
    /// The conversion that reads `pass` was submitted as `fence`. Only now
    /// does its texture count as holding the panel: a conversion that failed
    /// before submitting never ran its copy.
    pub(super) fn menu_submitted(&mut self, pass: &Pass, fence: u64) {
        self.menu.current = Some((pass.index, pass.revision));
        self.menu.ring[pass.index].read = fence;
        if pass.copy.is_some()
            && let Some(upload) = &mut self.menu.upload
        {
            upload.fence = fence;
        }
    }
}
fn menu_texture(device: &ID3D12Device, [width, height]: [u32; 2]) -> Result<Image> {
    let mut texture: Option<ID3D12Resource> = None;
    // SAFETY: The device is live and the descriptions outlive the call.
    unsafe {
        device.CreateCommittedResource(
            &D3D12_HEAP_PROPERTIES {
                Type: D3D12_HEAP_TYPE_DEFAULT,
                ..Default::default()
            },
            D3D12_HEAP_FLAG_NONE,
            &D3D12_RESOURCE_DESC {
                Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
                Width: u64::from(width),
                Height: height,
                DepthOrArraySize: 1,
                MipLevels: 1,
                Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
                Flags: D3D12_RESOURCE_FLAG_NONE,
                Alignment: 0,
            },
            D3D12_RESOURCE_STATE_COPY_DEST,
            None,
            &mut texture,
        )?;
    }
    Ok(Image {
        texture: texture.context("no couch menu texture")?,
        size: [width, height],
        read: 0,
    })
}
impl Pass {
    /// Before the closed dispatch: a new panel image into its texture.
    ///
    /// # Safety
    ///
    /// `list` is open on the converter's device.
    pub(super) unsafe fn copy(&self, list: &ID3D12GraphicsCommandList) {
        let Some((upload, [width, height], fresh)) = &self.copy else {
            return;
        };
        // SAFETY: Both resources are live; each copy location's reference is released after use.
        unsafe {
            if !fresh {
                let mut barriers = [transition(
                    &self.texture,
                    D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE,
                    D3D12_RESOURCE_STATE_COPY_DEST,
                )];
                list.ResourceBarrier(&barriers);
                drop_barriers(&mut barriers);
            }
            let destination = D3D12_TEXTURE_COPY_LOCATION {
                pResource: std::mem::ManuallyDrop::new(Some(self.texture.clone())),
                Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
                Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                    SubresourceIndex: 0,
                },
            };
            let source = D3D12_TEXTURE_COPY_LOCATION {
                pResource: std::mem::ManuallyDrop::new(Some(upload.clone())),
                Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
                Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                    PlacedFootprint: D3D12_PLACED_SUBRESOURCE_FOOTPRINT {
                        Offset: 0,
                        Footprint: D3D12_SUBRESOURCE_FOOTPRINT {
                            Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                            Width: *width,
                            Height: *height,
                            Depth: 1,
                            RowPitch: (width * 4).div_ceil(256) * 256,
                        },
                    },
                },
            };
            list.CopyTextureRegion(&destination, 0, 0, 0, &source, None);
            drop(std::mem::ManuallyDrop::into_inner(destination.pResource));
            drop(std::mem::ManuallyDrop::into_inner(source.pResource));
            let mut barriers = [transition(
                &self.texture,
                D3D12_RESOURCE_STATE_COPY_DEST,
                D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE,
            )];
            list.ResourceBarrier(&barriers);
            drop_barriers(&mut barriers);
        }
    }
    /// After the closed dispatch, while its outputs are still unordered
    /// access: the panel's blocks again. `constants` is the constant
    /// buffer's address and `table` a slot view's descriptor.
    ///
    /// # Safety
    ///
    /// `list` is open with the converter's heap set, and the slot's views
    /// are written.
    pub(super) unsafe fn record(
        &self,
        list: &ID3D12GraphicsCommandList,
        constants: u64,
        slot: usize,
        table: impl Fn(usize) -> D3D12_GPU_DESCRIPTOR_HANDLE,
    ) {
        // SAFETY: As the caller guarantees; the null UAV barrier holds no reference.
        unsafe {
            list.ResourceBarrier(&[uav_barrier()]);
            list.SetPipelineState(&self.pipeline);
            list.SetComputeRootSignature(&self.root);
            list.SetComputeRootConstantBufferView(0, constants + (slot * 256) as u64);
            list.SetComputeRootDescriptorTable(1, table(0));
            list.SetComputeRootDescriptorTable(2, table(2));
            list.SetComputeRootConstantBufferView(3, constants + ((SLOTS + slot) * 256) as u64);
            list.SetComputeRootDescriptorTable(4, table(VIEW));
            list.Dispatch(self.groups[0], self.groups[1], 1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlay_ring_reuses_a_texture_only_after_its_readers_finish() {
        let size = [32, 16];
        let done = |value: u64| value <= 10;
        assert_eq!(pick(&[], None, size, done), Pick::Push);
        // The only texture is still read: a second one, not a rewrite.
        assert_eq!(pick(&[(size, 11)], Some(0), size, done), Pick::Push);
        assert_eq!(pick(&[(size, u64::MAX)], Some(0), size, done), Pick::Push);
        assert_eq!(
            pick(&[(size, 11), (size, 9)], Some(0), size, done),
            Pick::Reuse(1)
        );
        // A finished texture of another size is not reused.
        assert_eq!(
            pick(&[(size, 11), ([16, 16], 9)], Some(0), size, done),
            Pick::Push
        );
        // A full ring still read replaces rather than rewrites.
        let full = [(size, 11), (size, 12), (size, u64::MAX)];
        assert_eq!(pick(&full, Some(2), size, done), Pick::Replace(0));
        assert_eq!(pick(&full, Some(0), size, done), Pick::Replace(1));
        let full = [(size, 11), ([16, 16], 3), (size, 12)];
        assert_eq!(pick(&full, Some(2), size, done), Pick::Replace(0));
        assert_eq!(pick(&full, Some(2), [16, 16], done), Pick::Reuse(1));
    }
}
