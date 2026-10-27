package com.limelight.ui;

import android.annotation.SuppressLint;
import android.content.Context;
import android.content.res.ColorStateList;
import android.graphics.Color;
import android.graphics.Typeface;
import android.graphics.drawable.GradientDrawable;
import android.view.Gravity;
import android.view.KeyEvent;
import android.view.MotionEvent;
import android.view.View;
import android.view.ViewGroup;
import android.widget.CheckBox;
import android.widget.EditText;
import android.widget.HorizontalScrollView;
import android.widget.LinearLayout;
import android.widget.ScrollView;
import android.widget.SeekBar;
import android.widget.TextView;

import androidx.appcompat.app.AlertDialog;
import androidx.appcompat.widget.TooltipCompat;

import com.google.android.material.button.MaterialButton;
import com.google.android.material.color.MaterialColors;
import com.google.android.material.dialog.MaterialAlertDialogBuilder;
import com.limelight.R;
import com.limelight.binding.input.DesktopControlsConfig;
import com.limelight.binding.input.KeyboardShortcuts;

import java.util.ArrayList;
import java.util.LinkedHashSet;
import java.util.List;
import java.util.Locale;
import java.util.Set;
import java.util.UUID;
import java.util.function.Consumer;
import java.util.function.IntConsumer;

/** Desktop controls with local state only. The owner validates every outgoing input event. */
@SuppressLint("ViewConstructor")
public class DesktopControlsPanel extends LinearLayout {
    public enum Mode { KEYBOARD, TRACKPAD, COMBINED }
    private enum HeaderPicker { NONE, MODE, BANK }

    public interface Sink {
        void sendKeys(int[] keys);
        /** Relative screen pixels, with panel sensitivity already applied. */
        void move(float dx, float dy);
        /** High-resolution wheel units (120 per notch); positive vertical means wheel up. */
        void scroll(float vertical, float horizontal);
        /** Moonlight mouse buttons: left=1, middle=2, right=3, back=4, forward=5. */
        void mouseButton(int button, boolean down);
    }

    private final Sink sink;
    private final Runnable systemKeyboard, hide;
    private DesktopControlsConfig config;
    private Mode mode;
    private String bank;
    private LinearLayout header;
    private HeaderPicker headerPicker = HeaderPicker.NONE;
    private boolean wide;
    private DesktopTrackpad trackpad;
    private final Set<Integer> selectedModifiers = new LinkedHashSet<>();
    private final List<MaterialButton> modifierButtons = new ArrayList<>();
    private final List<MaterialButton> mouseViews = new ArrayList<>();
    // The gesture pad and held buttons can own the same button without releasing each other.
    private final boolean[] buttonHeld = new boolean[6], padHeld = new boolean[6];
    private AlertDialog customization;
    private AlertDialog shortcutList, shortcutEdit, resetDialog;

    public DesktopControlsPanel(Context context, Sink sink, Runnable systemKeyboard, Runnable hide) {
        super(context);
        this.sink = sink;
        this.systemKeyboard = systemKeyboard;
        this.hide = hide;
        config = DesktopControlsConfig.load(context);
        mode = Mode.valueOf(config.mode.toUpperCase(Locale.ROOT));
        bank = config.bank;
        setOrientation(VERTICAL);
        // Empty space, including cutout padding, belongs to the panel rather than the stream.
        setClickable(true);
        setFocusable(false);
        setMotionEventSplittingEnabled(true);
        setPadding(dp(4), dp(3), dp(4), dp(3));
        setElevation(dp(12));
        setContentDescription(context.getString(R.string.desktop_controls_title));
        rebuild();
    }

    public Mode getMode() { return mode; }
    public DesktopControlsConfig getConfig() { return config.copy(); }
    public int getPreferredHeightDp() {
        if (config.panelHeightCustomized) return config.panelHeightDp;
        // The bank selector now shares the toolbar. Return its former row to the stream,
        // without reducing the space available to the keys or changing custom heights.
        if (mode == Mode.COMBINED && !wide) return 384;
        return mode == Mode.TRACKPAD ? config.panelHeightDp : config.panelHeightDp - 36;
    }

    public void setMode(Mode mode) {
        if (mode == null) return;
        releaseInputs();
        this.mode = mode;
        headerPicker = HeaderPicker.NONE;
        config.mode = mode.name().toLowerCase(Locale.ROOT);
        config.save(getContext());
        rebuild();
    }

    /** Reload on reopening so the standalone shortcut editor's changes become visible. */
    public void reloadConfiguration() {
        releaseInputs();
        config = DesktopControlsConfig.load(getContext());
        mode = Mode.valueOf(config.mode.toUpperCase(Locale.ROOT));
        bank = config.bank;
        headerPicker = HeaderPicker.NONE;
        rebuild();
    }

    private int dp(float value) { return Math.round(value * getResources().getDisplayMetrics().density); }
    private int color(int attribute) { return MaterialColors.getColor(getContext(), attribute, Color.DKGRAY); }

