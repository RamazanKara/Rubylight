package com.limelight.nvstream.av.video;

import org.junit.Test;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertNotNull;

public class HostTimingReportTest {
    private static void put32(byte[] data, int at, long value) {
        for (int i = 0; i < 4; i++) data[at + i] = (byte) (value >> (8 * i));
    }

    // version 1, then records of {frame, host, queue, encode, send}
    private static byte[] report(int version, int count, long[]... records) {
        byte[] data = new byte[4 + 20 * records.length];
        data[0] = (byte) version;
        data[1] = (byte) count;
        for (int i = 0; i < records.length; i++) {
            for (int field = 0; field < 5; field++) put32(data, 4 + 20 * i + 4 * field, records[i][field]);
        }
        return data;
    }

    @Test
    public void decodesRecordsInOrderFromLittleEndianFields() {
        byte[] data = report(1, 2, new long[] {7, 12000, 3000, 5000, 2500}, new long[] {8, 9000, 0, 9000, 0});
        HostTimingReport parsed = HostTimingReport.parse(data);
        assertNotNull(parsed);
        assertEquals(2, parsed.size());
        HostTimingReport.Record first = parsed.get(0);
        assertEquals(7, first.frame);
        assertEquals(12000, first.hostUs);
        assertEquals(3000, first.queueUs);
        assertEquals(5000, first.encodeUs);
        assertEquals(2500, first.sendUs);
        assertEquals(8, parsed.get(1).frame);
        assertEquals(9000, parsed.get(1).encodeUs);
        // Hand-checked byte layout: frame 7 is 07 00 00 00 right after the header.
        assertEquals(7, data[4]);
        assertEquals(12000 & 0xFF, data[8] & 0xFF);
        assertEquals(12000 >> 8, data[9] & 0xFF);
    }

    @Test
    public void valuesAboveTheSignedRangeStayUnsigned() {
        HostTimingReport parsed = HostTimingReport.parse(report(1, 1, new long[] {0xFFFFFFFFL, 0xFFFFFFF0L, 1, 2, 3}));
        assertNotNull(parsed);
        assertEquals(-1, parsed.get(0).frame);
        assertEquals(0xFFFFFFF0L, parsed.get(0).hostUs);
    }

    @Test
    public void acceptsExactlyThirtyTwoRecords() {
        long[][] records = new long[32][];
        for (int i = 0; i < records.length; i++) records[i] = new long[] {i, 100, 10, 20, 30};
        assertEquals(32, HostTimingReport.parse(report(1, 32, records)).size());
        assertNull(HostTimingReport.parse(report(1, 33, new long[33][5])));
    }

    @Test
    public void ignoresBytesAppendedByANewerHost() {
        byte[] data = report(1, 1, new long[] {1, 10, 1, 2, 3});
        byte[] longer = java.util.Arrays.copyOf(data, data.length + 9);
        longer[longer.length - 1] = 0x55;
        HostTimingReport parsed = HostTimingReport.parse(longer);
        assertNotNull(parsed);
        assertEquals(1, parsed.size());
        assertEquals(10, parsed.get(0).hostUs);
    }

    @Test
    public void rejectsWrongVersion() {
        assertNull(HostTimingReport.parse(report(0, 1, new long[] {1, 10, 1, 2, 3})));
        assertNull(HostTimingReport.parse(report(2, 1, new long[] {1, 10, 1, 2, 3})));
    }

    @Test
    public void rejectsZeroCount() {
        assertNull(HostTimingReport.parse(report(1, 0, new long[] {1, 10, 1, 2, 3})));
        assertNull(HostTimingReport.parse(new byte[] {1, 0, 0, 0}));
    }

    @Test
    public void rejectsCountAboveThirtyTwo() {
        assertNull(HostTimingReport.parse(report(1, 33, new long[40][5])));
        assertNull(HostTimingReport.parse(report(1, 255, new long[255][5])));
    }

    @Test
    public void rejectsDataShorterThanTheHeaderAndRecords() {
        assertNull(HostTimingReport.parse(null));
        assertNull(HostTimingReport.parse(new byte[0]));
        assertNull(HostTimingReport.parse(new byte[] {1, 1, 0}));
        byte[] data = report(1, 2, new long[] {1, 10, 1, 2, 3}, new long[] {2, 10, 1, 2, 3});
        assertNotNull(HostTimingReport.parse(data));
        assertNull(HostTimingReport.parse(java.util.Arrays.copyOf(data, data.length - 1)));
        // The header promises two records but only one follows.
        data[1] = 2;
        assertNull(HostTimingReport.parse(java.util.Arrays.copyOf(data, 4 + 20)));
    }

    @Test
    public void rejectsTheWholeReportWhenAnyStagesExceedTheTotal() {
        long[] good = {1, 100, 10, 20, 30};
        assertNotNull(HostTimingReport.parse(report(1, 1, new long[] {2, 60, 10, 20, 30})));
        assertNull(HostTimingReport.parse(report(1, 2, good, new long[] {2, 59, 10, 20, 30})));
        assertNull(HostTimingReport.parse(report(1, 2, new long[] {2, 59, 10, 20, 30}, good)));
        // Stage sums above 2^32 must not wrap around to a small number.
        assertNull(HostTimingReport.parse(report(1, 1, new long[] {3, 5, 0xFFFFFFFFL, 0xFFFFFFFFL, 7})));
    }

