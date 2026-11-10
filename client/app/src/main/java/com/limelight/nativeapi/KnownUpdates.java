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

    /**
     * The version the host would install, or null when it has none, was not read in this process, or already
     * installs it (or has it queued as an Install now). An automatic update waiting for an idle PC is still offered:
     * Install now helps it.
     */
    public static String offered(String hostUuid) {
        NativePcPower.Update update = hostUuid == null ? null : byHost.get(hostUuid);
        return update == null || update.installingNow ? null : update.offered();
    }

    /** Whether the host has answered GET /update in this process, so {@link #offered} speaks for it. */
    public static boolean answered(String hostUuid) {
        return hostUuid != null && byHost.containsKey(hostUuid);
    }

    /** The host took this device's install: no second one is offered until its next answer says otherwise. */
    static void installing(String hostUuid, NativePcPower.Update answer) {
        record(hostUuid, answer.installing());
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
