package com.limelight.binding.input.touch;

import org.junit.Test;

import static org.junit.Assert.*;

public class ThreeFingerTapTest {
    @Test
    public void aQuickStillTapCounts() {
        ThreeFingerTap tap = new ThreeFingerTap();
        tap.start(1000, 0, 100, 100);
        tap.move(110, 95);
        assertTrue(tap.finish(1000 + ThreeFingerTap.TIME_LIMIT_MS - 1, 40));
        assertFalse(tap.isStarted());
    }

    @Test
    public void aSlowTapDoesNot() {
        ThreeFingerTap tap = new ThreeFingerTap();
        tap.start(1000, 0, 100, 100);
        assertFalse(tap.finish(1000 + ThreeFingerTap.TIME_LIMIT_MS, 40));
    }

    @Test
    public void aThreeFingerSwipeDoesNot() {
        ThreeFingerTap tap = new ThreeFingerTap();
        tap.start(1000, 0, 100, 100);
        tap.move(200, 100);
        tap.move(100, 100);
        assertFalse(tap.finish(1100, 40));
    }

    @Test
    public void aFourthFingerMakesItAnotherGesture() {
        ThreeFingerTap tap = new ThreeFingerTap();
        tap.start(1000, 0, 100, 100);
        tap.pointers(4);
        assertFalse(tap.finish(1100, 40));
    }

    @Test
    public void withoutThreeFingersNothingIsReported() {
        ThreeFingerTap tap = new ThreeFingerTap();
        tap.pointers(2);
        tap.move(5, 5);
        assertFalse(tap.isStarted());
        assertFalse(tap.finish(10, 40));
    }

    @Test
    public void finishAndResetForgetTheGesture() {
        ThreeFingerTap tap = new ThreeFingerTap();
        tap.start(1000, 0, 100, 100);
        assertTrue(tap.finish(1100, 40));
        assertFalse(tap.finish(1150, 40));

        tap.start(2000, 0, 0, 0);
        tap.reset();
        assertFalse(tap.finish(2100, 40));
    }

    @Test
    public void startRemembersTheTrackedFinger() {
        ThreeFingerTap tap = new ThreeFingerTap();
        tap.start(1000, 7, 100, 100);
        assertEquals(7, tap.pointerId());
    }

    @Test
    public void aStillTapStaysATapWhenThePointerIndexMovesToAnotherFinger() {
        // The first finger lifts first, so the caller no longer finds it and reports no moves; the
        // other fingers, 100 px away, must not count as its travel.
        ThreeFingerTap tap = new ThreeFingerTap();
        tap.start(1000, 0, 100, 100);
        tap.move(104, 98);
        assertTrue(tap.finish(1100, 40));
    }
}
