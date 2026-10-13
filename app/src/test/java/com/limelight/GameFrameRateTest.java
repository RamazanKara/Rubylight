package com.limelight;

import android.app.Application;
import android.graphics.SurfaceTexture;
import android.media.MediaFormat;
import android.view.Surface;

import com.limelight.binding.video.MediaCodecDecoderRenderer;
import com.limelight.binding.video.MediaCodecHelper;
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
import org.robolectric.util.ReflectionHelpers;

import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class, shadows = GameFrameRateTest.ReconfigureBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class GameFrameRateTest {
    @Test
    @Config(sdk = 36, shadows = FrameRateSurface.class)
    public void automaticArrClearsTheMaximumRefreshFallbackVote() {
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

            ReflectionHelpers.setField(activity, "useArr", true);
            ReflectionHelpers.callInstanceMethod(activity, "voteSurfaceFrameRate",
                    ReflectionHelpers.ClassParameter.from(Surface.class, surface));
            assertEquals(0, vote.frameRate, 0);
            assertEquals(Surface.FRAME_RATE_COMPATIBILITY_DEFAULT, vote.compatibility);
            assertEquals(Surface.CHANGE_FRAME_RATE_ONLY_IF_SEAMLESS, vote.strategy);
        } finally {
            surface.release();
            texture.release();
        }
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
        protected void setFrameRate(float frameRate, int compatibility, int strategy) {
            this.frameRate = frameRate;
            this.compatibility = compatibility;
            this.strategy = strategy;
        }
    }

    private static class ReconfigurableRenderer extends MediaCodecDecoderRenderer {
        ReconfigurableRenderer(Game activity, PreferenceConfiguration prefs) {
            super(activity, prefs, null, 0, false, false, "", null);
            ReflectionHelpers.setField(this, "refreshRate", prefs.fps);
            ReflectionHelpers.setField(this, "configuredFormat", MediaFormat.createVideoFormat("video/avc", 1280, 720));
        }

        @Override public int[] getReconfigureLimits() {
            return new int[] {4096, 4096, 2, 2};
        }

        @Override public boolean prepareForReconfigure(int width, int height) {
            return true;
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
