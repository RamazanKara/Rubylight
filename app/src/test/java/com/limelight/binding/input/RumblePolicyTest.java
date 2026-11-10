package com.limelight.binding.input;

import android.app.Activity;
import android.app.Application;

import com.limelight.binding.input.driver.AbstractController;
import com.limelight.nvstream.NvConnection;
import com.limelight.nvstream.StreamConfiguration;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.preferences.PreferenceConfiguration;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;

import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class RumblePolicyTest {
    @Test
    public void disableRumbleClosesTheGate() {
        assertTrue(RumblePolicy.allows(false));
        assertFalse(RumblePolicy.allows(true));
    }

    @Test
    public void deviceFallbackStrengthScalesUnsignedLevelsAndCapsAtFull() {
        assertEquals((short) 0x7FFF, RumblePolicy.scaleMotor((short) 0xFFFE, 50));
        assertEquals((short) 0xFFFE, RumblePolicy.scaleMotor((short) 0xFFFF, 200));
        assertEquals((short) 0x2000, RumblePolicy.scaleMotor((short) 0x1000, 200));
        assertEquals(0, RumblePolicy.scaleMotor((short) 0xFFFF, 0));
        assertEquals(0, RumblePolicy.scaleMotor((short) 0xFFFF, -10));
    }

    @Test
    public void disabledRumbleReachesNoMotor() {
        assertEquals(0, rumbleCalls(true));
        // Motor rumble, trigger rumble and waveform haptics each reach the pad when allowed.
        assertEquals(3, rumbleCalls(false));
    }

    @Test
    public void removingThePadThePhoneVibratorStandsInForStopsTheVibrator() throws Exception {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            NvConnection connection = new NvConnection(null,
                    new ComputerDetails.AddressTuple("127.0.0.1", 47989), 47984, "test",
                    new StreamConfiguration.Builder().build(), null, null);
            PreferenceConfiguration config = new PreferenceConfiguration();
            ControllerHandler handler = new ControllerHandler(activity.get(), connection, null, config);
            android.os.Vibrator vibrator = (android.os.Vibrator) activity.get().getSystemService(android.content.Context.VIBRATOR_SERVICE);
            Pad pad = new Pad();
            ControllerHandler.UsbDeviceContext context = handler.new UsbDeviceContext();
            context.device = pad;
            context.assignedControllerNumber = true;
            context.controllerNumber = 0;
            usbContexts(handler).put(pad.getControllerId(), context);
            try {
                // The host's rumble for a pad without a motor went to the phone, for up to a minute.
                java.lang.reflect.Field fallback = ControllerHandler.class.getDeclaredField("deviceVibratorForPlayerOne");
                fallback.setAccessible(true);
                fallback.setBoolean(handler, true);
                vibrator.vibrate(60000);
                assertTrue(org.robolectric.Shadows.shadowOf(vibrator).isVibrating());

                handler.deviceRemoved(pad);

                assertFalse(org.robolectric.Shadows.shadowOf(vibrator).isVibrating());
            }
            finally {
                handler.destroy();
            }
        }
    }

    /** A pad whose rumble is a USB write that takes its time, as an Xbox pad's can for 100 ms. */
    private static class SlowPad extends Pad {
        final java.util.concurrent.CountDownLatch writing = new java.util.concurrent.CountDownLatch(1);
        final java.util.concurrent.CountDownLatch written = new java.util.concurrent.CountDownLatch(1);
        final java.util.List<String> events = java.util.Collections.synchronizedList(new java.util.ArrayList<>());

        @Override public void rumble(short low, short high) {
            writing.countDown();
            try {
                written.await(10, java.util.concurrent.TimeUnit.SECONDS);
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
            }
            events.add("rumble " + low);
        }

        @Override public void stop() {
            events.add("stop");
        }
    }

    @Test
    public void aSlowUsbRumbleHoldsUpNoControllerPacketAndNeverFollowsTheStop() throws Exception {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            NvConnection connection = new NvConnection(null,
                    new ComputerDetails.AddressTuple("127.0.0.1", 47989), 47984, "test",
                    new StreamConfiguration.Builder().build(), null, null);
            ControllerHandler handler = new ControllerHandler(activity.get(), connection, null,
                    new PreferenceConfiguration());
            SlowPad pad = new SlowPad();
            ControllerHandler.UsbDeviceContext context = handler.new UsbDeviceContext();
            context.device = pad;
            context.assignedControllerNumber = true;
            context.controllerNumber = 0;
            usbContexts(handler).put(pad.getControllerId(), context);
            try {
                Thread rumble = new Thread(() -> handler.handleRumble((short) 0, (short) 0x4000, (short) 0x4000));
                rumble.start();
                assertTrue("the write was reached", pad.writing.await(10, java.util.concurrent.TimeUnit.SECONDS));

                // Every controller packet asks for the active controllers under the handler's lock.
                Thread packet = new Thread(() ->
                        org.robolectric.util.ReflectionHelpers.callInstanceMethod(handler, "getActiveControllerMask"));
                packet.start();
                packet.join(5000);
                assertFalse("a controller packet waited for the USB write", packet.isAlive());

                // The stop waits for the write under way, so the pad's own stop is the last thing it gets.
                Thread stop = new Thread(handler::stop);
                stop.start();
                stop.join(300);
                assertTrue("stop() went ahead of a write under way", stop.isAlive());
                pad.written.countDown();
                stop.join(5000);
                rumble.join(5000);
                assertFalse(stop.isAlive());
                assertFalse(rumble.isAlive());

                handler.handleRumble((short) 0, (short) 0x7000, (short) 0x7000);
                handler.handleRumbleTriggers((short) 0, (short) 0x7000, (short) 0x7000);
                assertEquals(java.util.List.of("rumble 16384", "stop"), pad.events);
            }
            finally {
                pad.written.countDown();
                handler.destroy();
            }
        }
    }

    @Test
    public void usbRumblesReachThePadInTheOrderTheHostSentThem() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            NvConnection connection = new NvConnection(null,
                    new ComputerDetails.AddressTuple("127.0.0.1", 47989), 47984, "test",
                    new StreamConfiguration.Builder().build(), null, null);
            ControllerHandler handler = new ControllerHandler(activity.get(), connection, null,
                    new PreferenceConfiguration());
            SlowPad pad = new SlowPad();
            pad.written.countDown();
            SlowPad other = new SlowPad();
            other.written.countDown();
            ControllerHandler.UsbDeviceContext context = handler.new UsbDeviceContext();
            context.device = pad;
            context.assignedControllerNumber = true;
            context.controllerNumber = 0;
            usbContexts(handler).put(1, context);
            ControllerHandler.UsbDeviceContext second = handler.new UsbDeviceContext();
            second.device = other;
            second.assignedControllerNumber = true;
            second.controllerNumber = 1;
            usbContexts(handler).put(2, second);
            try {
                handler.handleRumble((short) 0, (short) 0x4000, (short) 0);
                handler.handleRumble((short) 1, (short) 0x2000, (short) 0);
                handler.handleRumble((short) 0, (short) 0, (short) 0);
                // The last one a pad got is the newest; another player's rumble is not this pad's.
                assertEquals(java.util.List.of("rumble 16384", "rumble 0"), pad.events);
                assertEquals(java.util.List.of("rumble 8192"), other.events);
            }
            finally {
                handler.destroy();
            }
        }
    }

    private static int rumbleCalls(boolean disableRumble) {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            NvConnection connection = new NvConnection(null,
                    new ComputerDetails.AddressTuple("127.0.0.1", 47989), 47984, "test",
                    new StreamConfiguration.Builder().build(), null, null);
            PreferenceConfiguration config = new PreferenceConfiguration();
            config.disableRumble = disableRumble;
            config.vibrateFallbackToDevice = true;
            ControllerHandler handler = new ControllerHandler(activity.get(), connection, null, config);
            Pad pad = new Pad();
            ControllerHandler.UsbDeviceContext context = handler.new UsbDeviceContext();
            context.device = pad;
            context.assignedControllerNumber = true;
            context.controllerNumber = 0;
            context.sendControllerArrival();
            usbContexts(handler).put(pad.getControllerId(), context);
            try {
                handler.handleRumble((short) 0, (short) 0x8000, (short) 0x4000);
                handler.handleRumbleTriggers((short) 0, (short) 0x8000, (short) 0);
                handler.handleControllerHaptics((short) 0, 1, new byte[] {1, 0, 2, 0});
                return pad.calls;
            }
            finally {
                context.destroy();
                handler.destroy();
            }
        }
    }

    @SuppressWarnings("unchecked")
    private static java.util.Map<Integer, ControllerHandler.UsbDeviceContext> usbContexts(ControllerHandler handler) {
        try {
            java.lang.reflect.Field field = ControllerHandler.class.getDeclaredField("usbDeviceContexts");
            field.setAccessible(true);
            return (java.util.Map<Integer, ControllerHandler.UsbDeviceContext>) field.get(handler);
        } catch (ReflectiveOperationException e) {
            throw new AssertionError(e);
        }
    }

    private static class Pad extends AbstractController {
        int calls;
        Pad() {
            super(0, null, 0x054c, 0x0ce6);
            capabilities = (short) (MoonBridge.LI_CCAP_RUMBLE | MoonBridge.LI_CCAP_TRIGGER_RUMBLE | MoonBridge.LI_CCAP_HAPTICS_PCM);
        }
        @Override public boolean start() { return true; }
        @Override public void stop() { }
        @Override public void rumble(short low, short high) { calls++; }
        @Override public void rumbleTriggers(short left, short right) { calls++; }
        @Override public void controllerHaptics(int sequence, byte[] pcm) { calls++; }
    }
}
