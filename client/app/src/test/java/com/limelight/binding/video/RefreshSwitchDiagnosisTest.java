package com.limelight.binding.video;

import static org.junit.Assert.*;

import com.limelight.binding.video.RefreshSwitchDiagnosis.Cause;
import com.limelight.binding.video.RefreshSwitchDiagnosis.Conditions;
import com.limelight.binding.video.RefreshSwitchDiagnosis.PanelMode;
import com.limelight.binding.video.RefreshSwitchDiagnosis.Result;

import org.junit.Test;

import java.util.Arrays;
import java.util.List;
import java.util.Locale;

/** Fake panels only: nothing here has run on a phone with several seamless rates. */
public class RefreshSwitchDiagnosisTest {
    private static final float[] NONE = new float[0];

    private static PanelMode mode(int id, float hz, float... alternatives) {
        return new PanelMode(id, 1080, 2400, hz, alternatives);
    }

    // A Pixel or Samsung class panel: 60/90/120 in one config group, so each lists the others.
    private static List<PanelMode> seamlessPhone() {
        return Arrays.asList(mode(1, 60f, 90f, 120f), mode(2, 90f, 60f, 120f), mode(3, 120f, 60f, 90f));
    }

    private static Result diagnose(int sdk, float vote, boolean seamlessOnly, PanelMode active,
                                   List<PanelMode> modes, Conditions conditions) {
        return RefreshSwitchDiagnosis.diagnose(sdk, vote, seamlessOnly, active, modes, conditions);
    }

    @Test
    public void aScreenAtTheVoteOrAWholeMultipleOfItHolds() {
        assertTrue(RefreshSwitchDiagnosis.holds(60f, 60f));
        assertTrue(RefreshSwitchDiagnosis.holds(60f, 59.94f));
        assertTrue(RefreshSwitchDiagnosis.holds(60f, 120f));
        assertTrue(RefreshSwitchDiagnosis.holds(30f, 90f));
        assertTrue(RefreshSwitchDiagnosis.holds(24f, 120f));
        assertTrue(RefreshSwitchDiagnosis.holds(59.94f, 119.88f));
        assertFalse(RefreshSwitchDiagnosis.holds(60f, 90f));
        assertFalse(RefreshSwitchDiagnosis.holds(90f, 60f));
        assertFalse(RefreshSwitchDiagnosis.holds(45f, 60f));
        assertFalse("No vote is nothing to hold", RefreshSwitchDiagnosis.holds(0f, 60f));
    }

    @Test
    public void aHoldingScreenIsNeverBlamed() {
        Result result = diagnose(34, 60f, true, mode(3, 120f, 60f, 90f), seamlessPhone(), Conditions.NONE);
        assertEquals(Cause.HOLDS, result.cause);
        assertFalse(result.mismatch());
    }

    @Test
    public void aRateTheScreenNeverOffersIsNamedWithTheRatesItHas() {
        // 144 fps content on a 60/90/120 panel: no mode is 144 or a multiple of it.
        Result result = diagnose(34, 144f, true, mode(1, 60f, 90f, 120f), seamlessPhone(), Conditions.NONE);
        assertEquals(Cause.NO_MODE, result.cause);
        assertArrayEquals(new float[] {60f, 90f, 120f}, result.rates, 0f);
        assertEquals(60f, result.activeHz, 0f);
    }

    @Test
    public void onlyModesOfTheCurrentSizeCount() {
        // 120 Hz exists, but only at another resolution: the stream cannot reach it.
        List<PanelMode> modes = Arrays.asList(mode(1, 60f),
                new PanelMode(2, 720, 1600, 120f, NONE));
        Result result = diagnose(34, 120f, false, mode(1, 60f), modes, Conditions.NONE);
        assertEquals(Cause.NO_MODE, result.cause);
        assertArrayEquals(new float[] {60f}, result.rates, 0f);
    }

    @Test
    public void aReachableRateThatWouldBlankTheScreenIsNotSeamless() {
        // 60 Hz and 120 Hz in different config groups: neither lists the other.
        List<PanelMode> modes = Arrays.asList(mode(1, 60f), mode(2, 120f));
        Result result = diagnose(34, 120f, true, mode(1, 60f), modes, Conditions.NONE);
        assertEquals(Cause.NOT_SEAMLESS, result.cause);
    }

    @Test
    public void aVoteThatMayBlankPointsAtTheMatchContentFrameRateSetting() {
        // The same panel, but the vote allows a blanking switch: Android needs the user's opt-in.
        List<PanelMode> modes = Arrays.asList(mode(1, 60f), mode(2, 120f));
        Result result = diagnose(34, 120f, false, mode(1, 60f), modes, Conditions.NONE);
        assertEquals(Cause.NEEDS_MATCH_SETTING, result.cause);
        // A seamless mode that exists is never blamed on the setting, whichever the vote.
        assertEquals(Cause.UNEXPLAINED,
                diagnose(34, 120f, false, mode(1, 60f, 90f, 120f), seamlessPhone(), Conditions.NONE).cause);
        // Before Android 12 nothing is known about seamlessness, so the setting is not blamed either.
        assertEquals(Cause.UNEXPLAINED, diagnose(30, 120f, false, mode(1, 60f), modes, Conditions.NONE).cause);
    }

