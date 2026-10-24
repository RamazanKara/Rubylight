package com.limelight.binding.video;

import static org.junit.Assert.*;

import com.limelight.binding.video.CrashAccounting.Verdict;

import org.junit.Test;

public class CrashAccountingTest {
    private static final long NOW = 100_000;

    @Test
    public void normalCrashIsCounted() {
        assertEquals(Verdict.COUNT, CrashAccounting.classify(false, false, NOW, -1, false));
        assertEquals(Verdict.COUNT, CrashAccounting.classify(false, true, NOW, -1, false));
    }

    @Test
    public void crashWhileTheStreamEndsIsNotCountedOnAnyDevice() {
        assertEquals(Verdict.STREAM_ENDING, CrashAccounting.classify(true, false, NOW, -1, false));
        assertEquals(Verdict.STREAM_ENDING, CrashAccounting.classify(true, true, NOW, NOW - 100, true));
    }

    @Test
    public void tvCrashRightAfterADisplayChangeIsADisplayLoss() {
        assertEquals(Verdict.DISPLAY_LOST, CrashAccounting.classify(false, true, NOW, NOW - 1, false));
        assertEquals(Verdict.DISPLAY_LOST,
                CrashAccounting.classify(false, true, NOW, NOW - CrashAccounting.DISPLAY_WINDOW_MS, false));
    }

    @Test
    public void tvCrashLongAfterADisplayChangeIsCounted() {
        assertEquals(Verdict.COUNT,
                CrashAccounting.classify(false, true, NOW, NOW - CrashAccounting.DISPLAY_WINDOW_MS - 1, false));
    }

    @Test
    public void tvCrashWithTheDisplayOffIsADisplayLoss() {
        assertEquals(Verdict.DISPLAY_LOST, CrashAccounting.classify(false, true, NOW, -1, true));
    }

    @Test
    public void phonesIgnoreDisplayChangesAndState() {
        assertEquals(Verdict.COUNT, CrashAccounting.classify(false, false, NOW, NOW - 10, false));
        assertEquals(Verdict.COUNT, CrashAccounting.classify(false, false, NOW, NOW - 10, true));
    }
}
