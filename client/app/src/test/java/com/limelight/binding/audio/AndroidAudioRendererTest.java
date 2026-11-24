package com.limelight.binding.audio;

import android.app.Application;
import android.content.Context;
import android.content.ContextWrapper;
import android.media.AudioTrack;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;

import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;

import static org.junit.Assert.*;

/**
 * Unit tests for {@link AndroidAudioRenderer} stereo downmix, AudioTrack write recovery,
 * headset volume gating, late-packet drops, and clock-drift trimming.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class AndroidAudioRendererTest {
    @Test public void onlyJitterGapsGrowTheAAudioReserve() {
        // nativeAudioStats: 6 underruns, 2 of them jitter gaps, 4 stalls. Growing on every underrun took
        // the Retroid's reserve to its 32 ms limit on Wi-Fi stalls it never covered.
        int[] stats = new int[13];
        stats[6] = 6;
        stats[AndroidAudioRenderer.STAT_JITTER_GAPS] = 2;
        stats[AndroidAudioRenderer.STAT_STALLS] = 4;
        assertEquals(11, AndroidAudioRenderer.STAT_JITTER_GAPS);
        assertEquals(12, AndroidAudioRenderer.STAT_STALLS);
        assertEquals(2, AndroidAudioRenderer.reserveStarvations(stats));
    }

    @Test public void headsetMixKeepsLeftRightAndCenterInTheirCorrectChannels() {
        for (int channels : new int[] {6, 8}) {
            short[][] expected = {{1000, 0}, {0, 1000}, {707, 707}, {0, 0},
                    {707, 0}, {0, 707}, {707, 0}, {0, 707}};
            for (int channel = 0; channel < channels; channel++) {
                short[] input = new short[channels];
                input[channel] = 1000;
                short[] stereo = new short[2];
                AndroidAudioRenderer.downmixStereo(input, input.length, channels, stereo);
                assertArrayEquals(expected[channel], stereo);
            }
        }
    }

    @Test public void headsetMixKeepsBothRearChannelsOfAFourChannelStream() {
        // Quad is FL FR RL RR: the left rear belongs to the left ear and the right rear to the right one.
        short[] frontLeftRearLeft = {1000, 0, 1000, 0};
        short[] frontRightRearRight = {0, 1000, 0, 1000};
        short[] stereo = new short[2];
        AndroidAudioRenderer.downmixStereo(frontLeftRearLeft, 4, 4, stereo);
        assertArrayEquals(new short[] {1707, 0}, stereo);
        AndroidAudioRenderer.downmixStereo(frontRightRearRight, 4, 4, stereo);
        assertArrayEquals(new short[] {0, 1707}, stereo);
    }

    @Test public void headsetMixKeepsTheHeightsOfSevenOneFourOnTheirSides() {
        // TFL TFR TBL TBR follow the 7.1 bed.
        short[][] heights = {{707, 0}, {0, 707}, {707, 0}, {0, 707}};
        for (int height = 0; height < 4; height++) {
            short[] input = new short[12];
            input[8 + height] = 1000;
            short[] stereo = new short[2];
            AndroidAudioRenderer.downmixStereo(input, input.length, 12, stereo);
            assertArrayEquals(heights[height], stereo);
        }
    }

    static final class ScriptedRenderer extends AndroidAudioRenderer {
        final int[] results;
        int nextResult;
        boolean recreationSucceeds = true;
        boolean fallbackSucceeds = true;
        boolean nativeWriteSucceeds = true;
        int recreations;
        String fallbackReason;
        long now = 1000;
        final List<Integer> offsets = new ArrayList<>();
        final List<Integer> lengths = new ArrayList<>();
        final List<short[]> writtenData = new ArrayList<>();

        ScriptedRenderer(int... results) {
            this(null, null, results);
        }

        ScriptedRenderer(Context context, HeadphoneSink headphones, int... results) {
            super(context, false, headphones);
            this.results = results;
        }

        @Override int writeAudioTrack(short[] data, int offset, int length) {
            offsets.add(offset);
            lengths.add(length);
            if (offset == 0) writtenData.add(data.clone());
            assertTrue("Unexpected retry", nextResult < results.length);
            return results[nextResult++];
        }

        @Override boolean recreateAudioTrack() {
            recreations++;
            return recreationSucceeds;
        }

        @Override boolean fallbackToAudioTrack(String reason) {
            fallbackReason = reason;
            ReflectionHelpers.setField(this, "nativeAudio", 0L);
            ReflectionHelpers.setField(this, "bufferPolicy", null);
            return fallbackSucceeds;
        }

        // A native write that waits for room in a full output, as nativeWriteAudio does for up to 100 ms
        // (here 10 s), until abortNativeAudio() cuts it short.
        final CountDownLatch writeWaiting = new CountDownLatch(1), writeAborted = new CountDownLatch(1);
        boolean stallWrites;
        volatile boolean writeWasAborted;
        volatile int aborts;

        @Override void abortNativeAudio() {
            aborts++;
            writeAborted.countDown();
        }

        @Override int writeNativeAudio(short[] data, int length) {
            nativeWrites++;
            if (stallWrites) {
                writeWaiting.countDown();
                try {
                    writeWasAborted = writeAborted.await(10, TimeUnit.SECONDS);
                } catch (InterruptedException e) {
                    Thread.currentThread().interrupt();
                }
                return NATIVE_WRITE_STOPPED;
            }
            if (stallNextNativeWrites > 0) {
                stallNextNativeWrites--;
                return NATIVE_WRITE_STALLED;
            }
            if (failNextNativeWrites > 0) {
                failNextNativeWrites--;
                return NATIVE_WRITE_STOPPED;
            }
            return nativeWriteSucceeds ? NATIVE_WRITE_DONE : NATIVE_WRITE_STOPPED;
        }

        boolean reopenSucceeds;
        int reopens, closes, nativeWrites, failNextNativeWrites, stallNextNativeWrites;

        @Override boolean openNativeAudio() {
            reopens++;
            if (!reopenSucceeds) return false;
            ReflectionHelpers.setField(this, "nativeAudio", 2L);
            ReflectionHelpers.setField(this, "bufferPolicy", new AudioBufferPolicy(192, 384, 384, 0, 0));
            ReflectionHelpers.setField(this, "lastBufferCheckMs", Long.MAX_VALUE);
            return true;
        }

        @Override void closeNativeAudio() {
            closes++;
            ReflectionHelpers.setField(this, "nativeAudio", 0L);
        }

        @Override long uptimeMillis() {
            return now;
        }

        int pendingMs, queuedFrames;

        @Override int pendingAudioMs() { return pendingMs; }
        @Override int outputQueuedFrames() { return queuedFrames; }

        boolean late;
        final List<Integer> lateChecks = new ArrayList<>(), latePending = new ArrayList<>();

        @Override boolean skipLateNativeAudio(int frames, int pendingFrames) {
            lateChecks.add(frames);
            latePending.add(pendingFrames);
            return late;
        }

        @Override int checkLateAndQueuedNativeAudio(int frames, int pendingFrames) {
            return skipLateNativeAudio(frames, pendingFrames) ? -1 : outputQueuedFrames();
        }

        void play() { playDecodedAudio(new short[6], 6); }
    }

    // TCL C6K TV on Wi-Fi: a 130 ms stall drained the 162 ms reserve to 30 ms, then the stalled audio came at once.
    static ScriptedRenderer afterAStall(int... results) {
        ScriptedRenderer renderer = new ScriptedRenderer(results);
        ReflectionHelpers.setField(renderer, "sampleRate", 48000);
        ReflectionHelpers.setField(renderer, "channels", 2);
        ReflectionHelpers.setField(renderer, "reserveFrames", 9 * 864);
        renderer.queuedFrames = 1440;
        renderer.pendingMs = 130;
        return renderer;
    }

    static ScriptedRenderer disconnectedAAudio() {
        ScriptedRenderer renderer = new ScriptedRenderer(6);
        renderer.nativeWriteSucceeds = false;
        ReflectionHelpers.setField(renderer, "nativeAudio", 1L);
        ReflectionHelpers.setField(renderer, "bufferPolicy", new AudioBufferPolicy(192, 384, 384, 0, 0));
        ReflectionHelpers.setField(renderer, "lastBufferCheckMs", Long.MAX_VALUE);
        return renderer;
    }

    @Test public void partialWritesContinueAtTheUnwrittenSamples() {
        ScriptedRenderer renderer = new ScriptedRenderer(2, 1, 3);
        assertTrue(renderer.writeAudioTrackFully(new short[6], 6));
        assertEquals(List.of(0, 2, 3), renderer.offsets);
        assertEquals(List.of(6, 4, 3), renderer.lengths);
        assertEquals(0, renderer.recreations);
    }

    @Test public void theBurstAfterANetworkStallPlaysInsteadOfBeingDropped() {
        ScriptedRenderer renderer = afterAStall(6);
        renderer.play();
        assertEquals("written to the drained track", 1, renderer.nextResult);
        assertEquals(Long.valueOf(0), ReflectionHelpers.<Long>getField(renderer, "lateDropped"));

        // Audio beyond the reserve and 40 ms is still late.
        renderer = afterAStall();
        renderer.pendingMs = 175;
        renderer.play();
        assertEquals(0, renderer.nextResult);
        assertEquals(Long.valueOf(1), ReflectionHelpers.<Long>getField(renderer, "lateDropped"));
    }

    @Test public void theAudioTrackPathNeverAsksForLatePackets() {
        ScriptedRenderer renderer = afterAStall(6);
        renderer.late = true;
        renderer.play();
        assertTrue(renderer.lateChecks.isEmpty());
        assertEquals(1, renderer.nextResult);
    }

    @Test public void deadTrackIsRecreatedWithoutReplayingAcceptedSamples() {
        ScriptedRenderer renderer = new ScriptedRenderer(2, AudioTrack.ERROR_DEAD_OBJECT, 4);
        assertTrue(renderer.writeAudioTrackFully(new short[6], 6));
        assertEquals(List.of(0, 2, 2), renderer.offsets);
        assertEquals(List.of(6, 4, 4), renderer.lengths);
        assertEquals(1, renderer.recreations);
    }

    @Test public void repeatedDisconnectStopsAfterOneRecreation() {
        ScriptedRenderer renderer = new ScriptedRenderer(
                AudioTrack.ERROR_DEAD_OBJECT, AudioTrack.ERROR_DEAD_OBJECT);
        assertFalse(renderer.writeAudioTrackFully(new short[6], 6));
        assertEquals(2, renderer.nextResult);
        assertEquals(1, renderer.recreations);
    }

    @Test public void failedRecreationDoesNotRetryTheDeadTrack() {
        ScriptedRenderer renderer = new ScriptedRenderer(AudioTrack.ERROR_DEAD_OBJECT);
        renderer.recreationSucceeds = false;
        assertFalse(renderer.writeAudioTrackFully(new short[6], 6));
        assertEquals(1, renderer.nextResult);
        assertEquals(1, renderer.recreations);
    }

    @Test public void zeroAndUnrecoverableErrorsDropOnlyThatBuffer() {
        for (int result : new int[] {0, AudioTrack.ERROR, AudioTrack.ERROR_BAD_VALUE, AudioTrack.ERROR_INVALID_OPERATION}) {
            ScriptedRenderer renderer = new ScriptedRenderer(result, 6);
            // The failed buffer is not retried on this thread and the track is kept.
            renderer.play();
            assertEquals(1, renderer.nextResult);
            assertEquals(0, renderer.recreations);
            // The next decoded buffer still plays: one bad write must not silence the session.
            renderer.play();
            assertEquals(2, renderer.nextResult);
            assertEquals(List.of(0, 0), renderer.offsets);
            assertEquals(0, renderer.recreations);
        }
    }

    @Test public void bufferDropsAreCountedForTheProblemReport() {
        ScriptedRenderer renderer = new ScriptedRenderer(AudioTrack.ERROR, 6);
        // Over the 40 ms queue bound: dropped before any write, counted as late.
        renderer.pendingMs = 45;
        renderer.play();
        renderer.play();
        assertEquals(0, renderer.nextResult);
        assertEquals(Long.valueOf(2), ReflectionHelpers.<Long>getField(renderer, "lateDropped"));
        assertEquals(Long.valueOf(0), ReflectionHelpers.<Long>getField(renderer, "droppedWrites"));
        // A refused write is counted separately and the next buffer still plays.
        renderer.pendingMs = 10;
        renderer.play();
        renderer.play();
        assertEquals(2, renderer.nextResult);
        assertEquals(Long.valueOf(2), ReflectionHelpers.<Long>getField(renderer, "lateDropped"));
        assertEquals(Long.valueOf(1), ReflectionHelpers.<Long>getField(renderer, "droppedWrites"));
    }

    @Test public void deadTrackRecreationIsRateLimitedAndRecoversLaterBuffers() {
        int dead = AudioTrack.ERROR_DEAD_OBJECT;
        ScriptedRenderer renderer = new ScriptedRenderer(dead, dead, dead, dead, 6);
        renderer.play();
        assertEquals("One recreation, then the still-dead buffer is dropped", 1, renderer.recreations);
        assertEquals(2, renderer.nextResult);
        renderer.now += 500;
        renderer.play();
        assertEquals("No recreation storm while the output stays dead", 1, renderer.recreations);
        assertEquals(3, renderer.nextResult);
        renderer.now += 500;
        renderer.play();
        assertEquals(2, renderer.recreations);
        assertEquals("The recreated track plays the buffer", 5, renderer.nextResult);
    }

    @Test public void headsetVolumeIsReadOnlyForALiveRouteAndOffTheRendererLock() {
        AtomicInteger volumeReads = new AtomicInteger();
        AndroidAudioRenderer[] renderer = new AndroidAudioRenderer[1];
        Context context = new ContextWrapper(RuntimeEnvironment.getApplication()) {
            @Override public Object getSystemService(String name) {
                if (Context.AUDIO_SERVICE.equals(name)) {
                    volumeReads.incrementAndGet();
                    assertFalse("stop() and cleanup() must not wait for AudioService", Thread.holdsLock(renderer[0]));
                }
                return super.getSystemService(name);
            }
        };
        boolean[] headsetRoute = {false};
        List<Float> headset = new ArrayList<>();
        AndroidAudioRenderer.HeadphoneSink sink = new AndroidAudioRenderer.HeadphoneSink() {
            @Override public boolean write(short[] pcm, int length, float volume) {
                if (!headsetRoute[0]) return false;
                headset.add(volume);
                return true;
            }
            @Override public boolean active() { return headsetRoute[0]; }
        };
        ScriptedRenderer scripted = new ScriptedRenderer(context, sink, 6, 6);
        renderer[0] = scripted;
        // A 48 kHz stereo stream, as negotiated in setup().
        ReflectionHelpers.setField(scripted, "sampleRate", 48000);
        ReflectionHelpers.setField(scripted, "channels", 2);

        scripted.play();
        scripted.now += 1000;
        scripted.play();
        assertEquals("No DualSense headset: no AudioService calls", 0, volumeReads.get());
        assertEquals(2, scripted.nextResult);

        headsetRoute[0] = true;
        scripted.play();
        scripted.now += 100;
        scripted.play();
        assertEquals("The volume is read at most twice a second", 1, volumeReads.get());
        assertEquals(2, headset.size());
        assertEquals("The headset took the audio", 2, scripted.nextResult);
    }

    // Before Android 7.0 a dead track's write() returns the native status (DEAD_OBJECT is -32),
    // never ERROR_DEAD_OBJECT, so only the run of failed buffers shows that the track is gone.
    private static final class LegacyDeadTrack extends AndroidAudioRenderer {
        boolean dead = true, recreationRevives = true;
        int writes, recreations;
        long now = 1000;

        LegacyDeadTrack() {
            super(null, false, null);
        }

        @Override int writeAudioTrack(short[] data, int offset, int length) {
            writes++;
            return dead ? -32 : length;
        }

        @Override boolean recreateAudioTrack() {
            recreations++;
            dead = !recreationRevives;
            return true;
        }

        @Override long uptimeMillis() {
            return now;
        }

        void play() {
            playDecodedAudio(new short[6], 6);
        }
    }

    @Test public void aTrackThatOnlyFailsIsRecreatedAfterAShortRunOfDroppedBuffers() {
        LegacyDeadTrack renderer = new LegacyDeadTrack();
        for (int i = 0; i < 10 && renderer.recreations == 0; i++) renderer.play();
        assertEquals("A track failing every write must not stay silent for the session", 1, renderer.recreations);
        int writes = renderer.writes;
        renderer.play();
        assertEquals("The recreated track plays the next buffer", writes + 1, renderer.writes);
        assertEquals(1, renderer.recreations);
    }

    @Test public void recreatingATrackThatOnlyFailsIsRateLimited() {
        LegacyDeadTrack renderer = new LegacyDeadTrack();
        renderer.recreationRevives = false;
        for (int i = 0; i < 50; i++) renderer.play();
        assertEquals("No recreation storm while the output stays broken", 1, renderer.recreations);
        renderer.now += 1000;
        renderer.recreationRevives = true;
        for (int i = 0; i < 10 && renderer.recreations == 1; i++) renderer.play();
        assertEquals(2, renderer.recreations);
        assertFalse(renderer.dead);
    }

    @Test public void clockDriftSurplusTrimsOnePacketAndTheNextFadesInFromIt() {
        // 5 ms stereo packets with 15 ms waiting beyond a full output: surplus a slow device clock built up.
        int[] results = new int[400];
        java.util.Arrays.fill(results, 480);
        ScriptedRenderer renderer = new ScriptedRenderer(results);
        ReflectionHelpers.setField(renderer, "sampleRate", 48000);
        ReflectionHelpers.setField(renderer, "channels", 2);
        ReflectionHelpers.setField(renderer, "reserveFrames", 672);
        renderer.queuedFrames = 672;
        renderer.pendingMs = 15;
        for (int i = 0; i < 401; i++) {
            short[] pcm = new short[480];
            java.util.Arrays.fill(pcm, (short)(i == 399 ? 1000 : i == 400 ? -1000 : 0));
            renderer.playDecodedAudio(pcm, 480);
        }
        // 2 s of packets above the line: the 400th goes, faded into the 401st, which starts where it would have.
        assertEquals(400, renderer.writtenData.size());
        assertEquals(Long.valueOf(1), ReflectionHelpers.<Long>getField(renderer, "driftTrimmed"));
        assertEquals(Long.valueOf(0), ReflectionHelpers.<Long>getField(renderer, "lateDropped"));
        short[] faded = renderer.writtenData.get(399);
        assertEquals(1000, faded[0]);
        assertEquals(1000, faded[1]);
        assertEquals((-1000 * 120 + 1000 * 120) / 240, faded[240]);
        assertEquals((-1000 * 239 + 1000) / 240, faded[479]);
        assertEquals(0, renderer.writtenData.get(398)[0]);
    }

    /** What the renderer logs while {@code packets} runs, lines starting with {@code prefix} only. */
    private static List<String> logged(String prefix, Runnable packets) {
        List<String> lines = new ArrayList<>();
        java.util.logging.Logger logger = java.util.logging.Logger.getLogger(com.limelight.LimeLog.class.getName());
        java.util.logging.Handler handler = new java.util.logging.Handler() {
            @Override public void publish(java.util.logging.LogRecord record) {
                if (record.getMessage().startsWith(prefix)) lines.add(record.getMessage());
            }
            @Override public void flush() { }
            @Override public void close() { }
        };
        logger.addHandler(handler);
        try {
            packets.run();
        } finally {
            logger.removeHandler(handler);
        }
        return lines;
    }

    @Test public void aRunOfLatePacketsIsDroppedOneByOneAndLoggedOnceASecond() {
        // Audio beyond the reserve and 40 ms, as after a long stall: every packet of the burst is late.
        ScriptedRenderer renderer = afterAStall();
        renderer.pendingMs = 175;
        List<String> lines = logged("Too much pending audio data", () -> {
            for (int i = 0; i < 50; i++) renderer.play();
            renderer.now += 999;
            renderer.play();
            renderer.now += 1;
            renderer.play();
            renderer.play();
        });
        // Each packet is still dropped and counted; only the lines are fewer.
        assertEquals(0, renderer.nextResult);
        assertEquals(Long.valueOf(53), ReflectionHelpers.<Long>getField(renderer, "lateDropped"));
        assertEquals(List.of(
                "Too much pending audio data: 175 ms, output 30 of 162 ms (1 dropped since the last line)",
                "Too much pending audio data: 175 ms, output 30 of 162 ms (51 dropped since the last line)"), lines);
    }

    @Test public void aRunOfDriftTrimsIsLoggedOnceASecondWithItsCount() {
        int[] results = new int[2400];
        java.util.Arrays.fill(results, 480);
        ScriptedRenderer renderer = new ScriptedRenderer(results);
        ReflectionHelpers.setField(renderer, "sampleRate", 48000);
        ReflectionHelpers.setField(renderer, "channels", 2);
        ReflectionHelpers.setField(renderer, "reserveFrames", 672);
        renderer.queuedFrames = 672;
        renderer.pendingMs = 15;
        // 5 ms packets above the line for 8 s, all inside one second of the renderer's clock.
        List<String> lines = logged("Audio clock drift: trimmed", () -> {
            for (int i = 0; i < 1600; i++) renderer.playDecodedAudio(new short[480], 480);
        });
        long trimmed = ReflectionHelpers.<Long>getField(renderer, "driftTrimmed");
        assertTrue("several packets go in 8 s: " + trimmed, trimmed > 1);
        assertEquals(1600 - trimmed, renderer.writtenData.size());
        assertEquals(List.of(
                "Audio clock drift: trimmed a packet, pending 15 ms, output 14 of 14 ms (1 trimmed, 1 since the last line)"),
                lines);

        renderer.now += 1000;
        lines = logged("Audio clock drift: trimmed", () -> {
            for (int i = 0; i < 800; i++) renderer.playDecodedAudio(new short[480], 480);
        });
        long later = ReflectionHelpers.<Long>getField(renderer, "driftTrimmed");
        assertTrue(later > trimmed);
        assertEquals(List.of("Audio clock drift: trimmed a packet, pending 15 ms, output 14 of 14 ms (" + (trimmed + 1)
                + " trimmed, " + trimmed + " since the last line)"), lines);
    }

    @Test public void aSteadyOutputIsNeverTrimmed() {
        int[] results = new int[2000];
        java.util.Arrays.fill(results, 480);
        ScriptedRenderer renderer = new ScriptedRenderer(results);
        ReflectionHelpers.setField(renderer, "sampleRate", 48000);
        ReflectionHelpers.setField(renderer, "channels", 2);
        ReflectionHelpers.setField(renderer, "reserveFrames", 672);
        // Pairs of packets: the first sees its partner waiting, the second only the output.
        for (int i = 0; i < 2000; i++) {
            renderer.queuedFrames = 672 - 240 * (i % 2);
            renderer.pendingMs = i % 2 == 0 ? 5 : 0;
            renderer.playDecodedAudio(new short[480], 480);
        }
        assertEquals(2000, renderer.writtenData.size());
        assertEquals(Long.valueOf(0), ReflectionHelpers.<Long>getField(renderer, "driftTrimmed"));
    }

    @Test public void failedRecreationIsRetriedByALaterBuffer() {
        int dead = AudioTrack.ERROR_DEAD_OBJECT;
        ScriptedRenderer renderer = new ScriptedRenderer(dead, dead, 6);
        renderer.recreationSucceeds = false;
        renderer.play();
        assertEquals(1, renderer.recreations);
        renderer.now += 1000;
        renderer.recreationSucceeds = true;
        renderer.play();
        assertEquals(2, renderer.recreations);
        assertEquals(3, renderer.nextResult);
    }
}
