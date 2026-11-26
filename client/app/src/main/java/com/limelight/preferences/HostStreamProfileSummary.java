package com.limelight.preferences;

import android.content.Intent;
import android.preference.Preference;
import android.preference.PreferenceCategory;

import com.limelight.R;
import com.limelight.RemoteSetupActivity;
import com.limelight.binding.video.DisplayFrameRatePolicy;
import com.limelight.binding.video.UpscalingPolicy;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.PairingManager;
import com.limelight.nvstream.wol.WakeTargets;
import com.limelight.ui.HostFeatureGate;
import com.limelight.utils.HelpLauncher;
import com.limelight.utils.HostDetailsDialog;
import com.limelight.utils.PyroWavePaceStore;
import com.limelight.utils.ServerHelper;

import java.util.List;

/**
 * Summary formatting, capability gating, and PC action-row rendering for {@link HostStreamSettings}.
 */
final class HostStreamProfileSummary {
    private HostStreamProfileSummary() {}

    static void showActions(HostStreamSettings.SettingsFragment fragment, HostStreamSettings screen,
                            PreferenceCategory connection, PreferenceCategory host) {
        if (connection == null) return;
        ComputerDetails pc = screen.computer;
        boolean online = screen.isOnline();
        boolean unpaired = pc.pairState == PairingManager.PairState.NOT_PAIRED;
        String needsOnline = screen.getString(R.string.pc_settings_needs_online);

        connection.removeAll();
        Preference test = fragment.row(connection, "connection_test", R.string.connection_test_title, screen::startConnectionTest);
        test.setSummary(screen.getString(pc.caps.mayHave(HostCapabilities.Feature.PYROWAVE_BANDWIDTH_PROBE) ?
                R.string.pc_settings_test_speed : R.string.pc_settings_test_delay) + (online ? "" : "\n" + needsOnline));
        test.setEnabled(online);
        PyroWavePaceStore.Calibration pace = PyroWavePaceStore.get(screen, pc.uuid);
        if (pc.supportsPyroWavePaceCalibration() || pace != null) {
            Preference calibrate = fragment.row(connection, "pace_calibration", R.string.pace_calibration_title,
                    screen::startPaceCalibration);
            calibrate.setSummary((pace != null ? screen.getString(R.string.pace_calibration_saved, pace.paceMbps) :
                    screen.getString(R.string.pace_calibration_summary)) + (online ? "" : "\n" + needsOnline));
            calibrate.setEnabled(online);
        }
        fragment.row(connection, "remote_setup", R.string.remote_setup_title, () -> fragment.startActivity(
                new Intent(screen, RemoteSetupActivity.class).putExtra(RemoteSetupActivity.EXTRA_PC_UUID, screen.hostUuid)))
                .setSummary(R.string.pc_settings_remote_summary);
        fragment.row(connection, "wake", R.string.pc_settings_wake, screen::showWakeHelp).setSummary(WakeTargets.hasMac(screen, pc) ?
                R.string.pc_settings_wake_ready : R.string.pc_settings_wake_unknown);
        List<String> wakeMacs = new WakeTargets(screen).manual(pc.uuid);
        fragment.row(connection, "wake_macs", R.string.pc_settings_wake_macs, fragment::showWakeMacs).setSummary(wakeMacs.isEmpty() ?
                screen.getString(R.string.pc_settings_wake_macs_none) : android.text.TextUtils.join(", ", wakeMacs));

        host.removeAll();
        HostFeatureGate webUi = gate(screen, HostCapabilities.Feature.WEB_UI);
        if (webUi.isVisible()) {
            Preference web = fragment.row(host, "web_ui", R.string.host_administration_menu, screen::showHostAdministration);
            okhttp3.HttpUrl url = ServerHelper.getHostSettingsUrl(pc);
            web.setSummary(url != null ? url.toString() : needsOnline);
            web.setEnabled(url != null);
            applyGate(screen, web, HostCapabilities.Feature.WEB_UI);
        }
        fragment.row(host, "details", R.string.pcview_menu_details, () -> HostDetailsDialog.show(screen, screen.computer))
                .setSummary(R.string.pc_settings_details_summary);
        if (!unpaired) {
            fragment.row(host, "frontend_export", R.string.frontend_export_menu, screen::startFrontendExport)
                    .setSummary(HostFrontendExporter.exportSummary(screen, pc));
            if (screen.canPinShortcuts()) {
                fragment.row(host, "shortcut", R.string.pc_settings_shortcut, screen::pinShortcut)
                        .setSummary(R.string.pc_settings_shortcut_summary);
            }
        }
        if (pc.caps.family() == HostCapabilities.Family.GAMESTREAM) {
            fragment.row(host, "gamestream_eol", R.string.pcview_menu_eol, () -> HelpLauncher.launchGameStreamEolFaq(screen));
        }
        fragment.row(host, "rename", R.string.host_menu_rename, () -> screen.hostActions.rename(screen.computer));
        if (online && pc.pairState == PairingManager.PairState.PAIRED) {
            fragment.row(host, "unpair", R.string.pcview_menu_unpair_pc, () -> screen.hostActions.confirmUnpair(screen.computer));
        }
        fragment.row(host, "remove", R.string.pcview_menu_delete_pc, () -> screen.hostActions.confirmRemove(screen.computer));
    }

