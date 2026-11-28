package com.limelight.binding.input.pad;

import android.content.Context;
import android.content.res.ColorStateList;
import android.graphics.Canvas;
import android.graphics.Color;
import android.graphics.DashPathEffect;
import android.graphics.Paint;
import android.graphics.RectF;
import android.graphics.Typeface;
import android.graphics.drawable.GradientDrawable;
import android.graphics.drawable.StateListDrawable;
import android.util.TypedValue;
import android.view.Gravity;
import android.view.View;
import android.view.ViewGroup;
import android.widget.LinearLayout;
import android.widget.SeekBar;
import android.widget.TextView;

import androidx.appcompat.widget.SwitchCompat;

import com.limelight.R;
import com.limelight.binding.input.touch.PointerAcceleration;

/**
 * Coordinates the Studio editor additions for {@link PadTouchZone} and {@link PadCustomButton}:
 * builds and binds the inspector sections, constructs repeatable Add tiles, and provides shared
 * surface drawing, hit-testing, snap-dragging, and corner-resizing helpers.
 */
final class PadCustomInspector {
    private static final float SNAP_ZONE_THRESHOLD = 0.02f;

    private final Context context;
    private final PadEditor editor;
    private final ViewGroup zoneSection;
    private final ViewGroup kbmSection;
    private final float density;
    private boolean binding;

    // Zone inspector widgets
    private final TextView[] kindChips = new TextView[4];
    private final TextView[] presetChips = new TextView[4];
    private View moveGroup;
    private SwitchCompat autoSprintSwitch;
    private SeekBar deadzoneBar;
    private TextView deadzoneValue;
    private View lookGroup;
    private SeekBar sensitivityBar;
    private TextView sensitivityValue;
    private SwitchCompat invertYSwitch;
    private final TextView[] curveChips = new TextView[4];
    private View tapRow;
    private final TextView[] tapChips = new TextView[3];
    private SeekBar zoneOpacityBar;
    private TextView zoneOpacityValue;

    // Custom button inspector widgets
    private TextView keySummaryButton;
    private final TextView[] shapeChips = new TextView[3];
    private SwitchCompat toggleSwitch;
    private SwitchCompat dragAimSwitch;
    private SeekBar btnOpacityBar;
    private TextView btnOpacityValue;
    private TextView duplicateButton;

    PadCustomInspector(Context context, PadEditor editor, ViewGroup zoneSection,
                       ViewGroup kbmSection, float density) {
        this.context = context;
        this.editor = editor;
        this.zoneSection = zoneSection;
        this.kbmSection = kbmSection;
        this.density = Math.max(1f, density);
        if (zoneSection != null) buildZoneSection();
        if (kbmSection != null) buildKbmSection();
    }

    void update(PadTouchZone zone, PadCustomButton button) {
        binding = true;
        try {
            if (zoneSection != null) {
                zoneSection.setVisibility(zone != null ? View.VISIBLE : View.GONE);
                if (zone != null) bindZone(zone);
            }
            if (kbmSection != null) {
                kbmSection.setVisibility(button != null ? View.VISIBLE : View.GONE);
                if (button != null) bindButton(button);
            }
        } finally {
            binding = false;
        }
    }

