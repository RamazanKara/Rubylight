/**
 * Decides when the PIN is asked for and shows the PIN screen over the app. Pure gate decisions and
 * grant/grace state live in {@code nightjar-policy::pin::PinGate} via {@link NightjarPinJni}; this
 * class owns only Android {@link Activity} lifecycle hooks and recents-thumbnail hiding.
 */
package com.limelight.pin;

import android.app.Activity;
import android.app.Application;
import android.content.Context;
import android.os.Build;
import android.os.Bundle;
import android.os.SystemClock;
import android.view.WindowManager;

import com.limelight.nvstream.jni.Json;
import com.limelight.nvstream.jni.NightjarPinJni;
import com.limelight.ui.NightjarActivity;

import java.lang.ref.WeakReference;
import java.util.Set;

public final class PinGate {
    /** How long the PIN screen's approval of one PC's stream start stays valid. */
    static final long GRANT_MS = 60_000;
    private static final long LAUNCH_MS = 3_000;

    private static boolean secondScreen;
    /** The app-lock PIN screen that is up, if any; weak so a lost screen cannot keep the lock from coming back. */
    private static WeakReference<Activity> screen;
    /** Until when a PIN screen that was just started is assumed to be on its way. */
    private static long launchingUntil;
    private static int started;
    private static PinStore cachedStore;
    private static Context cachedApp;

    private PinGate() {
    }

    static synchronized PinStore store(Context context) {
        Context app = context.getApplicationContext();
        Context resolved = app != null ? app : context;
        if (cachedStore == null || cachedApp != resolved) {
            if (cachedStore != null) {
                cachedStore.close();
            }
            cachedStore = new PinStore(resolved);
            cachedApp = resolved;
        }
        return cachedStore;
    }

    static synchronized void clearCachedStoreForTests() {
        if (cachedStore != null) {
            cachedStore.close();
            cachedStore = null;
        }
        cachedApp = null;
    }

    // ---- The decisions, delegated to nightjar-policy::pin ----

    static boolean appNeeded(boolean pinSet, PinStore.Mode mode, boolean unlocked) {
        return NightjarPinJni.gateAppNeeded(pinSet, mode.value, unlocked);
    }

    static boolean streamNeeded(boolean pinSet, PinStore.Mode mode, Set<String> pcs, String uuid,
                                boolean unlocked, boolean grantValid) {
        return NightjarPinJni.gateStreamNeeded(pinSet, mode.value, pcsJson(pcs), uuid, unlocked, grantValid);
    }

    static boolean contains(Set<String> pcs, String uuid) {
        return NightjarPinJni.gateContains(pcsJson(pcs), uuid);
    }

    /** Whether the app was left unlocked recently enough on this boot. */
    static boolean withinGrace(boolean sameBoot, long leftAt, long now, int graceMinutes) {
        return NightjarPinJni.gateWithinGrace(sameBoot, leftAt, now, graceMinutes);
    }

    private static String pcsJson(Set<String> pcs) {
        Json.Arr arr = Json.arr();
        if (pcs != null) {
            for (String pc : pcs) {
                arr.add(pc);
            }
        }
        return arr.toString();
    }

    // ---- Entry points ----

    /** The app is locked and nothing has unlocked it. */
    public static synchronized boolean needsPinForApp(Context context) {
        PinStore store = store(context);
        return NightjarPinJni.gateNeedsPinForApp(store.handle());
    }

    /**
     * Whether starting a stream on {@code uuid} must first go through the PIN screen. A yes answer from the PIN
     * screen ({@link #grant}) is used up by the start it was given for.
     */
    public static synchronized boolean needsPinForStream(Context context, String uuid) {
        if (secondScreen) {
            return false;
        }
        PinStore store = store(context);
        return NightjarPinJni.gateNeedsPinForStream(store.handle(), uuid, SystemClock.elapsedRealtime());
    }

    /** The same answer without using up a grant: Continue here's press asks this, and the start after it uses the grant. */
    public static synchronized boolean streamLocked(Context context, String uuid) {
        if (secondScreen) {
            return false;
        }
        PinStore store = store(context);
        return NightjarPinJni.gateStreamLocked(store.handle(), uuid, SystemClock.elapsedRealtime());
    }