    private void rebuild() {
        releaseInputs();
        removeAllViews();
        modifierButtons.clear();
        mouseViews.clear();
        trackpad = null;
        if (!bankEnabled(bank)) bank = "typing";
        int surface = color(com.google.android.material.R.attr.colorSurfaceContainer);
        GradientDrawable background = new GradientDrawable();
        background.setColor(Color.argb(config.opacityPercent * 255 / 100, Color.red(surface), Color.green(surface), Color.blue(surface)));
        background.setCornerRadii(new float[] {dp(18), dp(18), dp(18), dp(18), 0, 0, 0, 0});
        background.setStroke(dp(1), color(com.google.android.material.R.attr.colorOutlineVariant));
        setBackground(background);
        addHeader();

        LinearLayout body = new LinearLayout(getContext());
        body.setOrientation(mode == Mode.COMBINED && wide ? HORIZONTAL : VERTICAL);
        body.setMotionEventSplittingEnabled(true);
        addView(body, new LayoutParams(LayoutParams.MATCH_PARENT, 0, 1));
        if (mode == Mode.KEYBOARD) body.addView(createKeyboard(), fill());
        else if (mode == Mode.TRACKPAD) body.addView(createPad(), fill());
        else {
            View keyboard = createKeyboard(), pad = createPad();
            if (wide) {
                if (config.leftHanded) {
                    body.addView(pad, new LayoutParams(0, LayoutParams.MATCH_PARENT, .36f));
                    body.addView(keyboard, new LayoutParams(0, LayoutParams.MATCH_PARENT, .64f));
                } else {
                    body.addView(keyboard, new LayoutParams(0, LayoutParams.MATCH_PARENT, .64f));
                    body.addView(pad, new LayoutParams(0, LayoutParams.MATCH_PARENT, .36f));
                }
            } else {
                body.addView(keyboard, new LayoutParams(LayoutParams.MATCH_PARENT, 0, .66f));
                body.addView(pad, new LayoutParams(LayoutParams.MATCH_PARENT, 0, .34f));
            }
        }
        requestLayout();
    }

    private LayoutParams fill() { return new LayoutParams(LayoutParams.MATCH_PARENT, LayoutParams.MATCH_PARENT); }

    private void addHeader() {
        header = new LinearLayout(getContext());
        header.setGravity(Gravity.CENTER_VERTICAL);
        addView(header, new LayoutParams(LayoutParams.MATCH_PARENT, dp(48)));
        rebuildHeader();
    }

    private int modeTitle(Mode value) {
        switch (value) {
            case TRACKPAD: return R.string.desktop_controls_trackpad;
            case COMBINED: return R.string.desktop_controls_combined;
            default: return R.string.desktop_controls_keyboard;
        }
    }

    private int bankTitle() {
        switch (bank) {
            case "function": return R.string.desktop_controls_function;
            case "navigation": return R.string.desktop_controls_navigation;
            case "numpad": return R.string.desktop_controls_numpad;
            case "shortcuts": return R.string.desktop_controls_shortcuts;
            default: return R.string.desktop_controls_typing;
        }
    }

    private void rebuildHeader() {
        // Inline pickers retain the stream's focus and locally selected modifiers.
        // Opening a dialog or popup here would release them on window focus loss.
        header.removeAllViews();
        if (headerPicker != HeaderPicker.NONE) {
            MaterialButton back = headerIcon(R.drawable.ic_arrow_back, R.string.sheet_back, () -> {
                headerPicker = HeaderPicker.NONE;
                rebuildHeader();
            });
            header.addView(back, new LayoutParams(dp(48), dp(48)));
        }
        if (headerPicker == HeaderPicker.MODE) {
            for (Mode choice : Mode.values()) {
                MaterialButton button = button(getContext().getString(modeTitle(choice)), () -> setMode(choice));
                button.setTextSize(12);
                button.setPadding(dp(2), 0, dp(2), 0);
                describe(button, getContext().getString(modeTitle(choice)));
                selected(button, mode == choice);
                header.addView(button, new LayoutParams(0, dp(48), 1));
            }
        } else if (headerPicker == HeaderPicker.BANK) {
            HorizontalScrollView scroll = new HorizontalScrollView(getContext());
            scroll.setHorizontalScrollBarEnabled(false);
            LinearLayout banks = new LinearLayout(getContext());
            scroll.addView(banks);
            addBank(banks, "typing", R.string.desktop_controls_typing);
            if (config.functionBank) addBank(banks, "function", R.string.desktop_controls_function);
            if (config.navigationBank) addBank(banks, "navigation", R.string.desktop_controls_navigation);
            if (config.numpadBank) addBank(banks, "numpad", R.string.desktop_controls_numpad);
            if (config.shortcutsBank) addBank(banks, "shortcuts", R.string.desktop_controls_shortcuts);
            header.addView(scroll, new LayoutParams(0, dp(48), 1));
        } else {
            addSelector(modeTitle(mode), R.string.desktop_controls_choose_mode, HeaderPicker.MODE);
            if (mode != Mode.TRACKPAD) {
                addSelector(bankTitle(), R.string.desktop_controls_choose_bank, HeaderPicker.BANK);
            }
            MaterialButton phone = headerIcon(R.drawable.ic_keyboard, R.string.desktop_controls_system_keyboard,
                    () -> { releaseInputs(); systemKeyboard.run(); });
            header.addView(phone, new LayoutParams(dp(48), dp(48)));
            MaterialButton settings = headerIcon(R.drawable.ic_settings, R.string.desktop_controls_customize,
                    this::showCustomization);
            header.addView(settings, new LayoutParams(dp(48), dp(48)));
        }
        MaterialButton close = headerIcon(R.drawable.ic_close, R.string.desktop_controls_hide,
                () -> { releaseInputs(); hide.run(); });
        header.addView(close, new LayoutParams(dp(48), dp(48)));
    }

