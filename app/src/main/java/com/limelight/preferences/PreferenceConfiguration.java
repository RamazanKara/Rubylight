package com.limelight.preferences;

import android.content.Context;
import android.content.SharedPreferences;
import android.content.pm.PackageManager;
import android.os.Build;
import android.preference.PreferenceManager;
import android.hardware.display.DisplayManager;
import android.view.Display;

import com.limelight.binding.input.touch.PointerAcceleration;
import com.limelight.binding.input.touch.FingerTapGesture;
import com.limelight.binding.input.touch.TouchMode;
import com.limelight.binding.input.touch.TouchMomentum;
import com.limelight.binding.input.touch.TrackpadArea;
import com.limelight.computers.HostCapabilityCache;
import com.limelight.nvstream.http.FecChoice;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.binding.video.DisplayFrameRatePolicy;
import com.limelight.binding.video.PerformanceOverlay;
import com.limelight.binding.video.UpscalingPolicy;
import com.limelight.binding.video.VideoRendererChoice;
import com.limelight.ui.StreamOrientation;
import com.limelight.ui.VideoScaling;

public class PreferenceConfiguration {
    public enum FormatOption {
        AUTO,
        FORCE_AV1,
        FORCE_HEVC,
        FORCE_H264,
        FORCE_PYROWAVE,
    };

    public enum AnalogStickForScrolling {
        NONE,
        RIGHT,
        LEFT
    }

    private static final String LEGACY_RES_FPS_PREF_STRING = "list_resolution_fps";
    private static final String LEGACY_ENABLE_51_SURROUND_PREF_STRING = "checkbox_51_surround";

    static final String RESOLUTION_PREF_STRING = "list_resolution";
    static final String FPS_PREF_STRING = "list_fps";
    static final String BITRATE_PREF_STRING = "seekbar_bitrate_kbps";
    // Set only when the user picks a bitrate; defaults keep following resolution and frame rate.
    static final String BITRATE_USER_SET_PREF_STRING = "bitrate_user_set";
    static final String MOBILE_BITRATE_PREF_STRING = "list_mobile_bitrate_kbps";
    // Removed settings: the slider is the ceiling and the built-in floor is the minimum.
    static final String[] REMOVED_AUTO_BITRATE_LIMIT_PREFS = {"list_auto_bitrate_min_kbps", "list_auto_bitrate_max_kbps"};
    static final String PREFS_VERSION_PREF_STRING = "prefs_version";
    static final int NATIVE_DEFAULTS_VERSION = 2;
    // From version 3 "native" is the screen a stream starts on, not a size saved earlier.
    static final int FOLLOW_SCREEN_VERSION = 3;
    // From version 4 a PC profile stuck at 1920x1080 follows the screen too.
    static final int PROFILE_NATIVE_VERSION = 4;
    static final String RESOLUTION_FOLLOWS_SCREEN_PREF_STRING = "resolution_follows_screen";
    static final String BITRATE_PREF_OLD_STRING = "seekbar_bitrate";
    private static final String STRETCH_PREF_STRING = VideoScaling.STRETCH_PREF;
    private static final String SOPS_PREF_STRING = "checkbox_enable_sops";
    private static final String DISABLE_TOASTS_PREF_STRING = "checkbox_disable_warnings";
    private static final String HOST_AUDIO_PREF_STRING = "checkbox_host_audio";
    public static final String ENABLE_MIC_PREF_STRING = "checkbox_enable_mic";
    private static final String DEADZONE_PREF_STRING = "seekbar_deadzone";
    private static final String OSC_OPACITY_PREF_STRING = "seekbar_osc_opacity";
    static final String LANGUAGE_PREF_STRING = "list_languages";
    private static final String SMALL_ICONS_PREF_STRING = "checkbox_small_icon_mode";
    private static final String MULTI_CONTROLLER_PREF_STRING = "checkbox_multi_controller";
    static final String AUDIO_CONFIG_PREF_STRING = "list_audio_config";
    private static final String USB_DRIVER_PREF_SRING = "checkbox_usb_driver";
    private static final String USB_DUALSENSE_PREF_STRING = "checkbox_usb_dualsense";
    private static final String USB_SWITCH_PRO_PREF_STRING = "checkbox_usb_switch_pro";
    static final String VIDEO_FORMAT_PREF_STRING = "video_format";
    private static final String ONSCREEN_CONTROLLER_PREF_STRING = "checkbox_show_onscreen_controls";
    private static final String ONLY_L3_R3_PREF_STRING = "checkbox_only_show_L3R3";
    private static final String SHOW_GUIDE_BUTTON_PREF_STRING = "checkbox_show_guide_button";
    private static final String LEGACY_DISABLE_FRAME_DROP_PREF_STRING = "checkbox_disable_frame_drop";
    private static final String ENABLE_HDR_PREF_STRING = "checkbox_enable_hdr";
    private static final String ENABLE_SDR10_PREF_STRING = "checkbox_enable_sdr_10bit";
    private static final String ENABLE_PIP_PREF_STRING = "checkbox_enable_pip";
    private static final String AUTO_RECONNECT_PREF_STRING = "checkbox_auto_reconnect";
    private static final String NETWORK_GRACE_PREF_STRING = "list_network_grace";
    private static final String TELL_HOST_WIRELESS_PREF_STRING = "checkbox_tell_host_wireless";
    private static final String TELL_HOST_LINK_SPEED_PREF_STRING = "checkbox_tell_host_link_speed";
    static final String FEC_CHOICE_PREF_STRING = "list_fec_choice";
    static final String LINK_PROBE_PREF_STRING = "checkbox_link_probe";
    private static final String QUICK_RESUME_PREF_STRING = "checkbox_quick_resume";
    private static final String AUTO_RECONNECT_DEFAULT_OFF_PREF_STRING = "auto_reconnect_default_off";
    private static final String ENABLE_PERF_OVERLAY_STRING = "checkbox_enable_perf_overlay";
    private static final String BIND_ALL_USB_STRING = "checkbox_usb_bind_all";
    private static final String MOUSE_EMULATION_STRING = "checkbox_mouse_emulation";
    private static final String MOUSE_EMULATION_FIXED_STEP_STRING = "checkbox_mouse_emulation_fixed_step";
    private static final String DISABLE_RUMBLE_PREF_STRING = "checkbox_disable_rumble";
    private static final String BACK_AS_META_PREF_STRING = "checkbox_back_as_meta";
    private static final String BACK_OPENS_MENU_PREF_STRING = "checkbox_back_opens_menu";
    private static final String RIGHT_ALT_AS_META_PREF_STRING = "checkbox_right_alt_as_meta";
    private static final String ANALOG_SCROLLING_PREF_STRING = "analog_scrolling";
    private static final String MOUSE_NAV_BUTTONS_STRING = "checkbox_mouse_nav_buttons";
    static final String INPUT_ONLY_ANY_HOST_PREF_STRING = "checkbox_input_only_any_host";
    static final String UNLOCK_FPS_STRING = "checkbox_unlock_fps";
    private static final String VIBRATE_OSC_PREF_STRING = "checkbox_vibrate_osc";
    private static final String VIBRATE_FALLBACK_PREF_STRING = "checkbox_vibrate_fallback";
    private static final String VIBRATE_FALLBACK_STRENGTH_PREF_STRING = "seekbar_vibrate_fallback_strength";
    private static final String FLIP_FACE_BUTTONS_PREF_STRING = "checkbox_flip_face_buttons";
    private static final String TOUCHSCREEN_TRACKPAD_PREF_STRING = "checkbox_touchscreen_trackpad";
    private static final String TRACKPAD_SPEED_PREF_STRING = "seekbar_trackpad_speed";
    private static final String LOCAL_CURSOR_SPEED_PREF_STRING = "seekbar_local_cursor_speed";
    private static final String TRACKPAD_ACCELERATION_PREF_STRING = "checkbox_trackpad_acceleration";
    private static final String TRACKPAD_ACCELERATION_CURVE_PREF_STRING = "list_trackpad_acceleration_curve";
    private static final String TRACKPAD_MOMENTUM_PREF_STRING = "checkbox_trackpad_momentum";
    private static final String TRACKPAD_MOMENTUM_GLIDE_PREF_STRING = "list_trackpad_momentum_glide";
    private static final String TRACKPAD_AREA_PREF_STRING = "list_trackpad_area";
    private static final String THREE_FINGER_TAP_PREF_STRING = "list_three_finger_tap";
    private static final String FOUR_FINGER_TAP_PREF_STRING = "list_four_finger_tap";
    private static final String PC_KEYBOARD_LAYOUT_PREF_STRING = "list_pc_keyboard_layout";
    private static final String PC_KEYBOARD_SIZE_PREF_STRING = "seekbar_pc_keyboard_size";
    private static final String PC_KEYBOARD_OPACITY_PREF_STRING = "seekbar_pc_keyboard_opacity";
    private static final String MOUSE_SPEED_PREF_STRING = "seekbar_mouse_speed";
    private static final String LATENCY_TOAST_PREF_STRING = "checkbox_enable_post_stream_toast";
    // Off: streams use the C engine. The Rust engine is a beta and falls back per stream.
    static final String RUST_ENGINE_PREF_STRING = "checkbox_rust_engine";
    private static final String PACKED_444_PREF_STRING = "experimental_packed_444";
    private static final String CHIPSET_TUNING_PREF_STRING = "checkbox_experimental_chipset_tuning";
    static final String FRAME_PACING_PREF_STRING = "frame_pacing";
    private static final String ABSOLUTE_MOUSE_MODE_PREF_STRING = "checkbox_absolute_mouse_mode";
    private static final String ENABLE_AUDIO_FX_PREF_STRING = "checkbox_enable_audiofx";
    static final String AUDIO_PASSTHROUGH_PREF_STRING = "checkbox_audio_passthrough";
    private static final String REDUCE_REFRESH_RATE_PREF_STRING = "checkbox_reduce_refresh_rate";
    private static final String FULL_RANGE_PREF_STRING = "checkbox_full_range";
    private static final String GAMEPAD_TOUCHPAD_AS_MOUSE_PREF_STRING = "checkbox_gamepad_touchpad_as_mouse";
    private static final String GAMEPAD_MOTION_SENSORS_PREF_STRING = "checkbox_gamepad_motion_sensors";
    private static final String GAMEPAD_MOTION_FALLBACK_PREF_STRING = "checkbox_gamepad_motion_fallback";
    private static final String CODEC_LOW_LATENCY_PREF_STRING = "checkbox_codec_low_latency";
    private static final String VENDOR_LOW_LATENCY_PREF_STRING = "checkbox_vendor_low_latency";
    public static final String DISABLE_HEVC_LOW_LATENCY_PREF_STRING = "checkbox_disable_hevc_low_latency";
    private static final String CODEC_PERFORMANCE_PREF_STRING = "checkbox_codec_performance";
    private static final String PHONE_PERFORMANCE_HINTS_PREF_STRING = "checkbox_phone_performance_hints";
    static final String GPU_MAX_CLOCKS_PREF_STRING = "checkbox_gpu_max_clocks";
    // Opt-in: holding the GPU at full speed costs battery. Installs that stored a value keep it.
    static final boolean DEFAULT_GPU_MAX_CLOCKS = false;
    private static final String DROP_LATE_FRAMES_PREF_STRING = "checkbox_drop_late_frames";
    private static final String TEXTURE_VIEW_PREF_STRING = "checkbox_texture_view";
    private static final String UNBATCHED_INPUT_PREF_STRING = "checkbox_unbatched_input";
    private static final String NETWORK_PRIORITY_PREF_STRING = "checkbox_network_priority";
    // Client roaming (Experimental, Rust engine): on by default; onto mobile data only by choice.
    static final String CLIENT_ROAMING_PREF_STRING = "checkbox_client_roaming";
    static final String ROAMING_METERED_PREF_STRING = "checkbox_roaming_metered";
    private static final String VIRTUAL_DISPLAY_PREF_STRING = "checkbox_virtual_display";
    private static final String VIRTUAL_DISPLAY_SCALE_PREF_STRING = "seekbar_virtual_display_scale";
    private static final String YUV444_PREF_STRING = "checkbox_yuv444";
    private static final String VRR_PREF_STRING = "checkbox_vrr";
    static final String HOST_FRAME_TIMES_PREF_STRING = "checkbox_host_frame_times";
    static final String CONTENT_RATE_FRAME_RATE_PREF_STRING = "checkbox_content_rate_frame_rate";
    static final String UPSCALING_PREF_STRING = "upscaling_mode";
    static final String UPSCALING_SHARPNESS_PREF_STRING = "upscaling_sharpness";
    // "Render through OpenGL" is gone. Its key is only read once, to move its users to Vulkan, and
    // must not be reused for anything else.
    static final String RETIRED_GL_RENDER_PREF_STRING = "checkbox_gl_render";
    static final String VULKAN_RENDER_PREF_STRING = "checkbox_vulkan_render";

