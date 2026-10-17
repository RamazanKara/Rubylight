package com.limelight.ui;

import android.app.Application;
import android.os.Looper;
import android.view.KeyEvent;
import android.view.MotionEvent;
import android.view.View;
import android.view.ViewGroup;
import android.widget.EditText;

import androidx.appcompat.app.AlertDialog;

import com.google.android.material.button.MaterialButton;
import com.limelight.R;
import com.limelight.binding.input.DesktopControlsConfig;
import com.limelight.binding.input.KeyboardShortcuts;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
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
            button(panel, "Function").performClick();
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
            assertEquals(300, panel.getMeasuredHeight());
            assertTrue(button(panel, "Space").isShown());
            android.graphics.Rect bounds = new android.graphics.Rect();
            assertTrue(button(panel, "Space").getGlobalVisibleRect(bounds));
            assertTrue(bounds.bottom <= panel.getHeight());
            KeyboardShortcuts.save(screen.get(), Arrays.asList(new KeyboardShortcuts.Shortcut("test", "My chord",
                    KeyboardShortcuts.parseChord("CTRL+ALT+K"))));
            panel.reloadConfiguration();
            button(panel, "Shortcuts").performClick();
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
            assertEquals(420, panel.getMeasuredHeight());
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
            button(panel, "Shortcuts").performClick();
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