    private MaterialButton headerIcon(int icon, int description, Runnable action) {
        MaterialButton button = button("", action);
        button.setIconResource(icon);
        button.setIconSize(dp(24));
        button.setIconPadding(0);
        button.setIconTint(button.getTextColors());
        button.setPadding(0, 0, 0, 0);
        button.setIconGravity(MaterialButton.ICON_GRAVITY_TEXT_START);
        describe(button, getContext().getString(description));
        return button;
    }

    private void addSelector(int title, int description, HeaderPicker picker) {
        String label = getContext().getString(title);
        MaterialButton selector = button(label, () -> {
            headerPicker = picker;
            rebuildHeader();
        });
        selector.setTextSize(12);
        selector.setIconResource(R.drawable.ic_expand_more);
        selector.setIconSize(dp(18));
        selector.setIconPadding(dp(4));
        selector.setIconTint(selector.getTextColors());
        selector.setIconGravity(MaterialButton.ICON_GRAVITY_END);
        describe(selector, getContext().getString(description, label));
        header.addView(selector, new LayoutParams(0, dp(48), 1));
    }

    private void describe(View view, String text) {
        view.setContentDescription(text);
        TooltipCompat.setTooltipText(view, text);
    }

    private LinearLayout createKeyboard() {
        LinearLayout keyboard = new LinearLayout(getContext());
        keyboard.setOrientation(VERTICAL);
        ScrollView scroll = new ScrollView(getContext());
        scroll.setFillViewport(false);
        keyboard.addView(scroll, new LayoutParams(LayoutParams.MATCH_PARENT, 0, 1));
        LinearLayout keys = new LinearLayout(getContext());
        keys.setOrientation(VERTICAL);
        scroll.addView(keys, new ScrollView.LayoutParams(LayoutParams.MATCH_PARENT, LayoutParams.WRAP_CONTENT));
        switch (bank) {
            case "function": functionKeys(keys); break;
            case "navigation": navigationKeys(keys); break;
            case "numpad": numpadKeys(keys); break;
            case "shortcuts": shortcuts(keys); break;
            default: typingKeys(keys); break;
        }
        return keyboard;
    }

    private boolean bankEnabled(String value) {
        switch (value) {
            case "typing": return true;
            case "function": return config.functionBank;
            case "navigation": return config.navigationBank;
            case "numpad": return config.numpadBank;
            case "shortcuts": return config.shortcutsBank;
            default: return false;
        }
    }

    private void addBank(LinearLayout parent, String name, int title) {
        MaterialButton button = button(getContext().getString(title), () -> {
            // Changing key banks keeps local modifiers, so Ctrl then Function then F4 is possible.
            Set<Integer> modifiers = new LinkedHashSet<>(selectedModifiers);
            releaseInputs();
            bank = name;
            config.bank = name;
            config.save(getContext());
            headerPicker = HeaderPicker.NONE;
            rebuild();
            selectedModifiers.addAll(modifiers);
            updateModifiers();
        });
        button.setTextSize(12);
        selected(button, name.equals(bank));
        parent.addView(button, new LayoutParams(LayoutParams.WRAP_CONTENT, dp(48)));
    }

    private LinearLayout keyRow(LinearLayout parent) {
        LinearLayout row = new LinearLayout(getContext());
        row.setGravity(Gravity.CENTER_VERTICAL);
        parent.addView(row, new LayoutParams(LayoutParams.MATCH_PARENT, dp(config.keyHeightDp)));
        return row;
    }

    private void typingKeys(LinearLayout keys) {
        LinearLayout row = keyRow(keys);
        for (char c : "1234567890".toCharArray()) key(row, String.valueOf(c), KeyEvent.KEYCODE_0 + c - '0');
        row = keyRow(keys);
        for (char c : "QWERTYUIOP".toCharArray()) key(row, String.valueOf(c), KeyEvent.KEYCODE_A + c - 'A');
        row = keyRow(keys);
        for (char c : "ASDFGHJKL".toCharArray()) key(row, String.valueOf(c), KeyEvent.KEYCODE_A + c - 'A');
        key(row, "↵", KeyEvent.KEYCODE_ENTER).setContentDescription("Enter");
        row = keyRow(keys);
        modifier(row, "Shift", KeyEvent.KEYCODE_SHIFT_LEFT);
        for (char c : "ZXCVBNM".toCharArray()) key(row, String.valueOf(c), KeyEvent.KEYCODE_A + c - 'A');
        key(row, "⌫", KeyEvent.KEYCODE_DEL).setContentDescription("Backspace");
        row = keyRow(keys);
        modifier(row, "Ctrl", KeyEvent.KEYCODE_CTRL_LEFT);
        modifier(row, "Alt", KeyEvent.KEYCODE_ALT_LEFT);
        modifier(row, "Win", KeyEvent.KEYCODE_META_LEFT);
        key(row, "Tab", KeyEvent.KEYCODE_TAB);
        key(row, getContext().getString(R.string.desktop_controls_space), KeyEvent.KEYCODE_SPACE, 2);
        key(row, "Esc", KeyEvent.KEYCODE_ESCAPE);
    }

