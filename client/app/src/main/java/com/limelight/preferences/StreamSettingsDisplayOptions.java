package com.limelight.preferences;

import android.app.Activity;
import android.content.Context;
import android.content.SharedPreferences;
import android.content.pm.PackageManager;
import android.media.MediaCodecInfo;
import android.os.Build;
import android.preference.ListPreference;
import android.preference.Preference;
import android.preference.PreferenceManager;
import android.preference.SwitchPreference;
import android.util.DisplayMetrics;
import android.util.Range;
import android.view.Display;
import android.view.DisplayCutout;
import android.view.WindowInsets;

import com.limelight.LimeLog;
import com.limelight.R;
import com.limelight.binding.audio.Surround714;
import com.limelight.binding.video.DisplayFrameRatePolicy;
import com.limelight.binding.video.HdrPolicy;
import com.limelight.binding.video.MediaCodecHelper;

import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;

/**
 * Display mode, cutout, decoder capability, frame rate, audio channel, VRR, HDR, and bitrate
 * helpers for {@link StreamSettings.SettingsFragment}.
 */
final class StreamSettingsDisplayOptions {
    private StreamSettingsDisplayOptions() { }

    static void configureDisplayAndMediaCapabilities(StreamSettingsFragment fragment, SwitchPreference hdr) {
        Activity activity = fragment.getActivity();
        Display display = activity.getWindowManager().getDefaultDisplay();
        float maxSupportedFps = display.getRefreshRate();
        boolean television = activity.getPackageManager().hasSystemFeature(PackageManager.FEATURE_TELEVISION);

        // Hide non-supported resolution/FPS combinations
        int maxSupportedResW = 0;

        // Add a native resolution with any insets included for users that don't want content
        // behind the notch of their display
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
            DisplayCutout cutout;

            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                // Use the much nicer Display.getCutout() API on Android 10+
                cutout = display.getCutout();
            } else {
                // Android 9 only
                WindowInsets insets = activity.getWindow().getDecorView().getRootWindowInsets();
                cutout = insets == null ? null : insets.getDisplayCutout();
            }

