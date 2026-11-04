package com.limelight.computers;

import com.limelight.nativeapi.NativeApiClient;
import com.limelight.nativeapi.NativeApiException;
import com.limelight.nativeapi.NativePcPower;

import java.util.concurrent.Executor;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.function.LongSupplier;

/**
 * Update the PC's host, as desktop Nightjar does: ask the host to install its pending update (POST /update/install),
 * then ask /serverinfo every {@link #POLL_MS} until the PC answers with the new version, for up to {@link #WAIT_MS}.
 * Host calls and the wait run on {@code worker}; every outcome reaches {@link Ui} on {@code main}.
 */
final class HostUpdateFlow {
    /** How long an update may take before Nightjar stops waiting for the PC to come back. */
    static final long WAIT_MS = 15 * 60_000;
    static final long POLL_MS = 5_000;

    interface Probe {
        /** The host version /serverinfo reports now, or null while the PC does not answer. */
        String runningVersion();
    }

    interface Sleeper {
        void sleep(long ms) throws InterruptedException;
    }

    interface Ui {
        /** The host took the request and installs {@code version}. */
        void started(String version);

        /** The PC answers with {@code version}. */
        void back(String version);

        /** The PC did not come back with {@code version} within {@link #WAIT_MS}. */
        void timedOut(String version);

        /** The host refused: its words as sent. */
        void refused(String message);

        /** The host could not be asked or its answer was unreadable. */
        void failed(NativeApiException error);
    }

    private final SleepFlow.Client client;
    private final Probe probe;
    private final Ui ui;
    private final Executor worker;
    private final Executor main;
    private final LongSupplier clockMs;
    private final Sleeper sleeper;
    private final AtomicBoolean cancelled = new AtomicBoolean();

    HostUpdateFlow(SleepFlow.Client client, Probe probe, Ui ui, Executor worker, Executor main, LongSupplier clockMs,
                   Sleeper sleeper) {
        this.client = client;
        this.probe = probe;
        this.ui = ui;
        this.worker = worker;
        this.main = main;
        this.clockMs = clockMs;
        this.sleeper = sleeper;
    }

    /** Installs; {@code offered} is the version the confirmation named, used when the host's answer names none. */
    void start(String offered) {
        worker.execute(() -> {
            String target;
            try {
                NativeApiClient api = client.open();
                String installing = api.installHostUpdate().offered();
                target = installing != null ? installing : offered;
            } catch (NativeApiException e) {
                main.execute(() -> {
                    if (PcPowerFlow.isRefusal(e)) {
                        ui.refused(e.getMessage());
                    } else {
                        ui.failed(e);
                    }
                });
                return;
            }
            main.execute(() -> ui.started(target));
            long started = clockMs.getAsLong();
            while (!cancelled.get() && clockMs.getAsLong() - started < WAIT_MS) {
                try {
                    sleeper.sleep(POLL_MS);
                } catch (InterruptedException e) {
                    Thread.currentThread().interrupt();
                    return;
                }
                if (!cancelled.get() && NativePcPower.arrived(probe.runningVersion(), target)) {
                    main.execute(() -> ui.back(target));
                    return;
                }
            }
            if (!cancelled.get()) {
                main.execute(() -> ui.timedOut(target));
            }
        });
    }

    /** Stops the wait: nothing more is asked or reported. */
    void cancel() {
        cancelled.set(true);
    }
}
