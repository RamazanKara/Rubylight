package com.limelight.ui;

import android.annotation.SuppressLint;
import android.app.Activity;
import android.app.Dialog;
import android.content.ClipData;
import android.content.ClipboardManager;
import android.content.Context;
import android.content.Intent;
import android.graphics.Canvas;
import android.graphics.Color;
import android.graphics.Paint;
import android.os.SystemClock;
import android.text.InputFilter;
import android.text.InputType;
import android.view.InputDevice;
import android.view.MotionEvent;
import android.view.View;
import android.view.ViewGroup;
import android.widget.ArrayAdapter;
import android.widget.CheckBox;
import android.widget.EditText;
import android.widget.FrameLayout;
import android.widget.LinearLayout;
import android.widget.ScrollView;
import android.widget.Spinner;
import android.widget.TextView;
import androidx.appcompat.app.AlertDialog;
import androidx.core.view.ViewCompat;
import com.google.android.material.button.MaterialButton;
import com.google.android.material.dialog.MaterialAlertDialogBuilder;
import com.limelight.R;
import com.limelight.binding.input.HeldInputState;
import com.limelight.binding.input.KeyboardShortcuts;
import com.limelight.binding.input.LayoutGamepad;
import com.limelight.binding.input.LayoutStick;
import com.limelight.binding.input.TouchLayout;
import com.limelight.binding.input.TouchLayoutImport;
import java.text.NumberFormat;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.UUID;
import java.util.function.Consumer;

/** Independent per-game overlay. Touches beside the controls go to the stream's mouse/touch handling. */
@SuppressLint("ViewConstructor") // Created by Game with explicit profile/input ownership; never inflated from XML.
public final class GameTouchControls extends FrameLayout {
    private static final TouchLayout.Kind[] ADDABLE = {TouchLayout.Kind.KEYBOARD, TouchLayout.Kind.GAMEPAD,
            TouchLayout.Kind.STICK, TouchLayout.Kind.TRIGGER, TouchLayout.Kind.DPAD, TouchLayout.Kind.WASD};
    private final Activity activity;
    private String profile;
    private final Consumer<Dialog> showDialog;
    private final HeldInputState held;
    private final LayoutGamepad gamepad;
    private Runnable resend;
    // Not the view's own queue: a detached view holds posts until it is attached again.
    private final android.os.Handler resends = new android.os.Handler(android.os.Looper.getMainLooper());
    private boolean resending;
    private final java.util.function.BooleanSupplier canInput;
    private TouchLayout layout;
    private boolean editing, enabled, hiddenForPip;
    private int inputGeneration, snapGrid;
    private final Paint gridPaint = new Paint();
    // Bottom child covering the overlay. Splitting gives it every pointer that lands beside a
    // control, so a held control never swallows another finger aiming on the stream.
    private final View passthrough;
    private View.OnTouchListener streamTouch;
    private Runnable gestureCancelled;

    @SuppressLint("ClickableViewAccessibility") // The passthrough layer only forwards touches; it has no click.
    public GameTouchControls(Activity activity, String profile, HeldInputState.Sink sink,
                             java.util.function.BooleanSupplier canInput, Consumer<Dialog> showDialog) {
        super(activity);
        this.activity = activity; this.profile = profile; this.showDialog = showDialog; this.canInput = canInput;
        held = new HeldInputState(sink);
        gamepad = new LayoutGamepad(new HeldInputState.Sink() {
            @Override public void key(int key, boolean down) { }
            @Override public void mouse(int button, boolean down) { }
            @Override public void gamepad(int buttons, short leftX, short leftY, short rightX, short rightY,
                                          byte leftTrigger, byte rightTrigger) {
                sink.gamepad(buttons, leftX, leftY, rightX, rightY, leftTrigger, rightTrigger);
                if (resending) return;
                // As the fixed gamepad does: some hosts drop a report that follows another closely,
                // and a lost centering report would leave a stick pushed.
                resends.removeCallbacks(resend);
                resends.postDelayed(resend, 25);
                resends.postDelayed(resend, 50);
                resends.postDelayed(resend, 75);
            }
        });
        resend = () -> {
            resending = true;
            try { gamepad.resend(); } finally { resending = false; }
        };
        layout = TouchLayout.load(activity, profile);
        enabled = TouchLayout.enabled(activity, profile);
        snapGrid = TouchLayout.snapGrid(activity);
        gridPaint.setColor(0x40FFFFFF);
        gridPaint.setStrokeWidth(Math.max(1, getResources().getDisplayMetrics().density));
        setWillNotDraw(false);
        setClipChildren(true);
        setMotionEventSplittingEnabled(true);
        setVisibility(enabled ? VISIBLE : GONE);
        passthrough = new View(activity);
        passthrough.setImportantForAccessibility(IMPORTANT_FOR_ACCESSIBILITY_NO);
        passthrough.setOnTouchListener((v, event) -> streamTouch != null && streamTouch.onTouch(v, event));
        addView(passthrough, new LayoutParams(LayoutParams.MATCH_PARENT, LayoutParams.MATCH_PARENT));
        addOnLayoutChangeListener((v, l, t, r, b, ol, ot, or, ob) -> positionButtons());
        rebuild();
    }

