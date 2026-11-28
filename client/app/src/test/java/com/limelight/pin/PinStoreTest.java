package com.limelight.pin;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

import android.app.Application;
import android.content.Context;
import android.content.SharedPreferences;

import com.limelight.nvstream.jni.Json;
import com.limelight.nvstream.jni.NightjarPinJni;

import org.junit.After;
import org.junit.Before;
import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.TemporaryFolder;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

import java.io.File;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.util.Arrays;
import java.util.HashSet;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class)
public class PinStoreTest {
    private static final byte[] RIGHT = "4321".getBytes(StandardCharsets.US_ASCII);
    private static final byte[] WRONG = "1111".getBytes(StandardCharsets.US_ASCII);

    @Rule
    public TemporaryFolder tmp = new TemporaryFolder();

    private static class FakeClock implements PinStore.Clock {
        long now = 1_000;
        long boot = 1;

        @Override public long elapsed() { return now; }
        @Override public long boot() { return boot; }
    }

    private File dir;
    private FakeClock clock;
    private PinStore store;
    private int checks;
    private int failuresSeenWhileChecking;

    @Before
    public void setUp() throws IOException {
        PinStore.clearForTests(RuntimeEnvironment.getApplication());
        dir = tmp.newFolder("pin_store");
        clock = new FakeClock();
        store = new PinStore(dir, clock, (hash, pin) -> {
            checks++;
            failuresSeenWhileChecking = readDiskFails(dir);
            return Arrays.equals(pin, RIGHT);
        });
        store.setPinForTests(RIGHT, 1);
    }

    @After
    public void tearDown() {
        if (store != null) {
            store.close();
        }
        PinStore.clearForTests(RuntimeEnvironment.getApplication());
    }

    private static Json.Obj readDisk(File folder) {
        File file = new File(folder, PinStore.LOCK_FILE_NAME);
        if (!file.exists()) {
            return Json.obj();
        }
        try {
            String raw = new String(Files.readAllBytes(file.toPath()), StandardCharsets.UTF_8);
            return Json.parseObject(raw);
        } catch (IOException e) {
            throw new IllegalStateException(e);
        }
    }

    private static int readDiskFails(File folder) {
        return readDisk(folder).optInt("fails", 0);
    }

    @Test
    public void defaultsWithoutAPinLockNothing() throws IOException {
        File empty = tmp.newFolder("pin_empty");
        try (PinStore fresh = new PinStore(empty, clock, (hash, pin) -> false)) {
            assertFalse(fresh.hasPin());
            assertEquals(PinStore.Mode.OFF, fresh.mode());
            assertEquals(0, fresh.pinLength());
            assertEquals(0, fresh.lockedForMs());
        }
    }

    @Test
    public void aStoredModeOnlyCountsWhileAPinExists() {
        store.setMode(PinStore.Mode.STREAMS);
        assertEquals(PinStore.Mode.STREAMS, store.mode());
        store.removePin();
        store.flushForTests();
        assertEquals(PinStore.Mode.OFF, store.mode());
    }

    @Test
    public void rightPinOpensAndClearsTheCount() {
        assertEquals(PinStore.Result.WRONG, store.tryPin(WRONG).result);
        assertEquals(PinStore.Result.OK, store.tryPin(RIGHT).result);
        assertEquals(0, readDiskFails(dir));
    }

    @Test
    public void theFailureIsWrittenBeforeThePinIsChecked() {
        store.tryPin(RIGHT);
        assertEquals("a try is counted before it is verified", 1, failuresSeenWhileChecking);
        store.tryPin(WRONG);
        assertEquals(1, failuresSeenWhileChecking);
        assertEquals(1, readDiskFails(dir));
    }

    @Test
    public void fiveWrongPinsLockAndALockedStoreDoesNotCheck() {
        for (int i = 0; i < 4; i++) assertEquals(0, store.tryPin(WRONG).lockedForMs);
        PinStore.Attempt fifth = store.tryPin(WRONG);
        assertEquals(PinStore.Result.WRONG, fifth.result);
        assertEquals(30_000, fifth.lockedForMs);
        assertEquals(5, checks);
        assertEquals(30_000, store.lockedForMs());

        PinStore.Attempt refused = store.tryPin(RIGHT);
        assertEquals("even the right PIN is refused during the wait", PinStore.Result.LOCKED, refused.result);
        assertEquals(5, checks);

        clock.now += 30_000;
        assertEquals(0, store.lockedForMs());
        assertEquals(PinStore.Result.OK, store.tryPin(RIGHT).result);
    }

