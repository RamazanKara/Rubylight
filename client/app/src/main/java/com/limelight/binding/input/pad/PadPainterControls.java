package com.limelight.binding.input.pad;

import android.graphics.Canvas;
import android.graphics.Matrix;
import android.graphics.Paint;
import android.graphics.Path;
import android.graphics.RadialGradient;
import android.graphics.RectF;
import android.graphics.Shader;

/**
 * Draws the non-stick controls for {@link PadPainter}: D-pad cross and split arms, face cluster and split buttons,
 * shoulder/trigger/grip pills, Steam trackpads and pointer touchpad (with dot grid, mode chip and finger trail), and
 * round system buttons. Reuses scratch paints, paths and buffers so frames allocate nothing.
 */
final class PadPainterControls {
    private static final int DOTS = 0x33FFFFFF;
    /**
     * The face glyphs' colours per controller family, in {@link PadInput#FACE_CENTERS} order; null for the labels'
     * one neutral colour. PlayStation's shapes stay neutral, so they never take on the look of Sony's coloured mark.
     */
    private static final int[][] FACE_COLORS = {
            {0xFF7DD45E, 0xFFFF6B63, 0xFF5AAEFF, 0xFFFFD54F}, // Xbox: A green, B red, X blue, Y yellow
            null,                                              // PlayStation: neutral, as the other labels
            null,                                              // Nintendo: white, as on its pads
    };
    /** A face cluster's free sliver between its buttons and its box, as a share of the box's shorter side. */
    private static final float FACE_SLACK = 1 - PadInput.FACE_CENTERS[0][1] - PadInput.FACE_RADIUS;

    private final PadPainter p;
    private final Paint dots = new Paint(Paint.ANTI_ALIAS_FLAG), trail = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint fingerGlow;
    private final Path path = new Path(), unitPath = new Path();
    private final Matrix matrix = new Matrix();
    /** A touchpad's dot grid, refilled for each pad drawn and grown only when a larger pad needs more. */
    private float[] dotPoints = new float[128];

    PadPainterControls(PadPainter painter) {
        this.p = painter;
        dots.setStyle(Paint.Style.STROKE);
        dots.setStrokeCap(Paint.Cap.ROUND);
        dots.setColor(DOTS);
        trail.setStyle(Paint.Style.STROKE);
        trail.setStrokeCap(Paint.Cap.ROUND);
        fingerGlow = PadPainter.shaderPaint(new RadialGradient(0, 0, 1,
                new int[] {PadPainter.alpha(p.accent, .55f), PadPainter.alpha(p.accent, 0)},
                new float[] {.3f, 1}, Shader.TileMode.CLAMP));
    }

