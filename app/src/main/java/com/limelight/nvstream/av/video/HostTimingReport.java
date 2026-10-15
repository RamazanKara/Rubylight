package com.limelight.nvstream.av.video;

/**
 * One host timing report (control message 0x5536, protocol/core/src/control.rs): after each new
 * frame's last packet is handed to the socket, the host says how long it held the picture,
 * split into queue, encode and send. All numbers are little-endian u32.
 */
public final class HostTimingReport {
    public static final int MESSAGE_TYPE = 0x5536;
    public static final int VERSION = 1;
    public static final int HEADER_BYTES = 4;
    public static final int RECORD_BYTES = 20;
    public static final int MAX_RECORDS = 32;

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

        public Record(int frame, long hostUs, long queueUs, long encodeUs, long sendUs) {
            this.frame = frame;
            this.hostUs = hostUs;
            this.queueUs = queueUs;
            this.encodeUs = encodeUs;
            this.sendUs = sendUs;
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
     */
    public static HostTimingReport parse(byte[] data) {
        if (data == null || data.length < HEADER_BYTES || data[0] != VERSION) {
            return null;
        }
        int count = data[1] & 0xFF;
        if (count == 0 || count > MAX_RECORDS || data.length < HEADER_BYTES + count * RECORD_BYTES) {
            return null;
        }
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
            records[i] = new Record((int) u32(data, at), hostUs, queueUs, encodeUs, sendUs);
        }
        return new HostTimingReport(records);
    }

    private static long u32(byte[] data, int at) {
        return (data[at] & 0xFFL) | (data[at + 1] & 0xFFL) << 8 |
                (data[at + 2] & 0xFFL) << 16 | (data[at + 3] & 0xFFL) << 24;
    }
}
