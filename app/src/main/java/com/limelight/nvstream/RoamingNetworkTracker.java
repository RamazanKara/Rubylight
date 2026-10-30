package com.limelight.nvstream;

import com.limelight.nvstream.jni.MoonBridge;

/**
 * Client roaming (Experimental): which default-network callbacks are a network change the
 * Rust engine must hear about. Plain Java, so the decisions are tested on the JVM;
 * {@link RoamingNetworkWatch} feeds it from ConnectivityManager.
 *
 * <p>The network the stream started on is no change. A new default network is one, and so is
 * the same network coming back after it was lost. Capability updates of the network already
 * reported are not: each report starts a roaming attempt in the engine.
 *
 * <p>Losing the stream's network, and a network coming back after that, are reported too: a
 * return the stream's connection cannot carry on over lets the stream reconnect at once
 * instead of waiting for its connection to time out. A network is often back before it has an
 * address toward the PC (IPv6 first, DHCP later), and then the connection carries on for now;
 * so for {@link #RECHECK_MS} each change of its addresses asks again, and an answer that is no
 * longer "carries on" is reported as the return.
 */
final class RoamingNetworkTracker {
    interface Reporter {
        /**
         * MoonBridge.networkChanged: metered is the new network's !NET_CAPABILITY_NOT_METERED.
         * Returns what it means for the stream (MoonBridge.NETWORK_*).
         */
        int networkChanged(boolean metered);

        /** The stream's network went away; the stream cannot reach the PC until one returns. */
        default void streamNetworkLost() {}

        /**
         * A default network after the stream's was lost: change is networkChanged's answer,
         * metered the new network's.
         */
        default void streamNetworkReturned(int change, boolean metered) {}
    }

    /** Elapsed milliseconds on a monotonic clock. */
    interface Clock {
        long millis();
    }

    /**
     * How long a return the connection carries on over is asked about again: the connection's
     * own timeout (NetworkReturnRecovery.PEER_TIMEOUT_MS), which decides after that.
     */
    static final long RECHECK_MS = 10000;

    private final Reporter reporter;
    private final Clock clock;
    private Object current;
    private boolean lost;
    private boolean stopped;
    // The network that came back with the connection carrying on, while it is asked again.
    private Object recheck;
    private boolean recheckMetered;
    private long recheckUntil;
    private Object recheckAddresses;

    /** streamNetwork: the default network when the stream started, or null if unknown. */
    RoamingNetworkTracker(Object streamNetwork, Reporter reporter) {
        this(streamNetwork, reporter, () -> System.nanoTime() / 1000000);
    }

    RoamingNetworkTracker(Object streamNetwork, Reporter reporter, Clock clock) {
        this.reporter = reporter;
        this.clock = clock;
        current = streamNetwork;
    }

    /** The default network's capabilities, from onCapabilitiesChanged (or onAvailable). */
    synchronized void capabilities(Object network, boolean notMetered) {
        if (stopped) {
            return;
        }
        if (current == null) {
            // Nothing known about the stream's network: the first one seen is where it runs.
            current = network;
            return;
        }
        if (network.equals(current) && !lost) {
            if (network.equals(recheck)) {
                recheckMetered = !notMetered;
            }
            return;
        }
        boolean returned = lost;
        current = network;
        lost = false;
        recheck = null;
        // Under the lock, so stop() returning means no report is still on its way.
        int change = reporter.networkChanged(!notMetered);
        if (returned) {
            if (change == MoonBridge.NETWORK_CARRIES_ON) {
                // The same address back, or no route yet: ask again when its addresses change.
                recheck = network;
                recheckMetered = !notMetered;
                recheckUntil = clock.millis() + RECHECK_MS;
                recheckAddresses = null;
            }
            reporter.streamNetworkReturned(change, !notMetered);
        }
    }

    /**
     * A network's addresses, from onLinkPropertiesChanged (any object that is equal for the
     * same addresses). For the network that came back with the connection carrying on, a change
     * within {@link #RECHECK_MS} asks the engine again: an address toward the PC that arrived
     * after the return (DHCP) may strand the connection, and then it is reported as the return.
     */
    synchronized void addresses(Object network, Object addresses) {
        if (stopped || lost || recheck == null || !network.equals(recheck)) {
            return;
        }
        if (clock.millis() > recheckUntil) {
            recheck = null;
            return;
        }
        if (addresses.equals(recheckAddresses)) {
            return;
        }
        recheckAddresses = addresses;
        int change = reporter.networkChanged(recheckMetered);
        if (change != MoonBridge.NETWORK_CARRIES_ON) {
            recheck = null;
            reporter.streamNetworkReturned(change, recheckMetered);
        }
    }

    /** The default network went away; the same network coming back is then a change. */
    synchronized void lost(Object network) {
        if (stopped || lost || !network.equals(current)) {
            return;
        }
        lost = true;
        recheck = null;
        reporter.streamNetworkLost();
    }

    /** The stream ends: nothing is reported after this returns. */
    synchronized void stop() {
        stopped = true;
    }
}
