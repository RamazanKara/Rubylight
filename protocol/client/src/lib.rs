//! Nightjar's client streaming engine, shared by the Android app and the desktop clients.
//! The plan and status are in internal/RUST_CORE.md.

pub mod audio;
pub mod color;
pub mod control;
pub mod crypto;
pub mod fec;
pub mod http;
pub mod input;
pub mod priority;
pub mod report;
pub mod roaming;
pub mod rtsp;
pub mod sdp;
pub mod session;
pub mod timing_record;
pub mod usb;
pub mod video;

#[cfg(test)]
mod fuzz_tests;
