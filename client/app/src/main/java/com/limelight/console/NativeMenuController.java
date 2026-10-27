package com.limelight.console;

import com.limelight.nativeapi.NativeApiClient;
import com.limelight.nativeapi.NativeApiException;
import com.limelight.nativeapi.NativeRadeon;

import java.util.Collections;

/**
 * Keeps the Radeon panel of the stream menu current: reads GET /radeon while the panel is shown (at most
 * once a second, the next read starts a second after the previous one finished) and sends writes.
 * Nothing here runs on the render, decode or input threads: every native call is scheduled on the
 * {@link Scheduler}, which in the app is one background thread, so a write never overlaps a read and a
 * read never finishes out of order. The listener is called on the main thread through {@link Main}.
 *
 * A host from before the Radeon endpoints answers 404; the panel is then hidden and polling stops.
 */
public final class NativeMenuController {
    /** The pause between the end of one read and the start of the next. */
    public static final long POLL_INTERVAL_MS = 1000;

    public interface ClientSource {
        /** The client of the connected host; null when the host has no native API. */
        NativeApiClient client();
    }

    /** Runs a task on the one background thread after a delay. */
    public interface Scheduler {
        void schedule(Runnable task, long delayMs);
    }

    public interface Main {
        void post(Runnable task);
    }

    public interface Listener {
        void onState(State state);
    }

    /** What the panel shows right now. Immutable. */
    public static final class State {
        public static final State INITIAL = new State(null, false, null, null, null);

        /** The last host answer, or null before the first one. */
        public final RadeonPanelModel model;
        /** The host has no Radeon endpoints: draw no panel. */
        public final boolean hidden;
        /** Why there is no model yet (the host could not be reached); null otherwise. */
        public final RadeonPanelModel.Failure loadFailure;
        /** The last write that failed, shown at its setting; null when none. */
        public final RadeonPanelModel.Failure writeFailure;
        /** The setting a write is in flight for, or null. */
        public final String pendingId;

        State(RadeonPanelModel model, boolean hidden, RadeonPanelModel.Failure loadFailure,
              RadeonPanelModel.Failure writeFailure, String pendingId) {
            this.model = model;
            this.hidden = hidden;
            this.loadFailure = loadFailure;
            this.writeFailure = writeFailure;
            this.pendingId = pendingId;
        }

        public boolean loaded() {
            return model != null || hidden || loadFailure != null;
        }

        State with(RadeonPanelModel model, boolean hidden, RadeonPanelModel.Failure loadFailure,
                   RadeonPanelModel.Failure writeFailure, String pendingId) {
            return new State(model, hidden, loadFailure, writeFailure, pendingId);
        }
    }

    private final ClientSource source;
    private final Scheduler scheduler;
    private final Main main;

    private final Object lock = new Object();
    private Listener listener;
    private int generation;
    private State state = State.INITIAL;

    public NativeMenuController(ClientSource source, Scheduler scheduler, Main main) {
        this.source = source;
        this.scheduler = scheduler;
        this.main = main;
    }

    /** Call when the panel is shown. Delivers the current state at once, then each change. */
    public void start(Listener listener) {
        final int mine;
        synchronized (lock) {
            this.listener = listener;
            mine = ++generation;
            // A host that said 404 is not asked again within one stream; everything else is read afresh.
            state = state.hidden ? state : State.INITIAL.with(state.model, false, null, null, null);
        }
        deliver();
        if (!state.hidden) {
            scheduler.schedule(() -> poll(mine), 0);
        }
    }

    /** Call when the panel is hidden or the menu closes: ends the polling and drops queued reads. */
    public void stop() {
        synchronized (lock) {
            generation++;
            listener = null;
        }
    }

    public State state() {
        synchronized (lock) {
            return state;
        }
    }

    /** Sends one change; the answer replaces the panel. Returns at once. */
    public void write(RadeonPanelModel.Row row, Object requested) {
        final int mine;
        final Object value = RadeonPanelModel.writableValue(row, requested);
        synchronized (lock) {
            mine = generation;
            state = state.with(state.model, false, state.loadFailure, null, row.id);
        }
        deliver();
        scheduler.schedule(() -> send(mine, row, value), 0);
    }

    private void send(int mine, RadeonPanelModel.Row row, Object value) {
        NativeApiClient client = source.client();
        RadeonPanelModel model = null;
        RadeonPanelModel.Failure failure = null;
        boolean hidden = false;
        if (client == null) {
            hidden = true;
        } else {
            try {
                NativeRadeon answer = client.writeRadeon(row.gpu,
                        Collections.singletonList(new NativeRadeon.Change(row.id, value)));
                model = RadeonPanelModel.of(answer);
            } catch (NativeApiException e) {
                if (RadeonPanelModel.isMissing(e)) {
                    hidden = true;
                } else {
                    failure = RadeonPanelModel.Failure.of(e, row.id);
                }
            }
        }
        synchronized (lock) {
            if (hidden) {
                state = state.with(null, true, null, null, null);
            } else if (model != null) {
                state = state.with(model, false, null, null, null);
            } else {
                state = state.with(state.model, false, state.loadFailure, failure, null);
            }
        }
        deliver(mine);
        if (failure != null) {
            // The value on the host may differ from what the control shows: read it back now.
            poll(mine, false);
        }
    }

    private void poll(int mine) {
        poll(mine, true);
    }

    private void poll(int mine, boolean reschedule) {
        synchronized (lock) {
            if (mine != generation) {
                return;
            }
        }
        NativeApiClient client = source.client();
        RadeonPanelModel model = null;
        RadeonPanelModel.Failure failure = null;
        boolean hidden = false;
        if (client == null) {
            hidden = true;
        } else {
            try {
                model = RadeonPanelModel.of(client.radeon());
            } catch (NativeApiException e) {
                if (RadeonPanelModel.isMissing(e)) {
                    hidden = true;
                } else {
                    failure = RadeonPanelModel.Failure.of(e, null);
                }
            }
        }
        synchronized (lock) {
            if (mine != generation) {
                return;
            }
            if (hidden) {
                state = state.with(null, true, null, null, null);
            } else if (model != null) {
                RadeonPanelModel shown = state.model;
                // Keep the model object the screen already draws when nothing changed, so it is not rebuilt.
                if (shown != null && shown.sameAs(model)) {
                    model = shown;
                }
                state = state.with(model, false, null, state.writeFailure, state.pendingId);
            } else if (state.model == null) {
                state = state.with(null, false, failure, null, state.pendingId);
            }
            // A failed read with a panel on screen keeps the panel; the next read tries again.
        }
        deliver(mine);
        if (reschedule && !hidden) {
            scheduler.schedule(() -> poll(mine), POLL_INTERVAL_MS);
        }
    }

    private void deliver() {
        deliver(-1);
    }

    private void deliver(int mine) {
        final Listener to;
        final State snapshot;
        synchronized (lock) {
            if (mine >= 0 && mine != generation) {
                return;
            }
            to = listener;
            snapshot = state;
        }
        if (to != null) {
            main.post(() -> {
                synchronized (lock) {
                    if (listener != to) {
                        return;
                    }
                }
                to.onState(snapshot);
            });
        }
    }
}
