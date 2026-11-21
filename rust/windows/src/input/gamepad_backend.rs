use super::gamepads::{GamepadTouchRequest, request};
use super::{open_interface, vigem};
use anyhow::{Context, Result, bail};
use butterpollo_core::input_policy::{
    USBIP_DUALSENSE, USBIP_SWITCH2, VHF_AUTO, VHF_SWITCH, VIGEM_DS4, VIGEM_PROFILES, VIGEM_X360,
};
use windows::{
    Win32::{Foundation::*, System::IO::DeviceIoControl},
    core::GUID,
};

/// A chosen ViGEmBus pad without the driver: no other pad stands in, which a
/// tool waiting for this one wouldn't see either (#74).
#[derive(Debug)]
struct VigemMissing(&'static str);
impl std::fmt::Display for VigemMissing {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("ViGEmBus could not be opened")
    }
}

/// What the stream says about a driver that won't open, with what fixes it.
pub(super) fn unavailable(error: &anyhow::Error) -> String {
    let recorded = crate::steam::registry_string(
        windows::Win32::System::Registry::HKEY_LOCAL_MACHINE,
        "SOFTWARE\\Rubylight\\VirtualGamepad",
        "Problem",
    );
    unavailable_text(error, recorded)
}
fn unavailable_text(error: &anyhow::Error, recorded: Option<String>) -> String {
    const IGNORED: &str = "controller input is ignored while keyboard and mouse remain available";
    if let Some(VigemMissing(pad)) = error.downcast_ref::<VigemMissing>() {
        format!(
            "{pad} (ViGEmBus) controller unavailable ({error:#}); {IGNORED}. Rubylight doesn't install ViGEmBus: install it from github.com/nefarius/ViGEmBus/releases, or choose another controller type."
        )
    } else if let Some(recorded) = recorded.filter(|text| !text.trim().is_empty()) {
        recorded
    } else {
        format!(
            "Virtual gamepad driver unavailable ({error:#}); {IGNORED}. Run Rubylight setup again with the gamepad driver selected to restore it."
        )
    }
}

/// The VHF gamepad driver's device interface.
pub(super) const VHF_INTERFACE: GUID =
    GUID::from_u128(rubylight_vgamepad_proto::protocol::DEVICE_INTERFACE_GUID);

/// The VHF driver no longer accepts this client's handle: it answers every
/// request on it with ERROR_INVALID_HANDLE, as it did about half a second
/// after a pad was plugged following a boot (#138). Only a new handle can
/// help; the pads on this one are closed and the driver opened again.
#[derive(Debug)]
pub(super) struct DriverLost;
impl std::fmt::Display for DriverLost {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("the virtual gamepad driver no longer accepts the host's handle")
    }
}
/// Whether an error is the VHF driver refusing the host's handle itself.
pub(super) fn driver_lost(error: &anyhow::Error) -> bool {
    error.downcast_ref::<DriverLost>().is_some()
}
/// A VHF driver's answer, marked [`DriverLost`] when it refused the handle.
/// Only a real handle reaches the driver: the USB/IP profiles' placeholder
/// fails with its own error before any request.
pub(super) fn driver_error(error: windows::core::Error) -> anyhow::Error {
    if error.code() == E_HANDLE {
        anyhow::Error::new(error).context(DriverLost)
    } else {
        error.into()
    }
}

