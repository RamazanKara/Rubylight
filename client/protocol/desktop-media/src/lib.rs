//! Nightjar desktop media decode, GPU presentation, streaming pipeline, SDL3 runtime, USB
//! forwarding, controller input and audio modules, without `egui` or `nightjar-desktop-ui`.

#![allow(clippy::not_unsafe_ptr_arg_deref, reason = "SDL3 raw pointers are valid only on the UI thread by crate contract")]

pub use nightjar_desktop_config::{
    app_types, config, gamepad_types, hosts_types, latency_bench_state, network_types, pads_view,
    pin_lock, power_types, shortcuts, stream_types, strings, version_name, window_place_types,
    wol_types, VERSION,
};
pub use hosts_types as hosts;

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub mod amd_decode;
pub mod clipboard;
pub mod controller_audio;
pub mod controller_mouse;
pub mod dmabuf;
pub mod dualsense_bt_audio;
pub mod full_chroma;
pub mod gamepad;
#[cfg(windows)]
pub mod gpu_adapters;
pub mod handheld;
pub mod keymap;
pub mod kms;
pub mod latency_bench;
#[cfg(any(target_os = "linux", test, feature = "test-support"))]
pub mod linux_diagnosis;
#[cfg(any(target_os = "linux", test, feature = "test-support"))]
pub mod linux_video;
#[cfg(any(target_os = "macos", test, feature = "test-support"))]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub mod mac_display;
pub mod media;
pub mod mp4;
pub mod network;
pub mod render_device;
pub mod sdl;
pub mod second_screen;
pub mod shortcuts_runtime;
pub mod stream;
pub mod timing_csv;
pub mod usb;
pub mod usb_arbitration;
#[cfg(any(windows, test))]
pub mod usb_windows;
#[cfg(target_os = "linux")]
pub mod usb_linux;
#[cfg(any(windows, test, feature = "test-support"))]
pub mod usb_winusb;
// Linux and macOS (Unix) share Vulkan video decode.
#[cfg(any(unix, test, feature = "test-support"))]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub mod vulkan_video;
