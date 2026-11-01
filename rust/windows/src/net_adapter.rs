//! The physical network adapters as `Get-NetAdapter -Physical` lists them:
//! `MSFT_NetAdapter` in `root\StandardCimv2`, with the provider's own
//! `Physical` option. Setup shares this file (`#[path]` in
//! rust/setup/src/main.rs) for its Wake-on-LAN question.

use crate::wmi::{self, Wmi};
use anyhow::Result;

/// `NdisPhysicalMedium` of a Wi-Fi adapter; 0 (unspecified) and 14 (802.3)
/// are wired.
pub const MEDIUM_WIFI: u32 = 9;
const MEDIUM_UNSPECIFIED: u32 = 0;
const MEDIUM_802_3: u32 = 14;

/// One physical adapter.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PhysicalAdapter {
    /// `InterfaceGuid`, `{8-4-4-4-12}` in upper case.
    pub guid: String,
    /// `PnPDeviceID`, empty when Windows reports none.
    pub pnp: String,
    /// `Name`, the localized name the user can change.
    pub name: String,
    /// `InterfaceDescription`, the driver's name for the device.
    pub description: String,
    /// `NdisPhysicalMedium`.
    pub medium: u32,
    /// The instance's `__RELPATH`, for its methods.
    pub path: String,
}

impl PhysicalAdapter {
    pub fn wired(&self) -> bool {
        matches!(self.medium, MEDIUM_UNSPECIFIED | MEDIUM_802_3)
    }
}

/// The namespace of `MSFT_NetAdapter` and its advanced properties.
pub const NAMESPACE: &str = r"root\StandardCimv2";

/// `Get-NetAdapter -Physical`, through a connection to [`NAMESPACE`].
pub fn physical_adapters(wmi: &Wmi) -> Result<Vec<PhysicalAdapter>> {
    let physical = wmi::context(&[("Physical", wmi::Value::Flag(true))])?;
    // Every property: with a list that leaves out the keys, WMI gives no
    // `__RELPATH`.
    wmi.query("SELECT * FROM MSFT_NetAdapter", Some(&physical))?
        .iter()
        .map(|adapter| {
            Ok(PhysicalAdapter {
                guid: wmi::string(adapter, "InterfaceGuid")?
                    .unwrap_or_default()
                    .to_ascii_uppercase(),
                pnp: wmi::string(adapter, "PnPDeviceID")?.unwrap_or_default(),
                name: wmi::string(adapter, "Name")?.unwrap_or_default(),
                description: wmi::string(adapter, "InterfaceDescription")?.unwrap_or_default(),
                medium: wmi::number(adapter, "NdisPhysicalMedium")?.unwrap_or_default(),
                path: wmi::string(adapter, "__RELPATH")?.unwrap_or_default(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn this_pcs_physical_adapters_carry_what_a_fix_needs() -> Result<()> {
        // Read-only. Each instance is named, which restarting an adapter
        // needs.
        let adapters = wmi::on_com_thread("listing the adapters", Duration::from_secs(60), || {
            physical_adapters(&Wmi::connect(NAMESPACE)?)
        })?;
        println!("{adapters:#?}");
        for adapter in &adapters {
            assert_eq!(adapter.guid.len(), 38, "{adapter:?}");
            assert!(adapter.path.starts_with("MSFT_NetAdapter."), "{adapter:?}");
            assert!(!adapter.name.is_empty(), "{adapter:?}");
        }
        Ok(())
    }
}
