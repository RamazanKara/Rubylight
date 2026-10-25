package com.limelight.console;

import static org.junit.Assert.assertEquals;

import org.junit.Test;

/** The plan of waking a PC: send at once, resend while it stays away, give up in time, and an answer wins. */
public class WakeLaunchTest {
    @Test
    public void sendsAtOnceThenWaitsBetweenResends() {
        WakeLaunch plan = new WakeLaunch(1_000, 60_000, 8_000);
        assertEquals(WakeLaunch.Step.SEND, plan.next(false, 1_000));
        assertEquals(WakeLaunch.Step.WAIT, plan.next(false, 2_000));
        assertEquals(WakeLaunch.Step.WAIT, plan.next(false, 8_999));
        assertEquals(WakeLaunch.Step.SEND, plan.next(false, 9_000));
        assertEquals(WakeLaunch.Step.WAIT, plan.next(false, 10_000));
        assertEquals(WakeLaunch.Step.SEND, plan.next(false, 17_000));
    }

    @Test
    public void givesUpWhenTheTimeIsUp() {
        WakeLaunch plan = new WakeLaunch(0, 60_000, 8_000);
        assertEquals(WakeLaunch.Step.SEND, plan.next(false, 0));
        assertEquals(WakeLaunch.Step.WAIT, plan.next(false, 7_000));
        assertEquals(WakeLaunch.Step.TIMED_OUT, plan.next(false, 60_000));
    }

    @Test
    public void anAnsweringPcWinsEvenInTheTickTheTimeRanOut() {
        WakeLaunch plan = new WakeLaunch(0, 60_000, 8_000);
        assertEquals(WakeLaunch.Step.ONLINE, plan.next(true, 0));
        assertEquals(WakeLaunch.Step.ONLINE, plan.next(true, 60_000));
    }

    @Test
    public void theDefaultsGiveAResumeFromSleepNinetySeconds() {
        WakeLaunch plan = new WakeLaunch(0);
        assertEquals(WakeLaunch.Step.SEND, plan.next(false, 0));
        assertEquals(WakeLaunch.Step.WAIT, plan.next(false, 7_999));
        assertEquals(WakeLaunch.Step.SEND, plan.next(false, WakeLaunch.RESEND_MS));
        for (long at = 2 * WakeLaunch.RESEND_MS; at < WakeLaunch.TIMEOUT_MS; at += WakeLaunch.RESEND_MS) {
            assertEquals(WakeLaunch.Step.SEND, plan.next(false, at));
        }
        assertEquals(WakeLaunch.Step.WAIT, plan.next(false, WakeLaunch.TIMEOUT_MS - 1));
        assertEquals(WakeLaunch.Step.TIMED_OUT, plan.next(false, WakeLaunch.TIMEOUT_MS));
    }
}
