//! The GPU decode path against the CPU decoder, through the presenter (brief G2). These need the
//! SDK's DLL (`NIGHTJAR_PYROWAVE_SDK`, default `C:\src\butterpollo-release\nj-pyrowave-sdk`), a GPU
//! and a Direct3D 11 window, so they are ignored; run them one at a time under the GPU lock:
//! `cargo test -p nightjar-desktop -- --ignored --test-threads=1 stream::pyrowave::gpu_tests`.

use std::path::PathBuf;
use std::ptr;
use std::sync::{Mutex, MutexGuard};

use nightjar_core::sdp::{VIDEO_FORMAT_PYROWAVE, VIDEO_FORMAT_PYROWAVE_444, VIDEO_FORMAT_PYROWAVE_MAIN10, VIDEO_FORMAT_PYROWAVE_MAIN10_444};
use nightjar_core::session::VideoSetup;
use sdl3_sys::everything::*;
use windows::core::Interface;
use windows::Win32::Graphics::Direct3D11::ID3D11Device;

use super::*;
use crate::stream::pipeline::Decoded;
use crate::stream::present::VideoTexture;
use crate::stream::pyrowave_sdk::{Api, DLL_NAME};

static GPU: Mutex<()> = Mutex::new(());

fn gpu_lock() -> MutexGuard<'static, ()> {
    GPU.lock().unwrap_or_else(|e| e.into_inner())
}

fn sdk() -> std::sync::Arc<Api> {
    let dir = std::env::var_os("NIGHTJAR_PYROWAVE_SDK").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\src\butterpollo-release\nj-pyrowave-sdk"));
    Api::load_from(&dir.join(DLL_NAME)).unwrap_or_else(|e| panic!("{e}\nbuild it with scripts/build-pyrowave-sdk.ps1"))
}

/// A hidden Direct3D 11 renderer, the presenter's own kind of device.
struct Rig {
    window: *mut SDL_Window,
    renderer: *mut SDL_Renderer,
}

impl Rig {
    fn new() -> Self {
        // SAFETY: Plain SDL calls on objects made here.
        unsafe {
            assert!(SDL_Init(SDL_INIT_VIDEO), "{}", crate::sdl::error());
            SDL_SetHint(SDL_HINT_RENDER_DRIVER, c"direct3d11".as_ptr());
            SDL_SetHint(SDL_HINT_RENDER_DIRECT3D_THREADSAFE, c"1".as_ptr());
            let window = SDL_CreateWindow(c"PyroWave GPU decode".as_ptr(), 640, 360, SDL_WINDOW_HIDDEN);
            let renderer = crate::sdl::create_renderer(window, true);
            assert!(!renderer.is_null(), "{}", crate::sdl::error());
            Self { window, renderer }
        }
    }

