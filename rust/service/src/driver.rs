//! Whether Rubylight's virtual display and virtual gamepad drivers are there,
//! read from Plug and Play inside the service: no process starts for it. A
//! device counts as there when its interface is present, or when it is
//! installed and switched off, which the user may have done on purpose.
//! Whether the host can also use the display driver (its protocol) only the
//! host can say ([`crate::tasks::virtual_display`]).
use anyhow::Result;
use windows::{
    Win32::{Devices::DeviceAndDriverInstallation::*, Foundation::ERROR_NO_MORE_ITEMS},
    core::{GUID, PCWSTR},
};

/// One of the drivers setup installs and the service watches
/// ([`crate::driver_watch`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Driver {
    /// rubylight-vdisplay, the virtual display driver.
    Display,
    /// rubylight-vgamepad, the virtual gamepad (VHF) driver.
    Gamepad,
}
impl Driver {
    pub const ALL: [Self; 2] = [Self::Display, Self::Gamepad];
    /// Its name in the log.
    pub fn name(self) -> &'static str {
        match self {
            Self::Display => "virtual display",
            Self::Gamepad => "virtual gamepad",
        }
    }
    /// The device interface the running driver registers
    /// (`rubylight_vdisplay_proto::INTERFACE_GUID_U128`,
    /// `rubylight_vgamepad_proto::protocol::DEVICE_INTERFACE_GUID`).
    fn interface(self) -> GUID {
        match self {
            Self::Display => GUID::from_u128(0x5e24_0e6b_b196_4295_81c3_533c_1000_0c59),
            Self::Gamepad => GUID::from_u128(0xb8a3e710_4f2d_4c91_9a18_6e2f5b841a01),
        }
    }
    /// The root-enumerated hardware ID its INF binds
    /// (`rubylight_vdisplay_proto::HARDWARE_ID`,
    /// `rubylight_vgamepad_proto::protocol::ROOT_HARDWARE_ID`).
    fn hardware_id(self) -> &'static str {
        match self {
            Self::Display => r"Root\RubylightVDisplay",
            Self::Gamepad => r"Root\RubylightVirtualGamepad",
        }
    }
    /// Its setup class: display adapters, and system devices for the gamepad
    /// (`ClassGuid` in rubylight_vgamepad.inx).
    fn class(self) -> GUID {
        match self {
            Self::Display => GUID::from_u128(0x4d36e968_e325_11ce_bfc1_08002be10318),
            Self::Gamepad => GUID::from_u128(0x4d36e97d_e325_11ce_bfc1_08002be10318),
        }
    }
}

struct DeviceSet(HDEVINFO);
impl Drop for DeviceSet {
    fn drop(&mut self) {
        // SAFETY: this value owns the SetupAPI set and closes it once.
        let _ = unsafe { SetupDiDestroyDeviceInfoList(self.0) };
    }
}

/// Whether `driver`'s device is there: its interface present, or the device
/// switched off. An error when Plug and Play could not be read.
pub fn present(driver: Driver) -> Result<bool> {
    Ok(interface_present(driver.interface())? || switched_off(driver))
}

fn interface_present(interface_guid: GUID) -> Result<bool> {
    // SAFETY: a valid interface GUID and no window or enumerator; read-only enumeration.
    let set = DeviceSet(unsafe {
        SetupDiGetClassDevsW(
            Some(&interface_guid),
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
    match unsafe { SetupDiEnumDeviceInterfaces(set.0, None, &interface_guid, 0, &mut interface) } {
        Ok(()) => Ok(true),
        Err(error) if error.code() == ERROR_NO_MORE_ITEMS.to_hresult() => Ok(false),
        Err(error) => Err(error.into()),
    }
}

/// Whether a devnode of `driver` exists and is switched off (Device Manager,
/// or a policy). Anything this cannot read counts as not switched off, as in
/// the host's `driver_recovery::disabled`. The system class holds more
/// devices than the display class, hence the larger bound.
fn switched_off(driver: Driver) -> bool {
    let class = driver.class();
    // SAFETY: a valid class GUID and no window or enumerator; read-only enumeration.
    let Ok(handle) =
        (unsafe { SetupDiGetClassDevsW(Some(&class), PCWSTR::null(), None, DIGCF_PRESENT) })
    else {
        return false;
    };
    let set = DeviceSet(handle);
    for index in 0..4096 {
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
        if read == CR_SUCCESS && is_switched_off(driver, &ids, status, problem) {
            return true;
        }
    }
    false
}

fn is_switched_off(
    driver: Driver,
    ids: &[String],
    status: CM_DEVNODE_STATUS_FLAGS,
    problem: CM_PROB,
) -> bool {
    ids.iter()
        .any(|id| id.eq_ignore_ascii_case(driver.hardware_id()))
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
        for driver in Driver::ALL {
            let ours = vec![driver.hardware_id().to_ascii_lowercase()];
            let gpu = vec![r"PCI\VEN_1002&DEV_744C".to_owned()];
            let off =
                |ids: &[String], status, problem| is_switched_off(driver, ids, status, problem);
            assert!(off(&ours, DN_HAS_PROBLEM, CM_PROB_DISABLED));
            assert!(off(&ours, DN_HAS_PROBLEM, CM_PROB_DISABLED_SERVICE));
            assert!(!off(&gpu, DN_HAS_PROBLEM, CM_PROB_DISABLED));
            // A failed start is a broken driver, not a choice.
            assert!(!off(&ours, DN_HAS_PROBLEM, CM_PROB_FAILED_START));
            assert!(!off(
                &ours,
                CM_DEVNODE_STATUS_FLAGS::default(),
                CM_PROB_DISABLED
            ));
        }
        // The other driver's device switched off is not this one's.
        let display = vec![Driver::Display.hardware_id().to_owned()];
        assert!(!is_switched_off(
            Driver::Gamepad,
            &display,
            DN_HAS_PROBLEM,
            CM_PROB_DISABLED
        ));
        assert_ne!(Driver::Display.interface(), Driver::Gamepad.interface());
    }
    #[test]
    fn plug_and_play_answers_on_this_pc() {
        // Read-only; whether the drivers are installed here does not matter.
        for driver in Driver::ALL {
            present(driver).unwrap();
        }
    }
}
