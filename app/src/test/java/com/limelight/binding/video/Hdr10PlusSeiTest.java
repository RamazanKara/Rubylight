package com.limelight.binding.video;

import static org.junit.Assert.*;

import org.junit.Test;

import java.io.ByteArrayOutputStream;

/**
 * The goldens are what rust/core/src/hdr_dynamic.rs writes: Hdr10Plus::to_t35 of the tests' sample()
 * frame and of all-zero statistics, wrapped by hevc_t35_sei and av1_t35_obu, and HDR Vivid's
 * hevc_t35_sei for sample().
 */
public class Hdr10PlusSeiTest {
    private static final String T35 = "b5 00 3c 00 01 04 01 40 00 00 00 75 30 3a 98 1d 4c 01 c2 24 08 00 14 28 00 00 " +
            "50 03 fc c8 03 21 90 0c 82 58 1f 42 d0 5d c2 f8 8c a3 18 ea 60 00 00";
    private static final String HEVC_SEI = "00 00 00 01 4e 01 04 31 b5 00 3c 00 01 04 01 40 00 00 03 00 75 30 3a 98 " +
            "1d 4c 01 c2 24 08 00 14 28 00 00 50 03 fc c8 03 21 90 0c 82 58 1f 42 d0 5d c2 f8 8c a3 18 ea 60 00 00 80";
    /** All-zero statistics: emulation prevention all through. */
    private static final String HEVC_SEI_ZEROS = "00 00 00 01 4e 01 04 31 b5 00 3c 00 01 04 01 40 00 00 03 00 00 03 " +
            "00 00 03 00 00 03 00 00 03 00 24 08 00 00 28 00 00 50 00 00 c8 00 01 90 00 02 58 00 02 d0 00 02 f8 00 03 " +
            "18 00 00 03 00 00 80";
    private static final String AV1_OBU = "2a 33 04 b5 00 3c 00 01 04 01 40 00 00 00 75 30 3a 98 1d 4c 01 c2 24 08 00 " +
            "14 28 00 00 50 03 fc c8 03 21 90 0c 82 58 1f 42 d0 5d c2 f8 8c a3 18 ea 60 00 00 80";
    private static final String HEVC_VIVID = "00 00 00 01 4e 01 04 0d 26 00 04 00 05 01 0b d7 52 72 ce 70 00 80";

    private static final String HEVC_AUD = "00 00 00 01 46 01 50";
    private static final String HEVC_IDR_SLICE = "00 00 01 26 01 af 08 42 23 10 5f";
    private static final String HEVC_SLICE = "00 00 01 02 01 d0 09 7e 45";
    private static final String AV1_TEMPORAL_DELIMITER = "12 00";
    private static final String AV1_SEQUENCE_HEADER = "0a 0b 00 00 00 24 c6 af 7f fc 10 34 08";
    private static final String AV1_FRAME = "32 05 10 00 b5 3a 20";

    private static byte[] bytes(String... parts) {
        ByteArrayOutputStream out = new ByteArrayOutputStream();
        for (String part : parts) {
            for (String hex : part.trim().split("\\s+")) {
                out.write(Integer.parseInt(hex, 16));
            }
        }
        return out.toByteArray();
    }

    private static byte[] hevc(String... parts) {
        byte[] frame = bytes(parts);
        // Decode units come in a larger buffer: only the length counts.
        byte[] buffer = new byte[frame.length + 16];
        System.arraycopy(frame, 0, buffer, 0, frame.length);
        return Hdr10PlusSei.hevc(buffer, frame.length);
    }

    private static byte[] av1(String... parts) {
        byte[] frame = bytes(parts);
        return Hdr10PlusSei.av1(frame, frame.length);
    }

    @Test
    public void anHevcFrameGivesTheHostsT35Payload() {
        assertArrayEquals(bytes(T35), hevc(HEVC_AUD, HEVC_SEI, HEVC_VIVID, HEVC_IDR_SLICE));
        // HDR Vivid first, as another host may write them, and no AUD.
        assertArrayEquals(bytes(T35), hevc(HEVC_VIVID, HEVC_SEI, HEVC_SLICE));
    }

    @Test
    public void emulationPreventionIsTakenOut() {
        byte[] t35 = hevc(HEVC_SEI_ZEROS, HEVC_SLICE);
        // hdr10plus_known_vector_from_the_syntax_table
        byte[] expected = bytes("b5 00 3c 00 01 04 01 40 00 00 00", "00 00 00 00 00 00 00 00",
                "24 08 00 00 28 00 00 50 00 00 c8 00 01 90 00", "02 58 00 02 d0 00 02 f8 00 03 18 00 00 00 00");
        assertEquals(49, t35.length);
        assertArrayEquals(expected, t35);
    }

    @Test
    public void anHevcFrameWithoutHdr10PlusBeforeItsFirstSliceHasNone() {
        assertNull(hevc(HEVC_VIVID, HEVC_SLICE));
        assertNull(hevc(HEVC_SLICE, HEVC_SEI));
        assertNull(hevc(HEVC_AUD));
        assertNull(Hdr10PlusSei.hevc(new byte[0], 0));
        // Cut short inside the message: its size runs past the end.
        byte[] sei = bytes(HEVC_SEI);
        assertNull(Hdr10PlusSei.hevc(sei, 30));
    }

    @Test
    public void anAv1TemporalUnitGivesTheHostsT35Payload() {
        assertArrayEquals(bytes(T35), av1(AV1_TEMPORAL_DELIMITER, AV1_SEQUENCE_HEADER, AV1_OBU, AV1_FRAME));
        // The payload's own trailing zeros stay; zeros after the trailing one bit go.
        byte[] padded = bytes("2a 35 04", T35, "80 00 00");
        assertArrayEquals(bytes(T35), Hdr10PlusSei.av1(padded, padded.length));
        // An OBU with an extension header, and a last OBU without a size field.
        byte[] extended = bytes("2e 00 33 04", T35, "80");
        assertArrayEquals(bytes(T35), Hdr10PlusSei.av1(extended, extended.length));
        byte[] unsized = bytes("28 04", T35, "80");
        assertArrayEquals(bytes(T35), Hdr10PlusSei.av1(unsized, unsized.length));
    }

    @Test
    public void anAv1TemporalUnitWithoutHdr10PlusBeforeItsFrameHasNone() {
        assertNull(av1(AV1_TEMPORAL_DELIMITER, AV1_SEQUENCE_HEADER, AV1_FRAME, AV1_OBU));
        // Another provider's T.35 message (0x003B, Dolby), and another metadata type (HDR CLL, 1).
        byte[] dolby = bytes("2a 0a 04 b5 00 3b 00 00 08 00 01 80");
        assertNull(Hdr10PlusSei.av1(dolby, dolby.length));
        byte[] cll = bytes("2a 06 01 03 e8 01 90 80", AV1_FRAME);
        assertNull(Hdr10PlusSei.av1(cll, cll.length));
        // A size past the end, the forbidden bit, and no trailing one bit.
        byte[] truncated = bytes(AV1_OBU);
        assertNull(Hdr10PlusSei.av1(truncated, 20));
        byte[] forbidden = bytes("aa 33 04", T35, "80");
        assertNull(Hdr10PlusSei.av1(forbidden, forbidden.length));
        byte[] noTrailing = bytes("2a 32 04", T35);
        assertNull(Hdr10PlusSei.av1(noTrailing, noTrailing.length));
    }
}