    /** Receives touches beside the controls in this overlay's coordinates, each pointer exactly once. */
    public void setStreamTouchListener(View.OnTouchListener listener) {
        streamTouch = listener;
    }

    /** Runs after showing or hiding this overlay ended the touch gesture in progress. */
    public void setGestureCancelListener(Runnable listener) {
        gestureCancelled = listener;
    }

    @Override public void setVisibility(int visibility) {
        boolean changed = visibility != getVisibility();
        super.setVisibility(visibility);
        if (!changed) return;
        // Fingers already down stay with the view that took them, while new ones now land on this
        // overlay or on the stream below it. Both feed the same stream touch state, so a host touch
        // could stay pressed. End the gesture for every view in the container instead.
        if (getParent() instanceof ViewGroup) {
            long now = SystemClock.uptimeMillis();
            MotionEvent cancel = MotionEvent.obtain(now, now, MotionEvent.ACTION_CANCEL, 0, 0, 0);
            cancel.setSource(InputDevice.SOURCE_TOUCHSCREEN);
            try { ((ViewGroup) getParent()).dispatchTouchEvent(cancel); }
            finally { cancel.recycle(); }
        }
        if (gestureCancelled != null) gestureCancelled.run();
    }

    private int buttonCount() {
        return getChildCount() - 1;
    }

    private View buttonAt(int index) {
        return getChildAt(index + 1);
    }

    /** Select the established game's existing controls without migrating either saved layout. */
    public void setProfile(String profile) {
        if (this.profile.equals(profile)) return;
        releaseInput();
        editing = false;
        this.profile = profile;
        layout = TouchLayout.load(activity, profile);
        enabled = TouchLayout.enabled(activity, profile);
        setVisibility(hiddenForPip || !enabled ? GONE : VISIBLE);
        rebuild();
    }

    /** Lets go of everything: keys and mouse buttons up, gamepad buttons up, triggers to 0, sticks centerd. */
    public void releaseInput() {
        inputGeneration++;
        held.releaseAll();
        for (int i = 0; i < buttonCount(); i++) {
            View view = buttonAt(i); view.setSelected(false); view.setPressed(false); view.setAlpha(.8f);
            if (view instanceof LayoutPadView) {
                LayoutPadView pad = (LayoutPadView) view;
                pad.stick.release();
                pad.directions = 0;
                pad.invalidate();
            } else {
                ViewCompat.setStateDescription(view, activity.getString(R.string.touch_layout_released));
            }
        }
        gamepad.releaseAll();
    }
    public void hideForPip(boolean hide) {
        releaseInput();
        hiddenForPip = hide;
        setVisibility(hide || !enabled ? GONE : VISIBLE);
    }
    public boolean isEditing() { return editing; }

    /** The visible game buttons, in the coordinates of {@code parent}, for overlays to keep clear of. */
    public void collectVisibleBounds(View parent, List<android.graphics.Rect> out) {
        if (getVisibility() != VISIBLE) return;
        for (int i = 0; i < buttonCount(); i++) OverlayPlacement.addVisible(buttonAt(i), parent, out);
    }

    public void showMenu() {
        releaseInput();
        List<String> names = new ArrayList<>();
        names.add(activity.getString(enabled ? R.string.touch_layout_hide : R.string.touch_layout_show));
        names.add(activity.getString(R.string.touch_layout_add));
        names.add(activity.getString(editing ? R.string.touch_layout_save : R.string.touch_layout_move));
        names.add(activity.getString(R.string.touch_layout_snap, gridName(activity, snapGrid)));
        names.add(activity.getString(R.string.touch_layout_import));
        names.add(activity.getString(R.string.touch_layout_export));
        for (TouchLayout.Button b : layout.buttons) names.add(b.name);
        showDialog.accept(new MaterialAlertDialogBuilder(activity).setTitle(R.string.touch_layout_title)
                .setItems(names.toArray(new String[0]), (dialog, which) -> {
                    // Wait for the stream menu's dismissal/focus restoration before showing another dialog.
                    post(() -> {
                        if (which == 0) {
                            enabled = !enabled; editing = false;
                            TouchLayout.setEnabled(activity, profile, enabled);
                            setVisibility(enabled ? VISIBLE : GONE);
                        } else if (which == 1) {
                            if (layout.buttons.size() < TouchLayout.MAX_BUTTONS) chooseKind();
                            else info(R.string.touch_layout_limit);
                        } else if (which == 2) {
                            editing = !editing; enabled = true;
                            TouchLayout.setEnabled(activity, profile, true);
                            setVisibility(VISIBLE);
                            if (!editing) layout.save(activity, profile);
                            else info(R.string.touch_layout_move_help);
                            rebuild();
                        } else if (which == 3) chooseGrid();
                        else if (which == 4) importLayout();
                        else if (which == 5) exportLayout();
                        else edit(layout.buttons.get(which - 6), layout.buttons.get(which - 6).kind);
                    });
                }).setNegativeButton(android.R.string.cancel, null).create());
    }

