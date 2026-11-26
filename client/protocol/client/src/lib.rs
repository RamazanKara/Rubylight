//! Nightjar's client streaming engine, shared by the Android app and the desktop clients.
//! The plan and status are in internal/RUST_CORE.md.

pub mod audio;
pub mod color;
pub mod connect;
pub mod connectivity;
pub mod console;
pub mod control;
pub mod crypto;
pub mod fec;
pub mod frame_warp;
pub mod frame_warp_v2;
pub use nightjar_policy::full_chroma;
pub mod host_loss;
pub mod hosts;
pub mod http;
pub mod input;
pub mod internet;
pub mod invite;
pub mod mdns;
pub mod pin_lock;
pub mod priority;
pub mod report;
pub mod roaming;
pub mod route;
pub mod rtsp;
pub mod sdp;
pub mod session;
pub mod sharp_color;
pub mod stun;
pub use nightjar_policy::tile;
pub mod timing_record;
pub mod update;
pub mod usb;
pub mod video;
pub mod wol;

#[cfg(test)]
mod fuzz_tests;
