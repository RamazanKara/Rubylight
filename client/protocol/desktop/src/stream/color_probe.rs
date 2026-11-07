//! The decoder check behind "Prefer YUV 4:4:4" and "10-bit color (SDR)": decodes a fixture of each
//! 4:4:4 and 10-bit format with the decoder the stream will use and checks that the picture comes
//! out in a shape the presenter draws. Its result is `color::offer`'s `decodable` and reason.
//!
//! A hardware decoder is opened as required hardware: FFmpeg would otherwise decode a profile its
//! hardware lacks in software without a word, and the stream would not use the decoder it names.

use nightjar_core::sdp::*;
use sdl3_sys::everything::*;

use crate::media;

/// The fixtures: 64x64 lossless 4:4:4 pictures and 128x128 Main10 ones.
const FIXTURES: [(u32, media::Codec, &str, &[u8]); 5] = [
    (
        VIDEO_FORMAT_H264_HIGH8_444,
        media::Codec::H264,
        "H.264 4:4:4",
        include_bytes!("../../tests/fixtures/h264-444-8.bin"),
    ),
    (
        VIDEO_FORMAT_H265_REXT8_444,
        media::Codec::Hevc,
        "HEVC 4:4:4",
        include_bytes!("../../tests/fixtures/hevc-444-8.bin"),
    ),
    (
        VIDEO_FORMAT_AV1_HIGH8_444,
        media::Codec::Av1,
        "AV1 4:4:4",
        include_bytes!("../../tests/fixtures/av1-444-8.bin"),
    ),
    (
        VIDEO_FORMAT_H265_MAIN10,
        media::Codec::Hevc,
        "HEVC Main10",
        include_bytes!("../../../../app/src/main/assets/packed444/hevc-10.bin"),
    ),
    (
        VIDEO_FORMAT_AV1_MAIN10,
        media::Codec::Av1,
        "AV1 Main10",
        include_bytes!("../../../../app/src/main/assets/packed444/av1-10.bin"),
    ),
];

/// The plain HEVC and AV1 formats, and the Main10 ones HDR offers, whose hardware decoding
/// [`probe_base`] checks before they are offered: 128x128 4:2:0 pictures.
const BASE_FIXTURES: [(u32, media::Codec, &str, &[u8]); 4] = [
    (
        VIDEO_FORMAT_H265,
        media::Codec::Hevc,
        "HEVC",
        include_bytes!("../../../../app/src/main/assets/packed444/hevc-8.bin"),
    ),
    (
        VIDEO_FORMAT_H265_MAIN10,
        media::Codec::Hevc,
        "HEVC Main10",
        include_bytes!("../../../../app/src/main/assets/packed444/hevc-10.bin"),
    ),
    (
        VIDEO_FORMAT_AV1_MAIN8,
        media::Codec::Av1,
        "AV1",
        include_bytes!("../../../../app/src/main/assets/packed444/av1-8.bin"),
    ),
    (
        VIDEO_FORMAT_AV1_MAIN10,
        media::Codec::Av1,
        "AV1 Main10",
        include_bytes!("../../../../app/src/main/assets/packed444/av1-10.bin"),
    ),
];

/// The formats [`probe_base`] checks. H.264 is always offered.
pub const BASE_FORMATS: u32 =
    VIDEO_FORMAT_H265 | VIDEO_FORMAT_H265_MAIN10 | VIDEO_FORMAT_AV1_MAIN8 | VIDEO_FORMAT_AV1_MAIN10;

/// What [`probe_base`] found: the formats of [`BASE_FORMATS`] the stream's decoder decodes, why
/// the others are not offered, and the backend that decoded each (what reference frame
/// invalidation is asked for by, `connect::video_capabilities`).
#[derive(Clone, Debug, PartialEq)]
pub struct Base {
    pub decodable: u32,
    pub reason: String,
    pub backends: Vec<(u32, media::Hardware)>,
}

impl Base {
    /// Software decoding: every format, by the software decoder.
    pub fn software() -> Self {
        Self {
            decodable: BASE_FORMATS,
            reason: String::new(),
            backends: BASE_FIXTURES
                .iter()
                .map(|&(format, ..)| (format, media::Hardware::None))
                .collect(),
        }
    }
}

