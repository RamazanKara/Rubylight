package com.limelight.binding.video;

import static org.junit.Assert.*;

import org.junit.Test;

import java.io.ByteArrayOutputStream;

/** Made-up HEVC access units shaped like the Rubylight host's: slice, then the RPU, then FEC padding. */
public class HdrBitstreamTest {
    /** NAL type 62, layer 0, TemporalId 0; an RPU ends with 0x80. */
    private static byte[] rpu() {
        ByteArrayOutputStream nal = new ByteArrayOutputStream();
        nal.write(new byte[] {0, 0, 0, 1, (byte) (62 << 1), 0x01, 0x19}, 0, 7);
        for (int i = 0; i < 120; i++) nal.write((i % 233) | 1);
        nal.write(0x80);
        return nal.toByteArray();
    }

    /** A TRAIL_R slice ending with a zero byte and its 0x03, then the RPU, then {@code padding} zeros. */
    private static byte[] accessUnit(int padding) {
        ByteArrayOutputStream au = new ByteArrayOutputStream();
        au.write(new byte[] {0, 0, 0, 1, 1 << 1, 0x01}, 0, 6);
        for (int i = 0; i < 3000; i++) au.write((i % 249) | 1);
        au.write(new byte[] {0, 0, 3}, 0, 3);
        byte[] rpu = rpu();
        au.write(rpu, 0, rpu.length);
        au.write(new byte[padding], 0, padding);
        return au.toByteArray();
    }

    @Test
    public void aDolbyVisionFrameEndsAtTheRpuStopByte() {
        int unpadded = accessUnit(0).length;
        for (int padding : new int[] {1, 7, 1391}) {
            byte[] au = accessUnit(padding);
            int length = HdrBitstream.queuedLength(true, au, au.length);
            assertEquals(unpadded, length);
            assertEquals((byte) 0x80, au[length - 1]);
        }
    }

    @Test
    public void aDolbyVisionFrameWithoutPaddingIsWhole() {
        byte[] au = accessUnit(0);
        assertEquals(au.length, HdrBitstream.queuedLength(true, au, au.length));
    }

    @Test
    public void onlyTheGivenLengthCounts() {
        // The bridge's buffer is reused: bytes past the decode unit's length are an older frame's.
        byte[] buffer = new byte[8192];
        byte[] au = accessUnit(300);
        System.arraycopy(au, 0, buffer, 0, au.length);
        buffer[au.length] = 0x55;
        assertEquals(accessUnit(0).length, HdrBitstream.queuedLength(true, buffer, au.length));
    }

    @Test
    public void aFrameThatIsNotDolbyVisionIsUntouched() {
        byte[] au = accessUnit(1391);
        assertEquals(au.length, HdrBitstream.queuedLength(false, au, au.length));
    }

    /** A prefix SEI NAL unit holding one T.35 message, as rust/core/src/hdr_dynamic.rs hevc_t35_sei writes it. */
    private static byte[] t35Sei(int[] header, int bodyLength) {
        ByteArrayOutputStream nal = new ByteArrayOutputStream();
        nal.write(new byte[] {0, 0, 0, 1, (byte) (39 << 1), 0x01, 0x04}, 0, 7);
        int size = header.length + bodyLength;
        for (; size >= 255; size -= 255) nal.write(0xFF);
        nal.write(size);
        for (int b : header) nal.write(b);
        for (int i = 0; i < bodyLength; i++) nal.write((i % 241) | 1);
        nal.write(0x80);
        return nal.toByteArray();
    }

    private static final int[] HDR10_PLUS = {0xB5, 0x00, 0x3C, 0x00, 0x01, 4, 1};
    private static final int[] VIVID = {0x26, 0x00, 0x04, 0x00, 0x05, 0x01};

    private static byte[] concat(byte[]... parts) {
        ByteArrayOutputStream out = new ByteArrayOutputStream();
        for (byte[] part : parts) out.write(part, 0, part.length);
        return out.toByteArray();
    }

    private static byte[] slice() {
        byte[] au = accessUnit(0);
        return java.util.Arrays.copyOf(au, 6 + 3000 + 3);
    }

    private static final byte[] AUD = {0, 0, 0, 1, (byte) (35 << 1), 0x01, 0x50};

    @Test
    public void theVividSeiIsFoundBetweenTheHdr10PlusSeiAndTheSlice() {
        byte[] hdr10Plus = t35Sei(HDR10_PLUS, 20);
        byte[] vivid = t35Sei(VIVID, 300);
        byte[] au = concat(AUD, hdr10Plus, vivid, slice(), rpu());
        int[] range = new int[2];
        assertTrue(HdrBitstream.findVividSei(au, au.length, range));
        assertEquals(AUD.length + hdr10Plus.length, range[0]);
        assertEquals(AUD.length + hdr10Plus.length + vivid.length, range[1]);
        // What is left is the access unit without it.
        byte[] left = concat(java.util.Arrays.copyOfRange(au, 0, range[0]), java.util.Arrays.copyOfRange(au, range[1], au.length));
        assertArrayEquals(concat(AUD, hdr10Plus, slice(), rpu()), left);
    }

    @Test
    public void aVividSeiWithoutAnythingBeforeItIsFound() {
        byte[] vivid = t35Sei(VIVID, 10);
        byte[] au = concat(vivid, slice());
        int[] range = new int[2];
        assertTrue(HdrBitstream.findVividSei(au, au.length, range));
        assertEquals(0, range[0]);
        assertEquals(vivid.length, range[1]);
    }

