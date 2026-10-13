//! Virtual Steam Deck controller attachment and rumble.
use super::usbip_pad::UsbPad;
pub(super) use super::usbip_pad::usbip_exe;
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
}
impl DeckPad {
    pub(super) fn plug(exe: &Path, serial: &str) -> Result<Self> {
        Ok(Self {
            usb: UsbPad::plug(exe, SteamDeck::new(serial), REPORT_PERIOD)?,
            last_rumble: None,
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
