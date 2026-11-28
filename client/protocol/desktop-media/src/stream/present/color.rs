//! Colour spaces, HDR metadata/transfer decisions, and plane shader parameters.

use sdl3_sys::everything::*;

use crate::media::{self, FrameInfo};

/// Whether the session negotiated HDR: the client asked for it and the host chose a 10-bit format.
/// With "10-bit color (SDR)" a 10-bit format alone is SDR.
pub fn hdr_negotiated(asked_hdr: bool, negotiated_format: u32) -> bool {
    asked_hdr && negotiated_format & nightjar_core::sdp::VIDEO_FORMAT_MASK_10BIT != 0
}

/// FFmpeg's AVCOL_TRC_SMPTE2084 (PQ) and AVCOL_TRC_ARIB_STD_B67 (HLG).
pub(super) const TRC_PQ: i32 = 16;
pub(super) const TRC_HLG: i32 = 18;

/// Whether a picture is shown as HDR: as the host's HDR mode says once it has, else when it
/// carries the PQ or HLG transfer or the session negotiated HDR. A 10-bit format alone is not HDR.
pub(super) fn shows_hdr(hdr_mode: Option<bool>, hdr_stream: bool, color_trc: i32) -> bool {
    hdr_mode.unwrap_or(hdr_stream || color_trc == TRC_PQ || color_trc == TRC_HLG)
}

/// The plane shader's constants for a software-decoded planar picture: its colour description is
/// FFmpeg's, read as `colorspace` reads it (the shader's matrix 0 is BT.601, 2 BT.2020, else BT.709).
/// An HDR picture with the HLG transfer gets the shader's HLG flag (128), never PQ, with the
/// display it is drawn for (`planar_hlg`) in place of the wavelet pass's level and operation,
/// and that display's white as the SDR white.
pub(in crate::stream) fn planar_parameters(info: &FrameInfo, layout: media::PlanarLayout, hdr: bool, white_level: f32, headroom: f32) -> [u32; 8] {
    let matrix = match info.color_space { 5 | 6 => 0, 9 | 10 => 2, _ => 1 };
    let hlg = planar_hlg(info, hdr, white_level, headroom);
    let flags = u32::from(info.color_range == 2) | (u32::from(hdr) << 1) | (matrix << 2) | (u32::from(layout.ten_bit) << 4)
        | (u32::from(layout.chroma444) << 5) | (u32::from(layout.ten_bit) << 6) | (u32::from(hlg.is_some()) << 7)
        | (u32::from(info.centred_chroma) << 14);
    match hlg {
        Some(display) => [layout.width, layout.height, layout.width, layout.height, display.peak.to_bits(), display.gamma.to_bits(), flags, (display.white / 80.0).to_bits()],
        None => [layout.width, layout.height, layout.width, layout.height, 0, 3, flags, white_level.to_bits()],
    }
}

/// Whether a packed picture takes the other reconstruction than the one made: from planes for a
/// Vulkan picture (mapped VAAPI or Vulkan video, `vulkan_picture`) where the made one reads memory
/// (no `planes`), or the reverse, as when a new decoder's map fails.
// Both Linux and macOS use the Vulkan/Metal plane paths; tested everywhere.
#[cfg(any(unix, test))]
pub(super) fn packed_path_changed(reconstruction: bool, vulkan_picture: bool, planes: bool) -> bool {
    reconstruction && vulkan_picture != planes
}

/// `planar_parameters` for Vulkan video's copied planes when `vulkan`: flag 256 (the chroma
/// interleaved, 10-bit samples in the high bits) in place of 64 (in the low bits).
pub(in crate::stream) fn vulkan_flags(mut parameters: [u32; 8], vulkan: bool) -> [u32; 8] {
    if vulkan {
        parameters[6] = parameters[6] & !64 | 256;
    }
    parameters
}