/// The formats of [`BASE_FORMATS`] that `hardware` decodes in hardware. Each is decoded with the
/// stream's decoder order as required hardware: without the check a format the GPU lacks (HEVC
/// Main10 or AV1 on an older one) would be offered, and FFmpeg would decode it in software, or
/// for AV1 not at all. Software decoding decodes them all and is not checked.
pub fn probe_base(hardware: media::Hardware) -> Base {
    if hardware == media::Hardware::None {
        return Base::software();
    }
    let mut base = Base {
        decodable: 0,
        reason: String::new(),
        backends: Vec::new(),
    };
    let mut failures = Vec::new();
    for (format, codec, name, data) in BASE_FIXTURES {
        let pictures = media::Pictures {
            ten_bit: format & VIDEO_FORMAT_MASK_10BIT != 0,
            chroma444: false,
            packed: false,
            waiting: 0,
        };
        match decode(hardware, pictures, codec, data) {
            Ok((_, backend)) => {
                base.decodable |= format;
                base.backends.push((format, backend));
            }
            Err(reason) => {
                eprintln!("nightjar: {name} is not offered: no hardware decoder decodes it here: {reason}");
                failures.push(format!("{name}: {reason}"));
            }
        }
    }
    base.reason = failures.join("; ");
    base
}

/// The formats of `color::YUV444_FORMATS` and `color::SDR10_FORMATS` that `hardware` decodes into
/// pictures the presenter draws, and why the others are not ("" when all are). `planes` says
/// whether planar pictures (R16 planes when its argument is true) can be drawn.
pub fn probe(
    hardware: media::Hardware,
    planes: impl Fn(bool) -> Result<(), String>,
) -> (u32, String) {
    let mut decodable = 0;
    let mut failures = Vec::new();
    for (format, codec, name, data) in FIXTURES {
        let yuv444 = format & VIDEO_FORMAT_MASK_YUV444 != 0;
        let pictures = media::Pictures {
            ten_bit: format & VIDEO_FORMAT_MASK_10BIT != 0,
            chroma444: yuv444,
            packed: false,
            waiting: 0,
        };
        match decode(hardware, pictures, codec, data)
            .and_then(|(info, _)| drawable(&info, yuv444, &planes))
        {
            Ok(()) => decodable |= format,
            Err(reason) => {
                eprintln!("nightjar: {name} is not offered: {reason}");
                failures.push(format!("{name}: {reason}"));
            }
        }
    }
    (decodable, failures.join("; "))
}

fn decode(
    hardware: media::Hardware,
    pictures: media::Pictures,
    codec: media::Codec,
    data: &[u8],
) -> Result<(media::FrameInfo, media::Hardware), String> {
    // The stream's decoder order; the error names each backend that did not open.
    let mut decoder = match hardware {
        media::Hardware::None => media::VideoDecoder::open(codec, hardware, 1, false)?,
        hardware => media::VideoDecoder::open_first(codec, pictures, hardware, true, |_, _| {})?,
    };
    let status = decoder.send(data, 0);
    if status < 0 {
        return Err(format!("the decoder rejected the fixture ({status})"));
    }
    // An asynchronous decoder (V4L2 M2M) says "no picture yet" while the driver decodes: asked
    // again for up to a quarter second.
    let mut received = decoder.receive();
    let tries = if decoder.asynchronous() { 125 } else { 0 };
    for _ in 0..tries {
        if !matches!(received, media::Received::None) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
        received = decoder.receive();
    }
    match received {
        media::Received::Frame(frame) => Ok((frame.info(), decoder.hardware)),
        media::Received::None => Err("the decoder made no picture of the fixture".into()),
        media::Received::Error => Err("the decoder failed on the fixture".into()),
    }
}

/// Whether a decoded fixture picture is one the presenter draws as what was asked for.
fn drawable(
    info: &media::FrameInfo,
    yuv444: bool,
    planes: &impl Fn(bool) -> Result<(), String>,
) -> Result<(), String> {
    // A Vulkan video picture is drawn with the plane shader too, from its copied planes.
    let vulkan = media::PlanarLayout::of_vulkan(info);
    let planar = media::PlanarLayout::of(info).or(vulkan);
    if yuv444 && !planar.is_some_and(|layout| layout.chroma444) {
        return Err(format!(
            "the decoder's pictures (kind {}) are not 4:4:4 pictures this app draws",
            info.kind
        ));
    }
    if !yuv444
        && (info.ten_bit == 0
            || !(vulkan.is_some()
                || matches!(
                    info.kind,
                    media::FRAME_P010
                        | media::FRAME_D3D11
                        | media::FRAME_YUV420P10
                        | media::FRAME_YUV444P10
                )))
    {
        return Err(format!(
            "the decoder's pictures (kind {}) are not 10-bit pictures this app draws",
            info.kind
        ));
    }
    match planar {
        Some(layout) => planes(layout.ten_bit),
        None => Ok(()),
    }
}

