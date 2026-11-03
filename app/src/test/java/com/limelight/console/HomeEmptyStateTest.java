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
import com.limelight.nvstream.http.PairingManager;
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

/** "No PCs yet" on a start: only once no saved, paired PC is left to answer, never while one still may. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class, qualifiers = "television",
        shadows = PcViewLifecycleTest.NoNativeMoonBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
public class HomeEmptyStateTest {
    @Rule public TemporaryFolder temporary = new TemporaryFolder();

    static ConsoleRepository repository;
    /** The bind "works" but the manager never connects, as when it hangs on its start. */
    static boolean managerHangs;

    /** Home without the computer manager service, over the test's repository. */
    public static class Home extends ConsoleHomeActivity {
        @Override public boolean bindService(Intent intent, ServiceConnection connection, int flags) {
            return managerHangs;
        }

        @Override public void unbindService(ServiceConnection connection) { }

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
        managerHangs = false;
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
    public void theFeedWaitsUntilEverySavedPcAnsweredAndAnOfflineOneStaysPaired() throws Exception {
        Home home = open(savedTower());
        HostFeed feed = ReflectionHelpers.getField(home, "feed");
        assertTrue(feed.waiting());
        assertTrue(feed.hosts().isEmpty());
        ComputerDetails offline = savedTower();
        offline.state = ComputerDetails.State.OFFLINE;
        feed.update(offline);
        assertFalse("it answered", feed.waiting());
        assertEquals(1, feed.hosts().size());
        ComputerDetails shown = feed.hosts().get(0);
        assertEquals(ComputerDetails.State.OFFLINE, shown.state);
        assertEquals("paired as far as anyone knows", PairingManager.PairState.PAIRED, shown.pairState);
        // A PC that answers as not paired leaves Home.
        ComputerDetails unpaired = nativeHost("tower", "Tower", true);
        unpaired.pairState = PairingManager.PairState.NOT_PAIRED;
        feed.update(unpaired);
        assertTrue(feed.hosts().isEmpty());
    }

    /** Whether Home has a card for the host: on a TV's shelves, or a phone's list. */
    private static boolean hasHostCard(Home home, String uuid) {
        ConsoleHomeAdapter shelves = ReflectionHelpers.getField(home, "shelves");
        if (shelves != null) return shelves.positionOfKey("host:" + uuid) != null;
        ConsoleHomeListAdapter adapter = ReflectionHelpers.getField(home, "adapter");
        return adapter.positionOfKey("host:" + uuid) >= 0;
    }

    @Test
    public void aSavedPcThatAnswersOfflineWithNothingSavedIsAnOfflineCardWithWake() throws Exception {
        offlineCardWithWake();
    }

    @Test
    @Config(qualifiers = "w411dp-h891dp-port-xxhdpi")
    public void onAPhoneTooASavedOfflinePcIsACardWithWake() throws Exception {
        offlineCardWithWake();
    }

    private void offlineCardWithWake() throws Exception {
        ComputerDetails saved = savedTower();
        saved.macAddress = "aa:bb:cc:dd:ee:ff";
        Home home = open(saved);
        settle(home);
        HostFeed feed = ReflectionHelpers.getField(home, "feed");
        ComputerDetails offline = new ComputerDetails(saved);
        offline.state = ComputerDetails.State.OFFLINE;
        feed.update(offline);
        settle(home);
        assertFalse("a paired PC is known", shows(home, R.id.console_empty));
        assertFalse(shows(home, R.id.console_loading));
        assertTrue("its card", hasHostCard(home, "tower"));
        assertTrue("and Wake", HostWaker.canWake(home, feed.host("tower")));
    }

    @Test
    public void aSavedPcThatNeverAnswersShowsOfflineAfterTheLimitNotAnEndlessBar() throws Exception {
        Home home = open(savedTower());
        settle(home);
        assertTrue(shows(home, R.id.console_loading));
        shadowOf(Looper.getMainLooper()).idleFor(Duration.ofSeconds(10));
        settle(home);
        HostFeed feed = ReflectionHelpers.getField(home, "feed");
        assertFalse(feed.waiting());
        assertFalse("the bar stops", shows(home, R.id.console_loading));
        assertFalse(shows(home, R.id.console_empty));
        assertTrue("the PC shows offline", hasHostCard(home, "tower"));
        assertEquals(ComputerDetails.State.OFFLINE, feed.host("tower").state);
    }

    @Test
    public void aManagerThatNeverBecomesReadyEndsInNoPcsNotAnEndlessBar() throws Exception {
        managerHangs = true;
        controller = Robolectric.buildActivity(Home.class).setup().visible();
        Home home = controller.get();
        settle(home);
        assertTrue("still waiting for the manager", shows(home, R.id.console_loading));
        assertFalse(shows(home, R.id.console_empty));
        shadowOf(Looper.getMainLooper()).idleFor(Duration.ofSeconds(10));
        settle(home);
        assertFalse("the bar stops", shows(home, R.id.console_loading));
        assertTrue("Home says what it knows, with its way to the PCs", shows(home, R.id.console_empty));
    }
}
