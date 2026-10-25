package com.limelight.pin;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import android.app.Application;
import android.content.Context;
import android.content.SharedPreferences;

import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

import java.nio.charset.StandardCharsets;
import java.util.Arrays;
import java.util.HashSet;
import java.util.Map;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class)
public class PinStoreTest {
    private static final byte[] RIGHT = "4321".getBytes(StandardCharsets.US_ASCII);
    private static final byte[] WRONG = "1111".getBytes(StandardCharsets.US_ASCII);

    private static class FakeClock implements PinStore.Clock {
        long now = 1_000;
        long boot = 1;

        @Override public long elapsed() { return now; }
        @Override public long boot() { return boot; }
    }

    private SharedPreferences prefs;
    private FakeClock clock;
    private PinStore store;
    private int checks;
    private int failuresSeenWhileChecking;

    @Before
    public void setUp() {
        prefs = RuntimeEnvironment.getApplication().getSharedPreferences("pin_store_test", Context.MODE_PRIVATE);
        prefs.edit().clear().commit();
        clock = new FakeClock();
        store = new PinStore(prefs, clock, (hash, pin) -> {
            checks++;
            failuresSeenWhileChecking = prefs.getInt(PinStore.FAILS, -1);
            return Arrays.equals(pin, RIGHT);
        });
        prefs.edit().putString(PinStore.HASH, "v1$1$x$y").putInt(PinStore.LENGTH, 4).commit();
    }

    @Test
    public void defaultsWithoutAPinLockNothing() {
        SharedPreferences empty = RuntimeEnvironment.getApplication().getSharedPreferences("pin_empty", Context.MODE_PRIVATE);
        PinStore fresh = new PinStore(empty, clock, (hash, pin) -> false);
        assertFalse(fresh.hasPin());
        assertEquals(PinStore.Mode.OFF, fresh.mode());
        assertEquals(0, fresh.pinLength());
        assertEquals(0, fresh.lockedForMs());
    }

    @Test
    public void aStoredModeOnlyCountsWhileAPinExists() {
        store.setMode(PinStore.Mode.STREAMS);
        assertEquals(PinStore.Mode.STREAMS, store.mode());
        prefs.edit().remove(PinStore.HASH).commit();
        assertEquals(PinStore.Mode.OFF, store.mode());
    }

    @Test
    public void rightPinOpensAndClearsTheCount() {
        assertEquals(PinStore.Result.WRONG, store.tryPin(WRONG).result);
        assertEquals(PinStore.Result.OK, store.tryPin(RIGHT).result);
        assertEquals(0, prefs.getInt(PinStore.FAILS, -1));
    }

    @Test
    public void theFailureIsWrittenBeforeThePinIsChecked() {
        store.tryPin(RIGHT);
        assertEquals("a try is counted before it is verified", 1, failuresSeenWhileChecking);
        store.tryPin(WRONG);
        assertEquals(1, failuresSeenWhileChecking);
        assertEquals(1, prefs.getInt(PinStore.FAILS, -1));
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
        PinStore restarted = new PinStore(prefs, clock, (hash, pin) -> true);
        assertEquals(30_000, restarted.lockedForMs());
        assertEquals(PinStore.Result.LOCKED, restarted.tryPin(RIGHT).result);
    }

    @Test
    public void aRebootRenewsTheLockAtItsStep() {
        for (int i = 0; i < 6; i++) {
            store.tryPin(WRONG);
            clock.now += PinThrottle.MAX_MS + 1;
        }
        store.tryPin(WRONG);
        assertEquals(7, prefs.getInt(PinStore.FAILS, -1));
        // The device restarted: the elapsed clock starts again and the boot number changes.
        clock.boot = 2;
        clock.now = 4_000;
        assertEquals("the seventh failure waits two minutes", 120_000, store.lockedForMs());
        assertEquals(PinStore.Result.LOCKED, store.tryPin(RIGHT).result);
    }

    @Test
    public void killingTheAppMidCheckCostsATry() {
        // The verifier never returns here; what was stored before it ran is what a restart sees.
        PinStore dying = new PinStore(prefs, clock, (hash, pin) -> {
            throw new IllegalStateException("process killed");
        });
        for (int i = 0; i < 5; i++) {
            try {
                dying.tryPin(RIGHT);
            } catch (IllegalStateException expected) {
                // killed
            }
        }
        assertEquals(5, prefs.getInt(PinStore.FAILS, -1));
        assertEquals(30_000, dying.lockedForMs());
    }

    @Test
    public void settingAPinStoresOnlyASaltedHash() {
        SharedPreferences fresh = RuntimeEnvironment.getApplication().getSharedPreferences("pin_set", Context.MODE_PRIVATE);
        fresh.edit().clear().commit();
        PinStore real = new PinStore(fresh, clock, PinHasher::verify);
        real.setPin("90210".getBytes(StandardCharsets.US_ASCII));
        assertTrue(real.hasPin());
        assertEquals(5, real.pinLength());
        assertEquals(PinStore.Result.OK, real.tryPin("90210".getBytes(StandardCharsets.US_ASCII)).result);
        assertEquals(PinStore.Result.WRONG, real.tryPin("90211".getBytes(StandardCharsets.US_ASCII)).result);
        for (Map.Entry<String, ?> entry : fresh.getAll().entrySet()) {
            assertFalse(entry.getKey(), String.valueOf(entry.getValue()).contains("90210"));
        }
    }

    @Test
    public void settingAPinClearsEarlierFailures() {
        for (int i = 0; i < 5; i++) store.tryPin(WRONG);
        store.setPin("5555".getBytes(StandardCharsets.US_ASCII));
        assertEquals(0, store.lockedForMs());
        assertEquals(0, prefs.getInt(PinStore.FAILS, -1));
    }

    @Test
    public void removingThePinRemovesEverythingItNeeded() {
        store.setMode(PinStore.Mode.SELECTED);
        store.setPcs(new HashSet<>(Arrays.asList("a", "b")));
        store.setGraceMinutes(30);
        store.removePin();
        assertFalse(store.hasPin());
        assertEquals(PinStore.Mode.OFF, store.mode());
        assertTrue(store.pcs().isEmpty());
        assertTrue(prefs.getAll().isEmpty());
    }

    @Test
    public void graceMinutesFallBackToTheDefaultForUnknownValues() {
        assertEquals(PinStore.DEFAULT_GRACE_MINUTES, store.graceMinutes());
        store.setGraceMinutes(30);
        assertEquals(30, store.graceMinutes());
        store.setGraceMinutes(0);
        assertEquals(0, store.graceMinutes());
        prefs.edit().putInt(PinStore.GRACE, 17).commit();
        assertEquals(PinStore.DEFAULT_GRACE_MINUTES, store.graceMinutes());
    }
}
