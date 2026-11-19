//! Nightjar's stream policies, shared by the desktop clients and, once the C engine is gone, the
//! Android app. Pure logic: no I/O and no clock. Callers pass time in as `now_ms` from a
//! monotonic clock.
//!
//! Until Android calls this crate, the Java originals (and Java ports such as [`stick`]'s) stay
//! equal to it through the shared traces in `tests/traces` (see [`trace`]).

pub mod bitrate;
pub mod choices;
pub mod client_hint;
pub mod collections;
pub mod connection_test;
pub mod health;
pub mod continue_here;
pub mod controllers;
pub mod game_info;
pub mod launch;
pub mod library_order;
pub mod open_play;
pub mod overlay;
pub mod presets;
pub mod pyrowave;
pub mod quality;
pub mod recovery;
pub mod rumble;
pub mod shortcuts;
pub mod simple_profile;
pub mod stats;
pub mod stick;
pub mod stream_menu;
pub mod trace;
pub mod vsync;
