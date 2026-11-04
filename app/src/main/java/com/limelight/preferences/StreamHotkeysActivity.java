package com.limelight.preferences;

import android.content.SharedPreferences;
import android.os.Bundle;
import android.preference.PreferenceManager;
import android.view.Gravity;
import android.view.KeyEvent;
import android.widget.LinearLayout;
import android.widget.TextView;

import androidx.appcompat.app.AlertDialog;

import com.google.android.material.button.MaterialButton;
import com.limelight.R;
import com.limelight.binding.input.StreamHotkeys;
import com.limelight.ui.NavigationDialogBuilder;
import com.limelight.ui.NightjarActivity;
import com.limelight.ui.StreamMenuModel;
import com.limelight.ui.StreamMenuModel.Hotkey;
import com.limelight.ui.TopAppBar;
import com.limelight.utils.UiHelper;

import java.util.ArrayList;
import java.util.LinkedHashSet;
import java.util.List;
import java.util.Set;

/** Settings → Controls → Stream hotkeys: each of Android's stream hotkeys with Change, Off and Reset, as on desktop Nightjar. */
public final class StreamHotkeysActivity extends NightjarActivity {
    @Override
    protected boolean compactOnTelevision() {
        return true;
    }

    private SharedPreferences prefs;
    private LinearLayout content;
    private AlertDialog recordDialog;

    @Override protected void onCreate(Bundle state) {
        super.onCreate(state);
        UiHelper.setLocale(this);
        prefs = PreferenceManager.getDefaultSharedPreferences(this);
        android.widget.FrameLayout page = TopAppBar.setUpPage(this, R.string.stream_hotkeys_title, this::finish);
        content = new LinearLayout(this);
        content.setOrientation(LinearLayout.VERTICAL);
        TopAppBar.addScrollingBody(page, content);
        refresh();
        UiHelper.notifyNewRootView(this);
    }

    private void refresh() {
        content.removeAllViews();
        List<Hotkey> hotkeys;
        try {
            hotkeys = StreamHotkeys.load(prefs);
        } catch (StreamMenuModel.Refused e) {
            TextView message = text(getString(R.string.stream_hotkeys_unavailable, e.getMessage()), R.style.TextAppearance_Nightjar_Body);
            content.addView(message);
            return;
        }
        int padding = Math.round(8 * getResources().getDisplayMetrics().density);
        for (Hotkey hotkey : hotkeys) {
            TextView title = text(getString(actionLabel(hotkey.id)), R.style.TextAppearance_Nightjar_Body);
            title.setPadding(0, padding * 2, 0, 0);
            content.addView(title);
            content.addView(text(hotkey.chord == null ? getString(R.string.stream_hotkey_off) : hotkey.chord.label,
                    R.style.TextAppearance_Nightjar_Supporting));
            LinearLayout buttons = new LinearLayout(this);
            buttons.setOrientation(LinearLayout.HORIZONTAL);
            buttons.setGravity(Gravity.START);
            button(buttons, R.string.stream_hotkey_change, true, () -> record(hotkey));
            button(buttons, R.string.stream_hotkey_off, hotkey.chord != null, () -> {
                StreamHotkeys.turnOff(prefs, hotkey.id);
                refresh();
            });
            button(buttons, R.string.stream_hotkey_reset, hotkey.changed, () -> {
                StreamHotkeys.reset(prefs, hotkey.id);
                refresh();
            });
            content.addView(buttons);
        }
        com.limelight.ui.SubPageFocus.apply(this, findViewById(R.id.page_content));
    }

    private TextView text(String value, int appearance) {
        TextView view = new TextView(this);
        view.setTextAppearance(this, appearance);
        view.setText(value);
        return view;
    }

    private void button(LinearLayout row, int label, boolean enabled, Runnable action) {
        MaterialButton button = new MaterialButton(this, null, R.attr.nightjarButtonTonalStyle);
        button.setText(label);
        button.setAllCaps(false);
        button.setEnabled(enabled);
        button.setOnClickListener(view -> action.run());
        LinearLayout.LayoutParams params = new LinearLayout.LayoutParams(LinearLayout.LayoutParams.WRAP_CONTENT,
                LinearLayout.LayoutParams.WRAP_CONTENT);
        params.setMarginEnd(Math.round(8 * getResources().getDisplayMetrics().density));
        row.addView(button, params);
    }

