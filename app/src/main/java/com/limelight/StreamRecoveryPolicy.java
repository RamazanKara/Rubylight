package com.limelight;

/** A retry budget that survives replacing the stream activity; times use elapsed realtime. */
final class StreamRecoveryPolicy {
    static final int MAX_ATTEMPTS = 5;
    static final long WINDOW_MS = 60000;
    static final long STABLE_MS = 30000;
    private static final long[] DELAYS_MS = {1000, 2000, 4000, 8000, 12000};

    private int attempts;
    private long startedAt;

    StreamRecoveryPolicy(int attempts, long startedAt) {
        this.attempts = Math.max(0, Math.min(MAX_ATTEMPTS, attempts));
        this.startedAt = Math.max(0, startedAt);
    }

    long nextDelay(long now, long connectedAt) {
        resetAfterStableConnection(now, connectedAt);
        if (attempts == 0) startedAt = now;
        if (attempts >= MAX_ATTEMPTS || now < startedAt ||
                now - startedAt + DELAYS_MS[attempts] >= WINDOW_MS) return -1;
        return DELAYS_MS[attempts++];
    }

    void resetAfterStableConnection(long now, long connectedAt) {
        // Briefly starting a stream must not give a flapping connection an unlimited budget.
        if (connectedAt >= 0 && now - connectedAt >= STABLE_MS) {
            attempts = 0;
            startedAt = 0;
        }
    }

    int attempts() { return attempts; }
    long startedAt() { return startedAt; }

    static boolean transientTermination(int code) {
        // Native media timeouts and explicit transport errors. In particular, graceful (0),
        // protected content (-103), conversion (-104) and unrecognized host errors never retry.
        if (code == -100 || code == -101 || code == -102) return true;
        return transportTermination(code);
    }

    static boolean transportTermination(int code) {
        // Only ambiguous transport failures can stand in for a lost host-exit packet.
        // Media timeouts and an early host exit must retain their original meaning.
        switch (code) {
            case -1: // An ENet transport failure has no platform errno.
            case 32: case 54: case 57: case 60: case 64:
            case 100: case 101: case 102: case 103: case 104:
            case 107: case 110: case 111: case 112: case 113:
            case 10050: case 10051: case 10052: case 10053: case 10054:
            case 10057: case 10060: case 10061: case 10064: case 10065:
                return true;
            default: return false;
        }
    }

    static boolean transientStartup(String stage, int portFlags, int error) {
        // Startup retries are only used while recovering a previously connected session.
        // Host rejection, pairing, permission and decoder errors require a user decision.
        // NvConnection reports Java transport failures with the two HTTP port flags and
        // error 0. Its stage is the game's display name, not a native startup stage.
        if (portFlags == (0x0001 | 0x0002) && error == 0) return true;
        // Native RTSP/control startup propagates positive errno values (for example
        // ETIMEDOUT), while Java transport failures use 0. Positive host status codes
        // must still stay manual, so accept only the known transport errors here.
        return portFlags != 0 && (error <= 0 || transientTermination(error)) && stage != null &&
                !stage.toLowerCase(java.util.Locale.ROOT).contains("video");
    }
}
