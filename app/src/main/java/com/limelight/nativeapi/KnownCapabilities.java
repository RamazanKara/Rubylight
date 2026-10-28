package com.limelight.nativeapi;

import androidx.annotation.VisibleForTesting;

import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;

/**
 * The last GET /capabilities each host answered in this process, as the {@code capabilities} op returned it, for
 * menus that decide without asking the host (see
 * {@link com.limelight.nvstream.http.HostCapabilities#withNativeCapabilities}). The computer manager reads every
 * native host's capabilities when a poll first finds it online, so a menu opened after that knows them. It lacks the
 * file permissions the {@code files.capabilities} op adds: gate files on that op, not on this.
 */
public final class KnownCapabilities {
    private static final Map<String, String> byHost = new ConcurrentHashMap<>();

    private KnownCapabilities() {
    }

    static void record(String hostUuid, String json) {
        if (hostUuid != null) {
            byHost.put(hostUuid, json);
        }
    }

    /** The host's capabilities JSON, or null when it was not read in this process. */
    public static String of(String hostUuid) {
        return hostUuid == null ? null : byHost.get(hostUuid);
    }

    @VisibleForTesting
    public static void clear() {
        byHost.clear();
    }
}
