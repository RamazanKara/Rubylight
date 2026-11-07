package com.limelight.preferences;

import android.app.Application;
import android.preference.Preference;
import android.widget.EditText;

import androidx.appcompat.app.AlertDialog;

import com.limelight.R;
import com.limelight.binding.input.KeyboardShortcuts;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowDialog;
import org.robolectric.util.ReflectionHelpers;

import static org.junit.Assert.*;
import static org.robolectric.Shadows.shadowOf;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class KeyboardShortcutsActivityTest {
    @Test public void controlsSectionExposesTheShortcutEditor() {
        try (var controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            StreamSettings settings = controller.get();
            settings.showSection("input");
            settings.getFragmentManager().executePendingTransactions();
            StreamSettings.SettingsFragment fragment = (StreamSettings.SettingsFragment)
                    settings.getFragmentManager().findFragmentById(R.id.stream_settings);
            Preference editor = fragment.findPreference("keyboard_shortcuts_editor");
            assertNotNull(editor);
            assertTrue(editor.getOnPreferenceClickListener().onPreferenceClick(editor));
            assertEquals(KeyboardShortcutsActivity.class.getName(),
                    shadowOf(settings).getNextStartedActivity().getComponent().getClassName());
        }
    }

    private static void saveThree(android.content.Context context) {
        KeyboardShortcuts.save(context, java.util.Arrays.asList(
                new KeyboardShortcuts.Shortcut("a", "A", KeyboardShortcuts.parseChord("WIN + A")),
                new KeyboardShortcuts.Shortcut("b", "B", KeyboardShortcuts.parseChord("WIN + B")),
                new KeyboardShortcuts.Shortcut("c", "C", KeyboardShortcuts.parseChord("WIN + C"))));
    }

    private static String order(android.content.Context context) {
        StringBuilder ids = new StringBuilder();
        for (KeyboardShortcuts.Shortcut entry : KeyboardShortcuts.load(context)) ids.append(entry.id);
        return ids.toString();
    }

    private static android.view.MotionEvent touch(int action, float y) {
        return android.view.MotionEvent.obtain(0, 0, action, 0, y, 0);
    }

    /** A key's press and release, through the page as on a device, to whatever has the focus. */
    private static boolean key(android.view.View content, int keyCode) {
        // Frames between the events, as on a device: a group only passes keys to a child that has been laid out.
        org.robolectric.shadows.ShadowLooper.idleMainLooper();
        boolean down = content.dispatchKeyEvent(new android.view.KeyEvent(android.view.KeyEvent.ACTION_DOWN, keyCode));
        org.robolectric.shadows.ShadowLooper.idleMainLooper();
        boolean up = content.dispatchKeyEvent(new android.view.KeyEvent(android.view.KeyEvent.ACTION_UP, keyCode));
        return down && up;
    }

    @Test public void selectPicksAHandleUpSoUpAndDownMoveItsShortcutAndSelectOrBackPutsItDown() {
        try (var controller = Robolectric.buildActivity(KeyboardShortcutsActivity.class).setup()) {
            KeyboardShortcutsActivity activity = controller.get();
            saveThree(activity);
            ReflectionHelpers.callInstanceMethod(activity, "refresh");
            android.view.View content = activity.findViewById(android.R.id.content);
            org.robolectric.shadows.ShadowLooper.idleMainLooper();
            assertTrue(content.findViewWithTag("a").requestFocus());
            // Not picked up: the handle leaves down to the focus search, as on a TV, and the order stays.
            assertFalse(key(content, android.view.KeyEvent.KEYCODE_DPAD_DOWN));
            assertEquals("abc", order(activity));

            assertTrue(content.findViewWithTag("a").requestFocus());
            assertTrue(key(content, android.view.KeyEvent.KEYCODE_DPAD_CENTER));
            assertTrue("the first row has no way up: picked up, it stays", key(content, android.view.KeyEvent.KEYCODE_DPAD_UP));
            assertEquals("abc", order(activity));
            assertTrue(key(content, android.view.KeyEvent.KEYCODE_DPAD_DOWN));
            assertEquals("bac", order(activity));
            assertTrue(content.findViewWithTag("a").isFocused());
            assertTrue(key(content, android.view.KeyEvent.KEYCODE_DPAD_DOWN));
            assertEquals("bca", order(activity));
            assertTrue(content.findViewWithTag("a").isFocused());
            org.robolectric.shadows.ShadowLooper.idleMainLooper();
            assertTrue("a move keeps it picked up", key(content, android.view.KeyEvent.KEYCODE_DPAD_UP));
            assertEquals("bac", order(activity));

            // Back puts it down without leaving the page; then up moves the focus again.
            assertTrue(key(content, android.view.KeyEvent.KEYCODE_BACK));
            assertFalse(activity.isFinishing());
            assertFalse(key(content, android.view.KeyEvent.KEYCODE_DPAD_UP));
            assertEquals("bac", order(activity));

            // Select puts it down too.
            assertTrue(content.findViewWithTag("c").requestFocus());
            assertTrue(key(content, android.view.KeyEvent.KEYCODE_BUTTON_A));
            assertTrue(key(content, android.view.KeyEvent.KEYCODE_DPAD_UP));
            assertEquals("bca", order(activity));
            assertTrue(key(content, android.view.KeyEvent.KEYCODE_BUTTON_A));
            assertFalse(key(content, android.view.KeyEvent.KEYCODE_DPAD_UP));
            assertEquals("bca", order(activity));
        }
    }

    @Test public void talkBackMovesAShortcutWithMoveUpAndMoveDownOnItsHandle() {
        try (var controller = Robolectric.buildActivity(KeyboardShortcutsActivity.class).setup()) {
            KeyboardShortcutsActivity activity = controller.get();
            saveThree(activity);
            ReflectionHelpers.callInstanceMethod(activity, "refresh");
            android.view.View content = activity.findViewById(android.R.id.content);
            String up = activity.getString(com.limelight.R.string.keyboard_shortcuts_move_up);
            String down = activity.getString(com.limelight.R.string.keyboard_shortcuts_move_down);
            // Only the moves there is room for.
            assertEquals(java.util.Collections.singletonList(down), actions(content.findViewWithTag("a")));
            assertEquals(java.util.Arrays.asList(up, down), actions(content.findViewWithTag("b")));
            assertEquals(java.util.Collections.singletonList(up), actions(content.findViewWithTag("c")));

            assertTrue(perform(content.findViewWithTag("a"), down));
            assertEquals("bac", order(activity));
            assertTrue(perform(content.findViewWithTag("c"), up));
            assertEquals("bca", order(activity));
        }
    }

    private static java.util.List<String> actions(android.view.View handle) {
        android.view.accessibility.AccessibilityNodeInfo info = handle.createAccessibilityNodeInfo();
        java.util.List<String> labels = new java.util.ArrayList<>();
        for (android.view.accessibility.AccessibilityNodeInfo.AccessibilityAction action : info.getActionList()) {
            if (action.getLabel() != null) labels.add(action.getLabel().toString());
        }
        return labels;
    }

    private static boolean perform(android.view.View handle, String label) {
        android.view.accessibility.AccessibilityNodeInfo info = handle.createAccessibilityNodeInfo();
        for (android.view.accessibility.AccessibilityNodeInfo.AccessibilityAction action : info.getActionList()) {
            if (action.getLabel() != null && label.contentEquals(action.getLabel())) {
                return handle.performAccessibilityAction(action.getId(), null);
            }
        }
        return false;
    }

    @Test public void draggingAHandlePastANeighbourSavesTheNewOrderWhenTheFingerLifts() {
        try (var controller = Robolectric.buildActivity(KeyboardShortcutsActivity.class).setup()) {
            KeyboardShortcutsActivity activity = controller.get();
            saveThree(activity);
            ReflectionHelpers.callInstanceMethod(activity, "refresh");
            android.view.View content = activity.findViewById(android.R.id.content);
            content.measure(android.view.View.MeasureSpec.makeMeasureSpec(800, android.view.View.MeasureSpec.EXACTLY),
                    android.view.View.MeasureSpec.makeMeasureSpec(1600, android.view.View.MeasureSpec.EXACTLY));
            content.layout(0, 0, 800, 1600);
            android.view.View handle = content.findViewWithTag("a");
            android.view.View row = (android.view.View) handle.getParent();
            int height = row.getHeight();
            assertTrue(height > 0);
            // Through the whole tree as on a device, where rebuilding the page inside the lift's dispatch crashed.
            android.graphics.Rect at = new android.graphics.Rect();
            handle.getDrawingRect(at);
            ((android.view.ViewGroup) content).offsetDescendantRectToMyCoords(handle, at);
            content.dispatchTouchEvent(android.view.MotionEvent.obtain(0, 0, android.view.MotionEvent.ACTION_DOWN, at.centerX(), at.centerY(), 0));
            content.dispatchTouchEvent(android.view.MotionEvent.obtain(0, 0, android.view.MotionEvent.ACTION_MOVE, at.centerX(), at.centerY() + height + 1, 0));
            assertEquals("nothing is saved until the finger lifts", "abc", order(activity));
            content.dispatchTouchEvent(android.view.MotionEvent.obtain(0, 0, android.view.MotionEvent.ACTION_UP, at.centerX(), at.centerY() + height + 1, 0));
            org.robolectric.shadows.ShadowLooper.idleMainLooper();
            assertEquals("bac", order(activity));
            assertNotNull("the page is rebuilt after the lift", content.findViewWithTag("a"));
        }
    }

    @Test public void aFastDragPastTwoNeighboursInOneMoveSavesTheWholeWay() {
        try (var controller = Robolectric.buildActivity(KeyboardShortcutsActivity.class).setup()) {
            KeyboardShortcutsActivity activity = controller.get();
            saveThree(activity);
            ReflectionHelpers.callInstanceMethod(activity, "refresh");
            android.view.View content = activity.findViewById(android.R.id.content);
            content.measure(android.view.View.MeasureSpec.makeMeasureSpec(800, android.view.View.MeasureSpec.EXACTLY),
                    android.view.View.MeasureSpec.makeMeasureSpec(1600, android.view.View.MeasureSpec.EXACTLY));
            content.layout(0, 0, 800, 1600);
            android.view.View handle = content.findViewWithTag("a");
            int height = ((android.view.View) handle.getParent()).getHeight();
            android.graphics.Rect at = new android.graphics.Rect();
            handle.getDrawingRect(at);
            ((android.view.ViewGroup) content).offsetDescendantRectToMyCoords(handle, at);
            content.dispatchTouchEvent(android.view.MotionEvent.obtain(0, 0, android.view.MotionEvent.ACTION_DOWN, at.centerX(), at.centerY(), 0));
            // One move, with no layout pass after it, past both others.
            content.dispatchTouchEvent(android.view.MotionEvent.obtain(0, 0, android.view.MotionEvent.ACTION_MOVE, at.centerX(), at.centerY() + 2 * height + 1, 0));
            // And back up past one, still before any layout.
            content.dispatchTouchEvent(android.view.MotionEvent.obtain(0, 0, android.view.MotionEvent.ACTION_MOVE, at.centerX(), at.centerY() + height - 1, 0));
            content.dispatchTouchEvent(android.view.MotionEvent.obtain(0, 0, android.view.MotionEvent.ACTION_MOVE, at.centerX(), at.centerY() + 2 * height + 1, 0));
            content.dispatchTouchEvent(android.view.MotionEvent.obtain(0, 0, android.view.MotionEvent.ACTION_UP, at.centerX(), at.centerY() + 2 * height + 1, 0));
            org.robolectric.shadows.ShadowLooper.idleMainLooper();
            assertEquals("bca", order(activity));
        }
    }

    @Test public void aCancelledDragLeavesTheOrderAlone() {
        try (var controller = Robolectric.buildActivity(KeyboardShortcutsActivity.class).setup()) {
            KeyboardShortcutsActivity activity = controller.get();
            saveThree(activity);
            ReflectionHelpers.callInstanceMethod(activity, "refresh");
            android.view.View content = activity.findViewById(android.R.id.content);
            content.measure(android.view.View.MeasureSpec.makeMeasureSpec(800, android.view.View.MeasureSpec.EXACTLY),
                    android.view.View.MeasureSpec.makeMeasureSpec(1600, android.view.View.MeasureSpec.EXACTLY));
            content.layout(0, 0, 800, 1600);
            android.view.View handle = content.findViewWithTag("a");
            int height = ((android.view.View) handle.getParent()).getHeight();
            handle.dispatchTouchEvent(touch(android.view.MotionEvent.ACTION_DOWN, 0));
            handle.dispatchTouchEvent(touch(android.view.MotionEvent.ACTION_MOVE, height + 1));
            handle.dispatchTouchEvent(touch(android.view.MotionEvent.ACTION_CANCEL, height + 1));
            assertEquals("abc", order(activity));
            assertNotNull(content.findViewWithTag("a"));
        }
    }

    @Test public void unfinishedShortcutSurvivesRecreationAndCanBeSaved() {
        try (var controller = Robolectric.buildActivity(KeyboardShortcutsActivity.class).setup()) {
            ReflectionHelpers.callInstanceMethod(controller.get(), "edit",
                    ReflectionHelpers.ClassParameter.from(KeyboardShortcuts.Shortcut.class, null));
            ((EditText) ReflectionHelpers.getField(controller.get(), "editName")).setText("Desktop");
            ((EditText) ReflectionHelpers.getField(controller.get(), "editChord")).setText("WIN + D");
            AlertDialog before = (AlertDialog) ShadowDialog.getLatestDialog();
            controller.recreate();
            AlertDialog after = (AlertDialog) ShadowDialog.getLatestDialog();
            assertNotSame(before, after);
            assertFalse(before.isShowing());
            assertTrue(after.isShowing());
            assertEquals("Desktop", ((EditText) ReflectionHelpers.getField(controller.get(), "editName")).getText().toString());
            assertEquals("WIN + D", ((EditText) ReflectionHelpers.getField(controller.get(), "editChord")).getText().toString());
            after.getButton(AlertDialog.BUTTON_POSITIVE).performClick();
            var entries = KeyboardShortcuts.load(controller.get());
            assertEquals(1, entries.size());
            assertEquals("Desktop", entries.get(0).name);
            assertArrayEquals(KeyboardShortcuts.parseChord("WIN + D"), entries.get(0).keys());
        }
    }
}
