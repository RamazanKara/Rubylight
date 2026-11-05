package com.limelight.preferences;

import android.content.Context;
import android.content.Intent;

import com.limelight.pin.PinGate;

import java.util.Collections;
import java.util.Set;
import java.util.WeakHashMap;

/**
 * A pairing link from outside the app, until it can be asked about: nothing may interrupt a stream, a
 * pairing or a request on its way, so each of them holds the gate while it runs, and the newest link
 * waits behind it, so a page opening links in a loop cannot pile up questions.
 *
 * <p>A link holds a PIN and a passphrase, so it stays in this process's memory: no intent or saved
 * state carries it beyond {@link PairLinkActivity}'s own, which leaves no entry in Recents.
 */
public final class LinkGate {
    private static final Set<Object> holders = Collections.newSetFromMap(new WeakHashMap<>());
    private static String waiting;
    private static String confirmed;

    private LinkGate() {
    }

    /** Links wait from now until {@link #release}. */
    public static synchronized void hold(Object holder) {
        holders.add(holder);
    }

    public static void release(Object holder, Context context) {
        synchronized (LinkGate.class) {
            holders.remove(holder);
        }
        showWaiting(context);
    }

    /**
     * Asks about the waiting link if nothing is in the way. Every screen comes here when it comes to the
     * front, which picks up a link whose release could not bring a screen up from the background.
     */
    public static void showWaiting(Context context) {
        synchronized (LinkGate.class) {
            if (waiting == null || held()) {
                return;
            }
        }
        if (!PinGate.needsPinForApp(context)) {
            context.startActivity(new Intent(context.getApplicationContext(), PairLinkActivity.class)
                    .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK));
        }
    }

    static synchronized boolean held() {
        return !holders.isEmpty() || confirmed != null;
    }

    static synchronized void put(String link) {
        waiting = link;
    }

    static synchronized boolean hasWaiting() {
        return waiting != null;
    }

    static synchronized String take() {
        String link = waiting;
        waiting = null;
        return link;
    }

    /** The user chose Pair: {@link AddComputerManually} picks the link up as it opens. */
    static synchronized void confirm(String link) {
        confirmed = link;
    }

    static synchronized String takeConfirmed() {
        String link = confirmed;
        confirmed = null;
        return link;
    }

    /** Test hook: forget everything. */
    static synchronized void reset() {
        holders.clear();
        waiting = null;
        confirmed = null;
    }
}
