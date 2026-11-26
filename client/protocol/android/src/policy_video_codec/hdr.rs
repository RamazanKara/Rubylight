//! JNI bridge for `NightjarPolicyJni` 10-bit SDR (`Sdr10Policy`), HDR10/HLG (`HdrPolicy`),
//! and extended HDR format (`HdrFormatPolicy`) rules.

use jni::JNIEnv;
use jni::objects::{JByteArray, JClass};
use jni::sys::{JNI_FALSE, jboolean, jint, jstring};
use nightjar_policy::video_codec::hdr::{
    self, ColorTransfer, DrawnSurface, HdrMetadataState, HdrStatus, HlgWindow, Sdr10Refusal,
};

use super::{opt_bytes, opt_jstring};
use crate::policy::jbool;
use crate::policy_recovery::new_jstring;

const fn tri_bool(v: jint) -> Option<bool> {
    if v < 0 { None } else { Some(v != 0) }
}

// --- Sdr10Policy ---

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_sdr10Refusal(
    _: JNIEnv,
    _: JClass,
    media_stream: jboolean,
    enabled: jboolean,
    hdr_granted: jboolean,
    hevc_main10: jboolean,
    av1_main10: jboolean,
    pyro_wave_chosen: jboolean,
    packed444: jboolean,
) -> jint {
    hdr::sdr10_refusal(
        media_stream != JNI_FALSE,
        enabled != JNI_FALSE,
        hdr_granted != JNI_FALSE,
        hevc_main10 != JNI_FALSE,
        av1_main10 != JNI_FALSE,
        pyro_wave_chosen != JNI_FALSE,
        packed444 != JNI_FALSE,
    ) as jint
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_sdr10WorthSaying(
    _: JNIEnv,
    _: JClass,
    refusal_ordinal: jint,
) -> jboolean {
    jbool(hdr::sdr10_worth_saying(Sdr10Refusal::from_ordinal(
        refusal_ordinal,
    )))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_sdr10DecoderChoiceMayChange(
    _: JNIEnv,
    _: JClass,
    enabled: jboolean,
    hdr_requested: jboolean,
    pyro_wave_chosen: jboolean,
    packed444: jboolean,
) -> jboolean {
    jbool(hdr::sdr10_decoder_choice_may_change(
        enabled != JNI_FALSE,
        hdr_requested != JNI_FALSE,
        pyro_wave_chosen != JNI_FALSE,
        packed444 != JNI_FALSE,
    ))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_sdr10Yuv444Wins(
    _: JNIEnv,
    _: JClass,
    common_formats: jint,
) -> jboolean {
    jbool(hdr::sdr10_yuv444_wins(common_formats as u32))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_sdr10WithoutTenBit(
    _: JNIEnv,
    _: JClass,
    offered_formats: jint,
) -> jint {
    hdr::sdr10_without_ten_bit(offered_formats as u32) as jint
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_sdr10HostHdrStep(
    _: JNIEnv,
    _: JClass,
    ten_bit: jboolean,
    host_hdr: jint,
    previous: jint,
) -> jint {
    hdr::sdr10_host_hdr_step(ten_bit != JNI_FALSE, tri_bool(host_hdr), tri_bool(previous)) as jint
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_sdr10Formats(
    _: JNIEnv,
    _: JClass,
    refusal_ordinal: jint,
    hevc_main10: jboolean,
    av1_main10: jboolean,
) -> jint {
    hdr::sdr10_formats(
        Sdr10Refusal::from_ordinal(refusal_ordinal),
        hevc_main10 != JNI_FALSE,
        av1_main10 != JNI_FALSE,
    ) as jint
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_sdr10Status(
    _: JNIEnv,
    _: JClass,
    ten_bit: jboolean,
    host_hdr: jint,
) -> jint {
    hdr::sdr10_status(ten_bit != JNI_FALSE, tri_bool(host_hdr)) as jint
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_sdr10OverlayColor(
    env: JNIEnv,
    _: JClass,
    ten_bit: jboolean,
    sdr10: jboolean,
    host_hdr: jint,
) -> jstring {
    new_jstring(
        &env,
        hdr::sdr10_overlay_color(ten_bit != JNI_FALSE, sdr10 != JNI_FALSE, tri_bool(host_hdr)),
    )
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_sdr10AssumeHdr(
    _: JNIEnv,
    _: JClass,
    ten_bit: jboolean,
    sdr10: jboolean,
) -> jboolean {
    jbool(hdr::sdr10_assume_hdr(
        ten_bit != JNI_FALSE,
        sdr10 != JNI_FALSE,
    ))
}

// --- HdrPolicy ---

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_hdrDisplayRefusal(
    _: JNIEnv,
    _: JClass,
    sdk: jint,
    enabled: jboolean,
    display_mask: jint,
    any_mode_mask: jint,
    host_hdr_formats: jint,
    surface_ordinal: jint,
) -> jint {
    hdr::hdr_display_refusal(
        sdk,
        enabled != JNI_FALSE,
        display_mask as u32,
        any_mode_mask as u32,
        host_hdr_formats as u32,
        DrawnSurface::from_ordinal(surface_ordinal),
    ) as jint
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_hdrShouldForceHlgOnly(
    _: JNIEnv,
    _: JClass,
    debug_hlg_only: jboolean,
    display_mask: jint,
) -> jboolean {
    jbool(hdr::hdr_should_force_hlg_only(
        debug_hlg_only != JNI_FALSE,
        display_mask as u32,
    ))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_hdrHasHdr10(
    _: JNIEnv,
    _: JClass,
    display_mask: jint,
) -> jboolean {
    jbool(hdr::hdr_has_hdr10(display_mask as u32))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_hdrHlgOnly(
    _: JNIEnv,
    _: JClass,
    display_mask: jint,
) -> jboolean {
    jbool(hdr::hdr_hlg_only(display_mask as u32))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_hdrReportsOutput(
    _: JNIEnv,
    _: JClass,
    display_mask: jint,
    stream_transfer_code: jint,
) -> jboolean {
    jbool(hdr::hdr_reports_hdr_output(
        display_mask as u32,
        ColorTransfer::from_code(stream_transfer_code),
    ))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_hdrDecoderRefusal(
    _: JNIEnv,
    _: JClass,
    hevc_main10_hdr10: jboolean,
    av1_main10: jboolean,
    pyro_wave: jboolean,
) -> jint {
    hdr::hdr_decoder_refusal(
        hevc_main10_hdr10 != JNI_FALSE,
        av1_main10 != JNI_FALSE,
        pyro_wave != JNI_FALSE,
    ) as jint
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_hdrMetadata(
    env: JNIEnv,
    _: JClass,
    wire: JByteArray,
) -> jint {
    let bytes = opt_bytes(&env, &wire);
    hdr::hdr_metadata(bytes.as_deref()) as jint
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_hdrDrawnStatus(
    _: JNIEnv,
    _: JClass,
    status_ordinal: jint,
    tone_mapped: jboolean,
) -> jint {
    let Some(status) = HdrStatus::from_ordinal(status_ordinal) else {
        return status_ordinal;
    };
    hdr::hdr_drawn_status(status, tone_mapped != JNI_FALSE) as jint
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_hdrHandsBackFromVulkan(
    _: JNIEnv,
    _: JClass,
    hdr_reported: jboolean,
    tone_mapped: jboolean,
) -> jboolean {
    jbool(hdr::hdr_hands_back_from_vulkan(
        hdr_reported != JNI_FALSE,
        tone_mapped != JNI_FALSE,
    ))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_hdrHandOverAfterVulkanStop(
    _: JNIEnv,
    _: JClass,
    had_renderer: jboolean,
    stop_finished: jboolean,
) -> jboolean {
    jbool(hdr::hdr_hand_over_after_vulkan_stop(
        had_renderer != JNI_FALSE,
        stop_finished != JNI_FALSE,
    ))
}

#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_hdrStatus(
    _: JNIEnv,
    _: JClass,
    asked: jboolean,
    ten_bit: jboolean,
    host_hdr: jint,
    metadata_ordinal: jint,
    output_transfer_code: jint,
    stream_transfer_code: jint,
    hlg_only_screen: jboolean,
) -> jint {
    hdr::hdr_status(
        asked != JNI_FALSE,
        ten_bit != JNI_FALSE,
        tri_bool(host_hdr),
        HdrMetadataState::from_ordinal(metadata_ordinal),
        ColorTransfer::from_code(output_transfer_code),
        ColorTransfer::from_code(stream_transfer_code),
        hlg_only_screen != JNI_FALSE,
    ) as jint
}

// --- HdrFormatPolicy ---

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_hdrFormatHlgWindowOf(
    _: JNIEnv,
    _: JClass,
    call: jint,
) -> jint {
    HlgWindow::of_call(call) as jint
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_hdrFormatPick(
    _: JNIEnv,
    _: JClass,
    host_formats: jint,
    display_mask: jint,
    dolby_vision_decoders: jint,
    video_formats: jint,
    surface_ordinal: jint,
) -> jint {
    hdr::hdr_format_pick(
        host_formats as u32,
        display_mask as u32,
        dolby_vision_decoders as u32,
        video_formats as u32,
        DrawnSurface::from_ordinal(surface_ordinal),
    ) as jint
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_hdrFormatVulkanLimit(
    _: JNIEnv,
    _: JClass,
    host_formats: jint,
    display_mask: jint,
    dolby_vision_decoders: jint,
    video_formats: jint,
) -> jint {
    hdr::hdr_format_vulkan_limit(
        host_formats as u32,
        display_mask as u32,
        dolby_vision_decoders as u32,
        video_formats as u32,
    ) as jint
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_hdrFormatNeedsHlgProbe(
    _: JNIEnv,
    _: JClass,
    pyro_wave: jboolean,
    host_formats: jint,
    display_mask: jint,
    surface_ordinal: jint,
) -> jboolean {
    let needed = if pyro_wave != JNI_FALSE {
        hdr::hdr_format_pyrowave_needs_hlg_probe(host_formats as u32, display_mask as u32)
    } else {
        hdr::hdr_format_vulkan_needs_hlg_probe(
            host_formats as u32,
            display_mask as u32,
            DrawnSurface::from_ordinal(surface_ordinal),
        )
    };
    jbool(needed)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_hdrFormatMask(
    _: JNIEnv,
    _: JClass,
    host_formats: jint,
    choice_ordinal: jint,
) -> jint {
    hdr::HdrFormatChoice::from_ordinal(choice_ordinal).mask(host_formats)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_hdrFormatSurface(
    _: JNIEnv,
    _: JClass,
    op: jint,
    host_formats: jint,
    surface_ordinal: jint,
    window_ordinal: jint,
) -> jint {
    let window = HlgWindow::from_ordinal(window_ordinal).unwrap_or(HlgWindow::Refused);
    match op {
        0 => hdr::hdr_format_vulkan_surface(DrawnSurface::from_ordinal(surface_ordinal), window)
            as jint,
        1 => hdr::hdr_format_pyrowave_surface(host_formats as u32) as jint,
        _ => hdr::hdr_format_pyrowave_surface_with_window(host_formats as u32, window) as jint,
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_hdrFormatPassesHdr10Plus(
    _: JNIEnv,
    _: JClass,
    display_mask: jint,
    transfer_code: jint,
) -> jboolean {
    jbool(hdr::hdr_format_passes_hdr10_plus(
        display_mask as u32,
        ColorTransfer::from_code(transfer_code),
    ))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_hdrFormatPyroWaveHlgRefusedByDisplay(
    _: JNIEnv,
    _: JClass,
    host_formats: jint,
    display_mask: jint,
    window_ordinal: jint,
) -> jboolean {
    let window = HlgWindow::from_ordinal(window_ordinal).unwrap_or(HlgWindow::Refused);
    jbool(hdr::hdr_format_pyrowave_hlg_refused_by_display(
        host_formats as u32,
        display_mask as u32,
        window,
    ))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_hdrFormatPyroWaveHlgLost(
    _: JNIEnv,
    _: JClass,
    announced_format: jint,
    window_ordinal: jint,
    window_kept_hlg: jboolean,
    frames_stopped: jboolean,
) -> jboolean {
    jbool(hdr::hdr_format_pyrowave_hlg_lost(
        announced_format as u32,
        HlgWindow::from_ordinal(window_ordinal),
        window_kept_hlg != JNI_FALSE,
        frames_stopped != JNI_FALSE,
    ))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_hdrFormatPyroWaveLimit(
    _: JNIEnv,
    _: JClass,
    host_formats: jint,
    display_mask: jint,
    dolby_vision_decoders: jint,
    format: jint,
) -> jint {
    hdr::hdr_format_pyrowave_limit(
        host_formats as u32,
        display_mask as u32,
        dolby_vision_decoders as u32,
        format as u32,
    ) as jint
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_hdrFormatPyroWaveHlgNeedsHostUpdate(
    _: JNIEnv,
    _: JClass,
    host_formats: jint,
    display_mask: jint,
) -> jboolean {
    jbool(hdr::hdr_format_pyrowave_hlg_needs_host_update(
        host_formats as u32,
        display_mask as u32,
    ))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_hdrFormatHlgInstead(
    _: JNIEnv,
    _: JClass,
    host_formats: jint,
    display_mask: jint,
) -> jboolean {
    jbool(hdr::hdr_format_hlg_instead(
        host_formats as u32,
        display_mask as u32,
    ))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_hdrFormatDolbyVision(
    _: JNIEnv,
    _: JClass,
    format: jint,
) -> jboolean {
    jbool(hdr::hdr_format_dolby_vision(format as u32))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_hdrFormatCarriers(
    _: JNIEnv,
    _: JClass,
    format: jint,
) -> jint {
    hdr::hdr_format_carriers(format as u32) as jint
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_hdrFormatVideoFormats(
    _: JNIEnv,
    _: JClass,
    format: jint,
    video_formats: jint,
    automatic_codec: jboolean,
) -> jint {
    hdr::hdr_format_video_formats(
        format as u32,
        video_formats as u32,
        automatic_codec != JNI_FALSE,
    ) as jint
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_hdrFormatTransferIsHlg(
    _: JNIEnv,
    _: JClass,
    format: jint,
) -> jboolean {
    jbool(hdr::hdr_format_transfer_is_hlg(format as u32))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_hdrFormatText(
    env: JNIEnv,
    _: JClass,
    op: jint,
    format: jint,
    dolby_vision_decoding: jboolean,
    hdr10_plus: jboolean,
) -> jstring {
    let text = match op {
        0 => hdr::hdr_format_overlay_word(
            format as u32,
            dolby_vision_decoding != JNI_FALSE,
            hdr10_plus != JNI_FALSE,
        ),
        1 => hdr::hdr_format_base_layer_name(format as u32).to_owned(),
        2 => hdr::hdr_format_name(format as u32).to_owned(),
        _ => hdr::hdr_format_codec_names(format as u32).to_owned(),
    };
    opt_jstring(&env, Some(text))
}
