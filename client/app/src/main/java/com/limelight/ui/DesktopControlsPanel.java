package com.limelight.ui;

import android.annotation.SuppressLint;
import android.content.Context;
import android.content.res.ColorStateList;
import android.graphics.Color;
import android.graphics.Typeface;
import android.view.Gravity;
import android.view.MotionEvent;
import android.view.View;
import android.widget.HorizontalScrollView;
import android.widget.LinearLayout;
import android.widget.ScrollView;
import android.widget.TextView;

import androidx.appcompat.app.AlertDialog;
import androidx.appcompat.widget.TooltipCompat;
import androidx.core.content.ContextCompat;

import com.google.android.material.button.MaterialButton;
import com.google.android.material.color.MaterialColors;
import com.limelight.R;
import com.limelight.binding.input.DesktopControlsConfig;

import java.util.ArrayList;
import java.util.LinkedHashSet;
import java.util.List;
import java.util.Locale;
import java.util.Set;
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
    private final DesktopControlsDialogs dialogs;
    private DesktopControlsConfig config;
    private Mode mode;
    private String bank;
    private LinearLayout header;
    private HeaderPicker headerPicker = HeaderPicker.NONE;
    private boolean wide, haptics = true;
    private DesktopTrackpad trackpad;
    private final Set<Integer> selectedModifiers = new LinkedHashSet<>();
    private final List<MaterialButton> modifierButtons = new ArrayList<>();
    private final List<MaterialButton> mouseViews = new ArrayList<>();
    // The gesture pad and held buttons can own the same button without releasing each other.
    private final boolean[] buttonHeld = new boolean[6], padHeld = new boolean[6];
    private AlertDialog resetDialog;

    public DesktopControlsPanel(Context context, Sink sink, Runnable systemKeyboard, Runnable hide) {
        super(context);
        this.sink = sink;
        this.systemKeyboard = systemKeyboard;
        this.hide = hide;
        this.dialogs = new DesktopControlsDialogs(this);
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

    int dp(float value) { return Math.round(value * getResources().getDisplayMetrics().density); }
    private int color(int attribute) { return MaterialColors.getColor(getContext(), attribute, Color.DKGRAY); }

    void applyConfig(DesktopControlsConfig draft) {
        config = draft.copy();
        config.save(getContext());
        rebuild();
    }

    void rebuild() {
        releaseInputs();
        removeAllViews();
        modifierButtons.clear();
        mouseViews.clear();
        trackpad = null;
        if (!bankEnabled(bank)) bank = "typing";
        // The stream's glass for what stays up while the game plays: no blur, the keys carry their own fill. The opacity
        // setting is the panel fill's alpha, as before; the keys stay opaque.
        GlassPanelDrawable background = GlassPanelDrawable.hud(getContext(), new float[] {dp(18), dp(18), dp(18), dp(18), 0, 0, 0, 0});
        background.setFillOpacity(config.opacityPercent / 100f);
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
        DesktopControlsKeyBanks.populate(this, keys, bank);
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

    LinearLayout keyRow(LinearLayout parent) {
        LinearLayout row = new LinearLayout(getContext());
        row.setGravity(Gravity.CENTER_VERTICAL);
        parent.addView(row, new LayoutParams(LayoutParams.MATCH_PARENT, dp(config.keyHeightDp)));
        return row;
    }

    MaterialButton key(LinearLayout row, String label, int code) { return key(row, label, code, 1); }
    MaterialButton key(LinearLayout row, String label, int code, float weight) {
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

    void sendChord(int[] keys) {
        sink.sendKeys(keys.clone());
        if (!config.lockModifiers) resetModifiers();
    }

    void modifier(LinearLayout row, String label, int code) {
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

    void addKeyView(LinearLayout row, MaterialButton button, float weight) {
        button.setTypeface(Typeface.create("sans-serif-medium", Typeface.NORMAL));
        button.setTextSize(12);
        button.setPadding(0, 0, 0, 0);
        button.setBackgroundTintList(null);
        button.setBackground(new GlassControlDrawable(getContext(), GlassControlDrawable.Kind.KEY, dp(7)));
        button.setStateListAnimator(null);
        LayoutParams params = new LayoutParams(0, LayoutParams.MATCH_PARENT, weight);
        params.setMargins(dp(1), dp(2), dp(1), dp(2));
        row.addView(button, params);
        // A key answers the finger going down, like the PC keyboard's and the mouse buttons here; the click still sends it.
        button.setOnTouchListener((view, event) -> {
            if (haptics && event.getActionMasked() == MotionEvent.ACTION_DOWN) Haptics.perform(view, Haptics.Kind.KEY);
            return false;
        });
    }

    MaterialButton button(String title, Runnable action) {
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
        if (button.getBackground() instanceof GlassControlDrawable) {
            // A key's own drawable shows it selected.
            button.setTextColor(selected ? MaterialColors.getColor(StreamGlass.themed(getContext()), R.attr.glassOnSelected, 0)
                    : ContextCompat.getColor(getContext(), AppTheme.isHighContrast(getContext()) ? R.color.nj_hc_color_label : R.color.nj_color_label));
            return;
        }
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
        trackpad.setHaptics(haptics);
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
                    if (haptics) Haptics.perform(view, Haptics.Kind.CLICK);
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

    /** Whether the trackpad's taps and the mouse buttons vibrate: the stream's Vibrate on press. */
    public void setHaptics(boolean haptics) {
        this.haptics = haptics;
        if (trackpad != null) trackpad.setHaptics(haptics);
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
        dialogs.showCustomization(config);
    }

    void editShortcuts() {
        dialogs.editShortcuts();
    }

    void confirmReset() {
        resetDialog = new NavigationDialogBuilder(getContext()).startOnCancel().setTitle(R.string.desktop_controls_reset_title)
                .setMessage(R.string.desktop_controls_reset_message)
                .setPositiveButton(R.string.desktop_controls_reset, (dialog, which) -> {
                    config = new DesktopControlsConfig();
                    config.save(getContext());
                    mode = Mode.KEYBOARD;
                    bank = "typing";
                    rebuild();
                }).setNegativeButton(android.R.string.cancel, null).create();
        resetDialog.setOnDismissListener(dialog -> resetDialog = null);
        StreamGlass.show(resetDialog);
    }

    void addSlider(LinearLayout fields, int label, int initial, int minimum, int maximum, IntConsumer changed) {
        TextView title = new TextView(getContext());
        title.setText(getContext().getString(label, initial));
        title.setPadding(dp(4), dp(8), 0, 0);
        fields.addView(title);
        StepSlider slider = new StepSlider(getContext());
        slider.setMax(maximum - minimum);
        slider.setPosition(initial - minimum);
        slider.setContentDescription(title.getText());
        slider.setPositionText(progress -> valueOf(getContext().getString(label, minimum + progress), minimum + progress));
        slider.setOnPositionChangeListener((bar, progress) -> {
            int value = minimum + progress;
            changed.accept(value);
            title.setText(getContext().getString(label, value));
            bar.setContentDescription(title.getText());
        });
        fields.addView(slider, new LayoutParams(LayoutParams.MATCH_PARENT, dp(48)));
    }

    /** The value and its unit out of a slider's title, {@code 40 dp} out of {@code Key height: 40 dp}. */
    static String valueOf(String title, int value) {
        int at = title.lastIndexOf(String.valueOf(value));
        return at < 0 ? String.valueOf(value) : title.substring(at).trim();
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
        dialogs.dismissAll();
        if (resetDialog != null) resetDialog.dismiss();
        super.onDetachedFromWindow();
    }
}
