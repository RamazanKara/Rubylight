//! Nightjar's stream policies, shared by the desktop clients and, once the C engine is gone, the
//! Android app. Pure logic: no I/O and no clock. Callers pass time in as `now_ms` from a
//! monotonic clock.
//!
//! Until Android calls this crate, the Java originals stay equal to it through the shared traces
//! in `tests/traces` (see [`trace`]).

pub mod bitrate;
pub mod health;
pub mod launch;
pub mod pyrowave;
pub mod quality;
pub mod recovery;
pub mod trace;
