// Copyright (c) 2026 Chase Payne
// SPDX-License-Identifier: MIT
//
// Ported from Nonary/libvirtualgamepad driver/src/driver.cpp.
// Adapted for Rubylight by RamazanKara (https://github.com/RamazanKara/Rubylight).

//! Rubylight UMDF 2 VHF virtual gamepad driver entry and modules.

#![deny(unsafe_op_in_unsafe_fn)]
#![deny(clippy::all)]

#[macro_use]
mod log;
mod device;
mod entry;
mod keepalive;
mod vhf;
mod wdf;

pub use entry::driver_entry;

pub const STATUS_SUCCESS: wdk_sys::NTSTATUS = 0x0000_0000u32 as i32;
pub const STATUS_UNSUCCESSFUL: wdk_sys::NTSTATUS = 0xC000_0001u32 as i32;
pub const STATUS_INVALID_HANDLE: wdk_sys::NTSTATUS = 0xC000_0008u32 as i32;
pub const STATUS_INVALID_PARAMETER: wdk_sys::NTSTATUS = 0xC000_000Du32 as i32;
pub const STATUS_INVALID_DEVICE_REQUEST: wdk_sys::NTSTATUS = 0xC000_0010u32 as i32;
pub const STATUS_BUFFER_TOO_SMALL: wdk_sys::NTSTATUS = 0xC000_0023u32 as i32;
pub const STATUS_INSUFFICIENT_RESOURCES: wdk_sys::NTSTATUS = 0xC000_009Au32 as i32;
pub const STATUS_DEVICE_NOT_READY: wdk_sys::NTSTATUS = 0xC000_00A3u32 as i32;
pub const STATUS_NOT_SUPPORTED: wdk_sys::NTSTATUS = 0xC000_00BBu32 as i32;

#[inline]
pub const fn nt_success(status: wdk_sys::NTSTATUS) -> bool {
    status >= 0
}
