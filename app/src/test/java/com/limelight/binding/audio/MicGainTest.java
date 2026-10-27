package com.limelight.binding.audio;

import org.junit.Test;

import static org.junit.Assert.*;

public class MicGainTest {
    @Test public void hundredPercentLeavesEverySampleAlone() {
        short[] pcm = {0, 1, -1, 12000, -12000, Short.MAX_VALUE, Short.MIN_VALUE};
        short[] before = pcm.clone();
        MicGain.apply(pcm, pcm.length, 100);
        assertArrayEquals(before, pcm);
    }

    @Test public void quietSamplesScaleLinearly() {
        short[] pcm = {1000, -1000, 4000};
        MicGain.apply(pcm, pcm.length, 200);
        assertArrayEquals(new short[] {2000, -2000, 8000}, pcm);
        MicGain.apply(pcm, pcm.length, 50);
        assertArrayEquals(new short[] {1000, -1000, 4000}, pcm);
    }

    @Test public void fullScaleAtMaximumGainNeverWraps() {
        short[] pcm = {Short.MAX_VALUE, Short.MIN_VALUE, 30000, -30000, 20000};
        MicGain.apply(pcm, pcm.length, MicGain.MAX_PERCENT);
        for (short sample : pcm) {
            assertTrue("stays within full scale: " + sample, Math.abs((int) sample) <= Short.MAX_VALUE);
            assertTrue("keeps its loudness: " + sample, Math.abs((int) sample) > 0.75f * Short.MAX_VALUE);
        }
        assertTrue(pcm[0] > 0);
        assertTrue(pcm[1] < 0);
    }

    @Test public void limiterKeepsOrderSoLouderStaysLouder() {
        int previous = Integer.MIN_VALUE;
        for (int value = -32768; value <= 32767; value += 97) {
            int out = MicGain.sample((short) value, 4f);
            assertTrue("monotonic at " + value, out >= previous);
            previous = out;
        }
    }

    @Test public void limiterMeetsTheLinearPartWithoutAStep() {
        float knee = MicGain.KNEE * 32767f;
        int below = MicGain.sample((short) Math.round(knee / 2 - 2), 2f);
        int above = MicGain.sample((short) Math.round(knee / 2 + 2), 2f);
        assertTrue(Math.abs(above - below) <= 10);
    }

    @Test public void onlyTheGivenSamplesChange() {
        short[] pcm = {1000, 1000, 1000};
        MicGain.apply(pcm, 2, 200);
        assertArrayEquals(new short[] {2000, 2000, 1000}, pcm);
        MicGain.apply(pcm, 99, 50);
        assertArrayEquals(new short[] {1000, 1000, 500}, pcm);
    }

    @Test public void percentIsClampedToTheSliderRange() {
        assertEquals(MicGain.MIN_PERCENT, MicGain.clampPercent(0));
        assertEquals(MicGain.MAX_PERCENT, MicGain.clampPercent(1000));
        assertEquals(150, MicGain.clampPercent(150));
    }
}
