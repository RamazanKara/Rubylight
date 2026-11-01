//! Rubylight streams only with an AMD GPU and its AMF encoder or an NVIDIA
//! GPU and its NVENC encoder, so setup checks for one of them before it
//! installs or updates anything. The decision and its wording are
//! `butterpollo-core`'s `encoder_gpu`; this reads the PC.
use crate::encoder_gpu::{self, Adapter, Refusal, Runtimes};
use crate::system::{registry_string, subkeys, system32};
use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, KEY_WOW64_64KEY};

/// The Display adapters class, whose subkeys 0000, 0001, ... are the display
/// devices that have a driver, including ones Windows has hidden.
const DISPLAY_CLASS: &str =
    "SYSTEM\\CurrentControlSet\\Control\\Class\\{4d36e968-e325-11ce-bfc1-08002be10318}";

/// The PCI vendor in a device ID such as `PCI\VEN_1002&DEV_744C&SUBSYS_...`.
fn pci_vendor(id: &str) -> Option<u32> {
    let id = id.to_ascii_lowercase();
    let digits = id.split("ven_").nth(1)?.get(..4)?;
    u32::from_str_radix(digits, 16).ok()
}

/// A display device the check counts: a PCI one, which has a vendor. Virtual
/// displays, Remote Desktop's adapter and Microsoft's Basic Display Adapter
/// match no PCI vendor and are not counted.
fn adapter(description: &str, matching_id: &str) -> Option<Adapter> {
    Some(Adapter {
        name: description.trim().to_string(),
        vendor: pci_vendor(matching_id)?,
    })
}

fn installed_adapters() -> Vec<Adapter> {
    let mut found: Vec<Adapter> = Vec::new();
    for key in subkeys(HKEY_LOCAL_MACHINE, DISPLAY_CLASS, KEY_WOW64_64KEY) {
        if key.len() != 4 || !key.bytes().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let path = format!("{DISPLAY_CLASS}\\{key}");
        let Some(id) = registry_string(HKEY_LOCAL_MACHINE, &path, "MatchingDeviceId") else {
            continue;
        };
        let description = registry_string(HKEY_LOCAL_MACHINE, &path, "DriverDesc");
        if let Some(adapter) = adapter(description.as_deref().unwrap_or_default(), &id)
            && !found.contains(&adapter)
        {
            found.push(adapter);
        }
    }
    found
}

/// AMD's driver installs the AMF runtime and NVIDIA's the NVENC runtime in
/// System32, where the host loads them.
fn runtimes() -> Runtimes {
    let present = |name| std::path::Path::new(&system32(name)).is_file();
    Runtimes {
        amf: present("amfrt64.dll"),
        nvenc: present("nvEncodeAPI64.dll"),
    }
}

/// Why this PC cannot run Rubylight, in words for the person at it.
pub fn refusal() -> Option<Refusal> {
    encoder_gpu::check(&installed_adapters(), runtimes()).err()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_vendor_is_read_from_the_pci_device_id_in_any_case() {
        assert_eq!(
            pci_vendor("PCI\\VEN_1002&DEV_744C&SUBSYS_0E391002"),
            Some(0x1002)
        );
        assert_eq!(pci_vendor("pci\\ven_10de&dev_2684"), Some(0x10de));
        assert_eq!(pci_vendor("PCI\\VEN_8086&DEV_A780&REV_04"), Some(0x8086));
        for other in ["", "root\\rdpidd", "PCI\\CC_0300", "PCI\\VEN_10"] {
            assert_eq!(pci_vendor(other), None, "{other}");
        }
    }

    #[test]
    fn virtual_and_basic_adapters_are_not_counted() {
        assert_eq!(
            adapter("Microsoft Remote Display Adapter", "root\\rdpidd"),
            None
        );
        assert_eq!(
            adapter("Microsoft Basic Display Adapter", "PCI\\CC_0300"),
            None
        );
        assert_eq!(
            adapter(" AMD Radeon RX 6800 ", "pci\\ven_1002&dev_73bf"),
            Some(Adapter {
                name: "AMD Radeon RX 6800".into(),
                vendor: 0x1002
            })
        );
    }

    fn pc(devices: &[(&str, &str)], runtimes: Runtimes) -> Option<String> {
        let adapters: Vec<Adapter> = devices
            .iter()
            .filter_map(|(name, id)| adapter(name, id))
            .collect();
        encoder_gpu::check(&adapters, runtimes)
            .err()
            .map(|r| r.to_string())
    }

    const NO_RUNTIME: Runtimes = Runtimes {
        amf: false,
        nvenc: false,
    };

    #[test]
    fn setup_refuses_intel_and_a_pc_with_no_driver_in_words() {
        let intel = pc(
            &[("Intel(R) Arc(TM) A770", "PCI\\VEN_8086&DEV_56A0")],
            Runtimes::ALL,
        )
        .unwrap();
        assert!(intel.contains("only Intel(R) Arc(TM) A770"), "{intel}");
        assert!(intel.contains("stay on Rubylight 2.6.x"), "{intel}");
        assert!(intel.contains("/releases/tag/v2.6.0"), "{intel}");
        let none = pc(
            &[("Microsoft Basic Display Adapter", "PCI\\CC_0300")],
            Runtimes::ALL,
        )
        .unwrap();
        assert!(
            none.contains("no graphics card with a working driver"),
            "{none}"
        );
        assert!(
            none.contains("install its graphics driver from amd.com/support or nvidia.com/drivers"),
            "{none}"
        );
    }

    #[test]
    fn setup_accepts_any_amd_or_nvidia_gpu_with_its_runtime_and_explains_one_without() {
        let hybrid = [
            (
                "NVIDIA GeForce RTX 4060 Laptop GPU",
                "PCI\\VEN_10DE&DEV_28E0",
            ),
            ("AMD Radeon(TM) 780M", "PCI\\VEN_1002&DEV_15BF"),
        ];
        assert_eq!(pc(&hybrid, Runtimes::ALL), None);
        let nvidia = [("NVIDIA GeForce RTX 3060", "PCI\\VEN_10DE&DEV_2503")];
        assert_eq!(pc(&nvidia, Runtimes::ALL), None);
        let no_nvenc = pc(&nvidia, NO_RUNTIME).unwrap();
        assert!(
            no_nvenc.contains("nvEncodeAPI64.dll is missing"),
            "{no_nvenc}"
        );
        assert!(
            no_nvenc.contains("Install the latest NVIDIA graphics driver"),
            "{no_nvenc}"
        );
        let old = [("AMD Radeon HD 7870", "PCI\\VEN_1002&DEV_6818")];
        assert_eq!(pc(&old, Runtimes::ALL), None);
        let no_amf = pc(&old, NO_RUNTIME).unwrap();
        assert!(no_amf.contains("AMD Radeon HD 7870"), "{no_amf}");
        assert!(
            no_amf.contains("Install the latest AMD Radeon graphics driver"),
            "{no_amf}"
        );
    }

    #[test]
    fn this_pc_is_judged_without_panicking() {
        // Reads the registry and System32; the answer depends on the PC.
        let _ = refusal();
    }
}