/// `vulkan_flags` for the one copied image of a mapped 4:4:4 VAAPI picture when `interleaved`
/// (`media::FRAME_VULKAN_INTERLEAVED444`): flag 2048 in place of 256, Y, Cb and Cr read from one
/// texel, its 10-bit samples whole.
pub(in crate::stream) fn interleaved_flags(mut parameters: [u32; 8], interleaved: bool) -> [u32; 8] {
    if interleaved {
        parameters[6] = parameters[6] & !256 | 2048;
    }
    parameters
}

/// The display a plane shader picture shown as HDR with the HLG transfer is drawn for, as the
/// Windows draw pass draws it (`hlg::hlg_display`): an HDR output's own peak (`headroom` over the
/// SDR white of `white_level` x 80 nits), else HLG's 1000-nit reference display with its
/// reference white at SDR white, which SDL's tone map brings into SDR. None for other pictures.
pub(in crate::stream) fn planar_hlg(info: &FrameInfo, hdr: bool, white_level: f32, headroom: f32) -> Option<crate::stream::hlg::HlgDisplay> {
    (hdr && info.color_trc == TRC_HLG).then(|| crate::stream::hlg::hlg_display(headroom, 80.0 * white_level))
}

/// Whether a picture SDL would show (NV12 or P010 from memory, an imported VAAPI surface, or a
/// VideoToolbox surface) goes to the plane shader instead: SDL reads P010 as PQ and knows no HLG,
/// so an HLG picture is drawn as a 4:2:0 semi-planar picture with the shader's HLG flag whenever
/// SDL's `"gpu"` renderer is active. On non-GPU renderers (`"metal"`, `"opengl"`), returns `false`
/// so upload falls back safely instead of failing every frame inside `PlanesPass`. SDR and PQ
/// pictures keep SDL's upload or import as they are.
// Linux and macOS (unix) route HLG hardware surfaces through the plane shader when GPU renderer is active; tested everywhere.
#[cfg(any(unix, test))]
pub(in crate::stream) fn hlg_through_planes(info: &FrameInfo, gpu_renderer: bool) -> bool {
    gpu_renderer
        && info.color_trc == TRC_HLG
        && matches!(
            info.kind,
            media::FRAME_NV12 | media::FRAME_P010 | media::FRAME_DMABUF | media::FRAME_VIDEOTOOLBOX
        )
}

/// FFmpeg's colour description as an SDL colorspace.
pub(super) fn colorspace(info: &FrameInfo) -> SDL_Colorspace {
    // AVCOL_RANGE_JPEG = 2; AVCOL_SPC_BT709 = 1, BT470BG = 5, SMPTE170M = 6, BT2020_NCL = 9;
    // AVCOL_TRC_SMPTE2084 = 16.
    let full = info.color_range == 2;
    // A PQ picture is YCbCr BT.2020 (SDL's BT2020_* spaces carry the PQ transfer). SDL_COLORSPACE_HDR10
    // is an RGB space with the identity matrix: SDL's D3D11 renderer cannot make a P010 texture in it
    // ("Unsupported YUV colorspace"), which showed as "Cannot show d3d11 pictures".
    if info.color_trc == 16 {
        return if full { SDL_COLORSPACE_BT2020_FULL } else { SDL_COLORSPACE_BT2020_LIMITED };
    }
    match (info.color_space, full) {
        (9 | 10, false) => SDL_COLORSPACE_BT2020_LIMITED,
        (9 | 10, true) => SDL_COLORSPACE_BT2020_FULL,
        (5 | 6, false) => SDL_COLORSPACE_BT601_LIMITED,
        (5 | 6, true) => SDL_COLORSPACE_BT601_FULL,
        (_, true) => SDL_COLORSPACE_BT709_FULL,
        _ => SDL_COLORSPACE_BT709_LIMITED,
    }
}

