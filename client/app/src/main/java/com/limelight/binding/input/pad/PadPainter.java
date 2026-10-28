package com.limelight.binding.input.pad;

import android.content.Context;
import android.graphics.Canvas;
import android.graphics.DashPathEffect;
import android.graphics.Paint;
import android.graphics.Path;
import android.graphics.RectF;
import android.graphics.Typeface;

import androidx.core.content.ContextCompat;

import com.limelight.R;
import com.limelight.nvstream.input.ControllerPacket;

/**
 * Draws the modern controls. Resting controls are dark glass with a thin light rim, so they read over
 * bright and dark games alike; a press fills with the app's amber, like every selected thing in
 * Nightjar. The stream view and the layout editor share it, so the editor shows exactly what plays.
 */
final class PadPainter {
    /** What one control shows besides its shape. */
    static final class State {
        /** Gamepad flags held by this control (face buttons, D-pad directions, a button or a stick click). */
        int flags;
        /** A trigger or button held. */
        boolean pressed;
        /** A stick's position (-1..1, up positive) and its drawn center inside the box, in pixels; NaN centers the base. */
        float stickX, stickY, centerX = Float.NaN, centerY = Float.NaN;
        boolean stickActive;

        void clear() {
            flags = 0; pressed = false; stickX = 0; stickY = 0; centerX = Float.NaN; centerY = Float.NaN; stickActive = false;
        }
    }

    private static final int GLASS = 0x59000000, GLASS_STRONG = 0x80000000, RIM = 0x66FFFFFF, LABEL = 0xEBFFFFFF;
    private final int accent, onAccent;
    private final float density;
    private final Paint fill = new Paint(Paint.ANTI_ALIAS_FLAG), stroke = new Paint(Paint.ANTI_ALIAS_FLAG),
            text = new Paint(Paint.ANTI_ALIAS_FLAG), dashed = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final RectF rect = new RectF();
    private final Path path = new Path();

    PadPainter(Context context) {
        density = context.getResources().getDisplayMetrics().density;
        accent = ContextCompat.getColor(context, R.color.night_primary);
        onAccent = ContextCompat.getColor(context, R.color.night_on_primary);
        stroke.setStyle(Paint.Style.STROKE);
        stroke.setStrokeWidth(1.5f * density);
        stroke.setStrokeCap(Paint.Cap.ROUND);
        stroke.setStrokeJoin(Paint.Join.ROUND);
        text.setTextAlign(Paint.Align.CENTER);
        text.setTypeface(Typeface.create("sans-serif-medium", Typeface.NORMAL));
        dashed.setStyle(Paint.Style.STROKE);
        dashed.setStrokeWidth(1.5f * density);
        dashed.setPathEffect(new DashPathEffect(new float[] {6 * density, 5 * density}, 0));
    }

    int accent() {
        return accent;
    }

    /** Draws {@code control} filling the box ({@code left}, {@code top}, {@code width}, {@code height}). */
    void draw(Canvas canvas, PadLayout.Control control, float left, float top, float width, float height, State state) {
        switch (control.shape) {
            case STICK: drawStick(canvas, control, left, top, width, height, state); break;
            case DPAD: drawDpad(canvas, left, top, Math.min(width, height), state.flags); break;
            case FACE: drawFace(canvas, left, top, Math.min(width, height), state.flags); break;
            case PILL: drawPill(canvas, control, left, top, width, height, state.pressed); break;
            default: drawRound(canvas, control, left, top, Math.min(width, height), state.pressed); break;
        }
    }

