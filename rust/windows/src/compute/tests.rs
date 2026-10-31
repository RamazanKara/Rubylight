use super::*;

#[test]
fn pq_input_is_rejected_only_for_sdr_output() {
    for pixel in 0..=2 {
        for hdr in 0..=1 {
            let mut values = [0; 20];
            values[4] = pixel;
            values[5] = hdr;
            assert_eq!(
                validate_colour_input(&values).is_ok(),
                pixel != 2 || hdr != 0
            );
        }
    }
}

/// Always release the task-owned queue gate, including on assertion failure.
struct Gate(ID3D12Fence);
impl Drop for Gate {
    fn drop(&mut self) {
        // SAFETY: CPU-signalling this test-owned fence releases only this
        // test's queue. No desktop/capture production queue uses it.
        let _ = unsafe { self.0.Signal(1) };
    }
}

#[test]
#[ignore = "requires native D3D11/D3D12; gates only a task-owned compute queue for two seconds"]
fn timed_out_converter_retains_capture_until_its_queue_really_finishes() -> Result<()> {
    use crate::capture::{Device, GpuImage, Image, Pixel};
    let _com = crate::capture::ComGuard::new()?;
    let gpu = Device::new("")?;
    let shared = Compute::for_device(&gpu.device)?;
    // SAFETY: This is a read-only query on this test's live device.
    let luid = unsafe { shared.device.GetAdapterLuid() };
    let compute = Compute::new(luid)?;
    let mut converter = Converter::new(compute.clone(), 4, 4, false)?;
    converter.prepare_yuv()?;
    let image = GpuImage::upload(
        &gpu,
        &Image {
            width: 4,
            height: 4,
            stride: 16,
            bytes: vec![128; 64],
            captured: std::time::Instant::now(),
            pixel: Pixel::Bgra8,
        },
    )?;
    let capture = Arc::downgrade(&image.texture);
    converter.values = crate::gpu_color::constants(
        &butterpollo_core::rtsp::Negotiated {
            width: 4,
            height: 4,
            ..Default::default()
        },
        (4, 4, Pixel::Bgra8),
        [100., 1.],
        None,
    );
    // SAFETY: This gate and queue belong exclusively to this test. Gate's
    // destructor always signals it, even if any later assertion panics.
    let gate = Gate(unsafe { compute.device.CreateFence(0, D3D12_FENCE_FLAG_NONE)? });
    // SAFETY: This fence was created on this queue's device and stays owned.
    unsafe { compute.queue.Wait(&gate.0, 1)? };
    let converted = converter.convert(
        &compute.open(&image.texture)?,
        format(Pixel::Bgra8),
        None,
        &image,
    )?;
    drop(image);
    drop(converter);
    assert!(compute.is_stuck());
    assert!(
        capture.upgrade().is_some(),
        "timed-out GPU read lost its capture lease"
    );
    assert!(
        Compute::new(luid).is_err(),
        "retries must not grow retained GPU memory"
    );
    let tail = RETAINED
        .lock()
        .unwrap()
        .iter()
        .find(|work| !work.captures.is_empty())
        .and_then(|work| work.tail.clone())
        .context("retained converter tail missing")?;
    drop(gate);
    converted.wait()?;
    wait_fence(&tail, 1)?;
    let _replacement = Compute::new(luid)?;
    assert!(
        capture.upgrade().is_none(),
        "completed capture was never released"
    );
    Ok(())
}