    /**
     * The D-pad as one bevelled glass cross whose arms reach the box's edges, so a wide or tall box spreads it as
     * {@link PadInput#dpad} reads it; the arms keep the thickness of the box's shorter side. A held arm lights amber and
     * eases back once let go; a dimple marks the middle.
     */
    void drawDpad(Canvas canvas, float left, float top, float width, float height, PadPainter.State state) {
        float size = Math.min(width, height), edge = PadPainter.EDGE_DP * p.density;
        float arm = size * PadInput.DPAD_ARM, r = size * PadInput.DPAD_CORNER;
        float cx = left + width / 2, cy = top + height / 2;
        float l = left + edge, t = top + edge, w = width - 2 * edge, h = height - 2 * edge;
        plusOutline(l, t, w, h, arm, r);
        // Its shadow along the outline, then the glass.
        float reach = PadPainter.SHADOW_REACH_DP * p.density;
        int saved = canvas.save();
        canvas.translate(0, Math.min(PadPainter.SHADOW_DROP_DP * p.density, reach / 2));
        p.ring.setStrokeWidth(2 * reach);
        p.ring.setColor(PadPainter.SHADOW_SOFT);
        canvas.drawPath(path, p.ring);
        p.ring.setStrokeWidth(reach);
        p.ring.setColor(PadPainter.SHADOW);
        canvas.drawPath(path, p.ring);
        canvas.restoreToCount(saved);
        p.fill.setColor(PadPainter.GLASS);
        canvas.drawPath(path, p.fill);
        float reachX = (w - arm) / 2, reachY = (h - arm) / 2;
        armFill(canvas, state.press[0], cx - arm / 2, t, cx + arm / 2, t + reachY + r, r);
        armFill(canvas, state.press[1], cx - arm / 2, t + h - reachY - r, cx + arm / 2, t + h, r);
        armFill(canvas, state.press[2], l, cy - arm / 2, l + reachX + r, cy + arm / 2, r);
        armFill(canvas, state.press[3], l + w - reachX - r, cy - arm / 2, l + w, cy + arm / 2, r);
        // The light from above over the whole cross: its outline taken to the box's 0..1 and scaled back.
        if (w > 0 && h > 0) {
            matrix.setTranslate(-l, -t);
            matrix.postScale(1 / w, 1 / h);
            path.transform(matrix, unitPath);
            saved = canvas.save();
            canvas.translate(l, t);
            canvas.scale(w, h);
            canvas.drawPath(unitPath, p.highlight);
            canvas.restoreToCount(saved);
        }
        p.fill.setColor(0x2E000000);
        canvas.drawCircle(cx, cy, arm * .26f, p.fill);
        float rim = PadPainter.RIM_DP * p.density;
        p.stroke.setStrokeWidth(rim);
        p.stroke.setColor(PadPainter.RIM);
        canvas.drawPath(path, p.stroke);
        float chevron = arm * .22f, insetX = reachX * .5f, insetY = reachY * .5f;
        chevron(canvas, cx, t + insetY, 0, -1, chevron, state.press[0]);
        chevron(canvas, cx, t + h - insetY, 0, 1, chevron, state.press[1]);
        chevron(canvas, l + insetX, cy, -1, 0, chevron, state.press[2]);
        chevron(canvas, l + w - insetX, cy, 1, 0, chevron, state.press[3]);
    }

    /** Puts the plus's outline, its arms {@code arm} thick with corners of radius {@code r}, in {@link #path}. */
    private void plusOutline(float left, float top, float width, float height, float arm, float r) {
        float ax = (width - arm) / 2, bx = ax + arm, ay = (height - arm) / 2, by = ay + arm;
        float right = left + width, bottom = top + height;
        path.reset();
        path.moveTo(left + ax + r, top);
        path.lineTo(left + bx - r, top);
        path.quadTo(left + bx, top, left + bx, top + r);
        path.lineTo(left + bx, top + ay);
        path.lineTo(right - r, top + ay);
        path.quadTo(right, top + ay, right, top + ay + r);
        path.lineTo(right, top + by - r);
        path.quadTo(right, top + by, right - r, top + by);
        path.lineTo(left + bx, top + by);
        path.lineTo(left + bx, bottom - r);
        path.quadTo(left + bx, bottom, left + bx - r, bottom);
        path.lineTo(left + ax + r, bottom);
        path.quadTo(left + ax, bottom, left + ax, bottom - r);
        path.lineTo(left + ax, top + by);
        path.lineTo(left + r, top + by);
        path.quadTo(left, top + by, left, top + by - r);
        path.lineTo(left, top + ay + r);
        path.quadTo(left, top + ay, left + r, top + ay);
        path.lineTo(left + ax, top + ay);
        path.lineTo(left + ax, top + r);
        path.quadTo(left + ax, top, left + ax + r, top);
        path.close();
    }

    private void armFill(Canvas canvas, float level, float l, float t, float r, float b, float radius) {
        if (level <= 0) return;
        p.fill.setColor(PadPainter.alpha(p.accent, level));
        canvas.drawRoundRect(l, t, r, b, radius, radius, p.fill);
    }

