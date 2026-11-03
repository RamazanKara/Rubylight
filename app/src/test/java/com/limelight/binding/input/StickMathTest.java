package com.limelight.binding.input;

import org.junit.Test;

import java.lang.reflect.Method;
import java.lang.reflect.Modifier;

import static org.junit.Assert.*;

/** The stick's geometry as the modern on-screen controller draws it (values: PolicyTraceTest's stick traces). */
public class StickMathTest {
    private static final float SIZE = 264, STROKE = 3;

    @Test public void fullDeflectionPutsTheKnobCentreOnTheRimInEveryDirection() {
        float base = StickMath.baseRadius(SIZE, STROKE), travel = StickMath.travel(SIZE), c = SIZE / 2;
        for (int degrees = 0; degrees < 360; degrees += 30) {
            double angle = Math.toRadians(degrees);
            float dx = (float) Math.cos(angle) * travel, dy = (float) Math.sin(angle) * travel;
            float x = StickMath.deflectionX(dx, dy, travel, .08f), y = StickMath.deflectionY(dx, dy, travel, .08f);
            assertEquals(1, Math.hypot(x, y), 1e-5);
            float kx = StickMath.knobX(c, c, c, c, x, y, base), ky = StickMath.knobY(c, c, c, c, x, y, base);
            assertEquals(degrees + "°", base, Math.hypot(kx - c, ky - c), 1e-3);
        }
    }

    @Test public void theDrawingBoxHoldsTheKnobPastTheBase() {
        float base = StickMath.baseRadius(SIZE, STROKE), knob = StickMath.knobRadius(SIZE), shadow = 8;
        float overhang = StickMath.overhang(SIZE, STROKE, shadow);
        // Half the knob leaves the base, and the box plus the overhang still holds it and its shadow.
        assertTrue(base + knob > SIZE / 2);
        assertEquals(base + knob + shadow, SIZE / 2 + overhang, 1e-3);
    }

    @Test public void aFingerPastTheRimHoldsFullDeflectionAndTheDeadzoneStays() {
        float travel = StickMath.travel(SIZE);
        assertEquals(1, StickMath.deflectionX(SIZE * 4, 0, travel, .08f), 0);
        assertEquals(-1, StickMath.deflectionY(0, -SIZE, travel, .08f), 0);
        assertEquals(0, StickMath.deflectionX(travel * .07f, 0, travel, .08f), 0);
        assertEquals((.5f - .08f) / .92f, StickMath.deflectionX(travel * .5f, 0, travel, .08f), 1e-6);
    }

    /** The input and draw paths make no JNI calls: the stick is plain Java, kept equal to the crate by traces. */
    @Test public void noNativeCallsAndThePairFormsMatchTheSingleValues() {
        for (Method method : StickMath.class.getDeclaredMethods()) {
            assertFalse(method.getName(), Modifier.isNative(method.getModifiers()));
        }
        float travel = StickMath.travel(SIZE), base = StickMath.baseRadius(SIZE, STROKE), c = SIZE / 2;
        float[] out = new float[2];
        StickMath.deflection(-travel * .6f, travel * .9f, travel, .08f, out);
        assertEquals(StickMath.deflectionX(-travel * .6f, travel * .9f, travel, .08f), out[0], 0);
        assertEquals(StickMath.deflectionY(-travel * .6f, travel * .9f, travel, .08f), out[1], 0);
        float x = out[0], y = out[1];
        StickMath.knob(c, c, c + 20, c - 10, x, y, base, out);
        assertEquals(StickMath.knobX(c, c, c + 20, c - 10, x, y, base), out[0], 0);
        assertEquals(StickMath.knobY(c, c, c + 20, c - 10, x, y, base), out[1], 0);
    }
}
