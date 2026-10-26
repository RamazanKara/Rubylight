package com.limelight.nvstream.wol;

import com.limelight.LimeLog;
import com.limelight.nativeapi.NativeApiClient;
import com.limelight.nativeapi.NativeApiException;
import com.limelight.nativeapi.NativeCapabilities;
import com.limelight.nativeapi.NativeWake;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HostCapabilities;

import java.util.HashMap;
import java.util.Map;
import java.util.concurrent.Executor;
import java.util.concurrent.RejectedExecutionException;
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
    private final Executor requests;
    private final LongSupplier clock;
    /** The last ask of each PC that answers; a PC that went away is asked again once it answers. */
    private final Map<String, Ask> asks = new HashMap<>();

    /** One ask of one PC: what it reads is kept only while it is still the PC's last ask. */
    private static final class Ask {
        final long at;

        Ask(long at) {
            this.at = at;
        }
    }

    /** {@code requests} runs the HTTPS requests, so a poll never waits on them. */
    public WakeTargetsPoll(WakeTargets targets, Clients clients, Executor requests, LongSupplier clock) {
        this.targets = targets;
        this.clients = clients;
        this.requests = requests;
        this.clock = clock;
    }

    /** After each poll of the PC, on the polling thread; an ask that is due goes to the request executor. */
    public void polled(ComputerDetails computer) {
        String uuid = computer.uuid;
        if (uuid == null) {
            return;
        }
        long now = clock.getAsLong();
        Ask ask;
        synchronized (asks) {
            if (computer.state != ComputerDetails.State.ONLINE) {
                asks.remove(uuid);
                return;
            }
            Ask last = asks.get(uuid);
            if (!computer.caps.has(HostCapabilities.Feature.NATIVE_API) || (last != null && now - last.at < INTERVAL_MS)) {
                return;
            }
            // Reserved before the request, so a second caller (a PC being added while it is polled) sends none.
            ask = new Ask(now);
            asks.put(uuid, ask);
        }
        // The poll goes on updating the PC it polls.
        ComputerDetails asked = new ComputerDetails(computer);
        try {
            requests.execute(() -> ask(uuid, asked, ask));
        } catch (RejectedExecutionException e) {
            // The service is going away.
            drop(uuid, ask);
        }
    }

    /** A removed PC is asked nothing more, and an ask still in flight for it keeps nothing. */
    public void forget(String uuid) {
        synchronized (asks) {
            asks.remove(uuid);
        }
    }

    private void ask(String uuid, ComputerDetails computer, Ask ask) {
        NativeApiClient client = clients.open(computer);
        if (client == null) {
            // A PC not paired yet is asked on the first poll after it is.
            drop(uuid, ask);
            return;
        }
        try {
            NativeCapabilities capabilities = client.capabilities();
            NativeWake wake = capabilities.has(NativeCapabilities.WAKE) ? client.wake() : null;
            synchronized (asks) {
                // Stored under the lock, so a PC removed meanwhile keeps nothing of this ask.
                if (asks.get(uuid) != ask) {
                    return;
                }
                if (wake == null) {
                    targets.forget(uuid);
                } else {
                    targets.store(uuid, wake);
                }
            }
        } catch (NativeApiException e) {
            // A failed read keeps what is stored: those adapters still belong to the PC.
            LimeLog.info("Wake targets of " + uuid + " not read: " + e.getMessage());
        }
    }

    private void drop(String uuid, Ask ask) {
        synchronized (asks) {
            asks.remove(uuid, ask);
        }
    }
}
