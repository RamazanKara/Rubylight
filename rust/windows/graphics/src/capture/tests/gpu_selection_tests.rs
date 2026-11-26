//! Unit tests for hybrid-GPU selection, Desktop Duplication mismatch recovery,
//! headless GPU pinning, device-loss propagation, and DXGI pixel format mapping.

use super::*;

fn hybrid_gpus() -> [Gpu; 2] {
    [
        Gpu {
            name: "AMD Radeon(TM) 680M".into(),
            vendor: 0x1002,
            dedicated_memory: 512 << 20,
            device: 0,
            luid: (0x1_0001, 0),
            pnp_id: Some(r"PCI\VEN_1002&DEV_1681&SUBSYS_0B1F1028&REV_C8\3&11583659&0&10".into()),
            preference: None,
        },
        Gpu {
            name: "AMD Radeon RX 6850M XT".into(),
            vendor: 0x1002,
            dedicated_memory: 12 << 30,
            device: 0,
            luid: (0x1_0002, 0),
            pnp_id: Some(r"PCI\VEN_1002&DEV_73DF&SUBSYS_0B1F1028&REV_C3\4&2F6A3B4&0&0008".into()),
            preference: None,
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
    let warnings = butterpollo_media_policy::RecordedWarnings::default();
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
    let warnings = butterpollo_media_policy::RecordedWarnings::default();
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
    let warnings = butterpollo_media_policy::RecordedWarnings::default();
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
            device: 0,
            luid: (1, 0),
            pnp_id: None,
            preference: None,
        },
        {
            let [amd, _] = hybrid_gpus();
            amd
        },
    ];
    let error = require_encoding_duplication("Intel(R) HD Graphics 530", 0x8086, &all).unwrap_err();
    let detail = format!("{error:#}");
    assert!(
        detail
            .starts_with("Intel(R) HD Graphics 530 is neither an AMD nor an NVIDIA graphics card")
    );
    assert!(detail.contains("AMD Radeon(TM) 680M"));
    assert!(detail.contains("use WGC capture"));
    assert!(require_encoding_duplication("AMD Radeon(TM) 680M", 0x1002, &all).is_ok());
    // An NVIDIA GPU encodes with NVENC where it captures.
    assert!(require_encoding_duplication("NVIDIA RTX 3050", 0x10de, &all).is_ok());
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
    // An all-AMD PC with an iGPU and a card pins the card; one with no GPU,
    // or a single one, leaves Automatic alone.
    let [_, card] = hybrid_gpus();
    assert_eq!(
        pinned_choice(&hybrid_gpus(), None, "", ""),
        (card.name.clone(), card.pnp_id.clone().unwrap())
    );
    assert_eq!(pinned_choice(&[card], None, "", ""), none());
    assert_eq!(pinned_choice(&[], None, "", ""), none());
    // An Intel GPU beside an NVIDIA one: the NVIDIA GPU, which NVENC encodes on.
    let mut nvidia = hybrid_gpus();
    nvidia[0].vendor = 0x8086;
    nvidia[1].vendor = 0x10de;
    assert_eq!(
        pinned_choice(&nvidia, None, "", ""),
        (
            "AMD Radeon RX 6850M XT".to_string(),
            nvidia[1].pnp_id.clone().unwrap()
        )
    );
    // An all-NVIDIA PC pins its bigger GPU too, unless the display is on one.
    nvidia[0].vendor = 0x10de;
    assert_eq!(
        pinned_choice(&nvidia, None, "", "").0,
        "AMD Radeon RX 6850M XT"
    );
    assert_eq!(pinned_choice(&nvidia, Some(0x10de), "", ""), none());
}
#[test]
fn the_probe_without_a_display_opens_the_gpu_the_stream_pins() {
    // Issue 134: Automatic, no display, an AMD iGPU listed before an RTX 5090.
    let [igpu, _] = hybrid_gpus();
    let nvidia = Gpu {
        name: "NVIDIA GeForce RTX 5090".into(),
        vendor: 0x10de,
        dedicated_memory: 32 << 30,
        device: 0,
        luid: (1, 0),
        pnp_id: None,
        preference: None,
    };
    let gpus = [igpu, nvidia];
    let adapters: Vec<encoder_gpu::Ranked> = gpus
        .iter()
        .map(|gpu| encoder_gpu::Ranked::new(gpu.vendor, gpu.dedicated_memory))
        .collect();
    let probed = &gpus[encoder_gpu::headless(&adapters)];
    assert_eq!(probed.name, "NVIDIA GeForce RTX 5090");
    assert_eq!(pinned_choice(&gpus, None, "", "").0, probed.name);
    assert_eq!(headless_gpu(&gpus).unwrap().name, probed.name);
    // The adapter descriptions `Device::without_display` chooses among: the
    // iGPU's shared memory, larger than the card's, must not count.
    let desc = |vendor, dedicated: u64, shared: u64| DXGI_ADAPTER_DESC1 {
        VendorId: vendor,
        DedicatedVideoMemory: dedicated as usize,
        SharedSystemMemory: shared as usize,
        ..Default::default()
    };
    let descs = [
        desc(0x1002, 512 << 20, 48 << 30),
        desc(0x10de, 32 << 30, 16 << 30),
    ];
    assert_eq!(headless_candidate(&descs, &[]), 1);
    // One vendor's iGPU before its card: the card, whose codecs `/serverinfo`
    // reports.
    assert_eq!(
        headless_candidate(&[desc(0x1002, 1 << 30, 0), desc(0x1002, 12 << 30, 0)], &[]),
        1
    );
    // A single adapter is the one probed.
    assert_eq!(headless_candidate(&[desc(0x1002, 1 << 30, 0)], &[]), 0);
}
#[test]
fn a_stream_pins_to_the_biggest_radeon_and_leaves_a_display_already_on_one() {
    // Two Radeons (the 680M iGPU and a 12 GB RX 6850M XT) beside an NVIDIA GPU.
    let [igpu, dgpu] = hybrid_gpus();
    let nvidia = Gpu {
        name: "NVIDIA GeForce RTX 3050".into(),
        vendor: 0x10de,
        dedicated_memory: 4 << 30,
        device: 0,
        luid: (1, 0),
        pnp_id: None,
        preference: None,
    };
    let dgpu_choice = (dgpu.name.clone(), dgpu.pnp_id.clone().unwrap());
    let gpus = [nvidia, igpu, dgpu];
    // Not the first Radeon: the one with the most video memory.
    assert_eq!(pinned_choice(&gpus, None, "", ""), dgpu_choice);
    assert_eq!(pinned_choice(&gpus, Some(0x8086), "", ""), dgpu_choice);
    // The monitor is on a Radeon or the NVIDIA GPU already: capture stays on
    // that one, and its own encoder encodes.
    for vendor in [0x1002, 0x10de] {
        assert_eq!(
            pinned_choice(&gpus, Some(vendor), "", ""),
            (String::new(), String::new())
        );
    }
}
#[test]
fn with_no_display_the_stream_gpu_is_the_pinned_or_only_encoding_gpu_or_refused() {
    let other = |name: &str, vendor| Gpu {
        name: name.into(),
        vendor,
        dedicated_memory: 4 << 30,
        device: 0,
        luid: (vendor, 0),
        pnp_id: None,
        preference: None,
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
    // The only NVIDIA GPU, alone or beside an Intel one.
    assert_eq!(
        headless_gpu(&[nvidia()]).unwrap().name,
        "NVIDIA GeForce RTX 3050"
    );
    assert_eq!(
        headless_gpu(&[intel(), nvidia()]).unwrap().name,
        "NVIDIA GeForce RTX 3050"
    );
    // Several Radeons and none chosen: the card, not the iGPU listed first.
    assert_eq!(
        headless_gpu(&hybrid_gpus()).unwrap().name,
        "AMD Radeon RX 6850M XT"
    );
    // None at all: refused, not guessed.
    for gpus in [vec![intel()], vec![]] {
        let error = format!(
            "{:#}",
            headless_gpu(&gpus).map(|gpu| &gpu.name).unwrap_err()
        );
        assert!(error.contains("no AMD or NVIDIA GPU"), "{error}");
    }
}
#[test]
fn the_probe_skips_an_intel_display_only_when_no_gpu_is_chosen_and_an_encoding_one_exists() {
    let (nvidia, amd, intel) = (0x10de, 0x1002, 0x8086);
    // The monitor on Intel, an AMD or NVIDIA GPU beside it, Graphics card Automatic.
    assert!(probe_skips_display(intel, false, &[amd, intel]));
    assert!(probe_skips_display(intel, false, &[intel, nvidia]));
    // A chosen GPU is probed as chosen; an AMD or NVIDIA monitor stays as it is.
    assert!(!probe_skips_display(intel, true, &[intel, amd]));
    assert!(!probe_skips_display(amd, false, &[nvidia, amd]));
    assert!(!probe_skips_display(nvidia, false, &[nvidia, amd]));
    // Nothing to move to: the encoder says why on the display's GPU.
    assert!(!probe_skips_display(intel, false, &[intel]));
    assert!(!probe_skips_display(intel, false, &[]));
}

#[test]
fn wgc_device_removal_reaches_recovery_without_a_backend_fallback() {
    let loss = crate::device_loss::DeviceLost(DXGI_ERROR_DEVICE_REMOVED.0);
    let error = open_stream_capture::<()>("wgc", &RecordedWarnings::default(), |kind| {
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
fn dxgi_pixel_format_mapping_recognizes_hdr_and_sdr_surfaces() {
    assert_eq!(Pixel::try_from_dxgi_raw(87), Some(Pixel::Bgra8));
    assert_eq!(Pixel::try_from_dxgi_raw(10), Some(Pixel::RgbaF16));
    assert_eq!(Pixel::try_from_dxgi_raw(24), Some(Pixel::Rgba10Pq));
    assert_eq!(Pixel::try_from_dxgi_raw(0), None);
}
#[test]
fn windows_ranking_puts_a_laptops_card_before_an_igpu_with_more_memory() {
    // A Radeon 780M laptop whose firmware gives the iGPU 16 GB beside an 8 GB
    // RX 7700S: by memory alone the iGPU would win.
    let [mut igpu, mut card] = hybrid_gpus();
    igpu.dedicated_memory = 16 << 30;
    card.dedicated_memory = 8 << 30;
    // Windows lists the card first for performance and last to save power.
    let orders = orders_by_luid(&[card.luid, igpu.luid], &[igpu.luid, card.luid]);
    assert_eq!(orders, [(card.luid, (0, 1)), (igpu.luid, (1, 0))]);
    igpu.preference = Some((1, 0));
    card.preference = Some((0, 1));
    let gpus = [igpu, card];
    assert_eq!(
        pinned_choice(&gpus, None, "", "").0,
        "AMD Radeon RX 6850M XT"
    );
    assert_eq!(headless_gpu(&gpus).unwrap().name, "AMD Radeon RX 6850M XT");
    // The probe ranks DXGI's descriptions the same way.
    let desc = |luid: (u32, i32), dedicated: u64| DXGI_ADAPTER_DESC1 {
        VendorId: 0x1002,
        DedicatedVideoMemory: dedicated as usize,
        AdapterLuid: windows::Win32::Foundation::LUID {
            LowPart: luid.0,
            HighPart: luid.1,
        },
        ..Default::default()
    };
    let descs = [desc(gpus[0].luid, 16 << 30), desc(gpus[1].luid, 8 << 30)];
    assert_eq!(headless_candidate(&descs, &orders), 1);
    assert_eq!(headless_candidate(&descs, &[]), 0);
    // An explicit choice always wins, whatever Windows ranks first.
    assert_eq!(
        pinned_choice(&gpus, None, "AMD Radeon(TM) 680M", ""),
        ("AMD Radeon(TM) 680M".to_string(), String::new())
    );
    let identity = gpus[0].pnp_id.clone().unwrap();
    assert_eq!(
        pinned_choice(&gpus, None, "", &identity),
        (String::new(), identity.clone())
    );
    assert_eq!(
        select_gpu(&gpus, "", &identity).unwrap().unwrap().name,
        "AMD Radeon(TM) 680M"
    );
}
