package com.limelight.binding.video;

import android.app.Application;
import android.content.Context;
import android.preference.PreferenceManager;

import com.limelight.preferences.PreferenceConfiguration;
import com.limelight.utils.DeviceResults;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.MediaCodecInfoBuilder;
import org.robolectric.util.ReflectionHelpers;
import org.robolectric.util.ReflectionHelpers.ClassParameter;

import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 34, application = Application.class)
public class DecoderBenchmarkCompatibilityTest {
    private static void storeMode(Application app, int mode) {
        PreferenceManager.getDefaultSharedPreferences(app).edit()
                .putBoolean(PreferenceConfiguration.DISABLE_HEVC_LOW_LATENCY_PREF_STRING,
                        mode == PreferenceConfiguration.HEVC_LOW_LATENCY_OFF)
                .putString(PreferenceConfiguration.HEVC_LOW_LATENCY_MODE_PREF_STRING,
                        mode == PreferenceConfiguration.HEVC_LOW_LATENCY_ON ? "on" : "auto").commit();
    }

    private static String key(int mode) {
        return ReflectionHelpers.callStaticMethod(DecoderBenchmark.class, "key", ClassParameter.from(int.class, mode));
    }

    @Test
    public void changingTheHevcModeInvalidatesTimingsSummaryAndExportInEveryDirection() {
        Application app = RuntimeEnvironment.getApplication();
        String name = "c2.amlogic.hevc.decoder";
        android.media.MediaCodecInfo decoder = MediaCodecInfoBuilder.newBuilder().setName(name).build();
        int[] modes = {PreferenceConfiguration.HEVC_LOW_LATENCY_AUTO, PreferenceConfiguration.HEVC_LOW_LATENCY_ON,
                PreferenceConfiguration.HEVC_LOW_LATENCY_OFF};
        for (int measuredWith : modes) {
            app.getSharedPreferences("decoder_benchmark", Context.MODE_PRIVATE).edit().clear()
                    .putString("key", key(measuredWith)).putLong(name + "|video/hevc", 2500L).commit();
            for (int current : modes) {
                storeMode(app, current);
                boolean valid = current == measuredWith;
                assertEquals(valid ? 2500L : 0L, DecoderBenchmark.result(app, decoder, "video/hevc"));
                assertEquals(valid, DecoderBenchmark.summary(app) != null);
                DeviceResults results = new DeviceResults();
                DecoderBenchmark.addTo(app, results);
                assertEquals(valid, results.decoderBenchmarkMeasured);
                assertEquals(valid ? 1 : 0, results.decoders.size());
            }
        }
    }

    @Test
    public void autoAndOffKeepTheKeysMeasuredBeforeOnExisted() {
        assertTrue(key(PreferenceConfiguration.HEVC_LOW_LATENCY_AUTO).endsWith("|hevc-low-latency-off=false"));
        assertTrue(key(PreferenceConfiguration.HEVC_LOW_LATENCY_OFF).endsWith("|hevc-low-latency-off=true"));
    }
}