#[test]
#[ignore = "requires native D3D11/D3D12; gates only a task-owned copy queue for two seconds"]
fn timed_out_handoff_keeps_its_copy_executable_after_teardown() -> Result<()> {
    use crate::capture::{Device, GpuImage, Image, Pixel};
    let _com = crate::capture::ComGuard::new()?;
    let gpu = Device::new("")?;
    let compute = Compute::for_device(&gpu.device)?;
    let mut handoff = Handoff::new(compute.clone(), &gpu)?;
    let pixels = Image {
        width: 4,
        height: 4,
        stride: 16,
        bytes: vec![85; 64],
        captured: std::time::Instant::now(),
        pixel: Pixel::Bgra8,
    };
    let source = GpuImage::upload(&gpu, &pixels)?;
    let mut destination = GpuImage::upload(
        &gpu,
        &Image {
            bytes: vec![0; 64],
            ..pixels
        },
    )?;
    // SAFETY: Gate always signals on drop; only this test's copy queue
    // waits for it, and every submitted texture belongs to this test.
    let gate = Gate(unsafe { compute.device.CreateFence(0, D3D12_FENCE_FLAG_NONE)? });
    // SAFETY: The gate is on the same live device as the copy queue.
    unsafe { handoff.queue.Wait(&gate.0, 1)? };
    destination.ready = Some(handoff.copy(&destination.texture, &source.texture)?);
    drop(source);
    drop(handoff);
    assert!(compute.is_stuck());
    // SAFETY: This read-only query is on the live test device.
    let luid = unsafe { compute.device.GetAdapterLuid() };
    assert!(Compute::new(luid).is_err());
    let tail = RETAINED
        .lock()
        .unwrap()
        .iter()
        .find(|work| work.adapter == (luid.LowPart, luid.HighPart))
        .and_then(|work| work.tail.clone())
        .context("retained copy tail missing")?;
    drop(gate);
    destination.ready.as_ref().unwrap().wait()?;
    wait_fence(&tail, 1)?;
    let actual = destination.readback(&mut None)?;
    for row in actual.bytes.chunks_exact(actual.stride).take(4) {
        assert_eq!(&row[..16], &[85; 16]);
    }
    let _replacement = Compute::new(luid)?;
    Ok(())
}

#[test]
#[ignore = "requires native D3D12; validates distinct task-owned queues on a shared device"]
fn reader_values_belong_to_a_fence_not_the_shared_device() -> Result<()> {
    let _com = crate::capture::ComGuard::new()?;
    let gpu = crate::capture::Device::new("")?;
    let old = Compute::for_device(&gpu.device)?;
    // SAFETY: Querying a live device does not change its state.
    let new = Compute::new(unsafe { old.device.GetAdapterLuid() })?;
    assert_eq!(
        old.device, new.device,
        "D3D12 should reuse the adapter device"
    );
    assert_ne!(old.fence, new.fence);
    let readers = Arc::new(AtomicU64::new(0));
    let ready = Ready {
        fence: old.fence.clone(),
        value: 0,
        device: old.device.as_raw() as usize,
        readers: Some(readers.clone()),
        reader_fence: Some(old.fence.clone()),
    };
    let old_value = old.signal()?;
    ready.read_by(&old, old_value)?;
    assert_eq!(readers.load(Ordering::Acquire), old_value);
    let _ = new.signal()?;
    let new_value = new.signal()?;
    ready.read_by(&new, new_value)?;
    assert!(
        new.completed(new_value),
        "cross-generation read returned before finishing"
    );
    assert_eq!(
        readers.load(Ordering::Acquire),
        old_value,
        "new fence value entered the old timeline"
    );
    Ok(())
}

#[test]
#[ignore = "requires native D3D12; validates bounded reuse after a delayed target fence"]
fn released_encoder_target_waits_for_its_gpu_fence() -> Result<()> {
    let _com = crate::capture::ComGuard::new()?;
    let gpu = crate::capture::Device::new("")?;
    let compute = Compute::for_device(&gpu.device)?;
    let mut converter = Converter::new(compute, 4, 4, false)?;
    converter.prepare_yuv()?;
    let mut busy = Vec::new();
    for _ in 0..8 {
        let index = converter.target()?;
        busy.push(converter.targets[index].texture.clone());
    }
    converter.targets[0].value = 1;
    drop(busy.remove(0));
    let fence = converter.targets[0].fence.clone();
    std::thread::scope(|scope| -> Result<()> {
        scope.spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(25));
            // SAFETY: This test-owned signal models the encoder finishing
            // after it has already released its surface reference.
            unsafe { fence.Signal(1).unwrap() };
        });
        assert_eq!(converter.target()?, 0);
        Ok(())
    })?;
    Ok(())
}

