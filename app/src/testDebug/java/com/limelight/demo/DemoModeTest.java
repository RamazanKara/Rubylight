package com.limelight.demo;

import android.app.Application;
import android.content.ComponentName;
import android.content.Context;
import android.content.Intent;
import android.content.ServiceConnection;
import android.content.SharedPreferences;
import android.os.Build;
import android.os.Bundle;
import android.os.Looper;
import android.preference.Preference;
import android.preference.PreferenceFragment;
import android.preference.PreferenceManager;
import android.widget.LinearLayout;
import android.widget.TextView;

import com.google.android.material.appbar.MaterialToolbar;
import com.google.android.material.button.MaterialButton;

import com.limelight.AppView;
import com.limelight.R;
import com.limelight.PcViewLifecycleTest;
import com.limelight.computers.ComputerManagerService;
import com.limelight.computers.HostCapabilityCache;
import com.limelight.grid.AppGridAdapter;
import com.limelight.grid.PcGridAdapter;
import com.limelight.grid.assets.CachedAppAssetLoader;
import com.limelight.grid.assets.DiskAssetLoader;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.http.NvHTTP;
import com.limelight.nvstream.http.PairingManager.PairState;
import com.limelight.pin.PinGate;
import com.limelight.pin.PinLockActivity;
import com.limelight.pin.PinLockSettingsActivity;
import com.limelight.pin.PinStore;
import com.limelight.preferences.GlPreferences;
import com.limelight.preferences.StreamSettings;
import com.limelight.ui.StreamMenu;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowLooper;
import org.robolectric.util.ReflectionHelpers;

import java.io.StringReader;
import java.util.ArrayList;
import java.io.File;
import java.util.List;
import java.util.concurrent.TimeUnit;

import static org.junit.Assert.*;
import static org.robolectric.Shadows.shadowOf;

