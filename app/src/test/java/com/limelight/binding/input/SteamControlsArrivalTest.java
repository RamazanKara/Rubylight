package com.limelight.binding.input;

import android.app.Activity;
import android.app.Application;

import com.limelight.binding.input.driver.AbstractController;
import com.limelight.binding.input.virtual_controller.SteamControls;
import com.limelight.nvstream.NvConnection;
import com.limelight.nvstream.StreamConfiguration;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.input.ControllerPacket;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.preferences.PreferenceConfiguration;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;

import java.util.ArrayList;
import java.util.List;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

/**
 * The on-screen Steam Controller controls join a phone's pad: player 1 arrives with their
 * touchpad, grip and stick touch and back buttons, and their input merges with the pad's.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class SteamControlsArrivalTest {
    private static class Connection extends NvConnection {
        final List<Integer> buttons = new ArrayList<>();
        int arrivals, arrivalButtons;
        short arrivalCaps;
        byte touchController = -1, touchType;
        int touchPointer;
        float touchX;

        Connection() {
            super(null, new ComputerDetails.AddressTuple("127.0.0.1", 47989), 47984, "test",
                    new StreamConfiguration.Builder().build(), null, null);
        }

        @Override public boolean canSendInput() { return true; }

        @Override public int sendControllerArrivalEvent(byte slot, short activeMask, byte type,
                                                       int buttons, short capabilities) {
            arrivals++;
            arrivalButtons = buttons;
            arrivalCaps = capabilities;
            return 0;
        }

        @Override public int sendControllerTouchEvent(byte number, byte eventType, int pointerId,
                                                      float x, float y, float pressure) {
            touchController = number;
            touchType = eventType;
            touchPointer = pointerId;
            touchX = x;
            return 0;
        }

        @Override public void sendControllerInput(short number, short mask, int buttons, byte leftTrigger,
                                                   byte rightTrigger, short leftX, short leftY, short rightX, short rightY) {
            this.buttons.add(buttons);
        }

        int last() { return buttons.get(buttons.size() - 1); }
    }

    /** A pad without back buttons, touchpad or grips, as a Razer Kishi. */
    private static class PlainPad extends AbstractController {
        PlainPad() {
            super(42, null, 0x045e, 0x028e);
            type = MoonBridge.LI_CTYPE_XBOX;
            supportedButtonFlags = ControllerPacket.A_FLAG | ControllerPacket.B_FLAG;
        }
        @Override public boolean start() { return true; }
        @Override public void stop() { }
        @Override public void rumble(short low, short high) { }
        @Override public void rumbleTriggers(short left, short right) { }
    }

    @Test
    public void thePadSharingPlayerOneArrivesWithTheControlsAndMergesTheirInput() {
        Connection connection = arrive(true);
        assertEquals(SteamControls.CAPABILITIES, connection.arrivalCaps & SteamControls.CAPABILITIES);
        assertEquals(ControllerPacket.A_FLAG | ControllerPacket.B_FLAG | SteamControls.BUTTONS, connection.arrivalButtons);
    }

    @Test
    public void theModernControllerCarriesTheControlsToo() {
        // nightjar#106: the modern on-screen controller offers them as well, so the pad arrives with them.
        Connection connection = arrive(true, PreferenceConfiguration.OSC_STYLE_MODERN);
        assertEquals(SteamControls.CAPABILITIES, connection.arrivalCaps & SteamControls.CAPABILITIES);
        assertEquals(ControllerPacket.A_FLAG | ControllerPacket.B_FLAG | SteamControls.BUTTONS, connection.arrivalButtons);
        Connection without = arrive(false, PreferenceConfiguration.OSC_STYLE_MODERN);
        assertEquals(0, without.arrivalCaps & SteamControls.CAPABILITIES);
    }

    @Test
    public void withoutTheSettingThePadArrivesAsItIs() {
        Connection connection = arrive(false);
        assertEquals(0, connection.arrivalCaps & SteamControls.CAPABILITIES);
        assertEquals(ControllerPacket.A_FLAG | ControllerPacket.B_FLAG, connection.arrivalButtons);
    }

    private static Connection arrive(boolean steamControls) {
        return arrive(steamControls, null);
    }

    private static Connection arrive(boolean steamControls, String oscStyle) {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            Connection connection = new Connection();
            PreferenceConfiguration preferences = new PreferenceConfiguration();
            preferences.multiController = true;
            preferences.onscreenController = true;
            preferences.oscSteamControls = steamControls;
            if (oscStyle != null) preferences.oscStyle = oscStyle;
            ControllerHandler handler = new ControllerHandler(activity.get(), connection, null, preferences);
            try {
                handler.deviceAdded(new PlainPad());
                handler.reportControllerState(42, ControllerPacket.A_FLAG, 0, 0, 0, 0, 0, 0);
                assertEquals(1, connection.arrivals);

                // The controls' back button and grip ride with the pad's A on one pad, upper bits included.
                handler.reportOscState(SteamControls.L4_FLAG | SteamControls.LEFT_GRIP_TOUCH_FLAG,
                        (short) 0, (short) 0, (short) 0, (short) 0, (byte) 0, (byte) 0);
                assertEquals(ControllerPacket.A_FLAG | SteamControls.L4_FLAG | SteamControls.LEFT_GRIP_TOUCH_FLAG,
                        connection.last());

                // A trackpad finger goes to the same pad's touchpad 0.
                float x = SteamControls.touchX(SteamControls.RIGHT_PAD, 0.5f);
                handler.reportOscTouch(MoonBridge.LI_TOUCH_EVENT_DOWN, SteamControls.pointerId(SteamControls.RIGHT_PAD),
                        x, 0.5f, 1f);
                assertEquals(0, connection.touchController);
                assertEquals(MoonBridge.LI_TOUCH_EVENT_DOWN, connection.touchType);
                assertEquals(SteamControls.pointerId(SteamControls.RIGHT_PAD), connection.touchPointer);
                assertEquals(x, connection.touchX, 0f);
                return connection;
            } finally {
                handler.destroy();
            }
        }
    }

    @Test
    public void theOnscreenPadAloneCarriesTheControls() {
        int plain = ControllerHandler.onscreenPadButtons(false, false);
        assertEquals(0, plain & ControllerPacket.TOUCHPAD_FLAG);
        assertEquals(0, plain & SteamControls.BUTTONS);
        int steam = ControllerHandler.onscreenPadButtons(false, true);
        assertEquals(SteamControls.BUTTONS, steam & SteamControls.BUTTONS);
        assertEquals(0, steam & ControllerPacket.TOUCHPAD_FLAG);
        assertEquals(ControllerPacket.TOUCHPAD_FLAG,
                ControllerHandler.onscreenPadButtons(true, true) & ControllerPacket.TOUCHPAD_FLAG);
        assertEquals(0, ControllerHandler.onscreenPadCapabilities(false));
        assertEquals(SteamControls.CAPABILITIES, ControllerHandler.onscreenPadCapabilities(true));

        // Player 1's pad carries them while the controls share its slot; as their own player they announce themselves.
        assertTrue(ControllerHandler.carriesSteamControls(true, false, (short) 0));
        assertFalse(ControllerHandler.carriesSteamControls(true, false, (short) 1));
        assertFalse(ControllerHandler.carriesSteamControls(true, true, (short) 0));
        assertFalse(ControllerHandler.carriesSteamControls(false, false, (short) 0));
    }
}