    @Test
    public void theWaitDoublesAfterEachFurtherWrongPin() {
        for (int i = 0; i < 5; i++) store.tryPin(WRONG);
        clock.now += 30_000;
        assertEquals(60_000, store.tryPin(WRONG).lockedForMs);
    }

    @Test
    public void theLockSurvivesARestartOfTheProcess() {
        for (int i = 0; i < 5; i++) store.tryPin(WRONG);
        store.flushForTests();
        try (PinStore restarted = PinStore.reopenForTests(dir, clock, (hash, pin) -> true)) {
            assertEquals(30_000, restarted.lockedForMs());
            assertEquals(PinStore.Result.LOCKED, restarted.tryPin(RIGHT).result);
        }
    }

    @Test
    public void aRebootRenewsTheLockAtItsStep() {
        for (int i = 0; i < 6; i++) {
            store.tryPin(WRONG);
            clock.now += PinThrottle.MAX_MS + 1;
        }
        store.tryPin(WRONG);
        assertEquals(7, readDiskFails(dir));
        // The device restarted: the elapsed clock starts again and the boot number changes.
        clock.boot = 2;
        clock.now = 4_000;
        assertEquals("the seventh failure waits two minutes", 120_000, store.lockedForMs());
        assertEquals(PinStore.Result.LOCKED, store.tryPin(RIGHT).result);
    }

    private void failSevenTimesThenReboot() {
        for (int i = 0; i < 6; i++) {
            store.tryPin(WRONG);
            clock.now += PinThrottle.MAX_MS + 1;
        }
        store.tryPin(WRONG);
        clock.boot = 2;
        clock.now = 4_000;
    }

    @Test
    public void afterARebootTheRenewedLockCountsDownAndEnds() {
        failSevenTimesThenReboot();
        assertEquals(120_000, store.lockedForMs());
        clock.now += 1_000;
        assertEquals("the countdown moves on", 119_000, store.lockedForMs());
        clock.now += 119_000;
        assertEquals(0, store.lockedForMs());
        assertEquals(PinStore.Result.OK, store.tryPin(RIGHT).result);
    }

    @Test
    public void aRenewedLockIsStoredSoAnotherProcessSeesTheSameEnd() {
        failSevenTimesThenReboot();
        store.lockedForMs();
        store.flushForTests();
        assertEquals(2, readDisk(dir).optLong("lock_boot", -1));
        try (PinStore other = PinStore.reopenForTests(dir, clock, (hash, pin) -> true)) {
            clock.now += 60_000;
            assertEquals(60_000, other.lockedForMs());
        }
    }

    @Test
    public void twoScreensTryingAtOnceCountEveryTry() throws Exception {
        try (PinStore slow = new PinStore(dir, clock, (hash, pin) -> {
            try {
                Thread.sleep(40);
            } catch (InterruptedException e) {
                throw new IllegalStateException(e);
            }
            return false;
        })) {
            for (int i = 0; i < 3; i++) slow.tryPin(WRONG);
            Thread a = new Thread(() -> slow.tryPin(WRONG));
            Thread b = new Thread(() -> slow.tryPin(WRONG));
            a.start();
            b.start();
            a.join();
            b.join();
            slow.flushForTests();
            assertEquals(5, readDiskFails(dir));
        }
    }

    @Test
    public void killingTheAppMidCheckCostsATry() {
        // The verifier never returns here; what was stored before it ran is what a restart sees.
        try (PinStore dying = new PinStore(dir, clock, (hash, pin) -> {
            throw new IllegalStateException("process killed");
        })) {
            for (int i = 0; i < 5; i++) {
                try {
                    dying.tryPin(RIGHT);
                } catch (IllegalStateException expected) {
                    // killed
                }
            }
            assertEquals(5, readDiskFails(dir));
            assertEquals(30_000, dying.lockedForMs());
        }
    }