    private void modifierRow(LinearLayout keys) {
        LinearLayout row = keyRow(keys);
        modifier(row, "Ctrl", KeyEvent.KEYCODE_CTRL_LEFT);
        modifier(row, "Alt", KeyEvent.KEYCODE_ALT_LEFT);
        modifier(row, "Shift", KeyEvent.KEYCODE_SHIFT_LEFT);
        modifier(row, "Win", KeyEvent.KEYCODE_META_LEFT);
        key(row, "Esc", KeyEvent.KEYCODE_ESCAPE);
    }

    private void functionKeys(LinearLayout keys) {
        modifierRow(keys);
        for (int start = 0; start < 12; start += 4) {
            LinearLayout row = keyRow(keys);
            for (int i = start; i < start + 4; i++) key(row, "F" + (i + 1), KeyEvent.KEYCODE_F1 + i);
        }
        LinearLayout row = keyRow(keys);
        key(row, "Print", KeyEvent.KEYCODE_SYSRQ);
        key(row, "Pause", KeyEvent.KEYCODE_BREAK);
        key(row, "Menu", KeyEvent.KEYCODE_MENU);
        key(row, "Delete", KeyEvent.KEYCODE_FORWARD_DEL);
    }

    private void navigationKeys(LinearLayout keys) {
        modifierRow(keys);
        LinearLayout row = keyRow(keys);
        key(row, "Home", KeyEvent.KEYCODE_MOVE_HOME); key(row, "↑", KeyEvent.KEYCODE_DPAD_UP);
        key(row, "End", KeyEvent.KEYCODE_MOVE_END); key(row, "PgUp", KeyEvent.KEYCODE_PAGE_UP);
        row = keyRow(keys);
        key(row, "←", KeyEvent.KEYCODE_DPAD_LEFT); key(row, "↓", KeyEvent.KEYCODE_DPAD_DOWN);
        key(row, "→", KeyEvent.KEYCODE_DPAD_RIGHT); key(row, "PgDn", KeyEvent.KEYCODE_PAGE_DOWN);
        row = keyRow(keys);
        key(row, "Insert", KeyEvent.KEYCODE_INSERT); key(row, "Delete", KeyEvent.KEYCODE_FORWARD_DEL);
        key(row, "Backspace", KeyEvent.KEYCODE_DEL); key(row, "Enter", KeyEvent.KEYCODE_ENTER);
        row = keyRow(keys);
        key(row, ",", KeyEvent.KEYCODE_COMMA); key(row, ".", KeyEvent.KEYCODE_PERIOD);
        key(row, "/", KeyEvent.KEYCODE_SLASH); key(row, "\\", KeyEvent.KEYCODE_BACKSLASH);
        key(row, ";", KeyEvent.KEYCODE_SEMICOLON); key(row, "'", KeyEvent.KEYCODE_APOSTROPHE);
        key(row, "[", KeyEvent.KEYCODE_LEFT_BRACKET); key(row, "]", KeyEvent.KEYCODE_RIGHT_BRACKET);
        key(row, "`", KeyEvent.KEYCODE_GRAVE);
        row = keyRow(keys);
        key(row, "Caps", KeyEvent.KEYCODE_CAPS_LOCK); key(row, "−", KeyEvent.KEYCODE_MINUS);
        key(row, "=", KeyEvent.KEYCODE_EQUALS); key(row, "Scroll", KeyEvent.KEYCODE_SCROLL_LOCK);
    }

    private void numpadKeys(LinearLayout keys) {
        modifierRow(keys);
        LinearLayout row = keyRow(keys);
        key(row, "Num", KeyEvent.KEYCODE_NUM_LOCK); key(row, "/", KeyEvent.KEYCODE_NUMPAD_DIVIDE);
        key(row, "×", KeyEvent.KEYCODE_NUMPAD_MULTIPLY); key(row, "−", KeyEvent.KEYCODE_NUMPAD_SUBTRACT);
        for (int start : new int[] {7, 4, 1}) {
            row = keyRow(keys);
            for (int i = start; i < start + 3; i++) key(row, String.valueOf(i), KeyEvent.KEYCODE_NUMPAD_0 + i);
            if (start == 7) key(row, "+", KeyEvent.KEYCODE_NUMPAD_ADD);
            else if (start == 4) key(row, "Enter", KeyEvent.KEYCODE_NUMPAD_ENTER);
            else key(row, "=", KeyEvent.KEYCODE_NUMPAD_EQUALS);
        }
        row = keyRow(keys);
        key(row, "0", KeyEvent.KEYCODE_NUMPAD_0, 2); key(row, ".", KeyEvent.KEYCODE_NUMPAD_DOT);
        key(row, "⌫", KeyEvent.KEYCODE_DEL);
    }

    private void shortcuts(LinearLayout keys) {
        LinearLayout row = keyRow(keys);
        shortcut(row, R.string.desktop_controls_shortcut_copy, "CTRL+C");
        shortcut(row, R.string.desktop_controls_shortcut_paste, "CTRL+V");
        shortcut(row, R.string.desktop_controls_shortcut_cut, "CTRL+X");
        row = keyRow(keys);
        shortcut(row, R.string.desktop_controls_shortcut_undo, "CTRL+Z");
        shortcut(row, R.string.desktop_controls_shortcut_select_all, "CTRL+A");
        shortcut(row, R.string.desktop_controls_shortcut_desktop, "WIN+D");
        row = keyRow(keys);
        shortcut(row, R.string.keyboard_task_manager, "CTRL+SHIFT+ESC");
        shortcut(row, R.string.keyboard_switch_window, "ALT+TAB");
        shortcut(row, R.string.keyboard_close_window, "ALT+F4");
        List<KeyboardShortcuts.Shortcut> saved = KeyboardShortcuts.load(getContext());
        int index = 0;
        for (KeyboardShortcuts.Shortcut shortcut : saved) {
            if (index++ % 2 == 0) row = keyRow(keys);
            MaterialButton button = button(shortcut.name, () -> sendChord(shortcut.keys()));
            button.setContentDescription(shortcut.name + ": " + KeyboardShortcuts.format(shortcut.keys()));
            addKeyView(row, button, 1);
        }
        MaterialButton edit = button(getContext().getString(R.string.desktop_controls_edit_shortcuts), this::editShortcuts);
        keys.addView(edit, new LayoutParams(LayoutParams.MATCH_PARENT, dp(48)));
    }