    private void buildZoneSection() {
        zoneSection.removeAllViews();
        int gap = dp(6);

        PadTouchZone.Kind[] kinds = PadTouchZone.Kind.values();
        int[] kindLabels = {
                R.string.pad_edit_zone_move_wasd, R.string.pad_edit_zone_move_stick,
                R.string.pad_edit_zone_look_mouse, R.string.pad_edit_zone_look_stick
        };
        LinearLayout kindRow = makeRow(gap);
        for (int i = 0; i < kinds.length; i++) {
            final PadTouchZone.Kind k = kinds[i];
            kindChips[i] = makeChip(context.getString(kindLabels[i]));
            kindChips[i].setOnClickListener(v -> {
                PadTouchZone z = editor.selectedZone();
                if (z == null || z.kind == k) return;
                editor.recordCheckpoint();
                z.kind = k;
                if (k == PadTouchZone.Kind.MOVE_WASD && z.tapAction != PadTouchZone.TapAction.NONE) {
                    z.tapAction = PadTouchZone.TapAction.NONE;
                }
                editor.notifyEdited();
            });
            kindRow.addView(kindChips[i], weightLp(i < kinds.length - 1 ? gap : 0));
        }
        zoneSection.addView(kindRow, marginLp(0, gap, 0, gap));

        PadTouchZone.QuickPreset[] presets = PadTouchZone.QuickPreset.values();
        int[] presetLabels = {
                R.string.pad_zone_preset_50_50, R.string.pad_zone_preset_40_60,
                R.string.pad_zone_preset_right_cam, R.string.pad_zone_preset_custom
        };
        LinearLayout presetRow = makeRow(gap);
        for (int i = 0; i < presets.length; i++) {
            final PadTouchZone.QuickPreset p = presets[i];
            presetChips[i] = makeChip(context.getString(presetLabels[i]));
            presetChips[i].setOnClickListener(v -> editor.applyZoneQuickPreset(p));
            presetRow.addView(presetChips[i], weightLp(i < presets.length - 1 ? gap : 0));
        }
        zoneSection.addView(presetRow, marginLp(0, 0, 0, gap));

        LinearLayout moveCol = new LinearLayout(context);
        moveCol.setOrientation(LinearLayout.VERTICAL);
        autoSprintSwitch = makeSwitch(context.getString(R.string.pad_zone_auto_sprint), checked -> {
            if (binding) return;
            PadTouchZone z = editor.selectedZone();
            if (z == null) return;
            editor.recordCheckpoint();
            z.autoSprint = checked;
            editor.notifyEdited();
        });
        moveCol.addView(autoSprintSwitch);

        deadzoneValue = new TextView(context);
        deadzoneBar = makeSeekRow(moveCol, context.getString(R.string.pad_zone_deadzone), deadzoneValue, 40,
                progress -> {
                    PadTouchZone z = editor.selectedZone();
                    if (z == null) return;
                    z.setDeadzone(progress / 100f);
                    deadzoneValue.setText(progress + "%");
                });
        moveGroup = moveCol;
        zoneSection.addView(moveCol, marginLp(0, 0, 0, gap));

        LinearLayout lookCol = new LinearLayout(context);
        lookCol.setOrientation(LinearLayout.VERTICAL);
        sensitivityValue = new TextView(context);
        sensitivityBar = makeSeekRow(lookCol, context.getString(R.string.pad_zone_sensitivity), sensitivityValue,
                PadTouchZone.MAX_SENSITIVITY - PadTouchZone.MIN_SENSITIVITY,
                progress -> {
                    PadTouchZone z = editor.selectedZone();
                    if (z == null) return;
                    int sens = PadTouchZone.MIN_SENSITIVITY + progress;
                    z.setSensitivityPercent(sens);
                    sensitivityValue.setText(sens + "%");
                });

        invertYSwitch = makeSwitch(context.getString(R.string.pad_zone_invert_y), checked -> {
            if (binding) return;
            PadTouchZone z = editor.selectedZone();
            if (z == null) return;
            editor.recordCheckpoint();
            z.invertY = checked;
            editor.notifyEdited();
        });
        lookCol.addView(invertYSwitch);

        String[] curves = {
                PointerAcceleration.LINEAR, PointerAcceleration.GENTLE,
                PointerAcceleration.MEDIUM, PointerAcceleration.STRONG
        };
        int[] curveLabels = {
                R.string.pad_zone_accel_linear, R.string.pad_zone_accel_gentle,
                R.string.pad_zone_accel_medium, R.string.pad_zone_accel_strong
        };
        LinearLayout curveRow = makeRow(gap);
        for (int i = 0; i < curves.length; i++) {
            final String c = curves[i];
            curveChips[i] = makeChip(context.getString(curveLabels[i]));
            curveChips[i].setOnClickListener(v -> {
                PadTouchZone z = editor.selectedZone();
                if (z == null) return;
                editor.recordCheckpoint();
                z.setAccelerationCurve(c);
                editor.notifyEdited();
            });
            curveRow.addView(curveChips[i], weightLp(i < curves.length - 1 ? gap : 0));
        }
        lookCol.addView(curveRow, marginLp(0, gap, 0, gap));

        PadTouchZone.TapAction[] taps = PadTouchZone.TapAction.values();
        int[] tapLabels = {
                R.string.pad_zone_tap_none, R.string.pad_zone_tap_left, R.string.pad_zone_tap_right
        };
        LinearLayout tRow = makeRow(gap);
        for (int i = 0; i < taps.length; i++) {
            final PadTouchZone.TapAction t = taps[i];
            tapChips[i] = makeChip(context.getString(tapLabels[i]));
            tapChips[i].setOnClickListener(v -> {
                PadTouchZone z = editor.selectedZone();
                if (z == null) return;
                editor.recordCheckpoint();
                z.tapAction = t;
                editor.notifyEdited();
            });
            tRow.addView(tapChips[i], weightLp(i < taps.length - 1 ? gap : 0));
        }
        tapRow = tRow;
        lookCol.addView(tRow, marginLp(0, 0, 0, gap));
        lookGroup = lookCol;
        zoneSection.addView(lookCol);

        zoneOpacityValue = new TextView(context);
        zoneOpacityBar = makeSeekRow(zoneSection, context.getString(R.string.title_osc_opacity), zoneOpacityValue, 100,
                progress -> {
                    PadTouchZone z = editor.selectedZone();
                    if (z == null) return;
                    z.setOpacity(progress);
                    zoneOpacityValue.setText(progress + "%");
                });

        TextView deleteZone = makeChip(context.getString(R.string.pad_kbm_delete));
        deleteZone.setOnClickListener(v -> editor.deleteSelected());
        zoneSection.addView(deleteZone, marginLp(0, gap, 0, 0));
    }

