//! Virtual Switch 2 Pro Controller attachment and rumble.
use super::{gamepads::Slot, usbip_pad::UsbPad};
use anyhow::Result;
use butterpollo_core::{
    input::Input as Event,
    switch2::{REPORT_PERIOD, Switch2Pro},
};
use std::path::Path;

pub(super) struct Switch2Pad {
    usb: UsbPad<Switch2Pro>,
    last_rumble: Option<(u16, u16)>,
    // Field order keeps the slot reserved until USB detach has completed.
    _slot: Slot,
}
/// The controller's USB serial, unique on the host: Windows and Steam tell
/// two controllers apart by it, and every client numbers its own from 0.
fn serial(slot: &Slot) -> String {
    format!("BUTTERPOLLO{}", slot.id())
}
impl Switch2Pad {
    pub(super) fn plug(exe: &Path, slot: Slot) -> Result<Self> {
        Ok(Self {
            usb: UsbPad::plug(exe, Switch2Pro::new(&serial(&slot)), REPORT_PERIOD)?,
            last_rumble: None,
            _slot: slot,
        })
    }
    pub(super) fn port(&self) -> u32 {
        self.usb.port()
    }
    /// Applies a controller, motion or battery event. A change is reported
    /// at once, except while the host's driver calibrates the motion clock
    /// against the controller's 4 ms reports.
    pub(super) fn apply(&self, event: &Event) {
        if let Some(export) = &self.usb.export
            && export.with(|pad| pad.state.apply(event) && pad.immediate())
        {
            export.changed();
        }
    }
    /// Rumble the host set since the last call, as the VHF driver's Xbox
    /// feedback report: low and high motor speed, then trigger motors.
    pub(super) fn feedback(&mut self) -> Option<(u16, Vec<u8>)> {
        let rumble = self.usb.export.as_ref()?.with(Switch2Pro::take_rumble)?;
        if self.last_rumble.replace(rumble) == Some(rumble) {
            return None;
        }
        let mut data = rumble.0.to_le_bytes().to_vec();
        data.extend_from_slice(&rumble.1.to_le_bytes());
        data.extend_from_slice(&[0; 4]);
        Some((4, data))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_clients_controller_0_pads_get_different_serials() {
        let (first, second) = (Slot::reserve().unwrap(), Slot::reserve().unwrap());
        assert_ne!(serial(&first), serial(&second));
        // The serial fits the 16 characters the controller's flash holds.
        assert!(serial(&first).len() <= 16);
    }
}
