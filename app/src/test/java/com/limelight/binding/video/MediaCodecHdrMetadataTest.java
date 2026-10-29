package com.limelight.binding.video;

import android.app.Activity;
import android.app.Application;
import android.media.MediaCodec;
import android.media.MediaFormat;
import android.os.Bundle;

import com.limelight.PcViewLifecycleTest;
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
@Config(sdk = 29, application = Application.class, instrumentedPackages = "com.limelight.nvstream.jni",
        shadows = {MediaCodecHdrMetadataTest.RecordingCodec.class, PcViewLifecycleTest.NoNativeMoonBridge.class})
public class MediaCodecHdrMetadataTest {
    @Implements(MediaCodec.class)
    public static class RecordingCodec extends ShadowMediaCodec {
        static final List<String> calls = new ArrayList<>();
        static final List<Bundle> parameters = new ArrayList<>();
        static RuntimeException refuseParameters;
        static boolean createFails;
        static RuntimeException createRefused;

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
            if (createRefused != null) throw createRefused;
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
        RecordingCodec.createRefused = null;
        ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
    }

    /** A renderer streaming HDR on a running codec, taken to be HDR before the PC's report. */
    private MediaCodecDecoderRenderer streaming(Activity activity) throws IOException {
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.videoFormat = PreferenceConfiguration.FormatOption.AUTO;
        MediaCodecDecoderRenderer renderer = new MediaCodecDecoderRenderer(activity, prefs,
                crashes::add, 0, false, true, "", null);
        ReflectionHelpers.setField(renderer, "videoDecoder", MediaCodec.createDecoderByType("video/avc"));
        ReflectionHelpers.setField(renderer, "currentHdrMode", Boolean.TRUE);
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
    public void anyDecoderTakesTheMetadataAsAParameterUntilItsOutputShowsIt() throws IOException {
        // Not only MediaTek's Codec2 decoders: streaming() runs the platform's video/avc decoder.
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            MediaCodecDecoderRenderer renderer = streaming(activity.get());
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
            MediaCodecDecoderRenderer renderer = streaming(activity.get());
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
    public void metadataShowingInTheSixtiethOutputNeedsNoRestart() throws IOException {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            MediaCodecDecoderRenderer renderer = streaming(activity.get());
            AtomicInteger type = ReflectionHelpers.getField(renderer, "codecRecoveryType");
            renderer.setHdrMode(true, METADATA);
            ReflectionHelpers.setField(renderer, "outputFormat", outputWith(new byte[25]));
            for (int i = 1; i < MediaCodecDecoderRenderer.HDR_STATIC_INFO_OUTPUTS; i++) {
                checkOutput(renderer);
            }
            ReflectionHelpers.setField(renderer, "outputFormat",
                    outputWith(MediaCodecDecoderRenderer.createHdrStaticInfo(METADATA).array()));
            checkOutput(renderer);
            assertNull(ReflectionHelpers.getField(renderer, "uncheckedHdrStaticInfo"));
            for (int i = 0; i < 2 * MediaCodecDecoderRenderer.HDR_STATIC_INFO_OUTPUTS; i++) {
                checkOutput(renderer);
            }
            assertEquals(recovery("CR_RECOVERY_TYPE_NONE"), type.get());
        }
    }

    @Test
    public void newMetadataCountsSixtyOutputsAgainAndRestartsOnce() throws IOException {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            MediaCodecDecoderRenderer renderer = streaming(activity.get());
            AtomicInteger type = ReflectionHelpers.getField(renderer, "codecRecoveryType");
            renderer.setHdrMode(true, METADATA);
            ReflectionHelpers.setField(renderer, "outputFormat", outputWith(new byte[25]));
            for (int i = 1; i < MediaCodecDecoderRenderer.HDR_STATIC_INFO_OUTPUTS; i++) {
                checkOutput(renderer);
            }
            // The PC's metadata changes one output before the restart: the new metadata gets its own 60.
            byte[] brighter = METADATA.clone();
            brighter[16] = (byte) 0xe8;
            brighter[17] = 0x03;
            renderer.setHdrMode(true, brighter);
            assertEquals(2, RecordingCodec.parameters.size());
            assertEquals(0, (int) ReflectionHelpers.getField(renderer, "hdrStaticInfoOutputs"));
            for (int i = 1; i < MediaCodecDecoderRenderer.HDR_STATIC_INFO_OUTPUTS; i++) {
                checkOutput(renderer);
            }
            assertEquals(recovery("CR_RECOVERY_TYPE_NONE"), type.get());
            checkOutput(renderer);
            assertEquals(recovery("CR_RECOVERY_TYPE_RESTART"), type.get());

            // The restart is the only one: later outputs without the metadata ask for none.
            type.set(recovery("CR_RECOVERY_TYPE_NONE"));
            for (int i = 0; i < 2 * MediaCodecDecoderRenderer.HDR_STATIC_INFO_OUTPUTS; i++) {
                checkOutput(renderer);
            }
            assertEquals(recovery("CR_RECOVERY_TYPE_NONE"), type.get());
        }
    }

