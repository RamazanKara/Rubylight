package com.limelight.binding.input;

import org.junit.Test;

import static org.junit.Assert.*;

public class MouseDeltaAccumulatorTest {
    @Test
    public void smallTouchSamplesPreserveTheScaledDistance() {
        MouseDeltaAccumulator accumulator = new MouseDeltaAccumulator();
        int total = 0;
        for (int i = 0; i < 100; i++) {
            total += accumulator.scale(3, 0.45f);
        }
        assertEquals(135, total);
        assertEquals(new MouseDeltaAccumulator().scale(300, 0.45f), total);
    }

    @Test
    public void remainderSurvivesBothZeroAndNonzeroOutput() {
        MouseDeltaAccumulator accumulator = new MouseDeltaAccumulator();
        assertEquals(1, accumulator.scale(3, 0.45f));
        assertEquals(2, accumulator.scale(3, 0.45f));
        assertEquals(0, accumulator.scale(0.25f, 1));
        assertEquals(0, accumulator.scale(0.25f, 1));
        assertEquals(0, accumulator.scale(0.25f, 1));
        assertEquals(1, accumulator.scale(0.25f, 1));
    }

    @Test
    public void sensitivityScalesTouchAndMouseMovement() {
        for (int speed : new int[] {25, 100, 200, 400}) {
            MouseDeltaAccumulator touch = new MouseDeltaAccumulator();
            MouseDeltaAccumulator mouse = new MouseDeltaAccumulator();
            int touchTotal = 0;
            int mouseTotal = 0;
            for (int i = 0; i < 100; i++) {
                touchTotal += touch.scale(3, 0.45f * speed / 100f);
                mouseTotal += mouse.scale(0.25f, speed / 100f);
            }
            assertEquals(Math.round(135 * speed / 100f), touchTotal);
            assertEquals(Math.round(25 * speed / 100f), mouseTotal);
        }
    }

    @Test
    public void negativeMovesKeepTheirRemainder() {
        MouseDeltaAccumulator accumulator = new MouseDeltaAccumulator();
        int total = 0;
        for (int i = 0; i < 100; i++) {
            total += accumulator.scale(-3, 0.45f);
        }
        assertEquals(-135, total);
        for (int i = 0; i < 100; i++) {
            total += accumulator.scale(3, 0.45f);
        }
        assertEquals(0, total);
    }

    @Test
    public void reversingFractionalMouseMovesDoesNotDrift() {
        MouseDeltaAccumulator accumulator = new MouseDeltaAccumulator();
        int total = 0;
        for (int i = 0; i < 100; i++) {
            total += accumulator.scale(0.25f, 1);
            total += accumulator.scale(-0.25f, 1);
        }
        assertEquals(0, total);
        assertEquals(0, accumulator.scale(0, 1));
    }

    @Test
    public void fullWidthSwipeCoversTheStreamAtDefaultSpeed() {
        for (int viewWidth : new int[] {2400, 3120}) {
            for (int streamWidth : new int[] {1920, 3840}) {
                MouseDeltaAccumulator accumulator = new MouseDeltaAccumulator();
                int total = 0;
                for (int x = 0; x < viewWidth; x += 3) {
                    total += accumulator.scale(3, streamWidth / (float)viewWidth);
                }
                assertEquals(streamWidth, total);
            }
        }
    }
}