    private void info(int message) {
        showDialog.accept(new MaterialAlertDialogBuilder(activity).setMessage(message)
                .setPositiveButton(android.R.string.ok, null).create());
    }

    private void replace(TouchLayout next) {
        releaseInput(); layout = next; layout.save(activity, profile); rebuild();
    }

    static String gridName(Context context, int grid) {
        if (grid == 0) return context.getString(R.string.stream_disabled);
        NumberFormat format = NumberFormat.getPercentInstance();
        format.setMaximumFractionDigits(1);
        return format.format(grid / 1000.0);
    }

    private void chooseGrid() {
        String[] names = new String[TouchLayout.SNAP_GRIDS.length];
        int checked = 0;
        for (int i = 0; i < names.length; i++) {
            names[i] = gridName(activity, TouchLayout.SNAP_GRIDS[i]);
            if (TouchLayout.SNAP_GRIDS[i] == snapGrid) checked = i;
        }
        showDialog.accept(new MaterialAlertDialogBuilder(activity).setTitle(R.string.touch_layout_snap_title)
                .setSingleChoiceItems(names, checked, (dialog, which) -> {
                    snapGrid = TouchLayout.SNAP_GRIDS[which];
                    TouchLayout.setSnapGrid(activity, snapGrid);
                    invalidate();
                    dialog.dismiss();
                }).setNegativeButton(android.R.string.cancel, null).create());
    }

    private static int kindName(TouchLayout.Kind kind) {
        switch (kind) {
            case GAMEPAD: return R.string.touch_layout_kind_gamepad;
            case STICK: return R.string.touch_layout_kind_stick;
            case TRIGGER: return R.string.touch_layout_kind_trigger;
            case DPAD: return R.string.touch_layout_kind_dpad;
            case WASD: return R.string.touch_layout_kind_wasd;
            default: return R.string.touch_layout_kind_button;
        }
    }

    private void chooseKind() {
        String[] names = new String[ADDABLE.length];
        for (int i = 0; i < names.length; i++) names[i] = activity.getString(kindName(ADDABLE[i]));
        showDialog.accept(new MaterialAlertDialogBuilder(activity).setTitle(R.string.touch_layout_add)
                .setItems(names, (dialog, which) -> post(() -> edit(null, ADDABLE[which])))
                .setNegativeButton(android.R.string.cancel, null).create());
    }

    // Default size and place for a new control, as fractions: x, y, width, height.
    private static float[] defaults(TouchLayout.Kind kind) {
        switch (kind) {
            case GAMEPAD: return new float[] {.82f, .55f, .09f, .16f};
            case TRIGGER: return new float[] {.82f, .08f, .12f, .14f};
            case DPAD: return new float[] {.04f, .1f, .2f, .36f};
            case STICK: case WASD: return new float[] {.04f, .5f, .22f, .4f};
            default: return new float[] {.06f, .65f, .14f, .16f};
        }
    }

    private Spinner choice(LinearLayout parent, int label, String[] values, int selected) {
        TextView title = new TextView(activity); title.setText(label); parent.addView(title);
        Spinner spinner = new Spinner(activity); spinner.setId(View.generateViewId()); title.setLabelFor(spinner.getId());
        ArrayAdapter<String> adapter = new ArrayAdapter<>(activity, android.R.layout.simple_spinner_item, values);
        adapter.setDropDownViewResource(android.R.layout.simple_spinner_dropdown_item);
        spinner.setAdapter(adapter);
        spinner.setSelection(selected);
        parent.addView(spinner);
        return spinner;
    }

