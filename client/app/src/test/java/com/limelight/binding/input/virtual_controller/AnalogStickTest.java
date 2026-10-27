package com.limelight.binding.input.virtual_controller;

import org.junit.Test;

import java.lang.reflect.Method;

import static org.junit.Assert.*;

public class AnalogStickTest {
    private double angle(float x, float y) throws Exception {
        Method method = AnalogStick.class.getDeclaredMethod("getAngle", float.class, float.class);
        method.setAccessible(true);
        return (double) method.invoke(null, x, y);
    }

    @Test
    public void cardinalDirectionsAgreeWithNearbyDiagonalMovement() throws Exception {
        for (float y : new float[] {-100, 100}) {
            double vertical = angle(0, y);
            for (float x : new float[] {-0.001f, 0.001f}) {
                assertEquals(Math.cos(angle(x, y)), Math.cos(vertical), 0.00001);
            }
        }
        assertEquals(0, angle(0, -100), 0);
        assertEquals(Math.PI, angle(0, 100), 0);
        assertEquals(Math.PI / 2, angle(-100, 0), 0);
        assertEquals(3 * Math.PI / 2, angle(100, 0), 0);
    }
}
