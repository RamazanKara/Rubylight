//! Optional WebRTC server for browser and TV clients (`docs/webrtc-design.md`). The signalling
//! server, discovery and device registry run when enabled; the str0m media transport is not
//! wired into the stream yet (B4).
#![allow(dead_code)]

pub mod clients;
pub mod discovery;
pub mod media;
pub mod sdp;
pub mod server;
