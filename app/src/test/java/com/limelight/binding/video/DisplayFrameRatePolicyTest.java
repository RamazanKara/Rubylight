package com.limelight.binding.video;

import org.junit.Test;
import static org.junit.Assert.*;

public class DisplayFrameRatePolicyTest {
    @Test
    public void android15AndOlderCannotEnableArr() {
        for (int sdk : new int[] {21, 30, 31, 34, 35}) {
            assertFalse(DisplayFrameRatePolicy.useAdaptiveRefreshRate(sdk, true, false));
            assertFalse(DisplayFrameRatePolicy.useAdaptiveRefreshRate(sdk, true, true));
        }
    }

    @Test
    public void android16RequiresArrSupport() {
        for (int sdk : new int[] {36, 37}) {
            assertTrue(DisplayFrameRatePolicy.useAdaptiveRefreshRate(sdk, true, true));
            assertFalse(DisplayFrameRatePolicy.useAdaptiveRefreshRate(sdk, true, false));
            assertFalse(DisplayFrameRatePolicy.useAdaptiveRefreshRate(sdk, false, true));
        }
    }

    @Test
    public void streamRequestRoundsPanelMaximumWithinHostLimits() {
        assertEquals(120, DisplayFrameRatePolicy.streamFrameRate(119.88f));
        assertEquals(144, DisplayFrameRatePolicy.streamFrameRate(144));
        assertEquals(1000, DisplayFrameRatePolicy.streamFrameRate(1200));
        assertEquals(1, DisplayFrameRatePolicy.streamFrameRate(0.5f));
    }

    @Test
    public void contentRateVoteNeedsTheSettingARubylightHostAndASeamlessSurface() {
        assertTrue(DisplayFrameRatePolicy.followsContentRate(31, true, true, false, false, false));
        assertTrue(DisplayFrameRatePolicy.followsContentRate(30, true, true, false, false, false));
        assertTrue(DisplayFrameRatePolicy.followsContentRate(36, true, true, false, false, false));
        // Off, and a host that sends no clock sync (Sunshine, GFE, older Rubylight).
        assertFalse(DisplayFrameRatePolicy.followsContentRate(31, false, true, false, false, false));
        assertFalse(DisplayFrameRatePolicy.followsContentRate(31, true, false, false, false, false));
        // Below API 30 there is no frame rate vote.
        assertFalse(DisplayFrameRatePolicy.followsContentRate(29, true, true, false, false, false));
        assertFalse(DisplayFrameRatePolicy.followsContentRate(21, true, true, false, false, false));
        // ARR keeps vote 0, VRR keeps the panel maximum, a TextureView does not vote.
        assertFalse(DisplayFrameRatePolicy.followsContentRate(36, true, true, true, false, false));
        assertFalse(DisplayFrameRatePolicy.followsContentRate(36, true, true, true, true, false));
        assertFalse(DisplayFrameRatePolicy.followsContentRate(31, true, true, false, true, false));
        assertFalse(DisplayFrameRatePolicy.followsContentRate(31, true, true, false, false, true));
    }

    @Test
    public void contentVoteMayLowerTheRateOnlyWhereLowerRefreshIsAllowed() {
        // 120 Hz panel, a 120 fps stream whose game runs at 60: lowering is the point of the vote.
        assertEquals(60f, DisplayFrameRatePolicy.contentVote(60f, 120f, true, 120f), 0f);
        // With "Allow lower refresh rate" off the vote stays at the mode that setting keeps.
        assertEquals(120f, DisplayFrameRatePolicy.contentVote(60f, 120f, false, 120f), 0f);
        // A game faster than the base vote raises it either way, up to the panel maximum.
        assertEquals(90f, DisplayFrameRatePolicy.contentVote(90f, 60f, false, 120f), 0f);
        assertEquals(90f, DisplayFrameRatePolicy.contentVote(90f, 60f, true, 120f), 0f);
        assertEquals(120f, DisplayFrameRatePolicy.contentVote(144f, 60f, true, 120f), 0f);
        // An unknown panel maximum does not cap.
        assertEquals(144f, DisplayFrameRatePolicy.contentVote(144f, 60f, true, 0f), 0f);
    }

    @Test
    public void contentRateVotesOncePerTwoSecondsAndOnlyOnARealChange() {
        DisplayFrameRatePolicy.ContentRateVote vote = new DisplayFrameRatePolicy.ContentRateVote();
        // No content rate yet.
        assertEquals(0f, vote.next(0, 0), 0f);
        assertEquals(60f, vote.next(100, 60_000), 0f);
        // Within 3%: no change, now or later (59.94 against 60 is the usual noise).
        assertEquals(0f, vote.next(300, 59_940), 0f);
        assertEquals(0f, vote.next(9_000, 61_500), 0f);
        // A real change inside 2 s of the last vote waits, and is not lost.
        assertEquals(0f, vote.next(1_900, 90_000), 0f);
        assertEquals(0f, vote.next(2_099, 90_000), 0f);
        assertEquals(90f, vote.next(2_100, 90_000), 0f);
        assertEquals(0f, vote.next(2_200, 90_000), 0f);
        // The rate dropping out (tracker lost it) votes nothing and keeps the last vote.
        assertEquals(0f, vote.next(5_000, 0), 0f);
        // 3% hysteresis: 92 is inside it from 90, 93 is past it.
        assertEquals(0f, vote.next(9_000, 92_000), 0f);
        assertEquals(93f, vote.next(9_000, 93_000), 0f);
    }

    @Test
    public void contentRateVoteIsRepeatedAfterSomethingElseReplacedIt() {
        DisplayFrameRatePolicy.ContentRateVote vote = new DisplayFrameRatePolicy.ContentRateVote();
        assertEquals(90f, vote.next(0, 90_000), 0f);
        assertEquals(0f, vote.next(500, 90_000), 0f);
        // A new surface or stream rate made the base vote: the same content rate votes again,
        // still no sooner than 2 s after the last content vote.
        vote.invalidate();
        assertEquals(0f, vote.next(1_000, 90_000), 0f);
        assertEquals(90f, vote.next(2_000, 90_000), 0f);
        vote.invalidate();
        assertEquals(0f, vote.next(3_999, 90_000), 0f);
        assertEquals(90f, vote.next(4_000, 90_000), 0f);
    }
}
