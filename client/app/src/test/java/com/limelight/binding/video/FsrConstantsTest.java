package com.limelight.binding.video;

import org.junit.Test;
import static org.junit.Assert.*;

public class FsrConstantsTest {
    @Test
    public void easuMatchesAmdGoldenConstantsFor720pTo1440p() {
        // FsrEasuCon(1280, 720, 1280, 720, 2560, 1440), IEEE 754 words.
        int[] expected = {
                0x3f000000, 0x3f000000, 0xbe800000, 0xbe800000,
                0x3a4ccccd, 0x3ab60b61, 0x3a4ccccd, 0xbab60b61,
                0xba4ccccd, 0x3b360b61, 0x3a4ccccd, 0x3b360b61,
                0x00000000, 0x3bb60b61, 0x00000000, 0x00000000
        };
        float[] actual = FsrConstants.easu(1280, 720, 1280, 720, 2560, 1440);
        for (int i = 0; i < expected.length; i++) assertEquals("constant " + i, expected[i], Float.floatToIntBits(actual[i]));
    }

    @Test
    public void viewportScalingAndPaddedTextureCoordinatesAreIndependent() {
        float[] constants = FsrConstants.easu(1920, 1080, 2048, 1088, 2560, 1440);
        assertEquals(0.75f, constants[0], 0f);
        assertEquals(0.75f, constants[1], 0f);
        assertEquals(-0.125f, constants[2], 0f);
        assertEquals(-0.125f, constants[3], 0f);
        assertEquals(1f / 2048, constants[4], 0f);
        assertEquals(1f / 1088, constants[5], 0f);
        float[] nativeSize = FsrConstants.easu(1920, 1080, 1920, 1080, 1920, 1080);
        assertEquals(1f, nativeSize[0], 0.0000001f);
        assertEquals(0f, nativeSize[2], 0.0000001f);
    }

    @Test
    public void pixelCentersAndGatherOffsetsMatchTheTwelveTapFootprint() {
        float[] c = FsrConstants.easu(1280, 720, 1280, 720, 1920, 1080);
        assertEquals(-1f / 6, c[2], 0.0000001f);
        assertEquals(1279f + 1f / 6, 1919 * c[0] + c[2], 0.0002f);
        assertEquals(-1f / 720, c[7], 0f);
        assertEquals(-1f / 1280, c[8], 0f);
        assertEquals(2f / 720, c[9], 0f);
        assertEquals(4f / 720, c[13], 0f);
    }

    @Test
    public void rcasLinearStrengthMatchesAmdStopsAndHasAnExactZeroBypass() {
        assertEquals(0f, FsrConstants.rcas(0), 0f);
        assertEquals((float) Math.pow(2, -2), FsrConstants.rcas(25), 0f);
        assertEquals((float) Math.pow(2, -1), FsrConstants.rcas(50), 0f);
        assertEquals((float) Math.pow(2, 0), FsrConstants.rcas(100), 0f);
        assertThrows(IllegalArgumentException.class, () -> FsrConstants.rcas(-1));
        assertThrows(IllegalArgumentException.class, () -> FsrConstants.rcas(101));
        assertThrows(IllegalArgumentException.class, () -> FsrConstants.easu(1280, 720, 640, 720, 1920, 1080));
        assertThrows(IllegalArgumentException.class, () -> FsrConstants.easu(1280, 720, 1280, 720, 0, 1080));
    }
}
