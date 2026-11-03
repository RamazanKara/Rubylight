package com.limelight.binding.input.virtual_controller;

import android.app.Application;
import android.content.Context;
import android.os.SystemClock;
import android.view.MotionEvent;
import android.widget.FrameLayout;

import com.limelight.nvstream.input.ControllerPacket;
import com.limelight.preferences.PreferenceConfiguration;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.LooperMode;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertNull;

/** The classic controller's Touchpad button: off by default, and its click reaches the report whole. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class)
@LooperMode(LooperMode.Mode.PAUSED)
public class VirtualControllerTouchpadTest {
    @Test
    public void theTouchpadButtonShowsWithItsSettingAndClicksTheTouchpad() {
        Context context = RuntimeEnvironment.getApplication();
        PreferenceConfiguration config = new PreferenceConfiguration();
        assertNull("Off by default", touchpad(layout(context, config)));
        config.showTouchpadButton = true;
        config.onlyL3R3 = true;
        assertNull("Only L3 and R3 hides it", touchpad(layout(context, config)));

        config.onlyL3R3 = false;
        VirtualController controller = layout(context, config);
        DigitalButton button = (DigitalButton) touchpad(controller);
        assertNotNull(button);
        button.layout(0, 0, 100, 100);
        long now = SystemClock.uptimeMillis();
        MotionEvent down = MotionEvent.obtain(now, now, MotionEvent.ACTION_DOWN, 50, 50, 0);
        MotionEvent up = MotionEvent.obtain(now, now + 50, MotionEvent.ACTION_UP, 50, 50, 0);
        try {
            button.dispatchTouchEvent(down);
            // Above the 16 bits the report used to keep.
            assertEquals(ControllerPacket.TOUCHPAD_FLAG, controller.getControllerInputContext().inputMap);
            button.dispatchTouchEvent(up);
            assertEquals(0, controller.getControllerInputContext().inputMap);
        } finally {
            down.recycle();
            up.recycle();
        }
    }

    private static VirtualController layout(Context context, PreferenceConfiguration config) {
        VirtualController controller = new VirtualController(null, new FrameLayout(context), context);
        VirtualControllerConfigurationLoader.createDefaultLayout(controller, context, config);
        return controller;
    }

    private static VirtualControllerElement touchpad(VirtualController controller) {
        for (VirtualControllerElement element : controller.getElements()) {
            if (element.elementId == VirtualControllerElement.EID_TOUCHPAD) return element;
        }
        return null;
    }
}