            if (cutout != null) {
                int widthInsets = cutout.getSafeInsetLeft() + cutout.getSafeInsetRight();
                int heightInsets = cutout.getSafeInsetBottom() + cutout.getSafeInsetTop();

                if (widthInsets != 0 || heightInsets != 0) {
                    DisplayMetrics metrics = new DisplayMetrics();
                    display.getRealMetrics(metrics);

                    int width = Math.max(metrics.widthPixels - widthInsets, metrics.heightPixels - heightInsets);
                    int height = Math.min(metrics.widthPixels - widthInsets, metrics.heightPixels - heightInsets);

                    addNativeResolutionEntries(fragment, width, height, true);
                }
            }
        }

        // Always allow resolutions that are smaller or equal to the active
        // display resolution because decoders can report total non-sense to us.
        // For example, a p201 device reports:
        // AVC Decoder: OMX.amlogic.avc.decoder.awesome
        // HEVC Decoder: OMX.amlogic.hevc.decoder.awesome
        // AVC supported width range: 64 - 384
        // HEVC supported width range: 64 - 544
        for (Display.Mode candidate : display.getSupportedModes()) {
            // Some devices report their dimensions in the portrait orientation
            // where height > width. Normalize these to the conventional width > height
            // arrangement before we process them.
            int width = Math.max(candidate.getPhysicalWidth(), candidate.getPhysicalHeight());
            int height = Math.min(candidate.getPhysicalWidth(), candidate.getPhysicalHeight());

            // Some TVs report strange values here, so let's avoid native resolutions on a TV
            // unless they report greater than 4K resolutions.
            if (!television || (width > 3840 || height > 2160)) {
                addNativeResolutionEntries(fragment, width, height, false);
            }

            if ((width >= 3840 || height >= 2160) && maxSupportedResW < 3840) {
                maxSupportedResW = 3840;
            } else if ((width >= 2560 || height >= 1440) && maxSupportedResW < 2560) {
                maxSupportedResW = 2560;
            } else if ((width >= 1920 || height >= 1080) && maxSupportedResW < 1920) {
                maxSupportedResW = 1920;
            }

            if (candidate.getRefreshRate() > maxSupportedFps) {
                maxSupportedFps = candidate.getRefreshRate();
            }
        }

        // The decoders' width ranges can only raise the slot. Reading them builds a
        // MediaCodecList, a blocking call to the media service that hit an ANR on a
        // MediaTek Android 15 device (V+ c519d37): probe off the main thread and drop
        // the sizes the decoders can't take once it's done.
        final int displayResW = maxSupportedResW;
        final Context appContext = activity.getApplicationContext();
        final StreamSettings.SettingsFragment.DecoderWidthProbe probe = StreamSettings.SettingsFragment.decoderWidthProbe;
        fragment.decoderProbe = new Thread(() -> {
            int decoderResW = probe.probe(appContext);
            activity.runOnUiThread(() -> {
                if (fragment.isAdded() && fragment.getActivity() == activity && !activity.isFinishing()) {
                    removeUndecodableResolutions(fragment, Math.max(displayResW, decoderResW));
                }
            });
        }, "Decoder resolution probe");
        fragment.decoderProbe.start();
        // TVs can report a smaller panel than the screen they drive, so they keep every size.
        ((ResolutionPreference) fragment.findPreference(PreferenceConfiguration.RESOLUTION_PREF_STRING))
                .ensureSelectedResolutionEntry();
        sortResolutionEntries(fragment, !television);

        if (!PreferenceConfiguration.readPreferences(activity).unlockFps) {
            // We give some extra room in case the FPS is rounded down
            if (maxSupportedFps < 118) {
                removeValue(fragment, PreferenceConfiguration.FPS_PREF_STRING, "120", new Runnable() {
                    @Override
                    public void run() {
                        SharedPreferences prefs = PreferenceManager.getDefaultSharedPreferences(fragment.getActivity());
                        setValue(fragment, PreferenceConfiguration.FPS_PREF_STRING, "90");
                        resetBitrateToDefault(prefs, null, null);
                    }
                });
            }
            if (maxSupportedFps < 88) {
                // 1080p is unsupported
                removeValue(fragment, PreferenceConfiguration.FPS_PREF_STRING, "90", new Runnable() {
                    @Override
                    public void run() {
                        SharedPreferences prefs = PreferenceManager.getDefaultSharedPreferences(fragment.getActivity());
                        setValue(fragment, PreferenceConfiguration.FPS_PREF_STRING, "60");
                        resetBitrateToDefault(prefs, null, null);
                    }
                });
            }
            // Never remove 30 FPS or 60 FPS
        }
        addNativeFrameRateEntry(fragment, maxSupportedFps);

        // 7.1.4 only where this device plays it. A 7.1.4 choice made on another route (spatial
        // headphones) stays picked; the stream asks for 7.1 while it isn't playable.
        ListPreference audioConfig = (ListPreference) fragment.findPreference(PreferenceConfiguration.AUDIO_CONFIG_PREF_STRING);
        if (audioConfig != null && !PreferenceConfiguration.AUDIO_CONFIG_714.equals(audioConfig.getValue())
                && !Surround714.playable(activity)) {
            LimeLog.info("Excluding 7.1.4 audio: no output plays 12 channels");
            removeValue(fragment, PreferenceConfiguration.AUDIO_CONFIG_PREF_STRING, PreferenceConfiguration.AUDIO_CONFIG_714, () -> { });
        }

        // Variable refresh rate is gated by vrrOffered: where it is false it can't be turned on, and a
        // choice saved before reads as off (PreferenceConfiguration and SettingRules do the same).
        // The row shows off without saving that, so the choice returns on a newer Android.
        Preference vrr = fragment.findPreference(PreferenceConfiguration.VRR_PREF_STRING);
        if (vrr != null && !DisplayFrameRatePolicy.vrrOffered(activity)) {
            LimeLog.info("Hiding variable refresh rate: not offered");
            vrr.setPersistent(false);
            fragment.removeSetting(PreferenceConfiguration.VRR_PREF_STRING);
        }

        // We must now ensure our display is compatible with HDR10 (the capabilities are
        // null on some devices, e.g. Lenovo Mirage Solo, Android 8.0)
        boolean foundHdr10 = HdrPolicy.hasHdr10(HdrPolicy.displayHdrTypes(display));

        if (!foundHdr10) {
            LimeLog.info("Excluding HDR toggle based on display capabilities");
            fragment.removeSetting("checkbox_enable_hdr");
        } else if (PreferenceConfiguration.isShieldAtvFirmwareWithBrokenHdr()) {
            LimeLog.info("Disabling HDR toggle on old broken SHIELD TV firmware");
            hdr.setEnabled(false);
            hdr.setChecked(false);
            hdr.setSummary("Update the firmware on your NVIDIA SHIELD Android TV to enable HDR");
        }

        // Add a listener to the FPS and resolution preference
        // so the bitrate can be auto-adjusted
        fragment.findPreference(PreferenceConfiguration.RESOLUTION_PREF_STRING).setOnPreferenceChangeListener(new Preference.OnPreferenceChangeListener() {
            @Override
            public boolean onPreferenceChange(Preference preference, Object newValue) {
                SharedPreferences prefs = PreferenceManager.getDefaultSharedPreferences(fragment.getActivity());
                String valueStr = (String) newValue;

                // Picking this screen's native size keeps following the screen; any other size stays fixed.
                prefs.edit().putBoolean(PreferenceConfiguration.RESOLUTION_FOLLOWS_SCREEN_PREF_STRING,
                        !((ResolutionPreference) preference).isChoosingCustomResolution() &&
                                valueStr.equals(PreferenceConfiguration.nativeResolution(fragment.getActivity()))).apply();

                // Write the new bitrate value
                resetBitrateToDefault(prefs, valueStr, null);

                // Allow the original preference change to take place
                return true;
            }
        });
        fragment.findPreference(PreferenceConfiguration.FPS_PREF_STRING).setOnPreferenceChangeListener(new Preference.OnPreferenceChangeListener() {
            @Override
            public boolean onPreferenceChange(Preference preference, Object newValue) {
                SharedPreferences prefs = PreferenceManager.getDefaultSharedPreferences(fragment.getActivity());
                String valueStr = (String) newValue;

                // Write the new bitrate value
                resetBitrateToDefault(prefs, null, valueStr);

                // Allow the original preference change to take place
                return true;
            }
        });
    }

    static void setValue(StreamSettingsFragment fragment, String preferenceKey, String value) {
        ListPreference pref = (ListPreference) fragment.findPreference(preferenceKey);
        pref.setValue(value);
    }

    static void appendPreferenceEntry(ListPreference pref, String newEntryName, String newEntryValue) {
        CharSequence[] newEntries = Arrays.copyOf(pref.getEntries(), pref.getEntries().length + 1);
        CharSequence[] newValues = Arrays.copyOf(pref.getEntryValues(), pref.getEntryValues().length + 1);

        // Add the new option
        newEntries[newEntries.length - 1] = newEntryName;
        newValues[newValues.length - 1] = newEntryValue;

        pref.setEntries(newEntries);
        pref.setEntryValues(newValues);
    }

    // A size taken from this screen: the panel itself, or the area beside its camera cutout.
    static void addNativeResolutionEntry(StreamSettingsFragment fragment, int nativeWidth, int nativeHeight,
            boolean avoidsCutout, boolean portrait) {
        ListPreference pref = (ListPreference) fragment.findPreference(PreferenceConfiguration.RESOLUTION_PREF_STRING);
        String newValue = nativeWidth + "x" + nativeHeight;

        // Check if the native resolution is already present
        for (CharSequence value : pref.getEntryValues()) {
            if (newValue.equals(value.toString())) {
                // It is present in the default list, so don't add it again
                return;
            }
        }

        String size = nativeWidth + "×" + nativeHeight;
        String panel = PreferenceConfiguration.nativeResolution(fragment.getActivity());
        String landscape = Math.max(nativeWidth, nativeHeight) + "x" + Math.min(nativeWidth, nativeHeight);
        String newName;
        if (avoidsCutout) {
            newName = fragment.getString(portrait ? R.string.resolution_screen_cutout_portrait : R.string.resolution_screen_cutout, size);
        } else if (panel == null || panel.equals(landscape)) {
            newName = fragment.getString(portrait ? R.string.resolution_screen_portrait : R.string.resolution_screen, size);
        } else {
            // Another mode the panel can switch to
            newName = size;
        }
        appendPreferenceEntry(pref, newName, newValue);
    }

    static void addNativeResolutionEntries(StreamSettingsFragment fragment, int nativeWidth, int nativeHeight, boolean avoidsCutout) {
        if (PreferenceConfiguration.isSquarishScreen(nativeWidth, nativeHeight)) {
            addNativeResolutionEntry(fragment, nativeHeight, nativeWidth, avoidsCutout, true);
        }
        addNativeResolutionEntry(fragment, nativeWidth, nativeHeight, avoidsCutout, false);
    }

    // Fixed sizes larger than the panel would only be scaled down again; they stay listed while chosen.
    static boolean fitsPanel(String size, String panel) {
        if (panel == null) return true;
        return PreferenceConfiguration.getWidthFromResolutionString(size) <= PreferenceConfiguration.getWidthFromResolutionString(panel) &&
                PreferenceConfiguration.getHeightFromResolutionString(size) <= PreferenceConfiguration.getHeightFromResolutionString(panel);
    }

    // Largest first, so this screen's own size leads the list.
    static void sortResolutionEntries(StreamSettingsFragment fragment, boolean hideLargerThanPanel) {
        ListPreference pref = (ListPreference) fragment.findPreference(PreferenceConfiguration.RESOLUTION_PREF_STRING);
        String panel = PreferenceConfiguration.nativeResolution(fragment.getActivity());
        List<String[]> entries = new ArrayList<>();
        List<CharSequence> presets = Arrays.asList(fragment.getResources().getStringArray(R.array.resolution_values));
        for (int i = 0; i < pref.getEntryValues().length; i++) {
            String value = pref.getEntryValues()[i].toString();
            if (hideLargerThanPanel && presets.contains(value) && !fitsPanel(value, panel) && !value.equals(pref.getValue())) {
                continue;
            }
            entries.add(new String[] {pref.getEntries()[i].toString(), value});
        }
        java.util.Collections.sort(entries, (a, b) -> Long.compare(pixels(b[1]), pixels(a[1])));
        CharSequence[] names = new CharSequence[entries.size()];
        CharSequence[] values = new CharSequence[entries.size()];
        for (int i = 0; i < entries.size(); i++) {
            names[i] = entries.get(i)[0];
            values[i] = entries.get(i)[1];
        }
        pref.setEntries(names);
        pref.setEntryValues(values);
    }

    private static long pixels(String size) {
        return (long) PreferenceConfiguration.getWidthFromResolutionString(size) *
                PreferenceConfiguration.getHeightFromResolutionString(size);
    }

    static void addNativeFrameRateEntry(StreamSettingsFragment fragment, float framerate) {
        int frameRateRounded = Math.round(framerate);
        if (frameRateRounded == 0) {
            return;
        }

        ListPreference pref = (ListPreference) fragment.findPreference(PreferenceConfiguration.FPS_PREF_STRING);
        String fpsValue = Integer.toString(frameRateRounded);

        // Check if the native frame rate is already present
        for (CharSequence value : pref.getEntryValues()) {
            if (fpsValue.equals(value.toString())) {
                // It is present in the default list, so don't add it again
                return;
            }
        }

        appendPreferenceEntry(pref, fragment.getString(R.string.fps_screen, frameRateRounded), fpsValue);
    }

    static void labelEntry(ListPreference pref, String value, String label) {
        CharSequence[] entries = pref.getEntries().clone();
        for (int i = 0; i < entries.length; i++) {
            if (value.equals(pref.getEntryValues()[i].toString())) entries[i] = entries[i] + " · " + label;
        }
        pref.setEntries(entries);
    }

    static void removeValue(StreamSettingsFragment fragment, String preferenceKey, String value, Runnable onMatched) {
        int matchingCount = 0;

        ListPreference pref = (ListPreference) fragment.findPreference(preferenceKey);

        // Explicit dimensions stay fixed even when they match a preset outside the reported
        // decoder range. Custom dimensions already allow the same sizes; don't reset them here.
        if (PreferenceConfiguration.RESOLUTION_PREF_STRING.equals(preferenceKey) && value.equals(pref.getValue())) {
            return;
        }

        // Count the number of matching entries we'll be removing
        for (CharSequence seq : pref.getEntryValues()) {
            if (seq.toString().equalsIgnoreCase(value)) {
                matchingCount++;
            }
        }

        // Create the new arrays
        CharSequence[] entries = new CharSequence[pref.getEntries().length - matchingCount];
        CharSequence[] entryValues = new CharSequence[pref.getEntryValues().length - matchingCount];
        int outIndex = 0;
        for (int i = 0; i < pref.getEntryValues().length; i++) {
            if (pref.getEntryValues()[i].toString().equalsIgnoreCase(value)) {
                // Skip matching values
                continue;
            }

            entries[outIndex] = pref.getEntries()[i];
            entryValues[outIndex] = pref.getEntryValues()[i];
            outIndex++;
        }

        if (pref.getValue().equalsIgnoreCase(value)) {
            onMatched.run();
        }

        // Update the preference with the new list
        pref.setEntries(entries);
        pref.setEntryValues(entryValues);
    }

    // Drops the fixed sizes above the largest width the display or the decoders take.
    static void removeUndecodableResolutions(StreamSettingsFragment fragment, int maxSupportedResW) {
        LimeLog.info("Maximum resolution slot: " + maxSupportedResW);
        if (maxSupportedResW == 0) {
            return;
        }
        if (maxSupportedResW < 3840) {
            // 4K is unsupported
            removeValue(fragment, PreferenceConfiguration.RESOLUTION_PREF_STRING, PreferenceConfiguration.RES_4K, new Runnable() {
                @Override
                public void run() {
                    SharedPreferences prefs = PreferenceManager.getDefaultSharedPreferences(fragment.getActivity());
                    setValue(fragment, PreferenceConfiguration.RESOLUTION_PREF_STRING, PreferenceConfiguration.RES_1440P);
                    resetBitrateToDefault(prefs, null, null);
                }
            });
        }
        if (maxSupportedResW < 2560) {
            // 1440p is unsupported
            removeValue(fragment, PreferenceConfiguration.RESOLUTION_PREF_STRING, PreferenceConfiguration.RES_1440P, new Runnable() {
                @Override
                public void run() {
                    SharedPreferences prefs = PreferenceManager.getDefaultSharedPreferences(fragment.getActivity());
                    setValue(fragment, PreferenceConfiguration.RESOLUTION_PREF_STRING, PreferenceConfiguration.RES_1080P);
                    resetBitrateToDefault(prefs, null, null);
                }
            });
        }
        if (maxSupportedResW < 1920) {
            // 1080p is unsupported
            removeValue(fragment, PreferenceConfiguration.RESOLUTION_PREF_STRING, PreferenceConfiguration.RES_1080P, new Runnable() {
                @Override
                public void run() {
                    SharedPreferences prefs = PreferenceManager.getDefaultSharedPreferences(fragment.getActivity());
                    setValue(fragment, PreferenceConfiguration.RESOLUTION_PREF_STRING, PreferenceConfiguration.RES_720P);
                    resetBitrateToDefault(prefs, null, null);
                }
            });
        }
        // Never remove 720p
    }

    // The widest fixed size the AVC or HEVC decoder takes, or 0 when neither tells.
    static int probeDecoderWidths(Context context) {
        // This must be called to do runtime initialization before calling functions that evaluate
        // decoder lists.
        MediaCodecHelper.initialize(context, GlPreferences.readPreferences(context).glRenderer);
        return Math.max(
                decoderWidthSlot(MediaCodecHelper.findProbableSafeDecoder("video/avc", -1), "video/avc", "AVC"),
                decoderWidthSlot(MediaCodecHelper.findProbableSafeDecoder("video/hevc", -1), "video/hevc", "HEVC"));
    }

    private static int decoderWidthSlot(MediaCodecInfo decoder, String mime, String label) {
        if (decoder == null) {
            return 0;
        }
        Range<Integer> widths = decoder.getCapabilitiesForType(mime).getVideoCapabilities().getSupportedWidths();
        LimeLog.info(label + " supported width range: " + widths.getLower() + " - " + widths.getUpper());

        // If 720p is not reported as supported, ignore all results from this API
        if (!widths.contains(1280)) {
            return 0;
        }
        if (widths.contains(3840)) {
            return 3840;
        }
        return widths.contains(1920) ? 1920 : 1280;
    }

    static void resetBitrateToDefault(SharedPreferences prefs, String res, String fps) {
        resetBitrateToDefault(prefs, res, fps, null);
    }

    /**
     * Automatic bitrate on: the bitrate goes back to the default for the resolution, frame rate and
     * codec and follows them again. Off: the bitrate shown stays, as the user's.
     */
    static void setAutomaticBitrate(SharedPreferences prefs, boolean automatic) {
        SharedPreferences.Editor edit = prefs.edit()
                .putBoolean(PreferenceConfiguration.BITRATE_USER_SET_PREF_STRING, !automatic);
        if (automatic || !prefs.contains(PreferenceConfiguration.BITRATE_PREF_STRING)) {
            edit.putInt(PreferenceConfiguration.BITRATE_PREF_STRING,
                    PreferenceConfiguration.getShownDefaultBitrate(prefs, null, null, null));
        }
        edit.apply();
    }

    static void resetBitrateToDefault(SharedPreferences prefs, String res, String fps, String codec) {
        // A bitrate the user picked stays; a default one follows the new resolution, frame rate
        // and codec (PyroWave has its own).
        if (PreferenceConfiguration.isBitrateUserSet(prefs)) {
            return;
        }
        prefs.edit()
                .putInt(PreferenceConfiguration.BITRATE_PREF_STRING,
                        PreferenceConfiguration.getShownDefaultBitrate(prefs, res, fps, codec))
                .putBoolean(PreferenceConfiguration.BITRATE_USER_SET_PREF_STRING, false)
                .apply();
    }
}
