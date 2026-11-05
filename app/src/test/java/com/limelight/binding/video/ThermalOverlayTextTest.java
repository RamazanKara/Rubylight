package com.limelight.binding.video;

import android.app.Activity;
import android.app.Application;
import android.content.res.Resources;

import com.limelight.R;
import com.limelight.preferences.PreferenceConfiguration;

import org.junit.After;
import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

/** The overlay says what heat does to the stream only when it does something, in every language. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 34, application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class ThermalOverlayTextTest {
    private ActivityController<Activity> activity;

    @Before
    public void setUp() {
        ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
        activity = Robolectric.buildActivity(Activity.class).create();
    }

    @After
    public void tearDown() {
        activity.close();
    }

    @Test
    public void eachLevelHasItsOwnWords() {
        assertEquals(R.string.perf_overlay_thermal_hot, MediaCodecDecoderRenderer.thermalState(ThermalMonitor.HOT, false));
        assertEquals(R.string.perf_overlay_thermal_hot_easing, MediaCodecDecoderRenderer.thermalState(ThermalMonitor.HOT, true));
        assertEquals(R.string.perf_overlay_thermal_warm, MediaCodecDecoderRenderer.thermalState(ThermalMonitor.WARM, false));
        assertEquals(R.string.perf_overlay_thermal_warm_holding, MediaCodecDecoderRenderer.thermalState(ThermalMonitor.WARM, true));
        assertEquals(R.string.perf_overlay_thermal_normal, MediaCodecDecoderRenderer.thermalState(ThermalMonitor.NORMAL, false));
        assertEquals(R.string.perf_overlay_thermal_normal, MediaCodecDecoderRenderer.thermalState(ThermalMonitor.NORMAL, true));
    }

    @Test
    public void theLineFollowsWhatHeatIsDoing() {
        MediaCodecDecoderRenderer renderer = new MediaCodecDecoderRenderer(activity.get(), new PreferenceConfiguration(),
                null, 0, false, false, "", null);
        ReflectionHelpers.setField(renderer, "thermalLevel", ThermalMonitor.HOT);
        String plain = ReflectionHelpers.callInstanceMethod(renderer, "thermalStats");
        assertTrue(plain, plain.contains("hot"));
        assertFalse(plain, plain.contains("easing"));

        renderer.setHeatChangesStream(true);
        String easing = ReflectionHelpers.callInstanceMethod(renderer, "thermalStats");
        assertTrue(easing, easing.contains("hot, easing load"));
    }

    @Test
    public void theShortWordsLeadTheLongOnesInEveryLanguage() {
        for (String locale : new String[] {"en", "de", "es", "fr", "it", "ja", "ko", "pl", "pt-rBR", "ru", "tr", "uk", "vi",
                "zh-rCN", "zh-rTW"}) {
            RuntimeEnvironment.setQualifiers(locale);
            Resources res = RuntimeEnvironment.getApplication().getResources();
            String hot = res.getString(R.string.perf_overlay_thermal_hot);
            String easing = res.getString(R.string.perf_overlay_thermal_hot_easing);
            String warm = res.getString(R.string.perf_overlay_thermal_warm);
            String holding = res.getString(R.string.perf_overlay_thermal_warm_holding);
            assertTrue(locale + " " + easing, easing.startsWith(hot) && hot.length() < easing.length());
            assertTrue(locale + " " + holding, holding.startsWith(warm) && warm.length() < holding.length());
        }
    }
}
