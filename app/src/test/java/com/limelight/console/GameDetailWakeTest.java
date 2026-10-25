package com.limelight.console;

import static com.limelight.nativeapi.NativeApiFixtures.game;
import static com.limelight.nativeapi.NativeApiFixtures.library;
import static com.limelight.nativeapi.NativeApiFixtures.nativeHost;
import static com.limelight.nativeapi.NativeApiFixtures.ok;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;
import static org.robolectric.Shadows.shadowOf;

import android.app.Application;
import android.content.Context;
import android.content.Intent;
import android.content.ServiceConnection;
import android.os.Build;
import android.os.Looper;
import android.preference.PreferenceManager;
import android.widget.TextView;

import com.limelight.PcViewLifecycleTest;
import com.limelight.R;
import com.limelight.computers.ComputerManagerListener;
import com.limelight.computers.ComputerManagerService;
import com.limelight.nativeapi.ConsoleRepository;
import com.limelight.nativeapi.NativeApiFixtures;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvApp;
import com.limelight.preferences.GlPreferences;

import org.junit.After;
import org.junit.Before;
import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.TemporaryFolder;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;

import java.io.File;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.concurrent.TimeUnit;
import java.util.function.BooleanSupplier;

/** Play on a game's page when its PC is away: it says Wake PC and play, and pressing it sends the wake packet. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class, shadows = PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class GameDetailWakeTest {
    @Rule public TemporaryFolder temporary = new TemporaryFolder();

    /** The page without the computer manager service, with the test's library and a wake sender that stays off the network. */
    public static class Detail extends GameDetailActivity {
        static ConsoleRepository repository;
        static final List<String> woken = Collections.synchronizedList(new ArrayList<>());
        static final List<String> played = Collections.synchronizedList(new ArrayList<>());

        @Override public boolean bindService(Intent intent, ServiceConnection connection, int flags) {
            return false;
        }

        @Override protected ConsoleRepository createRepository(String uniqueId) {
            return repository;
        }

        @Override protected void sendWake(ComputerDetails computer) {
            woken.add(computer.uuid);
        }

        @Override protected void play(NvApp app, ComputerDetails computer) {
            played.add(app.getAppName());
        }
    }

    private Context context;

    @Before
    public void setUp() throws Exception {
        context = RuntimeEnvironment.getApplication();
        PreferenceManager.getDefaultSharedPreferences(context).edit().clear().commit();
        GlPreferences gl = GlPreferences.readPreferences(context);
        gl.savedFingerprint = Build.FINGERPRINT;
        gl.glRenderer = "Test renderer";
        gl.writePreferences();
        File directory = new File(context.getFilesDir(), "console-library");
        assertTrue(directory.isDirectory() || directory.mkdirs());
        Detail.woken.clear();
        Detail.played.clear();
    }

    @After
    public void tearDown() {
        Detail.repository = null;
        PreferenceManager.getDefaultSharedPreferences(context).edit().clear().commit();
    }

    private static ComputerManagerService.ComputerManagerBinder binder() {
        return new ComputerManagerService().new ComputerManagerBinder() {
            @Override public boolean isPersistent() { return false; }
            @Override public void startPolling(ComputerManagerListener listener) { }
            @Override public void stopPolling() { }
            @Override public void renameComputer(String uuid, String alias) { }
            @Override public void removeComputer(ComputerDetails computer) { }
            @Override public void invalidateStateForComputer(String uuid) { }
            @Override public String getUniqueId() { return "unique"; }
            @Override public void waitForPollingStopped() { }
        };
    }

    private static void await(BooleanSupplier done) throws Exception {
        long end = System.currentTimeMillis() + 5000;
        while (!done.getAsBoolean() && System.currentTimeMillis() < end) {
            shadowOf(Looper.getMainLooper()).idleFor(50, java.util.concurrent.TimeUnit.MILLISECONDS);
            Thread.sleep(20);
        }
        assertTrue("in time", done.getAsBoolean());
    }

    /** The page of Portal on Tower, opened while Tower is away (with a MAC address, unless {@code mac} is null). */
    private final class Page {
        final ConsoleRig rig;
        final ActivityController<Detail> controller;
        final Detail screen;
        final HostFeed feed;
        final TextView play;
        final TextView note;

        Page(String mac) throws Exception {
            rig = new ConsoleRig(temporary.newFolder());
            // The PC listed its games once while it was awake; now only that is left of it.
            rig.transport.reply("tower", "library", ok(library("r", game("t-portal", 11, "Portal", "steam:400", false))));
            rig.transport.reply("tower", "session", ok(NativeApiFixtures.IDLE));
            rig.repository.refresh(Collections.singletonList(nativeHost("tower", "Tower", true)));
            Detail.repository = rig.repository;

            Intent intent = new Intent(context, Detail.class)
                    .putExtra(GameDetailActivity.EXTRA_HOST_UUID, "tower")
                    .putExtra(GameDetailActivity.EXTRA_APP_ID, 11)
                    .putExtra(GameDetailActivity.EXTRA_TITLE, "Portal")
                    .putExtra(GameDetailActivity.EXTRA_HOST_NAME, "Tower");
            controller = Robolectric.buildActivity(Detail.class, intent).setup().visible();
            screen = controller.get();
            feed = ReflectionHelpers.getField(screen, "feed");
            ReflectionHelpers.setField(feed, "binder", binder());
            host(false, mac);
            play = screen.findViewById(R.id.detail_play);
            note = screen.findViewById(R.id.detail_note);
        }

        /** Tower as the feed reports it now. */
        void host(boolean online, String mac) {
            ComputerDetails tower = nativeHost("tower", "Tower", online);
            tower.macAddress = mac;
            feed.update(tower);
        }

        /** The time passes on the main looper, as the waker's one-second ticks need. */
        void pass(long ms) {
            shadowOf(Looper.getMainLooper()).idleFor(ms, TimeUnit.MILLISECONDS);
        }

        void waitForWakeButton() throws Exception {
            String wake = screen.getString(R.string.console_wake_and_play);
            await(() -> play.getText().toString().equals(wake) && play.isEnabled());
        }

        void close() {
            controller.pause().stop().destroy();
            rig.close();
        }
    }

    @Test
    public void playOnAPcThatIsAwayWakesItAndStartsTheGameOnceWhenItAnswers() throws Exception {
        Page page = new Page("aa:bb:cc:dd:ee:ff");
        try {
            page.waitForWakeButton();
            assertTrue(Detail.woken.isEmpty());

            page.play.performClick();
            await(() -> Detail.woken.size() == 1);
            assertEquals("tower", Detail.woken.get(0));
            assertEquals("the button says it is waiting", page.screen.getString(R.string.wol_waking_pc),
                    page.play.getText().toString());
            assertEquals(page.screen.getString(R.string.console_wake_waiting, "Tower"), page.note.getText().toString());
            assertTrue("nothing starts while it sleeps", Detail.played.isEmpty());

            page.host(true, "aa:bb:cc:dd:ee:ff");
            page.pass(1_100);
            await(() -> Detail.played.size() == 1);
            assertEquals("Portal", Detail.played.get(0));
            page.pass(60_000);
            Thread.sleep(200);
            page.pass(1_000);
            assertEquals("exactly one start", 1, Detail.played.size());
        } finally {
            page.close();
        }
    }

    @Test
    public void nothingStartsAfterTheWaitTimedOutEvenIfThePcComesUpLater() throws Exception {
        Page page = new Page("aa:bb:cc:dd:ee:ff");
        try {
            page.waitForWakeButton();
            page.play.performClick();
            await(() -> Detail.woken.size() == 1);

            page.pass(WakeLaunch.TIMEOUT_MS + 2_000);
            assertEquals("the waiting note is gone", page.screen.getString(R.string.library_offline),
                    page.note.getText().toString());
            assertEquals(page.screen.getString(R.string.console_wake_and_play), page.play.getText().toString());
            assertTrue("and Play is not greyed out, to try again", page.play.isEnabled());

            page.host(true, "aa:bb:cc:dd:ee:ff");
            page.pass(5_000);
            Thread.sleep(300);
            page.pass(1_000);
            assertTrue("a late answer starts nothing", Detail.played.isEmpty());
        } finally {
            page.close();
        }
    }

    @Test
    public void nothingStartsWhenThePageWasLeftWhileWaiting() throws Exception {
        Page page = new Page("aa:bb:cc:dd:ee:ff");
        try {
            page.waitForWakeButton();
            page.play.performClick();
            await(() -> Detail.woken.size() == 1);

            page.controller.pause();
            page.host(true, "aa:bb:cc:dd:ee:ff");
            page.pass(5_000);
            Thread.sleep(300);
            page.pass(1_000);
            assertTrue(Detail.played.isEmpty());
            page.controller.resume();
            page.pass(5_000);
            Thread.sleep(300);
            page.pass(1_000);
            assertTrue("and not when it comes back either", Detail.played.isEmpty());
        } finally {
            page.close();
        }
    }

    @Test
    public void whenTheServiceIsGoneAfterTheWakeTheGameDoesNotStartAndTheFailureIsShown() throws Exception {
        Page page = new Page("aa:bb:cc:dd:ee:ff");
        try {
            page.waitForWakeButton();
            page.play.performClick();
            await(() -> Detail.woken.size() == 1);

            ReflectionHelpers.setField(page.feed, "binder", null);
            page.host(true, "aa:bb:cc:dd:ee:ff");
            page.pass(1_100);
            await(() -> {
                android.app.Dialog dialog = org.robolectric.shadows.ShadowDialog.getLatestDialog();
                return dialog != null && dialog.isShowing();
            });
            assertTrue(Detail.played.isEmpty());
            androidx.appcompat.app.AlertDialog dialog =
                    (androidx.appcompat.app.AlertDialog) org.robolectric.shadows.ShadowDialog.getLatestDialog();
            assertEquals(page.screen.getString(R.string.error_manager_not_running),
                    ((TextView) dialog.findViewById(android.R.id.message)).getText().toString());
        } finally {
            page.close();
        }
    }

    @Test
    public void aPcWithoutAMacAddressKeepsPlayGreyedOutAsBefore() throws Exception {
        Page page = new Page(null);
        try {
            String offline = page.screen.getString(R.string.library_offline);
            await(() -> page.note.getText().toString().equals(offline));
            assertFalse(page.play.isEnabled());
            assertEquals(page.screen.getString(R.string.library_play), page.play.getText().toString());
            page.play.performClick();
            Thread.sleep(100);
            assertTrue(Detail.woken.isEmpty());
            assertTrue(Detail.played.isEmpty());
        } finally {
            page.close();
        }
    }
}
