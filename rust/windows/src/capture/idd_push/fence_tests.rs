//! Native driver-transport checks: a fake driver writes the ring on a D3D12 copy queue; no
//! driver installation.
use super::*;
use windows::Win32::Graphics::Direct3D12::{
    D3D12_COMMAND_LIST_TYPE_COPY, D3D12_COMMAND_QUEUE_DESC, D3D12_FENCE_FLAG_NONE,
    D3D12_RESOURCE_DIMENSION_TEXTURE2D, ID3D12CommandAllocator, ID3D12CommandList,
    ID3D12GraphicsCommandList, ID3D12Resource,
};

/// The driver's half of a fence ring, as WUDFHost has it: the ring's textures and fences
/// opened from their NT handles on a D3D12 device, written on a copy queue.
struct FakeDriver {
    queue: ID3D12CommandQueue,
    allocator: ID3D12CommandAllocator,
    list: ID3D12GraphicsCommandList,
    textures: Vec<ID3D12Resource>,
    written: ID3D12Fence,
    released: ID3D12Fence,
    /// The copy queue's own progress, so the allocator is reset only once it is idle.
    copied: ID3D12Fence,
    copies: u64,
    picker: ring::Picker,
}

impl FakeDriver {
    fn open(ring: &Ring, gpu: &Device) -> Result<Self> {
        let device = gpu.compute.device.clone();
        let fences = &ring.fences;
        let mut textures = Vec::new();
        for slot in &ring.slots {
            let mut texture: Option<ID3D12Resource> = None;
            // SAFETY: a live shared NT handle of a D3D12 texture; `texture` is a live local.
            unsafe { device.OpenSharedHandle(HANDLE(slot.shared.as_raw_handle()), &mut texture)? };
            let texture = texture.context("no texture")?;
            // The driver's checks before it adopts a ring (frame_ring.rs).
            // SAFETY: a live resource; GetDesc only reads it.
            let desc = unsafe { texture.GetDesc() };
            anyhow::ensure!(
                desc.Dimension == D3D12_RESOURCE_DIMENSION_TEXTURE2D
                    && (desc.Width, desc.Height)
                        == (u64::from(ring.layout.width), ring.layout.height)
                    && desc.Format.0 as u32 == ring.layout.format
                    && desc.DepthOrArraySize == 1
                    && desc.MipLevels == 1
                    && desc.SampleDesc.Count == 1,
                "a fence ring texture the driver refuses: {desc:?}"
            );
            textures.push(texture);
        }
        let open = |handle: &OwnedHandle| -> Result<ID3D12Fence> {
            let mut fence = None;
            // SAFETY: a live shared fence handle; `fence` is a live local.
            unsafe { device.OpenSharedHandle(HANDLE(handle.as_raw_handle()), &mut fence)? };
            fence.context("no fence")
        };
        // SAFETY: `device` is live; the list records into the allocator made with it and is
        // closed until the first copy.
        unsafe {
            let queue: ID3D12CommandQueue =
                device.CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC {
                    Type: D3D12_COMMAND_LIST_TYPE_COPY,
                    ..Default::default()
                })?;
            let allocator: ID3D12CommandAllocator =
                device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_COPY)?;
            let list: ID3D12GraphicsCommandList =
                device.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_COPY, &allocator, None)?;
            list.Close()?;
            Ok(Self {
                written: open(&fences.handles[0])?,
                released: open(&fences.handles[1])?,
                copied: device.CreateFence(0, D3D12_FENCE_FLAG_NONE)?,
                copies: 0,
                queue,
                allocator,
                list,
                textures,
                picker: ring::Picker::new(ring.slots.len()),
            })
        }
    }

    /// Copy `source` into `slot` on the copy queue and signal the written fence with `seq`.
    fn write(&mut self, slot: usize, seq: u32, source: &ID3D12Resource) -> Result<()> {
        until("the fake driver's last copy", || {
            // SAFETY: a plain read of a live fence.
            unsafe { self.copied.GetCompletedValue() >= self.copies }
        })?;
        self.copies += 1;
        // SAFETY: same device, size and format; the slot is ours in `Writing`, and the
        // allocator's last copy has finished (waited above).
        unsafe {
            self.allocator.Reset()?;
            self.list.Reset(&self.allocator, None)?;
            self.list.CopyResource(&self.textures[slot], source);
            self.list.Close()?;
            self.queue
                .ExecuteCommandLists(&[Some(self.list.cast::<ID3D12CommandList>()?)]);
            self.queue.Signal(&self.written, u64::from(seq))?;
            self.queue.Signal(&self.copied, self.copies)?;
        }
        Ok(())
    }

    /// Copy `source` into a free slot, signal the written fence and publish, as the
    /// driver's swap-chain worker does. `None` when it drops the frame.
    fn push(&mut self, header: &Header<'_>, source: &ID3D12Resource) -> Result<Option<u32>> {
        let released = self.released();
        let Some(claim) = self.picker.claim_fenced(header, released) else {
            return Ok(None);
        };
        let seq = self.picker.next_seq();
        self.write(claim.slot(), seq, source)?;
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

impl Drop for FakeDriver {
    fn drop(&mut self) {
        let _ = until("the fake driver's copies", || {
            // SAFETY: a plain read of a live fence.
            unsafe { self.copied.GetCompletedValue() >= self.copies }
        });
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

/// A ring of `slots` 16x16 textures read in place.
fn small_ring(gpu: &Device, slots: u32) -> Result<Ring> {
    Ring::fenced(
        gpu.compute.clone(),
        Layout {
            slot_count: slots,
            width: 16,
            height: 16,
            format: ring::FORMAT_BGRA8,
            generation: 1,
        },
        release_queue(gpu, None),
    )
}

#[test]
#[ignore = "requires native D3D12 sharing and compute conversion"]
fn a_fence_ring_carries_frames_both_ways() -> Result<()> {
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    for (in_place, pixel) in [
        (true, Pixel::Bgra8),
        (false, Pixel::Bgra8),
        (true, Pixel::RgbaF16),
        (false, Pixel::RgbaF16),
    ] {
        let layout = Layout {
            slot_count: 2,
            width: 16,
            height: 16,
            format: crate::compute::format(pixel).0 as u32,
            generation: 1,
        };
        let mut pool = (!in_place)
            .then(|| GpuPool::new(gpu.compute.clone()))
            .transpose()?;
        let ring = Ring::fenced(
            gpu.compute.clone(),
            layout,
            release_queue(&gpu, pool.as_ref()),
        )?;
        let fences = ring.fences.clone();
        let header = ring.section.header();
        let mut driver = FakeDriver::open(&ring, &gpu)?;
        let mut consumer = Consumer::new(1, 2);
        let ten_bit = pixel == Pixel::RgbaF16;
        let config = butterpollo_core::rtsp::Negotiated {
            width: 16,
            height: 16,
            codec: 1,
            sdr_10bit: ten_bit,
            ..Default::default()
        };
        let convert = |image: &GpuImage| -> Result<()> {
            let mut converter =
                crate::compute::Converter::new(gpu.compute.clone(), 16, 16, ten_bit)?;
            converter.values = crate::compute::constants(&config, (16, 16, pixel), [100., 1.]);
            converter.convert(image)?.wait()
        };
        for (frame, value) in [(1u32, 40u8), (2, 80), (3, 120)] {
            let expected = pixels(16, 16, pixel, value);
            let source = GpuImage::upload(&gpu, &expected)?;
            let seq = driver
                .push(&header, &source.texture)?
                .context("a free slot")?;
            assert_eq!(seq, frame);
            let (index, claimed) = consumer.claim(&header).context("the newest frame")?;
            assert_eq!(claimed, seq);
            consumer.consumed(u64::from(seq));
            let image = match &mut pool {
                None => {
                    let lent = Lent {
                        fences: fences.clone(),
                        slot: index,
                        seq,
                    };
                    let image =
                        lend_fenced(&gpu, &ring.slots[index].texture, lent, fences.written(seq))?;
                    // The compute conversion waits for the driver's copy on the GPU.
                    convert(&image)?;
                    anyhow::ensure!(
                        matches!(header.slot_state(index), ring::SlotState::Reading { .. }),
                        "lent until the image goes"
                    );
                    image
                }
                Some(pool) => {
                    let image = pool
                        .copy(&gpu, &ring.slots[index].texture, Some(&fences.written(seq)))?
                        .context("a free pool texture")?;
                    fences.free(index, seq)?;
                    image
                }
            };
            // A reader sees the driver's pixels, never an older frame.
            assert_eq!(image.readback(&mut None)?.bytes, expected.bytes);
            drop(image);
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
    };
    lend_fenced(gpu, &ring.slots[index].texture, lent, fences.written(seq))
}

#[test]
#[ignore = "requires native AMD D3D12 and the PyroWave runtime; does not install a driver"]
fn pyrowave_encoding_frees_fence_ring_slots() -> Result<()> {
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    let layout = Layout {
        slot_count: 4,
        width: 64,
        height: 64,
        format: crate::compute::format(Pixel::Bgra8).0 as u32,
        generation: 1,
    };
    let ring = Ring::fenced(gpu.compute.clone(), layout, release_queue(&gpu, None))?;
    let mut driver = FakeDriver::open(&ring, &gpu)?;
    let source = GpuImage::upload(&gpu, &pixels(layout.width, layout.height, Pixel::Bgra8, 80))?;
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
#[ignore = "requires native D3D12 shared textures; does not install a driver"]
fn a_fence_slot_waits_for_all_readers_and_survives_ring_replacement() -> Result<()> {
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    let ring = small_ring(&gpu, 2)?;
    let owner = Arc::downgrade(&ring.fences);
    let mut driver = FakeDriver::open(&ring, &gpu)?;
    let source = solid(&gpu, 40)?;
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
    // still holds its last image. That image must still convert successfully.
    drop(ring);
    assert!(owner.upgrade().is_some());
    let mut converter = crate::compute::Converter::new(gpu.compute.clone(), 16, 16, false)?;
    converter.convert(&second_reader)?.wait()?;
    drop(second_reader);
    drop(converter);
    assert!(
        owner.upgrade().is_none(),
        "last reader releases the old ring's fences"
    );
    Ok(())
}

#[test]
#[ignore = "requires native shared GPU fences; verifies bounded delayed completion without a driver"]
fn a_returned_slot_waits_for_gpu_completion_before_the_driver_reuses_it() -> Result<()> {
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
    let compute = gpu.compute.clone();
    let ring = small_ring(&gpu, 2)?;
    let mut driver = FakeDriver::open(&ring, &gpu)?;
    let source = solid(&gpu, 40)?;
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
    let ring = small_ring(&gpu, 2)?;
    let mut driver = FakeDriver::open(&ring, &gpu)?;
    let expected = pixels(16, 16, Pixel::Bgra8, 91);
    let source = GpuImage::upload(&gpu, &expected)?;
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
        std::thread::sleep(delay);
        // This is the only producer write for this sequence; the host cannot read until
        // precisely this fence value.
        driver.write(index, seq, &source.texture)
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
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    let compute = gpu.compute.clone();
    let expected = pixels(16, 16, Pixel::Bgra8, 73);
    let mut image = GpuImage::upload(&gpu, &expected)?;
    // An owned image isolates the readiness check: even if it regresses, no queue can be
    // left waiting on this deliberately unsignalled producer fence.
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
        "failure must precede staging allocation, the copy and the map"
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
    let luid = gpu.compute.luid();
    let first = crate::compute::Compute::for_adapter(luid)?;
    let shared = crate::compute::Compute::for_adapter(luid)?;
    assert!(Arc::ptr_eq(&first, &shared));
    first.retire();
    assert!(first.is_stuck());
    let fresh = crate::compute::Compute::for_adapter(luid)?;
    assert!(!Arc::ptr_eq(&first, &fresh));
    assert!(!fresh.is_stuck());
    Ok(())
}

/// The display driver of 2.6 is D3D11: it opens each ring texture with `OpenSharedResource1`,
/// which fails with E_INVALIDARG unless the D3D12 texture allows render targets and
/// simultaneous access (RX 7900 XT, October 16). The host keeps those flags so that a driver
/// already installed keeps working; the D3D12 driver opens the same textures (next test).
#[test]
#[ignore = "requires native D3D12 sharing and a D3D11 device on the same GPU; no driver is installed"]
fn fence_ring_textures_open_on_a_d3d11_device_like_the_driver_opens_them() -> Result<()> {
    use windows::Win32::{
        Foundation::HMODULE,
        Graphics::{
            Direct3D::D3D_DRIVER_TYPE_UNKNOWN,
            Direct3D11::{
                D3D11_CREATE_DEVICE_FLAG, D3D11_SDK_VERSION, D3D11CreateDevice, ID3D11Device,
                ID3D11Device1, ID3D11Texture2D,
            },
            Dxgi::{CreateDXGIFactory1, IDXGIAdapter, IDXGIAdapter1, IDXGIFactory4},
        },
    };
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    // SAFETY: the factory and adapter are live, and the device out-slot is a live local.
    let driver: ID3D11Device1 = unsafe {
        let factory: IDXGIFactory4 = CreateDXGIFactory1()?;
        let adapter: IDXGIAdapter = factory
            .EnumAdapterByLuid::<IDXGIAdapter1>(gpu.compute.luid())?
            .cast()?;
        let mut device: Option<ID3D11Device> = None;
        D3D11CreateDevice(
            &adapter,
            D3D_DRIVER_TYPE_UNKNOWN,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_FLAG(0),
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            None,
        )?;
        device.context("no D3D11 device")?.cast()?
    };
    for pixel in [Pixel::Bgra8, Pixel::RgbaF16] {
        let ring = Ring::fenced(
            gpu.compute.clone(),
            Layout {
                slot_count: 2,
                width: 64,
                height: 32,
                format: crate::compute::format(pixel).0 as u32,
                generation: 1,
            },
            release_queue(&gpu, None),
        )?;
        for slot in &ring.slots {
            // SAFETY: a live shared NT handle of a D3D12 texture on the same adapter.
            let texture: ID3D11Texture2D =
                unsafe { driver.OpenSharedResource1(HANDLE(slot.shared.as_raw_handle())) }
                    .with_context(|| {
                        format!("the driver's D3D11 open of a {pixel:?} ring texture")
                    })?;
            let mut desc = Default::default();
            // SAFETY: a live texture; GetDesc only writes the live local.
            unsafe { texture.GetDesc(&mut desc) };
            assert_eq!((desc.Width, desc.Height), (64, 32));
        }
    }
    Ok(())
}

/// The D3D12 display driver opens each ring texture and fence with `OpenSharedHandle` on its
/// own device and copies into the texture on a copy queue, then signals the written fence. The
/// flags the D3D11 driver needs must not get in its way.
#[test]
#[ignore = "requires native D3D12 sharing and a second D3D12 device on the same GPU; no driver is installed"]
fn fence_ring_textures_open_on_a_second_d3d12_device_like_the_d3d12_driver_opens_them() -> Result<()>
{
    use windows::Win32::Graphics::{
        Direct3D::D3D_FEATURE_LEVEL_11_0,
        Direct3D12::{
            D3D12_HEAP_FLAG_NONE, D3D12_HEAP_PROPERTIES, D3D12_HEAP_TYPE_DEFAULT,
            D3D12_RESOURCE_FLAG_NONE, D3D12_RESOURCE_STATE_COMMON, D3D12_TEXTURE_LAYOUT_UNKNOWN,
            D3D12CreateDevice, ID3D12Device,
        },
        Dxgi::{
            Common::{DXGI_FORMAT, DXGI_SAMPLE_DESC},
            CreateDXGIFactory1, IDXGIAdapter1, IDXGIFactory4,
        },
    };
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    // SAFETY: the factory and adapter are live, and the device out-slot is a live local.
    let driver: ID3D12Device = unsafe {
        let factory: IDXGIFactory4 = CreateDXGIFactory1()?;
        let adapter: IDXGIAdapter1 = factory.EnumAdapterByLuid(gpu.compute.luid())?;
        let mut device: Option<ID3D12Device> = None;
        D3D12CreateDevice(&adapter, D3D_FEATURE_LEVEL_11_0, &mut device)?;
        device.context("no D3D12 device")?
    };
    anyhow::ensure!(
        driver.as_raw() != gpu.compute.device.as_raw(),
        "the driver's device must be its own"
    );
    for pixel in [Pixel::Bgra8, Pixel::RgbaF16] {
        let format = crate::compute::format(pixel);
        let ring = Ring::fenced(
            gpu.compute.clone(),
            Layout {
                slot_count: 2,
                width: 64,
                height: 32,
                format: format.0 as u32,
                generation: 1,
            },
            release_queue(&gpu, None),
        )?;
        let open_fence = |handle: &OwnedHandle| -> Result<ID3D12Fence> {
            let mut fence = None;
            // SAFETY: a live shared fence handle; `fence` is a live local.
            unsafe { driver.OpenSharedHandle(HANDLE(handle.as_raw_handle()), &mut fence)? };
            fence.context("no fence")
        };
        let written = open_fence(&ring.fences.handles[0])?;
        open_fence(&ring.fences.handles[1])?;
        // SAFETY: `driver` is live; the queue, allocator and list are made with it, and the
        // source is a plain texture of the ring's size and format.
        unsafe {
            let queue: ID3D12CommandQueue =
                driver.CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC {
                    Type: D3D12_COMMAND_LIST_TYPE_COPY,
                    ..Default::default()
                })?;
            let allocator: ID3D12CommandAllocator =
                driver.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_COPY)?;
            let list: ID3D12GraphicsCommandList =
                driver.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_COPY, &allocator, None)?;
            let mut source: Option<ID3D12Resource> = None;
            driver.CreateCommittedResource(
                &D3D12_HEAP_PROPERTIES {
                    Type: D3D12_HEAP_TYPE_DEFAULT,
                    ..Default::default()
                },
                D3D12_HEAP_FLAG_NONE,
                &windows::Win32::Graphics::Direct3D12::D3D12_RESOURCE_DESC {
                    Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
                    Width: 64,
                    Height: 32,
                    DepthOrArraySize: 1,
                    MipLevels: 1,
                    Format: DXGI_FORMAT(format.0),
                    SampleDesc: DXGI_SAMPLE_DESC {
                        Count: 1,
                        Quality: 0,
                    },
                    Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
                    Flags: D3D12_RESOURCE_FLAG_NONE,
                    Alignment: 0,
                },
                D3D12_RESOURCE_STATE_COMMON,
                None,
                &mut source,
            )?;
            let source = source.context("no source texture")?;
            for (seq, slot) in (1u64..).zip(&ring.slots) {
                let mut texture: Option<ID3D12Resource> = None;
                driver.OpenSharedHandle(HANDLE(slot.shared.as_raw_handle()), &mut texture)?;
                let texture = texture.context("no texture")?;
                let desc = texture.GetDesc();
                assert_eq!(
                    (desc.Width, desc.Height, desc.Format),
                    (64, 32, DXGI_FORMAT(format.0)),
                    "the driver's D3D12 open of a {pixel:?} ring texture"
                );
                allocator.Reset()?;
                list.Reset(&allocator, None)?;
                list.CopyResource(&texture, &source);
                list.Close()?;
                queue.ExecuteCommandLists(&[Some(list.cast::<ID3D12CommandList>()?)]);
                queue.Signal(&written, seq)?;
                // The host's own fence object sees the driver's signal.
                until("the written fence", || {
                    ring.fences.written.GetCompletedValue() >= seq
                })?;
            }
        }
    }
    Ok(())
}

/// `compute_queue_realtime` is read once, when the shared compute queue is made, so the stream's
/// priority must be configured before the capture device exists. Hardware cannot show this
/// without a display driver, so the order in the source is pinned.
#[test]
fn a_stream_configures_gpu_priority_before_it_creates_the_compute_queue() {
    let source = include_str!("../idd_push.rs");
    let configure = source
        .find("crate::gpu_priority::configure(config)")
        .expect("the stream configures GPU priority");
    let device = source
        .find("Device::for_display(")
        .expect("the stream opens the capture device");
    assert!(
        configure < device,
        "configure GPU priority before Device::for_display"
    );
}
