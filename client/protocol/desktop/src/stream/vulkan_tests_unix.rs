//! Vulkan video against software decoding, through the presenter: FFmpeg decodes the fixtures on
//! the offscreen "gpu" renderer's own Vulkan device (`crate::vulkan_video`), the presenter copies
//! the picture's two planes on the GPU (`vulkan_video::Planes`) and draws them with the plane
//! shader, and the result must match the software decoder's picture drawn the way the presenter
//! draws it. These need a Vulkan device with video decoding (ANV only with `ANV_VIDEO_DECODE=1`) and
//! SDL's offscreen video driver, so they are ignored; run them one at a time:
//! `ANV_VIDEO_DECODE=1 cargo test -p nightjar-desktop -- --ignored --test-threads=1
//! stream::vulkan_tests_unix`.

use std::ptr;
use std::sync::{Mutex, MutexGuard};

use nightjar_core::sdp::{
    VIDEO_FORMAT_H264_HIGH8_444, VIDEO_FORMAT_H265, VIDEO_FORMAT_H265_MAIN10,
    VIDEO_FORMAT_H265_REXT8_444,
};
use nightjar_core::session::VideoSetup;
use sdl3_sys::everything::*;

use crate::media::{self, Codec, Hardware, Pictures, Received, VideoDecoder};
use crate::stream::pipeline::Decoded;
use crate::stream::present::VideoTexture;

static GPU: Mutex<()> = Mutex::new(());

fn gpu_lock() -> MutexGuard<'static, ()> {
    GPU.lock().unwrap_or_else(|e| e.into_inner())
}

/// A hidden window with the "gpu" renderer on SDL's offscreen video driver, made as the client
/// makes it for Vulkan video: with the video extensions and SDL's decode queues.
struct Rig {
    window: *mut SDL_Window,
    renderer: *mut SDL_Renderer,
}

impl Rig {
    fn new() -> Self {
        // SAFETY: Plain SDL calls on objects made here.
        unsafe {
            SDL_SetHint(SDL_HINT_VIDEO_DRIVER, c"offscreen".as_ptr());
            assert!(SDL_Init(SDL_INIT_VIDEO), "{}", crate::sdl::error());
            let window = SDL_CreateWindow(c"Vulkan video".as_ptr(), 640, 360, SDL_WINDOW_HIDDEN);
            let renderer = crate::sdl::create_renderer_for(window, true, true);
            assert!(!renderer.is_null(), "{}", crate::sdl::error());
            assert_eq!(
                crate::sdl::renderer_name(renderer),
                "gpu",
                "Vulkan video needs the SDL GPU renderer"
            );
            Self { window, renderer }
        }
    }

    /// The renderer's SDL GPU device, which FFmpeg decodes on.
    fn device(&self) -> *mut std::ffi::c_void {
        crate::sdl::renderer_pointer(self.renderer, SDL_PROP_RENDERER_GPU_DEVICE_POINTER)
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        // SAFETY: Objects made in `new`.
        unsafe {
            SDL_DestroyRenderer(self.renderer);
            SDL_DestroyWindow(self.window);
            SDL_Quit();
        }
    }
}

struct Case {
    name: &'static str,
    codec: Codec,
    bin: &'static [u8],
    format: u32,
    size: i32,
    ten: bool,
    chroma444: bool,
}

/// The 4:2:0 fixtures, 8- and 10-bit (128x128), and the lossless 4:4:4 ones (64x64), which a
/// decoder without those profiles declines before FFmpeg opens.
fn cases() -> [Case; 4] {
    let c = |name, codec, bin, format, size, ten, chroma444| Case {
        name,
        codec,
        bin,
        format,
        size,
        ten,
        chroma444,
    };
    [
        c(
            "HEVC 4:2:0 8-bit",
            Codec::Hevc,
            &include_bytes!("../../../../app/src/main/assets/packed444/hevc-8.bin")[..],
            VIDEO_FORMAT_H265,
            128,
            false,
            false,
        ),
        c(
            "HEVC 4:2:0 10-bit",
            Codec::Hevc,
            &include_bytes!("../../../../app/src/main/assets/packed444/hevc-10.bin")[..],
            VIDEO_FORMAT_H265_MAIN10,
            128,
            true,
            false,
        ),
        c(
            "H.264 4:4:4 8-bit",
            Codec::H264,
            &include_bytes!("../../tests/fixtures/h264-444-8.bin")[..],
            VIDEO_FORMAT_H264_HIGH8_444,
            64,
            false,
            true,
        ),
        c(
            "HEVC 4:4:4 8-bit",
            Codec::Hevc,
            &include_bytes!("../../tests/fixtures/hevc-444-8.bin")[..],
            VIDEO_FORMAT_H265_REXT8_444,
            64,
            false,
            true,
        ),
    ]
}

