package com.limelight.preferences;

import android.app.Application;
import android.content.SharedPreferences;
import android.preference.PreferenceManager;
import android.view.KeyEvent;
import android.widget.Button;
import android.widget.LinearLayout;
import android.widget.TextView;

import androidx.appcompat.app.AlertDialog;

import com.limelight.R;
import com.limelight.binding.input.StreamHotkeys;
import com.limelight.ui.RustStreamMenuModel;
import com.limelight.ui.StreamMenuModel;

import org.junit.After;
import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;

import static org.junit.Assert.*;

/** The stream hotkeys page: which buttons a row offers, and what the record dialog saves from the keys pressed. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class StreamHotkeysActivityTest {
    private SharedPreferences prefs;
    private ActivityController<StreamHotkeysActivity> controller;

    @Before
    public void page() {
        StreamMenuModel.use(new RustStreamMenuModel());
        prefs = PreferenceManager.getDefaultSharedPreferences(RuntimeEnvironment.getApplication());
        prefs.edit().clear().commit();
        controller = Robolectric.buildActivity(StreamHotkeysActivity.class).setup();
    }

    @After
    public void close() {
        controller.close();
        StreamMenuModel.use(null);
    }

    /** The row of an action: its title, its chord, then Change, Off and Reset. */
    private int row(int title) {
        LinearLayout content = ReflectionHelpers.getField(controller.get(), "content");
        String text = controller.get().getString(title);
        for (int i = 0; i < content.getChildCount(); i++) {
            if (content.getChildAt(i) instanceof TextView && text.contentEquals(((TextView) content.getChildAt(i)).getText())) return i;
        }
        throw new AssertionError("no row " + text);
    }

    private String chord(int title) {
        LinearLayout content = ReflectionHelpers.getField(controller.get(), "content");
        return ((TextView) content.getChildAt(row(title) + 1)).getText().toString();
    }

    private Button button(int title, int index) {
        LinearLayout content = ReflectionHelpers.getField(controller.get(), "content");
        return (Button) ((LinearLayout) content.getChildAt(row(title) + 2)).getChildAt(index);
    }

    private static final int CHANGE = 0, OFF = 1, RESET = 2;
    private static final int CURSOR = R.string.stream_hotkey_action_cursor;

    private AlertDialog record(int... keys) {
        button(CURSOR, CHANGE).performClick();
        AlertDialog dialog = controller.get().recordDialog();
        assertNotNull(dialog);
        for (int key : keys) {
            int action = key < 0 ? KeyEvent.ACTION_UP : KeyEvent.ACTION_DOWN;
            dialog.dispatchKeyEvent(new KeyEvent(action, Math.abs(key)));
        }
        return dialog;
    }

    private String saved() {
        return prefs.getString(StreamHotkeys.PREFIX + StreamHotkeys.CURSOR, null);
    }

    @Test
    public void offIsOfferedWhileAChordIsSetAndResetOnceChanged() {
        assertEquals("Ctrl + Alt + Shift + P", chord(CURSOR));
        assertTrue(button(CURSOR, OFF).isEnabled());
        assertFalse(button(CURSOR, RESET).isEnabled());
        button(CURSOR, OFF).performClick();
        assertEquals(controller.get().getString(R.string.stream_hotkey_off), chord(CURSOR));
        assertFalse(button(CURSOR, OFF).isEnabled());
        assertTrue(button(CURSOR, RESET).isEnabled());
        button(CURSOR, RESET).performClick();
        assertEquals("Ctrl + Alt + Shift + P", chord(CURSOR));
        assertFalse(button(CURSOR, RESET).isEnabled());
    }

    @Test
    public void theModifiersHeldAndTheKeyAreSaved() {
        AlertDialog dialog = record(KeyEvent.KEYCODE_CTRL_LEFT, KeyEvent.KEYCODE_F2);
        assertFalse(dialog.isShowing());
        assertEquals("CTRL+F2", saved());
        assertEquals("Ctrl + F2", chord(CURSOR));
    }

    @Test
    public void aKeyAloneGetsCtrlAltShift() {
        record(KeyEvent.KEYCODE_F6);
        assertEquals("CTRL+ALT+SHIFT+F6", saved());
    }

    /** Both Shifts down and one up: Shift is still held. */
    @Test
    public void oneOfTwoShiftsUpStillHoldsShift() {
        record(KeyEvent.KEYCODE_SHIFT_LEFT, KeyEvent.KEYCODE_SHIFT_RIGHT, -KeyEvent.KEYCODE_SHIFT_RIGHT, KeyEvent.KEYCODE_F3);
        assertEquals("SHIFT+F3", saved());
    }

    /** Back and the D-pad alone still move in the dialog, so a remote reaches Cancel. */
    @Test
    public void theKeysThatMoveInTheDialogAreNotRecordedAlone() {
        AlertDialog dialog = record(KeyEvent.KEYCODE_DPAD_DOWN, -KeyEvent.KEYCODE_DPAD_DOWN);
        assertTrue(dialog.isShowing());
        assertNull(saved());
        // The record listener leaves them to the dialog: it neither takes nor records them.
        android.content.DialogInterface.OnKeyListener listener = ReflectionHelpers.getField(dialog, "mOnKeyListener");
        for (int key : new int[] {KeyEvent.KEYCODE_BACK, KeyEvent.KEYCODE_ESCAPE, KeyEvent.KEYCODE_DPAD_CENTER, KeyEvent.KEYCODE_BUTTON_A}) {
            assertFalse(KeyEvent.keyCodeToString(key), listener.onKey(dialog, key, new KeyEvent(KeyEvent.ACTION_DOWN, key)));
        }
        assertTrue(dialog.isShowing());
        assertNull(saved());
    }

    @Test
    public void aTypingChordIsRefusedAndTheDialogStays() {
        AlertDialog dialog = record(KeyEvent.KEYCODE_SHIFT_LEFT, KeyEvent.KEYCODE_A);
        assertTrue(dialog.isShowing());
        assertNull(saved());
        String typing = controller.get().getString(R.string.stream_hotkey_typing);
        boolean said = false;
        for (TextView view : texts(dialog.getWindow().getDecorView())) said |= view.getText().toString().contains(typing);
        assertTrue("the refusal is shown", said);
    }

    private static java.util.List<TextView> texts(android.view.View view) {
        java.util.List<TextView> found = new java.util.ArrayList<>();
        if (view instanceof TextView) found.add((TextView) view);
        if (view instanceof android.view.ViewGroup) {
            android.view.ViewGroup group = (android.view.ViewGroup) view;
            for (int i = 0; i < group.getChildCount(); i++) found.addAll(texts(group.getChildAt(i)));
        }
        return found;
    }

    @Test
    public void aResetToATakenDefaultIsRefusedAndNothingChanges() throws Exception {
        StreamHotkeys.turnOff(prefs, StreamHotkeys.CURSOR);
        StreamHotkeys.record(prefs, StreamHotkeys.QUIT, new String[] {"CTRL", "ALT", "SHIFT", "P"});
        controller.recreate();
        button(CURSOR, RESET).performClick();
        assertEquals(controller.get().getString(R.string.stream_hotkey_taken,
                controller.get().getString(R.string.stream_hotkey_action_quit)),
                org.robolectric.shadows.ShadowToast.getTextOfLatestToast());
        assertEquals("off", saved());
    }
}
