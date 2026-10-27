//! The HDR statistics pass: readback parsing, and on a GPU the histogram,
//! extremes and percentiles of synthetic frames against the same samples
//! taken on the CPU.
use super::*;
use crate::capture::{ComGuard, Device, GpuImage, Image, Pixel};
use butterpollo_core::hdr_dynamic::{FrameStats, nits_to_pq};

/// Words for `samples` in `bins` (bin, count), with `min` and `max`.
fn words(min: f32, max: f32, bins: &[(usize, u32)]) -> Vec<u32> {
    let mut words = vec![0; WORDS];
    words[0] = min.to_bits();
    words[1] = max.to_bits();
    for &(bin, count) in bins {
        words[2 + bin] += count;
    }
    words
}

#[test]
fn readout_parses_consistent_words() {
    let readout = HdrStatsReadout::parse(7, 4, &words(0.25, 0.75, &[(256, 3), (768, 1)])).unwrap();
    assert_eq!(readout.conversion, 7);
    assert_eq!((readout.min_pq, readout.max_pq), (0.25, 0.75));
    assert_eq!(readout.histogram.len(), HDR_STATS_BINS);
    assert_eq!(readout.histogram[256], 3);
    let stats = readout.frame_stats().unwrap();
    assert_eq!(
        Some(stats),
        FrameStats::from_histogram(&readout.histogram, 0.25, 0.75)
    );
    // P50 reaches into bin 256 and P99 into bin 768, clamped to the extremes.
    assert_eq!(stats.percentiles_pq[4], 257. / 1024.);
    assert_eq!(stats.percentiles_pq[8], 0.75);
    // One PQ value fills one bin; the top bin holds 1.0.
    assert!(HdrStatsReadout::parse(1, 2, &words(0.5, 0.5, &[(512, 2)])).is_ok());
    assert!(HdrStatsReadout::parse(1, 1, &words(1., 1., &[(1023, 1)])).is_ok());
    assert!(HdrStatsReadout::parse(1, 1, &words(0., 0., &[(0, 1)])).is_ok());
}

#[test]
fn readout_rejects_inconsistent_words() {
    let good = words(0.25, 0.75, &[(256, 3), (768, 1)]);
    let parse = |samples, words: &[u32]| HdrStatsReadout::parse(1, samples, words);
    assert!(parse(4, &good).is_ok());
    // Wrong length, lost or extra samples, an empty histogram.
    assert!(parse(4, &good[..WORDS - 1]).is_err());
    assert!(parse(5, &good).is_err());
    assert!(parse(3, &good).is_err());
    assert!(parse(0, &words(0.25, 0.75, &[])).is_err());
    // The shader's untouched clear: no sample reached the extremes.
    let mut cleared = good.clone();
    cleared[0] = u32::MAX;
    cleared[1] = 0;
    assert!(parse(4, &cleared).is_err());
    // Extremes out of order, out of range or not a number.
    assert!(parse(4, &words(0.75, 0.25, &[(256, 3), (768, 1)])).is_err());
    assert!(parse(4, &words(0.25, 1.5, &[(256, 3), (768, 1)])).is_err());
    assert!(parse(4, &words(-0.25, 0.75, &[(256, 3), (768, 1)])).is_err());
    assert!(parse(4, &words(f32::NAN, 0.75, &[(256, 3), (768, 1)])).is_err());
    // Extremes outside the occupied bins, as a torn copy would leave them.
    assert!(parse(4, &words(0.2, 0.75, &[(256, 3), (768, 1)])).is_err());
    assert!(parse(4, &words(0.25, 0.8, &[(256, 3), (768, 1)])).is_err());
}

#[test]
fn grid_strides_large_sources_and_samples_small_ones_whole() {
    assert_eq!(grid(1920, 1080), (240, 135));
    assert_eq!(grid(3840, 2160), (240, 135));
    assert_eq!(grid(7680, 4320), (240, 135));
    assert_eq!(grid(96, 48), (96, 48));
    assert_eq!(grid(0, 0), (1, 1));
}