    private void edit(TouchLayout.Button existing, TouchLayout.Kind kind) {
        LinearLayout fields = new LinearLayout(activity); fields.setOrientation(LinearLayout.VERTICAL);
        int pad = Math.round(20 * getResources().getDisplayMetrics().density);
        fields.setPadding(pad, 0, pad, 0);
        float[] place = defaults(kind);
        EditText name = field(fields, R.string.touch_layout_name, existing == null ? "" : existing.name, 80);
        EditText binding = null;
        Spinner choice = null;
        if (kind == TouchLayout.Kind.KEYBOARD || kind == TouchLayout.Kind.MOUSE) {
            binding = field(fields, R.string.touch_layout_binding, existing == null ? "" :
                    existing.mouseButton == 0 ? KeyboardShortcuts.format(existing.keys()) : "MOUSE_" + existing.mouseButton, 256);
            TextView help = new TextView(activity); help.setText(R.string.touch_layout_binding_help); fields.addView(help);
        } else if (kind == TouchLayout.Kind.WASD) {
            binding = field(fields, R.string.touch_layout_wasd_keys, KeyboardShortcuts.format(existing == null ?
                    new int[] {51, 29, 47, 32} : existing.keys()), 256);
            TextView help = new TextView(activity); help.setText(R.string.touch_layout_wasd_help); fields.addView(help);
        } else if (kind == TouchLayout.Kind.GAMEPAD) {
            String current = existing == null ? "A" : TouchLayout.gamepadName(existing.gamepadButton);
            choice = choice(fields, R.string.touch_layout_kind_gamepad, TouchLayout.GAMEPAD_BUTTONS,
                    java.util.Arrays.asList(TouchLayout.GAMEPAD_BUTTONS).indexOf(current));
        } else if (kind == TouchLayout.Kind.STICK || kind == TouchLayout.Kind.TRIGGER) {
            choice = choice(fields, R.string.touch_layout_side, new String[] {activity.getString(R.string.touch_layout_side_left),
                    activity.getString(R.string.touch_layout_side_right)}, existing == null ? 0 : existing.side);
        }
        CheckBox floating = null;
        EditText deadzone = null, diagonal = null;
        if (kind == TouchLayout.Kind.STICK || kind == TouchLayout.Kind.WASD) {
            floating = new CheckBox(activity); floating.setText(R.string.touch_layout_floating);
            floating.setChecked(existing != null && existing.floating); fields.addView(floating);
            deadzone = number(fields, R.string.touch_layout_deadzone,
                    Math.round((existing == null ? TouchLayout.DEFAULT_DEADZONE : existing.deadzone) * 100));
        }
        if (kind == TouchLayout.Kind.WASD) {
            diagonal = number(fields, R.string.touch_layout_diagonal,
                    Math.round(existing == null ? TouchLayout.DEFAULT_DIAGONAL : existing.diagonal));
        }
        EditText width = number(fields, R.string.touch_layout_width, Math.round((existing == null ? place[2] : existing.width) * 100));
        EditText height = number(fields, R.string.touch_layout_height, Math.round((existing == null ? place[3] : existing.height) * 100));
        CheckBox toggle = null;
        if (!kind.directional()) {
            toggle = new CheckBox(activity); toggle.setText(R.string.touch_layout_toggle);
            toggle.setChecked(existing != null && existing.toggle); fields.addView(toggle);
        }
        ScrollView scroll = new ScrollView(activity); scroll.addView(fields);
        MaterialAlertDialogBuilder builder = new MaterialAlertDialogBuilder(activity)
                .setTitle(existing == null ? activity.getString(kindName(kind)) : existing.name)
                .setView(scroll).setPositiveButton(R.string.keyboard_shortcuts_save, null)
                .setNegativeButton(android.R.string.cancel, null);
        if (existing != null) builder.setNeutralButton(R.string.keyboard_shortcuts_delete, (dialog, which) -> {
            List<TouchLayout.Button> entries = new ArrayList<>(layout.buttons);
            entries.removeIf(b -> b.id.equals(existing.id)); replace(new TouchLayout(entries));
        });
        AlertDialog dialog = builder.create();
        EditText bindingField = binding, deadzoneField = deadzone, diagonalField = diagonal;
        Spinner choiceField = choice;
        CheckBox floatingBox = floating, toggleBox = toggle;
        dialog.setOnShowListener(ignored -> dialog.getButton(AlertDialog.BUTTON_POSITIVE).setOnClickListener(v -> {
            try {
                float w = Integer.parseInt(width.getText().toString()) / 100f;
                float h = Integer.parseInt(height.getText().toString()) / 100f;
                String id = existing == null ? UUID.randomUUID().toString() : existing.id;
                float x = Math.min(existing == null ? place[0] : existing.x, 1 - w);
                float y = Math.min(existing == null ? place[1] : existing.y, 1 - h);
                boolean toggled = toggleBox != null && toggleBox.isChecked();
                int selected = choiceField == null ? 0 : choiceField.getSelectedItemPosition();
                String label = name.getText().toString();
                TouchLayout.Button b;
                switch (kind) {
                    case GAMEPAD:
                        String button = TouchLayout.GAMEPAD_BUTTONS[selected];
                        b = TouchLayout.Button.gamepad(id, label.trim().isEmpty() ? button : label,
                                TouchLayout.gamepadFlag(button), toggled, x, y, w, h);
                        break;
                    case TRIGGER:
                        b = TouchLayout.Button.trigger(id, label.trim().isEmpty() ? (selected == 0 ? "LT" : "RT") : label,
                                selected, toggled, x, y, w, h);
                        break;
                    case STICK:
                        b = TouchLayout.Button.stick(id, label.trim().isEmpty() ? (selected == 0 ? "LS" : "RS") : label,
                                selected, floatingBox.isChecked(), percent(deadzoneField), x, y, w, h);
                        break;
                    case DPAD:
                        b = TouchLayout.Button.dpad(id, label.trim().isEmpty() ? activity.getString(R.string.touch_layout_kind_dpad) : label,
                                x, y, w, h);
                        break;
                    case WASD:
                        int[] keys = KeyboardShortcuts.parseChord(bindingField.getText().toString());
                        b = TouchLayout.Button.wasd(id, label.trim().isEmpty() ? KeyboardShortcuts.format(keys) : label, keys,
                                floatingBox.isChecked(), percent(deadzoneField),
                                Integer.parseInt(diagonalField.getText().toString()), x, y, w, h);
                        break;
                    default:
                        String value = bindingField.getText().toString().trim().toUpperCase(java.util.Locale.ROOT);
                        int mouse = value.matches("MOUSE_[1-5]") ? value.charAt(6) - '0' : 0;
                        b = new TouchLayout.Button(id, label, mouse == 0 ? KeyboardShortcuts.parseChord(value) : new int[0],
                                mouse, toggled, x, y, w, h);
                        break;
                }
                List<TouchLayout.Button> entries = new ArrayList<>(layout.buttons);
                int index = existing == null ? -1 : entries.indexOf(existing);
                if (index < 0) entries.add(b); else entries.set(index, b);
                enabled = true; TouchLayout.setEnabled(activity, profile, true); setVisibility(VISIBLE);
                replace(new TouchLayout(entries)); dialog.dismiss();
            } catch (IllegalArgumentException e) {
                (bindingField != null ? bindingField : name).setError(activity.getString(R.string.touch_layout_invalid));
            }
        }));
        showDialog.accept(dialog);
    }

