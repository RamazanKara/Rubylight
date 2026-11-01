package com.limelight.binding.input.virtual_controller;

import android.app.Application;
import android.content.Context;
import android.os.SystemClock;
import android.view.MotionEvent;
import android.widget.FrameLayout;

import com.google.android.material.slider.Slider;
import com.limelight.R;

import org.json.JSONObject;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.LooperMode;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

/** Each classic on-screen control's own opacity: set while resizing, saved with the layout, on top of the opacity setting. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class)
@LooperMode(LooperMode.Mode.PAUSED)
public class VirtualControllerOpacityTest {
    @Test
    public void aControlsOwnOpacityScalesTheOpacitySetting() {
        assertEquals(90, VirtualControllerElement.effectiveOpacity(90, 100));
        assertEquals(45, VirtualControllerElement.effectiveOpacity(90, 50));
        assertEquals(10, VirtualControllerElement.effectiveOpacity(100, 10));

        Context context = RuntimeEnvironment.getApplication();
        DigitalButton button = button(controller(context), context);
        button.setOpacity(80);
        button.setButtonOpacity(50);
        assertEquals(40 * 255 / 100, button.getNormalColor() >>> 24);
        // The setting changes later; the control keeps its share of it.
        button.setOpacity(100);
        assertEquals(50 * 255 / 100, button.getNormalColor() >>> 24);
    }

    @Test
    public void aControlsOpacityStaysWithinTenToAHundredPercent() {
        Context context = RuntimeEnvironment.getApplication();
        DigitalButton button = button(controller(context), context);
        button.setButtonOpacity(0);
        assertEquals(VirtualControllerElement.MIN_BUTTON_OPACITY, button.getButtonOpacity());
        button.setButtonOpacity(250);
        assertEquals(VirtualControllerElement.MAX_BUTTON_OPACITY, button.getButtonOpacity());
    }

    @Test
    public void theOpacityIsSavedWithTheLayoutAndOldLayoutsFollowTheSetting() throws Exception {
        Context context = RuntimeEnvironment.getApplication();
        VirtualController controller = controller(context);
        DigitalButton saved = button(controller, context);
        saved.setButtonOpacity(35);
        JSONObject configuration = saved.getConfiguration();
        assertEquals(35, configuration.getInt("OPACITY"));

        DigitalButton loaded = button(controller, context);
        loaded.setOpacity(90);
        loaded.loadConfiguration(configuration);
        assertEquals(35, loaded.getButtonOpacity());
        assertEquals(VirtualControllerElement.effectiveOpacity(90, 35) * 255 / 100, loaded.getNormalColor() >>> 24);

        // A layout saved before per-control opacity has no OPACITY: the control follows the setting alone.
        JSONObject old = new JSONObject().put("LEFT", 10).put("TOP", 20).put("WIDTH", 30).put("HEIGHT", 40);
        loaded.loadConfiguration(old);
        assertEquals(VirtualControllerElement.MAX_BUTTON_OPACITY, loaded.getButtonOpacity());
        assertEquals(90 * 255 / 100, loaded.getNormalColor() >>> 24);
    }

    @Test
    public void resizingShowsTheSliderForTheTouchedControl() {
        Context context = RuntimeEnvironment.getApplication();
        VirtualController controller = controller(context);
        DigitalButton first = button(controller, context);
        DigitalButton second = button(controller, context);
        assertNull(controller.getOpacityPanel());

        controller.setControllerMode(VirtualController.ControllerMode.MoveButtons);
        assertNull("moving has no opacity slider", controller.getOpacityPanel());
        controller.setControllerMode(VirtualController.ControllerMode.ResizeButtons);
        assertNotNull(controller.getOpacityPanel());
        Slider slider = controller.getOpacityPanel().findViewById(R.id.osc_opacity_slider);
        assertFalse("nothing to set before a control is touched", slider.isEnabled());
        controller.setEditingOpacity(20);
        assertEquals(VirtualControllerElement.MAX_BUTTON_OPACITY, first.getButtonOpacity());

        touchDown(second);
        assertTrue(slider.isEnabled());
        assertTrue(controller.isSelectedForEditing(second));
        controller.setEditingOpacity(40);
        assertEquals(40, second.getButtonOpacity());
        assertEquals(VirtualControllerElement.MAX_BUTTON_OPACITY, first.getButtonOpacity());

        touchDown(first);
        assertEquals(100f, slider.getValue(), 0);
        touchDown(second);
        assertEquals(40f, slider.getValue(), 0);

        // Saving leaves the editor: the panel goes and nothing stays picked.
        controller.setControllerMode(VirtualController.ControllerMode.Active);
        assertNull(controller.getOpacityPanel().getParent());
        assertFalse(controller.isSelectedForEditing(second));
        assertEquals(40, second.getButtonOpacity());
    }

    private static VirtualController controller(Context context) {
        return new VirtualController(null, new FrameLayout(context), context);
    }

    private static DigitalButton button(VirtualController controller, Context context) {
        DigitalButton button = new DigitalButton(controller, VirtualControllerElement.EID_A, 0, context);
        controller.addElement(button, 0, 0, 100, 100);
        button.layout(0, 0, 100, 100);
        return button;
    }

    private static void touchDown(DigitalButton button) {
        long now = SystemClock.uptimeMillis();
        MotionEvent down = MotionEvent.obtain(now, now, MotionEvent.ACTION_DOWN, 50, 50, 0);
        MotionEvent up = MotionEvent.obtain(now, now, MotionEvent.ACTION_UP, 50, 50, 0);
        try {
            button.dispatchTouchEvent(down);
            button.dispatchTouchEvent(up);
        } finally {
            down.recycle();
            up.recycle();
        }
    }
}