struct Rig {
    _com: ComGuard,
    gpu: Device,
    compute: Arc<Compute>,
}
impl Rig {
    fn new() -> Result<Self> {
        let _com = ComGuard::new()?;
        let gpu = Device::new("")?;
        let compute = Compute::for_device(&gpu.device)?;
        Ok(Self { _com, gpu, compute })
    }
    /// A converter for `image`, an HDR stream unless it is BGRA, with the
    /// statistics on.
    fn converter(&self, image: &Image) -> Result<Converter> {
        let hdr = image.pixel != Pixel::Bgra8;
        let mut converter = Converter::new(self.compute.clone(), image.width, image.height, hdr)?;
        converter.values = crate::gpu_color::constants(
            &butterpollo_core::rtsp::Negotiated {
                width: image.width,
                height: image.height,
                codec: 1,
                hdr,
                ..Default::default()
            },
            (image.width, image.height, image.pixel),
            [100., 1.],
            None,
        );
        converter.enable_hdr_stats()?;
        Ok(converter)
    }
    /// Convert `image` as a pooled capture, leaving its statistics queued.
    /// Returns the conversion, its slot and the capture's readers.
    fn queue(
        &self,
        converter: &mut Converter,
        image: &Image,
    ) -> Result<(Converted, usize, Arc<AtomicU64>)> {
        let mut source = GpuImage::upload(&self.gpu, image)?;
        let readers = Arc::new(AtomicU64::new(0));
        source.ready = Some(Ready {
            fence: self.compute.fence.clone(),
            value: 0,
            device: self.compute.device.as_raw() as usize,
            readers: Some(readers.clone()),
            reader_fence: Some(self.compute.fence.clone()),
        });
        let opened = self.compute.open(&source.texture)?;
        let converted = converter.convert(&opened, format(image.pixel), None, &source)?;
        let slot = (0..SLOTS)
            .find(|&slot| converter.slots[slot].fence == converted.conversion)
            .context("the conversion's slot")?;
        assert_eq!(readers.load(Ordering::Acquire), converted.conversion);
        Ok((converted, slot, readers))
    }
    /// [`Rig::queue`], submit the statistics, then wait for everything on the
    /// queue. The capture's readers must cover the pass, which still reads
    /// the source after the conversion.
    fn convert(&self, converter: &mut Converter, image: &Image) -> Result<Converted> {
        let (converted, slot, readers) = self.queue(converter, image)?;
        converter.submit_hdr_stats()?;
        assert_ne!(converter.slots[slot].stats, QUEUED);
        assert_eq!(
            readers.load(Ordering::Acquire),
            converter.slots[slot].last(),
            "the capture was handed back before every read of its source"
        );
        converted.wait()?;
        self.compute.wait(self.compute.signal()?)?;
        Ok(converted)
    }
}
/// Grey scRGB FP16 at `nits(x, y)`.
fn scrgb(width: u32, height: u32, nits: impl Fn(u32, u32) -> f32) -> Image {
    let mut bytes = Vec::with_capacity((width * height * 8) as usize);
    for y in 0..height {
        for x in 0..width {
            let grey = half::f16::from_f32(nits(x, y) / 80.)
                .to_bits()
                .to_le_bytes();
            for _ in 0..3 {
                bytes.extend_from_slice(&grey);
            }
            bytes.extend_from_slice(&half::f16::ONE.to_bits().to_le_bytes());
        }
    }
    Image {
        width,
        height,
        stride: width as usize * 8,
        bytes,
        captured: std::time::Instant::now(),
        pixel: Pixel::RgbaF16,
    }
}
/// Grey PQ R10G10B10A2 at `code(x, y)`.
fn pq10(width: u32, height: u32, code: impl Fn(u32, u32) -> u32) -> Image {
    let mut bytes = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            let code = code(x, y).min(1023);
            bytes.extend_from_slice(&(code | code << 10 | code << 20 | 3 << 30).to_le_bytes());
        }
    }
    Image {
        width,
        height,
        stride: width as usize * 4,
        bytes,
        captured: std::time::Instant::now(),
        pixel: Pixel::Rgba10Pq,
    }
}
/// The PQ value each pixel of `image` holds, as written.
fn pixel_pq(image: &Image, x: u32, y: u32) -> f32 {
    let at = y as usize * image.stride;
    match image.pixel {
        Pixel::RgbaF16 => {
            let at = at + x as usize * 8;
            let grey = half::f16::from_le_bytes([image.bytes[at], image.bytes[at + 1]]);
            nits_to_pq(grey.to_f32() * 80.)
        }
        Pixel::Rgba10Pq => {
            let at = at + x as usize * 4;
            let word = u32::from_le_bytes(image.bytes[at..at + 4].try_into().unwrap());
            (word & 1023) as f32 / 1023.
        }
        Pixel::Bgra8 => unreachable!("an SDR source"),
    }
}
/// The CPU's own samples at the pass's cell centres: the histogram and extremes.
fn sampled(image: &Image) -> (Vec<u32>, f32, f32) {
    let (columns, rows) = grid(image.width, image.height);
    let mut histogram = vec![0; HDR_STATS_BINS];
    let (mut min, mut max) = (f32::MAX, 0f32);
    for row in 0..rows {
        for column in 0..columns {
            let x = (2 * column + 1) * image.width / (2 * columns);
            let y = (2 * row + 1) * image.height / (2 * rows);
            let pq = pixel_pq(image, x, y);
            histogram[((pq * 1024.) as usize).min(1023)] += 1;
            min = min.min(pq);
            max = max.max(pq);
        }
    }
    (histogram, min, max)
}
fn close(actual: f32, expected: f32, tolerance: f32, what: &str) {
    assert!(
        (actual - expected).abs() <= tolerance,
        "{what}: {actual} is not within {tolerance} of {expected}"
    );
}
/// Convert `image` and check its statistics against [`sampled`]; returns them.
fn check(rig: &Rig, image: &Image, what: &str) -> Result<FrameStats> {
    let mut converter = rig.converter(image)?;
    let converted = rig.convert(&mut converter, image)?;
    let readout = converter
        .latest_hdr_stats(0)?
        .with_context(|| format!("{what}: no statistics"))?;
    assert_eq!(readout.conversion, converted.conversion, "{what}");
    let (histogram, min, max) = sampled(image);
    let (columns, rows) = grid(image.width, image.height);
    let mass: u64 = readout
        .histogram
        .iter()
        .map(|&count| u64::from(count))
        .sum();
    assert_eq!(mass, u64::from(columns * rows), "{what}: histogram mass");
    close(readout.min_pq, min, 2e-3, &format!("{what} min"));
    close(readout.max_pq, max, 2e-3, &format!("{what} max"));
    let gpu = readout.frame_stats().context("GPU frame statistics")?;
    let cpu = FrameStats::from_histogram(&histogram, min, max).context("CPU frame statistics")?;
    for (slot, percentage) in [(4, "P50"), (8, "P99")] {
        close(
            gpu.percentiles_pq[slot],
            cpu.percentiles_pq[slot],
            2.5 / 1024.,
            &format!("{what} {percentage}"),
        );
    }
    close(gpu.mean_pq, cpu.mean_pq, 2e-3, &format!("{what} mean"));
    Ok(gpu)
}

