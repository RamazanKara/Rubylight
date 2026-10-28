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

    @Test
    public void theLoggedTailEndsAtTheRpuStopByte() {
        byte[] au = accessUnit(5);
        int length = HdrBitstream.queuedLength(true, au, au.length);
        String tail = HdrBitstream.tail(au, length, 8);
        assertEquals(8 * 3 - 1, tail.length());
        assertTrue(tail.endsWith(" 80"));
    }
}