    /** The editor's outline around a control: amber when selected, dashed white otherwise. */
    void drawOutline(Canvas canvas, float left, float top, float width, float height, boolean selected, boolean hidden) {
        float pad = 3 * density, radius = 12 * density;
        rect.set(left - pad, top - pad, left + width + pad, top + height + pad);
        if (selected) {
            stroke.setColor(accent);
            stroke.setStrokeWidth(2.5f * density);
            canvas.drawRoundRect(rect, radius, radius, stroke);
            stroke.setStrokeWidth(1.5f * density);
            // Corner handles say "this one can be resized".
            fill.setColor(accent);
            float h = 4 * density;
            canvas.drawCircle(rect.left, rect.top, h, fill);
            canvas.drawCircle(rect.right, rect.top, h, fill);
            canvas.drawCircle(rect.left, rect.bottom, h, fill);
            canvas.drawCircle(rect.right, rect.bottom, h, fill);
        } else {
            dashed.setColor(hidden ? 0x66FFFFFF : 0x99FFFFFF);
            canvas.drawRoundRect(rect, radius, radius, dashed);
        }
    }

    private void glass(Canvas canvas, boolean pressed, float cx, float cy, float r) {
        fill.setColor(pressed ? accent : GLASS);
        canvas.drawCircle(cx, cy, r, fill);
        if (!pressed) {
            stroke.setColor(RIM);
            canvas.drawCircle(cx, cy, r, stroke);
        }
    }

    private void label(Canvas canvas, String label, float cx, float cy, float size, boolean pressed) {
        text.setTextSize(size);
        text.setColor(pressed ? onAccent : LABEL);
        canvas.drawText(label, cx, cy - (text.descent() + text.ascent()) / 2, text);
    }

    private void drawStick(Canvas canvas, PadLayout.Control control, float left, float top, float width, float height, State state) {
        float size = Math.min(width, height);
        float base = size / 2 - stroke.getStrokeWidth();
        // The base stays put; a floating stick's knob starts where the thumb landed.
        float cx = left + width / 2, cy = top + height / 2;
        float originX = cx, originY = cy;
        if (state.stickActive && !Float.isNaN(state.centerX)) {
            originX = left + state.centerX;
            originY = top + state.centerY;
        }
        float travel = travel(size);
        fill.setColor(state.stickActive ? GLASS_STRONG : GLASS);
        canvas.drawCircle(cx, cy, base, fill);
        stroke.setColor(RIM);
        canvas.drawCircle(cx, cy, base, stroke);
        float magnitude = (float) Math.min(1, Math.hypot(state.stickX, state.stickY));
        if (magnitude > 0) {
            // An amber arc on the rim points where the stick is pushed; it grows with the push.
            float angle = (float) Math.toDegrees(Math.atan2(-state.stickY, state.stickX));
            float sweep = 30 + 50 * magnitude;
            stroke.setColor(accent);
            stroke.setStrokeWidth(4 * density);
            float r = base - 3 * density;
            rect.set(cx - r, cy - r, cx + r, cy + r);
            canvas.drawArc(rect, angle - sweep / 2, sweep, false, stroke);
            stroke.setStrokeWidth(1.5f * density);
        }
        boolean clicked = (state.flags & PadInput.buttonFlag(control)) != 0;
        float knob = size * .2f;
        float kx = originX + state.stickX * travel, ky = originY - state.stickY * travel;
        // Kept inside the base so a thumb that landed near the rim never draws the knob off the control.
        float reach = base - knob, ox = kx - cx, oy = ky - cy, off = (float) Math.hypot(ox, oy);
        if (off > reach && off > 0) {
            kx = cx + ox / off * reach;
            ky = cy + oy / off * reach;
        }
        fill.setColor(clicked ? accent : state.stickActive ? 0xE6FFFFFF : 0x99FFFFFF);
        canvas.drawCircle(kx, ky, knob, fill);
        if (clicked) {
            label(canvas, control == PadLayout.Control.LEFT_STICK ? "L3" : "R3", kx, ky, knob * .7f, true);
        } else {
            // A small ring in the knob, like a thumb grip.
            stroke.setColor(state.stickActive ? 0x33000000 : 0x40000000);
            canvas.drawCircle(kx, ky, knob * .55f, stroke);
        }
    }

