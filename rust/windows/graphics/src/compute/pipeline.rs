//! D3D12 compute root signatures, pipelines, descriptor views, barriers and slot types.

use super::*;

pub(crate) fn format(pixel: crate::capture::Pixel) -> DXGI_FORMAT {
    match pixel {
        crate::capture::Pixel::Bgra8 => DXGI_FORMAT_B8G8R8A8_UNORM,
        crate::capture::Pixel::RgbaF16 => DXGI_FORMAT_R16G16B16A16_FLOAT,
        crate::capture::Pixel::Rgba10Pq => DXGI_FORMAT_R10G10B10A2_UNORM,
    }
}

pub(super) fn shader(entry: &[u8]) -> Result<&'static [u8]> {
    shader_bytecode(entry, b"cs_5_0\0").context("compute shader missing from the build")
}
/// Constants, two source views (t0, t1) and `uavs` output views from
/// register `u{first}`, no output table for 0.
pub(super) fn root_signature(
    device: &ID3D12Device,
    first: u32,
    uavs: u32,
) -> Result<ID3D12RootSignature> {
    root_signature_with(device, first, uavs, &[])
}
/// [`root_signature`] followed by `extra` parameters, from parameter 3 (2
/// without outputs).
pub(super) fn root_signature_with(
    device: &ID3D12Device,
    first: u32,
    uavs: u32,
    extra: &[D3D12_ROOT_PARAMETER],
) -> Result<ID3D12RootSignature> {
    // SAFETY: `srv`, `uav` and `parameters` outlive the serialize call that reads them, and the
    // slice covers exactly the blob's GetBufferSize bytes while `blob` is alive.
    unsafe {
        let srv = D3D12_DESCRIPTOR_RANGE {
            RangeType: D3D12_DESCRIPTOR_RANGE_TYPE_SRV,
            NumDescriptors: 2,
            BaseShaderRegister: 0,
            RegisterSpace: 0,
            OffsetInDescriptorsFromTableStart: 0,
        };
        let uav = D3D12_DESCRIPTOR_RANGE {
            RangeType: D3D12_DESCRIPTOR_RANGE_TYPE_UAV,
            NumDescriptors: uavs,
            BaseShaderRegister: first,
            ..srv
        };
        let mut parameters = vec![
            D3D12_ROOT_PARAMETER {
                ParameterType: D3D12_ROOT_PARAMETER_TYPE_CBV,
                Anonymous: D3D12_ROOT_PARAMETER_0 {
                    Descriptor: D3D12_ROOT_DESCRIPTOR {
                        ShaderRegister: 0,
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
                        pDescriptorRanges: &srv,
                    },
                },
                ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
            },
        ];
        if uavs > 0 {
            parameters.push(D3D12_ROOT_PARAMETER {
                ParameterType: D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE,
                Anonymous: D3D12_ROOT_PARAMETER_0 {
                    DescriptorTable: D3D12_ROOT_DESCRIPTOR_TABLE {
                        NumDescriptorRanges: 1,
                        pDescriptorRanges: &uav,
                    },
                },
                ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
            });
        }
        parameters.extend_from_slice(extra);
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
        let blob = blob.context("no root signature")?;
        Ok(device.CreateRootSignature(
            0,
            std::slice::from_raw_parts(blob.GetBufferPointer().cast::<u8>(), blob.GetBufferSize()),
        )?)
    }
}
pub(super) fn pipeline(
    device: &ID3D12Device,
    root: &ID3D12RootSignature,
    entry: &[u8],
) -> Result<ID3D12PipelineState> {
    let code = shader(entry)?;
    // SAFETY: `code` is the build's cs_5_0 bytecode with its exact length, and `root` is a live
    // root signature on `device`, borrowed without adding a reference to the descriptor.
    unsafe {
        Ok(
            device.CreateComputePipelineState(&D3D12_COMPUTE_PIPELINE_STATE_DESC {
                pRootSignature: std::mem::transmute_copy(root),
                CS: D3D12_SHADER_BYTECODE {
                    pShaderBytecode: code.as_ptr().cast(),
                    BytecodeLength: code.len(),
                },
                ..Default::default()
            })?,
        )
    }
}
/// One converted picture, ready for the encoder once `fence` reaches `value`.
pub struct Converted {
    pub texture: Arc<ID3D12Resource>,
    pub fence: ID3D12Fence,
    pub value: u64,
    /// The compute queue's fence value after this conversion: its focus
    /// map's [`MapView::value`], a different timeline from `value`.
    pub conversion: u64,
}
impl Converted {
    /// Block until the conversion is done.
    pub fn wait(&self) -> Result<()> {
        wait_fence(&self.fence, self.value)
    }
}
/// An output texture with a fence of its own. The encoder waits for the
/// fence and, in AMF's case, signals it again once it is done reading, so
/// the fence must not be shared with anything else.
pub(super) struct Target {
    pub(super) texture: Arc<ID3D12Resource>,
    pub(super) fence: ID3D12Fence,
    pub(super) value: u64,
}
impl Target {
    /// The value the fence was last set to wait for, by us or the encoder.
    pub(super) fn last(&self) -> u64 {
        crate::amf::gpu::fence_value(&self.fence).map_or(self.value, |v| v.max(self.value))
    }
    /// Free when nothing holds the texture and nothing will still signal
    /// its fence.
    pub(super) fn free(&self) -> Result<bool> {
        // SAFETY: `fence` is a live D3D12 fence owned by this target; GetCompletedValue only reads
        // it.
        let done = completed_value(unsafe { self.fence.GetCompletedValue() })?;
        Ok(Arc::strong_count(&self.texture) == 1 && done >= self.last())
    }
}
pub(super) struct Slot {
    pub(super) allocator: ID3D12CommandAllocator,
    pub(super) list: ID3D12GraphicsCommandList,
    pub(super) fence: u64,
    /// The source, cursor and panel image the slot's last conversion reads,
    /// kept until it has run: the adapter-wide cache may release its own
    /// references.
    pub(super) inputs: [Option<ID3D12Resource>; 3],
    pub(super) capture: Option<crate::capture::GpuImage>,
    pub(super) outputs: Vec<ID3D12Resource>,
    pub(super) waits: Vec<ID3D12Fence>,
    /// The HDR statistics pass reading the source after the conversion
    /// (`compute_hdr_stats.rs`): 0 for none, [`hdr_stats::QUEUED`] until it is
    /// submitted, `u64::MAX` while it is.
    pub(super) stats: u64,
    /// Cached source SRV identity `(raw_ptr, dxgi_format, cache_epoch)` in `at(0)`.
    pub(super) source_srv: Option<(usize, i32, u64)>,
    /// Cached pointer SRV identity `(raw_ptr_or_0, pointer_epoch)` in `at(1)`.
    pub(super) pointer_srv: Option<(usize, u64)>,
    /// The PyroWave planes whose three UAVs are in the slot's `at(2..=4)`, with
    /// their format, so a repeated conversion of the encoder's planes does not
    /// recreate them. The slot's `outputs` keep the planes alive while cached.
    pub(super) planar_view: Option<([usize; 3], i32)>,
}
impl Slot {
    /// When the slot's submitted lists stop reading its inputs and views.
    pub(super) fn last(&self) -> u64 {
        match self.stats {
            hdr_stats::QUEUED => self.fence,
            stats => self.fence.max(stats),
        }
    }
}
pub(super) struct PointerUpload {
    pub(super) fence: u64,
    pub(super) _allocator: ID3D12CommandAllocator,
    pub(super) _list: ID3D12GraphicsCommandList,
    pub(super) _upload: ID3D12Resource,
    pub(super) _texture: ID3D12Resource,
}
/// Target texture and UAV layout for [`Converter::convert`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OutputLayout {
    /// 8-bit 4:2:0: `DXGI_FORMAT_NV12` resource written through `R8_UNORM` and
    /// `R8G8_UNORM` plane UAVs.
    Nv12,
    /// 10-bit 4:2:0: `DXGI_FORMAT_P010` resource written through `R16_UNORM` and
    /// `R16G16_UNORM` plane UAVs.
    P010,
    /// 8-bit native 4:4:4 (Rubylight #150): `DXGI_FORMAT_AYUV` resource written
    /// through a single `R8G8B8A8_UINT` UAV (`DXGI_FORMAT_AYUV` itself is
    /// invalid as a UAV view format and removes the D3D12 device).
    Ayuv,
}

