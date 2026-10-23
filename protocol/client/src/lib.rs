//! Nightjar's client streaming engine, shared by the Android app and the desktop clients.
//! The plan and status are in internal/RUST_CORE.md.

pub mod audio;
pub mod control;
pub mod crypto;
pub mod fec;
pub mod http;
pub mod input;
pub mod rtsp;
pub mod sdp;
pub mod session;
pub mod timing_record;
pub mod video;