    fn device(&self) -> ID3D11Device {
        let raw = crate::sdl::renderer_pointer(self.renderer, SDL_PROP_RENDERER_D3D11_DEVICE_POINTER);
        // SAFETY: The renderer's live device, cloned (AddRef) here.
        unsafe { ID3D11Device::from_raw_borrowed(&raw) }.expect("the renderer has a Direct3D 11 device").clone()
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
    Decoded { picture: output.picture, setup, rtp: 0, number: 0, receive_us: 0, enqueue_us: 0, input_us: 0, ready_us: 0, host_latency: 0, early: 0, first_block_us: 0, race: output.race, bands: output.bands }
}

fn gpu_decoder(rig: &Rig, case: &Case, slots: usize) -> Pyrowave {
    let config = GpuConfig { width: case.width as u32, height: case.height as u32, chroma444: case.full, ten_bit: case.ten, slots };
    let decoder = GpuDecoder::with_api(sdk(), &rig.device(), config).unwrap_or_else(|e| panic!("GPU decoder: {e}"));
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
/// RGB. 10-bit pictures meet the GPU-wavelet tests' own bounds (one 8-bit code, 0.001 rms). The SDK
/// writes 8-bit pictures as R8 codes where the CPU path keeps floats, so each plane is rounded by up
/// to half a code (0.29 code rms) and the decoders agree to 1.1 codes (`pyrowave_sdk` tests); through
/// the YCbCr matrix that is 2.1 codes at most and 0.002 rms on the fixtures, hence the 8-bit bounds.
fn bounds(ten: bool) -> (f32, f64) {
    if ten { (1.0 / 255.0, 0.001) } else { (2.5 / 255.0, 0.0025) }
}

/// The most the GPU picture may be brighter or darker than the CPU one on average (encoded RGB):
/// a quarter of a code. A plane scale that is off (a constant gain) or a rounding that all goes one
/// way shows here long before it shows in the largest difference.
const MEAN_LIMIT: f64 = 0.25 / 255.0;

/// The same on real 8-bit frames: the SDK stores 8-bit planes as R8 and the GPU pictures come out
/// 0.29 of a code darker than the CPU decoder's (measured, -1.12e-3 on every one of the 24 frames,
/// the same sign each time; a bias in the SDK's 8-bit plane store is likely, not isolated). A gain or offset error
/// of half a code or more still fails.
const REAL_MEAN_LIMIT: f64 = 0.4 / 255.0;

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

fn cpu_picture(rig: &Rig, case: &Case, data: &[u8], setup: VideoSetup, hdr: bool) -> Vec<f32> {
    let mut cpu = Pyrowave::cpu(case.width, case.height, case.full).unwrap();
    let output = cpu.frame(data, &mut || false).unwrap_or_else(|e| panic!("CPU decode: {e:?}"));
    let mut texture = VideoTexture::default();
    texture.set_hdr_mode(hdr, None);
    present(rig, &mut texture, &decoded(output, setup))
}

fn gpu_picture(rig: &Rig, gpu: &mut Pyrowave, data: &[u8], setup: VideoSetup, hdr: bool) -> Vec<f32> {
    let output = gpu.frame(data, &mut || false).unwrap_or_else(|e| panic!("GPU decode: {e:?}"));
    assert!(matches!(output.picture, Picture::GpuPlanes { .. }), "the GPU decoder makes plane pictures");
    let mut texture = VideoTexture::default();
    texture.set_hdr_mode(hdr, None);
    present(rig, &mut texture, &decoded(output, setup))
}

#[test]
#[ignore = "needs the PyroWave SDK, a GPU and a display"]
fn the_gpu_picture_matches_the_cpu_picture_on_the_fixtures() {
    let _lock = gpu_lock();
    let rig = Rig::new();
    for (n, case) in cases().iter().enumerate() {
        let mut gpu = gpu_decoder(&rig, case, 2);
        for range in [0, 1] {
            let setup = setup(case.width, case.height, case.full, case.ten, range);
            let (max, rms, mean) = compare(&cpu_picture(&rig, case, case.bin, setup, false), &gpu_picture(&rig, &mut gpu, case.bin, setup, false));
            println!("fixture {n} ({}x{} 444={} 10-bit={} range={range}): max {:.5} ({:.2} codes), rms {rms:.6}, mean {:+.2e} (encoded RGB, GPU against CPU)", case.width, case.height, case.full, case.ten, max, max * 255.0, mean);
            let (limit, rms_limit) = bounds(case.ten);
            assert!(max < limit && rms < rms_limit, "fixture {n} range {range}: GPU picture differs from the CPU picture: max {max} (limit {limit}), rms {rms} (limit {rms_limit})");
            assert!(mean.abs() < MEAN_LIMIT, "fixture {n} range {range}: the GPU picture is {mean:+.2e} off the CPU picture on average (limit {MEAN_LIMIT:.2e})");
        }
    }
}

#[test]
#[ignore = "needs the PyroWave SDK, a GPU, a display and NIGHTJAR_PYROWAVE_FRAMES"]
fn the_gpu_picture_matches_the_cpu_picture_on_dumped_real_frames() {
    let Some(dir) = std::env::var_os("NIGHTJAR_PYROWAVE_FRAMES") else { panic!("set NIGHTJAR_PYROWAVE_FRAMES to a NIGHTJAR_PYROWAVE_DUMP directory") };
    let _lock = gpu_lock();
    let frames = super::tests::dumped_frames(std::path::Path::new(&dir));
    assert!(!frames.is_empty(), "no frame-*.bin envelopes in {dir:?}");
    let (width, height, full, _) = frames[0];
    let rig = Rig::new();
    let case = Case { bin: &[], width, height, full, ten: false };
    let mut gpu = gpu_decoder(&rig, &case, 2);
    let mut cpu = Pyrowave::cpu(width, height, full).unwrap();
    let setup = setup(width, height, full, false, 0);
    let mut worst = (0.0f32, 0.0f64);
    let mut means = 0.0f64;
    let mut cpu_texture = VideoTexture::default();
    let mut gpu_texture = VideoTexture::default();
    for (n, (w, h, f, data)) in frames.iter().enumerate().take(24) {
        assert_eq!((*w, *h, *f), (width, height, full), "one geometry per dump");
        let cpu_out = cpu.frame(data, &mut || false).unwrap();
        let gpu_out = gpu.frame(data, &mut || false).unwrap();
        let a = present(&rig, &mut cpu_texture, &decoded(cpu_out, setup));
        let b = present(&rig, &mut gpu_texture, &decoded(gpu_out, setup));
        let (max, rms, mean) = compare(&a, &b);
        if n % 6 == 0 { println!("frame {n}: max {:.5} ({:.2} codes), rms {rms:.6}, mean {mean:+.2e}", max, max * 255.0); }
        worst = (worst.0.max(max), worst.1.max(rms));
        means += mean;
    }
    println!("{} real frames {width}x{height} 444={full}: worst max {:.5} ({:.2} codes), worst rms {:.6}", frames.len().min(24), worst.0, worst.0 * 255.0, worst.1);
    let mean = means / frames.len().min(24) as f64;
    assert!(mean.abs() < REAL_MEAN_LIMIT, "the GPU pictures are {mean:+.2e} off the CPU pictures on average over real frames (limit {REAL_MEAN_LIMIT:.2e})");
    let (limit, rms_limit) = bounds(false);
    assert!(worst.0 < limit && worst.1 < rms_limit, "GPU picture differs from the CPU picture on real frames: max {} (limit {limit}), rms {} (limit {rms_limit})", worst.0, worst.1);
}

use super::tests::{arrived, bitstream, envelope, fixture, host_packets, spanning_packets};

fn fixture_case(which: usize) -> Case {
    let (bin, width, height, full) = fixture(which);
    Case { bin, width, height, full, ten: which % 2 == 1 }
}

/// The picture the stream draws for one frame of `packets`: whole, or chunk by chunk first.
fn frame_picture(rig: &Rig, gpu: &mut Pyrowave, packets: &[(u32, Vec<u8>)], early: bool, case: &Case) -> (Vec<f32>, Source) {
    if early {
        for (i, (flags, payload)) in packets.iter().enumerate() { gpu.chunk(i == 0, *flags, payload); }
    }
    let output = gpu.frame(&envelope(packets), &mut || false).unwrap_or_else(|e| panic!("GPU decode: {e:?}"));
    let source = gpu.source();
    let setup = setup(case.width, case.height, case.full, case.ten, 0);
    let mut texture = VideoTexture::default();
    texture.set_hdr_mode(false, None);
    (present(rig, &mut texture, &decoded(output, setup)), source)
}

#[test]
#[ignore = "needs the PyroWave SDK, a GPU and a display"]
fn gpu_early_chunks_give_the_whole_frames_picture_with_and_without_lost_packets() {
    let _lock = gpu_lock();
    let rig = Rig::new();
    for which in 0..4 {
        let case = fixture_case(which);
        let bits = bitstream(case.bin);
        let whole_packets = host_packets(&bits);
        let spanning = spanning_packets(&bits, 300);
        let count = whole_packets.len();
        // Frames in sequence: a complete one, then ones that lack packets (their blocks stay as the
        // decoder had them) and one whose records span packets.
        let mut sequences = vec![vec![arrived(&whole_packets, &[]), arrived(&spanning, &[])]];
        for lost in [vec![1], vec![count - 1], (1..count - 1).step_by(2).collect::<Vec<_>>()] {
            sequences.push(vec![arrived(&whole_packets, &[]), arrived(&whole_packets, &lost)]);
        }
        for (s, frames) in sequences.iter().enumerate() {
            let mut whole = gpu_decoder(&rig, &case, 2);
            let mut early = gpu_decoder(&rig, &case, 2);
            for (n, packets) in frames.iter().enumerate() {
                let (a, source_a) = frame_picture(&rig, &mut whole, packets, false, &case);
                let (b, source_b) = frame_picture(&rig, &mut early, packets, true, &case);
                assert_eq!(source_a, Source::Frame);
                assert_eq!(source_b, Source::Early, "fixture {which} sequence {s} frame {n}: the chunks were used");
                assert!(a == b, "fixture {which} sequence {s} frame {n}: the early picture differs from the whole frame's");
            }
        }
    }
}

#[test]
#[ignore = "needs the PyroWave SDK, a GPU and a display"]
fn gpu_frames_that_do_not_decode_are_frame_failures_and_the_next_frame_decodes() {
    let _lock = gpu_lock();
    let rig = Rig::new();
    let case = fixture_case(0);
    let packets = arrived(&host_packets(&bitstream(case.bin)), &[]);
    let mut gpu = gpu_decoder(&rig, &case, 2);
    let (good, _) = frame_picture(&rig, &mut gpu, &packets, false, &case);

    // A sequence header that names another picture size, whole and early.
    let mut bad = packets.clone();
    bad[0].1[8] ^= 1;
    let whole = gpu.frame(&envelope(&bad), &mut || false);
    assert!(matches!(whole, Err(Failure::Frame(_))), "whole: {:?}", whole.err());
    for (i, (flags, payload)) in bad.iter().enumerate() { gpu.chunk(i == 0, *flags, payload); }
    let early = gpu.frame(&envelope(&bad), &mut || false);
    assert!(matches!(early, Err(Failure::Frame(_))), "early: {:?}", early.err());
    // A container that is not one, and a frame over the packet budget.
    for junk in [vec![1u8, 2, 3], vec![0u8; rubylight_protocol::pyrowave::MAX_FRAME_BYTES + 1]] {
        let result = gpu.frame(&junk, &mut || false);
        assert!(matches!(result, Err(Failure::Frame(_))), "junk of {} bytes: {:?}", junk.len(), result.err());
    }
    // Nothing was kept from the failures: the next frame is the picture it was before.
    let (after, source) = frame_picture(&rig, &mut gpu, &packets, false, &case);
    assert_eq!(source, Source::Frame);
    assert!(after == good, "a failed frame left state behind");
}

#[test]
#[ignore = "needs the PyroWave SDK, a GPU and a display"]
fn gpu_frames_wait_for_a_free_plane_set_by_dropping_the_oldest_picture() {
    let _lock = gpu_lock();
    let rig = Rig::new();
    let case = fixture_case(0);
    let packets = arrived(&host_packets(&bitstream(case.bin)), &[]);
    let data = envelope(&packets);
    let mut gpu = gpu_decoder(&rig, &case, 2);
    let Pyrowave::Gpu(stream) = &gpu else { unreachable!() };
    let planes = stream.planes().clone();
    assert_eq!(planes.set_count(), 2);
    let mut held = std::collections::VecDeque::new();
    for _ in 0..2 { held.push_back(gpu.frame(&data, &mut || false).unwrap()); }
    assert_eq!(planes.free_count(), 0, "both sets are held by pictures");

    // No picture to give up: the frame is dropped and the decoder keeps working afterwards.
    assert_eq!(gpu.frame(&data, &mut || false).err(), Some(Failure::NoPlanes));
    // The queue gives up its oldest picture: its set is decoded into at once.
    let mut evicted = 0;
    let newest = gpu.frame(&data, &mut || { evicted += 1; held.pop_front().is_some() }).unwrap();
    assert_eq!(evicted, 1);
    assert_eq!(planes.free_count(), 0);
    held.push_back(newest);

    // Showing a picture frees its set (the draw signals the reuse fence); the next decode waits
    // for that on the GPU, and every picture is the same one.
    let setup = setup(case.width, case.height, case.full, case.ten, 0);
    let mut texture = VideoTexture::default();
    texture.set_hdr_mode(false, None);
    let mut shown = Vec::new();
    for _ in 0..6 {
        let output = held.pop_front().unwrap();
        shown.push(present(&rig, &mut texture, &decoded(output, setup)));
        held.push_back(gpu.frame(&data, &mut || false).unwrap());
    }
    assert!(shown.windows(2).all(|pair| pair[0] == pair[1]), "a reused plane set changed the picture");
    drop(held);
    assert_eq!(planes.free_count(), 2, "dropped pictures give their sets back");
}

#[test]
#[ignore = "needs the PyroWave SDK, a GPU and a display"]
fn a_decode_that_never_finishes_is_a_loud_presenter_error() {
    let _lock = gpu_lock();
    let rig = Rig::new();
    let case = fixture_case(0);
    let packets = arrived(&host_packets(&bitstream(case.bin)), &[]);
    let mut gpu = gpu_decoder(&rig, &case, 2);
    let mut output = gpu.frame(&envelope(&packets), &mut || false).unwrap();
    // A fence value the decoder will never signal, as after a lost device.
    let Picture::GpuPlanes { fence_value, .. } = &mut output.picture else { panic!("a plane picture") };
    *fence_value += 1_000_000;
    let setup = setup(case.width, case.height, case.full, case.ten, 0);
    let mut texture = VideoTexture::default();
    texture.set_hdr_mode(false, None);
    let started = std::time::Instant::now();
    assert!(!texture.upload(rig.renderer, &decoded(output, setup)));
    let failure = texture.failure.take().expect("the presenter says why");
    assert!(failure.contains("PyroWave GPU decoding failed"), "{failure}");
    assert!(started.elapsed() < std::time::Duration::from_secs(5), "the wait has a deadline");
}

/// The codes of plane `plane` as the SDK wrote them (R16 UNORM), read back through a staging copy
/// once the decode is done.
fn read_plane16(rig: &Rig, gpu: &Pyrowave, lease: &crate::stream::gpu_planes::Lease, fence_value: u64, plane: usize) -> (usize, usize, Vec<u16>) {
    use windows::Win32::Graphics::Direct3D11::*;
    let Pyrowave::Gpu(stream) = gpu else { unreachable!() };
    stream.gpu.wait(fence_value, std::time::Duration::from_secs(5)).expect("the decode finishes");
    let device = rig.device();
    // SAFETY: Plain D3D11 calls on live objects; the mapped rows are read before `Unmap`.
    unsafe {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        lease.textures()[plane].GetDesc(&mut desc);
        desc.Usage = D3D11_USAGE_STAGING;
        desc.BindFlags = 0;
        desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
        desc.MiscFlags = 0;
        let mut staging = None;
        device.CreateTexture2D(&desc, None, Some(&mut staging)).unwrap();
        let staging = staging.unwrap();
        let context = device.GetImmediateContext().unwrap();
        context.cast::<ID3D11DeviceContext4>().unwrap().Wait(lease.decode_fence(), fence_value).unwrap();
        context.CopyResource(&staging, &lease.textures()[plane]);
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped)).unwrap();
        let (width, height) = (desc.Width as usize, desc.Height as usize);
        let mut codes = Vec::with_capacity(width * height);
        for y in 0..height {
            codes.extend_from_slice(std::slice::from_raw_parts((mapped.pData as *const u8).add(y * mapped.RowPitch as usize).cast::<u16>(), width));
        }
        context.Unmap(&staging, 0);
        (width, height, codes)
    }
}

