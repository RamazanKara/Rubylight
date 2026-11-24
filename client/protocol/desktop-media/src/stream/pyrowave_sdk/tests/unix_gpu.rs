//! Unix (Linux and macOS) Vulkan CPU-readback `--ignored` GPU tests for the PyroWave SDK binding.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use rubylight_protocol::pyrowave::Records;

use super::*;
use crate::stream::pyrowave::Decoder as Cpu;

/// The three plane buffers of `config`, zeroed.
fn plane_buffers(config: GpuConfig) -> [Vec<u8>; 3] {
    [0, 1, 2].map(|plane| vec![0u8; config.plane_bytes(plane)])
}

/// The raw sample codes of CPU plane buffers, the way the Windows tests read R8 and R16 textures.
fn codes(buffers: &[Vec<u8>; 3], config: GpuConfig) -> [Vec<u16>; 3] {
    std::array::from_fn(|plane| {
        let (width, height) = config.plane_size(plane);
        assert_eq!(buffers[plane].len(), config.plane_bytes(plane));
        let samples: Vec<u16> = if config.ten_bit {
            buffers[plane].as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect()
        } else {
            buffers[plane].iter().map(|&b| u16::from(b)).collect()
        };
        assert_eq!(samples.len(), (width * height) as usize);
        samples
    })
}

/// Pushes the packets of one frame and decodes it into fresh buffers.
fn push_and_decode(gpu: &mut GpuDecoder, packets: &[Vec<u8>]) -> [Vec<u16>; 3] {
    let config = gpu.config();
    let mut buffers = plane_buffers(config);
    push_and_decode_into(gpu, packets, &mut buffers);
    codes(&buffers, config)
}

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
#[test]
#[ignore = "needs the PyroWave SDK and a Vulkan GPU"]
fn decode_timing_at_1080p_1440p_and_4k() {
    let _gpu = gpu_lock();
    let api = sdk();
    let pool = record_pool();
    for (name, width, height, chroma444, ten_bit) in TIMING_SIZES {
        let config = GpuConfig { width: width as u32, height: height as u32, chroma444, ten_bit, slots: 1 };
        let mut gpu = GpuDecoder::with_api(api.clone(), config).unwrap();
        let frames: Vec<Vec<Vec<u8>>> = (0..8).map(|s| synthetic_frame(&pool, width, height, chroma444, width * height * 3 / 10, s)).collect();
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
