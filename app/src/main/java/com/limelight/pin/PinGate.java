package com.limelight.pin;

import android.app.Activity;
import android.app.Application;
import android.content.Context;
import android.os.Bundle;
import android.os.SystemClock;

import com.limelight.ui.NightjarActivity;

import java.util.Locale;
import java.util.Set;

/**
 * Decides when the PIN is asked for and shows the PIN screen over the app.
 *
 * Two things are locked: opening the app (every NightjarActivity that comes to the front while the app is locked)
 * and starting a stream (the two code paths that build a stream intent, {@code ServerHelper.doStart} and
 * {@code ShortcutTrampoline}, ask {@link #needsPinForStream}). The stream screen itself is not gated: it restarts
 * itself for reconnects and recovery, and none of that is a new start.
 */
public final class PinGate {
    /** How long the PIN screen's approval of one PC's stream start stays valid. */
    static final long GRANT_MS = 60_000;

    private static boolean unlockedApp;
    private static String grantUuid;
    private static long grantAt;
    private static boolean showing;
    private static int started;
    /** Elapsed time when the last activity stopped, or -1 while the app is in front (or only rotating). */
    private static long leftAt = -1;

    private PinGate() {
    }

    // ---- The decisions, pure so the table can be tested row by row ----

    static boolean appNeeded(boolean pinSet, PinStore.Mode mode, boolean unlocked) {
        return pinSet && mode == PinStore.Mode.APP && !unlocked;
    }

    static boolean streamNeeded(boolean pinSet, PinStore.Mode mode, Set<String> pcs, String uuid,
                                boolean unlocked, boolean grantValid) {
        if (appNeeded(pinSet, mode, unlocked)) return true;
        if (!pinSet) return false;
        boolean asks = mode == PinStore.Mode.STREAMS || (mode == PinStore.Mode.SELECTED && contains(pcs, uuid));
        return asks && !grantValid;
    }

    static boolean contains(Set<String> pcs, String uuid) {
        if (uuid == null) return false;
        String wanted = uuid.toLowerCase(Locale.ROOT);
        for (String pc : pcs) if (pc != null && pc.toLowerCase(Locale.ROOT).equals(wanted)) return true;
        return false;
    }

    /** Whether the app was left unlocked recently enough on this boot. */
    static boolean withinGrace(boolean sameBoot, long leftAt, long now, int graceMinutes) {
        long away = now - leftAt;
        return sameBoot && graceMinutes > 0 && away >= 0 && away < graceMinutes * 60_000L;
    }

    // ---- Entry points ----

    /** The app is locked and nothing has unlocked it. */
    public static synchronized boolean needsPinForApp(Context context) {
        PinStore store = new PinStore(context);
        return appNeeded(store.hasPin(), store.mode(), unlockedApp);
    }

    /**
     * Whether starting a stream on {@code uuid} must first go through the PIN screen. A yes answer from the PIN
     * screen ({@link #grant}) is used up by the start it was given for.
     */
    public static synchronized boolean needsPinForStream(Context context, String uuid) {
        PinStore store = new PinStore(context);
        boolean granted = grantValid(uuid, SystemClock.elapsedRealtime());
        boolean needed = streamNeeded(store.hasPin(), store.mode(), store.pcs(), uuid, unlockedApp, granted);
        if (!needed && granted) grantUuid = null;
        return needed;
    }

    private static boolean grantValid(String uuid, long now) {
        return grantUuid != null && uuid != null && grantUuid.equalsIgnoreCase(uuid)
                && now - grantAt >= 0 && now - grantAt < GRANT_MS;
    }

    /** The PIN was entered for starting a stream on {@code uuid}. */
    public static synchronized void grant(String uuid) {
        grantUuid = uuid;
        grantAt = SystemClock.elapsedRealtime();
    }

    /** The PIN was entered (or has just been set): the app stays open until it is left for the grace time. */
    public static synchronized void unlockApp() {
        unlockedApp = true;
        leftAt = -1;
    }

    static synchronized void setShowing(boolean value) {
        showing = value;
    }

    /** Test hook: forget everything the gate remembers in this process. */
    public static synchronized void resetForTests() {
        unlockedApp = false;
        grantUuid = null;
        grantAt = 0;
        showing = false;
        started = 0;
        leftAt = -1;
    }

    /** Starts watching the app's screens; called once from the Application. */
    public static void install(Application app) {
        PinStore store = new PinStore(app);
        synchronized (PinGate.class) {
            // After a restart of the process the app counts as unlocked if it was left unlocked a moment ago.
            unlockedApp = store.hasPin() && store.mode() == PinStore.Mode.APP && withinGrace(
                    store.activeBoot() == store.boot(), store.activeAt(), store.elapsed(), store.graceMinutes());
        }
        app.registerActivityLifecycleCallbacks(new Application.ActivityLifecycleCallbacks() {
            @Override public void onActivityStarted(Activity activity) {
                PinGate.onStarted(activity, new PinStore(activity));
            }

            @Override public void onActivityStopped(Activity activity) {
                PinGate.onStopped(activity, new PinStore(activity));
            }

            @Override public void onActivityCreated(Activity activity, Bundle state) { }
            @Override public void onActivityResumed(Activity activity) { }
            @Override public void onActivityPaused(Activity activity) { }
            @Override public void onActivitySaveInstanceState(Activity activity, Bundle state) { }
            @Override public void onActivityDestroyed(Activity activity) { }
        });
    }

    static void onStarted(Activity activity, PinStore store) {
        boolean show;
        synchronized (PinGate.class) {
            if (started++ == 0) {
                // Coming back to the app after it was out of sight, not after a rotation.
                if (leftAt >= 0 && unlockedApp && store.mode() == PinStore.Mode.APP
                        && !withinGrace(true, leftAt, store.elapsed(), store.graceMinutes())) {
                    unlockedApp = false;
                }
                leftAt = -1;
            }
            show = activity instanceof NightjarActivity && !(activity instanceof PinLockActivity)
                    && !activity.isFinishing() && !showing
                    && appNeeded(store.hasPin(), store.mode(), unlockedApp);
            if (show) showing = true;
        }
        if (show) {
            activity.startActivity(PinLockActivity.forApp(activity));
            activity.overridePendingTransition(0, 0);
        }
    }

    static void onStopped(Activity activity, PinStore store) {
        synchronized (PinGate.class) {
            started = Math.max(0, started - 1);
            if (started == 0 && !activity.isChangingConfigurations()) {
                leftAt = store.elapsed();
                if (unlockedApp) store.setActive(leftAt, store.boot());
            }
        }
    }
}
