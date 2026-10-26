package com.limelight.nvstream.wol;

import com.limelight.LimeLog;
import com.limelight.nativeapi.NativeApiClient;
import com.limelight.nativeapi.NativeApiException;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HostCapabilities;

import java.util.HashMap;
import java.util.Map;
import java.util.function.LongSupplier;

/**
 * Keeps the wake targets of the PCs the computer manager polls, so a PC woken from the PC list or a shortcut reaches
 * every adapter its {@code /wake} lists, not only the PCs the console or a stream already asked. A PC is asked when a
 * poll first finds it online and again every {@link #INTERVAL_MS} while it stays so: its adapters change with
 * hardware, not between polls.
 */
public final class WakeTargetsPoll {
    static final long INTERVAL_MS = 10 * 60_000;

    public interface Clients {
        /** A client for the PC's native API, or null when it has no address or pinned certificate yet. */
        NativeApiClient open(ComputerDetails computer);
    }

    private final WakeTargets targets;
    private final Clients clients;
    private final LongSupplier clock;
    /** When each PC that answers was last asked; a PC that went away is asked again once it answers. */
    private final Map<String, Long> askedAt = new HashMap<>();

    public WakeTargetsPoll(WakeTargets targets, Clients clients, LongSupplier clock) {
        this.targets = targets;
        this.clients = clients;
        this.clock = clock;
    }

    /** After each poll of the PC, on the polling thread (blocks on HTTPS when an ask is due). */
    public void polled(ComputerDetails computer) {
        String uuid = computer.uuid;
        if (uuid == null) {
            return;
        }
        long now = clock.getAsLong();
        synchronized (askedAt) {
            if (computer.state != ComputerDetails.State.ONLINE) {
                askedAt.remove(uuid);
                return;
            }
            Long at = askedAt.get(uuid);
            if (!computer.caps.has(HostCapabilities.Feature.NATIVE_API) || (at != null && now - at < INTERVAL_MS)) {
                return;
            }
        }
        // A PC not paired yet is asked on the first poll after it is.
        NativeApiClient client = clients.open(computer);
        if (client == null) {
            return;
        }
        synchronized (askedAt) {
            askedAt.put(uuid, now);
        }
        try {
            targets.refresh(uuid, client.capabilities(), client);
        } catch (NativeApiException e) {
            LimeLog.info("Wake targets of " + uuid + " not read: " + e.getMessage());
        }
    }
}
