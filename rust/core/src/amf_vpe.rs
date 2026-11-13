//! What AMF's video converter (`AMFVideoConverter`, which a driver runs on the
//! Video Processing Engine where the GPU has one) may convert for an AMF
//! stream, and how. The Windows encoder (`butterpollo_windows::amf::vpe`)
//! applies the plan; everything a plan refuses keeps the shader conversion
//! (`gpu_color`), with the refusal's code in the log.
//!
//! The option and its conversion paths (SDR to NV12, scRGB to PQ or HLG P010
//! with a scene-linear HLG prepass, scaling with black bars) are ported from
//! Foundation Sunshine's `amd_vpe` (src/amf/amf_vpe_d3d11.cpp and
//! src/platform/windows/display_vram.cpp, AlkaidLab/foundation-sunshine
//! PR #1216, commit 1add1ed), Copyright (c) Foundation Sunshine authors,
//! GPL-3.0. Unlike Foundation's, the converted surface goes to the encoder
//! directly: no CPU wait for the GPU and no copy into an encoder texture.
use crate::rtsp::Negotiated;

/// The setting: `amd_vpe`, off by default as in Foundation Sunshine.
pub fn wanted(config: &crate::config::Config) -> bool {
    config.boolean("amd_vpe", false)
}

/// The captured picture's format, as `capture::Pixel` names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// 8-bit sRGB (`DXGI_FORMAT_B8G8R8A8_UNORM`).
    Bgra8,
    /// Windows' scRGB: linear Rec.709 primaries, 1.0 = 80 nits
    /// (`DXGI_FORMAT_R16G16B16A16_FLOAT`).
    ScRgbF16,
    /// PQ-coded BT.2020 (`DXGI_FORMAT_R10G10B10A2_UNORM`).
    Pq10,
}

/// The stream's transfer function.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transfer {
    Sdr,
    Pq,
    Hlg,
}

/// The parts of a negotiated stream a conversion plan depends on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stream {
    pub width: u32,
    pub height: u32,
    pub transfer: Transfer,
    /// `Negotiated::color_matrix`: 0 Rec.601, 1 Rec.709, 2 Rec.2020.
    pub matrix: u8,
    pub full_range: bool,
    pub ten_bit: bool,
    pub chroma_packing: u8,
    pub yuv444: bool,
}
impl Stream {
    pub fn of(config: &Negotiated) -> Self {
        Self {
            width: config.width,
            height: config.height,
            transfer: if config.hlg() {
                Transfer::Hlg
            } else if config.hdr {
                Transfer::Pq
            } else {
                Transfer::Sdr
            },
            matrix: config.color_matrix(),
            full_range: config.full_range(),
            ten_bit: config.ten_bit(),
            chroma_packing: config.chroma_packing,
            yuv444: config.yuv444,
        }
    }
}

/// Why a stream or a frame keeps the shader conversion. `code` is the stable
/// name in logs and timing reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// AMD's encoder takes 4:2:0 only; 4:4:4 never reaches AMF.
    Yuv444,
    /// Packed 4:4:4 lays chroma out in a picture of its own.
    PackedChroma,
    /// An odd or zero size: 4:2:0 output needs even dimensions.
    Size,
    /// An SDR desktop on an HDR stream: the shader lifts it to SDR white.
    SdrSourceOnHdrStream,
    /// An HDR desktop on an SDR stream: the shader maps it back to SDR white.
    HdrSourceOnSdrStream,
    /// The stream scales HDR brightness (`hdrScale` other than 1).
    LuminanceScale,
    /// The desktop pointer is drawn into this frame by the shader.
    Pointer,
    /// The couch menu is drawn into this frame by the shader.
    Menu,
}
impl Refusal {
    pub fn code(self) -> &'static str {
        match self {
            Self::Yuv444 => "vpe_yuv444",
            Self::PackedChroma => "vpe_packed_chroma",
            Self::Size => "vpe_size",
            Self::SdrSourceOnHdrStream => "vpe_sdr_source_on_hdr_stream",
            Self::HdrSourceOnSdrStream => "vpe_hdr_source_on_sdr_stream",
            Self::LuminanceScale => "vpe_luminance_scale",
            Self::Pointer => "vpe_pointer",
            Self::Menu => "vpe_menu",
        }
    }
    /// Whether the refusal holds for the whole stream, not just one frame.
    pub fn whole_stream(self) -> bool {
        matches!(self, Self::Yuv444 | Self::PackedChroma | Self::Size)
    }
}

