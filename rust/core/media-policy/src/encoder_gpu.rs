//! Rubylight streams with an AMD GPU and its AMF video encoder, or an NVIDIA
//! GPU and its NVENC encoder. This is the one place that decides whether a
//! machine qualifies and words the refusal; setup and the host both use it.
//! There is no generation floor and no Windows build floor: any AMD GPU with a
//! working AMF encoder and any NVIDIA GPU with a working NVENC encoder
//! qualify. Intel graphics stay on Rubylight 2.6.x.
//!
//! The file uses only `std`, so setup includes it by path.

/// The PCI vendor ID of AMD (ATI) display adapters.
pub const AMD_VENDOR: u32 = 0x1002;
/// The PCI vendor ID of NVIDIA display adapters.
pub const NVIDIA_VENDOR: u32 = 0x10de;
/// Where Intel users stay.
pub const LEGACY_RELEASE: &str = "https://github.com/RamazanKara/Rubylight/releases/tag/v2.6.0";
/// What a Moonlight client sees when a launch finds no encoder: the host
/// console has the specific reason.
pub const NO_ENCODER_LAUNCH: &str = "No video encoder is ready. Rubylight streams with an AMD Radeon GPU and its AMF video encoder or an NVIDIA GPU and its NVENC video encoder. Check the host console, update the graphics driver and retry. For Intel graphics, stay on Rubylight 2.6.x: https://github.com/RamazanKara/Rubylight/releases/tag/v2.6.0";

pub fn is_amd(vendor: u32) -> bool {
    vendor == AMD_VENDOR
}
pub fn is_nvidia(vendor: u32) -> bool {
    vendor == NVIDIA_VENDOR
}
/// Whether Rubylight has a native encoder for this vendor's GPUs.
pub fn encodes(vendor: u32) -> bool {
    is_amd(vendor) || is_nvidia(vendor)
}

/// A graphics adapter that has a driver, by name and PCI vendor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Adapter {
    pub name: String,
    pub vendor: u32,
}

/// Which vendor encoder runtimes the installed drivers put in System32:
/// AMD's `amfrt64.dll` and NVIDIA's `nvEncodeAPI64.dll`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Runtimes {
    pub amf: bool,
    pub nvenc: bool,
}
impl Runtimes {
    /// Both, for a check that leaves the runtimes to the encoder probe.
    pub const ALL: Self = Self {
        amf: true,
        nvenc: true,
    };
    fn has(self, vendor: u32) -> bool {
        (is_amd(vendor) && self.amf) || (is_nvidia(vendor) && self.nvenc)
    }
}

/// Why a machine, or the adapter a stream would use, does not qualify.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// No graphics adapter with a driver at all.
    NoGpu,
    /// Adapters exist, none of them AMD or NVIDIA.
    NoEncoderGpu { found: Vec<String> },
    /// An AMD or NVIDIA adapter exists, but no driver has its encoder runtime.
    NoRuntime { gpu: String, vendor: u32 },
    /// The adapter in use is neither AMD nor NVIDIA while such an adapter exists.
    UnsupportedAdapter { adapter: String, supported: String },
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoGpu => write!(
                f,
                "Rubylight needs an AMD Radeon or NVIDIA graphics card for its video encoder, and Windows lists no graphics card with a working driver on this PC. If this PC has one, install its graphics driver from amd.com/support or nvidia.com/drivers, then try again. For Intel graphics, stay on Rubylight 2.6.x: {LEGACY_RELEASE}"
            ),
            Self::NoEncoderGpu { found } => write!(
                f,
                "Rubylight needs an AMD Radeon or NVIDIA graphics card for its video encoder, and this PC has only {}. Intel graphics stay on Rubylight 2.6.x, which supports them: {LEGACY_RELEASE}",
                found.join(", ")
            ),
            Self::NoRuntime { gpu, vendor } if is_nvidia(*vendor) => write!(
                f,
                "This PC has an NVIDIA graphics card ({gpu}), but its driver has no NVENC video encoder (nvEncodeAPI64.dll is missing from Windows\\System32), and Rubylight cannot stream without it. Install the latest NVIDIA graphics driver from nvidia.com/drivers, restart Windows if the driver asks, then try again."
            ),
            Self::NoRuntime { gpu, .. } => write!(
                f,
                "This PC has an AMD graphics card ({gpu}), but its driver has no AMF video encoder (amfrt64.dll is missing from Windows\\System32), and Rubylight cannot stream without it. Install the latest AMD Radeon graphics driver from amd.com/support, restart Windows if the driver asks, then try again."
            ),
            Self::UnsupportedAdapter { adapter, supported } => write!(
                f,
                "{adapter} is neither an AMD nor an NVIDIA graphics card, and Rubylight encodes only on those. This PC also has {supported}: connect the monitor to it, choose it under Graphics card in the console's settings, or use WGC capture, then reconnect."
            ),
        }
    }
}
impl std::error::Error for Refusal {}

