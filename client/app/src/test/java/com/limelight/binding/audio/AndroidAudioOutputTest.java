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

import static org.junit.Assert.*;

/**
 * Unit tests for {@link AndroidAudioRenderer} AudioTrack setup, equalizer session reattachment,
 * surround buffer floors, route diagnostics, and AC3/E-AC3 passthrough playback.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class AndroidAudioOutputTest {
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

    private static String audioReport() {
        return ReflectionHelpers.getStaticField(com.limelight.utils.ProblemReport.class, "audioDiagnostics");
    }

    private static final com.limelight.nvstream.jni.MoonBridge.AudioConfiguration STEREO =
            new com.limelight.nvstream.jni.MoonBridge.AudioConfiguration(2, 0x3);

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
