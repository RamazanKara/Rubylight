package com.limelight;

import android.app.Application;
import android.content.Intent;
import android.content.ServiceConnection;
import android.database.sqlite.SQLiteDatabase;
import android.net.Uri;
import android.os.Looper;
import android.widget.TextView;

import com.limelight.computers.ComputerManagerListener;
import com.limelight.computers.ComputerManagerService;
import com.limelight.console.HostWaker;
import com.limelight.nativeapi.NativeApiFixtures;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.PairingManager;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.wol.WakeTargets;
import com.limelight.utils.CacheHelper;
import com.limelight.utils.FrontendEntry;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.Shadows;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowDialog;
import org.robolectric.shadows.ShadowLooper;
import org.robolectric.util.ReflectionHelpers;

import java.io.ByteArrayInputStream;
import java.io.OutputStream;
import java.nio.charset.StandardCharsets;
import java.util.List;
import java.util.concurrent.CopyOnWriteArrayList;
import java.util.concurrent.TimeUnit;

import static org.junit.Assert.*;

@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class)
public class ShortcutTrampolineLifecycleTest {
    public static class Trampoline extends ShortcutTrampoline {
        ServiceConnection connection;
        int unbinds;
        int databaseOpens;
        int launches;
        final List<ComputerDetails> wakes = new CopyOnWriteArrayList<>();

        @Override protected void sendWake(ComputerDetails host) {
            wakes.add(host);
        }

        @Override public boolean bindService(Intent intent, ServiceConnection connection, int flags) {
            this.connection = connection;
            return true;
        }

        @Override public void unbindService(ServiceConnection connection) {
            assertEquals("A service binding must be released once", 0, unbinds++);
        }

        @Override public void startActivities(Intent[] intents) {
            launches++;
        }

        @Override public SQLiteDatabase openOrCreateDatabase(String name, int mode, SQLiteDatabase.CursorFactory factory) {
            databaseOpens++;
            return super.openOrCreateDatabase(name, mode, factory);
        }
    }

    private Intent intent() {
        return new Intent().putExtra(AppView.UUID_EXTRA, "5d16bcc0-992e-4b09-a76c-9a94243015f8")
                .putExtra(AppView.NAME_EXTRA, "PC");
    }

    @Test
    public void uuidShortcutDoesNotOpenAnUnusedDatabase() {
        ActivityController<Trampoline> controller = Robolectric.buildActivity(Trampoline.class, intent()).create();
        assertEquals(0, controller.get().databaseOpens);
        controller.get().finish();
        controller.destroy();
    }

