//! Platform and media probe hooks injected by `nightjar-desktop` so `nightjar-desktop-ui`
//! remains completely free of `sdl3-sys`, GPU/FFmpeg SDKs, `self_update` and OS libraries.

use std::cell::Cell;
use std::sync::OnceLock;

/// Pure key event passed from the SDL launcher to the hotkey recorder dialog (`ui/dialogs.rs`)
/// without pulling `sdl3_sys` into `nightjar-desktop-ui`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HotkeyInput {
    /// Escape with no modifier held cancels the dialog.
    Cancel,
    /// An un-modified arrow or Enter key moves focus in the dialog.
    Navigate,
    /// A lone modifier key is ignored while waiting for the rest of the chord.
    ModifierOnly,
    /// A valid chord recorded in `nightjar-policy` key-name order (`["CTRL", "ALT", "Q"]`).
    Recorded(Vec<String>),
    /// A physical key whose SDL scancode has no mapping in `nightjar-policy` (`SDL_GetScancodeName`).
    Unsupported { name: String },
}

/// Pure gamepad navigation/hotkey button for the stream menu (`stream_menu`), decoupled from
/// `SDL_GamepadButton`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuNavButton {
    South,
    East,
    West,
    North,
    LeftShoulder,
    RightShoulder,
    Other,
}

/// Callbacks provided by `nightjar-desktop` at startup for hardware/platform queries in Settings.
#[derive(Clone, Copy)]
pub struct PlatformHooks {
    pub pyrowave_probe: fn(usize, bool, bool) -> Option<Result<(), String>>,
    /// Returns `None` while the GPU probe is still in progress, `Some(true)` if AI enhancement is
    /// supported, `Some(false)` if unavailable.
    pub enhance_probe: fn(usize) -> Option<bool>,
    pub clear_scan_sync: fn(),
    pub metalfx_supported: fn() -> bool,
    pub offers_714: fn() -> bool,
    pub self_update_checks: fn() -> bool,
    pub kms_active: fn() -> bool,
    /// `(power_saving_name, high_performance_name)` on Windows multi-GPU systems.
    pub gpu_pair: fn() -> Option<(String, String)>,
    pub video_driver: fn() -> String,
    pub local_day: fn(u64) -> i64,
}

impl Default for PlatformHooks {
    fn default() -> Self {
        Self {
            pyrowave_probe: |_, _, _| Some(Err("not available in UI tests".to_owned())),
            enhance_probe: |_| Some(false),
            clear_scan_sync: || {},
            metalfx_supported: || false,
            offers_714: || cfg!(windows),
            self_update_checks: || cfg!(windows),
            kms_active: || false,
            gpu_pair: || None,
            video_driver: String::new,
            local_day: |secs| (secs / 86_400) as i64,
        }
    }
}

static HOOKS: OnceLock<PlatformHooks> = OnceLock::new();
thread_local! {
    static PYROWAVE_PROBE_STARTED_FOR_TEST: Cell<bool> = const { Cell::new(false) };
}

/// Registers the real platform/media callbacks from `nightjar-desktop`.
pub fn register(hooks: PlatformHooks) {
    let _ = HOOKS.set(hooks);
}

pub fn hooks() -> &'static PlatformHooks {
    static DEFAULT: PlatformHooks = PlatformHooks {
        pyrowave_probe: |_, _, _| {
            PYROWAVE_PROBE_STARTED_FOR_TEST.with(|f| f.set(true));
            None
        },
        enhance_probe: |_| Some(false),
        clear_scan_sync: || {},
        metalfx_supported: || false,
        offers_714: || cfg!(windows),
        self_update_checks: || cfg!(windows),
        kms_active: || false,
        gpu_pair: || None,
        video_driver: String::new,
        local_day: |secs| (secs / 86_400) as i64,
    };
    HOOKS.get().unwrap_or(&DEFAULT)
}

/// Used by `ui/tests.rs` when testing under pure `nightjar-desktop-ui` without `desktop-media`.
pub fn pyrowave_probe_started_for_test() -> bool {
    PYROWAVE_PROBE_STARTED_FOR_TEST.with(Cell::get)
}
