package com.limelight.binding.video;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

public class VulkanPaceCheckTest {
    private static int windowsUntilReported(VulkanPaceCheck check, int received, int presented) {
        for (int window = 1; window <= 20; window++) {
            if (check.window(received, presented, 1000)) return window;
        }
        return -1;
    }

    @Test
    public void theTclTvsTenFpsOfSixtyIsReportedAfterFiveSeconds() {
        VulkanPaceCheck check = new VulkanPaceCheck();
        for (int window = 1; window < 5; window++) assertFalse(check.window(60, 10, 1000));
        assertTrue(check.window(60, 10, 1000));
    }

    @Test
    public void reportedOncePerStream() {
        VulkanPaceCheck check = new VulkanPaceCheck();
        windowsUntilReported(check, 60, 24);
        for (int window = 0; window < 20; window++) assertFalse(check.window(60, 24, 1000));
    }

    @Test
    public void eightyPercentOrMoreKeepsUp() {
        assertEquals(-1, windowsUntilReported(new VulkanPaceCheck(), 60, 48));
        assertEquals(-1, windowsUntilReported(new VulkanPaceCheck(), 60, 60));
        assertEquals(5, windowsUntilReported(new VulkanPaceCheck(), 60, 47));
    }

    @Test
    public void oneWindowThatKeepsUpStartsTheCountAgain() {
        VulkanPaceCheck check = new VulkanPaceCheck();
        for (int window = 0; window < 4; window++) assertFalse(check.window(60, 10, 1000));
        assertFalse(check.window(60, 60, 1000));
        for (int window = 0; window < 4; window++) assertFalse(check.window(60, 10, 1000));
        assertTrue(check.window(60, 10, 1000));
    }

    @Test
    public void aDecoderStallIsNotASlowRenderer() {
        // Nothing presented: the decoder hangs or restarts, which the renderer cannot help.
        assertEquals(-1, windowsUntilReported(new VulkanPaceCheck(), 60, 0));
    }

    @Test
    public void aStillPictureIsNotJudged() {
        // The host sends few frames while nothing moves; one of two presented says nothing.
        assertEquals(-1, windowsUntilReported(new VulkanPaceCheck(), 5, 1));
    }

    @Test
    public void windowLengthsAddUpToFiveSeconds() {
        VulkanPaceCheck check = new VulkanPaceCheck();
        assertFalse(check.window(150, 30, 2500));
        assertTrue(check.window(150, 30, 2500));
    }
}
