package com.limelight.binding.video;

import org.junit.Test;
import static org.junit.Assert.*;

public class SgsrConstantsTest {
    @Test
    public void viewportMatchesQualcommInputSizeAndReciprocals() {
        int[] expected = {0x3a4ccccd, 0x3ab60b61, 0x44a00000, 0x44340000};
        float[] actual = SgsrConstants.viewport(1280, 720);
        for (int i = 0; i < expected.length; i++) {
            assertEquals("constant " + i, expected[i], Float.floatToIntBits(actual[i]));
        }
        assertArrayEquals(new float[] {1f / 720, 1f / 1280, 720, 1280},
                SgsrConstants.viewport(720, 1280), 0f);
        assertArrayEquals(new float[] {1f / 1920, 1f / 1080, 1920, 1080},
                SgsrConstants.viewport(1920, 1080), 0f);
        assertArrayEquals(new float[] {1, 1, 1, 1}, SgsrConstants.viewport(1, 1), 0f);
    }

    @Test
    public void texelCentersRoundTripBeforeTheSurfaceTextureTransform() {
        float[] c = SgsrConstants.viewport(1280, 720);
        for (int axis = 0; axis < 2; axis++) {
            for (float pixel : new float[] {0, 32, c[axis + 2] - 1}) {
                float uv = (pixel + 0.5f) * c[axis];
                assertEquals(pixel, uv * c[axis + 2] - 0.5f, 0.0001f);
            }
        }
    }

    @Test
    public void sharpnessMapsLinearlyToTheDocumentedEdgeRange() {
        assertEquals(1f, SgsrConstants.edgeSharpness(0), 0f);
        assertEquals(1.25f, SgsrConstants.edgeSharpness(25), 0f);
        assertEquals(1.5f, SgsrConstants.edgeSharpness(50), 0f);
        assertEquals(2f, SgsrConstants.edgeSharpness(100), 0f);
        assertThrows(IllegalArgumentException.class, () -> SgsrConstants.edgeSharpness(-1));
        assertThrows(IllegalArgumentException.class, () -> SgsrConstants.edgeSharpness(101));
        assertThrows(IllegalArgumentException.class, () -> SgsrConstants.viewport(0, 720));
        assertThrows(IllegalArgumentException.class, () -> SgsrConstants.viewport(1280, -1));
    }
}
