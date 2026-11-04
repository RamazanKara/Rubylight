package com.limelight.nativeapi;

import androidx.annotation.VisibleForTesting;

import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;

/**
 * The last GET /update each host answered in this process, for the PC menu's "Update the PC's host", which decides
 * without asking the host. The computer manager reads it beside the capabilities of a host that lists
 * {@link NativePcPower#FEATURE_UPDATE} when a poll finds it online.
 */
public final class KnownUpdates {
    private static final Map<String, NativePcPower.Update> byHost = new ConcurrentHashMap<>();

    private KnownUpdates() {
    }

    static void record(String hostUuid, NativePcPower.Update update) {
        if (hostUuid != null) {
            byHost.put(hostUuid, update);
        }
    }

    /** The version the host would install, or null when it has none or was not read in this process. */
    public static String offered(String hostUuid) {
        NativePcPower.Update update = hostUuid == null ? null : byHost.get(hostUuid);
        return update == null ? null : update.offered();
    }

    /** The host is back on a new version: its last answer is out of date. */
    public static void forget(String hostUuid) {
        if (hostUuid != null) {
            byHost.remove(hostUuid);
        }
    }

    @VisibleForTesting
    public static void clear() {
        byHost.clear();
    }
}
