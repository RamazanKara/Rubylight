//! Unit tests (no GPU) and `--ignored` GPU tests for the SDK binding.
//!
//! The GPU tests need the built SDK (scripts/build-pyrowave-sdk.ps1): they look in
//! `%NIGHTJAR_PYROWAVE_SDK%`, else `C:\src\butterpollo-release\nj-pyrowave-sdk`. Run them with
//! `cargo test -p nightjar-desktop --release --bin nightjar -- --ignored --nocapture --test-threads=1 pyrowave_sdk`.
//! They share one Vulkan device per decoder life, so a hung GPU shows as a failed test and never retries.

use super::*;
use crate::stream::pyrowave::Decoder as Cpu;
use rubylight_protocol::pyrowave::{push_container, Records, PACKET_CONTIGUOUS, PACKET_FRAME_END, PACKET_FRAME_START, PACKET_RECORD_START};
use std::collections::HashSet;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

const HEADER_BIT: u32 = 0x8000_0000;

fn word(data: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(data[at..at + 4].try_into().unwrap())
}

// The C layout of pyrowave.h 1.0, printed by gcc (sizeof and offsetof on the header) when the
// binding was written.
#[test]
fn structs_have_the_layout_of_the_c_header() {
    use std::mem::{offset_of, size_of};
    assert_eq!(size_of::<DecoderCreateInfo>(), 24);
    assert_eq!((offset_of!(DecoderCreateInfo, width), offset_of!(DecoderCreateInfo, chroma), offset_of!(DecoderCreateInfo, fragment_path)), (8, 16, 20));
    assert_eq!(size_of::<SyncObjectCreateInfo>(), 32);
    assert_eq!(
        (offset_of!(SyncObjectCreateInfo, external_handle), offset_of!(SyncObjectCreateInfo, handle_type), offset_of!(SyncObjectCreateInfo, semaphore_type), offset_of!(SyncObjectCreateInfo, import_flags)),
        (8, 16, 20, 24)
    );
    assert_eq!(size_of::<ImageCreateInfo>(), 32);
    assert_eq!((offset_of!(ImageCreateInfo, handle_type), offset_of!(ImageCreateInfo, image_create_info)), (16, 24));
    assert_eq!(size_of::<ImageView>(), 48);
    assert_eq!(
        (offset_of!(ImageView, width), offset_of!(ImageView, image_format), offset_of!(ImageView, aspect), offset_of!(ImageView, swizzle), offset_of!(ImageView, layout)),
        (8, 16, 32, 36, 40)
    );
    assert_eq!(size_of::<GpuBuffers>(), 144);
    assert_eq!((size_of::<GpuExternalReference>(), offset_of!(GpuExternalReference, queue_family_index)), (16, 8));
    assert_eq!((size_of::<SyncPoint>(), offset_of!(SyncPoint, value)), (16, 8));
    assert_eq!((size_of::<GpuSyncOperation>(), offset_of!(GpuSyncOperation, num_images), offset_of!(GpuSyncOperation, sync)), (32, 8, 16));
}

#[test]
fn results_and_the_abi_rule_have_readable_errors() {
    assert_eq!(result_name(0), "success");
    assert_eq!(result_name(1), "timeout");
    assert_eq!(result_name(-7), "unsupported external handle");
    assert_eq!(result_name(-99), "unknown result");
    assert_eq!(check(0, "x"), Ok(()));
    assert_eq!(check(-2, "pushing a PyroWave packet"), Err("pushing a PyroWave packet: PyroWave error -2 (invalid argument)".to_owned()));
    assert_eq!(check_abi(1, 0, 0), Ok(()));
    assert_eq!(check_abi(1, 7, 3), Ok(()));
    assert_eq!(check_abi(2, 0, 0), Err("PyroWave SDK ABI 2.0.0 is not supported, this client needs 1.x".to_owned()));
    assert_eq!(check_abi(0, 9, 1), Err("PyroWave SDK ABI 0.9.1 is not supported, this client needs 1.x".to_owned()));
}

