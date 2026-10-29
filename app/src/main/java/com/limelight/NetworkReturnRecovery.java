package com.limelight;

/**
 * Recovering a stream after its network dropped. Plain Java with the clock passed in, so the
 * decisions are tested on the JVM; Game feeds it network events and route probes.
 *
 * <p>A network that comes back where the old connection cannot carry on (the route toward the
 * PC leaves from another address and the stream did not roam there) would only time out (the PC
 * times it out after 10 s of silence), so the stream reconnects at once instead. The same
 * address back, or no route yet, keeps the connection: it resumes in place, or its own timeout
 * decides. A Wi-Fi network is often back before it has an address toward the PC (DHCP after the
 * reconnect), and Android may validate it over IPv6 first, so each recovery waits while the PC
 * is unreachable (an immediate no-route error), checking every {@link #RETRY_MS}, instead of
 * failing once and backing off. A check that times out goes on to the attempt: a slow link is
 * not a missing one.
 */
final class NetworkReturnRecovery {
    /** Between two route checks while the PC is not reachable. */
    static final long RETRY_MS = 500;
    /** One route check's connect timeout: below RETRY_MS, so checks keep their pace. */
    static final int PROBE_TIMEOUT_MS = 400;

    private final long deadline;
    private int unreachable;

    /** deadline: elapsed realtime when the recovery gives up (its window ends). */
    NetworkReturnRecovery(long deadline) {
        this.deadline = deadline;
    }

    /** The connection's own timeout without a network drop tolerance: a tolerance up to it changes nothing. */
    static final int PEER_TIMEOUT_MS = 10000;

    /**
     * A network is back after the stream's was lost. True: reconnect now, since the engine said
     * the connection cannot carry on over it. Never when it roamed or carries on (the same
     * address back, or no route yet), with automatic reconnection off (the user decides), on a
     * metered network without "Continue on mobile data", or with a network drop tolerance above
     * {@link #PEER_TIMEOUT_MS} (it holds the connection, and decides).
     */
    static boolean reconnectsAtOnce(boolean cannotContinue, boolean metered, boolean mobileAllowed,
                                    boolean autoReconnect, int networkGraceMs) {
        return cannotContinue && (!metered || mobileAllowed) && autoReconnect && networkGraceMs <= PEER_TIMEOUT_MS;
    }

    /**
     * After a route check at {@code now}: 0 to reconnect now, a positive delay before the next
     * check, or -1 when the window ended without a route (the recovery gives up, loudly).
     */
    long next(long now, boolean reachable) {
        if (reachable) return 0;
        unreachable++;
        if (now >= deadline) return -1;
        return Math.min(RETRY_MS, deadline - now);
    }

    /** Route checks that found the PC unreachable so far. */
    int unreachableChecks() { return unreachable; }
}
