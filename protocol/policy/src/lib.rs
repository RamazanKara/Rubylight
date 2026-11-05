//! Nightjar's stream policies, shared by the desktop clients and, once the C engine is gone, the
//! Android app. Pure logic: no I/O and no clock. Callers pass time in as `now_ms` from a
//! monotonic clock.
//!
//! Until Android calls this crate, the Java originals (and Java ports such as [`stick`]'s) stay
//! equal to it through the shared traces in `tests/traces` (see [`trace`]).

pub mod bitrate;
pub mod client_hint;
pub mod health;
pub mod launch;
pub mod open_play;
pub mod overlay;
pub mod pyrowave;
pub mod quality;
pub mod recovery;
pub mod shortcuts;
pub mod stats;
pub mod stick;
pub mod stream_menu;
pub mod trace;
