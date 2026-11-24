package com.limelight.ui;

import android.content.Context;
import android.view.KeyEvent;
import android.widget.LinearLayout;

import com.google.android.material.button.MaterialButton;
import com.limelight.R;
import com.limelight.binding.input.KeyboardShortcuts;

import java.util.List;

/**
 * Keyboard bank layouts (typing, function, navigation, numpad, and shortcuts) for
 * {@link DesktopControlsPanel}.
 */
final class DesktopControlsKeyBanks {
    private DesktopControlsKeyBanks() {
    }

    static void populate(DesktopControlsPanel panel, LinearLayout keys, String bank) {
        switch (bank) {
            case "function": functionKeys(panel, keys); break;
            case "navigation": navigationKeys(panel, keys); break;
            case "numpad": numpadKeys(panel, keys); break;
            case "shortcuts": shortcuts(panel, keys); break;
            default: typingKeys(panel, keys); break;
        }
    }

    private static void typingKeys(DesktopControlsPanel panel, LinearLayout keys) {
        Context context = panel.getContext();
        LinearLayout row = panel.keyRow(keys);
        for (char c : "1234567890".toCharArray()) panel.key(row, String.valueOf(c), KeyEvent.KEYCODE_0 + c - '0');
        row = panel.keyRow(keys);
        for (char c : "QWERTYUIOP".toCharArray()) panel.key(row, String.valueOf(c), KeyEvent.KEYCODE_A + c - 'A');
        row = panel.keyRow(keys);
        for (char c : "ASDFGHJKL".toCharArray()) panel.key(row, String.valueOf(c), KeyEvent.KEYCODE_A + c - 'A');
        panel.key(row, "↵", KeyEvent.KEYCODE_ENTER).setContentDescription("Enter");
        row = panel.keyRow(keys);
        panel.modifier(row, "Shift", KeyEvent.KEYCODE_SHIFT_LEFT);
        for (char c : "ZXCVBNM".toCharArray()) panel.key(row, String.valueOf(c), KeyEvent.KEYCODE_A + c - 'A');
        panel.key(row, "⌫", KeyEvent.KEYCODE_DEL).setContentDescription("Backspace");
        row = panel.keyRow(keys);
        panel.modifier(row, "Ctrl", KeyEvent.KEYCODE_CTRL_LEFT);
        panel.modifier(row, "Alt", KeyEvent.KEYCODE_ALT_LEFT);
        panel.modifier(row, "Win", KeyEvent.KEYCODE_META_LEFT);
        panel.key(row, "Tab", KeyEvent.KEYCODE_TAB);
        panel.key(row, context.getString(R.string.desktop_controls_space), KeyEvent.KEYCODE_SPACE, 2);
        panel.key(row, "Esc", KeyEvent.KEYCODE_ESCAPE);
    }

    private static void modifierRow(DesktopControlsPanel panel, LinearLayout keys) {
        LinearLayout row = panel.keyRow(keys);
        panel.modifier(row, "Ctrl", KeyEvent.KEYCODE_CTRL_LEFT);
        panel.modifier(row, "Alt", KeyEvent.KEYCODE_ALT_LEFT);
        panel.modifier(row, "Shift", KeyEvent.KEYCODE_SHIFT_LEFT);
        panel.modifier(row, "Win", KeyEvent.KEYCODE_META_LEFT);
        panel.key(row, "Esc", KeyEvent.KEYCODE_ESCAPE);
    }

    private static void functionKeys(DesktopControlsPanel panel, LinearLayout keys) {
        modifierRow(panel, keys);
        for (int start = 0; start < 12; start += 4) {
            LinearLayout row = panel.keyRow(keys);
            for (int i = start; i < start + 4; i++) panel.key(row, "F" + (i + 1), KeyEvent.KEYCODE_F1 + i);
        }
        LinearLayout row = panel.keyRow(keys);
        panel.key(row, "Print", KeyEvent.KEYCODE_SYSRQ);
        panel.key(row, "Pause", KeyEvent.KEYCODE_BREAK);
        panel.key(row, "Menu", KeyEvent.KEYCODE_MENU);
        panel.key(row, "Delete", KeyEvent.KEYCODE_FORWARD_DEL);
    }

