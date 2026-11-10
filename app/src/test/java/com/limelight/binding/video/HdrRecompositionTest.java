package com.limelight.binding.video;

import org.junit.Test;

import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

/**
 * nightjar#93: a Vulkan HDR stream back from the home screen showed SDR colours until the stream menu's blur had the system
 * compose it on the GPU. The stream asks for that composition once per HDR start and once per return to the front.
 */
public class HdrRecompositionTest {
    @Test
    public void theFirstHdrOutputInFrontIsRecomposedOnce() {
        HdrRecomposition policy = new HdrRecomposition();
        assertTrue(policy.take(true, true));
        assertFalse("never on every poll", policy.take(true, true));
        assertFalse(policy.take(true, true));
    }

    @Test
    public void aReturnToTheFrontRecomposesAgain() {
        HdrRecomposition policy = new HdrRecomposition();
        assertTrue(policy.take(true, true));
        // Home, then back: onResume.
        policy.arm();
        assertTrue(policy.take(true, true));
        assertFalse(policy.take(true, true));
    }

    @Test
    public void itWaitsForTheWindowToBeInFront() {
        HdrRecomposition policy = new HdrRecomposition();
        policy.arm();
        assertFalse("menu open, picture-in-picture or no focus yet", policy.take(true, false));
        assertTrue("once in front", policy.take(true, true));
    }

    @Test
    public void itWaitsForTheHdrSwapchain() {
        HdrRecomposition policy = new HdrRecomposition();
        assertFalse("SDR before the host's HDR report", policy.take(false, true));
        assertTrue(policy.take(true, true));
        assertFalse(policy.take(true, true));
    }

    @Test
    public void hdrStartingAgainAfterSdrIsRecomposed() {
        HdrRecomposition policy = new HdrRecomposition();
        assertTrue(policy.take(true, true));
        assertFalse(policy.take(false, true));
        assertTrue(policy.take(true, true));
    }

    @Test
    public void onlyAnHdrSwapchainCounts() {
        assertTrue(HdrRecomposition.rendererShowsHdr("Vulkan HDR10"));
        assertTrue(HdrRecomposition.rendererShowsHdr("Vulkan HLG"));
        assertFalse("tone mapped", HdrRecomposition.rendererShowsHdr("Vulkan 10-bit SDR (tone mapped)"));
        assertFalse(HdrRecomposition.rendererShowsHdr("Vulkan 10-bit SDR"));
        assertFalse(HdrRecomposition.rendererShowsHdr("Vulkan 8-bit SDR"));
        assertFalse("no swapchain yet", HdrRecomposition.rendererShowsHdr("Vulkan "));
        assertFalse(HdrRecomposition.rendererShowsHdr(null));
    }
}
