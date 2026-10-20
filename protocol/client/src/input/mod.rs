//! Input: keyboard, text, mouse, scroll, controllers, touch, pen, controller touch, motion
//! and battery, sent as `0x0206` messages on the encrypted control stream, plus microphone
//! datagrams.
//!
//! Ported from moonlight-common-c's `InputStream.c` and `Input.h` (GPL-3.0, Cameron Gutman
//! and contributors) with Nightjar's bughunt-input and network-grace-input patches.

pub mod microphone;
pub mod packets;
pub mod queue;

pub use packets::InputEvent;
pub use queue::{InputError, InputPacket, InputQueue};
