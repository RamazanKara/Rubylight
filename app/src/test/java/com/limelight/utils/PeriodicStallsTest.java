package com.limelight.utils;

import org.junit.Test;

import java.util.Arrays;
import java.util.Random;

import static org.junit.Assert.*;

public class PeriodicStallsTest {
    private static long period(long... times) {
        return PeriodicStalls.periodMs(times, times.length);
    }

    @Test public void exactTwoMinuteRhythmIsFound() {
        assertEquals(120_000, period(10_000, 130_000, 250_000, 370_000, 490_000));
    }

    @Test public void fiveMinuteRhythmWithMultiSecondStallsIsFound() {
        // Each stall spans three one-second samples; only its start counts.
        long[] times = new long[12];
        for (int beat = 0; beat < 4; beat++) {
            for (int s = 0; s < 3; s++) times[beat * 3 + s] = 5_000 + beat * 300_000L + s * 1_000;
        }
        assertEquals(300_000, period(times));
    }

    @Test public void jitteredRhythmIsFoundWithoutDrift() {
        // Intervals of 122, 117, 123, 118, 122 and 119 s: sampling and smoothing blur each start by a second or two.
        long[] times = {0, 122_000, 239_000, 362_000, 480_000, 602_000, 721_000};
        long found = period(times);
        assertEquals(120_000, found, 2_000);
    }

    @Test public void oneMissedBeatAndOneUnrelatedStallStillMatch() {
        // Beat at 0, 120, (240 missed), 360, 480, 600 s plus an unrelated stall at 200 s.
        long found = period(0, 120_000, 200_000, 360_000, 480_000, 600_000);
        assertEquals(120_000, found, 1_000);
    }

    @Test public void tooFewStallsAreNotARhythm() {
        assertEquals(0, period());
        assertEquals(0, period(0, 120_000, 240_000));
        // Four events, but two pairs are one stall each.
        assertEquals(0, period(0, 1_000, 120_000, 121_000));
    }

    @Test public void continuousLossIsOneStallNotARhythm() {
        long[] times = new long[600];
        for (int i = 0; i < times.length; i++) times[i] = i * 1_000L;
        assertEquals(0, period(times));
    }

    @Test public void periodsOutsideTheScanRangeAreIgnored() {
        assertEquals(0, period(0, 10_000, 20_000, 30_000, 40_000, 50_000));
        assertEquals(0, period(0, 900_000, 1_800_000, 2_700_000, 3_600_000));
    }

    @Test public void irregularStallsAreNotARhythm() {
        assertEquals(0, period(0, 45_000, 160_000, 190_000, 400_000, 430_000, 700_000));
    }

    @Test public void randomStallsRarelyLookPeriodic() {
        int falsePositives = 0;
        for (int seed = 0; seed < 500; seed++) {
            Random random = new Random(seed);
            long[] times = new long[6 + random.nextInt(20)];
            for (int i = 0; i < times.length; i++) times[i] = (long) (random.nextDouble() * 20 * 60_000);
            Arrays.sort(times);
            if (period(times) != 0) falsePositives++;
        }
        assertTrue("false positives: " + falsePositives, falsePositives <= 5);
    }

    @Test public void rhythmSurvivesOccasionalRandomStalls() {
        int found = 0;
        for (int seed = 0; seed < 100; seed++) {
            Random random = new Random(seed);
            long[] times = new long[13];
            for (int beat = 0; beat < 10; beat++) times[beat] = 60_000 + beat * 120_000L + random.nextInt(2_000);
            for (int i = 10; i < times.length; i++) times[i] = (long) (random.nextDouble() * 1_260_000);
            Arrays.sort(times);
            long period = period(times);
            if (Math.abs(period - 120_000) <= 2_000) found++;
            else assertEquals("seed " + seed, 0, period);
        }
        assertTrue("found: " + found, found >= 95);
    }

    @Test public void denseRandomStallsAroundARhythmHideIt() {
        // A stall every ~20 s swamps any beat; the detector should not claim a Wi-Fi scan rhythm.
        Random random = new Random(7);
        long[] times = new long[60];
        long t = 0;
        for (int i = 0; i < times.length; i++) times[i] = t += 10_000 + random.nextInt(20_000);
        assertEquals(0, period(times));
    }

    @Test public void onlyTheCountedPrefixIsUsed() {
        long[] times = {0, 120_000, 240_000, 360_000, 480_000, 0, 0};
        assertEquals(120_000, PeriodicStalls.periodMs(times, 5));
        assertEquals(0, PeriodicStalls.periodMs(times, 3));
    }
}