    static void updateSummaries(HostStreamSettings.SettingsFragment fragment, HostStreamSettings activity) {
        PreferenceConfiguration config = activity.effective();
        HostStreamProfile own = activity.overrides;
        fragment.findPreference("resolution").setSummary(
                summary(activity, HostStreamProfile.Field.RESOLUTION, own.nativeResolution != null, resolutionText(activity, config)));
        fragment.findPreference("refresh").setSummary(
                summary(activity, HostStreamProfile.Field.REFRESH, own.refreshRateX100 != null, refreshText(activity, config)));
        String bitrate = summary(activity, HostStreamProfile.Field.BITRATE, own.bitrate != null,
                HostStreamProfile.formatBitrateMbps(config.bitrate) + " Mbps");
        String warning = PreferenceConfiguration.pyroWaveBelowMinimumWarning(activity, config);
        fragment.findPreference("bitrate").setSummary(warning == null ? bitrate : bitrate + "\n" + warning);
        String[] codecs = activity.getResources().getStringArray(R.array.host_profile_codecs);
        String codec = summary(activity, HostStreamProfile.Field.CODEC, own.codec != null, codecs[config.videoFormat.ordinal()]);
        if (config.videoFormat == PreferenceConfiguration.FormatOption.FORCE_PYROWAVE &&
                !activity.caps.mayHave(HostCapabilities.Feature.PYROWAVE)) {
            codec += "\n" + activity.getString(R.string.host_pyrowave_unavailable,
                    codecs[PreferenceConfiguration.codecWithoutPyroWave(activity.global.videoFormat).ordinal()]);
        }
        fragment.findPreference("codec").setSummary(codec);
        fragment.findPreference("upscaling").setSummary(
                summary(activity, HostStreamProfile.Field.UPSCALING, own.upscalingMode != null, upscalingText(activity, config)));
        fragment.findPreference("sharpness").setSummary(summary(activity, HostStreamProfile.Field.SHARPNESS,
                own.upscalingSharpness != null, activity.getString(R.string.upscaling_sharpness_value, config.upscalingSharpness)));
        fragment.findPreference("sharpness").setEnabled(config.upscalingMode == UpscalingPolicy.Mode.FSR1 ||
                config.upscalingMode == UpscalingPolicy.Mode.SGSR1);
        for (String key : new String[] {"hdr", "yuv444", "full_range"}) {
            fragment.findPreference(key).setSummary(summary(activity, switchField(key),
                    switchOverride(activity, key) != null, switchText(activity, key, switchValue(config, key))));
        }
        Preference hdrFormat = fragment.findPreference("hdr_format");
        if (hdrFormat != null) {
            String[] formats = activity.getResources().getStringArray(R.array.hdr_format_names);
            hdrFormat.setSummary(summary(activity, HostStreamProfile.Field.HDR_FORMAT, own.hdrFormat != null,
                    formats[config.hdrFormat.ordinal()]) + "\n" + activity.getString(R.string.summary_hdr_format));
        }
        fragment.findPreference("vrr").setSummary(
                summary(activity, HostStreamProfile.Field.VRR, own.vrr != null, vrrText(activity, config)));
        String pacing = summary(activity, HostStreamProfile.Field.PACING,
                own.has(HostStreamProfile.Field.PACING), pacingText(activity, config));
        fragment.findPreference("pacing").setSummary(config.vrr ? pacing + "\n" + activity.getString(R.string.layers_pacing_vrr) : pacing);
        Preference vrr = fragment.findPreference("vrr");
        vrr.setEnabled(true);
        vrr.setSummary(vrr.getSummary() + "\n" + activity.getString(R.string.summary_vrr));

        updateDisplayAndExtraSummaries(fragment, activity, config, own);
        activity.updateSaveState();
    }