    /** How far the knob travels for a stick of this size; ModernController uses it as the stick radius too. */
    static float travel(float size) {
        return size * .3f;
    }

    private void drawDpad(Canvas canvas, float left, float top, float size, int flags) {
        float arm = size * .34f, r = size * .07f;
        float cx = left + size / 2, cy = top + size / 2;
        // The plus as one rounded shape, then each held arm filled amber on top.
        path.reset();
        path.addRoundRect(cx - arm / 2, top, cx + arm / 2, top + size, r, r, Path.Direction.CW);
        path.addRoundRect(left, cy - arm / 2, left + size, cy + arm / 2, r, r, Path.Direction.CW);
        path.setFillType(Path.FillType.WINDING);
        fill.setColor(GLASS);
        canvas.drawPath(path, fill);
        stroke.setColor(RIM);
        drawPlusOutline(canvas, left, top, size, arm, r);
        float reach = (size - arm) / 2;
        armFill(canvas, (flags & ControllerPacket.UP_FLAG) != 0, cx - arm / 2, top, cx + arm / 2, top + reach + r, r);
        armFill(canvas, (flags & ControllerPacket.DOWN_FLAG) != 0, cx - arm / 2, top + size - reach - r, cx + arm / 2, top + size, r);
        armFill(canvas, (flags & ControllerPacket.LEFT_FLAG) != 0, left, cy - arm / 2, left + reach + r, cy + arm / 2, r);
        armFill(canvas, (flags & ControllerPacket.RIGHT_FLAG) != 0, left + size - reach - r, cy - arm / 2, left + size, cy + arm / 2, r);
        float chevron = arm * .22f, inset = reach * .5f;
        chevron(canvas, cx, top + inset, 0, -1, chevron, (flags & ControllerPacket.UP_FLAG) != 0);
        chevron(canvas, cx, top + size - inset, 0, 1, chevron, (flags & ControllerPacket.DOWN_FLAG) != 0);
        chevron(canvas, left + inset, cy, -1, 0, chevron, (flags & ControllerPacket.LEFT_FLAG) != 0);
        chevron(canvas, left + size - inset, cy, 1, 0, chevron, (flags & ControllerPacket.RIGHT_FLAG) != 0);
    }

    private void drawPlusOutline(Canvas canvas, float left, float top, float size, float arm, float r) {
        float a = (size - arm) / 2, b = a + arm, right = left + size, bottom = top + size;
        path.reset();
        path.moveTo(left + a + r, top);
        path.lineTo(left + b - r, top);
        path.quadTo(left + b, top, left + b, top + r);
        path.lineTo(left + b, top + a);
        path.lineTo(right - r, top + a);
        path.quadTo(right, top + a, right, top + a + r);
        path.lineTo(right, top + b - r);
        path.quadTo(right, top + b, right - r, top + b);
        path.lineTo(left + b, top + b);
        path.lineTo(left + b, bottom - r);
        path.quadTo(left + b, bottom, left + b - r, bottom);
        path.lineTo(left + a + r, bottom);
        path.quadTo(left + a, bottom, left + a, bottom - r);
        path.lineTo(left + a, top + b);
        path.lineTo(left + r, top + b);
        path.quadTo(left, top + b, left, top + b - r);
        path.lineTo(left, top + a + r);
        path.quadTo(left, top + a, left + r, top + a);
        path.lineTo(left + a, top + a);
        path.lineTo(left + a, top + r);
        path.quadTo(left + a, top, left + a + r, top);
        path.close();
        canvas.drawPath(path, stroke);
    }

    private void armFill(Canvas canvas, boolean held, float l, float t, float r, float b, float radius) {
        if (!held) return;
        fill.setColor(accent);
        canvas.drawRoundRect(l, t, r, b, radius, radius, fill);
    }

