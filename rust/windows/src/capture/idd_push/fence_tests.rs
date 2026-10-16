//! Native driver-transport checks using two GPU devices; no driver installation.
use super::*;

/// The driver's half of a fence ring, on a second D3D11 device as WUDFHost has: the
/// ring's textures and fences opened from their NT handles.
struct FakeDriver {
    gpu: Device,
    textures: Vec<ID3D11Texture2D>,
    written: ID3D11Fence,
    released: ID3D11Fence,
    context: ID3D11DeviceContext4,
    picker: ring::Picker,
}

impl FakeDriver {
    fn open(ring: &Ring) -> Result<Self> {
        let gpu = Device::new("")?;
        let device1: ID3D11Device1 = gpu.device.cast()?;
        let device5: ID3D11Device5 = gpu.device.cast()?;
        let mut textures = Vec::new();
        for slot in &ring.slots {
            // SAFETY: a live shared NT handle of a D3D12 texture.
            let texture: ID3D11Texture2D =
                unsafe { device1.OpenSharedResource1(HANDLE(slot.shared.as_raw_handle()))? };
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            // SAFETY: live texture, live local.
            unsafe { texture.GetDesc(&mut desc) };
            anyhow::ensure!(
                desc.MiscFlags & D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX.0 as u32 == 0,
                "a fence ring texture has no keyed mutex"
            );
            textures.push(texture);
        }
        let open = |handle: &OwnedHandle| -> Result<ID3D11Fence> {
            let mut fence = None;
            // SAFETY: a live shared fence handle; `fence` is a live local.
            unsafe { device5.OpenSharedFence(HANDLE(handle.as_raw_handle()), &mut fence)? };
            fence.context("no fence")
        };
        Ok(Self {
            written: open(&ring.fences.handles[0])?,
            released: open(&ring.fences.handles[1])?,
            context: gpu.context.cast()?,
            textures,
            gpu,
            picker: ring::Picker::new(ring.slots.len()),
        })
    }

    /// Copy `source` into a free slot, signal the written fence and publish, as the
    /// driver's swap-chain worker does. `None` when it drops the frame.
    fn push(&mut self, header: &Header<'_>, source: &ID3D11Texture2D) -> Result<Option<u32>> {
        let Some(claim) = self.picker.claim_fenced(header, self.released()) else {
            return Ok(None);
        };
        let seq = self.picker.next_seq();
        // SAFETY: same device, size and format; the slot is ours in `Writing`.
        unsafe {
            self.gpu
                .context
                .CopyResource(&self.textures[claim.slot()], source);
            self.context.Signal(&self.written, u64::from(seq))?;
            self.gpu.context.Flush();
        }
        header.write_slot(
            claim.slot(),
            SlotMeta {
                seq: u64::from(seq),
                ..Default::default()
            },
        );
        anyhow::ensure!(self.picker.publish_fenced(header, claim.slot(), seq));
        header.publish(ring::Token {
            generation: header.layout().unwrap().generation,
            seq,
            slot: claim.slot() as u8,
        });
        Ok(Some(seq))
    }

    fn released(&self) -> u64 {
        // SAFETY: a plain read of a fence opened on this device.
        unsafe { self.released.GetCompletedValue() }
    }
}

fn solid(gpu: &Device, value: u8) -> Result<GpuImage> {
    GpuImage::upload(gpu, &pixels(16, 16, Pixel::Bgra8, value))
}

fn pixels(width: u32, height: u32, pixel: Pixel, value: u8) -> Image {
    let stride = width as usize * if pixel == Pixel::RgbaF16 { 8 } else { 4 };
    let bytes = if pixel == Pixel::RgbaF16 {
        // Finite half-float inputs: 0.125, 0.25 or 0.5, with exact byte comparisons.
        let code = 0x3000u16 + u16::from(value % 3) * 0x400;
        code.to_le_bytes().repeat(stride * height as usize / 2)
    } else {
        vec![value; stride * height as usize]
    };
    Image {
        width,
        height,
        stride,
        bytes,
        captured: Instant::now(),
        pixel,
    }
}

