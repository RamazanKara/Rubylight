package com.limelight.binding.video;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertTrue;

import com.limelight.nvstream.av.video.HostTimingReport;

import org.junit.After;
import org.junit.Test;

import java.io.StringWriter;
import java.util.Locale;
import java.util.Random;

/**
 * The latency CSV is read by scripts, so a row must stay what it was to the character. The rows are now put
 * together in one buffer; this holds them against the code that wrote them before, kept below as it stood.
 */
public class FrameLatencyCsvRowTest {
    private final Locale locale = Locale.getDefault();

    @After
    public void restoreLocale() {
        Locale.setDefault(locale);
    }

    /** Values at every edge the row's arithmetic has: nothing, a sign, a millisecond, a second, a long's ends. */
    private static final long[] EDGES = {
            0, 1, -1, 9, 10, 999999, 1000000, 1000001, 1999999, 2000000, -5, -999999, -1000000, -1000001, -1500000,
            1234567890L, 16666667L, 1000000000L, 123456789012345L, -123456789012345L,
            Long.MAX_VALUE, Long.MAX_VALUE - 1, Long.MIN_VALUE, Long.MIN_VALUE + 1,
            Long.MAX_VALUE / 1000, Long.MAX_VALUE / 1000 + 1, Long.MIN_VALUE / 1000, Integer.MAX_VALUE, Integer.MIN_VALUE,
    };

    private static final String[] STATUSES = {"rendered", "dropped", "surface_changed", "expired", "", null};
    private static final String[] REASONS = {null, "", "late", "queue_full"};

    private static long value(Random random) {
        switch (random.nextInt(4)) {
            case 0: return EDGES[random.nextInt(EDGES.length)];
            case 1: return random.nextLong();
            // A stream's own clock readings, a few hours of nanoseconds.
            case 2: return 1 + (long) (random.nextDouble() * 2e13);
            default: return random.nextInt(40000000);
        }
    }

    private static HostTimingReport.Record record(Random random) {
        switch (random.nextInt(3)) {
            case 0: return null;
            case 1: return new HostTimingReport.Record(random.nextInt(), value(random), value(random), value(random),
                    value(random));
            default: return new HostTimingReport.Record(random.nextInt(), value(random), value(random), value(random),
                    value(random), value(random), value(random), value(random));
        }
    }

    private static FrameLatencyStats.Frame frame(Random random) {
        FrameLatencyStats.Frame frame = new FrameLatencyStats.Frame();
        frame.number = random.nextBoolean() ? random.nextInt() : random.nextInt(100000);
        frame.ptsUs = value(random);
        frame.receiveNs = value(random);
        frame.enqueueNs = value(random);
        frame.inputNs = value(random);
        frame.submitNs = value(random);
        frame.outputNs = value(random);
        frame.renderNs = value(random);
        frame.callbackReceivedNs = value(random);
        frame.releaseNs = value(random);
        frame.handoffNs = value(random);
        frame.hostProcessingNs = value(random);
        frame.hostRtpUs = random.nextBoolean() ? -1 : value(random);
        frame.hostTiming = record(random);
        frame.previousOutputNs = value(random);
        frame.previousReleaseNs = value(random);
        frame.status = STATUSES[random.nextInt(STATUSES.length)];
        frame.schedVsyncNs = value(random);
        frame.schedHoldNs = random.nextBoolean() ? -1 : value(random);
        frame.schedLate = random.nextBoolean();
        frame.queueDepth = random.nextBoolean() ? -1 : random.nextInt();
        frame.residencyExcessNs = random.nextBoolean() ? -1 : value(random);
        frame.inFlight = random.nextBoolean() ? -1 : random.nextInt();
        frame.dropReason = REASONS[random.nextInt(REASONS.length)];
        return frame;
    }

    /** A frame as a stream makes it: times in order, a few milliseconds apart. */
    private static FrameLatencyStats.Frame ordered(Random random) {
        FrameLatencyStats.Frame frame = frame(random);
        long at = 1 + (long) (random.nextDouble() * 2e13);
        frame.receiveNs = at;
        frame.enqueueNs = at += random.nextInt(3000000);
        frame.inputNs = at += random.nextInt(3000000);
        frame.outputNs = at += random.nextInt(30000000);
        frame.renderNs = at += random.nextInt(30000000);
        frame.releaseNs = at += random.nextInt(3000000);
        frame.callbackReceivedNs = at + random.nextInt(3000000);
        frame.previousOutputNs = frame.outputNs - random.nextInt(40000000);
        frame.previousReleaseNs = frame.releaseNs - random.nextInt(40000000);
        frame.hostProcessingNs = random.nextInt(20000000);
        frame.status = "rendered";
        return frame;
    }