/// Whether the machine has an AMD or NVIDIA adapter whose driver installed
/// its encoder runtime. An AMD adapter is reported before an NVIDIA one when
/// neither runtime is there.
pub fn check(adapters: &[Adapter], runtimes: Runtimes) -> Result<(), Refusal> {
    let mut supported: Vec<&Adapter> = adapters.iter().filter(|a| encodes(a.vendor)).collect();
    if supported.is_empty() {
        return Err(if adapters.is_empty() {
            Refusal::NoGpu
        } else {
            Refusal::NoEncoderGpu {
                found: adapters.iter().map(|a| a.name.clone()).collect(),
            }
        });
    }
    if supported.iter().any(|a| runtimes.has(a.vendor)) {
        return Ok(());
    }
    supported.sort_by_key(|a| !is_amd(a.vendor));
    Err(Refusal::NoRuntime {
        gpu: supported[0].name.clone(),
        vendor: supported[0].vendor,
    })
}

/// Whether a stream can encode on `adapter`, given every adapter on the PC.
/// An adapter Rubylight cannot encode on fails with the machine-wide reason
/// when no AMD or NVIDIA adapter exists, or names the one to use instead.
pub fn check_adapter(adapter: &Adapter, all: &[Adapter]) -> Result<(), Refusal> {
    if encodes(adapter.vendor) {
        return Ok(());
    }
    match all.iter().find(|a| encodes(a.vendor)) {
        Some(supported) => Err(Refusal::UnsupportedAdapter {
            adapter: adapter.name.clone(),
            supported: supported.name.clone(),
        }),
        None => {
            // The adapter in use counts even when the list missed it.
            let mut found: Vec<String> = all.iter().map(|a| a.name.clone()).collect();
            if !found.contains(&adapter.name) {
                found.push(adapter.name.clone());
            }
            Err(Refusal::NoEncoderGpu { found })
        }
    }
}

/// The index of the adapter that encodes among `vendors`: AMD first, then
/// NVIDIA, else the first adapter.
pub fn preferred(vendors: &[u32]) -> usize {
    vendors
        .iter()
        .position(|v| is_amd(*v))
        .or_else(|| vendors.iter().position(|v| is_nvidia(*v)))
        .unwrap_or(0)
}

/// A hardware adapter as the automatic GPU choice ranks it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Ranked {
    /// The PCI vendor ID.
    pub vendor: u32,
    /// Dedicated video memory in bytes (an iGPU's BIOS carve-out, a card's VRAM).
    pub dedicated_memory: u64,
    /// Where Windows lists the adapter when asked for the highest-performance
    /// GPU and when asked for the lowest-power one
    /// (`IDXGIFactory6::EnumAdapterByGpuPreference`). Microsoft documents the
    /// first order as external GPUs, then discrete, then integrated, and the
    /// second as its reverse. None where Windows gives no such order.
    pub preference: Option<(u32, u32)>,
}
impl Ranked {
    /// An adapter without a Windows preference order, ranked by memory alone.
    pub fn new(vendor: u32, dedicated_memory: u64) -> Self {
        Self {
            vendor,
            dedicated_memory,
            preference: None,
        }
    }
}

