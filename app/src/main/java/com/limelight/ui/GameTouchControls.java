package com.limelight.ui;

import android.annotation.SuppressLint;
import android.app.Activity;
import android.app.Dialog;
import android.content.ClipData;
import android.content.ClipboardManager;
import android.content.Context;
import android.content.Intent;
import android.graphics.Color;
import android.graphics.drawable.GradientDrawable;
import android.text.InputFilter;
import android.text.InputType;
import android.view.MotionEvent;
import android.view.View;
import android.widget.CheckBox;
import android.widget.EditText;
import android.widget.FrameLayout;
import android.widget.LinearLayout;
import android.widget.ScrollView;
import android.widget.TextView;
import androidx.appcompat.app.AlertDialog;
import androidx.core.view.ViewCompat;
import com.google.android.material.button.MaterialButton;
import com.google.android.material.dialog.MaterialAlertDialogBuilder;
import com.limelight.R;
import com.limelight.binding.input.HeldInputState;
import com.limelight.binding.input.KeyboardShortcuts;
import com.limelight.binding.input.TouchLayout;
import java.util.ArrayList;
import java.util.List;
import java.util.UUID;
import java.util.function.Consumer;

/** Independent per-game overlay. Blank areas pass through to the stream's mouse/touch handling. */
@SuppressLint("ViewConstructor") // Created by Game with explicit profile/input ownership; never inflated from XML.
public final class GameTouchControls extends FrameLayout {
    private final Activity activity;
    private String profile;
    private final Consumer<Dialog> showDialog;
    private final HeldInputState held;
    private final java.util.function.BooleanSupplier canInput;
    private TouchLayout layout;
    private boolean editing, enabled, hiddenForPip;
    private int inputGeneration;