    private void chevron(Canvas canvas, float cx, float cy, int dx, int dy, float s, float level) {
        path.reset();
        // Tip toward (dx, dy), arms back from it.
        float tipX = cx + dx * s * .6f, tipY = cy + dy * s * .6f;
        float backX = cx - dx * s * .4f, backY = cy - dy * s * .4f;
        path.moveTo(backX - dy * s, backY + dx * s);
        path.lineTo(tipX, tipY);
        path.lineTo(backX + dy * s, backY - dx * s);
        p.stroke.setColor(PadPainter.blend(PadPainter.LABEL, p.onAccent, level));
        p.stroke.setStrokeWidth(2 * p.density);
        canvas.drawPath(path, p.stroke);
    }

    /**
     * The face cluster: each button in its own part of the box (top, bottom, left, right), spread over a wide or tall
     * box as {@link PadInput#face} reads it, and sized to the box's shorter side so the four never overlap.
     */
    void drawFace(Canvas canvas, float left, float top, float width, float height, PadPainter.State state) {
        float size = Math.min(width, height), r = size * PadInput.FACE_RADIUS;
        // The buttons leave a sliver of the box free around them; their shadows stay inside it.
        float reach = Math.max(0, Math.min(PadPainter.SHADOW_REACH_DP * p.density, size * FACE_SLACK - PadPainter.SHADOW_DROP_DP * p.density));
        for (int i = 0; i < PadInput.FACE_CENTERS.length; i++) {
            float cx = left + PadInput.FACE_CENTERS[i][0] * width, cy = top + PadInput.FACE_CENTERS[i][1] * height;
            float level = state.press[i];
            p.disc(canvas, cx, cy, r, r, level, reach);
            faceGlyph(canvas, p.faceButtonAt(i), cx, cy, r - PadPainter.PRESS_IN_DP * p.density * level, level);
        }
    }

    /** One face button of a split cluster: the cluster's glass button filling its box, with the glyph set's glyph. */
    void drawFaceButton(Canvas canvas, PadLayout.Control control, float left, float top, float width,
                        float height, PadPainter.State state) {
        float level = state.press[0], edge = PadPainter.EDGE_DP * p.density;
        float cx = left + width / 2, cy = top + height / 2;
        float rx = width / 2 - edge, ry = height / 2 - edge;
        p.disc(canvas, cx, cy, rx, ry, level, PadPainter.SHADOW_REACH_DP * p.density);
        faceGlyph(canvas, control.memberIndex(), cx, cy, Math.min(rx, ry) - PadPainter.PRESS_IN_DP * p.density * level, level);
    }

    /**
     * The glyph on the face button at {@code index} of radius {@code r}: the family's letter, or PlayStation's shapes
     * drawn as lines so they stay crisp at any size; in the family's colour (PlayStation's neutral), dark on amber
     * while pressed.
     */
    private void faceGlyph(Canvas canvas, int index, float cx, float cy, float r, float level) {
        PadLayout.Glyph glyph = p.glyph();
        int[] colors = FACE_COLORS[glyph.ordinal()];
        int color = PadPainter.blend(colors == null ? PadPainter.LABEL : colors[index], p.onAccent, level);
        if (glyph != PadLayout.Glyph.PLAYSTATION) {
            p.label(canvas, PadGlyphs.face(glyph, index), cx, cy, r * .95f, color);
            return;
        }
        float s = r * .42f;
        p.stroke.setColor(color);
        p.stroke.setStrokeWidth(Math.max(1.5f * p.density, s * .24f));
        switch (index) {
            case 0: // Cross
                canvas.drawLine(cx - s * .8f, cy - s * .8f, cx + s * .8f, cy + s * .8f, p.stroke);
                canvas.drawLine(cx - s * .8f, cy + s * .8f, cx + s * .8f, cy - s * .8f, p.stroke);
                break;
            case 1: // Circle
                canvas.drawCircle(cx, cy, s * .9f, p.stroke);
                break;
            case 2: // Square
                p.rect.set(cx - s * .78f, cy - s * .78f, cx + s * .78f, cy + s * .78f);
                canvas.drawRoundRect(p.rect, s * .1f, s * .1f, p.stroke);
                break;
            default: // Triangle, raised a little so it looks centred
                path.reset();
                path.moveTo(cx, cy - s);
                path.lineTo(cx + s * .98f, cy + s * .66f);
                path.lineTo(cx - s * .98f, cy + s * .66f);
                path.close();
                canvas.drawPath(path, p.stroke);
                break;
        }
    }