    private void chevron(Canvas canvas, float cx, float cy, int dx, int dy, float s, boolean held) {
        path.reset();
        // Tip toward (dx, dy), arms back from it.
        float tipX = cx + dx * s * .6f, tipY = cy + dy * s * .6f;
        float backX = cx - dx * s * .4f, backY = cy - dy * s * .4f;
        path.moveTo(backX - dy * s, backY + dx * s);
        path.lineTo(tipX, tipY);
        path.lineTo(backX + dy * s, backY - dx * s);
        stroke.setColor(held ? onAccent : LABEL);
        stroke.setStrokeWidth(2 * density);
        canvas.drawPath(path, stroke);
        stroke.setStrokeWidth(1.5f * density);
    }

    private void drawFace(Canvas canvas, float left, float top, float size, int flags) {
        float r = size * PadInput.FACE_RADIUS;
        for (int i = 0; i < PadInput.FACE_CENTERS.length; i++) {
            float cx = left + PadInput.FACE_CENTERS[i][0] * size, cy = top + PadInput.FACE_CENTERS[i][1] * size;
            boolean held = (flags & PadInput.FACE_FLAGS[i]) != 0;
            glass(canvas, held, cx, cy, r);
            label(canvas, PadInput.FACE_LABELS[i], cx, cy, r * .95f, held);
        }
    }

    private void drawPill(Canvas canvas, PadLayout.Control control, float left, float top, float width, float height, boolean pressed) {
        float inset = stroke.getStrokeWidth();
        rect.set(left + inset, top + inset, left + width - inset, top + height - inset);
        float radius = rect.height() / 2;
        fill.setColor(pressed ? accent : GLASS);
        canvas.drawRoundRect(rect, radius, radius, fill);
        if (!pressed) {
            stroke.setColor(RIM);
            canvas.drawRoundRect(rect, radius, radius, stroke);
        }
        label(canvas, control.name(), rect.centerX(), rect.centerY(), rect.height() * .42f, pressed);
    }

    private void drawRound(Canvas canvas, PadLayout.Control control, float left, float top, float size, boolean pressed) {
        float cx = left + size / 2, cy = top + size / 2, r = size / 2 - stroke.getStrokeWidth();
        glass(canvas, pressed, cx, cy, r);
        int color = pressed ? onAccent : LABEL;
        float g = r * .42f;
        stroke.setColor(color);
        fill.setColor(color);
        switch (control) {
            case SELECT: {
                // View: two overlapping windows.
                float w = g * 1.1f, h = g * .85f, o = g * .35f;
                rect.set(cx - w / 2 - o, cy - h / 2 - o, cx + w / 2 - o, cy + h / 2 - o);
                canvas.drawRoundRect(rect, g * .15f, g * .15f, stroke);
                rect.offset(2 * o, 2 * o);
                fill.setColor(pressed ? accent : 0xFF2A2A2A);
                canvas.drawRoundRect(rect, g * .15f, g * .15f, fill);
                canvas.drawRoundRect(rect, g * .15f, g * .15f, stroke);
                break;
            }
            case START: {
                // Menu: three lines.
                for (int i = -1; i <= 1; i++) {
                    canvas.drawLine(cx - g * .75f, cy + i * g * .5f, cx + g * .75f, cy + i * g * .5f, stroke);
                }
                break;
            }
            case HOME: {
                // Home: a house.
                path.reset();
                path.moveTo(cx - g * .8f, cy - g * .05f);
                path.lineTo(cx, cy - g * .8f);
                path.lineTo(cx + g * .8f, cy - g * .05f);
                path.moveTo(cx - g * .55f, cy - g * .25f);
                path.lineTo(cx - g * .55f, cy + g * .7f);
                path.lineTo(cx + g * .55f, cy + g * .7f);
                path.lineTo(cx + g * .55f, cy - g * .25f);
                canvas.drawPath(path, stroke);
                break;
            }
            default:
                label(canvas, control.name(), cx, cy, r * .7f, pressed);
                break;
        }
    }
}
