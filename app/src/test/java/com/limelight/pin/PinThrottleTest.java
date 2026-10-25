package com.limelight.pin;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

public class PinThrottleTest {
    @Test
    public void firstFourFailuresAreFree() {
        for (int fails = 0; fails <= 4; fails++) assertEquals("after " + fails, 0, PinThrottle.delayAfter(fails));
    }

    @Test
    public void fifthFailureLocksForThirtySecondsAndEachNextOneDoubles() {
        assertEquals(30_000, PinThrottle.delayAfter(5));
        assertEquals(60_000, PinThrottle.delayAfter(6));
        assertEquals(120_000, PinThrottle.delayAfter(7));
        assertEquals(240_000, PinThrottle.delayAfter(8));
        assertEquals(480_000, PinThrottle.delayAfter(9));
        assertEquals(960_000, PinThrottle.delayAfter(10));
    }

    @Test
    public void delayStopsAtThirtyMinutesWhateverTheCount() {
        assertEquals(PinThrottle.MAX_MS, PinThrottle.delayAfter(11));
        assertEquals(PinThrottle.MAX_MS, PinThrottle.delayAfter(40));
        assertEquals(PinThrottle.MAX_MS, PinThrottle.delayAfter(Integer.MAX_VALUE));
    }

    @Test
    public void failedCountsTheTryAndStartsTheWaitOnTheCurrentBoot() {
        PinThrottle.State state = PinThrottle.State.CLEAN;
        for (int i = 1; i <= 4; i++) {
            state = PinThrottle.failed(state, 1_000, 7);
            assertEquals(i, state.fails);
            assertEquals(0, PinThrottle.remainingMs(state, 1_000));
        }
        state = PinThrottle.failed(state, 1_000, 7);
        assertEquals(5, state.fails);
        assertEquals(7, state.lockBoot);
        assertEquals(30_000, PinThrottle.remainingMs(state, 1_000));
        assertEquals(20_000, PinThrottle.remainingMs(state, 11_000));
        assertEquals(0, PinThrottle.remainingMs(state, 31_000));
    }

    @Test
    public void aLockFromAnotherBootIsRenewedAtItsStepNotDropped() {
        PinThrottle.State locked = new PinThrottle.State(7, 900_000, 1);
        PinThrottle.State after = PinThrottle.onBoot(locked, 5_000, 2);
        assertEquals(7, after.fails);
        assertEquals(2, after.lockBoot);
        // The seventh failure waits 2 minutes, counted from the new boot's clock.
        assertEquals(120_000, PinThrottle.remainingMs(after, 5_000));
    }

    @Test
    public void aBootWithoutALockStaysFree() {
        PinThrottle.State after = PinThrottle.onBoot(new PinThrottle.State(3, 0, 1), 5_000, 2);
        assertEquals(3, after.fails);
        assertEquals(0, PinThrottle.remainingMs(after, 5_000));
    }

    @Test
    public void theSameBootIsLeftAlone() {
        PinThrottle.State state = new PinThrottle.State(6, 50_000, 3);
        assertTrue(state == PinThrottle.onBoot(state, 10_000, 3));
    }

    @Test
    public void aCorruptStoredLockNeverWaitsLongerThanTheLongestStep() {
        PinThrottle.State state = new PinThrottle.State(5, Long.MAX_VALUE, 1);
        assertEquals(PinThrottle.MAX_MS, PinThrottle.remainingMs(state, 0));
    }
}