    static final String DEFAULT_RESOLUTION = "1280x720";
    static final String DEFAULT_FPS = "60";
    private static final boolean DEFAULT_STRETCH = false;
    private static final boolean DEFAULT_SOPS = true;
    private static final boolean DEFAULT_DISABLE_TOASTS = false;
    private static final boolean DEFAULT_HOST_AUDIO = false;
    private static final boolean DEFAULT_ENABLE_MIC = false;
    private static final int DEFAULT_DEADZONE = 7;
    private static final int DEFAULT_OPACITY = 90;
    public static final String DEFAULT_LANGUAGE = "default";
    private static final boolean DEFAULT_MULTI_CONTROLLER = true;
    private static final boolean DEFAULT_USB_DRIVER = true;
    static final String DEFAULT_VIDEO_FORMAT = "auto";

    private static final boolean ONSCREEN_CONTROLLER_DEFAULT = false;
    private static final boolean ONLY_L3_R3_DEFAULT = false;
    private static final boolean SHOW_GUIDE_BUTTON_DEFAULT = true;
    private static final boolean DEFAULT_ENABLE_HDR = false;
    private static final boolean DEFAULT_ENABLE_SDR10 = false;
    private static final boolean DEFAULT_ENABLE_PIP = false;
    private static final boolean DEFAULT_ENABLE_PERF_OVERLAY = false;
    private static final boolean DEFAULT_BIND_ALL_USB = false;
    private static final boolean DEFAULT_MOUSE_EMULATION = true;
    private static final String DEFAULT_ANALOG_STICK_FOR_SCROLLING = "right";
    private static final boolean DEFAULT_MOUSE_NAV_BUTTONS = false;
    private static final boolean DEFAULT_INPUT_ONLY_ANY_HOST = false;
    private static final boolean DEFAULT_UNLOCK_FPS = false;
    private static final boolean DEFAULT_VIBRATE_OSC = true;
    private static final boolean DEFAULT_VIBRATE_FALLBACK = false;
    private static final int DEFAULT_VIBRATE_FALLBACK_STRENGTH = 100;
    private static final boolean DEFAULT_FLIP_FACE_BUTTONS = false;
    private static final boolean DEFAULT_TOUCHSCREEN_TRACKPAD = true;
    private static final int DEFAULT_TRACKPAD_SPEED = 100;
    private static final int DEFAULT_MOUSE_SPEED = 100;
    private static final String DEFAULT_AUDIO_CONFIG = "2"; // Stereo
    private static final boolean DEFAULT_LATENCY_TOAST = false;
    static final String DEFAULT_FRAME_PACING = "latency";
    private static final boolean DEFAULT_ABSOLUTE_MOUSE_MODE = false;
    private static final boolean DEFAULT_ENABLE_AUDIO_FX = false;
    private static final boolean DEFAULT_REDUCE_REFRESH_RATE = false;
    private static final boolean DEFAULT_FULL_RANGE = false;
    private static final boolean DEFAULT_GAMEPAD_TOUCHPAD_AS_MOUSE = false;
    private static final boolean DEFAULT_GAMEPAD_MOTION_SENSORS = true;
    private static final boolean DEFAULT_GAMEPAD_MOTION_FALLBACK = false;

    public static final int FRAME_PACING_MIN_LATENCY = 0;
    public static final int FRAME_PACING_BALANCED = 1;
    public static final int FRAME_PACING_CAP_FPS = 2;
    public static final int FRAME_PACING_MAX_SMOOTHNESS = 3;

    public static final String RES_360P = "640x360";
    public static final String RES_480P = "854x480";
    public static final String RES_720P = "1280x720";
    public static final String RES_1080P = "1920x1080";
    public static final String RES_1440P = "2560x1440";
    public static final String RES_4K = "3840x2160";
    public static final String RES_NATIVE = "Native";

