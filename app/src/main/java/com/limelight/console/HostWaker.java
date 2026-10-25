package com.limelight.console;

import android.os.Handler;

import com.limelight.nvstream.http.ComputerDetails;

import java.io.IOException;
import java.util.concurrent.Executor;
import java.util.function.LongSupplier;
import java.util.function.Supplier;

/**
 * Wakes a PC and tells when it answers: it sends the packets the {@link WakeLaunch} plan asks for (off the UI thread)
 * and checks the host the feed reports every {@link #TICK_MS}. All callbacks arrive on the handler's thread. The
 * sender, the clock and the executor are handed in, so a test drives it without a network.
 */
final class HostWaker {
    interface Sender {
        void send(ComputerDetails host) throws IOException;
    }

    interface Listener {
        /** The PC answers now. */
        void onOnline();

        /** The PC did not answer within the plan's time. */
        void onTimedOut();

        /** A packet could not be sent at all (no network, no route); the wait ends. */
        void onSendFailed(IOException error);
    }

    static final long TICK_MS = 1_000;

    private final Handler handler;
    private final Executor io;
    private final Sender sender;
    private final Supplier<ComputerDetails> host;
    private final LongSupplier clock;
    private final Listener listener;
    private WakeLaunch plan;
    private int run;

    HostWaker(Handler handler, Executor io, Sender sender, Supplier<ComputerDetails> host, LongSupplier clock,
              Listener listener) {
        this.handler = handler;
        this.io = io;
        this.sender = sender;
        this.host = host;
        this.clock = clock;
        this.listener = listener;
    }

    /** A PC that is not online and has a MAC address to wake it by. */
    static boolean canWake(ComputerDetails host) {
        return host != null && host.state != ComputerDetails.State.ONLINE && host.macAddress != null;
    }

    boolean isWaiting() {
        return plan != null;
    }

    /** Starts the wait; false when one is under way already. */
    boolean start() {
        if (plan != null) {
            return false;
        }
        plan = new WakeLaunch(clock.getAsLong());
        tick(++run);
        return true;
    }

    void cancel() {
        plan = null;
        run++;
    }

    private void tick(int mine) {
        if (mine != run || plan == null) {
            return;
        }
        ComputerDetails current = host.get();
        boolean online = current != null && current.state == ComputerDetails.State.ONLINE;
        switch (plan.next(online, clock.getAsLong())) {
            case ONLINE:
                plan = null;
                listener.onOnline();
                return;
            case TIMED_OUT:
                plan = null;
                listener.onTimedOut();
                return;
            case SEND:
                if (current != null) {
                    send(new ComputerDetails(current), mine);
                }
                break;
            default:
                break;
        }
        handler.postDelayed(() -> tick(mine), TICK_MS);
    }

    private void send(ComputerDetails target, int mine) {
        io.execute(() -> {
            try {
                sender.send(target);
            } catch (IOException e) {
                handler.post(() -> {
                    if (mine == run && plan != null) {
                        plan = null;
                        listener.onSendFailed(e);
                    }
                });
            }
        });
    }
}
