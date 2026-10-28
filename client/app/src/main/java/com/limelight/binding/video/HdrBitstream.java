package com.limelight.binding.video;

/**
 * HDR fixes to an HEVC access unit on its way to the decoder, apart from MediaCodec so they can be
 * tested with made-up bytes.
 */
public final class HdrBitstream {
    private HdrBitstream() {}

    /**
     * The length of a picture data buffer to queue: a Dolby Vision access unit without the zero
     * bytes after its last NAL unit, anything else as it came.
     * <p>
     * The Rubylight host puts the RPU (NAL unit type 62) last, and the zero padding of the last
     * FEC shard follows it: moonlight-common-c and nightjar-core trim that padding only for codecs
     * other than H.264 and HEVC, where it is legal trailing_zero_8bits. MediaTek's Dolby Vision
     * parser reads those zeros as RPU extension data and rejects every RPU ("level 0 extension with
     * length 2147483647"), so the decoder shows only the base layer. Every NAL unit ends with a
     * nonzero byte (rbsp_stop_one_bit, or the 0x03 that follows a cabac_zero_word), so nothing of
     * the picture or the RPU is lost.
     */
    public static int queuedLength(boolean dolbyVision, byte[] data, int length) {
        if (!dolbyVision) return length;
        int end = length;
        while (end > 0 && data[end - 1] == 0) {
            end--;
        }
        return end;
    }

    /** The bytes before {@code end}, at most {@code count}, as hex for a log line. */
    static String tail(byte[] data, int end, int count) {
        StringBuilder hex = new StringBuilder();
        for (int i = Math.max(0, end - count); i < end; i++) {
            if (hex.length() != 0) hex.append(' ');
            hex.append(String.format("%02x", data[i] & 0xff));
        }
        return hex.toString();
    }
}