    @Test
    public void aFrameWithoutVividHasNoneToFind() {
        int[] range = {-7, -7};
        byte[] hdr10PlusOnly = concat(AUD, t35Sei(HDR10_PLUS, 20), slice());
        assertFalse(HdrBitstream.findVividSei(hdr10PlusOnly, hdr10PlusOnly.length, range));
        byte[] sdr = concat(AUD, slice());
        assertFalse(HdrBitstream.findVividSei(sdr, sdr.length, range));
        assertArrayEquals(new int[] {-7, -7}, range);
    }

    @Test
    public void aVividSeiAfterTheFirstSliceIsNotLookedFor() {
        // Only the units before the picture are the host's metadata; a slice may hold the same bytes.
        byte[] au = concat(slice(), t35Sei(VIVID, 10));
        assertFalse(HdrBitstream.findVividSei(au, au.length, new int[2]));
    }

    @Test
    public void aLongVividSeiHasAMultiByteSize() {
        byte[] vivid = t35Sei(VIVID, 600);
        byte[] au = concat(AUD, vivid, slice());
        int[] range = new int[2];
        assertTrue(HdrBitstream.findVividSei(au, au.length, range));
        assertEquals(AUD.length + vivid.length, range[1]);
    }

    @Test
    public void aTruncatedSeiIsNotVivid() {
        byte[] vivid = t35Sei(VIVID, 10);
        for (int cut = 4; cut < 12; cut++) {
            assertFalse(HdrBitstream.findVividSei(vivid, cut, new int[2]));
        }
    }

    @Test
    public void onlyMediaTekHevcAndDolbyVisionDecodersLoseVivid() {
        assertTrue(HdrBitstream.dropsVivid("c2.mtk.hevc.decoder", "video/hevc"));
        assertTrue(HdrBitstream.dropsVivid("c2.mtk.dvhe.st.decoder", HdrFormatPolicy.MIME_DOLBY_VISION));
        assertTrue(HdrBitstream.dropsVivid("OMX.MTK.VIDEO.DECODER.HEVC", "video/hevc"));
        // AV1 has no Vivid carriage, and its OBUs are not walked for start codes.
        assertFalse(HdrBitstream.dropsVivid("c2.mtk.av1.decoder", "video/av01"));
        assertFalse(HdrBitstream.dropsVivid("c2.mtk.avc.decoder", "video/avc"));
        for (String other : new String[] {"c2.qti.hevc.decoder", "OMX.amlogic.hevc.decoder.awesome2",
                "c2.exynos.hevc.decoder", "c2.android.hevc.decoder", "OMX.Nvidia.h265.decode"}) {
            assertFalse(other, HdrBitstream.dropsVivid(other, "video/hevc"));
        }
    }

    /** A VPS and an SPS, as a fused IDR frame puts them first. */
    private static final byte[] CSD = {0, 0, 0, 1, 0x40, 0x01, 0x0C, 0x01, 0, 0, 0, 1, 0x42, 0x01, 0x01, 0x01};

    @Test
    public void theDecoderGetsTheFrameAfterItsParameterSetsWithoutVividOrPadding() {
        byte[] hdr10Plus = t35Sei(HDR10_PLUS, 20);
        byte[] vivid = t35Sei(VIVID, 300);
        byte[] padded = concat(AUD, hdr10Plus, vivid, slice(), rpu(), new byte[1391]);
        // As submitDecodeUnit: the Dolby Vision trim, then the Vivid SEI in what is left of the frame.
        int length = HdrBitstream.queuedLength(true, padded, padded.length);
        int[] range = new int[2];
        assertTrue(HdrBitstream.findVividSei(padded, length, range));
        byte[] queued = concat(AUD, hdr10Plus, slice(), rpu());
        assertEquals(queued.length, HdrBitstream.putLength(length, range));

        java.nio.ByteBuffer buffer = java.nio.ByteBuffer.allocate(8192);
        buffer.put(CSD);
        HdrBitstream.put(buffer, padded, length, range);
        assertEquals(CSD.length + queued.length, buffer.position());
        assertArrayEquals(concat(CSD, queued), java.util.Arrays.copyOf(buffer.array(), buffer.position()));
    }

    @Test
    public void aVividSeiLastInWhatIsQueuedLeavesNothingAfterIt() {
        byte[] au = concat(AUD, t35Sei(VIVID, 10));
        int[] range = new int[2];
        assertTrue(HdrBitstream.findVividSei(au, au.length, range));
        assertEquals(au.length, range[1]);
        assertEquals(AUD.length, HdrBitstream.putLength(au.length, range));
        java.nio.ByteBuffer buffer = java.nio.ByteBuffer.allocate(64);
        HdrBitstream.put(buffer, au, au.length, range);
        assertArrayEquals(AUD, java.util.Arrays.copyOf(buffer.array(), buffer.position()));
    }

    @Test
    public void aFrameWithNothingToDropIsPutAsQueued() {
        // The bridge's buffer is reused: bytes past the decode unit's length are an older frame's.
        byte[] au = concat(AUD, slice());
        byte[] reused = java.util.Arrays.copyOf(au, au.length + 50);
        reused[au.length] = 0x55;
        assertEquals(au.length, HdrBitstream.putLength(au.length, null));
        java.nio.ByteBuffer buffer = java.nio.ByteBuffer.allocate(8192);
        buffer.put(CSD);
        HdrBitstream.put(buffer, reused, au.length, null);
        assertArrayEquals(concat(CSD, au), java.util.Arrays.copyOf(buffer.array(), buffer.position()));
    }

    @Test
    public void theLoggedTailEndsAtTheRpuStopByte() {
        byte[] au = accessUnit(5);
        int length = HdrBitstream.queuedLength(true, au, au.length);
        String tail = HdrBitstream.tail(au, length, 8);
        assertEquals(8 * 3 - 1, tail.length());
        assertTrue(tail.endsWith(" 80"));
    }
}