#[test]
#[ignore = "needs the PyroWave SDK, a GPU and a display"]
fn ten_bit_planes_are_unorm16_of_the_code_over_1023_not_the_code_shifted() {
    let _lock = gpu_lock();
    let rig = Rig::new();
    for which in [1, 3] {
        let case = fixture_case(which);
        assert!(case.ten);
        let mut gpu = gpu_decoder(&rig, &case, 2);
        let output = gpu.frame(case.bin, &mut || false).unwrap();
        let Picture::GpuPlanes { lease, fence_value } = &output.picture else { panic!("a plane picture") };
        let mut cpu = Decoder::new(case.width, case.height, case.full).unwrap();
        cpu.decode(case.bin).unwrap();
        // The luma plane: the SDK's UNORM16 value is the CPU decoder's sample (code/1023 in 0..1).
        let (width, height, planes) = read_plane16(&rig, &gpu, lease, *fence_value, 0);
        assert!(width >= case.width && height >= case.height);
        let (mut sum, mut worst, mut n, mut top) = (0.0f64, 0.0f64, 0usize, 0.0f64);
        for y in 0..case.height {
            for x in 0..case.width {
                let sdk_code = f64::from(planes[y * width + x]) / 65535.0 * 1023.0;
                let cpu_code = f64::from(cpu.sample(0, x, y)) * 1023.0;
                sum += sdk_code - cpu_code;
                worst = worst.max((sdk_code - cpu_code).abs());
                top = top.max(sdk_code);
                n += 1;
            }
        }
        let mean = sum / n as f64;
        println!("fixture {which}: luma codes, SDK minus CPU decoder: mean {mean:+.3}, worst {worst:.3}, brightest {top:.1}");
        // Shifting the code left by six instead (code * 64 of 65535) would be 0.1 % darker: half a
        // code on average over these ramps, and a missing code at the top.
        assert!(mean.abs() < 0.15, "fixture {which}: the SDK's luma is {mean:+.3} codes off the CPU decoder's on average");
        assert!(worst < 1.6, "fixture {which}: worst luma difference {worst:.3} codes");
        assert!(top > 1000.0, "fixture {which}: the fixture should reach the top of the range (brightest {top:.1})");
    }
}