/// The colour a picture is drawn in: its range, matrix and transfer (FFmpeg's values), set on the
/// decoded frame as a stream's VUI would, and whether the presenter shows HDR.
#[derive(Clone, Copy, Debug)]
struct Look {
    name: &'static str,
    range: i32,
    space: i32,
    trc: i32,
    hdr: bool,
}

const LIMITED: i32 = 1;
const FULL: i32 = 2;
const BT709: i32 = 1;
const BT2020: i32 = 9;
const HLG: i32 = 18;

/// SDR in both ranges for every fixture, and for 10-bit pictures also BT.2020 HLG shown in an HDR
/// stream. HDR10 (PQ) is not among them: the presenter refuses a PQ picture without an HDR
/// output, which SDL's offscreen driver never has.
fn looks(ten: bool) -> Vec<Look> {
    let l = |name, range, space, trc, hdr| Look {
        name,
        range,
        space,
        trc,
        hdr,
    };
    let mut looks = vec![
        l("SDR limited", LIMITED, BT709, BT709, false),
        l("SDR full", FULL, BT709, BT709, false),
    ];
    if ten {
        looks.extend([
            l("HLG limited", LIMITED, BT2020, HLG, true),
            l("HLG full", FULL, BT2020, HLG, true),
        ]);
    }
    looks
}

/// A decoder of `case` with `hardware` (required, so a decoder that would fall back to software
/// says why instead) and its first picture, tagged with `look`. The decoder lives as long as the
/// picture.
fn decode(
    case: &Case,
    hardware: Hardware,
    look: Look,
) -> Result<(VideoDecoder, media::Frame), String> {
    let pictures = Pictures {
        ten_bit: case.ten,
        chroma444: case.chroma444,
        packed: false,
    };
    let mut decoder = match hardware {
        Hardware::None => VideoDecoder::open(case.codec, hardware, 1, false)?,
        hardware => VideoDecoder::open_for(case.codec, pictures, hardware, 1, true)?,
    };
    let status = decoder.send(case.bin, 0);
    if status < 0 {
        return Err(format!("the decoder rejected the fixture ({status})"));
    }
    let frame = match decoder.receive() {
        Received::Frame(frame) => frame,
        Received::None => return Err("the decoder made no picture of the fixture".into()),
        Received::Error => return Err("the decoder failed on the fixture".into()),
    };
    tag(&frame, look);
    Ok((decoder, frame))
}

fn tag(frame: &media::Frame, look: Look) {
    // SAFETY: The live frame, owned by `frame`; only its colour fields change.
    unsafe {
        let raw = frame.raw.as_ptr();
        (*raw).color_range = look.range as _;
        (*raw).colorspace = look.space as _;
        (*raw).color_trc = look.trc as _;
    }
}

fn decoded(frame: media::Frame, case: &Case) -> Decoded {
    let setup = VideoSetup {
        format: case.format,
        width: case.size,
        height: case.size,
        fps: 60,
        chroma_packing: 0,
        logical_width: case.size,
        logical_height: case.size,
        color_space: 1,
        color_range: 0,
    };
    Decoded {
        picture: media::Picture::Frame(frame),
        setup,
        rtp: 0,
        number: 0,
        receive_us: 0,
        enqueue_us: 0,
        input_us: 0,
        ready_us: 0,
        host_latency: 0,
        early: 0,
        first_block_us: 0,
        bands: Vec::new(),
        race: Default::default(),
        race_seq: 0,
    }
}

