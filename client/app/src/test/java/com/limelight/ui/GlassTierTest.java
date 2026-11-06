package com.limelight.ui;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

/** Where a device lands is decided by what it can do, in the order the design gives. */
public class GlassTierTest {
    private static GlassTier at(int api) {
        return GlassTier.decide(api, false, false, false, false, null);
    }

    @Test
    public void theApiLevelSetsTheTopTier() {
        assertEquals(GlassTier.BAKED, at(21));
        assertEquals(GlassTier.BAKED, at(30));
        assertEquals(GlassTier.LIVE, at(31));
        assertEquals(GlassTier.LIVE, at(32));
        assertEquals(GlassTier.LENS, at(33));
    }

    @Test
    public void lowMemoryBatterySaverAndHeatLandOnBaked() {
        assertEquals(GlassTier.BAKED, GlassTier.decide(34, true, false, false, false, null));
        assertEquals(GlassTier.BAKED, GlassTier.decide(34, false, true, false, false, null));
        assertEquals(GlassTier.BAKED, GlassTier.decide(34, false, false, true, false, null));
    }

    @Test
    public void highContrastIsSolidOnEveryDevice() {
        assertEquals(GlassTier.SOLID, GlassTier.decide(21, true, true, true, true, null));
        assertEquals(GlassTier.SOLID, GlassTier.decide(34, false, false, false, true, GlassTier.LENS));
    }

    @Test
    public void aStoredVerdictOnlyEverLowersTheTier() {
        assertEquals(GlassTier.LIVE, GlassTier.decide(34, false, false, false, false, GlassTier.LIVE));
        assertEquals(GlassTier.BAKED, GlassTier.decide(34, false, false, false, false, GlassTier.BAKED));
        assertEquals(GlassTier.LIVE, GlassTier.decide(31, false, false, false, false, GlassTier.LENS));
    }

    @Test
    public void aFailedProbeStepsOneTierDown() {
        assertEquals(GlassTier.LIVE, GlassTier.LENS.stepDown());
        assertEquals(GlassTier.BAKED, GlassTier.LIVE.stepDown());
        assertEquals(GlassTier.BAKED, GlassTier.BAKED.stepDown());
    }

    @Test
    public void theProbeFailsAsSoonAsTheNinetiethPercentileCannotStayInsideItsShare() {
        // Sorted, the 90th percentile of 60 frames is the 54th: seven slow frames put it over, whatever the rest do.
        assertEquals(7, GlassProbe.slowFramesToFail(60));
        assertEquals(1, GlassProbe.slowFramesToFail(1));
    }

    @Test
    public void onlyAFrameBothOverItsShareAndLateIsSlow() {
        long budget = 8_333_000L;
        long allowed = (long) (budget * 0.7f);
        assertFalse("on time at 120 Hz, however low the clocks: Retroid", GlassProbe.slow(6_900_000L, 8_500_000L, allowed, budget));
        assertFalse("waking from idle is late, not heavy", GlassProbe.slow(3_000_000L, 20_600_000L, allowed, budget));
        assertTrue(GlassProbe.slow(12_000_000L, 13_800_000L, allowed, budget));
        assertTrue("a cheap TV at 60 Hz", GlassProbe.slow(50_000_000L, 61_000_000L, 11_666_000L, 16_666_000L));
    }

    @Test
    public void theProbeDoesNotJudgeTheFramesOfALaunch() {
        assertFalse("a first launch compiles its shaders here", GlassProbe.settled(1000, 1000 + GlassProbe.SETTLE_MILLIS - 1));
        assertTrue(GlassProbe.settled(1000, 1000 + GlassProbe.SETTLE_MILLIS));
    }
}
