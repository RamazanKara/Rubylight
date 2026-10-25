//! Optional WebRTC server for browser and TV clients (`docs/webrtc-design.md`). The signalling
//! server, discovery and device registry run when enabled; `session` starts sessions on the stream
//! pipeline over the str0m media transport.
#![allow(dead_code)]

pub mod clients;
pub mod discovery;
pub mod media;
pub mod sdp;
pub mod server;
pub mod session;