fn until(what: &str, mut done: impl FnMut() -> bool) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(2);
    while !done() {
        anyhow::ensure!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(1));
    }
    Ok(())
}

#[test]
#[ignore = "requires native D3D11/D3D12 sharing and compute conversion"]
fn a_fence_ring_carries_frames_both_ways_without_keyed_mutexes() -> Result<()> {
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    let compute = crate::compute::Compute::for_device(&gpu.device)?;
    for (in_place, pixel) in [
        (true, Pixel::Bgra8),
        (false, Pixel::Bgra8),
        (true, Pixel::RgbaF16),
        (false, Pixel::RgbaF16),
    ] {
        let layout = Layout {
            kind: Kind::Fence,
            slot_count: 2,
            width: 16,
            height: 16,
            format: crate::compute::format(pixel).0 as u32,
            generation: 1,
        };
        let ring = Ring::fenced(&gpu, compute.clone(), layout, in_place)?;
        // The AMF and PyroWave compute paths take only textures they can share.
        for slot in &ring.slots {
            anyhow::ensure!(
                crate::compute::shareable(&slot.texture),
                "a D3D12 ring texture must look shareable to the encoders"
            );
            compute.open(&slot.texture)?;
        }
        let fences = ring.fences.clone();
        let header = ring.section.header();
        let mut driver = FakeDriver::open(&ring)?;
        let mut consumer = Consumer::new(1, 2);
        let mut pool = GpuPool::default();
        let ten_bit = pixel == Pixel::RgbaF16;
        let mut converter = crate::compute::Converter::new(compute.clone(), 16, 16, ten_bit)?;
        for (frame, value) in [(1u32, 40u8), (2, 80), (3, 120)] {
            let expected = pixels(16, 16, pixel, value);
            let source = GpuImage::upload(&driver.gpu, &expected)?;
            let seq = driver
                .push(&header, &source.texture)?
                .context("a free slot")?;
            assert_eq!(seq, frame);
            let (index, claimed) = consumer.claim(&header).context("the newest frame")?;
            assert_eq!(claimed, seq);
            consumer.consumed(u64::from(seq));
            let image = if in_place {
                let lent = Lent {
                    fences: fences.clone(),
                    slot: index,
                    seq,
                    d3d11: Mutex::new(false),
                };
                let ready =
                    crate::compute::Ready::on(&compute, fences.written.clone(), u64::from(seq));
                let image = lend_fenced(&gpu, &ring.slots[index].texture, lent, ready)?;
                // The compute conversion waits for the driver's copy on the GPU.
                let opened = compute.open(&image.texture)?;
                converter
                    .convert(&opened, crate::compute::format(pixel), &image)?
                    .wait()?;
                anyhow::ensure!(
                    matches!(header.slot_state(index), ring::SlotState::Reading { .. }),
                    "lent until the image goes"
                );
                image
            } else {
                // SAFETY: the fence is opened on this context's device.
                unsafe { fences.context.Wait(&fences.written11, u64::from(seq))? };
                let image = pool.copy(&gpu, &ring.slots[index].texture)?.unwrap();
                fences.free(index, seq, false)?;
                image
            };
            // A D3D11 reader sees the driver's pixels, never an older frame.
            assert_eq!(image.readback(&mut None)?.bytes, expected.bytes);
            drop(image);
            // The converter keeps the image of its last conversion until its slot
            // comes round again; a new converter lets it go.
            converter = crate::compute::Converter::new(compute.clone(), 16, 16, ten_bit)?;
            // Freed behind every read: the driver may reuse the slot once the released
            // fence passes the value recorded in it.
            let ring::SlotState::Free { release } = header.slot_state(index) else {
                bail!("slot {index} still held: {:?}", header.slot_state(index));
            };
            until("the released fence", || driver.released() >= release)?;
        }
    }
    Ok(())
}

