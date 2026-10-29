use super::*;

#[test]
fn snapshot_backpressure_keeps_final_native_update_until_consumers_release_space() -> Result<()> {
    use std::{collections::VecDeque, sync::Arc};
    // Shared capture supports more consumers than the eight snapshot slots.
    // Model consumers holding distinct earlier frames while their senders lag.
    let textures: Vec<_> = (0..GPU_SNAPSHOT_CAPACITY).map(Arc::new).collect();
    let mut consumers = textures.clone();
    let mut native = VecDeque::from([GPU_SNAPSHOT_CAPACITY]);
    let mut acquisitions = 0;
    for _ in 0..3 {
        let image = acquire_snapshot_frame(snapshot_capacity(&textures), || {
            acquisitions += 1;
            Ok(native.pop_front())
        })?;
        assert_eq!(image, None);
    }
    assert_eq!(acquisitions, 0);
    assert_eq!(native.len(), 1);

    // There is no subsequent desktop event to rescue a discarded final frame.
    consumers.pop();
    let image = acquire_snapshot_frame(snapshot_capacity(&textures), || {
        acquisitions += 1;
        Ok(native.pop_front())
    })?;
    assert_eq!(image, Some(GPU_SNAPSHOT_CAPACITY));
    assert_eq!(acquisitions, 1);
    assert!(native.is_empty());
    assert_eq!(textures.len(), GPU_SNAPSHOT_CAPACITY);
    assert_eq!(
        consumers.iter().map(|image| **image).collect::<Vec<_>>(),
        (0..GPU_SNAPSHOT_CAPACITY - 1).collect::<Vec<_>>()
    );
    Ok(())
}

#[test]
fn pending_wgc_snapshot_keeps_only_the_newest_update_and_its_timestamp() -> Result<()> {
    use std::sync::Arc;
    let textures: Vec<_> = (0..GPU_SNAPSHOT_CAPACITY).map(Arc::new).collect();
    let mut consumers = textures.clone();
    let first = Instant::now();
    let last = first + Duration::from_millis(8);
    let mut held = Some((1, first));
    assert!(acquire_snapshot_frame(snapshot_capacity(&textures), || Ok(held.take()))?.is_none());
    assert_eq!(held.replace((2, last)), Some((1, first)));
    assert!(acquire_snapshot_frame(snapshot_capacity(&textures), || Ok(held.take()))?.is_none());
    consumers.pop();
    assert_eq!(
        acquire_snapshot_frame(snapshot_capacity(&textures), || Ok(held.take()))?,
        Some((2, last))
    );
    assert!(held.is_none());
    Ok(())
}

#[test]
fn snapshot_capacity_can_reclaim_the_desktop_cache_without_overwriting_consumers() {
    use std::sync::Arc;
    let textures: Vec<_> = (0..GPU_SNAPSHOT_CAPACITY).map(Arc::new).collect();
    let consumers = textures[..GPU_SNAPSHOT_CAPACITY - 1].to_vec();
    let mut cached = Some(textures.last().unwrap().clone());
    assert!(!snapshot_capacity(&textures));
    // DDX may be the only remaining reader of its cached desktop.
    cached.take();
    assert!(snapshot_capacity(&textures));
    assert!(consumers.iter().all(|image| Arc::strong_count(image) == 2));
    assert!(snapshot_capacity(&textures[..GPU_SNAPSHOT_CAPACITY - 1]));
}

#[test]
fn available_snapshot_does_not_swallow_native_acquisition_failures() {
    let error = acquire_snapshot_frame::<()>(true, || bail!("capture access lost")).unwrap_err();
    assert_eq!(error.to_string(), "capture access lost");
}

