//! The composite DualSense's USB attachment; never changes audio defaults.
use super::{gamepads::Slot, usbip_pad::UsbPad};
use anyhow::Result;
use butterpollo_core::{
    dualsense::{DualSense, REPORT_PERIOD},
    haptics,
    input::Input,
};
use std::path::Path;

pub(super) struct DualSensePad {
    // Field order keeps the slot reserved until USB detach has completed.
    usb: UsbPad<DualSense>,
    _slot: Slot,
}
impl DualSensePad {
    pub(super) fn plug(exe: &Path, slot: Slot, client: u16, caps: u16) -> Result<Self> {
        let mut device = DualSense::new(slot.id() as u8, client);
        device.capabilities(caps);
        let pad = Self {
            usb: UsbPad::plug(exe, device, REPORT_PERIOD)?,
            _slot: slot,
        };
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(6);
        while !pad
            .usb
            .export
            .as_ref()
            .is_some_and(|e| e.with(|d| d.enumerated()))
        {
            if std::time::Instant::now() >= deadline {
                anyhow::bail!(
                    "Windows has not finished enumerating the DualSense HID/audio interfaces"
                );
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        Ok(pad)
    }
    pub(super) fn audio(&self, kind: u16, payload: &[u8]) {
        if let Some(export) = &self.usb.export {
            export.with(|d| d.audio(kind, payload));
        }
    }
    pub(super) fn speaker(&self) -> Vec<haptics::Frame> {
        self.usb
            .export
            .as_ref()
            .map(|e| e.with(DualSense::take_speaker))
            .unwrap_or_default()
    }
    pub(super) fn apply(&self, event: &Input) {
        if let Some(export) = &self.usb.export {
            export.with(|device| device.apply(event));
            export.changed();
        }
    }
    pub(super) fn capabilities(&self, caps: u16) {
        if let Some(export) = &self.usb.export {
            export.with(|device| device.capabilities(caps));
        }
    }
    pub(super) fn feedback(&self) -> Vec<(u16, Vec<u8>)> {
        self.usb
            .export
            .as_ref()
            .map(|e| e.with(DualSense::take_feedback))
            .unwrap_or_default()
    }
    pub(super) fn haptics(&self) -> Vec<haptics::Frame> {
        self.usb
            .export
            .as_ref()
            .map(|e| e.with(DualSense::take_pcm))
            .unwrap_or_default()
    }
    pub(super) fn clear_pcm(&self) {
        if let Some(export) = &self.usb.export {
            export.with(DualSense::suspend_audio);
        }
    }
}
