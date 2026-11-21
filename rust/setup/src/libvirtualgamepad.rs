//! Removes the earlier virtual gamepad driver (`Root\VibeshineVirtualGamepad` and
//! `VibeshineVhfGamepad.cat`) once Rubylight's own virtual gamepad driver is running, or
//! when uninstalling without keeping shared drivers. With nothing left it changes nothing.
//! Elevated.

use crate::libvirtualdisplay::{Removal, Windows, remove_driver_package};

const HARDWARE_ID: &str = r"Root\VibeshineVirtualGamepad";
const CATALOG: &str = "VibeshineVhfGamepad.cat";

/// Removes the legacy virtual gamepad device and its driver store packages.
pub fn remove(windows: &mut impl Windows) -> Removal {
    let mut restart = false;
    let error = remove_driver_package(
        windows,
        HARDWARE_ID,
        CATALOG,
        "earlier virtual gamepad",
        &mut restart,
    )
    .err();
    Removal { restart, error }
}

#[cfg(test)]
#[path = "libvirtualgamepad_tests.rs"]
mod tests;