    private static void assertRowsAsBefore(long seed, int rows) {
        Random random = new Random(seed);
        // The buffer is the one a flush reuses, so a row must not leave anything behind for the next.
        StringBuilder row = new StringBuilder();
        for (int i = 0; i < rows; i++) {
            FrameLatencyStats.Frame frame = random.nextInt(3) == 0 ? ordered(random) : frame(random);
            boolean vrr = random.nextBoolean();
            long lost = value(random);
            long dropped = value(random);
            long sampleNs = value(random);
            String frameRates = random.nextBoolean() ? "59.94,60.00," : "119.88,120.00,118.50";
            String clockColumns = random.nextBoolean() ? "," : value(random) + "," + value(random);
            row.setLength(0);
            FrameLatencyStats.appendRow(row, frame, vrr, lost, dropped, sampleNs, frameRates, clockColumns);
            assertEquals("row " + i + " of seed " + seed,
                    oldRow(frame, vrr, lost, dropped, sampleNs, frameRates, clockColumns), row.toString());
        }
    }

    @Test
    public void everyRowIsWhatTheOldCodeWrote() {
        assertRowsAsBefore(20261110L, 20000);
    }

    @Test
    public void theRowsDoNotFollowTheDevicesLanguage() {
        for (String tag : new String[] {"de-DE", "fr-FR", "ar-SA-u-nu-arab", "hi-IN-u-nu-deva", "th-TH-u-nu-thai", "tr-TR"}) {
            Locale.setDefault(Locale.forLanguageTag(tag));
            assertRowsAsBefore(7, 2000);
        }
    }

    @Test
    public void everyEdgeInEveryPlaceIsWhatTheOldCodeWrote() {
        Random random = new Random(3);
        StringBuilder row = new StringBuilder();
        for (long edge : EDGES) {
            for (long other : EDGES) {
                FrameLatencyStats.Frame frame = frame(random);
                frame.number = (int) edge;
                frame.ptsUs = frame.receiveNs = frame.outputNs = frame.releaseNs = frame.previousOutputNs = edge;
                frame.hostProcessingNs = frame.schedHoldNs = frame.hostRtpUs = frame.schedVsyncNs = edge;
                frame.enqueueNs = frame.inputNs = frame.renderNs = frame.callbackReceivedNs = other;
                frame.previousReleaseNs = frame.residencyExcessNs = other;
                frame.queueDepth = frame.inFlight = (int) other;
                frame.hostTiming = new HostTimingReport.Record(1, edge, other, edge, other, other, edge, other);
                frame.status = "rendered";
                for (boolean vrr : new boolean[] {false, true}) {
                    row.setLength(0);
                    FrameLatencyStats.appendRow(row, frame, vrr, edge, other, edge, "0.00,0.00,", ",");
                    assertEquals(edge + " and " + other, oldRow(frame, vrr, edge, other, edge, "0.00,0.00,", ","),
                            row.toString());
                }
            }
        }
    }

    @Test
    public void aFlushWritesItsRowsOneAfterAnotherEachEndingItsLine() throws Exception {
        FrameLatencyStats stats = new FrameLatencyStats();
        for (int i = 1; i <= 3; i++) {
            stats.onDecoderInput(i, 100 * i, 1000000L * i, 1000001L * i, 1000011L * i, (char) 1);
            stats.onDecoderOutput(i, 100 * i, 1234567890L + i);
            stats.onOutputReleased(i, 1234567891L + i, false, false);
        }
        StringWriter csv = new StringWriter();
        stats.writeCsv(csv);
        String text = csv.toString();
        assertTrue(text.endsWith("\n"));
        String[] rows = text.split("\n");
        assertEquals(3, rows.length);
        int columns = FrameLatencyStats.CSV_HEADER.trim().split(",", -1).length;
        for (int i = 0; i < rows.length; i++) {
            assertTrue(rows[i].startsWith((i + 1) + "," + 100 * (i + 1) + ","));
            assertEquals(columns, rows[i].split(",", -1).length);
        }
    }

    @Test
    public void consecutiveFlushesReuseTheirBufferAndReportTheCharactersWritten() throws Exception {
        FrameLatencyStats stats = new FrameLatencyStats();
        StringBuilder expected = new StringBuilder();
        for (int flush = 0; flush < 3; flush++) {
            for (int i = 1; i <= 25; i++) {
                int number = flush * 100 + i;
                stats.onDecoderInput(number, 100 * i, 1000000L * i, 1000001L * i, 1000011L * i, (char) 1);
                stats.onDecoderOutput(number, 100 * i, 1234567890L + i);
                stats.onOutputReleased(number, 1234567891L + i, false, false);
            }
            StringWriter csv = new StringWriter();
            long written = stats.writeCsv(csv);
            assertEquals(csv.toString().length(), written);
            expected.append(csv);
        }
        // Each flush wrote whole rows, and the three flushes together hold 75 rows in order.
        String text = expected.toString();
        assertTrue(text.endsWith("\n"));
        String[] rows = text.split("\n");
        assertEquals(75, rows.length);
        for (int i = 0; i < rows.length; i++) {
            int number = (i / 25) * 100 + (i % 25) + 1;
            assertTrue(rows[i], rows[i].startsWith(number + ","));
        }
    }

