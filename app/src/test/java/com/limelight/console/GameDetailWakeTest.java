package com.limelight.console;

import static com.limelight.nativeapi.NativeApiFixtures.game;
import static com.limelight.nativeapi.NativeApiFixtures.library;
import static com.limelight.nativeapi.NativeApiFixtures.nativeHost;
import static com.limelight.nativeapi.NativeApiFixtures.ok;
import static org.junit.Assert.assertEquals;
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

        @Override public boolean bindService(Intent intent, ServiceConnection connection, int flags) {
            return false;
        }

        @Override protected ConsoleRepository createRepository(String uniqueId) {
            return repository;
        }

        @Override protected void sendWake(ComputerDetails computer) {
            woken.add(computer.uuid);
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

    @Test
    public void playOnAPcThatIsAwayWakesItInsteadOfStayingGreyedOut() throws Exception {
        ConsoleRig rig = new ConsoleRig(temporary.newFolder());
        ActivityController<Detail> controller = null;
        try {
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
            Detail screen = controller.get();
            HostFeed feed = ReflectionHelpers.getField(screen, "feed");
            ReflectionHelpers.setField(feed, "binder", binder());
            ComputerDetails tower = nativeHost("tower", "Tower", false);
            tower.macAddress = "aa:bb:cc:dd:ee:ff";
            feed.update(tower);

            TextView play = screen.findViewById(R.id.detail_play);
            String wake = screen.getString(R.string.console_wake_and_play);
            await(() -> play.getText().toString().equals(wake) && play.isEnabled());
            assertTrue(Detail.woken.isEmpty());

            play.performClick();
            await(() -> Detail.woken.size() == 1);
            assertEquals("tower", Detail.woken.get(0));
            assertEquals("the button says it is waiting", screen.getString(R.string.wol_waking_pc),
                    play.getText().toString());
        } finally {
            if (controller != null) {
                controller.pause().stop().destroy();
            }
            rig.close();
        }
    }
}