#[test]
fn a_missing_dll_names_the_file_and_where_it_was_expected() {
    let missing = std::env::temp_dir().join("nightjar-no-such-dir").join(DLL_NAME);
    let error = probe_in(&missing, None).unwrap_err();
    assert!(error.starts_with("libpyrowave-shared-1.dll not found next to the executable ("), "{error}");
    assert!(error.contains("nightjar-no-such-dir"), "{error}");
    assert_eq!(Api::load_from(&missing).err(), Some(error));
}

#[test]
fn a_dll_that_is_no_pyrowave_sdk_is_named_as_such() {
    let kernel32 = Path::new(r"C:\Windows\System32\kernel32.dll");
    let error = probe_in(kernel32, None).unwrap_err();
    assert!(error.contains("kernel32.dll is not a PyroWave SDK: pyrowave_get_api_version is missing"), "{error}");
}

#[test]
fn a_dll_with_another_abi_is_refused_with_its_version() {
    // A stand-in that reports 2.0.0 and nothing else; the version is read before any other symbol.
    let dir = std::env::temp_dir().join(format!("nightjar-abi-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let source = dir.join("fake.c");
    std::fs::write(&source, "__declspec(dllexport) void pyrowave_get_api_version(unsigned *a, unsigned *b, unsigned *c) { *a = 2; *b = 1; *c = 4; }\n").unwrap();
    let dll = dir.join("fake.dll");
    let built = std::process::Command::new("gcc").arg("-shared").arg("-o").arg(&dll).arg(&source).status();
    assert!(built.is_ok_and(|s| s.success()), "gcc (MSYS2 UCRT64) is needed on PATH to build the stand-in DLL");
    let error = probe_in(&dll, None).unwrap_err();
    assert_eq!(error, "PyroWave SDK ABI 2.1.4 is not supported, this client needs 1.x");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn plane_sizes_follow_the_chroma_format() {
    let config = |chroma444| GpuConfig { width: 132, height: 98, chroma444, ten_bit: false, slots: 1 };
    assert_eq!([0, 1, 2].map(|p| config(false).plane_size(p)), [(132, 98), (66, 49), (66, 49)]);
    assert_eq!([0, 1, 2].map(|p| config(true).plane_size(p)), [(132, 98); 3]);
}

// ---- GPU tests -------------------------------------------------------------------------------

/// One GPU test at a time: they each make Vulkan devices on the same adapter.
static GPU: Mutex<()> = Mutex::new(());

fn gpu_lock() -> MutexGuard<'static, ()> {
    GPU.lock().unwrap_or_else(|e| e.into_inner())
}

fn sdk_dll() -> PathBuf {
    let dir = std::env::var_os("NIGHTJAR_PYROWAVE_SDK").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\src\butterpollo-release\nj-pyrowave-sdk"));
    dir.join(DLL_NAME)
}

fn sdk() -> Arc<Api> {
    Api::load_from(&sdk_dll()).unwrap_or_else(|e| panic!("{e}\nbuild it with scripts/build-pyrowave-sdk.ps1"))
}

struct Fixture {
    bin: &'static [u8],
    yuv: &'static [u8],
    width: u32,
    height: u32,
    full: bool,
    ten: bool,
}

impl Fixture {
    fn config(&self) -> GpuConfig {
        GpuConfig { width: self.width, height: self.height, chroma444: self.full, ten_bit: self.ten, slots: 1 }
    }

    /// The packets the SDK is given: the sequence header, then the blocks.
    fn packets(&self) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        assert!(push_container(self.bin, |packet| { out.push(packet.to_vec()); true }));
        out
    }
}

fn fixtures() -> [Fixture; 4] {
    let f = |bin, yuv, width, height, full, ten| Fixture { bin, yuv, width, height, full, ten };
    [
        f(&include_bytes!("../../../tests/fixtures/pyrowave-420-8.bin")[..], &include_bytes!("../../../tests/fixtures/pyrowave-420-8.yuv")[..], 132, 98, false, false),
        f(&include_bytes!("../../../tests/fixtures/pyrowave-420-10.bin")[..], &include_bytes!("../../../tests/fixtures/pyrowave-420-10.yuv")[..], 132, 98, false, true),
        f(&include_bytes!("../../../tests/fixtures/pyrowave-444-8.bin")[..], &include_bytes!("../../../tests/fixtures/pyrowave-444-8.yuv")[..], 131, 97, true, false),
        f(&include_bytes!("../../../tests/fixtures/pyrowave-444-10.bin")[..], &include_bytes!("../../../tests/fixtures/pyrowave-444-10.yuv")[..], 131, 97, true, true),
    ]
}

/// Copies decoded planes to the CPU for comparison.
struct Readback {
    context: ID3D11DeviceContext,
    context4: ID3D11DeviceContext4,
    staging: Vec<ID3D11Texture2D>,
    config: GpuConfig,
}

impl Readback {
    fn new(d3d: &ID3D11Device, config: GpuConfig) -> Self {
        // SAFETY: Plain D3D11 calls with live locals.
        let (context, staging) = unsafe {
            let context = d3d.GetImmediateContext().unwrap();
            let staging: Vec<ID3D11Texture2D> = (0..3)
                .map(|plane| {
                    let (width, height) = config.plane_size(plane);
                    let mut texture = None;
                    d3d.CreateTexture2D(
                        &D3D11_TEXTURE2D_DESC {
                            Width: width,
                            Height: height,
                            MipLevels: 1,
                            ArraySize: 1,
                            Format: if config.ten_bit { DXGI_FORMAT_R16_UNORM } else { DXGI_FORMAT_R8_UNORM },
                            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                            Usage: D3D11_USAGE_STAGING,
                            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                            ..Default::default()
                        },
                        None,
                        Some(&mut texture),
                    )
                    .unwrap();
                    texture.unwrap()
                })
                .collect();
            (context, staging)
        };
        Self { context4: context.cast().expect("ID3D11DeviceContext4"), context, staging, config }
    }

    /// Waits for fence `value` on the CPU and then on the D3D11 queue (the way the presenter will),
    /// and returns the raw sample codes of `slot`'s planes.
    fn planes(&self, gpu: &GpuDecoder, slot: usize, value: u64) -> [Vec<u16>; 3] {
        gpu.wait(value, Duration::from_secs(10)).unwrap();
        // SAFETY: Plain D3D11 calls; the mapped rows are read before Unmap.
        unsafe {
            self.context4.Wait(gpu.fence(), value).unwrap();
            let bytes = if self.config.ten_bit { 2 } else { 1 };
            std::array::from_fn(|plane| {
                let (width, height) = self.config.plane_size(plane);
                let (width, height) = (width as usize, height as usize);
                self.context.CopyResource(&self.staging[plane], &gpu.planes(slot)[plane]);
                let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
                self.context.Map(&self.staging[plane], 0, D3D11_MAP_READ, 0, Some(&mut mapped)).unwrap();
                let mut out = Vec::with_capacity(width * height);
                for y in 0..height {
                    let row = std::slice::from_raw_parts(mapped.pData.cast::<u8>().add(y * mapped.RowPitch as usize), width * bytes);
                    out.extend(row.chunks(bytes).map(|c| if bytes == 2 { u16::from_le_bytes([c[0], c[1]]) } else { u16::from(c[0]) }));
                }
                self.context.Unmap(&self.staging[plane], 0);
                out
            })
        }
    }
}

/// A sample code as a value on the picture's 8 or 10-bit scale (R16 holds 10-bit pictures as UNORM16).
fn code(raw: u16, ten: bool) -> f32 {
    if ten { f32::from(raw) * 1023.0 / 65535.0 } else { f32::from(raw) }
}

fn push_and_decode(gpu: &mut GpuDecoder, packets: &[Vec<u8>]) -> u64 {
    gpu.clear();
    for packet in packets {
        gpu.push_packet(packet).unwrap();
    }
    assert!(gpu.is_ready(false), "the pushed packets did not complete the frame");
    gpu.decode(0).unwrap()
}

/// The largest difference, in code values, between the GPU planes and the CPU decoder's picture.
fn max_error_vs_cpu(planes: &[Vec<u16>; 3], cpu: &Cpu, config: GpuConfig) -> f32 {
    let max = if config.ten_bit { 1023.0 } else { 255.0 };
    let mut error = 0.0f32;
    for (p, samples) in planes.iter().enumerate() {
        let shift = usize::from(p != 0 && !config.chroma444);
        let (width, height) = config.plane_size(p);
        for y in 0..height as usize {
            for x in 0..width as usize {
                let actual = code(samples[y * width as usize + x], config.ten_bit);
                error = error.max((actual - cpu.sample(p, x << shift, y << shift) * max).abs());
            }
        }
    }
    error
}

#[test]
#[ignore = "needs the PyroWave SDK and a GPU"]
fn fixtures_decode_on_the_gpu_like_the_reference_and_the_cpu_decoder() {
    let _gpu = gpu_lock();
    let api = sdk();
    let d3d = default_device().unwrap();
    for (n, fixture) in fixtures().iter().enumerate() {
        let config = fixture.config();
        let mut gpu = GpuDecoder::with_api(api.clone(), &d3d, config).unwrap();
        let readback = Readback::new(&d3d, config);
        let value = push_and_decode(&mut gpu, &fixture.packets());
        let planes = readback.planes(&gpu, 0, value);

        let mut offset = 0;
        let mut golden = 0.0f32;
        for (p, samples) in planes.iter().enumerate() {
            let (width, height) = config.plane_size(p);
            assert_eq!(samples.len(), (width * height) as usize);
            for raw in samples {
                let expected = if fixture.ten { let v = u16::from_le_bytes([fixture.yuv[offset], fixture.yuv[offset + 1]]); offset += 2; v } else { let v = u16::from(fixture.yuv[offset]); offset += 1; v };
                golden = golden.max((code(*raw, fixture.ten) - f32::from(expected)).abs());
            }
        }
        assert_eq!(offset, fixture.yuv.len());

        let mut cpu = Cpu::new(fixture.width as usize, fixture.height as usize, fixture.full).unwrap();
        cpu.decode(fixture.bin).unwrap();
        let against_cpu = max_error_vs_cpu(&planes, &cpu, config);
        println!("fixture {n} ({}x{} 444={} 10-bit={}): max error {golden:.3} against the .yuv, {against_cpu:.3} against the CPU decoder", fixture.width, fixture.height, fixture.full, fixture.ten);
        assert!(golden <= 1.0, "fixture {n}: {golden} code values from the reference");
        assert!(against_cpu < 1.1, "fixture {n}: {against_cpu} code values from the CPU decoder");
    }
}

// Packet builders for the lossy variants, as the CPU decoder's tests make them.
fn bitstream(container: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    assert!(push_container(container, |packet| { out.extend_from_slice(packet); true }));
    out
}

/// The blocks packed whole into 1400-byte host packets, as (flags, payload).
fn host_packets(bitstream: &[u8]) -> Vec<(u32, Vec<u8>)> {
    let mut payloads: Vec<Vec<u8>> = vec![Vec::new()];
    let mut at = 8;
    while at < bitstream.len() {
        let length = ((word(bitstream, at) >> 16) & 0xfff) as usize * 4;
        let limit = if payloads.len() == 1 { 1400 - 16 } else { 1400 };
        assert!(length <= limit, "the fixture has a record larger than a packet");
        if payloads.last().unwrap().len() + length > limit {
            payloads.push(Vec::new());
        }
        payloads.last_mut().unwrap().extend_from_slice(&bitstream[at..at + length]);
        at += length;
    }
    let last = payloads.last().unwrap().len() + if payloads.len() == 1 { 16 } else { 0 };
    let mut first = vec![1, 0, 0, 0, last as u8, (last >> 8) as u8, 0, 0];
    first.extend_from_slice(&bitstream[..8]);
    first.extend_from_slice(&payloads[0]);
    payloads[0] = first;
    let count = payloads.len();
    payloads
        .into_iter()
        .enumerate()
        .map(|(i, payload)| {
            let mut flags = PACKET_RECORD_START;
            if i == 0 {
                flags |= PACKET_FRAME_START;
            }
            if i == count - 1 {
                flags |= PACKET_FRAME_END;
            }
            (flags, payload)
        })
        .collect()
}

/// The packets that arrived, in order, the one after a gap not contiguous.
fn arrived(packets: &[(u32, Vec<u8>)], lost: &[usize]) -> Vec<(u32, Vec<u8>)> {
    let mut out = Vec::new();
    let mut previous = None;
    for (i, (flags, payload)) in packets.iter().enumerate() {
        if lost.contains(&i) {
            continue;
        }
        let contiguous = if previous == Some(i.wrapping_sub(1)) { PACKET_CONTIGUOUS } else { 0 };
        out.push((flags | contiguous, payload.clone()));
        previous = Some(i);
    }
    out
}

/// The client's frame data for those packets.
fn envelope(packets: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let mut out = vec![0; 4];
    for (flags, payload) in packets {
        out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        out.extend_from_slice(&flags.to_le_bytes());
        out.extend_from_slice(payload);
    }
    out
}

/// Frames with lost packets, decoded three ways, must give one picture: the CPU decoder, the SDK fed
/// by `Records::push_frame` once the frame is complete, and the SDK fed record by record as they
/// arrive (`feed_packet` and `drain_new`), the way G2 will, with the recovered rest at the end.
#[test]
#[ignore = "needs the PyroWave SDK and a GPU"]
fn lossy_frames_decode_on_the_gpu_like_the_cpu_decoder_whole_and_early() {
    let _gpu = gpu_lock();
    let api = sdk();
    let d3d = default_device().unwrap();
    let fixtures = fixtures();
    let (older, newer) = (&fixtures[0], &fixtures[1]);
    // Both are 132x98 4:2:0; the bit depth is the output's, so both decode to 8-bit planes here.
    let config = GpuConfig { ten_bit: false, ..older.config() };
    let first = host_packets(&bitstream(older.bin));
    let second = host_packets(&bitstream(newer.bin));
    let count = second.len();
    let readback = Readback::new(&d3d, config);
    let mut gpu = GpuDecoder::with_api(api, &d3d, config).unwrap();
    for lost in [vec![], vec![1], vec![1, count - 2], vec![count - 1], (1..count - 1).step_by(2).collect()] {
        let frames = [arrived(&first, &[]), arrived(&second, &lost)];
        let mut cpu = Cpu::new(132, 98, false).unwrap();
        let mut whole = Records::with_geometry(132, 98, false);
        let mut early = Records::with_geometry(132, 98, false);
        for (n, packets) in frames.iter().enumerate() {
            let data = envelope(packets);
            cpu.decode(&data).unwrap();

            gpu.clear();
            assert!(whole.push_frame(&data, |packet| gpu.push_packet(packet).is_ok()));
            assert!(gpu.is_ready(false));
            let value = gpu.decode(0).unwrap();
            let whole_error = max_error_vs_cpu(&readback.planes(&gpu, 0, value), &cpu, config);

            gpu.clear();
            let mut pushed = HashSet::new();
            early.begin_frame();
            for (flags, payload) in packets {
                assert!(early.feed_packet(*flags, payload));
                early.drain_new(|index, record| {
                    gpu.push_packet(record).unwrap();
                    assert!(pushed.insert(index), "record {index} twice");
                });
            }
            let early_pushed = pushed.len();
            assert!(early.finish_frame(|packet| {
                if word(packet, 0) & HEADER_BIT != 0 || !pushed.contains(&(word(packet, 4) >> 8)) {
                    gpu.push_packet(packet).unwrap();
                }
                true
            }));
            assert!(gpu.is_ready(false));
            let value = gpu.decode(0).unwrap();
            let early_error = max_error_vs_cpu(&readback.planes(&gpu, 0, value), &cpu, config);
            println!("lost {lost:?}, frame {n}: {early_pushed} records pushed early; max error {whole_error:.3} whole, {early_error:.3} early (code values from the CPU decoder)");
            assert!(whole_error < 1.1 && early_error < 1.1, "lost {lost:?}, frame {n}: {whole_error} whole, {early_error} early");
        }
    }
}

#[test]
#[ignore = "needs the PyroWave SDK and a GPU"]
fn decoder_lives_leave_a_few_handles_each() {
    use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessHandleCount};
    let _gpu = gpu_lock();
    let api = sdk();
    let d3d = default_device().unwrap();
    let fixture = &fixtures()[0];
    let packets = fixture.packets();
    let handles = || {
        let mut count = 0;
        // SAFETY: The out-pointer is a live local.
        unsafe { GetProcessHandleCount(GetCurrentProcess(), &mut count) }.unwrap();
        count
    };
    let life = || {
        let mut gpu = GpuDecoder::with_api(api.clone(), &d3d, fixture.config()).unwrap();
        let value = push_and_decode(&mut gpu, &packets);
        gpu.wait(value, Duration::from_secs(10)).unwrap();
    };
    // The loader, the driver and the SDK's thread pool settle over the first lives.
    for _ in 0..10 {
        life();
    }
    let before = handles();
    for _ in 0..10 {
        life();
    }
    let growth = i64::from(handles()) - i64::from(before);
    println!("10 decoder lives: {before} -> {} handles ({growth:+})", handles());
    assert!(growth <= 4 * 10 + 4, "10 lives left {growth} handles");
}

