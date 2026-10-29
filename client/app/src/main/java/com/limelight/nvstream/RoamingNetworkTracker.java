package com.limelight.nvstream;

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
 * instead of waiting for its connection to time out.
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

    private final Reporter reporter;
    private Object current;
    private boolean lost;
    private boolean stopped;

    /** streamNetwork: the default network when the stream started, or null if unknown. */
    RoamingNetworkTracker(Object streamNetwork, Reporter reporter) {
        this.reporter = reporter;
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
            return;
        }
        boolean returned = lost;
        current = network;
        lost = false;
        // Under the lock, so stop() returning means no report is still on its way.
        int change = reporter.networkChanged(!notMetered);
        if (returned) {
            reporter.streamNetworkReturned(change, !notMetered);
        }
    }

    /** The default network went away; the same network coming back is then a change. */
    synchronized void lost(Object network) {
        if (stopped || lost || !network.equals(current)) {
            return;
        }
        lost = true;
        reporter.streamNetworkLost();
    }

    /** The stream ends: nothing is reported after this returns. */
    synchronized void stop() {
        stopped = true;
    }
}
