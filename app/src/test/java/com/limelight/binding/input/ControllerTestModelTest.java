package com.limelight.binding.input;

import android.view.KeyEvent;
import android.view.MotionEvent;

import com.limelight.nvstream.input.ControllerPacket;

import org.junit.Test;

import java.util.HashSet;
import java.util.Set;

import static org.junit.Assert.*;

public class ControllerTestModelTest {
    private static Set<Integer> axes(int... axes) {
        Set<Integer> set = new HashSet<>();
        for (int axis : axes) set.add(axis);
        return set;
    }

    @Test
    public void controllerKeysLightTheirButtonAndOthersNone() {
        assertEquals(ControllerPacket.A_FLAG, ControllerTestModel.buttonFlag(KeyEvent.KEYCODE_BUTTON_A));
        assertEquals(ControllerPacket.BACK_FLAG, ControllerTestModel.buttonFlag(KeyEvent.KEYCODE_BACK));
        assertEquals(ControllerPacket.SPECIAL_BUTTON_FLAG, ControllerTestModel.buttonFlag(KeyEvent.KEYCODE_BUTTON_MODE));
        assertEquals(ControllerTestModel.LT_FLAG, ControllerTestModel.buttonFlag(KeyEvent.KEYCODE_BUTTON_L2));
        assertEquals(ControllerPacket.LEFT_FLAG, ControllerTestModel.buttonFlag(KeyEvent.KEYCODE_DPAD_LEFT));
        assertEquals(0, ControllerTestModel.buttonFlag(KeyEvent.KEYCODE_A));
        assertEquals(0, ControllerTestModel.buttonFlag(KeyEvent.KEYCODE_DPAD_CENTER));
    }

    @Test
    public void everyChipIsADistinctButton() {
        Set<Integer> seen = new HashSet<>();
        for (int flag : ControllerTestModel.BUTTONS) {
            assertEquals("one bit per chip", 1, Integer.bitCount(flag));
            assertTrue(seen.add(flag));
        }
        // The trigger chips must not collide with protocol buttons.
        assertEquals(0, (ControllerTestModel.LT_FLAG | ControllerTestModel.RT_FLAG) & 0x3FFFFF);
    }

    @Test
    public void hatAndTriggersUseHalfTravel() {
        assertEquals(ControllerPacket.UP_FLAG | ControllerPacket.LEFT_FLAG, ControllerTestModel.hatFlags(-1, -1));
        assertEquals(ControllerPacket.DOWN_FLAG | ControllerPacket.RIGHT_FLAG, ControllerTestModel.hatFlags(1, 1));
        assertEquals(0, ControllerTestModel.hatFlags(0.4f, -0.4f));
        assertEquals(ControllerTestModel.RT_FLAG, ControllerTestModel.triggerFlags(0.5f, 0.51f));
    }

    @Test
    public void vibratorCountsMapToTheMotorsAStreamDrives() {
        assertArrayEquals(new ControllerTestModel.Motor[] {ControllerTestModel.Motor.WEAK, ControllerTestModel.Motor.STRONG},
                ControllerTestModel.motors(2));
        assertEquals(ControllerTestModel.Motor.RIGHT_TRIGGER, ControllerTestModel.motors(4)[3]);
        assertEquals(0, ControllerTestModel.motors(1).length);
        assertEquals(0, ControllerTestModel.motors(3).length);
    }

    @Test
    public void sensorsAndLightsFollowTheAndroidVersionWorkarounds() {
        assertFalse(ControllerTestModel.canProbeSensors(30, 0x054c));
        assertTrue(ControllerTestModel.canProbeSensors(31, 0x057e));
        assertFalse(ControllerTestModel.canProbeSensors(32, 0x045e));
        assertTrue(ControllerTestModel.canProbeSensors(33, 0x045e));
        assertTrue(ControllerTestModel.trustRgbLight(31, 0x054c, true));
        assertFalse(ControllerTestModel.trustRgbLight(33, 0x045e, true));
        assertTrue(ControllerTestModel.trustRgbLight(34, 0x045e, true));
        assertFalse(ControllerTestModel.trustRgbLight(34, 0x045e, false));
    }

    @Test
    public void xboxStyleAxesPutTheRightStickOnRxRyAndTriggersOnZRz() {
        ControllerTestModel.Layout layout = ControllerTestModel.layout(axes(MotionEvent.AXIS_X, MotionEvent.AXIS_Y,
                MotionEvent.AXIS_RX, MotionEvent.AXIS_RY, MotionEvent.AXIS_Z, MotionEvent.AXIS_RZ), false);
        assertEquals(MotionEvent.AXIS_RX, layout.rightStickX);
        assertEquals(MotionEvent.AXIS_Z, layout.leftTrigger);
        assertTrue(layout.triggersIdleNegative);
        assertEquals(0f, layout.trigger(-1), 0);
        assertEquals(1f, layout.trigger(1), 0);
    }

    @Test
    public void standardAxesPutTheRightStickOnZRz() {
        ControllerTestModel.Layout layout = ControllerTestModel.layout(axes(MotionEvent.AXIS_X, MotionEvent.AXIS_Y,
                MotionEvent.AXIS_Z, MotionEvent.AXIS_RZ, MotionEvent.AXIS_LTRIGGER, MotionEvent.AXIS_RTRIGGER), false);
        assertEquals(MotionEvent.AXIS_Z, layout.rightStickX);
        assertEquals(MotionEvent.AXIS_RZ, layout.rightStickY);
        assertEquals(MotionEvent.AXIS_RTRIGGER, layout.rightTrigger);
        assertFalse(layout.triggersIdleNegative);
        assertEquals(0.25f, layout.trigger(0.25f), 0);
    }

    @Test
    public void oldDualShock4DriverUsesRxRyForTriggers() {
        ControllerTestModel.Layout layout = ControllerTestModel.layout(axes(MotionEvent.AXIS_X, MotionEvent.AXIS_Y,
                MotionEvent.AXIS_RX, MotionEvent.AXIS_RY, MotionEvent.AXIS_Z, MotionEvent.AXIS_RZ), true);
        assertEquals(MotionEvent.AXIS_RX, layout.leftTrigger);
        assertEquals(MotionEvent.AXIS_Z, layout.rightStickX);
    }

    @Test
    public void aPadWithoutRightStickAxesHasNone() {
        ControllerTestModel.Layout layout = ControllerTestModel.layout(axes(MotionEvent.AXIS_X, MotionEvent.AXIS_Y), false);
        assertEquals(-1, layout.rightStickX);
        assertEquals(-1, layout.leftTrigger);
    }
}
