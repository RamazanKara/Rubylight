package com.limelight.binding.input;

import android.app.Application;
import android.content.Context;
import android.hardware.Sensor;
import android.hardware.SensorEvent;
import android.hardware.SensorEventListener;
import android.hardware.SensorManager;
import android.os.Handler;
import android.os.Looper;

import com.limelight.nvstream.NvConnection;
import com.limelight.nvstream.StreamConfiguration;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.preferences.PreferenceConfiguration;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.Implementation;
import org.robolectric.annotation.Implements;
import org.robolectric.shadow.api.Shadow;
import org.robolectric.shadows.ShadowSensor;
import org.robolectric.shadows.ShadowSensorManager;
import org.robolectric.util.ReflectionHelpers;

import java.util.ArrayList;
import java.util.Collections;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;

import static org.junit.Assert.*;

/**
 * Motion sensor events reach their listeners on a thread of their own and no longer on the main
 * thread, in order, at the rate asked for, and none reaches the host after the report that a pad
 * is at rest or after the stream ended.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class,
        shadows = {com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
                MotionSensorThreadTest.RecordingSensors.class,
                MotionSensorThreadTest.RecordingSystemSensors.class},
        instrumentedPackages = "com.limelight.nvstream.jni")
public class MotionSensorThreadTest {
    private static final long WAIT_MS = 5000;
    private static final float DEGREES = 57.2957795f;

    /** A sensor manager that remembers the handler and the sampling period of each registration. */
    @Implements(SensorManager.class)
    public static class RecordingSensors extends ShadowSensorManager {
        final Map<SensorEventListener, Handler> handlers = new HashMap<>();
        final Map<SensorEventListener, Integer> periodsUs = new HashMap<>();

        @Implementation
        @Override
        protected boolean registerListener(SensorEventListener listener, Sensor sensor, int rate, Handler handler) {
            boolean registered = super.registerListener(listener, sensor, rate, handler);
            handlers.put(listener, handler);
            periodsUs.put(listener, rate);
            return registered;
        }

        // Without a handler Android calls the listener on the main thread.
        @Implementation
        @Override
        protected boolean registerListener(SensorEventListener listener, Sensor sensor, int rate) {
            handlers.put(listener, null);
            periodsUs.put(listener, rate);
            return super.registerListener(listener, sensor, rate);
        }
    }

    /**
     * The same, named for the class of the sensor manager Android hands out. Robolectric decides a
     * new object's shadow where that object's own class is built, and decides again only when the
     * shadows named for that very class change; without this the manager kept the plain shadow
     * whenever a test before this one had asked for a sensor manager.
     */
    @Implements(className = "android.hardware.SystemSensorManager")
    public static class RecordingSystemSensors extends RecordingSensors {
    }

    /** The motion reports that reach the host, each with the thread it was sent from. */
    private static class RecordingConnection extends NvConnection {
        final List<String> motion = Collections.synchronizedList(new ArrayList<>());
        // While set, a report that moves waits here for release, as one held up in the send would.
        volatile CountDownLatch release;
        final CountDownLatch entered = new CountDownLatch(1);

        RecordingConnection() {
            super(null, new ComputerDetails.AddressTuple("127.0.0.1", 47989), 47984, "test",
                    new StreamConfiguration.Builder().build(), null, null);
        }

        @Override
        public int sendControllerMotionEvent(byte controllerNumber, byte motionType, float x, float y, float z) {
            CountDownLatch hold = release;
            if (hold != null && x != 0) {
                entered.countDown();
                try {
                    hold.await(WAIT_MS, TimeUnit.MILLISECONDS);
                } catch (InterruptedException e) {
                    Thread.currentThread().interrupt();
                }
            }
            motion.add(Thread.currentThread().getName() + ": pad " + controllerNumber + " type " + motionType
                    + " " + x + " " + y + " " + z);
            return 0;
        }
    }

    /** A stream with player 1 on a controller that has a gyroscope and an accelerometer of its own. */
    private static final class Pad {
        final ControllerInputReleaseTest.MenuGame activity =
                Robolectric.buildActivity(ControllerInputReleaseTest.MenuGame.class).get();
        final RecordingConnection connection = new RecordingConnection();
        final SensorManager sensors;
        final RecordingSensors recorded;
        final ControllerHandler handler;
        final ControllerHandler.InputDeviceContext context;

        Pad() {
            // A context of its own has its own sensor manager, standing in for the controller's.
            sensors = (SensorManager) activity.createConfigurationContext(
                    activity.getResources().getConfiguration()).getSystemService(Context.SENSOR_SERVICE);
            recorded = Shadow.extract(sensors);
            recorded.addSensor(ShadowSensor.newInstance(Sensor.TYPE_GYROSCOPE));
            recorded.addSensor(ShadowSensor.newInstance(Sensor.TYPE_ACCELEROMETER));
            handler = new ControllerHandler(activity, connection, null, new PreferenceConfiguration());
            context = ReflectionHelpers.getField(handler, "defaultContext");
            context.assignedControllerNumber = true;
            context.controllerNumber = 0;
            context.sensorManager = sensors;
            Map<Integer, ControllerHandler.InputDeviceContext> contexts =
                    ReflectionHelpers.getField(handler, "inputDeviceContexts");
            contexts.put(7, context);
        }

        /** Hands the listener an event the way the sensor does, and waits until it returned. */
        void deliver(SensorEventListener listener, SensorEvent event) throws Exception {
            CountDownLatch done = new CountDownLatch(1);
            assertTrue(recorded.handlers.get(listener).post(() -> {
                listener.onSensorChanged(event);
                done.countDown();
            }));
            assertTrue(done.await(WAIT_MS, TimeUnit.MILLISECONDS));
        }
    }

    private static SensorEvent event(float x, float y, float z) {
        SensorEvent event = ReflectionHelpers.callConstructor(SensorEvent.class,
                ReflectionHelpers.ClassParameter.from(int.class, 3));
        event.values[0] = x;
        event.values[1] = y;
        event.values[2] = z;
        return event;
    }

    private static String gyro(float x, float y, float z) {
        return MotionSensorThread.NAME + ": pad 0 type " + MoonBridge.LI_MOTION_TYPE_GYRO + " "
                + x * DEGREES + " " + y * DEGREES + " " + z * DEGREES;
    }

    private static String atRest(String thread) {
        return thread + ": pad 0 type " + MoonBridge.LI_MOTION_TYPE_GYRO + " 0.0 0.0 0.0";
    }

    @Test
    public void bothSensorsOfAPadAreHeardOnOneThreadThatIsNotTheMainThreadAtTheRateTheHostAsked() throws Exception {
        Pad pad = new Pad();
        try {
            pad.handler.handleSetMotionEventState((short) 0, MoonBridge.LI_MOTION_TYPE_GYRO, (short) 100);
            pad.handler.handleSetMotionEventState((short) 0, MoonBridge.LI_MOTION_TYPE_ACCEL, (short) 50);
            SensorEventListener gyro = pad.context.gyroListener;
            SensorEventListener accel = pad.context.accelListener;
            assertTrue(pad.recorded.hasListener(gyro));
            assertTrue(pad.recorded.hasListener(accel));

            Handler handler = pad.recorded.handlers.get(gyro);
            assertNotNull("The gyroscope was registered without a handler, which is the main thread", handler);
            assertNotSame(Looper.getMainLooper(), handler.getLooper());
            assertEquals(MotionSensorThread.NAME, handler.getLooper().getThread().getName());
            // One thread for both, so a pad's samples cannot overtake each other.
            assertSame(handler.getLooper(), pad.recorded.handlers.get(accel).getLooper());
            assertEquals(10_000, (int) pad.recorded.periodsUs.get(gyro));
            assertEquals(20_000, (int) pad.recorded.periodsUs.get(accel));

            // Samples go to the host from that thread, in the order they came, repeats left out.
            pad.deliver(gyro, event(0.5f, -0.25f, 1f));
            pad.deliver(gyro, event(0.5f, -0.25f, 1f));
            pad.deliver(accel, event(1f, 2f, 9.5f));
            pad.deliver(gyro, event(0.75f, 0f, -1f));
            assertEquals(java.util.Arrays.asList(
                    gyro(0.5f, -0.25f, 1f),
                    MotionSensorThread.NAME + ": pad 0 type " + MoonBridge.LI_MOTION_TYPE_ACCEL + " 1.0 2.0 9.5",
                    gyro(0.75f, 0f, -1f)), pad.connection.motion);
        } finally {
            pad.handler.destroy();
        }
    }

    // A sample the sensor thread was already handed when motion ends must not turn the camera
    // again after the report that the pad is at rest.
    @Test
    public void noSampleFollowsTheReportThatThePadIsAtRest() throws Exception {
        for (String ending : new String[] {"input off", "sensors off", "rate zero"}) {
            Pad pad = new Pad();
            try {
                pad.handler.handleSetMotionEventState((short) 0, MoonBridge.LI_MOTION_TYPE_GYRO, (short) 100);
                SensorEventListener gyro = pad.context.gyroListener;
                pad.deliver(gyro, event(0.5f, 0f, 0f));

                switch (ending) {
                    case "input off":
                        pad.handler.setInputEnabled(false);
                        break;
                    case "sensors off":
                        pad.handler.disableSensors();
                        break;
                    default:
                        pad.handler.handleSetMotionEventState((short) 0, MoonBridge.LI_MOTION_TYPE_GYRO, (short) 0);
                        break;
                }
                // The sensor had this one on its way.
                pad.deliver(gyro, event(2f, 2f, 2f));

                List<String> expected = new ArrayList<>();
                expected.add(gyro(0.5f, 0f, 0f));
                // Turning the rate to zero only takes the listener away; the other two stop the pad.
                if (!ending.equals("rate zero")) {
                    expected.add(atRest(Thread.currentThread().getName()));
                }
                assertEquals(ending, expected, pad.connection.motion);
            } finally {
                pad.handler.destroy();
            }
        }
    }

    // The same with the sample caught in the middle of its send: the report that the pad is at rest
    // waits for it and goes out second.
    @Test
    public void theReportThatThePadIsAtRestWaitsForASampleBeingSent() throws Exception {
        Pad pad = new Pad();
        try {
            pad.handler.handleSetMotionEventState((short) 0, MoonBridge.LI_MOTION_TYPE_GYRO, (short) 100);
            SensorEventListener gyro = pad.context.gyroListener;
            Handler sensorThread = pad.recorded.handlers.get(gyro);
            CountDownLatch release = new CountDownLatch(1);
            pad.connection.release = release;
            sensorThread.post(() -> gyro.onSensorChanged(event(1f, 0f, 0f)));
            assertTrue(pad.connection.entered.await(WAIT_MS, TimeUnit.MILLISECONDS));

            // Lets the sample through once this thread waits for it, or after a while if it never does.
            Thread main = Thread.currentThread();
            Thread releaser = new Thread(() -> {
                long deadline = System.nanoTime() + TimeUnit.MILLISECONDS.toNanos(2000);
                while (main.getState() != Thread.State.BLOCKED && System.nanoTime() < deadline) {
                    Thread.yield();
                }
                release.countDown();
            });
            releaser.start();
            pad.handler.setInputEnabled(false);
            releaser.join(WAIT_MS);

            assertEquals(java.util.Arrays.asList(gyro(1f, 0f, 0f), atRest(main.getName())), pad.connection.motion);
        } finally {
            pad.connection.release = null;
            pad.handler.destroy();
        }
    }

    @Test
    public void theThreadEndsWithTheStreamAndAStreamWithoutMotionStartsNone() throws Exception {
        Pad still = new Pad();
        MotionSensorThread unused = ReflectionHelpers.getField(still.handler, "motionSensorThread");
        assertNull(unused.handler());
        still.handler.destroy();
        assertNull(unused.handler());

        Pad pad = new Pad();
        pad.handler.handleSetMotionEventState((short) 0, MoonBridge.LI_MOTION_TYPE_GYRO, (short) 100);
        SensorEventListener gyro = pad.context.gyroListener;
        Thread thread = pad.recorded.handlers.get(gyro).getLooper().getThread();
        assertTrue(thread.isAlive());

        pad.handler.destroy();
        assertFalse(pad.recorded.hasListener(gyro));
        thread.join(WAIT_MS);
        assertFalse("The motion thread outlived the stream", thread.isAlive());
        // What comes after the end is neither registered nor sent.
        MotionSensorThread motion = ReflectionHelpers.getField(pad.handler, "motionSensorThread");
        assertFalse(motion.register(pad.sensors, pad.sensors.getDefaultSensor(Sensor.TYPE_GYROSCOPE), gyro, 10_000));
        assertFalse(pad.recorded.hasListener(gyro));
        gyro.onSensorChanged(event(3f, 3f, 3f));
        assertTrue(pad.connection.motion.isEmpty());
    }
}
