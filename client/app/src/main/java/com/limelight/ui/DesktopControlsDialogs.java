package com.limelight.ui;

import android.content.Context;
import android.widget.CheckBox;
import android.widget.EditText;
import android.widget.LinearLayout;
import android.widget.ScrollView;
import android.widget.TextView;

import androidx.appcompat.app.AlertDialog;

import com.google.android.material.button.MaterialButton;
import com.google.android.material.dialog.MaterialAlertDialogBuilder;
import com.limelight.R;
import com.limelight.binding.input.DesktopControlsConfig;
import com.limelight.binding.input.KeyboardShortcuts;

import java.util.List;
import java.util.UUID;
import java.util.function.Consumer;

/**
 * Customization and saved-shortcut dialogs owned by {@link DesktopControlsPanel}.
 * Uses {@link NavigationDialogBuilder} and {@link StreamGlass} to preserve glass panel styling
 * and controller focus behavior.
 */
final class DesktopControlsDialogs {
    private final DesktopControlsPanel panel;
    private AlertDialog customization, shortcutList, shortcutEdit;

    DesktopControlsDialogs(DesktopControlsPanel panel) {
        this.panel = panel;
    }

    void showCustomization(DesktopControlsConfig currentConfig) {
        panel.releaseInputs();
        if (customization != null) return;
        Context context = panel.getContext();
        DesktopControlsConfig draft = currentConfig.copy();
        LinearLayout fields = new LinearLayout(context);
        fields.setOrientation(LinearLayout.VERTICAL);
        fields.setPadding(panel.dp(20), panel.dp(4), panel.dp(20), panel.dp(12));
        ScrollView scroll = new ScrollView(context);
        scroll.addView(fields);
        panel.addSlider(fields, R.string.desktop_controls_key_height, draft.keyHeightDp, 36, 56, value -> draft.keyHeightDp = value);
        panel.addSlider(fields, R.string.desktop_controls_panel_height, panel.getPreferredHeightDp(), 220, 560, value -> {
            draft.panelHeightDp = value;
            draft.panelHeightCustomized = true;
        });
        panel.addSlider(fields, R.string.desktop_controls_opacity, draft.opacityPercent, 50, 100, value -> draft.opacityPercent = value);
        panel.addSlider(fields, R.string.desktop_controls_speed, draft.pointerSpeedPercent, 25, 400, value -> draft.pointerSpeedPercent = value);
        check(fields, R.string.desktop_controls_left_handed, draft.leftHanded, value -> draft.leftHanded = value);
        check(fields, R.string.desktop_controls_natural_scroll, draft.naturalScroll, value -> draft.naturalScroll = value);
        check(fields, R.string.desktop_controls_tap_to_click, draft.tapToClick, value -> draft.tapToClick = value);
        check(fields, R.string.desktop_controls_mouse_buttons, draft.mouseButtons, value -> draft.mouseButtons = value);
        check(fields, R.string.desktop_controls_middle_visible, draft.middleButton, value -> draft.middleButton = value);
        check(fields, R.string.desktop_controls_extra_visible, draft.extraButtons, value -> draft.extraButtons = value);
        check(fields, R.string.desktop_controls_lock_modifiers, draft.lockModifiers, value -> draft.lockModifiers = value);
        TextView help = new TextView(context);
        help.setText(R.string.desktop_controls_modifier_help);
        help.setTextSize(12);
        help.setPadding(panel.dp(4), 0, panel.dp(4), panel.dp(8));
        fields.addView(help);
        check(fields, R.string.desktop_controls_function_visible, draft.functionBank, value -> draft.functionBank = value);
        check(fields, R.string.desktop_controls_navigation_visible, draft.navigationBank, value -> draft.navigationBank = value);
        check(fields, R.string.desktop_controls_numpad_visible, draft.numpadBank, value -> draft.numpadBank = value);
        check(fields, R.string.desktop_controls_shortcuts_visible, draft.shortcutsBank, value -> draft.shortcutsBank = value);
        MaterialButton edit = panel.button(context.getString(R.string.desktop_controls_edit_shortcuts), () -> {
            if (customization != null) customization.dismiss();
            editShortcuts();
        });
        fields.addView(edit, new LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT, panel.dp(48)));
        customization = new NavigationDialogBuilder(context).setTitle(R.string.desktop_controls_customize)
                .setView(scroll).setPositiveButton(R.string.desktop_controls_apply, (dialog, which) -> panel.applyConfig(draft))
                .setNegativeButton(android.R.string.cancel, null)
                .setNeutralButton(R.string.desktop_controls_reset, (dialog, which) -> panel.confirmReset()).create();
        customization.setOnDismissListener(dialog -> customization = null);
        StreamGlass.show(customization);
    }

    private void check(LinearLayout fields, int label, boolean initial, Consumer<Boolean> changed) {
        CheckBox check = new CheckBox(panel.getContext());
        check.setText(label);
        check.setChecked(initial);
        check.setMinHeight(panel.dp(48));
        check.setOnCheckedChangeListener((button, checked) -> changed.accept(checked));
        fields.addView(check, new LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT, LinearLayout.LayoutParams.WRAP_CONTENT));
    }

    void editShortcuts() {
        panel.releaseInputs();
        if (shortcutList != null) return;
        Context context = panel.getContext();
        List<KeyboardShortcuts.Shortcut> entries = KeyboardShortcuts.load(context);
        LinearLayout list = new LinearLayout(context);
        list.setOrientation(LinearLayout.VERTICAL);
        list.setPadding(panel.dp(16), 0, panel.dp(16), panel.dp(8));
        ScrollView scroll = new ScrollView(context);
        scroll.addView(list);
        if (entries.isEmpty()) {
            TextView empty = new TextView(context);
            empty.setText(R.string.desktop_controls_no_shortcuts);
            empty.setPadding(panel.dp(8), panel.dp(8), panel.dp(8), panel.dp(8));
            list.addView(empty);
        }
        for (int i = 0; i < entries.size(); i++) {
            KeyboardShortcuts.Shortcut shortcut = entries.get(i);
            LinearLayout row = new LinearLayout(context);
            MaterialButton edit = panel.button(shortcut.name, () -> editShortcut(shortcut));
            edit.setContentDescription(shortcut.name + ": " + KeyboardShortcuts.format(shortcut.keys()));
            row.addView(edit, new LinearLayout.LayoutParams(0, panel.dp(48), 1));
            for (int step : new int[] {-1, 1}) {
                int target = i + step;
                MaterialButton move = panel.button(step < 0 ? "↑" : "↓", () -> {
                    List<KeyboardShortcuts.Shortcut> fresh = KeyboardShortcuts.load(context);
                    int from = indexOf(fresh, shortcut.id);
                    if (from < 0 || target < 0 || target >= fresh.size()) return;
                    fresh.add(target, fresh.remove(from));
                    KeyboardShortcuts.save(context, fresh);
                    refreshShortcutList();
                });
                move.setEnabled(target >= 0 && target < entries.size());
                move.setContentDescription(context.getString(step < 0 ?
                        R.string.desktop_controls_shortcut_up : R.string.desktop_controls_shortcut_down));
                row.addView(move, new LinearLayout.LayoutParams(panel.dp(48), panel.dp(48)));
            }
            list.addView(row);
        }
        shortcutList = new NavigationDialogBuilder(context).setTitle(R.string.keyboard_shortcuts_title)
                .setView(scroll).setPositiveButton(android.R.string.ok, null)
                .setNeutralButton(R.string.keyboard_shortcuts_add, null).create();
        shortcutList.setOnDismissListener(dialog -> { shortcutList = null; panel.rebuild(); });
        shortcutList.setOnShowListener(dialog -> {
            shortcutList.getButton(AlertDialog.BUTTON_NEUTRAL).setEnabled(entries.size() < KeyboardShortcuts.MAX_SHORTCUTS);
            shortcutList.getButton(AlertDialog.BUTTON_NEUTRAL).setOnClickListener(view -> editShortcut(null));
        });
        StreamGlass.show(shortcutList);
    }

    private static int indexOf(List<KeyboardShortcuts.Shortcut> entries, String id) {
        for (int i = 0; i < entries.size(); i++) if (entries.get(i).id.equals(id)) return i;
        return -1;
    }

    private void refreshShortcutList() {
        // Dialog dismissal notifications are posted; clear the old reference synchronously.
        AlertDialog previous = shortcutList;
        shortcutList = null;
        if (previous != null) {
            previous.setOnDismissListener(null);
            previous.dismiss();
        }
        editShortcuts();
    }

    private void editShortcut(KeyboardShortcuts.Shortcut existing) {
        if (shortcutEdit != null) return;
        Context context = panel.getContext();
        LinearLayout fields = new LinearLayout(context);
        fields.setOrientation(LinearLayout.VERTICAL);
        fields.setPadding(panel.dp(20), 0, panel.dp(20), panel.dp(8));
        EditText name = new EditText(context);
        name.setSingleLine(true);
        name.setHint(R.string.keyboard_shortcuts_name);
        name.setFilters(new android.text.InputFilter[] {new android.text.InputFilter.LengthFilter(80)});
        fields.addView(name);
        EditText chord = new EditText(context);
        chord.setSingleLine(true);
        chord.setHint(R.string.keyboard_shortcuts_chord);
        chord.setInputType(android.text.InputType.TYPE_CLASS_TEXT | android.text.InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS);
        chord.setFilters(new android.text.InputFilter[] {new android.text.InputFilter.LengthFilter(256)});
        fields.addView(chord);
        if (existing != null) {
            name.setText(existing.name);
            chord.setText(KeyboardShortcuts.format(existing.keys()));
        }
        ScrollView editorScroll = new ScrollView(context);
        editorScroll.addView(fields);
        MaterialAlertDialogBuilder builder = new NavigationDialogBuilder(context)
                .setTitle(existing == null ? R.string.keyboard_shortcuts_add : R.string.keyboard_shortcuts_edit)
                .setView(editorScroll).setPositiveButton(R.string.keyboard_shortcuts_save, null)
                .setNegativeButton(android.R.string.cancel, null);
        if (existing != null) builder.setNeutralButton(R.string.keyboard_shortcuts_delete, (dialog, which) -> {
            List<KeyboardShortcuts.Shortcut> entries = KeyboardShortcuts.load(context);
            entries.removeIf(entry -> entry.id.equals(existing.id));
            KeyboardShortcuts.save(context, entries);
            refreshShortcutList();
        });
        shortcutEdit = builder.create();
        shortcutEdit.setOnDismissListener(dialog -> shortcutEdit = null);
        shortcutEdit.setOnShowListener(dialog -> shortcutEdit.getButton(AlertDialog.BUTTON_POSITIVE).setOnClickListener(view -> {
            try {
                KeyboardShortcuts.Shortcut shortcut = new KeyboardShortcuts.Shortcut(existing == null ?
                        UUID.randomUUID().toString() : existing.id, name.getText().toString(),
                        KeyboardShortcuts.parseChord(chord.getText().toString()));
                List<KeyboardShortcuts.Shortcut> entries = KeyboardShortcuts.load(context);
                int index = indexOf(entries, shortcut.id);
                if (index >= 0) entries.set(index, shortcut);
                else entries.add(shortcut);
                KeyboardShortcuts.save(context, entries);
                AlertDialog previous = shortcutEdit;
                shortcutEdit = null;
                previous.setOnDismissListener(null);
                previous.dismiss();
                refreshShortcutList();
            } catch (IllegalArgumentException error) {
                chord.setError(context.getString(R.string.keyboard_shortcuts_invalid));
            }
        }));
        StreamGlass.show(shortcutEdit);
    }

    void dismissAll() {
        if (customization != null) customization.dismiss();
        if (shortcutEdit != null) shortcutEdit.dismiss();
        if (shortcutList != null) {
            shortcutList.setOnDismissListener(null);
            shortcutList.dismiss();
            shortcutList = null;
        }
    }
}