/// Whether Windows puts `a` in a lower power class than `b`: after it when
/// asked for performance and before it when asked to save power, as an
/// integrated GPU against a discrete one or a discrete one against an
/// external one. Adapters Windows does not classify keep one order in both
/// lists and are never ranked against each other this way.
fn lower_class(a: &Ranked, b: &Ranked) -> bool {
    match (a.preference, b.preference) {
        (Some((a_fast, a_frugal)), Some((b_fast, b_frugal))) => {
            a_fast > b_fast && a_frugal < b_frugal
        }
        _ => false,
    }
}

/// The adapter a stream pins to when no GPU is chosen, by index among the
/// PC's hardware `adapters`. Not when the display's own GPU (`display_vendor`)
/// is already AMD or NVIDIA, which capture then uses as it is with that
/// vendor's encoder, and not on a PC with a single adapter, which Windows
/// picks anyway. Otherwise it is the AMD or NVIDIA adapter that is discrete
/// rather than integrated (or external rather than discrete) as Windows ranks
/// them, and among those the one with the most video memory, the first on a
/// tie: on a PC that mixes vendors, where Windows would otherwise render and
/// capture on the wrong one, and on one with an iGPU beside a card of the same
/// vendor, where it would render a virtual display and probe on the iGPU.
/// None leaves the choice alone.
pub fn pinned(adapters: &[Ranked], display_vendor: Option<u32>) -> Option<usize> {
    if display_vendor.is_some_and(encodes) || adapters.len() < 2 {
        return None;
    }
    let encoding: Vec<usize> = (0..adapters.len())
        .filter(|&index| encodes(adapters[index].vendor))
        .collect();
    let mut best: Option<usize> = None;
    for &index in &encoding {
        let candidate = &adapters[index];
        let outranked = encoding
            .iter()
            .any(|&other| lower_class(candidate, &adapters[other]));
        if !outranked
            && best.is_none_or(|b| candidate.dedicated_memory > adapters[b].dedicated_memory)
        {
            best = Some(index);
        }
    }
    best
}