    public int width, height, fps;
    public int launchRefreshRateX100;
    public int bitrate;
    // The bitrate limit on mobile data in kbps; 0 for none.
    public int mobileBitrateLimit;
    public FormatOption videoFormat;
    public int deadzonePercentage;
    public int oscOpacity;
    // stretchVideo mirrors videoScaling == STRETCH for the renderers; change both with setVideoScaling.
    public boolean stretchVideo, enableSops, playHostAudio, disableWarnings;
    public int videoScaling = VideoScaling.FIT, videoAlignment = VideoScaling.CENTER;
    public int streamOrientation = StreamOrientation.AUTO;
    // Send this device's microphone to the host when the host offers to take it.
    public boolean enableMic;
    public String language;
    public boolean smallIconMode, multiController, usbDriver, usbDualSense, usbDualSenseHaptics, usbSwitchPro, flipFaceButtons;
    public boolean onscreenController;
    public boolean onlyL3R3;
    public boolean showGuideButton;
    public boolean enableHdr;
    /** Ask for 10-bit SDR when HDR is not streamed (see Sdr10Policy). Global only: not part of a PC's profile. */
    public boolean enableSdr10Bit;
    public boolean enablePip;
    public boolean autoReconnect;
    /** Seconds a stream waits for a silent network; 0 is off (see NetworkGrace). */
    public int networkGraceSeconds;
    /** Tell a Rubylight PC when the route to it is Wi-Fi or cellular. On unless switched off. */
    public boolean tellHostWireless = true;
    public boolean tellHostLinkSpeed = true;
    /** The video FEC asked for at launch from a PC that lets the client choose it; the PC's own by default. */
    public FecChoice fecChoice = FecChoice.HOST;
    /**
     * The bitrate is the default for the stream's size and rate, set by no profile, PC or game:
     * a new stream may test the link first and start lower (the mobile data limit still caps it).
     */
    public boolean automaticBitrate;
    /**
     * Link test before the first bitrate (Experimental). Off by default: on the NUC it started
     * streams lower with more loss and freezes (rust/PERFORMANCE.md, October 27).
     */
    public boolean linkProbe;
    public boolean quickResume;
    public com.limelight.nvstream.AdaptiveQualityController.Mode adaptiveQualityMode =
            com.limelight.nvstream.AdaptiveQualityController.Mode.OFF;
    public int qualityMinShortEdge = 720, qualityMinFps = 30;
    /** Automatic bitrate for PCs without a choice of their own; see {@link AutoBitrateChoice}. */
    /**
     * A snapshot of the Settings switch, read for the settings-keys test. Production code must use
     * {@link AutoBitrateChoice#isOn}, which also honours a PC's own choice.
     */
    public boolean autoBitrate;
    public volatile boolean enablePerfOverlay;
    /** The corner the performance overlay prefers: top_left, top_right, bottom_left or bottom_right. */
    public volatile String overlayPosition = "top_left";
    public boolean enableLatencyToast;
    public boolean rustEngine;
    public boolean experimentalPacked444;
    // Opt-in decoder keys still being measured (docs/devices.md, Candidate per-chip changes)
    public boolean experimentalChipsetTuning;
    public boolean bindAllUsb;
    public boolean mouseEmulation;
    public boolean mouseEmulationFixedStep;
    public boolean disableRumble;
    public boolean backAsMeta;
    public boolean backOpensMenu;
    public boolean rightAltAsMeta;
    public AnalogStickForScrolling analogStickForScrolling;
    public boolean mouseNavButtons;
    /** Offer Input-only over a small stream on PCs whose host has no Input-only mode. */
    public boolean inputOnlyAnyHost;
    public boolean unlockFps;
    public boolean vibrateOsc;
    public boolean vibrateFallbackToDevice;
    public int vibrateFallbackToDeviceStrength;
    public boolean touchscreenTrackpad;
    public int touchMode;
    public int trackpadSpeed, mouseSpeed, localCursorSpeed;
    public boolean trackpadAcceleration, trackpadMomentum;
    public String trackpadAccelerationCurve, trackpadMomentumGlide, trackpadArea, threeFingerTap;
    /** The on-screen PC keyboard: layout id or "auto", its share of the window height and its opacity, both in percent. */
    public String pcKeyboardLayout;
    public int pcKeyboardSize, pcKeyboardOpacity;
    public String fourFingerTap = FingerTapGesture.NONE;
    public MoonBridge.AudioConfiguration audioConfiguration;
    public int framePacing;
    public boolean absoluteMouseMode;
    public boolean enableAudioFx;
    // Ask a Rubylight PC for PCM, AC3 or E-AC3 the audio output plays (AudioPassthrough); Rust engine only.
    public boolean audioPassthrough;
    public boolean reduceRefreshRate;
    public boolean fullRange;
    public boolean gamepadMotionSensors;
    public boolean gamepadTouchpadAsMouse;
    public boolean gamepadMotionSensorsFallbackToDevice;
    public boolean codecLowLatency, vendorLowLatency, codecPerformance;
    public boolean disableHevcLowLatency;
    public boolean phonePerformanceHints;
    public boolean gpuMaxClocks;
    public boolean dropLateFrames, useTextureView, unbatchedInput, networkPriority;
    public boolean clientRoaming, roamingMetered;
    public boolean virtualDisplay, enableYuv444, vrr;
    // Show frames at the host's frame times (HostFrameClock); replaces frame pacing.
    public boolean hostFrameTimes;
    // Vote the content rate the host's frame times show to the display (Surface.setFrameRate).
    public boolean contentRateFrameRate;
    public int virtualDisplayScale;
    // Where the launch values came from, for the stream log.
    public boolean resolutionFollowsScreen, fromPcProfile;
    // What this PC's displays do during the stream, and the PC's own stream values (for the log).
    public HostDisplayChoice hostDisplay;
    public String pcSettings;
    // The layers this stream's values came from (named profile, PC, game); null for the main settings alone.
    public StreamLayers.Result layers;
    // The same, as the overlay shows it ("Settings: profile Hotel + this PC"); null for the main settings alone.
    public String layersText;
    // PyroWave was chosen, but this PC has shown it has none; videoFormat holds the codec used instead.
    public boolean pyroWaveUnavailable;
    public UpscalingPolicy.Mode upscalingMode = UpscalingPolicy.Mode.OFF;
    public int upscalingSharpness = 50;
    // Present MediaCodec frames through the GLES path even when nothing is upscaled.
    // Present MediaCodec frames through the native Vulkan renderer, which paces them itself.
    public boolean vulkanRender;

    /** A new stream tests the link before its first bitrate: switched on, and the bitrate is automatic. */
    public boolean probesFirstBitrate() {
        return linkProbe && automaticBitrate;
    }

    public static boolean isNativeResolution(int width, int height) {
        // It's not a native resolution if it matches an existing resolution option
        if (width == 640 && height == 360) {
            return false;
        }
        else if (width == 854 && height == 480) {
            return false;
        }
        else if (width == 1280 && height == 720) {
            return false;
        }
        else if (width == 1920 && height == 1080) {
            return false;
        }
        else if (width == 2560 && height == 1440) {
            return false;
        }
        else if (width == 3840 && height == 2160) {
            return false;
        }

        return true;
    }

    // If we have a screen that has semi-square dimensions, we may want to change our behavior
    // to allow any orientation and vertical+horizontal resolutions.
    public static boolean isSquarishScreen(int width, int height) {
        float longDim = Math.max(width, height);
        float shortDim = Math.min(width, height);

        // We just put the arbitrary cutoff for a square-ish screen at 1.3
        return longDim / shortDim < 1.3f;
    }

    public static boolean isSquarishScreen(Display display) {
        int width, height;

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) {
            width = display.getMode().getPhysicalWidth();
            height = display.getMode().getPhysicalHeight();
        }
        else {
            width = display.getWidth();
            height = display.getHeight();
        }

