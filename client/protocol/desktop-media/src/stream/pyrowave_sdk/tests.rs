//! Unit tests (no GPU) and `--ignored` GPU tests for the SDK binding.
//!
//! The GPU tests need the built SDK (scripts/build-pyrowave-sdk.ps1, or .sh on Linux): they look in
//! `%NIGHTJAR_PYROWAVE_SDK%`, else `C:\src\butterpollo-release\nj-pyrowave-sdk` (`$NIGHTJAR_PYROWAVE_SDK`,
//! else `~/nj-pyrowave-sdk` on Linux). Run them with
//! `cargo test -p nightjar-desktop --release --bin nightjar -- --ignored --nocapture --test-threads=1 pyrowave_sdk`.
//! Linux has no D3D11 side: the tests there read the planes the decoder wrote into CPU buffers.
//! They share one Vulkan device per decoder life, so a hung GPU shows as a failed test and never retries.

use std::sync::{Mutex, MutexGuard};
#[cfg(windows)]
use std::time::{Duration, Instant};

use rubylight_protocol::pyrowave::{push_container, PACKET_CONTIGUOUS, PACKET_FRAME_END, PACKET_FRAME_START, PACKET_RECORD_START};
#[cfg(windows)]
use windows::core::Interface;
#[cfg(windows)]
use windows::Win32::Graphics::Direct3D11::{
    ID3D11Device, ID3D11Device5, ID3D11DeviceContext4, ID3D11Fence, D3D11_FENCE_FLAG_NONE,
};

use super::*;
use crate::stream::pyrowave::Decoder as Cpu;

#[cfg(windows)]
mod win32_gpu;
// Unix (Linux and macOS): GPU decode tests through CPU plane readback.
#[cfg(unix)]
mod unix_gpu;

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
    // pyrowave_device_create_queue_info and pyrowave_device_create_info (the shared device):
    // a queue and two indices; the loader entry, three handles, two create infos, the queue
    // list and its count (padded), the two lock callbacks and their userdata.
    assert_eq!((size_of::<DeviceQueueInfo>(), offset_of!(DeviceQueueInfo, family_index), offset_of!(DeviceQueueInfo, index)), (16, 8, 12));
    assert_eq!(size_of::<SharedDeviceCreateInfo>(), 88);
    assert_eq!(
        (offset_of!(SharedDeviceCreateInfo, instance), offset_of!(SharedDeviceCreateInfo, device), offset_of!(SharedDeviceCreateInfo, instance_create_info), offset_of!(SharedDeviceCreateInfo, queue_info)),
        (8, 24, 32, 48),
    );
    assert_eq!(
        (offset_of!(SharedDeviceCreateInfo, queue_info_count), offset_of!(SharedDeviceCreateInfo, queue_lock_callback), offset_of!(SharedDeviceCreateInfo, queue_unlock_callback), offset_of!(SharedDeviceCreateInfo, userdata)),
        (56, 64, 72, 80),
    );
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
    // Unix (Linux and macOS): `probe_in` takes no D3D11 device parameter.
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
    // Unix (Linux and macOS): system C library used as a non-SDK shared object (skipped if absent).
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
    // Unix (Linux and macOS): shared libraries require position-independent code.
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

/// The SDK's per-decode GPU time in milliseconds from one report line, if it has one: the
/// report is "iDWT: 0.051 ms per frame", "Dequant: 0.020 ms per frame" and memory heap lines, and
/// a frame's GPU time is the sum of the passes.
fn parse_gpu_ms(line: &str) -> Option<f64> {
    super::pass_ms(line).map(|(_, ms)| ms)
}

#[test]
fn gpu_time_is_read_from_the_pass_lines_only() {
    assert_eq!(parse_gpu_ms("iDWT: 0.051 ms per frame"), Some(0.051));
    assert_eq!(super::pass_ms("Dequant: 0.020 ms per frame"), Some(("Dequant", 0.020)));
    assert_eq!(parse_gpu_ms("Memory Heap 0 (HOST): MaxSize 16346.938 MiB, BudgetSize 15578.938 MiB"), None);
    assert_eq!(parse_gpu_ms("nothing here"), None);
}