    @Test
    public void beaconContentEntryPrefersStableAppUuidOverStaleNumericId() throws Exception {
        String host = intent().getStringExtra(AppView.UUID_EXTRA);
        try (OutputStream out = CacheHelper.openCacheFileForOutput(
                RuntimeEnvironment.getApplication().getCacheDir(), "applist", host)) {
            out.write(("<root status_code=\"200\"><App><AppTitle>Renamed game</AppTitle>" +
                    "<ID>99</ID><UUID>app-uuid</UUID></App></root>").getBytes(StandardCharsets.UTF_8));
        }
        Uri uri = Uri.parse("content://beacon/games/Game.art");
        String contents = FrontendEntry.serialize(host, "PC", "app-uuid", "Old game name", 42);
        Shadows.shadowOf(RuntimeEnvironment.getApplication().getContentResolver()).registerInputStream(uri,
                new ByteArrayInputStream(contents.getBytes(StandardCharsets.UTF_8)));
        Intent launch = new Intent(Intent.ACTION_VIEW, uri).addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION);
        ActivityController<Trampoline> controller = Robolectric.buildActivity(Trampoline.class, launch).create();
        try {
            NvApp app = ReflectionHelpers.getField(controller.get(), "app");
            assertEquals("app-uuid", app.getAppUuid());
            assertEquals(99, app.getAppId());
            assertEquals("Renamed game", app.getAppName());
            assertNotNull(controller.get().connection);
        } finally {
            controller.get().finish();
            controller.destroy();
        }
    }

    @Test
    public void libraryEntryDoesNotInheritGameExtrasFromTheCallingLauncher() {
        String host = intent().getStringExtra(AppView.UUID_EXTRA);
        Uri uri = Uri.parse("content://beacon/games/PC.art");
        Shadows.shadowOf(RuntimeEnvironment.getApplication().getContentResolver()).registerInputStream(uri,
                new ByteArrayInputStream(FrontendEntry.serialize(host, "PC", null, null, 0)
                        .getBytes(StandardCharsets.UTF_8)));
        Intent launch = new Intent(Intent.ACTION_VIEW, uri).putExtra(Game.EXTRA_APP_ID, "42");
        ActivityController<Trampoline> controller = Robolectric.buildActivity(Trampoline.class, launch).create();
        try {
            assertNull(ReflectionHelpers.getField(controller.get(), "app"));
            assertFalse(controller.get().getIntent().hasExtra(Game.EXTRA_APP_ID));
        } finally {
            controller.get().finish();
            controller.destroy();
        }
    }

    @Test
    public void successfulShortcutLaunchReleasesItsBindingOnlyOnce() throws Exception {
        ActivityController<Trampoline> controller = Robolectric.buildActivity(Trampoline.class, intent())
                .create().start().resume();
        Trampoline activity = controller.get();
        ComputerDetails computer = new ComputerDetails();
        computer.uuid = intent().getStringExtra(AppView.UUID_EXTRA);
        computer.name = "PC";
        computer.state = ComputerDetails.State.ONLINE;
        computer.pairState = PairingManager.PairState.PAIRED;
        ComputerManagerService service = new ComputerManagerService();
        ComputerManagerService.ComputerManagerBinder binder = service.new ComputerManagerBinder() {
            @Override public boolean waitForReady() { return true; }
            @Override public ComputerDetails getComputer(String uuid) { return computer; }
            @Override public void invalidateStateForComputer(String uuid) { }
            @Override public void startPolling(ComputerManagerListener listener) { listener.notifyComputerUpdated(computer); }
            @Override public void stopPolling() { }
        };
        activity.connection.onServiceConnected(null, binder);
        Thread waiter = ReflectionHelpers.getField(activity, "serviceWaitThread");
        waiter.join(2000);
        assertFalse(waiter.isAlive());
        ShadowLooper.idleMainLooper();
        assertEquals(1, activity.launches);
        controller.pause().stop().destroy();
        assertEquals(1, activity.unbinds);
    }

    // The wake plan's resend interval and wait (WakeLaunch).
    private static final long RESEND_MS = 8_000;
    private static final long TIMEOUT_MS = 90_000;

    /** A trampoline bound to a manager whose polls the test hands in. */
    private static final class Polled {
        final ActivityController<Trampoline> controller = Robolectric.buildActivity(Trampoline.class, new Intent()
                .putExtra(AppView.UUID_EXTRA, "5d16bcc0-992e-4b09-a76c-9a94243015f8").putExtra(AppView.NAME_EXTRA, "PC"))
                .create().start().resume();
        final Trampoline activity = controller.get();
        final ComputerDetails computer = new ComputerDetails();
        final List<String> invalidated = new CopyOnWriteArrayList<>();
        ComputerManagerListener listener;

        Polled() throws Exception {
            computer.uuid = "5d16bcc0-992e-4b09-a76c-9a94243015f8";
            computer.name = "PC";
            computer.macAddress = "02:7c:16:b4:13:4f";
            computer.state = ComputerDetails.State.OFFLINE;
            computer.pairState = PairingManager.PairState.PAIRED;
            ComputerManagerService service = new ComputerManagerService();
            activity.connection.onServiceConnected(null, service.new ComputerManagerBinder() {
                @Override public boolean waitForReady() { return true; }
                @Override public ComputerDetails getComputer(String uuid) { return computer; }
                @Override public void invalidateStateForComputer(String uuid) { invalidated.add(uuid); }
                @Override public void startPolling(ComputerManagerListener l) { listener = l; }
                @Override public void stopPolling() { }
            });
            Thread waiter = ReflectionHelpers.getField(activity, "serviceWaitThread");
            waiter.join(2000);
            invalidated.clear();
        }

        void poll(ComputerDetails.State state) {
            computer.state = state;
            listener.notifyComputerUpdated(computer);
            ShadowLooper.idleMainLooper();
        }

        void pass(long ms) {
            Shadows.shadowOf(Looper.getMainLooper()).idleFor(ms, TimeUnit.MILLISECONDS);
        }

        /** The wake packets go out on their own thread. */
        void awaitWakes(int count) throws InterruptedException {
            for (int i = 0; i < 200 && (activity.wakes.size() < count || invalidated.size() < count); i++) {
                Thread.sleep(10);
            }
            assertEquals(count, activity.wakes.size());
            assertEquals(count, invalidated.size());
        }
    }

    @Test
    public void anAsleepPcIsWokenAndTheShortcutLaunchesOnceItAnswers() throws Exception {
        Polled polled = new Polled();
        polled.poll(ComputerDetails.State.OFFLINE);
        polled.awaitWakes(1);
        // Polls while it is away neither show an error nor send more than the wait's plan.
        polled.poll(ComputerDetails.State.OFFLINE);
        polled.poll(ComputerDetails.State.OFFLINE);
        assertEquals(1, polled.activity.wakes.size());
        assertEquals(0, polled.activity.unbinds);
        assertFalse(polled.activity.isFinishing());
        polled.pass(RESEND_MS);
        polled.awaitWakes(2);

        polled.poll(ComputerDetails.State.ONLINE);
        assertEquals(1, polled.activity.launches);
        polled.pass(TIMEOUT_MS);
        assertEquals("The wait ended with the launch", 2, polled.activity.wakes.size());
        polled.controller.pause().stop().destroy();
        assertEquals(1, polled.activity.unbinds);
    }

    @Test
    public void aPcThatNeverAnswersEndsTheWaitWithAnError() throws Exception {
        Polled polled = new Polled();
        polled.poll(ComputerDetails.State.OFFLINE);
        polled.awaitWakes(1);
        polled.pass(TIMEOUT_MS + 1_000);
        ShadowLooper.idleMainLooper();

        assertEquals(0, polled.activity.launches);
        assertEquals(1, polled.activity.unbinds);
        android.app.Dialog dialog = ShadowDialog.getLatestDialog();
        assertNotNull(dialog);
        assertTrue(((TextView) dialog.findViewById(android.R.id.message)).getText().toString()
                .contains("did not wake up in time"));
        int sent = polled.activity.wakes.size();
        assertTrue("It was woken again while the wait lasted", sent > 1);
        polled.pass(TIMEOUT_MS);
        assertEquals(sent, polled.activity.wakes.size());
        polled.controller.pause().stop().destroy();
    }

    @Test
    public void aPcWithoutAServerinfoMacWakesThroughTheMacsItsWakeListed() throws Exception {
        Polled polled = new Polled();
        polled.computer.macAddress = null;
        new WakeTargets(polled.activity).store(polled.computer.uuid, NativeApiFixtures.wake("02:e0:4c:68:00:01"));
        polled.poll(ComputerDetails.State.OFFLINE);
        polled.awaitWakes(1);
        polled.controller.pause().stop().destroy();
    }

    @Test
    public void aResendOnItsWayWhenTheShortcutStopsNeitherWakesNorPolls() throws Exception {
        Polled polled = new Polled();
        polled.poll(ComputerDetails.State.OFFLINE);
        polled.awaitWakes(1);
        HostWaker waker = ReflectionHelpers.getField(polled.activity, "waker");
        HostWaker.Sender sender = ReflectionHelpers.getField(waker, "sender");
        polled.controller.pause().stop();

        sender.send(polled.computer);
        assertEquals(1, polled.activity.wakes.size());
        assertEquals(1, polled.invalidated.size());
        polled.controller.destroy();
    }
}
