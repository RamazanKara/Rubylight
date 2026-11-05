package com.limelight;

import android.app.Activity;
import android.app.Application;

import com.limelight.binding.video.MediaCodecDecoderRenderer;
import com.limelight.nvstream.AdaptiveBitrateController;
import com.limelight.nvstream.AdaptiveQualityController;
import com.limelight.nvstream.NvConnection;
import com.limelight.nvstream.StreamConfiguration;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvApp;
import com.limelight.preferences.PreferenceConfiguration;

import org.junit.After;
import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowLooper;
import org.robolectric.shadows.ShadowSystemClock;
import org.robolectric.util.ReflectionHelpers;

import java.io.IOException;
import java.time.Duration;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

/** What the stream's once-a-second runnables hand on: to the renderer, and to the problem report. */
@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class,
        shadows = PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class GameStreamWiringTest {
    private ActivityController<Activity> activity;
    private Game game;
    private PreferenceConfiguration prefs;
    private MediaCodecDecoderRenderer renderer;

    @Before
    public void setUp() {
        ReflectionHelpers.setStaticField(com.limelight.binding.video.MediaCodecHelper.class, "initialized", true);
        activity = Robolectric.buildActivity(Activity.class).create();
        prefs = new PreferenceConfiguration();
        prefs.thermalProtection = true;
        prefs.bitrate = 63_000;
        renderer = new MediaCodecDecoderRenderer(activity.get(), prefs, null, 0, false, false, "", null);
        game = Robolectric.buildActivity(Game.class).get();
        ReflectionHelpers.setField(game, "prefConfig", prefs);
        ReflectionHelpers.setField(game, "decoderRenderer", renderer);
        ReflectionHelpers.setField(game, "conn", new FakeConnection());
        ReflectionHelpers.setField(game, "app", new NvApp("Game", 73, false));
        ReflectionHelpers.setField(game, "connected", true);
    }

    @After
    public void tearDown() {
        activity.close();
    }

    private boolean holdsBitrate() {
        return ReflectionHelpers.getField(renderer, "heatHoldsBitrate");
    }

    private boolean easesLoad() {
        return ReflectionHelpers.getField(renderer, "heatEasesLoad");
    }

    private static AdaptiveBitrateController bitrateController() {
        return new AdaptiveBitrateController(50000, 38988, 60, 0);
    }

    @Test
    public void theHealthTickTellsTheRendererWhatHeatDoesToThisStream() {
        ReflectionHelpers.setField(game, "adaptiveBitrate", bitrateController());
        game.adaptation.sampleStreamHealth.run();
        assertTrue(holdsBitrate());
        assertTrue(easesLoad());

        ReflectionHelpers.setField(game, "adaptiveBitrate", null);
        ReflectionHelpers.setField(game, "adaptiveQuality", new AdaptiveQualityController(
                AdaptiveQualityController.Mode.MOTION, 1920, 1080, 60, 720, 30, 2, 2, 0));
        game.adaptation.sampleStreamHealth.run();
        assertFalse("automatic quality has no bitrate to hold", holdsBitrate());
        assertTrue(easesLoad());

        prefs.thermalProtection = false;
        game.adaptation.sampleStreamHealth.run();
        assertFalse(holdsBitrate());
        assertFalse(easesLoad());
    }

    @Test
    public void theHealthTickGivesTheRendererTheBitrateAsItMoves() {
        game.adaptation.sampleStreamHealth.run();
        assertEquals("nothing moved it yet", 63_000, (int) ReflectionHelpers.getField(renderer, "streamBitrateKbps"));
        ReflectionHelpers.setField(game, "currentBitrate", 40_000);
        game.adaptation.sampleStreamHealth.run();
        assertEquals(40_000, (int) ReflectionHelpers.getField(renderer, "streamBitrateKbps"));
    }

    // The line in Game itself, with the controller's own reason: a problem report has to say why.
    @Test
    public void whyTheDeviceChangedTheBitrateReachesTheReport() throws InterruptedException {
        ReflectionHelpers.setField(game, "adaptiveBitrate", bitrateController());
        ReflectionHelpers.setField(game, "currentBitrate", 41_234);
        ReflectionHelpers.setField(game, "poorConnection", true);
        ShadowSystemClock.advanceBy(Duration.ofMinutes(1));
        game.updateBitrate.run();
        ShadowSystemClock.advanceBy(Duration.ofSeconds(5));
        game.updateBitrate.run();
        long deadline = System.nanoTime() + 2_000_000_000L;
        while ((boolean) ReflectionHelpers.getField(game, "hostActionInProgress") && System.nanoTime() < deadline) {
            ShadowLooper.idleMainLooper();
            Thread.sleep(5);
        }
        String log = LimeLog.getRedactedLog();
        assertTrue(log, log.matches("(?s).*Device controls bitrate: 41234 -> \\d+ kbps \\(connection warning\\).*"));
    }

    private static class FakeConnection extends NvConnection {
        FakeConnection() {
            super(null, new ComputerDetails.AddressTuple("192.0.2.1", 47989), 47984, "test",
                    new StreamConfiguration.Builder().build(), null, null);
        }

        @Override public int setBitrate(int bitrate) throws IOException {
            return bitrate;
        }
    }
}
