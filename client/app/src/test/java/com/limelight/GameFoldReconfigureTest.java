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
        // A size other than the screen's (one the host answered with, say) stays while the screen stays the same.
        Game activity = connectedNativeStream("800x720");
        ReflectionHelpers.callInstanceMethod(activity.videoOutput, "startReconfigureTracking");
        ShadowLooper.idleMainLooper(2, TimeUnit.SECONDS);
        assertTrue(GameRotateReconfigureTest.ReconfigureBridge.requests.isEmpty());
    }

    @Test
    public void streamDoctorResizeIsNotRevertedByCheckStreamReconfigureAndFactsUsesVideoWindow() {
        Game activity = connectedNativeStream("800x720");
        activity.prefConfig.width = 800;
        activity.prefConfig.height = 720;
        ReflectionHelpers.setField(activity, "streamWidth", 800);
        ReflectionHelpers.setField(activity, "streamHeight", 720);
        ReflectionHelpers.setField(activity, "app", new com.limelight.nvstream.http.NvApp("Game", 1, false));
        ReflectionHelpers.setField(activity, "conn", new com.limelight.nvstream.NvConnection(
                activity, new com.limelight.nvstream.http.ComputerDetails.AddressTuple("127.0.0.1", 47989),
                0, "id", new com.limelight.nvstream.StreamConfiguration.Builder().setApp(activity.app).build(),
                null, null));
        ReflectionHelpers.callInstanceMethod(activity.videoOutput, "startReconfigureTracking");
        ShadowLooper.idleMainLooper(2, TimeUnit.SECONDS);
        assertTrue(GameRotateReconfigureTest.ReconfigureBridge.requests.isEmpty());

        GameStreamDoctor doctor = ReflectionHelpers.getField(activity.menuActions, "doctor");
        assertTrue(doctor.resize(640, 576));
        assertEquals(1, GameRotateReconfigureTest.ReconfigureBridge.requests.size());
        assertArrayEquals(new int[] {640, 576, 60000}, GameRotateReconfigureTest.ReconfigureBridge.requests.get(0));

        // A later reconfigure check must keep the lowered resolution instead of snapping back to 800x720.
        ReflectionHelpers.callInstanceMethod(activity.videoOutput, "checkStreamReconfigure");
        assertEquals("Doctor resize must not be reverted by checkStreamReconfigure",
                1, GameRotateReconfigureTest.ReconfigureBridge.requests.size());

        // On an external presentation, facts() must report the presentation window's size, not the phone window's.
        int extId = ShadowDisplayManager.addDisplay("w1920dp-h1080dp");
        Display external = activity.getSystemService(android.hardware.display.DisplayManager.class).getDisplay(extId);
        com.limelight.ui.StreamPresentation presentation = new com.limelight.ui.StreamPresentation(activity, external, false);
        presentation.show();
        presentation.getWindow().getDecorView().layout(0, 0, 1920, 1080);
        activity.getWindow().getDecorView().layout(0, 0, 800, 720);
        ReflectionHelpers.setField(activity, "externalPresentation", presentation);
        try {
            com.limelight.binding.video.StreamDoctorController.Facts facts = doctor.facts();
            assertNotNull(facts);
            assertEquals(1920, facts.screenWidth);
            assertEquals(1080, facts.screenHeight);
        } finally {
            presentation.dismiss();
        }
    }

    /**
     * A plain size saved as the cover screen's own, as settings from before the follow-the-screen choice hold it,
     * launched on the inner screen: it follows the screen, as it did before; a custom size keeps its size.
     */
    @Test
    @Config(sdk = 30)
    public void theCoversOwnSizeSavedEarlierFollowsTheScreenFromTheInnerScreen() {
        Display display = ShadowDisplay.getDefaultDisplay();
        int id = display.getMode().getModeId();
        ShadowDisplayManager.setSupportedModes(display.getDisplayId(), mode(id, 720, 800), mode(id + 1, COVER_HEIGHT, COVER_WIDTH));
        org.robolectric.Shadows.shadowOf(org.robolectric.RuntimeEnvironment.getApplication().getPackageManager())
                .setSystemFeature(android.content.pm.PackageManager.FEATURE_SENSOR_HINGE_ANGLE, true);
        Game cover = GameQualityTest.launch(Game.class, COVER_WIDTH + "x" + COVER_HEIGHT, false);
        assertEquals("the inner screen is in use", "800x720", PreferenceConfiguration.nativeResolution(cover));
        assertTrue(ReflectionHelpers.<Boolean>getField(cover, "nativeResolutionStream"));
        cover.finish();
        Game custom = GameQualityTest.launch(Game.class, "1600x900", false);
        assertFalse(ReflectionHelpers.<Boolean>getField(custom, "nativeResolutionStream"));
        custom.finish();
    }

    private static Display.Mode mode(int id, int width, int height) {
        return ReflectionHelpers.callConstructor(Display.Mode.class,
                ReflectionHelpers.ClassParameter.from(int.class, id),
                ReflectionHelpers.ClassParameter.from(int.class, width),
                ReflectionHelpers.ClassParameter.from(int.class, height),
                ReflectionHelpers.ClassParameter.from(float.class, 60f));
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
