package com.limelight.binding.input;

import com.limelight.utils.Vector2d;

import org.junit.Test;

import static org.junit.Assert.*;

public class StickDeadzoneTest {
    private static Vector2d apply(float x, float y, float radius) {
        Vector2d v = new Vector2d();
        v.initialize(x, y);
        ControllerHandler.applyStickDeadzone(v, radius);
        return v;
    }

    @Test
    public void positiveDeadzoneDropsInputInsideTheRadiusAndKeepsTheRestRaw() {
        assertEquals(0, apply(0.05f, 0.05f, 0.1f).getMagnitude(), 0);
        Vector2d outside = apply(0.3f, 0.4f, 0.1f);
        assertEquals(0.3f, outside.getX(), 1e-6);
        assertEquals(0.4f, outside.getY(), 1e-6);
    }

    @Test
    public void negativeDeadzoneRemapsSmallInputPastTheHostDeadzone() {
        // -20%: a 10% push becomes 0.2 + 0.1 * 0.8 = 28%
        Vector2d v = apply(0.1f, 0, -0.2f);
        assertEquals(0.28f, v.getX(), 1e-6);
        assertEquals(0f, v.getY(), 0);
    }

    @Test
    public void negativeDeadzoneKeepsTheDirectionAndFullDeflection() {
        Vector2d v = apply(0.06f, -0.08f, -0.1f);
        double expected = 0.1 + 0.1 * 0.9;
        assertEquals(expected, v.getMagnitude(), 1e-6);
        assertEquals(0.6, v.getX() / v.getMagnitude(), 1e-6);
        assertEquals(-0.8, v.getY() / v.getMagnitude(), 1e-6);
        assertEquals(1f, apply(0, 1f, -0.2f).getY(), 0);
        assertEquals(1f, apply(-1f, 0, -0.2f).getX() * -1, 0);
    }

    @Test
    public void negativeDeadzoneIsMonotonicAndNeverShrinksInput() {
        double previous = 0;
        for (int i = 2; i <= 100; i++) {
            double m = apply(i / 100f, 0, -0.15f).getMagnitude();
            assertTrue(m >= i / 100.0 - 1e-6);
            assertTrue(m >= previous);
            assertTrue(m <= 1 + 1e-6);
            previous = m;
        }
    }

    @Test
    public void zeroAndNegativeKeepAOnePercentCentreZone() {
        for (float radius : new float[] {0f, -0.05f, -0.2f}) {
            assertEquals(0, apply(0.005f, -0.005f, radius).getMagnitude(), 0);
            assertEquals(0, apply(0.01f, 0, radius).getMagnitude(), 0);
        }
    }

    @Test
    public void zeroDeadzonePassesEverythingAboveTheCentreZoneThrough() {
        Vector2d v = apply(0.02f, 0.5f, 0f);
        assertEquals(0.02f, v.getX(), 1e-6);
        assertEquals(0.5f, v.getY(), 1e-6);
    }
}