    private void buildKbmSection() {
        kbmSection.removeAllViews();
        int gap = dp(6);

        keySummaryButton = makeChip(context.getString(R.string.pad_kbm_change_key));
        keySummaryButton.setOnClickListener(v -> {
            PadCustomButton current = editor.selectedButton();
            if (current == null) return;
            PadKeyPickerSheet.show(context, current, updated -> {
                PadCustomButton target = editor.selectedButton();
                if (target == null) return;
                editor.recordCheckpoint();
                target.copyFrom(updated);
                editor.notifyEdited();
            });
        });
        kbmSection.addView(keySummaryButton, marginLp(0, gap, 0, gap));

        PadCustomButton.Shape[] shapes = PadCustomButton.Shape.values();
        int[] shapeLabels = {
                R.string.pad_kbm_shape_round, R.string.pad_kbm_shape_pill, R.string.pad_kbm_shape_rect
        };
        LinearLayout shapeRow = makeRow(gap);
        for (int i = 0; i < shapes.length; i++) {
            final PadCustomButton.Shape s = shapes[i];
            shapeChips[i] = makeChip(context.getString(shapeLabels[i]));
            shapeChips[i].setOnClickListener(v -> {
                PadCustomButton b = editor.selectedButton();
                if (b == null || b.shape == s) return;
                editor.recordCheckpoint();
                b.shape = s;
                editor.notifyEdited();
            });
            shapeRow.addView(shapeChips[i], weightLp(i < shapes.length - 1 ? gap : 0));
        }
        kbmSection.addView(shapeRow, marginLp(0, 0, 0, gap));

        toggleSwitch = makeSwitch(context.getString(R.string.pad_kbm_toggle_mode), checked -> {
            if (binding) return;
            PadCustomButton b = editor.selectedButton();
            if (b == null) return;
            editor.recordCheckpoint();
            b.toggle = checked;
            editor.notifyEdited();
        });
        kbmSection.addView(toggleSwitch);

        dragAimSwitch = makeSwitch(context.getString(R.string.pad_kbm_drag_aim), checked -> {
            if (binding) return;
            PadCustomButton b = editor.selectedButton();
            if (b == null) return;
            editor.recordCheckpoint();
            b.dragAim = checked;
            editor.notifyEdited();
        });
        kbmSection.addView(dragAimSwitch);

        btnOpacityValue = new TextView(context);
        btnOpacityBar = makeSeekRow(kbmSection, context.getString(R.string.title_osc_opacity), btnOpacityValue, 100,
                progress -> {
                    PadCustomButton b = editor.selectedButton();
                    if (b == null) return;
                    b.setOpacity(progress);
                    btnOpacityValue.setText(progress + "%");
                });

        LinearLayout actions = makeRow(gap);
        duplicateButton = makeChip(context.getString(R.string.pad_kbm_duplicate));
        duplicateButton.setOnClickListener(v -> editor.duplicateSelectedButton());
        TextView deleteButton = makeChip(context.getString(R.string.pad_kbm_delete));
        deleteButton.setOnClickListener(v -> editor.deleteSelected());
        actions.addView(duplicateButton, weightLp(gap));
        actions.addView(deleteButton, weightLp(0));
        kbmSection.addView(actions, marginLp(0, gap, 0, 0));
    }