    private void shortcut(LinearLayout row, int title, String chord) {
        int[] keys = KeyboardShortcuts.parseChord(chord);
        addKeyView(row, button(getContext().getString(title), () -> sendChord(keys)), 1);
    }

    private MaterialButton key(LinearLayout row, String label, int code) { return key(row, label, code, 1); }
    private MaterialButton key(LinearLayout row, String label, int code, float weight) {
        MaterialButton button = button(label, () -> {
            int[] keys = new int[selectedModifiers.size() + 1];
            int index = 0;
            for (int modifier : selectedModifiers) keys[index++] = modifier;
            keys[index] = code;
            sink.sendKeys(keys);
            if (!config.lockModifiers) resetModifiers();
        });
        addKeyView(row, button, weight);
        return button;
    }

    private void sendChord(int[] keys) {
        sink.sendKeys(keys.clone());
        if (!config.lockModifiers) resetModifiers();
    }

    private void modifier(LinearLayout row, String label, int code) {
        MaterialButton button = button(label, () -> {
            if (!selectedModifiers.remove(code)) selectedModifiers.add(code);
            updateModifiers();
        });
        button.setTag(code);
        button.setContentDescription(getContext().getString(R.string.desktop_controls_modifier_action, label));
        androidx.appcompat.widget.TooltipCompat.setTooltipText(button,
                getContext().getString(R.string.desktop_controls_modifier_action, label));
        button.setOnLongClickListener(view -> {
            sink.sendKeys(new int[] {code});
            resetModifiers();
            return true;
        });
        modifierButtons.add(button);
        addKeyView(row, button, 1.2f);
    }

    private void updateModifiers() {
        for (MaterialButton button : modifierButtons) {
            boolean checked = selectedModifiers.contains((Integer) button.getTag());
            selected(button, checked);
            button.setContentDescription(checked ? getContext().getString(R.string.desktop_controls_selected_modifier,
                    button.getText()) : getContext().getString(R.string.desktop_controls_modifier_action, button.getText()));
        }
    }

    public void resetModifiers() {
        selectedModifiers.clear();
        updateModifiers();
    }

    private void addKeyView(LinearLayout row, MaterialButton button, float weight) {
        button.setTypeface(Typeface.create("sans-serif-medium", Typeface.NORMAL));
        button.setTextSize(12);
        button.setPadding(0, 0, 0, 0);
        button.setBackgroundTintList(ColorStateList.valueOf(color(com.google.android.material.R.attr.colorSurfaceContainerHigh)));
        LayoutParams params = new LayoutParams(0, LayoutParams.MATCH_PARENT, weight);
        params.setMargins(dp(1), dp(2), dp(1), dp(2));
        row.addView(button, params);
    }

    private MaterialButton button(String title, Runnable action) {
        MaterialButton button = new MaterialButton(getContext(), null, androidx.appcompat.R.attr.borderlessButtonStyle);
        button.setText(title);
        button.setAllCaps(false);
        button.setSingleLine(true);
        button.setEllipsize(android.text.TextUtils.TruncateAt.END);
        button.setMinWidth(0); button.setMinimumWidth(0);
        button.setMinHeight(0); button.setMinimumHeight(0);
        button.setInsetTop(0); button.setInsetBottom(0);
        button.setCornerRadius(dp(7));
        button.setPadding(dp(8), 0, dp(8), 0);
        button.setFocusable(false); // Keep Android IME focus on the stream view.
        button.setTextColor(color(com.google.android.material.R.attr.colorOnSurface));
        button.setOnClickListener(view -> action.run());
        return button;
    }

    private void selected(MaterialButton button, boolean selected) {
        button.setSelected(selected);
        button.setBackgroundTintList(ColorStateList.valueOf(selected ?
                color(com.google.android.material.R.attr.colorPrimaryContainer) : Color.TRANSPARENT));
        button.setTextColor(color(selected ? com.google.android.material.R.attr.colorOnPrimaryContainer :
                com.google.android.material.R.attr.colorOnSurface));
    }