impl OutputLayout {
    pub(crate) const fn for_420(ten_bit: bool) -> Self {
        if ten_bit { Self::P010 } else { Self::Nv12 }
    }
    pub(crate) const fn texture_format(self) -> DXGI_FORMAT {
        match self {
            Self::Nv12 => DXGI_FORMAT_NV12,
            Self::P010 => DXGI_FORMAT_P010,
            Self::Ayuv => DXGI_FORMAT_AYUV,
        }
    }
    /// View formats for `CreateUnorderedAccessView` on plane 0 (and plane 1 for
    /// semi-planar 4:2:0). Never returns `DXGI_FORMAT_AYUV` as a UAV format.
    pub(crate) const fn uav_formats(self) -> &'static [DXGI_FORMAT] {
        match self {
            Self::Nv12 => &[DXGI_FORMAT_R8_UNORM, DXGI_FORMAT_R8G8_UNORM],
            Self::P010 => &[DXGI_FORMAT_R16_UNORM, DXGI_FORMAT_R16G16_UNORM],
            Self::Ayuv => &[DXGI_FORMAT_R8G8B8A8_UINT],
        }
    }
    pub(crate) const fn uav_count(self) -> u32 {
        self.uav_formats().len() as u32
    }
    pub(crate) const fn shader_entry(self) -> &'static [u8] {
        match self {
            Self::Nv12 | Self::P010 => b"yuv420_cs\0",
            Self::Ayuv => b"yuv444_ayuv_cs\0",
        }
    }
    pub(crate) const fn focus_entry(self) -> &'static [u8] {
        match self {
            Self::Nv12 | Self::P010 => b"yuv420_focus_cs\0",
            Self::Ayuv => b"yuv444_ayuv_focus_cs\0",
        }
    }
    pub(crate) const fn menu_entry(
        self,
        packs_chroma: bool,
        packing: u32,
    ) -> (&'static [u8], [u32; 2]) {
        match (self, packs_chroma, packing) {
            (Self::Ayuv, _, _) => (b"yuv444_ayuv_menu_cs\0", [1, 1]),
            (_, false, _) => (b"yuv420_menu_cs\0", [2, 2]),
            (_, true, 2) => (b"chroma_pack_menu_cs\0", [2, 2]),
            (_, true, _) => (b"chroma_pack_menu_cs\0", [2, 1]),
        }
    }
    pub(crate) const fn dispatch_groups(self, width: u32, height: u32) -> (u32, u32) {
        match self {
            Self::Nv12 | Self::P010 => ((width / 2).div_ceil(8), (height / 2).div_ceil(8)),
            Self::Ayuv => (width.div_ceil(16), height.div_ceil(16)),
        }
    }
    /// Dispatch groups accounting for packed 4:4:4 v2, where each thread owns
    /// a 2x2 logical block (half as many rows as v1's 2x1 pairs).
    pub(crate) const fn dispatch_groups_for(
        self,
        width: u32,
        height: u32,
        packs_chroma: bool,
        packing: u32,
    ) -> (u32, u32) {
        if packs_chroma && packing == 2 {
            ((width / 2).div_ceil(8), (height / 4).div_ceil(8))
        } else {
            self.dispatch_groups(width, height)
        }
    }
}

