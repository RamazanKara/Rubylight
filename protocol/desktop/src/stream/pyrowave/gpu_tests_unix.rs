//! The Linux GPU decode path against the CPU decoder, through the presenter (brief G3): the SDK
//! decodes into CPU plane buffers, `planes_gpu` uploads them to the renderer's SDL GPU device and
//! converts them. These need the SDK's library (`NIGHTJAR_PYROWAVE_SDK`, default
//! `~/nj-pyrowave-sdk`), a Vulkan device and SDL's offscreen video driver, so they are ignored; run
//! them one at a time: `cargo test -p nightjar-desktop -- --ignored --test-threads=1
//! stream::pyrowave::gpu_tests_unix`.

use std::path::PathBuf;
use std::ptr;
use std::sync::{Arc, Mutex, MutexGuard};

use nightjar_core::sdp::{VIDEO_FORMAT_PYROWAVE, VIDEO_FORMAT_PYROWAVE_444, VIDEO_FORMAT_PYROWAVE_MAIN10, VIDEO_FORMAT_PYROWAVE_MAIN10_444};
use nightjar_core::session::VideoSetup;
use sdl3_sys::everything::*;

use super::*;
use crate::stream::pipeline::Decoded;
use crate::stream::present::VideoTexture;
use crate::stream::pyrowave_sdk::{Api, DLL_NAME};

static GPU: Mutex<()> = Mutex::new(());

fn gpu_lock() -> MutexGuard<'static, ()> {
    GPU.lock().unwrap_or_else(|e| e.into_inner())
}

fn sdk() -> Arc<Api> {
    let dir = std::env::var_os("NIGHTJAR_PYROWAVE_SDK").map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join("nj-pyrowave-sdk"));
    Api::load_from(&dir.join(DLL_NAME)).unwrap_or_else(|e| panic!("{e}\nbuild it with scripts/build-pyrowave-sdk.sh"))
}

/// A hidden window with the "gpu" renderer on SDL's offscreen video driver, the presenter's own
/// kind of device when nothing shows the picture.
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
            let window = SDL_CreateWindow(c"PyroWave GPU decode".as_ptr(), 640, 360, SDL_WINDOW_HIDDEN);
            let renderer = crate::sdl::create_renderer(window, true);
            assert!(!renderer.is_null(), "{}", crate::sdl::error());
            assert_eq!(crate::sdl::renderer_name(renderer), "gpu", "the presenter's GPU path needs the SDL GPU renderer");
            Self { window, renderer }
        }
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        // SAFETY: Objects made in `new`.
        unsafe { SDL_DestroyRenderer(self.renderer); SDL_DestroyWindow(self.window); SDL_Quit(); }
    }
}

struct Case {
    bin: &'static [u8],
    width: usize,
    height: usize,
    full: bool,
    ten: bool,
}

fn cases() -> [Case; 4] {
    let c = |bin, width, height, full, ten| Case { bin, width, height, full, ten };
    [
        c(&include_bytes!("../../../tests/fixtures/pyrowave-420-8.bin")[..], 132, 98, false, false),
        c(&include_bytes!("../../../tests/fixtures/pyrowave-420-10.bin")[..], 132, 98, false, true),
        c(&include_bytes!("../../../tests/fixtures/pyrowave-444-8.bin")[..], 131, 97, true, false),
        c(&include_bytes!("../../../tests/fixtures/pyrowave-444-10.bin")[..], 131, 97, true, true),
    ]
}

fn setup(width: usize, height: usize, full: bool, ten: bool, range: i32) -> VideoSetup {
    let format = match (full, ten) {
        (false, false) => VIDEO_FORMAT_PYROWAVE,
        (true, false) => VIDEO_FORMAT_PYROWAVE_444,
        (false, true) => VIDEO_FORMAT_PYROWAVE_MAIN10,
        (true, true) => VIDEO_FORMAT_PYROWAVE_MAIN10_444,
    };
    VideoSetup { format, width: width as i32, height: height as i32, fps: 60, chroma_packing: 0, logical_width: width as i32, logical_height: height as i32, color_space: 1, color_range: range }
}

fn decoded(output: Output, setup: VideoSetup) -> Decoded {
    Decoded { picture: output.picture, setup, rtp: 0, number: 0, receive_us: 0, enqueue_us: 0, input_us: 0, ready_us: 0, host_latency: 0, early: 0, first_block_us: 0 }
}

fn gpu_decoder(case: &Case, slots: usize) -> Pyrowave {
    let config = GpuConfig { width: case.width as u32, height: case.height as u32, chroma444: case.full, ten_bit: case.ten, slots };
    let decoder = GpuDecoder::with_api(sdk(), config).unwrap_or_else(|e| panic!("GPU decoder: {e}"));
    Pyrowave::Gpu(Box::new(GpuStream::with_decoder(decoder)))
}

/// Uploads `decoded` the way the presenter does and reads the texture back as RGBA32F.
fn present(rig: &Rig, texture: &mut VideoTexture, decoded: &Decoded) -> Vec<f32> {
    assert!(texture.upload(rig.renderer, decoded), "upload failed: {:?} {}", texture.failure, crate::sdl::error());
    // SAFETY: The texture belongs to the live renderer; the surfaces are destroyed here.
    unsafe {
        assert!(SDL_SetRenderTarget(rig.renderer, texture.texture), "{}", crate::sdl::error());
        let readback = SDL_RenderReadPixels(rig.renderer, ptr::null());
        assert!(!readback.is_null(), "{}", crate::sdl::error());
        let pixels = SDL_ConvertSurface(readback, SDL_PIXELFORMAT_RGBA128_FLOAT);
        SDL_DestroySurface(readback);
        assert!(!pixels.is_null());
        let mut out = Vec::new();
        for y in 0..(*pixels).h as usize {
            let row = std::slice::from_raw_parts(((*pixels).pixels as *const u8).add(y * (*pixels).pitch as usize).cast::<f32>(), (*pixels).w as usize * 4);
            out.extend_from_slice(row);
        }
        SDL_DestroySurface(pixels);
        SDL_SetRenderTarget(rig.renderer, ptr::null_mut());
        out
    }
}