    @Test
    public void settingAPinStoresOnlyASaltedHash() throws IOException {
        File fresh = tmp.newFolder("pin_set");
        try (PinStore real = new PinStore(fresh, clock, null)) {
            real.setPin("90210".getBytes(StandardCharsets.US_ASCII));
            assertTrue(real.hasPin());
            assertEquals(5, real.pinLength());
            assertEquals(PinStore.Result.OK, real.tryPin("90210".getBytes(StandardCharsets.US_ASCII)).result);
            assertEquals(PinStore.Result.WRONG, real.tryPin("90211".getBytes(StandardCharsets.US_ASCII)).result);
            String raw = new String(Files.readAllBytes(new File(fresh, PinStore.LOCK_FILE_NAME).toPath()),
                    StandardCharsets.UTF_8);
            assertFalse(raw.contains("90210"));
        }
    }

    @Test
    public void settingAPinClearsEarlierFailures() {
        for (int i = 0; i < 5; i++) store.tryPin(WRONG);
        store.setPin("5555".getBytes(StandardCharsets.US_ASCII));
        assertEquals(0, store.lockedForMs());
        assertEquals(0, readDiskFails(dir));
    }

    @Test
    public void removingThePinRemovesEverythingItNeeded() {
        store.setMode(PinStore.Mode.SELECTED);
        store.setPcs(new HashSet<>(Arrays.asList("a", "b")));
        store.setGraceMinutes(30);
        store.removePin();
        store.flushForTests();
        assertFalse(store.hasPin());
        assertEquals(PinStore.Mode.OFF, store.mode());
        assertTrue(store.pcs().isEmpty());
        assertFalse(new File(dir, PinStore.LOCK_FILE_NAME).exists());
    }

    @Test
    public void graceMinutesFallBackToTheDefaultForUnknownValues() throws IOException {
        assertEquals(PinStore.DEFAULT_GRACE_MINUTES, store.graceMinutes());
        store.setGraceMinutes(30);
        assertEquals(30, store.graceMinutes());
        store.setGraceMinutes(0);
        assertEquals(0, store.graceMinutes());
        store.flushForTests();
        Json.Obj disk = readDisk(dir);
        disk.put("grace_min", 17);
        Files.write(new File(dir, PinStore.LOCK_FILE_NAME).toPath(),
                disk.toString().getBytes(StandardCharsets.UTF_8));
        try (PinStore reopened = PinStore.reopenForTests(dir, clock, null)) {
            assertEquals(PinStore.DEFAULT_GRACE_MINUTES, reopened.graceMinutes());
        }
    }

    @Test
    public void migratesLegacySharedPreferencesOnceIncludingLockoutAndGrace() {
        Application app = RuntimeEnvironment.getApplication();
        SharedPreferences legacy = app.getSharedPreferences(PinStore.FILE, Context.MODE_PRIVATE);
        String hash = PinHasher.create(RIGHT, 1_000);
        legacy.edit()
                .putString(PinStore.HASH, hash)
                .putInt(PinStore.LENGTH, 4)
                .putString(PinStore.MODE, "selected")
                .putStringSet(PinStore.PCS, new HashSet<>(Arrays.asList("pc-1", "pc-2")))
                .putInt(PinStore.GRACE, 30)
                .putInt(PinStore.FAILS, 5)
                .putLong(PinStore.LOCK_UNTIL, 50_000)
                .putLong(PinStore.LOCK_BOOT, 3)
                .putLong(PinStore.ACTIVE_AT, 12_000)
                .putLong(PinStore.ACTIVE_BOOT, 3)
                .commit();

        PinStore migrated = new PinStore(app);
        assertTrue(migrated.hasPin());
        assertEquals(4, migrated.pinLength());
        assertEquals(PinStore.Mode.SELECTED, migrated.mode());
        assertEquals(new HashSet<>(Arrays.asList("pc-1", "pc-2")), migrated.pcs());
        assertEquals(30, migrated.graceMinutes());
        assertEquals(12_000, migrated.activeAt());
        assertEquals(3, migrated.activeBoot());
        assertEquals(PinStore.Result.LOCKED, migrated.tryPin(RIGHT).result);
        assertTrue(new File(PinStore.storeDir(app), PinStore.LOCK_FILE_NAME).exists());
        assertTrue(legacy.getAll().isEmpty());
    }

