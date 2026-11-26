//! Windows audio, input, process/service lifecycle, launchers, networking, and system I/O.
#![cfg(windows)]
#![warn(clippy::undocumented_unsafe_blocks)]
#![warn(clippy::print_stdout, clippy::print_stderr)]

pub mod audio;
pub mod audio_app_route;
pub mod audio_route;
pub mod background_write;
pub mod child_session;
pub mod clipboard;
pub mod crash;
pub mod files;
pub mod firewall;
pub mod hip;
pub mod hotkey;
pub mod input;
pub mod ipc;
pub mod keylayout;
pub mod launcher;
pub mod library;
pub mod limiter;
pub mod local_input;
pub mod local_time;
pub mod lossless;
pub mod mic;
pub mod net;
pub mod net_adapter;
pub mod nucleus;
pub(crate) mod os_helpers;
pub mod playnite;
pub mod power;
pub mod process;
pub mod rtss;
pub mod service;
pub mod session_end;
pub mod shared_folder;
pub mod steam;
mod text;
pub mod text_focus;
pub mod tray;
pub mod user_folder;
pub mod wake;
pub mod wifi_scan;
pub mod window_list;
pub mod window_placement;
pub mod wmi;
