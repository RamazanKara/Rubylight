package com.limelight.pin;

import android.content.Context;
import android.content.SharedPreferences;
import android.os.Build;
import android.os.SystemClock;
import android.provider.Settings;

import java.util.Arrays;
import java.util.HashSet;
import java.util.Set;

/**
 * The PIN lock's own preferences file. It is separate from the default settings so "Reset all settings" leaves
 * the lock alone, and backup excludes it with the rest of the shared preferences. Only the salted hash is kept,
 * never the PIN.
 */
public final class PinStore {
    public static final String FILE = "pin_lock";
    static final String HASH = "pin_hash";
    static final String LENGTH = "pin_length";
    static final String MODE = "pin_mode";
    static final String PCS = "pin_pcs";
    static final String GRACE = "pin_grace_min";
    static final String FAILS = "pin_fails";
    static final String LOCK_UNTIL = "pin_lock_until";
    static final String LOCK_BOOT = "pin_lock_boot";
    static final String ACTIVE_AT = "pin_active_at";
    static final String ACTIVE_BOOT = "pin_active_boot";

    public static final int MIN_DIGITS = 4;
    public static final int MAX_DIGITS = 8;
    public static final int DEFAULT_GRACE_MINUTES = 10;
    public static final int[] GRACE_CHOICES = {0, 5, 10, 30, 60};

    /** When the lock asks for the PIN. */
    public enum Mode {
        OFF("off"), APP("app"), STREAMS("streams"), SELECTED("selected");

        final String value;

        Mode(String value) {
            this.value = value;
        }

        static Mode of(String value) {
            for (Mode mode : values()) if (mode.value.equals(value)) return mode;
            return OFF;
        }
    }

    /** The clock the lockout runs on; replaced in tests. */
    interface Clock {
        long elapsed();

        long boot();
    }

    /** Elapsed realtime and a number that changes on every boot. */
    static final class SystemClockSource implements Clock {
        private final Context context;

        SystemClockSource(Context context) {
            this.context = context.getApplicationContext();
        }

        @Override public long elapsed() {
            return SystemClock.elapsedRealtime();
        }

        @Override public long boot() {
            if (Build.VERSION.SDK_INT >= 24) {
                try {
                    return Settings.Global.getInt(context.getContentResolver(), Settings.Global.BOOT_COUNT);
                } catch (Settings.SettingNotFoundException | RuntimeException ignored) {
                    // Fall through to the boot time estimate.
                }
            }
            return bootEstimate(System.currentTimeMillis(), SystemClock.elapsedRealtime());
        }
    }

    /** Boot time in 10 s steps; a clock change moves it, but then the lock is only renewed, never dropped. */
    static long bootEstimate(long wallClockMs, long elapsedMs) {
        return Math.round((wallClockMs - elapsedMs) / 10_000d);
    }

    /** What a try came to. */
    public enum Result { OK, WRONG, LOCKED }

    public static final class Attempt {
        public final Result result;
        /** For LOCKED, and for WRONG when that try started a lockout. */
        public final long lockedForMs;

        Attempt(Result result, long lockedForMs) {
            this.result = result;
            this.lockedForMs = lockedForMs;
        }
    }

    /** The check itself, replaced in tests to look at what is stored while it runs. */
    interface Verifier {
        boolean matches(String storedHash, byte[] pin);
    }

    /** One try at a time, across every PinStore in the process. */
    private static final Object TRY_LOCK = new Object();

    private final SharedPreferences prefs;
    private final Clock clock;
    private final Verifier verifier;

    public PinStore(Context context) {
        this(context.getApplicationContext().getSharedPreferences(FILE, Context.MODE_PRIVATE),
                new SystemClockSource(context), PinHasher::verify);
    }

    PinStore(SharedPreferences prefs, Clock clock, Verifier verifier) {
        this.prefs = prefs;
        this.clock = clock;
        this.verifier = verifier;
    }

    public boolean hasPin() {
        return prefs.getString(HASH, null) != null;
    }

    /** Digits in the PIN, for the dots and the automatic submit; 0 without a PIN. */
    public int pinLength() {
        return hasPin() ? Math.max(MIN_DIGITS, Math.min(MAX_DIGITS, prefs.getInt(LENGTH, MAX_DIGITS))) : 0;
    }

