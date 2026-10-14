package com.limelight.binding.input.virtual_controller;

import android.app.Application;
import android.content.Context;
import android.os.Looper;
import android.view.InputDevice;
import android.view.MotionEvent;
import android.view.View;
import android.widget.FrameLayout;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.Shadows;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.LooperMode;

import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;
import java.util.concurrent.TimeUnit;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class)
@LooperMode(LooperMode.Mode.PAUSED)
public class VirtualControllerElementTouchTest {
    @Test
    public void liftingTheButtonFingerReleasesItWhileAnotherFingerRemainsDown() {
        Context context = RuntimeEnvironment.getApplication();
        VirtualController controller = controller(context);
        DigitalButton button = new DigitalButton(controller, VirtualControllerElement.EID_A, 0, context);
        controller.addElement(button, 0, 0, 100, 100);
        button.layout(0, 0, 100, 100);
        List<String> transitions = new ArrayList<>();
        button.addDigitalButtonListener(new DigitalButton.DigitalButtonListener() {
            @Override public void onClick() { transitions.add("down"); }
            @Override public void onLongClick() { transitions.add("long"); }
            @Override public void onRelease() { transitions.add("up"); }
        });

        touch(button, MotionEvent.ACTION_DOWN, new int[] {7}, 50, 50);
        touch(button, pointerAction(MotionEvent.ACTION_POINTER_DOWN, 1), new int[] {7, 3},
                50, 50, 150, 150);
        touch(button, pointerAction(MotionEvent.ACTION_POINTER_UP, 0), new int[] {7, 3},
                50, 50, 150, 150);

        assertFalse(button.isPressed());
        assertEquals(Arrays.asList("down", "up"), transitions);
        Shadows.shadowOf(Looper.getMainLooper()).idleFor(4, TimeUnit.SECONDS);
        assertEquals(Arrays.asList("down", "up"), transitions);
    }

    @Test
    public void aRemainingFingerCannotTakeOverTheDpadAfterItsFingerLifts() {
        Context context = RuntimeEnvironment.getApplication();
        DigitalPad pad = pad(controller(context), context);
        List<Integer> directions = new ArrayList<>();
        pad.addDigitalPadListener(directions::add);

        touch(pad, MotionEvent.ACTION_DOWN, new int[] {7}, 10, 50);
        touch(pad, pointerAction(MotionEvent.ACTION_POINTER_DOWN, 1), new int[] {7, 3},
                10, 50, 90, 50);
        touch(pad, pointerAction(MotionEvent.ACTION_POINTER_UP, 0), new int[] {7, 3},
                10, 50, 90, 50);
        touch(pad, MotionEvent.ACTION_MOVE, new int[] {3}, 90, 50);
        assertEquals(DigitalPad.DIGITAL_PAD_DIRECTION_NO_DIRECTION, pad.direction);
        touch(pad, MotionEvent.ACTION_UP, new int[] {3}, 90, 50);

        touch(pad, MotionEvent.ACTION_DOWN, new int[] {1}, 90, 50);
        touch(pad, MotionEvent.ACTION_UP, new int[] {1}, 90, 50);
        assertEquals(Arrays.asList(DigitalPad.DIGITAL_PAD_DIRECTION_LEFT,
                DigitalPad.DIGITAL_PAD_DIRECTION_NO_DIRECTION,
                DigitalPad.DIGITAL_PAD_DIRECTION_RIGHT,
                DigitalPad.DIGITAL_PAD_DIRECTION_NO_DIRECTION), directions);
    }

    @Test
    public void theDpadTracksItsPointerIdWhenPointerIndicesChange() {
        Context context = RuntimeEnvironment.getApplication();
        DigitalPad pad = pad(controller(context), context);

        touch(pad, MotionEvent.ACTION_DOWN, new int[] {7}, 10, 50);
        touch(pad, pointerAction(MotionEvent.ACTION_POINTER_DOWN, 0), new int[] {3, 7},
                90, 50, 10, 50);
        touch(pad, MotionEvent.ACTION_MOVE, new int[] {3, 7}, 90, 50, 50, 10);
        assertEquals(DigitalPad.DIGITAL_PAD_DIRECTION_UP, pad.direction);
        touch(pad, pointerAction(MotionEvent.ACTION_POINTER_UP, 0), new int[] {3, 7},
                90, 50, 50, 10);
        assertEquals(DigitalPad.DIGITAL_PAD_DIRECTION_UP, pad.direction);
        touch(pad, MotionEvent.ACTION_UP, new int[] {7}, 50, 10);
        assertEquals(DigitalPad.DIGITAL_PAD_DIRECTION_NO_DIRECTION, pad.direction);
    }

