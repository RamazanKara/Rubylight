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
pub mod audio_dsp;
pub use rubylight_protocol::audio_ext;
pub mod audio_haptics;
pub mod audio_passthrough;
pub mod audio_scope;
pub mod auth;
pub mod background;
pub mod bandwidth_probe;
pub mod bitstream;
pub mod browse;
pub mod capture_policy;
pub mod catalog;
pub mod client_warnings;
pub mod clock_sync;
pub mod codec_support;
pub mod config;
pub mod controller_audio;
pub mod couch_menu;
pub mod crypto;
pub mod display_caps;
pub mod display_policy;
pub mod dovi;
pub mod dualsense;
pub mod edid;
pub mod encoder_backlog;
pub mod encoder_focus;
pub mod encoder_policy;
pub mod extensions;
pub mod fec_status;
pub mod file_transfer;
pub mod framegen;
pub mod haptics;
pub mod hdr;
pub mod hdr_dynamic;
pub mod host_timing;
pub mod hotkey;
pub mod input;
pub mod input_policy;
pub mod keyboard_layout;
pub mod keyframe_loss;
pub mod launcher_prompt;
pub mod library;
pub mod link_control;
pub mod link_notice;
pub mod link_rate;
pub mod log_analysis;
pub mod logfile;
pub mod lossless;
pub mod ltr;
pub mod mic;
pub mod migration;
pub mod native;
pub use rubylight_native_api as native_api;
pub mod network_pacing;
pub mod network_path;
pub mod os_gate;
pub mod packet;
pub use rubylight_protocol::packed444;
pub mod pairing;
pub mod paths;
pub mod performance;
pub mod phase_sync;
pub mod pipeline_rates;
pub mod playnite;
pub mod power_action;
pub mod present_timing;
pub mod pyrowave;
pub mod pyrowave_probe;
pub mod radeon_profile;
pub mod reconfigure;
pub mod remote;
pub mod roaming;
pub mod rtsp;
pub mod session;
pub mod session_state;
pub mod stall_watch;
pub mod state;
pub mod statistical_fec;
pub mod steam;
pub mod steam_deck;
pub mod steam_feedback;
pub mod stream_policy;
pub mod stream_preset;
pub mod text_input;
pub mod timing_record;
pub mod topology;
pub mod tray;
mod update_files;
pub mod update_recovery;
pub mod usbip;
/// USB forwarding's control message (0x553A) and the USB/IP framing its tunnel carries.
pub use rubylight_protocol::{usb as usb_forward, usbip as usbip_wire};
pub mod version;
pub mod virtual_output;
pub mod wake;
pub mod webhook;
pub mod webrtc;
