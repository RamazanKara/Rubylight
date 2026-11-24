package com.limelight.binding.input.pad;

import android.graphics.Canvas;
import android.graphics.Path;
import android.graphics.RectF;
import android.graphics.Region;
import android.os.Build;

/**
 * Draws the press cue past a control's edge for {@link PadPainter}: an amber halo around every held or easing part and
 * an expanding press ring so a thumb covering a small button still shows it answered. Reuses {@link PadPainter}'s
 * scratch paint and rectangles so frames allocate nothing.
 */
final class PadPainterHalo {
    /** The halo's reach past the edge in dp and its strength in rings from the edge out, and how far the press ring runs. */
    private static final float HALO_DP = 12, RIPPLE_DP = 20;
    private static final float[] HALO = {.40f, .28f, .18f, .10f, .05f, .02f};

    private final PadPainter p;
    private final Path clip = new Path();

    PadPainterHalo(PadPainter painter) {
        this.p = painter;
    }

    /**
     * Draws the press halo and expanding ring for {@code control}. Returns whether a press ring is still running so the
     * halo layer can schedule the next animation frame.
     */
    boolean drawHalo(Canvas canvas, PadLayout.Control control, float left, float top, float width, float height,
                     PadPainter.State state, float alpha, long now) {
        if (alpha <= 0) return false;
        float edge = PadPainter.EDGE_DP * p.density;
        boolean running = false;
        switch (control.shape) {
            case FACE: {
                float size = Math.min(width, height), r = size * PadInput.FACE_RADIUS;
                if (!lit(state, PadPainter.PARTS, now)) break;
                // Around the held button and between the others, never across them.
                int saved = canvas.save();
                clip.reset();
                for (int i = 0; i < PadPainter.PARTS; i++) {
                    clip.addCircle(left + PadInput.FACE_CENTERS[i][0] * width, top + PadInput.FACE_CENTERS[i][1] * height, r,
                            Path.Direction.CW);
                }
                clipOut(canvas, clip);
                for (int i = 0; i < PadPainter.PARTS; i++) {
                    running |= haloOval(canvas, left + PadInput.FACE_CENTERS[i][0] * width,
                            top + PadInput.FACE_CENTERS[i][1] * height, r, r, state, i, alpha, now);
                }
                canvas.restoreToCount(saved);
                break;
            }
            case ROUND:
            case FACE_BUTTON:
                running = haloOval(canvas, left + width / 2, top + height / 2, width / 2 - edge, height / 2 - edge, state, 0,
                        alpha, now);
                break;
            case PILL:
                p.rect.set(left + edge, top + edge, left + width - edge, top + height - edge);
                running = haloRound(canvas, p.rect, Math.min(p.rect.width(), p.rect.height()) / 2, state, 0, alpha, now);
                break;
            case DPAD_ARM: {
                boolean vertical = control == PadLayout.Control.DPAD_UP || control == PadLayout.Control.DPAD_DOWN;
                p.rect.set(left + edge, top + edge, left + width - edge, top + height - edge);
                running = haloRound(canvas, p.rect, PadPainterControls.armRadius(p.rect, vertical), state, 0, alpha, now);
                break;
            }
            case DPAD: {
                float size = Math.min(width, height), arm = size * PadInput.DPAD_ARM, r = size * PadInput.DPAD_CORNER;
                float cx = left + width / 2, cy = top + height / 2, reach = (HALO_DP + RIPPLE_DP) * p.density;
                float l = left + edge, t = top + edge, rgt = left + width - edge, b = top + height - edge;
                for (int i = 0; i < PadPainter.PARTS; i++) {
                    if (state.press[i] <= 0 && !rippling(state, i, now)) continue;
                    // The arm from its tip to the bar across it; the halo only past that bar, outside the cross.
                    int saved = canvas.save();
                    switch (i) {
                        case 0:
                            p.rect.set(cx - arm / 2, t, cx + arm / 2, cy - arm / 2);
                            canvas.clipRect(l - reach, t - reach, rgt + reach, cy - arm / 2);
                            break;
                        case 1:
                            p.rect.set(cx - arm / 2, cy + arm / 2, cx + arm / 2, b);
                            canvas.clipRect(l - reach, cy + arm / 2, rgt + reach, b + reach);
                            break;
                        case 2:
                            p.rect.set(l, cy - arm / 2, cx - arm / 2, cy + arm / 2);
                            canvas.clipRect(l - reach, t - reach, cx - arm / 2, b + reach);
                            break;
                        default:
                            p.rect.set(cx + arm / 2, cy - arm / 2, rgt, cy + arm / 2);
                            canvas.clipRect(cx + arm / 2, t - reach, rgt + reach, b + reach);
                            break;
                    }
                    running |= haloRound(canvas, p.rect, r, state, i, alpha, now);
                    canvas.restoreToCount(saved);
                }
                break;
            }
            case STICK: {
                float base = p.stickBaseFor(Math.min(width, height));
                float cx = left + width / 2, cy = top + height / 2;
                if (control != PadLayout.Control.WASD) {
                    running = haloOval(canvas, cx, cy, base, base, state, 0, alpha, now);
                    break;
                }
                // Each held key on its own side of the ring, as the D-pad's arms: up, down, left, right.
                float reach = base + (HALO_DP + RIPPLE_DP) * p.density, side = base * .5f;
                for (int i = 0; i < PadPainter.PARTS; i++) {
                    if (state.press[i] <= 0 && !rippling(state, i, now)) continue;
                    int saved = canvas.save();
                    switch (i) {
                        case 0: canvas.clipRect(cx - reach, cy - reach, cx + reach, cy - side); break;
                        case 1: canvas.clipRect(cx - reach, cy + side, cx + reach, cy + reach); break;
                        case 2: canvas.clipRect(cx - reach, cy - reach, cx - side, cy + reach); break;
                        default: canvas.clipRect(cx + side, cy - reach, cx + reach, cy + reach); break;
                    }
                    running |= haloOval(canvas, cx, cy, base, base, state, i, alpha, now);
                    canvas.restoreToCount(saved);
                }
                break;
            }
            case PAD:
            case TOUCHPAD: {
                p.rect.set(left + edge, top + edge, left + width - edge, top + height - edge);
                float corner = control.shape == PadLayout.Shape.PAD ? PadPainter.PAD_CORNER : PadPainter.TOUCHPAD_CORNER;
                running = haloRound(canvas, p.rect, Math.min(p.rect.width(), p.rect.height()) * corner, state, 0, alpha, now);
                break;
            }
            default:
                break;
        }
        return running;
    }