    private void bindZone(PadTouchZone zone) {
        PadTouchZone.Kind[] kinds = PadTouchZone.Kind.values();
        for (int i = 0; i < kinds.length; i++) {
            setChipSelected(kindChips[i], kinds[i] == zone.kind);
        }
        boolean move = zone.kind.isMove();
        moveGroup.setVisibility(move ? View.VISIBLE : View.GONE);
        lookGroup.setVisibility(move ? View.GONE : View.VISIBLE);
        if (move) {
            autoSprintSwitch.setVisibility(zone.kind == PadTouchZone.Kind.MOVE_WASD ? View.VISIBLE : View.GONE);
            autoSprintSwitch.setChecked(zone.autoSprint);
            int dz = Math.max(0, Math.min(40, Math.round(zone.deadzone * 100f)));
            deadzoneBar.setProgress(dz);
            deadzoneValue.setText(dz + "%");
        } else {
            int sensProg = Math.max(0, Math.min(
                    PadTouchZone.MAX_SENSITIVITY - PadTouchZone.MIN_SENSITIVITY,
                    zone.sensitivityPercent - PadTouchZone.MIN_SENSITIVITY));
            sensitivityBar.setProgress(sensProg);
            sensitivityValue.setText(zone.sensitivityPercent + "%");
            invertYSwitch.setChecked(zone.invertY);
            String[] curves = {
                    PointerAcceleration.LINEAR, PointerAcceleration.GENTLE,
                    PointerAcceleration.MEDIUM, PointerAcceleration.STRONG
            };
            String normalized = PadTouchZone.normalizeCurve(zone.accelerationCurve);
            for (int i = 0; i < curves.length; i++) {
                setChipSelected(curveChips[i], curves[i].equals(normalized));
            }
            tapRow.setVisibility(zone.kind == PadTouchZone.Kind.LOOK_MOUSE ? View.VISIBLE : View.GONE);
            PadTouchZone.TapAction[] taps = PadTouchZone.TapAction.values();
            for (int i = 0; i < taps.length; i++) {
                setChipSelected(tapChips[i], taps[i] == zone.tapAction);
            }
        }
        zoneOpacityBar.setProgress(zone.opacity);
        zoneOpacityValue.setText(zone.opacity + "%");
    }

    private void bindButton(PadCustomButton button) {
        String title = context.getString(R.string.pad_kbm_change_key) + ": " + button.effectiveLabel();
        keySummaryButton.setText(title);
        PadCustomButton.Shape[] shapes = PadCustomButton.Shape.values();
        for (int i = 0; i < shapes.length; i++) {
            setChipSelected(shapeChips[i], shapes[i] == button.shape);
        }
        toggleSwitch.setChecked(button.toggle);
        dragAimSwitch.setChecked(button.dragAim);
        btnOpacityBar.setProgress(button.opacity);
        btnOpacityValue.setText(button.opacity + "%");
        boolean canDup = editor.draft().customButtons().size() < PadCustomButton.MAX_CUSTOM_BUTTONS;
        duplicateButton.setEnabled(canDup);
        duplicateButton.setAlpha(canDup ? 1f : 0.4f);
    }

    static String kindName(Context context, PadTouchZone.Kind kind) {
        if (kind == null) return context.getString(R.string.pad_edit_touch_zone);
        switch (kind) {
            case MOVE_WASD: return context.getString(R.string.pad_edit_zone_move_wasd);
            case MOVE_STICK: return context.getString(R.string.pad_edit_zone_move_stick);
            case LOOK_STICK: return context.getString(R.string.pad_edit_zone_look_stick);
            case LOOK_MOUSE:
            default:
                return context.getString(R.string.pad_edit_zone_look_mouse);
        }
    }

