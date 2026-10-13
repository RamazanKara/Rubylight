package com.limelight.binding.video;

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
                "fps_sample_ns,received_fps,released_fps,shown_fps,present_drops,enqueue_ns,queue_wait_ms\n",
                FrameLatencyStats.CSV_HEADER);
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
        assertEquals(26, csv.toString().trim().split(",", -1).length);
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
        stats.onFrameRendered(2, 29000000);
        stats.onFrameRendered(1, 15000000);
        stats.onFrameRendered(2, 99000000);
        assertArrayEquals(new double[] {2, 4, 6, 6}, stats.summarize()[2], 0.00001);
        assertArrayEquals(new double[] {2, 7, 9, 9}, stats.summarize()[3], 0.00001);
        stats.writeCsv(csv);
        String[] rows = csv.toString().split("\n");
        assertEquals(2, rows.length);
        assertTrue(rows[0].contains(",6.000000,9.000000,,,,"));
        assertTrue(rows[1].contains(",2.000000,5.000000,,,,"));
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
            assertEquals(26, fields.length);
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
            assertEquals(26, fields.length);
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
}
