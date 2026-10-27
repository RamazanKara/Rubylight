package com.limelight;

import android.app.Application;

import com.limelight.binding.video.MediaCodecDecoderRenderer;
import com.limelight.binding.video.MediaCodecHelper;
import com.limelight.preferences.PreferenceConfiguration;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowLooper;
import org.robolectric.util.ReflectionHelpers;

import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.TimeUnit;

import static org.junit.Assert.*;

/** Experimental staged exit on Android TV with HDR: how Game.finish() is gated. */
@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class GameStagedExitTest {
    public static class RecordingGame extends Game {
        final List<String> events = new ArrayList<>();
        @Override public void finish() { events.add("finish"); super.finish(); }
    }

    private static RecordingGame stream(boolean tv, boolean hdr) {
        RecordingGame game = Robolectric.buildActivity(RecordingGame.class).get();
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
        MediaCodecDecoderRenderer renderer = new MediaCodecDecoderRenderer(game, prefs, null, 0,
                false, false, "", null);
        ReflectionHelpers.setField(game, "prefConfig", prefs);
        ReflectionHelpers.setField(game, "decoderRenderer", renderer);
        ReflectionHelpers.setField(game, "attemptedConnection", true);
        ReflectionHelpers.setField(game, "streamHdr", hdr);
        ReflectionHelpers.setField(game, "androidTv", tv);
        return game;
    }

    private static boolean exiting(Game game) {
        return (boolean) ReflectionHelpers.getField(game, "exiting");
    }

    private static boolean exitFinished(Game game) {
        return (boolean) ReflectionHelpers.getField(game, "exitFinished");
    }

    @Test public void phoneAndSdrStreamsFinishAtOnce() {
        RecordingGame phone = stream(false, true);
        phone.finish();
        assertTrue(exitFinished(phone));
        assertFalse(exiting(phone));

        RecordingGame sdrTv = stream(true, false);
        sdrTv.finish();
        assertTrue(exitFinished(sdrTv));
        assertFalse(exiting(sdrTv));
    }

    @Test public void tvHdrExitIsStagedAndFinishesAfterTheDisplaySettles() {
        RecordingGame game = stream(true, true);
        game.finish();
        assertTrue(exiting(game));
        assertFalse(exitFinished(game));
        assertTrue(game.isFinishing());

        ShadowLooper.idleMainLooper(300, TimeUnit.MILLISECONDS);
        assertFalse("still waiting for the display", exitFinished(game));

        ShadowLooper.idleMainLooper(10, TimeUnit.SECONDS);
        assertTrue(exitFinished(game));
    }

    @Test public void secondFinishDuringStagingIsIgnored() {
        RecordingGame game = stream(true, true);
        game.finish();
        game.finish();
        game.finish();
        assertTrue(exiting(game));
        assertFalse(exitFinished(game));

        ShadowLooper.idleMainLooper(10, TimeUnit.SECONDS);
        assertTrue(exitFinished(game));
        // The activity ends once; later calls fall through to the platform.
        game.finish();
        assertTrue(exitFinished(game));
    }

    @Test public void handOffToANewStreamFinishesAtOnceEvenDuringStaging() {
        RecordingGame game = stream(true, true);
        game.finish();
        assertFalse(exitFinished(game));

        ReflectionHelpers.callInstanceMethod(game, "finishNow");
        assertTrue(exitFinished(game));

        // The staged ticks that are still queued do nothing more.
        ShadowLooper.idleMainLooper(10, TimeUnit.SECONDS);
        assertTrue(exitFinished(game));
    }
}
