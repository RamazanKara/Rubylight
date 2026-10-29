package com.limelight;

/**
 * Recovering a stream after its network dropped. Plain Java with the clock passed in, so the
 * decisions are tested on the JVM; Game feeds it network events and route probes.
 *
 * <p>A network that comes back without the stream roaming onto it cannot carry the old
 * connection on (the PC times it out after 10 s of silence), so the stream reconnects at once
 * instead of waiting for its own timeout. A Wi-Fi network is often back before it has an address
 * toward the PC (DHCP after the reconnect), and Android may validate it over IPv6 first, so each
 * recovery waits until the PC is reachable, checking every {@link #RETRY_MS}, instead of failing
 * once and backing off.
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

    /**
     * A network is back after the stream's was lost. True: reconnect now. Not when the stream
     * roamed onto it (it carries on), with automatic reconnection off (the user decides), or with
     * a network drop tolerance (it holds the connection, and decides).
     */
    static boolean reconnectsAtOnce(boolean roamed, boolean autoReconnect, int networkGraceSeconds) {
        return !roamed && autoReconnect && networkGraceSeconds <= 0;
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
