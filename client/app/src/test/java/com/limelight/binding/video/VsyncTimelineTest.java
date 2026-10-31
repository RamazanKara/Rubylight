package com.limelight.binding.video;

import org.junit.Test;

import static org.junit.Assert.*;

public class VsyncTimelineTest {
    private static final long SIXTY = 16_666_667;

    @Test public void theAnchorIsTheLatestVsyncNotAfterTheFrameTime() {
        long frame = 1_000_000_000L;
        // Presented two refreshes and a bit after the callback's frame time.
        long present = frame + 2 * SIXTY + 3_000_000;
        long anchor = VsyncTimeline.anchorNs(present, frame, SIXTY);
        assertTrue(anchor <= frame);
        assertTrue(frame - anchor < SIXTY);
        assertEquals(0, (present - anchor) % SIXTY);
        // A presentation time on the frame time is that vsync.
        assertEquals(frame, VsyncTimeline.anchorNs(frame + 3 * SIXTY, frame, SIXTY));
        assertEquals(frame, VsyncTimeline.anchorNs(frame, frame, SIXTY));
    }

    @Test public void anImpossibleTimelineHasNoAnchor() {
        assertEquals(0, VsyncTimeline.anchorNs(5_000, 10_000, SIXTY));
        assertEquals(0, VsyncTimeline.anchorNs(50_000_000, 10_000, 0));
        assertEquals(0, VsyncTimeline.anchorNs(0, 10_000, SIXTY));
    }

    @Test public void theLeadIsThePresentationTimeLessTheDeadline() {
        long present = 2_000_000_000L;
        assertEquals(10_600_000, VsyncTimeline.deadlineLeadNs(present, present - 10_600_000, SIXTY));
        assertEquals(0, VsyncTimeline.deadlineLeadNs(present, present, SIXTY));
        // A deadline after the presentation time, or many refreshes before it, is rejected.
        assertEquals(-1, VsyncTimeline.deadlineLeadNs(present, present + 1, SIXTY));
        assertEquals(-1, VsyncTimeline.deadlineLeadNs(present, present - 4 * SIXTY, SIXTY));
        assertEquals(-1, VsyncTimeline.deadlineLeadNs(present, 0, SIXTY));
    }

    @Test public void onlyAChangedLeadIsLoggedAgain() {
        assertTrue(VsyncTimeline.worthLogging(-1, 10_000_000));
        assertFalse(VsyncTimeline.worthLogging(10_000_000, 10_400_000));
        assertTrue(VsyncTimeline.worthLogging(10_000_000, 11_000_000));
    }
}