    public GameTouchControls(Activity activity, String profile, HeldInputState.Sink sink,
                             java.util.function.BooleanSupplier canInput, Consumer<Dialog> showDialog) {
        super(activity);
        this.activity = activity; this.profile = profile; this.showDialog = showDialog; this.canInput = canInput;
        held = new HeldInputState(sink);
        layout = TouchLayout.load(activity, profile);
        enabled = TouchLayout.enabled(activity, profile);
        setClipChildren(true);
        setMotionEventSplittingEnabled(true);
        setVisibility(enabled ? VISIBLE : GONE);
        addOnLayoutChangeListener((v, l, t, r, b, ol, ot, or, ob) -> positionButtons());
        rebuild();
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

    public void releaseInput() {
        inputGeneration++;
        held.releaseAll();
        for (int i = 0; i < getChildCount(); i++) {
            View view = getChildAt(i); view.setSelected(false); view.setPressed(false); view.setAlpha(.8f);
            ViewCompat.setStateDescription(view, activity.getString(R.string.touch_layout_released));
        }
    }
    public void hideForPip(boolean hide) {
        releaseInput();
        hiddenForPip = hide;
        setVisibility(hide || !enabled ? GONE : VISIBLE);
    }
    public boolean isEditing() { return editing; }

    public void showMenu() {
        releaseInput();
        List<String> names = new ArrayList<>();
        names.add(activity.getString(enabled ? R.string.touch_layout_hide : R.string.touch_layout_show));
        names.add(activity.getString(R.string.touch_layout_add));
        names.add(activity.getString(editing ? R.string.touch_layout_save : R.string.touch_layout_move));
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
                            if (layout.buttons.size() < TouchLayout.MAX_BUTTONS) edit(null);
                            else info(R.string.touch_layout_limit);
                        } else if (which == 2) {
                            editing = !editing; enabled = true;
                            TouchLayout.setEnabled(activity, profile, true);
                            setVisibility(VISIBLE);
                            if (!editing) layout.save(activity, profile);
                            else info(R.string.touch_layout_move_help);
                            rebuild();
                        } else if (which == 3) importLayout();
                        else if (which == 4) exportLayout();
                        else edit(layout.buttons.get(which - 5));
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

    private void edit(TouchLayout.Button existing) {
        LinearLayout fields = new LinearLayout(activity); fields.setOrientation(LinearLayout.VERTICAL);
        int pad = Math.round(20 * getResources().getDisplayMetrics().density);
        fields.setPadding(pad, 0, pad, 0);
        EditText name = field(fields, R.string.touch_layout_name, existing == null ? "" : existing.name, 80);
        EditText binding = field(fields, R.string.touch_layout_binding, existing == null ? "" :
                existing.mouseButton == 0 ? KeyboardShortcuts.format(existing.keys()) : "MOUSE_" + existing.mouseButton, 256);
        TextView help = new TextView(activity); help.setText(R.string.touch_layout_binding_help); fields.addView(help);
        EditText width = field(fields, R.string.touch_layout_width,
                Integer.toString(existing == null ? 14 : Math.round(existing.width * 100)), 2);
        EditText height = field(fields, R.string.touch_layout_height,
                Integer.toString(existing == null ? 16 : Math.round(existing.height * 100)), 2);
        width.setInputType(InputType.TYPE_CLASS_NUMBER); height.setInputType(InputType.TYPE_CLASS_NUMBER);
        CheckBox toggle = new CheckBox(activity); toggle.setText(R.string.touch_layout_toggle);
        toggle.setChecked(existing != null && existing.toggle); fields.addView(toggle);
        ScrollView scroll = new ScrollView(activity); scroll.addView(fields);
        MaterialAlertDialogBuilder builder = new MaterialAlertDialogBuilder(activity).setTitle(R.string.touch_layout_title)
                .setView(scroll).setPositiveButton(R.string.keyboard_shortcuts_save, null)
                .setNegativeButton(android.R.string.cancel, null);
        if (existing != null) builder.setNeutralButton(R.string.keyboard_shortcuts_delete, (dialog, which) -> {
            List<TouchLayout.Button> entries = new ArrayList<>(layout.buttons);
            entries.removeIf(b -> b.id.equals(existing.id)); replace(new TouchLayout(entries));
        });
        AlertDialog dialog = builder.create();
        dialog.setOnShowListener(ignored -> dialog.getButton(AlertDialog.BUTTON_POSITIVE).setOnClickListener(v -> {
            try {
                String value = binding.getText().toString().trim().toUpperCase(java.util.Locale.ROOT);
                int mouse = value.matches("MOUSE_[1-5]") ? value.charAt(6) - '0' : 0;
                float w = Integer.parseInt(width.getText().toString()) / 100f;
                float h = Integer.parseInt(height.getText().toString()) / 100f;
                TouchLayout.Button b = new TouchLayout.Button(existing == null ? UUID.randomUUID().toString() : existing.id,
                        name.getText().toString(), mouse == 0 ? KeyboardShortcuts.parseChord(value) : new int[0], mouse,
                        toggle.isChecked(), Math.min(existing == null ? .06f : existing.x, 1 - w),
                        Math.min(existing == null ? .65f : existing.y, 1 - h), w, h);
                List<TouchLayout.Button> entries = new ArrayList<>(layout.buttons);
                int index = existing == null ? -1 : entries.indexOf(existing);
                if (index < 0) entries.add(b); else entries.set(index, b);
                enabled = true; TouchLayout.setEnabled(activity, profile, true); setVisibility(VISIBLE);
                replace(new TouchLayout(entries)); dialog.dismiss();
            } catch (IllegalArgumentException e) { binding.setError(activity.getString(R.string.touch_layout_invalid)); }
        }));
        showDialog.accept(dialog);
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
            try { replace(TouchLayout.parse(text.getText().toString())); dialog.dismiss(); }
            catch (IllegalArgumentException e) { text.setError(activity.getString(R.string.touch_layout_invalid)); }
        }));
        showDialog.accept(dialog);
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
                        java.io.File file = new java.io.File(directory, "rubylight-touch-layout.json");
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
        releaseInput(); removeAllViews();
        for (TouchLayout.Button b : layout.buttons) {
            MaterialButton view = new MaterialButton(activity);
            view.setText(b.name); view.setAllCaps(false); view.setTextColor(Color.WHITE); view.setTextSize(13);
            view.setPadding(0, 0, 0, 0); view.setInsetTop(0); view.setInsetBottom(0);
            view.setBackgroundTintList(android.content.res.ColorStateList.valueOf(editing ? 0xC0556677 : 0xB030454D));
            view.setContentDescription(b.name + ", " + activity.getString(b.toggle ? R.string.touch_layout_toggle : R.string.touch_layout_hold));
            view.setTag(b.id);
            ViewCompat.setStateDescription(view, activity.getString(R.string.touch_layout_released));
            view.setOnClickListener(v -> {
                // Keyboard, Switch Access and TalkBack activation uses the same release-safe state.
                if (view.getParent() != GameTouchControls.this || editing || !canInput.getAsBoolean()) return;
                if (b.toggle) toggle(b, view);
                else {
                    held.press(b);
                    int generation = inputGeneration;
                    postDelayed(() -> { if (generation == inputGeneration) held.release(b.id); }, 80);
                }
            });
            bindTouch(view, b);
            addView(view, new LayoutParams(1, 1));
        }
        positionButtons();
    }

    private void toggle(TouchLayout.Button b, View view) {
        if (held.isHeld(b.id)) held.release(b.id); else held.press(b);
        view.setSelected(held.isHeld(b.id));
        view.setAlpha(held.isHeld(b.id) ? 1 : .8f);
        ViewCompat.setStateDescription(view, activity.getString(held.isHeld(b.id) ? R.string.touch_layout_pressed : R.string.touch_layout_released));
    }

    @SuppressLint("ClickableViewAccessibility")
    private void bindTouch(View view, TouchLayout.Button initial) {
        view.setOnTouchListener(new View.OnTouchListener() {
            float downX, downY, startX, startY;
            int pressGeneration = -1;
            @Override public boolean onTouch(View v, MotionEvent event) {
                if (v.getParent() != GameTouchControls.this) return true;
                if (!editing && !canInput.getAsBoolean()) return true;
                switch (event.getActionMasked()) {
                    case MotionEvent.ACTION_DOWN:
                        pressGeneration = inputGeneration;
                        downX = event.getRawX(); downY = event.getRawY(); startX = v.getX(); startY = v.getY();
                        if (!editing && !initial.toggle) { held.press(initial); v.setPressed(true); }
                        return true;
                    case MotionEvent.ACTION_MOVE:
                        if (editing) {
                            v.setX(Math.max(0, Math.min(getWidth() - v.getWidth(), startX + event.getRawX() - downX)));
                            v.setY(Math.max(0, Math.min(getHeight() - v.getHeight(), startY + event.getRawY() - downY)));
                        }
                        return true;
                    case MotionEvent.ACTION_UP:
                        if (pressGeneration != inputGeneration) return true;
                        pressGeneration = -1;
                        if (editing && getWidth() > 0 && getHeight() > 0) {
                            List<TouchLayout.Button> entries = new ArrayList<>(layout.buttons);
                            for (int i = 0; i < entries.size(); i++) if (entries.get(i).id.equals(initial.id))
                                entries.set(i, entries.get(i).move(v.getX() / getWidth(), v.getY() / getHeight()));
                            layout = new TouchLayout(entries); layout.save(activity, profile);
                        } else if (initial.toggle) v.performClick();
                        else { held.release(initial.id); v.setPressed(false); }
                        return true;
                    case MotionEvent.ACTION_CANCEL:
                        pressGeneration = -1;
                        held.release(initial.id); v.setPressed(false); v.setSelected(false); return true;
                    default: return true;
                }
            }
        });
    }

    private void positionButtons() {
        if (getWidth() <= 0 || getHeight() <= 0) return;
        int min = Math.round(48 * getResources().getDisplayMetrics().density);
        for (int i = 0; i < getChildCount(); i++) {
            TouchLayout.Button b = layout.buttons.get(i); View v = getChildAt(i);
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
