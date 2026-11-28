package com.limelight.nativeapi;

import androidx.annotation.VisibleForTesting;

import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;

/**
 * The last GET /capabilities each host answered in this process, as the {@code capabilities} op returned it, for
 * menus that decide without asking the host (see
 * {@link com.limelight.nvstream.http.HostCapabilities#withNativeCapabilities}). The computer manager reads every
 * native host's capabilities when a poll first finds it online and again while it polls, so a menu opened after that
 * knows them and a permission changed in the console arrives within half a minute. It lacks the file permissions the {@code files.capabilities} op adds: gate files on that op, not on this.
 */
public final class KnownCapabilities {
    /** Observer invoked after native capabilities JSON is cached for a host. */
    public interface Listener {
        void onCapabilitiesRecorded(String hostUuid);
    }

    private static final Map<String, String> byHost = new ConcurrentHashMap<>();
    private static volatile Listener listener;

    private KnownCapabilities() {
    }

    public static void setListener(Listener newListener) {
        listener = newListener;
    }

    static void record(String hostUuid, String json) {
        if (hostUuid != null) {
            byHost.put(hostUuid, json);
            Listener active = listener;
            if (active != null) {
                active.onCapabilitiesRecorded(hostUuid);
            }
        }
    }

    /** The host's capabilities JSON, or null when it was not read in this process. */
    public static String of(String hostUuid) {
        return hostUuid == null ? null : byHost.get(hostUuid);
    }

    /** Clears cached native capabilities when a host is forgotten or unpaired. */
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