pub(super) enum Backend {
    /// The VHF gamepad driver, which the installer brings: Xbox One, Xbox
    /// Series, DualShock 4, DualSense, Switch Pro and Steam Controller pads in
    /// 16 slots.
    Vhf(Option<HANDLE>),
    /// ViGEmBus, only when `gamepad = vigem_x360` or `vigem_ds4` asks for it.
    Vigem(vigem::Client),
    /// Tests: a driver whose answers are scripted, for the paths that need a
    /// real driver's failures and recovery.
    #[cfg(test)]
    Fake(fake::Driver),
}
impl Backend {
    fn vigem(client: Result<vigem::Client>, profile: u16) -> Result<(Self, u32)> {
        let pad = if profile == VIGEM_DS4 {
            "DualShock 4"
        } else {
            "Xbox 360"
        };
        Ok((
            Self::Vigem(client.context(VigemMissing(pad))?),
            VIGEM_PROFILES,
        ))
    }
    /// Whether another slot may help when the driver refuses one: each VHF
    /// slot is the driver's own, and ViGEmBus has none.
    pub(super) fn every_slot_differs(&self) -> bool {
        matches!(self, Self::Vhf(_))
    }
    /// Whether the driver removes a client's pads as its handle closes, as
    /// ViGEmBus does.
    pub(super) fn frees_pads_on_close(&self) -> bool {
        matches!(self, Self::Vigem(_))
    }
    /// Returns the profiles the installed driver offers, as a bit per profile.
    pub(super) fn open(profile: u16) -> Result<(Self, u32)> {
        if profile == USBIP_DUALSENSE {
            return Ok((Self::Vhf(None), 1 << 5));
        }
        // Every controller is a Switch 2 Pro Controller over usbip-win2.
        if profile == USBIP_SWITCH2 {
            return Ok((Self::Vhf(None), 1 << (VHF_SWITCH - 1)));
        }
        if matches!(profile, VIGEM_X360 | VIGEM_DS4) {
            return Self::vigem(vigem::Client::open(), profile);
        }
        let mut backend = Self::Vhf(Some(open_interface(VHF_INTERFACE)?));
        let out = backend.ioctl(0x800, &request(8, None), 28)?;
        if out.len() != 28
            || u16::from_le_bytes(out[4..6].try_into().unwrap())
                != rubylight_vgamepad_proto::protocol::PROTOCOL_VERSION
        {
            bail!("incompatible VHF gamepad protocol");
        }
        let available = u32::from_le_bytes(out[12..16].try_into().unwrap());
        if !matches!(profile, 0 | VHF_AUTO) && available & (1 << (profile - 1)) == 0 {
            bail!("configured controller profile is unavailable in the installed VHF driver");
        }
        Ok((backend, available))
    }
    pub(super) fn name(&self) -> &'static str {
        match self {
            Self::Vhf(_) => "VHF",
            Self::Vigem(_) => "ViGEmBus",
            #[cfg(test)]
            Self::Fake(_) => "fake",
        }
    }
    fn ioctl(
        &mut self,
        function: u32,
        data: &[u8],
        output: usize,
    ) -> windows::core::Result<Vec<u8>> {
        // SAFETY: self owns a synchronous device handle; both initialized buffers and
        // the byte-count output live through DeviceIoControl, with lengths within their allocations.
        unsafe {
            let mut result = vec![0; output];
            let mut n = 0;
            DeviceIoControl(
                match self {
                    Self::Vhf(Some(handle)) => *handle,
                    // Not ERROR_INVALID_HANDLE: that is the driver's own
                    // answer when it lost a real handle (#138).
                    _ => {
                        return Err(windows::core::Error::new(
                            ERROR_NOT_SUPPORTED.to_hresult(),
                            "no VHF gamepad device: a USB/IP controller profile opens none",
                        ));
                    }
                },
                (0x22 << 16) | (3 << 14) | (function << 2),
                Some(data.as_ptr().cast()),
                data.len() as u32,
                (output != 0).then_some(result.as_mut_ptr().cast()),
                output as u32,
                Some(&mut n),
                None,
            )?;
            result.truncate(n as usize);
            Ok(result)
        }
    }
    /// A request to the VHF driver, its refusal of the handle marked.
    fn call(&mut self, function: u32, data: &[u8], output: usize) -> Result<Vec<u8>> {
        self.ioctl(function, data, output).map_err(driver_error)
    }
    pub(super) fn plug(&mut self, slot: u32, profile: u16) -> Result<()> {
        #[cfg(test)]
        if let Self::Fake(driver) = self {
            return driver.plug(slot);
        }
        if let Self::Vigem(client) = self {
            return client.plug(slot, profile);
        }
        let mut b = request(16, Some(slot));
        b.extend_from_slice(&profile.to_le_bytes());
        b.extend_from_slice(&0u16.to_le_bytes());
        self.call(0x801, &b, 0)?;
        Ok(())
    }
    pub(super) fn unplug(&mut self, slot: u32) -> Result<()> {
        #[cfg(test)]
        if let Self::Fake(driver) = self {
            return driver.unplug(slot);
        }
        if let Self::Vigem(client) = self {
            return client.unplug(slot);
        }
        self.call(0x802, &request(12, Some(slot)), 0)?;
        Ok(())
    }
    pub(super) fn submit(
        &mut self,
        slot: u32,
        buttons: u32,
        left: u8,
        right: u8,
        sticks: &[i16; 4],
    ) -> Result<()> {
        #[cfg(test)]
        if let Self::Fake(driver) = self {
            return driver.submit(slot);
        }
        if let Self::Vigem(client) = self {
            return client.submit(slot, buttons, left, right, sticks);
        }
        let mut b = request(28, Some(slot));
        b.extend_from_slice(&buttons.to_le_bytes());
        for stick in sticks {
            b.extend_from_slice(&stick.to_le_bytes());
        }
        b.extend_from_slice(&[left, right, 0, 0]);
        self.call(0x803, &b, 0)?;
        Ok(())
    }
    pub(super) fn motion(&mut self, slot: u32, kind: u8, xyz: &[f32; 3]) -> Result<()> {
        #[cfg(test)]
        if let Self::Fake(_) = self {
            return Ok(());
        }
        if let Self::Vigem(client) = self {
            return client.motion(slot, kind, xyz);
        }
        let mut b = request(28, Some(slot));
        b.extend_from_slice(&[kind, 0, 0, 0]);
        for f in xyz {
            b.extend_from_slice(
                &((*f as f64 * 1000.).clamp(i32::MIN as f64, i32::MAX as f64) as i32).to_le_bytes(),
            );
        }
        self.call(0x806, &b, 0)?;
        Ok(())
    }
    pub(super) fn touch(&mut self, update: &GamepadTouchRequest) -> Result<()> {
        #[cfg(test)]
        if let Self::Fake(_) = self {
            return Ok(());
        }
        if let Self::Vigem(client) = self {
            return client.touch(update.global, update.slot, update.event, update.position);
        }
        self.call(0x805, &update.packet, 0)?;
        Ok(())
    }
    pub(super) fn battery(&mut self, slot: u32, state: u8, percent: u8) -> Result<()> {
        #[cfg(test)]
        if let Self::Fake(_) = self {
            return Ok(());
        }
        if let Self::Vigem(client) = self {
            return client.battery(slot, state, percent);
        }
        let mut b = request(16, Some(slot));
        b.extend_from_slice(&[percent, state, 0, 0]);
        self.call(0x807, &b, 0)?;
        Ok(())
    }
    /// The real Steam Controller's firmware attributes for a Steam Controller
    /// slot. A driver that predates the request fails it with
    /// ERROR_INVALID_FUNCTION.
    pub(super) fn steam_attributes(
        &mut self,
        slot: u32,
        attributes: &butterpollo_core::steam_pad::SteamAttributes,
    ) -> Result<()> {
        use butterpollo_core::steam_pad::{ATTRIBUTES_IOCTL, attributes_request};
        #[cfg(test)]
        if let Self::Fake(_) = self {
            return Ok(());
        }
        self.call(ATTRIBUTES_IOCTL, &attributes_request(slot, attributes), 0)?;
        Ok(())
    }
    pub(super) fn feedback(&mut self, slot: u32) -> Result<Option<(u16, Vec<u8>)>> {
        #[cfg(test)]
        if let Self::Fake(driver) = self {
            return driver.feedback(slot);
        }
        if let Self::Vigem(client) = self {
            return client.feedback(slot);
        }
        decode_feedback(self.ioctl(0x804, &request(12, Some(slot)), 48))
    }
}

