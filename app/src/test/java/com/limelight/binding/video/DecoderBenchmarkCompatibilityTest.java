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
    @Test
    public void changingHevcFlagsInvalidatesTimingsSummaryAndExportInBothDirections() {
        Application app = RuntimeEnvironment.getApplication();
        String name = "c2.amlogic.hevc.decoder";
        android.media.MediaCodecInfo decoder = MediaCodecInfoBuilder.newBuilder().setName(name).build();
        for (boolean measuredWithFlagsDisabled : new boolean[] {false, true}) {
            String key = ReflectionHelpers.callStaticMethod(DecoderBenchmark.class, "key",
                    ClassParameter.from(boolean.class, measuredWithFlagsDisabled),
                    ClassParameter.from(boolean.class, true));
            app.getSharedPreferences("decoder_benchmark", Context.MODE_PRIVATE).edit().clear()
                    .putString("key", key).putLong(name + "|video/hevc", 2500L).commit();
            PreferenceManager.getDefaultSharedPreferences(app).edit()
                    .putBoolean(PreferenceConfiguration.DISABLE_HEVC_LOW_LATENCY_PREF_STRING,
                            measuredWithFlagsDisabled).commit();
            assertEquals(2500L, DecoderBenchmark.result(app, decoder, "video/hevc"));
            assertNotNull(DecoderBenchmark.summary(app));
            DeviceResults measured = new DeviceResults();
            DecoderBenchmark.addTo(app, measured);
            assertTrue(measured.decoderBenchmarkMeasured);
            assertEquals(1, measured.decoders.size());

            PreferenceManager.getDefaultSharedPreferences(app).edit()
                    .putBoolean(PreferenceConfiguration.DISABLE_HEVC_LOW_LATENCY_PREF_STRING,
                            !measuredWithFlagsDisabled).commit();
            assertEquals(0L, DecoderBenchmark.result(app, decoder, "video/hevc"));
            assertNull(DecoderBenchmark.summary(app));
            DeviceResults stale = new DeviceResults();
            DecoderBenchmark.addTo(app, stale);
            assertFalse(stale.decoderBenchmarkMeasured);
            assertTrue(stale.decoders.isEmpty());
        }
    }

    @Test
    public void changingChipsetTuningInvalidatesTimingsInBothDirections() {
        Application app = RuntimeEnvironment.getApplication();
        String name = "c2.mtk.hevc.decoder";
        android.media.MediaCodecInfo decoder = MediaCodecInfoBuilder.newBuilder().setName(name).build();
        for (boolean measuredWithTuning : new boolean[] {true, false}) {
            String key = ReflectionHelpers.callStaticMethod(DecoderBenchmark.class, "key",
                    ClassParameter.from(boolean.class, false),
                    ClassParameter.from(boolean.class, measuredWithTuning));
            app.getSharedPreferences("decoder_benchmark", Context.MODE_PRIVATE).edit().clear()
                    .putString("key", key).putLong(name + "|video/hevc", 2500L).commit();
            PreferenceManager.getDefaultSharedPreferences(app).edit().clear()
                    .putBoolean(PreferenceConfiguration.CHIPSET_TUNING_PREF_STRING, measuredWithTuning).commit();
            assertEquals(2500L, DecoderBenchmark.result(app, decoder, "video/hevc"));

            PreferenceManager.getDefaultSharedPreferences(app).edit()
                    .putBoolean(PreferenceConfiguration.CHIPSET_TUNING_PREF_STRING, !measuredWithTuning).commit();
            assertEquals(0L, DecoderBenchmark.result(app, decoder, "video/hevc"));
        }
        // Unset means on, the default.
        String onKey = ReflectionHelpers.callStaticMethod(DecoderBenchmark.class, "key",
                ClassParameter.from(boolean.class, false), ClassParameter.from(boolean.class, true));
        app.getSharedPreferences("decoder_benchmark", Context.MODE_PRIVATE).edit().clear()
                .putString("key", onKey).putLong(name + "|video/hevc", 2500L).commit();
        PreferenceManager.getDefaultSharedPreferences(app).edit().clear().commit();
        assertEquals(2500L, DecoderBenchmark.result(app, decoder, "video/hevc"));
    }
}
