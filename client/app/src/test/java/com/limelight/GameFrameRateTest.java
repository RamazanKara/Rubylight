package com.limelight;

import android.app.Application;
import android.graphics.SurfaceTexture;
import android.media.MediaFormat;
import android.view.Surface;

import com.limelight.binding.video.DisplayFrameRatePolicy;
import com.limelight.binding.video.MediaCodecDecoderRenderer;
import com.limelight.binding.video.MediaCodecHelper;
import com.limelight.binding.video.RefreshSwitchMonitorTest;
import com.limelight.nvstream.AdaptiveBitrateController;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.preferences.PreferenceConfiguration;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.Implementation;
import org.robolectric.annotation.Implements;
import org.robolectric.shadow.api.Shadow;
import org.robolectric.shadows.ShadowSurface;
import org.robolectric.shadows.ShadowSystemClock;
import org.robolectric.util.ReflectionHelpers;

import java.time.Duration;

import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class, shadows = GameFrameRateTest.ReconfigureBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class GameFrameRateTest {
    @Test
    @Config(sdk = 36, shadows = FrameRateSurface.class)
    public void automaticArrReplacesTheMaximumRefreshVoteWithASeamlessOne() {
        Game activity = Robolectric.buildActivity(Game.class).get();
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.vrr = true;
        ReflectionHelpers.setField(activity, "prefConfig", prefs);
        ReflectionHelpers.setField(activity, "panelMaxRefreshRate", 120f);
        SurfaceTexture texture = new SurfaceTexture(0);
        Surface surface = new Surface(texture);
        try {
            FrameRateSurface vote = Shadow.extract(surface);
            ReflectionHelpers.callInstanceMethod(activity, "voteSurfaceFrameRate",
                    ReflectionHelpers.ClassParameter.from(Surface.class, surface));
            assertEquals(120, vote.frameRate, 0);
            assertEquals(Surface.FRAME_RATE_COMPATIBILITY_FIXED_SOURCE, vote.compatibility);
            assertEquals(Surface.CHANGE_FRAME_RATE_ALWAYS, vote.strategy);

            // No stream rate known yet: ARR votes the panel's top rate, seamless only.
            ReflectionHelpers.setField(activity, "useArr", true);
            ReflectionHelpers.callInstanceMethod(activity, "voteSurfaceFrameRate",
                    ReflectionHelpers.ClassParameter.from(Surface.class, surface));
            assertEquals(120, vote.frameRate, 0);
            assertEquals(Surface.FRAME_RATE_COMPATIBILITY_FIXED_SOURCE, vote.compatibility);
            assertEquals(Surface.CHANGE_FRAME_RATE_ONLY_IF_SEAMLESS, vote.strategy);
        } finally {
            surface.release();
            texture.release();
        }
    }

    @Test
    @Config(sdk = {30, 36}, shadows = FrameRateSurface.class)
    public void trueFpsSurfaceVoteUsesExactNegotiatedRateAndFallsBackBeforeNegotiation() {
        for (int exactRate : new int[] {59_940, 119_880}) {
            Game activity = Robolectric.buildActivity(Game.class).get();
            PreferenceConfiguration prefs = new PreferenceConfiguration();
            prefs.videoFormat = PreferenceConfiguration.FormatOption.AUTO;
            prefs.fps = (exactRate + 500) / 1000;
            prefs.framePacing = PreferenceConfiguration.FRAME_PACING_BALANCED;
            prefs.reduceRefreshRate = true;
            ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
            ReconfigurableRenderer renderer = new ReconfigurableRenderer(activity, prefs);
            ReflectionHelpers.setField(activity, "prefConfig", prefs);
            ReflectionHelpers.setField(activity, "decoderRenderer", renderer);
            ReflectionHelpers.setField(activity, "desiredRefreshRate", 144f);
            SurfaceTexture texture = new SurfaceTexture(0);
            Surface surface = new Surface(texture);
            try {
                FrameRateSurface vote = Shadow.extract(surface);
                vote(activity, surface);
                assertEquals(prefs.fps, vote.frameRate, 0);

                renderer.setStreamFrameRateMilliHz(exactRate);
                vote(activity, surface);
                assertEquals(exactRate / 1000f, vote.frameRate, 0);
                assertEquals(Surface.FRAME_RATE_COMPATIBILITY_FIXED_SOURCE, vote.compatibility);
                assertEquals(exactRate / 1000f,
                        (float) ReflectionHelpers.getField(activity, "appSurfaceFrameRateVote"), 0);

                // An output slower than the requested stream also takes the true-FPS branch.
                prefs.reduceRefreshRate = false;
                ReflectionHelpers.setField(activity, "desiredRefreshRate", 50f);
                vote(activity, surface);
                assertEquals(exactRate / 1000f, vote.frameRate, 0);
            } finally {
                surface.release();
                texture.release();
            }
        }
    }

    @Test
    @Config(sdk = 36, shadows = FrameRateSurface.class)
    public void exactStreamRateIsTheArrVoteButNotThePanelVote() {
        Game activity = Robolectric.buildActivity(Game.class).get();
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.videoFormat = PreferenceConfiguration.FormatOption.AUTO;
        prefs.fps = 60;
        prefs.framePacing = PreferenceConfiguration.FRAME_PACING_BALANCED;
        ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
        ReconfigurableRenderer renderer = new ReconfigurableRenderer(activity, prefs);
        renderer.setStreamFrameRateMilliHz(59_940);
        ReflectionHelpers.setField(activity, "prefConfig", prefs);
        ReflectionHelpers.setField(activity, "decoderRenderer", renderer);
        ReflectionHelpers.setField(activity, "desiredRefreshRate", 120f);
        ReflectionHelpers.setField(activity, "panelMaxRefreshRate", 144f);
        SurfaceTexture texture = new SurfaceTexture(0);
        Surface surface = new Surface(texture);
        try {
            FrameRateSurface vote = Shadow.extract(surface);
            vote(activity, surface);
            assertEquals(120f, vote.frameRate, 0);

            prefs.vrr = true;
            vote(activity, surface);
            assertEquals(144f, vote.frameRate, 0);
            assertEquals(Surface.FRAME_RATE_COMPATIBILITY_FIXED_SOURCE, vote.compatibility);
            assertEquals(Surface.CHANGE_FRAME_RATE_ALWAYS, vote.strategy);

            // ARR votes the exact stream rate, seamless only: a 0 vote would leave a game at
            // Android's 60 Hz default.
            ReflectionHelpers.setField(activity, "useArr", true);
            vote(activity, surface);
            assertEquals(59.94f, vote.frameRate, 0.001f);
            assertEquals(Surface.FRAME_RATE_COMPATIBILITY_FIXED_SOURCE, vote.compatibility);
            assertEquals(Surface.CHANGE_FRAME_RATE_ONLY_IF_SEAMLESS, vote.strategy);
        } finally {
            surface.release();
            texture.release();
        }
    }

    @Test
    @Config(sdk = {30, 36}, shadows = FrameRateSurface.class)
    public void completedNegotiationRefreshesTheProvisionalSurfaceVote() {
        Game activity = Robolectric.buildActivity(Game.class).get();
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.videoFormat = PreferenceConfiguration.FormatOption.AUTO;
        prefs.fps = 60;
        prefs.framePacing = PreferenceConfiguration.FRAME_PACING_BALANCED;
        prefs.reduceRefreshRate = true;
        ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
        ReconfigurableRenderer renderer = new ReconfigurableRenderer(activity, prefs);
        ReflectionHelpers.setField(activity, "prefConfig", prefs);
        ReflectionHelpers.setField(activity, "decoderRenderer", renderer);
        ReflectionHelpers.setField(activity, "streamFps", 60);
        ReflectionHelpers.setField(activity, "desiredRefreshRate", 120f);
        ReflectionHelpers.setField(activity, "panelMaxRefreshRate", 120f);
        SurfaceTexture texture = new SurfaceTexture(0);
        Surface surface = new Surface(texture);
        try {
            FrameRateSurface vote = Shadow.extract(surface);
            ReflectionHelpers.setField(activity, "videoSurface", surface);
            ReflectionHelpers.setField(activity, "surfaceCreated", true);
            vote(activity, surface);
            assertEquals(60f, vote.frameRate, 0);

            // NvConnection supplies the negotiated rate after surface creation, before startup
            // tracking begins. Tracking must update the real surface, not only its stored FPS.
            renderer.setStreamFrameRateMilliHz(59_940);
            assertEquals(60f, vote.frameRate, 0);
            ReflectionHelpers.callInstanceMethod(activity, "startReconfigureTracking");

            assertEquals(59.94f, vote.frameRate, 0);
            assertEquals(59.94f, (float) ReflectionHelpers.getField(activity, "appSurfaceFrameRateVote"), 0);
            StreamReconfigureRequester requester = ReflectionHelpers.getField(activity, "reconfigureRequester");
            assertEquals(59_940, requester.fpsMillihz());
        } finally {
            surface.release();
            texture.release();
        }
    }

    @Test
    @Config(sdk = 36, shadows = FrameRateSurface.class)
    public void aStreamRateChangeRedoesTheArrVote() {
        Game activity = Robolectric.buildActivity(Game.class).get();
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.videoFormat = PreferenceConfiguration.FormatOption.AUTO;
        prefs.vrr = true;
        prefs.fps = 120;
        ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
        ReconfigurableRenderer renderer = new ReconfigurableRenderer(activity, prefs);
        renderer.setStreamFrameRateMilliHz(120_000);
        ReflectionHelpers.setField(activity, "prefConfig", prefs);
        ReflectionHelpers.setField(activity, "decoderRenderer", renderer);
        ReflectionHelpers.setField(activity, "useArr", true);
        ReflectionHelpers.setField(activity, "panelMaxRefreshRate", 120f);
        SurfaceTexture texture = new SurfaceTexture(0);
        Surface surface = new Surface(texture);
        try {
            FrameRateSurface vote = Shadow.extract(surface);
            ReflectionHelpers.setField(activity, "videoSurface", surface);
            ReflectionHelpers.setField(activity, "surfaceCreated", true);
            vote(activity, surface);
            assertEquals(120f, vote.frameRate, 0);

            // A half-rate verdict on the old vote does not carry over to the new one.
            DisplayFrameRatePolicy.ArrHalfRateCheck check = ReflectionHelpers.getField(activity, "arrHalfRate");
            for (long t = 0; t <= DisplayFrameRatePolicy.ArrHalfRateCheck.HOLD_MS; t += 1000) {
                check.update(t, 120f, 60f, 60f, 120f);
            }
            assertTrue(check.flagged());

            // A 0x553C rate change or a reconfigure moves the stream to 90 fps: ARR is told at once.
            ReflectionHelpers.callInstanceMethod(activity, "setStreamFrameRate",
                    ReflectionHelpers.ClassParameter.from(int.class, 90_000));
            assertEquals(90f, vote.frameRate, 0);
            assertEquals(Surface.FRAME_RATE_COMPATIBILITY_FIXED_SOURCE, vote.compatibility);
            assertEquals(Surface.CHANGE_FRAME_RATE_ONLY_IF_SEAMLESS, vote.strategy);
            assertFalse(check.flagged());

            ReflectionHelpers.callInstanceMethod(activity, "setStreamFrameRate",
                    ReflectionHelpers.ClassParameter.from(int.class, 59_940));
            assertEquals(59.94f, vote.frameRate, 0.001f);
            assertEquals(Surface.CHANGE_FRAME_RATE_ONLY_IF_SEAMLESS, vote.strategy);
        } finally {
            surface.release();
            texture.release();
        }
    }

    @Test
    @Config(sdk = 34, shadows = {FrameRateSurface.class, RefreshSwitchMonitorTest.FakePanel.class})
    public void vrrContentVoteLowersOnlyForJudderOrWithTheOptIn() {
        // A 60/90/120 phone at 120, VRR without ARR, a host that sends frame times.
        RefreshSwitchMonitorTest.FakePanel.set(RefreshSwitchMonitorTest.mode(3, 120f, 60f, 90f),
                RefreshSwitchMonitorTest.mode(1, 60f, 90f, 120f), RefreshSwitchMonitorTest.mode(2, 90f, 60f, 120f),
                RefreshSwitchMonitorTest.mode(3, 120f, 60f, 90f));
        Game activity = Robolectric.buildActivity(Game.class).get();
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.videoFormat = PreferenceConfiguration.FormatOption.AUTO;
        prefs.vrr = true;
        prefs.fps = 120;
        ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
        ReconfigurableRenderer renderer = new ReconfigurableRenderer(activity, prefs);
        ReflectionHelpers.setField(activity, "prefConfig", prefs);
        ReflectionHelpers.setField(activity, "decoderRenderer", renderer);
        ReflectionHelpers.setField(activity, "panelMaxRefreshRate", 120f);
        SurfaceTexture texture = new SurfaceTexture(0);
        Surface surface = new Surface(texture);
        try {
            FrameRateSurface vote = Shadow.extract(surface);
            ReflectionHelpers.setField(activity, "videoSurface", surface);
            ReflectionHelpers.setField(activity, "surfaceCreated", true);
            vote(activity, surface);
            assertEquals(120f, vote.frameRate, 0);
            assertEquals(Surface.CHANGE_FRAME_RATE_ALWAYS, vote.strategy);

            // No lower refresh rate allowed. A 60 fps game: 120 is a multiple, so the panel stays,
            // and the start-of-stream vote is not made again seamless-only (the status line keeps
            // its reason).
            renderer.contentMilliHz = 60_000;
            ReflectionHelpers.callInstanceMethod(activity, "voteContentRate");
            assertEquals(120f, vote.frameRate, 0);
            assertEquals(Surface.CHANGE_FRAME_RATE_ALWAYS, vote.strategy);
            assertFalse((boolean) ReflectionHelpers.getField(activity, "appSurfaceSeamlessOnly"));

            // A 90 fps game would judder at 120: 90 is a seamless alternative.
            ShadowSystemClock.advanceBy(Duration.ofSeconds(3));
            renderer.contentMilliHz = 90_000;
            ReflectionHelpers.callInstanceMethod(activity, "voteContentRate");
            assertEquals(90f, vote.frameRate, 0);
            assertEquals(Surface.CHANGE_FRAME_RATE_ONLY_IF_SEAMLESS, vote.strategy);

            // A fixed 120 Hz panel: a 45 fps game would want 90, which it lacks.
            RefreshSwitchMonitorTest.FakePanel.set(RefreshSwitchMonitorTest.mode(3, 120f),
                    RefreshSwitchMonitorTest.mode(3, 120f));
            ShadowSystemClock.advanceBy(Duration.ofSeconds(3));
            renderer.contentMilliHz = 45_000;
            ReflectionHelpers.callInstanceMethod(activity, "voteContentRate");
            assertEquals(120f, vote.frameRate, 0);

            // A lower refresh rate allowed: the vote without VRR, the game's own 60.
            prefs.reduceRefreshRate = true;
            ShadowSystemClock.advanceBy(Duration.ofSeconds(3));
            renderer.contentMilliHz = 60_000;
            ReflectionHelpers.callInstanceMethod(activity, "voteContentRate");
            assertEquals(60f, vote.frameRate, 0);
            assertEquals(Surface.CHANGE_FRAME_RATE_ONLY_IF_SEAMLESS, vote.strategy);
        } finally {
            RefreshSwitchMonitorTest.FakePanel.clear();
            surface.release();
            texture.release();
        }
    }

    private static void vote(Game activity, Surface surface) {
        ReflectionHelpers.callInstanceMethod(activity, "voteSurfaceFrameRate",
                ReflectionHelpers.ClassParameter.from(Surface.class, surface));
    }

    @Test
    public void failedReconfigureKeepsFrameRateAndSuccessfulRetryUpdatesExistingPolicies() {
        Game activity = Robolectric.buildActivity(Game.class).get();
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.videoFormat = PreferenceConfiguration.FormatOption.AUTO;
        prefs.fps = 60;
        ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
        ReconfigurableRenderer renderer = new ReconfigurableRenderer(activity, prefs);
        AdaptiveBitrateController bitrate = new AdaptiveBitrateController(20000, 16000, 60, 0);
        ReflectionHelpers.setField(activity, "prefConfig", prefs);
        ReflectionHelpers.setField(activity, "decoderRenderer", renderer);
        ReflectionHelpers.setField(activity, "adaptiveBitrate", bitrate);
        ReflectionHelpers.setField(activity, "connected", true);
        ReflectionHelpers.setField(activity, "fpsFollowsDisplay", true);
        ReflectionHelpers.setField(activity, "streamFps", 60);
        ReflectionHelpers.setField(activity, "streamPanelHz", 60f);
        ReflectionHelpers.setField(activity, "panelMaxRefreshRate", 120f);
        StreamReconfigureRequester requester = ReflectionHelpers.getField(activity, "reconfigureRequester");
        requester.start(1280, 720, 60000, 1280, 720, 60000);

        ReconfigureBridge.send = false;
        ReflectionHelpers.callInstanceMethod(activity, "checkStreamReconfigure");
        assertEquals(60, (int) ReflectionHelpers.getField(activity, "streamFps"));
        assertEquals(60, prefs.fps);
        assertEquals(60, (int) ReflectionHelpers.getField(renderer, "refreshRate"));
        assertEquals(60000, requester.fpsMillihz());
        assertEquals(0, bitrate.sample(16000, true, false, 0, 10));

        ReconfigureBridge.send = true;
        ReflectionHelpers.callInstanceMethod(activity, "checkStreamReconfigure");
        int streamFps = ReflectionHelpers.getField(activity, "streamFps");
        assertEquals(120, streamFps);
        assertEquals(120, prefs.fps);
        assertEquals(120, (int) ReflectionHelpers.getField(renderer, "refreshRate"));
        assertEquals(120000, requester.fpsMillihz());
        assertSame(bitrate, ReflectionHelpers.getField(activity, "adaptiveBitrate"));
        assertEquals(0, bitrate.sample(17000, true, false, 0, 10));
        assertEquals(12800, bitrate.sample(18000, true, false, 0, 10));
    }

    @Implements(Surface.class)
    public static class FrameRateSurface extends ShadowSurface {
        float frameRate;
        int compatibility;
        int strategy;

        @Implementation
        protected boolean isValid() {
            return true;
        }

        @Implementation
        protected void setFrameRate(float frameRate, int compatibility) {
            this.frameRate = frameRate;
            this.compatibility = compatibility;
        }

        @Implementation
        protected void setFrameRate(float frameRate, int compatibility, int strategy) {
            this.frameRate = frameRate;
            this.compatibility = compatibility;
            this.strategy = strategy;
        }
    }

    private static class ReconfigurableRenderer extends MediaCodecDecoderRenderer {
        int contentMilliHz;

        ReconfigurableRenderer(Game activity, PreferenceConfiguration prefs) {
            super(activity, prefs, null, 0, false, false, "", null);
            ReflectionHelpers.setField(this, "refreshRate", prefs.fps);
            ReflectionHelpers.setField(this, "configuredFormat", MediaFormat.createVideoFormat("video/avc", 1280, 720));
        }

        @Override public int[] getReconfigureLimits() {
            return new int[] {4096, 4096, 2, 2};
        }

        @Override public int[] getStreamSize() {
            return new int[] {1280, 720};
        }

        @Override public boolean prepareForReconfigure(int width, int height) {
            return true;
        }

        @Override public int getContentRateMilliHz() {
            return contentMilliHz;
        }
    }

    @Implements(value = MoonBridge.class, isInAndroidSdk = false)
    public static class ReconfigureBridge {
        static boolean send;

        @Implementation protected static void __staticInitializer__() { }

        @Implementation public static boolean hostSupportsControlMessage(int messageType) {
            return true;
        }

        @Implementation public static boolean requestReconfigure(int width, int height, int fpsMillihz) {
            return send;
        }
    }
}