    private static float percent(EditText field) {
        return Integer.parseInt(field.getText().toString()) / 100f;
    }

    private EditText number(LinearLayout parent, int label, int value) {
        EditText edit = field(parent, label, Integer.toString(value), 2);
        edit.setInputType(InputType.TYPE_CLASS_NUMBER);
        return edit;
    }

    private EditText field(LinearLayout parent, int label, String value, int length) {
        TextView title = new TextView(activity); title.setText(label); parent.addView(title);
        EditText edit = new EditText(activity); edit.setId(View.generateViewId()); title.setLabelFor(edit.getId());
        edit.setSingleLine(true); edit.setText(value); edit.setFilters(new InputFilter[] {new InputFilter.LengthFilter(length)});
        parent.addView(edit); return edit;
    }

    private void importLayout() {
        EditText text = new EditText(activity); text.setHint(R.string.touch_layout_paste);
        text.setInputType(InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_FLAG_MULTI_LINE | InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS);
        text.setMaxLines(8); text.setFilters(new InputFilter[] {new InputFilter.LengthFilter(TouchLayout.MAX_BYTES)});
        AlertDialog dialog = new MaterialAlertDialogBuilder(activity).setTitle(R.string.touch_layout_import)
                .setMessage(R.string.touch_layout_import_help).setView(text)
                .setPositiveButton(R.string.keyboard_shortcuts_replace_button, null)
                .setNegativeButton(android.R.string.cancel, null).create();
        dialog.setOnShowListener(ignored -> dialog.getButton(AlertDialog.BUTTON_POSITIVE).setOnClickListener(v -> {
            try {
                // Hidden controls are not measured; they fill the stream surface they sit on.
                View surface = getWidth() > 0 || !(getParent() instanceof View) ? this : (View) getParent();
                TouchLayoutImport.Result result = TouchLayoutImport.importFiles(Collections.singletonList(text.getText().toString()),
                        surface.getWidth() - surface.getPaddingLeft() - surface.getPaddingRight(),
                        surface.getHeight() - surface.getPaddingTop() - surface.getPaddingBottom());
                replace(result.layout);
                dialog.dismiss();
                if (result.format != TouchLayoutImport.Format.NIGHTJAR || !result.skipped.isEmpty()) {
                    post(() -> showDialog.accept(new MaterialAlertDialogBuilder(activity)
                            .setTitle(R.string.touch_layout_import_done).setMessage(importSummary(activity, result))
                            .setPositiveButton(android.R.string.ok, null).create()));
                }
            } catch (IllegalArgumentException e) { text.setError(errorMessage(activity, e)); }
        }));
        showDialog.accept(dialog);
    }

