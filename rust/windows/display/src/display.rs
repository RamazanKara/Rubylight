//! Windows display configuration and streaming display leases.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    mem::size_of,
    time::{Duration, Instant},
};
use windows::{
    Win32::{Devices::Display::*, Foundation::*, Graphics::Gdi::*},
    core::{GUID, PCWSTR},
};

mod color_state;
mod driver_recovery;
pub(crate) mod end_trace;
mod geometry;
mod hdr;
mod hotplug;
mod modes;
mod monitor;
mod recovery;
pub mod rubylight_vdd;
pub mod self_test;
mod topology;
mod virtual_display;

pub use geometry::{Geometry, Placed, geometry, placed};
pub(crate) use hdr::hdr_matches_applied;
pub use hdr::{sdr_white_nits, set_hdr};
pub use modes::{
    dpi_scale, highest_refresh, largest_scale, mode, output_scale, set_dpi_scale, set_mode,
    virtual_scale,
};
pub use recovery::{
    DisplayDiagnostics, DisplayStatus, Guard, Retained, Snapshot, apply_layout, baseline_nodes,
    leased_displays, restore_positions, stream_display_rotation,
};
pub(crate) use recovery::{restore_was_refused, rotation, set_rotation};
pub(crate) use topology::basic_monitors;
pub use topology::{
    Monitor, Topology, active_device_ids, active_displays, edid_refresh, monitors, shown_primary,
};
pub use virtual_display::{
    RenderAdapter, VirtualDisplay, VirtualOptions, configured_render_adapter_with,
    ensure_virtual_display_available, joinable_mode, virtual_display_available,
    virtual_display_installed, virtual_display_os, virtual_display_status, windows_build,
};

fn check(code: i32) -> Result<()> {
    if code == 0 {
        Ok(())
    } else {
        Err(std::io::Error::from_raw_os_error(code).into())
    }
}
fn wide(b: &[u16]) -> String {
    String::from_utf16_lossy(&b[..b.iter().position(|v| *v == 0).unwrap_or(b.len())])
}
fn header(
    kind: DISPLAYCONFIG_DEVICE_INFO_TYPE,
    size: usize,
    luid: LUID,
    id: u32,
) -> DISPLAYCONFIG_DEVICE_INFO_HEADER {
    DISPLAYCONFIG_DEVICE_INFO_HEADER {
        r#type: kind,
        size: size as u32,
        adapterId: luid,
        id,
    }
}

#[cfg(test)]
pub(crate) fn monitor(id: &str, adapter: u32, hdr: bool) -> Monitor {
    Monitor {
        device_id: id.into(),
        monitor_device_path: String::new(),
        display_name: r"\\.\DISPLAY1".into(),
        friendly_name: id.into(),
        hdr_supported: true,
        hdr_enabled: hdr,
        hdr_requested: hdr,
        primary: false,
        adapter: LUID {
            LowPart: adapter,
            HighPart: 0,
        },
        target: 7,
        source: 0,
    }
}