#[test]
#[ignore = "needs the PyroWave SDK, a GPU and a display"]
fn a_pacer_backlog_never_starves_the_decoder() {
    use crate::stream::pipeline::Shared;
    use crate::stream::present::Pending;
    let _lock = gpu_lock();
    let rig = Rig::new();
    let case = fixture_case(0);
    let data = envelope(&arrived(&host_packets(&bitstream(case.bin)), &[]));
    let setup = setup(case.width, case.height, case.full, case.ten, 0);
    let mut gpu = gpu_decoder(&rig, &case, 2);
    let shared = Shared::new(std::sync::Arc::new(|| {}), 8);
    // HostTimes: the presenter's queue is empty and both plane sets wait in the pacer.
    for n in 0..2 {
        let output = gpu.frame(&data, &mut || false).unwrap();
        crate::stream::pipeline::lock(&shared.pacer_backlog).push_back(Pending { decoded: decoded(output, setup), present_us: n, hold_us: 0, late: false });
    }
    assert!(crate::stream::pipeline::lock(&shared.frames).is_empty());
    let output = gpu.frame(&data, &mut || shared.evict_oldest()).unwrap_or_else(|e| panic!("the backlog starved the decoder: {e:?}"));
    let left: Vec<u64> = crate::stream::pipeline::lock(&shared.pacer_backlog).iter().map(|p| p.present_us).collect();
    assert_eq!(left, [1], "the oldest queued picture went");
    drop(output);
}

