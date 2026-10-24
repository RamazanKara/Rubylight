package com.limelight.binding.video;

import com.limelight.nvstream.av.video.HostTimingReport;
import com.limelight.nvstream.DecodeTiming;

import org.junit.Test;

import java.io.StringWriter;
import java.io.Writer;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.Future;
import java.util.concurrent.TimeUnit;

import static org.junit.Assert.*;

public class FrameLatencyStatsTest {
    @Test public void highFrameRateAllowsTwoFramesOfPipelineLatencyButStillCountsStalls() {
        FrameLatencyStats stats = new FrameLatencyStats();
        for (int i = 1; i <= 120; i++) {
            timedOutput(stats, i, 1_000_000_000L + i * 8_000_000L,
                    i % 10 == 0 ? 20_000_000L : 12_000_000L, 0, -1);
        }
        DecodeTiming timing = stats.takeDecodeTiming(2_000_000_000L, 120);
        assertEquals(120, timing.samples);
        assertEquals(12, timing.lateFrames);
    }

    private static void timedOutput(FrameLatencyStats stats, int number, long outputNs,
                                    long decodeNs, long queueNs, long displayNs) {
        long inputNs = outputNs - decodeNs;
        stats.onDecoderInput(number, number, inputNs - queueNs, inputNs - queueNs, inputNs, (char) 0);
        stats.onDecoderQueued(number, inputNs);
        stats.onDecoderOutput(0, number, outputNs);
        stats.onOutputReleased(0, outputNs, displayNs >= 0, displayNs >= 0);
        if (displayNs >= 0) stats.onFrameRendered(number, outputNs + displayNs);
    }

    @Test public void decodeTailCountsRecurringSpikesEvenWhenTheMeanIsLowAndFramesAreDropped() {
        FrameLatencyStats stats = new FrameLatencyStats();
        for (int i = 1; i <= 60; i++) {
            timedOutput(stats, i, 1_000_000_000L + i * 16_000_000L,
                    i % 10 == 0 ? 40_000_000L : 2_000_000L, 0, -1);
        }
        DecodeTiming timing = stats.takeDecodeTiming(2_000_000_000L, 60);
        assertEquals(60, timing.samples);
        assertEquals(6, timing.lateFrames);
        assertEquals(5.8f, stats.takeDecodeTimeMs(), 0.001f);
        assertEquals(60, stats.summarize()[1][0], 0);
        // Reading the same ring again cannot count that burst as a second pressure interval.
        assertEquals(0, stats.takeDecodeTiming(2_500_000_000L, 60).samples);
        timedOutput(stats, 61, 2_800_000_000L, 2_000_000L, 0, 0);
        DecodeTiming next = stats.takeDecodeTiming(3_000_000_000L, 60);
        assertEquals(1, next.samples);
        assertEquals(0, next.lateFrames);
        assertEquals(2_500_000_000L, next.windowStartNs);
    }

    @Test public void networkQueueAndDisplayWaitDoNotBecomeLateDecodeSamples() {
        FrameLatencyStats stats = new FrameLatencyStats();
        for (int i = 1; i <= 60; i++) {
            timedOutput(stats, i, 1_000_000_000L + i * 16_000_000L,
                    2_000_000L, 80_000_000L, 100_000_000L);
        }
        DecodeTiming timing = stats.takeDecodeTiming(2_000_000_000L, 60);
        assertEquals(60, timing.samples);
        assertEquals(0, timing.lateFrames);
        assertEquals(80, stats.summarize()[0][1], 0);
        assertEquals(100, stats.summarize()[2][1], 0);
    }

    @Test public void decodeTailBoundsHistoryAndUsesTheExactRequestedFrameBudget() {
        FrameLatencyStats stats = new FrameLatencyStats();
        timedOutput(stats, 1, 1_000_000_000L, 50_000_000L, 0, 0);
        // At 50 FPS the two-frame decode budget is 40 ms; only durations above it are late.
        timedOutput(stats, 2, 4_000_000_000L, 40_000_000L, 0, 0);
        timedOutput(stats, 3, 4_020_000_000L, 40_000_001L, 0, 0);
        DecodeTiming timing = stats.takeDecodeTiming(5_000_000_000L, 50);
        assertEquals(2, timing.samples);
        assertEquals(1, timing.lateFrames);
        for (int i = 0; i < FrameLatencyStats.WINDOW_SIZE + 100; i++) {
            timedOutput(stats, 10 + i, 6_000_000_000L + i * 1_000_000L, 1_000_000L, 0, 0);
        }
        assertEquals(FrameLatencyStats.WINDOW_SIZE, stats.takeDecodeTiming(7_000_000_000L, 60).samples);
    }

    @Test public void decoderRecoveryAndQualityResetExcludeThePreviousWorkloadWithoutClearingDiagnostics() {
        FrameLatencyStats stats = new FrameLatencyStats();
        timedOutput(stats, 1, 1_000_000_000L, 40_000_000L, 0, 0);
        DecodeTiming before = stats.takeDecodeTiming(1_100_000_000L, 60);
        timedOutput(stats, 2, 1_200_000_000L, 40_000_000L, 0, 0);
        stats.discardPending("codec_reset");
        timedOutput(stats, 3, 1_300_000_000L, 2_000_000L, 0, 0);
        DecodeTiming after = stats.takeDecodeTiming(1_400_000_000L, 60);
        assertNotEquals(before.generation, after.generation);
        assertEquals(1, after.samples);
        assertEquals(0, after.lateFrames);
        assertEquals(3, stats.summarize()[1][0], 0);
        stats.resetDecodeTiming();
        assertEquals(0, stats.takeDecodeTiming(1_500_000_000L, 120).samples);
    }

    @Test public void invalidAndFutureDecodeTimestampsCannotCountAsPressure() {
        FrameLatencyStats stats = new FrameLatencyStats();
        stats.onDecoderInput(1, 1, 1, 1, -1, (char) 0);
        stats.onDecoderOutput(0, 1, 10);
        stats.onOutputReleased(0, 10, false, false);
        stats.onDecoderInput(2, 2, 10, 10, 20, (char) 0);
        stats.onDecoderOutput(0, 2, 19);
        stats.onOutputReleased(0, 19, false, false);
        timedOutput(stats, 3, 5_000_000_000L, 40_000_000L, 0, 0);
        assertEquals(0, stats.takeDecodeTiming(4_000_000_000L, 60).samples);
    }

    @Test public void outputQueueAgeIsAvailableOnlyWhileTheTrackedBufferIsOwned() {
        FrameLatencyStats stats = new FrameLatencyStats();
        stats.onDecoderInput(1, 1, 100, 150, 200, (char) 0);
        stats.onDecoderOutput(7, 1, 300);
        assertEquals(150, stats.getEnqueueTimeNsForOutput(7));
        assertEquals(0, stats.getEnqueueTimeNsForOutput(8));
        stats.onOutputReleased(7, 400, false, false);
        assertEquals(0, stats.getEnqueueTimeNsForOutput(7));
        stats.onDecoderInput(2, 2, 100, 350, 200, (char) 0);
        stats.onDecoderOutput(7, 2, 300);
        assertEquals(0, stats.getEnqueueTimeNsForOutput(7));
        stats.discardPending("codec_reset");
        assertEquals(0, stats.getEnqueueTimeNsForOutput(7));
    }

    @Test
    public void pyroWaveQueuePreservesPreciseTimingsWithoutMediaCodecCounter() throws Exception {
        FrameLatencyStats stats = new FrameLatencyStats();
        stats.onDecoderInput(7, 123, 1000000, 1000001, 2000000, (char) 0);
        stats.onPyroWaveDecoderQueued(123, 2100011);
        stats.onDecoderOutput(0, 123, 7000033);
        stats.onArtemisDecodeCounted(123);
        assertEquals(0, stats.getArtemisDecodeMs());
        assertArrayEquals(new double[] {1, 1.100010, 1.100010, 1.100010}, stats.summarize()[0], 0.0000001);
        assertArrayEquals(new double[] {1, 4.900022, 4.900022, 4.900022}, stats.summarize()[1], 0.0000001);
        stats.onOutputReleased(0, 8000044, false, false);
        StringWriter csv = new StringWriter();
        stats.writeCsv(csv);
        String[] fields = csv.toString().trim().split(",", -1);
        assertEquals("4.900022", fields[9]);
        assertEquals("1.100010", fields[25]);
    }