impl Backend {
    /// The event a pad's pending feedback request signals; VHF has none and
    /// is polled.
    pub(super) fn wait_handle(&self, slot: u32) -> Option<HANDLE> {
        match self {
            Self::Vigem(client) => client.wait_handle(slot),
            Self::Vhf(_) => None,
            #[cfg(test)]
            Self::Fake(_) => None,
        }
    }
    /// Whether the pads are the UMDF VHF driver's, which WUDFHost runs.
    pub(super) fn hosted_by_wudfhost(&self) -> bool {
        matches!(self, Self::Vhf(Some(_)))
    }
    pub(super) fn feedback_ready(&self, slot: u32) -> bool {
        match self {
            Self::Vigem(client) => client.feedback_ready(slot),
            Self::Vhf(_) => false,
            #[cfg(test)]
            Self::Fake(_) => false,
        }
    }
    /// Keeps a ViGEmBus DualShock 4's report flowing; VHF pads need nothing.
    pub(super) fn refresh(&mut self) -> Result<()> {
        match self {
            Self::Vigem(client) => client.refresh(),
            Self::Vhf(_) => Ok(()),
            #[cfg(test)]
            Self::Fake(_) => Ok(()),
        }
    }
    /// The slots whose client should be asked for motion again.
    pub(super) fn motion_requests(&mut self) -> Vec<u32> {
        match self {
            Self::Vigem(client) => client.motion_requests(),
            Self::Vhf(_) => Vec::new(),
            #[cfg(test)]
            Self::Fake(_) => Vec::new(),
        }
    }
}