    /** The PIN was entered for starting a stream on {@code uuid}. */
    public static synchronized void grant(String uuid) {
        NightjarPinJni.gateGrant(uuid, SystemClock.elapsedRealtime());
    }

    /** The start a grant was left for will not come (Continue here's press failed): it must not open the next one. */
    public static synchronized void forgetGrant() {
        NightjarPinJni.gateForgetGrant();
    }

    /** The PIN was entered (or has just been set): the app stays open until it is left for the grace time. */
    public static synchronized void unlockApp() {
        NightjarPinJni.gateUnlockApp();
    }

    /** This process is the second screen's, started by a first stream the lock let through. */
    public static synchronized void followFirstScreen() {
        secondScreen = true;
        NightjarPinJni.gateFollowFirstScreen();
    }

    /** The app-lock PIN screen has come up. */
    static synchronized void setScreen(Activity activity) {
        screen = new WeakReference<>(activity);
        launchingUntil = 0;
    }

    /** That screen is going away (call before finishing it, so the lock can come back at once). */
    static synchronized void clearScreen(Activity activity) {
        if (screen != null && screen.get() == activity) {
            screen = null;
        }
    }

    private static boolean showing() {
        if (SystemClock.elapsedRealtime() < launchingUntil) {
            return true;
        }
        Activity live = screen == null ? null : screen.get();
        return live != null && !live.isFinishing() && !live.isDestroyed();
    }

    /** Test hook: forget in-memory gate state in this process without deleting {@code pin_lock.json}. */
    public static synchronized void resetForTests() {
        secondScreen = false;
        screen = null;
        launchingUntil = 0;
        started = 0;
        clearCachedStoreForTests();
        NightjarPinJni.gateResetForTests();
    }

    /** Starts watching the app's screens; called once from the Application. */
    public static void install(Application app) {
        synchronized (PinGate.class) {
            PinStore store = store(app);
            NightjarPinJni.gateInstall(store.handle(), store.elapsed(), store.boot());
        }
        app.registerActivityLifecycleCallbacks(new Application.ActivityLifecycleCallbacks() {
            @Override public void onActivityStarted(Activity activity) {
                // The Application's store: an activity may wrap its context (the debug demo redirects its prefs).
                PinGate.onStarted(activity, store(app));
            }

            @Override public void onActivityStopped(Activity activity) {
                PinGate.onStopped(activity, store(app));
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
                NightjarPinJni.gateReturned(store.handle(), store.elapsed());
            }
            show = activity instanceof NightjarActivity && !(activity instanceof PinLockActivity)
                    && !activity.isFinishing() && !showing()
                    && NightjarPinJni.gateNeedsPinForApp(store.handle());
            if (show) {
                launchingUntil = SystemClock.elapsedRealtime() + LAUNCH_MS;
            }
        }
        hideFromRecents(activity, store);
        if (show) {
            activity.startActivity(PinLockActivity.forApp(activity));
            activity.overridePendingTransition(0, 0);
        }
    }

    /**
     * With the app lock on, the recents thumbnail must not show the library: no thumbnail on Android 13 and later,
     * a secure window before that. Set whenever a screen starts, so a change of mode applies from the next screen.
     */
    @SuppressWarnings("deprecation")
    private static void hideFromRecents(Activity activity, PinStore store) {
        if (!(activity instanceof NightjarActivity)) {
            return;
        }
        boolean hide = store.mode() == PinStore.Mode.APP;
        if (Build.VERSION.SDK_INT >= 33) {
            activity.setRecentsScreenshotEnabled(!hide);
        } else if (hide) {
            activity.getWindow().addFlags(WindowManager.LayoutParams.FLAG_SECURE);
        } else {
            activity.getWindow().clearFlags(WindowManager.LayoutParams.FLAG_SECURE);
        }
    }

    static void onStopped(Activity activity, PinStore store) {
        synchronized (PinGate.class) {
            started = Math.max(0, started - 1);
            if (started == 0 && !activity.isChangingConfigurations()) {
                NightjarPinJni.gateLeft(store.handle(), store.elapsed(), store.boot());
            }
        }
    }
}