    @Test
    public void pyroWaveQueueDoesNotFillOrEvictTheMediaCodecCounter() {
        FrameLatencyStats stats = new FrameLatencyStats();
        stats.onDecoderQueued(1, 1000000);
        for (long pts = 2; pts < 3000; pts++) {
            stats.onPyroWaveDecoderQueued(pts, 2000000);
        }
        // PyroWave entries must not consume the bounded legacy map, even without pending frames.
        stats.onDecoderOutput(0, 2999, 9000000);
        stats.onArtemisDecodeCounted(2999);
        assertEquals(0, stats.getArtemisDecodeMs());
        stats.onDecoderOutput(1, 1, 5900000);
        stats.onArtemisDecodeCounted(1);
        assertEquals(4, stats.getArtemisDecodeMs());
    }

    @Test
    public void pyroWaveCompletionBeforeQueueKeepsItsOriginalStart() {
        FrameLatencyStats stats = new FrameLatencyStats();
        stats.onDecoderInput(1, 123, 1000000, 1500000, 2000000, (char) 0);
        stats.onDecoderOutput(0, 123, 3000000);
        stats.onPyroWaveDecoderQueued(123, 4000000);
        assertArrayEquals(new double[] {1, 0.5, 0.5, 0.5}, stats.summarize()[0], 0);
        assertArrayEquals(new double[] {1, 1, 1, 1}, stats.summarize()[1], 0);
    }

    @Test
    public void recentSummaryExpiresEachStageEvenWhenOtherStagesKeepReceivingFrames() {
        FrameLatencyStats stats = new FrameLatencyStats();
        stats.onDecoderInput(1, 1, 1000000000, 1000000000, 1001000000, (char) 300);
        stats.onDecoderQueued(1, 1001000000);
        stats.onDecoderOutput(0, 1, 1003000000);
        stats.onOutputReleased(0, 1004000000, true, true);
        stats.onFrameRendered(1, 1040000000);
        // Fresh frames have decode measurements, but no host timing or render callback.
        stats.onDecoderInput(2, 2, 5000000000L, 5000000000L, 5001000000L, (char) 0);
        stats.onDecoderQueued(2, 5001000000L);
        stats.onDecoderOutput(0, 2, 5004000000L);
        stats.onOutputReleased(0, 5005000000L, true, false);

        double[][] recent = stats.summarizeRecent(6000000000L, 2000000000L);
        assertArrayEquals(new double[] {1, 3, 3, 3}, recent[1], 0);
        assertEquals(0, recent[2][0], 0);
        assertEquals(0, recent[4][0], 0);
        // The existing last-N-frame overlay remains independent of diagnostic freshness.
        assertEquals(1, stats.summarize()[2][0], 0);
        assertEquals(1, stats.summarize()[4][0], 0);
    }

    @Test
    public void recentSummaryHonorsTimeBoundariesAndBoundsTheSampleRing() {
        FrameLatencyStats stats = new FrameLatencyStats();
        stats.onDecoderInput(1, 1, 799, 799, 799, (char) 100);
        stats.onDecoderInput(2, 2, 800, 800, 800, (char) 20);
        stats.onDecoderInput(3, 3, 1000, 1000, 1000, (char) 40);
        stats.onDecoderInput(4, 4, 1001, 1001, 1001, (char) 100);
        assertArrayEquals(new double[] {2, 3, 4, 4}, stats.summarizeRecent(1000, 200)[4], 0);

        for (int i = 0; i < FrameLatencyStats.WINDOW_SIZE + 10; i++) {
            stats.onDecoderInput(i + 10, i + 10, 1100, 1100, 1100, (char) 10);
        }
        assertArrayEquals(new double[] {600, 1, 1, 1}, stats.summarizeRecent(1100, 200)[4], 0);
        assertEquals(0, stats.summarizeRecent(1301, 200)[4][0], 0);
    }

    @Test
    public void surfaceRecreationDoesNotCountCancelledPresentationsAsDrops() throws Exception {
        FrameLatencyStats stats = new FrameLatencyStats();
        stats.onDecoderInput(1, 123, 1000000, 1500000, 2000000, (char) 0);
        stats.onDecoderOutput(0, 123, 3000000);
        stats.onOutputReleased(0, 4000000, true, true);
        stats.discard(123, "surface_changed");
        stats.expire(10000000000L);
        assertEquals(0, stats.getPresentDrops());
        StringWriter csv = new StringWriter();
        stats.writeCsv(csv);
        assertTrue(csv.toString().contains(",surface_changed,"));
    }

    @Test
    public void csvDurationsKeepNanosecondPrecisionWithoutLocaleFormatting() throws Exception {
        java.util.Locale previous = java.util.Locale.getDefault();
        try {
            java.util.Locale.setDefault(java.util.Locale.GERMANY);
            FrameLatencyStats stats = new FrameLatencyStats();
            stats.onDecoderInput(1, 123, 1000000, 1000001, 1000011, (char) 1);
            stats.onDecoderOutput(0, 123, 1234567890);
            stats.onOutputReleased(0, 1234567891, false, false);
            StringWriter csv = new StringWriter();
            stats.writeCsv(csv);
            String[] fields = csv.toString().trim().split(",", -1);
            assertEquals("0.000011", fields[8]);
            assertEquals("1233.567879", fields[9]);
            assertEquals("0.100000", fields[13]);
            assertEquals("0.000010", fields[25]);
        } finally {
            java.util.Locale.setDefault(previous);
        }
    }

    @Test
    public void supportedThousandFpsStreamKeepsFramesUntilTheCallbackTimeout() {
        FrameLatencyStats stats = new FrameLatencyStats(true);
        for (int i = 1; i <= 8000; i++) {
            long now = 1000000000L + i * 1000000L;
            stats.onFrameReceived(now);
            stats.onDecoderInput(i, i, now, now, now, (char) 0);
            stats.onDecoderQueued(i, now);
            stats.onDecoderOutput(0, i, now);
            stats.onOutputReleased(0, now, true, true);
            if (i % 1000 == 0) stats.expire(now);
        }
        stats.expire(14000000000L);
        assertEquals(8000, stats.getPresentDrops());
    }

    @Test
    public void maximumStreamRateRetainsFramesUntilCallbackTimeout() {
        FrameLatencyStats stats = new FrameLatencyStats();
        for (int i = 0; i < 5000; i++) {
            long timeNs = 1000000000L + i * 1000000L;
            stats.onDecoderInput(i, i, timeNs, timeNs, timeNs, (char) 0);
            stats.onDecoderQueued(i, timeNs);
            stats.onDecoderOutput(0, i, timeNs);
            stats.onOutputReleased(0, timeNs, true, true);
        }
        stats.expire(11000000000L);
        assertEquals(5000, stats.getPresentDrops());
    }

    @Test
    public void timingSplitExcludesPacketAssemblyAndKeepsFractionalMilliseconds() throws Exception {
        FrameLatencyStats stats = new FrameLatencyStats();
        stats.onDecoderInput(1, 123, 1000000, 15000000, 20000000, (char) 0);
        stats.onDecoderQueued(123, 20000000);
        stats.onDecoderOutput(0, 123, 22750000);
        stats.onOutputReleased(0, 23000000, true, true);
        stats.onFrameRendered(123, 27250000);

        double[][] summary = stats.summarize();
        assertArrayEquals(new double[] {1, 5, 5, 5}, summary[0], 0);
        assertArrayEquals(new double[] {1, 2.75, 2.75, 2.75}, summary[1], 0);
        assertArrayEquals(new double[] {1, 4.5, 4.5, 4.5}, summary[2], 0);
        assertEquals(26.25, summary[3][1], 0);
        assertEquals(2.75, stats.takeDecodeTimeMs(), 0);

        StringWriter csv = new StringWriter();
        stats.writeCsv(csv);
        String[] fields = csv.toString().trim().split(",", -1);
        assertEquals(FrameLatencyStats.CSV_HEADER.trim().split(",").length, fields.length);
        assertEquals("19.000000", fields[8]);
        assertEquals("2.750000", fields[9]);
        assertEquals("4.500000", fields[10]);
        assertEquals("15000000", fields[24]);
        assertEquals("5.000000", fields[25]);
    }

