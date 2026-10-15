package com.limelight.binding.input.touch;

import org.junit.Test;

import static org.junit.Assert.*;

public class PointerAccelerationTest {
    private static final String[] CURVES = {PointerAcceleration.GENTLE, PointerAcceleration.MEDIUM, PointerAcceleration.STRONG};

    @Test
    public void gainNeverDropsAsTheFingerSpeedsUp() {
        for (String curve : CURVES) {
            PointerAcceleration acceleration = PointerAcceleration.forCurve(curve);
            float previous = acceleration.gain(0);
            for (float speed = 0; speed <= 3; speed += 0.01f) {
                float gain = acceleration.gain(speed);
                assertTrue(curve + " at " + speed, gain >= previous - 1e-6f);
                previous = gain;
            }
        }
    }

    @Test
    public void slowMovesArePreciseAndFastMovesTravelFarther() {
        for (String curve : CURVES) {
            PointerAcceleration acceleration = PointerAcceleration.forCurve(curve);
            assertTrue(curve, acceleration.gain(0) < 1);
            assertTrue(curve, acceleration.gain(10) > 1);
            assertEquals(curve, acceleration.gain(PointerAcceleration.FAST_SPEED), acceleration.gain(100), 0);
            assertEquals(curve, acceleration.gain(0), acceleration.gain(PointerAcceleration.SLOW_SPEED), 0);
        }
    }

    @Test
    public void strongerCurvesReachFurther() {
        float gentle = PointerAcceleration.forCurve(PointerAcceleration.GENTLE).gain(5);
        float medium = PointerAcceleration.forCurve(PointerAcceleration.MEDIUM).gain(5);
        float strong = PointerAcceleration.forCurve(PointerAcceleration.STRONG).gain(5);
        assertTrue(gentle < medium && medium < strong);
    }

    @Test
    public void rampHasNoJumps() {
        PointerAcceleration acceleration = PointerAcceleration.forCurve(PointerAcceleration.STRONG);
        for (float speed = 0; speed <= 2; speed += 0.001f) {
            assertTrue(Math.abs(acceleration.gain(speed + 0.001f) - acceleration.gain(speed)) < 0.01f);
        }
    }

    @Test
    public void unknownCurveIsMediumAndBadSpeedsAreSlow() {
        assertEquals(PointerAcceleration.forCurve(PointerAcceleration.MEDIUM).gain(1),
                PointerAcceleration.forCurve(null).gain(1), 0);
        PointerAcceleration acceleration = PointerAcceleration.forCurve("unknown");
        assertEquals(acceleration.gain(0), acceleration.gain(Float.NaN), 0);
        assertEquals(acceleration.gain(0), acceleration.gain(-5), 0);
    }

    @Test
    public void speedIsInDpPerMillisecondWithAFloorOnTheInterval() {
        assertEquals(1f, PointerAcceleration.speed(30, 40, 25, 2), 1e-6f);
        // Same-millisecond samples count as 4 ms rather than dividing by zero.
        assertEquals(2.5f, PointerAcceleration.speed(10, 0, 0, 1), 1e-6f);
        assertEquals(10f, PointerAcceleration.speed(40, 0, 4, 0), 1e-6f);
    }
}
