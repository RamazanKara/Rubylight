package com.limelight.binding.audio;

import android.media.AudioTrack;

import org.junit.Test;

import java.util.ArrayList;
import java.util.List;

import static org.junit.Assert.*;

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

    @Test public void zeroAndUnrecoverableErrorsDoNotSpin() {
        for (int result : new int[] {0, AudioTrack.ERROR_BAD_VALUE, AudioTrack.ERROR_INVALID_OPERATION}) {
            ScriptedRenderer renderer = new ScriptedRenderer(result);
            assertFalse(renderer.writeAudioTrackFully(new short[6], 6));
            assertEquals(1, renderer.nextResult);
            assertEquals(0, renderer.recreations);
        }
    }
}
