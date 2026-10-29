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
        // ARR keeps the stream rate vote, a TextureView does not vote.
        assertFalse(DisplayFrameRatePolicy.followsContentRate(36, true, true, true, false, false));
        assertFalse(DisplayFrameRatePolicy.followsContentRate(36, true, true, true, true, false));
        assertFalse(DisplayFrameRatePolicy.followsContentRate(31, true, true, false, false, true));
    }

    @Test
    public void vrrWithoutArrFollowsTheContentRateWithoutTheSetting() {
        // Android before 16 (ANDROID_TOO_OLD) and a panel without ARR (PANEL_WITHOUT_ARR):
        // the panel maximum alone judders, so the game's rate is followed, setting or not.
        for (int sdk : new int[] {30, 31, 33, 35, 36}) {
            boolean arr = DisplayFrameRatePolicy.useAdaptiveRefreshRate(sdk, true, false);
            assertFalse(arr);
            assertTrue(DisplayFrameRatePolicy.followsContentRate(sdk, false, true, arr, true, false));
            assertTrue(DisplayFrameRatePolicy.followsContentRate(sdk, true, true, arr, true, false));
        }
        // Still only with a host's frame times, a seamless vote and a SurfaceView.
        assertFalse(DisplayFrameRatePolicy.followsContentRate(33, false, false, false, true, false));
        assertFalse(DisplayFrameRatePolicy.followsContentRate(29, false, true, false, true, false));
        assertFalse(DisplayFrameRatePolicy.followsContentRate(33, false, true, false, true, true));
        // Under ARR it stays off.
        assertTrue(DisplayFrameRatePolicy.useAdaptiveRefreshRate(36, true, true));
        assertFalse(DisplayFrameRatePolicy.followsContentRate(36, false, true, true, true, false));
    }

    @Test
    public void theRendererTracksTheContentRateForVrrOrTheSettingWithLowerRefresh() {
        assertTrue(DisplayFrameRatePolicy.tracksContentRate(true, true, false, false));
        assertFalse(DisplayFrameRatePolicy.tracksContentRate(true, false, false, false));
        assertFalse(DisplayFrameRatePolicy.tracksContentRate(false, true, false, false));
        // VRR turns lower refresh off and stands in for both.
        assertTrue(DisplayFrameRatePolicy.tracksContentRate(false, false, true, false));
        assertTrue(DisplayFrameRatePolicy.tracksContentRate(true, false, true, false));
        // A TextureView does not vote.
        assertFalse(DisplayFrameRatePolicy.tracksContentRate(true, true, false, true));
        assertFalse(DisplayFrameRatePolicy.tracksContentRate(false, false, true, true));
    }

    @Test
    public void aVrrContentVoteIsTheGamesRateUpToThePanelMaximum() {
        // VRR's base vote is the panel maximum; lower refresh is off, the vote still comes down.
        assertTrue(DisplayFrameRatePolicy.contentVoteMayLower(true, false));
        assertFalse(DisplayFrameRatePolicy.contentVoteMayLower(false, false));
        assertTrue(DisplayFrameRatePolicy.contentVoteMayLower(false, true));
        // Retroid: one 120 Hz panel, a 120 fps stream, a game at 60.
        boolean lower = DisplayFrameRatePolicy.contentVoteMayLower(true, false);
        assertEquals(60f, DisplayFrameRatePolicy.contentVote(60f, 120f, lower, 120f), 0f);
        assertEquals(90f, DisplayFrameRatePolicy.contentVote(90f, 120f, lower, 120f), 0f);
        // Never above the panel maximum.
        assertEquals(120f, DisplayFrameRatePolicy.contentVote(144f, 120f, lower, 120f), 0f);
    }

    @Test
    public void arrVotesTheStreamRateNeverZeroNorAboveThePanel() {
        assertEquals(120f, DisplayFrameRatePolicy.arrVote(120f, 120f), 0f);
        assertEquals(90f, DisplayFrameRatePolicy.arrVote(90f, 120f), 0f);
        assertEquals(59.94f, DisplayFrameRatePolicy.arrVote(59.94f, 120f), 0f);
        assertEquals(120f, DisplayFrameRatePolicy.arrVote(144f, 120f), 0f);
        // No stream rate yet: the panel maximum, which VRR streams at.
        assertEquals(120f, DisplayFrameRatePolicy.arrVote(0f, 120f), 0f);
        // An unknown panel maximum does not cap.
        assertEquals(144f, DisplayFrameRatePolicy.arrVote(144f, 0f), 0f);
    }

    @Test
    public void arrHalfRateIsFlaggedAfterThreeSecondsAndClearsAtOnce() {
        DisplayFrameRatePolicy.ArrHalfRateCheck check = new DisplayFrameRatePolicy.ArrHalfRateCheck();
        // 120 fps received, 60 shown: Android's 60 Hz cap for games.
        assertFalse(check.update(0, 120f, 60f));
        assertFalse(check.update(1_000, 119f, 61f));
        assertFalse(check.update(2_999, 120f, 59f));
        assertTrue(check.update(3_000, 120f, 60f));
        assertTrue(check.flagged());
        assertTrue(check.update(4_000, 120f, 60f));
        // The screen shows the received frames again: cleared on the first sample.
        assertFalse(check.update(5_000, 120f, 118f));
        assertFalse(check.flagged());
        // A broken run starts over.
        assertFalse(check.update(6_000, 120f, 60f));
        assertFalse(check.update(7_000, 120f, 100f));
        assertFalse(check.update(8_000, 120f, 60f));
        assertFalse(check.update(10_999, 120f, 60f));
        assertTrue(check.update(11_000, 120f, 60f));
        check.reset();
        assertFalse(check.flagged());
        assertFalse(check.update(12_000, 120f, 60f));
    }

    @Test
    public void onlyAboutHalfCountsAsHalf() {
        assertTrue(DisplayFrameRatePolicy.ArrHalfRateCheck.isHalf(120f, 60f));
        assertTrue(DisplayFrameRatePolicy.ArrHalfRateCheck.isHalf(144f, 72f));
        assertTrue(DisplayFrameRatePolicy.ArrHalfRateCheck.isHalf(120f, 50f));
        assertTrue(DisplayFrameRatePolicy.ArrHalfRateCheck.isHalf(120f, 70f));
        // Full rate, a few drops, or a third are not half.
        assertFalse(DisplayFrameRatePolicy.ArrHalfRateCheck.isHalf(120f, 120f));
        assertFalse(DisplayFrameRatePolicy.ArrHalfRateCheck.isHalf(120f, 100f));
        assertFalse(DisplayFrameRatePolicy.ArrHalfRateCheck.isHalf(120f, 40f));
        // Too few frames to judge, and no shown count (-1).
        assertFalse(DisplayFrameRatePolicy.ArrHalfRateCheck.isHalf(30f, 15f));
        assertFalse(DisplayFrameRatePolicy.ArrHalfRateCheck.isHalf(120f, -1f));
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

    @Test
    public void contentRateVoteIsWithdrawnWhenTheGameStopsHoldingARate() {
        DisplayFrameRatePolicy.ContentRateVote vote = new DisplayFrameRatePolicy.ContentRateVote();
        // Nothing voted: nothing to withdraw.
        assertFalse(vote.withdraw(0, 0));
        assertEquals(90f, vote.next(0, 90_000), 0f);
        // A rate still reported is not withdrawn; an unreported one only 2 s after the vote.
        assertFalse(vote.withdraw(5_000, 90_000));
        assertFalse(vote.withdraw(1_999, 0));
        assertTrue(vote.withdraw(2_000, 0));
        // Once withdrawn it is not withdrawn again, and the next steady rate votes again.
        assertFalse(vote.withdraw(3_000, 0));
        assertEquals(0f, vote.next(3_000, 60_000), 0f);
        assertEquals(60f, vote.next(4_000, 60_000), 0f);
    }

    @Test
    public void aVrrRequestThatArrCannotServeSaysWhyItFellBackToTheTopRate() {
        // Not asked for: nothing to explain, whatever the device is.
        for (int sdk : new int[] {30, 36}) {
            assertEquals(DisplayFrameRatePolicy.ArrState.NOT_REQUESTED, DisplayFrameRatePolicy.arrState(sdk, false, true));
            assertEquals(DisplayFrameRatePolicy.ArrState.NOT_REQUESTED, DisplayFrameRatePolicy.arrState(sdk, false, false));
        }
        // Before Android 16 there is no ARR, with or without a capable panel.
        for (int sdk : new int[] {21, 30, 31, 34, 35}) {
            assertEquals(DisplayFrameRatePolicy.ArrState.ANDROID_TOO_OLD, DisplayFrameRatePolicy.arrState(sdk, true, true));
            assertEquals(DisplayFrameRatePolicy.ArrState.ANDROID_TOO_OLD, DisplayFrameRatePolicy.arrState(sdk, true, false));
        }
        // On Android 16 the panel decides.
        assertEquals(DisplayFrameRatePolicy.ArrState.IN_USE, DisplayFrameRatePolicy.arrState(36, true, true));
        assertEquals(DisplayFrameRatePolicy.ArrState.PANEL_WITHOUT_ARR, DisplayFrameRatePolicy.arrState(36, true, false));
        assertEquals(DisplayFrameRatePolicy.ArrState.PANEL_WITHOUT_ARR, DisplayFrameRatePolicy.arrState(37, true, false));
    }
}