/// Uploads `decoded` the way the presenter does, draws its texture into an RGBA64F scRGB target
/// of the picture's size, as the presenter draws it into the window, and reads that back as
/// RGBA32F.
fn present(rig: &Rig, texture: &mut VideoTexture, decoded: &Decoded) -> Vec<f32> {
    assert!(
        texture.upload(rig.renderer, decoded, None),
        "upload failed: {:?} {}",
        texture.failure,
        crate::sdl::error()
    );
    let size = decoded.setup.width;
    // SAFETY: The textures belong to the live renderer; the target and the surfaces are destroyed here.
    unsafe {
        let props = SDL_CreateProperties();
        SDL_SetNumberProperty(
            props,
            SDL_PROP_TEXTURE_CREATE_FORMAT_NUMBER,
            SDL_PIXELFORMAT_RGBA64_FLOAT.0 as i64,
        );
        SDL_SetNumberProperty(
            props,
            SDL_PROP_TEXTURE_CREATE_ACCESS_NUMBER,
            SDL_TEXTUREACCESS_TARGET.0 as i64,
        );
        SDL_SetNumberProperty(
            props,
            SDL_PROP_TEXTURE_CREATE_COLORSPACE_NUMBER,
            SDL_COLORSPACE_SRGB_LINEAR.0 as i64,
        );
        SDL_SetNumberProperty(props, SDL_PROP_TEXTURE_CREATE_WIDTH_NUMBER, i64::from(size));
        SDL_SetNumberProperty(
            props,
            SDL_PROP_TEXTURE_CREATE_HEIGHT_NUMBER,
            i64::from(size),
        );
        let target = SDL_CreateTextureWithProperties(rig.renderer, props);
        SDL_DestroyProperties(props);
        assert!(!target.is_null(), "{}", crate::sdl::error());
        assert!(
            SDL_SetRenderTarget(rig.renderer, target),
            "{}",
            crate::sdl::error()
        );
        assert!(
            SDL_RenderTexture(rig.renderer, texture.texture, ptr::null(), ptr::null()),
            "{}",
            crate::sdl::error()
        );
        let readback = SDL_RenderReadPixels(rig.renderer, ptr::null());
        assert!(!readback.is_null(), "{}", crate::sdl::error());
        let pixels = SDL_ConvertSurface(readback, SDL_PIXELFORMAT_RGBA128_FLOAT);
        SDL_DestroySurface(readback);
        assert!(!pixels.is_null());
        let mut out = Vec::new();
        for y in 0..(*pixels).h as usize {
            let row = std::slice::from_raw_parts(
                ((*pixels).pixels as *const u8)
                    .add(y * (*pixels).pitch as usize)
                    .cast::<f32>(),
                (*pixels).w as usize * 4,
            );
            out.extend_from_slice(row);
        }
        SDL_DestroySurface(pixels);
        SDL_SetRenderTarget(rig.renderer, ptr::null_mut());
        SDL_DestroyTexture(target);
        out
    }
}

