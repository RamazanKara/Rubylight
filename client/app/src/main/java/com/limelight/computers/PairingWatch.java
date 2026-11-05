package com.limelight.computers;

import androidx.annotation.VisibleForTesting;

import com.limelight.LimeLog;
import com.limelight.nativeapi.NativeApiException;
import com.limelight.nativeapi.NativePairing;
import com.limelight.nvstream.http.ComputerDetails;

import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.concurrent.Executor;
import java.util.function.LongSupplier;
import java.util.function.Predicate;

/**
 * The devices waiting to pair with the PCs this device may approve for. It has no timer: the screen calls {@link #poll}
 * from its host updates, so nothing runs while nothing is on screen, and a PC without the capability is never asked.
 */
public final class PairingWatch {
    /** The most often a screen's host updates ask. */
    private static final long INTERVAL_MS = 4000;

    public interface Reader {
        List<NativePairing.Request> read(ComputerDetails host) throws NativeApiException;
    }

    public interface Listener {
        void onWaiting(List<Waiting> waiting);
    }

    /** A device waiting on one PC. */
    public static final class Waiting {
        public final ComputerDetails host;
        public final NativePairing.Request request;

        public Waiting(ComputerDetails host, NativePairing.Request request) {
            this.host = host;
            this.request = request;
        }
    }

    private final Reader reader;
    private final Predicate<ComputerDetails> asks;
    private final Executor worker;
    private final Executor main;
    private final LongSupplier clock;
    private final Listener listener;
    private boolean reading;
    private boolean stale = true;
    private long lastRead;
    private int generation;
    private List<Waiting> shown = Collections.emptyList();

    public PairingWatch(Reader reader, Predicate<ComputerDetails> asks, Executor worker, Executor main,
                        Listener listener) {
        this(reader, asks, worker, main, android.os.SystemClock::elapsedRealtime, listener);
    }

    @VisibleForTesting
    PairingWatch(Reader reader, Predicate<ComputerDetails> asks, Executor worker, Executor main, LongSupplier clock,
                 Listener listener) {
        this.reader = reader;
        this.asks = asks;
        this.worker = worker;
        this.main = main;
        this.clock = clock;
        this.listener = listener;
    }

    /** The screen's hosts were polled. A PC that doesn't answer drops out, logged. */
    public void poll(List<ComputerDetails> hosts) {
        final List<ComputerDetails> asked = new ArrayList<>();
        for (ComputerDetails host : hosts) {
            if (asks.test(host)) {
                asked.add(host);
            }
        }
        if (asked.isEmpty()) {
            generation++;
            reading = false;
            publish(Collections.emptyList());
            return;
        }
        long now = clock.getAsLong();
        if (reading || (!stale && now - lastRead < INTERVAL_MS)) {
            return;
        }
        reading = true;
        stale = false;
        lastRead = now;
        final int round = ++generation;
        worker.execute(() -> {
            final List<Waiting> waiting = new ArrayList<>();
            for (ComputerDetails host : asked) {
                try {
                    for (NativePairing.Request request : reader.read(host)) {
                        waiting.add(new Waiting(host, request));
                    }
                } catch (NativeApiException e) {
                    LimeLog.warning("Devices waiting to pair on " + host.uuid + ": " + e.getMessage());
                }
            }
            main.execute(() -> {
                if (round == generation) {
                    reading = false;
                    publish(waiting);
                }
            });
        });
    }

    /** The next {@link #poll} reads at once: the list just changed. */
    public void refreshSoon() {
        stale = true;
    }

    /** The screen left: an answer still on its way is dropped, and the next poll reads at once. */
    public void pause() {
        generation++;
        reading = false;
        stale = true;
    }

    private void publish(List<Waiting> waiting) {
        if (!sameRequests(shown, waiting)) {
            shown = waiting;
            listener.onWaiting(waiting);
        }
    }

    private static boolean sameRequests(List<Waiting> a, List<Waiting> b) {
        if (a.size() != b.size()) {
            return false;
        }
        for (int i = 0; i < a.size(); i++) {
            if (!a.get(i).request.id.equals(b.get(i).request.id)) {
                return false;
            }
        }
        return true;
    }
}
