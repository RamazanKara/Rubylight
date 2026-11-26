//! Controller, haptic, hotkey, keyboard layout, touch, and couch-menu input policy.
#![warn(
    clippy::print_stdout,
    clippy::print_stderr,
    clippy::undocumented_unsafe_blocks
)]

pub mod audio_haptics;
pub mod client_cursor;
pub mod controller_audio;
pub mod couch_menu;
pub mod dsu;
pub mod dualsense;
pub mod haptics;
pub mod hotkey;
pub mod input;
pub mod input_latency;
pub mod input_policy;
pub mod keyboard_layout;
pub mod menu_commands;
pub mod ps_speaker;
pub mod steam_deck;
pub mod steam_feedback;
pub mod steam_pad;
pub mod switch2;
pub mod text_input;
pub mod touch_profile;
pub mod usbip;
pub mod virtual_hid;
