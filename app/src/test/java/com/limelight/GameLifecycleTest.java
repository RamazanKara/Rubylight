package com.limelight;

import android.app.Application;
import android.content.Context;
import android.content.Intent;
import android.net.wifi.WifiManager;
import android.os.Looper;
import android.view.WindowManager;

import com.limelight.binding.input.KeyboardTranslator;

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
            ReflectionHelpers.callInstanceMethod(activity, "stopConnection");
            assertFalse(highPerf.isHeld());
            assertFalse(lowLatency.isHeld());
            assertEquals(0, activity.getWindow().getAttributes().flags & WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
            ReflectionHelpers.callInstanceMethod(activity, "stopConnection");
        } finally {
            controller.destroy();
        }
    }
}
