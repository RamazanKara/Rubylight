package com.limelight;

import android.app.Application;
import android.content.Context;
import android.content.Intent;
import android.hardware.display.DisplayManager;
import android.os.Handler;
import android.os.SystemClock;
import android.view.Display;
import com.limelight.binding.video.MediaCodecDecoderRenderer;
import com.limelight.binding.video.MediaCodecHelper;
import com.limelight.nvstream.AdaptiveQualityController;
import com.limelight.nvstream.DecodeTiming;
import com.limelight.nvstream.NvConnection;
import com.limelight.nvstream.StreamConfiguration;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.preferences.PreferenceConfiguration;
import com.limelight.ui.StreamPresentation;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.Implementation;
import org.robolectric.annotation.Implements;
import org.robolectric.shadows.ShadowDisplayManager;
import org.robolectric.shadows.ShadowLooper;
import org.robolectric.shadows.ShadowSystemClock;
import org.robolectric.util.ReflectionHelpers;
import static org.junit.Assert.*;

@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class GameQualityTest {
    @Test
    @Config(shadows = QualityBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void fractionalReconfigureChangesRendererOnlyAfterTheRequestIsSent() {
        Game game = reconfigurableGame();
        MediaCodecDecoderRenderer renderer = ReflectionHelpers.getField(game, "decoderRenderer");
        renderer.setStreamFrameRateMilliHz(119880);
        StreamReconfigureRequester requester = ReflectionHelpers.getField(game, "reconfigureRequester");
        requester.start(1920, 1080, 119880, 1920, 1080, 119880);
        for (boolean accepted : new boolean[] {false, true}) {
            QualityBridge.send = accepted;
            assertEquals(accepted, (boolean) ReflectionHelpers.callInstanceMethod(game, "requestStreamReconfigure",
                    ReflectionHelpers.ClassParameter.from(int[].class, new int[] {1920, 1080, 59940})));
            assertEquals(accepted ? 59940 : 119880, renderer.getStreamFrameRateMilliHz());
            assertEquals(accepted ? 59940 : 119880, requester.fpsMillihz());
        }
        clearQualityCallbacks(game);
    }

    @Test
    @Config(shadows = QualityBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void fractionalQualityCeilingSurvivesReductionAndRecovery() {
        Game game = reconfigurableGame();
        ReflectionHelpers.setField(game, "qualityMaxFrameRateMilliHz", 119880);
        PreferenceConfiguration config = ReflectionHelpers.getField(game, "prefConfig");
        config.adaptiveQualityMode = AdaptiveQualityController.Mode.DETAIL;
        try {
            ReflectionHelpers.callInstanceMethod(game, "startAdaptiveQuality");
            AdaptiveQualityController quality = ReflectionHelpers.getField(game, "adaptiveQuality");
            assertEquals(119880, QualityBridge.lastRate);
            long now = SystemClock.uptimeMillis() + 15000;
            AdaptiveQualityController.Target lower = pressuredTarget(quality, now);
            ReflectionHelpers.callInstanceMethod(game, "applyAdaptiveQuality",
                    ReflectionHelpers.ClassParameter.from(AdaptiveQualityController.Target.class, lower),
                    ReflectionHelpers.ClassParameter.from(long.class, now + 3000));
            assertEquals(90000, QualityBridge.lastRate);
            ReflectionHelpers.callInstanceMethod(game, "applyAdaptiveQuality",
                    ReflectionHelpers.ClassParameter.from(AdaptiveQualityController.Target.class, quality.ceiling()),
                    ReflectionHelpers.ClassParameter.from(long.class, now + 40000));
            assertEquals(119880, QualityBridge.lastRate);
            assertEquals(119880, (int) ReflectionHelpers.callInstanceMethod(game, "qualityCeilingFrameRateMilliHz"));
        } finally { clearQualityCallbacks(game); }
    }

    @Test
    @Config(shadows = QualityBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void fractionalOnlyPanelChangeReachesAutomaticQualityAndTracking() {
        Game game = reconfigurableGame();
        MediaCodecDecoderRenderer renderer = ReflectionHelpers.getField(game, "decoderRenderer");
        renderer.setStreamFrameRateMilliHz(120000);
        ReflectionHelpers.callInstanceMethod(game, "startReconfigureTracking");
        StreamReconfigureRequester requester = ReflectionHelpers.getField(game, "reconfigureRequester");
        assertEquals(120000, requester.fpsMillihz());
        PreferenceConfiguration config = ReflectionHelpers.getField(game, "prefConfig");
        config.adaptiveQualityMode = AdaptiveQualityController.Mode.DETAIL;
        ReflectionHelpers.callInstanceMethod(game, "startAdaptiveQuality");
        try {
            // Finish the display-settling delay, then change between rates with the same rounded FPS.
            ShadowLooper.idleMainLooper(StreamReconfigureRequester.SETTLE_MS,
                    java.util.concurrent.TimeUnit.MILLISECONDS);
            clearPendingQualityTarget(game);
            int before = QualityBridge.requests;
            ReflectionHelpers.setField(game, "panelMaxRefreshRate", 119.88f);
            ReflectionHelpers.callInstanceMethod(game, "checkStreamReconfigure");
            assertEquals(before + 1, QualityBridge.requests);
            assertEquals(119880, QualityBridge.lastRate);
            assertEquals(119880, renderer.getStreamFrameRateMilliHz());
            assertEquals(119880, (int) ReflectionHelpers.callInstanceMethod(game, "qualityCeilingFrameRateMilliHz"));
        } finally { clearQualityCallbacks(game); }
    }

    @Test
    @Config(shadows = QualityBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void qualityTicksUseDecoderTailAndResetEvidenceAcrossSuspensionAndPendingReconfigure() {
        for (int suspension = 0; suspension < 3; suspension++) {
            Game game = reconfigurableGame();
            PreferenceConfiguration config = ReflectionHelpers.getField(game, "prefConfig");
            TimingRenderer renderer = new TimingRenderer(game, config);
            ReflectionHelpers.setField(game, "decoderRenderer", renderer);
            AdaptiveQualityController quality = new AdaptiveQualityController(AdaptiveQualityController.Mode.DETAIL,
                    1920, 1080, 120, 1080, 30, 2, 2, SystemClock.uptimeMillis() - 20000);
            ReflectionHelpers.setField(game, "adaptiveQuality", quality);
            try {
                for (int i = 0; i < 3; i++) qualityTick(game);
                assertEquals(3, renderer.timingReads);
                assertEquals(120, renderer.sampledFps);
                assertEquals(0, QualityBridge.requests);

                if (suspension == 0) ReflectionHelpers.setField(game, "foreground", false);
                else if (suspension == 1) ReflectionHelpers.setField(game, "streamMenu", new android.app.Dialog(game));
                else ReflectionHelpers.setField(game, "pendingQualityTarget", quality.ceiling());
                qualityTick(game);
                assertEquals("Suspension must not consume a pressure observation", 3, renderer.timingReads);
                assertEquals(1, renderer.timingResets);
                if (suspension == 0) ReflectionHelpers.setField(game, "foreground", true);
                else if (suspension == 1) ReflectionHelpers.setField(game, "streamMenu", null);
                else assertNull(ReflectionHelpers.getField(game, "pendingQualityTarget"));

                // The mean is only 2 ms: a request proves that real updateQuality passed the tail
                // observation through, and four new ticks prove the old three votes were cleared.
                for (int i = 0; i < 3; i++) {
                    qualityTick(game);
                    assertEquals(0, QualityBridge.requests);
                }
                qualityTick(game);
                assertEquals(1, QualityBridge.requests);
                assertEquals(90000, QualityBridge.lastRate);
                assertEquals(90, (int) ReflectionHelpers.getField(game, "streamFps"));
                assertTrue("The accepted target starts a new timing window", renderer.timingResets > 1);
            } finally {
                clearQualityCallbacks(game);
            }
        }
    }

    private static void qualityTick(Game game) {
        clearQualityCallbacks(game);
        ShadowSystemClock.advanceBy(java.time.Duration.ofSeconds(1));
        Runnable update = ReflectionHelpers.getField(game, "updateQuality");
        update.run();
        clearQualityCallbacks(game);
    }

    private static final class TimingRenderer extends MediaCodecDecoderRenderer {
        int timingReads, timingResets, sampledFps;
        TimingRenderer(Game game, PreferenceConfiguration config) {
            super(game, config, null, 0, false, false, "", null);
        }
        @Override public int[] getReconfigureLimits() { return new int[] {4096, 4096, 2, 2}; }
        @Override public boolean prepareForReconfigure(int width, int height) { return true; }
        @Override public int[] getStreamSize() { return new int[] {1920, 1080}; }
        @Override public boolean hasRecentVideoFrames(long nowMs) { return true; }
        @Override public float getNetworkFrameLossPercent() { return 0; }
        @Override public float getDecodeTimeMs() { return 2; }
        @Override public float getQueueDelayMs() { return 0; }
        @Override public int getThermalLevel() { return 0; }
        @Override public void resetDecodeTiming() { timingResets++; }
        @Override public DecodeTiming takeDecodeTiming(int fps) {
            timingReads++;
            sampledFps = fps;
            long end = SystemClock.uptimeMillis() * 1_000_000L;
            return new DecodeTiming(timingResets, end - 1_000_000_000L, end,
                    end - 950_000_000L, end - 50_000_000L, fps, 120, 12);
        }
    }
    @Test
    @Config(shadows = QualityBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void externalRefreshChangesUpdateTheStreamAndQualityCeilingIndependentlyOfThePhone() {
        for (boolean automatic : new boolean[] {false, true}) {
            Game game = reconfigurableGame();
            int displayId = ShadowDisplayManager.addDisplay("w1920dp-h1080dp");
            Display external = ((DisplayManager) game.getSystemService(Context.DISPLAY_SERVICE)).getDisplay(displayId);
            setDisplayMode(external, 1920, 1080, 120);
            StreamPresentation presentation = new StreamPresentation(game, external, false);
            ReflectionHelpers.setField(game, "externalPresentation", presentation);
            ReflectionHelpers.setField(game, "renderView", presentation.videoView());
            ReflectionHelpers.setField(game, "displaySignature",
                    ReflectionHelpers.callInstanceMethod(game, "displaySignature"));
            if (automatic) {
                PreferenceConfiguration config = ReflectionHelpers.getField(game, "prefConfig");
                config.adaptiveQualityMode = AdaptiveQualityController.Mode.DETAIL;
                ReflectionHelpers.callInstanceMethod(game, "startAdaptiveQuality");
            }
            try {
                DisplayManager.DisplayListener listener = ReflectionHelpers.getField(game, "inputDisplayListener");
                for (int fps : new int[] {60, 120}) {
                    setDisplayMode(external, 1920, 1080, fps);
                    listener.onDisplayChanged(displayId);
                    assertEquals(fps, (float) ReflectionHelpers.getField(game, "panelMaxRefreshRate"), 0);
                    ShadowLooper.idleMainLooper(StreamReconfigureRequester.SETTLE_MS,
                            java.util.concurrent.TimeUnit.MILLISECONDS);
                    assertEquals(fps, (int) ReflectionHelpers.getField(game, "streamFps"));
                    assertEquals(fps, (int) ReflectionHelpers.getField(game, "qualityMaxFps"));
                    if (automatic) {
                        AdaptiveQualityController quality = ReflectionHelpers.getField(game, "adaptiveQuality");
                        assertEquals(fps, quality.ceiling().fps);
                        clearPendingQualityTarget(game);
                    }
                }
                assertEquals(2, QualityBridge.requests);
            } finally {
                clearQualityCallbacks(game);
                ShadowDisplayManager.removeDisplay(displayId);
            }
        }
    }

    @Test public void localDisplaySignatureStillTracksNativeResolutionChanges() {
        Game game = Robolectric.buildActivity(Game.class).get();
        Display local = game.getWindowManager().getDefaultDisplay();
        Display.Mode[] originalModes = local.getSupportedModes();
        try {
            setDisplayMode(local, 1080, 1920, 60);
            String before = ReflectionHelpers.callInstanceMethod(game, "displaySignature");
            setDisplayMode(local, 1440, 2560, 60);
            String after = ReflectionHelpers.callInstanceMethod(game, "displaySignature");
            assertNotEquals(before, after);
            assertTrue(after.contains("2560x1440"));
        } finally {
            ShadowDisplayManager.setSupportedModes(local.getDisplayId(), originalModes);
        }
    }

    private static void setDisplayMode(Display display, int width, int height, int fps) {
        Display.Mode mode = ReflectionHelpers.callConstructor(Display.Mode.class,
                ReflectionHelpers.ClassParameter.from(int.class, display.getMode().getModeId()),
                ReflectionHelpers.ClassParameter.from(int.class, width),
                ReflectionHelpers.ClassParameter.from(int.class, height),
                ReflectionHelpers.ClassParameter.from(float.class, (float) fps));
        ShadowDisplayManager.setSupportedModes(display.getDisplayId(), mode);
    }

    @Test
    @Config(shadows = QualityBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void enablingQualityUsesOnlyTheAcceptedDisplayCeiling() {
        for (boolean accepted : new boolean[] {false, true}) {
            Game game = reconfigurableGame();
            QualityBridge.send = accepted;
            ReflectionHelpers.setField(game, "panelMaxRefreshRate", 60f);
            try {
                ReflectionHelpers.callInstanceMethod(game, "checkStreamReconfigure");
                assertEquals(accepted ? 60 : 120, (int) ReflectionHelpers.getField(game, "qualityMaxFps"));
                assertEquals(accepted ? 60f : 120f, (float) ReflectionHelpers.getField(game, "streamPanelHz"), 0);
                assertEquals(1, QualityBridge.requests);
                PreferenceConfiguration config = ReflectionHelpers.getField(game, "prefConfig");
                config.adaptiveQualityMode = AdaptiveQualityController.Mode.DETAIL;
                ReflectionHelpers.callInstanceMethod(game, "startAdaptiveQuality");
                AdaptiveQualityController quality = ReflectionHelpers.getField(game, "adaptiveQuality");
                assertEquals(accepted ? 60 : 120, quality.ceiling().fps);
                assertEquals("Enabling quality must not restore the old display rate", 1, QualityBridge.requests);
            } finally {
                clearQualityCallbacks(game);
            }
        }
    }

    @Test
    @Config(shadows = QualityBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void adaptiveFpsStepDoesNotConsumeAPendingPanelChange() {
        Game game = reconfigurableGame();
        PreferenceConfiguration config = ReflectionHelpers.getField(game, "prefConfig");
        config.adaptiveQualityMode = AdaptiveQualityController.Mode.DETAIL;
        ReflectionHelpers.callInstanceMethod(game, "startAdaptiveQuality");
        AdaptiveQualityController quality = ReflectionHelpers.getField(game, "adaptiveQuality");
        long now = SystemClock.uptimeMillis() + 15000;
        AdaptiveQualityController.Target target = pressuredTarget(quality, now);
        ReflectionHelpers.setField(game, "panelMaxRefreshRate", 60f);
        try {
            assertTrue(ReflectionHelpers.callInstanceMethod(game, "applyAdaptiveQuality",
                    ReflectionHelpers.ClassParameter.from(AdaptiveQualityController.Target.class, target),
                    ReflectionHelpers.ClassParameter.from(long.class, now + 3000)));
            assertEquals(90, (int) ReflectionHelpers.getField(game, "streamFps"));
            assertEquals(120f, (float) ReflectionHelpers.getField(game, "streamPanelHz"), 0);
            clearPendingQualityTarget(game);
            ReflectionHelpers.callInstanceMethod(game, "checkStreamReconfigure");
            assertEquals(60, quality.ceiling().fps);
            assertEquals(60, (int) ReflectionHelpers.getField(game, "qualityMaxFps"));
            assertEquals(60f, (float) ReflectionHelpers.getField(game, "streamPanelHz"), 0);
        } finally {
            clearQualityCallbacks(game);
        }
    }

    @Test
    @Config(shadows = QualityBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void acceptedPanelChangeIsTrackedEvenWhenDegradedFpsIsUnchanged() {
        Game game = reconfigurableGame();
        PreferenceConfiguration config = ReflectionHelpers.getField(game, "prefConfig");
        config.adaptiveQualityMode = AdaptiveQualityController.Mode.DETAIL;
        config.qualityMinFps = 60;
        ReflectionHelpers.callInstanceMethod(game, "startAdaptiveQuality");
        AdaptiveQualityController quality = ReflectionHelpers.getField(game, "adaptiveQuality");
        try {
            for (int i = 1; i <= 2; i++) {
                long now = SystemClock.uptimeMillis() + i * 20000L;
                AdaptiveQualityController.Target target = pressuredTarget(quality, now);
                assertTrue(ReflectionHelpers.callInstanceMethod(game, "applyAdaptiveQuality",
                        ReflectionHelpers.ClassParameter.from(AdaptiveQualityController.Target.class, target),
                        ReflectionHelpers.ClassParameter.from(long.class, now + 3000)));
                clearPendingQualityTarget(game);
            }
            assertEquals(60, (int) ReflectionHelpers.getField(game, "streamFps"));
            ReflectionHelpers.setField(game, "panelMaxRefreshRate", 60f);
            ReflectionHelpers.callInstanceMethod(game, "checkStreamReconfigure");
            assertEquals(60, (int) ReflectionHelpers.getField(game, "streamFps"));
            assertEquals(60f, (float) ReflectionHelpers.getField(game, "streamPanelHz"), 0);
            clearPendingQualityTarget(game);
            ReflectionHelpers.setField(game, "panelMaxRefreshRate", 120f);
            ReflectionHelpers.callInstanceMethod(game, "checkStreamReconfigure");
            assertEquals(120, quality.ceiling().fps);
            assertEquals(120, (int) ReflectionHelpers.getField(game, "qualityMaxFps"));
        } finally {
            clearQualityCallbacks(game);
        }
    }

    @Test
    @Config(shadows = QualityBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void rejectedPanelChangeKeepsTheLastAcceptedQualityCeiling() {
        Game game = reconfigurableGame();
        PreferenceConfiguration config = ReflectionHelpers.getField(game, "prefConfig");
        config.adaptiveQualityMode = AdaptiveQualityController.Mode.DETAIL;
        ReflectionHelpers.callInstanceMethod(game, "startAdaptiveQuality");
        ReflectionHelpers.setField(game, "panelMaxRefreshRate", 60f);
        QualityBridge.send = false;
        try {
            ReflectionHelpers.callInstanceMethod(game, "checkStreamReconfigure");
            assertNull(ReflectionHelpers.getField(game, "adaptiveQuality"));
            assertEquals(120, (int) ReflectionHelpers.getField(game, "qualityMaxFps"));
            assertEquals(120f, (float) ReflectionHelpers.getField(game, "streamPanelHz"), 0);
            assertEquals(120, (int) ReflectionHelpers.getField(game, "streamFps"));
        } finally {
            clearQualityCallbacks(game);
        }
    }

    @Test
    @Config(shadows = QualityBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void automaticQualityUsesAutoBitrateForThisSessionWithoutSavingIt() {
        Game game = reconfigurableGame();
        NvConnection connection = ReflectionHelpers.getField(game, "conn");
        ComputerDetails host = new ComputerDetails();
        host.caps = com.limelight.nvstream.http.TestHosts.rubylight();
        host.encoderBitrateSupported = true;
        ReflectionHelpers.setField(connection, "hostDetails", host);
        PreferenceConfiguration config = ReflectionHelpers.getField(game, "prefConfig");
        config.adaptiveQualityMode = AdaptiveQualityController.Mode.DETAIL;
        try {
            ReflectionHelpers.callInstanceMethod(game, "startAdaptiveQuality");
            assertNotNull("Automatic quality adapts the bitrate during this stream",
                    ReflectionHelpers.getField(game, "adaptiveBitrate"));
            assertFalse("The user's own auto-bitrate choice for this PC must not change",
                    game.getSharedPreferences("AdaptiveBitrate", 0).contains("display-quality-pc"));
        } finally {
            clearQualityCallbacks(game);
        }
    }

    @Test
    @Config(shadows = QualityBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void turningAutomaticQualityOffRestoresTheAutoBitrateTheUserHad() {
        // {auto bitrate before quality started, the person toggles it from the menu meanwhile}
        boolean[][] cases = {{false, false}, {true, false}, {false, true}};
        for (boolean[] c : cases) {
            Game game = reconfigurableGame();
            NvConnection connection = ReflectionHelpers.getField(game, "conn");
            ComputerDetails host = new ComputerDetails();
            host.caps = com.limelight.nvstream.http.TestHosts.rubylight();
            host.encoderBitrateSupported = true;
            ReflectionHelpers.setField(connection, "hostDetails", host);
            PreferenceConfiguration config = ReflectionHelpers.getField(game, "prefConfig");
            config.adaptiveQualityMode = AdaptiveQualityController.Mode.DETAIL;
            try {
                // At stream start, auto bitrate is on only if the person saved it for this PC.
                if (c[0]) setAutoBitrate(game, true, false);
                ReflectionHelpers.callInstanceMethod(game, "startAdaptiveQuality");
                assertNotNull(ReflectionHelpers.getField(game, "adaptiveBitrate"));
                if (c[1]) {
                    // Disable, then enable again from the stream's bitrate menu: now it is the person's choice.
                    setAutoBitrate(game, false, true);
                    setAutoBitrate(game, true, true);
                }
                // The person turns Automatic quality off for this PC from the stream menu.
                game.getSharedPreferences("AdaptiveQuality", 0).edit().putString("display-quality-pc", "off").commit();
                ReflectionHelpers.callInstanceMethod(game, "startAdaptiveQuality");
                assertNull(ReflectionHelpers.getField(game, "adaptiveQuality"));
                assertEquals("before " + c[0] + ", menu " + c[1], c[0] || c[1],
                        ReflectionHelpers.getField(game, "adaptiveBitrate") != null);
            } finally {
                clearQualityCallbacks(game);
                game.getSharedPreferences("AdaptiveQuality", 0).edit().clear().commit();
                game.getSharedPreferences("AdaptiveBitrate", 0).edit().clear().commit();
            }
        }
    }

    @Test
    @Config(shadows = QualityBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void automaticQualityStoppedByAnErrorAlsoReturnsItsAutoBitrate() {
        Game game = reconfigurableGame();
        NvConnection connection = ReflectionHelpers.getField(game, "conn");
        ComputerDetails host = new ComputerDetails();
        host.caps = com.limelight.nvstream.http.TestHosts.rubylight();
        host.encoderBitrateSupported = true;
        ReflectionHelpers.setField(connection, "hostDetails", host);
        PreferenceConfiguration config = ReflectionHelpers.getField(game, "prefConfig");
        config.adaptiveQualityMode = AdaptiveQualityController.Mode.DETAIL;
        try {
            ReflectionHelpers.callInstanceMethod(game, "startAdaptiveQuality");
            assertNotNull(ReflectionHelpers.getField(game, "adaptiveBitrate"));
            // The host rejects a display change: Automatic quality stops for this stream.
            ReflectionHelpers.setField(game, "panelMaxRefreshRate", 60f);
            QualityBridge.send = false;
            ReflectionHelpers.callInstanceMethod(game, "checkStreamReconfigure");
            assertNull(ReflectionHelpers.getField(game, "adaptiveQuality"));
            assertNull(ReflectionHelpers.getField(game, "adaptiveBitrate"));
        } finally {
            clearQualityCallbacks(game);
        }
    }

    private static void setAutoBitrate(Game game, boolean enabled, boolean remember) {
        ReflectionHelpers.callInstanceMethod(game, "setAdaptiveBitrateEnabled",
                ReflectionHelpers.ClassParameter.from(boolean.class, enabled),
                ReflectionHelpers.ClassParameter.from(boolean.class, remember));
    }

    private static AdaptiveQualityController.Target pressuredTarget(AdaptiveQualityController quality, long now) {
        AdaptiveQualityController.Target target = null;
        for (int i = 0; i < 4; i++) target = quality.sample(now + i * 1000, true, false, 0, 2, 0, 0, 2, true);
        assertNotNull(target);
        return target;
    }

    private static void clearPendingQualityTarget(Game game) {
        Runnable update = ReflectionHelpers.getField(game, "updateQuality");
        update.run();
        assertNull(ReflectionHelpers.getField(game, "pendingQualityTarget"));
    }

    private static void clearQualityCallbacks(Game game) {
        Handler handler = ReflectionHelpers.getField(game, "bitrateHandler");
        handler.removeCallbacksAndMessages(null);
    }

    private Game reconfigurableGame() {
        Game game = Robolectric.buildActivity(Game.class).get();
        game.setIntent(new Intent().putExtra(Game.EXTRA_PC_UUID, "display-quality-pc"));
        PreferenceConfiguration config = new PreferenceConfiguration();
        config.videoFormat = PreferenceConfiguration.FormatOption.AUTO;
        config.adaptiveQualityMode = AdaptiveQualityController.Mode.OFF;
        config.width = 1920; config.height = 1080; config.fps = 120;
        config.qualityMinShortEdge = 1080; config.qualityMinFps = 30;
        ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
        MediaCodecDecoderRenderer renderer = new MediaCodecDecoderRenderer(game, config, null, 0,
                false, false, "", null) {
            @Override public int[] getReconfigureLimits() { return new int[] {4096, 4096, 2, 2}; }
            @Override public boolean prepareForReconfigure(int width, int height) { return true; }
            @Override public int[] getStreamSize() { return new int[] {1920, 1080}; }
        };
        ReflectionHelpers.setField(game, "prefConfig", config);
        ReflectionHelpers.setField(game, "decoderRenderer", renderer);
        ReflectionHelpers.setField(game, "connected", true);
        ReflectionHelpers.setField(game, "foreground", true);
        ReflectionHelpers.setField(game, "app", new NvApp("Game", 73, false));
        ReflectionHelpers.setField(game, "conn", new NvConnection(null,
                new ComputerDetails.AddressTuple("192.0.2.1", 47989), 47984, "test",
                new StreamConfiguration.Builder().build(), null, null));
        ReflectionHelpers.setField(game, "fpsFollowsDisplay", true);
        ReflectionHelpers.setField(game, "streamFps", 120);
        ReflectionHelpers.setField(game, "streamPanelHz", 120f);
        ReflectionHelpers.setField(game, "panelMaxRefreshRate", 120f);
        ReflectionHelpers.setField(game, "qualityMaxWidth", 1920);
        ReflectionHelpers.setField(game, "qualityMaxHeight", 1080);
        ReflectionHelpers.setField(game, "qualityMaxFps", 120);
        StreamReconfigureRequester requester = ReflectionHelpers.getField(game, "reconfigureRequester");
        requester.start(1920, 1080, 120000, 1920, 1080, 120000);
        QualityBridge.send = true;
        QualityBridge.requests = 0;
        QualityBridge.lastRate = 0;
        return game;
    }

    @Implements(value = MoonBridge.class, isInAndroidSdk = false)
    public static class QualityBridge {
        static boolean send;
        static int requests;
        static int lastRate;
        @Implementation protected static void __staticInitializer__() {}
        @Implementation public static boolean hostSupportsControlMessage(int messageType) { return true; }
        @Implementation public static long getEstimatedRttInfo() { return 0; }
        @Implementation public static boolean requestReconfigure(int width, int height, int fpsMillihz) {
            requests++;
            lastRate = fpsMillihz;
            return send;
        }
    }

    @Test public void pyrowaveExplainsUnsupportedQualityWithoutStartingBitrateOrReconfiguring() {
        Game game = Robolectric.buildActivity(Game.class).get();
        game.setIntent(new Intent().putExtra(Game.EXTRA_PC_UUID, "quality-pc"));
        PreferenceConfiguration config = new PreferenceConfiguration();
        config.videoFormat = PreferenceConfiguration.FormatOption.FORCE_PYROWAVE;
        config.adaptiveQualityMode = AdaptiveQualityController.Mode.MOTION;
        config.width = 1920; config.height = 1080; config.fps = 60;
        ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
        MediaCodecDecoderRenderer renderer = new MediaCodecDecoderRenderer(game, config, null, 0,
                false, false, "", null);
        ReflectionHelpers.setField(renderer, "videoFormat", MoonBridge.VIDEO_FORMAT_PYROWAVE);
        ReflectionHelpers.setField(game, "prefConfig", config);
        ReflectionHelpers.setField(game, "decoderRenderer", renderer);
        ReflectionHelpers.setField(game, "connected", true);
        ReflectionHelpers.setField(game, "app", new NvApp("Game", 73, false));
        ReflectionHelpers.callInstanceMethod(game, "startAdaptiveQuality");
        assertNull(ReflectionHelpers.getField(game, "adaptiveQuality"));
        assertNull(ReflectionHelpers.getField(game, "adaptiveBitrate"));
        assertNull(ReflectionHelpers.getField(game, "pyroWaveBitrate"));
        assertEquals(game.getString(R.string.quality_unavailable), ReflectionHelpers.getField(game, "qualityStatus"));
    }

    @Test public void offAndPcOverrideDoNotRequireAConnectedHost() {
        Game game = Robolectric.buildActivity(Game.class).get();
        game.setIntent(new Intent().putExtra(Game.EXTRA_PC_UUID, "quality-override"));
        PreferenceConfiguration config = new PreferenceConfiguration();
        config.adaptiveQualityMode = AdaptiveQualityController.Mode.DETAIL;
        ReflectionHelpers.setField(game, "prefConfig", config);
        game.getSharedPreferences("AdaptiveQuality", 0).edit().putString("quality-override", "off").commit();
        ReflectionHelpers.callInstanceMethod(game, "startAdaptiveQuality");
        assertNull(ReflectionHelpers.getField(game, "adaptiveQuality"));
        assertNull(ReflectionHelpers.getField(game, "qualityStatus"));
        game.getSharedPreferences("AdaptiveQuality", 0).edit().remove("quality-override").commit();
        assertEquals(AdaptiveQualityController.Mode.DETAIL, ReflectionHelpers.callInstanceMethod(game, "qualityMode"));
    }
}
