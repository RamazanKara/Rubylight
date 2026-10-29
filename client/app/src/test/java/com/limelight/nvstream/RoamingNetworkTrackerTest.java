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

    /** Records the loss and return reports, answering networkChanged with {@code roams}. */
    private static final class Events implements RoamingNetworkTracker.Reporter {
        final List<String> seen = new ArrayList<>();
        boolean roams;

        @Override public boolean networkChanged(boolean metered) {
            seen.add(metered ? "changed metered" : "changed");
            return roams;
        }
        @Override public void streamNetworkLost() { seen.add("lost"); }
        @Override public void streamNetworkReturned(boolean roamed) { seen.add(roamed ? "returned roamed" : "returned"); }
    }

    // The sweep's Wi-Fi blip: network 108 lost, network 109 back (a new object) and not roamed onto.
    @Test public void aLostNetworkComingBackIsAReturnWithTheEnginesAnswer() {
        Events events = new Events();
        RoamingNetworkTracker tracker = new RoamingNetworkTracker(WIFI, events);
        tracker.lost(WIFI);
        tracker.lost(WIFI);
        tracker.capabilities(OTHER_WIFI, true);
        // Validation later updates the same network's capabilities: no second return.
        tracker.capabilities(OTHER_WIFI, true);
        assertEquals(java.util.Arrays.asList("lost", "changed", "returned"), events.seen);

        events.seen.clear();
        events.roams = true;
        tracker.lost(OTHER_WIFI);
        tracker.capabilities(CELLULAR, false);
        assertEquals(java.util.Arrays.asList("lost", "changed metered", "returned roamed"), events.seen);
    }

    @Test public void aNewNetworkWhileTheStreamsOwnStaysIsNoReturn() {
        Events events = new Events();
        RoamingNetworkTracker tracker = new RoamingNetworkTracker(WIFI, events);
        tracker.capabilities(OTHER_WIFI, true);
        // Losing a network that is not the stream's is no loss either.
        tracker.lost(WIFI);
        assertEquals(java.util.Arrays.asList("changed"), events.seen);
    }

    @Test public void nothingIsReportedAfterTheStreamEnds() {
        RoamingNetworkTracker tracker = tracker(WIFI);
        tracker.stop();
        tracker.capabilities(CELLULAR, false);
        tracker.lost(WIFI);
        tracker.capabilities(WIFI, true);
        assertEquals(new ArrayList<Boolean>(), reports);

        Events events = new Events();
        RoamingNetworkTracker ended = new RoamingNetworkTracker(WIFI, events);
        ended.stop();
        ended.lost(WIFI);
        ended.capabilities(OTHER_WIFI, true);
        assertEquals(new ArrayList<String>(), events.seen);
    }
}