    /** A user-facing reason a layout was rejected. */
    public static String errorMessage(Context context, IllegalArgumentException e) {
        if (!(e instanceof TouchLayout.InvalidLayoutException)) return context.getString(R.string.touch_layout_invalid);
        TouchLayout.InvalidLayoutException error = (TouchLayout.InvalidLayoutException) e;
        switch (error.reason) {
            case VERSION: return context.getString(R.string.touch_layout_error_version, error.detail);
            case TOO_LARGE: return context.getString(R.string.touch_layout_error_size);
            case DUPLICATE: return context.getString(R.string.touch_layout_error_duplicate, error.detail);
            case CONTROL: return context.getString(R.string.touch_layout_error_control, error.control,
                    error.detail == null ? "?" : error.detail);
            default: return context.getString(R.string.touch_layout_error_json);
        }
    }

    /** What an import brought in and, line by line, what it left out and why. */
    public static String importSummary(Context context, TouchLayoutImport.Result result) {
        StringBuilder text = new StringBuilder(context.getResources().getQuantityString(R.plurals.touch_layout_imported,
                result.layout.buttons.size(), result.layout.buttons.size()));
        if (result.format == TouchLayoutImport.Format.ARTEMIS_LAYOUT) {
            text.append("\n\n").append(context.getString(result.scaled ? R.string.touch_layout_import_scaled :
                    R.string.touch_layout_import_pixels));
        }
        if (result.hidden > 0) {
            text.append("\n\n").append(context.getResources().getQuantityString(R.plurals.touch_layout_import_hidden,
                    result.hidden, result.hidden));
        }
        if (!result.skipped.isEmpty()) {
            text.append("\n\n").append(context.getString(R.string.touch_layout_import_skipped));
            for (TouchLayoutImport.Skipped skipped : result.skipped) {
                int reason;
                switch (skipped.reason) {
                    case UNSUPPORTED_KEY: reason = R.string.touch_layout_skip_key; break;
                    case MISSING_CUSTOM_KEY: reason = R.string.touch_layout_skip_custom; break;
                    case STICK_CENTER_KEY: reason = R.string.touch_layout_skip_center; break;
                    case TOO_MANY: reason = R.string.touch_layout_skip_too_many; break;
                    case INVALID_SIZE: reason = R.string.touch_layout_skip_size; break;
                    default: reason = R.string.touch_layout_skip_no_equivalent; break;
                }
                text.append("\n• ").append(context.getString(reason, skipped.name));
            }
        }
        return text.toString();
    }

