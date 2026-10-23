//! Unit tests (no GPU) and `--ignored` GPU tests for the SDK binding.
//!
//! The GPU tests need the built SDK (scripts/build-pyrowave-sdk.ps1, or .sh on Linux): they look in
//! `%NIGHTJAR_PYROWAVE_SDK%`, else `C:\src\butterpollo-release\nj-pyrowave-sdk` (`$NIGHTJAR_PYROWAVE_SDK`,
//! else `~/nj-pyrowave-sdk` on Linux). Run them with
//! `cargo test -p nightjar-desktop --release --bin nightjar -- --ignored --nocapture --test-threads=1 pyrowave_sdk`.
//! Linux has no D3D11 side: the tests there read the planes the decoder wrote into CPU buffers.
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
    // pyrowave_cpu_buffer (pyrowave.h): three pointers, three strides, three plane sizes, then
    // width, height and the format enum, padded to 8.
    assert_eq!(size_of::<CpuBuffer>(), 88);
    assert_eq!(
        (offset_of!(CpuBuffer, data), offset_of!(CpuBuffer, row_stride_in_bytes), offset_of!(CpuBuffer, plane_size_in_bytes)),
        (0, 24, 48),
    );
    assert_eq!((offset_of!(CpuBuffer, width), offset_of!(CpuBuffer, height), offset_of!(CpuBuffer, format)), (72, 76, 80));
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

/// The probe on the library at `path`, whichever platform's signature.
fn probe_at(path: &Path, ten_bit: bool) -> Result<(), String> {
    #[cfg(windows)]
    return probe_in(path, None, ten_bit);
    #[cfg(unix)]
    return probe_in(path, ten_bit);
}

#[test]
fn a_missing_dll_names_the_file_and_where_it_was_expected() {
    let missing = std::env::temp_dir().join("nightjar-no-such-dir").join(DLL_NAME);
    let error = probe_at(&missing, false).unwrap_err();
    assert!(error.starts_with(&format!("{DLL_NAME} not found next to the executable (")), "{error}");
    assert!(error.contains("nightjar-no-such-dir"), "{error}");
    assert_eq!(Api::load_from(&missing).err(), Some(error));
}

#[test]
fn a_dll_that_is_no_pyrowave_sdk_is_named_as_such() {
    #[cfg(windows)]
    let (other, name) = (Path::new(r"C:\Windows\System32\kernel32.dll"), "kernel32.dll");
    #[cfg(unix)]
    let (other, name) = (Path::new("/lib/x86_64-linux-gnu/libc.so.6"), "libc.so.6");
    if !other.exists() {
        eprintln!("skipped: {} does not exist on this machine", other.display());
        return;
    }
    let error = probe_at(other, false).unwrap_err();
    assert!(error.contains(&format!("{name} is not a PyroWave SDK: pyrowave_get_api_version is missing")), "{error}");
}

