package com.limelight.binding.input.pad;

import android.annotation.SuppressLint;
import android.content.Context;
import android.graphics.Canvas;
import android.graphics.Paint;
import android.graphics.RectF;
import android.view.Gravity;
import android.view.LayoutInflater;
import android.view.MotionEvent;
import android.view.View;
import android.widget.FrameLayout;
import android.widget.TextView;

import com.google.android.material.button.MaterialButton;
import com.google.android.material.materialswitch.MaterialSwitch;
import com.google.android.material.slider.Slider;
import com.limelight.R;

import java.util.Locale;

/**
 * Edits the modern controller's layout over a dimmed stream: drag a control to move it, pinch it to
 * resize it, tap it to pick it for the panel's size, opacity and hide buttons. With nothing picked
 * the panel changes every control. Done keeps the draft, for all games or only this one; Cancel and
 * Back drop it. Hidden controls stay here as faint ghosts so they can be shown again.
 */
@SuppressLint("ViewConstructor") // Created by ModernController.
final class PadEditor extends FrameLayout {
    /** Moved controls land on this grid, so rows and mirrored pairs line up without effort. */
    static final float GRID_DP = 8;
    private static final float SCALE_STEP = .1f, PANEL_WIDTH_DP = 400, PANEL_MARGIN_DP = 16;

    private final ModernController controller;
    private final PadPainter painter;
    private final PadLayout draft;
    private final boolean gameAvailable;
    private boolean perGame;
    private PadLayout.Control selected;
    private final Surface surface;
    private final View panel;
    private final TextView selection, sizeText;
    private final MaterialButton hide;
    private final Slider opacity;
    private final float density;
    private boolean settingOpacity;

    PadEditor(Context context, ModernController controller, PadPainter painter, PadLayout draft,
              boolean perGame, boolean gameAvailable) {
        super(context);
        this.controller = controller;
        this.painter = painter;
        this.draft = draft;
        this.perGame = perGame && gameAvailable;
        this.gameAvailable = gameAvailable;
        density = context.getResources().getDisplayMetrics().density;
        surface = new Surface(context);
        addView(surface, new LayoutParams(LayoutParams.MATCH_PARENT, LayoutParams.MATCH_PARENT));
        panel = LayoutInflater.from(context).inflate(R.layout.pad_editor_panel, this, false);
        addView(panel, new LayoutParams(LayoutParams.WRAP_CONTENT, LayoutParams.WRAP_CONTENT, Gravity.CENTER));
        selection = panel.findViewById(R.id.pad_editor_selection);
        sizeText = panel.findViewById(R.id.pad_editor_size);
        hide = panel.findViewById(R.id.pad_editor_hide);
        opacity = panel.findViewById(R.id.pad_editor_opacity);
        panel.findViewById(R.id.pad_editor_smaller).setOnClickListener(v -> scaleBy(-SCALE_STEP));
        panel.findViewById(R.id.pad_editor_larger).setOnClickListener(v -> scaleBy(SCALE_STEP));
        hide.setOnClickListener(v -> toggleHidden());
        panel.findViewById(R.id.pad_editor_reset).setOnClickListener(v -> {
            draft.reset();
            select(null);
        });
        panel.findViewById(R.id.pad_editor_cancel).setOnClickListener(v -> controller.finishEditing(false));
        panel.findViewById(R.id.pad_editor_done).setOnClickListener(v -> controller.finishEditing(true));
        MaterialSwitch thisGame = panel.findViewById(R.id.pad_editor_this_game);
        thisGame.setVisibility(gameAvailable ? VISIBLE : GONE);
        thisGame.setChecked(this.perGame);
        thisGame.setOnCheckedChangeListener((button, checked) -> this.perGame = checked);
        opacity.addOnChangeListener((slider, value, fromUser) -> {
            if (fromUser && !settingOpacity) setOpacity(Math.round(value));
        });
        setClickable(true);
        updatePanel();
    }

    @Override
    protected void onSizeChanged(int w, int h, int oldw, int oldh) {
        super.onSizeChanged(w, h, oldw, oldh);
        // As wide as the design asks on a tablet, never wider than a phone in portrait leaves room for.
        int width = Math.round(Math.min(PANEL_WIDTH_DP * density, w - 2 * PANEL_MARGIN_DP * density));
        LayoutParams params = (LayoutParams) panel.getLayoutParams();
        if (params.width != width) {
            params.width = Math.max(0, width);
            post(() -> panel.setLayoutParams(params));
        }
    }