    static boolean hasAddableExtras(PadLayout draft) {
        if (draft == null) return false;
        return draft.touchZones().size() < PadTouchZone.MAX_ZONES
                || draft.customButtons().size() < PadCustomButton.MAX_CUSTOM_BUTTONS;
    }

    static String addStateKey(PadLayout draft) {
        if (draft == null) return "";
        return (draft.touchZones().size() < PadTouchZone.MAX_ZONES ? "Z" : "")
                + (draft.customButtons().size() < PadCustomButton.MAX_CUSTOM_BUTTONS ? "K" : "");
    }

    static View makeAddTile(Context context, PadEditor editor, float density, boolean isZone) {
        float d = Math.max(1f, density);
        int padH = Math.round(12 * d);
        int padV = Math.round(10 * d);
        TextView tile = new TextView(context);
        String label = context.getString(isZone ? R.string.pad_edit_touch_zone : R.string.pad_edit_kbm_button);
        tile.setText((isZone ? "+ " : "+ ") + label);
        tile.setContentDescription(label);
        tile.setGravity(Gravity.CENTER);
        tile.setSingleLine(true);
        tile.setTextSize(TypedValue.COMPLEX_UNIT_SP, 12);
        tile.setTypeface(Typeface.DEFAULT_BOLD);
        tile.setMinHeight(Math.round(48 * d));
        tile.setMinWidth(Math.round(96 * d));
        tile.setPadding(padH, padV, padH, padV);
        tile.setClickable(true);
        tile.setFocusable(true);
        tile.setTextColor(Color.WHITE);
        tile.setBackground(chipBackground(d, false));
        tile.setOnClickListener(v -> {
            if (isZone) editor.addTouchZone();
            else editor.addCustomButton();
        });
        return tile;
    }

    static void drawTouchZones(Context context, Canvas canvas, PadLayout draft, float w, float h,
                               float density, String selectedZoneId) {
        if (draft == null || w <= 0f || h <= 0f) return;
        Paint fill = new Paint(Paint.ANTI_ALIAS_FLAG);
        Paint stroke = new Paint(Paint.ANTI_ALIAS_FLAG);
        stroke.setStyle(Paint.Style.STROKE);
        stroke.setPathEffect(new DashPathEffect(new float[] {6f * density, 4f * density}, 0f));
        Paint text = new Paint(Paint.ANTI_ALIAS_FLAG);
        text.setTextSize(12f * density);
        text.setTypeface(Typeface.DEFAULT_BOLD);
        float[] box = new float[4];
        RectF rect = new RectF();
        float radius = 12f * density;
        for (PadTouchZone z : draft.touchZones()) {
            z.pixelBounds(w, h, box);
            rect.set(box[0], box[1], box[0] + box[2], box[1] + box[3]);
            boolean selected = z.id.equals(selectedZoneId);
            boolean move = z.kind.isMove();
            fill.setColor(move
                    ? Color.argb(selected ? 46 : 26, 56, 189, 248)
                    : Color.argb(selected ? 46 : 26, 168, 85, 247));
            canvas.drawRoundRect(rect, radius, radius, fill);
            stroke.setStrokeWidth((selected ? 2f : 1.25f) * density);
            stroke.setColor(selected
                    ? Color.argb(230, 147, 197, 253)
                    : Color.argb(130, 255, 255, 255));
            canvas.drawRoundRect(rect, radius, radius, stroke);
            text.setColor(Color.argb(selected ? 245 : 190, 255, 255, 255));
            String badge = kindName(context, z.kind);
            if (z.opacity == 0) badge += " (0%)";
            canvas.drawText(badge, rect.left + 10f * density, rect.top + 18f * density, text);
        }
    }

    static void drawCustomButtons(PadSkin70 skin, Canvas canvas, PadLayout draft,
                                  float w, float h, float density, float baseOpacity) {
        if (draft == null || skin == null || w <= 0f || h <= 0f) return;
        float[] box = new float[4];
        try {
            for (PadCustomButton button : draft.customButtons()) {
                button.bounds(w, h, density, box);
                skin.setOpacity(Math.max(0.35f, baseOpacity * button.opacity / 100f));
                PadCustomButtonView.drawButton(canvas, skin, button, box[0], box[1], box[2], box[3], 0f, false);
            }
        } finally {
            skin.setOpacity(1f);
        }
    }