    @Test
    public void drainedBatchCountsEveryOutputButOnlyObservedPresentations() {
        FrameLatencyStats stats = new FrameLatencyStats();
        for (int i = 1; i <= 4; i++) {
            stats.onFrameReceived(1000000);
            stats.onDecoderInput(i, i, 1000000, 2000000, 3000000, (char) 0);
            stats.onDecoderQueued(i, 3000000);
        }
        stats.onDecoderOutput(0, 1, 4000000);
        stats.onOutputReleased(0, 5000000, false, true);
        stats.onDecoderOutput(0, 2, 7000000);
        stats.onOutputReleased(0, 8000000, false, true);
        stats.onDecoderOutput(0, 3, 9000000);
        stats.onOutputReleased(0, 10000000, true, true);
        stats.onFrameRendered(3, 13000000);
        stats.discard(4, "output_unobserved");

        double[][] summary = stats.summarize();
        assertArrayEquals(new double[] {4, 1, 1, 1}, summary[0], 0);
        assertArrayEquals(new double[] {3, 11.0 / 3, 6, 6}, summary[1], 0.00001);
        assertArrayEquals(new double[] {1, 4, 4, 4}, summary[2], 0);
        assertEquals(11.0 / 3, stats.takeDecodeTimeMs(), 0.00001);
        assertEquals(-1, stats.takeDecodeTimeMs(), 0);
        assertEquals(3, stats.getAverageDecoderLatency());
        assertEquals(5, stats.getAverageEndToEndLatency());

        stats.discardPending("codec_reset");
        assertEquals(3, stats.getAverageDecoderLatency());
        stats.onDecoderInput(5, 5, 20000000, 21000000, 22000000, (char) 0);
        stats.onDecoderQueued(5, 22000000);
        stats.onDecoderOutput(0, 5, 31000000);
        assertEquals(9, stats.takeDecodeTimeMs(), 0);
        assertEquals(5, stats.getAverageDecoderLatency());
        assertEquals(7, stats.getAverageEndToEndLatency());
    }

    @Test
    public void artemisDecodeTimeRoundsEachCountedFrameDownToWholeMilliseconds() {
        FrameLatencyStats stats = new FrameLatencyStats();
        assertEquals(0, stats.takeArtemisDecodeMs());
        // 4.9 ms counts as 4, 2.999 ms as 2 and 0.6 ms as 0, as in Artemis.
        long[][] frames = {{1, 10000000, 14900000}, {2, 20000000, 22999000}, {3, 30000000, 30600000}};
        for (long[] frame : frames) {
            stats.onDecoderQueued(frame[0], frame[1]);
            stats.onDecoderOutput((int) frame[0], frame[0], frame[2]);
            stats.onArtemisDecodeCounted(frame[0]);
        }
        assertEquals(6, stats.getArtemisDecodeMs());
        assertEquals(6, stats.takeArtemisDecodeMs());
        assertEquals(0, stats.getArtemisDecodeMs());
        assertEquals(0, stats.takeArtemisDecodeMs());
    }

    @Test
    public void artemisDecodeTimeSkipsUncountedImpossibleAndSecondLongFrames() {
        FrameLatencyStats stats = new FrameLatencyStats();
        // Replaced before display: queued but never counted.
        stats.onDecoderQueued(1, 1000000);
        stats.onDecoderOutput(0, 1, 4000000);
        // Counted twice: only the first count has a start time.
        stats.onDecoderQueued(2, 2000000);
        stats.onDecoderOutput(0, 2, 5000000);
        stats.onArtemisDecodeCounted(2);
        stats.onArtemisDecodeCounted(2);
        // Never queued, never returned by the decoder, a second or longer, and a clock going backwards.
        stats.onDecoderOutput(0, 3, 9000000);
        stats.onArtemisDecodeCounted(3);
        stats.onDecoderQueued(9, 3000000);
        stats.onArtemisDecodeCounted(9);
        stats.onDecoderQueued(4, 10000000);
        stats.onDecoderOutput(0, 4, 1010000000);
        stats.onArtemisDecodeCounted(4);
        stats.onDecoderQueued(5, 20000000);
        stats.onDecoderOutput(0, 5, 18000000);
        stats.onArtemisDecodeCounted(5);
        stats.onDecoderQueued(8, 30000000);
        stats.onDecoderOutput(0, 8, 29999999);
        stats.onArtemisDecodeCounted(8);
        assertEquals(3, stats.takeArtemisDecodeMs());
        // A codec reset forgets queued frames, not the time already counted.
        stats.onDecoderQueued(6, 30000000);
        stats.onDecoderOutput(0, 6, 37000000);
        stats.onArtemisDecodeCounted(6);
        stats.onDecoderQueued(7, 40000000);
        stats.onDecoderOutput(0, 7, 45000000);
        stats.discardPending("codec_reset");
        stats.onArtemisDecodeCounted(7);
        assertEquals(7, stats.takeArtemisDecodeMs());
    }

    @Test
    public void artemisDecodeTimeStopsWhenTheDecoderReturnsTheFrameNotWhenItIsReleased() {
        FrameLatencyStats stats = new FrameLatencyStats();
        long t0 = 10000000;
        stats.onDecoderInput(1, 1, t0 - 2000000, t0 - 1000000, t0 - 500000, (char) 0);
        stats.onDecoderQueued(1, t0);
        stats.onDecoderOutput(0, 1, t0 + 2400000);
        // Released and counted 5.9 ms after queueing: the 3.5 ms in between is pacing, not decode.
        stats.onOutputReleased(0, t0 + 5900000, true, false);
        stats.onArtemisDecodeCounted(1);
        assertEquals(2, stats.takeArtemisDecodeMs());
    }

    @Test
    public void artemisDecodeTimeUsesTheCountedCatchUpFramesOwnDequeueTime() {
        FrameLatencyStats stats = new FrameLatencyStats();
        long t0 = 10000000;
        stats.onDecoderQueued(1, t0);
        stats.onDecoderQueued(2, t0 + 1000000);
        stats.onDecoderOutput(0, 1, t0 + 2400000);
        stats.onDecoderOutput(1, 2, t0 + 7100000);
        stats.onOutputReleased(0, t0 + 7200000, false, false);
        // Frame 1 was dropped by the catch-up drain; frame 2 is the one counted, 6.1 ms after
        // its own queueing, whatever time the release and bookkeeping then took.
        stats.onOutputReleased(1, t0 + 12000000, true, false);
        stats.onArtemisDecodeCounted(2);
        assertEquals(6, stats.takeArtemisDecodeMs());
    }

    @Test
    public void artemisDecodeTimeKeepsAnOutputThatBeatsTheQueueBookkeeping() {
        FrameLatencyStats stats = new FrameLatencyStats();
        long t0 = 10000000;
        stats.onDecoderInput(1, 1, t0 - 2000000, t0 - 1000000, t0 - 500000, (char) 0);
        stats.onDecoderOutput(0, 1, t0 + 3300000);
        stats.onDecoderQueued(1, t0);
        stats.onArtemisDecodeCounted(1);
        assertEquals(3, stats.takeArtemisDecodeMs());
    }

    @Test
    public void artemisDecodeTimeLeavesThePreciseDecodeTimeAlone() {
        FrameLatencyStats stats = new FrameLatencyStats();
        stats.onDecoderInput(1, 1, 1000000, 1000000, 2000000, (char) 0);
        stats.onDecoderQueued(1, 2000000);
        stats.onDecoderOutput(0, 1, 6900000);
        stats.onArtemisDecodeCounted(1);
        assertEquals(4.9, stats.takeDecodeTimeMs(), 0.00001);
        assertEquals(4, stats.takeArtemisDecodeMs());
    }

    @Test
    public void renderedFpsCountsSubmittedFramesWithoutRenderCallbacks() {
        FrameLatencyStats stats = new FrameLatencyStats();
        long startNs = 1000000000L;
        for (int i = 1; i <= 5; i++) {
            long base = startNs + i * 100000000L;
            stats.onFrameReceived(base);
            stats.onDecoderInput(i, i, base, base, base + 1000000, (char) 0);
            stats.onDecoderQueued(i, base + 1000000);
            stats.onDecoderOutput(i, i, base + 3000000);
        }
        // Frames 1 to 3 are released for rendering, 4 and 5 are dropped; no render callback arrives.
        for (int i = 1; i <= 3; i++) {
            stats.onOutputReleased(i, startNs + i * 100000000L + 4000000, true, false);
        }
        stats.onOutputReleased(4, startNs + 400000000L + 4000000, false, false);
        stats.onOutputReleased(5, startNs + 500000000L + 4000000, false, false);
        float[] rates = stats.getFrameRates(startNs + 1100000000L);
        // The window starts at the first received frame, which its own start excludes from the
        // received count; the release stamps all fall inside it.
        assertEquals(4, rates[0], 0.001f);
        assertEquals(3, rates[1], 0.001f);
        // Compositor-confirmed rate stays unavailable without callbacks.
        assertEquals(-1, rates[2], 0);
    }

    @Test
    public void absentAndInvalidQueueTimestampsDoNotInventQueueWait() throws Exception {
        FrameLatencyStats stats = new FrameLatencyStats();
        for (long enqueueNs : new long[] {0, 4000000}) {
            stats.onDecoderInput(1, 1, 1000000, enqueueNs, 3000000, (char) 0);
            stats.onDecoderQueued(1, 3000000);
            stats.onDecoderOutput(0, 1, 4000000);
            stats.onOutputReleased(0, 5000000, false, true);
        }
        assertEquals(0, stats.summarize()[0][0], 0);
        assertEquals(2, stats.summarize()[1][0], 0);
        StringWriter csv = new StringWriter();
        stats.writeCsv(csv);
        for (String row : csv.toString().split("\n")) {
            assertEquals("", row.split(",", -1)[25]);
        }
    }