#[test]
#[ignore = "requires D3D12 compute"]
fn hdr_stats_measure_flat_and_split_frames() -> Result<()> {
    let rig = Rig::new()?;
    let (width, height) = (1920, 1080);
    let half = width / 2;
    let peak = nits_to_pq(4000.);
    let frames = [
        ("scRGB 100 nits", scrgb(width, height, |_, _| 100.), 100.),
        ("scRGB 1000 nits", scrgb(width, height, |_, _| 1000.), 1000.),
        (
            "PQ 1000 nits",
            pq10(width, height, |_, _| {
                (nits_to_pq(1000.) * 1023.).round() as u32
            }),
            1000.,
        ),
    ];
    for (what, image, nits) in &frames {
        let stats = check(&rig, image, what)?;
        let pq = pixel_pq(image, 0, 0);
        close(pq, nits_to_pq(*nits), 1e-3, what);
        close(stats.min_pq, pq, 2e-3, what);
        close(stats.max_pq, pq, 2e-3, what);
        close(stats.percentiles_pq[4], pq, 2e-3, what);
        close(stats.percentiles_pq[8], pq, 2e-3, what);
    }
    // Black on the left half, a ramp to 4000 nits on the right, linear in
    // light (scRGB) or in PQ. P99 is 98% up the ramp.
    let split = [
        (
            "scRGB split",
            scrgb(width, height, |x, _| {
                if x < half {
                    0.
                } else {
                    4000. * (x - half + 1) as f32 / half as f32
                }
            }),
            nits_to_pq(0.98 * 4000.),
        ),
        (
            "PQ split",
            pq10(width, height, |x, _| {
                if x < half {
                    0
                } else {
                    (1023. * peak * (x - half + 1) as f32 / half as f32).round() as u32
                }
            }),
            0.98 * peak,
        ),
    ];
    for (what, image, p99) in &split {
        let stats = check(&rig, image, what)?;
        close(stats.min_pq, 0., 1e-6, what);
        // The last cell centre is a few pixels short of the ramp's end.
        close(stats.max_pq, peak, 4e-3, what);
        // Half the samples are black: P50 is the first bin's upper edge.
        close(stats.percentiles_pq[4], 1. / 1024., 1e-6, what);
        close(stats.percentiles_pq[8], *p99, 4e-3, what);
    }
    Ok(())
}

