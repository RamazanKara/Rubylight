package com.limelight.binding.video;

import android.app.Activity;
import android.app.Application;
import android.content.Context;

import com.limelight.Game;
import com.limelight.R;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.preferences.PreferenceConfiguration;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;

import static org.junit.Assert.*;

// Fire TV Cube 3 (issue 21): the decoder rejected every format and the dialog blamed the host.
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class DecoderSetupFailureTest {
    @Test
    public void rejectedDecoderNamesTheDecoderFormatAndReason() {
        Context context = Robolectric.buildActivity(Activity.class).get();
        DecoderSetupFailure failure = new DecoderSetupFailure(DecoderSetupFailure.Kind.REJECTED,
                "OMX.amlogic.hevc.decoder.awesome", "video/hevc", 2560, 1440, 60,
                new IllegalStateException("configure failed"));
        String message = failure.message(context);
        assertTrue(message, message.contains("OMX.amlogic.hevc.decoder.awesome"));
        assertTrue(message, message.contains("HEVC"));
        assertTrue(message, message.contains("2560x1440"));
        assertTrue(message, message.contains("60 FPS"));
        assertTrue(message, message.contains("IllegalStateException: configure failed"));
        assertFalse(message, message.contains(context.getString(R.string.startup_host_error, -5)));
        assertTrue(failure.toString(), failure.toString().contains("OMX.amlogic.hevc.decoder.awesome"));
    }

    @Test
    public void rejectedDecoderWithoutAReasonStillReads() {
        Context context = Robolectric.buildActivity(Activity.class).get();
        String message = new DecoderSetupFailure(DecoderSetupFailure.Kind.REJECTED, "c2.test.decoder",
                "video/av01", 1920, 1080, 120, null).message(context);
        assertTrue(message, message.contains("AV1"));
        assertFalse(message, message.contains("null"));
    }

    @Test
    public void longCodecMessagesAreCut() {
        StringBuilder text = new StringBuilder();
        for (int i = 0; i < 500; i++) text.append('x');
        assertTrue(DecoderSetupFailure.describe(new IllegalArgumentException(text.toString())).length() < 200);
        assertNull(DecoderSetupFailure.describe(null));
    }

    @Test
    public void setupWithoutADecoderIsRecordedAndClearedOnTheNextSetup() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
            PreferenceConfiguration prefs = new PreferenceConfiguration();
            prefs.videoFormat = PreferenceConfiguration.FormatOption.FORCE_HEVC;
            prefs.width = 2560;
            prefs.height = 1440;
            MediaCodecDecoderRenderer renderer = new MediaCodecDecoderRenderer(activity.get(), prefs,
                    null, 0, false, false, "", null);
            try {
                assertNull(renderer.getSetupFailure());
                // Robolectric has no hardware decoders, so setup fails the way a device without one does.
                assertNotEquals(0, renderer.setup(MoonBridge.VIDEO_FORMAT_H265, 2560, 1440, 60));
                DecoderSetupFailure failure = renderer.getSetupFailure();
                assertNotNull(failure);
                assertEquals(DecoderSetupFailure.Kind.NO_DECODER, failure.kind);
                assertEquals("HEVC", failure.codec);
                assertEquals(1440, failure.height);
                String message = failure.message(activity.get());
                assertTrue(message, message.contains("HEVC"));
                // A setup that fails without a record (an unknown format) clears the old one.
                assertNotEquals(0, renderer.setup(0, 2560, 1440, 60));
                assertNull(renderer.getSetupFailure());
            } finally {
                renderer.stop();
                renderer.cleanup();
            }
        }
    }

    @Test
    public void theDialogBlamesTheDecoderOnlyForARecordedFailureAtAVideoStage() {
        Context context = Robolectric.buildActivity(Activity.class).get();
        DecoderSetupFailure failure = new DecoderSetupFailure(DecoderSetupFailure.Kind.REJECTED,
                "c2.test.decoder", "video/hevc", 1920, 1080, 60, new IllegalStateException("configure failed"));
        String host = context.getString(R.string.startup_host_error, -5);

        String decoderText = Game.startupFailureText(context, failure, "video stream", null, -5);
        assertEquals(failure.message(context), decoderText);
        assertFalse(decoderText, decoderText.contains(host));
        assertTrue(Game.isDecoderStageFailure(failure, "video stream"));

        assertEquals("no recorded failure: the host text", host, Game.startupFailureText(context, null, "video stream", null, -5));
        assertFalse(Game.isDecoderStageFailure(null, "video stream"));
        assertEquals("a recorded failure at an audio stage is not the decoder's", host,
                Game.startupFailureText(context, failure, "audio stream", null, -5));
        assertFalse(Game.isDecoderStageFailure(failure, "audio stream"));
        assertEquals("a connection error still wins over the host text", "refused",
                Game.startupFailureText(context, null, "control stream", "refused", -5));
    }
}