    @Test
    public void aRefusedParameterRestartsTheDecoder() throws IOException {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            MediaCodecDecoderRenderer renderer = streaming(activity.get());
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
            MediaCodecDecoderRenderer renderer = streaming(activity.get());
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
            MediaCodecDecoderRenderer renderer = streaming(activity.get());
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
            // initializeDecoder finds no format to set up, as a stream without decoders would: the
            // stream ends with the reason instead of a throw from the renderer thread.
            ReflectionHelpers.callInstanceMethod(renderer, "doCodecRecoveryIfRequired",
                    ReflectionHelpers.ClassParameter.from(int.class,
                            (int) ReflectionHelpers.getStaticField(MediaCodecDecoderRenderer.class, "CR_FLAG_RENDER_THREAD")));
            String failure = renderer.getDecoderFailure();
            assertNotNull(failure);
            assertTrue(failure, failure.contains("could not be set up again after it failed"));
            assertEquals(java.util.Arrays.asList("release", "create c2.mtk.dvhe.st.decoder"), RecordingCodec.calls);
            assertEquals(false, ReflectionHelpers.getField(renderer, "dolbyVisionFailed"));
            assertNull(ReflectionHelpers.getField(renderer, "uncheckedHdrStaticInfo"));
            // Reported once: the next stream runs without reference frame invalidation.
            assertEquals(1, crashes.size());
        }
    }

    @Test
    public void aDecoderThatFailsItsResetIsRebuiltWithoutDolbyVision() throws IOException {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            MediaCodecDecoderRenderer renderer = streaming(activity.get());
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
            MediaCodecDecoderRenderer renderer = streaming(activity.get());
            MediaCodec old = ReflectionHelpers.getField(renderer, "videoDecoder");
            CodecRecoverySteps.Codec codec = ReflectionHelpers.getField(renderer, "recoveryCodec");
            assertTrue(codec.recreate());
            assertNotSame(old, ReflectionHelpers.getField(renderer, "videoDecoder"));
            assertEquals(java.util.Arrays.asList("release", "create c2.mtk.dvhe.st.decoder"), RecordingCodec.calls);

            RecordingCodec.createFails = true;
            assertFalse(codec.recreate());
            // No decoder free yet (a CodecException): the released codec is not reset, it is set up again.
            RecordingCodec.createFails = false;
            RecordingCodec.createRefused = new IllegalStateException("NO_MEMORY");
            assertFalse(codec.recreate());
        }
    }

    @Test
    public void aDecoderThatCannotBeCreatedWhileRecoveringIsACodecError() throws IOException {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            MediaCodecDecoderRenderer renderer = streaming(activity.get());
            RecordingCodec.createFails = true;
            // Codec recovery reports an IllegalStateException as a crash; anything else would kill its thread.
            try {
                ReflectionHelpers.callInstanceMethod(renderer, "tryConfigureDecoder",
                        ReflectionHelpers.ClassParameter.from(android.media.MediaCodecInfo.class,
                                org.robolectric.shadows.MediaCodecInfoBuilder.newBuilder()
                                        .setName("c2.mtk.dvhe.st.decoder").build()),
                        ReflectionHelpers.ClassParameter.from(MediaFormat.class,
                                MediaFormat.createVideoFormat("video/hevc", 3840, 2160)),
                        ReflectionHelpers.ClassParameter.from(boolean.class, true));
                fail("the decoder could not be created");
            } catch (IllegalStateException expected) {
                assertTrue(expected.getCause() instanceof IOException);
            }
        }
    }
}
