package com.limelight.binding.video;

import android.app.Activity;
import android.app.Application;
import android.content.res.Resources;
import android.os.Handler;
import android.os.Looper;
import android.os.PowerManager;

import com.limelight.R;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.preferences.PreferenceConfiguration;

import org.junit.After;
import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.Shadows;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowSystemClock;
import org.robolectric.util.ReflectionHelpers;

import java.time.Duration;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

/** The overlay says what heat does to the stream only when it does something, in every language. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 34, application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = {"com.limelight.nvstream.jni", "com.limelight.binding.video.PyroWaveDecoderRenderer"})
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
        assertEquals(R.string.perf_overlay_thermal_hot, MediaCodecDecoderRenderer.thermalState(ThermalMonitor.HOT, false, false));
        assertEquals(R.string.perf_overlay_thermal_hot, MediaCodecDecoderRenderer.thermalState(ThermalMonitor.HOT, true, false));
        assertEquals(R.string.perf_overlay_thermal_hot_easing, MediaCodecDecoderRenderer.thermalState(ThermalMonitor.HOT, false, true));
        assertEquals(R.string.perf_overlay_thermal_warm, MediaCodecDecoderRenderer.thermalState(ThermalMonitor.WARM, false, false));
        assertEquals(R.string.perf_overlay_thermal_warm, MediaCodecDecoderRenderer.thermalState(ThermalMonitor.WARM, false, true));
        assertEquals(R.string.perf_overlay_thermal_warm_holding, MediaCodecDecoderRenderer.thermalState(ThermalMonitor.WARM, true, false));
        assertEquals(R.string.perf_overlay_thermal_normal, MediaCodecDecoderRenderer.thermalState(ThermalMonitor.NORMAL, false, false));
        assertEquals(R.string.perf_overlay_thermal_normal, MediaCodecDecoderRenderer.thermalState(ThermalMonitor.NORMAL, true, true));
    }

    private MediaCodecDecoderRenderer renderer(boolean thermalProtection) {
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.thermalProtection = thermalProtection;
        return new MediaCodecDecoderRenderer(activity.get(), prefs, null, 0, false, false, "", null);
    }

    @Test
    public void theLineFollowsWhatHeatIsDoing() {
        MediaCodecDecoderRenderer renderer = renderer(false);
        ReflectionHelpers.setField(renderer, "thermalLevel", ThermalMonitor.HOT);
        String plain = ReflectionHelpers.callInstanceMethod(renderer, "thermalStats");
        assertTrue(plain, plain.contains("hot"));
        assertFalse(plain, plain.contains("easing"));

        renderer.setHeatEffects(false, true);
        String easing = ReflectionHelpers.callInstanceMethod(renderer, "thermalStats");
        assertTrue(easing, easing.contains("hot, easing load"));
    }

    @Test
    public void automaticQualityAloneHasNoBitrateToHoldWhenWarm() {
        MediaCodecDecoderRenderer renderer = renderer(true);
        ReflectionHelpers.setField(renderer, "thermalLevel", ThermalMonitor.WARM);
        renderer.setHeatEffects(false, true);
        String quality = ReflectionHelpers.callInstanceMethod(renderer, "thermalStats");
        assertTrue(quality, quality.contains("warm"));
        assertFalse(quality, quality.contains("holding"));

        renderer.setHeatEffects(true, true);
        String bitrate = ReflectionHelpers.callInstanceMethod(renderer, "thermalStats");
        assertTrue(bitrate, bitrate.contains("warm, holding bitrate"));
    }

    // PyroWave gives up its forced clocks on a hot device with no controller at all: the once-a-second tick.
    @Test
    public void clocksEasedWithoutAControllerShowAsEasing() {
        MediaCodecDecoderRenderer renderer = renderer(true);
        Runnable tick = ReflectionHelpers.getField(renderer, "updateLatencyStats");
        ReflectionHelpers.setField(renderer, "latencyHandler", new Handler(Looper.getMainLooper()));
        ReflectionHelpers.setField(renderer, "videoFormat", MoonBridge.VIDEO_FORMAT_PYROWAVE);
        PyroWaveDecoderRenderer pyroWave = ReflectionHelpers.getField(renderer, "pyroWaveRenderer");
        ReflectionHelpers.setField(pyroWave, "maxClocksContext", activity.get().getApplicationContext());
        Shadows.shadowOf(activity.get().getSystemService(PowerManager.class))
                .setCurrentThermalStatus(PowerManager.THERMAL_STATUS_MODERATE);
        ShadowSystemClock.advanceBy(Duration.ofMinutes(1));
        tick.run();
        assertEquals("not hot yet", ThermalMonitor.WARM, renderer.getThermalLevel());
        assertTrue(pyroWave.hasMaxClocks());

        Shadows.shadowOf(activity.get().getSystemService(PowerManager.class))
                .setCurrentThermalStatus(PowerManager.THERMAL_STATUS_SEVERE);
        ShadowSystemClock.advanceBy(Duration.ofMinutes(1));
        String plain = ReflectionHelpers.callInstanceMethod(renderer, "thermalStats");
        assertFalse(plain, plain.contains("easing"));
        tick.run();
        assertEquals(ThermalMonitor.HOT, renderer.getThermalLevel());
        assertFalse("the tick gave them up", pyroWave.hasMaxClocks());
        String easing = ReflectionHelpers.callInstanceMethod(renderer, "thermalStats");
        assertTrue(easing, easing.contains("hot, easing load"));
    }

    @Test
    public void hotWithoutClocksOrAControllerStaysPlain() {
        MediaCodecDecoderRenderer renderer = renderer(true);
        Runnable tick = ReflectionHelpers.getField(renderer, "updateLatencyStats");
        ReflectionHelpers.setField(renderer, "latencyHandler", new Handler(Looper.getMainLooper()));
        Shadows.shadowOf(activity.get().getSystemService(PowerManager.class))
                .setCurrentThermalStatus(PowerManager.THERMAL_STATUS_SEVERE);
        ShadowSystemClock.advanceBy(Duration.ofMinutes(1));
        tick.run();
        assertEquals(ThermalMonitor.HOT, renderer.getThermalLevel());
        String plain = ReflectionHelpers.callInstanceMethod(renderer, "thermalStats");
        assertFalse(plain, plain.contains("easing"));
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