    @Test
    public void beforeAndroid12NoModeListsAlternativesSoNothingIsBlamedOnSeamlessness() {
        List<PanelMode> modes = Arrays.asList(mode(1, 60f), mode(2, 120f));
        Result result = diagnose(30, 120f, true, mode(1, 60f), modes, Conditions.NONE);
        assertEquals(Cause.UNEXPLAINED, result.cause);
        assertArrayEquals(new float[] {60f, 120f}, result.rates, 0f);
    }

    @Test
    public void aSeamlessModeThatAndroidStillDoesNotTakeIsSaidToBeUnexplained() {
        Result result = diagnose(34, 120f, true, mode(1, 60f, 90f, 120f), seamlessPhone(), Conditions.NONE);
        assertEquals(Cause.UNEXPLAINED, result.cause);
    }

    @Test
    public void theUsersRefreshRateSettingBelowTheVoteIsTheCause() {
        // Settings.System peak_refresh_rate = 60: Android's "Smooth display" is off.
        Result result = diagnose(34, 120f, true, mode(1, 60f, 90f, 120f), seamlessPhone(),
                new Conditions(false, false, 60f));
        assertEquals(Cause.USER_LIMIT, result.cause);
        assertEquals(60f, result.limitHz, 0f);
        // A limit that still allows the rate is not the cause.
        result = diagnose(34, 120f, true, mode(1, 60f, 90f, 120f), seamlessPhone(), new Conditions(false, false, 120f));
        assertEquals(Cause.UNEXPLAINED, result.cause);
    }

    @Test
    public void batterySaverAndHeatCapAppsAt60Hz() {
        Result saver = diagnose(34, 120f, true, mode(1, 60f, 90f, 120f), seamlessPhone(),
                new Conditions(true, false, 0f));
        assertEquals(Cause.BATTERY_SAVER, saver.cause);
        Result hot = diagnose(34, 120f, true, mode(1, 60f, 90f, 120f), seamlessPhone(),
                new Conditions(false, true, 0f));
        assertEquals(Cause.THERMAL, hot.cause);
        // Battery saver comes first when both apply: it is the one the user can change.
        Result both = diagnose(34, 120f, true, mode(1, 60f, 90f, 120f), seamlessPhone(),
                new Conditions(true, true, 60f));
        assertEquals(Cause.BATTERY_SAVER, both.cause);
    }

    @Test
    public void aSixtyHertzCapDoesNotExplainAVoteTheCapAllows() {
        // A 30 fps vote on a 60 Hz screen holds under every cap: it needs no switch.
        assertEquals(Cause.HOLDS, diagnose(34, 30f, true, mode(1, 60f), Arrays.asList(mode(1, 60f)),
                new Conditions(true, true, 60f)).cause);
        // Low rates the screen can not do at all are still NO_MODE, not the saver.
        assertEquals(Cause.NO_MODE, diagnose(34, 50f, true, mode(1, 60f), Arrays.asList(mode(1, 60f)),
                new Conditions(true, false, 0f)).cause);
    }

    @Test
    public void aMismatchIsShownOnlyAfterItHasLastedAndRestartsOnAChange() {
        RefreshSwitchDiagnosis.Watcher watcher = new RefreshSwitchDiagnosis.Watcher();
        Result unexplained = diagnose(34, 120f, true, mode(1, 60f, 90f, 120f), seamlessPhone(), Conditions.NONE);
        assertNull(watcher.observe(0, unexplained));
        assertNull(watcher.observe(RefreshSwitchDiagnosis.Watcher.SETTLE_MS - 1, unexplained));
        assertSame(unexplained, watcher.observe(RefreshSwitchDiagnosis.Watcher.SETTLE_MS, unexplained));

        // The screen follows: the verdict goes away at once and the wait starts over.
        Result holds = diagnose(34, 120f, true, mode(3, 120f, 60f, 90f), seamlessPhone(), Conditions.NONE);
        assertNull(watcher.observe(5_000, holds));
        assertNull(watcher.observe(6_000, unexplained));
        assertNull(watcher.observe(8_999, unexplained));
        assertSame(unexplained, watcher.observe(9_000, unexplained));

        // A new vote restarts the wait too: the old one had its chance, this one has not.
        Result other = diagnose(34, 90f, true, mode(1, 60f, 90f, 120f), seamlessPhone(), Conditions.NONE);
        assertNull(watcher.observe(9_500, other));
        assertNull(watcher.observe(12_000, other));
        assertSame(other, watcher.observe(12_500, other));

        // No result at all (the vote was withdrawn) clears the verdict.
        assertNull(watcher.observe(13_000, null));
        assertNull(watcher.observe(13_100, other));
    }

    @Test
    public void theLogLineNamesTheCauseTheNumbersAndTheKindOfVote() {
        Result result = diagnose(34, 120f, true, mode(1, 60f, 90f, 120f), seamlessPhone(),
                new Conditions(false, false, 60f));
        assertEquals("Refresh switch: USER_LIMIT, vote 120 Hz (seamless only), screen 60 Hz, modes 60, 90, 120," +
                " user limit 60 Hz", result.logLine(true));
        assertTrue(result.logLine(false).contains("may blank"));
    }

    @Test
    public void ratesAreFormattedWithoutNoiseAndWithTwoDecimalsWhenFractional() {
        assertEquals("60", RefreshSwitchDiagnosis.format(60.001f, Locale.ROOT));
        assertEquals("59.94", RefreshSwitchDiagnosis.format(59.94f, Locale.ROOT));
        assertEquals("48, 60, 119.88", RefreshSwitchDiagnosis.formatRates(new float[] {48f, 60f, 119.88f}, Locale.ROOT));
    }
}