    /** The corner radius of a split D-pad arm in {@code shape}, rounded as the cross's arms are. */
    static float armRadius(RectF shape, boolean vertical) {
        float thickness = vertical ? shape.width() : shape.height();
        return Math.min(thickness * PadInput.DPAD_CORNER / PadInput.DPAD_ARM, Math.min(shape.width(), shape.height()) / 2);
    }

    /**
     * One direction of a split D-pad: its arm filling the box, rounded as the D-pad's arms are, with its chevron
     * pointing out; amber while held.
     */
    void drawDpadArm(Canvas canvas, PadLayout.Control control, float left, float top, float width, float height,
                     PadPainter.State state) {
        boolean vertical = control == PadLayout.Control.DPAD_UP || control == PadLayout.Control.DPAD_DOWN;
        float level = state.press[0], edge = PadPainter.EDGE_DP * p.density;
        RectF shape = p.chip;
        shape.set(left + edge, top + edge, left + width - edge, top + height - edge);
        float thickness = vertical ? shape.width() : shape.height();
        p.slab(canvas, shape, armRadius(shape, vertical), level, PadPainter.SHADOW_REACH_DP * p.density);
        int dx = control == PadLayout.Control.DPAD_LEFT ? -1 : control == PadLayout.Control.DPAD_RIGHT ? 1 : 0;
        int dy = control == PadLayout.Control.DPAD_UP ? -1 : control == PadLayout.Control.DPAD_DOWN ? 1 : 0;
        float size = Math.min(thickness, vertical ? shape.height() : shape.width());
        chevron(canvas, shape.centerX(), shape.centerY(), dx, dy, size * .22f, level);
    }

    void drawPill(Canvas canvas, PadLayout.Control control, float left, float top, float width, float height,
                  PadPainter.State state) {
        float level = state.press[0], edge = PadPainter.EDGE_DP * p.density;
        RectF shape = p.chip;
        shape.set(left + edge, top + edge, left + width - edge, top + height - edge);
        // A stadium filling the box, lying or standing: the shorter side is the round end.
        float thickness = Math.min(shape.width(), shape.height());
        p.slab(canvas, shape, thickness / 2, level, PadPainter.SHADOW_REACH_DP * p.density);
        float sunk = Math.min(p.inner.width(), p.inner.height());
        int color = PadPainter.blend(PadPainter.LABEL, p.onAccent, level);
        if (control == PadLayout.Control.TOUCHPAD) {
            // Touchpad: a small pad with its click line, like the one on a PlayStation pad.
            float h = sunk * .4f, w = Math.min(p.inner.width() * .5f, h * 1.8f);
            float l = p.inner.centerX() - w / 2, t = p.inner.centerY() - h / 2;
            p.stroke.setColor(color);
            p.stroke.setStrokeWidth(1.5f * p.density);
            p.rect.set(l, t, l + w, t + h);
            canvas.drawRoundRect(p.rect, h * .25f, h * .25f, p.stroke);
            canvas.drawLine(p.rect.centerX(), t + h * .3f, p.rect.centerX(), t + h * .7f, p.stroke);
            return;
        }
        String name = PadGlyphs.shoulder(control, p.glyph());
        if (name == null) name = PadPainter.pillLabel(control);
        // A long label (the grips') shrinks to stay inside the pill.
        p.text.setTextSize(sunk * .42f);
        float fit = Math.min(1, p.inner.width() * .8f / Math.max(1, p.text.measureText(name)));
        p.label(canvas, name, p.inner.centerX(), p.inner.centerY(), sunk * .42f * fit, color);
    }

