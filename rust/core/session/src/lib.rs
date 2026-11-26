//! Durable host state, sessions, app catalogs, launchers, updates, roaming, and WebRTC.
#![warn(
    clippy::print_stdout,
    clippy::print_stderr,
    clippy::undocumented_unsafe_blocks
)]

pub mod audio_dsp_api;
pub mod audio_scope;
pub mod auto_lock;
pub mod background;
pub mod browse;
pub mod catalog;
pub mod child_session;
pub mod client_warnings;
pub mod clipboard_image;
pub mod config_ext;
pub mod connection_commands;
pub mod couch_menu_model;
pub mod device_overrides;
pub mod file_transfer;
#[cfg(test)]
mod fuzz;
pub mod game_files;
pub mod game_settings;
pub mod host_service;
pub mod invites;
pub mod launch_warning;
pub mod launcher_prompt;
pub mod library;
pub mod log_analysis;
pub mod log_redact;
pub mod logfile;
pub mod mic;
pub mod migration;
pub mod multiclient;
pub mod native;
pub mod network_path;
pub mod neural;
pub mod nucleus;
pub mod paths;
pub mod playnite;
pub mod power_action;
pub mod remote;
pub mod roaming;
pub mod session;
pub mod session_history;
pub mod session_state;
pub mod settings_backup;
pub mod state;
pub mod steam;
pub mod steamgriddb;
pub mod tray;
#[path = "../../src/update_files.rs"]
mod update_files;
#[path = "../../src/update_recovery.rs"]
pub mod update_recovery;
pub mod version;
pub mod wake;
pub mod webhook;
pub mod webrtc;
