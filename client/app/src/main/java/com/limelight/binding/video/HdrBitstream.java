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

    /**
     * Whether the host's HDR Vivid SEI is kept from this decoder. MediaTek's HEVC and Dolby Vision
     * decoders prefer HDR Vivid to the HDR10 and HDR10+ the stream also carries, and a display
     * without Vivid (Android has no HDR type for it) then shows the picture as SDR: a TCL C6K logs
     * "oriInFmt[Hdr HDRVIVID -> SDR]". With no dynamic metadata at all (hdr_dynamic_metadata off)
     * its Dolby Vision decoder showed the same stream as HDR10, and its AV1 decoder shows HDR10+.
     */
    public static boolean dropsVivid(String decoderName, String mimeType) {
        String name = decoderName.toLowerCase(java.util.Locale.ROOT);
        boolean hevc = "video/hevc".equals(mimeType) || HdrFormatPolicy.MIME_DOLBY_VISION.equals(mimeType);
        return hevc && (name.startsWith("c2.mtk.") || name.startsWith("omx.mtk."));
    }

    private static final int HEVC_LAST_VCL = 31;
    private static final int HEVC_PREFIX_SEI = 39;
    private static final int SEI_USER_DATA_REGISTERED_ITU_T_T35 = 4;
    /** The T.35 header of an HDR Vivid message: China, provider CUVA (0x0004), provider-oriented code 0x0005. */
    private static final byte[] VIVID_T35 = {0x26, 0x00, 0x04, 0x00, 0x05};

    /**
     * Finds the first HDR Vivid SEI NAL unit before the first slice of an HEVC access unit:
     * {@code range} gets the offset of its start code (with a leading zero_byte) and the offset of
     * the next NAL unit's, or {@code length}. Returns false, with {@code range} untouched, when there
     * is none.
     * <p>
     * The Rubylight host sends HDR10+ and HDR Vivid with every HDR frame, each message in its own
     * prefix SEI NAL unit (hdr_dynamic::hevc_t35_sei), before the first slice.
     */
    public static boolean findVividSei(byte[] data, int length, int[] range) {
        int code = startCode(data, 0, length);
        while (code >= 0) {
            int header = code + 3;
            if (header + 1 >= length) return false;
            int type = (data[header] >> 1) & 0x3F;
            if (type <= HEVC_LAST_VCL) return false;
            int next = startCode(data, header, length);
            int end = next < 0 ? length : unitStart(data, next);
            if (type == HEVC_PREFIX_SEI && isVivid(data, header + 2, end)) {
                range[0] = unitStart(data, code);
                range[1] = end;
                return true;
            }
            code = next;
        }
        return false;
    }

    /** The number of bytes {@link #put} queues: {@code length} without the range {@code drop}, if any. */
    public static int putLength(int length, int[] drop) {
        return drop == null ? length : length - (drop[1] - drop[0]);
    }

    /**
     * Puts the first {@code length} bytes of {@code data} into a decoder input buffer after what it
     * already holds (the parameter sets of a fused IDR frame), without the bytes from
     * {@code drop[0]} to {@code drop[1]} unless {@code drop} is null.
     */
    public static void put(java.nio.ByteBuffer buffer, byte[] data, int length, int[] drop) {
        if (drop == null) {
            buffer.put(data, 0, length);
        } else {
            buffer.put(data, 0, drop[0]);
            buffer.put(data, drop[1], length - drop[1]);
        }
    }

    /** Whether the SEI payload at {@code at} is a T.35 message with the HDR Vivid header. */
    private static boolean isVivid(byte[] data, int at, int end) {
        int payloadType = 0;
        while (at < end && data[at] == (byte) 0xFF) {
            payloadType += 255;
            at++;
        }
        if (at >= end || payloadType + (data[at++] & 0xFF) != SEI_USER_DATA_REGISTERED_ITU_T_T35) return false;
        // The size, whose value does not matter here.
        while (at < end && data[at] == (byte) 0xFF) at++;
        at++;
        if (at + VIVID_T35.length > end) return false;
        for (int i = 0; i < VIVID_T35.length; i++) {
            if (data[at + i] != VIVID_T35[i]) return false;
        }
        return true;
    }

    /** The offset of the next 00 00 01 at or after {@code from}, or -1. */
    private static int startCode(byte[] data, int from, int length) {
        for (int i = from; i + 2 < length; i++) {
            if (data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1) return i;
        }
        return -1;
    }

    /** A start code's offset, with the zero_byte of a four-byte start code. */
    private static int unitStart(byte[] data, int code) {
        return code > 0 && data[code - 1] == 0 ? code - 1 : code;
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
