package com.limelight.preferences;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

import android.app.Application;
import android.content.Intent;
import android.os.Build;
import android.os.Looper;
import android.preference.Preference;

import androidx.appcompat.app.AlertDialog;

import com.limelight.PcView;
import com.limelight.PcViewLifecycleTest;
import com.limelight.computers.ComputerManagerListener;
import com.limelight.computers.ComputerManagerService;
import com.limelight.console.HomeShellTest;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.PairingManager;

import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.Shadows;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowDialog;
import org.robolectric.util.ReflectionHelpers;

import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;

/** A PC removed on its own settings page also leaves the PC list and Home when they come back to the front. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class, shadows = PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class PcRemovedFromSettingsTest {
    private final Application app = RuntimeEnvironment.getApplication();
    /** What the computer manager knows; removing a PC takes it out, as the service does. */
    private final Map<String, ComputerDetails> known = new HashMap<>();

    @Before
    public void setUp() {
        GlPreferences gl = GlPreferences.readPreferences(app);
        gl.savedFingerprint = Build.FINGERPRINT;
        gl.glRenderer = "Test renderer";
        gl.writePreferences();
        app.getSharedPreferences("FirstRun", 0).edit().putBoolean("pairing_guide_shown", true).commit();
    }

    private ComputerManagerService.ComputerManagerBinder manager() {
        return new ComputerManagerService().new ComputerManagerBinder() {
            @Override public boolean isPersistent() { return false; }
            @Override public void startPolling(ComputerManagerListener listener) { }
            @Override public void stopPolling() { }
            @Override public void renameComputer(String uuid, String alias) { }
            @Override public void removeComputer(ComputerDetails computer) { known.remove(computer.uuid); }
            @Override public ComputerDetails getComputer(String uuid) { return known.get(uuid); }
            @Override public void invalidateStateForComputer(String uuid) { }
            @Override public String getUniqueId() { return null; }
            @Override public void waitForPollingStopped() { }
        };
    }

    private static ComputerDetails tower() {
        ComputerDetails computer = new ComputerDetails();
        computer.uuid = "tower";
        computer.name = "Tower";
        computer.state = ComputerDetails.State.ONLINE;
        computer.pairState = PairingManager.PairState.PAIRED;
        computer.activeAddress = new ComputerDetails.AddressTuple("192.0.2.10", 47989);
        computer.httpsPort = 47984;
        return computer;
    }

    /** Removes the PC through the settings page: the Remove row, then the confirmation. */
    private void removeOnItsSettingsPage() {
        Intent intent = new Intent(app, HostStreamSettings.class).putExtra("host_uuid", "tower").putExtra("host_name", "Tower");
        ActivityController<HostStreamSettings> controller = Robolectric.buildActivity(HostStreamSettings.class, intent).setup();
        try {
            HostStreamSettings screen = controller.get();
            ReflectionHelpers.setField(screen, "manager", manager());
            HostStreamSettings.SettingsFragment fragment =
                    (HostStreamSettings.SettingsFragment) screen.getFragmentManager().findFragmentById(com.limelight.R.id.host_profile_settings);
            Preference remove = fragment.findPreference("remove");
            remove.getOnPreferenceClickListener().onPreferenceClick(remove);
            Shadows.shadowOf(Looper.getMainLooper()).idle();
            ((AlertDialog) ShadowDialog.getLatestDialog()).getButton(AlertDialog.BUTTON_POSITIVE).performClick();
            Shadows.shadowOf(Looper.getMainLooper()).idle();
            assertTrue("the page leaves", screen.isFinishing());
        } finally {
            controller.pause().stop().destroy();
        }
        assertNull("the manager forgot the PC", known.get("tower"));
    }

    @Test
    public void thePcListDropsAPcRemovedOnItsSettingsPage() {
        known.put("tower", tower());
        ActivityController<PcViewLifecycleTest.Library> controller =
                Robolectric.buildActivity(PcViewLifecycleTest.Library.class).setup().visible();
        try {
            PcViewLifecycleTest.Library list = controller.get();
            ReflectionHelpers.setField(list, "managerBinder", manager());
            ReflectionHelpers.callInstanceMethod(list, "updateComputer",
                    ReflectionHelpers.ClassParameter.from(ComputerDetails.class, tower()));
            assertEquals(1, rows(list));

            controller.pause();
            removeOnItsSettingsPage();
            controller.resume();
            assertEquals("the stale row is gone", 0, rows(list));
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void homeDropsAPcRemovedOnItsSettingsPage() {
        known.put("tower", tower());
        ActivityController<HomeShellTest.Home> controller = Robolectric.buildActivity(HomeShellTest.Home.class).setup().visible();
        try {
            HomeShellTest.Home home = controller.get();
            Object feed = ReflectionHelpers.getField(home, "feed");
            ReflectionHelpers.setField(feed, "binder", manager());
            ReflectionHelpers.callInstanceMethod(feed, "update", ReflectionHelpers.ClassParameter.from(ComputerDetails.class, tower()));
            assertEquals(1, hosts(feed).size());

            controller.pause();
            removeOnItsSettingsPage();
            controller.resume();
            assertTrue("the stale host is gone", hosts(feed).isEmpty());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    private static int rows(PcView list) {
        Object adapter = ReflectionHelpers.getField(list, "pcGridAdapter");
        return (Integer) ReflectionHelpers.callInstanceMethod(adapter, "getCount");
    }

    @SuppressWarnings("unchecked")
    private static List<ComputerDetails> hosts(Object feed) {
        return new ArrayList<>((List<ComputerDetails>) ReflectionHelpers.callInstanceMethod(feed, "hosts"));
    }
}
