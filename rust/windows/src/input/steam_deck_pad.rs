//! Virtual Steam Deck controller attachment and rumble.
pub(super) use super::usbip_pad::usbip_exe;
use super::{gamepads::Slot, usbip_pad::UsbPad};
use anyhow::Result;
use butterpollo_core::{
    input::Input as Event,
    steam_deck::{REPORT_PERIOD, SteamDeck},
};
use std::path::Path;

/// Whether Steam is running on the host, which `auto` needs: without Steam
/// Input, games that read XInput would not see a Steam Deck controller.
pub(super) fn steam_running() -> bool {
    crate::process::processes().is_ok_and(|list| {
        list.iter()
            .any(|process| process.name.eq_ignore_ascii_case("steam.exe"))
    })
}

pub(super) struct DeckPad {
    usb: UsbPad<SteamDeck>,
    last_rumble: Option<(u16, u16)>,
    // Field order keeps the slot reserved until USB detach has completed.
    _slot: Slot,
}
/// The Deck's USB serial, unique on the host: Windows and Steam Input tell
/// two Decks apart by it, and every client numbers its controllers from 0.
fn serial(slot: &Slot) -> String {
    format!("BUTTERPOLLO{}", slot.id())
}
impl DeckPad {
    pub(super) fn plug(exe: &Path, slot: Slot) -> Result<Self> {
        Ok(Self {
            usb: UsbPad::plug(exe, SteamDeck::new(&serial(&slot)), REPORT_PERIOD)?,
            last_rumble: None,
            _slot: slot,
        })
    }
    pub(super) fn port(&self) -> u32 {
        self.usb.port()
    }
    /// Applies a controller, motion or touch event.
    pub(super) fn apply(&self, event: &Event) {
        if let Some(export) = &self.usb.export
            && export.with(|deck| deck.state.apply(event))
        {
            export.changed();
        }
    }
    /// Rumble Steam set since the last call, as the VHF driver's Xbox
    /// feedback report: low and high motor speed, then trigger motors.
    pub(super) fn feedback(&mut self) -> Option<(u16, Vec<u8>)> {
        let rumble = self.usb.export.as_ref()?.with(SteamDeck::take_rumble)?;
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
    fn two_clients_controller_0_decks_get_different_serials() {
        let (first, second) = (Slot::reserve().unwrap(), Slot::reserve().unwrap());
        assert_ne!(serial(&first), serial(&second));
        assert_eq!(serial(&first), format!("BUTTERPOLLO{}", first.id()));
    }
}
