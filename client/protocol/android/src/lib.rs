//! libnightjar_core.so: the JNI side of `com.limelight.nvstream.jni.RustBridge`, which runs
//! every stream on nightjar-core. `MoonBridge` starts a stream here, or fails the start with
//! the reason; see internal/RUST_CORE.md for what is covered.
//!
//! Each entry point was ported from the C native of the same name in Nightjar's former C
//! engine (`simplejni.c`, `callbacks.c`, `android_control.c`, `android_microphone.c`,
//! `android_present_clock.c`), which built on moonlight-common-c (GPL-3.0, Cameron Gutman and
//! contributors); the golden tests in `client/tests` keep what it recorded.

mod bridge;
pub mod capabilities;
pub mod collections;
pub mod connect;
pub mod display_profile;
pub mod console;
pub mod echo;
mod engine;
pub mod extras;
pub mod files;
mod frame_warp;
pub mod game_info;
pub mod hosts;
pub mod hosts_meta;
pub mod http;
pub mod http_folders;
pub mod http_launch;
pub mod http_probes;
pub mod identity;
pub mod internet;
pub mod jni_input;
pub mod jni_stream;
pub mod library_order;
mod listener;
pub mod mdns;
pub mod native_api;
pub mod open_play;
mod opus;
pub mod pin;
mod platform;
pub mod policy;
pub mod policy_connect;
pub mod policy_input;
pub mod policy_quality;
pub mod policy_recovery;
pub mod policy_roaming;
pub mod policy_video_codec;
pub mod policy_video_pacing;
pub mod policy_video_queue;
mod renderers;
pub mod session_actions;
pub mod sharp_color;
pub mod simple_profile;
mod slab;
pub mod stream_menu;
pub mod stun;
mod touchpad;
pub mod update;
pub mod uri;
mod usb;
pub mod wol;

use std::time::Duration;

use jni::JNIEnv;
use jni::objects::{JByteArray, JClass, JFloatArray, JIntArray, JString};
use jni::sys::{JNI_FALSE, JNI_TRUE, jboolean, jint, jlong, jstring};

use nightjar_core::session::{HostInfo, NetworkChange, SessionConfig};

pub(crate) use bridge::{jbool, string};

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_rustBridgeInit(
    mut env: JNIEnv,
    _: JClass,
    app_version: JString,
    moon_bridge_class: JClass,
) -> jboolean {
    // Requests to the host send `User-Agent: Nightjar/<versionName>`.
    if let Some(version) = string(&mut env, &app_version) {
        nightjar_core::http::set_app_version(&version);
    }
    match bridge::init(&mut env, &moon_bridge_class) {
        Ok(()) => JNI_TRUE,
        Err(e) => {
            let _ = env.exception_clear();
            platform::error(&format!("NightjarJni.rustBridgeInit failed: {e}"));
            JNI_FALSE
        }
    }
}

