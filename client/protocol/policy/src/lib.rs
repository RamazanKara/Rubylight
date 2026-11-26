//! Nightjar's stream policies, shared by the desktop clients and the
//! Android app. Pure logic: no I/O and no clock. Callers pass time in as `now_ms` from a
//! monotonic clock.
//!
//! Until Android calls this crate, the Java originals (and Java ports such as [`stick`]'s) stay
//! equal to it through the shared traces in `tests/traces` (see [`trace`]).

pub mod bitrate;
pub mod choices;
pub mod client_hint;
pub mod collections;
pub mod connect_text;
pub mod connection_test;
pub mod console;
pub mod continue_here;
pub mod controller;
pub mod controllers;
pub mod display_profile;
pub mod full_chroma;
pub mod game_info;
pub mod gamepad_constants;
pub mod health;
pub mod hosts;
pub mod http;
pub mod internet_path;
pub mod launch;
pub mod library_order;
pub mod mdns;
pub mod mic_route;
pub mod open_play;
pub mod overlay;
pub mod permissions;
pub mod pin;
pub mod presets;
pub mod pyrowave;
pub mod quality;
pub mod recovery;
pub mod roaming;
pub mod rumble;
pub mod safe_area;
pub mod settings_catalog_gen;
pub mod shortcuts;
pub mod simple_profile;
pub mod stats;
pub mod stick;
pub mod stream_menu;
pub mod tile;
pub mod trace;
pub mod update;
pub mod video_codec;
pub mod video_pacing;
pub mod video_queue;
pub mod vsync;
pub mod wol;
