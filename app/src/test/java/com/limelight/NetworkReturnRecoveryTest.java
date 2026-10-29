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

    // The engine's connection timeout, read from its source, is what the reconnect decision and
    // the tracker's second question assume (here and on the desktop, through nightjar-policy).
    @Test public void thePeerTimeoutIsTheEnginesConnectionTimeout() throws Exception {
        java.util.regex.Matcher engine = java.util.regex.Pattern.compile(
                "pub const CONTROL_PEER_TIMEOUT: Duration = Duration::from_secs\\((\\d+)\\);")
                .matcher(rust("client/src/control/driver.rs"));
        assertTrue("CONTROL_PEER_TIMEOUT in driver.rs", engine.find());
        int engineMs = Integer.parseInt(engine.group(1)) * 1000;
        java.util.regex.Matcher policy = java.util.regex.Pattern.compile(
                "pub const PEER_TIMEOUT_MS: u32 = ([\\d_]+);").matcher(rust("policy/src/recovery.rs"));
        assertTrue("PEER_TIMEOUT_MS in nightjar-policy", policy.find());
        assertEquals("nightjar-policy", engineMs, Integer.parseInt(policy.group(1).replace("_", "")));
        assertEquals(engineMs, NetworkReturnRecovery.PEER_TIMEOUT_MS);
        java.lang.reflect.Field recheck = Class.forName("com.limelight.nvstream.RoamingNetworkTracker")
                .getDeclaredField("RECHECK_MS");
        recheck.setAccessible(true);
        assertEquals("RoamingNetworkTracker.RECHECK_MS", engineMs, recheck.getLong(null));
    }

    private static String rust(String path) throws java.io.IOException {
        for (String root : new String[] {"../protocol/", "protocol/"}) {
            java.io.File file = new java.io.File(root + path);
            if (file.isFile()) {
                return new String(java.nio.file.Files.readAllBytes(file.toPath()),
                        java.nio.charset.StandardCharsets.UTF_8);
            }
        }
        throw new AssertionError(path + " not found from " + new java.io.File(".").getAbsolutePath());
    }
}