#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_startConnection(
    mut env: JNIEnv,
    _: JClass,
    address: JString,
    app_version: JString,
    rtsp_session_url: JString,
    server_codec_mode_support: jint,
    width: jint,
    height: jint,
    fps: jint,
    bitrate: jint,
    packet_size: jint,
    streaming_remotely: jint,
    audio_configuration: jint,
    supported_video_formats: jint,
    client_refresh_rate_x100: jint,
    ri_aes_key: JByteArray,
    ri_aes_iv: JByteArray,
    video_capabilities: jint,
    color_space: jint,
    color_range: jint,
    host_features: jint,
    input_only: jboolean,
    enable_mic: jboolean,
    haptics_pcm: jboolean,
    pyrowave_link_mbps: jint,
    pyrowave_pace_mbps: jint,
    client_wireless: jboolean,
    network_grace_ms: jint,
    packed444: jboolean,
    packed444_modes: jint,
    remote_monitor: jboolean,
    unbatched_input: jboolean,
    network_priority: jboolean,
    roaming: jboolean,
    roaming_metered: jboolean,
    audio_passthrough: jint,
    hdr_format: jint,
    video_fec_percentage: jint,
    pyrowave_early: jboolean,
    codec_direct: jboolean,
    receive_decode_priority: jboolean,
    frame_warp_requested: jboolean,
    receive_coalescing: jboolean,
    decode_queue_bound: jint,
) -> jint {
    let Some(bridge) = bridge::get() else {
        platform::error("RustBridge.startConnection before init");
        return -1;
    };
    let input_only = input_only == JNI_TRUE;
    let host = HostInfo {
        address: string(&mut env, &address).unwrap_or_default(),
        app_version: string(&mut env, &app_version).unwrap_or_default(),
        rtsp_session_url: string(&mut env, &rtsp_session_url),
        server_codec_mode_support: if input_only {
            1
        } else {
            server_codec_mode_support as u32
        },
    };
    let Some(ri_key) = bridge::key16(&mut env, &ri_aes_key) else {
        platform::error("startConnection: invalid remote input AES key length");
        return -1;
    };
    let Some(ri_iv) = bridge::key16(&mut env, &ri_aes_iv) else {
        platform::error("startConnection: invalid remote input AES IV length");
        return -1;
    };
    let mut stream = engine::stream_config(
        width,
        height,
        fps,
        bitrate,
        packet_size,
        audio_configuration,
        supported_video_formats,
        client_refresh_rate_x100,
        ri_key,
        ri_iv,
        video_capabilities,
        color_space,
        color_range,
    );
    stream.microphone = enable_mic == JNI_TRUE;
    stream.experimental_packed_444 = packed444 == JNI_TRUE;
    stream.packed444_modes = packed444_modes as u8 & rubylight_protocol::packed444::ALL_MODES;
    stream.haptics_pcm = haptics_pcm == JNI_TRUE;
    stream.pyrowave_records_supported = host_features & 1 != 0;
    stream.pyrowave_link_mbps = pyrowave_link_mbps.max(0);
    stream.pyrowave_pace_mbps = pyrowave_pace_mbps.max(0);
    stream.client_wireless = client_wireless == JNI_TRUE;
    stream.audio_passthrough = extras::audio_passthrough(audio_passthrough);
    stream.hdr_format = hdr_format as u8 & rubylight_protocol::hdr::ALL;
    // Java adds the flag when its Dolby Vision decoder takes the stream full range.
    stream.dolby_vision_full_range =
        hdr_format as u8 & rubylight_protocol::hdr::DV_81_FULL_RANGE != 0;
    stream.video_fec = extras::video_fec(video_fec_percentage);
    let config = SessionConfig {
        stream,
        location: extras::location(streaming_remotely),
        network_grace: Duration::from_millis(network_grace_ms.max(0) as u64),
        input_only,
        remote_monitor: remote_monitor == JNI_TRUE,
        unbatched_input: unbatched_input == JNI_TRUE,
        network_priority: network_priority == JNI_TRUE,
        receive_decode_priority: receive_decode_priority == JNI_TRUE,
        receive_coalescing: receive_coalescing == JNI_TRUE,
        // 0 and anything out of range keep the 15-frame bound (`video::decode_queue_bound`).
        decode_queue_bound: usize::try_from(decode_queue_bound).unwrap_or(0),
        roaming: extras::roaming_config(roaming == JNI_TRUE, roaming_metered == JNI_TRUE),
        // Shifts pictures, so not for a stream without them or one that sends no input.
        frame_warp: (frame_warp_requested == JNI_TRUE && !input_only && remote_monitor != JNI_TRUE)
            .then(|| {
                nightjar_core::frame_warp::FrameWarp::new(
                    Some(frame_warp::CONTROLLER),
                    frame_warp::MASK_MAX_WIDTH,
                )
            }),
    };
    engine::start(
        bridge,
        engine::StartParams {
            host,
            config,
            early_records: pyrowave_early == JNI_TRUE,
            codec_direct: codec_direct == JNI_TRUE,
            // HostCapabilities.NATIVE_CONTROLLER_IDENTITY.
            controller_identity: host_features & 2 != 0,
        },
    )
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_stopConnection(
    _: JNIEnv,
    _: JClass,
) {
    engine::stop();
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_interruptConnection(
    _: JNIEnv,
    _: JClass,
) {
    engine::interrupt();
}

/// The session clock in microseconds, the one the engine stamps packets with: the Vulkan
/// renderer maps its frames' receive times to the local clock with it.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_sessionNowUs(
    _: JNIEnv,
    _: JClass,
) -> jlong {
    nightjar_core::session::now_us() as jlong
}

/// Whether the Rust engine's connection ended with this session's host's disconnect, not a
/// silence.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_hostSaidGoodbye(
    _: JNIEnv,
    _: JClass,
) -> jboolean {
    if engine::with_session(false, |s| s.host_said_goodbye()) {
        JNI_TRUE
    } else {
        JNI_FALSE
    }
}