        return isSquarishScreen(width, height);
    }

    private static String convertFromLegacyResolutionString(String resString) {
        if (resString.equalsIgnoreCase("360p")) {
            return RES_360P;
        }
        else if (resString.equalsIgnoreCase("480p")) {
            return RES_480P;
        }
        else if (resString.equalsIgnoreCase("720p")) {
            return RES_720P;
        }
        else if (resString.equalsIgnoreCase("1080p")) {
            return RES_1080P;
        }
        else if (resString.equalsIgnoreCase("1440p")) {
            return RES_1440P;
        }
        else if (resString.equalsIgnoreCase("4K")) {
            return RES_4K;
        }
        else {
            // Should be unreachable
            return RES_720P;
        }
    }

    static int getWidthFromResolutionString(String resString) {
        return Integer.parseInt(resString.split("x")[0]);
    }

    static int getHeightFromResolutionString(String resString) {
        return Integer.parseInt(resString.split("x")[1]);
    }

    static String getResolutionText(int width, int height) {
        return width + "x" + height;
    }

    private static String getResolutionString(int width, int height) {
        switch (height) {
            case 360:
                return RES_360P;
            case 480:
                return RES_480P;
            default:
            case 720:
                return RES_720P;
            case 1080:
                return RES_1080P;
            case 1440:
                return RES_1440P;
            case 2160:
                return RES_4K;
        }
    }

    public static int getDefaultBitrate(String resString, String fpsString) {
        int width = getWidthFromResolutionString(resString);
        int height = getHeightFromResolutionString(resString);
        int fps = Integer.parseInt(fpsString);

        // This logic is shamelessly stolen from Moonlight Qt:
        // https://github.com/moonlight-stream/moonlight-qt/blob/master/app/settings/streamingpreferences.cpp

        // Don't scale bitrate linearly beyond 60 FPS. It's definitely not a linear
        // bitrate increase for frame rate once we get to values that high.
        double frameRateFactor = (fps <= 60 ? fps : (Math.sqrt(fps / 60.f) * 60.f)) / 30.f;

        // TODO: Collect some empirical data to see if these defaults make sense.
        // We're just using the values that the Shield used, as we have for years.
        int[] pixelVals = {
            640 * 360,
            854 * 480,
            1280 * 720,
            1920 * 1080,
            2560 * 1440,
            3840 * 2160,
            -1,
        };
        int[] factorVals = {
            1,
            2,
            5,
            10,
            20,
            40,
            -1
        };

        // Calculate the resolution factor by linear interpolation of the resolution table
        float resolutionFactor;
        int pixels = width * height;
        for (int i = 0; ; i++) {
            if (pixels == pixelVals[i]) {
                // We can bail immediately for exact matches
                resolutionFactor = factorVals[i];
                break;
            }
            else if (pixels < pixelVals[i]) {
                if (i == 0) {
                    // Never go below the lowest resolution entry
                    resolutionFactor = factorVals[i];
                }
                else {
                    // Interpolate between the entry greater than the chosen resolution (i) and the entry less than the chosen resolution (i-1)
                    resolutionFactor = ((float)(pixels - pixelVals[i-1]) / (pixelVals[i] - pixelVals[i-1])) * (factorVals[i] - factorVals[i-1]) + factorVals[i-1];
                }
                break;
            }
            else if (pixelVals[i] == -1) {
                // Never go above the highest resolution entry
                resolutionFactor = factorVals[i-1];
                break;
            }
        }

        return (int)Math.round(resolutionFactor * frameRateFactor) * 1000;
    }

    public static boolean getDefaultSmallMode(Context context) {
        PackageManager manager = context.getPackageManager();
        if (manager != null) {
            // TVs shouldn't use small mode by default
            if (manager.hasSystemFeature(PackageManager.FEATURE_TELEVISION)) {
                return false;
            }

            // API 21 uses LEANBACK instead of TELEVISION
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.LOLLIPOP_MR1) {
                if (manager.hasSystemFeature(PackageManager.FEATURE_LEANBACK)) {
                    return false;
                }
            }
        }

        // Use small mode on anything smaller than a 7" tablet
        return context.getResources().getConfiguration().smallestScreenWidthDp < 500;
    }

    // Defaults written by the native-defaults migration, a preset or a resolution change are not
    // the user's choice. Installs from before the flag count as user-set only when their saved
    // bitrate differs from the default for their saved resolution and frame rate.
    static boolean isBitrateUserSet(SharedPreferences prefs) {
        if (prefs.contains(BITRATE_USER_SET_PREF_STRING)) {
            return prefs.getBoolean(BITRATE_USER_SET_PREF_STRING, false);
        }
        return prefs.contains(BITRATE_PREF_STRING) && prefs.getInt(BITRATE_PREF_STRING, 0) != getDefaultBitrate(
                prefs.getString(RESOLUTION_PREF_STRING, DEFAULT_RESOLUTION), prefs.getString(FPS_PREF_STRING, DEFAULT_FPS));
    }

    public static int getDefaultBitrate(Context context) {
        SharedPreferences prefs = PreferenceManager.getDefaultSharedPreferences(context);
        return getDefaultBitrate(
                prefs.getString(RESOLUTION_PREF_STRING, DEFAULT_RESOLUTION),
                prefs.getString(FPS_PREF_STRING, DEFAULT_FPS));
    }

    private static FormatOption getVideoFormatValue(Context context) {
        SharedPreferences prefs = PreferenceManager.getDefaultSharedPreferences(context);

        String str = prefs.getString(VIDEO_FORMAT_PREF_STRING, DEFAULT_VIDEO_FORMAT);
        if (str.equals("auto")) {
            return FormatOption.AUTO;
        }
        else if (str.equals("forceav1")) {
            return FormatOption.FORCE_AV1;
        }
        else if (str.equals("forceh265")) {
            return FormatOption.FORCE_HEVC;
        }
        else if (str.equals("neverh265")) {
            return FormatOption.FORCE_H264;
        }
        else if (str.equals("forcepyrowave")) {
            return FormatOption.FORCE_PYROWAVE;
        }
        else {
            // Should never get here
            return FormatOption.AUTO;
        }
    }

    private static int getFramePacingValue(Context context) {
        SharedPreferences prefs = PreferenceManager.getDefaultSharedPreferences(context);

        // Migrate legacy never drop frames option to the new location
        if (prefs.contains(LEGACY_DISABLE_FRAME_DROP_PREF_STRING)) {
            boolean legacyNeverDropFrames = prefs.getBoolean(LEGACY_DISABLE_FRAME_DROP_PREF_STRING, false);
            prefs.edit()
                    .remove(LEGACY_DISABLE_FRAME_DROP_PREF_STRING)
                    .putString(FRAME_PACING_PREF_STRING, legacyNeverDropFrames ? "balanced" : "latency")
                    .apply();
        }

        return framePacingValue(prefs.getString(FRAME_PACING_PREF_STRING, DEFAULT_FRAME_PACING));
    }

    static int mobileBitrateLimit(String stored) {
        try {
            int kbps = Integer.parseInt(stored.trim());
            return kbps < 500 ? 0 : Math.min(kbps, HostStreamProfile.MAX_BITRATE_KBPS);
        } catch (NumberFormatException e) {
            return 0;
        }
    }

    static int framePacingValue(String str) {
        if (str.equals("latency")) {
            return FRAME_PACING_MIN_LATENCY;
        }
        else if (str.equals("balanced")) {
            return FRAME_PACING_BALANCED;
        }
        else if (str.equals("cap-fps")) {
            return FRAME_PACING_CAP_FPS;
        }
        else if (str.equals("smoothness")) {
            return FRAME_PACING_MAX_SMOOTHNESS;
        }
        else {
            // Should never get here
            return FRAME_PACING_MIN_LATENCY;
        }
    }

    private static AnalogStickForScrolling getAnalogStickForScrollingValue(Context context) {
        SharedPreferences prefs = PreferenceManager.getDefaultSharedPreferences(context);

        String str = prefs.getString(ANALOG_SCROLLING_PREF_STRING, DEFAULT_ANALOG_STICK_FOR_SCROLLING);
        if (str.equals("right")) {
            return AnalogStickForScrolling.RIGHT;
        }
        else if (str.equals("left")) {
            return AnalogStickForScrolling.LEFT;
        }
        else {
            return AnalogStickForScrolling.NONE;
        }
    }

    public static void resetStreamingSettings(Context context) {
        String crashHostUuid = context.getSharedPreferences("DecoderTombstone", Context.MODE_PRIVATE)
                .getString("CrashHostUuid", null);
        if (crashHostUuid != null) {
            HostStreamProfile.reset(context, crashHostUuid);
        }
        // We consider resolution, FPS, bitrate, HDR, and video format as "streaming settings" here
        SharedPreferences prefs = PreferenceManager.getDefaultSharedPreferences(context);
        prefs.edit()
                .remove(BITRATE_PREF_STRING)
                .remove(BITRATE_USER_SET_PREF_STRING)
                .remove(BITRATE_PREF_OLD_STRING)
                .remove(LEGACY_RES_FPS_PREF_STRING)
                .remove(RESOLUTION_PREF_STRING)
                .remove(FPS_PREF_STRING)
                .remove(VIDEO_FORMAT_PREF_STRING)
                .remove(ENABLE_HDR_PREF_STRING)
                .remove(ENABLE_SDR10_PREF_STRING)
                .remove(UNLOCK_FPS_STRING)
                .remove(FULL_RANGE_PREF_STRING)
                .apply();
    }

    public static void completeLanguagePreferenceMigration(Context context) {
        // Put our language option back to default which tells us that we've already migrated it
        SharedPreferences prefs = PreferenceManager.getDefaultSharedPreferences(context);
        prefs.edit().putString(LANGUAGE_PREF_STRING, DEFAULT_LANGUAGE).apply();
    }

    public void setVideoScaling(int mode) {
        videoScaling = mode;
        stretchVideo = mode == VideoScaling.STRETCH;
    }

    public static boolean isShieldAtvFirmwareWithBrokenHdr() {
        // This particular Shield TV firmware crashes when using HDR
        // https://www.nvidia.com/en-us/geforce/forums/notifications/comment/155192/
        return Build.MANUFACTURER.equalsIgnoreCase("NVIDIA") &&
                Build.FINGERPRINT.contains("PPR1.180610.011/4079208_2235.1395");
    }

    private static final String PC_PROFILES_OVERRIDES_PREF_STRING = "pc_profiles_overrides";

    public static PreferenceConfiguration readPreferences(Context context, String hostUuid) {
        return readPreferences(context, hostUuid, HostCapabilityCache.load(context, hostUuid));
    }

    /** The settings for a stream from this PC, given what the PC last showed it can do. */
    public static PreferenceConfiguration readPreferences(Context context, String hostUuid, HostCapabilities caps) {
        return readPreferences(context, hostUuid, caps, null, 0, StreamLayers.Network.OTHER);
    }

    /** What the layers below the main settings read from: profiles, this PC's and this game's values. */
    static StreamLayers.Input layerInput(Context context, PreferenceConfiguration main, String hostUuid,
                                         String appUuid, int appId) {
        StreamLayers.Input input = new StreamLayers.Input();
        input.profiles = StreamProfiles.load(context);
        input.pcValues = HostStreamProfile.raw(context, hostUuid);
        input.gameValues = AppStreamSettings.raw(context, hostUuid, appUuid, appId);
        input.mobileLimitKbps = main.mobileBitrateLimit;
        input.mainBitrate = main.bitrate;
        return input;
    }

    /**
     * The settings for a stream of a game from this PC: the main settings, then a named profile,
     * this PC's values and this game's values (see {@link StreamLayers}). The network picks the
     * profile when the player asked for that, and decides the mobile data limit; a null
     * {@code network} reads the network now.
     */
    public static PreferenceConfiguration readPreferences(Context context, String hostUuid, HostCapabilities caps,
                                                          String appUuid, int appId, StreamLayers.Network network) {
        PreferenceConfiguration config = readPreferences(context);
        FormatOption allPcsFormat = config.videoFormat;
        migratePcProfilesToOverrides(context);
        config.hostDisplay = hostUuid == null ? null : HostDisplayChoice.load(context, hostUuid);
        StreamLayers.Input input = layerInput(context, config, hostUuid, appUuid, appId);
        if (network == null) {
            StreamNetwork.fill(context, input);
        } else {
            input.network = network;
        }
        StreamLayers.Result layers = StreamLayers.resolve(input);
        config.layers = layers;
        config.automaticBitrate &= !layers.bitrateLayered;
        if (layers.layered()) {
            HostStreamProfile profile = layers.values;
            boolean followedScreen = config.resolutionFollowsScreen;
            profile.applyTo(config);
            StreamLayers.Source resolution = layers.source(HostStreamProfile.Field.RESOLUTION);
            config.fromPcProfile = resolution == StreamLayers.Source.PC || resolution == StreamLayers.Source.GAME;
            if (config.resolutionFollowsScreen && !followedScreen) {
                String nativeResolution = nativeResolution(context);
                if (nativeResolution != null) {
                    config.width = getWidthFromResolutionString(nativeResolution);
                    config.height = getHeightFromResolutionString(nativeResolution);
                }
            }
            config.pcSettings = layers.pcApplied ? input.pcValues : null;
            SharedPreferences prefs = PreferenceManager.getDefaultSharedPreferences(context);
            config.framePacing = config.vrr ? FRAME_PACING_MIN_LATENCY :
                    profile.pacing != null ? framePacingValue(profile.pacing) : getFramePacingValue(context);
            config.reduceRefreshRate = !config.vrr && prefs.getBoolean(REDUCE_REFRESH_RATE_PREF_STRING, DEFAULT_REDUCE_REFRESH_RATE);
            if (config.hostFrameTimes) config.framePacing = FRAME_PACING_MIN_LATENCY;
            config.enableHdr &= !isShieldAtvFirmwareWithBrokenHdr();
            config.useTextureView = prefs.getBoolean(TEXTURE_VIEW_PREF_STRING, false) && !config.enableHdr && !config.enableSdr10Bit &&
                    config.videoFormat != FormatOption.FORCE_PYROWAVE;
        }
        // Forcing PyroWave on a PC without it would still cost TextureView and keep HDR looking
        // available. Use the setting for all PCs instead, or let the device choose. A PC never
        // seen yet keeps PyroWave, and the stream negotiation decides.
        if (config.videoFormat == FormatOption.FORCE_PYROWAVE && !caps.mayHave(HostCapabilities.Feature.PYROWAVE)) {
            config.pyroWaveUnavailable = true;
            config.videoFormat = codecWithoutPyroWave(allPcsFormat);
            config.useTextureView = PreferenceManager.getDefaultSharedPreferences(context)
                    .getBoolean(TEXTURE_VIEW_PREF_STRING, false) && !config.enableHdr && !config.enableSdr10Bit;
        }
        config.contentRateFrameRate &= !config.vrr && !config.useTextureView && config.reduceRefreshRate;
        return config;
    }

    /** The codec for all PCs, or the device's own choice when that is PyroWave too. */
    public static FormatOption codecWithoutPyroWave(FormatOption allPcsFormat) {
        return allPcsFormat == FormatOption.FORCE_PYROWAVE ? FormatOption.AUTO : allPcsFormat;
    }

    // PC profiles saved before test.11 copied every global setting, so later changes to the global
    // settings never reached those PCs. Once, keep only the values that differ from the global ones.
    static void migratePcProfilesToOverrides(Context context) {
        SharedPreferences prefs = PreferenceManager.getDefaultSharedPreferences(context);
        if (prefs.getBoolean(PC_PROFILES_OVERRIDES_PREF_STRING, false)) {
            return;
        }
        int changed = HostStreamProfile.migrateAllToOverrides(context, readPreferences(context));
        if (changed > 0) {
            com.limelight.LimeLog.info("PC stream profiles now keep only their own values: " + changed);
        }
        prefs.edit().putBoolean(PC_PROFILES_OVERRIDES_PREF_STRING, true).apply();
    }

    // The built-in screen in use at its full size, landscape first; null when the screen can't be read.
    public static String nativeResolution(Context context) {
        Display display = defaultDisplay(context);
        if (display == null) {
            return null;
        }
        android.graphics.Point real = new android.graphics.Point();
        display.getRealSize(real);
        int[] size;
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) {
            Display.Mode[] supported = display.getSupportedModes();
            int[][] modes = new int[supported.length][];
            for (int i = 0; i < supported.length; i++) {
                modes[i] = new int[] {supported[i].getPhysicalWidth(), supported[i].getPhysicalHeight()};
            }
            size = panelSize(real.x, real.y, modes);
        } else {
            size = new int[] {real.x, real.y};
        }
        if (size == null || size[0] <= 0 || size[1] <= 0) {
            return null;
        }
        return Math.max(size[0], size[1]) + "x" + Math.min(size[0], size[1]);
    }

    /**
     * The largest mode in the shape of the screen in use ({@code realWidth} x {@code realHeight}).
     * A foldable such as the Galaxy Z Fold can list the modes of both panels on one display: the
     * largest is the near-square inner panel, which would pillarbox a stream on the 21:9 cover
     * screen. A panel running at a scaled resolution keeps its own, larger mode. Without a mode in
     * that shape (the size can describe a window), the largest mode, as before.
     */
    static int[] panelSize(int realWidth, int realHeight, int[][] modes) {
        int[] largest = null, matching = null;
        for (int[] mode : modes) {
            if (mode[0] <= 0 || mode[1] <= 0) continue;
            if (largest == null || area(mode) > area(largest)) largest = mode;
            if (sameShape(mode[0], mode[1], realWidth, realHeight, 0.01) && (matching == null || area(mode) > area(matching))) {
                matching = mode;
            }
        }
        return matching != null ? matching : largest;
    }

    private static long area(int[] size) {
        return (long) size[0] * size[1];
    }

    /** Whether a width x height stream has the shape of the screen in use, within {@code tolerance}. */
    public static boolean hasPanelShape(Context context, int width, int height, double tolerance) {
        String panel = nativeResolution(context);
        return panel != null && sameShape(width, height, getWidthFromResolutionString(panel),
                getHeightFromResolutionString(panel), tolerance);
    }

    /** Long-to-short side ratios within {@code tolerance} (0.01 is 1%) of each other, either way round. */
    static boolean sameShape(int width, int height, int otherWidth, int otherHeight, double tolerance) {
        if (width <= 0 || height <= 0 || otherWidth <= 0 || otherHeight <= 0) return false;
        double ratio = (double) Math.max(width, height) / Math.min(width, height);
        double other = (double) Math.max(otherWidth, otherHeight) / Math.min(otherWidth, otherHeight);
        return Math.abs(ratio - other) <= other * tolerance;
    }

    public static float panelMaxRefreshRate(Context context) {
        Display display = defaultDisplay(context);
        return display == null ? 60 : DisplayFrameRatePolicy.maxRefreshRate(display);
    }

    private static Display defaultDisplay(Context context) {
        DisplayManager displays = (DisplayManager) context.getSystemService(Context.DISPLAY_SERVICE);
        return displays == null ? null : displays.getDisplay(Display.DEFAULT_DISPLAY);
    }

    // Older installs hold the XML defaults written on first launch (720p60, latency switches off).
    // Move them once to the phone's native resolution and refresh rate with every latency switch on.
    // Codec, HDR, 4:4:4 and VRR choices stay as they are.
    static boolean migrateToNativeDefaults(SharedPreferences prefs, String nativeResolution, float panelMaxHz) {
        int version = prefs.getInt(PREFS_VERSION_PREF_STRING, 0);
        if (nativeResolution == null || version >= FOLLOW_SCREEN_VERSION) {
            return false;
        }
        SharedPreferences.Editor editor = prefs.edit();
        if (version < NATIVE_DEFAULTS_VERSION) {
            String fps = Integer.toString(DisplayFrameRatePolicy.streamFrameRate(panelMaxHz));
            editor.putString(RESOLUTION_PREF_STRING, nativeResolution)
                    .putString(FPS_PREF_STRING, fps)
                    .putInt(BITRATE_PREF_STRING, getDefaultBitrate(nativeResolution, fps))
                    .putBoolean(BITRATE_USER_SET_PREF_STRING, false)
                    .remove(BITRATE_PREF_OLD_STRING)
                    .putString(FRAME_PACING_PREF_STRING, DEFAULT_FRAME_PACING)
                    .putString(UPSCALING_PREF_STRING, "off");
            for (String key : StreamPreset.LATENCY_SWITCHES) {
                editor.putBoolean(key, true);
            }
        }
        // A size saved on another panel (a foldable's cover screen, say) no longer sticks: streams
        // start at the native size of the screen they start on.
        editor.putBoolean(RESOLUTION_FOLLOWS_SCREEN_PREF_STRING, true)
                .putInt(PREFS_VERSION_PREF_STRING, FOLLOW_SCREEN_VERSION);
        editor.apply();
        return true;
    }

    // The resolution to stream at: this screen's native size while the resolution follows the
    // screen, otherwise the saved one.
    static String effectiveResolution(SharedPreferences prefs, String saved, String nativeResolution) {
        return prefs.getBoolean(RESOLUTION_FOLLOWS_SCREEN_PREF_STRING, false) && nativeResolution != null ?
                nativeResolution : saved;
    }

    public static void migrateToNativeDefaults(Context context) {
        SharedPreferences prefs = PreferenceManager.getDefaultSharedPreferences(context);
        migrateToNativeDefaults(prefs, nativeResolution(context), panelMaxRefreshRate(context));
        if (prefs.getInt(PREFS_VERSION_PREF_STRING, 0) == FOLLOW_SCREEN_VERSION) {
            int changed = HostStreamProfile.migrateAllToNative(context);
            if (changed > 0) {
                com.limelight.LimeLog.info("PC stream profiles switched from 1920x1080 to native: " + changed);
            }
            prefs.edit().putInt(PREFS_VERSION_PREF_STRING, PROFILE_NATIVE_VERSION).apply();
        }
    }

    // Settings › Advanced › Low-latency decoding sets these together; the individual switches follow it.
    static final String[] LOW_LATENCY_DECODING_KEYS = {
            CODEC_LOW_LATENCY_PREF_STRING, VENDOR_LOW_LATENCY_PREF_STRING, PHONE_PERFORMANCE_HINTS_PREF_STRING,
    };

    /** How many of the low-latency decoding switches are on; each one defaults to on. */
    static int lowLatencyDecodingCount(SharedPreferences prefs) {
        int on = 0;
        for (String key : LOW_LATENCY_DECODING_KEYS) {
            if (prefs.getBoolean(key, true)) on++;
        }
        return on;
    }

    static void setLowLatencyDecoding(SharedPreferences prefs, boolean enabled) {
        SharedPreferences.Editor editor = prefs.edit();
        for (String key : LOW_LATENCY_DECODING_KEYS) {
            editor.putBoolean(key, enabled);
        }
        editor.apply();
    }

    // Stream recovery used to start on, and first launch stored that default like a choice. It now
    // starts off; a stored "on" is switched off once because it cannot be told apart from the old
    // default. Turning it back on afterwards is kept.
    static boolean migrateAutoReconnectDefault(SharedPreferences prefs) {
        if (prefs.getBoolean(AUTO_RECONNECT_DEFAULT_OFF_PREF_STRING, false)) {
            return false;
        }
        boolean wasOn = prefs.getBoolean(AUTO_RECONNECT_PREF_STRING, false);
        SharedPreferences.Editor editor = prefs.edit().putBoolean(AUTO_RECONNECT_DEFAULT_OFF_PREF_STRING, true);
        if (wasOn) {
            editor.putBoolean(AUTO_RECONNECT_PREF_STRING, false);
        }
        editor.apply();
        return wasOn;
    }

    // Set once the new streaming engine became the default, so a later choice of the classic engine is kept.
    static final String RUST_ENGINE_DEFAULT_ON_PREF_STRING = "rust_engine_default_on";

    // The new streaming engine used to start off, and first launch stored that default like a choice.
    // It now starts on; a stored "off" is switched on once because it cannot be told apart from the old
    // default. Choosing the classic engine afterwards is kept.
    static boolean migrateRustEngineDefault(SharedPreferences prefs) {
        if (prefs.getBoolean(RUST_ENGINE_DEFAULT_ON_PREF_STRING, false)) {
            return false;
        }
        boolean wasOff = prefs.contains(RUST_ENGINE_PREF_STRING) && !prefs.getBoolean(RUST_ENGINE_PREF_STRING, false);
        SharedPreferences.Editor editor = prefs.edit().putBoolean(RUST_ENGINE_DEFAULT_ON_PREF_STRING, true);
        if (wasOff) {
            editor.putBoolean(RUST_ENGINE_PREF_STRING, true);
        }
        editor.apply();
        return wasOff;
    }

    // For the overlay and the log: where this stream's resolution comes from.
    public String resolutionSource() {
        return resolutionFollowsScreen ? (fromPcProfile ? "native, PC profile" : "native") :
                (fromPcProfile ? "fixed, PC profile" : "fixed in settings");
    }

    static final String OVERLAY_MODE_PREF_STRING = "performance_overlay_mode";
    static final String OVERLAY_POSITION_PREF_STRING = "performance_overlay_position";
    // Set once the compact overlay became the default, so a later choice of Statistics is kept.
    static final String OVERLAY_COMPACT_DEFAULT_PREF_STRING = "performance_overlay_compact_default";

    /**
     * Compact is the overlay's default. Statistics was the default every install stored, even untouched,
     * so a stored Statistics cannot be told from a choice: it becomes compact once, like no value at all.
     * Debug and older names keep their meaning.
     */
    static void migrateOverlayMode(SharedPreferences prefs) {
        String stored = prefs.getString(OVERLAY_MODE_PREF_STRING, null);
        String mode = PerformanceOverlay.normalizeMode(stored);
        SharedPreferences.Editor editor = null;
        if (!prefs.getBoolean(OVERLAY_COMPACT_DEFAULT_PREF_STRING, false)) {
            if (stored == null || PerformanceOverlay.MODE_STATISTICS.equals(stored)) {
                mode = PerformanceOverlay.MODE_COMPACT;
            }
            editor = prefs.edit().putBoolean(OVERLAY_COMPACT_DEFAULT_PREF_STRING, true);
        }
        if (!mode.equals(stored)) {
            editor = (editor != null ? editor : prefs.edit()).putString(OVERLAY_MODE_PREF_STRING, mode);
        }
        if (editor != null) editor.apply();
    }

    /**
     * "Render through OpenGL" is removed. A user who had it on gets "Render through Vulkan", the
     * setting that replaced it for frame-generation overlays, only if that can take its place:
     * they never chose a value for it, no upscaler is selected (Vulkan would override it) and the
     * device runs Android 10 or later. The old key is dropped either way.
     *
     * @return whether Vulkan was switched on
     */
    static boolean migrateRetiredGlRender(SharedPreferences prefs, int sdkInt) {
        if (!prefs.contains(RETIRED_GL_RENDER_PREF_STRING)) return false;
        boolean wasOn = prefs.getBoolean(RETIRED_GL_RENDER_PREF_STRING, false);
        boolean chosen = prefs.contains(VULKAN_RENDER_PREF_STRING);
        boolean upscaling = UpscalingPolicy.Mode.fromPreference(
                prefs.getString(UPSCALING_PREF_STRING, "off")) != UpscalingPolicy.Mode.OFF;
        boolean supported = sdkInt >= VideoRendererChoice.MIN_SDK;
        boolean switchOn = wasOn && !chosen && !upscaling && supported;
        SharedPreferences.Editor editor = prefs.edit().remove(RETIRED_GL_RENDER_PREF_STRING);
        if (switchOn) editor.putBoolean(VULKAN_RENDER_PREF_STRING, true);
        editor.apply();
        if (wasOn && !switchOn) {
            com.limelight.LimeLog.info("\"Render through OpenGL\" was on and is gone; Vulkan stays off (" +
                    (chosen ? "a Vulkan choice exists" : upscaling ? "an upscaler is selected" :
                            "Android " + sdkInt + " is too old") + ")");
        }
        return switchOn;
    }

    /** Automatic bitrate's minimum and maximum are gone; their saved values must not linger. */
    static void dropAutoBitrateLimits(SharedPreferences prefs) {
        SharedPreferences.Editor editor = null;
        for (String key : REMOVED_AUTO_BITRATE_LIMIT_PREFS) {
            if (prefs.contains(key)) editor = (editor != null ? editor : prefs.edit()).remove(key);
        }
        if (editor != null) editor.apply();
    }

    public static PreferenceConfiguration readPreferences(Context context) {
        SharedPreferences prefs = PreferenceManager.getDefaultSharedPreferences(context);
        migrateToNativeDefaults(context);
        if (migrateAutoReconnectDefault(prefs)) {
            com.limelight.LimeLog.info("Stream recovery switched to its new default: off");
        }
        PreferenceConfiguration config = new PreferenceConfiguration();
        String savedResolution = null;

        // Migrate legacy preferences to the new locations
        migrateOverlayMode(prefs);
        if (migrateRetiredGlRender(prefs, Build.VERSION.SDK_INT)) {
            com.limelight.LimeLog.info("\"Render through OpenGL\" was on; it is now \"Render through Vulkan\"");
        }
        if (migrateRustEngineDefault(prefs)) {
            com.limelight.LimeLog.info("New streaming engine switched to its new default: on");
        }
        if (prefs.contains(LEGACY_ENABLE_51_SURROUND_PREF_STRING)) {
            if (prefs.getBoolean(LEGACY_ENABLE_51_SURROUND_PREF_STRING, false)) {
                prefs.edit()
                        .remove(LEGACY_ENABLE_51_SURROUND_PREF_STRING)
                        .putString(AUDIO_CONFIG_PREF_STRING, "51")
                        .apply();
            }
        }

        String str = prefs.getString(LEGACY_RES_FPS_PREF_STRING, null);
        if (str != null) {
            if (str.equals("360p30")) {
                config.width = 640;
                config.height = 360;
                config.fps = 30;
            }
            else if (str.equals("360p60")) {
                config.width = 640;
                config.height = 360;
                config.fps = 60;
            }
            else if (str.equals("720p30")) {
                config.width = 1280;
                config.height = 720;
                config.fps = 30;
            }
            else if (str.equals("720p60")) {
                config.width = 1280;
                config.height = 720;
                config.fps = 60;
            }
            else if (str.equals("1080p30")) {
                config.width = 1920;
                config.height = 1080;
                config.fps = 30;
            }
            else if (str.equals("1080p60")) {
                config.width = 1920;
                config.height = 1080;
                config.fps = 60;
            }
            else if (str.equals("4K30")) {
                config.width = 3840;
                config.height = 2160;
                config.fps = 30;
            }
            else if (str.equals("4K60")) {
                config.width = 3840;
                config.height = 2160;
                config.fps = 60;
            }
            else {
                // Should never get here
                config.width = 1280;
                config.height = 720;
                config.fps = 60;
            }

            prefs.edit()
                    .remove(LEGACY_RES_FPS_PREF_STRING)
                    .putString(RESOLUTION_PREF_STRING, getResolutionString(config.width, config.height))
                    .putString(FPS_PREF_STRING, ""+config.fps)
                    .apply();
        }
        else {
            // Use the new preference location
            String resStr = prefs.getString(RESOLUTION_PREF_STRING, PreferenceConfiguration.DEFAULT_RESOLUTION);

            // Convert legacy resolution strings to the new style
            if (!resStr.contains("x")) {
                resStr = PreferenceConfiguration.convertFromLegacyResolutionString(resStr);
                prefs.edit().putString(RESOLUTION_PREF_STRING, resStr).apply();
            }

            savedResolution = resStr;
            resStr = effectiveResolution(prefs, resStr, nativeResolution(context));
            config.resolutionFollowsScreen = !resStr.equals(savedResolution) ||
                    prefs.getBoolean(RESOLUTION_FOLLOWS_SCREEN_PREF_STRING, false);
            config.width = PreferenceConfiguration.getWidthFromResolutionString(resStr);
            config.height = PreferenceConfiguration.getHeightFromResolutionString(resStr);
            config.fps = Integer.parseInt(prefs.getString(FPS_PREF_STRING, PreferenceConfiguration.DEFAULT_FPS));
        }

        if (!prefs.contains(SMALL_ICONS_PREF_STRING)) {
            // We need to write small icon mode's default to disk for the settings page to display
            // the current state of the option properly
            prefs.edit().putBoolean(SMALL_ICONS_PREF_STRING, getDefaultSmallMode(context)).apply();
        }

        if (!prefs.contains(GAMEPAD_MOTION_SENSORS_PREF_STRING) && Build.VERSION.SDK_INT == Build.VERSION_CODES.S) {
            // Android 12 has a nasty bug that causes crashes when the app touches the InputDevice's
            // associated InputDeviceSensorManager (just calling getSensorManager() is enough).
            // As a workaround, we will override the default value for the gamepad motion sensor
            // option to disabled on Android 12 to reduce the impact of this bug.
            // https://cs.android.com/android/_/android/platform/frameworks/base/+/8970010a5e9f3dc5c069f56b4147552accfcbbeb
            prefs.edit().putBoolean(GAMEPAD_MOTION_SENSORS_PREF_STRING, false).apply();
        }

        // This must happen after the preferences migration to ensure the preferences are populated
        config.bitrate = prefs.getInt(BITRATE_PREF_STRING, prefs.getInt(BITRATE_PREF_OLD_STRING, 0) * 1000);
        if (config.bitrate == 0) {
            config.bitrate = getDefaultBitrate(context);
        }
        // A bitrate the user did not pick follows the size the stream really uses.
        String fpsString = Integer.toString(config.fps);
        String used = getResolutionText(config.width, config.height);
        config.automaticBitrate = !isBitrateUserSet(prefs);
        if (savedResolution != null && !used.equals(savedResolution) && config.automaticBitrate) {
            config.bitrate = getDefaultBitrate(used, fpsString);
        }

        String audioConfig = prefs.getString(AUDIO_CONFIG_PREF_STRING, DEFAULT_AUDIO_CONFIG);
        if (audioConfig.equals("71")) {
            config.audioConfiguration = MoonBridge.AUDIO_CONFIGURATION_71_SURROUND;
        }
        else if (audioConfig.equals("51")) {
            config.audioConfiguration = MoonBridge.AUDIO_CONFIGURATION_51_SURROUND;
        }
        else /* if (audioConfig.equals("2")) */ {
            config.audioConfiguration = MoonBridge.AUDIO_CONFIGURATION_STEREO;
        }

        config.mobileBitrateLimit = mobileBitrateLimit(prefs.getString(MOBILE_BITRATE_PREF_STRING, "0"));
        config.videoFormat = getVideoFormatValue(context);
        config.framePacing = getFramePacingValue(context);

        config.analogStickForScrolling = getAnalogStickForScrollingValue(context);

        config.deadzonePercentage = prefs.getInt(DEADZONE_PREF_STRING, DEFAULT_DEADZONE);

        config.oscOpacity = prefs.getInt(OSC_OPACITY_PREF_STRING, DEFAULT_OPACITY);

        config.language = prefs.getString(LANGUAGE_PREF_STRING, DEFAULT_LANGUAGE);

        // Checkbox preferences
        config.disableWarnings = prefs.getBoolean(DISABLE_TOASTS_PREF_STRING, DEFAULT_DISABLE_TOASTS);
        config.enableSops = prefs.getBoolean(SOPS_PREF_STRING, DEFAULT_SOPS);
        config.setVideoScaling(VideoScaling.resolve(prefs.getString(VideoScaling.PREF, null),
                prefs.getBoolean(STRETCH_PREF_STRING, DEFAULT_STRETCH)));
        config.videoAlignment = VideoScaling.alignment(prefs.getString(VideoScaling.ALIGNMENT_PREF, null));
        config.streamOrientation = StreamOrientation.fromValue(
                prefs.getString(StreamOrientation.PREF, null));
        config.playHostAudio = prefs.getBoolean(HOST_AUDIO_PREF_STRING, DEFAULT_HOST_AUDIO);
        config.enableMic = prefs.getBoolean(ENABLE_MIC_PREF_STRING, DEFAULT_ENABLE_MIC);
        config.smallIconMode = prefs.getBoolean(SMALL_ICONS_PREF_STRING, getDefaultSmallMode(context));
        config.multiController = prefs.getBoolean(MULTI_CONTROLLER_PREF_STRING, DEFAULT_MULTI_CONTROLLER);
        config.usbDriver = prefs.getBoolean(USB_DRIVER_PREF_SRING, DEFAULT_USB_DRIVER);
        config.usbDualSense = prefs.getBoolean(USB_DUALSENSE_PREF_STRING, false);
        config.usbDualSenseHaptics = config.usbDualSense && prefs.getBoolean("checkbox_usb_dualsense_haptics", false);
        config.usbSwitchPro = prefs.getBoolean(USB_SWITCH_PRO_PREF_STRING, false);
        config.onscreenController = prefs.getBoolean(ONSCREEN_CONTROLLER_PREF_STRING, ONSCREEN_CONTROLLER_DEFAULT);
        config.onlyL3R3 = prefs.getBoolean(ONLY_L3_R3_PREF_STRING, ONLY_L3_R3_DEFAULT);
        config.showGuideButton = prefs.getBoolean(SHOW_GUIDE_BUTTON_PREF_STRING, SHOW_GUIDE_BUTTON_DEFAULT);
        config.enableHdr = prefs.getBoolean(ENABLE_HDR_PREF_STRING, DEFAULT_ENABLE_HDR) && !isShieldAtvFirmwareWithBrokenHdr();
        config.enableSdr10Bit = prefs.getBoolean(ENABLE_SDR10_PREF_STRING, DEFAULT_ENABLE_SDR10);
        config.enablePip = prefs.getBoolean(ENABLE_PIP_PREF_STRING, DEFAULT_ENABLE_PIP);
        config.autoReconnect = prefs.getBoolean(AUTO_RECONNECT_PREF_STRING, false);
        config.networkGraceSeconds = com.limelight.utils.NetworkGrace.parseSeconds(
                prefs.getString(NETWORK_GRACE_PREF_STRING, "0"));
        config.tellHostWireless = prefs.getBoolean(TELL_HOST_WIRELESS_PREF_STRING, true);
        config.tellHostLinkSpeed = prefs.getBoolean(TELL_HOST_LINK_SPEED_PREF_STRING, true);
        config.fecChoice = FecChoice.fromPreference(prefs.getString(FEC_CHOICE_PREF_STRING, FecChoice.HOST.wireValue));
        config.linkProbe = prefs.getBoolean(LINK_PROBE_PREF_STRING, false);
        config.quickResume = prefs.getBoolean(QUICK_RESUME_PREF_STRING, false);
        config.adaptiveQualityMode = com.limelight.nvstream.AdaptiveQualityController.Mode.parse(
                prefs.getString("adaptive_quality_mode", "off"));
        String qualityFloor = prefs.getString("adaptive_quality_min_resolution", "720");
        config.qualityMinShortEdge = "360".equals(qualityFloor) ? 360 : "540".equals(qualityFloor) ? 540 :
                "1080".equals(qualityFloor) ? 1080 : 720;
        String qualityFps = prefs.getString("adaptive_quality_min_fps", "30");
        config.qualityMinFps = "45".equals(qualityFps) ? 45 : "60".equals(qualityFps) ? 60 : 30;
        dropAutoBitrateLimits(prefs);
        config.autoBitrate = prefs.getBoolean(AutoBitrateChoice.SETTINGS_KEY, false);
        config.enablePerfOverlay = prefs.getBoolean(ENABLE_PERF_OVERLAY_STRING, DEFAULT_ENABLE_PERF_OVERLAY);
        config.overlayPosition = com.limelight.ui.OverlayPlacement.Corner.parse(
                prefs.getString(OVERLAY_POSITION_PREF_STRING, null)).key;
        config.bindAllUsb = prefs.getBoolean(BIND_ALL_USB_STRING, DEFAULT_BIND_ALL_USB);
        config.mouseEmulation = prefs.getBoolean(MOUSE_EMULATION_STRING, DEFAULT_MOUSE_EMULATION);
        config.mouseEmulationFixedStep = prefs.getBoolean(MOUSE_EMULATION_FIXED_STEP_STRING, false);
        config.disableRumble = prefs.getBoolean(DISABLE_RUMBLE_PREF_STRING, false);
        config.backAsMeta = prefs.getBoolean(BACK_AS_META_PREF_STRING, false);
        config.backOpensMenu = prefs.getBoolean(BACK_OPENS_MENU_PREF_STRING, false);
        config.rightAltAsMeta = prefs.getBoolean(RIGHT_ALT_AS_META_PREF_STRING, false);
        config.mouseNavButtons = prefs.getBoolean(MOUSE_NAV_BUTTONS_STRING, DEFAULT_MOUSE_NAV_BUTTONS);
        config.inputOnlyAnyHost = prefs.getBoolean(INPUT_ONLY_ANY_HOST_PREF_STRING, DEFAULT_INPUT_ONLY_ANY_HOST);
        config.unlockFps = prefs.getBoolean(UNLOCK_FPS_STRING, DEFAULT_UNLOCK_FPS);
        config.vibrateOsc = prefs.getBoolean(VIBRATE_OSC_PREF_STRING, DEFAULT_VIBRATE_OSC);
        config.vibrateFallbackToDevice = prefs.getBoolean(VIBRATE_FALLBACK_PREF_STRING, DEFAULT_VIBRATE_FALLBACK);
        config.vibrateFallbackToDeviceStrength = prefs.getInt(VIBRATE_FALLBACK_STRENGTH_PREF_STRING, DEFAULT_VIBRATE_FALLBACK_STRENGTH);
        config.flipFaceButtons = prefs.getBoolean(FLIP_FACE_BUTTONS_PREF_STRING, DEFAULT_FLIP_FACE_BUTTONS);
        config.touchMode = TouchMode.resolve(
                prefs.getString(TouchMode.PREF, null),
                prefs.getBoolean(TOUCHSCREEN_TRACKPAD_PREF_STRING, DEFAULT_TOUCHSCREEN_TRACKPAD),
                prefs.getBoolean(TouchMode.NATIVE_TOUCH_PREF, false));
        config.touchscreenTrackpad = TouchMode.legacyTrackpad(config.touchMode);
        config.trackpadSpeed = Math.max(25, Math.min(400, prefs.getInt(TRACKPAD_SPEED_PREF_STRING, DEFAULT_TRACKPAD_SPEED)));
        config.localCursorSpeed = Math.max(25, Math.min(400, prefs.getInt(LOCAL_CURSOR_SPEED_PREF_STRING, DEFAULT_TRACKPAD_SPEED)));
        config.trackpadAcceleration = prefs.getBoolean(TRACKPAD_ACCELERATION_PREF_STRING, false);
        config.trackpadAccelerationCurve = prefs.getString(TRACKPAD_ACCELERATION_CURVE_PREF_STRING,
                PointerAcceleration.MEDIUM);
        config.trackpadMomentum = prefs.getBoolean(TRACKPAD_MOMENTUM_PREF_STRING, false);
        config.trackpadMomentumGlide = prefs.getString(TRACKPAD_MOMENTUM_GLIDE_PREF_STRING,
                TouchMomentum.MEDIUM);
        config.trackpadArea = prefs.getString(TRACKPAD_AREA_PREF_STRING, TrackpadArea.FULL);
        config.threeFingerTap = prefs.getString(THREE_FINGER_TAP_PREF_STRING,
                FingerTapGesture.KEYBOARD);
        config.fourFingerTap = prefs.getString(FOUR_FINGER_TAP_PREF_STRING,
                FingerTapGesture.NONE);
        config.pcKeyboardLayout = prefs.getString(PC_KEYBOARD_LAYOUT_PREF_STRING, "auto");
        config.pcKeyboardSize = Math.max(30, Math.min(70, prefs.getInt(PC_KEYBOARD_SIZE_PREF_STRING, 50)));
        config.pcKeyboardOpacity = Math.max(30, Math.min(100, prefs.getInt(PC_KEYBOARD_OPACITY_PREF_STRING, 90)));
        config.mouseSpeed = Math.max(25, Math.min(400, prefs.getInt(MOUSE_SPEED_PREF_STRING, DEFAULT_MOUSE_SPEED)));
        config.enableLatencyToast = prefs.getBoolean(LATENCY_TOAST_PREF_STRING, DEFAULT_LATENCY_TOAST);
        config.rustEngine = prefs.getBoolean(RUST_ENGINE_PREF_STRING, true);
        config.experimentalPacked444 = prefs.getBoolean(PACKED_444_PREF_STRING, false);
        config.experimentalChipsetTuning = prefs.getBoolean(CHIPSET_TUNING_PREF_STRING, false);
        config.absoluteMouseMode = prefs.getBoolean(ABSOLUTE_MOUSE_MODE_PREF_STRING, DEFAULT_ABSOLUTE_MOUSE_MODE);
        config.enableAudioFx = prefs.getBoolean(ENABLE_AUDIO_FX_PREF_STRING, DEFAULT_ENABLE_AUDIO_FX);
        config.audioPassthrough = prefs.getBoolean(AUDIO_PASSTHROUGH_PREF_STRING, false);
        config.reduceRefreshRate = prefs.getBoolean(REDUCE_REFRESH_RATE_PREF_STRING, DEFAULT_REDUCE_REFRESH_RATE);
        config.fullRange = prefs.getBoolean(FULL_RANGE_PREF_STRING, DEFAULT_FULL_RANGE);
        config.gamepadTouchpadAsMouse = prefs.getBoolean(GAMEPAD_TOUCHPAD_AS_MOUSE_PREF_STRING, DEFAULT_GAMEPAD_TOUCHPAD_AS_MOUSE);
        config.gamepadMotionSensors = prefs.getBoolean(GAMEPAD_MOTION_SENSORS_PREF_STRING, DEFAULT_GAMEPAD_MOTION_SENSORS);
        config.gamepadMotionSensorsFallbackToDevice = prefs.getBoolean(GAMEPAD_MOTION_FALLBACK_PREF_STRING, DEFAULT_GAMEPAD_MOTION_FALLBACK);
        config.codecLowLatency = prefs.getBoolean(CODEC_LOW_LATENCY_PREF_STRING, true);
        config.vendorLowLatency = prefs.getBoolean(VENDOR_LOW_LATENCY_PREF_STRING, true);
        config.disableHevcLowLatency = prefs.getBoolean(DISABLE_HEVC_LOW_LATENCY_PREF_STRING, false);
        config.codecPerformance = prefs.getBoolean(CODEC_PERFORMANCE_PREF_STRING, true);
        config.phonePerformanceHints = prefs.getBoolean(PHONE_PERFORMANCE_HINTS_PREF_STRING, true);
        config.gpuMaxClocks = prefs.getBoolean(GPU_MAX_CLOCKS_PREF_STRING, DEFAULT_GPU_MAX_CLOCKS);
        if (prefs.contains("checkbox_pyrowave_front_buffer")) {
            // Front-buffer rendering tore too much to play and is gone; drop its old switch.
            prefs.edit().remove("checkbox_pyrowave_front_buffer").apply();
        }
        config.dropLateFrames = prefs.getBoolean(DROP_LATE_FRAMES_PREF_STRING, true);
        // SurfaceView preserves HDR metadata (and 10-bit output: a SurfaceTexture takes 8-bit) and avoids TextureView's extra composition step.
        config.useTextureView = prefs.getBoolean(TEXTURE_VIEW_PREF_STRING, false) && !config.enableHdr && !config.enableSdr10Bit &&
                config.videoFormat != FormatOption.FORCE_PYROWAVE;
        config.unbatchedInput = prefs.getBoolean(UNBATCHED_INPUT_PREF_STRING, true);
        config.networkPriority = prefs.getBoolean(NETWORK_PRIORITY_PREF_STRING, true);
        config.clientRoaming = prefs.getBoolean(CLIENT_ROAMING_PREF_STRING, true);
        config.roamingMetered = prefs.getBoolean(ROAMING_METERED_PREF_STRING, false);
        config.virtualDisplay = prefs.getBoolean(VIRTUAL_DISPLAY_PREF_STRING, false);
        config.virtualDisplayScale = Math.max(50, Math.min(200, prefs.getInt(VIRTUAL_DISPLAY_SCALE_PREF_STRING, 100)));
        config.enableYuv444 = prefs.getBoolean(YUV444_PREF_STRING, false);
        config.vrr = prefs.getBoolean(VRR_PREF_STRING, false);
        config.upscalingMode = UpscalingPolicy.Mode.fromPreference(prefs.getString(UPSCALING_PREF_STRING, "off"));
        config.upscalingSharpness = Math.max(0, Math.min(100, prefs.getInt(UPSCALING_SHARPNESS_PREF_STRING, 50)));
        config.vulkanRender = prefs.getBoolean(VULKAN_RENDER_PREF_STRING, false);
        if (config.vrr) {
            // VRR streams arrive at the game's own cadence: show each frame as soon as it is
            // decoded; Game votes for ARR where supported, or the highest refresh rate otherwise.
            config.framePacing = FRAME_PACING_MIN_LATENCY;
            config.reduceRefreshRate = false;
        }
        config.hostFrameTimes = prefs.getBoolean(HOST_FRAME_TIMES_PREF_STRING, false);
        config.contentRateFrameRate = prefs.getBoolean(CONTENT_RATE_FRAME_RATE_PREF_STRING, false);
        // The vote is for a SurfaceView on a fixed-rate panel; VRR has its own vote. It needs the lower refresh
        // rate switch: the settings screen greys it without that, so a stored choice must not act on its own.
        config.contentRateFrameRate &= !config.vrr && !config.useTextureView && config.reduceRefreshRate;
        if (config.hostFrameTimes) {
            // Each frame is released at its scheduled time and the compositor shows it then.
            config.framePacing = FRAME_PACING_MIN_LATENCY;
        }

        return config;
    }
}
