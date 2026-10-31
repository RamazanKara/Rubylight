package com.limelight;

import android.app.Application;
import android.content.Context;
import android.content.Intent;
import android.graphics.SurfaceTexture;
import android.hardware.display.DisplayManager;
import android.view.Display;
import android.view.Surface;

import com.limelight.binding.video.DisplayFrameRatePolicy;
import com.limelight.binding.video.MediaCodecDecoderRenderer;
import com.limelight.binding.video.MediaCodecHelper;
import com.limelight.nvstream.http.NvApp;
import com.limelight.preferences.PreferenceConfiguration;
import com.limelight.ui.StreamPresentation;
import com.limelight.ui.StreamView;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowDisplayManager;
import org.robolectric.util.ReflectionHelpers;

import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;

import static org.junit.Assert.*;

@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class GameDisplayLifecycleTest {
    private static final String OUTPUT_DISPLAY = "rubylight.outputDisplay";

    @Test @Config(sdk = 23)
    public void refreshDiagnosticsAvoidArrApisOnOlderAndroid() {
        Game game = Robolectric.buildActivity(Game.class).get();
        ReflectionHelpers.setField(game, "prefConfig", new PreferenceConfiguration());

        ReflectionHelpers.callInstanceMethod(game.videoOutput, "logDisplayRefreshPolicy",
                ReflectionHelpers.ClassParameter.from(Display.class,
                        game.getWindowManager().getDefaultDisplay()));

        String log = LimeLog.getRedactedLog();
        assertTrue(log.contains("api=23, display=0"));
        assertTrue(log.contains("arrSupported=unavailable, selected=disabled"));
        assertTrue(log.contains("windowMode=0, windowHz=0.0"));
    }

    @Test public void refreshDiagnosticsRunBeforeTheUnchangedDisplaySignatureReturns() {
        Game game = Robolectric.buildActivity(Game.class).get();
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.vrr = true;
        ReflectionHelpers.setField(game, "prefConfig", prefs);
        ReflectionHelpers.setField(game.videoOutput, "displaySignature",
                ReflectionHelpers.callInstanceMethod(game.videoOutput, "displaySignature"));

        ReflectionHelpers.callInstanceMethod(game.videoOutput, "onDisplayModesChanged");
        String first = LimeLog.getRedactedLog();
        assertTrue(first.contains("Display refresh policy: api=29, display=0"));
        assertTrue(first.contains("arrSupported=unavailable, selected=max Hz"));
        ReflectionHelpers.callInstanceMethod(game.videoOutput, "onDisplayModesChanged");
        assertEquals(first, LimeLog.getRedactedLog());
    }

    @Test public void refreshOverlayReadsTheVideoOutputInsteadOfThePhoneDisplay() {
        Game game = Robolectric.buildActivity(Game.class).get();
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.vrr = true;
        ReflectionHelpers.setField(game, "prefConfig", prefs);
        int displayId = ShadowDisplayManager.addDisplay("w1920dp-h1080dp");
        Display external = game.getSystemService(DisplayManager.class).getDisplay(displayId);
        Display.Mode externalMode = ReflectionHelpers.callConstructor(Display.Mode.class,
                ReflectionHelpers.ClassParameter.from(int.class, external.getMode().getModeId()),
                ReflectionHelpers.ClassParameter.from(int.class, 1920),
                ReflectionHelpers.ClassParameter.from(int.class, 1080),
                ReflectionHelpers.ClassParameter.from(float.class, 90f));
        ShadowDisplayManager.setSupportedModes(displayId, externalMode);
        StreamPresentation presentation = new StreamPresentation(game, external, false);
        ReflectionHelpers.setField(game, "externalPresentation", presentation);
        ReflectionHelpers.setField(game, "renderView", presentation.videoView());

        String expected = "\n" + game.getString(R.string.perf_overlay_vrr, external.getMode().getRefreshRate(),
                external.getRefreshRate(), game.getString(R.string.perf_overlay_vrr_max_hz));
        assertEquals(expected, ReflectionHelpers.callInstanceMethod(game.overlays, "displayRefreshOverlayLine"));
        assertEquals(90f, external.getMode().getRefreshRate(), 0);
        assertNotEquals(90f, game.getWindowManager().getDefaultDisplay().getMode().getRefreshRate(), 0);
        prefs.vrr = false;
        assertEquals("", ReflectionHelpers.callInstanceMethod(game.overlays, "displayRefreshOverlayLine"));
    }

    @Test public void missingOutputRestoresLocalSurfaceAndVrrLaunchValues() {
        Game game = Robolectric.buildActivity(Game.class).get();
        game.setIntent(new Intent().putExtra(OUTPUT_DISPLAY, Integer.MAX_VALUE));
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.vrr = true;
        prefs.fps = 240;
        prefs.launchRefreshRateX100 = 24000;
        StreamView local = new StreamView(game);
        ReflectionHelpers.setField(game, "prefConfig", prefs);
        ReflectionHelpers.setField(game, "streamView", local);
        ReflectionHelpers.setField(game, "panelMaxRefreshRate", 240f);

        ReflectionHelpers.callInstanceMethod(game.videoOutput, "initializeVideoOutput");

        float localHz = DisplayFrameRatePolicy.maxRefreshRate(game.getWindowManager().getDefaultDisplay());
        assertFalse(game.getIntent().hasExtra(OUTPUT_DISPLAY));
        assertSame(local, ReflectionHelpers.getField(game, "renderView"));
        assertNull(ReflectionHelpers.getField(game, "externalPresentation"));
        assertNotNull(local.getHolder());
        assertEquals(localHz, (float) ReflectionHelpers.getField(game, "panelMaxRefreshRate"), 0);
        assertEquals(DisplayFrameRatePolicy.streamFrameRate(localHz), prefs.fps);
        assertEquals(Math.round(localHz * 100), prefs.launchRefreshRateX100);
    }

    @Test public void capabilitiesFollowActiveSurfaceUntilOutputReconnectCompletes() {
        Game game = Robolectric.buildActivity(Game.class).get();
        int displayId = ShadowDisplayManager.addDisplay("w1920dp-h1080dp");
        Display external = ((DisplayManager) game.getSystemService(Context.DISPLAY_SERVICE)).getDisplay(displayId);
        StreamPresentation presentation = new StreamPresentation(game, external, false);
        ReflectionHelpers.setField(game, "externalPresentation", presentation);
        ReflectionHelpers.setField(game, "renderView", presentation.videoView());
        // A detach or output selection changes the next activity's intent before teardown ends.
        game.getIntent().removeExtra(OUTPUT_DISPLAY);
        assertEquals(displayId, ((Display) ReflectionHelpers.callInstanceMethod(game.videoOutput, "getVideoDisplay")).getDisplayId());

        ReflectionHelpers.setField(game, "externalPresentation", null);
        ReflectionHelpers.setField(game, "renderView", new StreamView(game));
        game.getIntent().putExtra(OUTPUT_DISPLAY, displayId);
        assertEquals(game.getWindowManager().getDefaultDisplay().getDisplayId(),
                ((Display) ReflectionHelpers.callInstanceMethod(game.videoOutput, "getVideoDisplay")).getDisplayId());
    }

    @Test public void externalTextureDestructionStopsDecoderBeforeFallbackAndOnlyOnce() {
        RecordingGame game = Robolectric.buildActivity(RecordingGame.class).get();
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.useTextureView = true;
        ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
        MediaCodecDecoderRenderer renderer = new MediaCodecDecoderRenderer(game, prefs, null, 0,
                false, false, "", null) {
            @Override public void prepareForStop() { game.events.add("decoder-stop"); }
        };
        StreamPresentation presentation = new StreamPresentation(game,
                game.getWindowManager().getDefaultDisplay(), true);
        SurfaceTexture texture = new SurfaceTexture(0);
        Surface surface = new Surface(texture);
        ReflectionHelpers.setField(game, "prefConfig", prefs);
        ReflectionHelpers.setField(game, "decoderRenderer", renderer);
        ReflectionHelpers.setField(game, "externalPresentation", presentation);
        ReflectionHelpers.setField(game, "surfaceCreated", true);
        ReflectionHelpers.setField(game, "attemptedConnection", true);
        ReflectionHelpers.setField(game, "videoSurface", surface);
        try {
            // Background detach must finish without launching a new stream.
            assertTrue(game.onSurfaceTextureDestroyed(texture));
            assertEquals(Arrays.asList("decoder-stop", "finish"), game.events);
            assertEquals(Boolean.FALSE, ReflectionHelpers.getField(game, "surfaceCreated"));
            assertNull(ReflectionHelpers.getField(game, "videoSurface"));
            assertNull(org.robolectric.Shadows.shadowOf(game).getNextStartedActivity());
            assertTrue(game.onSurfaceTextureDestroyed(texture));
            assertEquals(Arrays.asList("decoder-stop", "finish"), game.events);
        } finally {
            surface.release();
            texture.release();
        }
    }

    @Test public void displayDismissedBeforeConnectionSetupCannotStartAReplacementStream() {
        RecordingGame game = Robolectric.buildActivity(RecordingGame.class).get();
        game.setIntent(new Intent().putExtra(OUTPUT_DISPLAY, 5));
        ReflectionHelpers.setField(game, "foreground", true);
        ReflectionHelpers.callInstanceMethod(game.videoOutput, "returnVideoToDevice");
        assertFalse(game.getIntent().hasExtra(OUTPUT_DISPLAY));
        assertEquals(Arrays.asList("finish"), game.events);
        assertNull(org.robolectric.Shadows.shadowOf(game).getNextStartedActivity());
    }

    @Test public void outputReconnectCapturesGameIdentitySeparatelyFromAutomaticRetryBudget() {
        Game game = Robolectric.buildActivity(Game.class).get();
        Intent original = new Intent().putExtra(Game.EXTRA_APP_ID, 2147483501)
                .putExtra(Game.EXTRA_APP_UUID, "9a1c5a25-58fe-40e0-b9aa-7d3f00000001")
                .putExtra("RecoveryAttempts", 3).putExtra("RecoveryStarted", 1000L)
                .putExtra(OUTPUT_DISPLAY, 42);
        game.setIntent(original);
        ReflectionHelpers.setField(game, "app", new NvApp("Resume", 2147483501, false));
        ReflectionHelpers.setField(game, "recoveryAppId", 73);
        ReflectionHelpers.setField(game, "recoveryAppUuid", "actual-game-uuid");
        ReflectionHelpers.setField(game, "outputSwitchPending", true);

        Intent retry = ReflectionHelpers.callInstanceMethod(game.streamRecovery, "createReconnectIntent");
        assertNotNull(retry);
        assertEquals(73, retry.getIntExtra(Game.EXTRA_APP_ID, 0));
        assertEquals("actual-game-uuid", retry.getStringExtra(Game.EXTRA_APP_UUID));
        assertEquals(42, retry.getIntExtra(OUTPUT_DISPLAY, 0));
        assertFalse(retry.hasExtra("RecoveryAttempts"));
        assertFalse(retry.hasExtra("RecoveryStarted"));
        assertEquals(2147483501, original.getIntExtra(Game.EXTRA_APP_ID, 0));

        Game replacement = Robolectric.buildActivity(Game.class).get();
        replacement.setIntent(retry);
        assertTrue(ReflectionHelpers.callInstanceMethod(replacement.streamRecovery, "isResumeOnlyStream"));
        assertFalse(ReflectionHelpers.callInstanceMethod(replacement.streamRecovery, "isRecoveryAttempt"));
    }

    @Test public void outputReconnectCannotReplayAnUnresolvedResumeOrDestructiveControlTile() {
        Game game = Robolectric.buildActivity(Game.class).get();
        ReflectionHelpers.setField(game, "outputSwitchPending", true);
        ReflectionHelpers.setField(game, "app", new NvApp("Resume", 2147483501, false));
        assertNull(ReflectionHelpers.callInstanceMethod(game.streamRecovery, "createReconnectIntent"));
        ReflectionHelpers.setField(game, "app", new NvApp("Terminate", 2147483504, false));
        assertNull(ReflectionHelpers.callInstanceMethod(game.streamRecovery, "createReconnectIntent"));
    }

    @Test public void manualReconnectKeepsUserSelectionAndStartsANewRetryBudget() {
        Game game = Robolectric.buildActivity(Game.class).get();
        game.setIntent(new Intent().putExtra(Game.EXTRA_APP_ID, 73)
                .putExtra("RecoveryAttempts", 3).putExtra("RecoveryStarted", 1000L)
                .putExtra("rubylight.outputResumeOnly", true));
        ReflectionHelpers.setField(game, "app", new NvApp("Game", 73, false));
        Intent retry = ReflectionHelpers.callInstanceMethod(game.streamRecovery, "createReconnectIntent");
        assertEquals(73, retry.getIntExtra(Game.EXTRA_APP_ID, 0));
        assertFalse(retry.hasExtra("RecoveryAttempts"));
        assertFalse(retry.hasExtra("RecoveryStarted"));
        assertFalse(retry.hasExtra("rubylight.outputResumeOnly"));
        game.setIntent(retry);
        assertFalse(ReflectionHelpers.callInstanceMethod(game.streamRecovery, "isResumeOnlyStream"));
    }

    public static class RecordingGame extends Game {
        final List<String> events = new ArrayList<>();
        @Override public void finish() { events.add("finish"); super.finish(); }
    }
}
