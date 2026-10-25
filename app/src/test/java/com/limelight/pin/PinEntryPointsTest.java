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
import android.content.ServiceConnection;
import android.provider.Settings;

import com.limelight.AppView;
import com.limelight.Game;
import com.limelight.HelpTopicsActivity;
import com.limelight.ShortcutTrampoline;
import com.limelight.computers.ComputerManagerService;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvApp;
import com.limelight.utils.ServerHelper;

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

import java.nio.charset.StandardCharsets;

/** The places a stream can start from, and what the PIN lock does at each. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class PinEntryPointsTest {
    private static final String PC = "5d16bcc0-992e-4b09-a76c-9a94243015f8";
    private static final String PIN = "2468";

    public static class Trampoline extends ShortcutTrampoline {
        ServiceConnection connection;

        @Override public boolean bindService(Intent intent, ServiceConnection connection, int flags) {
            this.connection = connection;
            return true;
        }

        @Override public void unbindService(ServiceConnection connection) { }

        @Override public void startActivities(Intent[] intents) { }
    }

    /** Records the close-the-app request instead of walking the task stack. */
    public static class Closing extends PinLockActivity {
        boolean affinityFinished;

        @Override public void finishAffinity() {
            affinityFinished = true;
            finish();
        }
    }

    private Application app;
    private PinStore store;

    @Before
    public void setUp() {
        app = RuntimeEnvironment.getApplication();
        Settings.Global.putInt(app.getContentResolver(), "boot_count", 3);
        PinGate.resetForTests();
        PinLockActivity.executorForTests = Runnable::run;
        app.getSharedPreferences(PinStore.FILE, Context.MODE_PRIVATE).edit().clear().commit();
        store = new PinStore(app);
        store.setPin(PIN.getBytes(StandardCharsets.US_ASCII));
    }

    @After
    public void tearDown() {
        PinLockActivity.executorForTests = null;
    }

    private static Intent nextStarted(Activity activity) {
        return Shadows.shadowOf(activity).getNextStartedActivity();
    }

    private static ComputerDetails onlinePc() {
        ComputerDetails computer = new ComputerDetails();
        computer.uuid = PC;
        computer.name = "Desk";
        computer.state = ComputerDetails.State.ONLINE;
        computer.activeAddress = new ComputerDetails.AddressTuple("127.0.0.1", 47989);
        return computer;
    }

    private static ComputerManagerService.ComputerManagerBinder binder() {
        return new ComputerManagerService().new ComputerManagerBinder() {
            @Override public String getUniqueId() { return "client"; }
        };
    }

    private void startStream(Activity from) {
        ServerHelper.doStart(from, new NvApp("Desktop", 1, false), onlinePc(), binder(), false);
    }

    private static void type(Activity pinScreen, String digits) {
        int[] keys = {com.limelight.R.id.pin_key_0, com.limelight.R.id.pin_key_1, com.limelight.R.id.pin_key_2,
                com.limelight.R.id.pin_key_3, com.limelight.R.id.pin_key_4, com.limelight.R.id.pin_key_5,
                com.limelight.R.id.pin_key_6, com.limelight.R.id.pin_key_7, com.limelight.R.id.pin_key_8,
                com.limelight.R.id.pin_key_9};
        for (char digit : digits.toCharArray()) pinScreen.findViewById(keys[digit - '0']).performClick();
    }

    // ---- ServerHelper.doStart ----

    @Test
    public void withoutALockTheStreamStartsAtOnce() {
        store.setMode(PinStore.Mode.OFF);
        Activity from = Robolectric.buildActivity(HelpTopicsActivity.class).create().get();
        startStream(from);
        assertEquals(Game.class.getName(), nextStarted(from).getComponent().getClassName());
    }

    @Test
    public void doStartUnderTheLockOpensThePinScreenCarryingTheGameIntent() {
        store.setMode(PinStore.Mode.STREAMS);
        Activity from = Robolectric.buildActivity(HelpTopicsActivity.class).create().get();
        startStream(from);

        Intent pin = nextStarted(from);
        assertEquals(PinLockActivity.class.getName(), pin.getComponent().getClassName());
        assertEquals("STREAM", pin.getStringExtra(PinLockActivity.EXTRA_PURPOSE));
        Intent next = pin.getParcelableExtra(PinLockActivity.EXTRA_NEXT);
        assertNotNull(next);
        assertEquals(Game.class.getName(), next.getComponent().getClassName());
        assertEquals("Desktop", next.getStringExtra(Game.EXTRA_APP_NAME));
        assertEquals(1, next.getIntExtra(Game.EXTRA_APP_ID, 0));
        assertEquals(PC, next.getStringExtra(Game.EXTRA_PC_UUID));
        assertNull("nothing else started", nextStarted(from));
    }

    @Test
    public void thePinThenStartsTheStreamAndAWrongPinDoesNot() {
        store.setMode(PinStore.Mode.STREAMS);
        Activity from = Robolectric.buildActivity(HelpTopicsActivity.class).create().get();
        startStream(from);
        Intent pin = nextStarted(from);

        ActivityController<PinLockActivity> screen = Robolectric.buildActivity(PinLockActivity.class, pin).setup();
        type(screen.get(), "1357");
        assertNull("a wrong PIN starts nothing", nextStarted(screen.get()));
        assertFalse(screen.get().isFinishing());

        type(screen.get(), PIN);
        Intent started = nextStarted(screen.get());
        assertNotNull(started);
        assertEquals(Game.class.getName(), started.getComponent().getClassName());
        assertTrue(screen.get().isFinishing());
    }

    @Test
    public void selectedPcsAskAndTheOthersDoNot() {
        store.setMode(PinStore.Mode.SELECTED);
        store.setPcs(java.util.Collections.singleton("some-other-pc"));
        Activity from = Robolectric.buildActivity(HelpTopicsActivity.class).create().get();
        startStream(from);
        assertEquals(Game.class.getName(), nextStarted(from).getComponent().getClassName());

        store.setPcs(java.util.Collections.singleton(PC));
        startStream(from);
        assertEquals(PinLockActivity.class.getName(), nextStarted(from).getComponent().getClassName());
    }

    // ---- ShortcutTrampoline ----

    private Intent shortcut(String appId) {
        Intent intent = new Intent().putExtra(AppView.UUID_EXTRA, PC).putExtra(AppView.NAME_EXTRA, "Desk");
        if (appId != null) intent.putExtra(Game.EXTRA_APP_ID, appId).putExtra(Game.EXTRA_APP_NAME, "Desktop");
        return intent;
    }

    @Test
    public void aShortcutThatStartsAnAppAsksForThePinBeforeDoingAnything() {
        store.setMode(PinStore.Mode.STREAMS);
        ActivityController<Trampoline> controller = Robolectric.buildActivity(Trampoline.class, shortcut("5")).create();
        Trampoline trampoline = controller.get();
        assertNull("the host is not contacted before the PIN", trampoline.connection);
        assertTrue(trampoline.isFinishing());

        Intent pin = nextStarted(trampoline);
        assertEquals(PinLockActivity.class.getName(), pin.getComponent().getClassName());
        assertEquals("TRAMPOLINE", pin.getStringExtra(PinLockActivity.EXTRA_PURPOSE));
        Intent again = pin.getParcelableExtra(PinLockActivity.EXTRA_NEXT);
        assertEquals(ShortcutTrampoline.class.getName(), again.getComponent().getClassName());
        assertEquals("5", again.getStringExtra(Game.EXTRA_APP_ID));
        assertEquals(PC, again.getStringExtra(AppView.UUID_EXTRA));
    }

    @Test
    public void afterThePinTheShortcutRunsOnceAndTheNextOneAsksAgain() {
        store.setMode(PinStore.Mode.STREAMS);
        Trampoline first = Robolectric.buildActivity(Trampoline.class, shortcut("5")).create().get();
        Intent pin = nextStarted(first);

        ActivityController<PinLockActivity> screen = Robolectric.buildActivity(PinLockActivity.class, pin).setup();
        type(screen.get(), PIN);
        Intent forwarded = nextStarted(screen.get());
        assertEquals(ShortcutTrampoline.class.getName(), forwarded.getComponent().getClassName());

        Trampoline second = Robolectric.buildActivity(Trampoline.class, forwarded).create().get();
        assertNotNull("with the PIN given, the shortcut goes on to the host", second.connection);

        Trampoline third = Robolectric.buildActivity(Trampoline.class, shortcut("5")).create().get();
        assertNull("the PIN covered one start", third.connection);
        assertEquals(PinLockActivity.class.getName(), nextStarted(third).getComponent().getClassName());
    }

    @Test
    public void onlyTheShortcutFieldsSurviveThePinScreen() {
        store.setMode(PinStore.Mode.STREAMS);
        Intent incoming = shortcut("5").putExtra("evil", "1").putExtra(Game.EXTRA_HOST, "10.0.0.66")
                .setAction(Intent.ACTION_VIEW);
        Intent pin = PinLockActivity.forTrampoline(app, incoming, PC);
        Intent again = pin.getParcelableExtra(PinLockActivity.EXTRA_NEXT);
        assertEquals(ShortcutTrampoline.class.getName(), again.getComponent().getClassName());
        assertNull(again.getAction());
        assertNull(again.getStringExtra("evil"));
        assertNull(again.getStringExtra(Game.EXTRA_HOST));
        assertEquals("5", again.getStringExtra(Game.EXTRA_APP_ID));
    }

    @Test
    public void aShortcutWithoutAnAppOnlyOpensTheAppListSoItIsNotAsked() {
        store.setMode(PinStore.Mode.STREAMS);
        Trampoline trampoline = Robolectric.buildActivity(Trampoline.class, shortcut(null)).create().get();
        assertNotNull(trampoline.connection);
        assertNull(nextStarted(trampoline));
    }

    @Test
    public void inAppModeEvenAnAppListShortcutAsks() {
        store.setMode(PinStore.Mode.APP);
        Trampoline trampoline = Robolectric.buildActivity(Trampoline.class, shortcut(null)).create().get();
        assertNull(trampoline.connection);
        assertEquals(PinLockActivity.class.getName(), nextStarted(trampoline).getComponent().getClassName());
    }

    @Test
    public void afterThePinInAppModeTheAppStaysUnlockedForTheShortcut() {
        store.setMode(PinStore.Mode.APP);
        Trampoline first = Robolectric.buildActivity(Trampoline.class, shortcut("5")).create().get();
        Intent pin = nextStarted(first);
        ActivityController<PinLockActivity> screen = Robolectric.buildActivity(PinLockActivity.class, pin).setup();
        type(screen.get(), PIN);
        assertFalse(PinGate.needsPinForApp(app));
        Trampoline second = Robolectric.buildActivity(Trampoline.class, nextStarted(screen.get())).create().get();
        assertNotNull(second.connection);
    }

    // ---- Opening the app ----

    @Test
    public void appModeAsksOnTheFirstScreenAndBackClosesTheApp() {
        store.setMode(PinStore.Mode.APP);
        PinGate.install(app);
        Activity first = Robolectric.buildActivity(HelpTopicsActivity.class).create().start().get();
        Intent pin = nextStarted(first);
        assertEquals("APP", pin.getStringExtra(PinLockActivity.EXTRA_PURPOSE));

        ActivityController<Closing> screen = Robolectric.buildActivity(Closing.class, pin).setup();
        screen.get().getOnBackPressedDispatcher().onBackPressed();
        assertTrue("Back closes every screen of the app", screen.get().affinityFinished);
        assertTrue(PinGate.needsPinForApp(app));
    }

    @Test
    public void theRightPinOpensTheAppAndTheScreenGoesAway() {
        store.setMode(PinStore.Mode.APP);
        PinGate.install(app);
        Activity first = Robolectric.buildActivity(HelpTopicsActivity.class).create().start().get();
        Intent pin = nextStarted(first);
        ActivityController<PinLockActivity> screen = Robolectric.buildActivity(PinLockActivity.class, pin).setup();
        type(screen.get(), PIN);
        assertTrue(screen.get().isFinishing());
        assertFalse(PinGate.needsPinForApp(app));
    }
}
