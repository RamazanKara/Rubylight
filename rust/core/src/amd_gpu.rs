//! Rubylight 3.0 streams only with an AMD GPU and its AMF video encoder. This
//! is the one place that decides whether a machine qualifies and words the
//! refusal; setup and the host both use it. There is no generation floor and
//! no Windows build floor: any AMD GPU with a working AMF encoder qualifies.
//!
//! The file uses only `std`, so setup includes it by path.

/// The PCI vendor ID of AMD (ATI) display adapters.
pub const AMD_VENDOR: u32 = 0x1002;
/// Where NVIDIA and Intel users stay.
pub const LEGACY_RELEASE: &str = "https://github.com/RamazanKara/Rubylight/releases/tag/v2.6.0";
/// What a Moonlight client sees when a launch finds no encoder: the host
/// console has the specific reason.
pub const NO_ENCODER_LAUNCH: &str = "No video encoder is ready. Rubylight 3.0 streams only with an AMD Radeon GPU and its AMF video encoder. Check the host console. If the GPU is AMD, update the Radeon driver and retry. For NVIDIA or Intel graphics, stay on Rubylight 2.6.x: https://github.com/RamazanKara/Rubylight/releases/tag/v2.6.0";

pub fn is_amd(vendor: u32) -> bool {
    vendor == AMD_VENDOR
}

/// A graphics adapter that has a driver, by name and PCI vendor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Adapter {
    pub name: String,
    pub vendor: u32,
}

/// Why a machine, or the adapter a stream would use, does not qualify.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// No graphics adapter with a driver at all.
    NoGpu,
    /// Adapters exist, none of them AMD.
    NoAmd { found: Vec<String> },
    /// An AMD adapter exists, but its driver has no AMF runtime.
    NoAmf { amd: String },
    /// The adapter in use is not AMD while an AMD one exists.
    NotAmdAdapter { adapter: String, amd: String },
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoGpu => write!(
                f,
                "Rubylight 3.0 needs an AMD Radeon graphics card for its AMF video encoder, and Windows lists no graphics card with a working driver on this PC. If this PC has a Radeon card, install the AMD Radeon graphics driver from amd.com/support, then try again. For NVIDIA or Intel graphics, stay on Rubylight 2.6.x: {LEGACY_RELEASE}"
            ),
            Self::NoAmd { found } => write!(
                f,
                "Rubylight 3.0 needs an AMD Radeon graphics card for its AMF video encoder, and this PC has only {}. NVIDIA and Intel graphics stay on Rubylight 2.6.x, which supports them: {LEGACY_RELEASE}",
                found.join(", ")
            ),
            Self::NoAmf { amd } => write!(
                f,
                "This PC has an AMD graphics card ({amd}), but its driver has no AMF video encoder (amfrt64.dll is missing from Windows\\System32), and Rubylight 3.0 cannot stream without it. Install the latest AMD Radeon graphics driver from amd.com/support, restart Windows if the driver asks, then try again."
            ),
            Self::NotAmdAdapter { adapter, amd } => write!(
                f,
                "{adapter} is not an AMD graphics card, and Rubylight 3.0 encodes only on AMD. This PC also has {amd}: connect the monitor to it, or choose it under Graphics card in the console's settings, then reconnect."
            ),
        }
    }
}
impl std::error::Error for Refusal {}

/// Whether the machine has an AMD adapter. `amf_runtime` is whether the
/// driver's AMF runtime is installed.
pub fn check(adapters: &[Adapter], amf_runtime: bool) -> Result<(), Refusal> {
    let Some(amd) = adapters.iter().find(|a| is_amd(a.vendor)) else {
        return Err(if adapters.is_empty() {
            Refusal::NoGpu
        } else {
            Refusal::NoAmd {
                found: adapters.iter().map(|a| a.name.clone()).collect(),
            }
        });
    };
    if !amf_runtime {
        return Err(Refusal::NoAmf {
            amd: amd.name.clone(),
        });
    }
    Ok(())
}

/// Whether a stream can encode on `adapter`, given every adapter on the PC.
/// A non-AMD adapter fails with the machine-wide reason when no AMD adapter
/// exists, or names the AMD one to use instead.
pub fn check_adapter(adapter: &Adapter, all: &[Adapter]) -> Result<(), Refusal> {
    if is_amd(adapter.vendor) {
        return Ok(());
    }
    match all.iter().find(|a| is_amd(a.vendor)) {
        Some(amd) => Err(Refusal::NotAmdAdapter {
            adapter: adapter.name.clone(),
            amd: amd.name.clone(),
        }),
        None => {
            // The adapter in use counts even when the list missed it.
            let mut found: Vec<String> = all.iter().map(|a| a.name.clone()).collect();
            if !found.contains(&adapter.name) {
                found.push(adapter.name.clone());
            }
            Err(Refusal::NoAmd { found })
        }
    }
}

