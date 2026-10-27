package com.limelight.pin;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

import android.app.Activity;
import android.app.Application;
import android.content.Context;
import android.content.Intent;
import android.os.Bundle;
import android.provider.Settings;

import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.Shadows;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;

/** The settings page must ask for the PIN every time it is left and come back to, and only once at a time. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class PinLockSettingsTest {
    private static final int REQUEST_OPEN = 1;

    private Application app;

    @Before
    public void setUp() {
        app = RuntimeEnvironment.getApplication();
        Settings.Global.putInt(app.getContentResolver(), "boot_count", 3);
        PinGate.resetForTests();
        app.getSharedPreferences(PinStore.FILE, Context.MODE_PRIVATE).edit().clear().commit();
        app.getSharedPreferences(PinStore.FILE, Context.MODE_PRIVATE).edit()
                .putString(PinStore.HASH, "v1$1$x$y").putInt(PinStore.LENGTH, 4).commit();
    }

    private static Intent verifyStarted(Activity activity) {
        Intent intent = Shadows.shadowOf(activity).getNextStartedActivity();
        if (intent == null) return null;
        assertEquals(PinLockActivity.class.getName(), intent.getComponent().getClassName());
        assertEquals("VERIFY", intent.getStringExtra(PinLockActivity.EXTRA_PURPOSE));
        return intent;
    }

    private static void verified(ActivityController<PinLockSettingsActivity> controller) {
        controller.get().onActivityResult(REQUEST_OPEN, Activity.RESULT_OK, null);
    }

    /** Leaves the page the way Home does and comes back to a new instance, as after process death. */
    private ActivityController<PinLockSettingsActivity> recreated(ActivityController<PinLockSettingsActivity> old) {
        Bundle saved = new Bundle();
        old.pause().saveInstanceState(saved).stop().destroy();
        return Robolectric.buildActivity(PinLockSettingsActivity.class).setup(saved);
    }

    @Test
    public void thePageAsksForThePinWhenItOpens() {
        ActivityController<PinLockSettingsActivity> page = Robolectric.buildActivity(PinLockSettingsActivity.class).setup();
        assertNotNull(verifyStarted(page.get()));
        assertNull(verifyStarted(page.get()));
    }

    @Test
    public void withoutAPinThePageOpensAtOnce() {
        app.getSharedPreferences(PinStore.FILE, Context.MODE_PRIVATE).edit().clear().commit();
        ActivityController<PinLockSettingsActivity> page = Robolectric.buildActivity(PinLockSettingsActivity.class).setup();
        assertNull(verifyStarted(page.get()));
    }

    @Test
    public void leavingThePageAndComingBackAsksAgain() {
        ActivityController<PinLockSettingsActivity> page = Robolectric.buildActivity(PinLockSettingsActivity.class).setup();
        verifyStarted(page.get());
        verified(page);
        page.pause().stop();
        assertNull("nothing yet", verifyStarted(page.get()));
        page.restart().start();
        assertNotNull("Home and back needs the PIN again", verifyStarted(page.get()));
    }

    @Test
    public void aRecreatedPageDoesNotRememberTheVerification() {
        ActivityController<PinLockSettingsActivity> page = Robolectric.buildActivity(PinLockSettingsActivity.class).setup();
        verifyStarted(page.get());
        verified(page);
        ActivityController<PinLockSettingsActivity> again = recreated(page);
        assertNotNull("process death and back needs the PIN again", verifyStarted(again.get()));
    }

    @Test
    public void aPinScreenOnTopIsNotLeavingThePage() {
        ActivityController<PinLockSettingsActivity> page = Robolectric.buildActivity(PinLockSettingsActivity.class).setup();
        verifyStarted(page.get());
        verified(page);
        // Change PIN: the page starts a PIN screen, is stopped behind it, and gets the result back.
        page.get().changePin();
        assertNotNull(verifyStarted(page.get()));
        page.pause().stop();
        page.restart().start();
        assertNull("the PIN screen's own result is still to come", verifyStarted(page.get()));
    }

    @Test
    public void aRecreationWhileThePinScreenIsUpDoesNotStartASecondOne() {
        ActivityController<PinLockSettingsActivity> page = Robolectric.buildActivity(PinLockSettingsActivity.class).setup();
        assertNotNull(verifyStarted(page.get()));
        ActivityController<PinLockSettingsActivity> again = recreated(page);
        assertNull("one PIN screen is already on its way", verifyStarted(again.get()));

        again.get().onActivityResult(REQUEST_OPEN, Activity.RESULT_CANCELED, null);
        assertTrue("cancelling the only PIN screen leaves the page", again.get().isFinishing());
    }

    @Test
    public void aCancelledPinScreenLeavesThePage() {
        ActivityController<PinLockSettingsActivity> page = Robolectric.buildActivity(PinLockSettingsActivity.class).setup();
        verifyStarted(page.get());
        page.get().onActivityResult(REQUEST_OPEN, Activity.RESULT_CANCELED, null);
        assertTrue(page.get().isFinishing());
        assertFalse(page.get().isDestroyed());
    }
}