    // What follows is the code the rows were written with before, word for word but for the names.

    private static String oldRow(FrameLatencyStats.Frame frame, boolean vrr, long lost, long dropped, long sampleNs,
                                 String frameRates, String clockColumns) {
        String decodeToPresent = "rendered".equals(frame.status) ? oldDuration(frame.outputNs, frame.renderNs) : "";
        String receiveToPresent = "rendered".equals(frame.status) ? oldDuration(frame.receiveNs, frame.renderNs) : "";
        return frame.number + "," + frame.ptsUs + "," + oldTimestamp(frame.receiveNs) + "," +
                oldTimestamp(frame.inputNs) + "," + oldTimestamp(frame.outputNs) + "," + oldTimestamp(frame.renderNs) + "," +
                oldTimestamp(frame.releaseNs) + "," + frame.status + "," +
                oldDuration(frame.receiveNs, frame.inputNs) + "," + oldDuration(frame.inputNs, frame.outputNs) + "," +
                decodeToPresent + "," + receiveToPresent + "," + lost + "," +
                (frame.hostProcessingNs == 0 ? "" : oldMilliseconds(frame.hostProcessingNs)) + "," +
                decodeToPresent + "," + receiveToPresent + "," +
                (vrr ? oldDuration(frame.outputNs, frame.releaseNs) : "") + "," +
                oldDuration(frame.previousOutputNs, frame.outputNs) + "," +
                oldDuration(frame.previousReleaseNs, frame.releaseNs) + "," + sampleNs + "," +
                frameRates + "," + dropped + "," +
                oldTimestamp(frame.enqueueNs) + "," + oldDuration(frame.enqueueNs, frame.inputNs) +
                oldHostTimingColumns(frame.hostTiming) + "," + oldTimestamp(frame.callbackReceivedNs) + "," +
                oldDuration(frame.renderNs, frame.callbackReceivedNs) +
                "," + (frame.hostRtpUs < 0 ? "" : Long.toString(frame.hostRtpUs)) +
                oldHostStampColumns(frame.hostTiming) + "," + clockColumns + oldSchedulerColumns(frame) + "\n";
    }

    private static String oldSchedulerColumns(FrameLatencyStats.Frame frame) {
        return "," + oldTimestamp(frame.schedVsyncNs) +
                "," + (frame.schedHoldNs < 0 ? "" : oldMilliseconds(frame.schedHoldNs)) +
                "," + (frame.schedHoldNs < 0 ? "" : frame.schedLate ? "1" : "0") +
                "," + (frame.queueDepth < 0 ? "" : Integer.toString(frame.queueDepth)) +
                "," + (frame.residencyExcessNs < 0 ? "" : oldMilliseconds(frame.residencyExcessNs)) +
                "," + (frame.inFlight < 0 ? "" : Integer.toString(frame.inFlight)) +
                "," + (frame.dropReason == null ? "" : frame.dropReason);
    }

    private static String oldHostTimingColumns(HostTimingReport.Record record) {
        return record == null ? ",,,," : "," + oldMilliseconds(record.hostUs * 1000L) +
                "," + oldMilliseconds(record.queueUs * 1000L) + "," + oldMilliseconds(record.encodeUs * 1000L) +
                "," + oldMilliseconds(record.sendUs * 1000L);
    }

    // Host present, acquired, submitted, encoded, first and last packet sent, on the host's video
    // clock (microseconds), each with its leading comma; empty without an extended report.
    private static String oldHostStampColumns(HostTimingReport.Record record) {
        if (record == null || !record.hasExtra) return ",,,,,,";
        long present = record.presentedAtUs;
        long submitted = present + record.queueUs;
        long lastSent = present + record.hostUs;
        return "," + Long.toUnsignedString(present) +
                "," + Long.toUnsignedString(present + record.captureUs) +
                "," + Long.toUnsignedString(submitted) +
                "," + Long.toUnsignedString(submitted + record.encodeUs) +
                "," + Long.toUnsignedString(lastSent - record.sendUs) +
                "," + Long.toUnsignedString(lastSent);
    }

    private static String oldTimestamp(long timeNs) {
        return timeNs == 0 ? "" : Long.toString(timeNs);
    }

    private static String oldDuration(long startNs, long endNs) {
        return startNs == 0 || endNs == 0 || endNs < startNs ? "" :
                oldMilliseconds(endNs - startNs);
    }

    private static String oldMilliseconds(long durationNs) {
        return durationNs / 1000000 + "." + Long.toString(1000000 + durationNs % 1000000).substring(1);
    }
}
