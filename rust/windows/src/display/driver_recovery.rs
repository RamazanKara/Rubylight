//! Bounded recovery of this driver's failed devnode. A transport error alone never
//! authorizes restarting a device: PnP must identify our exact stopped, failed driver.

use super::*;
use std::sync::{Mutex, mpsc};
use windows::Win32::Devices::DeviceAndDriverInstallation::*;

const COOLDOWN: Duration = Duration::from_secs(60);
const PROBE_INTERVAL: Duration = Duration::from_secs(5);
const MAX_ATTEMPTS: u8 = 3;

#[derive(Default)]
struct Budget {
    attempts: u8,
    last_attempt: Option<Instant>,
}
impl Budget {
    fn available(&self, now: Instant) -> bool {
        self.attempts < MAX_ATTEMPTS
            && self
                .last_attempt
                .is_none_or(|last| now.duration_since(last) >= COOLDOWN)
    }
    fn attempted(&mut self, now: Instant) {
        self.attempts += 1;
        self.last_attempt = Some(now);
    }
    fn healthy(&mut self, now: Instant) {
        // A brief reopening followed by failure must not grant an endless restart loop.
        if self
            .last_attempt
            .is_some_and(|last| now.duration_since(last) >= COOLDOWN)
        {
            *self = Self::default();
        }
    }
}

#[derive(Default)]
struct Recovery {
    budget: Budget,
    probe_after: Option<Instant>,
    worker: Option<mpsc::Receiver<Outcome>>,
}
struct Outcome {
    attempted: bool,
    result: Result<()>,
}
static RECOVERY: Mutex<Recovery> = Mutex::new(Recovery {
    budget: Budget {
        attempts: 0,
        last_attempt: None,
    },
    probe_after: None,
    worker: None,
});

pub fn healthy() {
    let mut recovery = RECOVERY.lock().unwrap();
    recovery.budget.healthy(Instant::now());
}

/// Never waits for native SetupAPI. One worker may remain in flight if Windows is
/// stuck; subsequent heartbeats cannot spawn another or block behind it.
pub fn request() -> bool {
    let mut recovery = RECOVERY.lock().unwrap();
    let now = Instant::now();
    if let Some(worker) = &recovery.worker {
        match worker.try_recv() {
            Ok(outcome) => {
                recovery.worker = None;
                if outcome.attempted {
                    recovery.budget.attempted(now);
                }
                match outcome.result {
                    Ok(()) if outcome.attempted => {
                        tracing::info!("requested recovery of failed Rubylight display driver")
                    }
                    Err(error) => {
                        tracing::warn!(%error, "Rubylight display driver recovery did not complete")
                    }
                    _ => {}
                }
                recovery.probe_after = Some(now + PROBE_INTERVAL);
            }
            Err(mpsc::TryRecvError::Empty) => return true,
            Err(mpsc::TryRecvError::Disconnected) => {
                recovery.worker = None;
                recovery.budget.attempted(now);
                tracing::warn!("Rubylight display driver recovery worker stopped unexpectedly");
            }
        }
    }
    if !recovery.budget.available(now) || recovery.probe_after.is_some_and(|next| now < next) {
        return false;
    }
    let (sender, receiver) = mpsc::channel();
    match std::thread::Builder::new()
        .name("vdisplay-recovery".into())
        .spawn(move || {
            let mut attempted = false;
            let result = restart_failed_driver(&mut attempted);
            let _ = sender.send(Outcome { attempted, result });
        }) {
        Ok(_) => {
            recovery.worker = Some(receiver);
            true
        }
        Err(error) => {
            recovery.probe_after = Some(now + PROBE_INTERVAL);
            tracing::warn!(%error, "could not start display driver recovery worker");
            false
        }
    }
}

fn eligible(
    hardware_ids: &[String],
    service: &str,
    status: CM_DEVNODE_STATUS_FLAGS,
    problem: CM_PROB,
) -> bool {
    hardware_ids
        .iter()
        .any(|id| id.eq_ignore_ascii_case(rubylight_vdisplay_proto::HARDWARE_ID))
        && service.eq_ignore_ascii_case("WUDFRd")
        && status.contains(DN_HAS_PROBLEM)
        && !status.contains(DN_STARTED)
        && matches!(
            problem,
            CM_PROB_FAILED_START | CM_PROB_FAILED_ADD | CM_PROB_FAILED_POST_START
        )
}

/// Our device is installed but switched off (Device Manager, or a policy):
/// that is a choice, not a missing driver. `eligible` refuses the same
/// problems, so no restart or reinstall overrides it either.
fn switched_off(
    hardware_ids: &[String],
    status: CM_DEVNODE_STATUS_FLAGS,
    problem: CM_PROB,
) -> bool {
    hardware_ids
        .iter()
        .any(|id| id.eq_ignore_ascii_case(rubylight_vdisplay_proto::HARDWARE_ID))
        && status.contains(DN_HAS_PROBLEM)
        && matches!(problem, CM_PROB_DISABLED | CM_PROB_DISABLED_SERVICE)
}