/// The sRGB encoding of a linear value: what an 8-bit display code is made of.
fn encode_sdr(linear: f32) -> f32 {
    let v = linear.clamp(0.0, 1.0);
    if v <= 0.003_130_8 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

/// The PQ encoding of a linear scRGB value (1.0 = 80 nits), what an HDR10 display code is made of.
fn encode_pq(linear: f32) -> f32 {
    let (m1, m2, c1, c2, c3) = (0.159_301_76, 78.843_75, 0.835_937_5, 18.851_563, 18.6875);
    let y = (f64::from(linear) * 80.0 / 10_000.0)
        .clamp(0.0, 1.0)
        .powf(m1);
    ((c1 + c2 * y) / (1.0 + c3 * y)).powf(m2) as f32
}

/// The most the Vulkan picture may differ from the software one, as (largest, root-mean-square) of
/// encoded RGB: the PyroWave GPU path's bounds (`pyrowave/gpu_tests_unix.rs`).
fn bounds(ten: bool) -> (f32, f64) {
    if ten {
        (1.0 / 255.0, 0.001)
    } else {
        (2.5 / 255.0, 0.0025)
    }
}

/// The most the Vulkan picture may be brighter or darker than the software one on average.
const MEAN_LIMIT: f64 = 0.25 / 255.0;

/// Largest, root-mean-square and signed mean (Vulkan minus software) difference of the colour
/// channels, as encoded 0..1 values (PQ for an HDR picture, else sRGB).
fn compare(software: &[f32], vulkan: &[f32], hdr: bool) -> (f32, f64, f64) {
    assert_eq!(software.len(), vulkan.len());
    let encode = if hdr { encode_pq } else { encode_sdr };
    let (mut max, mut squares, mut sum, mut n) = (0.0f32, 0.0f64, 0.0f64, 0usize);
    for (s, v) in software.chunks(4).zip(vulkan.chunks(4)) {
        for k in 0..3 {
            let signed = encode(v[k]) - encode(s[k]);
            max = max.max(signed.abs());
            squares += f64::from(signed).powi(2);
            sum += f64::from(signed);
            n += 1;
        }
    }
    (max, (squares / n as f64).sqrt(), sum / n as f64)
}

/// The picture of `case` in `look`, decoded with `hardware` and drawn by a new texture.
fn picture(rig: &Rig, case: &Case, hardware: Hardware, look: Look) -> Result<Vec<f32>, String> {
    let (_decoder, frame) = decode(case, hardware, look)?;
    let kind = frame.info().kind;
    let vulkan = matches!(hardware, Hardware::Vulkan(_));
    assert!(
        !vulkan || matches!(kind, media::FRAME_VULKAN | media::FRAME_VULKAN444),
        "{}: FFmpeg decoded the fixture in software (frame kind {kind}), not with Vulkan video",
        case.name
    );
    let mut texture = VideoTexture::default();
    texture.set_hdr_mode(look.hdr, None);
    Ok(present(rig, &mut texture, &decoded(frame, case)))
}

/// Whether Vulkan video declined `case` before FFmpeg opened because the device lacks its profile
/// or picture format (`vulkan_video::check_profile`), the one reason a case may not run.
fn declined(error: &str) -> bool {
    error.contains("the renderer's Vulkan device does not decode")
        || error.contains("the renderer's Vulkan device lists no picture formats")
        || error.contains("the renderer's Vulkan device decodes")
}

#[test]
#[ignore = "needs a Vulkan device with video decoding and SDL's offscreen driver"]
fn the_vulkan_picture_matches_the_software_picture_on_the_fixtures() {
    let _lock = gpu_lock();
    let rig = Rig::new();
    let mut ran = Vec::new();
    for case in cases() {
        let mut previous: Option<Vec<f32>> = None;
        for look in looks(case.ten) {
            let vulkan = match picture(&rig, &case, Hardware::Vulkan(rig.device()), look) {
                Ok(vulkan) => vulkan,
                Err(error) if case.chroma444 && declined(&error) => {
                    println!(
                        "{}: not run, Vulkan video declines it on this device: {error}",
                        case.name
                    );
                    break;
                }
                Err(error) => panic!("{} {}: Vulkan video: {error}", case.name, look.name),
            };
            let software = picture(&rig, &case, Hardware::None, look)
                .unwrap_or_else(|e| panic!("{} {}: software: {e}", case.name, look.name));
            // Each look must draw the fixture differently, or the comparison would not test it.
            if let Some(previous) = &previous {
                let (change, _, _) = compare(previous, &software, false);
                assert!(
                    change > 4.0 / 255.0,
                    "{} {}: the software picture is the previous look's ({change})",
                    case.name,
                    look.name
                );
            }
            let (max, rms, mean) = compare(&software, &vulkan, look.hdr);
            println!("{} {}: max {max:.5} ({:.2} codes), rms {rms:.6}, mean {mean:+.2e} (encoded RGB, Vulkan against software)", case.name, look.name, max * 255.0);
            let (limit, rms_limit) = bounds(case.ten);
            assert!(max < limit && rms < rms_limit, "{} {}: the Vulkan picture differs from the software picture: max {max} (limit {limit}), rms {rms} (limit {rms_limit})", case.name, look.name);
            assert!(mean.abs() < MEAN_LIMIT, "{} {}: the Vulkan picture is {mean:+.2e} off the software picture on average (limit {MEAN_LIMIT:.2e})", case.name, look.name);
            ran.push(format!("{} {}", case.name, look.name));
            previous = Some(software);
        }
    }
    println!("ran: {}", ran.join(", "));
    assert!(
        ran.iter().any(|run| run.starts_with("HEVC 4:2:0 8-bit"))
            && ran.iter().any(|run| run.starts_with("HEVC 4:2:0 10-bit")),
        "the 4:2:0 fixtures must run"
    );
}

/// More pictures than the planes' copy slots, from one decoder into one texture as in a stream:
/// each copy waits for its picture's decode and the slot it reuses, so every picture stays the
/// software one.
#[test]
#[ignore = "needs a Vulkan device with video decoding and SDL's offscreen driver"]
fn a_sequence_of_vulkan_pictures_reuses_the_copy_slots_and_stays_the_software_picture() {
    let _lock = gpu_lock();
    let rig = Rig::new();
    for case in &cases()[..2] {
        let look = looks(case.ten)[0];
        let software =
            picture(&rig, case, Hardware::None, look).unwrap_or_else(|e| panic!("software: {e}"));
        let (mut decoder, first) = decode(case, Hardware::Vulkan(rig.device()), look)
            .unwrap_or_else(|e| panic!("{}: Vulkan video: {e}", case.name));
        let mut texture = VideoTexture::default();
        texture.set_hdr_mode(look.hdr, None);
        let mut frame = Some(first);
        for round in 0..8 {
            let frame = match frame.take() {
                Some(frame) => frame,
                None => {
                    assert!(
                        decoder.send(case.bin, round) >= 0,
                        "{} picture {round}: the decoder rejected the fixture",
                        case.name
                    );
                    let Received::Frame(frame) = decoder.receive() else {
                        panic!("{} picture {round}: no picture", case.name)
                    };
                    tag(&frame, look);
                    frame
                }
            };
            let (max, rms, mean) = compare(
                &software,
                &present(&rig, &mut texture, &decoded(frame, case)),
                false,
            );
            let (limit, rms_limit) = bounds(case.ten);
            assert!(
                max < limit && rms < rms_limit && mean.abs() < MEAN_LIMIT,
                "{} picture {round}: max {max}, rms {rms}, mean {mean:+.2e}",
                case.name
            );
        }
        println!(
            "{}: 8 pictures in a row match the software picture",
            case.name
        );
    }
}
