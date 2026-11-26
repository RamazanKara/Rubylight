package com.limelight;

import android.app.Application;
import android.content.pm.PackageManager;
import android.graphics.SurfaceTexture;
import android.media.MediaFormat;
import android.os.Looper;
import android.view.Display;
import android.view.Surface;
import android.view.SurfaceHolder;
import android.view.View;
import android.view.WindowManager;

import com.limelight.binding.video.MediaCodecDecoderRenderer;
import com.limelight.binding.video.MediaCodecHelper;
import com.limelight.binding.video.RefreshSwitchMonitorTest;
import com.limelight.preferences.PreferenceConfiguration;
import com.limelight.ui.StreamView;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.Shadows;
import org.robolectric.annotation.Config;
import org.robolectric.shadow.api.Shadow;
import org.robolectric.shadows.ShadowSurfaceView;
import org.robolectric.shadows.ShadowSystemClock;
import org.robolectric.util.ReflectionHelpers;

import java.time.Duration;

import static org.junit.Assert.*;

/**
 * Window-level display mode pinning, Android 15 frame-rate power-savings opt-out and video view
 * requested frame rate across phones, Android TV, seamless content-rate steps, Android 16 ARR and
 * foldable panel switches.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 35, application = Application.class, shadows = GameFrameRateTest.ReconfigureBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class GameWindowRefreshModeTest {
    @Test
    @Config(sdk = 35, shadows = {GameFrameRateTest.FrameRateSurface.class, RefreshSwitchMonitorTest.FakePanel.class})
    public void android15PhonePinsWindowModeAndDisablesPowerSavingsThrottleEvenWhenAlreadyAt120Hz() {
        // Issue #168: realme RMX3867 (API 35, 60/120.00001 Hz) starts at mode 2 (120 Hz) from touch
        // boost; leaving preferredDisplayModeId=0 and power-savings balanced=true let the panel drop
        // back to 60 Hz 1 s after UI views went idle.
        Display.Mode m60 = RefreshSwitchMonitorTest.mode(1, 60f, 120.00001f);
        Display.Mode m120 = RefreshSwitchMonitorTest.mode(2, 120.00001f, 60f);
        try {
            for (Display.Mode initial : new Display.Mode[] {m120, m60}) {
                RefreshSwitchMonitorTest.FakePanel.set(initial, m60, m120);
                Game activity = Robolectric.buildActivity(Game.class).get();
                PreferenceConfiguration prefs = new PreferenceConfiguration();
                prefs.width = 2400;
                prefs.height = 1080;
                prefs.fps = 120;
                prefs.framePacing = PreferenceConfiguration.FRAME_PACING_MIN_LATENCY;
                StreamView view = new StreamView(activity);
                view.initializeSurface(false);
                ReflectionHelpers.setField(activity, "prefConfig", prefs);
                ReflectionHelpers.setField(activity, "streamView", view);
                ReflectionHelpers.setField(activity, "renderView", view);

                ReflectionHelpers.callInstanceMethod(activity.videoOutput, "prepareDisplayForRendering");
                WindowManager.LayoutParams attrs = activity.getWindow().getAttributes();
                assertEquals("initial mode " + initial.getModeId(), 2, attrs.preferredDisplayModeId);
                assertEquals("initial mode " + initial.getModeId(), 120.00001f, attrs.preferredRefreshRate, 0f);
                assertFalse("initial mode " + initial.getModeId(), attrs.isFrameRatePowerSavingsBalanced());

                SurfaceTexture texture = new SurfaceTexture(0);
                Surface surface = new Surface(texture);
                try {
                    GameFrameRateTest.FrameRateSurface vote = Shadow.extract(surface);
                    vote(activity, surface);
                    assertEquals(120.00001f, vote.frameRate, 0f);
                    assertEquals(Surface.FRAME_RATE_COMPATIBILITY_FIXED_SOURCE, vote.compatibility);
                    assertEquals(Surface.CHANGE_FRAME_RATE_ALWAYS, vote.strategy);
                    assertEquals(2, activity.getWindow().getAttributes().preferredDisplayModeId);
                    assertEquals(120.00001f, activity.getWindow().getAttributes().preferredRefreshRate, 0f);
                    View surfaceChild = view.getChildAt(0);
                    assertEquals(120.00001f, surfaceChild.getRequestedFrameRate(), 0f);
                    assertEquals(120.00001f, view.getRequestedFrameRate(), 0f);
                    assertEquals(120.00001f, activity.getWindow().getDecorView().getRequestedFrameRate(), 0f);
                } finally {
                    surface.release();
                    texture.release();
                }
            }
        } finally {
            RefreshSwitchMonitorTest.FakePanel.clear();
        }
    }

    @Test
    @Config(sdk = 35, shadows = {GameFrameRateTest.FrameRateSurface.class, RefreshSwitchMonitorTest.FakePanel.class})
    public void seamlessContentRateVotesUpdateThePinnedWindowModeWithoutBlankingNonSeamlessPanels() {
        Display.Mode m60 = RefreshSwitchMonitorTest.mode(1, 60f, 90f, 120f);
        Display.Mode m90 = RefreshSwitchMonitorTest.mode(2, 90f, 60f, 120f);
        Display.Mode m120 = RefreshSwitchMonitorTest.mode(3, 120f, 60f, 90f);
        RefreshSwitchMonitorTest.FakePanel.set(m120, m60, m90, m120);
        Game activity = Robolectric.buildActivity(Game.class).get();
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.videoFormat = PreferenceConfiguration.FormatOption.AUTO;
        prefs.width = 2400;
        prefs.height = 1080;
        prefs.fps = 120;
        prefs.vrr = true;
        ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
        ContentRateRenderer renderer = new ContentRateRenderer(activity, prefs);
        StreamView view = new StreamView(activity);
        view.initializeSurface(false);
        ReflectionHelpers.setField(activity, "prefConfig", prefs);
        ReflectionHelpers.setField(activity, "decoderRenderer", renderer);
        ReflectionHelpers.setField(activity, "panelMaxRefreshRate", 120f);
        ReflectionHelpers.setField(activity, "streamView", view);
        ReflectionHelpers.setField(activity, "renderView", view);
        ReflectionHelpers.callInstanceMethod(activity.videoOutput, "prepareDisplayForRendering");
        assertEquals(3, activity.getWindow().getAttributes().preferredDisplayModeId);

        SurfaceTexture texture = new SurfaceTexture(0);
        Surface surface = new Surface(texture);
        try {
            ReflectionHelpers.setField(activity, "videoSurface", surface);
            ReflectionHelpers.setField(activity, "surfaceCreated", true);
            vote(activity, surface);
            assertEquals(3, activity.getWindow().getAttributes().preferredDisplayModeId);

            // VRR without ARR steps down seamlessly to 90 Hz: the window pin moves with it.
            renderer.contentMilliHz = 90_000;
            ReflectionHelpers.callInstanceMethod(activity.videoOutput, "voteContentRate");
            assertEquals(2, activity.getWindow().getAttributes().preferredDisplayModeId);
            assertEquals(90f, activity.getWindow().getAttributes().preferredRefreshRate, 0f);
            assertEquals(90f, view.getChildAt(0).getRequestedFrameRate(), 0f);

            // Withdrawing returns seamlessly to 120 Hz.
            RefreshSwitchMonitorTest.FakePanel.set(m90, m60, m90, m120);
            ShadowSystemClock.advanceBy(Duration.ofSeconds(3));
            renderer.contentMilliHz = 0;
            ReflectionHelpers.callInstanceMethod(activity.videoOutput, "voteContentRate");
            assertEquals(3, activity.getWindow().getAttributes().preferredDisplayModeId);
            assertEquals(120f, activity.getWindow().getAttributes().preferredRefreshRate, 0f);
            assertEquals(120f, view.getChildAt(0).getRequestedFrameRate(), 0f);

            // Non-seamless panel: Lower refresh rate content vote must not force a blanking mode pin.
            Display.Mode ns60 = RefreshSwitchMonitorTest.mode(1, 60f);
            Display.Mode ns120 = RefreshSwitchMonitorTest.mode(2, 120f);
            RefreshSwitchMonitorTest.FakePanel.set(ns120, ns60, ns120);
            prefs.vrr = false;
            prefs.reduceRefreshRate = true;
            prefs.contentRateFrameRate = true;
            vote(activity, surface);
            assertEquals(2, activity.getWindow().getAttributes().preferredDisplayModeId);
            ShadowSystemClock.advanceBy(Duration.ofSeconds(3));
            renderer.contentMilliHz = 60_000;
            ReflectionHelpers.callInstanceMethod(activity.videoOutput, "voteContentRate");
            assertEquals(2, activity.getWindow().getAttributes().preferredDisplayModeId);
        } finally {
            RefreshSwitchMonitorTest.FakePanel.clear();
            surface.release();
            texture.release();
        }
    }

    @Test
    @Config(sdk = 35, shadows = {GameFrameRateTest.FrameRateSurface.class, RefreshSwitchMonitorTest.FakePanel.class})
    public void androidTvKeepsSetFrameRateOnlyForSameResolutionRefreshSwitches() {
        Shadows.shadowOf(RuntimeEnvironment.getApplication().getPackageManager())
                .setSystemFeature(PackageManager.FEATURE_LEANBACK, true);
        Display.Mode m60 = RefreshSwitchMonitorTest.mode(1, 60f);
        Display.Mode m120 = RefreshSwitchMonitorTest.mode(2, 120f);
        RefreshSwitchMonitorTest.FakePanel.set(m60, m60, m120);
        try {
            Game activity = Robolectric.buildActivity(Game.class).get();
            PreferenceConfiguration prefs = new PreferenceConfiguration();
            prefs.width = 1920;
            prefs.height = 1080;
            prefs.fps = 120;
            ReflectionHelpers.setField(activity, "prefConfig", prefs);
            giveStreamView(activity);

            ReflectionHelpers.callInstanceMethod(activity.videoOutput, "prepareDisplayForRendering");
            assertEquals(0, activity.getWindow().getAttributes().preferredDisplayModeId);
            assertEquals(0f, activity.getWindow().getAttributes().preferredRefreshRate, 0f);

            SurfaceTexture texture = new SurfaceTexture(0);
            Surface surface = new Surface(texture);
            try {
                vote(activity, surface);
                assertEquals(0, activity.getWindow().getAttributes().preferredDisplayModeId);
            } finally {
                surface.release();
                texture.release();
            }
        } finally {
            RefreshSwitchMonitorTest.FakePanel.clear();
        }
    }

    @Test
    @Config(sdk = 36, shadows = {GameFrameRateTest.FrameRateSurface.class, RefreshSwitchMonitorTest.FakePanel.class})
    public void android16ArrClearsPinnedWindowModeAndKeepsPowerSavingsBalanced() {
        Display.Mode m60 = RefreshSwitchMonitorTest.mode(1, 60f, 120f);
        Display.Mode m120 = RefreshSwitchMonitorTest.mode(2, 120f, 60f);
        RefreshSwitchMonitorTest.FakePanel.set(m120, m60, m120);
        Game activity = Robolectric.buildActivity(Game.class).get();
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.vrr = true;
        prefs.fps = 120;
        StreamView view = new StreamView(activity);
        view.initializeSurface(false);
        ReflectionHelpers.setField(activity, "prefConfig", prefs);
        ReflectionHelpers.setField(activity, "panelMaxRefreshRate", 120f);
        ReflectionHelpers.setField(activity, "streamView", view);
        ReflectionHelpers.setField(activity, "renderView", view);

        SurfaceTexture texture = new SurfaceTexture(0);
        Surface surface = new Surface(texture);
        try {
            // Non-ARR vote first pins the window mode and disables power-savings throttling.
            vote(activity, surface);
            assertEquals(2, activity.getWindow().getAttributes().preferredDisplayModeId);
            assertFalse(activity.getWindow().getAttributes().isFrameRatePowerSavingsBalanced());
            assertEquals(120f, view.getChildAt(0).getRequestedFrameRate(), 0f);

            // Switching to ARR clears the window pin and restores ARR power-savings balance.
            ReflectionHelpers.setField(activity, "useArr", true);
            vote(activity, surface);
            assertEquals(0, activity.getWindow().getAttributes().preferredDisplayModeId);
            assertEquals(0f, activity.getWindow().getAttributes().preferredRefreshRate, 0f);
            assertTrue(activity.getWindow().getAttributes().isFrameRatePowerSavingsBalanced());
            assertEquals(View.REQUESTED_FRAME_RATE_CATEGORY_DEFAULT,
                    view.getChildAt(0).getRequestedFrameRate(), 0f);
            assertEquals(View.REQUESTED_FRAME_RATE_CATEGORY_DEFAULT, view.getRequestedFrameRate(), 0f);
            assertEquals(View.REQUESTED_FRAME_RATE_CATEGORY_DEFAULT,
                    activity.getWindow().getDecorView().getRequestedFrameRate(), 0f);
        } finally {
            RefreshSwitchMonitorTest.FakePanel.clear();
            surface.release();
            texture.release();
        }
    }

    @Test
    @Config(sdk = 35, shadows = {GameFrameRateTest.FrameRateSurface.class, RefreshSwitchMonitorTest.FakePanel.class})
    public void panelChangeClearsStalePinnedModeIdWhenTheNewPanelLacksTheVotedRate() {
        Display.Mode inner60 = RefreshSwitchMonitorTest.mode(1, 60f, 120f);
        Display.Mode inner120 = RefreshSwitchMonitorTest.mode(2, 120f, 60f);
        RefreshSwitchMonitorTest.FakePanel.set(inner120, inner60, inner120);
        Game activity = Robolectric.buildActivity(Game.class).get();
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.width = 1920;
        prefs.height = 1080;
        prefs.fps = 120;
        ReflectionHelpers.setField(activity, "prefConfig", prefs);
        giveStreamView(activity);
        ReflectionHelpers.callInstanceMethod(activity.videoOutput, "prepareDisplayForRendering");
        SurfaceTexture texture = new SurfaceTexture(0);
        Surface surface = new Surface(texture);
        try {
            vote(activity, surface);
            assertEquals(2, activity.getWindow().getAttributes().preferredDisplayModeId);

            // Fold to a 60 Hz-only cover screen whose mode list has only id=1 (no mode 2).
            Display.Mode cover60 = RefreshSwitchMonitorTest.mode(1, 60f);
            RefreshSwitchMonitorTest.FakePanel.set(cover60, cover60);
            ReflectionHelpers.callInstanceMethod(activity.videoOutput, "onDisplayModesChanged");
            assertEquals(0, activity.getWindow().getAttributes().preferredDisplayModeId);
            assertEquals(0f, activity.getWindow().getAttributes().preferredRefreshRate, 0f);
        } finally {
            RefreshSwitchMonitorTest.FakePanel.clear();
            surface.release();
            texture.release();
        }
    }

    @Test
    @Config(sdk = 35, shadows = {GameFrameRateTest.FrameRateSurface.class, RefreshSwitchMonitorTest.FakePanel.class})
    public void pinnedPhoneRefreshDropReappliesPinThenStartsFallbackHeartbeatAndStopsOnTeardown() {
        // Issue #168: OriginOS AMOLED (e.g. iQOO 11s, 60/144 Hz without seamless alternatives) drops
        // to 60 Hz after UI views go idle; stage 1 re-applies the pin, stage 2 starts a UI heartbeat.
        Display.Mode m60 = RefreshSwitchMonitorTest.mode(1, 60f);
        Display.Mode m144 = RefreshSwitchMonitorTest.mode(2, 144f);
        RefreshSwitchMonitorTest.FakePanel.set(m144, m60, m144);
        Game activity = Robolectric.buildActivity(Game.class).get();
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.width = 2400;
        prefs.height = 1080;
        prefs.fps = 120;
        android.widget.FrameLayout holder = new android.widget.FrameLayout(activity);
        StreamView view = new StreamView(activity);
        view.initializeSurface(false);
        holder.addView(view);
        attachOverlayViews(activity, holder);
        ReflectionHelpers.setField(activity, "prefConfig", prefs);
        ReflectionHelpers.setField(activity, "streamView", view);
        ReflectionHelpers.setField(activity, "renderView", view);
        ReflectionHelpers.setField(activity, "connected", true);
        ReflectionHelpers.setField(activity, "foreground", true);
        ReflectionHelpers.setField(activity, "panelMaxRefreshRate", 144f);
        ReflectionHelpers.callInstanceMethod(activity.videoOutput, "prepareDisplayForRendering");

        SurfaceTexture texture = new SurfaceTexture(0);
        Surface surface = new Surface(texture);
        try {
            ReflectionHelpers.setField(activity, "videoSurface", surface);
            ReflectionHelpers.setField(activity, "surfaceCreated", true);
            vote(activity, surface);
            ReflectionHelpers.callInstanceMethod(activity.videoOutput, "startRefreshSwitchPolling");

            // Display drops to 60 Hz while idle.
            RefreshSwitchMonitorTest.FakePanel.set(m60, m60, m144);
            GameFrameRateTest.FrameRateSurface voteShadow = Shadow.extract(surface);
            voteShadow.frameRate = 0f;
            activity.videoOutput.refresh.pollRefreshSwitchTick();
            assertNull("Not settled yet", activity.videoOutput.refresh.refreshHeartbeat);

            // After settle window: stage 1 re-applies window mode pin and frame rate vote (no heartbeat yet).
            ShadowSystemClock.advanceBy(Duration.ofMillis(com.limelight.binding.video.RefreshSwitchDiagnosis.Watcher.SETTLE_MS));
            activity.videoOutput.refresh.pollRefreshSwitchTick();
            assertEquals(144f, voteShadow.frameRate, 0f);
            assertEquals(2, activity.getWindow().getAttributes().preferredDisplayModeId);
            assertNull(activity.videoOutput.refresh.refreshHeartbeat);

            // Still stuck at 60 Hz after another settle window: stage 2 starts fallback UI heartbeat.
            ShadowSystemClock.advanceBy(Duration.ofMillis(com.limelight.binding.video.RefreshSwitchDiagnosis.Watcher.SETTLE_MS));
            activity.videoOutput.refresh.pollRefreshSwitchTick();
            assertNotNull(activity.videoOutput.refresh.refreshHeartbeat);
            assertTrue(activity.videoOutput.refresh.refreshHeartbeat.running());
            assertEquals(144f, activity.videoOutput.refresh.refreshHeartbeat.dot().getRequestedFrameRate(), 0f);
            assertEquals(3, holder.getChildCount());

            // Teardown removes the fallback heartbeat dot cleanly.
            ReflectionHelpers.callInstanceMethod(activity.videoOutput, "stopRefreshSwitchPolling");
            assertNull(activity.videoOutput.refresh.refreshHeartbeat);
            Shadows.shadowOf(Looper.getMainLooper()).idle();
            assertEquals(2, holder.getChildCount());
        } finally {
            ReflectionHelpers.callInstanceMethod(activity.videoOutput, "stopRefreshSwitchPolling");
            Shadows.shadowOf(Looper.getMainLooper()).idle();
            RefreshSwitchMonitorTest.FakePanel.clear();
            surface.release();
            texture.release();
        }
    }

    @Test
    @Config(sdk = 35, shadows = {GameFrameRateTest.FrameRateSurface.class, RefreshSwitchMonitorTest.FakePanel.class})
    public void refreshDropRecoveryStartsHeartbeatImmediatelyOnSecondDropAfterReapplyAndStopsInBackground() {
        Display.Mode m60 = RefreshSwitchMonitorTest.mode(1, 60f);
        Display.Mode m144 = RefreshSwitchMonitorTest.mode(2, 144f);
        RefreshSwitchMonitorTest.FakePanel.set(m144, m60, m144);
        Game activity = Robolectric.buildActivity(Game.class).get();
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.width = 2400;
        prefs.height = 1080;
        prefs.fps = 120;
        android.widget.FrameLayout holder = new android.widget.FrameLayout(activity);
        StreamView view = new StreamView(activity);
        view.initializeSurface(false);
        holder.addView(view);
        attachOverlayViews(activity, holder);
        ReflectionHelpers.setField(activity, "prefConfig", prefs);
        ReflectionHelpers.setField(activity, "streamView", view);
        ReflectionHelpers.setField(activity, "renderView", view);
        ReflectionHelpers.setField(activity, "connected", true);
        ReflectionHelpers.setField(activity, "foreground", true);
        ReflectionHelpers.setField(activity, "panelMaxRefreshRate", 144f);
        ReflectionHelpers.callInstanceMethod(activity.videoOutput, "prepareDisplayForRendering");

        SurfaceTexture texture = new SurfaceTexture(0);
        Surface surface = new Surface(texture);
        try {
            ReflectionHelpers.setField(activity, "videoSurface", surface);
            ReflectionHelpers.setField(activity, "surfaceCreated", true);
            vote(activity, surface);
            ReflectionHelpers.callInstanceMethod(activity.videoOutput, "startRefreshSwitchPolling");

            // First drop settles -> stage 1 re-applies pin.
            RefreshSwitchMonitorTest.FakePanel.set(m60, m60, m144);
            activity.videoOutput.refresh.pollRefreshSwitchTick();
            ShadowSystemClock.advanceBy(Duration.ofMillis(com.limelight.binding.video.RefreshSwitchDiagnosis.Watcher.SETTLE_MS));
            activity.videoOutput.refresh.pollRefreshSwitchTick();
            assertNull(activity.videoOutput.refresh.refreshHeartbeat);

            // Re-apply temporarily restores 144 Hz, then idle drops it to 60 Hz a second time.
            RefreshSwitchMonitorTest.FakePanel.set(m144, m60, m144);
            ShadowSystemClock.advanceBy(Duration.ofSeconds(1));
            activity.videoOutput.refresh.pollRefreshSwitchTick();
            RefreshSwitchMonitorTest.FakePanel.set(m60, m60, m144);
            ShadowSystemClock.advanceBy(Duration.ofSeconds(1));
            activity.videoOutput.refresh.pollRefreshSwitchTick();
            ShadowSystemClock.advanceBy(Duration.ofMillis(com.limelight.binding.video.RefreshSwitchDiagnosis.Watcher.SETTLE_MS));
            activity.videoOutput.refresh.pollRefreshSwitchTick();
            assertNotNull(activity.videoOutput.refresh.refreshHeartbeat);
            assertTrue(activity.videoOutput.refresh.refreshHeartbeat.running());
            assertEquals(144f, activity.videoOutput.refresh.refreshHeartbeat.dot().getRequestedFrameRate(), 0f);

            // Moving out of foreground stops the fallback heartbeat on the next tick.
            ReflectionHelpers.setField(activity, "foreground", false);
            ShadowSystemClock.advanceBy(Duration.ofSeconds(1));
            activity.videoOutput.refresh.pollRefreshSwitchTick();
            assertNull(activity.videoOutput.refresh.refreshHeartbeat);
            Shadows.shadowOf(Looper.getMainLooper()).idle();
            assertEquals(2, holder.getChildCount());

            // Returning to foreground does not stay wedged at stage 2: a subsequent drop restarts
            // the fallback heartbeat cleanly (and even a transient missing parent does not wedge it).
            holder.removeView(view);
            ReflectionHelpers.setField(activity, "foreground", true);
            ShadowSystemClock.advanceBy(Duration.ofSeconds(1));
            activity.videoOutput.refresh.pollRefreshSwitchTick();
            assertNull("No parent holder yet; must stay retryable", activity.videoOutput.refresh.refreshHeartbeat);

            holder.addView(view);
            ShadowSystemClock.advanceBy(Duration.ofSeconds(1));
            activity.videoOutput.refresh.pollRefreshSwitchTick();
            assertNotNull(activity.videoOutput.refresh.refreshHeartbeat);
            assertTrue(activity.videoOutput.refresh.refreshHeartbeat.running());
            assertEquals(3, holder.getChildCount());

            // Pausing stops the fallback heartbeat immediately via updateCompositorHeartbeat().
            ReflectionHelpers.setField(activity, "foreground", false);
            activity.videoOutput.updateCompositorHeartbeat();
            assertNull(activity.videoOutput.refresh.refreshHeartbeat);
            Shadows.shadowOf(Looper.getMainLooper()).idle();
            assertEquals(2, holder.getChildCount());
        } finally {
            ReflectionHelpers.callInstanceMethod(activity.videoOutput, "stopRefreshSwitchPolling");
            Shadows.shadowOf(Looper.getMainLooper()).idle();
            RefreshSwitchMonitorTest.FakePanel.clear();
            surface.release();
            texture.release();
        }
    }

    @Test
    @Config(sdk = 35, shadows = {GameFrameRateTest.FrameRateSurface.class, RefreshSwitchMonitorTest.FakePanel.class})
    public void refreshDropRecoveryDoesNotEngageOnAndroidTvOrBatterySaver() {
        Display.Mode m60 = RefreshSwitchMonitorTest.mode(1, 60f);
        Display.Mode m120 = RefreshSwitchMonitorTest.mode(2, 120f);
        RefreshSwitchMonitorTest.FakePanel.set(m60, m60, m120);
        try {
            // Case 1: Battery saver caps the display to 60 Hz; Case 2: Android TV does not pin window modes.
            for (boolean tv : new boolean[] {false, true}) {
                Shadows.shadowOf((android.os.PowerManager) RuntimeEnvironment.getApplication()
                        .getSystemService(android.content.Context.POWER_SERVICE)).setIsPowerSaveMode(!tv);
                Shadows.shadowOf(RuntimeEnvironment.getApplication().getPackageManager())
                        .setSystemFeature(PackageManager.FEATURE_LEANBACK, tv);
                Game activity = Robolectric.buildActivity(Game.class).get();
                PreferenceConfiguration prefs = new PreferenceConfiguration();
                prefs.width = 2400;
                prefs.height = 1080;
                prefs.fps = 120;
                android.widget.FrameLayout holder = new android.widget.FrameLayout(activity);
                StreamView view = new StreamView(activity);
                view.initializeSurface(false);
                holder.addView(view);
                attachOverlayViews(activity, holder);
                ReflectionHelpers.setField(activity, "prefConfig", prefs);
                ReflectionHelpers.setField(activity, "streamView", view);
                ReflectionHelpers.setField(activity, "renderView", view);
                ReflectionHelpers.setField(activity, "connected", true);
                ReflectionHelpers.setField(activity, "foreground", true);
                ReflectionHelpers.setField(activity, "desiredRefreshRate", 120f);
                SurfaceTexture texture = new SurfaceTexture(0);
                Surface surface = new Surface(texture);
                try {
                    ReflectionHelpers.setField(activity, "videoSurface", surface);
                    ReflectionHelpers.setField(activity, "surfaceCreated", true);
                    vote(activity, surface);
                    ReflectionHelpers.callInstanceMethod(activity.videoOutput, "startRefreshSwitchPolling");
                    activity.videoOutput.refresh.pollRefreshSwitchTick();
                    ShadowSystemClock.advanceBy(Duration.ofMillis(com.limelight.binding.video.RefreshSwitchDiagnosis.Watcher.SETTLE_MS * 2));
                    activity.videoOutput.refresh.pollRefreshSwitchTick();
                    ShadowSystemClock.advanceBy(Duration.ofMillis(com.limelight.binding.video.RefreshSwitchDiagnosis.Watcher.SETTLE_MS * 2));
                    activity.videoOutput.refresh.pollRefreshSwitchTick();
                    assertNull(tv ? "Android TV" : "Battery saver", activity.videoOutput.refresh.refreshHeartbeat);
                } finally {
                    ReflectionHelpers.callInstanceMethod(activity.videoOutput, "stopRefreshSwitchPolling");
                    surface.release();
                    texture.release();
                }
            }
        } finally {
            Shadows.shadowOf((android.os.PowerManager) RuntimeEnvironment.getApplication()
                    .getSystemService(android.content.Context.POWER_SERVICE)).setIsPowerSaveMode(false);
            Shadows.shadowOf(RuntimeEnvironment.getApplication().getPackageManager())
                    .setSystemFeature(PackageManager.FEATURE_LEANBACK, false);
            RefreshSwitchMonitorTest.FakePanel.clear();
        }
    }

    @Test
    @Config(sdk = 35, shadows = {GameFrameRateTest.FrameRateSurface.class, RefreshSwitchMonitorTest.FakePanel.class})
    public void refreshHeartbeatTakesPrecedenceOverKeepaliveAndRestoresKeepaliveOnStop() {
        Display.Mode m60 = RefreshSwitchMonitorTest.mode(1, 60f);
        Display.Mode m144 = RefreshSwitchMonitorTest.mode(2, 144f);
        RefreshSwitchMonitorTest.FakePanel.set(m144, m60, m144);
        Game activity = Robolectric.buildActivity(Game.class).get();
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.width = 2400;
        prefs.height = 1080;
        prefs.fps = 120;
        prefs.tvDisplayCompatibility = true;
        android.widget.FrameLayout holder = new android.widget.FrameLayout(activity);
        StreamView view = new StreamView(activity);
        view.initializeSurface(false);
        holder.addView(view);
        attachOverlayViews(activity, holder);
        ReflectionHelpers.setField(activity, "prefConfig", prefs);
        ReflectionHelpers.setField(activity, "streamView", view);
        ReflectionHelpers.setField(activity, "renderView", view);
        ReflectionHelpers.setField(activity, "connected", true);
        ReflectionHelpers.setField(activity, "foreground", true);
        ReflectionHelpers.setField(activity, "panelMaxRefreshRate", 144f);
        ReflectionHelpers.callInstanceMethod(activity.videoOutput, "prepareDisplayForRendering");

        SurfaceTexture texture = new SurfaceTexture(0);
        Surface surface = new Surface(texture);
        try {
            ReflectionHelpers.setField(activity, "videoSurface", surface);
            ReflectionHelpers.setField(activity, "surfaceCreated", true);
            vote(activity, surface);
            activity.videoOutput.updateCompositorHeartbeat();
            assertNotNull("TV keepalive starts before #168 activates", activity.videoOutput.compositorHeartbeat);
            assertTrue(activity.videoOutput.compositorHeartbeat.running());

            ReflectionHelpers.callInstanceMethod(activity.videoOutput, "startRefreshSwitchPolling");
            RefreshSwitchMonitorTest.FakePanel.set(m60, m60, m144);
            GameFrameRateTest.FrameRateSurface voteShadow = Shadow.extract(surface);
            voteShadow.frameRate = 0f;
            activity.videoOutput.refresh.pollRefreshSwitchTick();
            ShadowSystemClock.advanceBy(Duration.ofMillis(com.limelight.binding.video.RefreshSwitchDiagnosis.Watcher.SETTLE_MS));
            activity.videoOutput.refresh.pollRefreshSwitchTick();
            ShadowSystemClock.advanceBy(Duration.ofMillis(com.limelight.binding.video.RefreshSwitchDiagnosis.Watcher.SETTLE_MS));
            activity.videoOutput.refresh.pollRefreshSwitchTick();

            assertNotNull("#168 refreshHeartbeat should start cleanly even with keepalive enabled",
                    activity.videoOutput.refresh.refreshHeartbeat);
            assertTrue(activity.videoOutput.refresh.refreshHeartbeat.running());
            assertNull("Keepalive should yield while #168 refreshHeartbeat owns the workaround",
                    activity.videoOutput.compositorHeartbeat);

            ReflectionHelpers.callInstanceMethod(activity.videoOutput, "stopRefreshSwitchPolling");
            Shadows.shadowOf(Looper.getMainLooper()).idle();
            assertNull(activity.videoOutput.refresh.refreshHeartbeat);
            assertNotNull("Keepalive should resume once #168 refreshHeartbeat stops",
                    activity.videoOutput.compositorHeartbeat);
            assertTrue(activity.videoOutput.compositorHeartbeat.running());
        } finally {
            ReflectionHelpers.setField(activity, "connected", false);
            activity.videoOutput.updateCompositorHeartbeat();
            ReflectionHelpers.callInstanceMethod(activity.videoOutput, "stopRefreshSwitchPolling");
            Shadows.shadowOf(Looper.getMainLooper()).idle();
            RefreshSwitchMonitorTest.FakePanel.clear();
            surface.release();
            texture.release();
        }
    }

    @Test
    public void fractionalLaunchRefreshRateIsPreservedInClientRefreshRateX100() {
        // Issue #119: explicit fractional stream rate (e.g. 120.04 on nominal 120.0 Hz Fold7)
        // must survive into RTSP clientRefreshRateX100 instead of being overwritten with 12000.
        assertEquals(12004, GameStreamUiSetup.clientRefreshRateX100(12004, 120, 120.0f));
        assertEquals(11988, GameStreamUiSetup.clientRefreshRateX100(11988, 120, 120.0f));
        assertEquals(5994, GameStreamUiSetup.clientRefreshRateX100(5994, 60, 120.0f));
        // Unset launchRefreshRateX100 or a stream clamped to a different integer rate keeps desiredRefreshRate.
        assertEquals(12000, GameStreamUiSetup.clientRefreshRateX100(0, 120, 120.0f));
        assertEquals(5994, GameStreamUiSetup.clientRefreshRateX100(0, 60, 59.94f));
        assertEquals(6000, GameStreamUiSetup.clientRefreshRateX100(12004, 60, 60.0f));
    }

    private static void attachOverlayViews(Game activity, android.widget.FrameLayout holder) {
        android.widget.ScrollView scroll = new android.widget.ScrollView(activity);
        android.widget.LinearLayout row = new android.widget.LinearLayout(activity);
        android.widget.TextView col1 = new android.widget.TextView(activity);
        android.widget.TextView col2 = new android.widget.TextView(activity);
        col2.setId(R.id.performanceOverlayColumn2);
        row.addView(col1);
        row.addView(col2);
        scroll.addView(row);
        holder.addView(scroll);
        ReflectionHelpers.setField(activity, "performanceOverlayView", col1);
        ReflectionHelpers.setField(activity, "performanceOverlayScroll", scroll);
    }

    private static void giveStreamView(Game activity) {
        SurfaceHolder holder = new ShadowSurfaceView.FakeSurfaceHolder();
        StreamView view = new StreamView(activity) {
            @Override public SurfaceHolder getHolder() { return holder; }
        };
        ReflectionHelpers.setField(activity, "streamView", view);
        ReflectionHelpers.setField(activity, "renderView", view);
    }

    private static void vote(Game activity, Surface surface) {
        ReflectionHelpers.callInstanceMethod(activity.videoOutput, "voteSurfaceFrameRate",
                ReflectionHelpers.ClassParameter.from(Surface.class, surface));
    }

    private static final class ContentRateRenderer extends MediaCodecDecoderRenderer {
        int contentMilliHz;

        ContentRateRenderer(Game activity, PreferenceConfiguration prefs) {
            super(activity, prefs, null, 0, false, false, "", null);
            ReflectionHelpers.setField(this, "refreshRate", prefs.fps);
            ReflectionHelpers.setField(this, "configuredFormat", MediaFormat.createVideoFormat("video/avc", 1280, 720));
        }

        @Override public int getContentRateMilliHz() {
            return contentMilliHz;
        }
    }
}
