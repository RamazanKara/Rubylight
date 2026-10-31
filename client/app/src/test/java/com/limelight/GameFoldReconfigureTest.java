package com.limelight;

import android.app.Application;
import android.media.MediaFormat;
import android.view.Display;

import com.limelight.binding.video.MediaCodecDecoderRenderer;
import com.limelight.binding.video.MediaCodecHelper;
import com.limelight.preferences.PreferenceConfiguration;

import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowDisplay;
import org.robolectric.shadows.ShadowDisplayManager;
import org.robolectric.shadows.ShadowLooper;
import org.robolectric.util.ReflectionHelpers;

import java.util.concurrent.TimeUnit;

import static org.junit.Assert.*;

/**
 * A native stream asked for on a foldable's cover screen, unfolded while it connected: the inner
 * screen (here 800x720) is in use by the time the stream starts, but the decoder has the cover size.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class, shadows = GameRotateReconfigureTest.ReconfigureBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni", qualifiers = "w800dp-h720dp-land-mdpi")
public class GameFoldReconfigureTest {
    private static final int COVER_WIDTH = 2520, COVER_HEIGHT = 1080;

    @Before public void innerScreenInUse() {
        GameRotateReconfigureTest.ReconfigureBridge.requests.clear();
        Display display = ShadowDisplay.getDefaultDisplay();
        Display.Mode inner = ReflectionHelpers.callConstructor(Display.Mode.class,
                ReflectionHelpers.ClassParameter.from(int.class, display.getMode().getModeId()),
                ReflectionHelpers.ClassParameter.from(int.class, 720),
                ReflectionHelpers.ClassParameter.from(int.class, 800),
                ReflectionHelpers.ClassParameter.from(float.class, 60f));
        ShadowDisplayManager.setSupportedModes(display.getDisplayId(), inner);
    }

    private static Game connectedNativeStream(String launchPanel) {
        Game activity = Robolectric.buildActivity(Game.class).get();
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.videoFormat = PreferenceConfiguration.FormatOption.AUTO;
        prefs.fps = 60;
        prefs.width = COVER_WIDTH;
        prefs.height = COVER_HEIGHT;
        prefs.resolutionFollowsScreen = true;
        ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
        ReflectionHelpers.setField(activity, "prefConfig", prefs);
        ReflectionHelpers.setField(activity, "decoderRenderer", new Renderer(activity, prefs));
        ReflectionHelpers.setField(activity, "nativeResolutionStream", true);
        ReflectionHelpers.setField(activity.videoOutput, "launchWidth", COVER_WIDTH);
        ReflectionHelpers.setField(activity.videoOutput, "launchHeight", COVER_HEIGHT);
        ReflectionHelpers.setField(activity.videoOutput, "launchPanel", launchPanel);
        ReflectionHelpers.setField(activity, "streamFps", 60);
        ReflectionHelpers.setField(activity, "connected", true);
        return activity;
    }

    @Test
    public void anUnfoldWhileConnectingAsksForTheInnerScreensSize() {
        Game activity = connectedNativeStream(COVER_WIDTH + "x" + COVER_HEIGHT);
        ReflectionHelpers.callInstanceMethod(activity.videoOutput, "startReconfigureTracking");
        ShadowLooper.idleMainLooper(2, TimeUnit.SECONDS);
        assertEquals(1, GameRotateReconfigureTest.ReconfigureBridge.requests.size());
        assertArrayEquals(new int[] {800, 720, 60000}, GameRotateReconfigureTest.ReconfigureBridge.requests.get(0));

        // Once resized, nothing more is asked.
        ReflectionHelpers.callInstanceMethod(activity.videoOutput, "checkStreamReconfigure");
        assertEquals(1, GameRotateReconfigureTest.ReconfigureBridge.requests.size());
    }

    @Test
    public void aStreamStartedOnTheScreenInUseAsksForNothing() {
        // The cutout-safe or a custom size keeps its own size while the screen stays the same.
        Game activity = connectedNativeStream("800x720");
        ReflectionHelpers.callInstanceMethod(activity.videoOutput, "startReconfigureTracking");
        ShadowLooper.idleMainLooper(2, TimeUnit.SECONDS);
        assertTrue(GameRotateReconfigureTest.ReconfigureBridge.requests.isEmpty());
    }

    private static class Renderer extends MediaCodecDecoderRenderer {
        Renderer(Game activity, PreferenceConfiguration prefs) {
            super(activity, prefs, null, 0, false, false, "", null);
            ReflectionHelpers.setField(this, "configuredFormat",
                    MediaFormat.createVideoFormat("video/avc", COVER_WIDTH, COVER_HEIGHT));
        }

        @Override public int[] getStreamSize() {
            return new int[] {COVER_WIDTH, COVER_HEIGHT};
        }

        @Override public int[] getReconfigureLimits() {
            return new int[] {4096, 4096, 2, 2};
        }

        @Override public boolean prepareForReconfigure(int width, int height) {
            return true;
        }
    }
}
