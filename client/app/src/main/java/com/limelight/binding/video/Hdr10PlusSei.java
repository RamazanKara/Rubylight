package com.limelight.binding.video;

import java.util.Arrays;

/**
 * The HDR10+ metadata of a frame on its way to the decoder, for "Render through Vulkan", which
 * hands it to the output window itself (jni/vulkan/hdr10plus_window.h): the T.35 payload of an
 * ST 2094-40 message from itu_t_t35_country_code on, the form the window takes. Apart from
 * MediaCodec so it can be tested with the host's own bytes.
 * <ul>
 * <li>HEVC: a user_data_registered_itu_t_t35 message (SEI payload type 4) in a prefix SEI NAL unit
 * before the first slice.</li>
 * <li>AV1: a metadata OBU of type ITU-T T.35 (4) before the first frame, frame header or tile
 * group OBU.</li>
 * </ul>
 * Only HDR10+ is taken: country US (0xB5), provider 0x003C, provider-oriented code 0x0001,
 * application identifier 4. Other T.35 messages (HDR Vivid, Dolby Vision) are passed over.
 */
public final class Hdr10PlusSei {
    private Hdr10PlusSei() {}

    /** The first bytes of every HDR10+ T.35 payload. */
    static final byte[] HEADER = {(byte) 0xB5, 0x00, 0x3C, 0x00, 0x01, 0x04};

    private static final int HEVC_LAST_VCL = 31;
    private static final int HEVC_PREFIX_SEI = 39;
    private static final int SEI_USER_DATA_REGISTERED_ITU_T_T35 = 4;
    private static final int OBU_FRAME_HEADER = 3;
    private static final int OBU_TILE_GROUP = 4;
    private static final int OBU_METADATA = 5;
    private static final int OBU_FRAME = 6;
    private static final int OBU_REDUNDANT_FRAME_HEADER = 7;
    private static final int METADATA_TYPE_ITUT_T35 = 4;

    /** The HDR10+ payload of an HEVC access unit in Annex B, or null when it has none before its first slice. */
    public static byte[] hevc(byte[] data, int length) {
        int code = startCode(data, 0, length);
        while (code >= 0) {
            int header = code + 3;
            if (header + 1 >= length) return null;
            int type = (data[header] >> 1) & 0x3F;
            if (type <= HEVC_LAST_VCL) return null;
            int next = startCode(data, header, length);
            if (type == HEVC_PREFIX_SEI) {
                byte[] t35 = seiT35(data, header + 2, next < 0 ? length : next);
                if (t35 != null) return t35;
            }
            code = next;
        }
        return null;
    }

    /**
     * The HDR10+ payload of an AV1 temporal unit (OBUs with size fields, the last one may go without),
     * or null when it has none before its first frame.
     */
    public static byte[] av1(byte[] data, int length) {
        int[] at = {0};
        while (at[0] < length) {
            int header = data[at[0]] & 0xFF;
            if ((header & 0x80) != 0) return null;
            int type = (header >> 3) & 0x0F;
            at[0] += (header & 0x04) != 0 ? 2 : 1;
            int end = length;
            if ((header & 0x02) != 0) {
                long size = leb128(data, at, length);
                if (size < 0 || size > length - at[0]) return null;
                end = at[0] + (int) size;
            }
            if (at[0] > end) return null;
            if (type == OBU_FRAME_HEADER || type == OBU_TILE_GROUP || type == OBU_FRAME ||
                    type == OBU_REDUNDANT_FRAME_HEADER) {
                return null;
            }
            if (type == OBU_METADATA && leb128(data, at, end) == METADATA_TYPE_ITUT_T35) {
                // The payload ends with trailing_bits: a one bit, then zeros to the end of the OBU.
                int last = end;
                while (last > at[0] && data[last - 1] == 0) last--;
                if (last > at[0] && data[last - 1] == (byte) 0x80 && isHdr10Plus(data, at[0], last - 1)) {
                    return Arrays.copyOfRange(data, at[0], last - 1);
                }
            }
            at[0] = end;
        }
        return null;
    }

    /** The HDR10+ payload among the SEI messages of the NAL unit body from {@code from} to {@code end}, or null. */
    private static byte[] seiT35(byte[] data, int from, int end) {
        byte[] rbsp = unescape(data, from, end);
        int length = rbsp.length;
        int at = 0;
        while (at < length && rbsp[at] != (byte) 0x80) {
            int payloadType = 0;
            while (at < length && rbsp[at] == (byte) 0xFF) {
                payloadType += 255;
                at++;
            }
            if (at >= length) return null;
            payloadType += rbsp[at++] & 0xFF;
            int payloadSize = 0;
            while (at < length && rbsp[at] == (byte) 0xFF) {
                payloadSize += 255;
                at++;
            }
            if (at >= length) return null;
            payloadSize += rbsp[at++] & 0xFF;
            if (payloadSize > length - at) return null;
            if (payloadType == SEI_USER_DATA_REGISTERED_ITU_T_T35 && isHdr10Plus(rbsp, at, at + payloadSize)) {
                return Arrays.copyOfRange(rbsp, at, at + payloadSize);
            }
            at += payloadSize;
        }
        return null;
    }

    private static boolean isHdr10Plus(byte[] data, int from, int end) {
        if (end - from < HEADER.length) return false;
        for (int i = 0; i < HEADER.length; i++) {
            if (data[from + i] != HEADER[i]) return false;
        }
        return true;
    }

    /** The bytes from {@code from} to {@code end} without the emulation prevention bytes (the 03 of 00 00 03). */
    static byte[] unescape(byte[] data, int from, int end) {
        byte[] out = new byte[end - from];
        int n = 0;
        int zeros = 0;
        for (int i = from; i < end; i++) {
            byte b = data[i];
            if (zeros >= 2 && b == 3) {
                zeros = 0;
                continue;
            }
            out[n++] = b;
            zeros = b == 0 ? zeros + 1 : 0;
        }
        return n == out.length ? out : Arrays.copyOf(out, n);
    }

    /** A leb128 value at {@code at[0]}, which moves past it, or -1 when it runs past {@code end} or 8 bytes. */
    private static long leb128(byte[] data, int[] at, int end) {
        long value = 0;
        for (int i = 0; i < 8; i++) {
            if (at[0] >= end) return -1;
            int b = data[at[0]++] & 0xFF;
            value |= (long) (b & 0x7F) << (7 * i);
            if ((b & 0x80) == 0) return value;
        }
        return -1;
    }

    /** The offset of the next 00 00 01 at or after {@code from}, or -1. */
    private static int startCode(byte[] data, int from, int length) {
        for (int i = from; i + 2 < length; i++) {
            if (data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1) return i;
        }
        return -1;
    }
}