/// AMF's surface formats, as `AMF_SURFACE_FORMAT` numbers them (Surface.h).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Nv12,
    P010,
    Bgra,
    RgbaF16,
    R10G10B10A2,
}
impl Format {
    pub fn amf(self) -> i64 {
        match self {
            Self::Nv12 => 1,
            Self::Bgra => 3,
            Self::P010 => 10,
            Self::RgbaF16 => 11,
            Self::R10G10B10A2 => 13,
        }
    }
}

/// One side of the conversion, in AMF's VUI numbering (ColorSpace.h):
/// `AMF_COLOR_PRIMARIES_ENUM`, `AMF_COLOR_TRANSFER_CHARACTERISTIC_ENUM`,
/// `AMF_COLOR_MATRIX_COEFF_ENUM` and `AMF_COLOR_RANGE_ENUM`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Color {
    pub primaries: i64,
    pub transfer: i64,
    pub matrix: i64,
    pub range: i64,
}
pub mod vui {
    pub const PRIMARIES_BT709: i64 = 1;
    pub const PRIMARIES_SMPTE170M: i64 = 6;
    pub const PRIMARIES_BT2020: i64 = 9;
    pub const PRIMARIES_CCCS: i64 = 1000;
    pub const TRANSFER_BT709: i64 = 1;
    pub const TRANSFER_SMPTE170M: i64 = 6;
    pub const TRANSFER_LINEAR: i64 = 8;
    pub const TRANSFER_SRGB: i64 = 13;
    pub const TRANSFER_BT2020_10: i64 = 14;
    pub const TRANSFER_PQ: i64 = 16;
    pub const TRANSFER_HLG: i64 = 18;
    pub const MATRIX_RGB: i64 = 0;
    pub const MATRIX_BT709: i64 = 1;
    pub const MATRIX_BT601: i64 = 6;
    pub const MATRIX_BT2020_NCL: i64 = 9;
    pub const RANGE_STUDIO: i64 = 1;
    pub const RANGE_FULL: i64 = 2;
    /// `AMF_VIDEO_CONVERTER_COLOR_PROFILE_ENUM`.
    pub const PROFILE_601: i64 = 0;
    pub const PROFILE_709: i64 = 1;
    pub const PROFILE_2020: i64 = 2;
    /// Full-range Rec.601.
    pub const PROFILE_JPEG: i64 = 3;
    pub const PROFILE_FULL_709: i64 = 7;
    pub const PROFILE_FULL_2020: i64 = 8;
    /// `AMF_VIDEO_CONVERTER_ACCELERATION_VPE_PREFERRED`.
    pub const ACCELERATION_VPE_PREFERRED: i64 = 2;
    /// `AMF_VIDEO_CONVERTER_ACCELERATION_COMPUTE`.
    pub const ACCELERATION_COMPUTE: i64 = 1;
    /// `AMF_VIDEO_CONVERTER_SCALE_BICUBIC`.
    pub const SCALE_BICUBIC: i64 = 1;
    /// `AMF_VIDEO_CONVERTER_TONEMAPPING_COPY`.
    pub const TONEMAPPING_COPY: i64 = 0;
}

/// The converter's output for a stream: the encoder's input surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Output {
    pub format: Format,
    pub profile: i64,
    pub color: Color,
    pub transfer: Transfer,
}