    @Test
    public void bitrateDecodeSamplesRequireCompletedValidFramesAndResetEachWindow() {
        FrameLatencyStats stats = new FrameLatencyStats();
        assertEquals(-1, stats.takeDecodeTimeMs(), 0);
        stats.onDecoderInput(1, 1000, 1000000, 1000000, 2000000, (char) 0);
        stats.onDecoderQueued(1000, 2000000);
        assertEquals(-1, stats.takeDecodeTimeMs(), 0);
        stats.onDecoderOutput(1, 1000, 6000000);
        stats.onDecoderInput(2, 2000, 7000000, 7000000, 8000000, (char) 0);
        stats.onDecoderQueued(2000, 8000000);
        stats.onDecoderOutput(2, 2000, 14000000);
        assertEquals(5, stats.takeDecodeTimeMs(), 0);
        assertEquals(-1, stats.takeDecodeTimeMs(), 0);
        stats.onDecoderInput(3, 3000, 15000000, 15000000, 16000000, (char) 0);
        stats.onDecoderQueued(3000, 16000000);
        stats.onDecoderOutput(3, 3000, 14000000);
        assertEquals(-1, stats.takeDecodeTimeMs(), 0);
        stats.onDecoderInput(4, 4000, 20000000, 20000000, 21000000, (char) 0);
        stats.onDecoderQueued(4000, 21000000);
        stats.onDecoderOutput(4, 4000, 22000000);
        stats.discardPending("stopped");
        assertEquals(-1, stats.takeDecodeTimeMs(), 0);
    }

    @Test
    public void reusedOutputIndexKeepsBothPendingRenderCallbacks() throws Exception {
        FrameLatencyStats stats = new FrameLatencyStats();
        stats.onDecoderInput(1, 10, 100, 100, 200, (char) 0);
        stats.onDecoderQueued(10, 200);
        stats.onDecoderOutput(0, 10, 300);
        stats.onOutputReleased(0, 400, true, true);
        stats.onDecoderInput(2, 20, 500, 500, 600, (char) 0);
        stats.onDecoderQueued(20, 600);
        stats.onDecoderOutput(0, 20, 700);
        stats.onFrameRendered(10, 450);
        stats.onOutputReleased(0, 800, true, true);
        stats.onFrameRendered(20, 900);

        StringWriter csv = new StringWriter();
        stats.writeCsv(csv);
        assertTrue(csv.toString().contains("1,10,100,200,300,450,400,rendered,"));
        assertTrue(csv.toString().contains("2,20,500,600,700,900,800,rendered,"));
        assertEquals(2, stats.summarize()[3][0], 0);
    }

    @Test
    public void lateCallbacksAfterCodecResetCannotCompleteNewBuffers() throws Exception {
        FrameLatencyStats stats = new FrameLatencyStats();
        stats.onDecoderInput(1, 10, 100, 100, 200, (char) 0);
        stats.onDecoderQueued(10, 200);
        stats.onDecoderOutput(0, 10, 300);
        stats.discardPending("codec_reset");
        stats.onDecoderInput(2, 20, 500, 500, 600, (char) 0);
        stats.onDecoderQueued(20, 600);
        stats.onDecoderOutput(0, 20, 700);
        stats.onFrameRendered(10, 800);
        stats.onOutputReleased(0, 900, false, true);
        stats.onFrameRendered(20, 1000);

        StringWriter csv = new StringWriter();
        stats.writeCsv(csv);
        assertTrue(csv.toString().contains("1,10,100,200,300,,,codec_reset,"));
        assertTrue(csv.toString().contains("2,20,500,600,700,,900,dropped,"));
        assertEquals(2, csv.toString().split("\n").length);
        assertEquals(0, stats.summarize()[3][0], 0);
    }

    @Test
    public void absentCallbacksExpireWithoutInventingRenderMeasurements() throws Exception {
        FrameLatencyStats stats = new FrameLatencyStats();
        stats.onDecoderInput(1, 10, 100, 100, 200, (char) 0);
        stats.onDecoderQueued(10, 200);
        stats.onDecoderOutput(0, 10, 300);
        stats.onOutputReleased(0, 400, true, true);
        stats.onDecoderInput(2, 20, 100, 100, 200, (char) 0);
        stats.onDecoderQueued(20, 200);
        stats.expire(5000000400L);
        stats.onFrameRendered(10, 5000000300L);

        StringWriter csv = new StringWriter();
        stats.writeCsv(csv);
        assertTrue(csv.toString().contains("render_unobserved"));
        assertTrue(csv.toString().contains("output_unobserved"));
        assertEquals(2, csv.toString().split("\n").length);
        assertEquals(0, stats.summarize()[3][0], 0);
    }

    @Test
    public void invalidClockOrderIsExcludedFromPercentiles() throws Exception {
        FrameLatencyStats stats = new FrameLatencyStats();
        stats.onDecoderInput(1, 10, 300, 300, 200, (char) 0);
        stats.onDecoderQueued(10, 200);
        stats.onDecoderOutput(0, 10, 100);
        stats.onOutputReleased(0, 400, true, true);
        stats.onFrameRendered(10, 500);
        for (int stage = 0; stage < 4; stage++) {
            assertEquals(0, stats.summarize()[stage][0], 0);
        }
        StringWriter csv = new StringWriter();
        stats.writeCsv(csv);
        assertTrue(csv.toString().contains("invalid_render_time"));
    }

    @Test
    public void stalledDecoderAndCsvWriterKeepTrackingBounded() throws Exception {
        FrameLatencyStats stats = new FrameLatencyStats();
        for (int i = 0; i < 15000; i++) {
            stats.onDecoderInput(i, i, 100, 100, 200, (char) 0);
            stats.onDecoderQueued(i, 200);
        }
        StringWriter overflowCsv = new StringWriter();
        stats.writeCsv(overflowCsv);
        assertEquals(4096, overflowCsv.toString().split("\n").length);
        assertTrue(overflowCsv.toString().contains("tracking_overflow"));
        stats.discardPending("stream_ended");
        assertEquals(15000 - 2 * 4096, stats.getCsvRowsLost());
        StringWriter csv = new StringWriter();
        stats.writeCsv(csv);
        assertEquals(4096, csv.toString().split("\n").length);
        assertTrue(csv.toString().contains("stream_ended"));
    }

    @Test
    public void blockedCsvIoDoesNotBlockDecoderCallbacks() throws Exception {
        FrameLatencyStats stats = new FrameLatencyStats();
        stats.onDecoderInput(1, 10, 100, 100, 200, (char) 0);
        stats.onDecoderQueued(10, 200);
        stats.discardPending("stream_ended");
        CountDownLatch writing = new CountDownLatch(1);
        CountDownLatch resume = new CountDownLatch(1);
        ExecutorService executor = Executors.newFixedThreadPool(2);
        try {
            Future<?> export = executor.submit(() -> {
                stats.writeCsv(new Writer() {
                    @Override
                    public void write(char[] data, int offset, int length) {
                        writing.countDown();
                        try {
                            assertTrue(resume.await(5, TimeUnit.SECONDS));
                        } catch (InterruptedException e) {
                            throw new AssertionError(e);
                        }
                    }

                    @Override
                    public void flush() { }

                    @Override
                    public void close() { }
                });
                return null;
            });
            assertTrue(writing.await(5, TimeUnit.SECONDS));
            executor.submit(() -> {
                stats.onDecoderInput(2, 20, 500, 500, 600, (char) 0);
                stats.onDecoderQueued(20, 600);
                stats.onDecoderOutput(0, 20, 700);
                stats.onOutputReleased(0, 800, true, true);
                stats.onFrameRendered(20, 900);
            }).get(5, TimeUnit.SECONDS);
            resume.countDown();
            export.get(5, TimeUnit.SECONDS);
            assertEquals(1, stats.summarize()[3][0], 0);
        } finally {
            resume.countDown();
            executor.shutdownNow();
        }
    }

    @Test
    public void completedGpuFenceDoesNotInventDisplayTiming() throws Exception {
        FrameLatencyStats stats = new FrameLatencyStats();
        stats.onDecoderInput(9, 9, 1000000, 1000000, 2000000, (char) 0);
        stats.onDecoderQueued(9, 2000000);
        stats.onDecoderOutput(0, 9, 3500000);
        stats.onOutputReleased(0, 6000000, true, false);
        assertArrayEquals(new double[] {1, 1.5, 1.5, 1.5}, stats.summarize()[1], 0.00001);
        assertEquals(0, stats.summarize()[2][0], 0);
        assertEquals(0, stats.summarize()[3][0], 0);
        StringWriter csv = new StringWriter();
        stats.writeCsv(csv);
        assertTrue(csv.toString().contains("render_unavailable"));
    }

