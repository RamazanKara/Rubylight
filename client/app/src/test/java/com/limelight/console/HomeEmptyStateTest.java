package com.limelight.console;

import static com.limelight.nativeapi.NativeApiFixtures.nativeHost;
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
import android.view.View;

import com.limelight.PcViewLifecycleTest;
import com.limelight.R;
import com.limelight.computers.ComputerManagerListener;
import com.limelight.computers.ComputerManagerService;
import com.limelight.nativeapi.ConsoleRepository;
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
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.TimeUnit;

/** "No PCs yet" on a start: only once no saved, paired PC is left to answer, never while one still may. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class, qualifiers = "television",
        shadows = PcViewLifecycleTest.NoNativeMoonBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
public class HomeEmptyStateTest {
    @Rule public TemporaryFolder temporary = new TemporaryFolder();

    static ConsoleRepository repository;

    /** Home without the computer manager service, over the test's repository. */
    public static class Home extends ConsoleHomeActivity {
        @Override public boolean bindService(Intent intent, ServiceConnection connection, int flags) {
            return false;
        }

        @Override protected ConsoleRepository createRepository(String uniqueId) {
            return repository;
        }
    }

    private Context context;
    private ConsoleRig rig;
    private ActivityController<Home> controller;

    @Before
    public void setUp() throws Exception {
        context = RuntimeEnvironment.getApplication();
        PreferenceManager.getDefaultSharedPreferences(context).edit().clear().commit();
        GlPreferences gl = GlPreferences.readPreferences(context);
        gl.savedFingerprint = Build.FINGERPRINT;
        gl.glRenderer = "Test renderer";
        gl.writePreferences();
        rig = new ConsoleRig(temporary.newFolder());
        repository = rig.repository;
    }

    @After
    public void tearDown() {
        if (controller != null) controller.pause().stop().destroy();
        rig.close();
        repository = null;
        PreferenceManager.getDefaultSharedPreferences(context).edit().clear().commit();
    }

    /** The PC as the manager reads it back from its saved list on a start: paired once, not polled yet. */
    private static ComputerDetails savedTower() {
        ComputerDetails saved = nativeHost("tower", "Tower", false);
        saved.state = null;
        saved.pairState = null;
        return saved;
    }

    /** Opens Home with a manager that reports {@code saved} when polling starts, as the real one does. */
    private Home open(ComputerDetails... saved) {
        controller = Robolectric.buildActivity(Home.class).create();
        Home home = controller.get();
        HostFeed feed = ReflectionHelpers.getField(home, "feed");
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

    /** Past the settle delay, then every load Home started and what it posts back. */
    private static void settle(Home home) throws Exception {
        ExecutorService worker = ReflectionHelpers.getField(home, "worker");
        for (int i = 0; i < 3; i++) {
            shadowOf(Looper.getMainLooper()).idleFor(Duration.ofMillis(2000));
            worker.submit(() -> { }).get(5, TimeUnit.SECONDS);
            shadowOf(Looper.getMainLooper()).idle();
        }
    }

    private static boolean shows(Home home, int id) {
        return home.findViewById(id).getVisibility() == View.VISIBLE;
    }

    @Test
    public void withNoSavedPcHomeSaysSoOnceItSettles() throws Exception {
        Home home = open();
        settle(home);
        assertTrue("no PC saved: nothing to wait for", shows(home, R.id.console_empty));
        assertFalse(shows(home, R.id.console_loading));
    }

    @Test
    public void aSavedPcThatHasNotAnsweredKeepsHomeLoadingNotEmpty() throws Exception {
        Home home = open(savedTower());
        settle(home);
        assertFalse("the saved PC may still answer", shows(home, R.id.console_empty));
        assertTrue("Home says it is still loading", shows(home, R.id.console_loading));
        // Its first answer: paired and awake. Its card comes, and "No PCs yet" never showed.
        HostFeed feed = ReflectionHelpers.getField(home, "feed");
        feed.update(nativeHost("tower", "Tower", true));
        settle(home);
        assertFalse(shows(home, R.id.console_empty));
        assertFalse(feed.waiting());
    }

    @Test
    public void aSavedPcIsWaitedForOnlyUntilItsFirstAnswer() {
        ComputerDetails saved = savedTower();
        assertTrue(HostFeed.waitsForFirstAnswer(saved));
        ComputerDetails unpaired = savedTower();
        unpaired.serverCert = null;
        assertFalse("never paired: nothing to wait for", HostFeed.waitsForFirstAnswer(unpaired));
        ComputerDetails answered = savedTower();
        answered.state = ComputerDetails.State.OFFLINE;
        assertFalse("its poll has answered", HostFeed.waitsForFirstAnswer(answered));
        ComputerDetails expired = nativeHost("tower", "Tower", true);
        expired.state = ComputerDetails.State.UNKNOWN;
        assertFalse("paired, its state only timed out", HostFeed.waitsForFirstAnswer(expired));
    }

    @Test
    public void theFeedWaitsUntilTheManagerReportedAndEverySavedPcAnswered() throws Exception {
        List<ComputerDetails> none = new ArrayList<>();
        Home home = open(savedTower());
        HostFeed feed = ReflectionHelpers.getField(home, "feed");
        assertTrue(feed.waiting());
        ComputerDetails offline = savedTower();
        offline.state = ComputerDetails.State.OFFLINE;
        feed.update(offline);
        assertFalse("it answered, though not as paired", feed.waiting());
        assertEquals(none, feed.hosts());
    }
}
