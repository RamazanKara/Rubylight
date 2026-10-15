package com.limelight.ui;

import android.annotation.SuppressLint;
import android.content.Context;
import android.graphics.Canvas;
import android.graphics.Paint;
import android.graphics.RectF;
import android.view.View;
import com.limelight.binding.input.LayoutStick;
import com.limelight.binding.input.TouchLayout;

/** A stick, D-pad or WASD control of a game layout. It draws its state; GameTouchControls drives it. */
@SuppressLint("ViewConstructor") // Created by GameTouchControls for one layout control.
final class LayoutPadView extends View {
    final TouchLayout.Button control;
    final LayoutStick stick;
    /** WASD: the {@link LayoutStick} direction bits whose keys are held. */
    int directions;
    // Floating sticks start where the thumb lands; this keeps the base a thumb's size inside a large zone.
    private final float thumbRadius;
    private final Paint fill = new Paint(Paint.ANTI_ALIAS_FLAG), line = new Paint(Paint.ANTI_ALIAS_FLAG),
            text = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final RectF arm = new RectF();
    private final String[] labels;
    private boolean editing;

    LayoutPadView(Context context, TouchLayout.Button control) {
        super(context);
        this.control = control;
        stick = new LayoutStick(control.floating,
                control.kind == TouchLayout.Kind.DPAD ? LayoutStick.DPAD_DEADZONE : control.deadzone);
        float density = context.getResources().getDisplayMetrics().density;
        thumbRadius = 64 * density;
        line.setStyle(Paint.Style.STROKE);
        line.setStrokeWidth(2 * density);
        text.setTextAlign(Paint.Align.CENTER);
        text.setTextSize(12 * density * context.getResources().getConfiguration().fontScale);
        text.setColor(0xFFFFFFFF);
        labels = new String[4];
        if (control.kind == TouchLayout.Kind.WASD) {
            int[] keys = control.keys();
            for (int i = 0; i < 4; i++) labels[i] = label(keys[i]);
        }
    }

    void setEditing(boolean editing) {
        this.editing = editing;
        invalidate();
    }

    /** The stick's travel in pixels for this view's size. */
    float radius() {
        float half = Math.min(getWidth(), getHeight()) / 2f;
        return control.floating ? Math.min(half, thumbRadius) : half;
    }

    @Override protected void onDraw(Canvas canvas) {
        float width = getWidth(), height = getHeight();
        if (editing || control.floating) {
            // The zone that takes the finger: a floating stick's whole area, or any control while editing.
            fill.setColor(editing ? 0x40556677 : 0x18FFFFFF);
            canvas.drawRoundRect(0, 0, width, height, 12, 12, fill);
        }
        float cx = width / 2, cy = height / 2, radius = radius();
        if (control.floating) {
            if (!stick.active()) {
                line.setColor(0x80FFFFFF);
                canvas.drawCircle(cx, cy, radius * .5f, line);
                canvas.drawText(control.name, cx, cy + radius * .5f + text.getTextSize() * 1.4f, text);
                return;
            }
            cx = stick.centerX(); cy = stick.centerY();
        }
        fill.setColor(0x6030454D);
        line.setColor(0xA0FFFFFF);
        if (control.kind == TouchLayout.Kind.DPAD) {
            float thickness = radius * .66f;
            int pressed = LayoutStick.directions(stick.x, stick.y, LayoutStick.DPAD_DIAGONAL);
            drawArm(canvas, cx - thickness / 2, cy - radius, cx + thickness / 2, cy, (pressed & LayoutStick.UP) != 0);
            drawArm(canvas, cx - thickness / 2, cy, cx + thickness / 2, cy + radius, (pressed & LayoutStick.DOWN) != 0);
            drawArm(canvas, cx - radius, cy - thickness / 2, cx, cy + thickness / 2, (pressed & LayoutStick.LEFT) != 0);
            drawArm(canvas, cx, cy - thickness / 2, cx + radius, cy + thickness / 2, (pressed & LayoutStick.RIGHT) != 0);
            return;
        }
        canvas.drawCircle(cx, cy, radius * .95f, fill);
        canvas.drawCircle(cx, cy, radius * .95f, line);
        if (control.kind == TouchLayout.Kind.WASD) {
            // The four keys around the ring, lit while held.
            float r = radius * .66f, baseline = text.getTextSize() * .35f;
            text.setAlpha((directions & LayoutStick.UP) != 0 ? 255 : 150);
            canvas.drawText(labels[0], cx, cy - r + baseline, text);
            text.setAlpha((directions & LayoutStick.LEFT) != 0 ? 255 : 150);
            canvas.drawText(labels[1], cx - r, cy + baseline, text);
            text.setAlpha((directions & LayoutStick.DOWN) != 0 ? 255 : 150);
            canvas.drawText(labels[2], cx, cy + r + baseline, text);
            text.setAlpha((directions & LayoutStick.RIGHT) != 0 ? 255 : 150);
            canvas.drawText(labels[3], cx + r, cy + baseline, text);
            text.setAlpha(255);
        }
        // The knob follows the stick's output, so a deadzone shows as a knob that stays put.
        fill.setColor(0xC0556677);
        canvas.drawCircle(cx + stick.x * radius * .6f, cy - stick.y * radius * .6f, radius * .35f, fill);
    }

    private void drawArm(Canvas canvas, float left, float top, float right, float bottom, boolean pressed) {
        arm.set(left, top, right, bottom);
        fill.setColor(pressed ? 0xC0556677 : 0x6030454D);
        canvas.drawRect(arm, fill);
        canvas.drawRect(arm, line);
    }

    private static String label(int key) {
        String name = com.limelight.binding.input.KeyboardShortcuts.format(new int[] {key});
        return name.length() > 4 ? name.substring(0, 4) : name;
    }
}
