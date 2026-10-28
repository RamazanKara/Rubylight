package com.limelight.binding.video;

import android.app.Activity;
import android.app.Application;
import android.media.MediaCodec;
import android.media.MediaFormat;
import android.os.Bundle;

import com.limelight.preferences.PreferenceConfiguration;

import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.Implementation;
import org.robolectric.annotation.Implements;
import org.robolectric.shadows.ShadowMediaCodec;
import org.robolectric.util.ReflectionHelpers;

import java.io.IOException;
import java.nio.ByteBuffer;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.atomic.AtomicInteger;

import static org.junit.Assert.*;

/** The PC's HDR report on a running decoder, and the restarts it and other recoveries take. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class, shadows = MediaCodecHdrMetadataTest.RecordingCodec.class)
public class MediaCodecHdrMetadataTest {
    @Implements(MediaCodec.class)
    public static class RecordingCodec extends ShadowMediaCodec {
        static final List<String> calls = new ArrayList<>();
        static final List<Bundle> parameters = new ArrayList<>();
        static RuntimeException refuseParameters;
        static boolean createFails;

        @Implementation public String getName() {
            return "c2.mtk.dvhe.st.decoder";
        }

        @Implementation public void setParameters(Bundle params) {
            if (refuseParameters != null) throw refuseParameters;
            parameters.add(params);
        }

        @Implementation public void stop() {
            calls.add("stop");
        }

        @Implementation public void reset() {
            calls.add("reset");
        }

        @Implementation public void release() {
            calls.add("release");
        }

        @Implementation public static MediaCodec createByCodecName(String name) throws IOException {
            calls.add("create " + name);
            if (createFails) throw new IOException("no " + name);
            return MediaCodec.createDecoderByType("video/avc");
        }
    }

    private static final byte[] METADATA = new byte[24];
    static {
        // CTA-861.3 values as the host sends them: red.x 0.708, then 400 nits.
        METADATA[0] = 0x48;
        METADATA[1] = (byte) 0x8a;
        METADATA[16] = (byte) 0x90;
        METADATA[17] = 0x01;
    }

    private final List<Exception> crashes = new ArrayList<>();

    @Before
    public void clear() {
        RecordingCodec.calls.clear();
        RecordingCodec.parameters.clear();
        RecordingCodec.refuseParameters = null;
        RecordingCodec.createFails = false;
        ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
    }

    /** A renderer streaming HDR on a running codec, taken to be HDR before the PC's report. */
    private MediaCodecDecoderRenderer streaming(Activity activity, boolean asParameter) throws IOException {
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.videoFormat = PreferenceConfiguration.FormatOption.AUTO;
        MediaCodecDecoderRenderer renderer = new MediaCodecDecoderRenderer(activity, prefs,
                crashes::add, 0, false, true, "", null);
        ReflectionHelpers.setField(renderer, "videoDecoder", MediaCodec.createDecoderByType("video/avc"));
        ReflectionHelpers.setField(renderer, "currentHdrMode", Boolean.TRUE);
        ReflectionHelpers.setField(renderer, "hdrMetadataAsParameter", asParameter);
        return renderer;
    }

    private static int recovery(String name) {
        return ReflectionHelpers.getStaticField(MediaCodecDecoderRenderer.class, name);
    }

    private static MediaFormat outputWith(byte[] hdrStaticInfo) {
        MediaFormat format = MediaFormat.createVideoFormat("video/raw", 3840, 2160);
        format.setByteBuffer(MediaFormat.KEY_HDR_STATIC_INFO, ByteBuffer.wrap(hdrStaticInfo));
        return format;
    }

    private static void checkOutput(MediaCodecDecoderRenderer renderer) {
        ReflectionHelpers.callInstanceMethod(renderer, "checkRunningHdrStaticInfo");
    }

    @Test
    public void aMediaTekDecoderTakesTheMetadataAsAParameterUntilItsOutputShowsIt() throws IOException {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            MediaCodecDecoderRenderer renderer = streaming(activity.get(), true);
            AtomicInteger type = ReflectionHelpers.getField(renderer, "codecRecoveryType");
            renderer.setHdrMode(true, METADATA);

            byte[] expected = MediaCodecDecoderRenderer.createHdrStaticInfo(METADATA).array();
            assertEquals(1, RecordingCodec.parameters.size());
            assertArrayEquals(expected, RecordingCodec.parameters.get(0).getByteArray(MediaFormat.KEY_HDR_STATIC_INFO));
            assertEquals(recovery("CR_RECOVERY_TYPE_NONE"), type.get());
            assertArrayEquals(expected, (byte[]) ReflectionHelpers.getField(renderer, "uncheckedHdrStaticInfo"));

            // The format from before the parameter, then the one that carries it.
            ReflectionHelpers.setField(renderer, "outputFormat", outputWith(new byte[25]));
            checkOutput(renderer);
            assertNotNull(ReflectionHelpers.getField(renderer, "uncheckedHdrStaticInfo"));
            ReflectionHelpers.setField(renderer, "outputFormat", outputWith(expected));
            checkOutput(renderer);
            assertNull(ReflectionHelpers.getField(renderer, "uncheckedHdrStaticInfo"));
            assertEquals(recovery("CR_RECOVERY_TYPE_NONE"), type.get());
            assertTrue(RecordingCodec.calls.isEmpty());
        }
    }

    @Test
    public void anOutputThatNeverShowsTheMetadataRestartsTheDecoder() throws IOException {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            MediaCodecDecoderRenderer renderer = streaming(activity.get(), true);
            AtomicInteger type = ReflectionHelpers.getField(renderer, "codecRecoveryType");
            renderer.setHdrMode(true, METADATA);
            ReflectionHelpers.setField(renderer, "outputFormat", outputWith(new byte[25]));
            for (int i = 1; i < MediaCodecDecoderRenderer.HDR_STATIC_INFO_OUTPUTS; i++) {
                checkOutput(renderer);
            }
            assertEquals(recovery("CR_RECOVERY_TYPE_NONE"), type.get());
            checkOutput(renderer);
            assertEquals(recovery("CR_RECOVERY_TYPE_RESTART"), type.get());
            assertEquals("HDR metadata missing from the output", ReflectionHelpers.getField(renderer, "restartReason"));
            assertNull(ReflectionHelpers.getField(renderer, "uncheckedHdrStaticInfo"));
        }
    }

    @Test
    public void otherDecodersRestartForTheMetadataAsBefore() throws IOException {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            MediaCodecDecoderRenderer renderer = streaming(activity.get(), false);
            AtomicInteger type = ReflectionHelpers.getField(renderer, "codecRecoveryType");
            renderer.setHdrMode(true, METADATA);
            assertTrue(RecordingCodec.parameters.isEmpty());
            assertEquals(recovery("CR_RECOVERY_TYPE_RESTART"), type.get());
            assertEquals("HDR metadata", ReflectionHelpers.getField(renderer, "restartReason"));
            assertNull(ReflectionHelpers.getField(renderer, "uncheckedHdrStaticInfo"));
        }
    }

    @Test
    public void aRefusedParameterRestartsTheDecoder() throws IOException {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            MediaCodecDecoderRenderer renderer = streaming(activity.get(), true);
            AtomicInteger type = ReflectionHelpers.getField(renderer, "codecRecoveryType");
            RecordingCodec.refuseParameters = new IllegalStateException("refused");
            renderer.setHdrMode(true, METADATA);
            assertEquals(recovery("CR_RECOVERY_TYPE_RESTART"), type.get());
            assertNull(ReflectionHelpers.getField(renderer, "uncheckedHdrStaticInfo"));
        }
    }

    @Test
    public void aPendingRecoveryConfiguresTheMetadataWithoutAParameter() throws IOException {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            MediaCodecDecoderRenderer renderer = streaming(activity.get(), true);
            AtomicInteger type = ReflectionHelpers.getField(renderer, "codecRecoveryType");
            type.set(recovery("CR_RECOVERY_TYPE_RESET"));
            renderer.setHdrMode(true, METADATA);
            assertTrue(RecordingCodec.parameters.isEmpty());
            assertEquals(recovery("CR_RECOVERY_TYPE_RESET"), type.get());
            assertArrayEquals(METADATA, (byte[]) ReflectionHelpers.getField(renderer, "currentHdrMetadata"));
            assertNull(ReflectionHelpers.getField(renderer, "uncheckedHdrStaticInfo"));
        }
    }

    @Test
    public void noNewCodecAfterTheReleaseSetsUpAgainWithoutResetOrBlamingDolbyVision() throws IOException {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            MediaCodecDecoderRenderer renderer = streaming(activity.get(), true);
            AtomicInteger type = ReflectionHelpers.getField(renderer, "codecRecoveryType");
            ReflectionHelpers.setField(renderer, "restartRecreates", true);
            ReflectionHelpers.setField(renderer, "dolbyVisionActive", true);
            ReflectionHelpers.setField(renderer, "uncheckedHdrStaticInfo", new byte[25]);
            RecordingCodec.createFails = true;
            type.set(recovery("CR_RECOVERY_TYPE_RESTART"));
            ReflectionHelpers.setField(renderer, "restartReason", "HDR on");

            // The input thread is quiesced; the renderer thread is the last one in.
            ReflectionHelpers.setField(renderer, "codecRecoveryThreadQuiescedFlags",
                    (int) ReflectionHelpers.getStaticField(MediaCodecDecoderRenderer.class, "CR_FLAG_INPUT_THREAD"));
            try {
                ReflectionHelpers.callInstanceMethod(renderer, "doCodecRecoveryIfRequired",
                        ReflectionHelpers.ClassParameter.from(int.class,
                                (int) ReflectionHelpers.getStaticField(MediaCodecDecoderRenderer.class, "CR_FLAG_RENDER_THREAD")));
                fail("this renderer has no decoder to set up");
            } catch (RuntimeException expected) {
                // initializeDecoder found no format to set up, as a stream without decoders would.
            }
            assertEquals(java.util.Arrays.asList("release", "create c2.mtk.dvhe.st.decoder"), RecordingCodec.calls);
            assertEquals(false, ReflectionHelpers.getField(renderer, "dolbyVisionFailed"));
            assertNull(ReflectionHelpers.getField(renderer, "uncheckedHdrStaticInfo"));
            assertEquals(1, crashes.size());
        }
    }

    @Test
    public void aDecoderThatFailsItsResetIsRebuiltWithoutDolbyVision() throws IOException {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            MediaCodecDecoderRenderer renderer = streaming(activity.get(), true);
            ReflectionHelpers.setField(renderer, "dolbyVisionActive", true);
            CodecRecoverySteps.Codec codec = ReflectionHelpers.getField(renderer, "recoveryCodec");
            try {
                codec.rebuild(true);
                fail("this renderer has no decoder to set up");
            } catch (IllegalStateException expected) {
            }
            assertEquals(java.util.Collections.singletonList("release"), RecordingCodec.calls);
            assertEquals(true, ReflectionHelpers.getField(renderer, "dolbyVisionFailed"));
        }
    }

    @Test
    public void aRecreatedCodecReplacesTheReleasedOne() throws IOException {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            MediaCodecDecoderRenderer renderer = streaming(activity.get(), true);
            MediaCodec old = ReflectionHelpers.getField(renderer, "videoDecoder");
            CodecRecoverySteps.Codec codec = ReflectionHelpers.getField(renderer, "recoveryCodec");
            assertTrue(codec.recreate());
            assertNotSame(old, ReflectionHelpers.getField(renderer, "videoDecoder"));
            assertEquals(java.util.Arrays.asList("release", "create c2.mtk.dvhe.st.decoder"), RecordingCodec.calls);

            RecordingCodec.createFails = true;
            assertFalse(codec.recreate());
        }
    }
}