    /** Mode in effect: without a PIN nothing is locked, whatever the stored mode says. */
    public Mode mode() {
        return hasPin() ? Mode.of(prefs.getString(MODE, Mode.OFF.value)) : Mode.OFF;
    }

    public void setMode(Mode mode) {
        prefs.edit().putString(MODE, mode.value).apply();
    }

    public Set<String> pcs() {
        Set<String> stored = prefs.getStringSet(PCS, null);
        return stored == null ? new HashSet<>() : new HashSet<>(stored);
    }

    public void setPcs(Set<String> uuids) {
        prefs.edit().putStringSet(PCS, new HashSet<>(uuids)).apply();
    }

    public int graceMinutes() {
        int minutes = prefs.getInt(GRACE, DEFAULT_GRACE_MINUTES);
        return Arrays.binarySearch(GRACE_CHOICES, minutes) >= 0 ? minutes : DEFAULT_GRACE_MINUTES;
    }

    public void setGraceMinutes(int minutes) {
        prefs.edit().putInt(GRACE, minutes).apply();
    }

    /** Stores a new PIN under a fresh salt and clears the failure count. Slow: measures the device. */
    public void setPin(byte[] pin) {
        String hash = PinHasher.create(pin, PinHasher.calibrate());
        prefs.edit().putString(HASH, hash).putInt(LENGTH, pin.length)
                .putInt(FAILS, 0).putLong(LOCK_UNTIL, 0).putLong(LOCK_BOOT, 0).commit();
    }

    /** Removes the PIN and everything that depended on it. */
    public void removePin() {
        prefs.edit().clear().commit();
    }

    private PinThrottle.State load() {
        return new PinThrottle.State(prefs.getInt(FAILS, 0), prefs.getLong(LOCK_UNTIL, 0), prefs.getLong(LOCK_BOOT, 0));
    }

    private void save(PinThrottle.State state) {
        prefs.edit().putInt(FAILS, state.fails).putLong(LOCK_UNTIL, state.lockUntil)
                .putLong(LOCK_BOOT, state.lockBoot).commit();
    }

    /** The stored state seen from this boot; a lock renewed for a new boot is saved so its countdown moves on. */
    private PinThrottle.State current(long now, long boot) {
        PinThrottle.State stored = load();
        PinThrottle.State state = PinThrottle.onBoot(stored, now, boot);
        if (state.lockBoot != stored.lockBoot || state.lockUntil != stored.lockUntil) save(state);
        return state;
    }

    /** Milliseconds until the next try is allowed, 0 when it is. */
    public long lockedForMs() {
        synchronized (TRY_LOCK) {
            long now = clock.elapsed();
            return PinThrottle.remainingMs(current(now, clock.boot()), now);
        }
    }

    /**
     * One try. Refused while locked out; otherwise the failure is written (and flushed) before the PIN is
     * checked, so killing the app mid-check cannot get a free guess, and a match clears it again. Slow.
     * Load, count and save run under one lock, so two PIN screens cannot both count against the same state.
     */
    public Attempt tryPin(byte[] pin) {
        synchronized (TRY_LOCK) {
            long now = clock.elapsed();
            long boot = clock.boot();
            PinThrottle.State state = current(now, boot);
            long locked = PinThrottle.remainingMs(state, now);
            if (locked > 0) return new Attempt(Result.LOCKED, locked);
            PinThrottle.State counted = PinThrottle.failed(state, now, boot);
            save(counted);
            if (!verifier.matches(prefs.getString(HASH, null), pin)) {
                return new Attempt(Result.WRONG, PinThrottle.remainingMs(counted, clock.elapsed()));
            }
            save(PinThrottle.State.CLEAN);
            return new Attempt(Result.OK, 0);
        }
    }

    /** Remembers when the app was left unlocked, for the "ask again after" time. */
    void setActive(long elapsed, long boot) {
        prefs.edit().putLong(ACTIVE_AT, elapsed).putLong(ACTIVE_BOOT, boot).apply();
    }

    long activeAt() {
        return prefs.getLong(ACTIVE_AT, 0);
    }

    long activeBoot() {
        return prefs.getLong(ACTIVE_BOOT, -1);
    }

    long elapsed() {
        return clock.elapsed();
    }

    long boot() {
        return clock.boot();
    }
}
