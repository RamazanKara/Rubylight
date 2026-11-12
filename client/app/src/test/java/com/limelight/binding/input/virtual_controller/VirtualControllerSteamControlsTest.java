package com.limelight.binding.input.virtual_controller;

import android.app.Application;
import android.content.Context;
import android.os.SystemClock;
import android.view.MotionEvent;
import android.widget.FrameLayout;

import com.limelight.nvstream.input.ControllerPacket;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.preferences.PreferenceConfiguration;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.LooperMode;

import java.util.ArrayList;
import java.util.HashSet;
import java.util.List;
import java.util.Set;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

/** The classic controller's Steam Controller controls: off by default, and what each one sends. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class)
@LooperMode(LooperMode.Mode.PAUSED)
public class VirtualControllerSteamControlsTest {
    private static final int[] STEAM_IDS = {
            VirtualControllerElement.EID_L4, VirtualControllerElement.EID_R4, VirtualControllerElement.EID_L5,
            VirtualControllerElement.EID_R5, VirtualControllerElement.EID_LEFT_TRACKPAD,
            VirtualControllerElement.EID_RIGHT_TRACKPAD, VirtualControllerElement.EID_LEFT_GRIP,
            VirtualControllerElement.EID_RIGHT_GRIP};

    private static final class Touch {
        final byte type;
        final int pointer;
        final float x, y;

        Touch(byte type, int pointer, float x, float y) {
            this.type = type;
            this.pointer = pointer;
            this.x = x;
            this.y = y;
        }
    }

    @Test
    public void theControlsShowWithTheirSettingEachUnderItsOwnSavedId() {
        Context context = RuntimeEnvironment.getApplication();
        PreferenceConfiguration config = new PreferenceConfiguration();
        VirtualController off = layout(context, config);
        for (int id : STEAM_IDS) assertNull("Off by default: " + id, element(off, id));

        config.oscSteamControls = true;
        VirtualController on = layout(context, config);
        for (int id : STEAM_IDS) assertNotNull("Shown: " + id, element(on, id));
        // The layout is saved by element id, so no two controls may share one.
        Set<Integer> ids = new HashSet<>();
        for (VirtualControllerElement element : on.getElements()) {
            assertTrue("Unique id " + element.elementId, ids.add(element.elementId));
        }

        // With a physical pad for everything else, they join L3 and R3.
        config.onlyL3R3 = true;
        VirtualController l3r3 = layout(context, config);
        for (int id : STEAM_IDS) assertNotNull("Shown with only L3 and R3: " + id, element(l3r3, id));
    }

    @Test
    public void backButtonsPressTheirPaddles() {
        VirtualController controller = steamLayout();
        int[][] pairs = {
                {VirtualControllerElement.EID_L4, ControllerPacket.PADDLE2_FLAG},
                {VirtualControllerElement.EID_R4, ControllerPacket.PADDLE1_FLAG},
                {VirtualControllerElement.EID_L5, ControllerPacket.PADDLE4_FLAG},
                {VirtualControllerElement.EID_R5, ControllerPacket.PADDLE3_FLAG}};
        for (int[] pair : pairs) {
            VirtualControllerElement button = element(controller, pair[0]);
            touch(button, MotionEvent.ACTION_DOWN, 50, 50);
            assertEquals(pair[1], controller.getControllerInputContext().inputMap);
            touch(button, MotionEvent.ACTION_UP, 50, 50);
            assertEquals(0, controller.getControllerInputContext().inputMap);
        }
    }

    @Test
    public void gripTouchHoldsFromOneTapToTheNextAndLetsGoOnRelease() {
        VirtualController controller = steamLayout();
        LatchingButton left = (LatchingButton) element(controller, VirtualControllerElement.EID_LEFT_GRIP);
        LatchingButton right = (LatchingButton) element(controller, VirtualControllerElement.EID_RIGHT_GRIP);

        tap(left);
        assertTrue(left.isLatched());
        assertEquals(SteamControls.LEFT_GRIP_TOUCH_FLAG, controller.getControllerInputContext().inputMap);
        tap(right);
        assertEquals(SteamControls.LEFT_GRIP_TOUCH_FLAG | SteamControls.RIGHT_GRIP_TOUCH_FLAG,
                controller.getControllerInputContext().inputMap);
        tap(left);
        assertFalse(left.isLatched());
        assertEquals(SteamControls.RIGHT_GRIP_TOUCH_FLAG, controller.getControllerInputContext().inputMap);

        // Hiding the controls or opening the layout editor lets every grip go.
        controller.releaseInput();
        assertFalse(right.isLatched());
        assertEquals(0, controller.getControllerInputContext().inputMap);
    }

    @Test
    public void trackpadsTouchTheirOwnPadWithoutAClick() {
        VirtualController controller = steamLayout();
        List<Touch> touches = new ArrayList<>();
        controller.setTouchSink((type, pointer, x, y, pressure) -> touches.add(new Touch(type, pointer, x, y)));

        VirtualControllerElement left = element(controller, VirtualControllerElement.EID_LEFT_TRACKPAD);
        touch(left, MotionEvent.ACTION_DOWN, 100, 0);
        touch(left, MotionEvent.ACTION_MOVE, 50, 100);
        touch(left, MotionEvent.ACTION_UP, 50, 100);
        assertEquals(3, touches.size());
        assertEquals(MoonBridge.LI_TOUCH_EVENT_DOWN, touches.get(0).type);
        assertEquals(MoonBridge.LI_TOUCH_EVENT_MOVE, touches.get(1).type);
        assertEquals(MoonBridge.LI_TOUCH_EVENT_UP, touches.get(2).type);
        // The right edge of the left pad stays below the right pad's half.
        assertTrue(touches.get(0).x < 0.5f && touches.get(0).x > 0.49f);
        assertEquals(0f, touches.get(0).y, 0.001f);
        assertEquals(0.25f, touches.get(1).x, 0.001f);
        assertEquals(1f, touches.get(1).y, 0.001f);
        assertEquals(SteamControls.pointerId(SteamControls.LEFT_PAD), touches.get(0).pointer);
        // A tap is a touch only.
        assertEquals(0, controller.getControllerInputContext().inputMap);

        touches.clear();
        VirtualControllerElement right = element(controller, VirtualControllerElement.EID_RIGHT_TRACKPAD);
        touch(right, MotionEvent.ACTION_DOWN, 0, 50);
        assertEquals(0.5f, touches.get(0).x, 0.001f);
        assertEquals(SteamControls.pointerId(SteamControls.RIGHT_PAD), touches.get(0).pointer);
        // Released with the rest of the controls: the finger is lifted, not left on the pad.
        controller.releaseInput();
        assertEquals(MoonBridge.LI_TOUCH_EVENT_CANCEL, touches.get(touches.size() - 1).type);
        assertEquals(SteamControls.pointerId(SteamControls.RIGHT_PAD), touches.get(touches.size() - 1).pointer);
    }

    @Test
    public void aFingerOnAStickTouchesIt() {
        VirtualController controller = steamLayout();
        VirtualControllerElement stick = element(controller, VirtualControllerElement.EID_LS);
        touch(stick, MotionEvent.ACTION_DOWN, 50, 50);
        assertEquals(SteamControls.LEFT_STICK_TOUCH_FLAG,
                controller.getControllerInputContext().inputMap & SteamControls.LEFT_STICK_TOUCH_FLAG);
        touch(stick, MotionEvent.ACTION_UP, 50, 50);
        assertEquals(0, controller.getControllerInputContext().inputMap & SteamControls.LEFT_STICK_TOUCH_FLAG);

        // Without the setting the stick sends no touch.
        PreferenceConfiguration config = new PreferenceConfiguration();
        VirtualController plain = layout(RuntimeEnvironment.getApplication(), config);
        VirtualControllerElement plainStick = element(plain, VirtualControllerElement.EID_LS);
        touch(plainStick, MotionEvent.ACTION_DOWN, 50, 50);
        assertEquals(0, plain.getControllerInputContext().inputMap & SteamControls.LEFT_STICK_TOUCH_FLAG);
    }

    private static VirtualController steamLayout() {
        PreferenceConfiguration config = new PreferenceConfiguration();
        config.oscSteamControls = true;
        return layout(RuntimeEnvironment.getApplication(), config);
    }

    private static VirtualController layout(Context context, PreferenceConfiguration config) {
        VirtualController controller = new VirtualController(null, new FrameLayout(context), context);
        VirtualControllerConfigurationLoader.createDefaultLayout(controller, context, config);
        for (VirtualControllerElement element : controller.getElements()) element.layout(0, 0, 100, 100);
        return controller;
    }

    private static VirtualControllerElement element(VirtualController controller, int id) {
        for (VirtualControllerElement element : controller.getElements()) {
            if (element.elementId == id) return element;
        }
        return null;
    }

    private static void tap(VirtualControllerElement element) {
        touch(element, MotionEvent.ACTION_DOWN, 50, 50);
        touch(element, MotionEvent.ACTION_UP, 50, 50);
    }

    private static void touch(VirtualControllerElement element, int action, float x, float y) {
        long now = SystemClock.uptimeMillis();
        MotionEvent event = MotionEvent.obtain(now, now, action, x, y, 0);
        try {
            element.dispatchTouchEvent(event);
        } finally {
            event.recycle();
        }
    }
}