fn claim_image(gpu: &Device, ring: &Ring, consumer: &mut Consumer) -> Result<GpuImage> {
    let header = ring.section.header();
    let fences = &ring.fences;
    let (index, seq) = consumer
        .claim(&header)
        .context("claim newest fenced frame")?;
    consumer.consumed(u64::from(seq));
    let lent = Lent {
        fences: fences.clone(),
        slot: index,
        seq,
        d3d11: Mutex::new(false),
    };
    let ready = crate::compute::Ready::on(&fences.compute, fences.written.clone(), u64::from(seq));
    lend_fenced(gpu, &ring.slots[index].texture, lent, ready)
}

#[test]
#[ignore = "requires native AMD D3D11/D3D12 and the PyroWave runtime; does not install a driver"]
fn pyrowave_encoding_frees_fence_ring_slots() -> Result<()> {
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    let compute = crate::compute::Compute::for_device(&gpu.device)?;
    let layout = Layout {
        kind: Kind::Fence,
        slot_count: 4,
        width: 64,
        height: 64,
        format: crate::compute::format(Pixel::Bgra8).0 as u32,
        generation: 1,
    };
    let ring = Ring::fenced(&gpu, compute, layout, true)?;
    let mut driver = FakeDriver::open(&ring)?;
    let source = GpuImage::upload(
        &driver.gpu,
        &pixels(layout.width, layout.height, Pixel::Bgra8, 80),
    )?;
    let mut consumer = Consumer::new(1, layout.slot_count);
    let config = butterpollo_core::rtsp::Negotiated {
        width: layout.width,
        height: layout.height,
        fps: 144,
        codec: 3,
        bitrate_kbps: 100_000,
        ..Default::default()
    };
    let warnings = Arc::new(butterpollo_core::session::Warnings::default());
    let mut encoder = crate::pyrowave::Encoder::new_device_reported(
        &config,
        gpu.clone(),
        &Default::default(),
        warnings.clone(),
    )?;
    for frame in 1..=16 {
        let deadline = Instant::now() + Duration::from_secs(2);
        while driver
            .push(&ring.section.header(), &source.texture)?
            .is_none()
        {
            anyhow::ensure!(
                Instant::now() < deadline,
                "all four capture slots remain held before frame {frame}"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        let image = claim_image(&gpu, &ring, &mut consumer)?;
        encoder.restart_interval();
        assert_eq!(
            encoder.encode_gpu(&image, true, config.bitrate_kbps)?.len(),
            1
        );
        assert!(warnings.snapshot().is_empty());
    }
    Ok(())
}

#[test]
#[ignore = "requires native D3D11/D3D12 shared textures; does not install a driver"]
fn a_fence_slot_waits_for_all_readers_and_survives_ring_replacement() -> Result<()> {
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    let compute = crate::compute::Compute::for_device(&gpu.device)?;
    let layout = Layout {
        kind: Kind::Fence,
        slot_count: 2,
        width: 16,
        height: 16,
        format: ring::FORMAT_BGRA8,
        generation: 1,
    };
    let ring = Ring::fenced(&gpu, compute.clone(), layout, true)?;
    let owner = Arc::downgrade(&ring.fences);
    let mut driver = FakeDriver::open(&ring)?;
    let source = solid(&driver.gpu, 40)?;
    let mut consumer = Consumer::new(1, 2);
    assert!(
        driver
            .push(&ring.section.header(), &source.texture)?
            .is_some()
    );
    let first = claim_image(&gpu, &ring, &mut consumer)?;
    let second_reader = first.clone();
    drop(first);
    assert!(
        driver
            .push(&ring.section.header(), &source.texture)?
            .is_some()
    );
    let second = claim_image(&gpu, &ring, &mut consumer)?;
    assert_eq!(
        driver.push(&ring.section.header(), &source.texture)?,
        None,
        "both slots still have readers"
    );
    drop(second);
    until("a returned slot", || driver.released() >= 1)?;
    assert!(
        driver
            .push(&ring.section.header(), &source.texture)?
            .is_some()
    );
    assert_eq!(
        second_reader.readback(&mut None)?.bytes,
        vec![40; 16 * 16 * 4]
    );
    // A refresh/HDR reconfiguration can discard the session's old ring while an encoder
    // still holds its last image. That image must still import and convert successfully.
    drop(ring);
    assert!(owner.upgrade().is_some());
    let input = compute.open(&second_reader.texture)?;
    let mut converter = crate::compute::Converter::new(compute, 16, 16, false)?;
    converter
        .convert(&input, DXGI_FORMAT_B8G8R8A8_UNORM, &second_reader)?
        .wait()?;
    drop(second_reader);
    drop(converter);
    assert!(
        owner.upgrade().is_none(),
        "last reader releases the old imports and fences"
    );
    Ok(())
}

#[test]
#[ignore = "requires native shared GPU fences; verifies bounded delayed completion without a driver"]
fn a_returned_slot_waits_for_gpu_completion_before_the_driver_reuses_it() -> Result<()> {
    use windows::Win32::Graphics::Direct3D12::{D3D12_FENCE_FLAG_NONE, ID3D12Fence};

    // Even an assertion/error below must release the private gate before any resource
    // cleanup. No driver, GPU reset, or unbounded GPU wait is part of this test.
    struct Gate(ID3D12Fence);
    impl Drop for Gate {
        fn drop(&mut self) {
            // SAFETY: CPU-signalling our own live test fence releases its queued wait.
            let _ = unsafe { self.0.Signal(1) };
        }
    }

    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    let compute = crate::compute::Compute::for_device(&gpu.device)?;
    let ring = Ring::fenced(
        &gpu,
        compute.clone(),
        Layout {
            kind: Kind::Fence,
            slot_count: 2,
            width: 16,
            height: 16,
            format: ring::FORMAT_BGRA8,
            generation: 1,
        },
        true,
    )?;
    let mut driver = FakeDriver::open(&ring)?;
    let source = solid(&driver.gpu, 40)?;
    let mut consumer = Consumer::new(1, 2);
    let header = ring.section.header();
    anyhow::ensure!(driver.push(&header, &source.texture)?.is_some());
    let first = claim_image(&gpu, &ring, &mut consumer)?;
    anyhow::ensure!(driver.push(&header, &source.texture)?.is_some());
    let second = claim_image(&gpu, &ring, &mut consumer)?;
    // SAFETY: the new fence belongs to the compute queue's device.
    let gate = Gate(unsafe { compute.device.CreateFence(0, D3D12_FENCE_FLAG_NONE)? });
    // SAFETY: this private wait is always released by Gate::drop, including unwinding.
    unsafe { compute.queue.Wait(&gate.0, 1)? };
    drop(first);
    let release = (0..2)
        .find_map(|slot| match header.slot_state(slot) {
            ring::SlotState::Free { release } => Some(release),
            _ => None,
        })
        .context("the released image returns its metadata slot")?;
    assert!(release > 0);
    assert!(driver.released() < release, "GPU completion is still gated");
    assert_eq!(
        driver.push(&header, &source.texture)?,
        None,
        "Free metadata cannot bypass the pending GPU release fence"
    );
    drop(gate);
    until("the gated GPU release", || driver.released() >= release)?;
    anyhow::ensure!(driver.push(&header, &source.texture)?.is_some());
    assert_eq!(second.readback(&mut None)?.bytes, vec![40; 16 * 16 * 4]);
    Ok(())
}

#[test]
#[ignore = "requires native shared GPU fences; delays the producer without gating a GPU queue"]
fn cpu_readback_waits_for_a_delayed_fence_producer() -> Result<()> {
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    let compute = crate::compute::Compute::for_device(&gpu.device)?;
    let ring = Ring::fenced(
        &gpu,
        compute,
        Layout {
            kind: Kind::Fence,
            slot_count: 2,
            width: 16,
            height: 16,
            format: ring::FORMAT_BGRA8,
            generation: 1,
        },
        true,
    )?;
    let mut driver = FakeDriver::open(&ring)?;
    let expected = pixels(16, 16, Pixel::Bgra8, 91);
    let source = GpuImage::upload(&driver.gpu, &expected)?;
    let header = ring.section.header();
    let released = driver.released();
    let claim = driver
        .picker
        .claim_fenced(&header, released)
        .context("a free producer slot")?;
    let index = claim.slot();
    let seq = driver.picker.next_seq();
    // Expose CPU publication before the GPU copy completes. Delaying submission itself
    // makes this deterministic without inserting an artificial wait on either GPU queue.
    header.write_slot(
        index,
        SlotMeta {
            seq: u64::from(seq),
            ..Default::default()
        },
    );
    anyhow::ensure!(driver.picker.publish_fenced(&header, index, seq));
    header.publish(ring::Token {
        generation: 1,
        seq,
        slot: index as u8,
    });
    let image = claim_image(&gpu, &ring, &mut Consumer::new(1, 2))?;
    let before = Instant::now();
    let delay = Duration::from_millis(50);
    let producer = std::thread::spawn(move || -> Result<()> {
        let _com = ComGuard::new()?;
        std::thread::sleep(delay);
        // SAFETY: this is the only producer write for this sequence; the host cannot read
        // until precisely this fence value. Source and target match on one device.
        unsafe {
            driver
                .gpu
                .context
                .CopyResource(&driver.textures[index], &*source.texture);
            driver
                .context
                .Signal(&driver.written, u64::from(seq))?;
            driver.gpu.context.Flush();
        }
        Ok(())
    });
    let readback = image.readback(&mut None);
    let elapsed = before.elapsed();
    producer.join().expect("delayed producer panicked")?;
    assert!(elapsed >= delay, "readback completed before its producer");
    assert_eq!(readback?.bytes, expected.bytes);
    assert!(!ring.fences.failed.load(Ordering::Acquire));
    Ok(())
}

#[test]
#[ignore = "requires a native GPU; bounds a CPU-only fence wait without queueing GPU waits"]
fn cpu_readback_rejects_missing_producer_completion_before_mapping() -> Result<()> {
    use windows::Win32::Graphics::Direct3D12::D3D12_FENCE_FLAG_NONE;

    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    let compute = crate::compute::Compute::for_device(&gpu.device)?;
    let expected = pixels(16, 16, Pixel::Bgra8, 73);
    let mut image = GpuImage::upload(&gpu, &expected)?;
    // An owned image isolates the readiness check: even if it regresses, no D3D11/12
    // queue can be left waiting on this deliberately unsignalled producer fence.
    // SAFETY: this private fence belongs to the live compute device and has no GPU users.
    let pending = unsafe { compute.device.CreateFence(0, D3D12_FENCE_FLAG_NONE)? };
    image.ready = Some(crate::compute::Ready::on(&compute, pending, 1));
    let mut staging = None;
    let before = Instant::now();
    let error = image
        .readback(&mut staging)
        .err()
        .context("an unfinished producer must prevent CPU readback")?;
    assert!(
        format!("{error:#}").contains("GPU work did not finish within two seconds"),
        "unexpected readback error: {error:#}"
    );
    assert!(before.elapsed() < Duration::from_secs(5));
    assert!(
        staging.is_none(),
        "failure must precede staging allocation, CopyResource and Map"
    );
    assert!(
        !compute.is_stuck(),
        "a CPU-only external fence wait must not retire a healthy compute queue"
    );
    // No GPU queue was blocked or poisoned by the timeout. A normal readback still works.
    image.ready = None;
    assert_eq!(image.readback(&mut staging)?.bytes, expected.bytes);
    Ok(())
}

#[test]
#[ignore = "requires native D3D12; validates cache retirement without a stalled GPU queue"]
fn a_retired_compute_queue_is_not_reused_for_the_same_capture_device() -> Result<()> {
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    let first = crate::compute::Compute::for_device(&gpu.device)?;
    let shared = crate::compute::Compute::for_device(&gpu.device)?;
    assert!(Arc::ptr_eq(&first, &shared));
    first.retire();
    assert!(first.is_stuck());
    let fresh = crate::compute::Compute::for_device(&gpu.device)?;
    assert!(!Arc::ptr_eq(&first, &fresh));
    assert!(!fresh.is_stuck());
    Ok(())
}