    PadLayout draft() {
        return draft;
    }

    boolean perGame() {
        return perGame && gameAvailable;
    }

    PadLayout.Control selected() {
        return selected;
    }

    void select(PadLayout.Control control) {
        selected = control;
        updatePanel();
        surface.invalidate();
    }

    /** The controls the editor shows: everything but what the settings keep off. */
    private boolean editable(PadLayout.Control control) {
        return !controller.forcedHidden(control);
    }

    void scaleBy(float step) {
        for (PadLayout.Control control : PadLayout.Control.values()) {
            if (!editable(control) || (selected != null && control != selected)) continue;
            // Round to the step so repeated taps land on 50 %, 60 %, ... rather than drifting.
            float scale = Math.round((draft.spec(control).scale + step) / SCALE_STEP) * SCALE_STEP;
            draft.setScale(control, scale);
        }
        changed();
    }

    void setOpacity(int value) {
        for (PadLayout.Control control : PadLayout.Control.values()) {
            if (!editable(control) || (selected != null && control != selected)) continue;
            draft.setOpacity(control, value);
        }
        surface.invalidate();
    }

    void toggleHidden() {
        if (selected == null) return;
        PadLayout.Spec spec = draft.spec(selected);
        spec.hidden = !spec.hidden;
        changed();
    }

    private void changed() {
        updatePanel();
        surface.invalidate();
    }

    private PadLayout.Spec reference() {
        if (selected != null) return draft.spec(selected);
        for (PadLayout.Control control : PadLayout.Control.values()) {
            if (editable(control)) return draft.spec(control);
        }
        return draft.spec(PadLayout.Control.L3);
    }

    private void updatePanel() {
        PadLayout.Spec spec = reference();
        String name = selected == null ? getContext().getString(R.string.pad_edit_all) : name(getContext(), selected);
        if (selected != null && spec.hidden) name = getContext().getString(R.string.pad_edit_hidden, name);
        selection.setText(name);
        sizeText.setText(String.format(Locale.getDefault(), "%d %%", Math.round(spec.scale * 100)));
        settingOpacity = true;
        opacity.setValue(Math.max(PadLayout.MIN_OPACITY, Math.min(PadLayout.MAX_OPACITY, Math.round(spec.opacity / 5f) * 5)));
        settingOpacity = false;
        hide.setEnabled(selected != null);
        hide.setText(selected != null && spec.hidden ? R.string.pad_edit_show : R.string.pad_edit_hide);
        hide.setIconResource(selected != null && spec.hidden ? R.drawable.ic_visibility : R.drawable.ic_visibility_off);
    }

    /** The control's name for people: its label, or what it is when it has none. */
    static String name(Context context, PadLayout.Control control) {
        switch (control) {
            case LEFT_STICK: return context.getString(R.string.pad_left_stick);
            case RIGHT_STICK: return context.getString(R.string.pad_right_stick);
            case DPAD: return context.getString(R.string.pad_dpad);
            case FACE: return context.getString(R.string.pad_face);
            case SELECT: return context.getString(R.string.pad_view);
            case START: return context.getString(R.string.pad_menu);
            case HOME: return context.getString(R.string.pad_home);
            default: return control.name();
        }
    }

    /** Moves the panel to the other half when the picked control sits under it. */
    private void keepPanelClear() {
        if (selected == null || panel.getWidth() == 0) return;
        float[] box = draft.bounds(selected, getWidth(), getHeight(), density, new float[4]);
        RectF control = new RectF(box[0], box[1], box[0] + box[2], box[1] + box[3]);
        RectF under = new RectF(panel.getLeft(), panel.getTop(), panel.getRight(), panel.getBottom());
        if (!RectF.intersects(control, under)) return;
        LayoutParams params = (LayoutParams) panel.getLayoutParams();
        boolean controlHigh = control.centerY() < getHeight() / 2f;
        params.gravity = Gravity.CENTER_HORIZONTAL | (controlHigh ? Gravity.BOTTOM : Gravity.TOP);
        params.topMargin = params.bottomMargin = Math.round(16 * density);
        panel.setLayoutParams(params);
    }