/// The sRGB encoding of a linear value: what an 8-bit display code is made of.
fn encode(linear: f32) -> f32 {
    let v = linear.clamp(0.0, 1.0);
    if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
}

/// The most the GPU picture may differ from the CPU one, as (largest, root-mean-square) of encoded
/// RGB: the Windows path's bounds (`gpu_tests.rs`), since the SDK writes the same planes.
fn bounds(ten: bool) -> (f32, f64) {
    if ten { (1.0 / 255.0, 0.001) } else { (2.5 / 255.0, 0.0025) }
}

/// The most the GPU picture may be brighter or darker than the CPU one on average (encoded RGB).
const MEAN_LIMIT: f64 = 0.25 / 255.0;

/// Largest, root-mean-square and signed mean (GPU minus CPU) difference of the colour channels, as
/// encoded 0..1 values.
fn compare(cpu: &[f32], gpu: &[f32]) -> (f32, f64, f64) {
    assert_eq!(cpu.len(), gpu.len());
    let (mut max, mut squares, mut sum, mut n) = (0.0f32, 0.0f64, 0.0f64, 0usize);
    for (c, g) in cpu.chunks(4).zip(gpu.chunks(4)) {
        for k in 0..3 {
            let signed = encode(g[k]) - encode(c[k]);
            max = max.max(signed.abs());
            squares += f64::from(signed).powi(2);
            sum += f64::from(signed);
            n += 1;
        }
    }
    (max, (squares / n as f64).sqrt(), sum / n as f64)
}

fn cpu_picture(rig: &Rig, case: &Case, setup: VideoSetup, hdr: bool) -> Vec<f32> {
    let mut cpu = Pyrowave::cpu(case.width, case.height, case.full).unwrap();
    let output = cpu.frame(case.bin, &mut || false).unwrap_or_else(|e| panic!("CPU decode: {e:?}"));
    let mut texture = VideoTexture::default();
    texture.set_hdr_mode(hdr, None);
    present(rig, &mut texture, &decoded(output, setup))
}

fn gpu_picture(rig: &Rig, gpu: &mut Pyrowave, case: &Case, setup: VideoSetup, hdr: bool) -> Vec<f32> {
    let output = gpu.frame(case.bin, &mut || false).unwrap_or_else(|e| panic!("GPU decode: {e:?}"));
    assert!(matches!(output.picture, Picture::GpuPlanes { .. }), "the GPU decoder makes plane pictures");
    let mut texture = VideoTexture::default();
    texture.set_hdr_mode(hdr, None);
    present(rig, &mut texture, &decoded(output, setup))
}

#[test]
#[ignore = "needs the PyroWave SDK, a Vulkan device and SDL's offscreen driver"]
fn the_gpu_picture_matches_the_cpu_picture_on_the_fixtures() {
    let _lock = gpu_lock();
    let rig = Rig::new();
    for (n, case) in cases().iter().enumerate() {
        let mut gpu = gpu_decoder(case, 2);
        for range in [0, 1] {
            let setup = setup(case.width, case.height, case.full, case.ten, range);
            let (max, rms, mean) = compare(&cpu_picture(&rig, case, setup, false), &gpu_picture(&rig, &mut gpu, case, setup, false));
            println!("fixture {n} ({}x{} 444={} 10-bit={} range={range}): max {:.5} ({:.2} codes), rms {rms:.6}, mean {:+.2e} (encoded RGB, GPU against CPU)", case.width, case.height, case.full, case.ten, max, max * 255.0, mean);
            let (limit, rms_limit) = bounds(case.ten);
            assert!(max < limit && rms < rms_limit, "fixture {n} range {range}: GPU picture differs from the CPU picture: max {max} (limit {limit}), rms {rms} (limit {rms_limit})");
            assert!(mean.abs() < MEAN_LIMIT, "fixture {n} range {range}: the GPU picture is {mean:+.2e} off the CPU picture on average (limit {MEAN_LIMIT:.2e})");
        }
    }
}

#[test]
#[ignore = "needs the PyroWave SDK, a Vulkan device and SDL's offscreen driver"]
fn a_sequence_of_pictures_reuses_the_plane_sets_and_stays_the_cpu_picture() {
    let _lock = gpu_lock();
    let rig = Rig::new();
    let case = &cases()[1];
    let mut gpu = gpu_decoder(case, 2);
    let setup = setup(case.width, case.height, case.full, case.ten, 0);
    let cpu = cpu_picture(&rig, case, setup, false);
    // More pictures than plane sets: each one is presented, which gives its set back.
    for round in 0..6 {
        let (max, rms, mean) = compare(&cpu, &gpu_picture(&rig, &mut gpu, case, setup, false));
        let (limit, rms_limit) = bounds(case.ten);
        assert!(max < limit && rms < rms_limit && mean.abs() < MEAN_LIMIT, "picture {round}: max {max}, rms {rms}, mean {mean:+.2e}");
    }
}