    static float[] selectedBox(PadEditor editor, float w, float h, float density, float[] out) {
        PadTouchZone zone = editor.selectedZone();
        if (zone != null) return zone.pixelBounds(w, h, out);
        PadCustomButton button = editor.selectedButton();
        if (button != null) return button.bounds(w, h, density, out);
        return null;
    }

    static PadCustomButton hitButton(PadLayout draft, float x, float y, float w, float h, float density, float[] box) {
        if (draft == null) return null;
        java.util.List<PadCustomButton> buttons = draft.customButtons();
        for (int i = buttons.size() - 1; i >= 0; i--) {
            PadCustomButton b = buttons.get(i);
            b.bounds(w, h, density, box);
            if (x >= box[0] && x <= box[0] + box[2] && y >= box[1] && y <= box[1] + box[3]) {
                return b;
            }
        }
        return null;
    }

    static PadTouchZone hitZone(PadLayout draft, float x, float y, float w, float h, float[] box) {
        if (draft == null) return null;
        java.util.List<PadTouchZone> zones = draft.touchZones();
        for (int i = zones.size() - 1; i >= 0; i--) {
            PadTouchZone z = zones.get(i);
            z.pixelBounds(w, h, box);
            if (x >= box[0] && x <= box[0] + box[2] && y >= box[1] && y <= box[1] + box[3]) {
                return z;
            }
        }
        return null;
    }

    static void dragZone(PadTouchZone zone, float dxNorm, float dyNorm,
                         float startL, float startT, float startR, float startB) {
        float w = startR - startL;
        float h = startB - startT;
        float nl = Math.max(0f, Math.min(1f - w, startL + dxNorm));
        float nt = Math.max(0f, Math.min(1f - h, startT + dyNorm));
        nl = snapZoneEdge(nl, w);
        nt = snapZoneEdge(nt, h);
        zone.setBounds(nl, nt, nl + w, nt + h);
    }

    static void resizeZoneCorner(PadTouchZone zone, int corner, float x, float y, float w, float h,
                                 float startL, float startT, float startR, float startB) {
        if (w <= 0f || h <= 0f) return;
        float nx = Math.max(0f, Math.min(1f, x / w));
        float ny = Math.max(0f, Math.min(1f, y / h));
        float min = PadTouchZone.MIN_SIZE;
        boolean right = (corner & PadEditor.RIGHT_CORNER) != 0;
        boolean bottom = (corner & PadEditor.BOTTOM_CORNER) != 0;
        float l = startL, t = startT, r = startR, b = startB;
        if (!right) l = Math.min(nx, r - min);
        else r = Math.max(nx, l + min);
        if (!bottom) t = Math.min(ny, b - min);
        else b = Math.max(ny, t + min);
        zone.setBounds(l, t, r, b);
    }

    private static float snapZoneEdge(float start, float span) {
        if (Math.abs(start) < SNAP_ZONE_THRESHOLD) return 0f;
        if (Math.abs(start + span - 1f) < SNAP_ZONE_THRESHOLD) return Math.max(0f, 1f - span);
        if (Math.abs(start + span / 2f - 0.5f) < SNAP_ZONE_THRESHOLD) return Math.max(0f, 0.5f - span / 2f);
        return start;
    }

    private interface IntAction { void accept(int value); }
    private interface BoolAction { void accept(boolean checked); }

    private LinearLayout makeRow(int gap) {
        LinearLayout row = new LinearLayout(context);
        row.setOrientation(LinearLayout.HORIZONTAL);
        row.setGravity(Gravity.CENTER_VERTICAL);
        return row;
    }

    private SwitchCompat makeSwitch(String label, BoolAction onChange) {
        SwitchCompat sw = new SwitchCompat(context);
        sw.setText(label);
        sw.setMinHeight(dp(48));
        sw.setTextColor(Color.WHITE);
        sw.setTextSize(TypedValue.COMPLEX_UNIT_SP, 12);
        sw.setOnCheckedChangeListener((btn, checked) -> onChange.accept(checked));
        return sw;
    }