    /** Draws the dimmed stream, the grid, every control and the outlines, and takes the editing touches. */
    private final class Surface extends View {
        private final Paint scrim = new Paint(), grid = new Paint();
        private final float[] box = new float[4];
        private final PadPainter.State idle = new PadPainter.State();
        private float offsetX, offsetY, pinchDistance, pinchScale;
        private boolean dragging, pinching;

        Surface(Context context) {
            super(context);
            scrim.setColor(0x99000000);
            grid.setColor(0x14FFFFFF);
            grid.setStrokeWidth(Math.max(1, density));
        }

        @Override
        protected void onDraw(Canvas canvas) {
            float width = getWidth(), height = getHeight();
            canvas.drawRect(0, 0, width, height, scrim);
            float step = GRID_DP * 3 * density;
            for (float x = step; x < width; x += step) canvas.drawLine(x, 0, x, height, grid);
            for (float y = step; y < height; y += step) canvas.drawLine(0, y, width, y, grid);
            float base = controller.prefs().oscOpacity / 100f;
            for (PadLayout.Control control : PadLayout.Control.values()) {
                if (!editable(control)) continue;
                PadLayout.Spec spec = draft.spec(control);
                draft.bounds(control, width, height, density, box);
                // Show the real opacity, but never so faint that a control cannot be found.
                float alpha = spec.hidden ? .3f : Math.max(.35f, base * spec.opacity / 100f);
                int saved = canvas.saveLayerAlpha(box[0], box[1], box[0] + box[2], box[1] + box[3],
                        Math.round(alpha * 255));
                painter.draw(canvas, control, box[0], box[1], box[2], box[3], idle);
                canvas.restoreToCount(saved);
                painter.drawOutline(canvas, box[0], box[1], box[2], box[3], control == selected, spec.hidden);
            }
        }

        private PadLayout.Control hit(float x, float y) {
            PadLayout.Control[] controls = PadLayout.Control.values();
            float slop = 8 * density;
            // Last drawn is on top, so it wins.
            for (int i = controls.length - 1; i >= 0; i--) {
                if (!editable(controls[i])) continue;
                draft.bounds(controls[i], getWidth(), getHeight(), density, box);
                if (x >= box[0] - slop && x <= box[0] + box[2] + slop && y >= box[1] - slop && y <= box[1] + box[3] + slop) {
                    return controls[i];
                }
            }
            return null;
        }

        @SuppressLint("ClickableViewAccessibility") // Editing gestures; the panel's buttons are the accessible path.
        @Override
        public boolean onTouchEvent(MotionEvent event) {
            switch (event.getActionMasked()) {
                case MotionEvent.ACTION_DOWN: {
                    PadLayout.Control control = hit(event.getX(), event.getY());
                    select(control);
                    dragging = control != null;
                    pinching = false;
                    if (dragging) {
                        draft.bounds(control, getWidth(), getHeight(), density, box);
                        offsetX = box[0] + box[2] / 2 - event.getX();
                        offsetY = box[1] + box[3] / 2 - event.getY();
                    }
                    break;
                }
                case MotionEvent.ACTION_POINTER_DOWN:
                    if (selected != null && event.getPointerCount() == 2) {
                        pinching = true;
                        dragging = false;
                        pinchDistance = distance(event);
                        pinchScale = draft.spec(selected).scale;
                    }
                    break;
                case MotionEvent.ACTION_MOVE:
                    if (pinching && event.getPointerCount() >= 2 && pinchDistance > 0) {
                        draft.setScale(selected, pinchScale * distance(event) / pinchDistance);
                        changed();
                    } else if (dragging && selected != null) {
                        float grid = GRID_DP * density;
                        float x = Math.round((event.getX() + offsetX) / grid) * grid;
                        float y = Math.round((event.getY() + offsetY) / grid) * grid;
                        draft.moveTo(selected, x, y, getWidth(), getHeight(), density);
                        invalidate();
                    }
                    break;
                case MotionEvent.ACTION_POINTER_UP:
                    // The pinch ends; the remaining finger does not jump the control.
                    pinching = false;
                    break;
                case MotionEvent.ACTION_UP:
                case MotionEvent.ACTION_CANCEL:
                    dragging = false;
                    pinching = false;
                    keepPanelClear();
                    updatePanel();
                    break;
                default:
                    break;
            }
            return true;
        }

        private float distance(MotionEvent event) {
            return (float) Math.hypot(event.getX(0) - event.getX(1), event.getY(0) - event.getY(1));
        }
    }
}