    private LinearLayout createPad() {
        LinearLayout pad = new LinearLayout(getContext());
        pad.setOrientation(VERTICAL);
        pad.setClickable(true);
        pad.setFocusable(false);
        pad.setPadding(dp(4), dp(3), dp(4), dp(3));
        trackpad = new DesktopTrackpad(getContext(), config, new DesktopTrackpad.Sink() {
            @Override public void move(float dx, float dy) { sink.move(dx, dy); }
            @Override public void scroll(float vertical, float horizontal) { sink.scroll(vertical, horizontal); }
            @Override public void mouseButton(int button, boolean down) { mouse(button, down, true); }
        });
        pad.addView(trackpad, new LayoutParams(LayoutParams.MATCH_PARENT, 0, 1));
        if (config.mouseButtons) {
            LinearLayout buttons = new LinearLayout(getContext());
            buttons.setMotionEventSplittingEnabled(true);
            pad.addView(buttons, new LayoutParams(LayoutParams.MATCH_PARENT, dp(48)));
            if (config.extraButtons) mouseButton(buttons, "‹", R.string.desktop_controls_back_button, 4);
            mouseButton(buttons, "L", R.string.desktop_controls_left_button, 1);
            if (config.middleButton) mouseButton(buttons, "M", R.string.desktop_controls_middle_button, 2);
            mouseButton(buttons, "R", R.string.desktop_controls_right_button, 3);
            if (config.extraButtons) mouseButton(buttons, "›", R.string.desktop_controls_forward_button, 5);
        }
        return pad;
    }

    @SuppressLint("ClickableViewAccessibility") // performClick is called on release; direct accessibility clicks also send input.
    private void mouseButton(LinearLayout parent, String label, int description, int code) {
        boolean[] touchClick = {false};
        int[] pointer = {-1};
        MaterialButton button = button(label, () -> {
            if (!touchClick[0]) {
                mouse(code, true, false);
                mouse(code, false, false);
            }
        });
        button.setTextSize(16);
        button.setContentDescription(getContext().getString(description));
        button.setOnTouchListener((view, event) -> {
            switch (event.getActionMasked()) {
                case MotionEvent.ACTION_DOWN:
                    pointer[0] = event.getPointerId(0);
                    view.setPressed(true);
                    mouse(code, true, false);
                    return true;
                case MotionEvent.ACTION_UP:
                    mouse(code, false, false);
                    pointer[0] = -1;
                    view.setPressed(false);
                    if ((event.getFlags() & MotionEvent.FLAG_CANCELED) == 0) {
                        touchClick[0] = true;
                        try { view.performClick(); } finally { touchClick[0] = false; }
                    }
                    return true;
                case MotionEvent.ACTION_POINTER_UP:
                    if (event.getPointerId(event.getActionIndex()) != pointer[0]) return true;
                    // Fall through: the owning finger lifted while another remains.
                case MotionEvent.ACTION_CANCEL:
                    mouse(code, false, false);
                    pointer[0] = -1;
                    view.setPressed(false);
                    return true;
                default: return true;
            }
        });
        mouseViews.add(button);
        parent.addView(button, new LayoutParams(0, dp(48), 1));
    }

    private void mouse(int button, boolean down, boolean fromPad) {
        if (button < 1 || button >= buttonHeld.length) return;
        boolean before = buttonHeld[button] || padHeld[button];
        (fromPad ? padHeld : buttonHeld)[button] = down;
        boolean after = buttonHeld[button] || padHeld[button];
        if (before != after) sink.mouseButton(button, after);
    }

    public void releaseInputs() {
        if (trackpad != null) trackpad.releaseInputs();
        for (int button = 1; button < buttonHeld.length; button++) {
            boolean down = buttonHeld[button] || padHeld[button];
            buttonHeld[button] = padHeld[button] = false;
            if (down) sink.mouseButton(button, false);
        }
        for (MaterialButton button : mouseViews) button.setPressed(false);
        resetModifiers();
    }

    public void showCustomization() {
        releaseInputs();
        if (customization != null) return;
        DesktopControlsConfig draft = config.copy();
        LinearLayout fields = new LinearLayout(getContext());
        fields.setOrientation(VERTICAL);
        fields.setPadding(dp(20), dp(4), dp(20), dp(12));
        ScrollView scroll = new ScrollView(getContext());
        scroll.addView(fields);
        slider(fields, R.string.desktop_controls_key_height, draft.keyHeightDp, 36, 56, value -> draft.keyHeightDp = value);
        slider(fields, R.string.desktop_controls_panel_height, getPreferredHeightDp(), 220, 560, value -> {
            draft.panelHeightDp = value;
            draft.panelHeightCustomized = true;
        });
        slider(fields, R.string.desktop_controls_opacity, draft.opacityPercent, 50, 100, value -> draft.opacityPercent = value);
        slider(fields, R.string.desktop_controls_speed, draft.pointerSpeedPercent, 25, 400, value -> draft.pointerSpeedPercent = value);
        check(fields, R.string.desktop_controls_left_handed, draft.leftHanded, value -> draft.leftHanded = value);
        check(fields, R.string.desktop_controls_natural_scroll, draft.naturalScroll, value -> draft.naturalScroll = value);
        check(fields, R.string.desktop_controls_tap_to_click, draft.tapToClick, value -> draft.tapToClick = value);
        check(fields, R.string.desktop_controls_mouse_buttons, draft.mouseButtons, value -> draft.mouseButtons = value);
        check(fields, R.string.desktop_controls_middle_visible, draft.middleButton, value -> draft.middleButton = value);
        check(fields, R.string.desktop_controls_extra_visible, draft.extraButtons, value -> draft.extraButtons = value);
        check(fields, R.string.desktop_controls_lock_modifiers, draft.lockModifiers, value -> draft.lockModifiers = value);
        TextView help = new TextView(getContext());
        help.setText(R.string.desktop_controls_modifier_help);
        help.setTextSize(12);
        help.setPadding(dp(4), 0, dp(4), dp(8));
        fields.addView(help);
        check(fields, R.string.desktop_controls_function_visible, draft.functionBank, value -> draft.functionBank = value);
        check(fields, R.string.desktop_controls_navigation_visible, draft.navigationBank, value -> draft.navigationBank = value);
        check(fields, R.string.desktop_controls_numpad_visible, draft.numpadBank, value -> draft.numpadBank = value);
        check(fields, R.string.desktop_controls_shortcuts_visible, draft.shortcutsBank, value -> draft.shortcutsBank = value);
        MaterialButton edit = button(getContext().getString(R.string.desktop_controls_edit_shortcuts), () -> {
            if (customization != null) customization.dismiss();
            editShortcuts();
        });
        fields.addView(edit, new LayoutParams(LayoutParams.MATCH_PARENT, dp(48)));
        customization = new NavigationDialogBuilder(getContext()).setTitle(R.string.desktop_controls_customize)
                .setView(scroll).setPositiveButton(R.string.desktop_controls_apply, (dialog, which) -> {
                    config = draft.copy();
                    config.save(getContext());
                    rebuild();
                }).setNegativeButton(android.R.string.cancel, null)
                .setNeutralButton(R.string.desktop_controls_reset, (dialog, which) -> confirmReset()).create();
        customization.setOnDismissListener(dialog -> customization = null);
        customization.show();
    }