    /**
     * A touchpad: a rounded glass pad with a faint dot grid, its rim lit amber while touched. A Steam Controller
     * trackpad shows its side's {@code letter} in the middle; the pointer touchpad shows its mode's word as a small chip
     * in a corner ({@code word}), so a glance says whether it moves the pointer or the camera. The finger shows as an
     * amber dot with a short trail that fades behind it.
     */
    void drawPad(Canvas canvas, float left, float top, float width, float height, PadPainter.State state, float corner,
                 String letter, String word) {
        float edge = PadPainter.EDGE_DP * p.density, level = state.press[0];
        RectF shape = p.chip;
        shape.set(left + edge, top + edge, left + width - edge, top + height - edge);
        float size = Math.min(shape.width(), shape.height()), radius = size * corner;
        p.shadowRound(canvas, shape, radius, PadPainter.SHADOW_REACH_DP * p.density, 1);
        p.fill.setColor(PadPainter.blend(PadPainter.GLASS, PadPainter.GLASS_HELD, level));
        canvas.drawRoundRect(shape, radius, radius, p.fill);
        p.highlightRound(canvas, shape, radius);
        drawDots(canvas, shape, radius);
        float rim = PadPainter.RIM_DP * p.density;
        p.stroke.setStrokeWidth(rim);
        p.stroke.setColor(PadPainter.blend(PadPainter.RIM, p.accent, level));
        p.rect.set(shape);
        p.rect.inset(rim / 2, rim / 2);
        canvas.drawRoundRect(p.rect, radius, radius, p.stroke);
        if (letter != null) p.label(canvas, letter, shape.centerX(), shape.centerY(), size * .22f, PadPainter.alpha(PadPainter.LABEL, .6f));
        if (word != null && !word.isEmpty()) drawChip(canvas, word, shape, size, radius);
        drawTrail(canvas, left, top, width, height, state);
        if (!Float.isNaN(state.touchX)) {
            float fx = left + Math.max(0, Math.min(width, state.touchX)), fy = top + Math.max(0, Math.min(height, state.touchY));
            float dot = Math.min(7 * p.density, size * .1f);
            PadPainter.unit(canvas, fingerGlow, fx, fy, dot * 2.6f, dot * 2.6f);
            p.fill.setColor(p.accent);
            canvas.drawCircle(fx, fy, dot, p.fill);
            p.stroke.setStrokeWidth(1.5f * p.density);
            p.stroke.setColor(0xCCFFFFFF);
            canvas.drawCircle(fx, fy, dot, p.stroke);
        }
    }

    /** A faint grid of dots over a touchpad's surface, one draw for all of them. */
    private void drawDots(Canvas canvas, RectF area, float radius) {
        float step = 16 * p.density, margin = Math.max(radius * .7f, 12 * p.density);
        float l = area.left + margin, t = area.top + margin, r = area.right - margin, b = area.bottom - margin;
        if (r < l || b < t) return;
        int columns = (int) ((r - l) / step) + 1, rows = (int) ((b - t) / step) + 1;
        float x0 = (l + r - (columns - 1) * step) / 2, y0 = (t + b - (rows - 1) * step) / 2;
        int count = columns * rows * 2;
        if (dotPoints.length < count) dotPoints = new float[count];
        int k = 0;
        for (int row = 0; row < rows; row++) {
            for (int column = 0; column < columns; column++) {
                dotPoints[k++] = x0 + column * step;
                dotPoints[k++] = y0 + row * step;
            }
        }
        dots.setStrokeWidth(2 * p.density);
        canvas.drawPoints(dotPoints, 0, k, dots);
    }