#[test]
#[ignore = "needs the PyroWave SDK, a GPU and a display"]
fn a_failed_signal_still_gives_the_plane_set_back() {
    let _lock = gpu_lock();
    let rig = Rig::new();
    let case = fixture_case(0);
    let data = envelope(&arrived(&host_packets(&bitstream(case.bin)), &[]));
    let mut gpu = gpu_decoder(&rig, &case, 2);
    let Pyrowave::Gpu(stream) = &gpu else { unreachable!() };
    let planes = stream.planes().clone();
    let output = gpu.frame(&data, &mut || false).unwrap();
    let Picture::GpuPlanes { lease, .. } = &output.picture else { panic!("a plane picture") };
    assert_eq!(planes.free_count(), 1);
    let value = planes.next_reuse_value();
    let result = crate::stream::d3d11::hand_back(lease, value, || Err("cannot release the PyroWave planes: device removed".into()));
    assert!(result.unwrap_err().contains("device removed"), "the error still reaches the caller");
    assert_eq!(planes.free_count(), 2, "the set is not held by a failed draw");
    drop(output);
    assert_eq!(planes.free_count(), 2, "and it is not given back twice");
}

#[test]
#[ignore = "needs the PyroWave SDK, a GPU and a display"]
fn a_new_decoders_first_draw_lets_go_of_the_old_decoders_planes() {
    let _lock = gpu_lock();
    let rig = Rig::new();
    let case = fixture_case(0);
    let data = envelope(&arrived(&host_packets(&bitstream(case.bin)), &[]));
    let setup = setup(case.width, case.height, case.full, case.ten, 0);
    let mut texture = VideoTexture::default();
    texture.set_hdr_mode(false, None);
    let mut first = gpu_decoder(&rig, &case, 2);
    let Pyrowave::Gpu(stream) = &first else { unreachable!() };
    let old = stream.planes().clone();
    present(&rig, &mut texture, &decoded(first.frame(&data, &mut || false).unwrap(), setup));
    assert_eq!(std::sync::Arc::strong_count(&old), 3, "the decoder, this test and the pass that drew from it");
    // The same picture format again, from a new decoder (a restarted stream): the pass keeps its
    // shader state but must not keep the old sets alive.
    let mut second = gpu_decoder(&rig, &case, 2);
    present(&rig, &mut texture, &decoded(second.frame(&data, &mut || false).unwrap(), setup));
    assert_eq!(std::sync::Arc::strong_count(&old), 2, "only the decoder and this test hold the old sets now");
}

