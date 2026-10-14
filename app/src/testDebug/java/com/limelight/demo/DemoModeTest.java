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
import android.preference.PreferenceManager;
import android.widget.TextView;

import com.limelight.AppView;
import com.limelight.R;
import com.limelight.PcViewLifecycleTest;
import com.limelight.computers.ComputerManagerService;
import com.limelight.grid.AppGridAdapter;
import com.limelight.grid.PcGridAdapter;
import com.limelight.grid.assets.CachedAppAssetLoader;
import com.limelight.grid.assets.DiskAssetLoader;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.http.NvHTTP;
import com.limelight.nvstream.http.PairingManager.PairState;
import com.limelight.preferences.GlPreferences;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;

import java.io.StringReader;
import java.util.ArrayList;
import java.util.List;

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
        DemoMetrics.update(context, (video, network, decode, statistics, compact) -> {
            assertTrue(video.contains("AV1"));
            assertTrue(video.contains("FSR 1"));
            assertTrue(statistics.toString().contains("119.9 FPS"));
            assertTrue(compact.toString().startsWith("● 119.9 FPS · 8 ms RTT · 0.1% loss"));
            samples[0] = decode;
        }, 0);
        DemoMetrics.update(context, (video, network, decode, statistics, compact) -> samples[1] = decode, 2);
        assertNotEquals(samples[0], samples[1]);
    }

    @Test public void streamChromeRestoresOverlayModeAndReleasesControls() {
        Intent intent = new Intent(RuntimeEnvironment.getApplication(), DemoGameActivity.class)
                .putExtra("state", "advanced");
        var first = Robolectric.buildActivity(DemoGameActivity.class, intent).create();
        TextView overlay = first.get().findViewById(R.id.performanceOverlay);
        assertTrue(overlay.getText().toString().contains("AV1"));
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
            assertEquals(6, overlay.getText().toString().split("\n").length);
            assertTrue(overlay.getText().toString().contains("119.9 FPS"));
            assertFalse(overlay.getText().toString().contains("p95"));
        } finally { restored.destroy(); }
    }
}
