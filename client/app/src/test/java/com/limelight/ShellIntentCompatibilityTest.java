package com.limelight;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;
import static org.robolectric.Shadows.shadowOf;

import android.app.Activity;
import android.app.Application;
import android.content.Context;
import android.content.Intent;
import android.content.ServiceConnection;
import android.content.pm.PackageManager;
import android.content.pm.ResolveInfo;
import android.net.Uri;
import android.os.Build;
import android.preference.PreferenceManager;
import android.view.View;

import com.limelight.computers.ComputerManagerListener;
import com.limelight.computers.ComputerManagerService;
import com.limelight.console.ConsoleHomeActivity;
import com.limelight.console.HomeSettings;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.http.PairingManager;
import com.limelight.preferences.GlPreferences;
import com.limelight.preferences.HostStreamSettings;
import com.limelight.preferences.StreamSettings;
import com.limelight.utils.ServerHelper;

import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowLooper;
import org.robolectric.util.ReflectionHelpers;

import java.io.File;
import java.util.ArrayList;
import java.util.List;

/**
 * The shell moves screens around but leaves every way into the app as it was: launchers, shortcuts, TV
 * channels, frontends and the extras the screens read. Each entry in the design's list has a test here.
 */
@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class, shadows = PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class ShellIntentCompatibilityTest {
    private Context context;

    @Before
    public void setUp() {
        context = RuntimeEnvironment.getApplication();
        PreferenceManager.getDefaultSharedPreferences(context).edit().clear().commit();
        GlPreferences gl = GlPreferences.readPreferences(context);
        gl.savedFingerprint = Build.FINGERPRINT;
        gl.glRenderer = "Test renderer";
        gl.writePreferences();
        context.getSharedPreferences("FirstRun", 0).edit().putBoolean("pairing_guide_shown", true).commit();
    }

    private void cacheALibrary() throws Exception {
        File directory = new File(context.getFilesDir(), "console-library");
        assertTrue(directory.isDirectory() || directory.mkdirs());
        assertTrue(new File(directory, "tower.json").createNewFile());
    }

    private List<String> resolve(Intent intent) {
        List<String> names = new ArrayList<>();
        for (ResolveInfo info : context.getPackageManager().queryIntentActivities(intent, PackageManager.MATCH_DEFAULT_ONLY
                | PackageManager.GET_RESOLVED_FILTER)) {
            if (info.activityInfo.packageName.equals(context.getPackageName())) names.add(info.activityInfo.name);
        }
        return names;
    }

    private List<String> resolveAll(Intent intent) {
        List<String> names = new ArrayList<>();
        for (ResolveInfo info : context.getPackageManager().queryIntentActivities(intent, 0)) {
            if (info.activityInfo.packageName.equals(context.getPackageName())) names.add(info.activityInfo.name);
        }
        return names;
    }

    @Test
    public void bothLaunchersResolveToPcView() {
        for (String category : new String[] {Intent.CATEGORY_LAUNCHER, Intent.CATEGORY_LEANBACK_LAUNCHER}) {
            Intent main = new Intent(Intent.ACTION_MAIN).addCategory(category).setPackage(context.getPackageName());
            // Debug builds add a preview launcher beside it.
            assertTrue(category, resolveAll(main).contains(PcView.class.getName()));
        }
    }

    @Test
    public void frontendContentEntriesStillOpenTheTrampoline() {
        Intent view = new Intent(Intent.ACTION_VIEW, Uri.parse("content://beacon/games/PC.art")).setPackage(context.getPackageName());
        assertEquals(java.util.Collections.singletonList(ShortcutTrampoline.class.getName()), resolve(view));
    }

    @Test
    public void everyScreenTheShellMovesStaysDeclaredWithTheSameExport() throws Exception {
        for (Class<?> screen : new Class<?>[] {PcView.class, ShortcutTrampoline.class, AppView.class, StreamSettings.class,
                HostStreamSettings.class, ConsoleHomeActivity.class}) {
            assertNotNull(screen.getName(), context.getPackageManager().getActivityInfo(
                    new android.content.ComponentName(context, screen), 0));
        }
        assertTrue(exported(PcView.class));
        assertTrue(exported(ShortcutTrampoline.class));
        assertFalse(exported(ConsoleHomeActivity.class));
        assertFalse(exported(HostStreamSettings.class));
    }

    private boolean exported(Class<? extends Activity> screen) {
        try {
            return context.getPackageManager().getActivityInfo(new android.content.ComponentName(context, screen), 0).exported;
        } catch (PackageManager.NameNotFoundException e) {
            throw new AssertionError(e);
        }
    }

    @Test
    public void aLauncherStartForwardsToHomeWhenHomeIsOn() throws Exception {
        cacheALibrary();
        ActivityController<PcViewLifecycleTest.Library> controller =
                Robolectric.buildActivity(PcViewLifecycleTest.Library.class, new Intent(Intent.ACTION_MAIN)).create();
        try {
            Intent forwarded = shadowOf(controller.get()).getNextStartedActivity();
            assertEquals(ConsoleHomeActivity.class.getName(), forwarded.getComponent().getClassName());
            assertTrue(controller.get().isFinishing());
        } finally {
            controller.destroy();
        }
    }

    @Test
    public void aLauncherStartStaysOnThePcsWithoutALibraryOrWithTheStayExtra() throws Exception {
        ActivityController<PcViewLifecycleTest.Library> noLibrary =
                Robolectric.buildActivity(PcViewLifecycleTest.Library.class, new Intent(Intent.ACTION_MAIN)).setup();
        try {
            assertNull(shadowOf(noLibrary.get()).getNextStartedActivity());
            assertFalse(noLibrary.get().isFinishing());
            assertNotNull(noLibrary.get().findViewById(R.id.shell_bar));
        } finally {
            noLibrary.pause().stop().destroy();
        }

        cacheALibrary();
        Intent stay = new Intent(Intent.ACTION_MAIN).putExtra(HomeSettings.EXTRA_STAY, true);
        ActivityController<PcViewLifecycleTest.Library> staying =
                Robolectric.buildActivity(PcViewLifecycleTest.Library.class, stay).setup();
        try {
            assertNull(shadowOf(staying.get()).getNextStartedActivity());
            assertFalse(staying.get().isFinishing());
        } finally {
            staying.pause().stop().destroy();
        }
    }

    @Test
    public void theWayBackToThePcsKeepsItsExtra() {
        Intent intent = HomeSettings.pcList(context);
        assertEquals(PcView.class.getName(), intent.getComponent().getClassName());
        assertEquals("com.limelight.console.STAY_ON_PC_LIST", HomeSettings.EXTRA_STAY);
        assertTrue(intent.getBooleanExtra(HomeSettings.EXTRA_STAY, false));
    }

    /** A trampoline that records what it would launch instead of starting it. */
    public static class Trampoline extends ShortcutTrampoline {
        ServiceConnection connection;
        Intent[] launched;

        @Override public boolean bindService(Intent intent, ServiceConnection connection, int flags) {
            this.connection = connection;
            return true;
        }

        @Override public void unbindService(ServiceConnection connection) { }

        @Override public void startActivities(Intent[] intents) {
            launched = intents;
        }
    }

    @Test
    public void aPcShortcutLaunchesTheListThenTheLibraryWithItsExtras() throws Exception {
        Intent shortcut = new Intent().putExtra(AppView.UUID_EXTRA, "5d16bcc0-992e-4b09-a76c-9a94243015f8")
                .putExtra(AppView.NAME_EXTRA, "PC");
        ActivityController<Trampoline> controller = Robolectric.buildActivity(Trampoline.class, shortcut)
                .create().start().resume();
        Trampoline activity = controller.get();
        ComputerDetails computer = new ComputerDetails();
        computer.uuid = shortcut.getStringExtra(AppView.UUID_EXTRA);
        computer.name = "PC";
        computer.state = ComputerDetails.State.ONLINE;
        computer.pairState = PairingManager.PairState.PAIRED;
        ComputerManagerService.ComputerManagerBinder binder = new ComputerManagerService().new ComputerManagerBinder() {
            @Override public boolean waitForReady() { return true; }
            @Override public ComputerDetails getComputer(String uuid) { return computer; }
            @Override public void invalidateStateForComputer(String uuid) { }
            @Override public void startPolling(ComputerManagerListener listener) { listener.notifyComputerUpdated(computer); }
            @Override public void stopPolling() { }
        };
        try {
            activity.connection.onServiceConnected(null, binder);
            Thread waiter = ReflectionHelpers.getField(activity, "serviceWaitThread");
            waiter.join(2000);
            ShadowLooper.idleMainLooper();
            assertNotNull(activity.launched);
            assertEquals(2, activity.launched.length);

            Intent pcs = activity.launched[0];
            assertEquals(PcView.class.getName(), pcs.getComponent().getClassName());
            assertEquals(Intent.ACTION_MAIN, pcs.getAction());
            assertEquals(Intent.FLAG_ACTIVITY_CLEAR_TASK | Intent.FLAG_ACTIVITY_NEW_TASK, pcs.getFlags());

            Intent library = activity.launched[1];
            assertEquals(AppView.class.getName(), library.getComponent().getClassName());
            assertEquals("5d16bcc0-992e-4b09-a76c-9a94243015f8", library.getStringExtra(AppView.UUID_EXTRA));
            assertEquals("PC", library.getStringExtra(AppView.NAME_EXTRA));
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void shortcutsAndTvChannelsStillPointAtTheTrampolineWithTheirExtras() {
        ComputerDetails computer = new ComputerDetails();
        computer.uuid = "host-uuid";
        computer.name = "PC";
        ActivityController<PcViewLifecycleTest.Library> controller =
                Robolectric.buildActivity(PcViewLifecycleTest.Library.class, HomeSettings.pcList(context)).setup();
        try {
            Intent pc = ServerHelper.createPcShortcutIntent(controller.get(), computer);
            assertEquals(ShortcutTrampoline.class.getName(), pc.getComponent().getClassName());
            assertEquals("host-uuid", pc.getStringExtra(AppView.UUID_EXTRA));
            assertEquals("PC", pc.getStringExtra(AppView.NAME_EXTRA));

            NvApp app = new NvApp("Portal", 7, false);
            Intent game = ServerHelper.createAppShortcutIntent(controller.get(), computer, app);
            assertEquals(ShortcutTrampoline.class.getName(), game.getComponent().getClassName());
            assertEquals("host-uuid", game.getStringExtra(AppView.UUID_EXTRA));
            assertEquals("Portal", game.getStringExtra(Game.EXTRA_APP_NAME));
            assertEquals("7", game.getStringExtra(Game.EXTRA_APP_ID));
            assertTrue(game.hasExtra(Game.EXTRA_APP_UUID));
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void extraNamesTheLibraryAndPcSettingsReadAreUnchanged() {
        assertEquals("Name", AppView.NAME_EXTRA);
        assertEquals("UUID", AppView.UUID_EXTRA);
        assertEquals("NewPair", AppView.NEW_PAIR_EXTRA);
        assertEquals("ShowHiddenApps", AppView.SHOW_HIDDEN_APPS_EXTRA);
        assertEquals("AppName", Game.EXTRA_APP_NAME);
        assertEquals("AppId", Game.EXTRA_APP_ID);
        assertEquals("AppUuid", Game.EXTRA_APP_UUID);
    }

    @Test
    public void hostSettingsEntryPointsKeepTheirExtras() {
        ActivityController<PcViewLifecycleTest.Library> controller =
                Robolectric.buildActivity(PcViewLifecycleTest.Library.class, HomeSettings.pcList(context)).setup();
        try {
            Activity screen = controller.get();
            HostStreamSettings.show(screen, "host-uuid", "PC");
            Intent host = shadowOf(screen).getNextStartedActivity();
            assertEquals(HostStreamSettings.class.getName(), host.getComponent().getClassName());
            assertEquals("host-uuid", host.getStringExtra("host_uuid"));
            assertEquals("PC", host.getStringExtra("host_name"));

            HostStreamSettings.showProfile(screen, "profile-1");
            Intent profile = shadowOf(screen).getNextStartedActivity();
            assertEquals("PROFILE", profile.getStringExtra("mode"));
            assertEquals("profile-1", profile.getStringExtra("profile_id"));

            HostStreamSettings.showGame(screen, "host-uuid", "PC", "app-uuid", 7, "Portal");
            Intent game = shadowOf(screen).getNextStartedActivity();
            assertEquals("GAME", game.getStringExtra("mode"));
            assertEquals("host-uuid", game.getStringExtra("host_uuid"));
            assertEquals("app-uuid", game.getStringExtra("app_uuid"));
            assertEquals(7, game.getIntExtra("app_id", -1));
            assertEquals("Portal", game.getStringExtra("app_name"));
        } finally {
            controller.pause().stop().destroy();
        }
    }

    private static String titleOf(StreamSettings screen) {
        // The window carries the page's name: on the overview the selected tab is the visible one.
        return screen.getTitle().toString();
    }

    private ActivityController<StreamSettings> settings(String section) {
        Intent intent = new Intent(context, StreamSettings.class);
        if (section != null) intent.putExtra(StreamSettings.EXTRA_SECTION, section);
        return Robolectric.buildActivity(StreamSettings.class, intent).setup().visible();
    }

    @Test
    public void settingsWithNoExtrasOpensTheListOfSections() {
        ActivityController<StreamSettings> controller = settings(null);
        try {
            assertEquals(context.getString(R.string.settings), titleOf(controller.get()));
            assertEquals(View.VISIBLE, controller.get().findViewById(R.id.shell_bar).getVisibility());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void settingsOpensTheNamedSectionAndTheLegacyDeveloperName() {
        String[][] cases = {
                {"video", context.getString(R.string.settings_section_video)},
                {"audio", context.getString(R.string.settings_section_audio)},
                {"streaming", context.getString(R.string.settings_section_streaming)},
                {"developer", context.getString(R.string.settings_section_advanced)},
        };
        for (String[] item : cases) {
            ActivityController<StreamSettings> controller = settings(item[0]);
            try {
                assertEquals(item[0], item[1], titleOf(controller.get()));
                assertEquals(item[0], View.GONE, controller.get().findViewById(R.id.shell_bar).getVisibility());
            } finally {
                controller.pause().stop().destroy();
            }
        }
    }

    @Test
    public void settingsFallsBackToTheListForAnUnknownSection() {
        ActivityController<StreamSettings> controller = settings("no-such-section");
        try {
            assertEquals(context.getString(R.string.settings), titleOf(controller.get()));
        } finally {
            controller.pause().stop().destroy();
        }
    }
}
