package com.limelight.binding.video;

import android.app.Application;
import android.content.Context;
import android.content.Intent;
import android.content.ServiceConnection;
import android.content.SharedPreferences;
import android.os.Build;
import android.os.Looper;
import android.preference.PreferenceManager;

import com.limelight.PcViewLifecycleTest;
import com.limelight.console.ConsoleHomeActivity;
import com.limelight.console.GameDetailActivity;
import com.limelight.preferences.GlPreferences;
import com.limelight.preferences.HdrFormatChoice;
import com.limelight.preferences.HostStreamProfile;
import com.limelight.preferences.PreferenceConfiguration;

import org.junit.After;
import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowDialog;
import org.robolectric.shadows.ShadowLooper;

import static org.junit.Assert.*;
import static org.robolectric.Shadows.shadowOf;

/**
 * Regression tests for AND-09: 3-strike decoder crash safety reset reachability from Console Home
 * and Game Detail, unclean stream-exit tombstone promotion across process restarts, and full
 * reset coverage across modern video and low-latency settings.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 34, application = Application.class,
        shadows = PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class CrashSafetyResetTest {
    public static class TestHome extends ConsoleHomeActivity {
        @Override public boolean bindService(Intent intent, ServiceConnection connection, int flags) { return false; }
        @Override protected void checkForUpdates() {}
    }

    public static class TestDetail extends GameDetailActivity {
        @Override public boolean bindService(Intent intent, ServiceConnection connection, int flags) { return false; }
    }

    private Application app;
    private SharedPreferences tombstone;
    private SharedPreferences prefs;

    @Before
    public void setUp() {
        app = RuntimeEnvironment.getApplication();
        tombstone = app.getSharedPreferences("DecoderTombstone", 0);
        tombstone.edit().clear().commit();
        prefs = PreferenceManager.getDefaultSharedPreferences(app);
        prefs.edit().clear().commit();
        GlPreferences gl = GlPreferences.readPreferences(app);
        gl.savedFingerprint = Build.FINGERPRINT;
        gl.glRenderer = "Test renderer";
        gl.writePreferences();
        ShadowDialog.reset();
    }

    @After
    public void tearDown() {
        tombstone.edit().clear().commit();
        prefs.edit().clear().commit();
        ShadowDialog.reset();
    }

    @Test
    public void uncleanStreamExitPromotesOnlyAcrossProcessRestart() {
        CrashAccounting.markStreamActive(tombstone, "pc-crash", 1001);
        assertFalse("same PID (e.g. active PiP stream) must not be promoted as a crash",
                CrashAccounting.promoteUncleanStreamExit(tombstone, 1001));
        assertEquals(0, tombstone.getInt("CrashCount", 0));

        assertTrue("different PID after process death must promote into CrashCount",
                CrashAccounting.promoteUncleanStreamExit(tombstone, 2002));
        assertEquals(1, tombstone.getInt("CrashCount", 0));
        assertEquals("LastNotifiedCrashCount must stay behind so dialog shows",
                0, tombstone.getInt("LastNotifiedCrashCount", 0));
        assertEquals("pc-crash", tombstone.getString("CrashHostUuid", null));

        // Second check in the same relaunched process is a no-op.
        assertFalse(CrashAccounting.promoteUncleanStreamExit(tombstone, 2002));
        assertEquals(1, tombstone.getInt("CrashCount", 0));

        // Clean stop clears StreamActive so no false crash is recorded.
        CrashAccounting.markStreamActive(tombstone, "pc-crash", 2002);
        CrashAccounting.clearStreamActive(tombstone);
        assertFalse(CrashAccounting.promoteUncleanStreamExit(tombstone, 3003));
        assertEquals(1, tombstone.getInt("CrashCount", 0));
    }

    @Test
    public void resetStreamingSettingsClearsModernVideoAndLowLatencyOverrides() {
        prefs.edit()
                .putString("video_format", "forceav1")
                .putBoolean("resolution_follows_screen", true)
                .putBoolean("checkbox_vulkan_render", true)
                .putBoolean("checkbox_gpu_max_clocks", true)
                .putBoolean("checkbox_pyrowave_preacquire", true)
                .putString("upscaling_mode", "fsr1")
                .putInt("upscaling_sharpness", 90)
                .putBoolean("checkbox_frame_warp", true)
                .putBoolean("checkbox_yuv444", true)
                .putBoolean("experimental_packed_444", true)
                .putString(HdrFormatChoice.PREF_KEY, HdrFormatChoice.HDR10.value)
                .putBoolean("debug_hdr_hlg_only", true)
                .putBoolean("checkbox_texture_view", true)
                .putBoolean("checkbox_vrr", true)
                .putBoolean("checkbox_real_vrr", false)
                .putBoolean("checkbox_host_frame_times", true)
                .putBoolean("checkbox_arr_fixed_timing", true)
                .putString("frame_pacing", "smoothness")
                .putBoolean("checkbox_experimental_chipset_tuning", true)
                .putString("list_hevc_low_latency_mode", "1")
                .putBoolean("checkbox_disable_hevc_low_latency", true)
                .putBoolean("checkbox_covering_decoder_size", true)
                .putBoolean("checkbox_codec_low_latency", true)
                .putBoolean("checkbox_vendor_low_latency", true)
                .putBoolean("checkbox_codec_performance", true)
                .putBoolean("checkbox_phone_performance_hints", true)
                .commit();
        tombstone.edit().putString("CrashHostUuid", "crash-host").commit();
        app.getSharedPreferences("HostStreamProfiles", Context.MODE_PRIVATE).edit()
                .putString("crash-host", "v2;codec=FORCE_AV1;vrr=true;upscaling=fsr1;sharpness=80")
                .commit();

        PreferenceConfiguration.resetStreamingSettings(app);

        PreferenceConfiguration after = PreferenceConfiguration.readPreferences(app);
        assertEquals(PreferenceConfiguration.FormatOption.AUTO, after.videoFormat);
        assertFalse(after.vulkanRender);
        assertFalse(after.gpuMaxClocks);
        assertFalse(after.pyroWavePreacquire);
        assertEquals(UpscalingPolicy.Mode.OFF, after.upscalingMode);
        assertFalse(after.frameWarp);
        assertFalse(after.enableYuv444);
        assertFalse(after.experimentalPacked444);
        assertEquals(HdrFormatChoice.AUTO, after.hdrFormat);
        assertFalse(after.debugHdrHlgOnly);
        assertFalse(after.useTextureView);
        assertFalse(after.vrr);
        assertFalse(after.gridHostFrameTimes);
        assertFalse(after.arrFixedTiming);
        assertFalse(after.experimentalChipsetTuning);
        assertEquals(PreferenceConfiguration.HEVC_LOW_LATENCY_AUTO, after.hevcLowLatencyMode);
        assertFalse(after.coveringDecoderSize);
        assertFalse("codec low-latency override must be disabled after crash reset", after.codecLowLatency);
        assertFalse("vendor low-latency override must be disabled after crash reset", after.vendorLowLatency);
        assertFalse("codec performance override must be disabled after crash reset", after.codecPerformance);
        assertFalse("phone performance hints must be disabled after crash reset", after.phonePerformanceHints);
        assertNull("crashed host profile must be cleared",
                app.getSharedPreferences("HostStreamProfiles", Context.MODE_PRIVATE).getString("crash-host", null));
    }

    @Test
    public void consoleHomeResumeTriggersThreeStrikeResetAndDialog() {
        prefs.edit().putBoolean("checkbox_vulkan_render", true).putBoolean("checkbox_frame_warp", true).commit();
        // Two earlier crashes + one unclean native crash from a dead process = 3 strikes.
        tombstone.edit().putInt("CrashCount", 2).putInt("LastNotifiedCrashCount", 2).commit();
        CrashAccounting.markStreamActive(tombstone, "pc-home", android.os.Process.myPid() + 77);

        ActivityController<TestHome> controller = Robolectric.buildActivity(TestHome.class).create().start().resume();
        try {
            ShadowLooper.idleMainLooper();
            assertNotNull("ConsoleHomeActivity must show decoder reset dialog on 3rd crash",
                    ShadowDialog.getLatestDialog());
            assertFalse("Vulkan must be reset on ConsoleHome resume", prefs.contains("checkbox_vulkan_render"));
            assertFalse("Frame Warp must be reset on ConsoleHome resume", prefs.contains("checkbox_frame_warp"));
            assertEquals(3, tombstone.getInt("CrashCount", 0));
        } finally {
            controller.pause().stop().destroy();
            shadowOf(Looper.getMainLooper()).idle();
        }
    }

    @Test
    public void gameDetailResumeTriggersThreeStrikeResetAndDialog() {
        prefs.edit().putBoolean("checkbox_vulkan_render", true).putString("upscaling_mode", "sgsr1").commit();
        CrashAccounting.recordGiveUp(tombstone, "pc-detail");
        CrashAccounting.recordGiveUp(tombstone, "pc-detail");
        CrashAccounting.recordGiveUp(tombstone, "pc-detail");

        Intent intent = new Intent(app, TestDetail.class)
                .putExtra(GameDetailActivity.EXTRA_HOST_UUID, "pc-detail")
                .putExtra(GameDetailActivity.EXTRA_APP_ID, 1)
                .putExtra(GameDetailActivity.EXTRA_TITLE, "Game")
                .putExtra(GameDetailActivity.EXTRA_HOST_NAME, "PC");
        ActivityController<TestDetail> controller = Robolectric.buildActivity(TestDetail.class, intent).create().start().resume();
        try {
            ShadowLooper.idleMainLooper();
            assertNotNull("GameDetailActivity must show decoder reset dialog on 3rd strike",
                    ShadowDialog.getLatestDialog());
            assertFalse("Vulkan must be reset on GameDetail resume", prefs.contains("checkbox_vulkan_render"));
            assertFalse("Upscaling must be reset on GameDetail resume", prefs.contains("upscaling_mode"));
        } finally {
            controller.pause().stop().destroy();
            shadowOf(Looper.getMainLooper()).idle();
        }
    }
}
