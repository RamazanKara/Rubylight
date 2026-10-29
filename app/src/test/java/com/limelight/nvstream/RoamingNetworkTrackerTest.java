package com.limelight.nvstream;

import static org.junit.Assert.assertEquals;

import java.util.ArrayList;
import java.util.List;

import org.junit.Test;

import com.limelight.nvstream.jni.MoonBridge;

/** Client roaming: which default-network callbacks reach the engine as a network change. */
public class RoamingNetworkTrackerTest {
    // Stand-ins for android.net.Network: the tracker only compares them.
    private static final Object WIFI = "wifi";
    private static final Object OTHER_WIFI = "other wifi";
    private static final Object CELLULAR = "cellular";

    private final List<Boolean> reports = new ArrayList<>();

    private RoamingNetworkTracker tracker(Object streamNetwork) {
        return new RoamingNetworkTracker(streamNetwork, metered -> {
            reports.add(metered);
            return MoonBridge.NETWORK_CARRIES_ON;
        });
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

    /** Records the loss and return reports, answering networkChanged with {@code answer}. */
    private static final class Events implements RoamingNetworkTracker.Reporter {
        final List<String> seen = new ArrayList<>();
        int answer = MoonBridge.NETWORK_CARRIES_ON;

        @Override public int networkChanged(boolean metered) {
            seen.add(metered ? "changed metered" : "changed");
            return answer;
        }
        @Override public void streamNetworkLost() { seen.add("lost"); }
        @Override public void streamNetworkReturned(int change, boolean metered) {
            seen.add("returned " + change + (metered ? " metered" : ""));
        }
    }

    // The sweep's Wi-Fi blip: network 108 lost, network 109 back (a new object), the engine's
    // answer passed on as it is: here the connection carries on (same address, or no route yet).
    @Test public void aLostNetworkComingBackIsAReturnWithTheEnginesAnswer() {
        Events events = new Events();
        RoamingNetworkTracker tracker = new RoamingNetworkTracker(WIFI, events);
        tracker.lost(WIFI);
        tracker.lost(WIFI);
        tracker.capabilities(OTHER_WIFI, true);
        // Validation later updates the same network's capabilities: no second return.
        tracker.capabilities(OTHER_WIFI, true);
        assertEquals(java.util.Arrays.asList("lost", "changed", "returned " + MoonBridge.NETWORK_CARRIES_ON), events.seen);

        events.seen.clear();
        events.answer = MoonBridge.NETWORK_CANNOT_CONTINUE;
        tracker.lost(OTHER_WIFI);
        tracker.capabilities(CELLULAR, false);
        assertEquals(java.util.Arrays.asList("lost", "changed metered",
                "returned " + MoonBridge.NETWORK_CANNOT_CONTINUE + " metered"), events.seen);
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
