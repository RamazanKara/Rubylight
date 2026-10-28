//! Rubylight wire formats, written once for the host and its clients.
//!
//! Both sides used to carry their own copy of each format: the host in `butterpollo-core`
//! (Rust), the Android client in C and C++. This crate is the single definition. It is
//! `no_std` with `alloc` and has no dependencies, so it builds into the Windows host as an
//! ordinary dependency and into the Android client as a static library through
//! `rubylight-protocol-ffi`.
#![no_std]

extern crate alloc;

pub mod audio_ext;
pub mod clipboard_sync;
pub mod control;
pub mod cursor_shape;
pub mod fec_report;
pub mod hdr;
pub mod ids;
pub mod keyboard_layout;
pub mod ltr_recovery;
pub mod packed444;
pub mod pairing_uri;
pub mod pen;
pub mod phase_lock;
pub mod pipeline_rates;
pub mod present_clock;
pub mod pyrowave;
pub mod rate_change;
pub mod roaming;
pub mod steam_controller;
pub mod text_input;
pub mod usb;
pub mod usbip;