    @Test
    public void hostDurationIsIndependentOfClientClockAndSurvivesCsvExport() throws Exception {
        FrameLatencyStats stats = new FrameLatencyStats();
        stats.onDecoderInput(42, 123, 1000000, 1000000, 2000000, (char) 123);
        stats.onDecoderQueued(123, 2000000);
        stats.onDecoderOutput(0, 123, 5000000);
        stats.onOutputReleased(0, 6000000, true, true);
        stats.onFrameRendered(123, 7000000);
        assertEquals(6.0, stats.summarize()[3][1], 0.00001);
        assertArrayEquals(new double[] {1, 12.3, 12.3, 12.3}, stats.summarize()[4], 0.00001);
        StringWriter csv = new StringWriter();
        stats.writeCsv(csv);
        assertEquals("12.300000", csv.toString().trim().split(",", -1)[13]);
        assertEquals(FrameLatencyStats.CSV_HEADER.trim().split(",").length, csv.toString().trim().split(",", -1).length);
    }

    private static HostTimingReport report(long[]... records) {
        byte[] data = new byte[4 + 20 * records.length];
        data[0] = 1;
        data[1] = (byte) records.length;
        for (int i = 0; i < records.length; i++) {
            for (int field = 0; field < 5; field++) {
                for (int b = 0; b < 4; b++) data[4 + 20 * i + 4 * field + b] = (byte) (records[i][field] >> (8 * b));
            }
        }
        return HostTimingReport.parse(data);
    }

    private static void renderFrame(FrameLatencyStats stats, int number, long ptsUs, char headerLatency) {
        stats.onDecoderInput(number, ptsUs, 1000000, 1000000, 2000000, headerLatency);
        stats.onDecoderQueued(ptsUs, 2000000);
        stats.onDecoderOutput(0, ptsUs, 5000000);
        stats.onOutputReleased(0, 6000000, true, true);
        stats.onFrameRendered(ptsUs, 7000000);
    }

    @Test
    public void hostTimingReportsFillTheirOwnStagesAndTheCsvColumns() throws Exception {
        FrameLatencyStats stats = new FrameLatencyStats();
        // The report can arrive before or after its frame; the header says 5 ms (50 x 100 us).
        stats.onHostTiming(report(new long[] {42, 12000, 3000, 5000, 2500}), 1000000000L);
        renderFrame(stats, 42, 123, (char) 50);
        stats.onDecoderInput(43, 124, 1000000, 1000000, 2000000, (char) 50);
        stats.onHostTiming(report(new long[] {43, 8000, 1000, 2000, 3000}), 1000000100L);

        double[][] summary = stats.summarize();
        // Host processing (stage 4) is the two header values, 5 ms each; the reports' present to
        // last packet sent is stage 8.
        assertArrayEquals(new double[] {2, 5, 5, 5}, summary[4], 0);
        assertArrayEquals(new double[] {2, 10, 12, 12}, summary[FrameLatencyStats.HOST_SENT_STAGE], 0);
        assertEquals(2, summary[5][1], 0);
        assertEquals(3.5, summary[6][1], 0);
        assertEquals(2.75, summary[7][1], 0);

        StringWriter csv = new StringWriter();
        stats.writeCsv(csv);
        String[] rows = csv.toString().trim().split("\n");
        assertEquals(1, rows.length);
        String[] fields = rows[0].split(",", -1);
        assertEquals(FrameLatencyStats.CSV_HEADER.trim().split(",").length, fields.length);
        assertEquals("5.000000", fields[13]);
        assertEquals("12.000000", fields[26]);
        assertEquals("3.000000", fields[27]);
        assertEquals("5.000000", fields[28]);
        assertEquals("2.500000", fields[29]);

        // A frame finished before its report arrived is still matched.
        stats.onDecoderOutput(1, 124, 5000000);
        stats.onOutputReleased(1, 6000000, true, true);
        stats.onFrameRendered(124, 7000000);
        csv = new StringWriter();
        stats.writeCsv(csv);
        fields = csv.toString().trim().split(",", -1);
        assertEquals("43", fields[0]);
        assertEquals("8.000000", fields[26]);
        assertEquals("3.000000", fields[29]);
    }

    @Test
    public void hostProcessingStaysTheHeaderValueWhateverTheReportsSay() {
        // Regression: host timing reports (present to last packet sent, with the network send) once
        // replaced the header's host processing in the overlay, so Nightjar showed 4.9 ms where
        // Artemis showed the header's 0.5 ms.
        FrameLatencyStats stats = new FrameLatencyStats();
        stats.onDecoderInput(1, 1, 1000000, 1000000, 2000000, (char) 5);
        stats.onHostTiming(report(new long[] {1, 5000, 100, 600, 4200}), 3000000000L);
        // Reports are fresh, and the header keeps counting.
        stats.onDecoderInput(2, 2, 3100000000L, 3100000000L, 3100000000L, (char) 5);
        double[][] summary = stats.summarize();
        assertArrayEquals(new double[] {2, 0.5, 0.5, 0.5}, summary[4], 0.00001);
        assertArrayEquals(new double[] {1, 5, 5, 5}, summary[FrameLatencyStats.HOST_SENT_STAGE], 0);
        // Long after the last report the header's average is still the same stage.
        stats.onDecoderInput(3, 3, 9000000000L, 9000000000L, 9000000000L, (char) 5);
        assertArrayEquals(new double[] {3, 0.5, 0.5, 0.5}, stats.summarize()[4], 0.00001);
    }

    @Test
    public void reportsAloneLeaveHostProcessingEmpty() {
        FrameLatencyStats stats = new FrameLatencyStats();
        stats.onHostTiming(report(new long[] {1, 5000, 100, 600, 4200}), 3000000000L);
        assertEquals(0, stats.summarize()[4][0], 0);
        assertEquals(1, stats.summarize()[FrameLatencyStats.HOST_SENT_STAGE][0], 0);
    }

    @Test
    public void unmatchedReportsStayBoundedAndHeaderOnlyStreamsHaveBlankSplitColumns() throws Exception {
        FrameLatencyStats stats = new FrameLatencyStats();
        for (int i = 0; i < 1000; i++) {
            stats.onHostTiming(report(new long[] {i, 10, 1, 1, 1}), 1000L + i);
        }
        // The oldest were dropped, so frame 0 gets nothing but frame 999 does.
        renderFrame(stats, 0, 1, (char) 0);
        StringWriter csv = new StringWriter();
        stats.writeCsv(csv);
        String[] fields = csv.toString().trim().split(",", -1);
        assertEquals("", fields[26]);
        assertEquals("", fields[29]);
        assertEquals(FrameLatencyStats.CSV_HEADER.trim().split(",").length, fields.length);

        renderFrame(stats, 999, 2, (char) 0);
        csv = new StringWriter();
        stats.writeCsv(csv);
        assertEquals("0.010000", csv.toString().trim().split(",", -1)[26]);
    }

    @Test
    public void missingHostTimingIsBlankNotZeroLatency() throws Exception {
        FrameLatencyStats stats = new FrameLatencyStats();
        stats.onDecoderInput(1, 1, 100, 100, 200, (char) 0);
        stats.onDecoderQueued(1, 200);
        stats.discardPending("stopped");
        assertEquals(0, stats.summarize()[4][0], 0);
        StringWriter csv = new StringWriter();
        stats.writeCsv(csv);
        assertEquals("", csv.toString().trim().split(",", -1)[13]);
    }

    @Test
    public void wireTimingIsUnsignedAndWindowIsBounded() {
        FrameLatencyStats stats = new FrameLatencyStats();
        stats.onDecoderInput(1, 1, 100, 100, 200, (char) 65535);
        stats.onDecoderQueued(1, 200);
        assertEquals(6553.5, stats.summarize()[4][1], 0.00001);
        stats.discardPending("stopped");
        for (int i = 0; i < FrameLatencyStats.WINDOW_SIZE; i++) {
            stats.onDecoderInput(i, i, 100, 100, 200, (char) 10);
            stats.onDecoderQueued(i, 200);
            stats.discardPending("stopped");
        }
        assertArrayEquals(new double[] {600, 1, 1, 1}, stats.summarize()[4], 0.00001);
    }

    @Test
    public void hostPercentilesUseObservedSamplesOnly() {
        FrameLatencyStats stats = new FrameLatencyStats();
        for (int i = 0; i <= 100; i++) {
            stats.onDecoderInput(i, i, 100, 100, 200, (char) i);
            stats.discardPending("stopped");
        }
        assertArrayEquals(new double[] {100, 5.05, 9.5, 9.9}, stats.summarize()[4], 0.00001);
    }

