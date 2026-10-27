package com.limelight.binding.video;

import static org.junit.Assert.*;

import com.limelight.binding.video.StagedStreamExit.Action;
import com.limelight.binding.video.StagedStreamExit.Stage;

import org.junit.Test;

public class StagedStreamExitTest {
    @Test
    public void onlyAnAndroidTvStreamingHdrIsStaged() {
        assertTrue(StagedStreamExit.applies(true, true));
        assertFalse(StagedStreamExit.applies(true, false));
        assertFalse(StagedStreamExit.applies(false, true));
        assertFalse(StagedStreamExit.applies(false, false));
    }

    @Test
    public void quietExitWalksThroughEveryStageInOrder() {
        StagedStreamExit exit = new StagedStreamExit();
        assertTrue(exit.begin(1000));
        assertEquals(Action.WAIT, exit.poll(1000));
        assertEquals(Action.WAIT, exit.poll(1300));

        exit.onStopFinished();
        assertEquals(Action.RELEASE_OUTPUT, exit.poll(1400));
        assertFalse(exit.stopTimedOut());
        assertEquals(Stage.DISPLAY_SETTLE, exit.stage());

        // Quiet is counted from releasing the output.
        assertEquals(Action.WAIT, exit.poll(1400 + StagedStreamExit.DISPLAY_QUIET_MS - 1));
        assertEquals(Action.RESTORE_DISPLAY_MODE, exit.poll(1400 + StagedStreamExit.DISPLAY_QUIET_MS));
        assertEquals(Stage.MODE_SETTLE, exit.stage());

        long restored = 1400 + StagedStreamExit.DISPLAY_QUIET_MS;
        assertEquals(Action.WAIT, exit.poll(restored + StagedStreamExit.MODE_QUIET_MS - 1));
        assertEquals(Action.FINISH, exit.poll(restored + StagedStreamExit.MODE_QUIET_MS));
        assertEquals(Stage.DONE, exit.stage());
    }

    @Test
    public void stopThatNeverEndsIsGivenUpOn() {
        StagedStreamExit exit = new StagedStreamExit();
        exit.begin(0);
        assertEquals(Action.WAIT, exit.poll(StagedStreamExit.STOP_TIMEOUT_MS - 1));
        assertEquals(Action.RELEASE_OUTPUT, exit.poll(StagedStreamExit.STOP_TIMEOUT_MS));
        assertTrue(exit.stopTimedOut());
    }

    @Test
    public void lateStopHoldsTheModeRestoreWhileTheCapAllows() {
        StagedStreamExit exit = new StagedStreamExit();
        exit.begin(0);
        long released = StagedStreamExit.STOP_TIMEOUT_MS;
        assertEquals(Action.RELEASE_OUTPUT, exit.poll(released));
        assertTrue(exit.stopTimedOut());

        // The display is quiet, but the decoder release is still running.
        assertEquals(Action.WAIT, exit.poll(released + StagedStreamExit.DISPLAY_QUIET_MS));
        assertEquals(Action.WAIT, exit.poll(released + StagedStreamExit.DISPLAY_SETTLE_MAX_MS - 1));
        exit.onStopFinished();
        exit.onDisplayChanged(released + 1500);
        assertEquals(Action.WAIT, exit.poll(released + 1500 + StagedStreamExit.DISPLAY_QUIET_MS - 1));
        assertEquals(Action.RESTORE_DISPLAY_MODE, exit.poll(released + 1500 + StagedStreamExit.DISPLAY_QUIET_MS));
    }

    @Test
    public void stopThatNeverFinishesStillRestoresAtTheCap() {
        StagedStreamExit exit = new StagedStreamExit();
        exit.begin(0);
        long released = StagedStreamExit.STOP_TIMEOUT_MS;
        assertEquals(Action.RELEASE_OUTPUT, exit.poll(released));
        assertEquals(Action.RESTORE_DISPLAY_MODE, exit.poll(released + StagedStreamExit.DISPLAY_SETTLE_MAX_MS));
    }

    @Test
    public void displayChangesPushTheSettleOutUntilTheCap() {
        StagedStreamExit exit = new StagedStreamExit();
        exit.begin(0);
        exit.onStopFinished();
        assertEquals(Action.RELEASE_OUTPUT, exit.poll(100));

        // A change every 500 ms keeps the display from ever being quiet for 750 ms.
        for (long t = 600; t < 100 + StagedStreamExit.DISPLAY_SETTLE_MAX_MS; t += 500) {
            exit.onDisplayChanged(t);
            assertEquals("at " + t, Action.WAIT, exit.poll(t + 400));
        }
        exit.onDisplayChanged(2400);
        assertEquals(Action.WAIT, exit.poll(2599));
        assertEquals(Action.RESTORE_DISPLAY_MODE, exit.poll(100 + StagedStreamExit.DISPLAY_SETTLE_MAX_MS));
    }

    @Test
    public void aChangeRestartsTheQuietTime() {
        StagedStreamExit exit = new StagedStreamExit();
        exit.begin(0);
        exit.onStopFinished();
        exit.poll(0);
        exit.onDisplayChanged(500);
        assertEquals(Action.WAIT, exit.poll(500 + StagedStreamExit.DISPLAY_QUIET_MS - 1));
        assertEquals(Action.RESTORE_DISPLAY_MODE, exit.poll(500 + StagedStreamExit.DISPLAY_QUIET_MS));
    }

    @Test
    public void modeSettleIsCappedToo() {
        StagedStreamExit exit = new StagedStreamExit();
        exit.begin(0);
        exit.onStopFinished();
        exit.poll(0);
        assertEquals(Action.RESTORE_DISPLAY_MODE, exit.poll(StagedStreamExit.DISPLAY_QUIET_MS));
        long restored = StagedStreamExit.DISPLAY_QUIET_MS;
        for (long t = restored + 300; t < restored + StagedStreamExit.MODE_SETTLE_MAX_MS; t += 300) {
            exit.onDisplayChanged(t);
            assertEquals(Action.WAIT, exit.poll(t + 100));
        }
        assertEquals(Action.FINISH, exit.poll(restored + StagedStreamExit.MODE_SETTLE_MAX_MS));
    }

    @Test
    public void worstCaseStaysBounded() {
        StagedStreamExit exit = new StagedStreamExit();
        exit.begin(0);
        long now = 0;
        int finishes = 0;
        // The display never settles and the stop never ends; a change arrives on every poll.
        while (finishes == 0) {
            now += StagedStreamExit.POLL_MS;
            exit.onDisplayChanged(now);
            if (exit.poll(now) == Action.FINISH) finishes++;
            assertTrue("never finished", now < 60000);
        }
        long cap = StagedStreamExit.STOP_TIMEOUT_MS + StagedStreamExit.DISPLAY_SETTLE_MAX_MS +
                StagedStreamExit.MODE_SETTLE_MAX_MS;
        assertTrue(now <= cap + StagedStreamExit.POLL_MS);
    }

    @Test
    public void secondBeginAndLatePollsDoNothing() {
        StagedStreamExit exit = new StagedStreamExit();
        assertFalse(exit.started());
        assertEquals(Action.WAIT, exit.poll(0));
        assertTrue(exit.begin(0));
        assertFalse(exit.begin(10));
        exit.onStopFinished();
        exit.poll(0);
        exit.poll(StagedStreamExit.DISPLAY_QUIET_MS);
        assertEquals(Action.FINISH, exit.poll(10000));
        // FINISH is returned once.
        assertEquals(Action.WAIT, exit.poll(10100));
        assertFalse(exit.begin(10200));
    }
}