    /** The pointer touchpad's mode word as a small amber chip in its upper left corner, sized to the pad. */
    private void drawChip(Canvas canvas, String word, RectF area, float size, float radius) {
        float textSize = Math.max(7 * p.density, Math.min(12 * p.density, size * .14f));
        p.text.setTextSize(textSize);
        float pad = textSize * .7f, w = p.text.measureText(word) + 2 * pad, h = textSize * 1.75f;
        float inset = Math.max(4 * p.density, Math.min(radius * .45f, size * .14f));
        p.rect.set(area.left + inset, area.top + inset, area.left + inset + w, area.top + inset + h);
        p.fill.setColor(PadPainter.alpha(p.accent, .22f));
        canvas.drawRoundRect(p.rect, h / 2, h / 2, p.fill);
        p.stroke.setStrokeWidth(p.density);
        p.stroke.setColor(PadPainter.alpha(p.accent, .6f));
        canvas.drawRoundRect(p.rect, h / 2, h / 2, p.stroke);
        p.label(canvas, word, p.rect.centerX(), p.rect.centerY(), textSize, p.accent);
    }

    /** The finger's last places as amber strokes, thinner and fainter the older they are, gone after {@link PadPainter#TRAIL_MS}. */
    private void drawTrail(Canvas canvas, float left, float top, float width, float height, PadPainter.State state) {
        if (state.trailCount < 2) return;
        for (int i = 1; i < state.trailCount; i++) {
            int from = (state.trailStart + i - 1) % PadPainter.TRAIL, to = (state.trailStart + i) % PadPainter.TRAIL;
            long age = Math.max(0, state.now - state.trailAt[to]);
            if (age >= PadPainter.TRAIL_MS) continue;
            float fade = 1 - age / (float) PadPainter.TRAIL_MS, along = i / (float) (state.trailCount - 1);
            trail.setStrokeWidth((1.5f + 3.5f * along) * p.density);
            trail.setColor(PadPainter.alpha(p.accent, .7f * fade * (.3f + .7f * along)));
            canvas.drawLine(left + Math.max(0, Math.min(width, state.trailX[from])), top + Math.max(0, Math.min(height, state.trailY[from])),
                    left + Math.max(0, Math.min(width, state.trailX[to])), top + Math.max(0, Math.min(height, state.trailY[to])), trail);
        }
    }

    /** A round button filling its box: a disc in a square one, an oval in a resized one, its mark in the middle. */
    void drawRound(Canvas canvas, PadLayout.Control control, float left, float top, float width, float height,
                   PadPainter.State state) {
        float level = state.press[0], edge = PadPainter.EDGE_DP * p.density;
        float cx = left + width / 2, cy = top + height / 2;
        float rx = width / 2 - edge, ry = height / 2 - edge;
        p.disc(canvas, cx, cy, rx, ry, level, PadPainter.SHADOW_REACH_DP * p.density);
        float r = Math.min(rx, ry) - PadPainter.PRESS_IN_DP * p.density * level;
        int color = PadPainter.blend(PadPainter.LABEL, p.onAccent, level);
        float g = r * .42f;
        p.stroke.setColor(color);
        p.stroke.setStrokeWidth(1.75f * p.density);
        switch (control) {
            case SELECT: {
                // View: two overlapping windows.
                float w = g * 1.1f, h = g * .85f, o = g * .35f;
                p.rect.set(cx - w / 2 - o, cy - h / 2 - o, cx + w / 2 - o, cy + h / 2 - o);
                canvas.drawRoundRect(p.rect, g * .15f, g * .15f, p.stroke);
                p.rect.offset(2 * o, 2 * o);
                p.fill.setColor(PadPainter.blend(0xFF22262E, p.accent, level));
                canvas.drawRoundRect(p.rect, g * .15f, g * .15f, p.fill);
                canvas.drawRoundRect(p.rect, g * .15f, g * .15f, p.stroke);
                break;
            }
            case START: {
                // Menu: three lines.
                for (int i = -1; i <= 1; i++) {
                    canvas.drawLine(cx - g * .75f, cy + i * g * .5f, cx + g * .75f, cy + i * g * .5f, p.stroke);
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
                canvas.drawPath(path, p.stroke);
                break;
            }
            default:
                p.label(canvas, control.name(), cx, cy, r * .7f, color);
                break;
        }
    }
}