    private static void updateDisplayAndExtraSummaries(HostStreamSettings.SettingsFragment fragment,
                                                       HostStreamSettings activity,
                                                       PreferenceConfiguration config,
                                                       HostStreamProfile own) {
        Preference virtual = fragment.findPreference("virtual");
        Preference scale = fragment.findPreference("scale");
        Preference dpi = fragment.findPreference("dpi");
        if (virtual != null) {
            virtual.setSummary(summary(activity, HostStreamProfile.Field.VIRTUAL_DISPLAY,
                    switchOverride(activity, "virtual") != null, onOff(activity, switchValue(config, "virtual"))));
        }
        if (scale != null) {
            scale.setSummary(summary(activity, HostStreamProfile.Field.RENDER_SCALE, own.renderScale != null,
                    config.virtualDisplayScale + "%") + "\n" + activity.getString(R.string.summary_virtual_display_scale));
        }
        if (dpi != null) {
            dpi.setSummary(summary(activity, HostStreamProfile.Field.DISPLAY_SCALE, own.displayScale != null,
                    displayScaleText(activity, config.virtualDisplayDpi)) + "\n" + activity.getString(R.string.summary_virtual_display_dpi));
        }
        Preference rtxHdr = fragment.findPreference("rtx_hdr");
        if (rtxHdr != null) {
            String value = activity.mode == HostStreamSettings.Mode.GAME
                    ? summary(activity, HostStreamProfile.Field.RTX_HDR, own.rtxHdr != null, enhancementText(activity, config.rtxHdr))
                    : enhancementText(activity, own.rtxHdr);
            rtxHdr.setSummary(value + "\n" + activity.getString(R.string.summary_rtx_hdr));
        }
        Preference rtxVideo = fragment.findPreference("rtx_video");
        if (rtxVideo != null) {
            String value = activity.mode == HostStreamSettings.Mode.GAME
                    ? summary(activity, HostStreamProfile.Field.RTX_VIDEO, own.rtxVideo != null, enhancementText(activity, config.rtxVideo))
                    : enhancementText(activity, own.rtxVideo);
            rtxVideo.setSummary(value + "\n" + activity.getString(R.string.summary_rtx_video));
        }
        Preference preset = fragment.findPreference("preset");
        if (preset != null) {
            GameProfilePreset current = GameProfilePreset.of(own, activity.qualityBitrate());
            preset.setSummary(activity.getString(current.title) + "\n" + activity.getString(current.summary));
        }
        fragment.findPreference("reset").setEnabled(!own.isEmpty());
        boolean pc = activity.mode == HostStreamSettings.Mode.PC;
        fragment.findPreference("reset").setSummary(own.isEmpty() ?
                activity.getString(pc ? R.string.host_profile_status_global : R.string.layers_status_empty) :
                activity.getString(pc ? R.string.host_profile_status_custom : R.string.layers_status_custom));
        Preference display = fragment.findPreference("host_display");
        HostDisplayChoice displayChoice = pc && activity.caps.mayHave(HostCapabilities.Feature.HOST_DISPLAY_MODES) ?
                HostDisplayChoice.load(activity, activity.hostUuid) : HostDisplayChoice.HOST_DEFAULT;
        if (virtual != null) {
            virtual.setEnabled(displayChoice == HostDisplayChoice.HOST_DEFAULT);
            if (displayChoice != HostDisplayChoice.HOST_DEFAULT) {
                virtual.setSummary(activity.getString(R.string.host_display_controls_virtual, activity.getString(displayChoice.label)));
            }
            applyGate(activity, virtual, HostCapabilities.Feature.VIRTUAL_DISPLAY);
        }
        if (scale != null) applyGate(activity, scale, HostCapabilities.Feature.VIRTUAL_DISPLAY);
        if (dpi != null) applyGate(activity, dpi, HostCapabilities.Feature.DISPLAY_SCALE);
        if (display != null) {
            String name = DisplayArrangementChoice.names(activity, true)[
                    DisplayArrangementChoice.choice(displayChoice, switchValue(config, "virtual"), true)].toString();
            display.setSummary((ownDisplay(activity) ? name : sameAs(activity, name)) + "\n" +
                    activity.getString(R.string.settings_profile_display_immediate));
            applyGate(activity, display, HostCapabilities.Feature.HOST_DISPLAY_MODES);
        }
    }

    static HostFeatureGate gate(HostStreamSettings activity, HostCapabilities.Feature feature) {
        return HostFeatureGate.of(activity.caps, feature);
    }

    static void applyGate(HostStreamSettings activity, Preference row, HostCapabilities.Feature feature) {
        HostFeatureGate gate = gate(activity, feature);
        if (!gate.isEnabled()) {
            row.setEnabled(false);
        }
        row.setSummary(gate.withNote(activity, row.getSummary()));
    }

