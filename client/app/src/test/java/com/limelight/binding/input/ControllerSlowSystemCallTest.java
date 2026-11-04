package com.limelight.binding.input;

import android.app.Application;
import android.hardware.BatteryState;
import android.hardware.lights.LightsManager;
import android.os.Handler;
import android.os.Vibrator;
import android.view.InputDevice;
import com.limelight.nvstream.NvConnection;
import com.limelight.nvstream.StreamConfiguration;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.preferences.PreferenceConfiguration;
import org.cgutman.shieldcontrollerextensions.SceManager;
import org.junit.After;
import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.Implementation;
import org.robolectric.annotation.Implements;
import org.robolectric.shadow.api.Shadow;
import org.robolectric.util.ReflectionHelpers;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import static org.junit.Assert.*;

/**
 * nightjar#73: on a OnePlus Pad 2 InputDevice.getBatteryState() blocked for seconds while the battery
 * poll held the context's lock, so Game.onPause waited for it in destroy() until Android reported
 * an ANR, on quitting and on leaving for Home. The lock must never be held across these calls.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 34, application = Application.class,
        shadows = {com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
                ControllerSlowSystemCallTest.SlowDevice.class},
        instrumentedPackages = "com.limelight.nvstream.jni")
public class ControllerSlowSystemCallTest {
    /** An input device whose system calls block until the test lets them go. */
    @Implements(InputDevice.class)
    public static class SlowDevice {
        static volatile CountDownLatch entered;
        static volatile CountDownLatch released;
        static volatile boolean batteryPresent;

        static void block() {
            entered.countDown();
            try {
                released.await();
            }
            catch (InterruptedException e) {
                Thread.currentThread().interrupt();
            }
        }

        @Implementation
        protected BatteryState getBatteryState() {
            block();
            return new BatteryState() {
                @Override public int getStatus() { return STATUS_CHARGING; }
                @Override public float getCapacity() { return 0.5f; }
                @Override public boolean isPresent() { return batteryPresent; }
            };
        }

        @Implementation
        protected LightsManager getLightsManager() {
            block();
            throw new IllegalStateException("the device went away");
        }
    }

    private static class RecordingConnection extends NvConnection {
        final List<byte[]> batteryEvents = new ArrayList<>();

        RecordingConnection() {
            super(null, new ComputerDetails.AddressTuple("127.0.0.1", 47989), 47984, "test",
                    new StreamConfiguration.Builder().build(), null, null);
        }

        @Override
        public void sendControllerBatteryEvent(byte controllerNumber, byte batteryState, byte batteryPercentage) {
            batteryEvents.add(new byte[] {controllerNumber, batteryState, batteryPercentage});
        }
    }

    /** A Shield rumble call that blocks until the test lets it go, then leaves the pad's own vibrator to run. */
    private static class SlowSce extends SceManager {
        SlowSce(android.content.Context context) {
            super(context);
        }

        @Override
        public boolean rumble(InputDevice device, int lowFreqMotor, int highFreqMotor) {
            SlowDevice.block();
            return false;
        }
    }

    private RecordingConnection connection;
    private ControllerHandler handler;
    private ControllerHandler.InputDeviceContext context;

    @Before
    public void setUp() {
        SlowDevice.entered = new CountDownLatch(1);
        SlowDevice.released = new CountDownLatch(1);
        SlowDevice.batteryPresent = true;
        connection = new RecordingConnection();
        handler = new ControllerHandler(Robolectric.buildActivity(android.app.Activity.class).create().get(),
                connection, null, new PreferenceConfiguration());
        context = ReflectionHelpers.getField(handler, "defaultContext");
        context.assignedControllerNumber = true;
        context.inputDevice = Shadow.newInstanceOf(InputDevice.class);
    }

    @After
    public void tearDown() {
        SlowDevice.released.countDown();
        handler.destroy();
    }

    // destroy() on its own thread, so a regression fails here instead of hanging the suite.
    private void assertDestroyDoesNotWaitForTheSystemCall(Thread poll) throws InterruptedException {
        poll.start();
        assertTrue("the call was reached", SlowDevice.entered.await(10, TimeUnit.SECONDS));

        Thread main = new Thread(context::destroy);
        main.start();
        main.join(5000);
        boolean waited = main.isAlive();

        SlowDevice.released.countDown();
        main.join(5000);
        poll.join(5000);
        assertFalse("destroy() waited for a blocked system call", waited);
        assertFalse(poll.isAlive());
    }

    @Test
    public void destroyDoesNotWaitForABlockedBatteryRead() throws InterruptedException {
        assertDestroyDoesNotWaitForTheSystemCall(new Thread(context.batteryStateUpdateRunnable));
        assertTrue("nothing reaches the host for a destroyed controller", connection.batteryEvents.isEmpty());
        Handler background = ReflectionHelpers.getField(handler, "backgroundThreadHandler");
        assertFalse("a destroyed controller stops polling", background.hasCallbacks(context.batteryStateUpdateRunnable));
    }

    @Test
    public void destroyDoesNotWaitForBlockedLights() throws InterruptedException {
        assertDestroyDoesNotWaitForTheSystemCall(new Thread(context.setLedStateRunnable));
    }

    // The handler's lock is what the main thread takes on quitting, on key events and on device changes.
    @Test
    public void stopDoesNotWaitForABlockedRumbleAndTheMotorEndsWithIt() throws InterruptedException {
        android.app.Activity activity = Robolectric.buildActivity(android.app.Activity.class).create().get();
        Vibrator vibrator = (Vibrator) activity.getSystemService(android.content.Context.VIBRATOR_SERVICE);
        org.robolectric.Shadows.shadowOf(vibrator).setHasAmplitudeControl(true);
        context.vibrator = vibrator;
        context.controllerNumber = 0;
        ReflectionHelpers.setField(handler, "sceManager", new SlowSce(activity));
        java.util.Map<Integer, ControllerHandler.InputDeviceContext> pads = ReflectionHelpers.getField(handler, "inputDeviceContexts");
        pads.put(1, context);

        Thread rumble = new Thread(() -> handler.handleRumble((short) 0, (short) 0x8000, (short) 0x8000));
        rumble.start();
        assertTrue("the call was reached", SlowDevice.entered.await(10, TimeUnit.SECONDS));

        Thread main = new Thread(handler::stop);
        main.start();
        main.join(5000);
        boolean waited = main.isAlive();

        SlowDevice.released.countDown();
        main.join(5000);
        rumble.join(5000);
        assertFalse("stop() waited for a blocked rumble", waited);
        assertFalse(rumble.isAlive());
        assertFalse("the motor stops with the stream", org.robolectric.Shadows.shadowOf(vibrator).isVibrating());
    }

    @Test
    public void aBatteryReadThatReturnsStillReachesTheHostAndKeepsPolling() {
        SlowDevice.released.countDown();
        context.batteryStateUpdateRunnable.run();

        assertEquals(1, connection.batteryEvents.size());
        assertEquals(MoonBridge.LI_BATTERY_STATE_CHARGING, connection.batteryEvents.get(0)[1]);
        assertEquals(50, connection.batteryEvents.get(0)[2]);
        Handler background = ReflectionHelpers.getField(handler, "backgroundThreadHandler");
        assertTrue(background.hasCallbacks(context.batteryStateUpdateRunnable));
    }

    @Test
    public void aDeviceWithoutABatterySendsNothingAndKeepsPolling() {
        SlowDevice.batteryPresent = false;
        SlowDevice.released.countDown();
        context.batteryStateUpdateRunnable.run();

        assertTrue(connection.batteryEvents.isEmpty());
        Handler background = ReflectionHelpers.getField(handler, "backgroundThreadHandler");
        assertTrue(background.hasCallbacks(context.batteryStateUpdateRunnable));
    }
}