/// The YCbCr matrix (0 BT.601, 1 BT.709, 2 BT.2020) and full range of an SDL colorspace from
/// `colorspace`, for `stream::direct`.
#[cfg(windows)]
pub(super) fn direct_matrix(space: SDL_Colorspace) -> (u32, bool) {
    match space {
        SDL_COLORSPACE_BT601_LIMITED => (0, false),
        SDL_COLORSPACE_BT601_FULL => (0, true),
        SDL_COLORSPACE_BT2020_LIMITED => (2, false),
        SDL_COLORSPACE_BT2020_FULL => (2, true),
        SDL_COLORSPACE_BT709_FULL => (1, true),
        _ => (1, false),
    }
}

/// A decoded picture's transfer, one decision for every path that shows it. When the stream
/// shows HDR (`shows_hdr`: the host's HDR mode, else the PQ or HLG transfer or the negotiated
/// HDR), a picture with the HLG transfer is HLG and any other P010 picture PQ, even when the
/// bitstream does not signal PQ. Everything else is SDR: an NV12 picture is never PQ.
#[cfg(windows)]
pub(super) fn picture_transfer(format: SDL_PixelFormat, hdr_mode: Option<bool>, hdr_stream: bool, hdr_frame: bool, hlg_frame: bool) -> crate::stream::direct::Transfer {
    use crate::stream::direct::Transfer;
    let trc = if hlg_frame { TRC_HLG } else if hdr_frame { TRC_PQ } else { 0 };
    if !matches!(format, SDL_PIXELFORMAT_P010 | SDL_PIXELFORMAT_NV12) || !shows_hdr(hdr_mode, hdr_stream, trc) {
        Transfer::Sdr
    } else if hlg_frame {
        Transfer::Hlg
    } else if format == SDL_PIXELFORMAT_P010 {
        Transfer::Pq
    } else {
        Transfer::Sdr
    }
}

/// SDL's D3D11 shaders decode every P010 texture as PQ and NV12 as gamma, and know no HLG: a
/// picture whose `picture_transfer` differs reaches SDL converted to RGB by the pass.
#[cfg(windows)]
pub(super) fn sdl_needs_rgb(format: SDL_PixelFormat, transfer: crate::stream::direct::Transfer) -> bool {
    use crate::stream::direct::Transfer;
    let sdl_reads = if format == SDL_PIXELFORMAT_P010 { Transfer::Pq } else { Transfer::Sdr };
    sdl_reads != transfer
}

/// The HDR properties of a texture request.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct HdrProperties {
    /// `SDL_PROP_TEXTURE_CREATE_SDR_WHITE_POINT_FLOAT`, in nits (HDR10 textures only).
    pub(super) white_point: Option<f32>,
    /// `SDL_PROP_TEXTURE_CREATE_HDR_HEADROOM_FLOAT`, relative to the SDR white.
    pub(super) headroom: Option<f32>,
}

/// The white point and headroom a texture is created with. `white_level` is the renderer's SDR
/// white as a multiple of 80 nits; a value that is not a positive finite number is not passed on.
pub(super) fn hdr_properties(hdr10: bool, white_level: f32, metadata: Option<nightjar_core::control::HdrMetadata>) -> HdrProperties {
    let white_nits = 80.0 * white_level;
    if !white_nits.is_finite() || white_nits <= 0.0 {
        return HdrProperties { white_point: None, headroom: None };
    }
    let headroom = metadata.and_then(|metadata| {
        let peak = if metadata.max_content_light_level != 0 {
            metadata.max_content_light_level
        } else {
            metadata.max_display_luminance
        };
        (peak > 0).then(|| f32::from(peak) / white_nits)
    });
    HdrProperties { white_point: hdr10.then_some(white_nits), headroom }
}

pub(super) fn pixel_format_name(format: SDL_PixelFormat) -> String {
    let name = SDL_GetPixelFormatName(format);
    if name.is_null() {
        return format!("format {:#x}", format.0);
    }
    // SAFETY: checked non-null above.
    unsafe { std::ffi::CStr::from_ptr(name) }.to_string_lossy().into_owned()
}