    private void exportLayout() {
        String json = layout.encode();
        TextView text = new TextView(activity); text.setText(json); text.setTextIsSelectable(true);
        ScrollView scroll = new ScrollView(activity); scroll.addView(text);
        showDialog.accept(new MaterialAlertDialogBuilder(activity).setTitle(R.string.touch_layout_export).setView(scroll)
                .setPositiveButton(R.string.touch_layout_copy, (dialog, which) ->
                        ((ClipboardManager) activity.getSystemService(Context.CLIPBOARD_SERVICE))
                                .setPrimaryClip(ClipData.newPlainText(activity.getString(R.string.touch_layout_title), json)))
                .setNeutralButton(R.string.touch_layout_share, (dialog, which) -> {
                    try {
                        java.io.File directory = new java.io.File(activity.getCacheDir(), "reports");
                        if (!directory.isDirectory() && !directory.mkdirs()) throw new java.io.IOException("Cannot create export directory");
                        java.io.File file = new java.io.File(directory, "nightjar-touch-layout.json");
                        try (java.io.OutputStream output = new java.io.FileOutputStream(file)) {
                            output.write(json.getBytes(java.nio.charset.StandardCharsets.UTF_8));
                        }
                        android.net.Uri uri = androidx.core.content.FileProvider.getUriForFile(activity,
                                activity.getPackageName() + ".reports", file);
                        Intent share = new Intent(Intent.ACTION_SEND).setType("application/json")
                                .putExtra(Intent.EXTRA_STREAM, uri).putExtra(Intent.EXTRA_TEXT, json)
                                .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION);
                        share.setClipData(ClipData.newUri(activity.getContentResolver(), "Touch layout", uri));
                        activity.startActivity(Intent.createChooser(share, activity.getString(R.string.touch_layout_share)));
                    } catch (java.io.IOException | android.content.ActivityNotFoundException | IllegalArgumentException e) {
                        post(() -> info(R.string.touch_layout_share_error));
                    }
                }).setNegativeButton(android.R.string.cancel, null).create());
    }

    private void rebuild() {
        releaseInput(); removeViews(1, buttonCount());
        for (int slot = 0; slot < layout.buttons.size(); slot++) {
            TouchLayout.Button b = layout.buttons.get(slot);
            View view;
            if (b.kind.directional()) {
                LayoutPadView pad = new LayoutPadView(activity, b);
                pad.setEditing(editing);
                pad.setContentDescription(b.name + ", " + activity.getString(kindName(b.kind)));
                view = pad;
            } else {
                MaterialButton button = new MaterialButton(activity);
                button.setText(b.name); button.setAllCaps(false); button.setTextColor(Color.WHITE); button.setTextSize(13);
                button.setPadding(0, 0, 0, 0); button.setInsetTop(0); button.setInsetBottom(0);
                button.setBackgroundTintList(android.content.res.ColorStateList.valueOf(editing ? 0xC0556677 :
                        b.kind.gamepad() ? 0xB0454D30 : 0xB030454D));
                button.setContentDescription(b.name + ", " + activity.getString(b.toggle ? R.string.touch_layout_toggle : R.string.touch_layout_hold));
                ViewCompat.setStateDescription(button, activity.getString(R.string.touch_layout_released));
                view = button;
            }
            // Enter and Space belong to the host; a focused control would take them.
            view.setFocusable(false);
            view.setFocusableInTouchMode(false);
            view.setTag(b.id);
            int control = slot;
            view.setOnClickListener(v -> {
                // Keyboard, Switch Access and TalkBack activation uses the same release-safe state.
                if (view.getParent() != GameTouchControls.this || editing || !canInput.getAsBoolean() || b.kind.directional()) return;
                if (b.toggle) toggle(b, view, control);
                else {
                    press(b, control);
                    int generation = inputGeneration;
                    postDelayed(() -> { if (generation == inputGeneration) release(b, control); }, 80);
                }
            });
            bindTouch(view, b, slot);
            addView(view, new LayoutParams(1, 1));
        }
        positionButtons();
        invalidate();
    }

    private void press(TouchLayout.Button b, int slot) {
        if (b.kind == TouchLayout.Kind.GAMEPAD) gamepad.setButtons(slot, b.gamepadButton);
        else if (b.kind == TouchLayout.Kind.TRIGGER) gamepad.setTrigger(slot, b.side, 1);
        else held.press(b);
    }

    private void release(TouchLayout.Button b, int slot) {
        if (b.kind == TouchLayout.Kind.GAMEPAD) gamepad.setButtons(slot, 0);
        else if (b.kind == TouchLayout.Kind.TRIGGER) gamepad.setTrigger(slot, b.side, 0);
        else held.release(b.id);
    }

    private boolean isHeld(TouchLayout.Button b, int slot) {
        return b.kind.gamepad() ? gamepad.holds(slot) : held.isHeld(b.id);
    }

    private void toggle(TouchLayout.Button b, View view, int slot) {
        if (isHeld(b, slot)) release(b, slot); else press(b, slot);
        boolean on = isHeld(b, slot);
        view.setSelected(on);
        view.setAlpha(on ? 1 : .8f);
        ViewCompat.setStateDescription(view, activity.getString(on ? R.string.touch_layout_pressed : R.string.touch_layout_released));
    }

    // Sends a stick, D-pad or WASD control's current position.
    private void steer(LayoutPadView pad, int slot, TouchLayout.Button[] directionKeys) {
        TouchLayout.Button control = pad.control;
        if (control.kind == TouchLayout.Kind.STICK) {
            gamepad.setStick(slot, control.side, pad.stick.x, pad.stick.y);
        } else if (control.kind == TouchLayout.Kind.DPAD) {
            gamepad.setButtons(slot, LayoutStick.dpadFlags(LayoutStick.directions(pad.stick.x, pad.stick.y, LayoutStick.DPAD_DIAGONAL)));
        } else {
            int next = LayoutStick.directions(pad.stick.x, pad.stick.y, control.diagonal);
            int changed = next ^ pad.directions;
            // Keys let go first, so sliding from W to D never holds a key the finger already left.
            for (int i = 0; i < 4; i++) if ((changed & ~next & (1 << i)) != 0) held.release(directionKeys[i].id);
            for (int i = 0; i < 4; i++) if ((changed & next & (1 << i)) != 0) held.press(directionKeys[i]);
            pad.directions = next;
        }
        pad.invalidate();
    }

    private float gridPixels() {
        return snapGrid / 1000f * Math.min(getWidth(), getHeight());
    }

    @Override protected void onDraw(Canvas canvas) {
        super.onDraw(canvas);
        float grid = gridPixels();
        if (!editing || grid < 4) return;
        for (float x = grid; x < getWidth(); x += grid) canvas.drawLine(x, 0, x, getHeight(), gridPaint);
        for (float y = grid; y < getHeight(); y += grid) canvas.drawLine(0, y, getWidth(), y, gridPaint);
    }

    @SuppressLint("ClickableViewAccessibility")
    private void bindTouch(View view, TouchLayout.Button initial, int slot) {
        LayoutPadView pad = view instanceof LayoutPadView ? (LayoutPadView) view : null;
        TouchLayout.Button[] directionKeys = initial.kind == TouchLayout.Kind.WASD ? new TouchLayout.Button[] {
                initial.directionKey(0), initial.directionKey(1), initial.directionKey(2), initial.directionKey(3)} : null;
        view.setOnTouchListener(new View.OnTouchListener() {
            float downX, downY, startX, startY;
            int pressGeneration = -1, pointerId = -1;
            @Override public boolean onTouch(View v, MotionEvent event) {
                if (v.getParent() != GameTouchControls.this) return true;
                if (!editing && !canInput.getAsBoolean()) return true;
                // Android hands a held control any later finger nothing else takes, such as one the
                // stream declines over the debug overlay. Only the finger that pressed it counts.
                int index = event.findPointerIndex(pointerId);
                switch (event.getActionMasked()) {
                    case MotionEvent.ACTION_DOWN:
                        pressGeneration = inputGeneration;
                        pointerId = event.getPointerId(0);
                        downX = event.getRawX(); downY = event.getRawY(); startX = v.getX(); startY = v.getY();
                        if (editing) return true;
                        if (pad != null) {
                            pad.stick.down(event.getX(), event.getY(), v.getWidth(), v.getHeight(), pad.radius());
                            steer(pad, slot, directionKeys);
                        } else if (!initial.toggle) { press(initial, slot); v.setPressed(true); }
                        return true;
                    case MotionEvent.ACTION_MOVE:
                        if (index < 0) return true;
                        if (editing) {
                            // Raw coordinates exist only for the first pointer before API 29.
                            float rawX = event.getX(index) + event.getRawX() - event.getX();
                            float rawY = event.getY(index) + event.getRawY() - event.getY();
                            float grid = gridPixels();
                            v.setX(LayoutStick.snap(startX + rawX - downX, v.getWidth(), getWidth(), grid));
                            v.setY(LayoutStick.snap(startY + rawY - downY, v.getHeight(), getHeight(), grid));
                        } else if (pad != null && pressGeneration == inputGeneration) {
                            pad.stick.move(event.getX(index), event.getY(index));
                            steer(pad, slot, directionKeys);
                        }
                        return true;
                    case MotionEvent.ACTION_POINTER_UP:
                    case MotionEvent.ACTION_UP:
                        if (index != event.getActionIndex() || pressGeneration != inputGeneration) return true;
                        pressGeneration = -1;
                        pointerId = -1;
                        if (editing && getWidth() > 0 && getHeight() > 0) {
                            List<TouchLayout.Button> entries = new ArrayList<>(layout.buttons);
                            for (int i = 0; i < entries.size(); i++) if (entries.get(i).id.equals(initial.id))
                                entries.set(i, entries.get(i).move(v.getX() / getWidth(), v.getY() / getHeight()));
                            layout = new TouchLayout(entries); layout.save(activity, profile);
                        } else if (pad != null) {
                            pad.stick.release();
                            steer(pad, slot, directionKeys);
                        } else if (initial.toggle) v.performClick();
                        else { release(initial, slot); v.setPressed(false); }
                        return true;
                    case MotionEvent.ACTION_CANCEL:
                        pressGeneration = -1;
                        pointerId = -1;
                        if (pad != null) {
                            pad.stick.release();
                            steer(pad, slot, directionKeys);
                        } else {
                            release(initial, slot); v.setPressed(false); v.setSelected(false);
                        }
                        return true;
                    default: return true;
                }
            }
        });
    }

    private void positionButtons() {
        if (getWidth() <= 0 || getHeight() <= 0) return;
        int min = Math.round(48 * getResources().getDisplayMetrics().density);
        for (int i = 0; i < buttonCount(); i++) {
            TouchLayout.Button b = layout.buttons.get(i); View v = buttonAt(i);
            int width = Math.min(getWidth(), Math.max(min, Math.round(b.width * getWidth())));
            int height = Math.min(getHeight(), Math.max(min, Math.round(b.height * getHeight())));
            LayoutParams params = (LayoutParams)v.getLayoutParams(); params.width = width; params.height = height;
            params.leftMargin = Math.min(getWidth() - width, Math.round(b.x * getWidth()));
            params.topMargin = Math.min(getHeight() - height, Math.round(b.y * getHeight()));
            v.setLayoutParams(params); v.setTranslationX(0); v.setTranslationY(0);
        }
    }

    @Override protected void onDetachedFromWindow() { releaseInput(); super.onDetachedFromWindow(); }
}
