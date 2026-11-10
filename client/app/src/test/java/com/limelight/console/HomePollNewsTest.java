package com.limelight.console;

import static com.limelight.nativeapi.NativeApiFixtures.nativeHost;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertTrue;
import static org.robolectric.Shadows.shadowOf;

import android.app.Application;
import android.content.Context;
import android.os.Build;
import android.os.Looper;
import android.preference.PreferenceManager;

import com.limelight.PcViewLifecycleTest;
import com.limelight.computers.ComputerManagerListener;
import com.limelight.computers.ComputerManagerService;
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

import java.time.Duration;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.TimeUnit;

/**
 * The feed reports every PC with every poll. Home reads the library and binds its cards again only for a poll that
 * brought news, and for the one poll after a read of the PCs, whose result the saved library then replaces.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class, qualifiers = "television",
        shadows = PcViewLifecycleTest.NoNativeMoonBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
public class HomePollNewsTest {
    @Rule public TemporaryFolder temporary = new TemporaryFolder();

    private Context context;
    private ConsoleRig rig;
    private ActivityController<HomeEmptyStateTest.Home> controller;
    private HostFeed feed;

    @Before
    public void setUp() throws Exception {
        context = RuntimeEnvironment.getApplication();
        PreferenceManager.getDefaultSharedPreferences(context).edit().clear().commit();
        GlPreferences gl = GlPreferences.readPreferences(context);
        gl.savedFingerprint = Build.FINGERPRINT;
        gl.glRenderer = "Test renderer";
        gl.writePreferences();
        rig = new ConsoleRig(temporary.newFolder());
        HomeEmptyStateTest.repository = rig.repository;
    }

    @After
    public void tearDown() {
        if (controller != null) controller.pause().stop().destroy();
        rig.close();
        HomeEmptyStateTest.repository = null;
        PreferenceManager.getDefaultSharedPreferences(context).edit().clear().commit();
    }

    /** Home over a manager that reports {@code saved} when polling starts, with the wall clock held still. */
    private HomeEmptyStateTest.Home open(ComputerDetails... saved) {
        controller = Robolectric.buildActivity(HomeEmptyStateTest.Home.class).create();
        HomeEmptyStateTest.Home home = controller.get();
        // The cards say how long ago a game was played, so the minute turning over is news of its own.
        ReflectionHelpers.setField(home, "drawn", new HostsDrawn(() -> 0));
        feed = ReflectionHelpers.getField(home, "feed");
        ReflectionHelpers.setField(feed, "binder", new ComputerManagerService().new ComputerManagerBinder() {
            @Override public boolean isPersistent() { return false; }
            @Override public void startPolling(ComputerManagerListener listener) {
                for (ComputerDetails details : saved) listener.notifyComputerUpdated(details);
            }
            @Override public void stopPolling() { }
            @Override public void invalidateStateForComputer(String uuid) { }
            @Override public String getUniqueId() { return "unique"; }
            @Override public ComputerDetails getComputer(String uuid) {
                for (ComputerDetails details : saved) if (details.uuid.equals(uuid)) return details;
                return null;
            }
            @Override public void waitForPollingStopped() { }
        });
        controller.start().resume().visible();
        return home;
    }

    /** Past the debounce and the settle delay, then every load Home started and what it posts back. */
    private static void settle(HomeEmptyStateTest.Home home) throws Exception {
        ExecutorService worker = ReflectionHelpers.getField(home, "worker");
        for (int i = 0; i < 3; i++) {
            shadowOf(Looper.getMainLooper()).idleFor(Duration.ofMillis(2000));
            worker.submit(() -> { }).get(5, TimeUnit.SECONDS);
            shadowOf(Looper.getMainLooper()).idle();
        }
    }

    /** How many loads Home has started. */
    private static int loads(HomeEmptyStateTest.Home home) {
        return ReflectionHelpers.getField(home, "generation");
    }

    @Test
    public void onlyAPollWithNewsLoadsHomeAgain() throws Exception {
        HomeEmptyStateTest.Home home = open(nativeHost("tower", "Tower", true), nativeHost("den", "Den", true));
        settle(home);
        // The first load read the PCs. What that read saved is drawn by the load Home starts once it has settled,
        // or else by the next poll.
        feed.update(nativeHost("tower", "Tower", true));
        settle(home);

        // From here the polls report the same two PCs, each as a new copy, over and over.
        int quiet = loads(home);
        for (int i = 0; i < 5; i++) {
            feed.update(nativeHost("tower", "Tower", true));
            feed.update(nativeHost("den", "Den", true));
            settle(home);
        }
        assertEquals("no poll brought news", quiet, loads(home));

        // One of them goes away.
        feed.update(nativeHost("den", "Den", false));
        settle(home);
        assertTrue("Den went offline", loads(home) > quiet);

        // That changed what Home asks the PCs, so it read them again and the next poll draws once more; then quiet.
        feed.update(nativeHost("den", "Den", false));
        settle(home);
        int quietAgain = loads(home);
        feed.update(nativeHost("tower", "Tower", true));
        feed.update(nativeHost("den", "Den", false));
        settle(home);
        assertEquals(quietAgain, loads(home));

        // A field Home shows nothing of today is news all the same.
        ComputerDetails tower = nativeHost("tower", "Tower", true);
        tower.encoderBitrateKbps += 1000;
        feed.update(tower);
        settle(home);
        assertEquals(quietAgain + 1, loads(home));
    }
}