    @Test
    public void presentColumnsAreAppendedWithoutChangingExistingColumnOrder() throws Exception {
        assertEquals("frame_number,pts_us,receive_ns,decoder_input_ns,decoder_output_ns," +
                "render_ns,release_ns,status,receive_to_input_ms,input_to_output_ms,output_to_render_ms," +
                "receive_to_render_ms,csv_rows_lost,host_processing_ms,decode_to_present_ms,receive_to_present_ms," +
                "decode_to_release_ms,decode_interval_ms,release_interval_ms," +
                "fps_sample_ns,received_fps,released_fps,shown_fps,present_drops,enqueue_ns,queue_wait_ms," +
                "host_present_to_sent_ms,host_queue_ms,host_encode_ms,host_send_ms," +
                "callback_received_ns,callback_delay_ms," +
                "rtp_host_us,host_present_us,host_acquired_us,host_submitted_us,host_encoded_us," +
                "host_first_sent_us,host_last_sent_us,clock_offset_us,clock_rtt_us",
                FrameLatencyStats.CSV_HEADER.substring(0, FrameLatencyStats.CSV_HEADER.indexOf(",sched_vsync_ns")));
        FrameLatencyStats stats = new FrameLatencyStats();
        stats.onDecoderInput(42, 123, 1000000, 1000000, 2000000, (char) 123);
        stats.onDecoderQueued(123, 2000000);
        stats.onDecoderOutput(0, 123, 5000000);
        stats.onOutputReleased(0, 6000000, true, true);
        stats.onFrameRendered(123, 9000000);
        StringWriter csv = new StringWriter();
        stats.writeCsv(csv);
        assertTrue(csv.toString().startsWith("42,123,1000000,2000000,5000000,9000000,6000000,rendered," +
                "1.000000,3.000000,4.000000,8.000000,0,12.300000,4.000000,8.000000,,,,"));
        assertEquals(41 + SCHEDULER_COLUMNS, csv.toString().trim().split(",", -1).length);
    }

    @Test
    public void batchedOutOfOrderCallbacksUseFrameTimestampsOnce() throws Exception {
        FrameLatencyStats stats = new FrameLatencyStats();
        for (int i = 1; i <= 2; i++) {
            long start = i * 10000000L;
            stats.onDecoderInput(i, i, start, start, start + 1000000, (char) 0);
            stats.onDecoderQueued(i, start + 1000000);
            stats.onDecoderOutput(0, i, start + 3000000);
            stats.onOutputReleased(0, start + 4000000, true, true);
        }
        StringWriter csv = new StringWriter();
        stats.writeCsv(csv);
        assertEquals("", csv.toString());
        stats.onFrameRendered(2, 29000000, 100000000);
        stats.onFrameRendered(1, 15000000, 100000000);
        stats.onFrameRendered(2, 99000000, 190000000);
        assertArrayEquals(new double[] {2, 4, 6, 6}, stats.summarize()[2], 0.00001);
        assertArrayEquals(new double[] {2, 7, 9, 9}, stats.summarize()[3], 0.00001);
        stats.writeCsv(csv);
        String[] rows = csv.toString().split("\n");
        assertEquals(2, rows.length);
        assertTrue(rows[0].contains(",6.000000,9.000000,,,,"));
        assertTrue(rows[1].contains(",2.000000,5.000000,,,,"));
        assertEquals("100000000", rows[0].split(",", -1)[30]);
        assertEquals("71.000000", rows[0].split(",", -1)[31]);
        assertEquals("85.000000", rows[1].split(",", -1)[31]);
    }

    @Test
    public void unavailableOrReversedCallbackClocksDoNotInventDeliveryDelays() throws Exception {
        FrameLatencyStats stats = new FrameLatencyStats();
        for (int i = 1; i <= 2; i++) {
            stats.onDecoderInput(i, i, 1000000, 1000000, 2000000, (char) 0);
            stats.onDecoderQueued(i, 2000000);
            stats.onDecoderOutput(0, i, 3000000);
            stats.onOutputReleased(0, 4000000, true, true);
            stats.onFrameRendered(i, 5000000, i == 1 ? 0 : 4999999);
        }
        StringWriter csv = new StringWriter();
        stats.writeCsv(csv);
        String[] rows = csv.toString().trim().split("\n");
        assertEquals("", rows[0].split(",", -1)[30]);
        for (String row : rows) {
            assertEquals("", row.split(",", -1)[31]);
        }
        assertEquals(2, stats.summarize()[2][1], 0.00001);
    }

    @Test
    public void unobservedDroppedAndUnsupportedFramesLeavePresentColumnsBlank() throws Exception {
        FrameLatencyStats stats = new FrameLatencyStats();
        for (int i = 1; i <= 3; i++) {
            stats.onDecoderInput(i, i, 1000000, 1000000, 2000000, (char) 0);
            stats.onDecoderQueued(i, 2000000);
            stats.onDecoderOutput(0, i, 3000000);
            stats.onOutputReleased(0, 4000000, i != 2, i != 3);
        }
        stats.expire(6000000000L);
        stats.onFrameRendered(1, 5000000);
        StringWriter csv = new StringWriter();
        stats.writeCsv(csv);
        String[] rows = csv.toString().split("\n");
        assertEquals(3, rows.length);
        for (String row : rows) {
            String[] fields = row.split(",", -1);
            assertEquals(41 + SCHEDULER_COLUMNS, fields.length);
            for (int column : new int[] {5, 10, 11, 14, 15}) assertEquals("", fields[column]);
        }
        assertEquals(0, stats.summarize()[2][0], 0);
        assertEquals(0, stats.summarize()[3][0], 0);
    }

    @Test
    public void invalidRenderTimesLeavePresentDurationsBlank() throws Exception {
        FrameLatencyStats stats = new FrameLatencyStats();
        for (int i = 1; i <= 3; i++) {
            stats.onDecoderInput(i, i, 1000000, 1000000, 2000000, (char) 0);
            stats.onDecoderQueued(i, 2000000);
            if (i != 3) stats.onDecoderOutput(0, i, i == 1 ? 1500000 : 3000000);
            stats.onOutputReleased(0, 4000000, true, true);
            stats.onFrameRendered(i, i == 2 ? 2500000 : 5000000);
        }
        StringWriter csv = new StringWriter();
        stats.writeCsv(csv);
        for (String row : csv.toString().split("\n")) {
            String[] fields = row.split(",", -1);
            assertEquals("invalid_render_time", fields[7]);
            for (int column : new int[] {10, 11, 14, 15}) assertEquals("", fields[column]);
        }
        assertEquals(0, stats.summarize()[2][0], 0);
        assertEquals(0, stats.summarize()[3][0], 0);
    }

    @Test
    public void presentPercentilesUseOnlyObservedFramesAndRollOver() {
        FrameLatencyStats stats = new FrameLatencyStats();
        for (int i = 1; i <= 100; i++) {
            stats.onDecoderInput(i, i, 1000000, 1000000, 2000000, (char) 0);
            stats.onDecoderQueued(i, 2000000);
            stats.onDecoderOutput(0, i, 3000000);
            stats.onOutputReleased(0, 3000000, true, true);
            stats.onFrameRendered(i, 3000000 + i * 1000000L);
            stats.onDecoderInput(i + 100, i + 100, 1000000, 1000000, 2000000, (char) 0);
            stats.onDecoderQueued(i + 100, 2000000);
            stats.discard(i + 100, "dropped");
        }
        assertArrayEquals(new double[] {100, 50.5, 95, 99}, stats.summarize()[2], 0.00001);
        assertArrayEquals(new double[] {100, 52.5, 97, 101}, stats.summarize()[3], 0.00001);
        for (int i = 0; i < FrameLatencyStats.WINDOW_SIZE; i++) {
            stats.onDecoderInput(i, i, 1000000, 1000000, 2000000, (char) 0);
            stats.onDecoderQueued(i, 2000000);
            stats.onDecoderOutput(0, i, 3000000);
            stats.onOutputReleased(0, 3000000, true, true);
            stats.onFrameRendered(i, 3000000);
        }
        assertArrayEquals(new double[] {600, 0, 0, 0}, stats.summarize()[2], 0.00001);
        assertArrayEquals(new double[] {600, 2, 2, 2}, stats.summarize()[3], 0.00001);
    }