    private static void navigationKeys(DesktopControlsPanel panel, LinearLayout keys) {
        modifierRow(panel, keys);
        LinearLayout row = panel.keyRow(keys);
        panel.key(row, "Home", KeyEvent.KEYCODE_MOVE_HOME); panel.key(row, "↑", KeyEvent.KEYCODE_DPAD_UP);
        panel.key(row, "End", KeyEvent.KEYCODE_MOVE_END); panel.key(row, "PgUp", KeyEvent.KEYCODE_PAGE_UP);
        row = panel.keyRow(keys);
        panel.key(row, "←", KeyEvent.KEYCODE_DPAD_LEFT); panel.key(row, "↓", KeyEvent.KEYCODE_DPAD_DOWN);
        panel.key(row, "→", KeyEvent.KEYCODE_DPAD_RIGHT); panel.key(row, "PgDn", KeyEvent.KEYCODE_PAGE_DOWN);
        row = panel.keyRow(keys);
        panel.key(row, "Insert", KeyEvent.KEYCODE_INSERT); panel.key(row, "Delete", KeyEvent.KEYCODE_FORWARD_DEL);
        panel.key(row, "Backspace", KeyEvent.KEYCODE_DEL); panel.key(row, "Enter", KeyEvent.KEYCODE_ENTER);
        row = panel.keyRow(keys);
        panel.key(row, ",", KeyEvent.KEYCODE_COMMA); panel.key(row, ".", KeyEvent.KEYCODE_PERIOD);
        panel.key(row, "/", KeyEvent.KEYCODE_SLASH); panel.key(row, "\\", KeyEvent.KEYCODE_BACKSLASH);
        panel.key(row, ";", KeyEvent.KEYCODE_SEMICOLON); panel.key(row, "'", KeyEvent.KEYCODE_APOSTROPHE);
        panel.key(row, "[", KeyEvent.KEYCODE_LEFT_BRACKET); panel.key(row, "]", KeyEvent.KEYCODE_RIGHT_BRACKET);
        panel.key(row, "`", KeyEvent.KEYCODE_GRAVE);
        row = panel.keyRow(keys);
        panel.key(row, "Caps", KeyEvent.KEYCODE_CAPS_LOCK); panel.key(row, "−", KeyEvent.KEYCODE_MINUS);
        panel.key(row, "=", KeyEvent.KEYCODE_EQUALS); panel.key(row, "Scroll", KeyEvent.KEYCODE_SCROLL_LOCK);
    }

    private static void numpadKeys(DesktopControlsPanel panel, LinearLayout keys) {
        modifierRow(panel, keys);
        LinearLayout row = panel.keyRow(keys);
        panel.key(row, "Num", KeyEvent.KEYCODE_NUM_LOCK); panel.key(row, "/", KeyEvent.KEYCODE_NUMPAD_DIVIDE);
        panel.key(row, "×", KeyEvent.KEYCODE_NUMPAD_MULTIPLY); panel.key(row, "−", KeyEvent.KEYCODE_NUMPAD_SUBTRACT);
        for (int start : new int[] {7, 4, 1}) {
            row = panel.keyRow(keys);
            for (int i = start; i < start + 3; i++) panel.key(row, String.valueOf(i), KeyEvent.KEYCODE_NUMPAD_0 + i);
            if (start == 7) panel.key(row, "+", KeyEvent.KEYCODE_NUMPAD_ADD);
            else if (start == 4) panel.key(row, "Enter", KeyEvent.KEYCODE_NUMPAD_ENTER);
            else panel.key(row, "=", KeyEvent.KEYCODE_NUMPAD_EQUALS);
        }
        row = panel.keyRow(keys);
        panel.key(row, "0", KeyEvent.KEYCODE_NUMPAD_0, 2); panel.key(row, ".", KeyEvent.KEYCODE_NUMPAD_DOT);
        panel.key(row, "⌫", KeyEvent.KEYCODE_DEL);
    }

    private static void shortcuts(DesktopControlsPanel panel, LinearLayout keys) {
        Context context = panel.getContext();
        LinearLayout row = panel.keyRow(keys);
        shortcut(panel, row, R.string.desktop_controls_shortcut_copy, "CTRL+C");
        shortcut(panel, row, R.string.desktop_controls_shortcut_paste, "CTRL+V");
        shortcut(panel, row, R.string.desktop_controls_shortcut_cut, "CTRL+X");
        row = panel.keyRow(keys);
        shortcut(panel, row, R.string.desktop_controls_shortcut_undo, "CTRL+Z");
        shortcut(panel, row, R.string.desktop_controls_shortcut_select_all, "CTRL+A");
        shortcut(panel, row, R.string.desktop_controls_shortcut_desktop, "WIN+D");
        row = panel.keyRow(keys);
        shortcut(panel, row, R.string.keyboard_task_manager, "CTRL+SHIFT+ESC");
        shortcut(panel, row, R.string.keyboard_switch_window, "ALT+TAB");
        shortcut(panel, row, R.string.keyboard_close_window, "ALT+F4");
        List<KeyboardShortcuts.Shortcut> saved = KeyboardShortcuts.load(context);
        int index = 0;
        for (KeyboardShortcuts.Shortcut shortcut : saved) {
            if (index++ % 2 == 0) row = panel.keyRow(keys);
            MaterialButton button = panel.button(shortcut.name, () -> panel.sendChord(shortcut.keys()));
            button.setContentDescription(shortcut.name + ": " + KeyboardShortcuts.format(shortcut.keys()));
            panel.addKeyView(row, button, 1);
        }
        MaterialButton edit = panel.button(context.getString(R.string.desktop_controls_edit_shortcuts), panel::editShortcuts);
        keys.addView(edit, new LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT, panel.dp(48)));
    }

    private static void shortcut(DesktopControlsPanel panel, LinearLayout row, int title, String chord) {
        int[] keys = KeyboardShortcuts.parseChord(chord);
        panel.addKeyView(row, panel.button(panel.getContext().getString(title), () -> panel.sendChord(keys)), 1);
    }
}
