package com.limelight.binding.input;

import android.app.Activity;
import android.app.Application;
import android.view.KeyEvent;

import com.limelight.nvstream.NvConnection;
import com.limelight.nvstream.StreamConfiguration;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.input.ControllerPacket;
import com.limelight.nvstream.input.MouseButtonPacket;
import com.limelight.preferences.PreferenceConfiguration;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;

import java.util.ArrayList;
import java.util.List;
import java.util.Map;

import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class ControllerInputReleaseTest {
    @Test
    public void releasingInputClearsEveryContextAndDoesNotFuseStaleAxes() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            RecordingConnection connection = new RecordingConnection();
            ControllerHandler handler = new ControllerHandler(activity.get(), connection, null, new PreferenceConfiguration());
            Map<Integer, ControllerHandler.InputDeviceContext> devices = ReflectionHelpers.getField(handler, "inputDeviceContexts");
            Map<Integer, ControllerHandler.UsbDeviceContext> usb = ReflectionHelpers.getField(handler, "usbDeviceContexts");
            ControllerHandler.InputDeviceContext first = handler.new InputDeviceContext();
            ControllerHandler.InputDeviceContext second = handler.new InputDeviceContext();
            ControllerHandler.UsbDeviceContext directUsb = handler.new UsbDeviceContext();
            ControllerHandler.InputDeviceContext defaults = ReflectionHelpers.getField(handler, "defaultContext");
            devices.put(1, first);
            devices.put(2, second);
            usb.put(3, directUsb);
            first.pressedButtons.put(304, KeyEvent.KEYCODE_BUTTON_A);
            first.emulatingButtonFlags = 1;
            first.pendingExit = true;
            first.mouseEmulationLastInputMap = ControllerPacket.A_FLAG | ControllerPacket.B_FLAG;
            List<ControllerHandler.GenericControllerContext> contexts = List.of(first, second, directUsb, defaults);
            for (ControllerHandler.GenericControllerContext context : contexts) {
                context.assignedControllerNumber = true;
                context.controllerNumber = 0;
                context.inputMap = ControllerPacket.A_FLAG | ControllerPacket.UP_FLAG;
                context.leftTrigger = context.rightTrigger = (byte) 255;
                context.leftStickX = context.rightStickX = 10000;
                context.leftStickY = context.rightStickY = -10000;
            }
            try {
                handler.releaseInput();
                assertEquals(4, connection.states.size());
                for (int[] state : connection.states) assertArrayEquals(new int[7], state);
                assertEquals(List.of(MouseButtonPacket.BUTTON_LEFT, MouseButtonPacket.BUTTON_RIGHT), connection.mouseUps);
                for (ControllerHandler.GenericControllerContext context : contexts) {
                    assertEquals(0, context.inputMap);
                    assertEquals(0, context.leftTrigger);
                    assertEquals(0, context.rightTrigger);
                    assertEquals(0, context.leftStickX);
                    assertEquals(0, context.leftStickY);
                    assertEquals(0, context.rightStickX);
                    assertEquals(0, context.rightStickY);
                }
                assertTrue(first.pressedButtons.isEmpty());
                assertEquals(0, first.emulatingButtonFlags);
                assertFalse(first.pendingExit);
                assertTrue(ControllerHandler.updateMappedButton(first.pressedButtons, 304, KeyEvent.KEYCODE_BUTTON_A, true));
            } finally {
                usb.clear();
                handler.destroy();
            }
        }
    }

    @Test
    public void usbReportsCannotRestoreHeldInputWhileTheMenuHasFocus() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            RecordingConnection connection = new RecordingConnection();
            ControllerHandler handler = new ControllerHandler(activity.get(), connection, null, new PreferenceConfiguration());
            Map<Integer, ControllerHandler.UsbDeviceContext> usb = ReflectionHelpers.getField(handler, "usbDeviceContexts");
            ControllerHandler.UsbDeviceContext context = handler.new UsbDeviceContext();
            context.assignedControllerNumber = true;
            context.buttonMap = new ControllerButtonMap();
            usb.put(3, context);
            try {
                handler.setInputEnabled(false);
                connection.states.clear();
                handler.reportControllerState(3, ControllerPacket.A_FLAG, 1, 1, 1, 1, 1, 1);
                assertTrue(connection.states.isEmpty());
                assertEquals(0, context.leftStickX);
                handler.setInputEnabled(true);
                handler.reportControllerState(3, ControllerPacket.A_FLAG, 1, 1, 1, 1, 1, 1);
                assertEquals(1, connection.states.size());
                assertEquals(ControllerPacket.A_FLAG, connection.states.get(0)[0]);
                assertTrue(context.leftStickX != 0);
            } finally {
                usb.clear();
                handler.destroy();
            }
        }
    }

    private static class RecordingConnection extends NvConnection {
        final List<int[]> states = new ArrayList<>();
        final List<Byte> mouseUps = new ArrayList<>();

        RecordingConnection() {
            super(null, new ComputerDetails.AddressTuple("127.0.0.1", 47989), 47984, "test",
                    new StreamConfiguration.Builder().build(), null, null);
        }

        @Override public void sendControllerInput(short number, short mask, int buttons, byte leftTrigger,
                                                   byte rightTrigger, short leftX, short leftY, short rightX, short rightY) {
            states.add(new int[] {buttons, leftTrigger, rightTrigger, leftX, leftY, rightX, rightY});
        }

        @Override public void sendMouseButtonUp(byte button) {
            mouseUps.add(button);
        }
    }
}
