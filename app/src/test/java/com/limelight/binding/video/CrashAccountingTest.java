package com.limelight.binding.video;

import static org.junit.Assert.*;

import com.limelight.binding.video.CrashAccounting.Verdict;

import org.junit.Test;

public class CrashAccountingTest {
    @Test
    public void normalCrashIsCounted() {
        assertEquals(Verdict.COUNT, CrashAccounting.classify(false, false, false));
        assertEquals(Verdict.COUNT, CrashAccounting.classify(false, true, false));
    }

    @Test
    public void crashWhileTheStreamEndsIsNotCountedOnAnyDevice() {
        assertEquals(Verdict.STREAM_ENDING, CrashAccounting.classify(true, false, false));
        assertEquals(Verdict.STREAM_ENDING, CrashAccounting.classify(true, true, true));
    }

    @Test
    public void tvCrashWithTheDisplayOffOrGoneIsADisplayLoss() {
        assertEquals(Verdict.DISPLAY_LOST, CrashAccounting.classify(false, true, true));
    }

    @Test
    public void tvCrashWithTheDisplayStillOnIsCountedWhateverTheDisplayDidBefore() {
        // Our own display-mode and frame-rate requests change the display at stream start and
        // with content-rate votes; a decoder failure seconds later is still a decoder crash. The
        // classification takes no display history, only whether the display is on right now.
        assertEquals(Verdict.COUNT, CrashAccounting.classify(false, true, false));
    }

    @Test
    public void phonesIgnoreTheDisplayState() {
        assertEquals(Verdict.COUNT, CrashAccounting.classify(false, false, true));
    }
}
