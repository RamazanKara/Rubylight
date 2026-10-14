package com.limelight.ui;

import android.content.Context;
import android.content.res.ColorStateList;
import android.view.KeyEvent;
import android.widget.HorizontalScrollView;
import android.widget.LinearLayout;


import com.google.android.material.button.MaterialButton;
import com.google.android.material.color.MaterialColors;
import com.limelight.R;
import com.limelight.binding.input.KeyboardShortcuts;

import java.util.ArrayList;
import java.util.List;
import java.util.function.Consumer;

public final class KeyboardOverlay extends LinearLayout {
    private final List<MaterialButton> modifiers = new ArrayList<>();
    private final Consumer<int[]> sendKeys;
    private final int spacing;

    public KeyboardOverlay(Context context, Consumer<int[]> sendKeys, Runnable keyboard, Runnable hide) {
        super(context);
        this.sendKeys = sendKeys;
        spacing = Math.round(4 * getResources().getDisplayMetrics().density);
        setOrientation(VERTICAL);
        setBackgroundColor(MaterialColors.getColor(context, com.google.android.material.R.attr.colorSurfaceContainerHigh, 0));
        setPadding(spacing, 0, spacing, 0);

        LinearLayout row = addRow();
        addKey(row, "Esc", KeyEvent.KEYCODE_ESCAPE);
        addButton(row, context.getString(R.string.keyboard_keys_hide), hide);
        addButton(row, context.getString(R.string.stream_keyboard), keyboard);
        addKey(row, "Tab", KeyEvent.KEYCODE_TAB);
        addModifier(row, "Ctrl", KeyEvent.KEYCODE_CTRL_LEFT);
        addModifier(row, "Alt", KeyEvent.KEYCODE_ALT_LEFT);
        addModifier(row, "Shift", KeyEvent.KEYCODE_SHIFT_LEFT);
        addModifier(row, "Win", KeyEvent.KEYCODE_META_LEFT);
        addKey(row, "Enter", KeyEvent.KEYCODE_ENTER);
        addKey(row, "Backspace", KeyEvent.KEYCODE_DEL);
        addKey(row, "Delete", KeyEvent.KEYCODE_FORWARD_DEL);
        addKey(row, "Home", KeyEvent.KEYCODE_MOVE_HOME);
        addKey(row, "End", KeyEvent.KEYCODE_MOVE_END);
        addKey(row, "PgUp", KeyEvent.KEYCODE_PAGE_UP);
        addKey(row, "PgDn", KeyEvent.KEYCODE_PAGE_DOWN);

        row = addRow();
        addButton(row, context.getString(R.string.keyboard_task_manager),
                () -> sendShortcut(KeyEvent.KEYCODE_CTRL_LEFT, KeyEvent.KEYCODE_SHIFT_LEFT, KeyEvent.KEYCODE_ESCAPE));
        addButton(row, context.getString(R.string.keyboard_switch_window),
                () -> sendShortcut(KeyEvent.KEYCODE_ALT_LEFT, KeyEvent.KEYCODE_TAB));
        addButton(row, context.getString(R.string.keyboard_close_window),
                () -> sendShortcut(KeyEvent.KEYCODE_ALT_LEFT, KeyEvent.KEYCODE_F4));
        addKey(row, "←", KeyEvent.KEYCODE_DPAD_LEFT).setContentDescription(context.getString(R.string.keyboard_arrow_left));
        addKey(row, "↑", KeyEvent.KEYCODE_DPAD_UP).setContentDescription(context.getString(R.string.keyboard_arrow_up));
        addKey(row, "↓", KeyEvent.KEYCODE_DPAD_DOWN).setContentDescription(context.getString(R.string.keyboard_arrow_down));
        addKey(row, "→", KeyEvent.KEYCODE_DPAD_RIGHT).setContentDescription(context.getString(R.string.keyboard_arrow_right));
        for (int i = 0; i < 12; i++) {
            addKey(row, "F" + (i + 1), KeyEvent.KEYCODE_F1 + i);
        }
        List<KeyboardShortcuts.Shortcut> shortcuts = KeyboardShortcuts.load(context);
        if (!shortcuts.isEmpty()) {
            row = addRow();
            for (KeyboardShortcuts.Shortcut shortcut : shortcuts) {
                addButton(row, shortcut.name, () -> sendShortcut(shortcut.keys()));
            }
        }
    }

    private LinearLayout addRow() {
        HorizontalScrollView scroll = new HorizontalScrollView(getContext());
        scroll.setFillViewport(true);
        LinearLayout row = new LinearLayout(getContext());
        scroll.addView(row);
        addView(scroll, new LayoutParams(LayoutParams.MATCH_PARENT, LayoutParams.WRAP_CONTENT));
        return row;
    }

    private MaterialButton addButton(LinearLayout row, String label, Runnable action) {
        MaterialButton button = new MaterialButton(getContext(), null, androidx.appcompat.R.attr.borderlessButtonStyle);
        button.setText(label);
        button.setTextColor(MaterialColors.getColor(getContext(), com.google.android.material.R.attr.colorOnSurface, 0));
        button.setAllCaps(false);
        button.setMinWidth(spacing * 12);
        button.setMinimumWidth(spacing * 12);
        button.setMinHeight(spacing * 12);
        button.setMinimumHeight(spacing * 12);
        button.setPadding(spacing * 2, 0, spacing * 2, 0);
        button.setFocusable(false);
        button.setOnClickListener(view -> action.run());
        row.addView(button, new LayoutParams(LayoutParams.WRAP_CONTENT, LayoutParams.WRAP_CONTENT));
        return button;
    }

    private void addModifier(LinearLayout row, String label, int key) {
        MaterialButton button = addButton(row, label, () -> { });
        button.setCheckable(true);
        button.setBackgroundTintList(new ColorStateList(new int[][] {
                new int[] {android.R.attr.state_checked}, new int[] { }
        }, new int[] {MaterialColors.getColor(getContext(), com.google.android.material.R.attr.colorPrimaryContainer, 0),
                android.graphics.Color.TRANSPARENT}));
        button.setTag(key);
        modifiers.add(button);
    }

    private MaterialButton addKey(LinearLayout row, String label, int key) {
        return addButton(row, label, () -> {
            int count = 1;
            for (MaterialButton modifier : modifiers) {
                if (modifier.isChecked()) count++;
            }
            int[] keys = new int[count];
            int index = 0;
            for (MaterialButton modifier : modifiers) {
                if (modifier.isChecked()) keys[index++] = (int) modifier.getTag();
            }
            keys[index] = key;
            sendShortcut(keys);
        });
    }

    private void sendShortcut(int... keys) {
        resetModifiers();
        sendKeys.accept(keys);
    }

    public void resetModifiers() {
        for (MaterialButton modifier : modifiers) {
            modifier.setChecked(false);
        }
    }
}
