use super::*;

#[test]
fn snapshot_capacity_can_reclaim_the_desktop_cache_without_overwriting_consumers() {
    use std::sync::Arc;
    let textures: Vec<_> = (0..GPU_SNAPSHOT_CAPACITY).map(Arc::new).collect();
    let consumers = textures[..GPU_SNAPSHOT_CAPACITY - 1].to_vec();
    let mut cached = Some(textures.last().unwrap().clone());
    assert!(!snapshot_capacity(&textures));
    // The pool may be the only remaining holder of a texture.
    cached.take();
    assert!(snapshot_capacity(&textures));
    assert!(consumers.iter().all(|image| Arc::strong_count(image) == 2));
    assert!(snapshot_capacity(&textures[..GPU_SNAPSHOT_CAPACITY - 1]));
}

#[test]
fn basic_render_driver_is_not_hardware_even_without_the_software_flag() {
    for flags in [0, DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32] {
        let basic = DXGI_ADAPTER_DESC1 {
            VendorId: 0x1414,
            DeviceId: 0x8c,
            Flags: flags,
            ..Default::default()
        };
        assert!(!hardware_adapter(&basic));
        let radeon = DXGI_ADAPTER_DESC1 {
            VendorId: 0x1002,
            ..Default::default()
        };
        let adapters = [basic, radeon];
        assert_eq!(adapters.iter().position(hardware_adapter), Some(1));
        assert!(!adapters[..1].iter().any(hardware_adapter));
    }
}

#[test]
fn hardware_adapter_selection_keeps_other_devices_and_rejects_software() {
    for (vendor, device) in [
        (0x1002, 0x7550),
        (0x10de, 0x2684),
        (0x8086, 0x56a0),
        (0x1414, 0x1234),
    ] {
        let mut desc = DXGI_ADAPTER_DESC1 {
            VendorId: vendor,
            DeviceId: device,
            ..Default::default()
        };
        assert!(hardware_adapter(&desc));
        desc.Flags = DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32;
        assert!(!hardware_adapter(&desc));
    }
}

