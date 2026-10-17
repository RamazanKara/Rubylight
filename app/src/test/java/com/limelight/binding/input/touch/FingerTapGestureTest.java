package com.limelight.binding.input.touch;

import org.junit.Test;

import static org.junit.Assert.*;

public class FingerTapGestureTest {
    @Test public void fourFingersReportOnlyFourRegardlessOfLiftOrder() {
        FingerTapGesture tap = new FingerTapGesture();
        tap.start(1000, 7, 100, 100);
        tap.pointers(4);
        tap.pointers(3);
        tap.pointers(2);
        tap.pointers(1);
        assertEquals(7, tap.pointerId());
        assertEquals(4, tap.finish(1299, 40));
        assertFalse(tap.isStarted());
        assertEquals(0, tap.finish(1299, 40));
    }

    @Test public void addingAFourthFingerDoesNotRestartTheTimeOrTravelLimit() {
        FingerTapGesture tap = new FingerTapGesture();
        tap.start(1000, 0, 0, 0);
        tap.pointers(4);
        assertEquals(0, tap.finish(1300, 40));
        tap.start(2000, 0, 0, 0);
        tap.move(41, 0);
        tap.pointers(4);
        tap.move(0, 0);
        assertEquals(0, tap.finish(2100, 40));
    }

    @Test public void fiveFingersAndCanceledSequencesNeverBecomeTapsAfterFingersLift() {
        FingerTapGesture tap = new FingerTapGesture();
        tap.start(1000, 0, 0, 0);
        tap.pointers(5);
        tap.pointers(3);
        assertEquals(0, tap.finish(1100, 40));
        tap.start(2000, 0, 0, 0);
        tap.invalidate();
        assertTrue(tap.isStarted());
        tap.pointers(4);
        assertEquals(0, tap.finish(2100, 40));
        tap.start(3000, 0, 0, 0);
        assertEquals(3, tap.finish(3100, 40));
    }

    @Test public void resetDiscardsTheCandidateAndMissingOrBackwardTimesDoNotCount() {
        FingerTapGesture tap = new FingerTapGesture();
        assertEquals(0, tap.finish(100, 40));
        tap.start(1000, 0, 0, 0);
        tap.reset();
        assertEquals(0, tap.finish(1100, 40));
        tap.start(2000, 0, 0, 0);
        assertEquals(0, tap.finish(1999, 40));
    }
}
