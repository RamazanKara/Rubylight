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
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.PairingManager;
import com.limelight.nvstream.http.TestHosts;
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
}
