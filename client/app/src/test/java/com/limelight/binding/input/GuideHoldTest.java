package com.limelight.binding.input;

import android.app.Application;
import android.os.Looper;
import android.view.KeyEvent;

import com.limelight.binding.input.driver.AbstractController;
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
import org.robolectric.util.ReflectionHelpers;

import java.time.Duration;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;

import static org.junit.Assert.*;
import static org.robolectric.Shadows.shadowOf;

/**
 * Hold Guide to open the stream menu (nightjar #61, #63): held past the hold time the stream menu
 * opens and Guide never reaches the host; let go before it, Guide goes out as a short tap; other
 * buttons are never held back; Off sends Guide at once as before.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class GuideHoldTest {
    private static final int GUIDE = ControllerPacket.SPECIAL_BUTTON_FLAG;

    @org.junit.Before
    public void noShieldService() {
        // The tests run the main looper; the SHIELD accessory service is not on this device.
        shadowOf(org.robolectric.RuntimeEnvironment.getApplication()).declareComponentUnbindable(
                new android.content.ComponentName("com.nvidia.blakepairing", "com.nvidia.blakepairing.AccessoryService"));
    }

    @Test
    public void theGestureHoldsGuideBackUntilItsReleaseOrTheHoldTime() {
        GuideHold hold = new GuideHold();
        assertFalse("Off: Guide goes out at once", hold.down(0, false));
        assertEquals(GuideHold.Release.PASS, hold.up());

        assertTrue(hold.down(1000, false));
        assertTrue(hold.active());
        assertTrue("a key repeat keeps the same hold", hold.down(1000, true));
        assertEquals(GuideHold.Release.TAP, hold.up());
        assertFalse(hold.expire());

        assertTrue(hold.down(1000, false));
        assertTrue(hold.expire());
        assertTrue("held on past the menu", hold.active());
        assertEquals(GuideHold.Release.SWALLOW, hold.up());
        assertFalse(hold.active());

        // The menu took the release: the next press starts over instead of being swallowed.
        assertTrue(hold.down(1000, false));
        assertTrue(hold.expire());
        assertTrue(hold.down(1000, false));
        assertEquals(GuideHold.Release.TAP, hold.up());

        // The input let go (the menu opened another way, focus lost): neither menu nor tap follows.
        assertTrue(hold.down(1000, false));
        assertTrue(hold.cancelPending());
        assertFalse(hold.expire());
        assertEquals(GuideHold.Release.PASS, hold.up());
    }

    @Test
    public void aGuideWaitingWhenTheInputIsLetGoOpensNoMenu() {
        ControllerInputReleaseTest.MenuGame activity = Robolectric.buildActivity(ControllerInputReleaseTest.MenuGame.class).get();
        RecordingConnection connection = new RecordingConnection();
        ControllerHandler handler = handler(activity, connection, new PreferenceConfiguration());
        try {
            assertTrue(handler.handleButtonDown(key(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_BUTTON_MODE, 1000, 1000)));
            idle(500);
            handler.releaseInput();
            idle(2000);
            assertEquals(0, activity.menuCount);
            assertTrue(handler.handleButtonUp(key(KeyEvent.ACTION_UP, KeyEvent.KEYCODE_BUTTON_MODE, 1000, 3500)));
            idle(100);
            assertFalse(connection.sent(GUIDE));
        } finally {
            handler.destroy();
        }
    }

    @Test
    public void holdingGuideOpensTheStreamMenuWithoutSendingGuide() {
        ControllerInputReleaseTest.MenuGame activity = Robolectric.buildActivity(ControllerInputReleaseTest.MenuGame.class).get();
        RecordingConnection connection = new RecordingConnection();
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        assertEquals("on by default", 1000, prefs.guideMenuHoldMs);
        ControllerHandler handler = handler(activity, connection, prefs);
        try {
            assertTrue(handler.handleButtonDown(key(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_BUTTON_MODE, 1000, 1000)));
            idle(999);
            assertEquals(0, activity.menuCount);
            idle(1);
            assertEquals(1, activity.menuCount);
            assertTrue(handler.handleButtonUp(key(KeyEvent.ACTION_UP, KeyEvent.KEYCODE_BUTTON_MODE, 1000, 2500)));
            idle(100);
            assertEquals(1, activity.menuCount);
            assertFalse("Guide never reached the host", connection.sent(GUIDE));
        } finally {
            handler.destroy();
        }
    }

    @Test
    public void aShortGuidePressReachesTheHostOnReleaseAndOtherButtonsAreNotHeldBack() {
        ControllerInputReleaseTest.MenuGame activity = Robolectric.buildActivity(ControllerInputReleaseTest.MenuGame.class).get();
        RecordingConnection connection = new RecordingConnection();
        ControllerHandler handler = handler(activity, connection, new PreferenceConfiguration());
        try {
            assertTrue(handler.handleButtonDown(key(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_BUTTON_MODE, 1000, 1000)));
            assertTrue(handler.handleButtonDown(key(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_BUTTON_A, 1100, 1100)));
            assertEquals("A goes out while Guide waits", ControllerPacket.A_FLAG, connection.last());
            assertTrue(handler.handleButtonUp(key(KeyEvent.ACTION_UP, KeyEvent.KEYCODE_BUTTON_A, 1100, 1200)));
            assertEquals(0, connection.last());

            idle(300);
            assertTrue(handler.handleButtonUp(key(KeyEvent.ACTION_UP, KeyEvent.KEYCODE_BUTTON_MODE, 1000, 1300)));
            assertEquals("the tap goes out on release", GUIDE, connection.last());
            idle(25);
            assertEquals(0, connection.last());
            idle(2000);
            assertEquals(0, activity.menuCount);
        } finally {
            handler.destroy();
        }
    }

    @Test
    public void offSendsGuideAtOnceAsBefore() {
        ControllerInputReleaseTest.MenuGame activity = Robolectric.buildActivity(ControllerInputReleaseTest.MenuGame.class).get();
        RecordingConnection connection = new RecordingConnection();
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.guideMenuHoldMs = 0;
        ControllerHandler handler = handler(activity, connection, prefs);
        try {
            assertTrue(handler.handleButtonDown(key(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_BUTTON_MODE, 1000, 1000)));
            assertEquals(GUIDE, connection.last());
            idle(3000);
            assertEquals(0, activity.menuCount);
            assertTrue(handler.handleButtonUp(key(KeyEvent.ACTION_UP, KeyEvent.KEYCODE_BUTTON_MODE, 1000, 4000)));
            assertEquals(0, connection.last());
        } finally {
            handler.destroy();
        }
    }

    // Android TV boxes and handhelds whose pad has no Guide use Back as Guide: holding that Back works too.
    @Test
    public void holdingBackAsGuideOpensTheStreamMenu() {
        ControllerInputReleaseTest.MenuGame activity = Robolectric.buildActivity(ControllerInputReleaseTest.MenuGame.class).get();
        RecordingConnection connection = new RecordingConnection();
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.backAsGuide = true;
        ControllerHandler handler = handler(activity, connection, prefs);
        try {
            assertTrue(handler.handleButtonDown(key(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_BACK, 1000, 1000)));
            idle(1000);
            assertEquals(1, activity.menuCount);
            assertTrue(handler.handleButtonUp(key(KeyEvent.ACTION_UP, KeyEvent.KEYCODE_BACK, 1000, 2200)));
            assertFalse(connection.sent(GUIDE));

            assertTrue(handler.handleButtonDown(key(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_BACK, 3000, 3000)));
            assertTrue(handler.handleButtonUp(key(KeyEvent.ACTION_UP, KeyEvent.KEYCODE_BACK, 3000, 3100)));
            assertEquals("a short Back is still Guide", GUIDE, connection.last());
            idle(25);
            assertEquals(0, connection.last());
            assertEquals(1, activity.menuCount);
        } finally {
            handler.destroy();
        }
    }

    @Test
    public void aUsbDriverPadHoldsGuideTheSameWay() {
        ControllerInputReleaseTest.MenuGame activity = Robolectric.buildActivity(ControllerInputReleaseTest.MenuGame.class).get();
        RecordingConnection connection = new RecordingConnection();
        ControllerHandler handler = new ControllerHandler(activity, connection, null, new PreferenceConfiguration());
        Map<Integer, ControllerHandler.UsbDeviceContext> usb = ReflectionHelpers.getField(handler, "usbDeviceContexts");
        ControllerHandler.UsbDeviceContext pad = handler.new UsbDeviceContext();
        pad.assignedControllerNumber = true;
        pad.buttonMap = new ControllerButtonMap();
        pad.device = new UsbController();
        usb.put(3, pad);
        try {
            usb(handler, GUIDE);
            usb(handler, GUIDE | ControllerPacket.B_FLAG);
            assertEquals("B goes out while Guide waits", ControllerPacket.B_FLAG, connection.last());
            idle(1000);
            assertEquals(1, activity.menuCount);
            usb(handler, 0);
            assertFalse(connection.sent(GUIDE));

            int before = connection.buttons.size();
            usb(handler, GUIDE);
            usb(handler, 0);
            List<Integer> tap = connection.buttons.subList(before, connection.buttons.size());
            assertEquals("a short press is a Guide tap", GUIDE, (int) tap.get(tap.size() - 2));
            assertEquals(0, connection.last());
            idle(2000);
            assertEquals(1, activity.menuCount);
        } finally {
            handler.destroy();
        }
    }

    private static ControllerHandler handler(ControllerInputReleaseTest.MenuGame activity, RecordingConnection connection,
                                             PreferenceConfiguration prefs) {
        ControllerHandler handler = new ControllerHandler(activity, connection, null, prefs);
        ControllerHandler.InputDeviceContext context = ReflectionHelpers.getField(handler, "defaultContext");
        context.assignedControllerNumber = true;
        return handler;
    }

    private static KeyEvent key(int action, int keyCode, long downTime, long eventTime) {
        return new KeyEvent(downTime, eventTime, action, keyCode, 0, 0, 0, 0);
    }

    private static void usb(ControllerHandler handler, int buttons) {
        handler.reportControllerState(3, buttons, 0, 0, 0, 0, 0, 0);
    }

    private static void idle(long ms) {
        shadowOf(Looper.getMainLooper()).idleFor(Duration.ofMillis(ms));
    }

    /** Only the buttons that reach the host. */
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

        int last() {
            return buttons.get(buttons.size() - 1);
        }

        boolean sent(int flag) {
            for (int b : buttons) if ((b & flag) != 0) return true;
            return false;
        }
    }

    private static class UsbController extends AbstractController {
        UsbController() {
            super(3, null, 0x045e, 0x02ea);
        }

        @Override public boolean start() { return true; }
        @Override public void stop() { }
        @Override public void rumble(short low, short high) { }
        @Override public void rumbleTriggers(short left, short right) { }
    }
}
