//! The Linux GPU decode path against the CPU decoder, through the presenter (brief G3): the SDK
//! decodes into CPU plane buffers, `planes_gpu` uploads them to the renderer's SDL GPU device and
//! converts them; or, sharing that device (`zero_copy.rs`), it decodes into images there that
//! `planes_gpu` copies on the GPU. These need the SDK's library (`NIGHTJAR_PYROWAVE_SDK`, default
//! `~/nj-pyrowave-sdk`), a Vulkan device and SDL's offscreen video driver, so they are ignored; run
//! them one at a time: `cargo test -p nightjar-desktop -- --ignored --test-threads=1
//! stream::pyrowave::gpu_tests_unix`.

use std::path::PathBuf;
use std::ptr;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

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
/// kind of device when nothing shows the picture. With `share`, the renderer asks for the
/// decoder's Vulkan features, as the client's does when the SDK is beside it.
struct Rig {
    window: *mut SDL_Window,
    renderer: *mut SDL_Renderer,
}

impl Rig {
    fn new() -> Self {
        Self::with(true)
    }

    fn with(share: bool) -> Self {
        // SAFETY: Plain SDL calls on objects made here.
        unsafe {
            SDL_SetHint(SDL_HINT_VIDEO_DRIVER, c"offscreen".as_ptr());
            assert!(SDL_Init(SDL_INIT_VIDEO), "{}", crate::sdl::error());
            let window = SDL_CreateWindow(c"PyroWave GPU decode".as_ptr(), 640, 360, SDL_WINDOW_HIDDEN);
            let renderer = crate::sdl::create_renderer_for(window, true, share, false);
            assert!(!renderer.is_null(), "{}", crate::sdl::error());
            assert_eq!(crate::sdl::renderer_name(renderer), "gpu", "the presenter's GPU path needs the SDL GPU renderer");
            Self { window, renderer }
        }
    }