    static String onOff(HostStreamSettings activity, boolean value) {
        return activity.getString(value ? R.string.host_profile_on : R.string.host_profile_off);
    }

    static String switchText(HostStreamSettings activity, String key, boolean value) {
        if ("yuv444".equals(key)) {
            return activity.getResources().getStringArray(R.array.sharp_color_names)[value ? 1 : 0];
        }
        return onOff(activity, value);
    }

    static String summary(HostStreamSettings activity, HostStreamProfile.Field field, boolean own, String value) {
        if (own) return value;
        StreamLayers.Source source = activity.below.source(field);
        if (activity.mode == HostStreamSettings.Mode.PC && source == StreamLayers.Source.MAIN) {
            return activity.getString(R.string.host_profile_follows, value);
        }
        return activity.getString(R.string.layers_value_from, value, StreamLayersText.source(activity, activity.below, source));
    }

    static String sameAs(HostStreamSettings activity, String value) {
        return activity.getString(activity.mode == HostStreamSettings.Mode.PC ? R.string.host_profile_same_as_all_value :
                R.string.layers_not_set_value, value);
    }

    static HostStreamProfile.Field switchField(String key) {
        switch (key) {
            case "hdr": return HostStreamProfile.Field.HDR;
            case "virtual": return HostStreamProfile.Field.VIRTUAL_DISPLAY;
            case "yuv444": return HostStreamProfile.Field.YUV444;
            default: return HostStreamProfile.Field.FULL_RANGE;
        }
    }

    static String pacingText(HostStreamSettings activity, PreferenceConfiguration config) {
        String[] names = activity.getResources().getStringArray(R.array.frame_pacing_choice_names);
        return names[Math.max(0, FramePacingChoice.index(FramePacingChoice.of(config)))];
    }

    static String resolutionText(HostStreamSettings activity, PreferenceConfiguration config) {
        return config.resolutionFollowsScreen ? activity.getString(R.string.host_profile_native) :
                config.width + " × " + config.height;
    }

    static String refreshText(HostStreamSettings activity, PreferenceConfiguration config) {
        return HostStreamProfile.formatRefreshRate(config.launchRefreshRateX100 == 0 ?
                config.fps * 100 : config.launchRefreshRateX100) + " " + activity.getString(R.string.fps_suffix_fps);
    }

    static String displayScaleText(HostStreamSettings activity, int percent) {
        return percent == 0 ? activity.getString(R.string.virtual_display_dpi_pc_own) : percent + "%";
    }

    static String upscalingText(HostStreamSettings activity, PreferenceConfiguration config) {
        String upscaling = activity.getResources().getStringArray(R.array.upscaling_names)[config.upscalingMode.ordinal()];
        if (config.upscalingMode != UpscalingPolicy.Mode.OFF) {
            if (config.enableHdr) upscaling += "\n" + activity.getString(R.string.upscaling_hdr);
            else if (config.videoFormat == PreferenceConfiguration.FormatOption.FORCE_PYROWAVE) {
                upscaling += "\n" + activity.getString(R.string.upscaling_pyrowave);
            }
        }
        return upscaling;
    }

    static boolean switchValue(PreferenceConfiguration config, String key) {
        switch (key) {
            case "hdr": return config.enableHdr;
            case "virtual": return config.virtualDisplay;
            case "yuv444": return config.enableYuv444;
            default: return config.fullRange;
        }
    }

    static Boolean switchOverride(HostStreamSettings activity, String key) {
        HostStreamProfile own = activity.overrides;
        switch (key) {
            case "hdr": return own.hdr;
            case "virtual": return own.virtualDisplay;
            case "yuv444": return own.yuv444;
            default: return own.fullRange;
        }
    }

    static void setSwitchOverride(HostStreamSettings activity, String key, Boolean value) {
        HostStreamProfile own = activity.overrides;
        switch (key) {
            case "hdr": own.hdr = value; break;
            case "virtual": own.virtualDisplay = value; break;
            case "yuv444": own.yuv444 = value; break;
            default: own.fullRange = value; break;
        }
    }

    static String vrrText(HostStreamSettings activity, PreferenceConfiguration config) {
        return activity.getResources().getStringArray(R.array.vrr_choice_names)[
                VrrChoice.of(config.vrr && DisplayFrameRatePolicy.vrrOffered(activity), config.realVrr)];
    }

    static String enhancementText(HostStreamSettings activity, Boolean choice) {
        return choice == null ? activity.getString(R.string.fec_choice_host) : onOff(activity, choice);
    }

    static boolean ownDisplay(HostStreamSettings activity) {
        return HostDisplayChoice.hasOwn(activity, activity.hostUuid) || switchOverride(activity, "virtual") != null;
    }
}