    @Test
    public void releasingControllerInputDiscardsTheRestOfTheTouchGesture() {
        Context context = RuntimeEnvironment.getApplication();
        VirtualController controller = controller(context);
        DigitalPad pad = pad(controller, context);

        touch(pad, MotionEvent.ACTION_DOWN, new int[] {7}, 10, 50);
        controller.releaseInput();
        assertEquals(DigitalPad.DIGITAL_PAD_DIRECTION_NO_DIRECTION, pad.direction);
        touch(pad, MotionEvent.ACTION_MOVE, new int[] {7}, 90, 50);
        assertEquals(DigitalPad.DIGITAL_PAD_DIRECTION_NO_DIRECTION, pad.direction);
        touch(pad, MotionEvent.ACTION_UP, new int[] {7}, 90, 50);
        touch(pad, MotionEvent.ACTION_DOWN, new int[] {2}, 90, 50);
        assertEquals(DigitalPad.DIGITAL_PAD_DIRECTION_RIGHT, pad.direction);
        touch(pad, MotionEvent.ACTION_CANCEL, new int[] {2}, 90, 50);
        assertEquals(DigitalPad.DIGITAL_PAD_DIRECTION_NO_DIRECTION, pad.direction);
    }

    @Test
    public void separateControlsCanStillBeHeldAndReleasedIndependently() {
        Context context = RuntimeEnvironment.getApplication();
        FrameLayout layout = new FrameLayout(context);
        layout.setMotionEventSplittingEnabled(true);
        VirtualController controller = new VirtualController(null, layout, context);
        DigitalPad pad = pad(controller, context);
        DigitalButton button = new DigitalButton(controller, VirtualControllerElement.EID_A, 0, context);
        controller.addElement(button, 150, 0, 100, 100);
        layout.layout(0, 0, 300, 100);
        pad.layout(0, 0, 100, 100);
        button.layout(150, 0, 250, 100);

        touch(layout, MotionEvent.ACTION_DOWN, new int[] {7}, 10, 50);
        touch(layout, pointerAction(MotionEvent.ACTION_POINTER_DOWN, 1), new int[] {7, 3},
                10, 50, 200, 50);
        assertEquals(DigitalPad.DIGITAL_PAD_DIRECTION_LEFT, pad.direction);
        assertTrue(button.isPressed());
        touch(layout, pointerAction(MotionEvent.ACTION_POINTER_UP, 0), new int[] {7, 3},
                10, 50, 200, 50);
        assertEquals(DigitalPad.DIGITAL_PAD_DIRECTION_NO_DIRECTION, pad.direction);
        assertTrue(button.isPressed());
        touch(layout, MotionEvent.ACTION_UP, new int[] {3}, 200, 50);
        assertFalse(button.isPressed());
    }

    @Test
    public void cancellationAlsoReleasesButtonsHeldBySlidingFromAnotherButton() {
        Context context = RuntimeEnvironment.getApplication();
        VirtualController controller = controller(context);
        DigitalButton first = new DigitalButton(controller, VirtualControllerElement.EID_A, 0, context);
        DigitalButton second = new DigitalButton(controller, VirtualControllerElement.EID_B, 0, context);
        controller.addElement(first, 0, 0, 100, 100);
        controller.addElement(second, 100, 0, 100, 100);
        first.layout(0, 0, 100, 100);
        second.layout(100, 0, 200, 100);
        List<String> longClicks = new ArrayList<>();
        second.addDigitalButtonListener(new DigitalButton.DigitalButtonListener() {
            @Override public void onClick() { }
            @Override public void onLongClick() { longClicks.add("long"); }
            @Override public void onRelease() { }
        });

        touch(first, MotionEvent.ACTION_DOWN, new int[] {7}, 50, 50);
        touch(first, MotionEvent.ACTION_MOVE, new int[] {7}, 150, 50);
        assertTrue(first.isPressed());
        assertTrue(second.isPressed());
        controller.releaseInput();
        assertFalse(first.isPressed());
        assertFalse(second.isPressed());
        touch(first, MotionEvent.ACTION_MOVE, new int[] {7}, 150, 50);
        Shadows.shadowOf(Looper.getMainLooper()).idleFor(4, TimeUnit.SECONDS);
        assertFalse(second.isPressed());
        assertTrue(longClicks.isEmpty());
    }

    private static VirtualController controller(Context context) {
        return new VirtualController(null, new FrameLayout(context), context);
    }

    private static DigitalPad pad(VirtualController controller, Context context) {
        DigitalPad pad = new DigitalPad(controller, context);
        controller.addElement(pad, 0, 0, 100, 100);
        pad.layout(0, 0, 100, 100);
        return pad;
    }

    private static int pointerAction(int action, int index) {
        return action | (index << MotionEvent.ACTION_POINTER_INDEX_SHIFT);
    }

    private static void touch(View element, int action, int[] ids, float... xy) {
        MotionEvent.PointerProperties[] properties = new MotionEvent.PointerProperties[ids.length];
        MotionEvent.PointerCoords[] coordinates = new MotionEvent.PointerCoords[ids.length];
        for (int i = 0; i < ids.length; i++) {
            properties[i] = new MotionEvent.PointerProperties();
            properties[i].id = ids[i];
            properties[i].toolType = MotionEvent.TOOL_TYPE_FINGER;
            coordinates[i] = new MotionEvent.PointerCoords();
            coordinates[i].x = xy[i * 2];
            coordinates[i].y = xy[i * 2 + 1];
            coordinates[i].pressure = 1;
            coordinates[i].size = 1;
        }
        MotionEvent event = MotionEvent.obtain(1000, 1100, action, ids.length, properties,
                coordinates, 0, 0, 1, 1, 0, 0, InputDevice.SOURCE_TOUCHSCREEN, 0);
        try {
            element.dispatchTouchEvent(event);
        } finally {
            event.recycle();
        }
    }
}
