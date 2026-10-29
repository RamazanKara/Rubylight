package com.limelight;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

/** Recovering after the stream's network dropped: when to reconnect, and the wait for a route. */
public class NetworkReturnRecoveryTest {
    @Test public void onlyANetworkBackTheConnectionCannotCarryOnOverReconnectsAtOnce() {
        assertTrue(NetworkReturnRecovery.reconnectsAtOnce(true, false, false, true, 0));
        // Roamed, or the same address back (not roamed) or no route yet: the connection carries on.
        assertFalse(NetworkReturnRecovery.reconnectsAtOnce(false, false, false, true, 0));
        // Automatic reconnection off: the user decides when the connection times out.
        assertFalse(NetworkReturnRecovery.reconnectsAtOnce(true, false, false, false, 0));
        // Mobile data only with "Continue on mobile data".
        assertFalse(NetworkReturnRecovery.reconnectsAtOnce(true, true, false, true, 0));
        assertTrue(NetworkReturnRecovery.reconnectsAtOnce(true, true, true, true, 0));
        // A network drop tolerance above the connection's own timeout holds it and decides.
        assertTrue(NetworkReturnRecovery.reconnectsAtOnce(true, false, false, true, NetworkReturnRecovery.PEER_TIMEOUT_MS));
        assertFalse(NetworkReturnRecovery.reconnectsAtOnce(true, false, false, true, 20000));
    }

    /**
     * Replays the sweep's Wi-Fi blip on a fake clock (ms on the device's clock from 09:02:00):
     * the network back at 12025 and reconnected at once, but its IPv4 address (DHCP) only at
     * 19777. The attempt starts within one check of the route, where the old backoff's second
     * attempt only came at 20685 after a failed one at 18545.
     */
    @Test public void theRecoveryStartsWithinACheckOfTheRouteComingBack() {
        long networkBack = 12025;
        long routeBack = 19777;
        long window = networkBack + StreamRecoveryPolicy.WINDOW_MS;
        NetworkReturnRecovery wait = new NetworkReturnRecovery(window);
        long now = networkBack;
        long go = -1;
        for (int checks = 0; checks < 1000 && go < 0; checks++) {
            long next = wait.next(now, now >= routeBack);
            if (next == 0) {
                go = now;
            } else {
                assertTrue("never gives up while the window runs", next > 0);
                assertEquals(NetworkReturnRecovery.RETRY_MS, next);
                now += next;
            }
        }
        assertTrue(go >= routeBack);
        assertTrue("started at " + go, go < routeBack + NetworkReturnRecovery.RETRY_MS);
        assertTrue(go < 20685);
        assertEquals(16, wait.unreachableChecks());
    }

    @Test public void aRouteThereAlreadyStartsWithoutWaiting() {
        NetworkReturnRecovery wait = new NetworkReturnRecovery(60000);
        assertEquals(0, wait.next(1000, true));
        assertEquals(0, wait.unreachableChecks());
    }

    @Test public void noRouteUntilTheWindowEndsGivesUp() {
        NetworkReturnRecovery wait = new NetworkReturnRecovery(10000);
        assertEquals(NetworkReturnRecovery.RETRY_MS, wait.next(0, false));
        // The last wait ends on the deadline, not past it.
        assertEquals(200, wait.next(9800, false));
        assertEquals(-1, wait.next(10000, false));
        assertEquals(-1, wait.next(12000, false));
    }

    @Test public void oneCheckFitsInsideTheRetryPace() {
        assertTrue(NetworkReturnRecovery.PROBE_TIMEOUT_MS < NetworkReturnRecovery.RETRY_MS);
    }
}
