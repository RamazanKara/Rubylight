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
    public void theProbeFailsOnlyWhenTheNinetiethPercentileOverrunsItsShare() {
        long budget = 16_000_000L;
        long[] quick = new long[60];
        java.util.Arrays.fill(quick, 8_000_000L);
        assertFalse(GlassProbe.failed(quick, budget, 0.7f));
        long[] oneSpike = quick.clone();
        oneSpike[0] = 40_000_000L;
        assertFalse("a single hitch is no verdict", GlassProbe.failed(oneSpike, budget, 0.7f));
        long[] slow = new long[60];
        java.util.Arrays.fill(slow, 12_000_000L);
        assertTrue(GlassProbe.failed(slow, budget, 0.7f));
    }
}