/// Whether the stream can be converted by AMF at all, and into what.
pub fn stream_plan(stream: &Stream) -> Result<Output, Refusal> {
    if stream.yuv444 {
        return Err(Refusal::Yuv444);
    }
    if stream.chroma_packing != 0 {
        return Err(Refusal::PackedChroma);
    }
    if stream.width == 0
        || stream.height == 0
        || !stream.width.is_multiple_of(2)
        || !stream.height.is_multiple_of(2)
    {
        return Err(Refusal::Size);
    }
    let range = if stream.full_range {
        vui::RANGE_FULL
    } else {
        vui::RANGE_STUDIO
    };
    let format = if stream.ten_bit {
        Format::P010
    } else {
        Format::Nv12
    };
    let (profile, color) = match (stream.transfer, stream.matrix) {
        (Transfer::Pq | Transfer::Hlg, _) => (
            if stream.full_range {
                vui::PROFILE_FULL_2020
            } else {
                vui::PROFILE_2020
            },
            Color {
                primaries: vui::PRIMARIES_BT2020,
                transfer: if stream.transfer == Transfer::Hlg {
                    vui::TRANSFER_HLG
                } else {
                    vui::TRANSFER_PQ
                },
                matrix: vui::MATRIX_BT2020_NCL,
                range,
            },
        ),
        (Transfer::Sdr, 0) => (
            if stream.full_range {
                vui::PROFILE_JPEG
            } else {
                vui::PROFILE_601
            },
            Color {
                primaries: vui::PRIMARIES_SMPTE170M,
                transfer: vui::TRANSFER_SMPTE170M,
                matrix: vui::MATRIX_BT601,
                range,
            },
        ),
        (Transfer::Sdr, 2) => (
            if stream.full_range {
                vui::PROFILE_FULL_2020
            } else {
                vui::PROFILE_2020
            },
            Color {
                primaries: vui::PRIMARIES_BT2020,
                transfer: vui::TRANSFER_BT2020_10,
                matrix: vui::MATRIX_BT2020_NCL,
                range,
            },
        ),
        (Transfer::Sdr, _) => (
            if stream.full_range {
                vui::PROFILE_FULL_709
            } else {
                vui::PROFILE_709
            },
            Color {
                primaries: vui::PRIMARIES_BT709,
                transfer: vui::TRANSFER_BT709,
                matrix: vui::MATRIX_BT709,
                range,
            },
        ),
    };
    Ok(Output {
        format,
        profile,
        color,
        transfer: stream.transfer,
    })
}

/// The converter's input for one frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Input {
    pub format: Format,
    pub color: Color,
    /// The frame goes through the scene-linear HLG prepass first
    /// (`hlg_scene_cs` in color.hlsl), and `format` is that pass's output.
    pub hlg_prepass: bool,
    /// The converter's input brightness limits (`InputHdrMetadata`): an HDR
    /// input's nominal 0 to 10000 nits, so AMF neither lifts black nor clips
    /// above its 1000-nit default. `None` for SDR.
    pub hdr_peak_nits: Option<u32>,
}

/// What one frame needs besides its pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub source: Source,
    /// `luminance[1]`, the stream's HDR brightness scale (1.0 = unscaled).
    pub hdr_scale: f32,
    pub pointer: bool,
    pub menu: bool,
}

/// How AMF converts this frame for `output`, or why the shader does.
pub fn frame_plan(output: &Output, frame: &Frame) -> Result<Input, Refusal> {
    if frame.menu {
        return Err(Refusal::Menu);
    }
    if frame.pointer {
        return Err(Refusal::Pointer);
    }
    let full = vui::RANGE_FULL;
    match (output.transfer, frame.source) {
        (Transfer::Sdr, Source::Bgra8) => Ok(Input {
            format: Format::Bgra,
            // The shader codes the desktop's sRGB values as they are, without
            // linearizing them, for Rec.601 and Rec.709. Naming the output's
            // own curve as the input's keeps AMF from converting it either.
            // Rec.2020 SDR goes through linear light to the BT.2020 curve, as
            // the shader's `bt2020_sdr` does.
            color: Color {
                primaries: vui::PRIMARIES_BT709,
                transfer: if output.color.primaries == vui::PRIMARIES_BT2020 {
                    vui::TRANSFER_SRGB
                } else {
                    output.color.transfer
                },
                matrix: vui::MATRIX_RGB,
                range: full,
            },
            hlg_prepass: false,
            hdr_peak_nits: None,
        }),
        (Transfer::Sdr, _) => Err(Refusal::HdrSourceOnSdrStream),
        (Transfer::Pq | Transfer::Hlg, Source::Bgra8) => Err(Refusal::SdrSourceOnHdrStream),
        (_, _) if frame.hdr_scale != 1.0 => Err(Refusal::LuminanceScale),
        (Transfer::Pq, Source::ScRgbF16) => Ok(Input {
            format: Format::RgbaF16,
            color: Color {
                primaries: vui::PRIMARIES_CCCS,
                transfer: vui::TRANSFER_LINEAR,
                matrix: vui::MATRIX_RGB,
                range: full,
            },
            hlg_prepass: false,
            hdr_peak_nits: Some(10_000),
        }),
        (Transfer::Pq, Source::Pq10) => Ok(Input {
            format: Format::R10G10B10A2,
            color: Color {
                primaries: vui::PRIMARIES_BT2020,
                transfer: vui::TRANSFER_PQ,
                matrix: vui::MATRIX_RGB,
                range: full,
            },
            hlg_prepass: false,
            hdr_peak_nits: Some(10_000),
        }),
        // Foundation Sunshine's HLG path: the prepass turns scRGB (or PQ
        // capture) into scene-linear BT.2020 for a 1000-nit display (the
        // inverse OOTF the shader's `hlg` applies), and AMF applies the HLG
        // OETF to a linear input with a 10000-nit nominal range.
        (Transfer::Hlg, Source::ScRgbF16 | Source::Pq10) => Ok(Input {
            format: Format::RgbaF16,
            color: Color {
                primaries: vui::PRIMARIES_BT2020,
                transfer: vui::TRANSFER_LINEAR,
                matrix: vui::MATRIX_RGB,
                range: full,
            },
            hlg_prepass: true,
            hdr_peak_nits: Some(10_000),
        }),
    }
}

