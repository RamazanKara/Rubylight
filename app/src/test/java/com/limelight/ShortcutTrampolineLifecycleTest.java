package com.limelight;

import android.app.Application;
import android.content.Intent;
import android.content.ServiceConnection;
import android.database.sqlite.SQLiteDatabase;
import android.net.Uri;

import com.limelight.computers.ComputerManagerListener;
import com.limelight.computers.ComputerManagerService;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.PairingManager;
import com.limelight.nvstream.http.NvApp;
import com.limelight.utils.CacheHelper;
import com.limelight.utils.FrontendEntry;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.Shadows;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowLooper;
import org.robolectric.util.ReflectionHelpers;

import java.io.ByteArrayInputStream;
import java.io.OutputStream;
import java.nio.charset.StandardCharsets;

import static org.junit.Assert.*;

@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class)
public class ShortcutTrampolineLifecycleTest {
    public static class Trampoline extends ShortcutTrampoline {
        ServiceConnection connection;
        int unbinds;
        int databaseOpens;
        int launches;

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
}