#[test]
fn compute_texture_states_accept_only_supported_reads_or_one_write() {
    for state in [
        D3D12_RESOURCE_STATE_COMMON,
        D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE,
        D3D12_RESOURCE_STATE_COPY_SOURCE,
        D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE | D3D12_RESOURCE_STATE_COPY_SOURCE,
        D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
        D3D12_RESOURCE_STATE_COPY_DEST,
    ] {
        assert!(ensure_compute_texture_state(state).is_ok(), "{state:?}");
    }
    for state in [
        D3D12_RESOURCE_STATE_VIDEO_ENCODE_READ,
        D3D12_RESOURCE_STATE_VIDEO_ENCODE_WRITE,
        D3D12_RESOURCE_STATE_VIDEO_DECODE_READ,
        D3D12_RESOURCE_STATE_VIDEO_DECODE_WRITE,
        D3D12_RESOURCE_STATE_VIDEO_PROCESS_READ,
        D3D12_RESOURCE_STATE_VIDEO_PROCESS_WRITE,
        D3D12_RESOURCE_STATE_RENDER_TARGET,
        D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE,
        D3D12_RESOURCE_STATE_DEPTH_READ,
        D3D12_RESOURCE_STATE_DEPTH_WRITE,
        D3D12_RESOURCE_STATE_GENERIC_READ,
        D3D12_RESOURCE_STATE_COPY_DEST | D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
        D3D12_RESOURCE_STATE_COPY_SOURCE | D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
        D3D12_RESOURCE_STATE_COPY_DEST | D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE,
        D3D12_RESOURCE_STATES(i32::MIN),
    ] {
        assert!(ensure_compute_texture_state(state).is_err(), "{state:?}");
    }
}

#[test]
#[ignore = "requires native D3D11/D3D12; changes only task-owned AMF metadata"]
fn unsupported_amf_state_is_rejected_before_recording_conversion() -> Result<()> {
    use crate::capture::{Device, GpuImage, Image, Pixel};

    let _com = crate::capture::ComGuard::new()?;
    let gpu = Device::new("")?;
    let compute = Compute::for_device(&gpu.device)?;
    let image = GpuImage::upload(
        &gpu,
        &Image {
            width: 4,
            height: 4,
            stride: 16,
            bytes: vec![128; 64],
            captured: std::time::Instant::now(),
            pixel: Pixel::Bgra8,
        },
    )?;
    let source = compute.open(&image.texture)?;
    let mut converter = Converter::new(compute.clone(), 4, 4, false)?;
    converter.values = crate::gpu_color::constants(
        &butterpollo_core::rtsp::Negotiated {
            width: 4,
            height: 4,
            ..Default::default()
        },
        (4, 4, Pixel::Bgra8),
        [100., 1.],
        None,
    );
    let index = converter.target()?;
    let submitted = *compute.next.lock().unwrap();
    let slot = converter.slot;
    for state in [
        D3D12_RESOURCE_STATE_VIDEO_ENCODE_READ,
        D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE,
        D3D12_RESOURCE_STATE_COPY_DEST | D3D12_RESOURCE_STATE_COPY_SOURCE,
    ] {
        // This changes only AMF's private-data record. The texture remains
        // COMMON; rejection must happen before recording any GPU commands.
        crate::amf::gpu::set_resource_state(&converter.targets[index].texture, state)?;
        let error = converter
            .convert(&source, format(Pixel::Bgra8), None, &image)
            .err()
            .expect("unsupported AMF state accepted");
        assert!(format!("{error:#}").contains("cannot transition on the compute queue"));
        assert_eq!(*compute.next.lock().unwrap(), submitted);
        assert_eq!(converter.slot, slot);
        assert_eq!(converter.targets.len(), 1);
        assert_eq!(converter.targets[index].value, 0);
        assert!(converter.slots.iter().all(|slot| slot.capture.is_none()));
        assert_eq!(
            crate::amf::gpu::resource_state(&converter.targets[index].texture)?,
            state
        );
    }
    crate::amf::gpu::set_resource_state(
        &converter.targets[index].texture,
        D3D12_RESOURCE_STATE_COMMON,
    )?;
    converter
        .convert(&source, format(Pixel::Bgra8), None, &image)?
        .wait()?;
    converter.release_completed();
    Ok(())
}

