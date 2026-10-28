package com.limelight.preferences;

import android.app.Application;
import android.content.Intent;
import android.net.Uri;
import android.os.Looper;
import android.preference.Preference;
import android.preference.PreferenceCategory;
import android.preference.PreferenceScreen;
import android.widget.TextView;

import androidx.appcompat.app.AlertDialog;

import com.google.android.material.appbar.MaterialToolbar;
import com.limelight.R;
import com.limelight.computers.HostCapabilityCache;
import com.limelight.nativeapi.NativeApiFixtures;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.PairingManager;
import com.limelight.nvstream.http.TestHosts;
import com.limelight.nvstream.wol.WakeTargets;
import com.limelight.utils.CacheHelper;
import com.limelight.utils.FrontendLauncher;
import com.limelight.utils.FrontendExportStore;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.Shadows;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowDialog;
import org.robolectric.shadows.ShadowToast;

import java.io.OutputStream;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.List;

import static org.junit.Assert.*;

/** A PC's settings screen: the PC as its title, its stream values, display, connection and host. */
@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class PcSettingsTest {
    private final Application app = RuntimeEnvironment.getApplication();

    private ActivityController<HostStreamSettings> open(String uuid, HostCapabilities caps) {
        if (caps != null) {
            HostCapabilityCache.save(app, uuid, caps);
        }
        Intent intent = new Intent(app, HostStreamSettings.class).putExtra("host_uuid", uuid).putExtra("host_name", "Den PC");
        return Robolectric.buildActivity(HostStreamSettings.class, intent).setup();
    }

    private static HostStreamSettings.SettingsFragment settings(HostStreamSettings screen) {
        return (HostStreamSettings.SettingsFragment) screen.getFragmentManager().findFragmentById(R.id.host_profile_settings);
    }

    private static List<String> groups(HostStreamSettings screen) {
        PreferenceScreen root = settings(screen).getPreferenceScreen();
        List<String> titles = new ArrayList<>();
        for (int i = 0; i < root.getPreferenceCount(); i++) {
            if (root.getPreference(i) instanceof PreferenceCategory) {
                titles.add(root.getPreference(i).getTitle().toString());
            }
        }
        return titles;
    }

    private static Preference row(HostStreamSettings screen, String key) {
        return settings(screen).findPreference(key);
    }

    private static String summary(Preference preference) {
        return preference.getSummary() == null ? "" : preference.getSummary().toString();
    }

    private static void click(Preference preference) {
        preference.getOnPreferenceClickListener().onPreferenceClick(preference);
        Shadows.shadowOf(Looper.getMainLooper()).idle();
    }

    private static ComputerDetails pc(String uuid, HostCapabilities caps, boolean online) {
        ComputerDetails computer = new ComputerDetails();
        computer.uuid = uuid;
        computer.name = "Den PC";
        computer.caps = caps;
        computer.state = online ? ComputerDetails.State.ONLINE : ComputerDetails.State.OFFLINE;
        computer.pairState = PairingManager.PairState.PAIRED;
        computer.activeAddress = online ? new ComputerDetails.AddressTuple("192.0.2.10", 47989) : null;
        computer.httpsPort = 47984;
        return computer;
    }

    @Test
    public void thePcNamesThePageAndItsHostIsTheSubtitle() {
        ActivityController<HostStreamSettings> controller = open("named-pc", TestHosts.rubylight());
        try {
            HostStreamSettings screen = controller.get();
            MaterialToolbar toolbar = screen.findViewById(R.id.host_profile_toolbar);
            assertEquals("Den PC", toolbar.getTitle().toString());
            assertEquals(TestHosts.rubylight().familyLabel(app), toolbar.getSubtitle().toString());
            assertEquals(List.of(app.getString(R.string.settings_section_video), app.getString(R.string.pc_settings_display),
                    app.getString(R.string.pc_settings_connection), app.getString(R.string.pc_settings_host)), groups(screen));
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void aHostWithoutDisplayControlsHasNoDisplayGroup() {
        ActivityController<HostStreamSettings> controller = open("sunshine-hub", TestHosts.fixture("sunshine-serverinfo.xml"));
        try {
            assertEquals(List.of(app.getString(R.string.settings_section_video), app.getString(R.string.pc_settings_connection),
                    app.getString(R.string.pc_settings_host)), groups(controller.get()));
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void streamValuesSaySameAsAllPcsUntilThisPcSetsThem() {
        HostStreamProfile profile = new HostStreamProfile();
        profile.bitrate = 45500;
        profile.save(app, "own-bitrate-pc");
        ActivityController<HostStreamSettings> controller = open("own-bitrate-pc", TestHosts.rubylight());
        try {
            HostStreamSettings screen = controller.get();
            String same = app.getString(R.string.host_profile_follows, "");
            for (String key : new String[] {"resolution", "refresh", "codec", "hdr", "vrr"}) {
                assertTrue(key, summary(row(screen, key)).startsWith(same));
            }
            assertFalse(summary(row(screen, "bitrate")).startsWith(same));
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void connectionTestWaitsForThePcAndSaysWhatItMeasures() {
        ActivityController<HostStreamSettings> controller = open("test-pc", TestHosts.rubylight());
        try {
            HostStreamSettings screen = controller.get();
            Preference test = row(screen, "connection_test");
            assertFalse(test.isEnabled());
            assertTrue(summary(test).endsWith(app.getString(R.string.pc_settings_needs_online)));

            screen.showComputer(pc("test-pc", TestHosts.rubylight(), true));
            test = row(screen, "connection_test");
            assertTrue(test.isEnabled());
            assertEquals(app.getString(R.string.pc_settings_test_speed), summary(test));

            screen.showComputer(pc("test-pc", TestHosts.fixture("sunshine-serverinfo.xml"), true));
            assertEquals(app.getString(R.string.pc_settings_test_delay), summary(row(screen, "connection_test")));
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void connectionTestFollowsThePcWhileTheScreenIsInFront() {
        // The manager's copy when the screen opened did not know the PC answers; only the polls after it do.
        List<com.limelight.computers.ComputerManagerListener> listeners = new ArrayList<>();
        List<String> stops = new ArrayList<>();
        ActivityController<HostStreamSettings> controller = open("live-pc", TestHosts.rubylight());
        try {
            HostStreamSettings screen = controller.get();
            org.robolectric.util.ReflectionHelpers.setField(screen, "manager", polling(listeners, stops));
            controller.pause().resume();
            assertEquals("the screen polls while it is in front", 1, listeners.size());
            assertFalse(row(screen, "connection_test").isEnabled());

            listeners.get(0).notifyComputerUpdated(pc("live-pc", TestHosts.rubylight(), true));
            Shadows.shadowOf(Looper.getMainLooper()).idle();
            Preference test = row(screen, "connection_test");
            assertTrue(test.isEnabled());

            listeners.get(0).notifyComputerUpdated(pc("live-pc", TestHosts.rubylight(), true));
            listeners.get(0).notifyComputerUpdated(pc("other-pc", TestHosts.rubylight(), false));
            Shadows.shadowOf(Looper.getMainLooper()).idle();
            assertSame("an unchanged PC keeps the rows, and the focused one", test, row(screen, "connection_test"));
            assertTrue(test.isEnabled());

            listeners.get(0).notifyComputerUpdated(pc("live-pc", TestHosts.rubylight(), false));
            Shadows.shadowOf(Looper.getMainLooper()).idle();
            assertFalse(row(screen, "connection_test").isEnabled());

            controller.pause();
            assertEquals("the list behind polls again once this screen leaves", List.of("stop"), stops);
            controller.resume();
        } finally {
            controller.pause().stop().destroy();
        }
    }

    private static com.limelight.computers.ComputerManagerService.ComputerManagerBinder polling(
            List<com.limelight.computers.ComputerManagerListener> listeners, List<String> stops) {
        return new com.limelight.computers.ComputerManagerService().new ComputerManagerBinder() {
            @Override public boolean isPersistent() { return false; }
            @Override public void startPolling(com.limelight.computers.ComputerManagerListener listener) {
                listeners.add(listener);
            }
            @Override public void stopPolling() { stops.add("stop"); }
        };
    }

    @Test
    public void hostWebUiExplainsLoginThenOpensTheCurrentHostsConsole() {
        ActivityController<HostStreamSettings> controller = open("web-pc", TestHosts.rubylight());
        try {
            HostStreamSettings screen = controller.get();
            assertFalse("No address yet", row(screen, "web_ui").isEnabled());
            ComputerDetails computer = pc("web-pc", TestHosts.rubylight(), true);
            computer.activeAddress = new ComputerDetails.AddressTuple("2001:db8::1", 50000);
            computer.httpsPort = 49995;
            screen.showComputer(computer);
            Preference web = row(screen, "web_ui");
            assertEquals("https://[2001:db8::1]:50001/", summary(web));
            click(web);
            AlertDialog dialog = (AlertDialog) ShadowDialog.getLatestDialog();
            TextView message = dialog.findViewById(android.R.id.message);
            assertEquals(app.getString(R.string.host_administration_help, "https://[2001:db8::1]:50001/"),
                    message.getText().toString());
            assertTrue(message.isTextSelectable());
            assertNull(Shadows.shadowOf(screen).getNextStartedActivity());
            dialog.getButton(AlertDialog.BUTTON_POSITIVE).performClick();
            Shadows.shadowOf(Looper.getMainLooper()).idle();
            Intent browser = Shadows.shadowOf(screen).getNextStartedActivity();
            assertEquals(Intent.ACTION_VIEW, browser.getAction());
            assertEquals("https://[2001:db8::1]:50001/", browser.getDataString());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void gameStreamHasNoWebUiButExplainsItsEnd() {
        ActivityController<HostStreamSettings> controller = open("gfe-pc", TestHosts.fixture("gamestream-serverinfo.xml"));
        try {
            HostStreamSettings screen = controller.get();
            assertNull(row(screen, "web_ui"));
            assertNotNull(row(screen, "gamestream_eol"));
            assertEquals(app.getString(R.string.pc_settings_details_summary), summary(row(screen, "details")));
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void anUnpairedPcHasNoFrontendExport() {
        ActivityController<HostStreamSettings> controller = open("unpaired-pc", TestHosts.rubylight());
        try {
            HostStreamSettings screen = controller.get();
            assertNotNull(row(screen, "frontend_export"));
            ComputerDetails computer = pc("unpaired-pc", TestHosts.rubylight(), true);
            computer.pairState = PairingManager.PairState.NOT_PAIRED;
            screen.showComputer(computer);
            assertNull(row(screen, "frontend_export"));
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void wakeHelpOffersToWakeAnOfflinePcThisDeviceKnows() {
        ActivityController<HostStreamSettings> controller = open("asleep-pc", TestHosts.rubylight());
        try {
            HostStreamSettings screen = controller.get();
            assertEquals(app.getString(R.string.pc_settings_wake_unknown), summary(row(screen, "wake")));
            ComputerDetails computer = pc("asleep-pc", TestHosts.rubylight(), false);
            computer.macAddress = "02:00:00:00:00:01";
            screen.showComputer(computer);
            Preference wake = row(screen, "wake");
            assertEquals(app.getString(R.string.pc_settings_wake_ready), summary(wake));
            click(wake);
            AlertDialog dialog = (AlertDialog) ShadowDialog.getLatestDialog();
            assertEquals(app.getString(R.string.pcview_menu_send_wol),
                    dialog.getButton(AlertDialog.BUTTON_POSITIVE).getText().toString());
            dialog.dismiss();
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void wakeHelpCountsTheMacsAnOfflinePcsWakeListed() throws Exception {
        ActivityController<HostStreamSettings> controller = open("asleep-pc", TestHosts.rubylight());
        try {
            HostStreamSettings screen = controller.get();
            new WakeTargets(app).store("asleep-pc", NativeApiFixtures.wake("02:e0:4c:68:00:01"));
            ComputerDetails computer = pc("asleep-pc", TestHosts.rubylight(), false);
            computer.macAddress = null;
            screen.showComputer(computer);
            assertEquals(app.getString(R.string.pc_settings_wake_ready), summary(row(screen, "wake")));
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void frontendExportKeepsItsFirstFolderAcrossRecreation() throws Exception {
        try (OutputStream out = CacheHelper.openCacheFileForOutput(app.getCacheDir(), "applist", "export-pc")) {
            out.write("<root status_code=\"200\"><App><AppTitle>Racing Game</AppTitle><ID>2</ID></App></root>"
                    .getBytes(StandardCharsets.UTF_8));
        }
        ActivityController<HostStreamSettings> controller = open("export-pc", TestHosts.rubylight());
        try {
            click(row(controller.get(), "frontend_export"));
            AlertDialog launchers = (AlertDialog) ShadowDialog.getLatestDialog();
            launchers.getListView().performItemClick(null, FrontendLauncher.ES_DE.ordinal(), 0);
            ((AlertDialog) ShadowDialog.getLatestDialog()).getButton(AlertDialog.BUTTON_POSITIVE).performClick();
            Shadows.shadowOf(Looper.getMainLooper()).idle();
            Intent picker = Shadows.shadowOf(controller.get()).getNextStartedActivityForResult().intent;
            assertEquals(Intent.ACTION_OPEN_DOCUMENT_TREE, picker.getAction());
            assertTrue((picker.getFlags() & Intent.FLAG_GRANT_PERSISTABLE_URI_PERMISSION) != 0);
            Shadows.shadowOf(controller.get()).receiveResult(picker, android.app.Activity.RESULT_OK,
                    new Intent().setData(Uri.parse("content://documents/tree/roms")));
            Shadows.shadowOf(Looper.getMainLooper()).idle();

            controller.recreate();
            // The ES-DE folder arrives in the new activity, which still knows the ROMs folder.
            controller.get().onActivityResult(2, android.app.Activity.RESULT_CANCELED, null);
            Shadows.shadowOf(Looper.getMainLooper()).idle();
            assertEquals(app.getString(R.string.frontend_export_working), ShadowToast.getTextOfLatestToast());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void savedBeaconExportOffersSetupWithoutPickingFoldersAgain() throws Exception {
        try (OutputStream out = CacheHelper.openCacheFileForOutput(app.getCacheDir(), "applist", "beacon-pc")) {
            out.write("<root status_code=\"200\"><App><AppTitle>Game</AppTitle><ID>2</ID></App></root>"
                    .getBytes(StandardCharsets.UTF_8));
        }
        new FrontendExportStore(app, "beacon-pc", FrontendLauncher.BEACON)
                .save(Uri.parse("content://documents/tree/roms"), null);
        ActivityController<HostStreamSettings> controller = open("beacon-pc", TestHosts.rubylight());
        try {
            HostStreamSettings screen = controller.get();
            click(row(screen, "frontend_export"));
            AlertDialog launchers = (AlertDialog) ShadowDialog.getLatestDialog();
            assertEquals("Beacon", launchers.getListView().getAdapter().getItem(0));
            launchers.getListView().performItemClick(null, FrontendLauncher.BEACON.ordinal(), 0);
            AlertDialog actions = (AlertDialog) ShadowDialog.getLatestDialog();
            assertEquals(app.getString(R.string.frontend_export_update), actions.getListView().getAdapter().getItem(0));
            actions.getListView().performItemClick(null, 2, 0);
            AlertDialog setup = (AlertDialog) ShadowDialog.getLatestDialog();
            String command = FrontendLauncher.BEACON.command(screen.getPackageName());
            assertTrue(((TextView) setup.findViewById(android.R.id.message)).getText().toString().contains(command));
            setup.getButton(AlertDialog.BUTTON_NEUTRAL).performClick();
            Shadows.shadowOf(Looper.getMainLooper()).idle();
            android.content.ClipboardManager clipboard = app.getSystemService(android.content.ClipboardManager.class);
            assertEquals(command, clipboard.getPrimaryClip().getItemAt(0).getText().toString());
            assertNull(Shadows.shadowOf(screen).getNextStartedActivityForResult());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    private static com.limelight.computers.ComputerManagerService.ComputerManagerBinder manager(
            List<String> renamed, List<ComputerDetails> removed) {
        return new com.limelight.computers.ComputerManagerService().new ComputerManagerBinder() {
            @Override public boolean isPersistent() { return false; }
            @Override public void startPolling(com.limelight.computers.ComputerManagerListener listener) { }
            @Override public void stopPolling() { }
            @Override public void renameComputer(String uuid, String alias) { renamed.add(uuid + "=" + alias); }
            @Override public void removeComputer(ComputerDetails computer) { removed.add(computer); }
            @Override public void invalidateStateForComputer(String uuid) { }
            @Override public String getUniqueId() { return null; }
            @Override public void waitForPollingStopped() { }
        };
    }

    @Test
    public void theNameGivenWithRenameIsThePageTitle() {
        com.limelight.computers.HostNames.set(app, "renamed-pc", "Study");
        ActivityController<HostStreamSettings> controller = open("renamed-pc", TestHosts.rubylight());
        try {
            HostStreamSettings screen = controller.get();
            MaterialToolbar toolbar = screen.findViewById(R.id.host_profile_toolbar);
            assertEquals("Study", toolbar.getTitle().toString());
            // The manager's copy has the PC's own name; the alias still wins.
            screen.showComputer(pc("renamed-pc", TestHosts.rubylight(), true));
            assertEquals("Study", toolbar.getTitle().toString());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void renameFromThePageStoresTheNameAndRetitlesIt() {
        List<String> renamed = new ArrayList<>();
        ActivityController<HostStreamSettings> controller = open("rename-pc", TestHosts.rubylight());
        try {
            HostStreamSettings screen = controller.get();
            org.robolectric.util.ReflectionHelpers.setField(screen, "manager", manager(renamed, new ArrayList<>()));
            click(row(screen, "rename"));
            AlertDialog dialog = (AlertDialog) ShadowDialog.getLatestDialog();
            android.widget.EditText name = dialog.findViewById(R.id.rename_name);
            assertEquals("Den PC", name.getText().toString());
            name.setText("Attic");
            dialog.getButton(AlertDialog.BUTTON_POSITIVE).performClick();
            Shadows.shadowOf(Looper.getMainLooper()).idle();
            assertEquals(List.of("rename-pc=Attic"), renamed);
            MaterialToolbar toolbar = screen.findViewById(R.id.host_profile_toolbar);
            assertEquals("Attic", toolbar.getTitle().toString());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void removeFromThePageAsksThenRemovesAndLeaves() {
        List<ComputerDetails> removed = new ArrayList<>();
        ActivityController<HostStreamSettings> controller = open("remove-pc", TestHosts.rubylight());
        try {
            HostStreamSettings screen = controller.get();
            org.robolectric.util.ReflectionHelpers.setField(screen, "manager", manager(new ArrayList<>(), removed));
            click(row(screen, "remove"));
            assertTrue("it asks first", removed.isEmpty());
            AlertDialog dialog = (AlertDialog) ShadowDialog.getLatestDialog();
            dialog.getButton(AlertDialog.BUTTON_POSITIVE).performClick();
            Shadows.shadowOf(Looper.getMainLooper()).idle();
            assertEquals(1, removed.size());
            assertEquals("remove-pc", removed.get(0).uuid);
            assertTrue(screen.isFinishing());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void unpairIsOfferedForAPairedPcThatAnswers() {
        ActivityController<HostStreamSettings> controller = open("unpair-pc", TestHosts.rubylight());
        try {
            HostStreamSettings screen = controller.get();
            assertNull("nothing is known about the PC yet", row(screen, "unpair"));
            screen.showComputer(pc("unpair-pc", TestHosts.rubylight(), false));
            assertNull("an offline PC cannot be asked to forget this device", row(screen, "unpair"));
            screen.showComputer(pc("unpair-pc", TestHosts.rubylight(), true));
            assertNotNull(row(screen, "unpair"));
            ComputerDetails unpaired = pc("unpair-pc", TestHosts.rubylight(), true);
            unpaired.pairState = PairingManager.PairState.NOT_PAIRED;
            screen.showComputer(unpaired);
            assertNull(row(screen, "unpair"));
            assertNotNull("Rename and Remove stay", row(screen, "rename"));
            assertNotNull(row(screen, "remove"));
        } finally {
            controller.pause().stop().destroy();
        }
    }
}
