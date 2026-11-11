//! The decoder check behind "Prefer YUV 4:4:4" and "10-bit color (SDR)": decodes a fixture of each
//! 4:4:4 and 10-bit format with the decoder the stream will use and checks that the picture comes
//! out in a shape the presenter draws. Its result is `color::offer`'s `decodable` and reason.
//!
//! A hardware decoder is opened as required hardware: FFmpeg would otherwise decode a profile its
//! hardware lacks in software without a word, and the stream would not use the decoder it names.

use nightjar_core::sdp::*;
use sdl3_sys::everything::*;

use crate::media;

/// The fixtures: 256x256 lossless 4:4:4 pictures (at least NVDEC's minimum size, so NVIDIA is
/// offered 4:4:4) and 256x256 Main10 ones. The 10-bit 4:4:4 one is 16 flat blocks of known codes
/// (`media`'s software test lists them): lossless 10-bit noise would be over 140 KiB.
const FIXTURES: [(u32, media::Codec, &str, &[u8]); 6] = [
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
        include_bytes!("../../tests/fixtures/hevc-420-10.bin"),
    ),
    (
        VIDEO_FORMAT_AV1_MAIN10,
        media::Codec::Av1,
        "AV1 Main10",
        include_bytes!("../../tests/fixtures/av1-420-10.bin"),
    ),
    (
        VIDEO_FORMAT_H265_REXT10_444,
        media::Codec::Hevc,
        "HEVC 4:4:4 10-bit",
        include_bytes!("../../tests/fixtures/hevc-444-10.bin"),
    ),
];