#[test]
fn failed_shared_texture_ordering_never_releases_or_reuses_the_slot() {
    use std::{cell::RefCell, sync::atomic::AtomicBool};
    for failure in [None, Some("signal"), Some("wait"), Some("release")] {
        let retired = AtomicBool::new(false);
        let calls = RefCell::new(Vec::new());
        let returned = return_after_reads(
            &retired,
            || {
                for stage in ["signal", "wait"] {
                    calls.borrow_mut().push(stage);
                    anyhow::ensure!(failure != Some(stage), "injected {stage} failure");
                }
                Ok(())
            },
            || {
                calls.borrow_mut().push("release");
                anyhow::ensure!(failure != Some("release"), "injected release failure");
                Ok(())
            },
        );
        assert_eq!(returned.is_err(), failure.is_some());
        assert_eq!(retired.load(Ordering::Acquire), failure.is_some());
        assert_eq!(
            *calls.borrow(),
            match failure {
                Some("signal") => vec!["signal"],
                Some("wait") => vec!["signal", "wait"],
                _ => vec!["signal", "wait", "release"],
            }
        );
        if failure.is_some() {
            assert!(
                return_after_reads(
                    &retired,
                    || panic!("retired queue must reopen before ordering more work"),
                    || panic!("retired queue must not release another texture"),
                )
                .is_err()
            );
        }
    }
}

#[test]
#[ignore = "requires native D3D11/D3D12; retires only a completed task-owned queue"]
fn retired_queue_rejects_conversion_upload_and_capture_before_mutating_resources() -> Result<()> {
    use crate::capture::{Device, GpuImage, Image, Pixel};
    fn rejected<T>(result: Result<T>) {
        let error = result.err().expect("retired queue accepted new work");
        assert!(format!("{error:#}").contains("compute queue is retired"));
    }

    let _com = crate::capture::ComGuard::new()?;
    let gpu = Device::new("")?;
    let compute = Compute::for_device(&gpu.device)?;
    let pixels = Image {
        width: 4,
        height: 4,
        stride: 16,
        bytes: vec![128; 64],
        captured: std::time::Instant::now(),
        pixel: Pixel::Bgra8,
    };
    let image = GpuImage::upload(&gpu, &pixels)?;
    let destination = GpuImage::upload(&gpu, &pixels)?;
    let source = compute.open(&image.texture)?;
    let mut handoff = Handoff::new(compute.clone(), &gpu)?;
    let mut returns = Returns::new(compute.clone(), &gpu)?;
    let mut converter = Converter::new(compute.clone(), 4, 4, false)?;
    converter.values = crate::gpu_color::constants(
        &butterpollo_core::rtsp::Negotiated {
            width: 4,
            height: 4,
            ..Default::default()
        },
        (4, 4, Pixel::Bgra8),
        [100., 1.],
        None,
    );
    let cursor = crate::cursor::Cursor::new(
        &gpu,
        &DXGI_OUTDUPL_POINTER_SHAPE_INFO {
            Type: DXGI_OUTDUPL_POINTER_SHAPE_TYPE_COLOR.0 as u32,
            Width: 4,
            Height: 4,
            Pitch: 16,
            ..Default::default()
        },
        &[64; 64],
    )?;
    let pointer = converter.pointer(&cursor)?;
    converter
        .convert(&source, format(Pixel::Bgra8), Some(&pointer), &image)?
        .wait()?;
    converter.release_completed();

    let plane = || -> Result<ID3D12Resource> {
        let mut resource = None;
        // SAFETY: The description is a valid small UAV texture on this test's
        // device. Even a missing retirement guard must receive valid planes.
        unsafe {
            compute.device.CreateCommittedResource(
                &D3D12_HEAP_PROPERTIES {
                    Type: D3D12_HEAP_TYPE_DEFAULT,
                    ..Default::default()
                },
                D3D12_HEAP_FLAG_NONE,
                &D3D12_RESOURCE_DESC {
                    Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
                    Width: 4,
                    Height: 4,
                    DepthOrArraySize: 1,
                    MipLevels: 1,
                    Format: DXGI_FORMAT_R8_UNORM,
                    SampleDesc: DXGI_SAMPLE_DESC {
                        Count: 1,
                        Quality: 0,
                    },
                    Flags: D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
                    ..Default::default()
                },
                D3D12_RESOURCE_STATE_COMMON,
                None,
                &mut resource,
            )?;
        }
        resource.context("no test plane")
    };
    let planes = Planes {
        planes: [plane()?, plane()?, plane()?],
        format: DXGI_FORMAT_R8_UNORM,
        // SAFETY: The test owns this unused fence on its compute device.
        fence: unsafe { compute.device.CreateFence(0, D3D12_FENCE_FLAG_NONE)? },
        after: 0,
        done: 1,
    };
    let submitted = *compute.next.lock().unwrap();
    let slot = converter.slot;
    let targets = converter.targets.len();
    compute.retire();
    // Old fences remain usable for cleanup, but their completion cannot
    // allow new work on the quarantined queue.
    compute.wait(submitted)?;
    assert!(compute.completed(submitted));
    rejected(converter.pointer(&cursor));
    rejected(converter.convert(&source, format(Pixel::Bgra8), None, &image));
    rejected(converter.convert_planes(&source, format(Pixel::Bgra8), None, None, &planes, None));
    rejected(compute.execute(&converter.slots[slot].list));
    rejected(compute.open(&image.texture));
    rejected(handoff.copy(&destination.texture, &image.texture));
    rejected(returns.acquired());
    rejected(Converter::new(compute.clone(), 4, 4, false));
    rejected(Handoff::new(compute.clone(), &gpu));
    rejected(Returns::new(compute.clone(), &gpu));
    assert_eq!(*compute.next.lock().unwrap(), submitted);
    assert_eq!(converter.slot, slot);
    assert_eq!(converter.targets.len(), targets);
    assert!(converter.planar.is_none());
    assert!(converter.uploads.is_empty());
    assert!(handoff.readers.is_empty());
    assert_eq!(returns.value, 0);
    let fresh = Compute::for_device(&gpu.device)?;
    assert!(!Arc::ptr_eq(&compute, &fresh));
    assert!(!fresh.is_stuck());
    Ok(())
}