#[test]
#[ignore = "requires D3D12 compute"]
fn hdr_stats_follow_hdr_conversions_only() -> Result<()> {
    let rig = Rig::new()?;
    let sdr = Image {
        width: 64,
        height: 32,
        stride: 64 * 4,
        bytes: vec![200; 64 * 32 * 4],
        captured: std::time::Instant::now(),
        pixel: Pixel::Bgra8,
    };
    let mut converter = rig.converter(&sdr)?;
    rig.convert(&mut converter, &sdr)?;
    assert!(converter.latest_hdr_stats(0)?.is_none());
    let hdr = scrgb(64, 32, |x, _| 10. * x as f32);
    let mut converter = rig.converter(&hdr)?;
    let mut newest = 0;
    for _ in 0..SLOTS + 2 {
        let converted = rig.convert(&mut converter, &hdr)?;
        // Each pass runs behind the encoder's signal, in its own submission.
        let slot = (0..SLOTS)
            .find(|&slot| converter.slots[slot].fence == converted.conversion)
            .context("the conversion's slot")?;
        assert!(converter.slots[slot].stats > converted.conversion);
        let readout = converter
            .latest_hdr_stats(newest)?
            .context("statistics for the conversion")?;
        assert_eq!(readout.conversion, converted.conversion);
        assert!(converter.latest_hdr_stats(converted.conversion)?.is_none());
        newest = converted.conversion;
    }
    // Back to SDR: the slot it converts in loses its statistics.
    converter.values[5] = 0;
    converter.values[4] = 1;
    let converted = rig.convert(&mut converter, &hdr)?;
    let slot = (0..SLOTS)
        .find(|&slot| converter.slots[slot].fence == converted.conversion)
        .context("the conversion's slot")?;
    assert_eq!(converter.slots[slot].stats, 0);
    let stats = converter.hdr_stats.as_deref().context("statistics on")?;
    assert_eq!(stats.slots[slot].queued, None);
    assert_eq!(stats.slots[slot].measured, None);
    assert!(
        converter
            .latest_hdr_stats(converted.conversion - 1)?
            .is_none()
    );
    Ok(())
}

