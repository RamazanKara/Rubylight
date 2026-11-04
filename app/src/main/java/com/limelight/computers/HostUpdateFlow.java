package com.limelight.computers;

import com.limelight.nativeapi.NativeApiClient;
import com.limelight.nativeapi.NativeApiException;
import com.limelight.nativeapi.NativePcPower;

import java.util.Objects;
import java.util.concurrent.Executor;
import java.util.concurrent.atomic.AtomicBoolean;

/**
 * Update the PC's host, as desktop Nightjar's follow_update does: ask the host to install its pending update (POST
 * /update/install), then every {@link #POLL_MS} ask /serverinfo whether the PC is back on the new version and GET
 * /update how the install goes, for up to {@link #WAIT_MS}. The updater's failure, or a PC back on the old version
 * with nothing under way, ends the wait; time spent waiting for another device's stream does not count. Host calls
 * and the wait run on {@code worker}; every outcome reaches {@link Ui} on {@code main}.
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

        /** The install waits for another device's stream: the host's words, once per reason. */
        void waiting(String reason);

        /** The PC answers with {@code version}. */
        void back(String version);

        /** The PC did not come back with {@code version} within {@link #WAIT_MS}. */
        void timedOut(String version);

        /** The host's 202 offers no version to install: nothing to wait for. */
        void nothingToInstall();

        /** The PC's updater failed: its words, or null when it gave none. */
        void updateFailed(String error);

        /** The PC came back on {@code version}, not the new one: setup gave up, with the updater's error or null. */
        void cameBackOn(String version, String error);

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
    private final Sleeper sleeper;
    private final AtomicBoolean cancelled = new AtomicBoolean();

    HostUpdateFlow(SleepFlow.Client client, Probe probe, Ui ui, Executor worker, Executor main, Sleeper sleeper) {
        this.client = client;
        this.probe = probe;
        this.ui = ui;
        this.worker = worker;
        this.main = main;
        this.sleeper = sleeper;
    }

    void start() {
        worker.execute(() -> {
            NativeApiClient api;
            String target;
            try {
                api = client.open();
                target = api.installHostUpdate().offered();
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
            if (target == null) {
                // No 15-minute wait for a version that isn't coming.
                main.execute(ui::nothingToInstall);
                return;
            }
            main.execute(() -> ui.started(target));
            follow(api, target);
        });
    }

    private void follow(NativeApiClient api, String target) {
        long polls = WAIT_MS / POLL_MS;
        long counted = 0;
        boolean gone = false;
        String waiting = null;
        while (counted < polls) {
            try {
                sleeper.sleep(POLL_MS);
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
                return;
            }
            if (cancelled.get()) {
                return;
            }
            if (NativePcPower.arrived(probe.runningVersion(), target)) {
                main.execute(() -> ui.back(target));
                return;
            }
            NativePcPower.Update update;
            try {
                update = api.hostUpdate(target);
            } catch (NativeApiException e) {
                // Down while it installs.
                gone = true;
                counted++;
                continue;
            }
            if (update.failed) {
                main.execute(() -> ui.updateFailed(update.failure));
                return;
            }
            if (gone && update.gaveUp) {
                main.execute(() -> ui.cameBackOn(update.currentVersion, update.error));
                return;
            }
            if (!Objects.equals(update.waitingFor, waiting)) {
                waiting = update.waitingFor;
                String reason = waiting;
                if (reason != null) {
                    main.execute(() -> ui.waiting(reason));
                }
            }
            if (waiting == null) {
                counted++;
            }
        }
        if (!cancelled.get()) {
            main.execute(() -> ui.timedOut(target));
        }
    }

    /** Stops the wait: nothing more is asked or reported. */
    void cancel() {
        cancelled.set(true);
    }
}