/// The blocks in a frame (the CPU decoder's band geometry): the indices a record can carry.
fn block_count(width: usize, height: usize, full: bool) -> usize {
    let stride = width.div_ceil(32).max(4) * 32;
    let rows = height.div_ceil(32).max(4) * 32;
    let mut count = 0;
    for level in (0..5).rev() {
        for component in 0..3 {
            if level == 0 && component != 0 && !full {
                continue;
            }
            let (w, h) = (stride >> (level + 1), rows >> (level + 1));
            count += (if level == 4 { 4 } else { 3 }) * w.div_ceil(32) * h.div_ceil(32);
        }
    }
    count
}

/// A full 4:2:0 frame of `width`x`height` as the SDK's packets: the sequence header and one record
/// for every block, each a fixture record re-addressed. The records are picked for a frame about
/// `bytes` long, which with Rubylight's link-speed rate (300 Mbps at 60 fps) is 0.3 byte per pixel.
fn synthetic_frame(pool: &[Vec<u8>], width: usize, height: usize, bytes: usize, sequence: u32) -> Vec<Vec<u8>> {
    let count = block_count(width, height, false);
    let average = bytes / count;
    let mut pool: Vec<&Vec<u8>> = pool.iter().collect();
    pool.sort_by_key(|record| record.len().abs_diff(average));
    pool.truncate(128);
    let mut header = vec![0u8; 8];
    header[..4].copy_from_slice(&(HEADER_BIT | (width as u32 - 1) | ((height as u32 - 1) << 14) | (sequence << 28)).to_le_bytes());
    header[4..].copy_from_slice(&(count as u32).to_le_bytes());
    let mut out = vec![header];
    for index in 0..count {
        let mut record = pool[index % pool.len()].clone();
        let first = (word(&record, 0) & !(7 << 28)) | (sequence << 28);
        let second = (index as u32) << 8 | (word(&record, 4) & 0xff);
        record[..4].copy_from_slice(&first.to_le_bytes());
        record[4..8].copy_from_slice(&second.to_le_bytes());
        out.push(record);
    }
    out
}

