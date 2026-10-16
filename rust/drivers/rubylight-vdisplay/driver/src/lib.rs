//! rubylight-vdisplay: Rubylight's UMDF IddCx virtual display driver.
//!
//! Based on punktfunk's pf-vdisplay by Enrico Bühler (unom), MIT OR Apache-2.0
//! (<https://github.com/unom/punktfunk>). Rubylight keeps its IddCx skeleton, monitor and
//! mode handling, owner-scoped control plane and watchdog, and replaces the in-driver encoder
//! with a frame push: each composed frame is copied into a texture ring the host shares, and
//! the host converts and encodes it (`rubylight_vdisplay_proto::ring`).
//!
//! The driver is FFI-bound by nature (IddCx DDIs, D3D12, cross-process shared textures and fences), so
//! it is unsafe-audited rather than unsafe-free: every `unsafe {}` carries its proof and an
//! `unsafe fn` body is not one big unsafe block (workspace lints).

#![allow(non_snake_case, clippy::missing_safety_doc)]

#[macro_use]
mod log;
mod adapter;
mod callbacks;
mod control;
mod copier;
mod direct_3d_device;
mod entry;
mod frame_ring;
mod monitor;
mod registry;
mod swap_chain_processor;
mod watchdog;
mod wdf;
mod worker;

use rubylight_vdisplay_proto::control::status;
use wdk_sys::NTSTATUS;

pub(crate) const STATUS_SUCCESS: NTSTATUS = status::SUCCESS;
pub(crate) const STATUS_NOT_IMPLEMENTED: NTSTATUS = 0xC000_0002u32 as NTSTATUS;
pub(crate) const STATUS_NOT_FOUND: NTSTATUS = status::NOT_FOUND;
pub(crate) const STATUS_NOT_SUPPORTED: NTSTATUS = status::NOT_SUPPORTED;
pub(crate) const STATUS_INVALID_PARAMETER: NTSTATUS = status::INVALID_PARAMETER;
pub(crate) const STATUS_BUFFER_TOO_SMALL: NTSTATUS = status::BUFFER_TOO_SMALL;
pub(crate) const STATUS_ACCESS_DENIED: NTSTATUS = status::ACCESS_DENIED;
pub(crate) const STATUS_DEVICE_BUSY: NTSTATUS = status::DEVICE_BUSY;
pub(crate) const STATUS_INSUFFICIENT_RESOURCES: NTSTATUS = status::INSUFFICIENT_RESOURCES;

/// IddCx in stub mode requires the driver to export its minimum framework version. `11` is
/// IddCx 1.11, the truthful floor: frames arrive through the D3D12 swap chain
/// (`IddCxSwapChainSetDevice2`), and `DriverEntry` also refuses a 1.11 framework that does not
/// report it. An older framework refuses to bind instead of dispatching past its DDI table.
#[unsafe(no_mangle)]
pub static IddMinimumVersionRequired: wdk_sys::ULONG = 11;
