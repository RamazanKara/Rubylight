package com.limelight.binding.audio;

import android.app.Application;
import android.media.AudioTrack;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

import java.util.ArrayList;
import java.util.List;

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
        private int recreations;
        private long now = 1000;
        private final List<Integer> offsets = new ArrayList<>();
        private final List<Integer> lengths = new ArrayList<>();

        ScriptedRenderer(int... results) {
            super(null, false);
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

        @Override long uptimeMillis() {
            return now;
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