/// The plain HEVC and AV1 formats, and the Main10 ones HDR offers, whose hardware decoding
/// [`probe_base`] checks before they are offered: single 256x256 4:2:0 key frames, above every
/// backend's smallest picture (NVDEC's HEVC minimum is 144x144, which 128x128 ones were under).
const BASE_FIXTURES: [(u32, media::Codec, &str, &[u8]); 4] = [
    (
        VIDEO_FORMAT_H265,
        media::Codec::Hevc,
        "HEVC",
        include_bytes!("../../tests/fixtures/hevc-420-8.bin"),
    ),
    (
        VIDEO_FORMAT_H265_MAIN10,
        media::Codec::Hevc,
        "HEVC Main10",
        include_bytes!("../../tests/fixtures/hevc-420-10.bin"),
    ),
    (
        VIDEO_FORMAT_AV1_MAIN8,
        media::Codec::Av1,
        "AV1",
        include_bytes!("../../tests/fixtures/av1-420-8.bin"),
    ),
    (
        VIDEO_FORMAT_AV1_MAIN10,
        media::Codec::Av1,
        "AV1 Main10",
        include_bytes!("../../tests/fixtures/av1-420-10.bin"),
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
/// for AV1 not at all. Software decoding decodes them all and is not checked. Only the formats of
/// `wanted` are decoded (`negotiate::base_probe_formats`): the others are not offered anyway.
pub fn probe_base(hardware: media::Hardware, wanted: u32) -> Base {
    if hardware == media::Hardware::None {
        return Base::software();
    }
    let mut base = Base {
        decodable: 0,
        reason: String::new(),
        backends: Vec::new(),
    };
    let mut failures = Vec::new();
    // VAAPI on the GPU the stream will decode on.
    let vaapi_node = media::vaapi_node(hardware);
    for (format, codec, name, data) in BASE_FIXTURES {
        if format & wanted == 0 {
            continue;
        }
        let pictures = media::Pictures {
            ten_bit: format & VIDEO_FORMAT_MASK_10BIT != 0,
            chroma444: false,
            packed: false,
            waiting: 0,
            vaapi_node,
        };
        match decode(hardware, pictures, codec, data) {
            Ok((_, backend)) => {
                eprintln!("nightjar: decoder probe: {name} {} ok", backend.label());
                base.decodable |= format;
                base.backends.push((format, backend));
            }
            Err(reason) => {
                eprintln!("nightjar: decoder probe: {name} failed, so it is not offered: no hardware decoder decodes it here: {reason}");
                failures.push(format!("{name}: {reason}"));
            }
        }
    }
    base.reason = failures.join("; ");
    base
}

/// The formats of `color::YUV444_FORMATS`, `color::SDR10_FORMATS` and `color::YUV444_10_FORMATS`
/// that `hardware` decodes into pictures the presenter draws, and why the others are not ("" when
/// all are). `planes` says whether planar pictures (R16 planes when its argument is true) can be
/// drawn. Only the formats of `wanted` are decoded (`negotiate::color_probe_formats`): the others are not offered anyway.
pub fn probe(
    hardware: media::Hardware,
    wanted: u32,
    planes: impl Fn(bool) -> Result<(), String>,
) -> (u32, String) {
    let mut decodable = 0;
    let mut failures = Vec::new();
    // VAAPI on the GPU the stream will decode on.
    let vaapi_node = media::vaapi_node(hardware);
    for (format, codec, name, data) in FIXTURES {
        if format & wanted == 0 {
            continue;
        }
        let yuv444 = format & VIDEO_FORMAT_MASK_YUV444 != 0;
        let pictures = media::Pictures {
            ten_bit: format & VIDEO_FORMAT_MASK_10BIT != 0,
            chroma444: yuv444,
            packed: false,
            waiting: 0,
            vaapi_node,
        };
        match decode(hardware, pictures, codec, data)
            .and_then(|(info, backend)| {
                drawable(&info, yuv444, &planes)?;
                keeps_depth(&info, pictures.ten_bit)?;
                Ok(backend)
            })
        {
            Ok(backend) => {
                eprintln!("nightjar: decoder probe: {name} {} ok", backend.label());
                decodable |= format;
            }
            Err(reason) => {
                eprintln!("nightjar: decoder probe: {name} failed, so it is not offered: {reason}");
                failures.push(format!("{name}: {reason}"));
            }
        }
    }
    #[cfg(windows)]
    if let Some(hint) = other_gpu_hint(hardware, wanted & !decodable) {
        failures.push(hint);
    }
    (decodable, failures.join("; "))
}

/// When HEVC 4:4:4 (of `failed`) does not decode on the renderer's GPU, D3D11VA's, but does on
/// another GPU of this PC (a laptop drawing on its integrated GPU): how to move Nightjar there.
#[cfg(windows)]
fn other_gpu_hint(hardware: media::Hardware, failed: u32) -> Option<String> {
    let media::Hardware::D3d11(device) = hardware else { return None };
    let ten_bit = if failed & VIDEO_FORMAT_H265_REXT8_444 != 0 {
        false
    } else if failed & VIDEO_FORMAT_H265_REXT10_444 != 0 {
        true
    } else {
        return None;
    };
    let depth = if ten_bit { " 10-bit" } else { "" };
    match crate::gpu_adapters::others_decoding_hevc444(device, ten_bit)? {
        (renderer, crate::gpu_adapters::Hevc444Gpus::Renderer) => {
            // Moving to another GPU would not help: the decoder itself lacks the profile.
            eprintln!("nightjar: decoder probe ran on {renderer}, whose DXVA lists HEVC 4:4:4{depth}, so this build's decoder failed, not the GPU choice");
            None
        }
        (renderer, crate::gpu_adapters::Hevc444Gpus::Others(others)) => {
            eprintln!("nightjar: decoder probe ran on {renderer}; other GPUs that decode HEVC 4:4:4{depth}: {others:?}");
            others.first().map(|other| crate::gpu_adapters::hint(&renderer, other))
        }
    }
}

fn decode(
    hardware: media::Hardware,
    pictures: media::Pictures,
    codec: media::Codec,
    data: &[u8],
) -> Result<(media::FrameInfo, media::Hardware), String> {
    if hardware == media::Hardware::None {
        let mut decoder = media::VideoDecoder::open(codec, hardware, 1, false)?;
        return decode_with(&mut decoder, data).map(|info| (info, hardware));
    }
    // The stream's decoder order, as the stream goes on after a backend that fails mid-stream.
    first_that_decodes(hardware.order(), |order| {
        let mut decoder = media::VideoDecoder::open_any(codec, pictures, order, true, |_, _| {})?;
        // VAAPI's 4:4:4 picture is one interleaved surface the presenter draws only mapped into
        // the GPU renderer's Vulkan images, as the stream maps it.
        if pictures.chroma444 && decoder.hardware == media::Hardware::Vaapi {
            super::pipeline::map_to_vulkan(hardware, &mut decoder);
        }
        Ok((decoder.hardware, decode_with(&mut decoder, data)))
    })
}

/// The first backend of `order` that decodes: `attempt` opens the first of the backends it is
/// given that opens (the error names each that did not) and decodes with it. A backend that opens
/// but fails on the fixture has the ones after it tried, not just the first that opens.
fn first_that_decodes<T>(
    mut order: Vec<media::Hardware>,
    mut attempt: impl FnMut(
        Vec<media::Hardware>,
    ) -> Result<(media::Hardware, Result<T, String>), String>,
) -> Result<(T, media::Hardware), String> {
    let mut failures = Vec::new();
    loop {
        match attempt(order.clone()) {
            Ok((backend, Ok(decoded))) => return Ok((decoded, backend)),
            Ok((backend, Err(reason))) => {
                failures.push(format!("{}: {reason}", backend.label()));
                order = super::pipeline::backends_after(order, backend);
                if order.is_empty() {
                    break;
                }
            }
            Err(reason) => {
                failures.push(reason);
                break;
            }
        }
    }
    Err(failures.join("; "))
}

/// Decodes the fixture `data` with `decoder`, required to stay its backend.
fn decode_with(decoder: &mut media::VideoDecoder, data: &[u8]) -> Result<media::FrameInfo, String> {
    let status = decoder.send(data, 0);
    if status < 0 {
        return Err(format!("the decoder rejected the fixture ({status})"));
    }
    // An asynchronous decoder (V4L2 M2M) holds a lone frame until it is told the input ended,
    // then says "no picture yet" while the driver decodes: asked again for up to a quarter second.
    let tries = if decoder.asynchronous() {
        let status = decoder.drain();
        if status < 0 {
            return Err(format!("the decoder refused the end of input ({status})"));
        }
        125
    } else {
        0
    };
    let mut received = decoder.receive();
    for _ in 0..tries {
        if !matches!(received, media::Received::None) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
        received = decoder.receive();
    }
    match received {
        media::Received::Frame(_) if decoder.refused() => {
            Err("the picture came out in memory, not the hardware decoder's".into())
        }
        media::Received::Frame(frame) => Ok(frame.info()),
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
    // A Vulkan video picture is drawn with the plane shader too, from its copied planes, and a
    // D3D11VA 4:4:4 surface (AYUV, Y410, Y416) from its copy.
    let vulkan = media::PlanarLayout::of_vulkan(info);
    let planar = media::PlanarLayout::of(info).or(vulkan).or_else(|| media::PlanarLayout::of_d3d11(info));
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
                        | media::FRAME_VIDEOTOOLBOX
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

/// Whether a decoded fixture picture kept the depth asked for: a 10-bit 4:4:4 format decoded into
/// 8-bit pictures would be drawn without its two low bits.
fn keeps_depth(info: &media::FrameInfo, ten_bit: bool) -> Result<(), String> {
    if ten_bit && info.ten_bit == 0 {
        return Err(format!("the decoder's pictures (kind {}) are 8-bit, not the 10-bit pictures asked for", info.kind));
    }
    Ok(())
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
        let (decodable, reason) = probe(media::Hardware::None, YUV444_FORMATS | SDR10_FORMATS, |_| Ok(()));
        assert_eq!(decodable, YUV444_FORMATS | SDR10_FORMATS, "{reason}");
        assert_eq!(reason, "");
    }

    #[test]
    fn without_a_plane_pass_nothing_planar_is_offered_and_the_reason_says_why() {
        // Software Main10 pictures are planar too, so 10-bit goes with 4:4:4.
        let (decodable, reason) = probe(media::Hardware::None, YUV444_FORMATS | SDR10_FORMATS, |ten| {
            Err(format!("no R{} planes", if ten { 16 } else { 8 }))
        });
        assert_eq!(decodable, 0);
        assert!(
            reason.contains("HEVC 4:4:4: no R8 planes")
                && reason.contains("AV1 Main10: no R16 planes"),
            "{reason}"
        );
        // 8-bit planes alone: 4:4:4 is offered, 10-bit is not.
        let (decodable, _) = probe(media::Hardware::None, YUV444_FORMATS | SDR10_FORMATS, |ten| {
            if ten {
                Err("no R16 planes".into())
            } else {
                Ok(())
            }
        });
        assert_eq!(decodable, YUV444_FORMATS);
    }

    /// 10-bit launches decoded the 4:4:4 fixtures too (about 12 ms) with 4:4:4 off.
    #[test]
    fn only_the_wanted_formats_are_decoded() {
        let (decodable, reason) = probe(media::Hardware::None, SDR10_FORMATS, |_| Err("refused".into()));
        assert_eq!(decodable, 0);
        assert!(reason.contains("HEVC Main10") && !reason.contains("4:4:4"), "{reason}");
        assert_eq!(probe(media::Hardware::None, 0, |_| Err("refused".into())), (0, String::new()));
    }

    /// The 128x128 fixtures were under NVDEC's smallest HEVC picture (144x144), so the probe
    /// refused HEVC on every NVIDIA GPU; the 64x64 4:4:4 ones never offered NVIDIA 4:4:4. The
    /// 4:4:4 ones are lossless (`media`'s software test compares every sample), so larger.
    #[test]
    fn the_fixtures_are_above_every_backends_smallest_picture_and_stay_small() {
        for &(format, codec, name, data) in BASE_FIXTURES.iter().chain(&FIXTURES) {
            let (info, _) = decode(media::Hardware::None, media::Pictures::default(), codec, data)
                .unwrap_or_else(|error| panic!("{name}: {error}"));
            assert!(info.width >= 256 && info.height >= 256, "{name}: {}x{}", info.width, info.height);
            let limit = if format & VIDEO_FORMAT_MASK_YUV444 == 0 { 8 } else { 80 };
            assert!(data.len() <= limit * 1024, "{name}: {} bytes", data.len());
        }
    }

    /// The probe stopped at the first backend that opened, so a backend that opens but cannot
    /// decode the format (Vulkan video without the profile) hid the ones after it.
    #[test]
    fn a_backend_that_opens_but_fails_has_the_ones_after_it_tried() {
        use media::Hardware::{Cuda, V4l2m2m, Vaapi, Vulkan};
        let gpu = std::ptr::dangling_mut::<std::ffi::c_void>();
        let mut asked = Vec::new();
        let decoded = first_that_decodes(vec![Vulkan(gpu), Vaapi, V4l2m2m, Cuda], |order| {
            asked.push(order.clone());
            match order[0] {
                Vulkan(_) => Ok((order[0], Err("no picture".to_owned()))),
                // VAAPI does not open, so V4L2 M2M is the first that does.
                Vaapi => Ok((V4l2m2m, Ok(7))),
                _ => unreachable!(),
            }
        });
        assert_eq!(decoded, Ok((7, V4l2m2m)));
        assert_eq!(asked, [vec![Vulkan(gpu), Vaapi, V4l2m2m, Cuda], vec![Vaapi, V4l2m2m, Cuda]]);
        // Every backend failing, or none opening, names each reason.
        let failed = first_that_decodes(vec![Vaapi, Cuda], |order| {
            Ok::<_, String>((order[0], Err::<(), _>(format!("{} failed", order[0].label()))))
        });
        let reason = failed.unwrap_err();
        assert!(reason.contains("VAAPI failed") && reason.contains("NVDEC failed"), "{reason}");
        let unopened = first_that_decodes(vec![Vaapi], |_| Err::<(_, Result<(), _>), _>("nothing opens".to_owned()));
        assert_eq!(unopened, Err("nothing opens".to_owned()));
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
        // Kind 4 is a picture nothing draws.
        assert!(drawable(&info(4, 0), true, &planes)
            .unwrap_err()
            .contains("not 4:4:4"));
        assert!(drawable(&info(media::FRAME_D3D11, 0), true, &planes).is_err());
        // A D3D11VA 4:4:4 surface (AYUV, Y410, Y416) is drawn from its copy by the plane pass.
        for ten_bit in [0, 1] {
            assert!(drawable(&info(media::FRAME_D3D11_444, ten_bit), true, &planes).is_ok());
            assert!(drawable(&info(media::FRAME_D3D11_444, ten_bit), true, &|_| Err("no pass".into())).is_err());
        }
        // NVDEC's 10-bit 4:4:4 in memory, its samples in the high bits, goes through the planes.
        #[cfg(unix)]
        assert!(drawable(&info(media::FRAME_YUV444P16, 1), true, &planes).is_ok());
        assert!(drawable(&info(media::FRAME_YUV420P10, 1), true, &planes).is_err());
        assert!(drawable(&info(media::FRAME_YUV444P, 0), true, &planes).is_ok());
        assert!(drawable(&info(media::FRAME_YUV444P10, 1), true, &planes).is_ok());
        // ...unless it was mapped into Vulkan as one image the planes copy whole.
        #[cfg(unix)]
        for ten_bit in [0, 1] {
            assert!(drawable(&info(media::FRAME_VULKAN_INTERLEAVED444, ten_bit), true, &planes).is_ok());
        }
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
        // 10-bit 4:4:4 decoded into 8-bit pictures lost its depth.
        assert!(keeps_depth(&info(media::FRAME_D3D11_444, 0), true).unwrap_err().contains("8-bit"));
        assert!(keeps_depth(&info(media::FRAME_YUV444P, 0), true).is_err());
        assert!(keeps_depth(&info(media::FRAME_D3D11_444, 1), true).is_ok());
        assert!(keeps_depth(&info(media::FRAME_D3D11_444, 0), false).is_ok());
    }

    /// The fixtures through D3D11VA on this PC's GPU: plain HEVC and Main10 decode wherever the GPU
    /// has HEVC, and HEVC 4:4:4 decodes exactly where the GPU's DXVA lists the Range Extension
    /// profile. Rubylight issue 84: FFmpeg 8 has no DXVA HEVC 4:4:4, so an RTX 5060 that lists the
    /// profile refused every 4:4:4 stream until the FFmpeg patch (`ffmpeg-patches/windows`).
    #[cfg(windows)]
    #[test]
    fn d3d11va_decodes_hevc_4_4_4_where_the_gpu_lists_it() {
        use windows::core::Interface;
        use windows::Win32::Foundation::HMODULE;
        use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
        use windows::Win32::Graphics::Direct3D11::*;
        let mut device = None;
        // SAFETY: A device of the test's own, kept alive past every use of its pointer.
        let made = unsafe {
            D3D11CreateDevice(None, D3D_DRIVER_TYPE_HARDWARE, HMODULE::default(), D3D11_CREATE_DEVICE_VIDEO_SUPPORT, None,
                D3D11_SDK_VERSION, Some(&mut device), None, None)
        };
        let Some(device) = device.filter(|_| made.is_ok()) else {
            eprintln!("no hardware Direct3D 11 device here");
            return;
        };
        let hardware = media::Hardware::D3d11(device.as_raw());
        let base = probe_base(hardware, VIDEO_FORMAT_H265 | VIDEO_FORMAT_H265_MAIN10);
        assert_eq!(base.decodable, VIDEO_FORMAT_H265 | VIDEO_FORMAT_H265_MAIN10, "{}", base.reason);
        for (format, ten_bit) in [(VIDEO_FORMAT_H265_REXT8_444, false), (VIDEO_FORMAT_H265_REXT10_444, true)] {
            let lists = matches!(
                crate::gpu_adapters::others_decoding_hevc444(device.as_raw(), ten_bit),
                Some((_, crate::gpu_adapters::Hevc444Gpus::Renderer))
            );
            let (decodable, reason) = probe(hardware, format, |_| Ok(()));
            eprintln!("HEVC 4:4:4 (10-bit {ten_bit}): DXVA lists it {lists}, decodes {}: {reason}", decodable != 0);
            assert_eq!(decodable != 0, lists, "{reason}");
        }
    }

    /// Both settings on ask for HEVC RExt 4:4:4 10-bit: the software decoder makes 10-bit 4:4:4
    /// planar pictures of the fixture, offered where the R16 planes draw.
    #[test]
    fn ten_bit_4_4_4_is_offered_where_its_pictures_draw() {
        use nightjar_core::color::YUV444_10_FORMATS;
        let (decodable, reason) = probe(media::Hardware::None, YUV444_10_FORMATS, |_| Ok(()));
        assert_eq!((decodable, reason.as_str()), (VIDEO_FORMAT_H265_REXT10_444, ""));
        let (decodable, reason) = probe(media::Hardware::None, YUV444_10_FORMATS, |ten| {
            if ten { Err("no R16 planes".into()) } else { Ok(()) }
        });
        assert_eq!(decodable, 0);
        assert!(reason.contains("HEVC 4:4:4 10-bit: no R16 planes"), "{reason}");
    }
}
