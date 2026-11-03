package com.limelight.console;

import android.app.Activity;
import android.app.Service;
import android.content.ComponentName;
import android.content.Intent;
import android.content.ServiceConnection;
import android.os.IBinder;

import com.limelight.computers.ComputerManagerListener;
import com.limelight.computers.ComputerManagerService;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.PairingManager;

import java.util.ArrayList;
import java.util.HashSet;
import java.util.Iterator;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.Set;

/**
 * The paired hosts the app knows, for Home and the game detail. It binds the computer manager service as
 * the list of PCs does, polls while the screen is in front and calls the listener on the UI thread with
 * every change. Use it from the UI thread.
 */
final class HostFeed {
    interface Listener {
        /** A host was added or its state changed. */
        void onHostsChanged();
    }

    private final Activity activity;
    private final Listener listener;
    private final Map<String, ComputerDetails> hosts = new LinkedHashMap<>();
    /** Saved PCs paired before that have not answered yet; on a start each one is like that until its first poll. */
    private final Set<String> unanswered = new HashSet<>();
    private volatile ComputerManagerService.ComputerManagerBinder binder;
    private boolean bound;
    private boolean polling;
    /** The manager has reported the PCs it saved. */
    private boolean reported;
    private boolean foreground;
    private Thread waiter;

    private final ServiceConnection connection = new ServiceConnection() {
        @Override
        public void onServiceConnected(ComponentName name, IBinder service) {
            // No binder (as under Robolectric) leaves nothing to wait for.
            if (!(service instanceof ComputerManagerService.ComputerManagerBinder)) return;
            ComputerManagerService.ComputerManagerBinder candidate = (ComputerManagerService.ComputerManagerBinder) service;
            // Waiting for the service to be ready can take a while; keep it off the UI thread.
            waiter = new Thread(() -> {
                if (!candidate.waitForReady() || activity.isFinishing() || activity.isDestroyed()) {
                    return;
                }
                activity.runOnUiThread(() -> {
                    if (!bound || activity.isFinishing() || activity.isDestroyed()) {
                        return;
                    }
                    binder = candidate;
                    startPolling();
                });
            }, "console-host-feed");
            waiter.start();
        }

        @Override
        public void onServiceDisconnected(ComponentName name) {
            binder = null;
            polling = false;
        }
    };

    HostFeed(Activity activity, Listener listener) {
        this.activity = activity;
        this.listener = listener;
    }

    void bind() {
        bound = activity.bindService(new Intent(activity, ComputerManagerService.class), connection,
                Service.BIND_AUTO_CREATE);
        if (!bound) {
            // Nothing will report the saved PCs, so there is nothing to wait for.
            com.limelight.LimeLog.warning("Home could not bind the computer manager");
            reported = true;
        }
    }

    void unbind() {
        if (waiter != null) {
            waiter.interrupt();
        }
        if (bound) {
            activity.unbindService(connection);
            bound = false;
        }
        binder = null;
        polling = false;
    }

    void resume() {
        foreground = true;
        startPolling();
    }

    void pause() {
        foreground = false;
        ComputerManagerService.ComputerManagerBinder current = binder;
        if (current != null && polling) {
            current.stopPolling();
            polling = false;
        }
    }

    private void startPolling() {
        ComputerManagerService.ComputerManagerBinder current = binder;
        if (current == null || polling || !foreground) {
            return;
        }
        polling = true;
        current.startPolling(new ComputerManagerListener() {
            @Override
            public void notifyComputerUpdated(ComputerDetails details) {
                ComputerDetails copy = new ComputerDetails(details);
                activity.runOnUiThread(() -> {
                    if (activity.isFinishing() || activity.isDestroyed()) {
                        return;
                    }
                    update(copy);
                });
            }
        });
        // The manager reports its saved PCs from startPolling itself, so they are all in by now.
        if (!reported) {
            reported = true;
            listener.onHostsChanged();
        }
    }

    void update(ComputerDetails details) {
        boolean changed = waitsForFirstAnswer(details) ? unanswered.add(details.uuid) : unanswered.remove(details.uuid);
        if (details.pairState != PairingManager.PairState.PAIRED) {
            if (hosts.remove(details.uuid) != null || changed) {
                listener.onHostsChanged();
            }
            return;
        }
        hosts.put(details.uuid, details);
        listener.onHostsChanged();
    }

    /**
     * A PC read back from the saved list before its first poll: no state and no pair state yet, but the
     * certificate of an earlier pairing. It is paired as far as anyone knows until the poll says otherwise.
     */
    static boolean waitsForFirstAnswer(ComputerDetails details) {
        return details.pairState == null && details.serverCert != null
                && (details.state == null || details.state == ComputerDetails.State.UNKNOWN);
    }

    /**
     * Whether a paired PC may still turn up: the manager has not reported its saved PCs yet, or one of them,
     * paired before, has not answered its first poll. "No PCs yet" waits for this, so it shows only when no
     * paired PC is left to hear from.
     */
    boolean waiting() {
        return !reported || !unanswered.isEmpty();
    }

    /** A host was removed from this device; it leaves the feed at once instead of at the next poll. */
    void forget(String uuid) {
        boolean waitedFor = unanswered.remove(uuid);
        if (hosts.remove(uuid) != null || waitedFor) {
            listener.onHostsChanged();
        }
    }

    /** Drops the hosts the manager no longer knows, such as one removed from its own settings page. */
    void prune() {
        ComputerManagerService.ComputerManagerBinder current = binder;
        if (current == null) {
            return;
        }
        boolean changed = unanswered.removeIf(uuid -> current.getComputer(uuid) == null);
        for (Iterator<String> it = hosts.keySet().iterator(); it.hasNext(); ) {
            if (current.getComputer(it.next()) == null) {
                it.remove();
                changed = true;
            }
        }
        if (changed) {
            listener.onHostsChanged();
        }
    }

    /** The paired hosts, in the order they were first seen. */
    List<ComputerDetails> hosts() {
        return new ArrayList<>(hosts.values());
    }

    ComputerDetails host(String uuid) {
        return hosts.get(uuid);
    }

    /** Null until the service is ready. */
    ComputerManagerService.ComputerManagerBinder binder() {
        return binder;
    }

    /** The app's client identity; null until the service is ready. */
    String uniqueId() {
        ComputerManagerService.ComputerManagerBinder current = binder;
        return current == null ? null : current.getUniqueId();
    }
}