    private static void put64(byte[] data, int at, long value) {
        put32(data, at, value & 0xFFFFFFFFL);
        put32(data, at + 4, value >>> 32);
    }

    // An extended report: the v1 records, then {presentedAt, capture, convert} per record.
    private static byte[] extended(int extraBytes, long[][] records, long[][] extras) {
        byte[] data = new byte[4 + (20 + extraBytes) * records.length];
        data[0] = 1;
        data[1] = (byte) records.length;
        data[2] = (byte) extraBytes;
        for (int i = 0; i < records.length; i++) {
            for (int field = 0; field < 5; field++) put32(data, 4 + 20 * i + 4 * field, records[i][field]);
            int at = 4 + 20 * records.length + extraBytes * i;
            put64(data, at, extras[i][0]);
            put32(data, at + 8, extras[i][1]);
            put32(data, at + 12, extras[i][2]);
        }
        return data;
    }

    @Test
    public void readsTheExtensionOfEachRecord() {
        byte[] data = extended(16,
                new long[][] {{7, 12000, 3000, 5000, 2500}, {8, 9000, 400, 8000, 0}},
                new long[][] {{0x1122334455667788L, 1000, 2000}, {5_000_000_000L, 0, 400}});
        HostTimingReport parsed = HostTimingReport.parse(data);
        assertNotNull(parsed);
        HostTimingReport.Record first = parsed.get(0);
        assertTrue(first.hasExtra);
        assertEquals(0x1122334455667788L, first.presentedAtUs);
        assertEquals(1000, first.captureUs);
        assertEquals(2000, first.convertUs);
        assertEquals(3000, first.queueUs);
        assertEquals(5_000_000_000L, parsed.get(1).presentedAtUs);
        assertEquals(400, parsed.get(1).convertUs);
    }

    @Test
    public void aV1ReportHasNoExtension() {
        HostTimingReport parsed = HostTimingReport.parse(report(1, 1, new long[] {1, 10, 1, 2, 3}));
        assertFalse(parsed.get(0).hasExtra);
        assertEquals(0, parsed.get(0).presentedAtUs);
    }

    @Test
    public void anExtensionThatFitsNoRecordKeepsTheBaseRecord() {
        long[][] base = {{1, 100, 10, 20, 30}, {2, 100, 10, 20, 30}};
        // Capture plus convert above the queue time drops that record's extension only.
        HostTimingReport parsed = HostTimingReport.parse(
                extended(16, base, new long[][] {{50, 6, 5}, {60, 5, 5}}));
        assertNotNull(parsed);
        assertFalse(parsed.get(0).hasExtra);
        assertEquals(100, parsed.get(0).hostUs);
        assertTrue(parsed.get(1).hasExtra);
        // The sum must not wrap around 2^32.
        parsed = HostTimingReport.parse(extended(16, new long[][] {base[0]},
                new long[][] {{50, 0xFFFFFFFFL, 0xFFFFFFFFL}}));
        assertFalse(parsed.get(0).hasExtra);
    }

    @Test
    public void anExtensionCutShortOrTooWideForTheLimitKeepsTheBaseRecords() {
        long[][] base = new long[2][];
        long[][] extras = new long[2][];
        for (int i = 0; i < 2; i++) {
            base[i] = new long[] {i, 100, 10, 20, 30};
            extras[i] = new long[] {50, 1, 1};
        }
        byte[] data = extended(16, base, extras);
        assertTrue(HostTimingReport.parse(data).get(1).hasExtra);
        HostTimingReport cut = HostTimingReport.parse(java.util.Arrays.copyOf(data, data.length - 1));
        assertNotNull(cut);
        assertEquals(2, cut.size());
        assertFalse(cut.get(0).hasExtra);
        assertFalse(cut.get(1).hasExtra);
        // A byte 2 below 16 is not an extension.
        data[2] = 15;
        assertFalse(HostTimingReport.parse(data).get(0).hasExtra);
        // Eighteen records are over the extended limit, even with their bytes present.
        long[][] many = new long[18][];
        long[][] manyExtras = new long[18][];
        for (int i = 0; i < 18; i++) {
            many[i] = new long[] {i, 100, 10, 20, 30};
            manyExtras[i] = new long[] {50, 1, 1};
        }
        HostTimingReport over = HostTimingReport.parse(extended(16, many, manyExtras));
        assertEquals(18, over.size());
        assertFalse(over.get(0).hasExtra);
        long[][] most = java.util.Arrays.copyOf(many, 17);
        long[][] mostExtras = java.util.Arrays.copyOf(manyExtras, 17);
        assertTrue(HostTimingReport.parse(extended(16, most, mostExtras)).get(16).hasExtra);
    }

    @Test
    public void entriesWiderThanSixteenBytesAreSteppedOver() {
        long[][] base = {{1, 100, 10, 20, 30}, {2, 100, 10, 20, 30}};
        byte[] data = extended(24, base, new long[][] {{50, 1, 1}, {70, 2, 3}});
        HostTimingReport parsed = HostTimingReport.parse(data);
        assertEquals(70, parsed.get(1).presentedAtUs);
        assertEquals(2, parsed.get(1).captureUs);
        assertEquals(3, parsed.get(1).convertUs);
    }
}