    static int actionLabel(String id) {
        switch (id) {
            case StreamHotkeys.STREAM_MENU: return R.string.stream_hotkey_action_stream_menu;
            case StreamHotkeys.QUIT: return R.string.stream_hotkey_action_quit;
            case StreamHotkeys.CAPTURE: return R.string.stream_hotkey_action_capture;
            case StreamHotkeys.OVERLAY: return R.string.stream_hotkey_action_overlay;
            case StreamHotkeys.PC_KEYBOARD: return R.string.pc_keyboard_title;
            case StreamHotkeys.CURSOR: return R.string.stream_hotkey_action_cursor;
            default: throw new IllegalArgumentException("No label for stream hotkey " + id);
        }
    }

    /** The words for a refused chord; a code the page cannot cause (an unknown action) says the model's own words. */
    String refusal(StreamMenuModel.Refused e) {
        switch (e.code) {
            case "taken": {
                String action = e.detail;
                try {
                    action = getString(actionLabel(e.detail));
                } catch (IllegalArgumentException ignored) {
                    // Another client's action keeps its id.
                }
                return getString(R.string.stream_hotkey_taken, action);
            }
            case "hotkey_length": return getString(R.string.stream_hotkey_length);
            case "hotkey_modifier": return getString(R.string.stream_hotkey_modifier);
            case "hotkey_one_key": return getString(R.string.stream_hotkey_one_key);
            case "reserved": return getString(R.string.stream_hotkey_reserved);
            case "unsupported_key": return getString(R.string.stream_hotkey_unsupported, e.detail);
            case "repeated_key": return getString(R.string.stream_hotkey_repeated, e.detail);
            default: return getString(R.string.stream_hotkeys_unavailable, e.getMessage());
        }
    }

    /** Records the next chord: the modifiers held when a key besides them goes down. */
    private void record(Hotkey hotkey) {
        if (recordDialog != null) return;
        TextView message = text(getString(R.string.stream_hotkey_record), R.style.TextAppearance_Nightjar_Body);
        int padding = Math.round(24 * getResources().getDisplayMetrics().density);
        message.setPadding(padding, padding / 2, padding, 0);
        Set<String> held = new LinkedHashSet<>();
        AlertDialog dialog = new NavigationDialogBuilder(this)
                .setTitle(actionLabel(hotkey.id))
                .setView(message)
                .setNegativeButton(android.R.string.cancel, null)
                .create();
        dialog.setOnKeyListener((ignored, keyCode, event) -> {
            if (StreamHotkeys.isModifier(keyCode)) {
                if (event.getAction() == KeyEvent.ACTION_DOWN) held.add(StreamHotkeys.keyName(keyCode));
                else if (event.getAction() == KeyEvent.ACTION_UP) held.remove(StreamHotkeys.keyName(keyCode));
                return true;
            }
            // A key alone is no chord: Back, the D-pad and Enter move in the dialog as ever.
            if (held.isEmpty()) return false;
            if (event.getAction() != KeyEvent.ACTION_DOWN || event.getRepeatCount() != 0) return true;
            List<String> keys = new ArrayList<>(held);
            keys.add(StreamHotkeys.keyName(keyCode));
            try {
                StreamHotkeys.record(prefs, hotkey.id, keys.toArray(new String[0]));
                dialog.dismiss();
                refresh();
            } catch (StreamMenuModel.Refused e) {
                message.setText(getString(R.string.stream_hotkey_record) + "\n" + refusal(e));
            }
            return true;
        });
        dialog.setOnDismissListener(ignored -> recordDialog = null);
        recordDialog = dialog;
        dialog.show();
    }

    @Override protected void onDestroy() {
        if (recordDialog != null) recordDialog.dismiss();
        super.onDestroy();
    }
}