    private void confirmReset() {
        resetDialog = new NavigationDialogBuilder(getContext()).setTitle(R.string.desktop_controls_reset_title)
                .setMessage(R.string.desktop_controls_reset_message)
                .setPositiveButton(R.string.desktop_controls_reset, (dialog, which) -> {
                    config = new DesktopControlsConfig();
                    config.save(getContext());
                    mode = Mode.KEYBOARD;
                    bank = "typing";
                    rebuild();
                }).setNegativeButton(android.R.string.cancel, null).create();
        resetDialog.setOnDismissListener(dialog -> resetDialog = null);
        resetDialog.show();
    }

    private void slider(LinearLayout fields, int label, int initial, int minimum, int maximum, IntConsumer changed) {
        TextView title = new TextView(getContext());
        title.setText(getContext().getString(label, initial));
        title.setPadding(dp(4), dp(8), 0, 0);
        fields.addView(title);
        SeekBar slider = new SeekBar(getContext());
        slider.setMax(maximum - minimum);
        slider.setProgress(initial - minimum);
        slider.setContentDescription(title.getText());
        slider.setOnSeekBarChangeListener(new SeekBar.OnSeekBarChangeListener() {
            @Override public void onProgressChanged(SeekBar bar, int progress, boolean fromUser) {
                int value = minimum + progress;
                changed.accept(value);
                title.setText(getContext().getString(label, value));
                bar.setContentDescription(title.getText());
            }
            @Override public void onStartTrackingTouch(SeekBar bar) { }
            @Override public void onStopTrackingTouch(SeekBar bar) { }
        });
        fields.addView(slider, new LayoutParams(LayoutParams.MATCH_PARENT, dp(48)));
    }

    private void check(LinearLayout fields, int label, boolean initial, Consumer<Boolean> changed) {
        CheckBox check = new CheckBox(getContext());
        check.setText(label);
        check.setChecked(initial);
        check.setMinHeight(dp(48));
        check.setOnCheckedChangeListener((button, checked) -> changed.accept(checked));
        fields.addView(check, new LayoutParams(LayoutParams.MATCH_PARENT, LayoutParams.WRAP_CONTENT));
    }