    @Test
    public void vrrCsvIntervalsUseDecoderOutputEvenWithoutOrWithReorderedPresentCallbacks() throws Exception {
        FrameLatencyStats stats = new FrameLatencyStats(true);
        for (int i = 1; i <= 3; i++) {
            long start = i * 20000000L;
            stats.onDecoderInput(i, i, start, start, start + 1000000, (char) 0);
            stats.onDecoderQueued(i, start + 1000000);
            stats.onDecoderOutput(0, i, start + 3000000);
            stats.onOutputReleased(0, start + 4000000, true, i != 3);
        }
        stats.onFrameRendered(2, 49000000);
        stats.onFrameRendered(1, 29000000);
        StringWriter csv = new StringWriter();
        stats.writeCsv(csv);
        String[] rows = csv.toString().split("\n");
        assertEquals(3, rows.length);
        assertTrue(rows[0].contains("render_unavailable"));
        assertTrue(rows[0].contains(",1.000000,20.000000,20.000000,"));
        assertTrue(rows[1].contains(",1.000000,20.000000,20.000000,"));
        assertTrue(rows[2].contains(",1.000000,,,"));
    }

    @Test
    public void vrrCsvIntervalsIncludeOutputsDroppedBeforeReleaseAndResetStartsANewSeries() throws Exception {
        FrameLatencyStats stats = new FrameLatencyStats(true);
        for (int i = 1; i <= 3; i++) {
            long start = i * 10000000L;
            stats.onDecoderInput(i, i, start, start, start + 1000000, (char) 0);
            stats.onDecoderQueued(i, start + 1000000);
            stats.onDecoderOutput(0, i, start + 2000000);
            stats.onOutputReleased(0, start + 3000000, i != 2, false);
        }
        stats.discardPending("codec_reset");
        stats.onDecoderInput(4, 4, 40000000, 40000000, 41000000, (char) 0);
        stats.onDecoderQueued(4, 41000000);
        stats.onDecoderOutput(0, 4, 42000000);
        stats.onOutputReleased(0, 43000000, true, false);
        StringWriter csv = new StringWriter();
        stats.writeCsv(csv);
        String[] rows = csv.toString().split("\n");
        assertTrue(rows[1].contains("dropped"));
        assertTrue(rows[1].contains(",1.000000,10.000000,,"));
        assertTrue(rows[2].contains(",1.000000,10.000000,20.000000,"));
        assertTrue(rows[3].contains(",1.000000,,,"));
    }

    @Test
    public void vrrInvalidReleaseTimesAndUnreleasedFramesDoNotInventIntervals() throws Exception {
        FrameLatencyStats stats = new FrameLatencyStats(true);
        stats.onDecoderInput(1, 1, 100, 100, 200, (char) 0);
        stats.onDecoderQueued(1, 200);
        stats.onDecoderOutput(0, 1, 300);
        stats.onOutputReleased(0, 250, true, false);
        stats.onDecoderInput(2, 2, 400, 400, 500, (char) 0);
        stats.onDecoderQueued(2, 500);
        stats.discardPending("codec_reset");
        StringWriter csv = new StringWriter();
        stats.writeCsv(csv);
        for (String row : csv.toString().split("\n")) {
            String[] fields = row.split(",", -1);
            for (int column : new int[] {16, 17, 18}) assertEquals("", fields[column]);
        }
    }

    @Test
    public void halfRatePresentationIsVisibleInFpsAndPresentDropCount() throws Exception {
        FrameLatencyStats stats = new FrameLatencyStats(true);
        long startNs = 1000000000L;
        stats.onFrameReceived(startNs);
        for (int i = 1; i <= 240; i++) {
            long receiveNs = startNs + i * 8333333L;
            stats.onFrameReceived(receiveNs);
            stats.onDecoderInput(i, i, receiveNs, receiveNs, receiveNs, (char) 0);
            stats.onDecoderQueued(i, receiveNs);
            stats.onDecoderOutput(0, i, receiveNs);
            stats.onOutputReleased(0, receiveNs, true, true);
        }
        // Deliver one batch backwards to ensure callback delivery time and ordering do not determine FPS.
        for (int i = 240; i >= 2; i -= 2) {
            stats.onFrameRendered(i, startNs + i * 8333333L);
            stats.onFrameRendered(i, startNs + i * 8333333L);
        }
        assertArrayEquals(new float[] {120, 120, 60}, stats.getFrameRates(startNs + 2000000000L), 0.001f);
        assertEquals(0, stats.getPresentDrops());
        stats.expire(startNs + 7000000000L);
        assertEquals(120, stats.getPresentDrops());
        stats.expire(startNs + 8000000000L);
        assertEquals(120, stats.getPresentDrops());
        StringWriter csv = new StringWriter();
        stats.writeCsv(csv);
        int unobserved = 0;
        for (String row : csv.toString().split("\n")) {
            String[] fields = row.split(",", -1);
            assertEquals(41 + SCHEDULER_COLUMNS, fields.length);
            assertTrue(Long.parseLong(fields[19]) > 0);
            assertEquals("120", fields[23]);
            if (fields[7].equals("render_unobserved")) unobserved++;
        }
        assertEquals(120, unobserved);
        assertArrayEquals(new float[] {0, 0, 0}, stats.getFrameRates(startNs + 8000000000L), 0);
    }

    @Test
    public void delayedCallbacksPreRenderDropsAndResetsDoNotCountAsPresentDrops() {
        FrameLatencyStats stats = new FrameLatencyStats();
        for (int i = 1; i <= 4; i++) {
            stats.onFrameReceived(i * 1000000L);
            stats.onDecoderInput(i, i, i * 1000000L, i * 1000000L, i * 1000000L, (char) 0);
            stats.onDecoderQueued(i, i * 1000000L);
            stats.onDecoderOutput(0, i, i * 1000000L);
            stats.onOutputReleased(0, 1000000000L, i != 2, i != 3);
        }
        stats.expire(5999999999L);
        assertEquals(0, stats.getPresentDrops());
        stats.onFrameRendered(1, 2000000000L);
        stats.discardPending("codec_reset");
        stats.expire(9000000000L);
        assertEquals(0, stats.getPresentDrops());
    }

    @Test
    public void missingRenderCallbackSupportDoesNotClaimShownFrames() throws Exception {
        FrameLatencyStats stats = new FrameLatencyStats();
        stats.onFrameReceived(1000000);
        stats.onDecoderInput(1, 1, 1000000, 1000000, 2000000, (char) 0);
        stats.onDecoderQueued(1, 2000000);
        stats.onDecoderOutput(0, 1, 3000000);
        stats.onOutputReleased(0, 4000000, true, false);
        assertEquals(-1, stats.getFrameRates(5000000)[2], 0);
        stats.expire(6000000000L);
        assertEquals(0, stats.getPresentDrops());
        StringWriter csv = new StringWriter();
        stats.writeCsv(csv);
        assertEquals("", csv.toString().trim().split(",", -1)[22]);
    }

    // The receiver's columns before the host timing record's nine were appended.
    private static final String HEADER_BEFORE_THE_HOST_RECORD =
            "frame_number,pts_us,receive_ns,decoder_input_ns,decoder_output_ns," +
            "render_ns,release_ns,status,receive_to_input_ms,input_to_output_ms,output_to_render_ms," +
            "receive_to_render_ms,csv_rows_lost,host_processing_ms," +
            "decode_to_present_ms,receive_to_present_ms," +
            "decode_to_release_ms,decode_interval_ms,release_interval_ms," +
            "fps_sample_ns,received_fps,released_fps,shown_fps,present_drops,enqueue_ns,queue_wait_ms," +
            "host_present_to_sent_ms,host_queue_ms,host_encode_ms,host_send_ms," +
            "callback_received_ns,callback_delay_ms";
    private static final int HOST_RECORD_FIRST_COLUMN = 32;
    private static final int SCHEDULER_COLUMNS = 7;

    // An extended report: {frame, host, queue, encode, send, presentedAt, capture, convert}.
    private static HostTimingReport extendedReport(long[] r) {
        byte[] data = new byte[4 + 20 + 16];
        data[0] = 1;
        data[1] = 1;
        data[2] = 16;
        for (int field = 0; field < 5; field++) {
            for (int b = 0; b < 4; b++) data[4 + 4 * field + b] = (byte) (r[field] >> (8 * b));
        }
        for (int b = 0; b < 8; b++) data[24 + b] = (byte) (r[5] >> (8 * b));
        for (int b = 0; b < 4; b++) data[32 + b] = (byte) (r[6] >> (8 * b));
        for (int b = 0; b < 4; b++) data[36 + b] = (byte) (r[7] >> (8 * b));
        return HostTimingReport.parse(data);
    }

    @Test
    public void theHostRecordColumnsFollowTheReceiversColumnsUnchanged() {
        String header = FrameLatencyStats.CSV_HEADER;
        assertTrue(header.startsWith(HEADER_BEFORE_THE_HOST_RECORD + ","));
        assertEquals(HEADER_BEFORE_THE_HOST_RECORD.split(",").length, HOST_RECORD_FIRST_COLUMN);
        assertTrue(header.contains(",rtp_host_us,host_present_us,host_acquired_us,host_submitted_us," +
                "host_encoded_us,host_first_sent_us,host_last_sent_us,clock_offset_us,clock_rtt_us,sched_vsync_ns"));
        assertEquals(HOST_RECORD_FIRST_COLUMN + 9 + SCHEDULER_COLUMNS, header.trim().split(",").length);
    }

