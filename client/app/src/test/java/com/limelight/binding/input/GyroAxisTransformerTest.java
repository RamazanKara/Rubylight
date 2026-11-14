package com.limelight.binding.input;

import static org.junit.Assert.assertArrayEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertSame;
import static org.junit.Assert.assertThrows;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

/** After jvanderv42's Nightjar fork (GPL-3.0), GyroAxisTransformerTest. */
public class GyroAxisTransformerTest {
    @Test
    public void identityLeavesValuesUnchangedAndSaysSo() {
        assertTrue(GyroAxisTransformer.identity().isIdentity());
        assertTrue(new GyroAxisTransformer(0, 1, 2, 1, 1, 1).isIdentity());
        assertArrayEquals(new float[] {1f, -2f, 3f},
                GyroAxisTransformer.identity().transform(1f, -2f, 3f), 0f);
    }

    @Test
    public void swapsAxesAndInvertsSelectedDirections() {
        GyroAxisTransformer transformer = new GyroAxisTransformer(1, 0, 2, -1, 1, -1);
        assertFalse(transformer.isIdentity());
        assertArrayEquals(new float[] {2f, 1f, -3f},
                transformer.transform(1f, -2f, 3f), 0f);
    }

    @Test
    public void sensitivityScalesPitchAndYawButNotRoll() {
        GyroAxisTransformer transformer = new GyroAxisTransformer(0, 1, 2, 1, 1, 1, 2f, 0.5f);
        assertFalse("a sensitivity other than 1x is not the identity", transformer.isIdentity());
        // X is pitch (vertical), Y is yaw (horizontal), Z is roll.
        assertArrayEquals(new float[] {0.5f, 4f, 3f}, transformer.transform(1f, 2f, 3f), 0f);
    }

    @Test
    public void theReusedBufferHoldsOnlyTheLatestSample() {
        GyroAxisTransformer transformer = GyroMappingPreferences.FLYDIGI_TRANSFORM;
        float[] out = new float[3];
        transformer.transform(1f, 2f, 3f, out);
        assertArrayEquals(new float[] {-2f, 3f, 1f}, out, 0f);
        transformer.transform(4f, 5f, 6f, out);
        assertArrayEquals(new float[] {-5f, 6f, 4f}, out, 0f);
    }

    @Test
    public void identityIsShared() {
        assertSame(GyroAxisTransformer.identity(), GyroAxisTransformer.identity());
    }

    @Test
    public void rejectsDuplicateAxes() {
        assertThrows(IllegalArgumentException.class,
                () -> new GyroAxisTransformer(0, 0, 2, 1, 1, 1));
    }

    @Test
    public void rejectsInvalidSign() {
        assertThrows(IllegalArgumentException.class,
                () -> new GyroAxisTransformer(0, 1, 2, 1, 0, 1));
    }

    @Test
    public void rejectsSensitivityOutsideTheRange() {
        assertThrows(IllegalArgumentException.class,
                () -> new GyroAxisTransformer(0, 1, 2, 1, 1, 1, 0.05f, 1f));
        assertThrows(IllegalArgumentException.class,
                () -> new GyroAxisTransformer(0, 1, 2, 1, 1, 1, 1f, Float.POSITIVE_INFINITY));
    }
}