/// Whether a devnode of our driver exists and is switched off. Its interface
/// is not there to open, so `virtual_display_available` calls it missing.
/// Anything this cannot read counts as not switched off.
pub fn disabled() -> bool {
    let display_class = GUID::from_u128(0x4d36e968_e325_11ce_bfc1_08002be10318);
    // SAFETY: a valid class GUID and no window/enumerator; this is read-only enumeration.
    let Ok(handle) = (unsafe {
        SetupDiGetClassDevsW(Some(&display_class), PCWSTR::null(), None, DIGCF_PRESENT)
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
        let Ok(ids) = strings(set.0, &device, SPDRP_HARDWAREID) else {
            continue;
        };
        let mut status = CM_DEVNODE_STATUS_FLAGS::default();
        let mut problem = CM_PROB::default();
        // SAFETY: both outputs are live and DevInst came from this present device set.
        let read = unsafe { CM_Get_DevNode_Status(&mut status, &mut problem, device.DevInst, 0) };
        if read == CR_SUCCESS && switched_off(&ids, status, problem) {
            return true;
        }
    }
    false
}

struct DeviceSet(HDEVINFO);
impl Drop for DeviceSet {
    fn drop(&mut self) {
        // SAFETY: this value owns the SetupAPI set and closes it once.
        let _ = unsafe { SetupDiDestroyDeviceInfoList(self.0) };
    }
}

fn strings(
    set: HDEVINFO,
    device: &SP_DEVINFO_DATA,
    property: SETUP_DI_REGISTRY_PROPERTY,
) -> Result<Vec<String>> {
    let mut bytes = vec![0u8; 8192];
    let mut size = 0;
    // SAFETY: device belongs to the live set; the writable buffer has the passed size.
    unsafe {
        SetupDiGetDeviceRegistryPropertyW(
            set,
            device,
            property,
            None,
            Some(&mut bytes),
            Some(&mut size),
        )?;
    }
    anyhow::ensure!(
        size as usize <= bytes.len() && size.is_multiple_of(2),
        "invalid device identity property"
    );
    let words: Vec<u16> = bytes[..size as usize]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .collect();
    words
        .split(|c| *c == 0)
        .filter(|s| !s.is_empty())
        .map(|s| String::from_utf16(s).map_err(Into::into))
        .collect()
}

fn failed(set: HDEVINFO, device: &SP_DEVINFO_DATA) -> Result<bool> {
    let ids = strings(set, device, SPDRP_HARDWAREID)?;
    // Most display adapters are physical GPUs; do not inspect or act on them further.
    if !ids
        .iter()
        .any(|id| id.eq_ignore_ascii_case(rubylight_vdisplay_proto::HARDWARE_ID))
    {
        return Ok(false);
    }
    let service = strings(set, device, SPDRP_SERVICE)?
        .into_iter()
        .next()
        .unwrap_or_default();
    let mut status = CM_DEVNODE_STATUS_FLAGS::default();
    let mut problem = CM_PROB::default();
    // SAFETY: both outputs are live and DevInst came from this present device set.
    let result = unsafe { CM_Get_DevNode_Status(&mut status, &mut problem, device.DevInst, 0) };
    anyhow::ensure!(
        result == CR_SUCCESS,
        "cannot inspect Rubylight display driver state ({})",
        result.0
    );
    Ok(eligible(&ids, &service, status, problem))
}

fn restart_failed_driver(attempted: &mut bool) -> Result<()> {
    // The display setup class contains adapter devices, not the physical monitor class.
    let display_class = GUID::from_u128(0x4d36e968_e325_11ce_bfc1_08002be10318);
    // SAFETY: a valid class GUID and no window/enumerator; this is read-only enumeration.
    let set = DeviceSet(unsafe {
        SetupDiGetClassDevsW(Some(&display_class), PCWSTR::null(), None, DIGCF_PRESENT)?
    });
    let mut candidates = Vec::new();
    let mut complete = false;
    for index in 0..256 {
        let mut device = SP_DEVINFO_DATA {
            cbSize: size_of::<SP_DEVINFO_DATA>() as u32,
            ..Default::default()
        };
        // SAFETY: correctly sized output structure and a live device set.
        match unsafe { SetupDiEnumDeviceInfo(set.0, index, &mut device) } {
            Ok(()) => {
                if strings(set.0, &device, SPDRP_HARDWAREID).is_ok_and(|ids| {
                    ids.iter()
                        .any(|id| id.eq_ignore_ascii_case(rubylight_vdisplay_proto::HARDWARE_ID))
                }) {
                    candidates.push(device);
                }
            }
            Err(error) if error.code() == ERROR_NO_MORE_ITEMS.to_hresult() => {
                complete = true;
                break;
            }
            Err(error) => return Err(error.into()),
        }
    }
    // Never choose arbitrarily between multiple installations.
    if !complete || candidates.len() != 1 {
        return Ok(());
    }
    let device = &candidates[0];
    if !failed(set.0, device)? {
        return Ok(());
    }
    let mut install = SP_DEVINSTALL_PARAMS_W {
        cbSize: size_of::<SP_DEVINSTALL_PARAMS_W>() as u32,
        ..Default::default()
    };
    // SAFETY: device and correctly sized output belong to the live set.
    unsafe {
        SetupDiGetDeviceInstallParamsW(set.0, Some(device), &mut install)?;
    }
    anyhow::ensure!(
        install.Flags.0 & (DI_DONOTCALLCONFIGMG | DI_NEEDREBOOT | DI_NEEDRESTART).0 == 0,
        "Rubylight display driver requires setup or a Windows restart"
    );
    // Re-read immediately before the mutation: it may have recovered while enumerating.
    if !failed(set.0, device)? {
        return Ok(());
    }
    let change = SP_PROPCHANGE_PARAMS {
        ClassInstallHeader: SP_CLASSINSTALL_HEADER {
            cbSize: size_of::<SP_CLASSINSTALL_HEADER>() as u32,
            InstallFunction: DIF_PROPERTYCHANGE,
        },
        StateChange: DICS_PROPCHANGE,
        Scope: DICS_FLAG_CONFIGSPECIFIC,
        HwProfile: 0,
    };
    *attempted = true;
    // SAFETY: parameters describe a restart of this one verified failed devnode. No
    // disable/enable, device installation, physical GPU operation or reboot is requested.
    unsafe {
        SetupDiSetClassInstallParamsW(
            set.0,
            Some(device),
            Some(&change.ClassInstallHeader),
            size_of::<SP_PROPCHANGE_PARAMS>() as u32,
        )?;
        SetupDiCallClassInstaller(DIF_PROPERTYCHANGE, set.0, Some(device))?;
        SetupDiGetDeviceInstallParamsW(set.0, Some(device), &mut install)?;
    }
    anyhow::ensure!(
        install.Flags.0 & (DI_NEEDREBOOT | DI_NEEDRESTART).0 == 0,
        "Windows deferred display driver recovery until restart"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recovery_requires_our_stopped_failed_driver() {
        let own = vec![rubylight_vdisplay_proto::HARDWARE_ID.to_owned()];
        for problem in [
            CM_PROB_FAILED_START,
            CM_PROB_FAILED_ADD,
            CM_PROB_FAILED_POST_START,
        ] {
            assert!(eligible(&own, "WUDFRd", DN_HAS_PROBLEM, problem));
            assert!(!eligible(
                &own,
                "WUDFRd",
                DN_HAS_PROBLEM | DN_STARTED,
                problem
            ));
            assert!(!eligible(
                &own,
                "WUDFRd",
                CM_DEVNODE_STATUS_FLAGS::default(),
                problem
            ));
            assert!(!eligible(
                &[r"PCI\VEN_1002".into()],
                "WUDFRd",
                DN_HAS_PROBLEM,
                problem
            ));
            assert!(!eligible(&own, "other", DN_HAS_PROBLEM, problem));
            // UmdfService is the user-mode driver name; SPDRP_SERVICE names its reflector.
            assert!(!eligible(
                &own,
                "RubylightVDisplay",
                DN_HAS_PROBLEM,
                problem
            ));
        }
        for problem in [
            CM_PROB::default(),
            CM_PROB_DISABLED,
            CM_PROB_DISABLED_SERVICE,
            CM_PROB_NEED_RESTART,
            CM_PROB_FAILED_INSTALL,
        ] {
            assert!(!eligible(&own, "WUDFRd", DN_HAS_PROBLEM, problem));
        }
    }
    #[test]
    fn a_device_the_user_switched_off_is_not_a_missing_driver() {
        let own = vec![rubylight_vdisplay_proto::HARDWARE_ID.to_owned()];
        for problem in [CM_PROB_DISABLED, CM_PROB_DISABLED_SERVICE] {
            assert!(switched_off(&own, DN_HAS_PROBLEM, problem));
            // Neither switched off nor ours.
            assert!(!switched_off(
                &own,
                CM_DEVNODE_STATUS_FLAGS::default(),
                problem
            ));
            assert!(!switched_off(
                &[r"PCI\VEN_1002".into()],
                DN_HAS_PROBLEM,
                problem
            ));
            // The restart path stays out of it too.
            assert!(!eligible(&own, "WUDFRd", DN_HAS_PROBLEM, problem));
        }
        // A failed or healthy device is a different case.
        for problem in [
            CM_PROB::default(),
            CM_PROB_FAILED_START,
            CM_PROB_NEED_RESTART,
        ] {
            assert!(!switched_off(&own, DN_HAS_PROBLEM, problem));
        }
    }
    #[test]
    fn restarts_have_a_cooldown_and_stop_after_three_failures() {
        let mut budget = Budget::default();
        let start = Instant::now();
        for attempt in 0..MAX_ATTEMPTS {
            let now = start + COOLDOWN * u32::from(attempt);
            assert!(budget.available(now));
            budget.attempted(now);
            budget.healthy(now + Duration::from_secs(1));
            assert!(!budget.available(now + Duration::from_secs(1)));
        }
        assert!(!budget.available(start + COOLDOWN * 10));
        budget.healthy(start + COOLDOWN * 10);
        assert!(budget.available(start + COOLDOWN * 10));
    }
}
