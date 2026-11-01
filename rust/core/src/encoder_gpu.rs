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

/// The adapter a stream pins to when no GPU is chosen, by index among
/// `adapters` (PCI vendor, dedicated video memory): only on a PC that mixes
/// vendors, where Windows would otherwise render and capture on the wrong
/// one. Not when the display's own GPU (`display_vendor`) is already AMD or
/// NVIDIA, which capture then uses as it is with that vendor's encoder. Among
/// several AMD and NVIDIA adapters it is the one with the most video memory,
/// the first on a tie. None leaves the choice alone, as on a PC whose adapters
/// are all one encoding vendor's (or none).
pub fn pinned(adapters: &[(u32, u64)], display_vendor: Option<u32>) -> Option<usize> {
    let one_vendor =
        adapters.iter().all(|(v, _)| is_amd(*v)) || adapters.iter().all(|(v, _)| is_nvidia(*v));
    if display_vendor.is_some_and(encodes) || one_vendor {
        return None;
    }
    let mut best: Option<usize> = None;
    for (index, (vendor, memory)) in adapters.iter().enumerate() {
        if encodes(*vendor) && best.is_none_or(|b| *memory > adapters[b].1) {
            best = Some(index);
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    const GB: u64 = 1 << 30;
    const INTEL: u32 = 0x8086;

    #[test]
    fn a_mixed_pc_pins_the_stream_to_an_encoding_adapter() {
        let (nvidia, amd) = (NVIDIA_VENDOR, AMD_VENDOR);
        assert_eq!(pinned(&[(INTEL, 0), (amd, GB)], None), Some(1));
        assert_eq!(pinned(&[(amd, GB), (INTEL, 0)], None), Some(0));
        assert_eq!(pinned(&[(INTEL, 0), (nvidia, 8 * GB)], None), Some(1));
        // Several Radeons: the one with the most video memory, not the first.
        assert_eq!(
            pinned(
                &[(INTEL, 0), (amd, GB), (amd, 12 * GB), (amd, 12 * GB)],
                None
            ),
            Some(2)
        );
        // An AMD iGPU beside an NVIDIA card: the one with the most memory.
        assert_eq!(pinned(&[(nvidia, 8 * GB), (amd, GB)], None), Some(0));
        assert_eq!(pinned(&[(amd, 16 * GB), (nvidia, 8 * GB)], None), Some(0));
        // A display already on an AMD or NVIDIA GPU is left where it is.
        assert_eq!(
            pinned(&[(nvidia, 8 * GB), (amd, GB), (amd, 12 * GB)], Some(amd)),
            None
        );
        assert_eq!(pinned(&[(nvidia, 8 * GB), (amd, GB)], Some(nvidia)), None);
        // A display on an Intel GPU moves to the encoding one.
        assert_eq!(pinned(&[(INTEL, 0), (amd, GB)], Some(INTEL)), Some(1));
        assert_eq!(pinned(&[(INTEL, 0), (nvidia, GB)], Some(INTEL)), Some(1));
        // Nothing to pin on a one-vendor PC, or one with no encoding adapter.
        assert_eq!(pinned(&[(amd, GB), (amd, 12 * GB)], None), None);
        assert_eq!(pinned(&[(nvidia, GB), (nvidia, 12 * GB)], None), None);
        assert_eq!(pinned(&[(INTEL, GB)], None), None);
        assert_eq!(pinned(&[], None), None);
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
