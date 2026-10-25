package com.limelight.console;

import android.app.Activity;
import android.os.Handler;
import android.os.SystemClock;
import android.widget.Toast;

import com.limelight.R;
import com.limelight.nvstream.http.ComputerDetails;

import java.io.IOException;
import java.util.function.Function;

/**
 * The Wake PC action on Home: tapping a card of a PC that is away sends the wake packets and waits for it, telling
 * the player how it went. One PC at a time (tapping another moves the wait to it); the wait ends when the screen is
 * left, since the feed stops polling then.
 */
final class HomeWake {
    private final Activity activity;
    private final Function<String, ComputerDetails> hosts;
    private final Runnable onWoke;
    private final HostWaker waker;
    private String uuid;
    private String name;

    /**
     * @param hosts  the PC for a UUID, as the feed knows it now
     * @param onWoke called when the PC answers, so Home refreshes it
     */
    HomeWake(Activity activity, Handler handler, Function<String, ComputerDetails> hosts, HostWaker.Sender sender,
             Runnable onWoke) {
        this.activity = activity;
        this.hosts = hosts;
        this.onWoke = onWoke;
        this.waker = new HostWaker(handler, r -> new Thread(r, "console-wake").start(), sender,
                () -> hosts.apply(uuid), SystemClock::elapsedRealtime, new HostWaker.Listener() {
            @Override
            public void onOnline() {
                Toast.makeText(activity, activity.getString(R.string.wol_pc_online), Toast.LENGTH_SHORT).show();
                HomeWake.this.onWoke.run();
            }

            @Override
            public void onTimedOut() {
                WakeFeedback.timedOut(activity, name, () -> wake(uuid));
            }

            @Override
            public void onSendFailed(IOException error) {
                WakeFeedback.failed(activity, name, R.string.wol_fail, () -> wake(uuid));
            }
        });
    }

    /**
     * Wakes the PC and waits for it.
     *
     * @return false when this PC cannot be woken from here (unknown, online, or no MAC address); the caller then
     *         falls back to the list of PCs, which explains how to set Wake-on-LAN up
     */
    boolean wake(String hostUuid) {
        ComputerDetails host = hosts.apply(hostUuid);
        if (!HostWaker.canWake(host)) {
            return false;
        }
        if (waker.isWaiting()) {
            if (hostUuid.equals(uuid)) {
                return true;
            }
            // Another PC was tapped: the wait moves to it, the first one is no longer waited for.
            waker.cancel();
        }
        uuid = hostUuid;
        name = host.displayName();
        Toast.makeText(activity, activity.getString(R.string.wol_waking_pc), Toast.LENGTH_LONG).show();
        waker.start();
        return true;
    }

    void cancel() {
        waker.cancel();
    }
}