#[test]
fn snapshot_backpressure_uses_the_normal_poll_instead_of_an_expired_deadline() {
    let now = Instant::now();
    let expired = now - Duration::from_millis(8);
    let poll = now + Duration::from_micros(500);
    // This is the capture worker's deadline selection. Holding the last WGC
    // update must not turn bounded backpressure into a busy loop.
    let blocked = snapshot_publication_deadline(false, Some(expired));
    assert_eq!(blocked.unwrap_or(poll).min(poll), poll);
    assert_eq!(
        snapshot_publication_deadline(true, Some(expired)),
        Some(expired)
    );
    assert_eq!(snapshot_publication_deadline(true, None), None);
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
fn desktop_duplication_refuses_a_turned_display_rather_than_stream_it_sideways() {
    for upright in [DXGI_MODE_ROTATION_UNSPECIFIED, DXGI_MODE_ROTATION_IDENTITY] {
        duplication_upright(upright, r"\\.\DISPLAY1").unwrap();
    }
    for turned in [
        DXGI_MODE_ROTATION_ROTATE90,
        DXGI_MODE_ROTATION_ROTATE180,
        DXGI_MODE_ROTATION_ROTATE270,
    ] {
        let error = duplication_upright(turned, r"\\.\DISPLAY1")
            .unwrap_err()
            .to_string();
        assert!(error.contains(r"\\.\DISPLAY1 is turned"), "{error}");
        assert!(error.contains("capture = auto or wgc"), "{error}");
    }
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

#[test]
fn wgc_ticks_convert_from_100ns_on_other_qpc_frequencies() {
    let now = Instant::now();
    let frequency = 24_000_000;
    assert_eq!(wgc_qpc(10_000_000, frequency), frequency);
    assert_eq!(wgc_qpc(0, frequency), 0);
    assert_eq!(wgc_qpc(-1, frequency), 0);
    assert_eq!(wgc_qpc(i64::MAX, frequency), i64::MAX);
    let stamp = now - Duration::from_micros(125);
    assert_eq!(
        qpc_timestamps_at(wgc_qpc(9_998_750, frequency), frequency, frequency, now),
        (stamp, Some(stamp))
    );
}

#[test]
fn newest_frame_selection_is_bounded_and_preserves_lone_static_updates() -> Result<()> {
    for limit in [1, WGC_FRAME_POOL_CAPACITY] {
        for frames in [vec![], vec![1], vec![1, 2, 3]] {
            let mut pending = std::collections::VecDeque::from(frames.clone());
            let mut acquired = 0;
            let mut checked = Vec::new();
            let mut closed = Vec::new();
            let newest = newest_capture_frame(
                limit,
                || {
                    acquired += 1;
                    Ok(pending.pop_front())
                },
                |frame| {
                    checked.push(*frame);
                    Ok(())
                },
                |frame| {
                    closed.push(frame);
                    Ok(())
                },
            )?;
            let consumed = frames.len().min(limit);
            assert_eq!(acquired, (frames.len() + 1).min(limit));
            assert_eq!(newest, frames[..consumed].last().copied());
            assert_eq!(checked, frames[..consumed]);
            assert_eq!(closed, frames[..consumed.saturating_sub(1)]);
            assert_eq!(pending.into_iter().collect::<Vec<_>>(), frames[consumed..]);
        }
    }
    Ok(())
}

#[test]
fn newest_frame_selection_closes_every_acquired_frame_on_resize() {
    for invalid in [1, 2] {
        let mut acquired = 0;
        let mut closed = Vec::new();
        let error = newest_capture_frame(
            WGC_FRAME_POOL_CAPACITY,
            || {
                acquired += 1;
                Ok(Some(acquired))
            },
            |frame| {
                if *frame == invalid {
                    bail!("capture output dimensions changed");
                }
                Ok(())
            },
            |frame| {
                closed.push(frame);
                Ok(())
            },
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "capture output dimensions changed");
        assert_eq!(acquired, invalid);
        closed.sort_unstable();
        assert_eq!(closed, (1..=invalid).collect::<Vec<_>>());
    }
}

#[test]
fn newest_frame_selection_preserves_pool_errors_and_closes_a_selected_frame() {
    let mut acquired = 0;
    let mut closed = Vec::new();
    let error = newest_capture_frame(
        WGC_FRAME_POOL_CAPACITY,
        || {
            acquired += 1;
            if acquired == 2 {
                bail!("capture pool lost access");
            }
            Ok(Some(acquired))
        },
        |_| Ok(()),
        |frame| {
            closed.push(frame);
            bail!("frame also failed to close");
        },
    )
    .unwrap_err();
    assert_eq!(error.to_string(), "capture pool lost access");
    assert_eq!(acquired, 2);
    assert_eq!(closed, [1]);
}

#[test]
fn newest_frame_selection_closes_the_replacement_when_superseded_close_fails() {
    let mut acquired = 0;
    let mut closed = Vec::new();
    let error = newest_capture_frame(
        WGC_FRAME_POOL_CAPACITY,
        || {
            acquired += 1;
            Ok(Some(acquired))
        },
        |_| Ok(()),
        |frame| {
            closed.push(frame);
            if frame == 1 {
                bail!("superseded frame failed to close");
            }
            Ok(())
        },
    )
    .unwrap_err();
    assert_eq!(error.to_string(), "superseded frame failed to close");
    assert_eq!(acquired, 2);
    assert_eq!(closed, [1, 2]);
}

#[test]
fn wgc_startup_failure_uses_the_same_fallback_as_recovery() {
    let policy = butterpollo_core::framegen::Policy::resolve(
        &Default::default(),
        butterpollo_core::framegen::Rate(60000),
        false,
        "none",
        false,
        false,
    )
    .unwrap();
    let mut attempted = Vec::new();
    let warnings = butterpollo_core::session::Warnings::default();
    let capture = open_stream_capture(&policy.capture, &warnings, |kind| {
        attempted.push(kind.to_owned());
        match kind {
            "wgc" => bail!("CreateForMonitor: 0x80070424"),
            "ddx" => Ok("working duplication"),
            _ => unreachable!(),
        }
    })
    .unwrap();
    assert_eq!(capture, "working duplication");
    assert_eq!(attempted, ["wgc", "ddx"]);
    let warning = &warnings.snapshot()[0];
    assert_eq!(warning.code, "capture_backend");
    assert!(warning.message.contains("0x80070424"));
    assert!(warning.message.contains("Desktop Duplication"));
    open_stream_capture("wgc", &warnings, |_| Ok(())).unwrap();
    assert!(warnings.snapshot().is_empty());
    open_stream_capture("auto", &warnings, |_| {
        open_stream_capture("wgc", &warnings, |kind| {
            if kind == "wgc" {
                bail!("helper unavailable");
            }
            Ok(())
        })
    })
    .unwrap();
    assert_eq!(warnings.snapshot()[0].code, "capture_backend");

    let error = open_stream_capture::<()>("wgc", &warnings, |kind| match kind {
        "wgc" => bail!("CreateForMonitor: 0x80070424"),
        _ => bail!("DuplicateOutput: access denied"),
    })
    .unwrap_err();
    let detail = format!("{error:#}");
    assert!(detail.contains("0x80070424"));
    assert!(detail.contains("DuplicateOutput: access denied"));
}

#[test]
fn working_wgc_and_explicit_ddx_do_not_open_another_backend() {
    for kind in ["wgc", "ddx", "dxgi"] {
        let mut attempts = 0;
        open_stream_capture(kind, &Default::default(), |backend| {
            attempts += 1;
            assert_eq!(backend, kind);
            Ok(())
        })
        .unwrap();
        assert_eq!(attempts, 1);
    }
    for kind in ["ddx", "dxgi"] {
        let mut attempts = 0;
        let error = open_stream_capture::<()>(kind, &Default::default(), |_| {
            attempts += 1;
            bail!("duplication lost access")
        })
        .unwrap_err();
        assert_eq!(attempts, 1);
        assert_eq!(error.to_string(), "duplication lost access");
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
fn gpu_renders_the_display(owner: &str, owner_vendor: u32, selected: &str) -> GpuMismatch {
    GpuMismatch {
        display: r"\\.\DISPLAY24".into(),
        owner: owner.into(),
        owner_vendor,
        selected: selected.into(),
    }
}
#[test]
fn desktop_duplication_moves_to_the_amd_gpu_that_renders_the_display() {
    // Issue #28: Desktop Duplication on the selected GPU was refused because
    // another GPU renders the virtual display. The stream ended with no
    // picture; when that GPU is AMD it now captures and encodes there and
    // says so.
    let amd_renders = || gpu_renders_the_display("AMD Radeon(TM) 680M", 0x1002, "NVIDIA RTX 3050");
    let warnings = butterpollo_core::session::Warnings::default();
    let capture = open_stream_capture("wgc", &warnings, |kind| match kind {
        "wgc" => bail!("The parameter is incorrect. (0x80070057)"),
        _ => duplication_device(
            "device on NVIDIA RTX 3050",
            |_| Some(amd_renders()),
            || Ok("device on AMD Radeon(TM) 680M"),
            &warnings,
        ),
    })
    .unwrap();
    assert_eq!(capture, "device on AMD Radeon(TM) 680M");
    let snapshot = warnings.snapshot();
    let adapter = snapshot
        .iter()
        .find(|w| w.code == "capture_adapter")
        .expect("the GPU fallback is reported");
    assert!(adapter.message.starts_with(
        "Capturing and encoding on AMD Radeon(TM) 680M, not on the selected NVIDIA RTX 3050"
    ));
    assert!(adapter.message.contains(r"\\.\DISPLAY24"));
    assert!(snapshot.iter().any(|w| w.code == "capture_backend"));

    // On the display's own GPU nothing is reopened or reported.
    let warnings = butterpollo_core::session::Warnings::default();
    let device = duplication_device(
        "device on AMD Radeon(TM) 680M",
        |_| None,
        || unreachable!("the selected GPU renders the display"),
        &warnings,
    )
    .unwrap();
    assert_eq!(device, "device on AMD Radeon(TM) 680M");
    assert!(warnings.snapshot().is_empty());

    // A display GPU that cannot be opened fails loudly with both GPUs named.
    let error = duplication_device::<&str>(
        "device on NVIDIA RTX 3050",
        |_| Some(amd_renders()),
        || bail!("selected capture display is unavailable"),
        &warnings,
    )
    .unwrap_err();
    let detail = format!("{error:#}");
    assert!(detail.contains("only on the GPU it is connected to (AMD Radeon(TM) 680M)"));
    assert!(detail.contains("not on NVIDIA RTX 3050"));
    assert!(detail.contains("selected capture display is unavailable"));
    assert!(warnings.snapshot().is_empty());
}
#[test]
fn desktop_duplication_on_a_display_driven_by_another_vendor_suggests_wgc() {
    // The monitor sits on the Intel iGPU and AMD is selected: moving there
    // would encode on Intel, which 3.0 refuses, so it fails with WGC advice.
    let warnings = butterpollo_core::session::Warnings::default();
    let error = duplication_device::<&str>(
        "device on AMD Radeon R7 M370",
        |_| {
            Some(gpu_renders_the_display(
                "Intel(R) HD Graphics 530",
                0x8086,
                "AMD Radeon R7 M370",
            ))
        },
        || unreachable!("the Intel GPU is not opened for capture"),
        &warnings,
    )
    .unwrap_err();
    let detail = format!("{error:#}");
    assert!(detail.contains("only on the GPU it is connected to (Intel(R) HD Graphics 530)"));
    assert!(detail.contains("use WGC capture"));
    assert!(warnings.snapshot().is_empty());

    // No GPU chosen, so the device is already the Intel one: the refusal.
    let all = [
        Gpu {
            name: "Intel(R) HD Graphics 530".into(),
            vendor: 0x8086,
            dedicated_memory: 0,
            luid: (1, 0),
            pnp_id: None,
        },
        {
            let [amd, _] = hybrid_gpus();
            amd
        },
    ];
    let error = require_amd_duplication("Intel(R) HD Graphics 530", 0x8086, &all).unwrap_err();
    let detail = format!("{error:#}");
    assert!(detail.starts_with("Intel(R) HD Graphics 530 is not an AMD graphics card"));
    assert!(detail.contains("AMD Radeon(TM) 680M"));
    assert!(detail.contains("use WGC capture"));
    assert!(require_amd_duplication("AMD Radeon(TM) 680M", 0x1002, &all).is_ok());
}
#[test]
fn a_stream_pins_to_the_amd_gpu_when_the_pc_has_another_vendors_too() {
    let none = || (String::new(), String::new());
    let mut gpus = hybrid_gpus();
    gpus[0].vendor = 0x8086;
    gpus[0].name = "Intel(R) UHD Graphics".into();
    // Automatic: the AMD GPU, by its identity.
    assert_eq!(
        pinned_choice(&gpus, None, "", ""),
        (
            "AMD Radeon RX 6850M XT".to_string(),
            gpus[1].pnp_id.clone().unwrap()
        )
    );
    // A display on the Intel GPU moves there too.
    assert_eq!(
        pinned_choice(&gpus, Some(0x8086), "", ""),
        pinned_choice(&gpus, None, "", "")
    );
    // A chosen GPU stays, whatever its vendor.
    assert_eq!(
        pinned_choice(&gpus, None, "Intel(R) UHD Graphics", ""),
        ("Intel(R) UHD Graphics".to_string(), String::new())
    );
    // An all-AMD PC, or one with no AMD GPU, leaves Automatic alone.
    assert_eq!(pinned_choice(&hybrid_gpus(), None, "", ""), none());
    let mut no_amd = hybrid_gpus();
    no_amd[0].vendor = 0x8086;
    no_amd[1].vendor = 0x10de;
    assert_eq!(pinned_choice(&no_amd, None, "", ""), none());
    assert_eq!(pinned_choice(&[], None, "", ""), none());
}
#[test]
fn a_stream_pins_to_the_biggest_radeon_and_leaves_a_display_already_on_one() {
    // Two Radeons (the 680M iGPU and a 12 GB RX 6850M XT) beside an NVIDIA GPU.
    let [igpu, dgpu] = hybrid_gpus();
    let nvidia = Gpu {
        name: "NVIDIA GeForce RTX 3050".into(),
        vendor: 0x10de,
        dedicated_memory: 4 << 30,
        luid: (1, 0),
        pnp_id: None,
    };
    let dgpu_choice = (dgpu.name.clone(), dgpu.pnp_id.clone().unwrap());
    let gpus = [nvidia, igpu, dgpu];
    // Not the first Radeon: the one with the most video memory.
    assert_eq!(pinned_choice(&gpus, None, "", ""), dgpu_choice);
    assert_eq!(pinned_choice(&gpus, Some(0x10de), "", ""), dgpu_choice);
    // The monitor is on a Radeon already: capture stays on that one.
    assert_eq!(
        pinned_choice(&gpus, Some(0x1002), "", ""),
        (String::new(), String::new())
    );
}
#[test]
fn with_no_display_the_stream_gpu_is_the_pinned_or_only_radeon_or_refused() {
    let other = |name: &str, vendor| Gpu {
        name: name.into(),
        vendor,
        dedicated_memory: 4 << 30,
        luid: (vendor, 0),
        pnp_id: None,
    };
    let nvidia = || other("NVIDIA GeForce RTX 3050", 0x10de);
    let intel = || other("Intel(R) UHD Graphics", 0x8086);
    // Beside another vendor's GPU: the Radeon a stream pins to, the biggest.
    let [igpu, dgpu] = hybrid_gpus();
    let gpus = [nvidia(), igpu, dgpu];
    assert_eq!(headless_gpu(&gpus).unwrap().name, "AMD Radeon RX 6850M XT");
    // The only Radeon, alone or beside another vendor's GPU.
    let [igpu, dgpu] = hybrid_gpus();
    assert_eq!(
        headless_gpu(&[dgpu]).unwrap().name,
        "AMD Radeon RX 6850M XT"
    );
    assert_eq!(
        headless_gpu(&[intel(), igpu]).unwrap().name,
        "AMD Radeon(TM) 680M"
    );
    // Several Radeons and none chosen, or none at all: refused, not guessed.
    let error = format!(
        "{:#}",
        headless_gpu(&hybrid_gpus())
            .map(|gpu| &gpu.name)
            .unwrap_err()
    );
    assert!(error.contains("Graphics card setting"), "{error}");
    for gpus in [vec![nvidia(), intel()], vec![]] {
        let error = format!(
            "{:#}",
            headless_gpu(&gpus).map(|gpu| &gpu.name).unwrap_err()
        );
        assert!(error.contains("no AMD GPU"), "{error}");
    }
}
#[test]
fn the_probe_skips_a_non_amd_display_only_when_no_gpu_is_chosen_and_an_amd_one_exists() {
    let (nvidia, amd, intel) = (0x10de, 0x1002, 0x8086);
    // The monitor on NVIDIA, an AMD iGPU beside it, Graphics card Automatic.
    assert!(probe_skips_display(nvidia, false, &[nvidia, amd]));
    assert!(probe_skips_display(intel, false, &[amd, intel]));
    // A chosen GPU is probed as chosen; an AMD monitor stays as it is.
    assert!(!probe_skips_display(nvidia, true, &[nvidia, amd]));
    assert!(!probe_skips_display(amd, false, &[nvidia, amd]));
    // Nothing AMD to move to: the encoder says why on the display's GPU.
    assert!(!probe_skips_display(nvidia, false, &[nvidia, intel]));
    assert!(!probe_skips_display(nvidia, false, &[]));
}

#[test]
fn wgc_device_removal_reaches_recovery_without_a_backend_fallback() {
    let loss = crate::device_loss::DeviceLost(DXGI_ERROR_DEVICE_REMOVED.0);
    let error = open_stream_capture::<()>("wgc", &Default::default(), |kind| {
        assert_eq!(kind, "wgc");
        Err(anyhow::Error::new(loss).context("frame pool failed"))
    })
    .unwrap_err();
    assert_eq!(
        crate::device_loss::DeviceLost::from_error(&error),
        Some(loss)
    );
}

#[test]
#[ignore = "opens brief WGC sessions; requires Windows MinUpdateInterval support"]
fn wgc_low_rate_explicitly_disables_the_windows_capture_throttle() -> Result<()> {
    enable_dpi_awareness();
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    for (high_rate, ticks) in [(false, 0), (true, 10_000), (false, 0)] {
        let capture = Wgc::new_device(gpu.clone(), false, high_rate, None)?;
        assert_eq!(capture.session.MinUpdateInterval()?.Duration, ticks);
    }
    Ok(())
}

#[test]
#[ignore = "requires an interactive Desktop Duplication output"]
fn ddx_snapshots_survive_reacquisition_and_duplication_teardown() -> Result<()> {
    enable_dpi_awareness();
    let _display_awake = crate::timing::DisplayAwake::enter()?;
    let _com = ComGuard::new()?;
    let timer = crate::timing::Timer::new()?;
    for _ in 0..3 {
        let mut capture = Duplication::new("")?;
        let deadline = Instant::now() + Duration::from_secs(3);
        let image = loop {
            if let Some(image) = capture.next_gpu()? {
                break image;
            }
            anyhow::ensure!(
                Instant::now() < deadline,
                "DDX produced no initial snapshot"
            );
            timer.until(Instant::now() + Duration::from_millis(1));
        };
        let original = image.readback(&mut None)?;
        // Keep the original owned snapshot through new acquisitions and a
        // CPU/GPU transition. Nothing is written to disk or the display.
        capture.next(Duration::ZERO)?;
        for _ in 0..24 {
            capture.next_gpu()?;
            timer.until(Instant::now() + Duration::from_millis(1));
        }
        drop(capture);
        assert_eq!(image.readback(&mut None)?.bytes, original.bytes);
    }
    Ok(())
}

#[test]
#[ignore = "requires native D3D11 texture copies and readback"]
fn pointer_only_snapshots_reuse_pixels_and_missed_desktop_updates_invalidate_them() -> Result<()> {
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    let upload = |value| {
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
    let first_source = upload(32)?;
    let second_source = upload(128)?;
    let third_source = upload(224)?;
    let mut pool = GpuPool::default();
    let mut cached = None;
    let first = pool
        .desktop_snapshot(&gpu, &first_source.texture, &mut cached, true)?
        .unwrap();
    let pointer = pool
        .desktop_snapshot(&gpu, &second_source.texture, &mut cached, false)?
        .unwrap();
    assert!(std::sync::Arc::ptr_eq(&first.texture, &pointer.texture));
    let second = pool
        .desktop_snapshot(&gpu, &second_source.texture, &mut cached, true)?
        .unwrap();
    assert!(!std::sync::Arc::ptr_eq(&first.texture, &second.texture));
    assert_eq!(first.readback(&mut None)?.bytes, vec![32; 64]);
    assert_eq!(second.readback(&mut None)?.bytes, vec![128; 64]);
    let mut held = vec![first, pointer, second];
    for _ in 0..6 {
        held.push(
            pool.desktop_snapshot(&gpu, &second_source.texture, &mut cached, true)?
                .unwrap(),
        );
    }
    assert!(
        pool.desktop_snapshot(&gpu, &third_source.texture, &mut cached, true)?
            .is_none()
    );
    assert!(cached.is_none());
    held.clear();
    let recovered = pool
        .desktop_snapshot(&gpu, &third_source.texture, &mut cached, false)?
        .unwrap();
    assert_eq!(recovered.readback(&mut None)?.bytes, vec![224; 64]);
    Ok(())
}

#[test]
#[ignore = "requires native D3D11/D3D12 sharing and readback"]
fn unshareable_frame_falls_back_without_waiting_for_another_desktop_update() -> Result<()> {
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    let expected = [32, 64, 128, 255].repeat(16);
    let uploaded = GpuImage::upload(
        &gpu,
        &Image {
            width: 4,
            height: 4,
            stride: 16,
            bytes: expected.clone(),
            captured: Instant::now(),
            pixel: Pixel::Bgra8,
        },
    )?;
    // A normal D3D11-only snapshot cannot be opened by the compute queue.
    let source = GpuPool::default().copy(&gpu, &uploaded.texture)?.unwrap();
    assert!(!crate::compute::shareable(&source.texture));
    let mut pool = GpuPool {
        compute: Some(crate::compute::Handoff::new(
            crate::compute::Compute::for_device(&gpu.device)?,
            &gpu,
        )?),
        ..Default::default()
    };
    let frame = pool
        .copy(&gpu, &source.texture)?
        .context("fallback must return this frame, even if the desktop never updates again")?;
    assert!(pool.compute.is_none());
    assert!(!crate::compute::shareable(&frame.texture));
    assert!(frame.ready.is_none());
    drop(pool);
    assert_eq!(frame.readback(&mut None)?.bytes, expected);
    Ok(())
}

#[test]
#[ignore = "requires a moving desktop, WGC and native AMD D3D12 sharing"]
fn wgc_compute_snapshots_match_d3d11_during_motion() -> Result<()> {
    enable_dpi_awareness();
    let _com = ComGuard::new()?;
    let _awake = crate::timing::DisplayAwake::enter()?;
    let mut capture = Capture::new_options("", "wgc", false, &Default::default())?;
    let Capture::Wgc(wgc) = &mut capture else {
        bail!("expected WGC");
    };
    anyhow::ensure!(wgc.owned.compute.is_some(), "compute copies unavailable");
    let deadline = Instant::now() + Duration::from_secs(8);
    let (mut checked, mut changed) = (0, 0);
    let mut previous = Vec::new();
    let mut held = None;
    let mut reference_staging = None;
    let mut actual_staging = None;
    while checked < 120 && Instant::now() < deadline {
        let Some(frame) = wgc.try_frame()? else {
            std::thread::sleep(Duration::from_millis(1));
            continue;
        };
        let result = (|| -> Result<()> {
            wgc.check_size(&frame)?;
            let surface = frame.Surface()?;
            let access: IDirect3DDxgiInterfaceAccess = surface.cast()?;
            // SAFETY: `access` is the surface of `frame`, which is closed only after this closure.
            let source: ID3D11Texture2D = unsafe { access.GetInterface()? };
            let snapshot = wgc.owned.copy(&wgc.gpu, &source)?.context("copy dropped")?;
            anyhow::ensure!(snapshot.ready.is_some(), "compute fell back to graphics");
            // Read the candidate first. Reading the source before submitting
            // the copy would hide missing producer/consumer synchronization.
            let actual = snapshot.readback(&mut actual_staging)?;
            let reference = read_texture(&wgc.gpu, &source, &mut reference_staging)?;
            anyhow::ensure!(
                actual.bytes == reference.bytes,
                "WGC compute copy differed on frame {checked}"
            );
            if !previous.is_empty() && previous != reference.bytes {
                changed += 1;
            }
            if held.is_none() {
                held = Some((snapshot, reference.bytes.clone()));
            }
            previous = reference.bytes;
            checked += 1;
            Ok(())
        })();
        frame.Close()?;
        result?;
    }
    eprintln!("WGC compute verification: {checked} exact frames, {changed} content changes");
    anyhow::ensure!(
        checked >= 30 && changed >= 10,
        "moving source required; static frames do not validate synchronization"
    );
    drop(capture);
    let (snapshot, expected) = held.context("no retained snapshot")?;
    assert_eq!(snapshot.readback(&mut None)?.bytes, expected);
    Ok(())
}

#[test]
#[ignore = "requires an interactive Windows desktop with WGC support"]
fn wgc_reconnect_and_com_teardown_keep_the_runtime_loaded() -> Result<()> {
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    for cycle in 0..16 {
        std::thread::spawn(move || -> Result<()> {
            let com = ComGuard::new()?;
            let mut capture = Wgc::new("")?;
            capture.enable_notifications()?;
            let timer = crate::timing::Timer::new()?;
            let until = Instant::now() + Duration::from_secs(5);
            loop {
                if let Some(frame) = capture.next_gpu()? {
                    assert!(frame.width > 0 && frame.height > 0);
                    break;
                }
                anyhow::ensure!(
                    Instant::now() < until,
                    "WGC reconnect {cycle} produced no frame"
                );
                timer.until_or_signal(until, &capture.notifications.as_ref().unwrap().0)?;
            }
            capture.close();
            assert!(
                capture.next_gpu().is_err(),
                "a closed pool must trigger GPU capture recovery"
            );
            assert!(
                capture.next_frame().is_err(),
                "a closed pool must trigger CPU capture recovery"
            );
            drop(capture);
            drop(com);
            // Reproduce the unload boundary before a subsequent stream/thread.
            // SAFETY: CoFreeUnusedLibrariesEx takes no pointers, and the name is a static
            // NUL-terminated literal.
            unsafe {
                CoFreeUnusedLibrariesEx(0, None);
                GetModuleHandleW(windows::core::w!("GraphicsCapture.dll"))?;
            }
            Ok(())
        })
        .join()
        .expect("capture worker panicked")?;
    }
    Ok(())
}

/// A top-level window of the predefined STATIC class, destroyed on drop.
struct TestWindow(windows::Win32::Foundation::HWND);
impl TestWindow {
    fn new(style: windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE) -> Result<Self> {
        use windows::Win32::UI::WindowsAndMessaging::{CreateWindowExW, WINDOW_EX_STYLE};
        // SAFETY: The class and title are static NUL-terminated literals; no other pointers.
        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                windows::core::w!("STATIC"),
                windows::core::w!("Rubylight capture test"),
                style,
                100,
                100,
                640,
                360,
                None,
                None,
                None,
                None,
            )?
        };
        Ok(Self(hwnd))
    }
    fn target(&self, display: &str) -> String {
        WindowTarget::new(self.0.0 as u64, display).to_string()
    }
    /// Handles its messages; the test thread owns the window.
    fn pump(&self) {
        use windows::Win32::UI::WindowsAndMessaging::{
            DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, TranslateMessage,
        };
        let mut message = MSG::default();
        // SAFETY: `message` is a live local; the messages are this thread's.
        unsafe {
            while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
    }
}
impl Drop for TestWindow {
    fn drop(&mut self) {
        // SAFETY: The window was created on this thread; a destroyed one only fails the call.
        let _ = unsafe { windows::Win32::UI::WindowsAndMessaging::DestroyWindow(self.0) };
    }
}

fn open_error(name: &str, kind: &str) -> String {
    let warnings = std::sync::Arc::new(butterpollo_core::session::Warnings::default());
    let error =
        Capture::open_for_stream_reported(name, kind, false, &Default::default(), warnings.clone())
            .err()
            .expect("the window target opened");
    // Nothing else was tried in its place.
    assert!(
        warnings.snapshot().is_empty(),
        "{:?}",
        warnings
            .snapshot()
            .into_iter()
            .map(|warning| warning.message)
            .collect::<Vec<_>>()
    );
    format!("{error:#}")
}

#[test]
fn window_targets_never_fall_back_to_display_capture() -> Result<()> {
    use windows::Win32::UI::WindowsAndMessaging::{WS_MINIMIZE, WS_POPUP};
    // Hidden windows: nothing appears, and no GPU is opened before the refusals.
    let normal = TestWindow::new(WS_POPUP)?;
    let minimised = TestWindow::new(WS_POPUP | WS_MINIMIZE)?;
    let display = r"\\.\DISPLAY1";
    for kind in ["ddx", "dxgi", "idd-push"] {
        let error = open_error(&normal.target(display), kind);
        assert!(
            error.contains("only be captured with Windows Graphics Capture"),
            "{kind}: {error}"
        );
    }
    for kind in ["wgc", "auto"] {
        let error = open_error(&minimised.target(display), kind);
        assert!(
            error.starts_with("the game window is minimised"),
            "{kind}: {error}"
        );
    }
    let closed = normal.target(display);
    drop(normal);
    for kind in ["wgc", "auto"] {
        let error = open_error(&closed, kind);
        assert!(
            error.starts_with("the game window closed"),
            "{kind}: {error}"
        );
    }
    let error = open_error("window:12@", "wgc");
    assert!(error.contains("names no display"), "{error}");
    // Recovery never swaps a window for the primary display: the reopen
    // reports what is wrong with the window target.
    assert!(display_present(&closed));
    assert!(display_present(r"window:12@\\.\NO_SUCH_DISPLAY"));
    assert!(display_present("window:garbage"));
    assert!(!display_present(r"\\.\NO_SUCH_DISPLAY"));
    Ok(())
}

#[test]
#[ignore = "shows a test window and captures it with WGC; needs an idle interactive desktop"]
fn wgc_window_capture_follows_resize_and_fails_on_minimise_and_close() -> Result<()> {
    use windows::Win32::UI::WindowsAndMessaging::{
        SW_MINIMIZE, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOZORDER, SetWindowPos,
        ShowWindow, WS_OVERLAPPEDWINDOW,
    };
    // Its log names the restart at the new size, which rc.4 lacked.
    let _ = tracing_subscriber::fmt()
        .with_test_writer()
        .with_ansi(false)
        .with_max_level(tracing::Level::INFO)
        .try_init();
    enable_dpi_awareness();
    let _com = ComGuard::new()?;
    let window = TestWindow::new(WS_OVERLAPPEDWINDOW)?;
    // SAFETY: The window was created on this thread.
    unsafe {
        let _ = ShowWindow(window.0, SW_SHOWNOACTIVATE);
    }
    let display = primary_display().context("no display")?;
    let mut capture = Capture::new(&window.target(&display), "wgc")?;
    assert_eq!(capture.backend(), "wgc");
    // what names the step: rc.4's resize failure read as a missing first frame.
    let next = |capture: &mut Capture,
                what: &str,
                until: &dyn Fn(&GpuImage) -> bool|
     -> Result<GpuImage> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            window.pump();
            if let Some(image) = capture.next_gpu()?
                && until(&image)
            {
                return Ok(image);
            }
            ensure!(Instant::now() < deadline, "no {what} within 5 s");
            std::thread::sleep(Duration::from_millis(5));
        }
    };
    let first = next(&mut capture, "first window frame", &|_| true)?;
    tracing::info!(
        width = first.width,
        height = first.height,
        "first window frame"
    );
    // SAFETY: The window was created on this thread.
    unsafe {
        SetWindowPos(
            window.0,
            None,
            0,
            0,
            960,
            540,
            SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
        )?;
    }
    // The STATIC window draws once after the resize and then never again, like
    // a paused game: its new-size frame comes only from the restarted capture.
    let resized = next(&mut capture, "window frame at the new size", &|image| {
        image.width != first.width
    })?;
    assert!(resized.width > first.width && resized.height > first.height);
    // SAFETY: The window was created on this thread.
    unsafe {
        let _ = ShowWindow(window.0, SW_MINIMIZE);
    }
    let Err(error) = next(&mut capture, "refusal of the minimised window", &|_| false) else {
        unreachable!("no frame matches");
    };
    let error = format!("{error:#}");
    assert!(error.starts_with("the game window is minimised"), "{error}");
    let mut capture = {
        // SAFETY: The window was created on this thread.
        unsafe {
            let _ = ShowWindow(window.0, SW_SHOWNOACTIVATE);
        }
        window.pump();
        Capture::new(&window.target(&display), "wgc")?
    };
    next(&mut capture, "window frame after the restore", &|_| true)?;
    drop(window);
    let deadline = Instant::now() + Duration::from_secs(5);
    let error = loop {
        match capture.next_gpu() {
            Err(error) => break format!("{error:#}"),
            Ok(_) => {
                ensure!(
                    Instant::now() < deadline,
                    "the closed window kept capturing"
                );
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    };
    assert!(error.starts_with("the game window closed"), "{error}");
    Ok(())
}