/// The adapter that probes the encoders when no display gives one, by index
/// among `adapters`: the one a stream with no GPU chosen pins to ([`pinned`]
/// with no display), so the codecs the host reports in `/serverinfo` are those
/// of the GPU that encodes. Otherwise, as on a PC with a single adapter, AMD
/// first, then NVIDIA ([`preferred`]).
pub fn headless(adapters: &[Ranked]) -> usize {
    pinned(adapters, None).unwrap_or_else(|| {
        let vendors: Vec<u32> = adapters.iter().map(|adapter| adapter.vendor).collect();
        preferred(&vendors)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const GB: u64 = 1 << 30;
    const INTEL: u32 = 0x8086;

    /// Adapters ranked by memory alone, as `(vendor, dedicated memory)`.
    fn plain(adapters: &[(u32, u64)]) -> Vec<Ranked> {
        adapters
            .iter()
            .map(|&(vendor, memory)| Ranked::new(vendor, memory))
            .collect()
    }
    /// Adapters with Windows' preference orders, as `(vendor, dedicated
    /// memory, high-performance position, minimum-power position)`.
    fn ordered(adapters: &[(u32, u64, u32, u32)]) -> Vec<Ranked> {
        adapters
            .iter()
            .map(|&(vendor, dedicated_memory, fast, frugal)| Ranked {
                vendor,
                dedicated_memory,
                preference: Some((fast, frugal)),
            })
            .collect()
    }
    fn pin(adapters: &[(u32, u64)], display: Option<u32>) -> Option<usize> {
        pinned(&plain(adapters), display)
    }

    #[test]
    fn a_mixed_pc_pins_the_stream_to_an_encoding_adapter() {
        let (nvidia, amd) = (NVIDIA_VENDOR, AMD_VENDOR);
        assert_eq!(pin(&[(INTEL, 0), (amd, GB)], None), Some(1));
        assert_eq!(pin(&[(amd, GB), (INTEL, 0)], None), Some(0));
        assert_eq!(pin(&[(INTEL, 0), (nvidia, 8 * GB)], None), Some(1));
        // Several Radeons: the one with the most video memory, not the first.
        assert_eq!(
            pin(
                &[(INTEL, 0), (amd, GB), (amd, 12 * GB), (amd, 12 * GB)],
                None
            ),
            Some(2)
        );
        // An AMD iGPU beside an NVIDIA card: the one with the most memory.
        assert_eq!(pin(&[(nvidia, 8 * GB), (amd, GB)], None), Some(0));
        assert_eq!(pin(&[(amd, 16 * GB), (nvidia, 8 * GB)], None), Some(0));
        // A display already on an AMD or NVIDIA GPU is left where it is.
        assert_eq!(
            pin(&[(nvidia, 8 * GB), (amd, GB), (amd, 12 * GB)], Some(amd)),
            None
        );
        assert_eq!(pin(&[(nvidia, 8 * GB), (amd, GB)], Some(nvidia)), None);
        // A display on an Intel GPU moves to the encoding one.
        assert_eq!(pin(&[(INTEL, 0), (amd, GB)], Some(INTEL)), Some(1));
        assert_eq!(pin(&[(INTEL, 0), (nvidia, GB)], Some(INTEL)), Some(1));
        // Nothing to pin with no encoding adapter.
        assert_eq!(pin(&[(INTEL, GB)], None), None);
        assert_eq!(pin(&[(INTEL, GB), (INTEL, 0)], None), None);
        assert_eq!(pin(&[], None), None);
    }

    #[test]
    fn a_lone_gpu_is_left_to_windows() {
        // The default single-GPU path: nothing pinned, whatever its memory.
        for vendor in [AMD_VENDOR, NVIDIA_VENDOR, INTEL] {
            assert_eq!(pin(&[(vendor, 16 * GB)], None), None);
            assert_eq!(pinned(&ordered(&[(vendor, GB, 0, 0)]), None), None);
            assert_eq!(headless(&plain(&[(vendor, GB)])), 0);
        }
    }

    #[test]
    fn an_igpu_beside_a_card_of_the_same_vendor_pins_the_card() {
        let (nvidia, amd) = (NVIDIA_VENDOR, AMD_VENDOR);
        // A Ryzen desktop's 512 MB iGPU listed before an RX 7900 XT: before,
        // one vendor pinned nothing and the iGPU rendered and probed.
        let desktop = [(amd, 512 << 20), (amd, 20 * GB)];
        assert_eq!(pin(&desktop, None), Some(1));
        assert_eq!(pin(&[(amd, 20 * GB), (amd, 512 << 20)], None), Some(0));
        // A Radeon 780M laptop with an RX 7700S, whose firmware gives the
        // iGPU as much memory as the card: Windows' order decides.
        let laptop = ordered(&[(amd, 8 * GB, 1, 0), (amd, 8 * GB, 0, 1)]);
        assert_eq!(pinned(&laptop, None), Some(1));
        // Even when the iGPU has more memory than the card.
        let laptop = ordered(&[(amd, 16 * GB, 1, 0), (amd, 8 * GB, 0, 1)]);
        assert_eq!(pinned(&laptop, None), Some(1));
        // The NVIDIA shapes: two NVIDIA adapters Windows ranks, and an AMD
        // iGPU beside an NVIDIA card (MUXless).
        let nvidia_pair = ordered(&[(nvidia, GB, 1, 0), (nvidia, 8 * GB, 0, 1)]);
        assert_eq!(pinned(&nvidia_pair, None), Some(1));
        let muxless = ordered(&[(amd, 16 * GB, 1, 0), (nvidia, 8 * GB, 0, 1)]);
        assert_eq!(pinned(&muxless, None), Some(1));
        assert_eq!(pin(&[(nvidia, GB), (nvidia, 8 * GB)], None), Some(1));
        // A display already on either GPU stays there.
        assert_eq!(pin(&desktop, Some(amd)), None);
        assert_eq!(pinned(&muxless, Some(amd)), None);
    }

    #[test]
    fn two_cards_pin_the_one_with_more_memory_unless_windows_ranks_them() {
        let (nvidia, amd) = (NVIDIA_VENDOR, AMD_VENDOR);
        // Two discrete cards Windows lists in one order for both requests.
        let pair = ordered(&[(amd, 4 * GB, 0, 0), (amd, 20 * GB, 1, 1)]);
        assert_eq!(pinned(&pair, None), Some(1));
        assert_eq!(pin(&[(nvidia, 12 * GB), (nvidia, 24 * GB)], None), Some(1));
        // Two identical cards: the first.
        assert_eq!(pin(&[(amd, 20 * GB), (amd, 20 * GB)], None), Some(0));
        // An external GPU outranks the internal card it was plugged in for.
        let egpu = ordered(&[
            (amd, 512 << 20, 2, 0),
            (amd, 8 * GB, 1, 1),
            (nvidia, 8 * GB, 0, 2),
        ]);
        assert_eq!(pinned(&egpu, None), Some(2));
        // An Intel adapter's rank never counts: it cannot encode.
        let intel_first = ordered(&[(INTEL, 0, 0, 1), (amd, 512 << 20, 1, 0)]);
        assert_eq!(pinned(&intel_first, None), Some(1));
        // Part of the list without an order: memory alone among those.
        let mut mixed = ordered(&[(amd, 512 << 20, 1, 0), (amd, 8 * GB, 0, 1)]);
        mixed[1].preference = None;
        assert_eq!(pinned(&mixed, None), Some(1));
    }

    fn adapter(name: &str, vendor: u32) -> Adapter {
        Adapter {
            name: name.into(),
            vendor,
        }
    }
    fn nvidia() -> Adapter {
        adapter("NVIDIA GeForce RTX 4070", NVIDIA_VENDOR)
    }
    fn intel() -> Adapter {
        adapter("Intel(R) UHD Graphics 770", INTEL)
    }
    fn radeon() -> Adapter {
        adapter("AMD Radeon RX 7900 XT", AMD_VENDOR)
    }
    const NONE: Runtimes = Runtimes {
        amf: false,
        nvenc: false,
    };

    #[test]
    fn a_pc_without_a_graphics_adapter_is_refused_with_the_driver_hint() {
        let refusal = check(&[], Runtimes::ALL).unwrap_err();
        assert_eq!(refusal, Refusal::NoGpu);
        let text = refusal.to_string();
        assert!(
            text.contains("no graphics card with a working driver"),
            "{text}"
        );
        assert!(
            text.contains("amd.com/support or nvidia.com/drivers"),
            "{text}"
        );
        assert!(
            text.contains("For Intel graphics, stay on Rubylight 2.6.x"),
            "{text}"
        );
        assert!(text.contains(LEGACY_RELEASE), "{text}");
    }

    #[test]
    fn intel_alone_is_sent_to_2_6_with_the_link() {
        for (others, named) in [
            (vec![intel()], "only Intel(R) UHD Graphics 770."),
            (
                vec![intel(), adapter("Intel(R) Arc(TM) A770", INTEL)],
                "only Intel(R) UHD Graphics 770, Intel(R) Arc(TM) A770.",
            ),
        ] {
            let text = check(&others, Runtimes::ALL).unwrap_err().to_string();
            assert!(text.contains(named), "{text}");
            assert!(text.contains("stay on Rubylight 2.6.x"), "{text}");
            assert!(
                text.ends_with("https://github.com/RamazanKara/Rubylight/releases/tag/v2.6.0"),
                "{text}"
            );
        }
    }

    #[test]
    fn an_amd_or_nvidia_adapter_beside_another_one_qualifies() {
        for adapters in [
            vec![radeon()],
            vec![nvidia()],
            vec![nvidia(), radeon()],
            vec![radeon(), intel()],
            vec![intel(), nvidia()],
        ] {
            assert_eq!(check(&adapters, Runtimes::ALL), Ok(()));
        }
    }

    #[test]
    fn every_amd_and_nvidia_generation_qualifies() {
        // There is no generation floor: the check reads only the vendor.
        for name in [
            "AMD Radeon HD 7970",
            "AMD Radeon RX 580 Series",
            "AMD Radeon RX 6600",
            "AMD Radeon(TM) Graphics",
            "AMD Radeon RX 9070 XT",
        ] {
            assert_eq!(
                check(&[adapter(name, AMD_VENDOR)], Runtimes::ALL),
                Ok(()),
                "{name}"
            );
        }
        for name in [
            "NVIDIA GeForce GTX 1050",
            "NVIDIA GeForce RTX 2060",
            "NVIDIA RTX A4000",
            "NVIDIA GeForce RTX 5090",
        ] {
            assert_eq!(
                check(&[adapter(name, NVIDIA_VENDOR)], Runtimes::ALL),
                Ok(()),
                "{name}"
            );
        }
    }

    #[test]
    fn a_missing_runtime_is_told_to_update_that_vendors_driver() {
        let refusal = check(&[nvidia(), radeon()], NONE).unwrap_err();
        assert_eq!(
            refusal,
            Refusal::NoRuntime {
                gpu: "AMD Radeon RX 7900 XT".into(),
                vendor: AMD_VENDOR,
            }
        );
        let text = refusal.to_string();
        assert!(text.contains("AMD Radeon RX 7900 XT"), "{text}");
        assert!(text.contains("amfrt64.dll is missing"), "{text}");
        assert!(
            text.contains("Install the latest AMD Radeon graphics driver"),
            "{text}"
        );
        // This is not an Intel problem: no 2.6.x advice.
        assert!(!text.contains("2.6"), "{text}");
        let text = check(&[intel(), nvidia()], NONE).unwrap_err().to_string();
        assert!(text.contains("NVIDIA GeForce RTX 4070"), "{text}");
        assert!(text.contains("nvEncodeAPI64.dll is missing"), "{text}");
        assert!(text.contains("latest NVIDIA graphics driver"), "{text}");
        assert!(!text.contains("2.6"), "{text}");
    }

    #[test]
    fn one_vendors_runtime_is_enough() {
        let only_nvenc = Runtimes {
            amf: false,
            nvenc: true,
        };
        let only_amf = Runtimes {
            amf: true,
            nvenc: false,
        };
        assert_eq!(check(&[radeon(), nvidia()], only_nvenc), Ok(()));
        assert_eq!(check(&[radeon(), nvidia()], only_amf), Ok(()));
        assert!(check(&[radeon()], only_nvenc).is_err());
        assert!(check(&[nvidia()], only_amf).is_err());
    }

    #[test]
    fn an_intel_adapter_in_use_names_the_encoding_one_to_use_instead() {
        for supported in [radeon(), nvidia()] {
            let all = [intel(), supported.clone()];
            assert_eq!(check_adapter(&all[1], &all), Ok(()));
            let text = check_adapter(&all[0], &all).unwrap_err().to_string();
            assert!(
                text.starts_with(
                    "Intel(R) UHD Graphics 770 is neither an AMD nor an NVIDIA graphics card"
                ),
                "{text}"
            );
            assert!(
                text.contains(&format!("also has {}", supported.name)),
                "{text}"
            );
            assert!(text.contains("Graphics card"), "{text}");
        }
        // NVIDIA beside AMD is no refusal at all.
        assert_eq!(check_adapter(&nvidia(), &[nvidia(), radeon()]), Ok(()));
    }

    #[test]
    fn an_intel_adapter_alone_is_the_machine_wide_refusal() {
        let refusal = check_adapter(&intel(), &[intel()]).unwrap_err();
        assert_eq!(refusal, check(&[intel()], Runtimes::ALL).unwrap_err());
        // A list that missed the adapter in use still names it.
        let text = check_adapter(&intel(), &[]).unwrap_err().to_string();
        assert!(text.contains("only Intel(R) UHD Graphics 770."), "{text}");
        assert!(text.contains(LEGACY_RELEASE), "{text}");
    }

    #[test]
    fn the_probe_without_a_display_takes_the_gpu_a_stream_pins() {
        let (nvidia, amd) = (NVIDIA_VENDOR, AMD_VENDOR);
        // Issue 134: an AMD iGPU listed before an RTX 5090. The stream encodes
        // on the NVIDIA card, so the probe must not take the first AMD one.
        let hybrid = plain(&[(amd, 512 << 20), (nvidia, 32 * GB)]);
        assert_eq!(headless(&hybrid), 1);
        assert_eq!(Some(headless(&hybrid)), pinned(&hybrid, None));
        // A bigger Radeon beside a small NVIDIA card stays the Radeon.
        assert_eq!(headless(&plain(&[(nvidia, 4 * GB), (amd, 16 * GB)])), 1);
        // An Intel GPU first: the encoding one.
        assert_eq!(headless(&plain(&[(INTEL, 0), (nvidia, 8 * GB)])), 1);
        // One vendor's iGPU before its card: the card, whose codecs
        // `/serverinfo` then reports (AV1 on an RX 7900 XT, which a Ryzen
        // desktop's iGPU lacks).
        assert_eq!(headless(&plain(&[(amd, 512 << 20), (amd, 20 * GB)])), 1);
        assert_eq!(headless(&plain(&[(nvidia, GB), (nvidia, 12 * GB)])), 1);
        let laptop = ordered(&[(amd, 16 * GB, 1, 0), (amd, 8 * GB, 0, 1)]);
        assert_eq!(headless(&laptop), 1);
        // Nothing pinned: the only encoding adapter, else the first.
        assert_eq!(headless(&plain(&[(INTEL, GB)])), 0);
        assert_eq!(headless(&[]), 0);
    }

    #[test]
    fn amd_then_nvidia_is_preferred_among_candidates() {
        assert_eq!(preferred(&[NVIDIA_VENDOR, AMD_VENDOR]), 1);
        assert_eq!(preferred(&[AMD_VENDOR, NVIDIA_VENDOR]), 0);
        assert_eq!(preferred(&[INTEL, NVIDIA_VENDOR]), 1);
        // Nothing that encodes: the first adapter, which the encoder then refuses.
        assert_eq!(preferred(&[INTEL, INTEL]), 0);
        assert_eq!(preferred(&[]), 0);
    }

    #[test]
    fn no_text_states_a_generation_floor() {
        let texts = [
            Refusal::NoGpu.to_string(),
            check(&[intel()], Runtimes::ALL).unwrap_err().to_string(),
            check(&[radeon()], NONE).unwrap_err().to_string(),
            check(&[nvidia()], NONE).unwrap_err().to_string(),
            check_adapter(&intel(), &[intel(), radeon()])
                .unwrap_err()
                .to_string(),
            NO_ENCODER_LAUNCH.to_string(),
        ];
        for text in texts {
            assert!(!text.contains("RX 7000"), "{text}");
            assert!(!text.contains("or newer"), "{text}");
        }
        assert!(NO_ENCODER_LAUNCH.starts_with("No video encoder is ready."));
        assert!(NO_ENCODER_LAUNCH.contains("NVENC"));
        assert!(NO_ENCODER_LAUNCH.ends_with(LEGACY_RELEASE));
    }
}
