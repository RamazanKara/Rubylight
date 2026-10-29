package com.limelight.binding.input;

import android.app.Application;
import android.content.Context;
import android.hardware.Sensor;
import android.hardware.SensorManager;
import android.view.KeyEvent;

import com.limelight.nvstream.NvConnection;
import com.limelight.nvstream.StreamConfiguration;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.input.ControllerPacket;
import com.limelight.preferences.PreferenceConfiguration;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;
import org.robolectric.shadow.api.Shadow;
import org.robolectric.shadows.ShadowSensor;
import org.robolectric.shadows.ShadowSensorManager;
import org.robolectric.util.ReflectionHelpers;

import java.lang.reflect.Method;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;

import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class ControllerHandlerTest {
    /** Only what reaches the host. */
    private static class RecordingConnection extends NvConnection {
        final List<Integer> buttons = new ArrayList<>();

        RecordingConnection() {
            super(null, new ComputerDetails.AddressTuple("127.0.0.1", 47989), 47984, "test",
                    new StreamConfiguration.Builder().build(), null, null);
        }

        @Override public void sendControllerInput(short number, short mask, int buttons, byte leftTrigger,
                                                   byte rightTrigger, short leftX, short leftY, short rightX, short rightY) {
            this.buttons.add(buttons);
        }
    }

    // A chord the host may claim (its couch menu opens on L3+R3 held by default, and the chord can change) is
    // ordinary input here: both stick clicks reach the host for as long as they are held, and Nightjar opens no
    // menu of its own.
    @Test
    public void stickClicksHeldTogetherReachTheHostUnchangedAndOpenNoNightjarMenu() {
        ControllerInputReleaseTest.MenuGame activity = Robolectric.buildActivity(ControllerInputReleaseTest.MenuGame.class).get();
        RecordingConnection connection = new RecordingConnection();
        ControllerHandler handler = new ControllerHandler(activity, connection, null, new PreferenceConfiguration());
        ControllerHandler.InputDeviceContext context = ReflectionHelpers.getField(handler, "defaultContext");
        context.assignedControllerNumber = true;
        int both = ControllerPacket.LS_CLK_FLAG | ControllerPacket.RS_CLK_FLAG;
        try {
            assertTrue(handler.handleButtonDown(new KeyEvent(1000, 1000, KeyEvent.ACTION_DOWN,
                    KeyEvent.KEYCODE_BUTTON_THUMBL, 0, 0, 0, 0)));
            assertTrue(handler.handleButtonDown(new KeyEvent(1000, 1010, KeyEvent.ACTION_DOWN,
                    KeyEvent.KEYCODE_BUTTON_THUMBR, 0, 0, 0, 0)));
            assertEquals(both, connection.buttons.get(connection.buttons.size() - 1) & both);
            assertEquals("Only the stick clicks", both, (int) connection.buttons.get(connection.buttons.size() - 1));

            // Held for two seconds: the release events carry the time, as Android's do.
            assertTrue(handler.handleButtonUp(new KeyEvent(1000, 3010, KeyEvent.ACTION_UP,
                    KeyEvent.KEYCODE_BUTTON_THUMBL, 0, 0, 0, 0)));
            assertTrue(handler.handleButtonUp(new KeyEvent(1010, 3020, KeyEvent.ACTION_UP,
                    KeyEvent.KEYCODE_BUTTON_THUMBR, 0, 0, 0, 0)));
            assertEquals(0, (int) connection.buttons.get(connection.buttons.size() - 1));
            assertEquals(0, activity.menuCount);
            assertTrue(activity.steered.isEmpty());
            assertFalse(activity.isFinishing());
        } finally {
            handler.destroy();
        }
    }

    // #49: a handheld's Back (the default context, like a built-in pad, leaves Back to Android) sends
    // Guide only with Back button as Guide on, and releases it on the way up.
    @Test
    public void backAsGuideSendsGuideOnlyWhenEnabled() {
        ControllerInputReleaseTest.MenuGame activity = Robolectric.buildActivity(ControllerInputReleaseTest.MenuGame.class).get();
        RecordingConnection connection = new RecordingConnection();
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        ControllerHandler handler = new ControllerHandler(activity, connection, null, prefs);
        ControllerHandler.InputDeviceContext context = ReflectionHelpers.getField(handler, "defaultContext");
        context.assignedControllerNumber = true;
        try {
            assertFalse("Back stays Android's when off", handler.handleButtonDown(new KeyEvent(1000, 1000,
                    KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_BACK, 0, 0, 0, 0)));
            assertTrue(connection.buttons.isEmpty());

            prefs.backAsGuide = true;
            assertTrue(handler.handleButtonDown(new KeyEvent(2000, 2000, KeyEvent.ACTION_DOWN,
                    KeyEvent.KEYCODE_BACK, 0, 0, 0, 0)));
            assertEquals(ControllerPacket.SPECIAL_BUTTON_FLAG, (int) connection.buttons.get(connection.buttons.size() - 1));
            assertTrue(handler.handleButtonUp(new KeyEvent(2000, 2100, KeyEvent.ACTION_UP,
                    KeyEvent.KEYCODE_BACK, 0, 0, 0, 0)));
            assertEquals(0, (int) connection.buttons.get(connection.buttons.size() - 1));
            assertEquals(0, activity.menuCount);
            assertFalse(activity.isFinishing());
        } finally {
            handler.destroy();
        }
    }

    @Test
    public void onlyAnInjectedBackOnAHandheldCountsAsItsSystemButton() {
        int keyboard = android.view.InputDevice.SOURCE_KEYBOARD;
        assertTrue(ControllerHandler.isInjectedHandheldBack(KeyEvent.KEYCODE_BACK, 0, keyboard, true, false, true));
        // No built-in gamepad (a phone), or a real device (gamepads go through handleButtonDown).
        assertFalse(ControllerHandler.isInjectedHandheldBack(KeyEvent.KEYCODE_BACK, 0, keyboard, true, false, false));
        assertFalse(ControllerHandler.isInjectedHandheldBack(KeyEvent.KEYCODE_BACK, 0, keyboard, false, false, true));
        assertFalse(ControllerHandler.isInjectedHandheldBack(KeyEvent.KEYCODE_BACK, 0, keyboard, true, true, true));
        // Android's navigation bar and gesture, Esc's fallback Back and a mouse's right click leave the stream as before.
        assertFalse(ControllerHandler.isInjectedHandheldBack(KeyEvent.KEYCODE_BACK, KeyEvent.FLAG_VIRTUAL_HARD_KEY, keyboard, true, false, true));
        assertFalse(ControllerHandler.isInjectedHandheldBack(KeyEvent.KEYCODE_BACK, KeyEvent.FLAG_FALLBACK, keyboard, true, false, true));
        assertFalse(ControllerHandler.isInjectedHandheldBack(KeyEvent.KEYCODE_BACK, 0, android.view.InputDevice.SOURCE_MOUSE, true, false, true));
        assertFalse(ControllerHandler.isInjectedHandheldBack(KeyEvent.KEYCODE_ESCAPE, 0, keyboard, true, false, true));
    }

    @Test
    public void mergedMappedButtonsStayDownUntilTheLastPhysicalKeyReleases() {
        for (int firstReleased : new int[] {304, 305}) {
            Map<Integer, Integer> pressed = new HashMap<>();
            int target = KeyEvent.KEYCODE_BUTTON_B;
            assertTrue(ControllerHandler.updateMappedButton(pressed, 304, target, true));
            assertFalse(ControllerHandler.updateMappedButton(pressed, 305, target, true));
            assertFalse(ControllerHandler.updateMappedButton(pressed, 304, target, true));
            assertFalse(ControllerHandler.updateMappedButton(pressed, firstReleased, target, false));
            assertTrue(ControllerHandler.updateMappedButton(pressed, 609 - firstReleased, target, false));
            assertTrue(pressed.isEmpty());
        }
    }

    @Test
    public void menuAndStartShareOneHeldTarget() {
        Map<Integer, Integer> pressed = new HashMap<>();
        assertTrue(ControllerHandler.updateMappedButton(pressed, 315, KeyEvent.KEYCODE_BUTTON_START, true));
        assertFalse(ControllerHandler.updateMappedButton(pressed, 139, KeyEvent.KEYCODE_MENU, true));
        assertFalse(ControllerHandler.updateMappedButton(pressed, 315, KeyEvent.KEYCODE_BUTTON_START, false));
        assertTrue(ControllerHandler.updateMappedButton(pressed, 139, KeyEvent.KEYCODE_MENU, false));
    }

    @Test
    public void independentlyMappedTargetsAndControllersDoNotInterfere() {
        Map<Integer, Integer> first = new HashMap<>();
        Map<Integer, Integer> second = new HashMap<>();
        assertTrue(ControllerHandler.updateMappedButton(first, 304, KeyEvent.KEYCODE_BUTTON_A, true));
        assertTrue(ControllerHandler.updateMappedButton(first, 305, KeyEvent.KEYCODE_BUTTON_B, true));
        assertTrue(ControllerHandler.updateMappedButton(second, 304, KeyEvent.KEYCODE_BUTTON_A, true));
        assertTrue(ControllerHandler.updateMappedButton(first, 304, KeyEvent.KEYCODE_BUTTON_A, false));
        assertEquals(1, first.size());
        assertEquals(1, second.size());
    }

    @Test
    public void triggerFusionComparesTheFullUnsignedRange() throws Exception {
        Method maximum = ControllerHandler.class.getDeclaredMethod("maxByMagnitude", byte.class, byte.class);
        maximum.setAccessible(true);
        for (int a : new int[] {0, 1, 64, 127, 128, 192, 254, 255}) {
            for (int b : new int[] {0, 1, 64, 127, 128, 192, 254, 255}) {
                assertEquals(Math.max(a, b), ((Byte) maximum.invoke(null, (byte) a, (byte) b)) & 0xFF);
            }
        }
    }

    @Test
    public void stickFusionPreservesTheSignOfTheLargestMagnitude() throws Exception {
        Method maximum = ControllerHandler.class.getDeclaredMethod("maxByMagnitude", short.class, short.class);
        maximum.setAccessible(true);
        assertEquals((short) -32768, maximum.invoke(null, (short) 32767, (short) -32768));
        assertEquals((short) -12345, maximum.invoke(null, (short) -12345, (short) 12000));
        assertEquals((short) 12000, maximum.invoke(null, (short) -1000, (short) 12000));
    }

    // A controller gyroscope that reports only zeros hands player 1's motion to the device sensors.
    @Test
    public void aSilentControllerGyroscopeMovesPlayerOneToTheDeviceSensors() {
        ControllerInputReleaseTest.MenuGame activity = Robolectric.buildActivity(ControllerInputReleaseTest.MenuGame.class).get();
        SensorManager device = (SensorManager) activity.getSystemService(Context.SENSOR_SERVICE);
        ShadowSensorManager shadowDevice = Shadow.extract(device);
        shadowDevice.addSensor(ShadowSensor.newInstance(Sensor.TYPE_GYROSCOPE));
        shadowDevice.addSensor(ShadowSensor.newInstance(Sensor.TYPE_ACCELEROMETER));
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.gamepadMotionSensorsFallbackToDevice = true;
        ControllerHandler handler = new ControllerHandler(activity, new RecordingConnection(), null, prefs);
        ControllerHandler.InputDeviceContext context = ReflectionHelpers.getField(handler, "defaultContext");
        try {
            // Only player 1 on a controller's own sensors, and only with the fallback allowed.
            assertTrue(handler.checksGyroLiveness((short) 0, null));
            assertFalse(handler.checksGyroLiveness((short) 1, null));
            assertFalse(handler.checksGyroLiveness((short) 0, device));
            prefs.gamepadMotionSensorsFallbackToDevice = false;
            assertFalse(handler.checksGyroLiveness((short) 0, null));
            prefs.gamepadMotionSensorsFallbackToDevice = true;

            context.assignedControllerNumber = true;
            context.controllerNumber = 0;
            context.gyroReportRateHz = 100;
            context.accelReportRateHz = 100;
            Map<Integer, ControllerHandler.InputDeviceContext> contexts =
                    ReflectionHelpers.getField(handler, "inputDeviceContexts");
            contexts.put(7, context);
            handler.fallBackToDeviceSensors(context);
            assertSame(device, context.sensorManager);
            assertNotNull(context.gyroListener);
            assertNotNull(context.accelListener);
            assertTrue(shadowDevice.hasListener(context.gyroListener));
            assertTrue(shadowDevice.hasListener(context.accelListener));
            // Once on the device sensors, it stays there.
            Object gyro = context.gyroListener;
            handler.fallBackToDeviceSensors(context);
            assertSame(gyro, context.gyroListener);
        } finally {
            handler.destroy();
        }
    }

    // Through the listener the controller's sensor manager gets: only a gyroscope that never moved
    // since it was registered hands player 1 to the device; one that moved and then reads zeros
    // (a controller at rest) keeps its own.
    @Test
    public void onlyAGyroscopeThatNeverMovedFallsBackThroughItsListener() {
        // The test runs the main looper; the SHIELD accessory service is not on this device.
        org.robolectric.Shadows.shadowOf(org.robolectric.RuntimeEnvironment.getApplication()).declareComponentUnbindable(
                new android.content.ComponentName("com.nvidia.blakepairing", "com.nvidia.blakepairing.AccessoryService"));
        for (boolean movesFirst : new boolean[] {false, true}) {
            ControllerInputReleaseTest.MenuGame activity = Robolectric.buildActivity(ControllerInputReleaseTest.MenuGame.class).get();
            SensorManager device = (SensorManager) activity.getSystemService(Context.SENSOR_SERVICE);
            // A context of its own has its own sensor manager, standing in for the controller's.
            SensorManager controller = (SensorManager) activity.createConfigurationContext(
                    activity.getResources().getConfiguration()).getSystemService(Context.SENSOR_SERVICE);
            assertNotSame(device, controller);
            ShadowSensorManager shadowDevice = Shadow.extract(device);
            ShadowSensorManager shadowController = Shadow.extract(controller);
            shadowDevice.addSensor(ShadowSensor.newInstance(Sensor.TYPE_GYROSCOPE));
            shadowController.addSensor(ShadowSensor.newInstance(Sensor.TYPE_GYROSCOPE));
            PreferenceConfiguration prefs = new PreferenceConfiguration();
            prefs.gamepadMotionSensorsFallbackToDevice = true;
            ControllerHandler handler = new ControllerHandler(activity, new RecordingConnection(), null, prefs);
            ControllerHandler.InputDeviceContext context = ReflectionHelpers.getField(handler, "defaultContext");
            try {
                context.assignedControllerNumber = true;
                context.controllerNumber = 0;
                context.sensorManager = controller;
                Map<Integer, ControllerHandler.InputDeviceContext> contexts =
                        ReflectionHelpers.getField(handler, "inputDeviceContexts");
                contexts.put(7, context);
                handler.handleSetMotionEventState((short) 0, com.limelight.nvstream.jni.MoonBridge.LI_MOTION_TYPE_GYRO, (short) 100);
                android.hardware.SensorEventListener listener = context.gyroListener;
                assertNotNull(listener);
                assertTrue(shadowController.hasListener(listener));

                long t = 1_000_000_000L;
                if (movesFirst) {
                    listener.onSensorChanged(gyroEvent(t, 0.01f));
                }
                // Two seconds of exact zeros at 100 Hz.
                for (int i = 1; i <= 200; i++) {
                    listener.onSensorChanged(gyroEvent(t + i * 10_000_000L, 0f));
                }
                org.robolectric.Shadows.shadowOf(android.os.Looper.getMainLooper()).idle();
                assertSame("moves first " + movesFirst, movesFirst ? controller : device, context.sensorManager);
                // The controller's listener stays registered, or makes way for one on the device.
                assertEquals(movesFirst, shadowController.hasListener(listener));
                assertEquals(movesFirst, listener == context.gyroListener);
                if (!movesFirst) assertTrue(shadowDevice.hasListener(context.gyroListener));
            } finally {
                handler.destroy();
            }
        }
    }

    private static android.hardware.SensorEvent gyroEvent(long timestampNs, float x) {
        android.hardware.SensorEvent event = ReflectionHelpers.callConstructor(android.hardware.SensorEvent.class,
                ReflectionHelpers.ClassParameter.from(int.class, 3));
        event.values[0] = x;
        event.timestamp = timestampNs;
        return event;
    }
}
