package com.limelight;

import android.app.Application;
import android.content.Context;
import android.content.Intent;
import android.net.wifi.WifiManager;
import android.os.Looper;
import android.view.SurfaceHolder;
import android.view.View;
import android.view.WindowManager;

import com.limelight.binding.input.KeyboardTranslator;
import com.limelight.binding.video.UpscalingPolicy;
import com.limelight.preferences.AutoBitrateChoice;
import com.limelight.preferences.PreferenceConfiguration;
import com.limelight.ui.StreamView;
import com.limelight.ui.VideoScaling;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;

import java.util.concurrent.atomic.AtomicReference;

import static org.junit.Assert.*;

@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class GameLifecycleTest {
    @Test
    public void invalidBitrateStaysInTheDialogForCorrection() {
        Game activity = Robolectric.buildActivity(Game.class).get();
        activity.setTheme(com.limelight.R.style.AppTheme);
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.bitrate = 10000;
        ReflectionHelpers.setField(activity, "prefConfig", prefs);
        ReflectionHelpers.callInstanceMethod(activity.adaptation, "showBitrateDialog");
        androidx.appcompat.app.AlertDialog dialog = ReflectionHelpers.getField(activity, "streamMenu");
        try {
            java.util.ArrayDeque<View> views = new java.util.ArrayDeque<>();
            views.add(dialog.getWindow().getDecorView());
            com.google.android.material.textfield.TextInputLayout field = null;
            while (!views.isEmpty()) {
                View view = views.remove();
                if (view instanceof com.google.android.material.textfield.TextInputLayout) {
                    field = (com.google.android.material.textfield.TextInputLayout) view;
                    break;
                }
                if (view instanceof android.view.ViewGroup) {
                    android.view.ViewGroup group = (android.view.ViewGroup) view;
                    for (int i = 0; i < group.getChildCount(); i++) views.add(group.getChildAt(i));
                }
            }
            assertNotNull(field);
            field.getEditText().setText("0.1");
            dialog.getButton(androidx.appcompat.app.AlertDialog.BUTTON_POSITIVE).performClick();
            assertTrue(dialog.isShowing());
            assertEquals(activity.getString(com.limelight.R.string.stream_bitrate_invalid), field.getError().toString());
        } finally {
            dialog.dismiss();
        }
    }

    @Test
    public void overlayTouchReachesTheStreamWithTranslatedCoordinates() {
        TouchGame activity = Robolectric.buildActivity(TouchGame.class).get();
        StreamView stream = new StreamView(activity);
        stream.setX(100);
        stream.setY(20);
        View overlay = new View(activity);
        overlay.setX(8);
        overlay.setY(8);
        ReflectionHelpers.setField(activity, "streamView", stream);
        android.view.MotionEvent event = android.view.MotionEvent.obtain(0, 0,
                android.view.MotionEvent.ACTION_DOWN, 200, 100, 0);
        try {
            assertTrue(ReflectionHelpers.callInstanceMethod(activity.touchInput, "onOverlayTouch",
                    ReflectionHelpers.ClassParameter.from(View.class, overlay),
                    ReflectionHelpers.ClassParameter.from(android.view.MotionEvent.class, event)));
            assertSame(stream, activity.touchView);
            assertEquals(108, activity.touchX, 0);
            assertEquals(88, activity.touchY, 0);
            assertEquals(200, event.getX(), 0);
        } finally {
            event.recycle();
        }
    }

    public static class TouchGame extends Game {
        View touchView;
        float touchX, touchY;
        @Override public boolean onTouch(View view, android.view.MotionEvent event) {
            touchView = view;
            touchX = event.getX();
            touchY = event.getY();
            return true;
        }
    }

    @Test
    public void bitrateFailureRetainsTheEnabledControllerAndCanRetry() throws Exception {
        Game activity = Robolectric.buildActivity(Game.class).get();
        com.limelight.nvstream.AdaptiveBitrateController controller =
                new com.limelight.nvstream.AdaptiveBitrateController(10000, 8000, 60, 0);
        RetryConnection connection = new RetryConnection();
        ReflectionHelpers.setField(activity, "adaptiveBitrate", controller);
        ReflectionHelpers.setField(activity, "conn", connection);
        ReflectionHelpers.setField(activity, "connected", true);
        activity.getSharedPreferences("AdaptiveBitrate", Context.MODE_PRIVATE).edit().putBoolean("pc", true).commit();
        ReflectionHelpers.callInstanceMethod(activity.adaptation, "applyAdaptiveBitrate",
                ReflectionHelpers.ClassParameter.from(int.class, 6400));
        awaitBitrateRequest(activity);
        assertSame(controller, ReflectionHelpers.getField(activity, "adaptiveBitrate"));
        assertTrue(activity.getSharedPreferences("AdaptiveBitrate", Context.MODE_PRIVATE).getBoolean("pc", false));
        assertTrue((long) ReflectionHelpers.getField(activity, "adaptiveBitrateRetryAt") > 0);
        connection.fail = false;
        ReflectionHelpers.callInstanceMethod(activity.adaptation, "applyAdaptiveBitrate",
                ReflectionHelpers.ClassParameter.from(int.class, 6400));
        awaitBitrateRequest(activity);
        assertEquals(6400, (int) ReflectionHelpers.getField(activity, "currentBitrate"));
        assertEquals(0, (long) ReflectionHelpers.getField(activity, "adaptiveBitrateRetryAt"));
    }

    private static void awaitBitrateRequest(Game activity) throws InterruptedException {
        long deadline = System.nanoTime() + 2_000_000_000L;
        while ((boolean) ReflectionHelpers.getField(activity, "hostActionInProgress") && System.nanoTime() < deadline) {
            org.robolectric.shadows.ShadowLooper.idleMainLooper();
            Thread.sleep(5);
        }
        assertFalse(ReflectionHelpers.getField(activity, "hostActionInProgress"));
    }

    @Test
    public void autoBitrateRequiresMetadataSupportButNotAnActiveStream() {
        Game activity = Robolectric.buildActivity(Game.class).get();
        RetryConnection connection = new RetryConnection();
        com.limelight.nvstream.http.ComputerDetails details = connection.getHostDetails();
        details.caps = com.limelight.nvstream.http.TestHosts.rubylight();
        details.permission = com.limelight.nvstream.http.ComputerDetails.PERMISSION_VIEW;
        ReflectionHelpers.setField(activity, "app", new com.limelight.nvstream.http.NvApp());
        ReflectionHelpers.setField(activity, "conn", connection);
        activity.getSharedPreferences("AdaptiveBitrate", Context.MODE_PRIVATE).edit().putBoolean("pc", true).commit();
        assertFalse(ReflectionHelpers.callInstanceMethod(activity.adaptation, "supportsAdaptiveBitrate"));
        assertTrue(activity.getSharedPreferences("AdaptiveBitrate", Context.MODE_PRIVATE).getBoolean("pc", false));
        details.encoderBitrateSupported = true;
        assertEquals(0, details.encoderBitrateKbps);
        assertEquals(0, details.initialEncoderBitrateKbps);
        assertTrue(ReflectionHelpers.callInstanceMethod(activity.adaptation, "supportsAdaptiveBitrate"));
        details.permission = 0;
        assertFalse(ReflectionHelpers.callInstanceMethod(activity.adaptation, "supportsAdaptiveBitrate"));
    }

    @Test
    public void enablingAutoBitrateClaimsTheCurrentEncoderBudgetWithoutIncreasingIt() throws Exception {
        for (int format : new int[] {com.limelight.nvstream.jni.MoonBridge.VIDEO_FORMAT_H264,
                com.limelight.nvstream.jni.MoonBridge.VIDEO_FORMAT_PYROWAVE}) {
            Game activity = Robolectric.buildActivity(Game.class).get();
            activity.setIntent(new Intent().putExtra(Game.EXTRA_PC_UUID, "pc"));
            PreferenceConfiguration prefs = new PreferenceConfiguration();
            prefs.videoFormat = PreferenceConfiguration.FormatOption.AUTO;
            prefs.bitrate = 50000;
            prefs.fps = 60;
            ReflectionHelpers.setStaticField(com.limelight.binding.video.MediaCodecHelper.class, "initialized", true);
            com.limelight.binding.video.MediaCodecDecoderRenderer renderer =
                    new com.limelight.binding.video.MediaCodecDecoderRenderer(activity, prefs, null, 0,
                            false, false, "", null);
            ReflectionHelpers.setField(renderer, "videoFormat", format);
            RetryConnection connection = new RetryConnection();
            connection.fail = false;
            connection.details.encoderBitrateKbps = 38988;
            connection.details.initialEncoderBitrateKbps = 38988;
            ReflectionHelpers.setField(activity, "decoderRenderer", renderer);
            ReflectionHelpers.setField(activity, "prefConfig", prefs);
            ReflectionHelpers.setField(activity, "streamFps", 60);
            ReflectionHelpers.setField(activity, "currentBitrate", 50000);
            ReflectionHelpers.setField(activity, "conn", connection);
            ReflectionHelpers.setField(activity, "connected", true);
            try {
                ReflectionHelpers.callInstanceMethod(activity.adaptation, "setAdaptiveBitrateEnabled",
                        ReflectionHelpers.ClassParameter.from(boolean.class, true),
                        ReflectionHelpers.ClassParameter.from(boolean.class, true));
                awaitBitrateRequest(activity);
                assertEquals(1, connection.writes);
                assertEquals(38988, connection.lastWrite);
                assertEquals(38988, (int) ReflectionHelpers.getField(activity, "currentBitrate"));
                com.limelight.nvstream.AdaptiveBitrateController controller = ReflectionHelpers.getField(activity, "adaptiveBitrate");
                com.limelight.nvstream.PyroWaveBitrateController pyro = ReflectionHelpers.getField(activity, "pyroWaveBitrate");
                for (long time : new long[] {30000, 60000, 90000}) {
                    int target = controller != null ? controller.sample(time, true, false, 0, 2, 0, 0) :
                            pyro.sample(time, true, false, 0, 0, 2, 0);
                    assertEquals(0, target);
                }
                if (controller != null) controller.applied(30000, 30000, 90000);
                else pyro.applied(30000, 30000, 90000);
                int current = 30000;
                for (long time = 120000; time <= 1800000; time += 30000) {
                    int target = controller != null ? controller.sample(time, true, false, 0, 2, 0, 0) :
                            pyro.sample(time, true, false, 0, 0, 2, 0);
                    if (target != 0) {
                        assertTrue(target <= 38988);
                        current = target;
                        if (controller != null) controller.applied(target, target, time);
                        else pyro.applied(target, target, time);
                    }
                }
                assertEquals(38988, current);
            } finally {
                ReflectionHelpers.callInstanceMethod(activity.adaptation, "setAdaptiveBitrateEnabled",
                        ReflectionHelpers.ClassParameter.from(boolean.class, false),
                        ReflectionHelpers.ClassParameter.from(boolean.class, true));
            }
        }
    }

    @Test
    public void missingEncoderBudgetRetriesWithoutChangingTheHost() throws Exception {
        Game activity = Robolectric.buildActivity(Game.class).get();
        com.limelight.nvstream.AdaptiveBitrateController waiting =
                new com.limelight.nvstream.AdaptiveBitrateController(0, 0, 60, 0);
        RetryConnection connection = new RetryConnection();
        connection.fail = false;
        connection.details.encoderBitrateSupported = true;
        ReflectionHelpers.setField(activity, "adaptiveBitrate", waiting);
        ReflectionHelpers.setField(activity, "prefConfig", new PreferenceConfiguration());
        ReflectionHelpers.setField(activity, "streamFps", 60);
        ReflectionHelpers.setField(activity, "conn", connection);
        ReflectionHelpers.setField(activity, "connected", true);
        ReflectionHelpers.callInstanceMethod(activity.adaptation, "applyAdaptiveBitrate",
                ReflectionHelpers.ClassParameter.from(int.class, 0));
        awaitBitrateRequest(activity);
        assertSame(waiting, ReflectionHelpers.getField(activity, "adaptiveBitrate"));
        assertTrue((long) ReflectionHelpers.getField(activity, "adaptiveBitrateRetryAt") > 0);
        assertEquals(0, connection.writes);
        connection.details.encoderBitrateKbps = 45000;
        connection.details.initialEncoderBitrateKbps = 38988;
        long retryAt = ReflectionHelpers.getField(activity, "adaptiveBitrateRetryAt");
        android.os.Handler handler = ReflectionHelpers.getField(activity, "bitrateHandler");
        Runnable update = ReflectionHelpers.getField(activity, "updateBitrate");
        handler.postAtTime(update, retryAt);
        try {
            org.robolectric.Shadows.shadowOf(Looper.getMainLooper()).idleFor(java.time.Duration.ofMillis(
                    retryAt - android.os.SystemClock.uptimeMillis() - 1));
            assertSame(waiting, ReflectionHelpers.getField(activity, "adaptiveBitrate"));
            assertEquals(1, connection.reads);
            org.robolectric.Shadows.shadowOf(Looper.getMainLooper()).idleFor(java.time.Duration.ofMillis(1));
            awaitBitrateRequest(activity);
            assertEquals(45000, (int) ReflectionHelpers.getField(activity, "currentBitrate"));
            assertEquals(0, (long) ReflectionHelpers.getField(activity, "adaptiveBitrateRetryAt"));
            com.limelight.nvstream.AdaptiveBitrateController controller = ReflectionHelpers.getField(activity, "adaptiveBitrate");
            assertNotSame(waiting, controller);
            assertEquals(45000, (int) ReflectionHelpers.getField(controller, "ceilingKbps"));
            assertEquals(2, connection.reads);
            assertEquals(1, connection.writes);
            assertEquals(45000, connection.lastWrite);
        } finally {
            handler.removeCallbacks(update);
        }
    }

    @Test
    public void theStreamsBitrateIsTheCeilingAndTheBuiltInFloorApplies() throws Exception {
        Game activity = Robolectric.buildActivity(Game.class).get();
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        RetryConnection connection = new RetryConnection();
        connection.fail = false;
        connection.details.encoderBitrateSupported = true;
        connection.details.encoderBitrateKbps = 38988;
        connection.details.initialEncoderBitrateKbps = 38988;
        ReflectionHelpers.setField(activity, "prefConfig", prefs);
        ReflectionHelpers.setField(activity, "adaptiveBitrate",
                new com.limelight.nvstream.AdaptiveBitrateController(0, 0, 60, 0));
        ReflectionHelpers.setField(activity, "streamFps", 60);
        ReflectionHelpers.setField(activity, "conn", connection);
        ReflectionHelpers.setField(activity, "connected", true);
        ReflectionHelpers.callInstanceMethod(activity.adaptation, "applyAdaptiveBitrate",
                ReflectionHelpers.ClassParameter.from(int.class, 0));
        awaitBitrateRequest(activity);
        com.limelight.nvstream.AdaptiveBitrateController controller = ReflectionHelpers.getField(activity, "adaptiveBitrate");
        // No minimum or maximum any more: the bitrate the stream started at is the ceiling, the floor the built-in one.
        assertEquals(38988, (int) ReflectionHelpers.getField(controller, "ceilingKbps"));
        assertEquals(com.limelight.nvstream.AdaptiveBitrateController.DEFAULT_FLOOR_KBPS,
                (int) ReflectionHelpers.getField(controller, "floorKbps"));
    }

    @Test
    public void theStreamMenuToggleAndTheSettingsSwitchShareOneChoicePerPc() throws Exception {
        for (boolean pyroWave : new boolean[] {false, true}) {
            Game activity = Robolectric.buildActivity(Game.class).get();
            activity.setIntent(new Intent().putExtra(Game.EXTRA_PC_UUID, "pc"));
            PreferenceConfiguration prefs = new PreferenceConfiguration();
            prefs.videoFormat = PreferenceConfiguration.FormatOption.AUTO;
            ReflectionHelpers.setStaticField(com.limelight.binding.video.MediaCodecHelper.class, "initialized", true);
            com.limelight.binding.video.MediaCodecDecoderRenderer renderer =
                    new com.limelight.binding.video.MediaCodecDecoderRenderer(activity, prefs, null, 0,
                            false, false, "", null);
            ReflectionHelpers.setField(renderer, "videoFormat", pyroWave ?
                    com.limelight.nvstream.jni.MoonBridge.VIDEO_FORMAT_PYROWAVE :
                    com.limelight.nvstream.jni.MoonBridge.VIDEO_FORMAT_H264);
            RetryConnection connection = new RetryConnection();
            connection.fail = false;
            connection.details.encoderBitrateKbps = 20000;
            connection.details.initialEncoderBitrateKbps = 20000;
            ReflectionHelpers.setField(activity, "decoderRenderer", renderer);
            ReflectionHelpers.setField(activity, "prefConfig", prefs);
            ReflectionHelpers.setField(activity, "streamFps", 60);
            ReflectionHelpers.setField(activity, "conn", connection);
            ReflectionHelpers.setField(activity, "connected", true);
            String store = pyroWave ? "PyroWaveBitrate" : "AdaptiveBitrate";
            android.preference.PreferenceManager.getDefaultSharedPreferences(activity).edit()
                    .putBoolean("checkbox_auto_bitrate", true).commit();
            // The Settings switch is the default; the menu's toggle then chooses for this PC alone.
            assertTrue(AutoBitrateChoice.isOn(activity, store, "pc"));
            try {
                ReflectionHelpers.callInstanceMethod(activity.adaptation, "setAdaptiveBitrateEnabled",
                        ReflectionHelpers.ClassParameter.from(boolean.class, false),
                        ReflectionHelpers.ClassParameter.from(boolean.class, true));
                assertFalse(AutoBitrateChoice.isOn(activity, store, "pc"));
                assertTrue(AutoBitrateChoice.isOn(activity, store, "other"));
                ReflectionHelpers.callInstanceMethod(activity.adaptation, "setAdaptiveBitrateEnabled",
                        ReflectionHelpers.ClassParameter.from(boolean.class, true),
                        ReflectionHelpers.ClassParameter.from(boolean.class, true));
                awaitBitrateRequest(activity);
                assertTrue(AutoBitrateChoice.isOn(activity, store, "pc"));
            } finally {
                ReflectionHelpers.callInstanceMethod(activity.adaptation, "setAdaptiveBitrateEnabled",
                        ReflectionHelpers.ClassParameter.from(boolean.class, false),
                        ReflectionHelpers.ClassParameter.from(boolean.class, false));
                android.preference.PreferenceManager.getDefaultSharedPreferences(activity).edit().clear().commit();
                activity.getSharedPreferences(store, Context.MODE_PRIVATE).edit().clear().commit();
            }
        }
    }

    @Test
    public void theStreamStartsWithAutomaticBitrateFromThisPcsChoiceElseTheSettingsSwitch() {
        for (boolean pyroWave : new boolean[] {false, true}) {
            Game activity = Robolectric.buildActivity(Game.class).get();
            activity.setIntent(new Intent().putExtra(Game.EXTRA_PC_UUID, "pc"));
            PreferenceConfiguration prefs = new PreferenceConfiguration();
            prefs.videoFormat = PreferenceConfiguration.FormatOption.AUTO;
            ReflectionHelpers.setStaticField(com.limelight.binding.video.MediaCodecHelper.class, "initialized", true);
            com.limelight.binding.video.MediaCodecDecoderRenderer renderer =
                    new com.limelight.binding.video.MediaCodecDecoderRenderer(activity, prefs, null, 0,
                            false, false, "", null);
            ReflectionHelpers.setField(renderer, "videoFormat", pyroWave ?
                    com.limelight.nvstream.jni.MoonBridge.VIDEO_FORMAT_PYROWAVE :
                    com.limelight.nvstream.jni.MoonBridge.VIDEO_FORMAT_H264);
            RetryConnection connection = new RetryConnection();
            com.limelight.nvstream.http.ComputerDetails details = connection.getHostDetails();
            details.caps = com.limelight.nvstream.http.TestHosts.rubylight();
            details.permission = com.limelight.nvstream.http.ComputerDetails.PERMISSION_VIEW |
                    com.limelight.nvstream.http.ComputerDetails.PERMISSION_LAUNCH;
            details.encoderBitrateSupported = true;
            ReflectionHelpers.setField(activity, "app", new com.limelight.nvstream.http.NvApp());
            ReflectionHelpers.setField(activity, "conn", connection);
            ReflectionHelpers.setField(activity, "decoderRenderer", renderer);
            String store = pyroWave ? "PyroWaveBitrate" : "AdaptiveBitrate";
            String other = pyroWave ? "AdaptiveBitrate" : "PyroWaveBitrate";
            android.content.SharedPreferences settings =
                    android.preference.PreferenceManager.getDefaultSharedPreferences(activity);
            try {
                assertFalse("off by default", activity.adaptation.autoBitrateAtStart());
                settings.edit().putBoolean("checkbox_auto_bitrate", true).commit();
                assertTrue("the switch is the default", activity.adaptation.autoBitrateAtStart());
                // This PC chose off in the stream menu.
                activity.getSharedPreferences(store, Context.MODE_PRIVATE).edit().putBoolean("pc", false).commit();
                assertFalse("the PC's own choice wins", activity.adaptation.autoBitrateAtStart());
                // Only the store of the stream's codec counts.
                activity.getSharedPreferences(store, Context.MODE_PRIVATE).edit().clear().commit();
                activity.getSharedPreferences(other, Context.MODE_PRIVATE).edit().putBoolean("pc", false).commit();
                assertTrue(activity.adaptation.autoBitrateAtStart());
                // A host without runtime bitrate support never starts it.
                details.encoderBitrateSupported = false;
                assertFalse(activity.adaptation.autoBitrateAtStart());
            } finally {
                settings.edit().clear().commit();
                activity.getSharedPreferences("AdaptiveBitrate", Context.MODE_PRIVATE).edit().clear().commit();
                activity.getSharedPreferences("PyroWaveBitrate", Context.MODE_PRIVATE).edit().clear().commit();
            }
        }
    }

    @Test
    public void aManualBitrateStopsAutomaticBitrateForTheSessionButKeepsTheChoice() throws Exception {
        Game activity = Robolectric.buildActivity(Game.class).get();
        activity.setTheme(com.limelight.R.style.AppTheme);
        activity.setIntent(new Intent().putExtra(Game.EXTRA_PC_UUID, "pc"));
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.videoFormat = PreferenceConfiguration.FormatOption.AUTO;
        prefs.bitrate = 20000;
        ReflectionHelpers.setStaticField(com.limelight.binding.video.MediaCodecHelper.class, "initialized", true);
        com.limelight.binding.video.MediaCodecDecoderRenderer renderer =
                new com.limelight.binding.video.MediaCodecDecoderRenderer(activity, prefs, null, 0,
                        false, false, "", null);
        ReflectionHelpers.setField(renderer, "videoFormat", com.limelight.nvstream.jni.MoonBridge.VIDEO_FORMAT_H264);
        RetryConnection connection = new RetryConnection();
        connection.fail = false;
        ReflectionHelpers.setField(activity, "decoderRenderer", renderer);
        ReflectionHelpers.setField(activity, "prefConfig", prefs);
        ReflectionHelpers.setField(activity, "conn", connection);
        ReflectionHelpers.setField(activity, "connected", true);
        ReflectionHelpers.setField(activity, "foreground", true);
        ReflectionHelpers.setField(activity, "adaptiveBitrate",
                new com.limelight.nvstream.AdaptiveBitrateController(20000, 20000, 60, 0));
        android.content.SharedPreferences settings =
                android.preference.PreferenceManager.getDefaultSharedPreferences(activity);
        settings.edit().putBoolean("checkbox_auto_bitrate", true).commit();
        ReflectionHelpers.callInstanceMethod(activity.adaptation, "showBitrateDialog");
        androidx.appcompat.app.AlertDialog dialog = ReflectionHelpers.getField(activity, "streamMenu");
        try {
            java.util.ArrayDeque<View> views = new java.util.ArrayDeque<>();
            views.add(dialog.getWindow().getDecorView());
            android.widget.EditText input = null;
            while (!views.isEmpty() && input == null) {
                View view = views.remove();
                if (view instanceof android.widget.EditText) {
                    input = (android.widget.EditText) view;
                } else if (view instanceof android.view.ViewGroup) {
                    android.view.ViewGroup group = (android.view.ViewGroup) view;
                    for (int i = 0; i < group.getChildCount(); i++) views.add(group.getChildAt(i));
                }
            }
            assertNotNull(input);
            input.setText("30");
            dialog.getButton(androidx.appcompat.app.AlertDialog.BUTTON_POSITIVE).performClick();
            awaitBitrateRequest(activity);
            assertEquals(30000, connection.lastWrite);
            assertNull("automatic bitrate stops for this session", ReflectionHelpers.getField(activity, "adaptiveBitrate"));
            assertTrue("the choice stays on", AutoBitrateChoice.isOn(activity, "AdaptiveBitrate", "pc"));
        } finally {
            dialog.dismiss();
            settings.edit().clear().commit();
            activity.getSharedPreferences("AdaptiveBitrate", Context.MODE_PRIVATE).edit().clear().commit();
        }
    }

    @Test
    public void autoBitrateStopsClaimingWhenTheHostNeverReportsItsEncoderBudget() throws Exception {
        for (boolean supported : new boolean[] {true, false}) {
            Game activity = Robolectric.buildActivity(Game.class).get();
            activity.setIntent(new Intent().putExtra(Game.EXTRA_PC_UUID, "pc"));
            activity.getSharedPreferences("AdaptiveBitrate", Context.MODE_PRIVATE).edit().putBoolean("pc", true).commit();
            RetryConnection connection = new RetryConnection();
            connection.fail = false;
            // The host answers, but reports encoder bitrate 0 (or no runtime bitrate support at all).
            connection.details.encoderBitrateSupported = supported;
            ReflectionHelpers.setField(activity, "adaptiveBitrate",
                    new com.limelight.nvstream.AdaptiveBitrateController(0, 0, 60, 0));
            ReflectionHelpers.setField(activity, "streamFps", 60);
            ReflectionHelpers.setField(activity, "conn", connection);
            ReflectionHelpers.setField(activity, "connected", true);
            android.os.Handler handler = ReflectionHelpers.getField(activity, "bitrateHandler");
            Runnable update = ReflectionHelpers.getField(activity, "updateBitrate");
            try {
                ReflectionHelpers.callInstanceMethod(activity.adaptation, "applyAdaptiveBitrate",
                        ReflectionHelpers.ClassParameter.from(int.class, 0));
                awaitBitrateRequest(activity);
                handler.postDelayed(update, 1000);
                // Five minutes of the stream's 1 s bitrate timer.
                for (int second = 0; second < 300; second++) {
                    org.robolectric.Shadows.shadowOf(Looper.getMainLooper()).idleFor(java.time.Duration.ofSeconds(1));
                    awaitBitrateRequest(activity);
                }
                assertNull("Auto bitrate gives up for this stream", ReflectionHelpers.getField(activity, "adaptiveBitrate"));
                assertTrue("supported " + supported + ": " + connection.reads + " claims",
                        supported ? connection.reads <= 10 : connection.reads == 1);
                assertEquals("The host's bitrate is never changed", 0, connection.writes);
                assertTrue("The PC's saved choice stays the person's own",
                        activity.getSharedPreferences("AdaptiveBitrate", Context.MODE_PRIVATE).getBoolean("pc", false));
            } finally {
                handler.removeCallbacks(update);
            }
        }
    }

    private static class RetryConnection extends com.limelight.nvstream.NvConnection {
        boolean fail = true;
        int writes;
        int lastWrite;
        int reads;
        final com.limelight.nvstream.http.ComputerDetails details = new com.limelight.nvstream.http.ComputerDetails();
        RetryConnection() {
            super(null, new com.limelight.nvstream.http.ComputerDetails.AddressTuple("127.0.0.1", 47989),
                    47984, "test", new com.limelight.nvstream.StreamConfiguration.Builder().build(), null, null);
        }
        @Override public int setBitrate(int bitrate) throws java.io.IOException {
            writes++;
            lastWrite = bitrate;
            if (fail) throw new java.io.IOException("temporary failure");
            return bitrate;
        }
        @Override public com.limelight.nvstream.http.ComputerDetails refreshHostDetails() throws java.io.IOException {
            reads++;
            if (fail) throw new java.io.IOException("temporary failure");
            return details;
        }
    }

    @Test
    @Config(qualifiers = "sw600dp-w1000dp-h800dp")
    public void largeWindowsDoNotForceTheDisplayIntoLandscape() {
        Game activity = Robolectric.buildActivity(Game.class).get();
        ReflectionHelpers.callInstanceMethod(activity.videoOutput, "setPreferredOrientationForCurrentDisplay");
        assertEquals(android.content.pm.ActivityInfo.SCREEN_ORIENTATION_FULL_USER, activity.getRequestedOrientation());
    }

    @Test
    public void offUsesStreamSizedCompositorBuffersWithoutLosingLetterboxing() {
        Game activity = Robolectric.buildActivity(Game.class).get();
        for (UpscalingPolicy.Mode mode : UpscalingPolicy.Mode.values()) {
            for (boolean stretch : new boolean[] {false, true}) {
                PreferenceConfiguration prefs = new PreferenceConfiguration();
                prefs.width = 1280;
                prefs.height = 720;
                prefs.fps = 60;
                prefs.upscalingMode = mode;
                prefs.setVideoScaling(stretch ? VideoScaling.STRETCH : VideoScaling.FIT);
                int[] bufferSize = new int[2];
                SurfaceHolder holder = new org.robolectric.shadows.ShadowSurfaceView.FakeSurfaceHolder() {
                    @Override public void setFixedSize(int width, int height) {
                        bufferSize[0] = width;
                        bufferSize[1] = height;
                    }
                };
                StreamView view = new StreamView(activity) {
                    @Override public SurfaceHolder getHolder() { return holder; }
                };
                ReflectionHelpers.setField(activity, "prefConfig", prefs);
                ReflectionHelpers.setField(activity, "streamView", view);
                ReflectionHelpers.setField(activity, "renderView", view);
                ReflectionHelpers.callInstanceMethod(activity.videoOutput, "prepareDisplayForRendering");
                assertArrayEquals(mode == UpscalingPolicy.Mode.OFF ? new int[] {1280, 720} : new int[2],
                        bufferSize);
                view.measure(View.MeasureSpec.makeMeasureSpec(2400, View.MeasureSpec.EXACTLY),
                        View.MeasureSpec.makeMeasureSpec(1080, View.MeasureSpec.EXACTLY));
                assertEquals(stretch ? 2400 : 1920, view.getMeasuredWidth());
                assertEquals(1080, view.getMeasuredHeight());
            }
        }
    }

    @Test
    public void aNewLaunchIntentReplacesTheExistingSingleTaskStream() {
        ActivityController<Game> controller = Robolectric.buildActivity(Game.class).create();
        Intent nextStream = new Intent(controller.get(), Game.class)
                .putExtra(Game.EXTRA_HOST, "192.0.2.2")
                .putExtra(Game.EXTRA_APP_ID, 42);
        try {
            controller.newIntent(nextStream);
            Intent launched = org.robolectric.Shadows.shadowOf(controller.get()).getNextStartedActivity();
            assertNotNull(launched);
            assertEquals("192.0.2.2", launched.getStringExtra(Game.EXTRA_HOST));
            assertEquals(42, launched.getIntExtra(Game.EXTRA_APP_ID, 0));
            assertTrue(controller.get().isFinishing());
        } finally {
            controller.destroy();
        }
    }

    @Test
    public void rootKeyboardCallbacksUseTheMainThread() throws Exception {
        Game activity = Robolectric.buildActivity(Game.class).get();
        AtomicReference<Thread> callbackThread = new AtomicReference<>();
        ReflectionHelpers.setField(activity, "keyboardTranslator", new KeyboardTranslator() {
            @Override public short translate(int keycode, int deviceId) {
                callbackThread.set(Thread.currentThread());
                return 0;
            }
        });
        Thread inputThread = new Thread(() -> activity.keyboardEvent(true, (short) 29));
        inputThread.start();
        inputThread.join(2000);
        assertFalse(inputThread.isAlive());
        org.robolectric.shadows.ShadowLooper.idleMainLooper();
        assertSame(Looper.getMainLooper().getThread(), callbackThread.get());
    }

    @Test
    public void stoppingAStreamReleasesPowerLocksBeforeTheErrorDialogIsClosed() {
        ActivityController<Game> controller = Robolectric.buildActivity(Game.class).create();
        Game activity = controller.get();
        WifiManager wifi = (WifiManager) activity.getApplicationContext().getSystemService(Context.WIFI_SERVICE);
        WifiManager.WifiLock highPerf = wifi.createWifiLock(WifiManager.WIFI_MODE_FULL_HIGH_PERF, "test-high");
        WifiManager.WifiLock lowLatency = wifi.createWifiLock(WifiManager.WIFI_MODE_FULL_LOW_LATENCY, "test-low");
        highPerf.acquire();
        lowLatency.acquire();
        ReflectionHelpers.setField(activity, "highPerfWifiLock", highPerf);
        ReflectionHelpers.setField(activity, "lowLatencyWifiLock", lowLatency);
        activity.getWindow().addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
        try {
            ReflectionHelpers.callInstanceMethod(activity.streamConnection, "stopConnection");
            assertFalse(highPerf.isHeld());
            assertFalse(lowLatency.isHeld());
            assertEquals(0, activity.getWindow().getAttributes().flags & WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
            ReflectionHelpers.callInstanceMethod(activity.streamConnection, "stopConnection");
        } finally {
            controller.destroy();
        }
    }
}