/// Where the picture lands in the stream, in whole pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}
/// The source's place in a `target` stream: its aspect ratio kept, centred
/// with black bars, and a size within a pixel of the stream's shape filling
/// it, as the shader's `content` places it. AMF's output rectangle is whole
/// pixels; the bars snap to even pixels so 4:2:0 chroma never straddles an
/// edge, where the shader blends the edge pixel instead.
pub fn content_rect(source: (u32, u32), target: (u32, u32)) -> Rect {
    let full = Rect {
        x: 0,
        y: 0,
        width: target.0,
        height: target.1,
    };
    if source.0 == 0 || source.1 == 0 || target.0 == 0 || target.1 == 0 {
        return full;
    }
    let ratio = (
        f64::from(target.0) / f64::from(source.0),
        f64::from(target.1) / f64::from(source.1),
    );
    let scale = ratio.0.min(ratio.1);
    let size = (f64::from(source.0) * scale, f64::from(source.1) * scale);
    if (size.0 - f64::from(target.0)).abs() < 1. && (size.1 - f64::from(target.1)).abs() < 1. {
        return full;
    }
    let even = |extent: f64, limit: u32| -> u32 {
        let rounded = ((extent / 2.).round() as u32) * 2;
        rounded.clamp(2.min(limit), limit)
    };
    let width = even(size.0, target.0);
    let height = even(size.1, target.1);
    Rect {
        x: (target.0 - width) / 2 & !1,
        y: (target.1 - height) / 2 & !1,
        width,
        height,
    }
}

/// How a stream converts its frames: the stable label the timing log and
/// the A/B harness report.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Path {
    /// `gpu_color`'s pixel or compute shader on the D3D11 graphics device.
    Shader,
    /// The D3D12 compute-queue conversion (`compute::Converter`).
    ComputeQueue,
    /// AMF's video converter, asked for the VPE.
    Vpe,
}
impl Path {
    pub fn label(self) -> &'static str {
        match self {
            Self::Shader => "shader",
            Self::ComputeQueue => "compute_queue",
            Self::Vpe => "amf_vpe",
        }
    }
}

/// A running count of frames each path converted and their conversion
/// time, logged once a second-ish by the encoder (`vpe_conversion` lines).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tally {
    pub vpe_frames: u64,
    pub shader_frames: u64,
    pub vpe_us: u64,
    pub shader_us: u64,
    pub vpe_worst_us: u64,
    /// Capture to encoder submission, across both paths.
    pub capture_to_submit_us: u64,
    pub capture_to_submit_worst_us: u64,
    /// The latest refusal, if any frame was refused.
    pub refused: Option<&'static str>,
}
impl Tally {
    pub fn add(&mut self, vpe: bool, conversion_us: u64, capture_to_submit_us: u64) {
        if vpe {
            self.vpe_frames += 1;
            self.vpe_us += conversion_us;
            self.vpe_worst_us = self.vpe_worst_us.max(conversion_us);
        } else {
            self.shader_frames += 1;
            self.shader_us += conversion_us;
        }
        self.capture_to_submit_us += capture_to_submit_us;
        self.capture_to_submit_worst_us = self.capture_to_submit_worst_us.max(capture_to_submit_us);
    }
    pub fn frames(&self) -> u64 {
        self.vpe_frames + self.shader_frames
    }
    pub fn mean_vpe_us(&self) -> f64 {
        mean(self.vpe_us, self.vpe_frames)
    }
    pub fn mean_shader_us(&self) -> f64 {
        mean(self.shader_us, self.shader_frames)
    }
    pub fn mean_capture_to_submit_us(&self) -> f64 {
        mean(self.capture_to_submit_us, self.frames())
    }
}
fn mean(total: u64, count: u64) -> f64 {
    if count == 0 {
        0.
    } else {
        total as f64 / count as f64
    }
}

#[cfg(test)]
mod tests;
