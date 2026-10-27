package com.limelight.nvstream;

import org.robolectric.util.ReflectionHelpers;

// Waits for a test connection's startup thread. Nothing in it sleeps or polls, so it only fails to
// finish when it hangs; the bound turns a hang into a failure and is not a speed check, which a
// loaded test machine (Robolectric under a parallel build) would miss now and then.
public final class ConnectionStartup {
    public static final long HANG_GUARD_MS = 60_000;

    private ConnectionStartup() {}

    public static Thread await(NvConnection connection) throws InterruptedException {
        Thread worker = ReflectionHelpers.getField(connection, "connectionThread");
        worker.join(HANG_GUARD_MS);
        return worker;
    }
}
