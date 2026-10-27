package com.limelight.pin;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

import android.app.Activity;
import android.app.Application;
import android.content.Context;
import android.content.Intent;
import android.provider.Settings;

import com.limelight.HelpTopicsActivity;

import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.Shadows;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowSystemClock;

import java.time.Duration;
import java.util.Arrays;
import java.util.Collections;
import java.util.HashSet;
import java.util.Set;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class PinGateTest {
    private static final String PC = "5d16bcc0-992e-4b09-a76c-9a94243015f8";
    private static final String OTHER = "11111111-2222-3333-4444-555555555555";

    private Application app;
    private PinStore store;

    @Before
    public void setUp() {
        app = RuntimeEnvironment.getApplication();
        Settings.Global.putInt(app.getContentResolver(), "boot_count", 5);
        PinGate.resetForTests();
        app.getSharedPreferences(PinStore.FILE, Context.MODE_PRIVATE).edit().clear().commit();
        store = new PinStore(app);
    }

    /** A PIN is set without paying for the key derivation: only the presence of a record matters to the gate. */
    private void lock(PinStore.Mode mode) {
        app.getSharedPreferences(PinStore.FILE, Context.MODE_PRIVATE).edit()
                .putString(PinStore.HASH, "v1$1$x$y").putInt(PinStore.LENGTH, 4).commit();
        store.setMode(mode);
    }

    // ---- The decision table ----

    @Test
    public void appLockAsksOnlyWithAPinInAppModeAndNothingUnlocked() {
        for (PinStore.Mode mode : PinStore.Mode.values()) {
            for (boolean pin : new boolean[] {false, true}) {
                for (boolean unlocked : new boolean[] {false, true}) {
                    boolean expected = pin && mode == PinStore.Mode.APP && !unlocked;
                    assertEquals(mode + " pin=" + pin + " unlocked=" + unlocked, expected,
                            PinGate.appNeeded(pin, mode, unlocked));
                }
            }
        }
    }

    @Test
    public void streamLockTable() {
        Set<String> pcs = Collections.singleton(PC.toUpperCase());
        // No PIN: never.
        for (PinStore.Mode mode : PinStore.Mode.values()) {
            assertFalse(PinGate.streamNeeded(false, mode, pcs, PC, false, false));
        }
        assertFalse(PinGate.streamNeeded(true, PinStore.Mode.OFF, pcs, PC, false, false));
        assertTrue(PinGate.streamNeeded(true, PinStore.Mode.STREAMS, pcs, PC, false, false));
        assertFalse("a grant covers one start", PinGate.streamNeeded(true, PinStore.Mode.STREAMS, pcs, PC, false, true));
        assertTrue(PinGate.streamNeeded(true, PinStore.Mode.SELECTED, pcs, PC, false, false));
        assertFalse(PinGate.streamNeeded(true, PinStore.Mode.SELECTED, pcs, OTHER, false, false));
        assertFalse(PinGate.streamNeeded(true, PinStore.Mode.SELECTED, pcs, null, false, false));
        assertFalse(PinGate.streamNeeded(true, PinStore.Mode.SELECTED, pcs, PC, false, true));
        // App mode locks every start until the app is unlocked, and a grant does not stand in for that.
        assertTrue(PinGate.streamNeeded(true, PinStore.Mode.APP, pcs, PC, false, false));
        assertTrue(PinGate.streamNeeded(true, PinStore.Mode.APP, pcs, PC, false, true));
        assertFalse(PinGate.streamNeeded(true, PinStore.Mode.APP, pcs, PC, true, false));
    }

    @Test
    public void selectedPcsMatchIgnoringCase() {
        assertTrue(PinGate.contains(new HashSet<>(Arrays.asList("ABC-1", "x")), "abc-1"));
        assertFalse(PinGate.contains(new HashSet<>(Arrays.asList("ABC-1")), "abc-2"));
        assertFalse(PinGate.contains(Collections.<String>emptySet(), "abc-1"));
    }

    @Test
    public void graceNeedsTheSameBootAndAPositiveTimeThatHasNotRunOut() {
        long minute = 60_000;
        assertTrue(PinGate.withinGrace(true, 0, 9 * minute, 10));
        assertFalse(PinGate.withinGrace(true, 0, 10 * minute, 10));
        assertFalse("another boot", PinGate.withinGrace(false, 0, minute, 10));
        assertFalse("ask every time", PinGate.withinGrace(true, 0, 1, 0));
        assertFalse("clock went backwards", PinGate.withinGrace(true, 5000, 1000, 10));
    }

    // ---- Stream starts ----

    @Test
    public void streamsModeAsksUntilThePinScreenGrantsOneStart() {
        lock(PinStore.Mode.STREAMS);
        assertTrue(PinGate.needsPinForStream(app, PC));
        PinGate.grant(PC);
        assertFalse(PinGate.needsPinForStream(app, PC));
        assertTrue("the grant was used up", PinGate.needsPinForStream(app, PC));
    }

    @Test
    public void aGrantForOnePcDoesNotOpenAnother() {
        lock(PinStore.Mode.STREAMS);
        PinGate.grant(OTHER);
        assertTrue(PinGate.needsPinForStream(app, PC));
    }

    @Test
    public void aGrantRunsOutAfterAMinute() {
        lock(PinStore.Mode.STREAMS);
        PinGate.grant(PC);
        ShadowSystemClock.advanceBy(Duration.ofSeconds(61));
        assertTrue(PinGate.needsPinForStream(app, PC));
    }

    @Test
    public void selectedModeOnlyAsksForSelectedPcs() {
        lock(PinStore.Mode.SELECTED);
        store.setPcs(Collections.singleton(PC));
        assertTrue(PinGate.needsPinForStream(app, PC));
        assertFalse(PinGate.needsPinForStream(app, OTHER));
    }

    @Test
    public void offModeAndNoPinNeverAsk() {
        assertFalse(PinGate.needsPinForStream(app, PC));
        assertFalse(PinGate.needsPinForApp(app));
        lock(PinStore.Mode.OFF);
        assertFalse(PinGate.needsPinForStream(app, PC));
        assertFalse(PinGate.needsPinForApp(app));
    }

    @Test
    public void appModeAlsoGuardsStreamsUntilUnlocked() {
        lock(PinStore.Mode.APP);
        assertTrue(PinGate.needsPinForApp(app));
        assertTrue(PinGate.needsPinForStream(app, PC));
        PinGate.unlockApp();
        assertFalse(PinGate.needsPinForApp(app));
        assertFalse(PinGate.needsPinForStream(app, PC));
    }

    // ---- Leaving and coming back (the lifecycle hooks installed by the Application) ----

    private Intent nextStarted(Activity activity) {
        return Shadows.shadowOf(activity).getNextStartedActivity();
    }

    private ActivityController<HelpTopicsActivity> open() {
        return Robolectric.buildActivity(HelpTopicsActivity.class).create().start();
    }

    @Test
    public void appModeCoversTheFirstScreenWithThePinScreen() {
        lock(PinStore.Mode.APP);
        PinGate.install(app);
        ActivityController<HelpTopicsActivity> screen = open();
        Intent next = nextStarted(screen.get());
        assertEquals(PinLockActivity.class.getName(), next.getComponent().getClassName());
        assertEquals("APP", next.getStringExtra(PinLockActivity.EXTRA_PURPOSE));
    }

    @Test
    public void anOtherModeNeverCoversTheApp() {
        lock(PinStore.Mode.STREAMS);
        PinGate.install(app);
        assertNull(nextStarted(open().get()));
    }

    @Test
    public void aLivePinScreenIsNotCoveredAgainButAFinishingOneIs() {
        lock(PinStore.Mode.APP);
        Activity other = Robolectric.buildActivity(HelpTopicsActivity.class).create().get();
        ActivityController<PinLockActivity> pin = Robolectric.buildActivity(PinLockActivity.class,
                PinLockActivity.forApp(app)).create();
        PinGate.onStarted(other, store);
        assertNull("the PIN screen is up", nextStarted(other));

        pin.get().finish();
        PinGate.onStarted(other, store);
        assertEquals("a screen on its way out no longer counts, even before it is destroyed",
                PinLockActivity.class.getName(), nextStarted(other).getComponent().getClassName());
    }

    @Test
    public void aPinScreenThatNeverCameUpOnlyHoldsTheLockBackBriefly() {
        lock(PinStore.Mode.APP);
        Activity first = Robolectric.buildActivity(HelpTopicsActivity.class).create().get();
        Activity second = Robolectric.buildActivity(HelpTopicsActivity.class).create().get();
        PinGate.onStarted(first, store);
        assertEquals(PinLockActivity.class.getName(), nextStarted(first).getComponent().getClassName());
        PinGate.onStarted(second, store);
        assertNull("the first launch is still on its way", nextStarted(second));

        ShadowSystemClock.advanceBy(Duration.ofSeconds(5));
        PinGate.onStarted(second, store);
        assertEquals("the launch was dropped; the lock comes back", PinLockActivity.class.getName(),
                nextStarted(second).getComponent().getClassName());
    }

    @Test
    public void appModeHidesTheScreensFromRecentsAndOtherModesDoNot() {
        Activity screen = Robolectric.buildActivity(HelpTopicsActivity.class).create().get();
        lock(PinStore.Mode.APP);
        PinGate.unlockApp();
        PinGate.onStarted(screen, store);
        assertTrue((screen.getWindow().getAttributes().flags
                & android.view.WindowManager.LayoutParams.FLAG_SECURE) != 0);

        store.setMode(PinStore.Mode.STREAMS);
        PinGate.onStarted(screen, store);
        assertTrue((screen.getWindow().getAttributes().flags
                & android.view.WindowManager.LayoutParams.FLAG_SECURE) == 0);
    }

    @Test
    public void thePinScreenIsNotCoveredByItself() {
        lock(PinStore.Mode.APP);
        PinGate.install(app);
        ActivityController<PinLockActivity> pin = Robolectric.buildActivity(PinLockActivity.class,
                PinLockActivity.forApp(app)).create().start();
        assertNull(nextStarted(pin.get()));
    }

    @Test
    public void comingBackInsideTheGraceDoesNotAsk() {
        lock(PinStore.Mode.APP);
        store.setGraceMinutes(10);
        PinGate.install(app);
        PinGate.unlockApp();
        ActivityController<HelpTopicsActivity> screen = open().resume();
        screen.pause().stop();

        ShadowSystemClock.advanceBy(Duration.ofMinutes(5));
        screen.restart().start();
        assertNull("5 minutes is inside the 10 minute grace", nextStarted(screen.get()));
        assertFalse(PinGate.needsPinForApp(app));
    }

    @Test
    public void comingBackAfterTheGraceAsksAgain() {
        lock(PinStore.Mode.APP);
        store.setGraceMinutes(10);
        PinGate.install(app);
        PinGate.unlockApp();
        ActivityController<HelpTopicsActivity> screen = open().resume();
        screen.pause().stop();

        ShadowSystemClock.advanceBy(Duration.ofMinutes(11));
        screen.restart().start();
        assertEquals(PinLockActivity.class.getName(), nextStarted(screen.get()).getComponent().getClassName());
    }

    @Test
    public void graceZeroAsksEveryTimeTheAppIsLeft() {
        lock(PinStore.Mode.APP);
        store.setGraceMinutes(0);
        PinGate.install(app);
        PinGate.unlockApp();
        ActivityController<HelpTopicsActivity> screen = open().resume();
        screen.pause().stop();
        screen.restart().start();
        assertEquals(PinLockActivity.class.getName(), nextStarted(screen.get()).getComponent().getClassName());
    }

    @Test
    public void aRestartedProcessKeepsTheAppOpenInsideTheGrace() {
        lock(PinStore.Mode.APP);
        store.setGraceMinutes(10);
        store.setActive(store.elapsed(), store.boot());
        ShadowSystemClock.advanceBy(Duration.ofMinutes(2));
        PinGate.install(app);
        assertFalse(PinGate.needsPinForApp(app));
    }

    @Test
    public void aRestartedProcessAsksAfterTheGraceOrAReboot() {
        lock(PinStore.Mode.APP);
        store.setGraceMinutes(10);
        store.setActive(store.elapsed(), store.boot());
        ShadowSystemClock.advanceBy(Duration.ofMinutes(20));
        PinGate.install(app);
        assertTrue(PinGate.needsPinForApp(app));

        PinGate.resetForTests();
        store.setActive(store.elapsed(), store.boot() + 1);
        PinGate.install(app);
        assertTrue("another boot", PinGate.needsPinForApp(app));
    }
}
