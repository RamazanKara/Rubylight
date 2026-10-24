//! Optional WebRTC server for browser and TV clients (`docs/webrtc-design.md`). Not wired into
//! the stream yet: B2 is the str0m media transport, signalling and input come in later briefs.
#![allow(dead_code)]

pub mod media;
pub mod sdp;