pub(super) fn validate_colour_input(values: &[u32; 20]) -> Result<()> {
    // cbuffer Config: pixel=4, hdr=5, toneMap=17. scRGB/FP16 has an SDR
    // path, PQ only through the tone mapping.
    anyhow::ensure!(
        values[4] != 2 || values[5] != 0 || values[17] != 0,
        "HDR surface supplied to an SDR converter (hdr_sdr_tone_map is off)"
    );
    Ok(())
}

pub(super) fn require_format_support(
    device: &ID3D12Device,
    format: DXGI_FORMAT,
    support1: D3D12_FORMAT_SUPPORT1,
    support2: D3D12_FORMAT_SUPPORT2,
) -> Result<()> {
    let mut support = D3D12_FEATURE_DATA_FORMAT_SUPPORT {
        Format: format,
        ..Default::default()
    };
    // SAFETY: The feature selector matches the live initialized structure,
    // whose exact size is supplied. This is a read-only device capability query.
    unsafe {
        device.CheckFeatureSupport(
            D3D12_FEATURE_FORMAT_SUPPORT,
            (&mut support as *mut D3D12_FEATURE_DATA_FORMAT_SUPPORT).cast(),
            std::mem::size_of_val(&support) as u32,
        )?;
    }
    anyhow::ensure!(
        support.Support1.contains(support1) && support.Support2.contains(support2),
        "D3D12 format {format:?} does not support compute YUV writes"
    );
    Ok(())
}