#[test]
#[ignore = "requires an idle D3D12 GPU, debug layer and task-owned report path; run alone"]
fn native_cursor_upload_is_shader_readable() -> Result<()> {
    let report = std::path::PathBuf::from(
        std::env::var_os("BUTTERPOLLO_TEST_CURSOR_STATE_REPORT")
            .context("set BUTTERPOLLO_TEST_CURSOR_STATE_REPORT")?,
    );
    anyhow::ensure!(report.is_absolute(), "report path must be absolute");
    let mut debug: Option<ID3D12Debug> = None;
    // SAFETY: This opt-in test runs alone and enables validation before creating
    // its first D3D12 device. Validation is local to this test process.
    unsafe {
        D3D12GetDebugInterface(&mut debug)?;
        debug
            .context("D3D12 debug layer unavailable")?
            .EnableDebugLayer();
    }
    let _com = crate::capture::ComGuard::new()?;
    eprintln!("cursor validation: creating capture device");
    let gpu = crate::capture::Device::new("")?;
    eprintln!("cursor validation: creating compute device");
    let compute = Compute::for_device(&gpu.device)?;
    let messages: ID3D12InfoQueue = compute.device.cast()?;
    // SAFETY: This only disables debugger breaks in this isolated test; the
    // validation result below still fails the test on a wrong resource state.
    unsafe {
        messages.SetBreakOnSeverity(D3D12_MESSAGE_SEVERITY_CORRUPTION, false)?;
        messages.SetBreakOnSeverity(D3D12_MESSAGE_SEVERITY_ERROR, false)?;
    }
    let validation: ID3D12DebugCommandQueue = compute.queue.cast()?;
    eprintln!("cursor validation: creating converter");
    let mut converter = Converter::new(compute.clone(), 4, 4, false)?;
    let shape = DXGI_OUTDUPL_POINTER_SHAPE_INFO {
        Type: DXGI_OUTDUPL_POINTER_SHAPE_TYPE_COLOR.0 as u32,
        Width: 9,
        Height: 5,
        Pitch: 9 * 4,
        ..Default::default()
    };
    let mut checks = Vec::new();
    for colour in [64u8, 192] {
        let cursor = crate::cursor::Cursor::new(&gpu, &shape, &[colour; 9 * 5 * 4])?;
        eprintln!("cursor validation: uploading {colour}");
        let texture = converter.pointer(&cursor)?;
        let upload = converter
            .uploads
            .last()
            .context("cursor upload was not queued")?
            .fence;
        compute.wait(upload)?;
        let cached = converter.pointer(&cursor)?;
        anyhow::ensure!(texture.as_raw() == cached.as_raw(), "cursor cache missed");
        eprintln!("cursor validation: checking {colour}");
        // SAFETY: Both interfaces belong to the same live device. This asks the
        // independent Direct3D validation layer whether a completed upload can be
        // sampled by the compute shader, without submitting an invalid GPU read.
        let readable = unsafe {
            validation.AssertResourceState(
                &texture,
                0,
                D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE.0 as u32,
            )
        }
        .as_bool();
        // The queue's validation operation can itself reference the resource.
        // Complete it before the next shape can release this cached texture.
        compute.wait(compute.signal()?)?;
        checks.push(serde_json::json!({"colour":colour,"shader_readable":readable}));
        std::fs::write(&report, serde_json::to_vec_pretty(&checks)?)?;
    }
    std::fs::write(&report, serde_json::to_vec_pretty(&checks)?)?;
    anyhow::ensure!(
        checks.iter().all(|check| check["shader_readable"] == true),
        "cursor upload is not shader-readable; see {}",
        report.display()
    );
    Ok(())
}

