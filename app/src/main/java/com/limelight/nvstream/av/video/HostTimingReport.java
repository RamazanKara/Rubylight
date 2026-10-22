package com.limelight.nvstream.av.video;

/**
 * One host timing report (control message 0x5536, protocol/core/src/control.rs): after each new
 * frame's last packet is handed to the socket, the host says how long it held the picture,
 * split into queue, encode and send. All numbers are little-endian u32, except the extension's
 * present stamp (u64). An extended report (header byte 2 = 16) follows the records with one
 * 16-byte entry per record: the host's present stamp on its video clock, then the capture and
 * convert times that make up part of the queue time.
 */
public final class HostTimingReport {
    public static final int MESSAGE_TYPE = 0x5536;
    public static final int VERSION = 1;
    public static final int HEADER_BYTES = 4;
    public static final int RECORD_BYTES = 20;
    public static final int MAX_RECORDS = 32;
    /** Bytes per record of the extension (header byte 2). */
    public static final int EXTRA_BYTES = 16;
    /** An extended report holds at most this many records, so it stays one ENet packet. */
    public static final int EXTENDED_MAX_RECORDS = 17;

    /** Timing of one frame, in microseconds. */
    public static final class Record {
        /** Moonlight video frame index, the frame header's frameNumber. */
        public final int frame;
        /** The host's present stamp to the frame's last packet handed to the socket. */
        public final long hostUs;
        /** Present stamp to the encoder taking the picture. */
        public final long queueUs;
        /** Encoder taking the picture to the encoded frame in hand. */
        public final long encodeUs;
        /** First packet to last packet. */
        public final long sendUs;
        /** True when the extension below came with the record. */
        public final boolean hasExtra;
        /** The host's present stamp on its video clock (microseconds, u64 as a signed long). */
        public final long presentedAtUs;
        /** Present stamp to the captured picture in hand; part of {@link #queueUs}. */
        public final long captureUs;
        /** Captured picture to the converted picture; with captureUs at most queueUs. */
        public final long convertUs;

        public Record(int frame, long hostUs, long queueUs, long encodeUs, long sendUs) {
            this(frame, hostUs, queueUs, encodeUs, sendUs, false, 0, 0, 0);
        }

        public Record(int frame, long hostUs, long queueUs, long encodeUs, long sendUs,
                      long presentedAtUs, long captureUs, long convertUs) {
            this(frame, hostUs, queueUs, encodeUs, sendUs, true, presentedAtUs, captureUs, convertUs);
        }

        private Record(int frame, long hostUs, long queueUs, long encodeUs, long sendUs,
                       boolean hasExtra, long presentedAtUs, long captureUs, long convertUs) {
            this.frame = frame;
            this.hostUs = hostUs;
            this.queueUs = queueUs;
            this.encodeUs = encodeUs;
            this.sendUs = sendUs;
            this.hasExtra = hasExtra;
            this.presentedAtUs = presentedAtUs;
            this.captureUs = captureUs;
            this.convertUs = convertUs;
        }
    }

    private final Record[] records;

    private HostTimingReport(Record[] records) {
        this.records = records;
    }

    public int size() {
        return records.length;
    }

    public Record get(int index) {
        return records[index];
    }

    /**
     * Decodes a report, or returns null for the whole message when the version is not 1, the
     * count is 0 or above 32, the data is shorter than the header plus count records, or any
     * record's stages add up to more than its total. Trailing bytes from a newer host are ignored.
     * The extension (header byte 2 of at least 16, at most 17 records, all entries present) is
     * read as the host's decoder does: a record's extension is dropped, keeping the base record,
     * when the report has none, is cut short or has too many records, or when the record's
     * capture plus convert time exceeds its queue time. Entries wider than 16 bytes are
     * stepped over.
     */
    public static HostTimingReport parse(byte[] data) {
        if (data == null || data.length < HEADER_BYTES || data[0] != VERSION) {
            return null;
        }
        int count = data[1] & 0xFF;
        if (count == 0 || count > MAX_RECORDS || data.length < HEADER_BYTES + count * RECORD_BYTES) {
            return null;
        }
        int extra = data[2] & 0xFF;
        int baseEnd = HEADER_BYTES + count * RECORD_BYTES;
        boolean extended = extra >= EXTRA_BYTES && count <= EXTENDED_MAX_RECORDS &&
                data.length >= baseEnd + (long) count * extra;
        Record[] records = new Record[count];
        for (int i = 0; i < count; i++) {
            int at = HEADER_BYTES + i * RECORD_BYTES;
            long hostUs = u32(data, at + 4);
            long queueUs = u32(data, at + 8);
            long encodeUs = u32(data, at + 12);
            long sendUs = u32(data, at + 16);
            if (queueUs + encodeUs + sendUs > hostUs) {
                return null;
            }
            int frame = (int) u32(data, at);
            records[i] = new Record(frame, hostUs, queueUs, encodeUs, sendUs);
            if (extended) {
                int x = baseEnd + i * extra;
                long captureUs = u32(data, x + 8);
                long convertUs = u32(data, x + 12);
                if (captureUs + convertUs <= queueUs) {
                    records[i] = new Record(frame, hostUs, queueUs, encodeUs, sendUs,
                            u64(data, x), captureUs, convertUs);
                }
            }
        }
        return new HostTimingReport(records);
    }

    private static long u64(byte[] data, int at) {
        return u32(data, at) | u32(data, at + 4) << 32;
    }

    private static long u32(byte[] data, int at) {
        return (data[at] & 0xFFL) | (data[at + 1] & 0xFFL) << 8 |
                (data[at + 2] & 0xFFL) << 16 | (data[at + 3] & 0xFFL) << 24;
    }
}