#[test]
fn the_pass_summary_sums_the_passes_and_skips_the_heap_lines() {
    let lines = ["Dequant: 0.020 ms per frame", "Memory Heap 0 (HOST): MaxSize 16346.938 MiB, BudgetSize 15578.938 MiB", "iDWT: 1.500 ms per frame"]
        .map(String::from);
    assert_eq!(super::pass_summary(&lines).as_deref(), Some("(SDK GPU timestamps, ms per frame): 1.52 in all; Dequant 0.020, iDWT 1.500"));
    assert_eq!(super::pass_summary(&lines[1..2]), None, "no pass line, no summary");
    assert_eq!(super::pass_summary(&[]), None);
}

#[test]
fn the_device_is_asked_for_the_queue_priority_its_name_says() {
    // The decoder was run and measured at Vulkan's HIGH global queue priority; the name says so.
    assert_eq!(PRIORITY_HIGH, ash::vk::QueueGlobalPriorityKHR::HIGH.as_raw());
}

// ---- Shared GPU test helpers -----------------------------------------------------------------

/// One GPU test at a time: they each make Vulkan devices on the same adapter.
static GPU: Mutex<()> = Mutex::new(());

fn gpu_lock() -> MutexGuard<'static, ()> {
    GPU.lock().unwrap_or_else(|e| e.into_inner())
}

fn sdk_dll() -> PathBuf {
    #[cfg(windows)]
    let default = PathBuf::from(r"C:\src\butterpollo-release\nj-pyrowave-sdk");
    // Unix (Linux and macOS): default SDK output directory under $HOME.
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

/// A sample code as a value on the picture's 8 or 10-bit scale (R16 holds 10-bit pictures as UNORM16).
fn code(raw: u16, ten: bool) -> f32 {
    if ten { f32::from(raw) * 1023.0 / 65535.0 } else { f32::from(raw) }
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

/// A full frame of `width`x`height` (4:4:4 with `full`, else 4:2:0) as the SDK's packets: the sequence
/// header and one record for every block, each a fixture record re-addressed. The records are picked for a frame about
/// `bytes` long, which with Rubylight's link-speed rate (300 Mbps at 60 fps) is 0.3 byte per pixel.
pub(crate) fn synthetic_frame(pool: &[Vec<u8>], width: usize, height: usize, full: bool, bytes: usize, sequence: u32) -> Vec<Vec<u8>> {
    let count = block_count(width, height, full);
    let average = bytes / count;
    let mut pool: Vec<&Vec<u8>> = pool.iter().collect();
    pool.sort_by_key(|record| record.len().abs_diff(average));
    pool.truncate(128);
    let mut header = vec![0u8; 8];
    header[..4].copy_from_slice(&(HEADER_BIT | (width as u32 - 1) | ((height as u32 - 1) << 14) | (sequence << 28)).to_le_bytes());
    header[4..].copy_from_slice(&(count as u32 | u32::from(full) << 26).to_le_bytes());
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

/// The records of the 4:2:0 fixtures, one by one, for `synthetic_frame`.
pub(crate) fn record_pool() -> Vec<Vec<u8>> {
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
    pool
}

pub(crate) fn percentile(values: &mut [f64], p: usize) -> f64 {
    values.sort_by(f64::total_cmp);
    values[(values.len() * p / 100).min(values.len() - 1)]
}

/// The decode timing tests' pictures (name, width, height, 4:4:4, 10-bit): three sizes in 8-bit
/// 4:2:0, and 4K once more in 4:4:4 and into the 16-bit planes of a 10-bit stream.
const TIMING_SIZES: [(&str, usize, usize, bool, bool); 6] = [
    ("1080p", 1920, 1080, false, false), ("1440p", 2560, 1440, false, false), ("4K", 3840, 2160, false, false),
    ("4K 10-bit", 3840, 2160, false, true), ("4K 4:4:4", 3840, 2160, true, false), ("4K 4:4:4 10-bit", 3840, 2160, true, true),
];
