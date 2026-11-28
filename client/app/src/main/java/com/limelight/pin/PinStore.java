/**
 * File-backed PIN lock store ({@code pin_lock.json} in {@code noBackupFilesDir}) backed by
 * {@code nightjar-core::pin_lock} via {@link NightjarPinJni}, with one-time migration from
 * legacy {@code SharedPreferences("pin_lock")}.
 */
package com.limelight.pin;

import android.content.Context;
import android.content.SharedPreferences;
import android.os.SystemClock;
import android.provider.Settings;

import com.limelight.nvstream.jni.HandleReaper;
import com.limelight.nvstream.jni.Json;
import com.limelight.nvstream.jni.NightjarPinJni;

import java.io.Closeable;
import java.io.File;
import java.util.HashSet;
import java.util.Set;

public final class PinStore implements Closeable {
    public static final String FILE = "pin_lock";
    public static final String LOCK_FILE_NAME = "pin_lock.json";
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
            for (Mode mode : values()) {
                if (mode.value.equals(value)) {
                    return mode;
                }
            }
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
            Context app = context.getApplicationContext();
            this.context = app != null ? app : context;
        }

        @Override public long elapsed() {
            return SystemClock.elapsedRealtime();
        }

        @Override public long boot() {
            try {
                return Settings.Global.getInt(context.getContentResolver(), Settings.Global.BOOT_COUNT);
            } catch (Settings.SettingNotFoundException | RuntimeException ignored) {
                // Fall through to the boot time estimate.
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

    private static volatile boolean legacyMigrationDone;

    private final long handle;
    private final Clock clock;
    private final Verifier verifier;

    public PinStore(Context context) {
        this(resolveApp(context), new SystemClockSource(context));
    }

    private PinStore(Context app, Clock clock) {
        this(migrateAndPrepareDir(app), true, clock, null);
    }

    PinStore(File dir, Clock clock, Verifier verifier) {
        this(dir, false, clock, verifier);
    }

    private PinStore(File dir, boolean cached, Clock clock, Verifier verifier) {
        this.clock = clock;
        this.verifier = verifier;
        this.handle = NightjarPinJni.storeOpen(dir.getAbsolutePath(), cached);
        HandleReaper.register(this, handle, NightjarPinJni::storeDestroy);
    }

    private static Context resolveApp(Context context) {
        Context app = context.getApplicationContext();
        return app != null ? app : context;
    }

    static File storeDir(Context context) {
        Context app = resolveApp(context);
        File noBackup = app.getNoBackupFilesDir();
        return noBackup != null ? noBackup : app.getFilesDir();
    }

    private static File migrateAndPrepareDir(Context app) {
        File dir = storeDir(app);
        migrateLegacyIfNeeded(app, dir);
        return dir;
    }

    private static void migrateLegacyIfNeeded(Context app, File dir) {
        if (legacyMigrationDone) {
            return;
        }
        synchronized (PinStore.class) {
            if (legacyMigrationDone) {
                return;
            }
            SharedPreferences legacy = app.getSharedPreferences(FILE, Context.MODE_PRIVATE);
            if (legacy == null || legacy.getAll().isEmpty()) {
                legacyMigrationDone = true;
                return;
            }
            Json.Obj obj = Json.obj();
            if (legacy.contains(HASH)) {
                obj.put(HASH, legacy.getString(HASH, null));
            }
            if (legacy.contains(LENGTH)) {
                obj.put(LENGTH, legacy.getInt(LENGTH, MAX_DIGITS));
            }
            if (legacy.contains(MODE)) {
                obj.put(MODE, legacy.getString(MODE, Mode.OFF.value));
            }
            Set<String> pcs = legacy.getStringSet(PCS, null);
            if (pcs != null) {
                Json.Arr arr = Json.arr();
                for (String pc : pcs) {
                    arr.add(pc);
                }
                obj.put(PCS, arr);
            }
            if (legacy.contains(GRACE)) {
                obj.put(GRACE, legacy.getInt(GRACE, DEFAULT_GRACE_MINUTES));
            }
            if (legacy.contains(FAILS)) {
                obj.put(FAILS, legacy.getInt(FAILS, 0));
            }
            if (legacy.contains(LOCK_UNTIL)) {
                obj.put(LOCK_UNTIL, legacy.getLong(LOCK_UNTIL, 0));
            }
            if (legacy.contains(LOCK_BOOT)) {
                obj.put(LOCK_BOOT, legacy.getLong(LOCK_BOOT, 0));
            }
            if (legacy.contains(ACTIVE_AT)) {
                obj.put(ACTIVE_AT, legacy.getLong(ACTIVE_AT, 0));
            }
            if (legacy.contains(ACTIVE_BOOT)) {
                obj.put(ACTIVE_BOOT, legacy.getLong(ACTIVE_BOOT, -1));
            }
            int status = NightjarPinJni.storeImportLegacy(dir.getAbsolutePath(), obj.toString());
            if (status >= 0) {
                legacy.edit().clear().commit();
                legacyMigrationDone = true;
            }
        }
    }

    long handle() {
        return handle;
    }

    private Json.Obj view() {
        return Json.parseObject(NightjarPinJni.storeViewJson(handle));
    }

    public boolean hasPin() {
        return view().optBoolean("has_pin", false);
    }

    /** Digits in the PIN, for the dots and the automatic submit; 0 without a PIN. */
    public int pinLength() {
        return view().optInt("pin_length", 0);
    }

    /** Mode in effect: without a PIN nothing is locked, whatever the stored mode says. */
    public Mode mode() {
        return Mode.of(view().optString("mode", Mode.OFF.value));
    }

    public void setMode(Mode mode) {
        NightjarPinJni.storeSetMode(handle, (mode == null ? Mode.OFF : mode).value);
    }

    public Set<String> pcs() {
        Json.Obj obj = view();
        Set<String> out = new HashSet<>();
        if (obj.has("pcs") && !obj.isNull("pcs")) {
            Json.Arr arr = obj.getArray("pcs");
            for (int i = 0; i < arr.length(); i++) {
                out.add(arr.getString(i));
            }
        }
        return out;
    }

    public void setPcs(Set<String> uuids) {
        Json.Arr arr = Json.arr();
        if (uuids != null) {
            for (String uuid : uuids) {
                arr.add(uuid);
            }
        }
        NightjarPinJni.storeSetPcs(handle, arr.toString());
    }

    public int graceMinutes() {
        return view().optInt("grace_minutes", DEFAULT_GRACE_MINUTES);
    }

    public void setGraceMinutes(int minutes) {
        NightjarPinJni.storeSetGraceMinutes(handle, minutes);
    }

    /** Stores a new PIN under a fresh salt and clears the failure count. Slow: measures the device. */
    public void setPin(byte[] pin) {
        String err = NightjarPinJni.storeSetPin(handle, pin, 0);
        if (err != null) {
            throw new IllegalArgumentException(err);
        }
    }

    void setPinForTests(byte[] pin, int iterations) {
        String err = NightjarPinJni.storeSetPin(handle, pin, iterations);
        if (err != null) {
            throw new IllegalArgumentException(err);
        }
    }

    /** Removes the PIN and everything that depended on it. */
    public void removePin() {
        NightjarPinJni.storeRemovePin(handle);
    }

    /** Milliseconds until the next try is allowed, 0 when it is. */
    public long lockedForMs() {
        return NightjarPinJni.storeLockedForMs(handle, clock.elapsed(), clock.boot());
    }

    /**
     * One try. Refused while locked out; otherwise the failure is written (and flushed) before the PIN is
     * checked, so killing the app mid-check cannot get a free guess, and a match clears it again. Slow.
     */
    public Attempt tryPin(byte[] pin) {
        NightjarPinJni.PinVerifier callback = verifier == null ? null : verifier::matches;
        long[] res = NightjarPinJni.storeTryPin(handle, pin, clock.elapsed(), clock.boot(), callback);
        if (res == null || res.length < 2) {
            return new Attempt(Result.LOCKED, 0);
        }
        switch ((int) res[0]) {
            case 0:
                return new Attempt(Result.OK, 0);
            case 1:
                return new Attempt(Result.WRONG, res[1]);
            default:
                return new Attempt(Result.LOCKED, res[1]);
        }
    }

    /** Remembers when the app was left unlocked, for the "ask again after" time. */
    void setActive(long elapsed, long boot) {
        NightjarPinJni.storeSetActive(handle, elapsed, boot);
    }

    long activeAt() {
        return NightjarPinJni.storeActiveAt(handle);
    }

    long activeBoot() {
        return NightjarPinJni.storeActiveBoot(handle);
    }

    long elapsed() {
        return clock.elapsed();
    }

    long boot() {
        return clock.boot();
    }

    void flushForTests() {
        NightjarPinJni.storeFlush(handle);
    }

    static PinStore reopenForTests(File dir, Clock clock, Verifier verifier) {
        return new PinStore(dir, clock, verifier);
    }

    static void resetLegacyMigrationForTests() {
        legacyMigrationDone = false;
    }

    public static void clearForTests(Context context) {
        PinGate.clearCachedStoreForTests();
        legacyMigrationDone = false;
        Context app = resolveApp(context);
        File dir = storeDir(app);
        NightjarPinJni.storeEvictCacheForTests(dir.getAbsolutePath());
        File lockFile = new File(dir, LOCK_FILE_NAME);
        if (lockFile.exists()) {
            lockFile.delete();
        }
        SharedPreferences legacy = app.getSharedPreferences(FILE, Context.MODE_PRIVATE);
        if (legacy != null) {
            legacy.edit().clear().commit();
        }
    }

    @Override public void close() {
        NightjarPinJni.storeFlush(handle);
        NightjarPinJni.storeDestroy(handle);
    }
}
