package com.limelight.nvstream;

import static org.junit.Assert.assertEquals;

import java.util.ArrayList;
import java.util.List;

import org.junit.Test;

/** Client roaming: which default-network callbacks reach the engine as a network change. */
public class RoamingNetworkTrackerTest {
    // Stand-ins for android.net.Network: the tracker only compares them.
    private static final Object WIFI = "wifi";
    private static final Object OTHER_WIFI = "other wifi";
    private static final Object CELLULAR = "cellular";

    private final List<Boolean> reports = new ArrayList<>();

    private RoamingNetworkTracker tracker(Object streamNetwork) {
        return new RoamingNetworkTracker(streamNetwork, reports::add);
    }

    @Test public void theStreamsOwnNetworkAndItsCapabilityUpdatesAreNoChange() {
        RoamingNetworkTracker tracker = tracker(WIFI);
        // Registering delivers the current default network; then its signal or bandwidth changes.
        tracker.capabilities(WIFI, true);
        tracker.capabilities(WIFI, true);
        tracker.capabilities(WIFI, false);
        assertEquals(new ArrayList<Boolean>(), reports);
    }

    @Test public void aNewDefaultNetworkIsReportedOnceWithWhetherItIsMetered() {
        RoamingNetworkTracker tracker = tracker(WIFI);
        tracker.capabilities(CELLULAR, false);
        tracker.capabilities(CELLULAR, false);
        tracker.capabilities(OTHER_WIFI, true);
        assertEquals(java.util.Arrays.asList(true, false), reports);
    }

    @Test public void theSameNetworkComingBackAfterItWasLostIsAChange() {
        RoamingNetworkTracker tracker = tracker(WIFI);
        tracker.lost(WIFI);
        tracker.capabilities(WIFI, true);
        tracker.capabilities(WIFI, true);
        assertEquals(java.util.Arrays.asList(false), reports);
        // Losing a network that is not the current one changes nothing.
        tracker.lost(CELLULAR);
        tracker.capabilities(WIFI, true);
        assertEquals(java.util.Arrays.asList(false), reports);
    }

    @Test public void withoutTheStartNetworkTheFirstOneSeenIsTheBaseline() {
        RoamingNetworkTracker tracker = tracker(null);
        tracker.capabilities(WIFI, true);
        assertEquals(new ArrayList<Boolean>(), reports);
        tracker.capabilities(CELLULAR, false);
        assertEquals(java.util.Arrays.asList(true), reports);
    }

    @Test public void nothingIsReportedAfterTheStreamEnds() {
        RoamingNetworkTracker tracker = tracker(WIFI);
        tracker.stop();
        tracker.capabilities(CELLULAR, false);
        tracker.lost(WIFI);
        tracker.capabilities(WIFI, true);
        assertEquals(new ArrayList<Boolean>(), reports);
    }
}