    /** Whether any of the first {@code parts} parts is pressed, easing back or ringing. */
    private static boolean lit(PadPainter.State state, int parts, long now) {
        for (int i = 0; i < parts; i++) {
            if (state.press[i] > 0 || rippling(state, i, now)) return true;
        }
        return false;
    }

    private static boolean rippling(PadPainter.State state, int part, long now) {
        long since = now - state.pressedAt[part];
        return state.pressedAt[part] != 0 && since >= 0 && since < PadPainter.RIPPLE_MS;
    }

    private static void clipOut(Canvas canvas, Path shape) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            canvas.clipOutPath(shape);
        } else {
            canvas.clipPath(shape, Region.Op.DIFFERENCE);
        }
    }

    /** The halo and press ring around an oval or disc part. */
    private boolean haloOval(Canvas canvas, float cx, float cy, float rx, float ry, PadPainter.State state, int part,
                             float alpha, long now) {
        float level = state.press[part];
        if (level > 0) {
            float step = HALO_DP * p.density / HALO.length;
            p.ring.setStrokeWidth(step);
            for (int k = 0; k < HALO.length; k++) {
                float out = (k + .5f) * step;
                p.ring.setColor(PadPainter.alpha(p.accent, alpha * level * HALO[k]));
                p.oval(canvas, cx, cy, rx + out, ry + out, p.ring);
            }
        }
        float out = ripple(state, part, alpha, now);
        if (out < 0) return false;
        p.oval(canvas, cx, cy, rx + out, ry + out, p.ring);
        return true;
    }

    /** The halo and press ring around a rounded part. */
    private boolean haloRound(Canvas canvas, RectF shape, float radius, PadPainter.State state, int part,
                              float alpha, long now) {
        float level = state.press[part];
        if (level > 0) {
            float step = HALO_DP * p.density / HALO.length;
            p.ring.setStrokeWidth(step);
            for (int k = 0; k < HALO.length; k++) {
                float out = (k + .5f) * step;
                p.ring.setColor(PadPainter.alpha(p.accent, alpha * level * HALO[k]));
                p.inner.set(shape);
                p.inner.inset(-out, -out);
                canvas.drawRoundRect(p.inner, radius + out, radius + out, p.ring);
            }
        }
        float out = ripple(state, part, alpha, now);
        if (out < 0) return false;
        p.inner.set(shape);
        p.inner.inset(-out, -out);
        canvas.drawRoundRect(p.inner, radius + out, radius + out, p.ring);
        return true;
    }

    /**
     * Sets the ring paint for the press ring of {@code part} at {@code now} and returns how far past the edge it is,
     * or -1 when none runs: it starts at the edge and runs out {@link #RIPPLE_DP}, slowing and fading as it goes.
     */
    private float ripple(PadPainter.State state, int part, float alpha, long now) {
        if (!rippling(state, part, now)) return -1;
        float t = (now - state.pressedAt[part]) / (float) PadPainter.RIPPLE_MS, rest = 1 - t;
        p.ring.setStrokeWidth(2 * p.density);
        p.ring.setColor(PadPainter.alpha(p.accent, alpha * .6f * rest * rest));
        return (2 + (RIPPLE_DP - 2) * (1 - rest * rest)) * p.density;
    }
}