fn percentile(values: &mut [f64], p: usize) -> f64 {
    values.sort_by(f64::total_cmp);
    values[(values.len() * p / 100).min(values.len() - 1)]
}

/// CPU time to push the records and submit, and the GPU's own time, per decode at three sizes.
/// The input is synthetic (fixture records spread over every block of a frame), so the GPU time
/// is for a realistic data volume and not for a real picture.
#[test]
#[ignore = "needs the PyroWave SDK and a GPU"]
fn decode_timing_at_1080p_1440p_and_4k() {
    let _gpu = gpu_lock();
    let api = sdk();
    let d3d = default_device().unwrap();
    let mut pool = Vec::new();
    for fixture in &fixtures()[..2] {
        for packet in fixture.packets().into_iter().skip(1) {
            let mut at = 0;
            while at < packet.len() {
                let length = ((word(&packet, at) >> 16) & 0xfff) as usize * 4;
                pool.push(packet[at..at + length].to_vec());
                at += length;
            }
        }
    }
    for (name, width, height) in [("1080p", 1920usize, 1080usize), ("1440p", 2560, 1440), ("4K", 3840, 2160)] {
        let config = GpuConfig { width: width as u32, height: height as u32, chroma444: false, ten_bit: false, slots: 1 };
        let mut gpu = GpuDecoder::with_api(api.clone(), &d3d, config).unwrap();
        let frames: Vec<Vec<Vec<u8>>> = (0..8).map(|s| synthetic_frame(&pool, width, height, width * height * 3 / 10, s)).collect();
        let size: usize = frames[0].iter().map(Vec::len).sum();
        let (mut submit, mut total, mut gpu_ms) = (Vec::new(), Vec::new(), Vec::new());
        for i in 0..320 {
            let start = Instant::now();
            let value = push_and_decode(&mut gpu, &frames[i % 8]);
            let submitted = start.elapsed();
            gpu.wait(value, Duration::from_secs(10)).unwrap();
            let finished = start.elapsed();
            let stats = gpu.performance_stats(true);
            if i == 20 {
                println!("{name}: SDK performance stats after one decode: {stats:?}");
            }
            if i >= 20 {
                submit.push(submitted.as_secs_f64() * 1000.0);
                total.push(finished.as_secs_f64() * 1000.0);
                if let Some(ms) = stats.iter().find_map(|line| parse_gpu_ms(line)) {
                    gpu_ms.push(ms);
                }
            }
        }
        let gpu_text = if gpu_ms.is_empty() {
            "no GPU time parsed".to_owned()
        } else {
            format!("GPU p50 {:.3} ms p95 {:.3} ms ({} samples)", percentile(&mut gpu_ms.clone(), 50), percentile(&mut gpu_ms, 95), gpu_ms.len())
        };
        println!(
            "{name}: {size} bytes/frame; CPU push+submit p50 {:.3} ms p95 {:.3} ms; submit to signalled p50 {:.3} ms p95 {:.3} ms; {gpu_text}",
            percentile(&mut submit.clone(), 50),
            percentile(&mut submit, 95),
            percentile(&mut total.clone(), 50),
            percentile(&mut total, 95),
        );
    }
}

/// The SDK's per-decode GPU time in milliseconds from one report line, if it has one.
fn parse_gpu_ms(line: &str) -> Option<f64> {
    let _ = line;
    None
}
