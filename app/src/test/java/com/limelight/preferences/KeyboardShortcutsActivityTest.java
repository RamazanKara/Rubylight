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