    /// The renderer's SDL GPU device.
    fn device(&self) -> *mut std::ffi::c_void {
        crate::sdl::renderer_pointer(self.renderer, SDL_PROP_RENDERER_GPU_DEVICE_POINTER)
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
    Decoded { picture: output.picture, setup, rtp: 0, number: 0, receive_us: 0, enqueue_us: 0, input_us: 0, ready_us: 0, host_latency: 0, early: 0, first_block_us: 0, bands: Vec::new(), race: Default::default(), race_seq: 0 }
}

fn gpu_decoder(case: &Case, slots: usize) -> Pyrowave {
    let config = GpuConfig { width: case.width as u32, height: case.height as u32, chroma444: case.full, ten_bit: case.ten, slots };
    let decoder = GpuDecoder::with_api(sdk(), config).unwrap_or_else(|e| panic!("GPU decoder: {e}"));
    Pyrowave::Gpu(Box::new(GpuStream::with_decoder(decoder)))
}

/// The decoder on the renderer's own device, its planes images there.
fn shared_decoder(rig: &Rig, case: &Case, slots: usize) -> Pyrowave {
    let config = GpuConfig { width: case.width as u32, height: case.height as u32, chroma444: case.full, ten_bit: case.ten, slots };
    let decoder = GpuDecoder::share(sdk(), rig.device(), config).unwrap_or_else(|e| panic!("the decoder cannot share the renderer's device: {e}"));
    assert!(decoder.images().is_some());
    Pyrowave::Gpu(Box::new(GpuStream::with_decoder(decoder)))
}

/// Uploads `decoded` the way the presenter does and reads the texture back as RGBA32F.
fn present(rig: &Rig, texture: &mut VideoTexture, decoded: &Decoded) -> Vec<f32> {
    assert!(texture.upload(rig.renderer, decoded, None), "upload failed: {:?} {}", texture.failure, crate::sdl::error());
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

/// The most the zero-copy picture may differ from the readback one (encoded RGB): the same SDK
/// decodes the same records into the same planes, so only rounding may differ.
const SAME_LIMIT: f32 = 0.5 / 255.0;

#[test]
#[ignore = "needs the PyroWave SDK, a Vulkan device and SDL's offscreen driver"]
fn the_zero_copy_picture_is_the_readback_picture_on_the_fixtures() {
    let _lock = gpu_lock();
    let rig = Rig::new();
    for (n, case) in cases().iter().enumerate() {
        let mut readback = gpu_decoder(case, 2);
        let mut shared = shared_decoder(&rig, case, 2);
        for range in [0, 1] {
            let setup = setup(case.width, case.height, case.full, case.ten, range);
            let read = gpu_picture(&rig, &mut readback, case, setup, false);
            let zero = gpu_picture(&rig, &mut shared, case, setup, false);
            let (max, rms, mean) = compare(&read, &zero);
            println!("fixture {n} ({}x{} 444={} 10-bit={} range={range}): zero-copy against readback max {max:.6}, rms {rms:.7}, mean {mean:+.2e}", case.width, case.height, case.full, case.ten);
            assert!(max <= SAME_LIMIT, "fixture {n} range {range}: the zero-copy picture differs from the readback one by {max}");
            let (max, rms, mean) = compare(&cpu_picture(&rig, case, setup, false), &zero);
            let (limit, rms_limit) = bounds(case.ten);
            assert!(max < limit && rms < rms_limit && mean.abs() < MEAN_LIMIT, "fixture {n} range {range}: against the CPU picture max {max}, rms {rms}, mean {mean:+.2e}");
        }
    }
}

/// More pictures than plane sets: each set goes back to the decoder with the value its copy
/// signalled, and the decode into it waits for that on the GPU, so no picture shows the next one.
#[test]
#[ignore = "needs the PyroWave SDK, a Vulkan device and SDL's offscreen driver"]
fn a_sequence_of_zero_copy_pictures_reuses_the_plane_sets_after_their_copies() {
    let _lock = gpu_lock();
    let rig = Rig::new();
    let case = &cases()[0];
    let mut gpu = shared_decoder(&rig, case, 2);
    let setup = setup(case.width, case.height, case.full, case.ten, 0);
    let cpu = cpu_picture(&rig, case, setup, false);
    for round in 0..6 {
        let (max, rms, mean) = compare(&cpu, &gpu_picture(&rig, &mut gpu, case, setup, false));
        let (limit, rms_limit) = bounds(case.ten);
        assert!(max < limit && rms < rms_limit && mean.abs() < MEAN_LIMIT, "picture {round}: max {max}, rms {rms}, mean {mean:+.2e}");
    }
    let Pyrowave::Gpu(stream) = &gpu else { unreachable!() };
    let (_, after) = stream.planes().acquire().unwrap();
    assert!(after >= 5, "the set waits for the copy of the picture before last: {after}");
}

/// A renderer made without the decoder's features cannot be shared, and says so.
#[test]
#[ignore = "needs the PyroWave SDK, a Vulkan device and SDL's offscreen driver"]
fn a_renderer_made_without_the_features_is_not_shared() {
    let _lock = gpu_lock();
    let rig = Rig::with(false);
    let config = GpuConfig { width: 128, height: 128, chroma444: false, ten_bit: false, slots: 2 };
    let error = GpuDecoder::share(sdk(), rig.device(), config).map(drop).unwrap_err();
    assert!(error.contains("created without the decoders' features"), "{error}");
}

/// The frame container `push_container` reads: the packet count, then each packet's size and bytes.
fn container(packets: &[Vec<u8>]) -> Vec<u8> {
    let mut out = (packets.len() as u32).to_le_bytes().to_vec();
    for packet in packets {
        out.extend_from_slice(&(packet.len() as u32).to_le_bytes());
        out.extend_from_slice(packet);
    }
    out
}

/// The process's CPU time (all threads, user and system), in seconds.
fn cpu_seconds() -> f64 {
    // SAFETY: getrusage fills the struct.
    let usage = unsafe {
        let mut usage = std::mem::zeroed::<libc::rusage>();
        libc::getrusage(libc::RUSAGE_SELF, &mut usage);
        usage
    };
    let seconds = |t: libc::timeval| t.tv_sec as f64 + t.tv_usec as f64 / 1e6;
    seconds(usage.ru_utime) + seconds(usage.ru_stime)
}

/// The readback against zero-copy through the stream and the presenter: per frame, the time from
/// the frame to its planes being final (`frame`: push, decode, and the readback), the presenter's
/// CPU time to copy or upload them and queue the conversion (`upload`), the process's CPU time and
/// the wall time. Synthetic 4:2:0 frames at 0.3 byte per pixel, as `decode_timing_at_1080p_1440p_and_4k`.
#[test]
#[ignore = "needs the PyroWave SDK, a Vulkan device and SDL's offscreen driver"]
fn zero_copy_against_readback_timing_at_1080p_and_4k() {
    use crate::stream::pyrowave_sdk::tests::{percentile, record_pool, synthetic_frame};
    let _lock = gpu_lock();
    let rig = Rig::new();
    let pool = record_pool();
    let (warmup, frames) = (30, 300);
    for (name, width, height) in [("1080p", 1920usize, 1080usize), ("4K", 3840, 2160)] {
        let data: Vec<Vec<u8>> = (0..8).map(|s| container(&synthetic_frame(&pool, width, height, width * height * 3 / 10, s))).collect();
        let setup = setup(width, height, false, false, 0);
        for zero_copy in [false, true] {
            let config = GpuConfig { width: width as u32, height: height as u32, chroma444: false, ten_bit: false, slots: 3 };
            let decoder = if zero_copy { GpuDecoder::share(sdk(), rig.device(), config) } else { GpuDecoder::with_api(sdk(), config) };
            let mut gpu = Pyrowave::Gpu(Box::new(GpuStream::with_decoder(decoder.unwrap_or_else(|e| panic!("{e}")))));
            let mut texture = VideoTexture::default();
            texture.set_hdr_mode(false, None);
            let (mut decode, mut upload) = (Vec::new(), Vec::new());
            let (mut cpu, mut wall) = (0.0, Instant::now());
            for i in 0..warmup + frames {
                if i == warmup { (cpu, wall) = (cpu_seconds(), Instant::now()); }
                let start = Instant::now();
                let output = gpu.frame(&data[i % data.len()], &mut || false).unwrap_or_else(|e| panic!("frame {i}: {e:?}"));
                let ready = start.elapsed();
                let picture = decoded(output, setup);
                let start = Instant::now();
                assert!(texture.upload(rig.renderer, &picture, None), "upload failed: {:?}", texture.failure);
                let uploaded = start.elapsed();
                // As the window does: SDL submits the renderer's commands only when it presents, and
                // until then every plane texture they sample stays in use, so each copy or upload
                // cycles to a new one until device memory runs out. Without a swapchain nothing
                // bounds the GPU's backlog either, so the wait stands in for the display's pace.
                // SAFETY: The rig's live renderer and its device.
                unsafe {
                    SDL_RenderPresent(rig.renderer);
                    SDL_WaitForGPUIdle(rig.device().cast());
                }
                if i >= warmup {
                    decode.push(ready.as_secs_f64() * 1000.0);
                    upload.push(uploaded.as_secs_f64() * 1000.0);
                }
            }
            // SAFETY: The renderer's live device.
            unsafe { SDL_WaitForGPUIdle(rig.device().cast()) };
            let (cpu, wall) = ((cpu_seconds() - cpu) * 1000.0 / frames as f64, wall.elapsed().as_secs_f64() * 1000.0 / frames as f64);
            println!(
                "{name} {}: frame to final planes p50 {:.3} ms p95 {:.3} ms; presenter copy or upload p50 {:.3} ms p95 {:.3} ms; CPU {cpu:.2} ms/frame; wall {wall:.2} ms/frame",
                if zero_copy { "zero-copy" } else { "readback" },
                percentile(&mut decode.clone(), 50), percentile(&mut decode, 95), percentile(&mut upload.clone(), 50), percentile(&mut upload, 95),
            );
        }
    }
}
