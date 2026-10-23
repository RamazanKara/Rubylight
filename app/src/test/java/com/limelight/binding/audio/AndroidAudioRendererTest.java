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

        private int pendingMs;

        @Override int pendingAudioMs() {
            return pendingMs;
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
        assertEquals("AAudio keeps underrunning at its maximum reserve", renderer.fallbackReason);
        assertEquals(List.of(0), renderer.offsets);
        assertEquals(List.of(6), renderer.lengths);
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

        @Override int pendingAudioMs() {
            return pendingMs;
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

    @Test public void unroutedOutputListsEveryAttachedOutputUntilTheTrackIsRouted() {
        UnavailableOutput renderer = new UnavailableOutput(RuntimeEnvironment.getApplication());
        renderer.attached = new int[][] {{1, AudioDeviceInfo.TYPE_BUILTIN_SPEAKER}, {7, AudioDeviceInfo.TYPE_BLUETOOTH_A2DP}};
        assertEquals(0, renderer.setup(STEREO, 48000, 240));
        assertTrue(audioReport(), audioReport().contains("route=unknown (attached: speaker+bluetooth-a2dp)"));

        // The next periodic report finds the track routed.
        renderer.routedType = AudioDeviceInfo.TYPE_BLUETOOTH_A2DP;
        renderer.pendingMs = 45;
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
}