#[test]
#[ignore = "requires native D3D11/D3D12 shared capture textures"]
fn capture_ownership_ends_at_conversion_and_handoff_forgets_its_textures() -> Result<()> {
    use crate::capture::{Device, GpuImage, Image, Pixel};
    let gpu = Device::new("")?;
    let compute = Compute::for_device(&gpu.device)?;
    let pixels = Image {
        width: 4,
        height: 4,
        stride: 16,
        bytes: vec![128; 64],
        captured: std::time::Instant::now(),
        pixel: Pixel::Bgra8,
    };
    let source = GpuImage::upload(&gpu, &pixels)?;
    let mut image = GpuImage::upload(&gpu, &pixels)?;
    let mut handoff = Handoff::new(compute.clone(), &gpu)?;
    image.ready = Some(handoff.copy(&image.texture, &source.texture)?);
    let mut converter = Converter::new(compute.clone(), 4, 4, false)?;
    converter.values = crate::gpu_color::constants(
        &butterpollo_core::rtsp::Negotiated {
            width: 4,
            height: 4,
            ..Default::default()
        },
        (4, 4, Pixel::Bgra8),
        [100., 1.],
        None,
    );
    let input = compute.open(&image.texture)?;
    let before = Arc::strong_count(&image.texture);
    let converted = converter.convert(&input, format(Pixel::Bgra8), None, &image)?;
    assert_eq!(Arc::strong_count(&image.texture), before + 1);
    assert_eq!(
        image
            .ready
            .as_ref()
            .unwrap()
            .readers
            .as_ref()
            .unwrap()
            .load(Ordering::Acquire),
        converter.slots[0].fence
    );
    converted.wait()?;
    converter.release_completed();
    assert_eq!(Arc::strong_count(&image.texture), before);
    drop(handoff);
    let opened = compute.opened.lock().unwrap();
    assert!(!opened.contains_key(&(image.texture.as_raw() as usize)));
    assert!(!opened.contains_key(&(source.texture.as_raw() as usize)));
    Ok(())
}

#[test]
fn removed_device_fence_never_counts_as_completed_work() {
    for value in [0, 1, 100, u64::MAX - 1] {
        assert_eq!(completed_value(value).unwrap(), value);
    }
    assert_eq!(
        crate::device_loss::DeviceLost::from_error(&completed_value(u64::MAX).unwrap_err()),
        Some(crate::device_loss::DeviceLost(DXGI_ERROR_DEVICE_REMOVED.0))
    );
}
