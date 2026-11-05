package com.limelight.binding.video;

import android.app.Activity;
import android.app.Application;
import android.graphics.SurfaceTexture;
import android.media.MediaCodec;
import android.media.MediaCodecInfo;
import android.media.MediaCrypto;
import android.media.MediaFormat;
import android.view.Surface;

import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.preferences.PreferenceConfiguration;

import org.junit.After;
import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.Implementation;
import org.robolectric.annotation.Implements;
import org.robolectric.shadows.MediaCodecInfoBuilder;
import org.robolectric.shadows.ShadowBuild;
import org.robolectric.shadows.ShadowMediaCodec;
import org.robolectric.shadows.ShadowSystemClock;
import org.robolectric.util.ReflectionHelpers;
import org.robolectric.util.ReflectionHelpers.ClassParameter;

import java.nio.ByteBuffer;
import java.time.Duration;
import java.util.ArrayList;
import java.util.List;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertNotSame;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertSame;
import static org.junit.Assert.assertTrue;

/**
 * The stream's side of {@link KeyframeSafetyNet} and {@link InputSpacer}: only the one Fire TV's MediaTek
 * decoder in Android's low-latency mode asks for keyframes or holds frames, the frame that asks still
 * decodes, any keyframe restarts the wait, and the gap before an answered request is no network loss.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 34, application = Application.class,
        shadows = {KeyframeSafetyNetWiringTest.QueueingCodec.class, com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class},
        instrumentedPackages = "com.limelight.nvstream.jni")
public class KeyframeSafetyNetWiringTest {
    private static final String MODEL = "AFTNE4E43F";
    private static final String MTK = "c2.mtk.hevc.decoder";

    /** Takes every buffer it is given, as a decoder with room does. */
    @Implements(MediaCodec.class)
    public static class QueueingCodec extends ShadowMediaCodec {
        static int queued;
        static final List<Long> queuedAtNs = new ArrayList<>();
        static long lastTimeUs;
        static int inputBufferBytes = 4096;
        static MediaCodecInfo info;

        @Implementation public void configure(MediaFormat format, Surface surface, MediaCrypto crypto, int flags) {}
        @Implementation public void start() {}
        @Implementation public int dequeueInputBuffer(long timeoutUs) {
            return 0;
        }
        @Implementation public ByteBuffer getInputBuffer(int index) {
            return ByteBuffer.allocate(inputBufferBytes);
        }
        @Implementation public MediaCodecInfo getCodecInfo() {
            return info;
        }
        @Implementation public void queueInputBuffer(int index, int offset, int size, long timeUs, int flags) {
            queued++;
            queuedAtNs.add(System.nanoTime());
            lastTimeUs = timeUs;
        }
    }

    private ActivityController<Activity> activity;
    private SurfaceTexture texture;
    private Surface surface;
    private MediaCodecDecoderRenderer renderer;
    private int frame;

    @Before
    public void setUp() {
        ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
        QueueingCodec.queued = 0;
        QueueingCodec.queuedAtNs.clear();
        QueueingCodec.inputBufferBytes = 4096;
        QueueingCodec.info = null;
        activity = Robolectric.buildActivity(Activity.class).create();
        texture = new SurfaceTexture(0);
        surface = new Surface(texture);
        frame = 0;
    }

    @After
    public void tearDown() {
        surface.release();
        texture.release();
        activity.close();
    }

    private void configure(String model, String decoder, boolean lowLatency, boolean hevcOverrideOff) {
        configure(model, decoder, lowLatency, hevcOverrideOff, 0);
    }

    // The format as the renderer's own ladder builds it for this try.
    private void configure(String model, String decoder, boolean lowLatency, boolean hevcOverrideOff, int tryNumber) {
        ShadowBuild.setModel(model);
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.videoFormat = PreferenceConfiguration.FormatOption.FORCE_HEVC;
        prefs.codecLowLatency = lowLatency;
        prefs.disableHevcLowLatency = hevcOverrideOff;
        prefs.bitrate = 63_000;
        renderer = new MediaCodecDecoderRenderer(activity.get(), prefs, null, 0, false, false, "", null);
        ReflectionHelpers.setField(renderer, "streamFrameRateMilliHz", 60_000);
        ReflectionHelpers.setField(renderer, "renderTarget", surface);
        ReflectionHelpers.setField(renderer, "videoFormat", MoonBridge.VIDEO_FORMAT_H265);
        MediaFormat format = MediaFormat.createVideoFormat("video/hevc", 3840, 2160);
        MediaCodecInfo info = MediaCodecInfoBuilder.newBuilder().setName(decoder).build();
        MediaCodecHelper.setDecoderLowLatencyOptions(format, info, tryNumber, lowLatency, false, false, hevcOverrideOff);
        ReflectionHelpers.callInstanceMethod(renderer, "tryConfigureDecoder",
                ClassParameter.from(MediaCodecInfo.class, info),
                ClassParameter.from(MediaFormat.class, format),
                ClassParameter.from(boolean.class, true));
    }

    private int submit(int frameType) {
        frame++;
        return renderer.submitDecodeUnit(new byte[64], 64, MoonBridge.BUFFER_TYPE_PICDATA, frame, frameType, (char) 0,
                frame * 16_000L, frame * 16_000L, frame * 16_000_000L, frame * 16_000L);
    }

    private static void advance(long ms) {
        ShadowSystemClock.advanceBy(Duration.ofMillis(ms));
    }

    /** A frame numbered as the core numbers it: after a request, the ones until the keyframe never get here. */
    private int submitAt(int number, int frameType) {
        frame = number - 1;
        return submit(frameType);
    }

    private VideoStats activeStats() {
        return ReflectionHelpers.getField(renderer, "activeWindowVideoStats");
    }

    private KeyframeSafetyNet.Counts counts() {
        return ReflectionHelpers.getField(renderer, "repairCounts");
    }

    // The hold goes by real time: a test of hundreds of frames about something else doesn't wait for it.
    private void withoutHolds() {
        ReflectionHelpers.setField(renderer, "inputSpacer", null);
    }

    // Frames are the small ones of a calm picture here: the net's quiet refresh is what asks.
    @Test
    public void aCalmPictureAsksAfterTenSecondsAndTheAskingFrameStillDecodes() {
        configure(MODEL, MTK, true, false);
        assertNotNull(ReflectionHelpers.getField(renderer, "keyframeSafetyNet"));
        assertEquals(MoonBridge.DR_OK, submit(MoonBridge.FRAME_TYPE_IDR));
        assertEquals(MoonBridge.DR_OK, submit(MoonBridge.FRAME_TYPE_PFRAME));
        advance(9_999);
        assertEquals(MoonBridge.DR_OK, submit(MoonBridge.FRAME_TYPE_PFRAME));
        advance(1);
        int queued = QueueingCodec.queued;
        assertEquals(MoonBridge.DR_NEED_IDR, submit(MoonBridge.FRAME_TYPE_PFRAME));
        assertEquals("the frame that asks is queued", queued + 1, QueueingCodec.queued);
        // Frames in flight until the keyframe do not ask again.
        assertEquals(MoonBridge.DR_OK, submit(MoonBridge.FRAME_TYPE_PFRAME));
        assertEquals(1, counts().quietRepairs);
    }

    @Test
    public void anyKeyframeRestartsTheWaitAndNeverAsksItself() {
        configure(MODEL, MTK, true, false);
        assertEquals(MoonBridge.DR_OK, submit(MoonBridge.FRAME_TYPE_IDR));
        advance(9_000);
        assertEquals("an IDR after loss counts", MoonBridge.DR_OK, submit(MoonBridge.FRAME_TYPE_IDR));
        advance(9_999);
        assertEquals(MoonBridge.DR_OK, submit(MoonBridge.FRAME_TYPE_PFRAME));
        advance(30_000);
        assertEquals("a keyframe past the time answers it", MoonBridge.DR_OK, submit(MoonBridge.FRAME_TYPE_IDR));
        advance(100);
        assertEquals(MoonBridge.DR_OK, submit(MoonBridge.FRAME_TYPE_PFRAME));
    }

    @Test
    public void theGapBeforeAnAnsweredRepairIsNoLoss() {
        configure(MODEL, MTK, true, false);
        submitAt(1, MoonBridge.FRAME_TYPE_IDR);
        advance(10_000);
        assertEquals(MoonBridge.DR_NEED_IDR, submitAt(2, MoonBridge.FRAME_TYPE_PFRAME));
        // The core drops frames 3 to 6 while it waits for the keyframe.
        submitAt(7, MoonBridge.FRAME_TYPE_IDR);
        assertEquals("not the network's", 0, activeStats().framesLost);
        assertEquals(0, activeStats().frameLossEvents);
        assertEquals(4, counts().gapFrames);

        // The next gap is the network's again.
        submitAt(8, MoonBridge.FRAME_TYPE_PFRAME);
        submitAt(12, MoonBridge.FRAME_TYPE_PFRAME);
        assertEquals(3, activeStats().framesLost);
        assertEquals(1, activeStats().frameLossEvents);
        assertEquals(4, counts().gapFrames);
    }

    @Test
    public void aGapBeforeAKeyframeNobodyHereAskedForIsLoss() {
        configure(MODEL, MTK, true, false);
        submitAt(1, MoonBridge.FRAME_TYPE_IDR);
        submitAt(5, MoonBridge.FRAME_TYPE_IDR);
        assertEquals(3, activeStats().framesLost);
        assertEquals(0, counts().gapFrames);
    }

    @Test
    public void aRepairAnswersOneGapOnly() {
        configure(MODEL, MTK, true, false);
        submitAt(1, MoonBridge.FRAME_TYPE_IDR);
        advance(10_000);
        assertEquals(MoonBridge.DR_NEED_IDR, submitAt(2, MoonBridge.FRAME_TYPE_PFRAME));
        submitAt(4, MoonBridge.FRAME_TYPE_IDR);
        submitAt(7, MoonBridge.FRAME_TYPE_IDR);
        assertEquals("only the second one", 2, activeStats().framesLost);
    }

    @Test
    public void onAnotherDecoderEveryGapStaysLoss() {
        configure("AFTMM", MTK, true, false);
        submitAt(1, MoonBridge.FRAME_TYPE_IDR);
        submitAt(5, MoonBridge.FRAME_TYPE_IDR);
        assertEquals(3, activeStats().framesLost);
    }

    @Test
    public void aRetryWithoutTheFlagGetsNoRepairsWhateverTheSwitchSays() {
        configure(MODEL, MTK, true, false, 1);
        assertNull(ReflectionHelpers.getField(renderer, "keyframeSafetyNet"));
        assertNull(ReflectionHelpers.getField(renderer, "inputSpacer"));
    }

    private InputSpacer spacer() {
        return ReflectionHelpers.getField(renderer, "inputSpacer");
    }

    // A last input a second from now: a hold that runs at all waits that second and is counted.
    private void lastInputFarAhead() {
        spacer().onInput(System.nanoTime() + 1_000_000_000L);
    }

    @Test
    public void framesRightBehindEachOtherAreHeldApart() {
        configure(MODEL, MTK, true, false);
        ReflectionHelpers.setField(renderer, "streamFrameRateMilliHz", 30_000);
        submit(MoonBridge.FRAME_TYPE_IDR);
        // The times the hold itself keeps, not when the queue call got to run: a loaded machine can't make them
        // too close. A millisecond's slack: a sleep may wake that early.
        long half = FramePacingPolicy.frameIntervalNs(30_000) / 2 - 1_000_000;
        long last = ReflectionHelpers.<Long>getField(spacer(), "lastInputNs");
        for (int i = 0; i < 9; i++) {
            submit(MoonBridge.FRAME_TYPE_PFRAME);
            long input = ReflectionHelpers.<Long>getField(spacer(), "lastInputNs");
            assertTrue("spacing " + (input - last), input - last >= half);
            last = input;
        }
        assertTrue(counts().describe(), counts().heldFrames > 0 && counts().heldNs > 0);
    }

    @Test
    public void aFrameOnTimeIsNotHeld() throws InterruptedException {
        configure(MODEL, MTK, true, false);
        ReflectionHelpers.setField(renderer, "streamFrameRateMilliHz", 60_000);
        submit(MoonBridge.FRAME_TYPE_IDR);
        // Real time: the hold goes by System.nanoTime, not the test's clock.
        Thread.sleep(10);
        submit(MoonBridge.FRAME_TYPE_PFRAME);
        assertEquals(0, counts().heldFrames);
    }

    @Test
    public void aLowFrameRateHoldsNoLongerThanTheCap() {
        configure(MODEL, MTK, true, false);
        ReflectionHelpers.setField(renderer, "streamFrameRateMilliHz", 1_000);
        submit(MoonBridge.FRAME_TYPE_IDR);
        for (int i = 0; i < 4; i++) submit(MoonBridge.FRAME_TYPE_PFRAME);
        long start = System.nanoTime();
        submit(MoonBridge.FRAME_TYPE_PFRAME);
        long took = System.nanoTime() - start;
        assertTrue(counts().describe(), counts().heldFrames > 0);
        assertTrue("half this frame rate's interval would be 500 ms, took " + took, took < 400_000_000L);
    }

    @Test
    public void anInterruptGoesOnAndStaysSetForWhoeverStopsTheThread() {
        configure(MODEL, MTK, true, false);
        submit(MoonBridge.FRAME_TYPE_IDR);
        int held = counts().heldFrames;
        lastInputFarAhead();
        long start = System.nanoTime();
        Thread.currentThread().interrupt();
        try {
            assertEquals(MoonBridge.DR_OK, submit(MoonBridge.FRAME_TYPE_PFRAME));
            assertTrue(Thread.currentThread().isInterrupted());
        } finally {
            Thread.interrupted();
        }
        assertTrue("the wait ended there: " + (System.nanoTime() - start), System.nanoTime() - start < 500_000_000L);
        assertEquals("it did get to the wait", held + 1, counts().heldFrames);
    }

    private void assertHoldsNothing() {
        int held = counts().heldFrames;
        lastInputFarAhead();
        long start = System.nanoTime();
        ReflectionHelpers.callInstanceMethod(renderer, "holdForInputSpacing");
        assertEquals(held, counts().heldFrames);
        assertTrue("took " + (System.nanoTime() - start), System.nanoTime() - start < 500_000_000L);
    }

    @Test
    public void aStopHoldsNothing() {
        configure(MODEL, MTK, true, false);
        submit(MoonBridge.FRAME_TYPE_IDR);
        // Past submitDecodeUnit's own check for a stop, as another thread's stop can be.
        renderer.prepareForStop();
        assertHoldsNothing();
    }

    @Test
    public void aCodecRecoveryHoldsNothing() {
        configure(MODEL, MTK, true, false);
        submit(MoonBridge.FRAME_TYPE_IDR);
        ReflectionHelpers.<java.util.concurrent.atomic.AtomicInteger>getField(renderer, "codecRecoveryType")
                .set(ReflectionHelpers.<Integer>getStaticField(MediaCodecDecoderRenderer.class, "CR_RECOVERY_TYPE_FLUSH"));
        assertHoldsNothing();
    }

    @Test
    public void anotherDecoderHoldsNothing() {
        configure("AFTMM", MTK, true, false);
        ReflectionHelpers.setField(renderer, "streamFrameRateMilliHz", 60_000);
        assertNull(ReflectionHelpers.getField(renderer, "inputSpacer"));
        for (int i = 0; i < 5; i++) {
            submit(i == 0 ? MoonBridge.FRAME_TYPE_IDR : MoonBridge.FRAME_TYPE_PFRAME);
        }
        assertNull(counts());
    }

    @Test
    public void aClimbOnTheOutputSideAsksFromTheInputSideAndReachesTheReport() {
        configure(MODEL, MTK, true, false);
        withoutHolds();
        submit(MoonBridge.FRAME_TYPE_IDR);
        int asked = 0;
        // Six seconds at 60 fps: 3 ms decode for three, then 20 ms.
        for (int i = 0; i < 375; i++) {
            advance(16);
            if (submit(MoonBridge.FRAME_TYPE_PFRAME) == MoonBridge.DR_NEED_IDR) asked++;
            long latencyNs = (i < 190 ? 3 : 20) * 1_000_000L;
            ReflectionHelpers.callInstanceMethod(renderer, "noteDecoderOutput",
                    ClassParameter.from(int.class, i % 8),
                    ClassParameter.from(long.class, QueueingCodec.lastTimeUs),
                    ClassParameter.from(long.class, System.nanoTime() + latencyNs));
        }
        assertEquals(1, counts().climbs);
        assertEquals(1, counts().climbRepairs);
        assertEquals(1, asked);
        String log = com.limelight.LimeLog.getRedactedLog();
        assertTrue(log, log.contains("Decoder latency climb: "));
    }

    // 9 s at 3 ms, then 3 s at this level, through the renderer at this frame rate.
    private int climbsThroughTheRenderer(int fpsMilliHz, float levelMs) {
        configure(MODEL, MTK, true, false);
        withoutHolds();
        ReflectionHelpers.setField(renderer, "streamFrameRateMilliHz", fpsMilliHz);
        submit(MoonBridge.FRAME_TYPE_IDR);
        long frameMs = 1_000_000L / fpsMilliHz;
        for (long at = 0; at < 12_000; at += frameMs) {
            advance(frameMs);
            submit(MoonBridge.FRAME_TYPE_PFRAME);
            outputAt(at < 9_000 ? 3 : levelMs);
        }
        return counts().climbs;
    }

    // The floor is half this stream's frame interval: 16.7, 8.3 and 4.2 ms. The latency reaches the net with it.
    @Test
    public void theFloorFollowsTheStreamsFrameRateAt30And60And120() {
        assertEquals(0, climbsThroughTheRenderer(30_000, 15));
        assertEquals(1, climbsThroughTheRenderer(30_000, 18));
        assertEquals(0, climbsThroughTheRenderer(60_000, 7));
        assertEquals(1, climbsThroughTheRenderer(60_000, 10));
        assertEquals("under 60 fps' floor", 1, climbsThroughTheRenderer(120_000, 8));
    }

    private int submitBytes(int bytes) {
        frame++;
        return renderer.submitDecodeUnit(new byte[bytes], bytes, MoonBridge.BUFFER_TYPE_PICDATA, frame,
                MoonBridge.FRAME_TYPE_PFRAME, (char) 0, frame * 16_000L, frame * 16_000L, frame * 16_000_000L, frame * 16_000L);
    }

    // 20 KB frames: full motion at 10 Mbps, under a quarter of a frame's share at 63.
    @Test
    public void whatIsCalmFollowsTheBitrateTheStreamIsAskedFor() {
        QueueingCodec.inputBufferBytes = 64 * 1024;
        configure(MODEL, MTK, true, false);
        withoutHolds();
        renderer.setStreamBitrateKbps(10_000);
        submit(MoonBridge.FRAME_TYPE_IDR);
        for (int i = 0; i < 110; i++) {
            advance(100);
            assertEquals(MoonBridge.DR_OK, submitBytes(20 * 1024));
        }
        assertEquals(0, counts().requests());
        renderer.setStreamBitrateKbps(63_000);
        assertEquals(MoonBridge.DR_NEED_IDR, submitBytes(20 * 1024));
        assertEquals(1, counts().quietRepairs);
    }

    // 40 KB frames at 63 Mbps: over a quarter of a frame's share at 60 fps, under it at 30.
    @Test
    public void whatIsCalmFollowsTheStreamsFrameRate() {
        for (int fpsMilliHz : new int[] {60_000, 30_000}) {
            QueueingCodec.inputBufferBytes = 64 * 1024;
            configure(MODEL, MTK, true, false);
            withoutHolds();
            ReflectionHelpers.setField(renderer, "streamFrameRateMilliHz", fpsMilliHz);
            submit(MoonBridge.FRAME_TYPE_IDR);
            for (int i = 0; i < 110; i++) {
                advance(100);
                submitBytes(40 * 1024);
            }
            assertEquals("at " + fpsMilliHz, fpsMilliHz == 30_000 ? 1 : 0, counts().quietRepairs);
        }
    }

    // Until the first once-a-second tick, what the stream started with.
    @Test
    public void beforeAnythingMovedTheBitrateTheSettingCounts() {
        QueueingCodec.inputBufferBytes = 64 * 1024;
        configure(MODEL, MTK, true, false);
        withoutHolds();
        submit(MoonBridge.FRAME_TYPE_IDR);
        advance(10_000);
        assertEquals(MoonBridge.DR_NEED_IDR, submitBytes(20 * 1024));
    }

    private void outputAt(float latencyMs) {
        ReflectionHelpers.callInstanceMethod(renderer, "noteDecoderOutput",
                ClassParameter.from(int.class, frame % 8),
                ClassParameter.from(long.class, QueueingCodec.lastTimeUs),
                ClassParameter.from(long.class, System.nanoTime() + (long) (latencyMs * 1_000_000)));
    }

    private void framesAt(int count, float latencyMs) {
        for (int i = 0; i < count; i++) {
            advance(16);
            submit(MoonBridge.FRAME_TYPE_PFRAME);
            outputAt(latencyMs);
        }
    }

    // A resize inside the configured maximum keeps the decoder, so the net goes on. A larger picture's slower
    // decode is one climb, one repair, and a minute later the new baseline: no lasting keyframes from a resize.
    @Test
    public void aResizeInPlaceKeepsTheNetAndItsNewLevelSettles() {
        MediaCodecInfo.CodecProfileLevel level = new MediaCodecInfo.CodecProfileLevel();
        level.profile = MediaCodecInfo.CodecProfileLevel.HEVCProfileMain;
        level.level = MediaCodecInfo.CodecProfileLevel.HEVCMainTierLevel51;
        QueueingCodec.info = MediaCodecInfoBuilder.newBuilder().setName(MTK).setCapabilities(
                MediaCodecInfoBuilder.CodecCapabilitiesBuilder.newBuilder()
                        .setMediaFormat(MediaFormat.createVideoFormat("video/hevc", 3840, 2160))
                        .setProfileLevels(new MediaCodecInfo.CodecProfileLevel[] {level})
                        .setColorFormats(new int[] {MediaCodecInfo.CodecCapabilities.COLOR_FormatYUV420Flexible}).build()).build();
        configure(MODEL, MTK, true, false);
        withoutHolds();
        ReflectionHelpers.setField(renderer, "adaptivePlayback", true);
        ReflectionHelpers.setField(renderer, "maxDecodeWidth", 3840);
        ReflectionHelpers.setField(renderer, "maxDecodeHeight", 2160);
        KeyframeSafetyNet net = ReflectionHelpers.getField(renderer, "keyframeSafetyNet");
        KeyframeSafetyNet.Counts before = counts();
        submit(MoonBridge.FRAME_TYPE_IDR);
        framesAt(560, 3);

        assertTrue(renderer.prepareForReconfigure(1920, 1080));
        assertEquals("no restart asked",
                ReflectionHelpers.<Integer>getStaticField(MediaCodecDecoderRenderer.class, "CR_RECOVERY_TYPE_NONE").intValue(),
                ReflectionHelpers.<java.util.concurrent.atomic.AtomicInteger>getField(renderer, "codecRecoveryType").get());
        submit(MoonBridge.FRAME_TYPE_IDR);
        assertSame(net, ReflectionHelpers.getField(renderer, "keyframeSafetyNet"));
        assertSame(before, counts());

        framesAt(4400, 12);
        assertEquals(1, counts().climbs);
        assertEquals(1, counts().climbRepairs);
        assertEquals(1, counts().settledClimbs);
        assertEquals(0, counts().climbEndRepairs);
    }

    // Another decoder for this stream starts its own watch and adds to the same counts.
    @Test
    public void aRestartedDecoderGetsANewNetAndKeepsTheCounts() {
        configure(MODEL, MTK, true, false);
        KeyframeSafetyNet first = ReflectionHelpers.getField(renderer, "keyframeSafetyNet");
        KeyframeSafetyNet.Counts counts = counts();
        MediaFormat format = MediaFormat.createVideoFormat("video/hevc", 3840, 2160);
        MediaCodecInfo info = MediaCodecInfoBuilder.newBuilder().setName(MTK).build();
        MediaCodecHelper.setDecoderLowLatencyOptions(format, info, 0, true, false, false, false);
        ReflectionHelpers.callInstanceMethod(renderer, "tryConfigureDecoder",
                ClassParameter.from(MediaCodecInfo.class, info),
                ClassParameter.from(MediaFormat.class, format),
                ClassParameter.from(boolean.class, true));
        assertNotSame(first, ReflectionHelpers.getField(renderer, "keyframeSafetyNet"));
        assertSame(counts, counts());
    }

    @Test
    public void anOutputNothingWentInForIsNoLatency() {
        configure(MODEL, MTK, true, false);
        submit(MoonBridge.FRAME_TYPE_IDR);
        KeyframeSafetyNet net = ReflectionHelpers.getField(renderer, "keyframeSafetyNet");
        for (int i = 0; i < 1000; i++) {
            advance(16);
            ReflectionHelpers.callInstanceMethod(renderer, "noteDecoderOutput",
                    ClassParameter.from(int.class, 7),
                    ClassParameter.from(long.class, 0L),
                    ClassParameter.from(long.class, System.nanoTime()));
        }
        assertNull(net.takeEvent());
        assertEquals(0, counts().climbs);
    }

    @Test
    public void theModeAndItsRepairsAreNamedInTheReport() {
        configure(MODEL, MTK, true, false);
        String log = com.limelight.LimeLog.getRedactedLog();
        assertTrue(log, log.contains("Decoder " + MTK + " in Android low latency: keyframes on a decode latency climb"));
    }

    @Test
    public void lowLatencyOffNeverAsks() {
        configure(MODEL, MTK, false, false);
        assertUntouched();
    }

    @Test
    public void theHevcOverrideLeavingTheFlagOutNeverAsks() {
        configure(MODEL, MTK, true, true);
        assertUntouched();
    }

    @Test
    public void anotherModelWithTheSameDecoderNeverAsks() {
        configure("AFTMM", MTK, true, false);
        assertUntouched();
    }

    @Test
    public void theModelWithAnotherDecoderNeverAsks() {
        configure(MODEL, "c2.amlogic.hevc.decoder", true, false);
        assertUntouched();
    }

    private void assertUntouched() {
        assertNull(ReflectionHelpers.getField(renderer, "keyframeSafetyNet"));
        assertEquals(MoonBridge.DR_OK, submit(MoonBridge.FRAME_TYPE_IDR));
        for (int i = 0; i < 3; i++) {
            advance(20_000);
            assertEquals(MoonBridge.DR_OK, submit(MoonBridge.FRAME_TYPE_PFRAME));
        }
    }
}
