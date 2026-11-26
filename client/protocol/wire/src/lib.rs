//! Shared std wire codecs, packet framing, and crypto primitives for Rubylight and Nightjar.
pub mod controller_audio;
pub mod crypto;
pub mod fec_status;
pub mod haptics;
pub mod input;
#[cfg(test)]
mod input_shared_magic_tests;
pub mod mic;
pub mod packet;
pub mod rtsp;