/// [`nightjar_core::connectivity::test_client_connectivity`] for the connection test dialogs:
/// the flags of the ports that did not answer, 0 for none, -1 (`ML_TEST_RESULT_INCONCLUSIVE`)
/// when the test could not run. Blocks for up to three seconds.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_testClientConnectivity(
    mut env: JNIEnv,
    _: JClass,
    test_server: JString,
    reference_port: jint,
    test_flags: jint,
) -> jint {
    let Some(server) = string(&mut env, &test_server) else {
        return -1;
    };
    nightjar_core::connectivity::test_client_connectivity(
        &server,
        reference_port as u16,
        test_flags as u32,
    ) as jint
}

/// [`nightjar_core::stun::find_external_ipv4`]: this device's WAN IPv4 address as text, or null
/// when the STUN server did not answer in three seconds.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_findExternalAddressIP4(
    mut env: JNIEnv,
    _: JClass,
    stun_host_name: JString,
    stun_port: jint,
) -> jstring {
    let Some(server) = string(&mut env, &stun_host_name) else {
        return std::ptr::null_mut();
    };
    match nightjar_core::stun::find_external_ipv4(
        &server,
        stun_port as u16,
        nightjar_core::stun::STUN_TIMEOUT,
    ) {
        Ok(Some(address)) => env
            .new_string(address.to_string())
            .map_or(std::ptr::null_mut(), |text| text.into_raw()),
        Ok(None) => std::ptr::null_mut(),
        Err(e) => {
            platform::warn(&format!("STUN {server}:{stun_port} failed: {e}"));
            std::ptr::null_mut()
        }
    }
}

/// The address the running session connected to, as text (`HostLossWatch` asks it, never DNS);
/// null without a session.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_getHostAddress(
    env: JNIEnv,
    _: JClass,
) -> jstring {
    match engine::with_session(None, |s| Some(s.remote_address().ip().to_string()))
        .map(|text| env.new_string(text))
    {
        Some(Ok(text)) => text.into_raw(),
        _ => std::ptr::null_mut(),
    }
}

/// The default network changed while a stream runs (`NvConnection`'s network callback):
/// `MoonBridge.NETWORK_CARRIES_ON` (0), `NETWORK_ROAMED` (1) or `NETWORK_CANNOT_CONTINUE` (2).
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_networkChanged(
    _: JNIEnv,
    _: JClass,
    metered: jboolean,
) -> jint {
    network_change_code(engine::network_changed(metered == JNI_TRUE))
}

fn network_change_code(change: NetworkChange) -> jint {
    match change {
        NetworkChange::CarriesOn => 0,
        NetworkChange::Roamed => 1,
        NetworkChange::CannotContinue => 2,
    }
}

/// Whether this stream draws with Frame Warp (`frame_warp::offered`), unless a renderer declined
/// it: a renderer set up again later (a decoder reset) keeps to that.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_frameWarpOffered(
    _: JNIEnv,
    _: JClass,
) -> jboolean {
    jbool(frame_warp::offered() && !frame_warp::declined())
}

/// The address of the C table the native renderers call (`frame_warp_api.h`).
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_frameWarpApi(
    _: JNIEnv,
    _: JClass,
) -> jlong {
    frame_warp::api_address()
}

/// The decoder queued `frame` with presentation time `pts_us`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_frameWarpNoteFrame(
    _: JNIEnv,
    _: JClass,
    pts_us: jlong,
    frame: jint,
) {
    frame_warp::note_frame(pts_us, frame as u32);
}