#[test]
#[ignore = "requires an AMD GPU with AMF H.264, HEVC and AV1 encoding"]
fn headless_hardware_device_encodes_without_a_display_output() -> Result<()> {
    let _com = ComGuard::new()?;
    let gpu = Device::without_display("", "")?;
    assert!(gpu.output.is_none());
    assert!(gpu.display.display_name.is_empty());
    let image = Image {
        width: 640,
        height: 480,
        stride: 2560,
        bytes: vec![128; 640 * 480 * 4],
        captured: Instant::now(),
        pixel: Pixel::Bgra8,
    };
    for codec in 0..=2 {
        let config = butterpollo_core::rtsp::Negotiated {
            width: image.width,
            height: image.height,
            fps: 30,
            bitrate_kbps: 2000,
            codec,
            ..Default::default()
        };
        let mut encoder = crate::amf::Encoder::new_device(&config, gpu.clone())?;
        let mut encoded = false;
        for frame in 0..8 {
            if !encoder
                .encode(&image, frame == 0, config.bitrate_kbps)?
                .is_empty()
            {
                encoded = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            encoded,
            "codec {codec} produced no packet without an output"
        );
        eprintln!(
            "{}: codec {codec} encoded without an output",
            gpu.display.adapter
        );
    }
    Ok(())
}

#[test]
fn a_missing_selected_capture_display_is_an_error_not_the_desktop() {
    let display = |name: &str, primary| Display {
        device_id: format!("id-{name}"),
        display_name: name.into(),
        friendly_name: name.into(),
        width: 2560,
        height: 1440,
        x: 0,
        y: 0,
        primary,
        adapter: String::new(),
        adapter_index: 0,
        output_index: 0,
    };
    let choices = [display("virtual", false), display("physical", true)];
    assert_eq!(
        select_display(&choices, "virtual")
            .unwrap()
            .unwrap()
            .display_name,
        "virtual"
    );
    assert_eq!(
        select_display(&choices, "id-virtual")
            .unwrap()
            .unwrap()
            .display_name,
        "virtual"
    );
    assert!(select_display(&choices[1..], "virtual").is_err());
    assert!(select_display(&[], "virtual").is_err());
    assert_eq!(
        select_display(&choices, "").unwrap().unwrap().display_name,
        "physical"
    );
    assert_eq!(
        select_display(&choices[..1], "")
            .unwrap()
            .unwrap()
            .display_name,
        "virtual"
    );
    assert!(select_display(&[], "").unwrap().is_none());
}

#[test]
fn capture_timestamps_preserve_past_and_future_stamps_without_changing_pacing() {
    let now = Instant::now();
    let frequency = 10_000_000;
    let current = 100_000_000;
    for (offset, expected) in [
        (-10_000, now - Duration::from_millis(1)),
        (0, now),
        (7_900, now + Duration::from_micros(790)),
    ] {
        let (captured, stamp) = qpc_timestamps_at(current + offset, current, frequency, now);
        assert_eq!(stamp, Some(expected));
        assert_eq!(captured, expected.min(now));
    }
}

#[test]
fn capture_timestamps_keep_old_stamp_diagnostics_and_the_two_second_pacing_cutoff() {
    let now = Instant::now();
    for (age, clamped) in [(20_000_000, false), (20_000_001, true), (30_000_000, true)] {
        let expected = now - Duration::from_nanos(age as u64 * 100);
        let (captured, stamp) = qpc_timestamps_at(100_000_000 - age, 100_000_000, 10_000_000, now);
        assert_eq!(stamp, Some(expected));
        assert_eq!(captured, if clamped { now } else { expected });
    }
}

#[test]
fn capture_timestamps_do_not_report_missing_or_invalid_stamps_as_zero_delay() {
    let now = Instant::now();
    for (ticks, current, frequency) in [(0, 100, 10), (-1, 100, 10), (1, 0, 10), (1, 100, 0)] {
        assert_eq!(
            qpc_timestamps_at(ticks, current, frequency, now),
            (now, None)
        );
    }
}

fn hybrid_gpus() -> [Gpu; 2] {
    [
        Gpu {
            name: "AMD Radeon(TM) 680M".into(),
            vendor: 0x1002,
            dedicated_memory: 512 << 20,
            luid: (0x1_0001, 0),
            pnp_id: Some(r"PCI\VEN_1002&DEV_1681&SUBSYS_0B1F1028&REV_C8\3&11583659&0&10".into()),
        },
        Gpu {
            name: "AMD Radeon RX 6850M XT".into(),
            vendor: 0x1002,
            dedicated_memory: 12 << 30,
            luid: (0x1_0002, 0),
            pnp_id: Some(r"PCI\VEN_1002&DEV_73DF&SUBSYS_0B1F1028&REV_C3\4&2F6A3B4&0&0008".into()),
        },
    ]
}
#[test]
fn the_graphics_card_setting_names_one_gpu_of_a_hybrid_pc() {
    let gpus = hybrid_gpus();
    // Automatic leaves the GPU to the display.
    assert!(select_gpu(&gpus, "", "").unwrap().is_none());
    let radeon = select_gpu(&gpus, "AMD Radeon RX 6850M XT", "")
        .unwrap()
        .unwrap();
    assert_eq!((radeon.vendor, radeon.luid), (0x1002, (0x1_0002, 0)));
    // The stored identity wins over a name the driver has since changed,
    // compared without case as capture compares it.
    let identity = gpus[1].pnp_id.as_deref().unwrap().to_lowercase();
    let radeon = select_gpu(&gpus, "Radeon (TM) RX 6850M XT", &identity)
        .unwrap()
        .unwrap();
    assert_eq!(radeon.luid, (0x1_0002, 0));
    // A GPU that is gone is an error, never the display's GPU instead.
    assert!(select_gpu(&gpus, "AMD Radeon RX 9070 XT", "").is_err());
    assert!(select_gpu(&gpus, "AMD Radeon RX 6850M XT", r"PCI\VEN_1002&DEV_7550\0").is_err());
    // Two identical cards: the name picks the first; an identity must be unique.
    let mut twins = hybrid_gpus();
    twins[0].name = twins[1].name.clone();
    assert_eq!(
        select_gpu(&twins, "AMD Radeon RX 6850M XT", "")
            .unwrap()
            .unwrap()
            .luid,
        (0x1_0001, 0)
    );
    twins[0].pnp_id = twins[1].pnp_id.clone();
    assert!(select_gpu(&twins, "", gpus[1].pnp_id.as_deref().unwrap()).is_err());
}
#[test]
#[ignore = "requires native D3D12 compute copies and readback"]
fn the_pool_copies_on_the_compute_queue_and_never_overwrites_a_held_frame() -> Result<()> {
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    let upload = |value: u8| {
        GpuImage::upload(
            &gpu,
            &Image {
                width: 4,
                height: 4,
                stride: 16,
                bytes: vec![value; 64],
                captured: Instant::now(),
                pixel: Pixel::Bgra8,
            },
        )
    };
    let first = upload(32)?;
    let mut pool = GpuPool::new(gpu.compute.clone())?;
    let mut held = Vec::new();
    for _ in 0..GPU_SNAPSHOT_CAPACITY {
        held.push(
            pool.copy(&gpu, &first.texture, first.ready.as_ref())?
                .context("a free pool texture")?,
        );
    }
    // Every texture is held: the pool says so instead of overwriting one.
    assert!(!pool.has_capacity());
    assert!(pool.copy(&gpu, &first.texture, None)?.is_none());
    let second = upload(200)?;
    let released = held.pop().unwrap();
    let reused = std::sync::Arc::as_ptr(&released.texture);
    drop(released);
    let copy = pool
        .copy(&gpu, &second.texture, second.ready.as_ref())?
        .context("the released texture")?;
    assert_eq!(std::sync::Arc::as_ptr(&copy.texture), reused);
    assert_eq!(copy.readback(&mut None)?.bytes, vec![200; 64]);
    for frame in &held {
        assert_eq!(frame.readback(&mut None)?.bytes, vec![32; 64]);
    }
    drop(pool);
    // A frame outlives the pool that copied it.
    assert_eq!(copy.readback(&mut None)?.bytes, vec![200; 64]);
    Ok(())
}
