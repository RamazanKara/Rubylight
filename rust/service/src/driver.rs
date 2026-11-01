//! Whether Rubylight's virtual display driver is there, read from Plug and
//! Play inside the service: no process starts for it. The device counts as
//! there when its interface is present, or when it is installed and switched
//! off, which the user may have done on purpose. Whether the host can also use
//! it (its protocol) only the host can say ([`crate::tasks::virtual_display`]).
use anyhow::Result;
use windows::{
    Win32::{Devices::DeviceAndDriverInstallation::*, Foundation::ERROR_NO_MORE_ITEMS},
    core::{GUID, PCWSTR},
};

/// The device interface the driver registers
/// (`rubylight_vdisplay_proto::INTERFACE_GUID_U128`).
const INTERFACE: GUID = GUID::from_u128(0x5e24_0e6b_b196_4295_81c3_533c_1000_0c59);
/// The root-enumerated hardware ID its INF binds
/// (`rubylight_vdisplay_proto::HARDWARE_ID`).
const HARDWARE_ID: &str = r"Root\RubylightVDisplay";
/// The display adapter setup class.
const DISPLAY_CLASS: GUID = GUID::from_u128(0x4d36e968_e325_11ce_bfc1_08002be10318);

struct DeviceSet(HDEVINFO);
impl Drop for DeviceSet {
    fn drop(&mut self) {
        // SAFETY: this value owns the SetupAPI set and closes it once.
        let _ = unsafe { SetupDiDestroyDeviceInfoList(self.0) };
    }
}

/// Whether the driver's device is there: its interface present, or the device
/// switched off. An error when Plug and Play could not be read.
pub fn present() -> Result<bool> {
    Ok(interface_present()? || switched_off())
}

fn interface_present() -> Result<bool> {
    // SAFETY: a valid interface GUID and no window or enumerator; read-only enumeration.
    let set = DeviceSet(unsafe {
        SetupDiGetClassDevsW(
            Some(&INTERFACE),
            PCWSTR::null(),
            None,
            DIGCF_DEVICEINTERFACE | DIGCF_PRESENT,
        )?
    });
    let mut interface = SP_DEVICE_INTERFACE_DATA {
        cbSize: size_of::<SP_DEVICE_INTERFACE_DATA>() as u32,
        ..Default::default()
    };
    // SAFETY: correctly sized output structure and a live device set.
    match unsafe { SetupDiEnumDeviceInterfaces(set.0, None, &INTERFACE, 0, &mut interface) } {
        Ok(()) => Ok(true),
        Err(error) if error.code() == ERROR_NO_MORE_ITEMS.to_hresult() => Ok(false),
        Err(error) => Err(error.into()),
    }
}

/// Whether a devnode of the driver exists and is switched off (Device Manager,
/// or a policy). Anything this cannot read counts as not switched off, as in
/// the host's `driver_recovery::disabled`.
fn switched_off() -> bool {
    // SAFETY: a valid class GUID and no window or enumerator; read-only enumeration.
    let Ok(handle) = (unsafe {
        SetupDiGetClassDevsW(Some(&DISPLAY_CLASS), PCWSTR::null(), None, DIGCF_PRESENT)
    }) else {
        return false;
    };
    let set = DeviceSet(handle);
    for index in 0..256 {
        let mut device = SP_DEVINFO_DATA {
            cbSize: size_of::<SP_DEVINFO_DATA>() as u32,
            ..Default::default()
        };
        // SAFETY: correctly sized output structure and a live device set.
        if unsafe { SetupDiEnumDeviceInfo(set.0, index, &mut device) }.is_err() {
            return false;
        }
        let Some(ids) = hardware_ids(&set, &device) else {
            continue;
        };
        let mut status = CM_DEVNODE_STATUS_FLAGS::default();
        let mut problem = CM_PROB::default();
        // SAFETY: both outputs are live and DevInst came from this present device set.
        let read = unsafe { CM_Get_DevNode_Status(&mut status, &mut problem, device.DevInst, 0) };
        if read == CR_SUCCESS && is_switched_off(&ids, status, problem) {
            return true;
        }
    }
    false
}

fn is_switched_off(ids: &[String], status: CM_DEVNODE_STATUS_FLAGS, problem: CM_PROB) -> bool {
    ids.iter().any(|id| id.eq_ignore_ascii_case(HARDWARE_ID))
        && status.contains(DN_HAS_PROBLEM)
        && matches!(problem, CM_PROB_DISABLED | CM_PROB_DISABLED_SERVICE)
}

fn hardware_ids(set: &DeviceSet, device: &SP_DEVINFO_DATA) -> Option<Vec<String>> {
    let mut bytes = vec![0u8; 8192];
    let mut size = 0;
    // SAFETY: device belongs to the live set; the writable buffer has the passed size.
    unsafe {
        SetupDiGetDeviceRegistryPropertyW(
            set.0,
            device,
            SPDRP_HARDWAREID,
            None,
            Some(&mut bytes),
            Some(&mut size),
        )
        .ok()?;
    }
    if size as usize > bytes.len() || !size.is_multiple_of(2) {
        return None;
    }
    let words: Vec<u16> = bytes[..size as usize]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| u16::from_le_bytes(*b))
        .collect();
    Some(
        words
            .split(|c| *c == 0)
            .filter(|s| !s.is_empty())
            .map(String::from_utf16_lossy)
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_our_device_switched_off_counts_as_there() {
        let ours = vec![HARDWARE_ID.to_ascii_lowercase()];
        let gpu = vec![r"PCI\VEN_1002&DEV_744C".to_owned()];
        assert!(is_switched_off(&ours, DN_HAS_PROBLEM, CM_PROB_DISABLED));
        assert!(is_switched_off(
            &ours,
            DN_HAS_PROBLEM,
            CM_PROB_DISABLED_SERVICE
        ));
        assert!(!is_switched_off(&gpu, DN_HAS_PROBLEM, CM_PROB_DISABLED));
        // A failed start is a broken driver, not a choice.
        assert!(!is_switched_off(
            &ours,
            DN_HAS_PROBLEM,
            CM_PROB_FAILED_START
        ));
        assert!(!is_switched_off(
            &ours,
            CM_DEVNODE_STATUS_FLAGS::default(),
            CM_PROB_DISABLED
        ));
    }
    #[test]
    fn plug_and_play_answers_on_this_pc() {
        // Read-only; whether the driver is installed here does not matter.
        present().unwrap();
    }
}