/// The bytes of plane `plane` as the SDK wrote them (any of its formats: R8 or R16 UNORM), rows
/// without padding, read back through a staging copy once `fence_value` is signalled.
fn read_plane_bytes(rig: &Rig, gpu: &GpuDecoder, lease: &crate::stream::gpu_planes::Lease, fence_value: u64, plane: usize) -> (usize, Vec<u8>) {
    use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_R16_UNORM, DXGI_FORMAT_R8_UNORM};
    use windows::Win32::Graphics::Direct3D11::*;
    gpu.wait(fence_value, std::time::Duration::from_secs(5)).expect("the decode finishes");
    let device = rig.device();
    // SAFETY: Plain D3D11 calls on live objects; the mapped rows are read before `Unmap`.
    unsafe {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        lease.textures()[plane].GetDesc(&mut desc);
        let texel = match desc.Format { DXGI_FORMAT_R8_UNORM => 1, DXGI_FORMAT_R16_UNORM => 2, other => panic!("unexpected plane format {other:?}") };
        desc.Usage = D3D11_USAGE_STAGING;
        desc.BindFlags = 0;
        desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
        desc.MiscFlags = 0;
        let mut staging = None;
        device.CreateTexture2D(&desc, None, Some(&mut staging)).unwrap();
        let staging = staging.unwrap();
        let context = device.GetImmediateContext().unwrap();
        context.cast::<ID3D11DeviceContext4>().unwrap().Wait(lease.decode_fence(), fence_value).unwrap();
        context.CopyResource(&staging, &lease.textures()[plane]);
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped)).unwrap();
        let row = desc.Width as usize * texel;
        let mut bytes = Vec::with_capacity(row * desc.Height as usize);
        for y in 0..desc.Height as usize {
            bytes.extend_from_slice(std::slice::from_raw_parts((mapped.pData as *const u8).add(y * mapped.RowPitch as usize), row));
        }
        context.Unmap(&staging, 0);
        (row, bytes)
    }
}

