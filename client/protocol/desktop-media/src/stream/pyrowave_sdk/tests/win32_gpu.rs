//! Windows Direct3D 11 / Vulkan interop `--ignored` GPU tests for the PyroWave SDK binding.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use rubylight_protocol::pyrowave::Records;
use windows::core::Interface;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_R16_UNORM, DXGI_FORMAT_R8_UNORM, DXGI_SAMPLE_DESC};

use super::*;
use crate::stream::pyrowave::Decoder as Cpu;

/// Copies decoded planes to the CPU for comparison.
struct Readback {
    context: ID3D11DeviceContext,
    context4: ID3D11DeviceContext4,
    staging: Vec<ID3D11Texture2D>,
    config: GpuConfig,
}

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

fn push_and_decode(gpu: &mut GpuDecoder, packets: &[Vec<u8>]) -> u64 {
    push_and_decode_to(gpu, packets, 0, 0)
}

/// Decodes into `slot` once the reuse fence reaches `after`.
fn push_and_decode_to(gpu: &mut GpuDecoder, packets: &[Vec<u8>], slot: usize, after: u64) -> u64 {
    gpu.clear();
    for packet in packets {
        gpu.push_packet(packet).unwrap();
    }
    assert!(gpu.is_ready(false), "the pushed packets did not complete the frame");
    gpu.decode(slot, after).unwrap()
}

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
    // The same records into 16-bit planes: what a 10-bit stream's decode adds (its output).
    for (name, width, height, chroma444, ten_bit) in TIMING_SIZES {
        let config = GpuConfig { width: width as u32, height: height as u32, chroma444, ten_bit, slots: 1 };
        let mut gpu = GpuDecoder::with_api(api.clone(), &d3d, config).unwrap();
        let frames: Vec<Vec<Vec<u8>>> = (0..8).map(|s| synthetic_frame(&pool, width, height, chroma444, width * height * 3 / 10, s)).collect();
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
struct Handoff {
    context: ID3D11DeviceContext,
    context4: ID3D11DeviceContext4,
    query: ID3D11Query,
}

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

/// The Vulkan side must not overwrite a plane set while D3D11 may still read the picture in it: the
/// decode waits on the reuse fence. Deterministic: the "presenter" is the test, which holds the
/// signal back and checks the decode has not completed, then releases it.
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
