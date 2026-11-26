//! Field-wise read, write, and diff dispatch for [`Settings`], avoiding full-struct JSON
//! round-trips in [`super::Overrides`] and per-frame UI checks.

use super::{migrate, Settings};

macro_rules! settings_fields {
    (
        $(plain $field:ident),* $(,)?
        ;
        $(retired $retired_key:literal => $retired_field:ident),* $(,)?
    ) => {
        pub(super) fn apply_field(
            out: &mut Settings,
            key: &str,
            value: &serde_json::Value,
        ) -> Result<(), serde_json::Error> {
            match key {
                $(stringify!($field) => {
                    out.$field = serde_json::from_value(value.clone())?;
                })*
                "virtual_display_scale" => {
                    out.virtual_display_scale = migrate::render_scale(value)?;
                }
                "virtual_display_dpi" => {
                    out.virtual_display_dpi = migrate::display_scale(value)?;
                }
                $($retired_key => {
                    out.$retired_field = serde_json::from_value(value.clone())?;
                })*
                // Unknown keys from a newer Nightjar stay ignored, as serde without
                // `deny_unknown_fields` does when deserializing `Settings`.
                _ => {}
            }
            Ok(())
        }

        pub(super) fn capture_field(settings: &Settings, key: &str) -> serde_json::Value {
            match key {
                $(stringify!($field) => {
                    serde_json::to_value(&settings.$field).expect("setting serialize")
                })*
                "virtual_display_scale" => {
                    serde_json::to_value(settings.virtual_display_scale).expect("setting serialize")
                }
                "virtual_display_dpi" => {
                    serde_json::to_value(settings.virtual_display_dpi).expect("setting serialize")
                }
                other => {
                    // Fallback ensures any future field added before updating this table still captures.
                    let all = serde_json::to_value(settings).expect("settings serialize");
                    all.get(other).cloned().unwrap_or(serde_json::Value::Null)
                }
            }
        }

        pub(super) fn field_differs(a: &Settings, b: &Settings, key: &str) -> Option<bool> {
            match key {
                $(stringify!($field) => Some(a.$field != b.$field),)*
                "virtual_display_scale" => {
                    Some(a.virtual_display_scale != b.virtual_display_scale)
                }
                "virtual_display_dpi" => Some(a.virtual_display_dpi != b.virtual_display_dpi),
                $($retired_key => Some(a.$retired_field != b.$retired_field),)*
                _ => None,
            }
        }
    };
}

settings_fields!(
    plain width,
    plain height,
    plain native_scale,
    plain fps,
    plain bitrate_kbps,
    plain lower_bitrate,
    plain adaptive_quality,
    plain quality_min_short_edge,
    plain quality_min_fps,
    plain codec,
    plain hdr,
    plain hdr_desktop_brightness,
    plain experimental_packed_444,
    plain prefer_yuv444,
    plain sdr_10bit,
    plain audio,
    plain play_audio_on_host,
    plain mute_audio,
    plain audio_passthrough,
    plain window_mode,
    plain windowed_launcher,
    plain video_scaling,
    plain align_video_top,
    plain vrr,
    plain present_at_host_times,
    plain vrr_tear_free,
    plain vrr_wait,
    plain vsync,
    plain frame_pacing,
    plain drop_late_frames,
    plain host_frame_times,
    plain fractional_vsync,
    plain overlay,
    plain overlay_mode,
    plain overlay_position,
    plain compact_overlay_device_info,
    plain mouse_mode,
    plain mouse_speed,
    plain mouse_nav_buttons,
    plain native_touchpad,
    plain right_alt_as_meta,
    plain capture_system_keys,
    plain system_keys_fullscreen_only,
    plain controllers,
    plain multi_controller,
    plain swap_face_buttons,
    plain controller_motion,
    plain controller_audio,
    plain back_opens_menu,
    plain back_as_guide,
    plain guide_hold_ms,
    plain gamepad_quit_combo,
    plain stream_menu_buttons,
    plain controller_mouse,
    plain analog_scrolling,
    plain mouse_fixed_step,
    plain gamepad_touchpad_as_mouse,
    plain deadzone,
    plain trigger_deadzone,
    plain disable_rumble,
    plain rumble_feel,
    plain background_controllers,
    plain decoder,
    plain early_decode,
    plain beam_race,
    plain beam_race_margin_ms,
    plain pyrowave_decoder,
    plain renderer,
    plain graphics_card,
    plain display_output,
    plain video_planes,
    plain high_performance_gpu_power,
    plain microphone,
    plain mic_echo_cancel,
    plain mic_gain,
    plain mic_noise_suppression,
    plain mic_auto_gain,
    plain cursor_shapes,
    plain network_grace_ms,
    plain hide_connection_warnings,
    plain client_roaming,
    plain roaming_metered,
    plain receive_coalescing,
    plain jumbo_frames,
    plain internet_play,
    plain usb_forwarding,
    plain usb_auto,
    plain clipboard_sync,
    plain keyboard_follows_text_fields,
    plain dual_screen,
    plain second_screen_width,
    plain second_screen_height,
    plain tell_host_wireless,
    plain tell_host_link_speed,
    plain unbatched_input,
    plain network_priority,
    plain software_slices,
    plain vulkan_video_first,
    plain vulkan_rfi,
    plain videotoolbox_rfi,
    plain pyrowave_async_compute,
    plain auto_reconnect,
    plain auto_reconnect_default_on,
    plain real_vrr_default_on,
    plain kept_settings_checked,
    plain virtual_display,
    plain host_display,
    plain error_correction,
    plain hdr_format,
    plain rtx_hdr,
    plain rtx_video,
    plain link_probe,
    plain sops,
    plain decode_queue_frames,
    plain full_range,
    plain upscaling,
    plain upscale_sharpness,
    plain frame_warp,
    plain open_on_start,
    plain console_home,
    plain simple_smoothness,
    plain simple_picture,
    plain simple_hdr,
    plain quick_resume,
    plain input_only_any_host,
    plain home_layout,
    plain home_group,
    plain home_card_size,
    plain home_sort,
    plain home_order,
    plain settings_more_open,
    plain setup_card_host,
    plain setup_card_seen,
    plain ui_zoom,
    plain ui_zoom_auto_migrated,
    plain high_contrast,
    plain show_debug_settings,
    plain theme,
    plain reduce_motion,
    plain language,
    plain check_updates,
    plain post_stream_decode_time,
    ;
    retired "adaptive_bitrate" => retired_busy_bitrate,
    retired "client_hint_bitrate" => retired_lost_bitrate,
    retired "keep_bitrate" => retired_keep_bitrate,
);
