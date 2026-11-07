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

    @Test public void upAndDownOnAHandleMoveItsShortcutAndKeepTheFocusOnIt() {
        try (var controller = Robolectric.buildActivity(KeyboardShortcutsActivity.class).setup()) {
            KeyboardShortcutsActivity activity = controller.get();
            saveThree(activity);
            ReflectionHelpers.callInstanceMethod(activity, "refresh");
            android.view.View handle = activity.findViewById(android.R.id.content).findViewWithTag("a");
            assertFalse("the first row has no way up: the focus moves on instead",
                    handle.dispatchKeyEvent(new android.view.KeyEvent(android.view.KeyEvent.ACTION_DOWN, android.view.KeyEvent.KEYCODE_DPAD_UP)));
            assertEquals("abc", order(activity));
            assertTrue(handle.dispatchKeyEvent(new android.view.KeyEvent(android.view.KeyEvent.ACTION_DOWN, android.view.KeyEvent.KEYCODE_DPAD_DOWN)));
            assertEquals("bac", order(activity));
            assertTrue(activity.findViewById(android.R.id.content).findViewWithTag("a").isFocused());
        }
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