fn decode_feedback(result: windows::core::Result<Vec<u8>>) -> Result<Option<(u16, Vec<u8>)>> {
    let b = match result {
        // libvirtualgamepad's poll_feedback consumes one pending event, or
        // returns STATUS_NO_MORE_ENTRIES. An empty queue is normal at 125 Hz.
        Err(error) if error.code() == ERROR_NO_MORE_ITEMS.to_hresult() => return Ok(None),
        result => result.map_err(driver_error)?,
    };
    if b.len() == 48 {
        let kind = u16::from_le_bytes(b[12..14].try_into().unwrap());
        let len = u16::from_le_bytes(b[14..16].try_into().unwrap()) as usize;
        if kind != 0 && len <= 32 {
            return Ok(Some((kind, b[16..16 + len].to_vec())));
        }
    }
    Ok(None)
}

impl Drop for Backend {
    fn drop(&mut self) {
        // SAFETY: open_interface transferred this handle to Backend, which closes it
        // exactly once after its synchronous I/O has completed.
        unsafe {
            if let Self::Vhf(Some(handle)) = self {
                let _ = CloseHandle(*handle);
            }
        }
    }
}

/// A driver whose answers a test scripts: the post-sleep recovery needs a
/// request to fail and then succeed, without a Windows device.
#[cfg(test)]
pub(crate) mod fake {
    use super::*;
    use std::collections::{BTreeMap, VecDeque};

    #[derive(Default)]
    pub(crate) struct Driver {
        /// Every request, in the order the pads made it: `plug 2`, `submit 2`,
        /// `unplug 0`, `feedback 2`.
        pub(crate) calls: Vec<String>,
        pub(crate) plugs: VecDeque<Result<()>>,
        pub(crate) unplugs: VecDeque<Result<()>>,
        pub(crate) submits: VecDeque<Result<()>>,
        /// What each slot's feedback poll answers; an empty queue is no report.
        pub(crate) feedback: BTreeMap<u32, FeedbackQueue>,
    }
    pub(crate) type FeedbackQueue = VecDeque<Result<Option<(u16, Vec<u8>)>>>;
    impl Driver {
        fn answer(queue: &mut VecDeque<Result<()>>) -> Result<()> {
            queue.pop_front().unwrap_or(Ok(()))
        }
        pub(super) fn plug(&mut self, slot: u32) -> Result<()> {
            self.calls.push(format!("plug {slot}"));
            Self::answer(&mut self.plugs)
        }
        pub(super) fn unplug(&mut self, slot: u32) -> Result<()> {
            self.calls.push(format!("unplug {slot}"));
            Self::answer(&mut self.unplugs)
        }
        pub(super) fn submit(&mut self, slot: u32) -> Result<()> {
            self.calls.push(format!("submit {slot}"));
            Self::answer(&mut self.submits)
        }
        pub(super) fn feedback(&mut self, slot: u32) -> Result<Option<(u16, Vec<u8>)>> {
            self.calls.push(format!("feedback {slot}"));
            self.feedback
                .get_mut(&slot)
                .and_then(VecDeque::pop_front)
                .unwrap_or(Ok(None))
        }
    }
}

#[cfg(test)]
#[path = "feedback_tests.rs"]
mod tests;
