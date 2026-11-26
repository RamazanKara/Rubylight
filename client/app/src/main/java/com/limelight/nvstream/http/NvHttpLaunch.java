package com.limelight.nvstream.http;

import com.limelight.nvstream.ConnectionContext;
import com.limelight.nvstream.StreamConfiguration;
import com.limelight.nvstream.jni.Json;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.nvstream.jni.NightjarHttpJni;
import com.limelight.preferences.HostDisplayChoice;

import org.xmlpull.v1.XmlPullParserException;

import java.io.IOException;

/**
 * Package-private launch query serialization and stream launch parameter helpers extracted from {@link NvHTTP}.
 */
final class NvHttpLaunch {
    private static final char[] HEX = "0123456789ABCDEF".toCharArray();

    private NvHttpLaunch() {}

    static HostDisplayChoice launchDisplayChoice(ConnectionContext context, NvApp.Role role) {
        if (!context.hostCaps.has(HostCapabilities.Feature.HOST_DISPLAY_MODES)) {
            return HostDisplayChoice.HOST_DEFAULT;
        }
        HostDisplayChoice c = context.streamConfig.getHostDisplay() != null
                ? context.streamConfig.getHostDisplay() : context.hostDisplay;
        return c == null ? HostDisplayChoice.HOST_DEFAULT : c.forRole(role);
    }

    static boolean wantsVirtualDisplay(ConnectionContext context, NvApp.Role role) {
        return role != NvApp.Role.INPUT_ONLY
                && launchDisplayChoice(context, role).requestsVirtualDisplay(context.streamConfig.getVirtualDisplay());
    }

    static int getStreamFrameRateMilliHz(ConnectionContext context) {
        StreamConfiguration sc = context.streamConfig;
        int streamFps = Math.max(0, sc.getRefreshRate());
        int launchFps = Math.max(0, sc.getLaunchRefreshRate());
        if (context.hostCaps.has(HostCapabilities.Feature.FRACTIONAL_REFRESH)) {
            int explicit = sc.getLaunchRefreshRateX100();
            int frac = (explicit >= 100 && explicit <= 100_000) ? explicit : 0;
            if (frac == 0) {
                int display = sc.getClientRefreshRateX100();
                if (display >= 100 && display <= 100_000 && (display + 50) / 100 == launchFps) {
                    frac = display;
                }
            }
            if (frac > 0 && (frac + 50) / 100 == streamFps) {
                return frac * 10;
            }
        }
        return streamFps * 1000;
    }

    static boolean asksClipboardSync(ConnectionContext context) {
        NvApp app = context.streamConfig.getApp();
        return app != null && app.getRole() != NvApp.Role.REMOTE_MONITOR && context.streamConfig.getClipboardSync()
                && getLaunchQuery(context, app.getAppId(), false).contains("&rlClipboardSync=1");
    }

    static String getLaunchQuery(ConnectionContext context, int appId, boolean enableHdr) {
        try {
            String req = buildLaunchRequestJson(context, appId, enableHdr);
            return NvHttpJson.unwrapOk(NightjarHttpJni.buildLaunchQuery(
                    req, context.hostCaps.toJson(), context.isNvidiaServerSoftware), false).getString("ok");
        } catch (IOException | XmlPullParserException | Json.SyntaxException e) {
            throw new IllegalStateException(e);
        }
    }

    static String buildLaunchRequestJson(ConnectionContext ctx, int appId, boolean enableHdr) {
        StreamConfiguration sc = ctx.streamConfig;
        NvApp app = sc.getApp();
        NvApp.Role role = app != null ? app.getRole() : NvApp.Role.STREAM;
        MoonBridge.AudioConfiguration audio = sc.getAudioConfiguration();
        String audioKind = audio == null ? "STEREO"
                : audio.channelCount == 12 ? "SURROUND_714"
                : audio.channelCount == 8 ? "SURROUND_71"
                : audio.channelCount == 6 ? "SURROUND_51" : "STEREO";
        Json.Obj j = Json.obj()
                .put("app_id", appId)
                .put("app_uuid", app != null ? app.getAppUuid() : "")
                .put("role", role.name())
                .put("small_stream_input_only", app != null && app.isSmallStreamInputOnly())
                .put("mic_only", app != null && app.isMicOnly())
                .put("width", ctx.negotiatedWidth).put("height", ctx.negotiatedHeight)
                .put("fps", sc.getLaunchRefreshRate())
                .put("launch_refresh_rate_x100", sc.getLaunchRefreshRateX100())
                .put("client_refresh_rate_x100", sc.getClientRefreshRateX100())
                .put("virtual_display", sc.getVirtualDisplay())
                .put("virtual_display_scale", sc.getVirtualDisplayScale())
                .put("virtual_display_dpi", sc.getVirtualDisplayDpi())
                .put("host_display", launchDisplayChoice(ctx, role).name())
                .put("vrr", sc.getVrr()).put("sops", sc.getSops())
                .put("play_local_audio", sc.getPlayLocalAudio())
                .put("hdr", enableHdr).put("ten_bit_sdr", !enableHdr && ctx.negotiated10Bit)
                .put("audio_surround", audioKind)
                .put("ri_key_hex", bytesToHex(ctx.riKey.getEncoded())).put("ri_key_id", ctx.riKeyId)
                .put("prepared_controllers", ctx.preparedControllers == null ? "" : ctx.preparedControllers)
                .put("gamepad_mask", sc.getAttachedGamepadMask())
                .put("playstation_mask", sc.getPlayStationGamepadMask() & 0xFFFF)
                .put("persist_gamepads", sc.getPersistGamepadsAfterDisconnect())
                .put("rate_change", ctx.rateChange).put("confirm_replace", ctx.confirmReplace)
                .put("clipboard_sync", ctx.streamConfig.getClipboardSync());
        j.put("hdr_static_only", sc.getHdrStaticOnly());
        if (ctx.fecChoice != null) j.put("fec", ctx.fecChoice.wireValue);
        if (sc.getDisplayLuminance() != null) {
            j.put("display_luminance", Json.obj()
                    .put("max_nits", sc.getDisplayLuminance().maxNits)
                    .put("min_nits", sc.getDisplayLuminance().minNits)
                    .put("max_full_frame_nits", sc.getDisplayLuminance().maxFullFrameNits)
                    .put("estimated_from_ratio", sc.getDisplayLuminance().estimatedFromRatio));
        }
        if (sc.getRtxHdr() != null) j.put("rtx_hdr", sc.getRtxHdr().booleanValue());
        if (sc.getRtxVideo() != null) j.put("rtx_video", sc.getRtxVideo().booleanValue());
        return j.toString();
    }

    private static String bytesToHex(byte[] bytes) {
        char[] out = new char[bytes.length * 2];
        for (int i = 0; i < bytes.length; i++) {
            int v = bytes[i] & 0xFF;
            out[i * 2] = HEX[v >>> 4];
            out[i * 2 + 1] = HEX[v & 0x0F];
        }
        return new String(out);
    }
}