@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class, shadows = PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class DemoModeTest {
    @Test public void preparationKeepsNormalPreferencesAndArtworkSeparate() throws Exception {
        Context normal = RuntimeEnvironment.getApplication();
        Context demo = new DemoContext(normal);
        SharedPreferences normalPrefs = PreferenceManager.getDefaultSharedPreferences(normal);
        normalPrefs.edit().putInt("seekbar_bitrate_kbps", 9000).commit();
        DemoFixtures.prepare(demo);
        assertEquals(9000, normalPrefs.getInt("seekbar_bitrate_kbps", 0));
        assertEquals(40000, PreferenceManager.getDefaultSharedPreferences(demo).getInt("seekbar_bitrate_kbps", 0));
        assertNotEquals(normal.getCacheDir(), demo.getCacheDir());
        ComputerDetails pc = DemoFixtures.computers().get(0);
        List<NvApp> apps = NvHTTP.getAppListByReader(new StringReader(pc.rawAppList));
        assertEquals(10, apps.size());
        for (NvApp app : apps) {
            assertTrue(new DiskAssetLoader(demo).checkCacheExists(new CachedAppAssetLoader.LoaderTuple(pc, app)));
            assertFalse(new DiskAssetLoader(normal).checkCacheExists(new CachedAppAssetLoader.LoaderTuple(pc, app)));
        }
    }

    @Test public void binderSuppliesFixturesWithoutDiscoveryOrPersistentShortcuts() {
        var service = Robolectric.buildService(DemoComputerManagerService.class).create();
        var binder = (ComputerManagerService.ComputerManagerBinder) service.get().onBind(new Intent());
        List<ComputerDetails> computers = new ArrayList<>();
        assertTrue(binder.waitForReady());
        assertFalse(binder.isPersistent());
        binder.startPolling(computers::add);
        assertEquals(3, computers.size());
        assertEquals(PairState.PAIRED, computers.get(0).pairState);
        assertEquals(ComputerDetails.State.ONLINE, computers.get(1).state);
        assertEquals(ComputerDetails.State.OFFLINE, computers.get(2).state);
        assertNotNull(computers.get(2).macAddress);
        for (ComputerDetails computer : computers) assertNull(computer.activeAddress);
        binder.createAppListPoller(computers.get(0)).start();
        binder.stopPolling();
        binder.waitForPollingStopped();
        assertNull(shadowOf(RuntimeEnvironment.getApplication()).getNextStartedService());
        service.destroy();
    }

    @Test public void pcSettingsShowLivingRoomPcAsAnOnlineRubylightHost() throws Exception {
        Context normal = RuntimeEnvironment.getApplication();
        DemoContext demo = new DemoContext(normal);
        DemoFixtures.prepare(demo);
        assertEquals(HostCapabilities.Family.RUBYLIGHT, HostCapabilityCache.load(demo, DemoFixtures.PC_UUID).family());
        assertFalse(HostCapabilityCache.load(normal, DemoFixtures.PC_UUID).isKnown());

        var service = Robolectric.buildService(DemoComputerManagerService.class).create();
        var plain = (ComputerManagerService.ComputerManagerBinder) service.get().onBind(new Intent());
        var settings = (ComputerManagerService.ComputerManagerBinder) service.get()
                .onBind(new Intent(DemoComputerManagerService.ACTION_PC_SETTINGS));
        // Only the PC settings screen sees an address, and only for the online PC.
        assertNull(plain.getComputer(DemoFixtures.PC_UUID).activeAddress);
        ComputerDetails pc = settings.getComputer(DemoFixtures.PC_UUID);
        assertEquals(DemoComputerManagerService.SETTINGS_ADDRESS, pc.activeAddress.address);
        assertNull(settings.getComputer(DemoFixtures.computers().get(2).uuid).activeAddress);
        for (HostCapabilities.Feature feature : new HostCapabilities.Feature[] {HostCapabilities.Feature.HOST_DISPLAY_MODES,
                HostCapabilities.Feature.VIRTUAL_DISPLAY, HostCapabilities.Feature.RUNTIME_BITRATE, HostCapabilities.Feature.WEB_UI}) {
            assertTrue(feature.name(), pc.caps.has(feature));
        }
        service.destroy();

        Intent intent = new Intent(demo, DemoHostStreamSettings.class)
                .putExtra("host_uuid", DemoFixtures.PC_UUID).putExtra("host_name", "Living-room PC");
        var page = Robolectric.buildActivity(DemoHostStreamSettings.class, intent).setup();
        try {
            MaterialToolbar toolbar = page.get().findViewById(R.id.host_profile_toolbar);
            assertEquals("Living-room PC", toolbar.getTitle().toString());
            assertEquals("Rubylight 2.2.21", toolbar.getSubtitle().toString());
            PreferenceFragment fragment = (PreferenceFragment) page.get().getFragmentManager()
                    .findFragmentById(R.id.host_profile_settings);
            List<String> groups = new ArrayList<>();
            for (int i = 0; i < fragment.getPreferenceScreen().getPreferenceCount(); i++) {
                Preference preference = fragment.getPreferenceScreen().getPreference(i);
                if (preference instanceof android.preference.PreferenceCategory) groups.add(preference.getTitle().toString());
            }
            assertEquals(List.of(demo.getString(R.string.settings_section_video), demo.getString(R.string.pc_settings_display),
                    demo.getString(R.string.pc_settings_connection), demo.getString(R.string.pc_settings_host)), groups);
            for (String key : new String[] {"host_display", "virtual", "scale", "web_ui"}) {
                assertNotNull(key, fragment.findPreference(key));
            }
        } finally { page.pause().stop().destroy(); }
    }

    @Test public void realPcAndAppAdaptersReceiveDemoRowsAndRunningState() throws Exception {
        DemoContext context = new DemoContext(RuntimeEnvironment.getApplication());
        DemoFixtures.prepare(context);
        GlPreferences gl = GlPreferences.readPreferences(context);
        gl.savedFingerprint = Build.FINGERPRINT;
        gl.glRenderer = "Test renderer";
        gl.writePreferences();
        var service = Robolectric.buildService(DemoComputerManagerService.class).create();
        var binder = service.get().onBind(new Intent());
        var pc = Robolectric.buildActivity(DemoPcView.class).setup();
        try {
            ServiceConnection connection = ReflectionHelpers.getField(pc.get(), "serviceConnection");
            connection.onServiceConnected(new ComponentName(context, DemoComputerManagerService.class), binder);
            Thread worker = ReflectionHelpers.getField(pc.get(), "serviceWaitThread");
            worker.join(3000);
            shadowOf(Looper.getMainLooper()).idle();
            PcGridAdapter adapter = ReflectionHelpers.getField(pc.get(), "pcGridAdapter");
            assertEquals(3, adapter.getCount());
        } finally { pc.pause().stop().destroy(); }
        Intent intent = new Intent(context, DemoAppView.class)
                .putExtra(AppView.UUID_EXTRA, DemoFixtures.PC_UUID).putExtra(AppView.NAME_EXTRA, "Living-room PC");
        var library = Robolectric.buildActivity(DemoAppView.class, intent).setup();
        try {
            ServiceConnection connection = ReflectionHelpers.getField(library.get(), "serviceConnection");
            connection.onServiceConnected(new ComponentName(context, DemoComputerManagerService.class), binder);
            Thread worker = ReflectionHelpers.getField(library.get(), "serviceWaitThread");
            worker.join(3000);
            shadowOf(Looper.getMainLooper()).idle();
            AppGridAdapter adapter = ReflectionHelpers.getField(library.get(), "appGridAdapter");
            assertEquals(10, adapter.getCount());
            int running = 0;
            for (int i = 0; i < adapter.getCount(); i++) {
                AppView.AppObject app = (AppView.AppObject) adapter.getItem(i);
                if (app.isRunning) {
                    running++;
                    assertEquals("Racing Game", app.app.getAppName());
                }
            }
            assertEquals(1, running);
        } finally {
            library.pause().stop().destroy();
            service.destroy();
        }
    }

    @Test public void metricsUseTheProductionListenerAndChangeBetweenSamples() {
        Context context = RuntimeEnvironment.getApplication();
        String[] samples = new String[2];
        DemoMetrics.sample = "av1";
        DemoMetrics.update(context, (video, network, decode, statistics, compact) -> {
            assertTrue(video.contains("AV1"));
            assertTrue(video.contains("FSR 1"));
            assertTrue(statistics.toString().contains("119.9 FPS"));
            assertTrue(compact.toString().startsWith("● 119.9 FPS · 8 ms RTT · 0.1% loss"));
            samples[0] = decode;
        }, 0);
        DemoMetrics.update(context, (video, network, decode, statistics, compact) -> samples[1] = decode, 2);
        assertNotEquals(samples[0], samples[1]);
        // The overlay describes the sample that plays: the 8-bit fallback is labelled H.264, not AV1 HDR.
        DemoMetrics.sample = "h264";
        DemoMetrics.update(context, (video, network, decode, statistics, compact) -> {
            assertTrue(video.contains("H.264 · 8-bit SDR"));
            assertFalse(video.contains("AV1"));
            assertTrue(statistics.toString().contains("H.264 · SDR"));
            assertFalse(statistics.toString().contains("HDR"));
        }, 0);
        DemoMetrics.sample = "av1";
    }

    @Test public void streamChromeRestoresOverlayModeAndReleasesControls() {
        Intent intent = new Intent(RuntimeEnvironment.getApplication(), DemoGameActivity.class)
                .putExtra("state", "advanced");
        var first = Robolectric.buildActivity(DemoGameActivity.class, intent).create();
        TextView overlay = first.get().findViewById(R.id.performanceOverlay);
        // Robolectric has no hardware AV1 or HEVC decoder, so the demo plays the 8-bit sample and says so.
        assertTrue(overlay.getText().toString().contains("H.264"));
        assertTrue(overlay.getText().toString().contains("p95"));
        assertEquals(Integer.MAX_VALUE, overlay.getMaxLines());
        Bundle saved = new Bundle();
        first.saveInstanceState(saved).destroy();
        var restored = Robolectric.buildActivity(DemoGameActivity.class, intent).create(saved);
        try {
            overlay = restored.get().findViewById(R.id.performanceOverlay);
            assertEquals(Integer.MAX_VALUE, overlay.getMaxLines());
            assertTrue(overlay.getText().toString().contains("p95"));
            // Debug, then the compact line, then the six statistics lines.
            overlay.performLongClick();
            assertTrue(overlay.getText().toString().startsWith("● 119.9 FPS"));
            assertFalse(overlay.getText().toString().contains("\n"));
            overlay.performLongClick();
            assertEquals(Integer.MAX_VALUE, overlay.getMaxLines());
            // Six stream lines, then the host's CPU and GPU, as a Rubylight host's hardware readings show them.
            String[] lines = overlay.getText().toString().split("\n");
            assertEquals(8, lines.length);
            assertTrue(overlay.getText().toString().contains("119.9 FPS"));
            assertTrue(lines[6], lines[6].startsWith("Host CPU: "));
            assertTrue(lines[7], lines[7].startsWith("Host GPU 0: "));
            assertFalse(overlay.getText().toString().contains("p95"));
        } finally { restored.destroy(); }
    }

    @Test public void settingsShowTheDriverAndUsbControllerRowsOnAnyDevice() {
        var controller = Robolectric.buildActivity(DemoStreamSettings.class).setup();
        try {
            StreamSettings screen = controller.get();
            // Robolectric's device has neither an Adreno GPU nor USB host support: the release checks hide both rows.
            for (String[] row : new String[][] {{"input", "category_usb_controllers"}, {"advanced", "gpu_driver"}}) {
                ReflectionHelpers.callInstanceMethod(screen, "showSection", ReflectionHelpers.ClassParameter.from(String.class, row[0]));
                ShadowLooper.idleMainLooper(150, TimeUnit.MILLISECONDS);
                screen.getFragmentManager().executePendingTransactions();
                PreferenceFragment fragment = (PreferenceFragment) screen.getFragmentManager().findFragmentById(R.id.stream_settings);
                assertNotNull(row[1], fragment.findPreference(row[1]));
            }
            PreferenceFragment advanced = (PreferenceFragment) screen.getFragmentManager().findFragmentById(R.id.stream_settings);
            Preference driver = advanced.findPreference("gpu_driver");
            driver.getOnPreferenceClickListener().onPreferenceClick(driver);
            assertEquals(DemoGpuDriverSettings.class.getName(), shadowOf(screen).getNextStartedActivity().getComponent().getClassName());
        } finally { controller.pause().stop().destroy(); }
    }

    @Test public void settingsOpenThePinLockPage() {
        var controller = Robolectric.buildActivity(DemoStreamSettings.class).setup();
        try {
            StreamSettings screen = controller.get();
            ReflectionHelpers.callInstanceMethod(screen, "showSection", ReflectionHelpers.ClassParameter.from(String.class, "app"));
            ShadowLooper.idleMainLooper(150, TimeUnit.MILLISECONDS);
            screen.getFragmentManager().executePendingTransactions();
            PreferenceFragment app = (PreferenceFragment) screen.getFragmentManager().findFragmentById(R.id.stream_settings);
            Preference row = app.findPreference("pin_lock");
            row.getOnPreferenceClickListener().onPreferenceClick(row);
            Intent next = shadowOf(screen).getNextStartedActivity();
            assertNotNull("the demo dropped the PIN lock page", next);
            assertEquals(PinLockSettingsActivity.class.getName(), next.getComponent().getClassName());
        } finally { controller.pause().stop().destroy(); }
    }

    @Test public void launcherPinStatesSetAndClearTheDemoPin() {
        Context context = RuntimeEnvironment.getApplication();
        PinStore store = new PinStore(context);
        Robolectric.buildActivity(DemoLauncherActivity.class,
                new Intent(context, DemoLauncherActivity.class).putExtra("state", "pin-lock")).setup();
        assertTrue(store.hasPin());
        assertEquals(PinStore.Mode.APP, store.mode());
        assertEquals(0, store.graceMinutes());
        assertTrue(PinGate.needsPinForApp(context));

        var set = Robolectric.buildActivity(DemoLauncherActivity.class,
                new Intent(context, DemoLauncherActivity.class).putExtra("state", "pin-set")).setup();
        assertFalse(store.hasPin());
        assertEquals(PinLockActivity.class.getName(), shadowOf(set.get()).getNextStartedActivity().getComponent().getClassName());

        Robolectric.buildActivity(DemoLauncherActivity.class,
                new Intent(context, DemoLauncherActivity.class).putExtra("state", "pin-off")).setup();
        assertFalse(store.hasPin());
        assertEquals(PinStore.Mode.OFF, store.mode());
        PinGate.resetForTests();
    }

    @Test public void driverScreenReadsAsOnASupportedPhoneAndImportsNothing() {
        var controller = Robolectric.buildActivity(DemoGpuDriverSettings.class).setup();
        try {
            LinearLayout content = controller.get().findViewById(R.id.gpu_driver_content);
            assertEquals(controller.get().getString(R.string.gpu_driver_explanation),
                    ((TextView) content.getChildAt(0)).getText().toString());
            MaterialButton importButton = null;
            for (int i = 0; i < content.getChildCount(); i++) {
                if (content.getChildAt(i) instanceof MaterialButton && ((MaterialButton) content.getChildAt(i)).getText().toString()
                        .equals(controller.get().getString(R.string.gpu_driver_import))) importButton = (MaterialButton) content.getChildAt(i);
            }
            assertNotNull(importButton);
            assertTrue(importButton.isEnabled());
            importButton.performClick();
            assertNull(shadowOf(controller.get()).getNextStartedActivityForResult());
            // The demo lists its own driver folder, never the user's drivers.
            Context normal = RuntimeEnvironment.getApplication();
            assertEquals(new File(normal.getNoBackupFilesDir(), "demo"), new DemoContext(normal).getNoBackupFilesDir());
        } finally { controller.pause().stop().destroy(); }
    }

    @Test public void streamMenuOffersTheMicrophoneAndTogglesItWithoutRecording() {
        Intent intent = new Intent(RuntimeEnvironment.getApplication(), DemoGameActivity.class).putExtra("state", "compact");
        var stream = Robolectric.buildActivity(DemoGameActivity.class, intent).create();
        try {
            stream.get().onBackPressed();
            StreamMenu menu = ReflectionHelpers.getField(stream.get(), "menu");
            MaterialButton microphone = menu.row(StreamMenu.Item.MICROPHONE);
            assertNotNull(microphone);
            assertTrue(microphone.isChecked());
            microphone.performClick();
            assertFalse(menu.row(StreamMenu.Item.MICROPHONE).isChecked());
            assertTrue(menu.sheet().isShowing());
        } finally { stream.destroy(); }
    }
}