#[test]
#[ignore = "requires D3D12 compute"]
fn hdr_stats_skip_captures_read_on_another_timeline() -> Result<()> {
    let rig = Rig::new()?;
    let hdr = scrgb(64, 32, |_, _| 5.);
    let mut converter = rig.converter(&hdr)?;
    let mut source = GpuImage::upload(&rig.gpu, &hdr)?;
    // SAFETY: A new fence on the queue's live device.
    let other: ID3D12Fence = unsafe { rig.compute.device.CreateFence(0, D3D12_FENCE_FLAG_NONE)? };
    source.ready = Some(Ready {
        fence: rig.compute.fence.clone(),
        value: 0,
        device: rig.compute.device.as_raw() as usize,
        readers: Some(Arc::new(AtomicU64::new(0))),
        reader_fence: Some(other),
    });
    let opened = rig.compute.open(&source.texture)?;
    let converted = converter.convert(&opened, format(hdr.pixel), None, &source)?;
    // Its pass would make `submit_hdr_stats` wait for the GPU on the CPU.
    let slot = (0..SLOTS)
        .find(|&slot| converter.slots[slot].fence == converted.conversion)
        .context("the conversion's slot")?;
    assert_eq!(converter.slots[slot].stats, 0);
    converter.submit_hdr_stats()?;
    assert_eq!(converter.slots[slot].stats, 0);
    Ok(())
}

/// Opens a gate the queue waits behind, also when the test fails first.
struct Gate(ID3D12Fence);
impl Drop for Gate {
    fn drop(&mut self) {
        // SAFETY: A CPU-side signal of a live fence.
        let _ = unsafe { self.0.Signal(1) };
    }
}

#[test]
#[ignore = "requires D3D12 compute"]
fn hdr_stats_pass_holds_the_capture_until_it_has_run() -> Result<()> {
    let rig = Rig::new()?;
    let hdr = scrgb(64, 32, |x, _| 10. * x as f32);
    let mut converter = rig.converter(&hdr)?;
    let (converted, slot, readers) = rig.queue(&mut converter, &hdr)?;
    // `convert` submits nothing for the statistics: the encoder's frame is ready without them.
    assert_eq!(converter.slots[slot].stats, QUEUED);
    converted.wait()?;
    rig.compute.wait(rig.compute.signal()?)?;
    converter.release_completed();
    assert!(
        converter.slots[slot].capture.is_some(),
        "a queued pass lost its source"
    );
    // A pass the GPU has not run yet keeps the capture and has no readout.
    // SAFETY: A new fence on the queue's live device; the queue waits for it until `gate`
    // drops, before `converter` does.
    let gate = unsafe {
        let gate = Gate(rig.compute.device.CreateFence(0, D3D12_FENCE_FLAG_NONE)?);
        rig.compute.queue.Wait(&gate.0, 1)?;
        gate
    };
    converter.submit_hdr_stats()?;
    let pass = converter.slots[slot].stats;
    assert!(pass > converted.conversion && pass != QUEUED);
    assert_eq!(readers.load(Ordering::Acquire), pass);
    converter.release_completed();
    assert!(
        converter.slots[slot].capture.is_some(),
        "an unfinished pass lost its source"
    );
    assert!(converter.latest_hdr_stats(0)?.is_none());
    drop(gate);
    rig.compute.wait(rig.compute.signal()?)?;
    converter.release_completed();
    assert!(converter.slots[slot].capture.is_none());
    let readout = converter
        .latest_hdr_stats(0)?
        .context("statistics once the pass ran")?;
    assert_eq!(readout.conversion, converted.conversion);
    Ok(())
}

