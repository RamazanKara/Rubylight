package com.limelight.binding.input.touch;

import org.junit.Test;

import static org.junit.Assert.*;

public class TouchMomentumTest {
    // A steady swipe of 8 px every 8 ms (1 px/ms) ending at t = 80.
    private static TouchMomentum flick(String glide) {
        TouchMomentum momentum = new TouchMomentum(glide);
        for (long t = 0; t <= 80; t += 8) momentum.track(8, 0, t);
        return momentum;
    }

    private static int[] glide(TouchMomentum momentum, long start, long frameMs, int maxFrames) {
        int x = 0, y = 0, frames = 0;
        long t = start;
        while (frames < maxFrames) {
            boolean more = momentum.step(t);
            x += momentum.stepX();
            y += momentum.stepY();
            frames++;
            if (!more) break;
            t += frameMs;
        }
        return new int[] {x, y, frames};
    }

    @Test
    public void aQuickFlickGlidesAndConverges() {
        for (String glide : new String[] {TouchMomentum.SHORT, TouchMomentum.MEDIUM, TouchMomentum.LONG}) {
            TouchMomentum momentum = flick(glide);
            assertTrue(momentum.release(85));
            assertTrue(momentum.isActive());
            int[] travel = glide(momentum, 1000, 8, 10_000);
            assertFalse(glide, momentum.isActive());
            // An exponential glide from 1 px/ms covers about one time constant's worth of pixels.
            float expected = momentum.timeConstantMs();
            assertEquals(glide, expected, travel[0], expected * 0.1f);
            assertEquals(0, travel[1]);
            assertTrue(glide + " took " + travel[2] + " frames", travel[2] < TouchMomentum.MAX_GLIDE_MS / 8 + 2);
        }
    }

    @Test
    public void longerGlidesTravelFarther() {
        int[] distances = new int[3];
        String[] glides = {TouchMomentum.SHORT, TouchMomentum.MEDIUM, TouchMomentum.LONG};
        for (int i = 0; i < glides.length; i++) {
            TouchMomentum momentum = flick(glides[i]);
            momentum.release(85);
            distances[i] = glide(momentum, 0, 16, 10_000)[0];
        }
        assertTrue(distances[0] < distances[1] && distances[1] < distances[2]);
    }

    @Test
    public void frameRateDoesNotChangeTheDistance() {
        TouchMomentum at60 = flick(TouchMomentum.MEDIUM), at120 = flick(TouchMomentum.MEDIUM);
        at60.release(85);
        at120.release(85);
        assertEquals(glide(at60, 0, 16, 10_000)[0], glide(at120, 0, 8, 10_000)[0], 3);
    }

    @Test
    public void aSlowMoveOrARestingFingerDoesNotGlide() {
        TouchMomentum slow = new TouchMomentum(TouchMomentum.MEDIUM);
        for (long t = 0; t <= 80; t += 8) slow.track(1, 0, t);
        assertFalse(slow.release(85));
        assertFalse(slow.step(100));

        TouchMomentum rested = flick(TouchMomentum.MEDIUM);
        assertFalse(rested.release(80 + TouchMomentum.REST_MS + 1));
        assertFalse(rested.isActive());

        assertFalse(new TouchMomentum(TouchMomentum.MEDIUM).release(0));
    }

    @Test
    public void stopEndsTheGlideAtOnceAndANewTouchDoesToo() {
        TouchMomentum momentum = flick(TouchMomentum.LONG);
        assertTrue(momentum.release(85));
        momentum.step(0);
        momentum.step(16);
        momentum.stop();
        assertFalse(momentum.isActive());
        assertFalse(momentum.step(32));
        assertEquals(0, momentum.stepX());

        momentum = flick(TouchMomentum.LONG);
        momentum.release(85);
        // The next gesture's first move ends the glide.
        momentum.track(0, 0, 200);
        assertFalse(momentum.isActive());
    }

    @Test
    public void theFirstFrameOnlyStartsTheClock() {
        TouchMomentum momentum = flick(TouchMomentum.MEDIUM);
        momentum.release(85);
        assertTrue(momentum.step(123_456));
        assertEquals(0, momentum.stepX());
        assertTrue(momentum.step(123_472));
        assertTrue(momentum.stepX() > 0);
    }

    @Test
    public void aLateFrameMovesOnlyABoundedStep() {
        TouchMomentum momentum = flick(TouchMomentum.LONG);
        momentum.release(85);
        momentum.step(0);
        momentum.step(10_000);
        assertTrue(momentum.stepX() <= TouchMomentum.MAX_STEP_MS + 1);
        // Time going backwards moves nothing.
        TouchMomentum backwards = flick(TouchMomentum.LONG);
        backwards.release(85);
        backwards.step(100);
        backwards.step(50);
        assertEquals(0, backwards.stepX());
    }

    @Test
    public void hugeFlicksAreCappedAndEndWithinTheTimeLimit() {
        TouchMomentum momentum = new TouchMomentum(TouchMomentum.LONG);
        for (long t = 0; t <= 16; t += 8) momentum.track(5000, -5000, t);
        assertTrue(momentum.release(16));
        int[] travel = glide(momentum, 0, 16, 100_000);
        assertFalse(momentum.isActive());
        assertTrue(travel[2] <= TouchMomentum.MAX_GLIDE_MS / 16 + 2);
        float cap = TouchMomentum.MAX_SPEED * momentum.timeConstantMs();
        assertTrue(Math.hypot(travel[0], travel[1]) <= cap * 1.01);
        assertTrue(travel[0] > 0 && travel[1] < 0);
    }

    @Test
    public void sameTimestampSamplesStillCount() {
        TouchMomentum momentum = new TouchMomentum(TouchMomentum.MEDIUM);
        momentum.track(0, 0, 0);
        momentum.track(10, 0, 8);
        momentum.track(10, 0, 8);
        momentum.track(10, 0, 16);
        assertTrue(momentum.release(16));
    }
}
