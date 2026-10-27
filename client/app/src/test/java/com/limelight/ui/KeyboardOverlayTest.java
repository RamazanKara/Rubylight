package com.limelight.ui;

import android.app.Application;
import android.view.KeyEvent;
import android.view.View;
import android.view.ViewGroup;

import com.google.android.material.button.MaterialButton;
import com.limelight.R;
import com.limelight.binding.input.KeyboardShortcuts;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;

import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.atomic.AtomicInteger;

import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class KeyboardOverlayTest {
    @Test
    public void customButtonUsesTheSameShortcutDispatch() {
        try (ActivityController<UiNavigationTest.Screen> controller = Robolectric.buildActivity(UiNavigationTest.Screen.class).setup()) {
            KeyboardShortcuts.save(controller.get(), java.util.Collections.singletonList(
                    new KeyboardShortcuts.Shortcut("desktop", "Desktop", KeyboardShortcuts.parseChord("WIN + D"))));
            assertEquals(1, KeyboardShortcuts.load(controller.get()).size());
            List<int[]> sent = new ArrayList<>();
            KeyboardOverlay overlay = new KeyboardOverlay(controller.get(), sent::add, () -> { }, () -> { });
            DesktopControlsPanelTest.selectBank(overlay, controller.get().getString(R.string.desktop_controls_shortcuts));
            button(overlay, "Desktop").performClick();
            assertArrayEquals(new int[] {KeyEvent.KEYCODE_META_LEFT, KeyEvent.KEYCODE_D}, sent.get(0));
            KeyboardShortcuts.save(controller.get(), java.util.Collections.emptyList());
        }
    }

    @Test
    public void modifiersApplyToOneKeyWithoutHoldingRemoteKeys() {
        try (ActivityController<UiNavigationTest.Screen> controller = Robolectric.buildActivity(UiNavigationTest.Screen.class).setup()) {
            List<int[]> sent = new ArrayList<>();
            KeyboardOverlay overlay = new KeyboardOverlay(controller.get(), sent::add, () -> { }, () -> { });
            MaterialButton ctrl = button(overlay, "Ctrl");
            MaterialButton shift = button(overlay, "Shift");
            ctrl.performClick();
            shift.performClick();
            assertTrue(ctrl.isSelected());
            assertTrue(shift.isSelected());
            assertTrue(sent.isEmpty());
            button(overlay, "Esc").performClick();
            assertArrayEquals(new int[] {KeyEvent.KEYCODE_CTRL_LEFT, KeyEvent.KEYCODE_SHIFT_LEFT, KeyEvent.KEYCODE_ESCAPE}, sent.get(0));
            assertFalse(ctrl.isSelected());
            assertFalse(shift.isSelected());
            DesktopControlsPanelTest.selectBank(overlay, controller.get().getString(R.string.desktop_controls_function));
            button(overlay, "F12").performClick();
            assertArrayEquals(new int[] {KeyEvent.KEYCODE_F12}, sent.get(1));
            ctrl = button(overlay, "Ctrl");
            ctrl.performClick();
            overlay.resetModifiers();
            assertFalse(ctrl.isSelected());
            assertEquals(2, sent.size());
        }
    }

    @Test
    public void shortcutsKeyboardAndHideButtonsRunTheirActionsAndEscapeIsVisible() {
        try (ActivityController<UiNavigationTest.Screen> controller = Robolectric.buildActivity(UiNavigationTest.Screen.class).setup()) {
            List<int[]> sent = new ArrayList<>();
            AtomicInteger keyboard = new AtomicInteger();
            AtomicInteger hide = new AtomicInteger();
            KeyboardOverlay overlay = new KeyboardOverlay(controller.get(), sent::add, keyboard::incrementAndGet, hide::incrementAndGet);
            controller.get().setContentView(overlay);
            overlay.measure(View.MeasureSpec.makeMeasureSpec(360, View.MeasureSpec.EXACTLY),
                    View.MeasureSpec.makeMeasureSpec(640, View.MeasureSpec.AT_MOST));
            overlay.layout(0, 0, 360, overlay.getMeasuredHeight());
            assertTrue(button(overlay, "Esc").getRight() <= 360);
            DesktopControlsPanelTest.selectBank(overlay, controller.get().getString(R.string.desktop_controls_shortcuts));
            button(overlay, controller.get().getString(R.string.keyboard_task_manager)).performClick();
            button(overlay, controller.get().getString(R.string.keyboard_switch_window)).performClick();
            button(overlay, controller.get().getString(R.string.keyboard_close_window)).performClick();
            DesktopControlsPanelTest.described(overlay, controller.get().getString(R.string.desktop_controls_system_keyboard)).performClick();
            DesktopControlsPanelTest.described(overlay, controller.get().getString(R.string.desktop_controls_hide)).performClick();
            assertArrayEquals(new int[] {KeyEvent.KEYCODE_CTRL_LEFT, KeyEvent.KEYCODE_SHIFT_LEFT, KeyEvent.KEYCODE_ESCAPE}, sent.get(0));
            assertArrayEquals(new int[] {KeyEvent.KEYCODE_ALT_LEFT, KeyEvent.KEYCODE_TAB}, sent.get(1));
            assertArrayEquals(new int[] {KeyEvent.KEYCODE_ALT_LEFT, KeyEvent.KEYCODE_F4}, sent.get(2));
            assertEquals(1, keyboard.get());
            assertEquals(1, hide.get());
        }
    }

    private static MaterialButton button(ViewGroup group, String text) {
        for (int i = 0; i < group.getChildCount(); i++) {
            View child = group.getChildAt(i);
            if (child instanceof MaterialButton && ((MaterialButton) child).getText().toString().equals(text)) {
                return (MaterialButton) child;
            }
            if (child instanceof ViewGroup) {
                MaterialButton result = button((ViewGroup) child, text);
                if (result != null) return result;
            }
        }
        return null;
    }
}
