//! Portable core of the Rubylight host.
//!
//! The Moonlight protocol (pairing, RTSP, packet framing, FEC, input decoding),
//! the host's durable state and configuration, and the policy that decides
//! what the host does: encoder settings, capture pacing, display planning,
//! frame generation, library sync for Steam and Playnite, and updates.
//!
//! Nothing here calls a vendor SDK, so the crate builds and is tested on any
//! OS. `butterpollo-windows` applies these decisions on Windows and the
//! `butterpollo` host crate wires both into its servers. The only Windows
//! calls are the few file operations that need `MoveFileExW`, behind
//! `cfg(windows)`.
#![warn(
    clippy::print_stdout,
    clippy::print_stderr,
    clippy::undocumented_unsafe_blocks
)]
pub mod adaptive_bitrate;
pub mod adaptive_fec;
pub mod adaptive_pacing;
pub mod amd_gpu;
pub mod audio;
pub mod audio_defaults;
pub mod audio_scope;
pub mod auth;
pub mod background;
pub mod bitstream;
pub mod browse;
pub mod capture_policy;
pub mod catalog;
pub mod clock_sync;
pub mod config;
pub mod controller_audio;
pub mod crypto;
pub mod display_caps;
pub mod display_policy;
pub mod dualsense;
pub mod edid;
pub mod encoder_backlog;
pub mod encoder_policy;
pub mod fec_status;
pub mod framegen;
pub mod haptics;
pub mod hdr;
pub mod host_timing;
pub mod hotkey;
pub mod input;
pub mod input_policy;
pub mod keyframe_loss;
pub mod library;
pub mod logfile;
pub mod lossless;
pub mod ltr;
pub mod mic;
pub mod migration;
pub use rubylight_native_api as native_api;
pub mod network_pacing;
pub mod network_path;
pub mod packet;
pub use rubylight_protocol::packed444;
pub mod pairing;
pub mod paths;
pub mod performance;
pub mod phase_sync;
pub mod playnite;
pub mod present_timing;
pub mod pyrowave;
pub mod pyrowave_probe;
pub mod reconfigure;
pub mod remote;
pub mod rtsp;
pub mod session;
pub mod stall_watch;
pub mod state;
pub mod steam;
pub mod steam_deck;
pub mod stream_policy;
pub mod topology;
pub mod tray;
mod update_files;
pub mod update_recovery;
pub mod usbip;
pub mod version;
pub mod virtual_output;
