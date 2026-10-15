package com.limelight.binding.input;

import org.junit.Test;

import static org.junit.Assert.assertEquals;

public class FixedStepMouseTest {
    @Test
    public void yDoublesTheStepAndWrapsAfter128() {
        int step = FixedStepMouse.MIN_STEP;
        int[] expected = {2, 4, 8, 16, 32, 64, 128, 1, 2};
        for (int next : expected) {
            step = FixedStepMouse.nextStep(step);
            assertEquals(next, step);
        }
    }

    @Test
    public void anOddOrOutOfRangeStepRecovers() {
        assertEquals(1, FixedStepMouse.nextStep(0));
        assertEquals(1, FixedStepMouse.nextStep(-4));
        assertEquals(1, FixedStepMouse.nextStep(300));
        assertEquals(8, FixedStepMouse.nextStep(5));
    }

    @Test
    public void eachAxisMovesExactlyOneStepInTheStickDirection() {
        assertEquals(16, FixedStepMouse.delta(3.7, 16));
        assertEquals(-16, FixedStepMouse.delta(-1.0, 16));
        assertEquals(1, FixedStepMouse.delta(4.0, 1));
    }

    @Test
    public void aComponentUnderOnePixelStaysStill() {
        assertEquals(0, FixedStepMouse.delta(0.99, 64));
        assertEquals(0, FixedStepMouse.delta(-0.5, 64));
        assertEquals(0, FixedStepMouse.delta(0, 64));
    }
}
