//! FFmpeg and Opus through Rust bindings to the platform libraries.

#[cfg(any(windows, test))]
use std::ffi::c_void;
use std::ffi::{c_char, c_int, CStr};
#[cfg(test)]
use std::ptr;
#[cfg(all(test, target_os = "linux"))]
use std::ptr::NonNull;

#[cfg(test)]
use nightjar_core::video::{FrameBytes, FRAME_PADDING};

#[allow(
    non_upper_case_globals,
    non_camel_case_types,
    non_snake_case,
    dead_code,
    clippy::all
)]
pub(crate) mod ffi {
    include!(concat!(env!("OUT_DIR"), "/media.rs"));
}
use ffi::*;

mod audio;
#[cfg(windows)]
mod d3d11;
mod decoder;
mod decoder_io;
mod frame;
mod hardware;
mod wavelet_pool;

pub use audio::{pcm_s16le, OpusDecoder};
#[cfg(windows)]
pub use d3d11::ProbeVideoDevice;
#[cfg(test)]
pub use decoder::v4l2_pts_round_trip;
pub use decoder::{packet_timebase, Received, VideoDecoder, AGAIN};
#[cfg(windows)]
pub use frame::{D3d11Surface, DolbyVision};
pub use frame::{Frame, Picture};
#[cfg(target_os = "linux")]
pub use hardware::vaapi_device;
pub use hardware::{
    pool_extra, use_videotoolbox_rfi_setting, use_vulkan_video_settings, vaapi_node,
    videotoolbox_rfi_setting, vulkan_rfi_setting, Codec, DeviceRef, Hardware, Pictures,
    ProbeHardware, VaapiNode, EXTRA_HW_FRAMES, HELD_BEYOND_WAITING,
};

#[cfg(all(test, target_os = "linux"))]
use crate::stream::stage_times::Stages;
#[cfg(test)]
use decoder::{reopen_size, v4l2_open_size};
#[cfg(all(test, target_os = "linux"))]
use decoder::DECODE_STAGES;
#[cfg(test)]
use decoder_io::{
    access_unit, copy_or_drop_map, lend_packet, packet_buffer_size, refused_by, refused_name,
};
#[cfg(all(test, target_os = "linux"))]
use decoder_io::{stage_summary, time_receive};
#[cfg(test)]
use frame::frame_kind;
#[cfg(test)]
use hardware::{first_opened, mapped_extra, vaapi_node_path};

#[derive(Clone, Copy)]
pub struct FrameInfo {
    pub width: c_int,
    pub height: c_int,
    pub kind: c_int,
    pub ten_bit: c_int,
    pub pts: i64,
    pub data: [*mut u8; 3],
    pub linesize: [c_int; 3],
    pub color_space: c_int,
    pub color_range: c_int,
    pub color_trc: c_int,
}

/// Whether chroma at `location` sits midway between two luma columns. Unspecified counts as
/// left: chroma_sample_loc_type 0, which H.264, HEVC and the Moonlight hosts use and AV1
/// streams signal (CSP_VERTICAL).
pub fn centred_chroma(location: AVChromaLocation) -> bool {
    matches!(location, AVCHROMA_LOC_CENTER | AVCHROMA_LOC_TOP | AVCHROMA_LOC_BOTTOM)
}

