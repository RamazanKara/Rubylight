package com.limelight.ui;

import static org.junit.Assert.*;

import org.junit.Test;

public class FastTravelTest {
    @Test
    public void aSinglePressIsNotFastTravel() {
        FastTravel travel = new FastTravel();
        assertFalse(travel.onMove(0, 1000));
        assertFalse(travel.isActive(1000));
        // Another press well after the first is still a single one.
        assertFalse(travel.onMove(0, 1000 + FastTravel.GAP_MS));
    }

    @Test
    public void aHeldKeyIsFastFromItsFirstRepeat() {
        FastTravel travel = new FastTravel();
        assertFalse("the press itself", travel.onMove(0, 0));
        assertTrue(travel.onMove(1, 400));
        assertTrue(travel.isActive(400));
        assertTrue(travel.onMove(2, 433));
    }

    @Test
    public void pressesCloserThanTheGapFormARun() {
        FastTravel travel = new FastTravel();
        travel.onMove(0, 0);
        assertTrue("119 ms later", travel.onMove(0, FastTravel.GAP_MS - 1));
        assertTrue(travel.onMove(0, 2 * FastTravel.GAP_MS - 2));
        assertTrue(travel.isActive(2 * FastTravel.GAP_MS - 2));
    }

    @Test
    public void theRunEndsAfterTheHoldTime() {
        FastTravel travel = new FastTravel();
        travel.onMove(1, 0);
        assertTrue(travel.isActive(FastTravel.HOLD_MS - 1));
        assertFalse(travel.isActive(FastTravel.HOLD_MS));
        // The next lone press starts over as a single move.
        assertFalse(travel.onMove(0, 5000));
        assertFalse(travel.isActive(5000));
    }

    @Test
    public void resetForgetsTheRun() {
        FastTravel travel = new FastTravel();
        travel.onMove(1, 0);
        travel.reset();
        assertFalse(travel.isActive(10));
        assertFalse("no previous move to follow", travel.onMove(0, 20));
    }
}