/// The shift for the picture queued with `pts_us`, drawn now: `out` gets uv x, uv y, px x, px y,
/// the lens terms k.x, k.y, n.x, n.y and the shader row (`frame_warp::WARP_FLOATS` floats), and
/// the return is the HUD mask generation, -1 without a mask or -2 to draw it as it is
/// (also when `out` is shorter).
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_frameWarpForPts(
    env: JNIEnv,
    _: JClass,
    pts_us: jlong,
    out: JFloatArray,
) -> jint {
    let (code, values) = frame_warp::warp_for(frame_warp::frame_for_pts(pts_us));
    if env.get_array_length(&out).unwrap_or(0) < values.len() as i32
        || env.set_float_array_region(&out, 0, &values).is_err()
    {
        return frame_warp::WARP_INACTIVE;
    }
    code
}

/// As `frameWarpForPts`, without counting it in the statistics: whether the GLES presenter
/// should draw the picture on screen again (`frame_warp::peek_for`).
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_frameWarpPeekForPts(
    env: JNIEnv,
    _: JClass,
    pts_us: jlong,
    out: JFloatArray,
) -> jint {
    let (code, values) = frame_warp::peek_for(frame_warp::frame_for_pts(pts_us));
    if env.get_array_length(&out).unwrap_or(0) < values.len() as i32
        || env.set_float_array_region(&out, 0, &values).is_err()
    {
        return frame_warp::WARP_INACTIVE;
    }
    code
}

/// What the GLES presenter checks each refresh before asking for a shift
/// (`frame_warp::input_state`): the count of inputs noted in the low 32 bits, bit 32 while the
/// tracked right stick is deflected.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_frameWarpInputState(
    _: JNIEnv,
    _: JClass,
) -> jlong {
    frame_warp::input_state()
}

/// The HUD mask when its generation is not `known` (-1 for none): `size` gets width and height,
/// and `cells`, when it has room, one byte per cell (0 or 255, rows from the top). Returns the
/// generation, -1 unchanged, -2 cleared, or -3 when `cells` is too small.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_frameWarpMask(
    env: JNIEnv,
    _: JClass,
    known: jint,
    size: JIntArray,
    cells: JByteArray,
) -> jint {
    let warp = frame_warp::current();
    let mask = match frame_warp::mask_update(warp.as_deref(), known) {
        frame_warp::MaskUpdate::Unchanged => return frame_warp::MASK_UNCHANGED,
        frame_warp::MaskUpdate::Cleared => return frame_warp::MASK_CLEARED,
        frame_warp::MaskUpdate::New(mask) => mask,
    };
    if env.get_array_length(&size).unwrap_or(0) < 2
        || env
            .set_int_array_region(&size, 0, &[i32::from(mask.width), i32::from(mask.height)])
            .is_err()
    {
        return frame_warp::MASK_TOO_SMALL;
    }
    let count = usize::from(mask.width) * usize::from(mask.height);
    if (env.get_array_length(&cells).unwrap_or(0) as usize) < count {
        return frame_warp::MASK_TOO_SMALL;
    }
    let mut bytes = vec![0u8; count];
    if !frame_warp::pack_r8(&mask, &mut bytes)
        || env
            .set_byte_array_region(&cells, 0, bridge::jbytes(&bytes))
            .is_err()
    {
        return frame_warp::MASK_TOO_SMALL;
    }
    i32::from(mask.generation)
}

/// The renderer cannot draw Frame Warp for this stream: the host stops its work for it.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_frameWarpDecline(
    _: JNIEnv,
    _: JClass,
) {
    frame_warp::decline();
}

/// The overlay's Frame Warp line: returns `RustBridge.FRAME_WARP_*` and writes gain, lag ms,
/// shift px, peak px, the confidence (-1 before the host's first) and the gyro's lock to `out`
/// (`frame_warp::STATUS_NUMBERS` floats).
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_frameWarpStatus(
    env: JNIEnv,
    _: JClass,
    out: JFloatArray,
) -> jint {
    let mut numbers = [0.0f32; frame_warp::STATUS_NUMBERS];
    let state = frame_warp::status(&mut numbers);
    if env.get_array_length(&out).unwrap_or(0) >= numbers.len() as i32 {
        let _ = env.set_float_array_region(&out, 0, &numbers);
    }
    state
}