#[test]
fn a_dll_with_another_abi_is_refused_with_its_version() {
    // A stand-in that reports 2.0.0 and nothing else; the version is read before any other symbol.
    let dir = std::env::temp_dir().join(format!("nightjar-abi-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let source = dir.join("fake.c");
    let export = if cfg!(windows) { "__declspec(dllexport)" } else { "__attribute__((visibility(\"default\")))" };
    std::fs::write(&source, format!("{export} void pyrowave_get_api_version(unsigned *a, unsigned *b, unsigned *c) {{ *a = 2; *b = 1; *c = 4; }}\n")).unwrap();
    let dll = dir.join(if cfg!(windows) { "fake.dll" } else { "fake.so" });
    let mut compiler = std::process::Command::new("gcc");
    compiler.arg("-shared");
    if cfg!(unix) { compiler.arg("-fPIC"); }
    let built = compiler.arg("-o").arg(&dll).arg(&source).status();
    assert!(built.is_ok_and(|s| s.success()), "gcc is needed on PATH to build the stand-in library");
    let error = probe_at(&dll, true).unwrap_err();
    assert_eq!(error, "PyroWave SDK ABI 2.1.4 is not supported, this client needs 1.x");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn plane_sizes_follow_the_chroma_format() {
    let config = |chroma444| GpuConfig { width: 132, height: 98, chroma444, ten_bit: false, slots: 1 };
    assert_eq!([0, 1, 2].map(|p| config(false).plane_size(p)), [(132, 98), (66, 49), (66, 49)]);
    assert_eq!([0, 1, 2].map(|p| config(true).plane_size(p)), [(132, 98); 3]);
}

/// Queues a signal of `fence` on `device`'s immediate context and flushes it.
#[cfg(windows)]
fn signal(device: &ID3D11Device, fence: &ID3D11Fence, value: u64) {
    // SAFETY: Plain D3D11 calls on the calling thread.
    unsafe {
        let context = device.GetImmediateContext().unwrap();
        context.cast::<ID3D11DeviceContext4>().unwrap().Signal(fence, value).unwrap();
        context.Flush();
    }
}

#[cfg(windows)]
fn device_of(fence: &ID3D11Fence) -> ID3D11Device {
    // SAFETY: Plain D3D11 call.
    unsafe { fence.GetDevice() }.unwrap()
}

#[test]
fn a_probe_result_is_kept_per_adapter_and_plane_format() {
    let cache = ProbeCache::default();
    let (first, second) = (Luid { luid: [1; 8] }, Luid { luid: [2; 8] });
    let calls = std::cell::Cell::new(0);
    let ask = |luid, ten_bit, result: Result<(), String>| {
        cache.get_or_probe(luid, ten_bit, || {
            calls.set(calls.get() + 1);
            result
        })
    };
    assert_eq!(ask(first, false, Err("no subgroup size control".into())), Err("no subgroup size control".into()));
    // Remembered, errors included: the second ask neither probes nor changes the answer.
    assert_eq!(ask(first, false, Ok(())), Err("no subgroup size control".into()));
    assert_eq!(calls.get(), 1);
    assert_eq!(ask(first, true, Ok(())), Ok(()));
    assert_eq!(ask(second, false, Ok(())), Ok(()));
    assert_eq!(calls.get(), 3);
}

#[test]
fn a_refused_decoder_names_the_requirement_the_adapter_misses() {
    assert_eq!(
        decoder_refused(-1, "Radeon lacks subgroup size control"),
        "creating the PyroWave decoder: PyroWave error -1 (generic error); it needs Vulkan 1.3 with subgroup size control and the subgroup operations: Radeon lacks subgroup size control"
    );
}

#[test]
fn the_decoder_moves_between_threads_but_is_not_shared() {
    fn send<T: Send>() {}
    send::<GpuDecoder>();
    // Compiles only while GpuDecoder is not Sync (a Sync type would make the call ambiguous).
    trait AmbiguousIfSync<A> {
        fn check() {}
    }
    impl<T: ?Sized> AmbiguousIfSync<()> for T {}
    impl<T: ?Sized + Sync> AmbiguousIfSync<u8> for T {}
    <GpuDecoder as AmbiguousIfSync<_>>::check();
}

#[cfg(windows)]
#[test]
fn a_stalled_fence_is_a_loud_error_after_the_deadline_and_a_signalled_one_is_ready() {
    let device = default_device().unwrap().cast::<ID3D11Device5>().unwrap();
    let mut fence = None;
    // SAFETY: A plain D3D11 call with a live out-pointer.
    unsafe { device.CreateFence(0, D3D11_FENCE_FLAG_NONE, &mut fence) }.unwrap();
    let fence: ID3D11Fence = fence.unwrap();
    assert_eq!(fence_ready_within(&fence, 0, Duration::ZERO), Ok(()));
    let start = Instant::now();
    let error = fence_ready_within(&fence, 1, Duration::from_millis(80)).unwrap_err();
    assert!(start.elapsed() >= Duration::from_millis(70), "gave up after {:?}", start.elapsed());
    assert!(error.contains("fence value 1") && error.contains("stands at 0") && error.contains("hung or lost"), "{error}");
    // The GPU side signals (queued, then flushed): the wait ends at the signal, long before the deadline.
    signal(&device_of(&fence), &fence, 1);
    let start = Instant::now();
    assert_eq!(fence_ready_within(&fence, 1, Duration::from_secs(5)), Ok(()));
    assert!(start.elapsed() < Duration::from_secs(1), "woke after {:?}", start.elapsed());
}

// ---- GPU tests -------------------------------------------------------------------------------

/// One GPU test at a time: they each make Vulkan devices on the same adapter.
static GPU: Mutex<()> = Mutex::new(());

fn gpu_lock() -> MutexGuard<'static, ()> {
    GPU.lock().unwrap_or_else(|e| e.into_inner())
}

fn sdk_dll() -> PathBuf {
    #[cfg(windows)]
    let default = PathBuf::from(r"C:\src\butterpollo-release\nj-pyrowave-sdk");
    #[cfg(unix)]
    let default = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default().join("nj-pyrowave-sdk");
    std::env::var_os("NIGHTJAR_PYROWAVE_SDK").map(PathBuf::from).unwrap_or(default).join(DLL_NAME)
}

fn sdk() -> Arc<Api> {
    Api::load_from(&sdk_dll()).unwrap_or_else(|e| panic!("{e}\nbuild it with scripts/build-pyrowave-sdk.{}", if cfg!(windows) { "ps1" } else { "sh" }))
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
#[cfg(windows)]
struct Readback {
    context: ID3D11DeviceContext,
    context4: ID3D11DeviceContext4,
    staging: Vec<ID3D11Texture2D>,
    config: GpuConfig,
}

#[cfg(windows)]
impl Readback {
    fn new(d3d: &ID3D11Device, config: GpuConfig) -> Self {
        // SAFETY: Plain D3D11 call.
        let context = unsafe { d3d.GetImmediateContext().unwrap() };
        Self { context4: context.cast().expect("ID3D11DeviceContext4"), context, staging: Self::staging(d3d, config), config }
    }

    /// Three CPU-readable textures the size of the picture's planes.
    fn staging(d3d: &ID3D11Device, config: GpuConfig) -> Vec<ID3D11Texture2D> {
        (0..3)
            .map(|plane| {
                let (width, height) = config.plane_size(plane);
                let mut texture = None;
                // SAFETY: A plain D3D11 call with live locals.
                unsafe {
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
                }
                texture.unwrap()
            })
            .collect()
    }

    /// Waits for fence `value` on the CPU and then on the D3D11 queue (the way the presenter will),
    /// and returns the raw sample codes of `slot`'s planes.
    fn planes(&self, gpu: &GpuDecoder, slot: usize, value: u64) -> [Vec<u16>; 3] {
        gpu.wait(value, Duration::from_secs(10)).unwrap();
        // SAFETY: Plain D3D11 calls.
        unsafe {
            self.context4.Wait(gpu.fence(), value).unwrap();
            for plane in 0..3 {
                self.context.CopyResource(&self.staging[plane], &gpu.planes(slot)[plane]);
            }
        }
        self.read(&self.staging)
    }

    /// The raw sample codes of staging planes already copied to (blocks until the copy is done).
    fn read(&self, staging: &[ID3D11Texture2D]) -> [Vec<u16>; 3] {
        // SAFETY: Plain D3D11 calls; the mapped rows are read before Unmap.
        unsafe {
            let bytes = if self.config.ten_bit { 2 } else { 1 };
            std::array::from_fn(|plane| {
                let (width, height) = self.config.plane_size(plane);
                let (width, height) = (width as usize, height as usize);
                let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
                self.context.Map(&staging[plane], 0, D3D11_MAP_READ, 0, Some(&mut mapped)).unwrap();
                let mut out = Vec::with_capacity(width * height);
                for y in 0..height {
                    let row = std::slice::from_raw_parts(mapped.pData.cast::<u8>().add(y * mapped.RowPitch as usize), width * bytes);
                    out.extend(row.chunks(bytes).map(|c| if bytes == 2 { u16::from_le_bytes([c[0], c[1]]) } else { u16::from(c[0]) }));
                }
                self.context.Unmap(&staging[plane], 0);
                out
            })
        }
    }
}

/// A sample code as a value on the picture's 8 or 10-bit scale (R16 holds 10-bit pictures as UNORM16).
fn code(raw: u16, ten: bool) -> f32 {
    if ten { f32::from(raw) * 1023.0 / 65535.0 } else { f32::from(raw) }
}

#[cfg(windows)]
fn push_and_decode(gpu: &mut GpuDecoder, packets: &[Vec<u8>]) -> u64 {
    push_and_decode_to(gpu, packets, 0, 0)
}

/// Decodes into `slot` once the reuse fence reaches `after`.
#[cfg(windows)]
fn push_and_decode_to(gpu: &mut GpuDecoder, packets: &[Vec<u8>], slot: usize, after: u64) -> u64 {
    gpu.clear();
    for packet in packets {
        gpu.push_packet(packet).unwrap();
    }
    assert!(gpu.is_ready(false), "the pushed packets did not complete the frame");
    gpu.decode(slot, after).unwrap()
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

#[cfg(windows)]
#[test]
#[ignore = "needs the PyroWave SDK and a GPU"]
fn fixtures_decode_on_the_gpu_like_the_reference_and_the_cpu_decoder() {
    let _gpu = gpu_lock();
    let api = sdk();
    let d3d = default_device().unwrap();
    let mut failed = Vec::new();
    for (n, fixture) in fixtures().iter().enumerate() {
        let config = fixture.config();
        let mut gpu = GpuDecoder::with_api(api.clone(), &d3d, config).unwrap();
        let readback = Readback::new(&d3d, config);
        let value = push_and_decode(&mut gpu, &fixture.packets());
        let planes = readback.planes(&gpu, 0, value);

        let mut offset = 0;
        let (mut golden, mut rounded) = (0.0f32, 0.0f32);
        for (p, samples) in planes.iter().enumerate() {
            let (width, height) = config.plane_size(p);
            assert_eq!(samples.len(), (width * height) as usize);
            for raw in samples {
                let expected = if fixture.ten { let v = u16::from_le_bytes([fixture.yuv[offset], fixture.yuv[offset + 1]]); offset += 2; v } else { let v = u16::from(fixture.yuv[offset]); offset += 1; v };
                golden = golden.max((code(*raw, fixture.ten) - f32::from(expected)).abs());
                rounded = rounded.max((code(*raw, fixture.ten).round() - f32::from(expected)).abs());
            }
        }
        assert_eq!(offset, fixture.yuv.len());

        let mut cpu = Cpu::new(fixture.width as usize, fixture.height as usize, fixture.full).unwrap();
        cpu.decode(fixture.bin).unwrap();
        let against_cpu = max_error_vs_cpu(&planes, &cpu, config);
        println!(
            "fixture {n} ({}x{} 444={} 10-bit={}): max error {golden:.3} code values against the .yuv ({rounded} once rounded to a code), {against_cpu:.3} against the CPU decoder",
            fixture.width, fixture.height, fixture.full, fixture.ten
        );
        // A picture is shown as whole codes, so the reference is matched within 1 code once
        // rounded; unrounded, the 10-bit planes stray a little past 1 from the reference, which was
        // made with FP32 storage. The CPU decoder's own test allows 1.1 for the same fixtures.
        if rounded > 1.0 || against_cpu >= 1.1 {
            failed.push(format!("fixture {n}: {rounded} codes from the reference once rounded, {against_cpu} from the CPU decoder"));
        }
    }
    assert!(failed.is_empty(), "{failed:?}");
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
#[cfg(windows)]
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
            let value = gpu.decode(0, 0).unwrap();
            let whole_planes = readback.planes(&gpu, 0, value);
            let whole_error = max_error_vs_cpu(&whole_planes, &cpu, config);

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
            let value = gpu.decode(0, 0).unwrap();
            let early_planes = readback.planes(&gpu, 0, value);
            let early_error = max_error_vs_cpu(&early_planes, &cpu, config);
            assert!(early_planes == whole_planes, "lost {lost:?}, frame {n}: the early planes differ from the whole planes");
            println!("lost {lost:?}, frame {n}: {early_pushed} records pushed early; max error {whole_error:.3} whole, {early_error:.3} early (code values from the CPU decoder)");
            assert!(whole_error < 1.1 && early_error < 1.1, "lost {lost:?}, frame {n}: {whole_error} whole, {early_error} early");
        }
    }
}

#[cfg(windows)]
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
    // A life builds the decoder, its plane textures and the fence; with `decode`, runs one frame
    // and waits for it; with `own_device`, on a D3D11 device made and dropped with it.
    let life = |decode: bool, own_device: bool| {
        let own = own_device.then(|| default_device().unwrap());
        let device = own.as_ref().unwrap_or(&d3d);
        let mut gpu = GpuDecoder::with_api(api.clone(), device, fixture.config()).unwrap();
        if decode {
            let value = push_and_decode(&mut gpu, &packets);
            gpu.wait(value, Duration::from_secs(10)).unwrap();
        }
        drop(gpu);
        // D3D11 frees a released resource at the next flush of its context, which the presenter
        // does every frame: without it a long-lived device holds the textures' handles.
        // SAFETY: The context belongs to a device this test owns, used from this thread only.
        unsafe { device.GetImmediateContext().unwrap().Flush() };
    };
    // The driver releases some handles from its own threads a little after a life ends.
    let settled = || {
        std::thread::sleep(Duration::from_millis(500));
        handles()
    };
    let mut failed = Vec::new();
    for (name, decode, own_device) in [("create, decode, drop", true, false), ("create, decode, drop, own D3D11 device", true, true), ("create and drop", false, false)] {
        // The loader, the driver and the SDK's thread pool settle over the first lives.
        for _ in 0..10 {
            life(decode, own_device);
        }
        let before = settled();
        for _ in 0..10 {
            life(decode, own_device);
        }
        let after = settled();
        let growth = i64::from(after) - i64::from(before);
        println!("{name}: 10 decoder lives, {before} -> {after} handles ({growth:+})");
        if growth > 10 {
            failed.push(format!("{name}: {growth}"));
        }
    }
    assert!(failed.is_empty(), "10 lives left more than 10 handles in all: {failed:?}");
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
#[cfg(windows)]
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
        let queue = Handoff::new(&d3d);
        let (mut idle_event, mut wait_event, mut wait_extra, mut d3d_total, mut d3d_extra) = (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
        for i in 0..320 {
            let start = Instant::now();
            let value = push_and_decode(&mut gpu, &frames[i % 8]);
            let submitted = start.elapsed();
            gpu.wait(value, Duration::from_secs(10)).unwrap();
            let finished = start.elapsed();
            let stats = gpu.performance_stats(true);
            // The planes are final. What D3D11 then pays to wait on the fence: the same fence wait
            // and an event query, against the event query alone on the idle queue.
            let idle = queue.event_ms(None);
            let waited = queue.event_ms(Some((gpu.fence(), value)));
            // The other way round, with no CPU wait: submit, make D3D11 wait on the fence, and
            // see when D3D11 is done, against when the CPU saw the fence signalled above.
            let start_b = Instant::now();
            let value_b = push_and_decode(&mut gpu, &frames[i % 8]);
            queue.event_ms(Some((gpu.fence(), value_b)));
            let d3d_done = start_b.elapsed().as_secs_f64() * 1000.0;
            if i >= 20 {
                idle_event.push(idle);
                wait_event.push(waited);
                wait_extra.push(waited - idle);
                d3d_total.push(d3d_done);
                d3d_extra.push(d3d_done - finished.as_secs_f64() * 1000.0);
            }
            gpu.performance_stats(true);
            if i == 20 {
                println!("{name}: SDK performance stats after one decode: {stats:?}");
            }
            if i >= 20 {
                submit.push(submitted.as_secs_f64() * 1000.0);
                total.push(finished.as_secs_f64() * 1000.0);
                let passes: Vec<f64> = stats.iter().filter_map(|line| parse_gpu_ms(line)).collect();
                if !passes.is_empty() {
                    gpu_ms.push(passes.iter().sum());
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
        let p = |v: &mut Vec<f64>| format!("p50 {:.3} ms p95 {:.3} ms", percentile(&mut v.clone(), 50), percentile(v, 95));
        println!(
            "{name}: handoff to D3D11 (no copy on the D3D11 side; the shared textures are sampled in place): event query on an idle queue {}; fence wait on an already signalled value + event query {}; \
             so the wait adds {}; submit to D3D11-done with no CPU wait {}, which is {} after the CPU saw the signal",
            p(&mut idle_event),
            p(&mut wait_event),
            p(&mut wait_extra),
            p(&mut d3d_total),
            p(&mut d3d_extra),
        );
    }
}

/// An idle-queue probe for the cost of D3D11's side of the fence handoff.
#[cfg(windows)]
struct Handoff {
    context: ID3D11DeviceContext,
    context4: ID3D11DeviceContext4,
    query: ID3D11Query,
}

#[cfg(windows)]
impl Handoff {
    fn new(d3d: &ID3D11Device) -> Self {
        // SAFETY: Plain D3D11 calls with live locals.
        unsafe {
            let context = d3d.GetImmediateContext().unwrap();
            let mut query = None;
            d3d.CreateQuery(&D3D11_QUERY_DESC { Query: D3D11_QUERY_EVENT, MiscFlags: 0 }, Some(&mut query)).unwrap();
            Self { context4: context.cast().unwrap(), context, query: query.unwrap() }
        }
    }

    /// Queues the optional fence wait and an event, flushes, and spins until D3D11 has reached the
    /// event; milliseconds from before the wait was queued.
    fn event_ms(&self, wait: Option<(&ID3D11Fence, u64)>) -> f64 {
        let start = Instant::now();
        // SAFETY: Plain D3D11 calls on live objects.
        unsafe {
            if let Some((fence, value)) = wait {
                self.context4.Wait(fence, value).unwrap();
            }
            self.context.End(&self.query);
            self.context.Flush();
            // S_FALSE (not yet) is also Ok, so the event's BOOL says when it has been reached.
            let mut reached = windows::core::BOOL(0);
            while !reached.as_bool() {
                let _ = self.context.GetData(&self.query, Some((&raw mut reached).cast()), 4, 0);
                std::hint::spin_loop();
            }
        }
        start.elapsed().as_secs_f64() * 1000.0
    }
}

/// The SDK's per-decode GPU time in milliseconds from one report line, if it has one: the
/// report is "iDWT: 0.051 ms per frame", "Dequant: 0.020 ms per frame" and memory heap lines, and
/// a frame's GPU time is the sum of the passes.
fn parse_gpu_ms(line: &str) -> Option<f64> {
    let (_, rest) = line.split_once(": ")?;
    rest.strip_suffix(" ms per frame")?.parse().ok()
}

#[test]
fn gpu_time_is_read_from_the_pass_lines_only() {
    assert_eq!(parse_gpu_ms("iDWT: 0.051 ms per frame"), Some(0.051));
    assert_eq!(parse_gpu_ms("Memory Heap 0 (HOST): MaxSize 16346.938 MiB, BudgetSize 15578.938 MiB"), None);
    assert_eq!(parse_gpu_ms("nothing here"), None);
}

/// The Vulkan side must not overwrite a plane set while D3D11 may still read the picture in it: the
/// decode waits on the reuse fence. Deterministic: the "presenter" is the test, which holds the
/// signal back and checks the decode has not completed, then releases it.
#[cfg(windows)]
#[test]
#[ignore = "needs the PyroWave SDK and a GPU"]
fn a_decode_into_a_plane_set_waits_for_the_reuse_fence() {
    let _gpu = gpu_lock();
    let api = sdk();
    let d3d = default_device().unwrap();
    let fixture = &fixtures()[0];
    let config = GpuConfig { slots: 2, ..fixture.config() };
    let packets = fixture.packets();
    let mut gpu = GpuDecoder::with_api(api, &d3d, config).unwrap();
    let first = push_and_decode_to(&mut gpu, &packets, 0, 0);
    gpu.wait(first, Duration::from_secs(10)).unwrap();
    // "D3D11 is still drawing slot 0": the reuse fence has not reached 1.
    let second = push_and_decode_to(&mut gpu, &packets, 0, 1);
    let held = gpu.wait(second, Duration::from_millis(400));
    // SAFETY: A plain fence query.
    let early = unsafe { gpu.reuse_fence().GetCompletedValue() };
    // Release before asserting: a failed assert must not leave the GPU waiting in Drop.
    signal(&d3d, gpu.reuse_fence(), 1);
    assert_eq!(early, 0);
    assert!(held.as_ref().is_err_and(|e| e.contains("did not finish")), "the decode ran before the reuse fence reached its value: {held:?}");
    gpu.wait(second, Duration::from_secs(10)).unwrap();
    gpu.ready_within(second, Duration::from_secs(1)).unwrap();
}

/// A ring of plane sets decoded back to back with no CPU wait, each picture read by a D3D11 pass
/// queued behind the decode fence and followed by a signal of the reuse fence, the way the
/// presenter will run. Two different pictures alternate, so a plane set overwritten early, or read
/// before its decode, shows as the other picture or as a mix.
#[cfg(windows)]
#[test]
#[ignore = "needs the PyroWave SDK and a GPU"]
fn a_ring_of_plane_sets_decodes_a_sequence_without_cpu_waits() {
    const FRAMES: usize = 24;
    const SLOTS: usize = 3;
    let _gpu = gpu_lock();
    let api = sdk();
    let d3d = default_device().unwrap();
    let fixtures = fixtures();
    // Both are 132x98 4:2:0, decoded to 8-bit planes.
    let config = GpuConfig { ten_bit: false, slots: SLOTS, ..fixtures[0].config() };
    let packets = [fixtures[0].packets(), fixtures[1].packets()];
    let cpu: Vec<Cpu> = fixtures[..2]
        .iter()
        .map(|f| {
            let mut cpu = Cpu::new(132, 98, false).unwrap();
            cpu.decode(f.bin).unwrap();
            cpu
        })
        .collect();
    let mut gpu = GpuDecoder::with_api(api, &d3d, config).unwrap();
    let readback = Readback::new(&d3d, config);
    let staging: Vec<Vec<ID3D11Texture2D>> = (0..FRAMES).map(|_| Readback::staging(&d3d, config)).collect();
    let mut reuse_after = [0u64; SLOTS];
    let mut signalled = 0u64;
    for k in 0..FRAMES {
        let slot = k % SLOTS;
        let value = push_and_decode_to(&mut gpu, &packets[k % 2], slot, reuse_after[slot]);
        // SAFETY: Plain D3D11 calls; the planes and fences are the decoder's.
        unsafe {
            readback.context4.Wait(gpu.fence(), value).unwrap();
            for (target, source) in staging[k].iter().zip(gpu.planes(slot)) {
                readback.context.CopyResource(target, source);
            }
            signalled += 1;
            readback.context4.Signal(gpu.reuse_fence(), signalled).unwrap();
            readback.context.Flush();
        }
        reuse_after[slot] = signalled;
    }
    let mut worst = 0.0f32;
    for (k, staging) in staging.iter().enumerate() {
        let planes = readback.read(staging);
        let own = max_error_vs_cpu(&planes, &cpu[k % 2], config);
        let other = max_error_vs_cpu(&planes, &cpu[1 - k % 2], config);
        assert!(own < 1.1, "frame {k} (picture {}, plane set {}) is {own} code values from its own fixture", k % 2, k % SLOTS);
        assert!(other > 2.0, "frame {k}: the two fixtures are not told apart by this test ({other})");
        worst = worst.max(own);
    }
    println!("ring of {SLOTS} plane sets, {FRAMES} frames of two pictures with no CPU waits: worst error against each frame's own fixture {worst:.3} code values");
}

/// The DLL loads once and a repeated probe answers from memory with the same result, for the
/// 8-bit R8 planes and for the 10-bit R16 planes.
#[cfg(windows)]
#[test]
#[ignore = "needs the PyroWave SDK and a GPU"]
fn probing_twice_loads_the_sdk_once_and_gives_the_same_answer() {
    let _gpu = gpu_lock();
    // The shared loader looks next to the executable: put the SDK there for this run.
    let beside = dll_path().unwrap();
    if !beside.exists() {
        std::fs::copy(sdk_dll(), &beside).unwrap();
    }
    let d3d = default_device().unwrap();
    let before = Api::shared_loads();
    let start = Instant::now();
    let first = probe(Some(&d3d), false);
    let first_ms = start.elapsed().as_secs_f64() * 1000.0;
    let start = Instant::now();
    let second = probe(Some(&d3d), false);
    let second_ms = start.elapsed().as_secs_f64() * 1000.0;
    assert_eq!(first, Ok(()));
    assert_eq!(first, second);
    assert_eq!(probe(Some(&d3d), true), Ok(()), "the 10-bit R16 planes must import");
    assert_eq!(probe(None, true), Ok(()));
    assert_eq!(Api::shared_loads() - before, 1, "the DLL was loaded more than once");
    assert!(Arc::ptr_eq(&Api::shared().unwrap(), &Api::shared().unwrap()));
    assert_eq!(Api::shared_loads() - before, 1);
    println!("probe: first {first_ms:.1} ms, second {second_ms:.3} ms, one DLL load");
    assert!(second_ms < first_ms, "the second probe was not answered from memory");
}

// ---- Linux GPU tests -------------------------------------------------------------------------

/// The three plane buffers of `config`, zeroed.
#[cfg(unix)]
fn plane_buffers(config: GpuConfig) -> [Vec<u8>; 3] {
    [0, 1, 2].map(|plane| vec![0u8; config.plane_bytes(plane)])
}

/// The raw sample codes of CPU plane buffers, the way the Windows tests read R8 and R16 textures.
#[cfg(unix)]
fn codes(buffers: &[Vec<u8>; 3], config: GpuConfig) -> [Vec<u16>; 3] {
    std::array::from_fn(|plane| {
        let (width, height) = config.plane_size(plane);
        assert_eq!(buffers[plane].len(), config.plane_bytes(plane));
        let samples: Vec<u16> = if config.ten_bit {
            buffers[plane].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect()
        } else {
            buffers[plane].iter().map(|&b| u16::from(b)).collect()
        };
        assert_eq!(samples.len(), (width * height) as usize);
        samples
    })
}

/// Pushes the packets of one frame and decodes it into fresh buffers.
#[cfg(unix)]
fn push_and_decode(gpu: &mut GpuDecoder, packets: &[Vec<u8>]) -> [Vec<u16>; 3] {
    let config = gpu.config();
    let mut buffers = plane_buffers(config);
    push_and_decode_into(gpu, packets, &mut buffers);
    codes(&buffers, config)
}

#[cfg(unix)]
fn push_and_decode_into(gpu: &mut GpuDecoder, packets: &[Vec<u8>], buffers: &mut [Vec<u8>; 3]) {
    gpu.clear();
    for packet in packets {
        gpu.push_packet(packet).unwrap();
    }
    assert!(gpu.is_ready(false), "the pushed packets did not complete the frame");
    let [a, b, c] = buffers;
    gpu.decode([a.as_mut_slice(), b.as_mut_slice(), c.as_mut_slice()]).unwrap();
}

/// A deadline the GPU cannot meet stands in for a hung device (the SDK's wait has no timeout of its
/// own): the decode fails loudly, the decoder is stalled and refuses everything after.
#[cfg(unix)]
#[test]
#[ignore = "needs the PyroWave SDK and a Vulkan GPU"]
fn a_decode_that_misses_its_deadline_is_a_loud_error_and_leaves_the_decoder_stalled() {
    let _gpu = gpu_lock();
    let fixture = &fixtures()[0];
    let mut gpu = GpuDecoder::with_api(sdk(), fixture.config()).unwrap();
    gpu.set_decode_timeout(Duration::ZERO);
    let mut buffers = plane_buffers(gpu.config());
    gpu.clear();
    for packet in fixture.packets() {
        gpu.push_packet(&packet).unwrap();
    }
    let [a, b, c] = &mut buffers;
    let error = gpu.decode([a.as_mut_slice(), b.as_mut_slice(), c.as_mut_slice()]).unwrap_err();
    assert!(error.contains("did not finish within") && error.contains("hung or lost"), "{error}");
    assert!(gpu.stalled());
    assert!(!gpu.is_ready(true) && gpu.push_packet(&[0; 8]).is_err());
    let [a, b, c] = &mut buffers;
    assert!(gpu.decode([a.as_mut_slice(), b.as_mut_slice(), c.as_mut_slice()]).unwrap_err().contains("stalled"));
    // Dropping it leaks the device instead of idling it, which would block on a hung GPU, and the
    // buffers stay too: the stuck call may still copy into them.
    drop(gpu);
    std::mem::forget(buffers);
}

#[cfg(unix)]
#[test]
#[ignore = "needs the PyroWave SDK and a Vulkan GPU"]
fn fixtures_decode_on_the_gpu_like_the_reference_and_the_cpu_decoder() {
    let _gpu = gpu_lock();
    let api = sdk();
    let mut failed = Vec::new();
    for (n, fixture) in fixtures().iter().enumerate() {
        let config = fixture.config();
        let mut gpu = GpuDecoder::with_api(api.clone(), config).unwrap();
        let planes = push_and_decode(&mut gpu, &fixture.packets());

        let mut offset = 0;
        let (mut golden, mut rounded) = (0.0f32, 0.0f32);
        for samples in &planes {
            for raw in samples {
                let expected = if fixture.ten { let v = u16::from_le_bytes([fixture.yuv[offset], fixture.yuv[offset + 1]]); offset += 2; v } else { let v = u16::from(fixture.yuv[offset]); offset += 1; v };
                golden = golden.max((code(*raw, fixture.ten) - f32::from(expected)).abs());
                rounded = rounded.max((code(*raw, fixture.ten).round() - f32::from(expected)).abs());
            }
        }
        assert_eq!(offset, fixture.yuv.len());

        let mut cpu = Cpu::new(fixture.width as usize, fixture.height as usize, fixture.full).unwrap();
        cpu.decode(fixture.bin).unwrap();
        let against_cpu = max_error_vs_cpu(&planes, &cpu, config);
        println!(
            "fixture {n} ({}x{} 444={} 10-bit={}): max error {golden:.3} code values against the .yuv ({rounded} once rounded to a code), {against_cpu:.3} against the CPU decoder",
            fixture.width, fixture.height, fixture.full, fixture.ten
        );
        // The same bounds as the Windows test: whole codes within 1 of the reference, 1.1 of the CPU decoder.
        if rounded > 1.0 || against_cpu >= 1.1 {
            failed.push(format!("fixture {n}: {rounded} codes from the reference once rounded, {against_cpu} from the CPU decoder"));
        }
    }
    assert!(failed.is_empty(), "{failed:?}");
}

/// Frames with lost packets, decoded three ways, must give one picture: the CPU decoder, the SDK fed
/// by `Records::push_frame` once the frame is complete, and the SDK fed record by record as they
/// arrive (`feed_packet` and `drain_new`), with the recovered rest at the end.
#[cfg(unix)]
#[test]
#[ignore = "needs the PyroWave SDK and a Vulkan GPU"]
fn lossy_frames_decode_on_the_gpu_like_the_cpu_decoder_whole_and_early() {
    let _gpu = gpu_lock();
    let api = sdk();
    let fixtures = fixtures();
    let (older, newer) = (&fixtures[0], &fixtures[1]);
    // Both are 132x98 4:2:0; the bit depth is the output's, so both decode to 8-bit planes here.
    let config = GpuConfig { ten_bit: false, ..older.config() };
    let first = host_packets(&bitstream(older.bin));
    let second = host_packets(&bitstream(newer.bin));
    let count = second.len();
    let mut gpu = GpuDecoder::with_api(api, config).unwrap();
    let mut buffers = plane_buffers(config);
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
            let [a, b, c] = &mut buffers;
            gpu.decode([a.as_mut_slice(), b.as_mut_slice(), c.as_mut_slice()]).unwrap();
            let whole_planes = codes(&buffers, config);
            let whole_error = max_error_vs_cpu(&whole_planes, &cpu, config);

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
            let [a, b, c] = &mut buffers;
            gpu.decode([a.as_mut_slice(), b.as_mut_slice(), c.as_mut_slice()]).unwrap();
            let early_planes = codes(&buffers, config);
            let early_error = max_error_vs_cpu(&early_planes, &cpu, config);
            assert!(early_planes == whole_planes, "lost {lost:?}, frame {n}: the early planes differ from the whole planes");
            println!("lost {lost:?}, frame {n}: {early_pushed} records pushed early; max error {whole_error:.3} whole, {early_error:.3} early (code values from the CPU decoder)");
            assert!(whole_error < 1.1 && early_error < 1.1, "lost {lost:?}, frame {n}: {whole_error} whole, {early_error} early");
        }
    }
}

/// Plane sets reused in a ring, each decode into a buffer that held another picture, must give the
/// picture just decoded and nothing of the one before: two different pictures alternate.
#[cfg(unix)]
#[test]
#[ignore = "needs the PyroWave SDK and a Vulkan GPU"]
fn a_ring_of_plane_buffers_decodes_a_sequence_without_mixing_pictures() {
    const FRAMES: usize = 24;
    const SLOTS: usize = 3;
    let _gpu = gpu_lock();
    let api = sdk();
    let fixtures = fixtures();
    let config = GpuConfig { ten_bit: false, slots: SLOTS, ..fixtures[0].config() };
    let packets = [fixtures[0].packets(), fixtures[1].packets()];
    let cpu: Vec<Cpu> = fixtures[..2]
        .iter()
        .map(|f| {
            let mut cpu = Cpu::new(132, 98, false).unwrap();
            cpu.decode(f.bin).unwrap();
            cpu
        })
        .collect();
    let mut gpu = GpuDecoder::with_api(api, config).unwrap();
    let mut ring: Vec<[Vec<u8>; 3]> = (0..SLOTS).map(|_| plane_buffers(config)).collect();
    let mut worst = 0.0f32;
    for k in 0..FRAMES {
        push_and_decode_into(&mut gpu, &packets[k % 2], &mut ring[k % SLOTS]);
        let planes = codes(&ring[k % SLOTS], config);
        let own = max_error_vs_cpu(&planes, &cpu[k % 2], config);
        let other = max_error_vs_cpu(&planes, &cpu[1 - k % 2], config);
        assert!(own < 1.1, "frame {k} (picture {}, plane set {}) is {own} code values from its own fixture", k % 2, k % SLOTS);
        assert!(other > 2.0, "frame {k}: the two fixtures are not told apart by this test ({other})");
        worst = worst.max(own);
    }
    println!("ring of {SLOTS} plane buffers, {FRAMES} frames of two pictures: worst error against each frame's own fixture {worst:.3} code values");
}

#[cfg(unix)]
#[test]
#[ignore = "needs the PyroWave SDK and a Vulkan GPU"]
fn a_plane_buffer_that_is_too_small_is_a_loud_error_and_the_decoder_stays_usable() {
    let _gpu = gpu_lock();
    let fixture = &fixtures()[0];
    let config = fixture.config();
    let mut gpu = GpuDecoder::with_api(sdk(), config).unwrap();
    gpu.clear();
    for packet in fixture.packets() {
        gpu.push_packet(&packet).unwrap();
    }
    let mut small = [vec![0u8; 10], vec![0u8; config.plane_bytes(1)], vec![0u8; config.plane_bytes(2)]];
    let [a, b, c] = &mut small;
    let error = gpu.decode([a.as_mut_slice(), b.as_mut_slice(), c.as_mut_slice()]).unwrap_err();
    assert!(error.contains("plane 0 buffer holds 10 bytes"), "{error}");
    let planes = push_and_decode(&mut gpu, &fixture.packets());
    let mut cpu = Cpu::new(fixture.width as usize, fixture.height as usize, fixture.full).unwrap();
    cpu.decode(fixture.bin).unwrap();
    assert!(max_error_vs_cpu(&planes, &cpu, config) < 1.1);
}

/// The library loads once and a repeated probe answers from memory with the same result, for the
/// 8-bit and the 16-bit planes.
#[cfg(unix)]
#[test]
#[ignore = "needs the PyroWave SDK and a Vulkan GPU"]
fn probing_twice_loads_the_sdk_once_and_gives_the_same_answer() {
    let _gpu = gpu_lock();
    // The shared loader looks next to the executable: put the SDK there for this run.
    let beside = dll_path().unwrap();
    if !beside.exists() {
        std::fs::copy(sdk_dll(), &beside).unwrap();
    }
    let before = Api::shared_loads();
    let start = Instant::now();
    let first = probe(false);
    let first_ms = start.elapsed().as_secs_f64() * 1000.0;
    let start = Instant::now();
    let second = probe(false);
    let second_ms = start.elapsed().as_secs_f64() * 1000.0;
    assert_eq!(first, Ok(()), "{}", diagnose_vulkan());
    assert_eq!(first, second);
    assert_eq!(probe(true), Ok(()), "the 16-bit planes must decode");
    assert_eq!(Api::shared_loads() - before, 1, "the library was loaded more than once");
    assert!(Arc::ptr_eq(&Api::shared().unwrap(), &Api::shared().unwrap()));
    println!("probe: first {first_ms:.1} ms, second {second_ms:.3} ms, one library load; Vulkan: {}", diagnose_vulkan());
    assert!(second_ms < first_ms, "the second probe was not answered from memory");
}

/// CPU time to push the records, and push to planes-in-memory (the decode and the readback), per
/// decode at three sizes. The input is synthetic like the Windows timing test's.
#[cfg(unix)]
#[test]
#[ignore = "needs the PyroWave SDK and a Vulkan GPU"]
fn decode_timing_at_1080p_1440p_and_4k() {
    let _gpu = gpu_lock();
    let api = sdk();
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
        let mut gpu = GpuDecoder::with_api(api.clone(), config).unwrap();
        let frames: Vec<Vec<Vec<u8>>> = (0..8).map(|s| synthetic_frame(&pool, width, height, width * height * 3 / 10, s)).collect();
        let size: usize = frames[0].iter().map(Vec::len).sum();
        let mut buffers = plane_buffers(config);
        let (mut push, mut total, mut gpu_ms) = (Vec::new(), Vec::new(), Vec::new());
        for i in 0..320 {
            let start = Instant::now();
            gpu.clear();
            for packet in &frames[i % 8] {
                gpu.push_packet(packet).unwrap();
            }
            let pushed = start.elapsed();
            let [a, b, c] = &mut buffers;
            gpu.decode([a.as_mut_slice(), b.as_mut_slice(), c.as_mut_slice()]).unwrap();
            let finished = start.elapsed();
            let stats = gpu.performance_stats(true);
            if i == 20 {
                println!("{name}: SDK performance stats after one decode: {stats:?}");
            }
            if i >= 20 {
                push.push(pushed.as_secs_f64() * 1000.0);
                total.push(finished.as_secs_f64() * 1000.0);
                let passes: Vec<f64> = stats.iter().filter_map(|line| parse_gpu_ms(line)).collect();
                if !passes.is_empty() {
                    gpu_ms.push(passes.iter().sum());
                }
            }
        }
        let gpu_text = if gpu_ms.is_empty() {
            "no GPU time parsed".to_owned()
        } else {
            format!("GPU p50 {:.3} ms p95 {:.3} ms ({} samples)", percentile(&mut gpu_ms.clone(), 50), percentile(&mut gpu_ms, 95), gpu_ms.len())
        };
        println!(
            "{name}: {size} bytes/frame; CPU push p50 {:.3} ms p95 {:.3} ms; push to planes in memory p50 {:.3} ms p95 {:.3} ms; {gpu_text}",
            percentile(&mut push.clone(), 50),
            percentile(&mut push, 95),
            percentile(&mut total.clone(), 50),
            percentile(&mut total, 95),
        );
    }
}
