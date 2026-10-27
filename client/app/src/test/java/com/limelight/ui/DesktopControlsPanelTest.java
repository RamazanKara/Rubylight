package com.limelight.ui;

import android.app.Application;
import android.graphics.Rect;
import android.os.Looper;
import android.view.KeyEvent;
import android.view.MotionEvent;
import android.view.View;
import android.view.ViewGroup;
import android.widget.EditText;
import android.widget.FrameLayout;

import androidx.appcompat.app.AlertDialog;

import com.google.android.material.button.MaterialButton;
import com.limelight.R;
import com.limelight.binding.input.DesktopControlsConfig;
import com.limelight.binding.input.KeyboardShortcuts;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowDialog;

import java.time.Duration;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;

import static org.junit.Assert.*;
import static org.robolectric.Shadows.shadowOf;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class DesktopControlsPanelTest {
    @Test public void panelPaddingAndTheGapBetweenControlsDoNotReachTheStreamOrTakeFocus() {
        try (ActivityController<UiNavigationTest.Screen> screen = Robolectric.buildActivity(UiNavigationTest.Screen.class).setup()) {
            FrameLayout root = new FrameLayout(screen.get());
            int[] streamTouches = {0};
            View stream = new View(screen.get());
            stream.setOnTouchListener((view, event) -> { streamTouches[0]++; return true; });
            root.addView(stream, new FrameLayout.LayoutParams(-1, -1));
            RecordingSink sink = new RecordingSink();
            DesktopControlsPanel panel = new DesktopControlsPanel(screen.get(), sink, () -> {}, () -> {});
            panel.setMode(DesktopControlsPanel.Mode.COMBINED);
            panel.setPadding(48, 12, 48, 24); // Include a landscape display cutout margin.
            root.addView(panel, new FrameLayout.LayoutParams(-1, 600, android.view.Gravity.BOTTOM));
            screen.get().setContentView(root);
            root.measure(View.MeasureSpec.makeMeasureSpec(1200, View.MeasureSpec.EXACTLY),
                    View.MeasureSpec.makeMeasureSpec(800, View.MeasureSpec.EXACTLY));
            root.layout(0, 0, 1200, 800);
            DesktopTrackpad trackpad = descendant(panel, DesktopTrackpad.class);
            View pad = (View) trackpad.getParent();
            Rect bounds = new Rect(0, 0, pad.getWidth(), pad.getHeight());
            root.offsetDescendantRectToMyCoords(pad, bounds);
            int[][] points = {{1, panel.getTop() + 100}, {1199, panel.getTop() + 100},
                    {600, 799}, {bounds.left + 1, bounds.top + 1}};
            for (int[] point : points) {
                for (int action : new int[] {MotionEvent.ACTION_DOWN, MotionEvent.ACTION_UP}) {
                    MotionEvent event = MotionEvent.obtain(0, 10, action, point[0], point[1], 0);
                    try { assertTrue(root.dispatchTouchEvent(event)); } finally { event.recycle(); }
                }
            }
            assertEquals(0, streamTouches[0]);
            assertTrue(sink.keys.isEmpty());
            assertTrue(sink.mouse.isEmpty());
            assertFalse(panel.isFocusable());
            assertFalse(pad.isFocusable());
            assertFalse(trackpad.isFocusable());
            touch(stream, MotionEvent.ACTION_DOWN);
            touch(stream, MotionEvent.ACTION_UP);
            assertEquals(2, streamTouches[0]);
        }
    }

    static final class RecordingSink implements DesktopControlsPanel.Sink {
        final List<int[]> keys = new ArrayList<>();
        final List<String> mouse = new ArrayList<>();
        @Override public void sendKeys(int[] value) { keys.add(value); }
        @Override public void move(float dx, float dy) { }
        @Override public void scroll(float vertical, float horizontal) { }
        @Override public void mouseButton(int button, boolean down) { mouse.add(button + ":" + down); }
    }

    @Test public void modifiersAreLocalOneShotAndCanCrossKeyBanks() {
        try (ActivityController<UiNavigationTest.Screen> screen = Robolectric.buildActivity(UiNavigationTest.Screen.class).setup()) {
            RecordingSink sink = new RecordingSink();
            DesktopControlsPanel panel = panel(screen, sink);
            button(panel, "Ctrl").performClick();
            assertTrue(sink.keys.isEmpty());
            button(panel, "C").performClick();
            button(panel, "A").performClick();
            assertArrayEquals(new int[] {KeyEvent.KEYCODE_CTRL_LEFT, KeyEvent.KEYCODE_C}, sink.keys.get(0));
            assertArrayEquals(new int[] {KeyEvent.KEYCODE_A}, sink.keys.get(1));
            button(panel, "Alt").performClick();
            selectBank(panel, "Function");
            button(panel, "F4").performClick();
            assertArrayEquals(new int[] {KeyEvent.KEYCODE_ALT_LEFT, KeyEvent.KEYCODE_F4}, sink.keys.get(2));
            button(panel, "Win").performLongClick();
            assertArrayEquals(new int[] {KeyEvent.KEYCODE_META_LEFT}, sink.keys.get(3));
        }
    }

    @Test public void explicitlyLockedModifiersPersistOnlyUntilReleaseOrModeChange() {
        try (ActivityController<UiNavigationTest.Screen> screen = Robolectric.buildActivity(UiNavigationTest.Screen.class).setup()) {
            DesktopControlsConfig config = new DesktopControlsConfig();
            config.lockModifiers = true;
            config.save(screen.get());
            RecordingSink sink = new RecordingSink();
            DesktopControlsPanel panel = panel(screen, sink);
            button(panel, "Win").performClick();
            button(panel, "D").performClick();
            button(panel, "E").performClick();
            assertArrayEquals(new int[] {KeyEvent.KEYCODE_META_LEFT, KeyEvent.KEYCODE_E}, sink.keys.get(1));
            panel.releaseInputs();
            button(panel, "A").performClick();
            assertArrayEquals(new int[] {KeyEvent.KEYCODE_A}, sink.keys.get(2));
            button(panel, "Ctrl").performClick();
            panel.setMode(DesktopControlsPanel.Mode.TRACKPAD);
            panel.setMode(DesktopControlsPanel.Mode.KEYBOARD);
            button(panel, "C").performClick();
            assertArrayEquals(new int[] {KeyEvent.KEYCODE_C}, sink.keys.get(3));
        }
    }

    @Test public void mouseHoldSurvivesPadTapAndReleasesOnModeChange() {
        try (ActivityController<UiNavigationTest.Screen> screen = Robolectric.buildActivity(UiNavigationTest.Screen.class).setup()) {
            RecordingSink sink = new RecordingSink();
            DesktopControlsPanel panel = panel(screen, sink);
            panel.setMode(DesktopControlsPanel.Mode.TRACKPAD);
            View left = described(panel, screen.get().getString(R.string.desktop_controls_left_button));
            touch(left, MotionEvent.ACTION_DOWN);
            DesktopTrackpad pad = descendant(panel, DesktopTrackpad.class);
            pad.performClick();
            shadowOf(Looper.getMainLooper()).idleFor(Duration.ofMillis(150));
            assertEquals(Arrays.asList("1:true"), sink.mouse);
            panel.setMode(DesktopControlsPanel.Mode.KEYBOARD);
            assertEquals(Arrays.asList("1:true", "1:false"), sink.mouse);
            shadowOf(Looper.getMainLooper()).idleFor(Duration.ofSeconds(1));
            assertEquals(2, sink.mouse.size());
        }
    }

    @Test public void cancellationAndFocusLossReleaseButtonsAndSelectedModifiers() {
        try (ActivityController<UiNavigationTest.Screen> screen = Robolectric.buildActivity(UiNavigationTest.Screen.class).setup()) {
            RecordingSink sink = new RecordingSink();
            DesktopControlsPanel panel = panel(screen, sink);
            panel.setMode(DesktopControlsPanel.Mode.TRACKPAD);
            View right = described(panel, screen.get().getString(R.string.desktop_controls_right_button));
            touch(right, MotionEvent.ACTION_DOWN);
            touch(right, MotionEvent.ACTION_CANCEL);
            assertEquals(Arrays.asList("3:true", "3:false"), sink.mouse);
            touch(right, MotionEvent.ACTION_DOWN);
            panel.onWindowFocusChanged(false);
            assertEquals(Arrays.asList("3:true", "3:false", "3:true", "3:false"), sink.mouse);
            panel.setMode(DesktopControlsPanel.Mode.KEYBOARD);
            button(panel, "Ctrl").performClick();
            panel.onWindowFocusChanged(false);
            button(panel, "A").performClick();
            assertArrayEquals(new int[] {KeyEvent.KEYCODE_A}, sink.keys.get(0));
        }
    }

    @Test public void compactTypingFitsAndSavedShortcutsReloadWithoutChangingTheStream() {
        try (ActivityController<UiNavigationTest.Screen> screen = Robolectric.buildActivity(UiNavigationTest.Screen.class).setup()) {
            RecordingSink sink = new RecordingSink();
            DesktopControlsPanel panel = panel(screen, sink);
            panel.measure(View.MeasureSpec.makeMeasureSpec(360, View.MeasureSpec.EXACTLY),
                    View.MeasureSpec.makeMeasureSpec(300, View.MeasureSpec.AT_MOST));
            panel.layout(0, 0, panel.getMeasuredWidth(), panel.getMeasuredHeight());
            assertEquals(264, panel.getMeasuredHeight());
            assertTrue(button(panel, "Space").isShown());
            android.graphics.Rect bounds = new android.graphics.Rect();
            assertTrue(button(panel, "Space").getGlobalVisibleRect(bounds));
            assertTrue(bounds.bottom <= panel.getHeight());
            KeyboardShortcuts.save(screen.get(), Arrays.asList(new KeyboardShortcuts.Shortcut("test", "My chord",
                    KeyboardShortcuts.parseChord("CTRL+ALT+K"))));
            panel.reloadConfiguration();
            selectBank(panel, "Shortcuts");
            button(panel, "My chord").performClick();
            assertArrayEquals(KeyboardShortcuts.parseChord("CTRL+ALT+K"), sink.keys.get(0));
        }
    }

    @Test public void combinedUsesMoreHeightOnlyBeforeAnExplicitHeightChoice() {
        try (ActivityController<UiNavigationTest.Screen> screen = Robolectric.buildActivity(UiNavigationTest.Screen.class).setup()) {
            RecordingSink sink = new RecordingSink();
            DesktopControlsPanel panel = panel(screen, sink);
            panel.setMode(DesktopControlsPanel.Mode.COMBINED);
            panel.measure(View.MeasureSpec.makeMeasureSpec(412, View.MeasureSpec.EXACTLY),
                    View.MeasureSpec.makeMeasureSpec(600, View.MeasureSpec.AT_MOST));
            assertEquals(384, panel.getMeasuredHeight());
            DesktopControlsConfig saved = panel.getConfig();
            saved.panelHeightDp = 300;
            saved.panelHeightCustomized = true;
            saved.save(screen.get());
            panel.reloadConfiguration();
            panel.measure(View.MeasureSpec.makeMeasureSpec(412, View.MeasureSpec.EXACTLY),
                    View.MeasureSpec.makeMeasureSpec(600, View.MeasureSpec.AT_MOST));
            assertEquals(300, panel.getMeasuredHeight());
        }
    }

    @Test public void shortcutEditorValidatesAndSavesWithoutLaunchingAnotherActivityOrSendingKeys() {
        try (ActivityController<UiNavigationTest.Screen> screen = Robolectric.buildActivity(UiNavigationTest.Screen.class).setup()) {
            RecordingSink sink = new RecordingSink();
            DesktopControlsPanel panel = panel(screen, sink);
            selectBank(panel, "Shortcuts");
            button(panel, screen.get().getString(R.string.desktop_controls_edit_shortcuts)).performClick();
            shadowOf(Looper.getMainLooper()).idle();
            AlertDialog list = (AlertDialog) ShadowDialog.getLatestDialog();
            list.getButton(AlertDialog.BUTTON_NEUTRAL).performClick();
            shadowOf(Looper.getMainLooper()).idle();
            AlertDialog edit = (AlertDialog) ShadowDialog.getLatestDialog();
            List<EditText> fields = new ArrayList<>();
            collect(edit.getWindow().getDecorView(), EditText.class, fields);
            assertEquals(2, fields.size());
            fields.get(0).setText("My copy");
            fields.get(1).setText("CTRL+CTRL");
            edit.getButton(AlertDialog.BUTTON_POSITIVE).performClick();
            assertNotNull(fields.get(1).getError());
            assertTrue(KeyboardShortcuts.load(screen.get()).isEmpty());
            fields.get(1).setText("CTRL+C");
            edit.getButton(AlertDialog.BUTTON_POSITIVE).performClick();
            shadowOf(Looper.getMainLooper()).idle();
            assertEquals("My copy", KeyboardShortcuts.load(screen.get()).get(0).name);
            assertTrue(sink.keys.isEmpty());
            assertTrue(sink.mouse.isEmpty());
            assertNull(shadowOf(screen.get()).getNextStartedActivity());
        }
    }

    @Test public void inlinePickersUseOneToolbarAndRetainModifiersWithoutReplacingTheKeys() {
        try (ActivityController<UiNavigationTest.Screen> screen = Robolectric.buildActivity(UiNavigationTest.Screen.class).setup()) {
            RecordingSink sink = new RecordingSink();
            DesktopControlsPanel panel = panel(screen, sink);
            MaterialButton ctrl = button(panel, "Ctrl");
            ctrl.performClick();
            openBankPicker(panel);
            assertSame(ctrl, button(panel, "Ctrl"));
            assertTrue(ctrl.isSelected());
            assertNull(ShadowDialog.getLatestDialog());
            described(panel, screen.get().getString(R.string.sheet_back)).performClick();
            assertSame(ctrl, button(panel, "Ctrl"));
            assertTrue(ctrl.isSelected());
            selectBank(panel, "Function");
            button(panel, "F4").performClick();
            assertArrayEquals(new int[] {KeyEvent.KEYCODE_CTRL_LEFT, KeyEvent.KEYCODE_F4}, sink.keys.get(0));
            assertNotNull(described(panel, screen.get().getString(R.string.desktop_controls_choose_bank, "Function")));
            described(panel, screen.get().getString(R.string.desktop_controls_choose_mode,
                    screen.get().getString(R.string.desktop_controls_keyboard))).performClick();
            button(panel, "Trackpad").performClick();
            assertEquals(DesktopControlsPanel.Mode.TRACKPAD, panel.getMode());
            assertNotNull(described(panel, screen.get().getString(R.string.desktop_controls_choose_mode, "Trackpad")));
            assertNull(described(panel, screen.get().getString(R.string.desktop_controls_choose_bank, "Function")));
            assertNotNull(described(panel, screen.get().getString(R.string.desktop_controls_system_keyboard)));
            assertEquals(300, panel.getPreferredHeightDp());
        }
    }

    @Test public void disabledSavedBankFallsBackBeforeDrawingTheToolbarAndWideCombinedStaysCompact() {
        try (ActivityController<UiNavigationTest.Screen> screen = Robolectric.buildActivity(UiNavigationTest.Screen.class).setup()) {
            DesktopControlsConfig saved = new DesktopControlsConfig();
            saved.bank = "function";
            saved.functionBank = false;
            saved.save(screen.get());
            DesktopControlsPanel panel = panel(screen, new RecordingSink());
            assertNotNull(described(panel, screen.get().getString(R.string.desktop_controls_choose_bank, "Typing")));
            assertNotNull(button(panel, "A"));
            openBankPicker(panel);
            assertNull(button(panel, "Function"));
            described(panel, screen.get().getString(R.string.sheet_back)).performClick();
            panel.setMode(DesktopControlsPanel.Mode.COMBINED);
            panel.measure(View.MeasureSpec.makeMeasureSpec(1000, View.MeasureSpec.EXACTLY),
                    View.MeasureSpec.makeMeasureSpec(600, View.MeasureSpec.AT_MOST));
            assertEquals(264, panel.getMeasuredHeight());
        }
    }

    @Test @Config(qualifiers = "w360dp-h800dp-mdpi")
    public void toolbarIconsAndSelectorArrowsFitAtTwiceTheNormalFontSize() {
        RuntimeEnvironment.setFontScale(2f);
        try (ActivityController<UiNavigationTest.Screen> screen = Robolectric.buildActivity(UiNavigationTest.Screen.class).setup()) {
            DesktopControlsPanel panel = panel(screen, new RecordingSink());
            panel.measure(View.MeasureSpec.makeMeasureSpec(360, View.MeasureSpec.EXACTLY),
                    View.MeasureSpec.makeMeasureSpec(800, View.MeasureSpec.AT_MOST));
            panel.layout(0, 0, 360, panel.getMeasuredHeight());
            ViewGroup toolbar = (ViewGroup) panel.getChildAt(0);
            assertEquals(5, toolbar.getChildCount());
            for (int i = 0; i < toolbar.getChildCount(); i++) {
                MaterialButton control = (MaterialButton) toolbar.getChildAt(i);
                assertNotNull(control.getContentDescription());
                assertNotNull("Missing icon for " + control.getContentDescription(), control.getIcon());
                assertTrue(control.getIconSize() > 0);
                assertTrue(control.getIconSize() <= control.getWidth());
                assertTrue(control.getIconSize() <= control.getHeight());
                assertTrue(control.getLeft() >= 0 && control.getRight() <= toolbar.getWidth());
            }
        }
    }

    static void selectBank(DesktopControlsPanel panel, String bank) {
        openBankPicker(panel);
        MaterialButton choice = button(panel, bank);
        assertNotNull("Missing bank " + bank, choice);
        choice.performClick();
    }

    private static void openBankPicker(DesktopControlsPanel panel) {
        ViewGroup toolbar = (ViewGroup) panel.getChildAt(0);
        for (int i = 0; i < toolbar.getChildCount(); i++) {
            View child = toolbar.getChildAt(i);
            if (!(child instanceof MaterialButton)) continue;
            String text = ((MaterialButton) child).getText().toString();
            if (panel.getContext().getString(R.string.desktop_controls_choose_bank, text)
                    .contentEquals(child.getContentDescription())) {
                child.performClick();
                return;
            }
        }
        fail("Missing key-bank selector");
    }

    private static DesktopControlsPanel panel(ActivityController<UiNavigationTest.Screen> screen, RecordingSink sink) {
        DesktopControlsPanel panel = new DesktopControlsPanel(screen.get(), sink, () -> { }, () -> { });
        screen.get().setContentView(panel);
        return panel;
    }

    private static void touch(View view, int action) {
        MotionEvent event = MotionEvent.obtain(0, 10, action, 4, 4, 0);
        try { view.dispatchTouchEvent(event); } finally { event.recycle(); }
    }

    static MaterialButton button(ViewGroup group, String text) {
        for (int i = 0; i < group.getChildCount(); i++) {
            View child = group.getChildAt(i);
            if (child instanceof MaterialButton && ((MaterialButton) child).getText().toString().equals(text)) return (MaterialButton) child;
            if (child instanceof ViewGroup) {
                MaterialButton found = button((ViewGroup) child, text);
                if (found != null) return found;
            }
        }
        return null;
    }

    static View described(ViewGroup group, String text) {
        for (int i = 0; i < group.getChildCount(); i++) {
            View child = group.getChildAt(i);
            if (text.contentEquals(child.getContentDescription() == null ? "" : child.getContentDescription())) return child;
            if (child instanceof ViewGroup) {
                View found = described((ViewGroup) child, text);
                if (found != null) return found;
            }
        }
        return null;
    }

    static <T extends View> T descendant(ViewGroup group, Class<T> type) {
        for (int i = 0; i < group.getChildCount(); i++) {
            View child = group.getChildAt(i);
            if (type.isInstance(child)) return type.cast(child);
            if (child instanceof ViewGroup) {
                T found = descendant((ViewGroup) child, type);
                if (found != null) return found;
            }
        }
        return null;
    }

    private static <T extends View> void collect(View view, Class<T> type, List<T> results) {
        if (type.isInstance(view)) results.add(type.cast(view));
        if (view instanceof ViewGroup) {
            ViewGroup group = (ViewGroup) view;
            for (int i = 0; i < group.getChildCount(); i++) collect(group.getChildAt(i), type, results);
        }
    }
}