/// A transition of every subresource; `drop_barriers` releases its reference.
pub(super) fn transition(
    resource: &ID3D12Resource,
    before: D3D12_RESOURCE_STATES,
    after: D3D12_RESOURCE_STATES,
) -> D3D12_RESOURCE_BARRIER {
    D3D12_RESOURCE_BARRIER {
        Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
        Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
        Anonymous: D3D12_RESOURCE_BARRIER_0 {
            Transition: std::mem::ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER {
                pResource: std::mem::ManuallyDrop::new(Some(resource.clone())),
                Subresource: D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES,
                StateBefore: before,
                StateAfter: after,
            }),
        },
    }
}
/// Release the resource references a transition barrier holds.
pub(super) fn drop_barriers(barriers: &mut [D3D12_RESOURCE_BARRIER]) {
    for barrier in barriers {
        // SAFETY: Callers pass only barriers from `transition`, once each, so `Transition` is the
        // active field and still holds the one reference released here.
        unsafe {
            let mut transition = std::mem::ManuallyDrop::take(&mut barrier.Anonymous.Transition);
            std::mem::ManuallyDrop::drop(&mut transition.pResource);
        }
    }
}
pub(super) fn buffer_desc(size: u64) -> D3D12_RESOURCE_DESC {
    D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Width: size,
        Height: 1,
        DepthOrArraySize: 1,
        MipLevels: 1,
        Format: DXGI_FORMAT_UNKNOWN,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        Flags: D3D12_RESOURCE_FLAG_NONE,
        Alignment: 0,
    }
}
/// A shader-resource view of a whole one-mip 2D texture.
pub(super) fn texture_srv(format: DXGI_FORMAT) -> D3D12_SHADER_RESOURCE_VIEW_DESC {
    D3D12_SHADER_RESOURCE_VIEW_DESC {
        Format: format,
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
    }
}
/// An unordered-access view of `plane` of a 2D texture's first mip.
pub(super) fn texture_uav(format: DXGI_FORMAT, plane: u32) -> D3D12_UNORDERED_ACCESS_VIEW_DESC {
    D3D12_UNORDERED_ACCESS_VIEW_DESC {
        Format: format,
        ViewDimension: D3D12_UAV_DIMENSION_TEXTURE2D,
        Anonymous: D3D12_UNORDERED_ACCESS_VIEW_DESC_0 {
            Texture2D: D3D12_TEX2D_UAV {
                MipSlice: 0,
                PlaneSlice: plane,
            },
        },
    }
}