    @Test
    public void hostRecordColumnsComeFromTheExtendedReportAndTheClockOffset() throws Exception {
        FrameLatencyStats stats = new FrameLatencyStats();
        // rtp 9000 ticks = 100000 us. Present 1000000; queue 300 with capture 100 and convert 150;
        // total 900 with encode 200 and send 100.
        stats.onHostTiming(extendedReport(new long[] {42, 900, 300, 200, 100, 1000000, 100, 150}), 1000000000L);
        stats.onDecoderInput(42, 123, 1000000, 1000000, 2000000, (char) 0, 100000);
        stats.onDecoderQueued(123, 2000000);
        stats.onDecoderOutput(0, 123, 5000000);
        stats.onOutputReleased(0, 6000000, true, true);
        stats.onFrameRendered(123, 7000000);

        StringWriter csv = new StringWriter();
        long[] offsetCalls = new long[1];
        stats.writeCsv(csv, out -> {
            offsetCalls[0]++;
            out[0] = -3955;
            out[1] = 90;
            return true;
        });
        String[] fields = csv.toString().trim().split(",", -1);
        assertEquals(FrameLatencyStats.CSV_HEADER.trim().split(",").length, fields.length);
        // The receiver's own host timing columns are unchanged by the extension.
        assertEquals("0.900000", fields[26]);
        assertEquals("0.300000", fields[27]);
        String[] host = java.util.Arrays.copyOfRange(fields, HOST_RECORD_FIRST_COLUMN, HOST_RECORD_FIRST_COLUMN + 9);
        assertArrayEquals(new String[] {"100000", "1000000", "1000100", "1000300", "1000500", "1000800",
                "1000900", "-3955", "90"}, host);
        assertEquals(1, offsetCalls[0]);
    }

    @Test
    public void hostRecordColumnsStayEmptyForAnOldReportAndWithoutAClockExchange() throws Exception {
        FrameLatencyStats stats = new FrameLatencyStats();
        stats.onHostTiming(report(new long[] {42, 900, 300, 200, 100}), 1000000000L);
        stats.onDecoderInput(42, 123, 1000000, 1000000, 2000000, (char) 0, 100000);
        stats.onDecoderQueued(123, 2000000);
        stats.onDecoderOutput(0, 123, 5000000);
        stats.onOutputReleased(0, 6000000, true, true);
        stats.onFrameRendered(123, 7000000);

        // An old (v1) report fills the old columns only; a host without clock sync gives no offset.
        StringWriter csv = new StringWriter();
        stats.writeCsv(csv, out -> false);
        String[] fields = csv.toString().trim().split(",", -1);
        assertEquals(FrameLatencyStats.CSV_HEADER.trim().split(",").length, fields.length);
        assertEquals("0.900000", fields[26]);
        String[] host = java.util.Arrays.copyOfRange(fields, HOST_RECORD_FIRST_COLUMN, HOST_RECORD_FIRST_COLUMN + 9);
        assertArrayEquals(new String[] {"100000", "", "", "", "", "", "", "", ""}, host);

        // A frame without an RTP time (the six-argument call) leaves that column empty too, and
        // writeCsv(Writer) without a source behaves as no clock sync.
        stats.onDecoderInput(43, 124, 1000000, 1000000, 2000000, (char) 0);
        stats.onDecoderQueued(124, 2000000);
        stats.onDecoderOutput(1, 124, 5000000);
        stats.onOutputReleased(1, 6000000, true, true);
        stats.onFrameRendered(124, 7000000);
        csv = new StringWriter();
        stats.writeCsv(csv);
        fields = csv.toString().trim().split(",", -1);
        assertEquals(FrameLatencyStats.CSV_HEADER.trim().split(",").length, fields.length);
        host = java.util.Arrays.copyOfRange(fields, HOST_RECORD_FIRST_COLUMN, HOST_RECORD_FIRST_COLUMN + 9);
        assertArrayEquals(new String[] {"", "", "", "", "", "", "", "", ""}, host);
    }

    @Test
    public void theSchedulerColumnsAreTheLastOnesAndNamedInOrder() {
        String header = FrameLatencyStats.CSV_HEADER.trim();
        assertTrue(header.endsWith(",clock_offset_us,clock_rtt_us,sched_vsync_ns,sched_hold_ms,sched_late," +
                "queue_depth,residency_excess_ms,in_flight,drop_reason"));
        assertEquals(HOST_RECORD_FIRST_COLUMN + 9 + SCHEDULER_COLUMNS, header.split(",").length);
    }

    @Test
    public void theSchedulerColumnsAreEmptyForAFrameThatWasNeverScheduled() throws Exception {
        FrameLatencyStats stats = new FrameLatencyStats();
        stats.onDecoderInput(1, 1, 1000000, 1000000, 2000000, (char) 0, 1);
        stats.onDecoderQueued(1, 2000000);
        stats.onDecoderOutput(0, 1, 5000000);
        stats.onOutputReleased(0, 6000000, true, true);
        stats.onFrameRendered(1, 7000000);
        StringWriter csv = new StringWriter();
        stats.writeCsv(csv);
        String[] fields = csv.toString().trim().split(",", -1);
        assertEquals(FrameLatencyStats.CSV_HEADER.trim().split(",").length, fields.length);
        assertArrayEquals(new String[] {"", "", "", "", "", "", ""},
                java.util.Arrays.copyOfRange(fields, fields.length - SCHEDULER_COLUMNS, fields.length));
    }

    @Test
    public void theSchedulerColumnsCarryThePlanTheQueueAndTheDropReason() throws Exception {
        FrameLatencyStats stats = new FrameLatencyStats();
        for (int i = 0; i < 2; i++) {
            stats.onDecoderInput(i + 1, i + 1, 1000000, 1000000, 2000000, (char) 0, i + 1);
            stats.onDecoderQueued(i + 1, 2000000);
            stats.onDecoderOutput(i, i + 1, 5000000);
        }
        assertEquals("input to output of the owned buffer", 3000000, stats.getInputToOutputNs(0));
        assertEquals(0, stats.getInputToOutputNs(7));
        stats.onQueueState(0, 3, 6500000);
        stats.onDropReason(0, "late");
        stats.onOutputReleased(0, 6000000, false, true);
        stats.onScheduled(1, 123456789L, 2500000L, true, 2);
        stats.onQueueState(1, 0, 0);
        stats.onOutputReleased(1, 6100000, true, true);
        stats.onFrameRendered(2, 7000000);
        // Calls for a buffer that is not owned are ignored.
        stats.onScheduled(9, 1, 1, true, 1);
        stats.onQueueState(9, 1, 1);
        stats.onDropReason(9, "x");

        StringWriter csv = new StringWriter();
        stats.writeCsv(csv);
        String[] rows = csv.toString().trim().split("\n");
        assertEquals(2, rows.length);
        java.util.Map<Long, String[]> byPts = new java.util.HashMap<>();
        for (String row : rows) {
            String[] fields = row.split(",", -1);
            assertEquals(FrameLatencyStats.CSV_HEADER.trim().split(",").length, fields.length);
            byPts.put(Long.parseLong(fields[1]), java.util.Arrays.copyOfRange(fields,
                    fields.length - SCHEDULER_COLUMNS, fields.length));
        }
        assertArrayEquals(new String[] {"", "", "", "3", "6.500000", "", "late"}, byPts.get(1L));
        assertArrayEquals(new String[] {"123456789", "2.500000", "1", "0", "0.000000", "2", ""}, byPts.get(2L));
    }

    @Test
    public void theClockOffsetIsReadOnceAFlushAndNotUnderTheStatsLock() throws Exception {
        FrameLatencyStats stats = new FrameLatencyStats();
        for (int i = 1; i <= 3; i++) {
            stats.onDecoderInput(i, i, 1000000, 1000000, 2000000, (char) 0, i);
            stats.onDecoderQueued(i, 2000000);
            stats.onDecoderOutput(0, i, 5000000);
            stats.onOutputReleased(0, 6000000, true, true);
            stats.onFrameRendered(i, 7000000);
        }
        int[] reads = new int[1];
        boolean[] heldTheLock = new boolean[1];
        stats.writeCsv(new StringWriter(), out -> {
            reads[0]++;
            heldTheLock[0] = Thread.holdsLock(stats);
            return false;
        });
        assertEquals(1, reads[0]);
        assertFalse(heldTheLock[0]);
    }
}