/// The index of the AMD adapter among `vendors`, else the first adapter: with
/// an AMD iGPU beside an NVIDIA dGPU, the AMD one is the encoder.
pub fn preferred(vendors: &[u32]) -> usize {
    vendors.iter().position(|v| is_amd(*v)).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn adapter(name: &str, vendor: u32) -> Adapter {
        Adapter {
            name: name.into(),
            vendor,
        }
    }
    fn nvidia() -> Adapter {
        adapter("NVIDIA GeForce RTX 4070", 0x10de)
    }
    fn intel() -> Adapter {
        adapter("Intel(R) UHD Graphics 770", 0x8086)
    }
    fn radeon() -> Adapter {
        adapter("AMD Radeon RX 7900 XT", AMD_VENDOR)
    }

    #[test]
    fn a_pc_without_a_graphics_adapter_is_refused_with_the_driver_hint() {
        let refusal = check(&[], true).unwrap_err();
        assert_eq!(refusal, Refusal::NoGpu);
        let text = refusal.to_string();
        assert!(
            text.contains("no graphics card with a working driver"),
            "{text}"
        );
        assert!(
            text.contains("install the AMD Radeon graphics driver"),
            "{text}"
        );
        assert!(text.contains("stay on Rubylight 2.6.x"), "{text}");
        assert!(text.contains(LEGACY_RELEASE), "{text}");
    }

    #[test]
    fn nvidia_and_intel_are_sent_to_2_6_with_the_link() {
        for (others, named) in [
            (vec![nvidia()], "only NVIDIA GeForce RTX 4070."),
            (vec![intel()], "only Intel(R) UHD Graphics 770."),
            (
                vec![intel(), nvidia()],
                "only Intel(R) UHD Graphics 770, NVIDIA GeForce RTX 4070.",
            ),
        ] {
            let text = check(&others, true).unwrap_err().to_string();
            assert!(text.contains(named), "{text}");
            assert!(text.contains("stay on Rubylight 2.6.x"), "{text}");
            assert!(
                text.ends_with("https://github.com/RamazanKara/Rubylight/releases/tag/v2.6.0"),
                "{text}"
            );
        }
    }

    #[test]
    fn an_amd_adapter_beside_another_one_qualifies() {
        for adapters in [
            vec![radeon()],
            vec![nvidia(), radeon()],
            vec![radeon(), intel()],
        ] {
            assert_eq!(check(&adapters, true), Ok(()));
        }
    }

    #[test]
    fn every_amd_generation_qualifies() {
        // There is no generation floor: the check reads only the vendor.
        for name in [
            "AMD Radeon HD 7970",
            "AMD Radeon RX 580 Series",
            "AMD Radeon RX 6600",
            "AMD Radeon(TM) Graphics",
            "AMD Radeon RX 9070 XT",
        ] {
            assert_eq!(check(&[adapter(name, AMD_VENDOR)], true), Ok(()), "{name}");
        }
    }

    #[test]
    fn an_amd_adapter_without_the_amf_runtime_is_told_to_update_the_driver() {
        let refusal = check(&[nvidia(), radeon()], false).unwrap_err();
        assert_eq!(
            refusal,
            Refusal::NoAmf {
                amd: "AMD Radeon RX 7900 XT".into()
            }
        );
        let text = refusal.to_string();
        assert!(text.contains("AMD Radeon RX 7900 XT"), "{text}");
        assert!(text.contains("amfrt64.dll is missing"), "{text}");
        assert!(
            text.contains("Install the latest AMD Radeon graphics driver"),
            "{text}"
        );
        // This is not an NVIDIA or Intel problem: no 2.6.x advice.
        assert!(!text.contains("2.6"), "{text}");
    }

    #[test]
    fn a_non_amd_adapter_in_use_names_the_amd_one_to_use_instead() {
        let all = [nvidia(), radeon()];
        assert_eq!(check_adapter(&all[1], &all), Ok(()));
        let text = check_adapter(&all[0], &all).unwrap_err().to_string();
        assert!(
            text.starts_with("NVIDIA GeForce RTX 4070 is not an AMD graphics card"),
            "{text}"
        );
        assert!(text.contains("also has AMD Radeon RX 7900 XT"), "{text}");
        assert!(text.contains("Graphics card"), "{text}");
    }

    #[test]
    fn a_non_amd_adapter_alone_is_the_machine_wide_refusal() {
        let refusal = check_adapter(&nvidia(), &[nvidia()]).unwrap_err();
        assert_eq!(refusal, check(&[nvidia()], true).unwrap_err());
        // A list that missed the adapter in use still names it.
        let text = check_adapter(&intel(), &[]).unwrap_err().to_string();
        assert!(text.contains("only Intel(R) UHD Graphics 770."), "{text}");
        assert!(text.contains(LEGACY_RELEASE), "{text}");
    }

    #[test]
    fn the_amd_igpu_is_picked_beside_an_nvidia_dgpu() {
        assert_eq!(preferred(&[0x10de, AMD_VENDOR]), 1);
        assert_eq!(preferred(&[AMD_VENDOR, 0x10de]), 0);
        // Nothing AMD: the first adapter, which the encoder then refuses.
        assert_eq!(preferred(&[0x10de, 0x8086]), 0);
        assert_eq!(preferred(&[]), 0);
    }

    #[test]
    fn no_text_states_a_generation_floor() {
        let texts = [
            Refusal::NoGpu.to_string(),
            check(&[nvidia()], true).unwrap_err().to_string(),
            check(&[radeon()], false).unwrap_err().to_string(),
            check_adapter(&nvidia(), &[nvidia(), radeon()])
                .unwrap_err()
                .to_string(),
            NO_ENCODER_LAUNCH.to_string(),
        ];
        for text in texts {
            assert!(!text.contains("RX 7000"), "{text}");
            assert!(!text.contains("or newer"), "{text}");
        }
        assert!(NO_ENCODER_LAUNCH.starts_with("No video encoder is ready."));
        assert!(NO_ENCODER_LAUNCH.ends_with(LEGACY_RELEASE));
    }
}
