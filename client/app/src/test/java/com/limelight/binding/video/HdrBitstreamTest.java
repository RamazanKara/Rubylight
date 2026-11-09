package com.limelight.binding.video;

import static org.junit.Assert.*;

import com.limelight.nvstream.jni.MoonBridge;

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

    @Test
    public void onlyHevcDolbyVisionFramesEndAtTheRpu() {
        // Profile 8: the RPU NAL unit comes last, then the FEC padding.
        assertTrue(HdrBitstream.endsAtRpu(MoonBridge.VIDEO_FORMAT_H265_MAIN10));
        assertTrue(HdrBitstream.endsAtRpu(MoonBridge.VIDEO_FORMAT_H265));
        // Profile 10: the RPU OBU comes before the frame, and a zero byte can end an AV1 tile.
        assertFalse(HdrBitstream.endsAtRpu(MoonBridge.VIDEO_FORMAT_AV1_MAIN10));
        assertFalse(HdrBitstream.endsAtRpu(MoonBridge.VIDEO_FORMAT_AV1_MAIN8));
        assertFalse(HdrBitstream.endsAtRpu(MoonBridge.VIDEO_FORMAT_H264));
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
        int hevc = MoonBridge.VIDEO_FORMAT_H265_MAIN10;
        int av1 = MoonBridge.VIDEO_FORMAT_AV1_MAIN10;
        assertTrue(HdrBitstream.dropsVivid("c2.mtk.hevc.decoder", hevc));
        assertTrue(HdrBitstream.dropsVivid("c2.mtk.hevc.decoder", MoonBridge.VIDEO_FORMAT_H265));
        assertTrue(HdrBitstream.dropsVivid("c2.mtk.dvhe.st.decoder", hevc));
        assertTrue(HdrBitstream.dropsVivid("OMX.MTK.VIDEO.DECODER.HEVC", hevc));
        // AV1 has no Vivid carriage, and its OBUs are not walked for start codes: not through the
        // AV1 decoder, nor through the Dolby Vision profile 10 one (same MIME type as profile 8).
        assertFalse(HdrBitstream.dropsVivid("c2.mtk.av1.decoder", av1));
        assertFalse(HdrBitstream.dropsVivid("c2.mtk.dvav1.10.decoder", av1));
        assertFalse(HdrBitstream.dropsVivid("c2.mtk.avc.decoder", MoonBridge.VIDEO_FORMAT_H264));
        for (String other : new String[] {"c2.qti.hevc.decoder", "OMX.amlogic.hevc.decoder.awesome2",
                "c2.exynos.hevc.decoder", "c2.android.hevc.decoder", "OMX.Nvidia.h265.decode"}) {
            assertFalse(other, HdrBitstream.dropsVivid(other, hevc));
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

    private static java.nio.ByteBuffer direct(byte[] data, int extra) {
        java.nio.ByteBuffer direct = java.nio.ByteBuffer.allocateDirect(data.length + extra);
        direct.put(data);
        // Bytes past the decode unit's length are an older frame's.
        for (int i = 0; i < extra; i++) direct.put((byte) 0x55);
        return direct;
    }

    @Test
    public void aFrameHandedOverInPlaceQueuesTheSameBytesAsItsArray() {
        byte[] hdr10Plus = t35Sei(HDR10_PLUS, 20);
        byte[] vivid = t35Sei(VIVID, 300);
        byte[][] frames = {
                concat(AUD, slice()),
                accessUnit(1391),
                concat(AUD, hdr10Plus, vivid, slice()),
                concat(AUD, hdr10Plus, vivid, slice(), rpu(), new byte[1391]),
                concat(vivid, slice(), rpu()),
                concat(AUD, t35Sei(VIVID, 10)),
        };
        for (byte[] frame : frames) {
            for (boolean dolbyVision : new boolean[] {false, true}) {
                for (boolean dropVivid : new boolean[] {false, true}) {
                    java.nio.ByteBuffer direct = direct(frame, 50);
                    int position = direct.position();
                    int limit = direct.limit();

                    int length = HdrBitstream.queuedLength(dolbyVision, frame, frame.length);
                    assertEquals(length, HdrBitstream.queuedLength(dolbyVision, direct, frame.length));
                    assertEquals(HdrBitstream.tail(frame, length, 8), HdrBitstream.tail(direct, length, 8));
                    int[] arrayRange = new int[2];
                    int[] directRange = new int[2];
                    int[] drop = dropVivid && HdrBitstream.findVividSei(frame, length, arrayRange) ? arrayRange : null;
                    assertEquals(drop != null, dropVivid && HdrBitstream.findVividSei(direct, length, directRange));
                    if (drop != null) assertArrayEquals(arrayRange, directRange);

                    java.nio.ByteBuffer fromArray = java.nio.ByteBuffer.allocate(8192);
                    java.nio.ByteBuffer fromDirect = java.nio.ByteBuffer.allocate(8192);
                    fromArray.put(CSD);
                    fromDirect.put(CSD);
                    HdrBitstream.put(fromArray, frame, length, drop);
                    HdrBitstream.put(fromDirect, direct, length, drop);
                    assertEquals(CSD.length + HdrBitstream.putLength(length, drop), fromDirect.position());
                    assertArrayEquals(java.util.Arrays.copyOf(fromArray.array(), fromArray.position()),
                            java.util.Arrays.copyOf(fromDirect.array(), fromDirect.position()));
                    assertEquals(position, direct.position());
                    assertEquals(limit, direct.limit());
                }
            }
        }
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