pub const FRAME_NV12: c_int = 0;
pub const FRAME_P010: c_int = 1;
pub const FRAME_YUV420P: c_int = 2;
/// A VideoToolbox CVPixelBuffer whose NV12 or P010 planes SDL Metal can import.
pub const FRAME_VIDEOTOOLBOX: c_int = 15;
pub const FRAME_D3D11: c_int = 3;
pub const FRAME_WAVELET: c_int = 5;
/// Software-decoded planar pictures: 8-bit 4:4:4, and 10-bit 4:2:0 and 4:4:4 with the samples in
/// the low bits of each 16-bit word. The presenter draws them with the plane shader (`PlanarLayout`).
pub const FRAME_YUV444P: c_int = 6;
pub const FRAME_YUV420P10: c_int = 7;
pub const FRAME_YUV444P10: c_int = 8;
/// A VAAPI surface exported as DMA-BUFs for the presenter to import (`crate::dmabuf`), Linux only.
// Linux and macOS (unix): DMA-BUF picture kind constant.
#[cfg(unix)]
pub const FRAME_DMABUF: c_int = 9;
/// A Vulkan video picture on the GPU renderer's own device (`crate::vulkan_video`), Linux only:
/// two planes, 4:2:0 or (`FRAME_VULKAN444`) 4:4:4, 8- or 10-bit (`ten_bit`). The presenter copies
/// its planes on the GPU and draws them with the plane shader.
// Linux and macOS (unix): Vulkan 4:2:0 picture kind constant.
#[cfg(unix)]
pub const FRAME_VULKAN: c_int = 10;
// Linux and macOS (unix): Vulkan 4:4:4 picture kind constant.
#[cfg(unix)]
pub const FRAME_VULKAN444: c_int = 11;
/// A 4:4:4 VAAPI picture mapped into the GPU renderer's device as one image holding Y, Cb and Cr
/// (`vulkan_video::interleaved_444`), Linux only. The presenter copies it whole and the plane
/// shader reads all three from it.
// Linux and macOS (unix): Vulkan interleaved 4:4:4 picture kind constant.
#[cfg(unix)]
pub const FRAME_VULKAN_INTERLEAVED444: c_int = 12;
/// A D3D11VA 4:4:4 picture: an AYUV (8-bit), Y410 or Y416 (10-bit, `ten_bit`) surface, which NVIDIA
/// and Intel decoders make for H.264 High 4:4:4 and HEVC RExt 4:4:4. The presenter copies it and
/// draws it with the plane shader's surface entry (`stream::d3d11::PlanesPass::draw_surface`).
pub const FRAME_D3D11_444: c_int = 13;
/// 10-bit 4:4:4 in three 16-bit planes with the samples in the high bits of each word, as P010's:
/// NVDEC's 10-bit 4:4:4 (`YUV444P16`) copied to memory. The plane upload moves them to the low
/// bits (`stream::planes_gpu`), so it draws as `FRAME_YUV444P10`. Linux only.
pub const FRAME_YUV444P16: c_int = 14;

/// Whether `info` is a `FRAME_VULKAN_INTERLEAVED444` picture.
pub fn interleaved_444(info: &FrameInfo) -> bool {
    // Linux and macOS (unix): Vulkan interleaved 4:4:4 frame kind check.
    #[cfg(unix)]
    return info.kind == FRAME_VULKAN_INTERLEAVED444;
    // Non-Unix platforms (Windows): Vulkan interleaved 4:4:4 is unused.
    #[cfg(not(unix))]
    {
        let _ = info;
        false
    }
}

/// The shape of a planar picture's textures: R8 planes, or R16 for 10-bit, with half-size chroma
/// (rounded up, as FFmpeg's) unless the picture is 4:4:4.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlanarLayout {
    pub width: u32,
    pub height: u32,
    pub chroma444: bool,
    pub ten_bit: bool,
}

impl PlanarLayout {
    /// The layout of a `FRAME_YUV444P`, `FRAME_YUV420P10` or `FRAME_YUV444P10` picture, and on
    /// Linux of a `FRAME_YUV444P16` one (its high-bit samples moved to the low bits on upload).
    pub fn of(info: &FrameInfo) -> Option<Self> {
        let (chroma444, ten_bit) = match info.kind {
            FRAME_YUV444P => (true, false),
            FRAME_YUV420P10 => (false, true),
            FRAME_YUV444P10 => (true, true),
            // Linux and macOS (unix): NVDEC high-bit 10-bit 4:4:4 planar kind.
            #[cfg(unix)]
            FRAME_YUV444P16 => (true, true),
            _ => return None,
        };
        (info.width > 0 && info.height > 0).then_some(Self { width: info.width as u32, height: info.height as u32, chroma444, ten_bit })
    }

    /// The layout of a `FRAME_D3D11_444` picture as the plane shader's surface entry reads its
    /// copy: one texel holding Y, Cb and Cr, 8-bit (AYUV) or 10-bit (Y410, Y416).
    pub fn of_d3d11(info: &FrameInfo) -> Option<Self> {
        (info.kind == FRAME_D3D11_444 && info.width > 0 && info.height > 0)
            .then_some(Self { width: info.width as u32, height: info.height as u32, chroma444: true, ten_bit: info.ten_bit != 0 })
    }