/// Whether `renderer` draws planar pictures: the plane shader pass of its renderer.
pub fn planes_drawable(renderer: *mut SDL_Renderer, ten_bit: bool) -> Result<(), String> {
    #[cfg(windows)]
    {
        let _ = ten_bit;
        if crate::sdl::renderer_pointer(renderer, SDL_PROP_RENDERER_D3D11_DEVICE_POINTER).is_null()
        {
            return Err("drawing these pictures needs the Direct3D 11 renderer".into());
        }
        super::d3d11::PlanesPass::compiles()
    }
    #[cfg(unix)]
    {
        let device = crate::sdl::renderer_pointer(renderer, SDL_PROP_RENDERER_GPU_DEVICE_POINTER);
        if device.is_null() {
            return Err("drawing these pictures needs the SDL GPU renderer".into());
        }
        // SAFETY: The renderer's live GPU device, on the renderer's thread.
        unsafe { super::planes_gpu::PlanesPass::compiles(device.cast(), ten_bit) }
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = (renderer, ten_bit);
        Err("this platform has no plane shader pass".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nightjar_core::color::{SDR10_FORMATS, YUV444_FORMATS};

    #[test]
    fn software_decoding_offers_every_4_4_4_and_10_bit_format_when_planes_draw() {
        let (decodable, reason) = probe(media::Hardware::None, |_| Ok(()));
        assert_eq!(decodable, YUV444_FORMATS | SDR10_FORMATS, "{reason}");
        assert_eq!(reason, "");
    }

    #[test]
    fn without_a_plane_pass_nothing_planar_is_offered_and_the_reason_says_why() {
        // Software Main10 pictures are planar too, so 10-bit goes with 4:4:4.
        let (decodable, reason) = probe(media::Hardware::None, |ten| {
            Err(format!("no R{} planes", if ten { 16 } else { 8 }))
        });
        assert_eq!(decodable, 0);
        assert!(
            reason.contains("HEVC 4:4:4: no R8 planes")
                && reason.contains("AV1 Main10: no R16 planes"),
            "{reason}"
        );
        // 8-bit planes alone: 4:4:4 is offered, 10-bit is not.
        let (decodable, _) = probe(media::Hardware::None, |ten| {
            if ten {
                Err("no R16 planes".into())
            } else {
                Ok(())
            }
        });
        assert_eq!(decodable, YUV444_FORMATS);
    }

    #[test]
    fn pictures_that_lost_their_chroma_or_depth_are_not_drawable() {
        let info = |kind, ten_bit| media::FrameInfo {
            width: 64,
            height: 64,
            kind,
            ten_bit,
            pts: 0,
            data: [std::ptr::null_mut(); 3],
            linesize: [0; 3],
            color_space: 1,
            color_range: 1,
            color_trc: 1,
        };
        let planes = |_| Ok(());
        // A hardware decoder's 4:4:4 surface (AYUV, Y410) is kind 4, which nothing draws.
        assert!(drawable(&info(4, 0), true, &planes)
            .unwrap_err()
            .contains("not 4:4:4"));
        assert!(drawable(&info(media::FRAME_D3D11, 0), true, &planes).is_err());
        assert!(drawable(&info(media::FRAME_YUV420P10, 1), true, &planes).is_err());
        assert!(drawable(&info(media::FRAME_YUV444P, 0), true, &planes).is_ok());
        assert!(drawable(&info(media::FRAME_YUV444P10, 1), true, &planes).is_ok());
        // 10-bit: a hardware P010 surface draws through SDL, a software one through the planes.
        assert!(drawable(&info(media::FRAME_D3D11, 1), false, &planes).is_ok());
        assert!(drawable(&info(media::FRAME_P010, 1), false, &planes).is_ok());
        assert!(drawable(&info(media::FRAME_YUV420P10, 1), false, &planes).is_ok());
        assert!(drawable(&info(media::FRAME_D3D11, 0), false, &planes)
            .unwrap_err()
            .contains("not 10-bit"));
        assert!(drawable(&info(media::FRAME_NV12, 0), false, &planes).is_err());
        assert!(drawable(&info(media::FRAME_YUV420P10, 1), false, &|_| Err(
            "no planes".into()
        ))
        .is_err());
    }
}