    @Test
    public void legacyMigrationDoesNotOverwriteExistingLockFile() {
        Application app = RuntimeEnvironment.getApplication();
        PinStore current = new PinStore(app);
        current.setPinForTests("1234".getBytes(StandardCharsets.US_ASCII), 1);
        NightjarPinJni.storeEvictCacheForTests(PinStore.storeDir(app).getAbsolutePath());

        SharedPreferences legacy = app.getSharedPreferences(PinStore.FILE, Context.MODE_PRIVATE);
        legacy.edit().putString(PinStore.HASH, PinHasher.create("9999".getBytes(StandardCharsets.US_ASCII), 1_000))
                .putInt(PinStore.LENGTH, 4).commit();
        PinStore.resetLegacyMigrationForTests();

        PinStore after = new PinStore(app);
        assertEquals(PinStore.Result.OK, after.tryPin("1234".getBytes(StandardCharsets.US_ASCII)).result);
        assertTrue(legacy.getAll().isEmpty());
    }

    @Test
    public void legacyMigrationSkipsSharedPreferencesAfterEmptyCheck() {
        Application app = RuntimeEnvironment.getApplication();
        PinStore first = new PinStore(app);
        first.close();

        java.util.concurrent.atomic.AtomicInteger legacyLookups = new java.util.concurrent.atomic.AtomicInteger();
        android.content.ContextWrapper counting = new android.content.ContextWrapper(app) {
            @Override public Context getApplicationContext() { return this; }
            @Override public SharedPreferences getSharedPreferences(String name, int mode) {
                if (PinStore.FILE.equals(name)) legacyLookups.incrementAndGet();
                return super.getSharedPreferences(name, mode);
            }
        };
        PinStore second = new PinStore(counting);
        second.close();
        assertEquals("legacy prefs must not be queried after migration is marked done", 0, legacyLookups.get());
    }

    @Test
    public void corruptLegacyHashDoesNotCreateDamagedLockFile() {
        Application app = RuntimeEnvironment.getApplication();
        SharedPreferences legacy = app.getSharedPreferences(PinStore.FILE, Context.MODE_PRIVATE);
        legacy.edit().putString(PinStore.HASH, "corrupt-hash").putInt(PinStore.LENGTH, 4).commit();

        PinStore opened = new PinStore(app);
        assertFalse(opened.hasPin());
        assertFalse(new File(PinStore.storeDir(app), PinStore.LOCK_FILE_NAME).exists());
    }

    @Test
    public void cachedStoreOpenBeforeDirExistsSharesCacheAcrossCreation() {
        File uncreated = new File(tmp.getRoot(), "uncreated_pin_dir/nested");
        assertFalse(uncreated.exists());
        long h1 = NightjarPinJni.storeOpen(uncreated.getAbsolutePath(), true);
        long h2 = 0;
        try {
            assertTrue(h1 > 0);
            assertFalse("opening an uncreated dir must not create it eagerly", uncreated.exists());
            assertNull(NightjarPinJni.storeSetPin(h1, RIGHT, 1));
            assertTrue(uncreated.exists());

            h2 = NightjarPinJni.storeOpen(uncreated.getAbsolutePath(), true);
            assertTrue(h2 > 0);
            NightjarPinJni.storeSetMode(h1, "app");

            Json.Obj v1 = Json.parseObject(NightjarPinJni.storeViewJson(h1));
            Json.Obj v2 = Json.parseObject(NightjarPinJni.storeViewJson(h2));
            assertTrue(v1.optBoolean("has_pin", false));
            assertTrue(v2.optBoolean("has_pin", false));
            assertEquals("app", v1.optString("mode", ""));
            assertEquals("app", v2.optString("mode", ""));
        } finally {
            NightjarPinJni.storeEvictCacheForTests(uncreated.getAbsolutePath());
            if (h2 != 0) NightjarPinJni.storeDestroy(h2);
            if (h1 != 0) NightjarPinJni.storeDestroy(h1);
        }
    }
}