/// The pass's GPU time on this machine, by timestamps around a recorded
/// pass (`cargo test ... hdr_stats_gpu_time -- --ignored --nocapture`).
#[test]
#[ignore = "requires D3D12 compute and GPU timestamp queries; prints the pass's GPU time"]
fn hdr_stats_gpu_time() -> Result<()> {
    let rig = Rig::new()?;
    let ramp = |width: u32| move |x: u32, _: u32| 4000. * x as f32 / width as f32;
    let frames = [
        ("1080p scRGB", scrgb(1920, 1080, ramp(1920))),
        ("4K scRGB", scrgb(3840, 2160, ramp(3840))),
        ("4K PQ", pq10(3840, 2160, |x, _| x * 1023 / 3840)),
    ];
    let device = &rig.compute.device;
    // SAFETY: Test-owned objects on the live device; every list is waited for before its
    // allocator is reset or its results are read, and the converter's slot 0 views and
    // constants stay unchanged while no conversion runs.
    unsafe {
        let mut queries: Option<ID3D12QueryHeap> = None;
        device.CreateQueryHeap(
            &D3D12_QUERY_HEAP_DESC {
                Type: D3D12_QUERY_HEAP_TYPE_TIMESTAMP,
                Count: 2,
                NodeMask: 0,
            },
            &mut queries,
        )?;
        let queries = queries.context("no timestamp query heap")?;
        let ticks = focus_buffer(
            device,
            D3D12_HEAP_TYPE_READBACK,
            16,
            D3D12_RESOURCE_STATE_COPY_DEST,
        )?;
        let allocator: ID3D12CommandAllocator =
            device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_COMPUTE)?;
        let list: ID3D12GraphicsCommandList =
            device.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_COMPUTE, &allocator, None)?;
        list.Close()?;
        let frequency = rig.compute.queue.GetTimestampFrequency()? as f64;
        for (what, image) in &frames {
            let mut converter = rig.converter(image)?;
            rig.convert(&mut converter, image)?;
            let stats = converter.hdr_stats.as_deref().context("statistics")?;
            let views = converter.heap.GetGPUDescriptorHandleForHeapStart();
            let config = converter.constants.GetGPUVirtualAddress();
            let mut times = Vec::new();
            for _ in 0..64 {
                allocator.Reset()?;
                list.Reset(&allocator, None)?;
                list.EndQuery(&queries, D3D12_QUERY_TYPE_TIMESTAMP, 0);
                stats.record(
                    &list,
                    &converter.heap,
                    views,
                    config,
                    grid(image.width, image.height),
                    &stats.slots[0].readback,
                );
                list.EndQuery(&queries, D3D12_QUERY_TYPE_TIMESTAMP, 1);
                list.ResolveQueryData(&queries, D3D12_QUERY_TYPE_TIMESTAMP, 0, 2, &ticks, 0);
                list.Close()?;
                rig.compute.wait(rig.compute.execute(&list)?)?;
                let mut mapped = std::ptr::null_mut();
                ticks.Map(0, None, Some(&mut mapped))?;
                let [start, end] = *mapped.cast::<[u64; 2]>();
                ticks.Unmap(0, None);
                times.push(end.saturating_sub(start) as f64 / frequency * 1e6);
            }
            // The pass left the conversion's statistics in slot 0's readback.
            let samples = {
                let (columns, rows) = grid(image.width, image.height);
                u64::from(columns * rows)
            };
            let words = std::slice::from_raw_parts(stats.slots[0].mapped, WORDS);
            HdrStatsReadout::parse(0, samples, words)?;
            times.sort_by(f64::total_cmp);
            println!(
                "{what} {}x{}: HDR statistics pass {:.1} us median, {:.1} us min, {:.1} us p90",
                image.width,
                image.height,
                times[times.len() / 2],
                times[0],
                times[times.len() * 9 / 10],
            );
        }
    }
    Ok(())
}