    private void editShortcuts() {
        releaseInputs();
        if (shortcutList != null) return;
        List<KeyboardShortcuts.Shortcut> entries = KeyboardShortcuts.load(getContext());
        LinearLayout list = new LinearLayout(getContext());
        list.setOrientation(VERTICAL);
        list.setPadding(dp(16), 0, dp(16), dp(8));
        ScrollView scroll = new ScrollView(getContext());
        scroll.addView(list);
        if (entries.isEmpty()) {
            TextView empty = new TextView(getContext());
            empty.setText(R.string.desktop_controls_no_shortcuts);
            empty.setPadding(dp(8), dp(8), dp(8), dp(8));
            list.addView(empty);
        }
        for (int i = 0; i < entries.size(); i++) {
            KeyboardShortcuts.Shortcut shortcut = entries.get(i);
            LinearLayout row = new LinearLayout(getContext());
            MaterialButton edit = button(shortcut.name, () -> editShortcut(shortcut));
            edit.setContentDescription(shortcut.name + ": " + KeyboardShortcuts.format(shortcut.keys()));
            row.addView(edit, new LayoutParams(0, dp(48), 1));
            for (int step : new int[] {-1, 1}) {
                int target = i + step;
                MaterialButton move = button(step < 0 ? "↑" : "↓", () -> {
                    List<KeyboardShortcuts.Shortcut> fresh = KeyboardShortcuts.load(getContext());
                    int from = indexOf(fresh, shortcut.id);
                    if (from < 0 || target < 0 || target >= fresh.size()) return;
                    fresh.add(target, fresh.remove(from));
                    KeyboardShortcuts.save(getContext(), fresh);
                    refreshShortcutList();
                });
                move.setEnabled(target >= 0 && target < entries.size());
                move.setContentDescription(getContext().getString(step < 0 ?
                        R.string.desktop_controls_shortcut_up : R.string.desktop_controls_shortcut_down));
                row.addView(move, new LayoutParams(dp(48), dp(48)));
            }
            list.addView(row);
        }
        shortcutList = new NavigationDialogBuilder(getContext()).setTitle(R.string.keyboard_shortcuts_title)
                .setView(scroll).setPositiveButton(android.R.string.ok, null)
                .setNeutralButton(R.string.keyboard_shortcuts_add, null).create();
        shortcutList.setOnDismissListener(dialog -> { shortcutList = null; rebuild(); });
        shortcutList.setOnShowListener(dialog -> {
            shortcutList.getButton(AlertDialog.BUTTON_NEUTRAL).setEnabled(entries.size() < KeyboardShortcuts.MAX_SHORTCUTS);
            shortcutList.getButton(AlertDialog.BUTTON_NEUTRAL).setOnClickListener(view -> editShortcut(null));
        });
        shortcutList.show();
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
        LinearLayout fields = new LinearLayout(getContext());
        fields.setOrientation(VERTICAL);
        fields.setPadding(dp(20), 0, dp(20), dp(8));
        EditText name = new EditText(getContext());
        name.setSingleLine(true);
        name.setHint(R.string.keyboard_shortcuts_name);
        name.setFilters(new android.text.InputFilter[] {new android.text.InputFilter.LengthFilter(80)});
        fields.addView(name);
        EditText chord = new EditText(getContext());
        chord.setSingleLine(true);
        chord.setHint(R.string.keyboard_shortcuts_chord);
        chord.setInputType(android.text.InputType.TYPE_CLASS_TEXT | android.text.InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS);
        chord.setFilters(new android.text.InputFilter[] {new android.text.InputFilter.LengthFilter(256)});
        fields.addView(chord);
        if (existing != null) {
            name.setText(existing.name);
            chord.setText(KeyboardShortcuts.format(existing.keys()));
        }
        ScrollView editorScroll = new ScrollView(getContext());
        editorScroll.addView(fields);
        MaterialAlertDialogBuilder builder = new NavigationDialogBuilder(getContext())
                .setTitle(existing == null ? R.string.keyboard_shortcuts_add : R.string.keyboard_shortcuts_edit)
                .setView(editorScroll).setPositiveButton(R.string.keyboard_shortcuts_save, null)
                .setNegativeButton(android.R.string.cancel, null);
        if (existing != null) builder.setNeutralButton(R.string.keyboard_shortcuts_delete, (dialog, which) -> {
            List<KeyboardShortcuts.Shortcut> entries = KeyboardShortcuts.load(getContext());
            entries.removeIf(entry -> entry.id.equals(existing.id));
            KeyboardShortcuts.save(getContext(), entries);
            refreshShortcutList();
        });
        shortcutEdit = builder.create();
        shortcutEdit.setOnDismissListener(dialog -> shortcutEdit = null);
        shortcutEdit.setOnShowListener(dialog -> shortcutEdit.getButton(AlertDialog.BUTTON_POSITIVE).setOnClickListener(view -> {
            try {
                KeyboardShortcuts.Shortcut shortcut = new KeyboardShortcuts.Shortcut(existing == null ?
                        UUID.randomUUID().toString() : existing.id, name.getText().toString(),
                        KeyboardShortcuts.parseChord(chord.getText().toString()));
                List<KeyboardShortcuts.Shortcut> entries = KeyboardShortcuts.load(getContext());
                int index = indexOf(entries, shortcut.id);
                if (index >= 0) entries.set(index, shortcut);
                else entries.add(shortcut);
                KeyboardShortcuts.save(getContext(), entries);
                AlertDialog previous = shortcutEdit;
                shortcutEdit = null;
                previous.setOnDismissListener(null);
                previous.dismiss();
                refreshShortcutList();
            } catch (IllegalArgumentException error) {
                chord.setError(getContext().getString(R.string.keyboard_shortcuts_invalid));
            }
        }));
        shortcutEdit.show();
    }

    @Override protected void onMeasure(int widthMeasureSpec, int heightMeasureSpec) {
        boolean nowWide = MeasureSpec.getSize(widthMeasureSpec) >= dp(650);
        if (wide != nowWide) {
            wide = nowWide;
            rebuild();
        }
        int available = MeasureSpec.getMode(heightMeasureSpec) == MeasureSpec.UNSPECIFIED ?
                dp(getPreferredHeightDp()) : MeasureSpec.getSize(heightMeasureSpec);
        int height = Math.min(dp(getPreferredHeightDp()), available);
        super.onMeasure(widthMeasureSpec, MeasureSpec.makeMeasureSpec(height, MeasureSpec.EXACTLY));
    }

    @Override public void onWindowFocusChanged(boolean hasWindowFocus) {
        super.onWindowFocusChanged(hasWindowFocus);
        if (!hasWindowFocus) releaseInputs();
    }

    @Override protected void onVisibilityChanged(View changedView, int visibility) {
        super.onVisibilityChanged(changedView, visibility);
        if (visibility != VISIBLE && selectedModifiers != null) releaseInputs();
    }

    @Override protected void onDetachedFromWindow() {
        releaseInputs();
        if (customization != null) customization.dismiss();
        if (shortcutEdit != null) shortcutEdit.dismiss();
        if (shortcutList != null) {
            shortcutList.setOnDismissListener(null);
            shortcutList.dismiss();
            shortcutList = null;
        }
        if (resetDialog != null) resetDialog.dismiss();
        super.onDetachedFromWindow();
    }
}