    private SeekBar makeSeekRow(ViewGroup parent, String title, TextView valueView, int max,
                                IntAction onProgress) {
        LinearLayout header = new LinearLayout(context);
        header.setOrientation(LinearLayout.HORIZONTAL);
        TextView titleView = new TextView(context);
        titleView.setText(title);
        titleView.setTextColor(Color.WHITE);
        titleView.setTextSize(TypedValue.COMPLEX_UNIT_SP, 12);
        header.addView(titleView, new LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f));
        valueView.setTextColor(Color.argb(210, 255, 255, 255));
        valueView.setTextSize(TypedValue.COMPLEX_UNIT_SP, 12);
        header.addView(valueView);
        parent.addView(header, marginLp(0, dp(4), 0, 0));

        SeekBar bar = new SeekBar(context);
        bar.setMax(max);
        bar.setMinimumHeight(dp(48));
        final Object[] activeRun = { new Object() };
        bar.setOnSeekBarChangeListener(new SeekBar.OnSeekBarChangeListener() {
            @Override
            public void onProgressChanged(SeekBar seekBar, int progress, boolean fromUser) {
                if (fromUser && !binding) {
                    editor.checkpoint(activeRun[0]);
                    onProgress.accept(progress);
                    editor.notifyEdited();
                }
            }

            @Override
            public void onStartTrackingTouch(SeekBar seekBar) {
                if (!binding) editor.checkpoint(activeRun[0]);
            }

            @Override
            public void onStopTrackingTouch(SeekBar seekBar) {
                if (!binding) {
                    activeRun[0] = new Object();
                    editor.notifyEdited();
                }
            }
        });
        parent.addView(bar, new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT));
        return bar;
    }

    private TextView makeChip(String text) {
        TextView chip = new TextView(context);
        chip.setText(text);
        chip.setGravity(Gravity.CENTER);
        chip.setSingleLine(true);
        chip.setTextSize(TypedValue.COMPLEX_UNIT_SP, 11);
        chip.setMinHeight(dp(48));
        chip.setMinWidth(dp(48));
        chip.setPadding(dp(8), dp(6), dp(8), dp(6));
        chip.setClickable(true);
        chip.setFocusable(true);
        chip.setTextColor(ColorStateList.valueOf(Color.WHITE));
        chip.setBackground(chipBackground(density, false));
        return chip;
    }

    private void setChipSelected(TextView chip, boolean selected) {
        if (chip == null) return;
        chip.setBackground(chipBackground(density, selected));
    }

    private static StateListDrawable chipBackground(float d, boolean selected) {
        StateListDrawable states = new StateListDrawable();
        float r = 10f * d;
        GradientDrawable focused = rectDrawable(r, Color.argb(90, 96, 165, 250),
                Math.round(2 * d), Color.argb(240, 147, 197, 253));
        GradientDrawable normal = rectDrawable(r,
                selected ? Color.argb(88, 59, 130, 246) : Color.argb(36, 255, 255, 255),
                Math.round(1 * d),
                selected ? Color.argb(220, 147, 197, 253) : Color.argb(80, 255, 255, 255));
        states.addState(new int[] {android.R.attr.state_focused}, focused);
        states.addState(new int[0], normal);
        return states;
    }

    private static GradientDrawable rectDrawable(float r, int fill, int strokeW, int stroke) {
        GradientDrawable d = new GradientDrawable();
        d.setCornerRadius(r);
        d.setColor(fill);
        d.setStroke(strokeW, stroke);
        return d;
    }

    private LinearLayout.LayoutParams weightLp(int rightMargin) {
        LinearLayout.LayoutParams lp = new LinearLayout.LayoutParams(
                0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f);
        if (rightMargin > 0) lp.setMargins(0, 0, rightMargin, 0);
        return lp;
    }

    private static LinearLayout.LayoutParams marginLp(int l, int t, int r, int b) {
        LinearLayout.LayoutParams lp = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        lp.setMargins(l, t, r, b);
        return lp;
    }

    private int dp(int value) {
        return Math.round(value * density);
    }
}