/// The packets of a records envelope as (flags, payload).
fn envelope_packets(data: &[u8]) -> Vec<(u32, Vec<u8>)> {
    let mut packets = Vec::new();
    let mut at = 4;
    while at + 8 <= data.len() {
        let length = u32::from_le_bytes(data[at..at + 4].try_into().unwrap()) as usize;
        let flags = u32::from_le_bytes(data[at + 4..at + 8].try_into().unwrap());
        packets.push((flags, data[at + 8..at + 8 + length].to_vec()));
        at += 8 + length;
    }
    packets
}

#[test]
#[ignore = "needs the PyroWave SDK, a GPU, a display and row-ordered frames in NIGHTJAR_PYROWAVE_FRAMES"]
fn a_slice_decode_is_exact_on_its_rows_on_real_row_ordered_frames() {
    let Some(dir) = std::env::var_os("NIGHTJAR_PYROWAVE_FRAMES") else { panic!("set NIGHTJAR_PYROWAVE_FRAMES to a NIGHTJAR_PYROWAVE_DUMP directory of a row-ordered stream") };
    let _lock = gpu_lock();
    let frames = super::tests::dumped_frames(std::path::Path::new(&dir));
    assert!(!frames.is_empty(), "no frame-*.bin envelopes in {dir:?}");
    let (width, height, full, _) = frames[0];
    let rig = Rig::new();
    let case = Case { bin: &[], width, height, full, ten: false };
    let mut gpu = gpu_decoder(&rig, &case, 4);
    let Pyrowave::Gpu(stream) = &mut gpu else { unreachable!() };
    stream.set_slices(4);
    let (mut slices, mut sliced_frames) = (0, 0);
    for (n, (_, _, _, data)) in frames.iter().enumerate().take(24) {
        // Each slice's planes, read the moment its decode has finished (the final decode then
        // overwrites them): chunk by chunk, as the stream feeds the decoder.
        type Planes = Vec<(usize, Vec<u8>)>;
        let mut captured: Vec<(BandReady, Planes)> = Vec::new();
        for (i, (flags, payload)) in envelope_packets(data).iter().enumerate() {
            stream.chunk(i == 0, *flags, payload);
            let race = stream.race.as_ref().expect("the beam race is on");
            while captured.len() < race.bands.len() {
                let band = race.bands[captured.len()];
                let (lease, _) = race.lease.as_ref().expect("the slices decode into a plane set");
                captured.push((band, (0..3).map(|p| read_plane_bytes(&rig, &stream.gpu, lease, band.fence_value, p)).collect()));
            }
        }
        let output = stream.frame(data, &mut || false).unwrap_or_else(|e| panic!("GPU decode: {e:?}"));
        let Picture::GpuPlanes { lease, fence_value } = &output.picture else { panic!("a plane picture") };
        let last: Vec<_> = (0..3).map(|p| read_plane_bytes(&rig, &stream.gpu, lease, *fence_value, p)).collect();
        assert_eq!(output.race.ordered, RaceReport::ORDERED, "frame {n} is not in row order: is this a row-ordered dump?");
        assert_eq!(output.bands.len(), captured.len() + 1, "frame {n}: the final decode is the last slice");
        assert_eq!(output.race.slices as usize, output.bands.len());
        sliced_frames += usize::from(!captured.is_empty());
        // The reference: the same frame decoded once, whole, with the beam race off.
        stream.set_slices(0);
        let whole = stream.frame(data, &mut || false).unwrap_or_else(|e| panic!("GPU decode (whole): {e:?}"));
        let Picture::GpuPlanes { lease: whole_lease, fence_value: whole_fence } = &whole.picture else { panic!("a plane picture") };
        let reference: Vec<_> = (0..3).map(|p| read_plane_bytes(&rig, &stream.gpu, whole_lease, *whole_fence, p)).collect();
        stream.set_slices(4);
        for plane in 0..3 {
            assert!(last[plane].1 == reference[plane].1, "frame {n} plane {plane}: the final decode after its slices differs from the whole frame's decode");
        }
        for (band, planes) in &captured {
            for (plane, (row, bytes)) in planes.iter().enumerate() {
                let scale = if plane == 0 || full { 1 } else { 2 };
                let (first, end) = (band.first_row as usize / scale, band.end_row as usize / scale);
                assert!(bytes[first * row..end * row] == reference[plane].1[first * row..end * row],
                    "frame {n} slice {} plane {plane}: rows {first}..{end} differ between the slice decode and the whole frame's decode", band.slice);
            }
            slices += 1;
        }
    }
    println!("{sliced_frames} frames with {slices} slice decodes, every one exact on its rows against the whole decode, and the final decodes byte-identical ({width}x{height} 444={full})");
    assert!(slices > 0, "no slice came due: are the dumped frames row-ordered?");
}
