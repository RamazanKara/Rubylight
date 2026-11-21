// Copyright (c) 2026 Chase Payne
// SPDX-License-Identifier: MIT
//
// Ported from Nonary/libvirtualgamepad (https://github.com/Nonary/libvirtualgamepad)
// and adapted for Rubylight by RamazanKara (https://github.com/RamazanKara/Rubylight).

//! Host-driver wire protocol, portable HID controller profiles, force-feedback
//! engine, report pacing pump, and lifecycle state machine for `rubylight-vgamepad`.

#![no_std]

pub mod lifecycle;
pub mod profiles;
pub mod protocol;
pub mod report_pump;
pub mod steam_haptic_queue;

/// Splits a big-endian RFC 4122 UUID `u128` into WDK/Win32 `GUID` fields
/// `(Data1, Data2, Data3, Data4)`.
pub const fn guid_fields(uuid: u128) -> (u32, u16, u16, [u8; 8]) {
    let bytes = uuid.to_be_bytes();
    (
        u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
        u16::from_be_bytes([bytes[4], bytes[5]]),
        u16::from_be_bytes([bytes[6], bytes[7]]),
        [
            bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15],
        ],
    )
}
