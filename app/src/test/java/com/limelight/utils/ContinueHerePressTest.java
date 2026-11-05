package com.limelight.utils;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertSame;
import static org.junit.Assert.assertTrue;

import android.app.Activity;
import android.app.Application;
import android.content.Context;
import android.content.Intent;
import android.os.Looper;

import com.limelight.R;
import com.limelight.nativeapi.NativeApiException;
import com.limelight.pin.PinGate;
import com.limelight.pin.PinStore;

import org.junit.After;
import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.Shadows;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowToast;

import java.nio.charset.StandardCharsets;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicReference;

/** What Continue here's press leaves behind: the start it owes, or no grant at all. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class ContinueHerePressTest {
    private static final String PC = "desk-uuid";

    private Application app;
    private ActivityController<Activity> screen;
    private final AtomicReference<Context> startedFrom = new AtomicReference<>();
    private final CountDownLatch answered = new CountDownLatch(1);

    @Before
    public void setUp() {
        app = RuntimeEnvironment.getApplication();
        PinGate.resetForTests();
        PinStore store = new PinStore(app);
        store.setPin("2468".getBytes(StandardCharsets.US_ASCII));
        store.setMode(PinStore.Mode.STREAMS);
        screen = Robolectric.buildActivity(Activity.class).create();
        screen.get().setTheme(androidx.appcompat.R.style.Theme_AppCompat);
    }

    @After
    public void tearDown() {
        new PinStore(app).setMode(PinStore.Mode.OFF);
        PinGate.resetForTests();
    }

    /** Presses, lets {@code meanwhile} happen before the PC answers, and runs the UI thread until that answer is in. */
    private void press(ContinueHereGate.Press pc, Runnable meanwhile) throws Exception {
        CountDownLatch release = new CountDownLatch(1);
        ContinueHereGate.press(screen.get(), () -> {
            try {
                assertTrue(release.await(30, TimeUnit.SECONDS));
                pc.send();
            } catch (InterruptedException e) {
                throw new AssertionError(e);
            } finally {
                answered.countDown();
            }
        }, startedFrom::set);
        meanwhile.run();
        release.countDown();
        assertTrue(answered.await(30, TimeUnit.SECONDS));
        for (int i = 0; i < 20; i++) {
            Thread.sleep(5);
            Shadows.shadowOf(Looper.getMainLooper()).idle();
        }
    }

    private static void refused() throws NativeApiException {
        throw new NativeApiException(NativeApiException.Kind.NETWORK, "refused");
    }

    @Test
    public void aPressThatFailsLeavesNoGrantAndNoStart() throws Exception {
        PinGate.grant(PC);
        assertFalse(PinGate.streamLocked(app, PC));
        press(ContinueHerePressTest::refused, () -> { });
        assertTrue("the next start on this PC asks", PinGate.streamLocked(app, PC));
        assertEquals(null, startedFrom.get());
    }

    @Test
    public void aPressThatWorksLeavesTheGrantToTheStartAfterIt() throws Exception {
        PinGate.grant(PC);
        press(() -> { }, () -> { });
        assertFalse(PinGate.streamLocked(app, PC));
        assertSame(screen.get(), startedFrom.get());
    }

    @Test
    public void aScreenThatWentAwayDuringThePressStillGetsItsStreamStarted() throws Exception {
        press(() -> { }, () -> screen.destroy());
        assertSame("the first device has lost its stream by now", app, startedFrom.get());
    }

    @Test
    public void aFailureOnAScreenThatWentAwayStillSaysSoAndLeavesNoGrant() throws Exception {
        PinGate.grant(PC);
        press(ContinueHerePressTest::refused, () -> screen.destroy());
        assertEquals(null, startedFrom.get());
        assertTrue(PinGate.streamLocked(app, PC));
        assertNotNull(ShadowToast.getLatestToast());
        assertTrue(ShadowToast.getTextOfLatestToast().startsWith(app.getString(R.string.continue_here_failed)));
    }

    @Test
    public void onlyTheApplicationStartsIntoANewTask() {
        Activity activity = screen.get();
        Intent from = new Intent(app, Activity.class);
        ServerHelper.startFrom(activity, from, null);
        assertEquals(0, Shadows.shadowOf(activity).getNextStartedActivity().getFlags() & Intent.FLAG_ACTIVITY_NEW_TASK);
        ServerHelper.startFrom(app, from, null);
        assertTrue((Shadows.shadowOf(app).getNextStartedActivity().getFlags() & Intent.FLAG_ACTIVITY_NEW_TASK) != 0);
    }
}
