package com.limelight.binding.audio;

import android.app.Application;
import android.content.Context;
import android.content.ContextWrapper;
import android.media.AudioDeviceInfo;
import android.media.AudioTrack;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;

import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.atomic.AtomicInteger;

import static org.junit.Assert.*;

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

    private static final class ScriptedRenderer extends AndroidAudioRenderer {
        private final int[] results;
        private int nextResult;
        private boolean recreationSucceeds = true;
        private boolean fallbackSucceeds = true;
        private boolean nativeWriteSucceeds = true;
        private int recreations;
        private String fallbackReason;
        private long now = 1000;
        private final List<Integer> offsets = new ArrayList<>();
        private final List<Integer> lengths = new ArrayList<>();

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

        @Override boolean writeNativeAudio(short[] data, int length) {
            return nativeWriteSucceeds;
        }

        @Override long uptimeMillis() {
            return now;
        }

        private int pendingMs, queuedFrames;

        @Override int pendingAudioMs() {
            return pendingMs;
        }

        @Override int outputQueuedFrames() {
            return queuedFrames;
        }

        private boolean late;
        private final List<Integer> lateChecks = new ArrayList<>(), latePending = new ArrayList<>();

        @Override boolean skipLateNativeAudio(int frames, int pendingFrames) {
            lateChecks.add(frames);
            latePending.add(pendingFrames);
            return late;
        }

        void play() {
            playDecodedAudio(new short[6], 6);
        }
    }

    @Test public void partialWritesContinueAtTheUnwrittenSamples() {
        ScriptedRenderer renderer = new ScriptedRenderer(2, 1, 3);
        assertTrue(renderer.writeAudioTrackFully(new short[6], 6));
        assertEquals(List.of(0, 2, 3), renderer.offsets);
        assertEquals(List.of(6, 4, 3), renderer.lengths);
        assertEquals(0, renderer.recreations);
    }

    @Test public void persistentNativeUnderrunsSwitchOutputBeforeWritingTheNextPacket() {
        ScriptedRenderer renderer = new ScriptedRenderer(6);
        AudioBufferPolicy policy = new AudioBufferPolicy(192, 384, 384, 0, 0);
        for (int i = 1; i <= 3; i++) policy.sample(i, i * 1000L);
        ReflectionHelpers.setField(renderer, "nativeAudio", 1L);
        ReflectionHelpers.setField(renderer, "bufferPolicy", policy);
        renderer.play();
        assertEquals("AAudio hardware keeps underrunning at its largest device buffer", renderer.fallbackReason);
        assertEquals(List.of(0), renderer.offsets);
        assertEquals(List.of(6), renderer.lengths);
    }

    // TCL C6K TV on Wi-Fi: a 130 ms stall drained the 162 ms reserve to 30 ms, then the stalled audio came at once.
    private static ScriptedRenderer afterAStall(int... results) {
        ScriptedRenderer renderer = new ScriptedRenderer(results);
        ReflectionHelpers.setField(renderer, "sampleRate", 48000);
        ReflectionHelpers.setField(renderer, "channels", 2);
        ReflectionHelpers.setField(renderer, "reserveFrames", 9 * 864);
        renderer.queuedFrames = 1440;
        renderer.pendingMs = 130;
        return renderer;
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

    @Test public void aStallsConcealedPacketsAreSkippedNotWrittenOrCountedAsDrops() {
        ScriptedRenderer renderer = afterAStall(6);
        ReflectionHelpers.setField(renderer, "nativeAudio", 1L);
        ReflectionHelpers.setField(renderer, "bufferPolicy", new AudioBufferPolicy(864, 6912, 1728, 0, 0));
        ReflectionHelpers.setField(renderer, "lastBufferCheckMs", Long.MAX_VALUE);
        renderer.nativeWriteSucceeds = false;
        renderer.late = true;
        renderer.play();
        // Asked per packet in frames, with the network queue behind it in frames (130 ms at 48 kHz): those
        // packets refill the reserve, so the native side skips only the excess. Skipped before any write or fallback.
        assertEquals(List.of(3), renderer.lateChecks);
        assertEquals(List.of(6240), renderer.latePending);
        assertNull(renderer.fallbackReason);
        assertEquals(0, renderer.nextResult);
        assertEquals(Long.valueOf(0), ReflectionHelpers.<Long>getField(renderer, "lateDropped"));
        assertEquals(Long.valueOf(0), ReflectionHelpers.<Long>getField(renderer, "droppedWrites"));
        // A packet in time is written as before.
        renderer.late = false;
        renderer.nativeWriteSucceeds = true;
        renderer.play();
        assertNull(renderer.fallbackReason);
        assertEquals(List.of(3, 3), renderer.lateChecks);
        ReflectionHelpers.setField(renderer, "nativeAudio", 0L);
    }

    @Test public void theAudioTrackPathNeverAsksForLatePackets() {
        ScriptedRenderer renderer = afterAStall(6);
        renderer.late = true;
        renderer.play();
        assertTrue(renderer.lateChecks.isEmpty());
        assertEquals(1, renderer.nextResult);
    }

    @Test public void aaudioReserveStarvationDoesNotSwitchToAudioTrack() {
        ScriptedRenderer renderer = afterAStall();
        AudioBufferPolicy starved = AudioBufferPolicy.forReserve(192, 384, 384, 0, 0, 0);
        for (int i = 1; i <= 4; i++) starved.sample(i, i * 1000L);
        assertTrue(starved.needsFallback());
        ReflectionHelpers.setField(renderer, "nativeAudio", 1L);
        ReflectionHelpers.setField(renderer, "bufferPolicy", new AudioBufferPolicy(192, 384, 384, 0, 0));
        ReflectionHelpers.setField(renderer, "reservePolicy", starved);
        ReflectionHelpers.setField(renderer, "lastBufferCheckMs", Long.MAX_VALUE);
        renderer.play();
        // Late network audio would starve AudioTrack the same way: AAudio keeps it.
        assertNull(renderer.fallbackReason);
        assertEquals(0, renderer.nextResult);
        assertEquals(Long.valueOf(0), ReflectionHelpers.<Long>getField(renderer, "lateDropped"));
        assertEquals(Long.valueOf(0), ReflectionHelpers.<Long>getField(renderer, "droppedWrites"));
        ReflectionHelpers.setField(renderer, "nativeAudio", 0L);
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

    // A real renderer whose output can be made to refuse new tracks, as during a route change.
    private static final class UnavailableOutput extends AndroidAudioRenderer {
        int refusedTracks;
        int rateReads, rate = 44100, routedType = -1, pendingMs;
        int perfMode = AudioBufferPolicy.PERF_NONE, burst = 192;
        long now = 1000;
        int[][] attached = new int[0][];

        @Override int nativeOutputRate() {
            rateReads++;
            return rate;
        }

        @Override int routedTrackType() {
            return routedType;
        }

        @Override int[][] attachedOutputs() {
            return attached;
        }

        // Thrown once a track exists, as when the framework fails it before it plays.
        RuntimeException trackFailure;

        @Override int trackPerformanceMode() {
            if (trackFailure != null) throw trackFailure;
            return perfMode;
        }

        @Override int outputFramesPerBuffer() {
            return burst;
        }

        @Override int pendingAudioMs() {
            return pendingMs;
        }

        // The framework minimum in bytes; negative asks the platform.
        int minBytes = -1;

        @Override int minBufferBytes() {
            return minBytes >= 0 ? minBytes : super.minBufferBytes();
        }

        @Override long uptimeMillis() {
            return now;
        }

        UnavailableOutput(Context context) {
            super(context, true, null);
        }

        @Override AudioTrack createAudioTrack(int channelConfig, int sampleRate, int bufferSize, boolean lowLatency) {
            if (refusedTracks > 0) {
                refusedTracks--;
                throw new UnsupportedOperationException("No audio output available");
            }
            return super.createAudioTrack(channelConfig, sampleRate, bufferSize, lowLatency);
        }
    }

    @Test public void equalizersReattachAfterARecreationThatFirstFailed() {
        List<String> effectSessions = new ArrayList<>();
        Context context = new ContextWrapper(RuntimeEnvironment.getApplication()) {
            @Override public void sendBroadcast(android.content.Intent intent) {
                effectSessions.add(intent.getAction().endsWith("OPEN_AUDIO_EFFECT_CONTROL_SESSION") ? "open" : "close");
            }
        };
        UnavailableOutput renderer = new UnavailableOutput(context);
        assertEquals(0, renderer.setup(new com.limelight.nvstream.jni.MoonBridge.AudioConfiguration(2, 0x3), 48000, 240));
        renderer.start();
        assertEquals(List.of("open"), effectSessions);

        // The output is gone: every attempt to build a new track fails, so audio stays down for now.
        renderer.refusedTracks = Integer.MAX_VALUE;
        assertFalse(renderer.recreateAudioTrack());
        assertEquals(List.of("open", "close"), effectSessions);
        // A second later the output is back.
        renderer.refusedTracks = 0;
        assertTrue(renderer.recreateAudioTrack());
        assertEquals("The equalizer session must follow the replacement track",
                List.of("open", "close", "open"), effectSessions);

        // A stopped stream does not ask for effects again.
        renderer.stop();
        assertTrue(renderer.recreateAudioTrack());
        assertEquals(List.of("open", "close", "open", "close"), effectSessions);
        renderer.cleanup();
    }

    @Test public void noTrackLeavesNoReserveForTheLateBound() {
        UnavailableOutput renderer = new UnavailableOutput(RuntimeEnvironment.getApplication());
        assertEquals(0, renderer.setup(STEREO, 48000, 240));
        assertTrue((int) ReflectionHelpers.getField(renderer, "reserveFrames") > 0);
        // Every track is built and sized, then fails before it plays: the reserve it was given goes too.
        renderer.trackFailure = new IllegalStateException("track died");
        assertFalse(renderer.recreateAudioTrack());
        assertEquals(0, (int) ReflectionHelpers.getField(renderer, "reserveFrames"));
        renderer.cleanup();
    }

    private static String audioReport() {
        return ReflectionHelpers.getStaticField(com.limelight.utils.ProblemReport.class, "audioDiagnostics");
    }

    private static final com.limelight.nvstream.jni.MoonBridge.AudioConfiguration STEREO =
            new com.limelight.nvstream.jni.MoonBridge.AudioConfiguration(2, 0x3);

    @Test public void reportReadsTheNativeRateOnceAndFollowsTheRecreatedOutput() {
        UnavailableOutput renderer = new UnavailableOutput(RuntimeEnvironment.getApplication());
        renderer.routedType = AudioDeviceInfo.TYPE_BLUETOOTH_A2DP;
        renderer.attached = new int[][] {{1, AudioDeviceInfo.TYPE_BUILTIN_SPEAKER}, {7, AudioDeviceInfo.TYPE_BLUETOOTH_A2DP}};
        assertEquals(0, renderer.setup(STEREO, 48000, 240));
        // One read for the AAudio decision, reused by the track; reports must not ask again.
        assertEquals(1, renderer.rateReads);
        for (int i = 0; i < 3; i++) renderer.outputDetails();
        assertEquals(1, renderer.rateReads);
        assertTrue(audioReport(), audioReport().contains("nativeRate=44100"));
        assertTrue(audioReport(), audioReport().contains("route=bluetooth-a2dp"));
        assertFalse("only the routed device is named", audioReport().contains("speaker"));

        // The output moved and the track was rebuilt: both values are read again.
        renderer.rate = 48000;
        renderer.routedType = AudioDeviceInfo.TYPE_BUILTIN_SPEAKER;
        assertTrue(renderer.recreateAudioTrack());
        assertEquals(2, renderer.rateReads);
        assertTrue(audioReport(), audioReport().contains("nativeRate=48000"));
        assertTrue(audioReport(), audioReport().contains("route=speaker"));
        renderer.cleanup();
    }

    @Test public void reportNamesTheGrantedPathAndTheBufferFloor() {
        // #15: the fast path was denied, so the track runs on the normal mixer at the framework minimum.
        UnavailableOutput renderer = new UnavailableOutput(RuntimeEnvironment.getApplication());
        renderer.rate = 48000;
        int minFrames = AudioTrack.getMinBufferSize(48000, android.media.AudioFormat.CHANNEL_OUT_STEREO,
                android.media.AudioFormat.ENCODING_PCM_16BIT) / 4;
        assertEquals(0, renderer.setup(STEREO, 48000, 240));
        assertTrue(audioReport(), audioReport().contains("perfMode=none"));
        assertTrue(audioReport(), audioReport().contains("outputBurst=192"));
        assertTrue(audioReport(), audioReport().contains("minBufferFrames=" + minFrames));
        assertTrue(audioReport(), audioReport().contains("floorFrames=" + Math.max(480, minFrames)));
        renderer.cleanup();

        // A granted fast path keeps the two-packet floor.
        renderer = new UnavailableOutput(RuntimeEnvironment.getApplication());
        renderer.rate = 48000;
        renderer.perfMode = AudioBufferPolicy.PERF_LOW_LATENCY;
        assertEquals(0, renderer.setup(STEREO, 48000, 240));
        assertTrue(audioReport(), audioReport().contains("perfMode=lowLatency"));
        assertTrue(audioReport(), audioReport().contains("floorFrames=480"));
        renderer.cleanup();
    }

    // Surround never takes AAudio, and the fast path is not granted to a 6 or 8 channel track: it runs on the
    // normal mixer. On a Retroid Pocket 6 the 5.1 minimum is 3844 frames; the old two-packet buffer underran
    // 118 times a minute there.
    @Test public void surroundTracksRefusedTheFastPathKeepTheirMinimumAndSizeBytesPerChannel() {
        int[][] layouts = {{6, 0x3F}, {8, 0x63F}};
        for (int[] layout : layouts) {
            int channels = layout[0];
            UnavailableOutput renderer = new UnavailableOutput(RuntimeEnvironment.getApplication());
            renderer.rate = 48000;
            renderer.minBytes = 3844 * channels * 2;
            assertEquals(0, renderer.setup(new com.limelight.nvstream.jni.MoonBridge.AudioConfiguration(channels, layout[1]),
                    48000, 240));
            String report = audioReport();
            assertTrue(report, report.contains("channels=" + channels + ","));
            // Two packets of 240 frames, two bytes per sample.
            assertTrue(report, report.contains("bufferBytes=" + 240 * channels * 2 * 2 + ","));
            assertTrue(report, report.contains("perfMode=none"));
            assertTrue(report, report.contains("minBufferFrames=3844"));
            assertTrue(report, report.contains("floorFrames=3844"));
            assertTrue(report, report.contains("nativeRate=48000"));

            renderer.cleanup();
        }
    }

    @Test public void surroundTrackOnAGrantedFastPathKeepsTheTwoPacketFloor() {
        UnavailableOutput renderer = new UnavailableOutput(RuntimeEnvironment.getApplication());
        renderer.rate = 48000;
        renderer.perfMode = AudioBufferPolicy.PERF_LOW_LATENCY;
        renderer.minBytes = 3844 * 6 * 2;
        assertEquals(0, renderer.setup(new com.limelight.nvstream.jni.MoonBridge.AudioConfiguration(6, 0x3F), 48000, 240));
        assertTrue(audioReport(), audioReport().contains("floorFrames=480"));
        renderer.cleanup();
    }

    @Test public void unroutedOutputListsEveryAttachedOutputUntilTheTrackIsRouted() {
        UnavailableOutput renderer = new UnavailableOutput(RuntimeEnvironment.getApplication());
        renderer.attached = new int[][] {{1, AudioDeviceInfo.TYPE_BUILTIN_SPEAKER}, {7, AudioDeviceInfo.TYPE_BLUETOOTH_A2DP}};
        assertEquals(0, renderer.setup(STEREO, 48000, 240));
        assertTrue(audioReport(), audioReport().contains("route=unknown (attached: speaker+bluetooth-a2dp)"));

        // The next periodic report finds the track routed.
        renderer.routedType = AudioDeviceInfo.TYPE_BLUETOOTH_A2DP;
        // Late beyond the track's buffer too.
        renderer.pendingMs = 1000;
        renderer.playDecodedAudio(new short[6], 6);
        assertTrue(audioReport(), audioReport().contains("route=bluetooth-a2dp"));
        assertTrue(audioReport(), audioReport().contains("lateDropped=1"));
        renderer.cleanup();
    }

    @Test public void aaudioRouteComesFromTheStreamDeviceId() {
        UnavailableOutput renderer = new UnavailableOutput(RuntimeEnvironment.getApplication());
        renderer.attached = new int[][] {{1, AudioDeviceInfo.TYPE_BUILTIN_SPEAKER}, {7, AudioDeviceInfo.TYPE_USB_HEADSET}};
        // With a stream open the track is irrelevant, whatever it reports.
        renderer.routedType = AudioDeviceInfo.TYPE_BLUETOOTH_A2DP;
        ReflectionHelpers.setField(renderer, "nativeAudio", 1L);
        renderer.refreshRoute(7);
        assertEquals("usb-headset", ReflectionHelpers.getField(renderer, "routes"));
        renderer.refreshRoute(0);
        assertEquals("unknown (attached: speaker+usb-headset)", ReflectionHelpers.getField(renderer, "routes"));
        ReflectionHelpers.setField(renderer, "nativeAudio", 0L);
    }

    @Test public void aaudioFailuresThatEndTheBufferAreCountedAsDropped() {
        // Falling back from persistent underruns fails: the buffer is lost.
        ScriptedRenderer renderer = new ScriptedRenderer();
        renderer.fallbackSucceeds = false;
        AudioBufferPolicy policy = new AudioBufferPolicy(192, 384, 384, 0, 0);
        for (int i = 1; i <= 3; i++) policy.sample(i, i * 1000L);
        ReflectionHelpers.setField(renderer, "nativeAudio", 1L);
        ReflectionHelpers.setField(renderer, "bufferPolicy", policy);
        renderer.play();
        assertEquals(Long.valueOf(1), ReflectionHelpers.<Long>getField(renderer, "droppedWrites"));

        // The stream disconnects and the fallback cannot start either.
        renderer = new ScriptedRenderer();
        renderer.fallbackSucceeds = false;
        renderer.nativeWriteSucceeds = false;
        ReflectionHelpers.setField(renderer, "nativeAudio", 1L);
        ReflectionHelpers.setField(renderer, "bufferPolicy", new AudioBufferPolicy(192, 384, 384, 0, 0));
        renderer.play();
        assertEquals("AAudio disconnected", renderer.fallbackReason);
        assertEquals(Long.valueOf(1), ReflectionHelpers.<Long>getField(renderer, "droppedWrites"));
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

    private static final class EncodedRenderer extends AndroidAudioRenderer {
        final List<int[]> tracks = new ArrayList<>();
        final List<Integer> writes = new ArrayList<>();
        int writeResult = Integer.MIN_VALUE;
        boolean refuse;
        long now = 1000;
        int pendingMs;

        EncodedRenderer() {
            super(null, false, null);
        }

        @Override AudioTrack createEncodedTrack(int encoding, int channelMask, int bufferBytes) {
            tracks.add(new int[] {encoding, channelMask, bufferBytes});
            if (refuse) throw new UnsupportedOperationException("Cannot create AudioTrack");
            return null;
        }

        @Override int writeEncoded(byte[] frame, int length) {
            writes.add(length);
            return writeResult == Integer.MIN_VALUE ? length : writeResult;
        }

        @Override long uptimeMillis() {
            return now;
        }

        @Override int pendingAudioMs() {
            return pendingMs;
        }
    }

    private static final com.limelight.nvstream.jni.MoonBridge.AudioConfiguration SURROUND_51 =
            new com.limelight.nvstream.jni.MoonBridge.AudioConfiguration(6, 0x3F);

    @Test public void passthroughOpensACompressedTrackOfTheStreamsCodecAndLayout() {
        EncodedRenderer renderer = new EncodedRenderer();
        assertEquals(0, renderer.setupEncoded(SURROUND_51, AudioPassthrough.CODEC_EAC3));
        assertEquals(1, renderer.tracks.size());
        assertEquals(android.media.AudioFormat.ENCODING_E_AC3, renderer.tracks.get(0)[0]);
        assertEquals(android.media.AudioFormat.CHANNEL_OUT_5POINT1, renderer.tracks.get(0)[1]);
        // Room for two of the largest frames at least, so a blocking write never splits one.
        assertTrue(renderer.tracks.get(0)[2] >= 2 * AudioPassthrough.MAX_FRAME_BYTES);
        renderer.playEncodedAudio(new byte[2560], 2560);
        assertEquals(List.of(2560), renderer.writes);
    }

    @Test public void passthroughRefusesWhatItCannotPlayInsteadOfPlayingSilence() {
        EncodedRenderer renderer = new EncodedRenderer();
        // PCM is not a compressed format, and AC3 has no 4.0 layout here.
        assertEquals(-1, renderer.setupEncoded(SURROUND_51, AudioPassthrough.CODEC_PCM));
        assertEquals(-1, renderer.setupEncoded(
                new com.limelight.nvstream.jni.MoonBridge.AudioConfiguration(4, 0x33), AudioPassthrough.CODEC_AC3));
        assertTrue(renderer.tracks.isEmpty());
        // The output refuses the format.
        renderer.refuse = true;
        assertEquals(-1, renderer.setupEncoded(SURROUND_51, AudioPassthrough.CODEC_AC3));
        assertEquals(1, renderer.tracks.size());
    }

    @Test public void deadPassthroughTrackIsRebuiltAtMostOnceASecondAndFailedFramesAreCounted() {
        EncodedRenderer renderer = new EncodedRenderer();
        assertEquals(0, renderer.setupEncoded(SURROUND_51, AudioPassthrough.CODEC_AC3));
        renderer.writeResult = AudioTrack.ERROR_DEAD_OBJECT;
        renderer.playEncodedAudio(new byte[2560], 2560);
        assertEquals(2, renderer.tracks.size());
        renderer.playEncodedAudio(new byte[2560], 2560);
        assertEquals(2, renderer.tracks.size());
        renderer.now += 1000;
        renderer.playEncodedAudio(new byte[2560], 2560);
        assertEquals(3, renderer.tracks.size());
        // A partial write is a broken frame too.
        renderer.writeResult = 100;
        renderer.playEncodedAudio(new byte[2560], 2560);
        assertEquals(3, renderer.tracks.size());
        assertEquals(Long.valueOf(4), ReflectionHelpers.<Long>getField(renderer, "droppedWrites"));
    }

    @Test public void passthroughFramesKeepTheBoundOnPendingAudioAndStopWithTheStream() {
        EncodedRenderer renderer = new EncodedRenderer();
        assertEquals(0, renderer.setupEncoded(SURROUND_51, AudioPassthrough.CODEC_AC3));
        renderer.pendingMs = 40;
        renderer.playEncodedAudio(new byte[2560], 2560);
        assertTrue(renderer.writes.isEmpty());
        assertEquals(Long.valueOf(1), ReflectionHelpers.<Long>getField(renderer, "lateDropped"));
        renderer.pendingMs = 0;
        renderer.stop();
        renderer.playEncodedAudio(new byte[2560], 2560);
        assertTrue(renderer.writes.isEmpty());
    }
}