    /// The 4:2:0 layout an NV12 or P010 picture in memory is drawn in by the plane shader: the
    /// upload splits its interleaved chroma into two planes and moves P010's high-bit samples to
    /// the low bits (`planes_gpu`), so it reads as `FRAME_YUV420P` or `FRAME_YUV420P10`.
    // Linux and macOS (unix): semi-planar CPU-memory upload layout.
    #[cfg(unix)]
    pub fn of_semi_planar(info: &FrameInfo) -> Option<Self> {
        let ten_bit = match info.kind {
            FRAME_NV12 => false,
            FRAME_P010 => true,
            _ => return None,
        };
        (info.width > 0 && info.height > 0).then_some(Self { width: info.width as u32, height: info.height as u32, chroma444: false, ten_bit })
    }

    /// The layout of a `FRAME_VULKAN`, `FRAME_VULKAN444` or `FRAME_VULKAN_INTERLEAVED444`
    /// picture's planes as the plane shader reads them once copied (`vulkan_video::Planes`), its
    /// chroma interleaved.
    // Linux and macOS (unix): Vulkan picture layout for GPU plane shader.
    #[cfg(unix)]
    pub fn of_vulkan(info: &FrameInfo) -> Option<Self> {
        let chroma444 = match info.kind {
            FRAME_VULKAN => false,
            FRAME_VULKAN444 | FRAME_VULKAN_INTERLEAVED444 => true,
            _ => return None,
        };
        (info.width > 0 && info.height > 0).then_some(Self { width: info.width as u32, height: info.height as u32, chroma444, ten_bit: info.ten_bit != 0 })
    }

    // Non-Unix platforms (Windows): Vulkan picture layout is unused.
    #[cfg(not(unix))]
    pub fn of_vulkan(_: &FrameInfo) -> Option<Self> {
        None
    }

    pub fn plane_size(&self, plane: usize) -> (u32, u32) {
        if plane == 0 || self.chroma444 { (self.width, self.height) } else { (self.width.div_ceil(2), self.height.div_ceil(2)) }
    }
}

pub(crate) fn error(code: i32) -> String {
    let mut buffer = [0 as c_char; 128];
    unsafe {
        av_strerror(code, buffer.as_mut_ptr(), buffer.len());
        CStr::from_ptr(buffer.as_ptr())
            .to_string_lossy()
            .into_owned()
    }
}

#[cfg(windows)]
pub fn swapchain_allows_tearing(swapchain: *mut c_void) -> bool {
    use windows::core::Interface;
    use windows::Win32::Graphics::Dxgi::{IDXGISwapChain1, DXGI_SWAP_CHAIN_FLAG_ALLOW_TEARING};
    unsafe {
        IDXGISwapChain1::from_raw_borrowed(&swapchain)
            .and_then(|chain| chain.GetDesc1().ok())
            .is_some_and(|desc| desc.Flags & DXGI_SWAP_CHAIN_FLAG_ALLOW_TEARING.0 as u32 != 0)
    }
}

/// The codes of `tests/fixtures/hevc-444-10.bin`, a 256x256 HEVC RExt 4:4:4 10-bit picture
/// (limited range, no colour description) of 4x4 flat 64x64 blocks: block i (row * 4 + column)
/// has Y 65 + 53i, Cb 66 + 57 (5i mod 16) and Cr 67 + 59 (11i mod 16). libx265 (main444-10,
/// level-idc=2.1, crf=4, lossless=0, bframes=0, ref=1) encoded it from these codes so hardware
/// HEVC RExt decoders accept it without lossless transquant bypass or Level 8.5.
#[cfg(test)]
pub(crate) fn hevc_444_10_fixture_code(plane: usize, x: usize, y: usize) -> u16 {
    let i = (y / 64 * 4 + x / 64) as u16;
    [65 + 53 * i, 66 + 57 * ((5 * i) % 16), 67 + 59 * ((11 * i) % 16)][plane]
}

#[cfg(test)]
mod tests;
