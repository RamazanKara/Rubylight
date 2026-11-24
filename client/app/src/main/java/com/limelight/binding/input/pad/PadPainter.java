package com.limelight.binding.input.pad;

import android.content.Context;
import android.graphics.Canvas;
import android.graphics.Color;
import android.graphics.DashPathEffect;
import android.graphics.LinearGradient;
import android.graphics.Paint;
import android.graphics.RadialGradient;
import android.graphics.RectF;
import android.graphics.Shader;
import android.graphics.Typeface;

import androidx.core.content.ContextCompat;
import androidx.core.graphics.ColorUtils;

import com.limelight.R;
import com.limelight.binding.input.LayoutStick;
import com.limelight.binding.input.StickMath;
import com.limelight.nvstream.input.ControllerPacket;

import java.util.Arrays;

/**
 * Draws the modern controls in Nightjar's glass. At rest a control is dark glass with a soft light from above, a thin
 * bright rim and a soft shadow, so it reads over bright and dark games alike; its glyphs are crisp and, where the chosen
 * controller family colours them, in that family's colours. A press fills it with the app's amber, like every selected
 * thing in Nightjar, sinks it in a little and lights a halo around it that reaches past a thumb ({@link #drawHalo});
 * let go, it eases back within {@link #RELEASE_MS}. The stream view, the layout editor and its Add tiles share it, so
 * the editor shows exactly what plays.
 *
 * <p>A control draws only inside its own box, since its view's layer and the editor's cut there; the halo and a stick's
 * knob, which reach past it, draw in layers of their own. It is all fills, strokes and shaders built once and placed by
 * moving the canvas, so a frame allocates nothing and hardware acceleration draws it the same on every Android.
 */
final class PadPainter {
    /** The parts a press lights on their own: a face cluster's buttons, a D-pad's arms or the WASD keys; one elsewhere. */
    static final int PARTS = 4;
    /** How long a part takes to ease back once let go, the touchpad finger's trail to fade and the press ring to run. */
    static final long RELEASE_MS = 120, TRAIL_MS = 220, RIPPLE_MS = 240;
    /** How many of a touchpad finger's last places its trail keeps. */
    static final int TRAIL = 12;

    /** What one control shows besides its shape. */
    static final class State {
        /**
         * Gamepad flags held by this control (face buttons, D-pad directions, a button or a stick click); for the
         * WASD stick its held keys as {@link LayoutStick} direction bits.
         */
        int flags;
        /** A trigger or button held. */
        boolean pressed;
        /** A stick's position (-1..1, up positive) and its drawn center inside the box, in pixels; NaN centers the base. */
        float stickX, stickY, centerX = Float.NaN, centerY = Float.NaN;
        boolean stickActive;
        /** A trackpad's finger inside the box, in pixels; NaN while none touches it. */
        float touchX = Float.NaN, touchY = Float.NaN;
        /** The pointer touchpad's short mode word ({@link #pointerLabel}); empty for other controls. */
        String label = "";
        /**
         * How far each part is pressed in, 0 to 1: a face cluster's buttons in {@link PadInput#FACE_CENTERS} order, a
         * D-pad's arms and the WASD keys up, down, left, right, and part 0 for every other control. It is 1 the moment
         * a part is held; {@link PadControlView} eases it back to 0 once let go ({@link #easeBack}).
         */
        final float[] press = new float[PARTS];
        /** When each part last went down, for the ring that runs out from it ({@link #drawHalo}); 0 for not lately. */
        final long[] pressedAt = new long[PARTS];
        /** A touchpad finger's last places and when it was there, the oldest at {@link #trailStart}, for its trail. */
        final float[] trailX = new float[TRAIL], trailY = new float[TRAIL];
        final long[] trailAt = new long[TRAIL];
        int trailStart, trailCount;
        /** The time this frame draws at, on the clock of {@link #trailAt} and {@link #pressedAt}. */
        long now;

        void clear() {
            flags = 0; pressed = false; stickX = 0; stickY = 0; centerX = Float.NaN; centerY = Float.NaN; stickActive = false;
            touchX = Float.NaN; touchY = Float.NaN;
            Arrays.fill(press, 0);
            Arrays.fill(pressedAt, 0);
            trailStart = trailCount = 0;
        }

        /** The finger was at ({@code x}, {@code y}) at {@code at}: the trail grows, its oldest place giving way. */
        void trail(float x, float y, long at) {
            int index;
            if (trailCount < TRAIL) {
                index = (trailStart + trailCount) % TRAIL;
                trailCount++;
            } else {
                index = trailStart;
                trailStart = (trailStart + 1) % TRAIL;
            }
            trailX[index] = x;
            trailY[index] = y;
            trailAt[index] = at;
        }

        /** Whether some of the trail still shows at {@code now}. */
        boolean trailShows(long now) {
            return trailCount > 1 && now - trailAt[(trailStart + trailCount - 1) % TRAIL] < TRAIL_MS;
        }
    }

    static final int GLASS = 0x520A0D12, GLASS_HELD = 0x7A0A0D12, RIM = 0x8CFFFFFF, RIM_PRESSED = 0x40FFFFFF,
            LABEL = 0xF2FFFFFF, SHADOW = 0x38000000, SHADOW_SOFT = 0x1A000000;
    /**
     * In dp: the stroke the stick geometry is measured with, how far a shape that fills its box stays inside it (its
     * shadow's room), the rim, how far a press sinks a shape, and the shadow's drop and reach.
     */
    private static final float STROKE_DP = 1.5f;
    static final float EDGE_DP = 3, RIM_DP = 1.25f, PRESS_IN_DP = 2, SHADOW_DROP_DP = 1, SHADOW_REACH_DP = 2;
    /** A Steam trackpad's and the pointer touchpad's corner radius, as a share of their shorter side. */
    static final float PAD_CORNER = .2f, TOUCHPAD_CORNER = .18f;
    /** How far outside a control's box the editor draws its outline and, at the outline's corners, its handles. */
    static final float OUTLINE_DP = 3;
    /** The radius of the editor's round corner handles. */
    static final float HANDLE_DP = 6;
    /** The stick base's well: how far below the middle its shade is centred, as a share of the radius. */
    private static final float WELL_LIFT = .16f;
    /** The held knob's shadow: a disc this much larger than the knob, dropped this share of its radius. */
    private static final float SHADOW_SCALE = 1.2f, SHADOW_DROP = .12f;

    final int accent, onAccent;
    final float density;
    private final float edgeStroke;
    final Paint fill = new Paint(Paint.ANTI_ALIAS_FLAG), stroke = new Paint(Paint.ANTI_ALIAS_FLAG),
            text = new Paint(Paint.ANTI_ALIAS_FLAG), ring = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint dashed = new Paint(Paint.ANTI_ALIAS_FLAG);
    final RectF rect = new RectF(), inner = new RectF(), chip = new RectF();
    // Unit shaders (radius 1, or the box 0..1): the canvas is moved and scaled onto each shape instead of the shader,
    // so a frame neither allocates nor rebuilds a native shader.
    private final Paint knobShadow = shaderPaint(new RadialGradient(0, 0, 1,
            new int[] {0x66000000, 0x4D000000, 0x00000000}, new float[] {0, .8f, 1}, Shader.TileMode.CLAMP));
    /** Round under a disc: clear in the middle, darkest just inside the edge, gone a little past it. */
    private final Paint discShadow = shaderPaint(new RadialGradient(0, 0, 1,
            new int[] {0, 0, 0x47000000, 0}, new float[] {0, .7f, .9f, 1}, Shader.TileMode.CLAMP));
    /** The light from above on glass, over the box 0..1. */
    final Paint highlight = shaderPaint(new LinearGradient(0, 0, 0, 1,
            new int[] {0x4DFFFFFF, 0x14FFFFFF, 0x00FFFFFF}, new float[] {0, .46f, .72f}, Shader.TileMode.CLAMP));
    /** The stick base's well: shade under its upper wall. */
    private final Paint well = shaderPaint(new RadialGradient(0, 0, 1,
            new int[] {0, 0, 0x45000000}, new float[] {0, .78f, 1}, Shader.TileMode.CLAMP));
    private final Paint knobGloss = shaderPaint(new RadialGradient(-.35f, -.45f, 1.45f,
            new int[] {0xFFFFFFFF, 0xFFE9EBEF, 0xFFAFB4BE}, new float[] {0, .45f, 1}, Shader.TileMode.CLAMP));
    private final Paint knobGlossPressed;
    /** The face letters and shoulder names; the layout editor changes it and every view redraws. */
    private PadLayout.Glyph glyph = PadLayout.Glyph.XBOX;
    /** The face cluster's buttons stand in the Nintendo arrangement ({@link PadLayout#facesSwapped}). */
    private boolean facesSwapped;
    /** The stick geometry for the last size drawn, recomputed only when a stick's size changes. */
    private float stickSize = Float.NaN, stickBase, stickKnob;
    private final float[] knobAt = new float[2];
    /** The pointer touchpad's mode words in its chip ({@link #pointerLabel}). */
    private final String mouseWord, cameraWord;
    private final PadPainterHalo halo;
    private final PadPainterControls controls;

    PadPainter(Context context) {
        density = context.getResources().getDisplayMetrics().density;
        mouseWord = context.getString(R.string.pad_touchpad_chip_mouse);
        cameraWord = context.getString(R.string.pad_touchpad_chip_camera);
        edgeStroke = STROKE_DP * density;
        accent = ContextCompat.getColor(context, R.color.night_primary);
        onAccent = ContextCompat.getColor(context, R.color.night_on_primary);
        stroke.setStyle(Paint.Style.STROKE);
        stroke.setStrokeWidth(edgeStroke);
        stroke.setStrokeCap(Paint.Cap.ROUND);
        stroke.setStrokeJoin(Paint.Join.ROUND);
        text.setTextAlign(Paint.Align.CENTER);
        text.setTypeface(Typeface.create("sans-serif-medium", Typeface.NORMAL));
        dashed.setStyle(Paint.Style.STROKE);
        dashed.setStrokeWidth(2 * density);
        dashed.setPathEffect(new DashPathEffect(new float[] {6 * density, 4 * density}, 0));
        ring.setStyle(Paint.Style.STROKE);
        knobGlossPressed = shaderPaint(new RadialGradient(-.35f, -.45f, 1.45f,
                new int[] {ColorUtils.blendARGB(accent, 0xFFFFFFFF, .45f), accent, ColorUtils.blendARGB(accent, 0xFF000000, .2f)},
                new float[] {0, .5f, 1}, Shader.TileMode.CLAMP));
        halo = new PadPainterHalo(this);
        controls = new PadPainterControls(this);
    }

    static Paint shaderPaint(Shader shader) {
        Paint paint = new Paint(Paint.ANTI_ALIAS_FLAG);
        paint.setShader(shader);
        return paint;
    }

    int accent() {
        return accent;
    }

    /** The word the pointer touchpad shows for its mode, in play and in the editor. */
    String pointerLabel(boolean camera) {
        return camera ? cameraWord : mouseWord;
    }

    PadLayout.Glyph glyph() {
        return glyph;
    }

    void setGlyph(PadLayout.Glyph glyph) {
        this.glyph = glyph;
    }

    /** Draws and reads the face cluster with its buttons in the Nintendo arrangement, or the standard one. */
    void setFacesSwapped(boolean swapped) {
        facesSwapped = swapped;
    }

    boolean facesSwapped() {
        return facesSwapped;
    }

    /** The face button drawn at {@code place} of the cluster, in {@link PadInput#FACE_CENTERS} order. */
    int faceButtonAt(int place) {
        return PadInput.faceOrder(facesSwapped)[place];
    }

    /** The parts {@code state} holds now, as bits in {@link State#press} order, with the standard face buttons. */
    static int heldParts(PadLayout.Control control, State state) {
        return heldParts(control, state, false);
    }

    /** The parts {@code state} holds now; a face cluster's places hold its buttons in the Nintendo order if swapped. */
    static int heldParts(PadLayout.Control control, State state, boolean facesSwapped) {
        switch (control.shape) {
            case FACE: {
                int[] order = PadInput.faceOrder(facesSwapped);
                int bits = 0;
                for (int i = 0; i < PARTS; i++) {
                    if ((state.flags & PadInput.FACE_FLAGS[order[i]]) != 0) bits |= 1 << i;
                }
                return bits;
            }
            case DPAD:
                return directions(state.flags, ControllerPacket.UP_FLAG, ControllerPacket.DOWN_FLAG,
                        ControllerPacket.LEFT_FLAG, ControllerPacket.RIGHT_FLAG);
            case STICK:
                if (control == PadLayout.Control.WASD) {
                    return directions(state.flags, LayoutStick.UP, LayoutStick.DOWN, LayoutStick.LEFT, LayoutStick.RIGHT);
                }
                return (state.flags & PadInput.buttonFlag(control)) != 0 ? 1 : 0;
            default:
                return state.pressed ? 1 : 0;
        }
    }

    private static int directions(int flags, int up, int down, int left, int right) {
        return ((flags & up) != 0 ? 1 : 0) | ((flags & down) != 0 ? 2 : 0) | ((flags & left) != 0 ? 4 : 0)
                | ((flags & right) != 0 ? 8 : 0);
    }

    /** How far a part let go {@code sinceMs} ago is still pressed in: from 1 back to 0 over {@link #RELEASE_MS}, fast at first. */
    static float easeBack(long sinceMs) {
        if (sinceMs <= 0) return 1;
        if (sinceMs >= RELEASE_MS) return 0;
        float rest = 1 - sinceMs / (float) RELEASE_MS;
        return rest * rest;
    }

    /** Draws {@code control} filling the box ({@code left}, {@code top}, {@code width}, {@code height}). */
    void draw(Canvas canvas, PadLayout.Control control, float left, float top, float width, float height, State state) {
        switch (control.shape) {
            case STICK: drawStick(canvas, control, left, top, width, height, state); break;
            case DPAD: controls.drawDpad(canvas, left, top, width, height, state); break;
            case FACE: controls.drawFace(canvas, left, top, width, height, state); break;
            case PILL: controls.drawPill(canvas, control, left, top, width, height, state); break;
            case PAD: controls.drawPad(canvas, left, top, width, height, state, PAD_CORNER,
                    control == PadLayout.Control.LEFT_PAD ? "L" : "R", null); break;
            case TOUCHPAD: controls.drawPad(canvas, left, top, width, height, state, TOUCHPAD_CORNER, null, state.label); break;
            case FACE_BUTTON: controls.drawFaceButton(canvas, control, left, top, width, height, state); break;
            case DPAD_ARM: controls.drawDpadArm(canvas, control, left, top, width, height, state); break;
            default: controls.drawRound(canvas, control, left, top, width, height, state); break;
        }
    }

    /** The editor's outline around a control: dashed amber with round corner handles when selected, a faint line otherwise. */
    void drawOutline(Canvas canvas, float left, float top, float width, float height, boolean selected) {
        float pad = OUTLINE_DP * density, radius = 12 * density;
        rect.set(left - pad, top - pad, left + width + pad, top + height + pad);
        if (selected) {
            dashed.setColor(accent);
            canvas.drawRoundRect(rect, radius, radius, dashed);
            // Corner handles: dragging one sizes the control's width and height (PadEditor).
            float h = HANDLE_DP * density;
            fill.setColor(0xFFFFFFFF);
            stroke.setColor(accent);
            stroke.setStrokeWidth(2 * density);
            for (int corner = 0; corner < 4; corner++) {
                float x = (corner & 1) != 0 ? rect.right : rect.left, y = (corner & 2) != 0 ? rect.bottom : rect.top;
                canvas.drawCircle(x, y, h, fill);
                canvas.drawCircle(x, y, h, stroke);
            }
        } else {
            stroke.setColor(0x38FFFFFF);
            stroke.setStrokeWidth(density);
            canvas.drawRoundRect(rect, radius, radius, stroke);
        }
    }

    /**
     * The press cue past a control's edge, drawn under the controls by {@link PadControlView.HaloLayer}: an amber halo
     * around every held part that fades as it eases back, and a ring that runs out from it the moment it went down, so a
     * thumb covering a small button still shows it answered. Every part a player presses has one: face buttons, the
     * D-pad's arms, pills, round buttons, a stick's click, each held key of the WASD stick on its side of the ring and a
     * finger on a touchpad; pushing an analog stick shows in its arc instead. {@code alpha} is the control's own, and 0
     * draws nothing. Returns whether a ring is still running, so the layer draws the next frame.
     */
    boolean drawHalo(Canvas canvas, PadLayout.Control control, float left, float top, float width, float height,
                     State state, float alpha, long now) {
        return halo.drawHalo(canvas, control, left, top, width, height, state, alpha, now);
    }

    void oval(Canvas canvas, float cx, float cy, float rx, float ry, Paint paint) {
        if (rx == ry) {
            canvas.drawCircle(cx, cy, rx, paint);
        } else {
            rect.set(cx - rx, cy - ry, cx + rx, cy + ry);
            canvas.drawOval(rect, paint);
        }
    }

    /** Draws a unit shader's disc (radius 1 at the origin) stretched onto the oval at ({@code cx}, {@code cy}). */
    static void unit(Canvas canvas, Paint paint, float cx, float cy, float rx, float ry) {
        if (rx <= 0 || ry <= 0) return;
        int saved = canvas.save();
        canvas.translate(cx, cy);
        canvas.scale(rx, ry);
        canvas.drawCircle(0, 0, 1, paint);
        canvas.restoreToCount(saved);
    }

    /** The light from above on an oval. */
    private void highlightOval(Canvas canvas, float cx, float cy, float rx, float ry) {
        if (rx <= 0 || ry <= 0) return;
        int saved = canvas.save();
        canvas.translate(cx - rx, cy - ry);
        canvas.scale(2 * rx, 2 * ry);
        canvas.drawOval(0, 0, 1, 1, highlight);
        canvas.restoreToCount(saved);
    }

    /** The light from above on a rounded rectangle; the box is scaled to 0..1, so the corners stay round. */
    void highlightRound(Canvas canvas, RectF shape, float radius) {
        float w = shape.width(), h = shape.height();
        if (w <= 0 || h <= 0) return;
        int saved = canvas.save();
        canvas.translate(shape.left, shape.top);
        canvas.scale(w, h);
        canvas.drawRoundRect(0, 0, 1, 1, radius / w, radius / h, highlight);
        canvas.restoreToCount(saved);
    }

    /** A soft shadow along a rounded rectangle's edge, dropped a little; {@code reach} past the edge at most. */
    void shadowRound(Canvas canvas, RectF shape, float radius, float reach, float strength) {
        if (reach <= 0 || strength <= 0) return;
        int saved = canvas.save();
        canvas.translate(0, Math.min(SHADOW_DROP_DP * density, reach / 2));
        ring.setStrokeWidth(2 * reach);
        ring.setColor(alpha(SHADOW_SOFT, strength));
        canvas.drawRoundRect(shape, radius, radius, ring);
        ring.setStrokeWidth(reach);
        ring.setColor(alpha(SHADOW, strength));
        canvas.drawRoundRect(shape, radius, radius, ring);
        canvas.restoreToCount(saved);
    }

    /**
     * A round glass part pressed in by {@code level}: its shadow, then the disc or oval sunk in by the press, filling
     * with amber, its light from above and its rim. The shadow keeps within {@code reach} of the edge.
     */
    void disc(Canvas canvas, float cx, float cy, float rx, float ry, float level, float reach) {
        if (level < 1 && reach > 0) {
            discShadow.setAlpha(Math.round(255 * (1 - level)));
            float drop = Math.min(SHADOW_DROP_DP * density, reach / 2);
            unit(canvas, discShadow, cx, cy + drop, rx + reach - drop, ry + reach - drop);
        }
        float sink = PRESS_IN_DP * density * level;
        rx -= sink;
        ry -= sink;
        fill.setColor(blend(GLASS, accent, level));
        oval(canvas, cx, cy, rx, ry, fill);
        highlightOval(canvas, cx, cy, rx, ry);
        float rim = RIM_DP * density;
        stroke.setStrokeWidth(rim);
        stroke.setColor(blend(RIM, RIM_PRESSED, level));
        oval(canvas, cx, cy, rx - rim / 2, ry - rim / 2, stroke);
    }

    /** A rounded glass part pressed in by {@code level}, as {@link #disc}; afterwards {@link #inner} holds its shape. */
    void slab(Canvas canvas, RectF shape, float radius, float level, float reach) {
        if (level < 1) shadowRound(canvas, shape, radius, reach, 1 - level);
        float sink = PRESS_IN_DP * density * level;
        inner.set(shape);
        inner.inset(sink, sink);
        float r = Math.max(0, radius - sink);
        fill.setColor(blend(GLASS, accent, level));
        canvas.drawRoundRect(inner, r, r, fill);
        highlightRound(canvas, inner, r);
        float rim = RIM_DP * density;
        stroke.setStrokeWidth(rim);
        stroke.setColor(blend(RIM, RIM_PRESSED, level));
        rect.set(inner);
        rect.inset(rim / 2, rim / 2);
        canvas.drawRoundRect(rect, Math.max(0, r - rim / 2), Math.max(0, r - rim / 2), stroke);
    }

    void label(Canvas canvas, String label, float cx, float cy, float size, int color) {
        text.setTextSize(size);
        text.setColor(color);
        canvas.drawText(label, cx, cy - (text.descent() + text.ascent()) / 2, text);
    }

    private void drawStick(Canvas canvas, PadLayout.Control control, float left, float top, float width, float height, State state) {
        drawStickBase(canvas, control, left, top, width, height, state);
        drawStickKnob(canvas, control, left, top, width, height, state);
    }

    /**
     * A stick's base: a glass well, shaded under its upper wall and lit on its lower one, with its rim, a ring where the
     * knob rests and ticks at the four directions; while pushed, an amber arc on the rim points where, growing and
     * glowing with the push. The WASD stick shows its four keys instead of the ticks, a held one in amber.
     */
    void drawStickBase(Canvas canvas, PadLayout.Control control, float left, float top, float width, float height,
                       State state) {
        stickGeometry(Math.min(width, height));
        float base = stickBase;
        float cx = left + width / 2, cy = top + height / 2;
        fill.setColor(state.stickActive ? GLASS_HELD : GLASS);
        canvas.drawCircle(cx, cy, base, fill);
        // The shade's centre sits below the middle, so the upper wall is darkest.
        int saved = canvas.save();
        canvas.translate(cx, cy + base * WELL_LIFT);
        canvas.scale(base, base);
        canvas.drawCircle(0, -WELL_LIFT, 1, well);
        canvas.restoreToCount(saved);
        stroke.setStrokeWidth(density);
        stroke.setColor(0x29FFFFFF);
        float lit = base - 2.5f * density;
        rect.set(cx - lit, cy - lit, cx + lit, cy + lit);
        canvas.drawArc(rect, 30, 120, false, stroke);
        float rim = RIM_DP * density;
        stroke.setStrokeWidth(rim);
        stroke.setColor(RIM);
        canvas.drawCircle(cx, cy, base - rim / 2, stroke);
        stroke.setStrokeWidth(density);
        stroke.setColor(0x1FFFFFFF);
        canvas.drawCircle(cx, cy, base * .64f, stroke);
        if (control != PadLayout.Control.WASD) {
            stroke.setStrokeWidth(1.5f * density);
            stroke.setColor(0x52FFFFFF);
            for (int i = 0; i < 4; i++) {
                int dx = i == 0 ? 1 : i == 2 ? -1 : 0, dy = i == 1 ? 1 : i == 3 ? -1 : 0;
                canvas.drawLine(cx + dx * base * .80f, cy + dy * base * .80f, cx + dx * base * .89f, cy + dy * base * .89f, stroke);
            }
        }
        float magnitude = (float) Math.min(1, Math.hypot(state.stickX, state.stickY));
        if (magnitude > 0) {
            float angle = (float) Math.toDegrees(Math.atan2(-state.stickY, state.stickX));
            float sweep = 30 + 60 * magnitude;
            float r = base - 3.5f * density;
            rect.set(cx - r, cy - r, cx + r, cy + r);
            stroke.setColor(alpha(accent, .3f * magnitude));
            stroke.setStrokeWidth((5 + 3 * magnitude) * density);
            canvas.drawArc(rect, angle - sweep / 2, sweep, false, stroke);
            stroke.setColor(accent);
            stroke.setStrokeWidth((2.5f + 2 * magnitude) * density);
            canvas.drawArc(rect, angle - sweep / 2, sweep, false, stroke);
        }
        if (control == PadLayout.Control.WASD) {
            float reach = base * .7f, size = base * .26f;
            wasdLabel(canvas, "W", cx, cy - reach, size, state.press[0]);
            wasdLabel(canvas, "S", cx, cy + reach, size, state.press[1]);
            wasdLabel(canvas, "A", cx - reach, cy, size, state.press[2]);
            wasdLabel(canvas, "D", cx + reach, cy, size, state.press[3]);
        }
    }

    private void wasdLabel(Canvas canvas, String key, float cx, float cy, float size, float level) {
        label(canvas, key, cx, cy, size, blend(LABEL, accent, level));
    }

    /**
     * A stick's knob for the control in the box ({@code left}, {@code top}, {@code width},
     * {@code height}): glossy, lit from the upper left. Its centre travels to the base's rim at full deflection, so the
     * canvas must reach {@link #knobOverhang} past the box on every side; while held it lifts on a soft shadow.
     */
    void drawStickKnob(Canvas canvas, PadLayout.Control control, float left, float top, float width, float height, State state) {
        stickGeometry(Math.min(width, height));
        float base = stickBase, knob = stickKnob;
        // The base stays put; a floating stick's knob starts where the thumb landed.
        float cx = left + width / 2, cy = top + height / 2;
        float originX = cx, originY = cy;
        if (state.stickActive && !Float.isNaN(state.centerX)) {
            originX = left + state.centerX;
            originY = top + state.centerY;
        }
        StickMath.knob(cx, cy, originX, originY, state.stickX, state.stickY, base, knobAt);
        float kx = knobAt[0], ky = knobAt[1];
        // Light from above: the shadow falls a little below the knob and fades out past its edge; resting, it is faint.
        float shadow = knob * SHADOW_SCALE;
        knobShadow.setAlpha(state.stickActive ? 255 : 128);
        int saved = canvas.save();
        canvas.translate(kx, ky + knob * SHADOW_DROP);
        canvas.scale(shadow, shadow);
        canvas.drawCircle(0, 0, 1, knobShadow);
        canvas.restoreToCount(saved);
        boolean clicked = (state.flags & PadInput.buttonFlag(control)) != 0;
        // Held, the knob is solid: it sits on top of the rim it rides over, not behind glass.
        Paint gloss = clicked ? knobGlossPressed : knobGloss;
        gloss.setAlpha(clicked || state.stickActive ? 255 : 0xA6);
        unit(canvas, gloss, kx, ky, knob, knob);
        stroke.setStrokeWidth(density);
        stroke.setColor(0x33000000);
        canvas.drawCircle(kx, ky, knob - density / 2, stroke);
        if (clicked) {
            label(canvas, control == PadLayout.Control.LEFT_STICK ? "L3" : "R3", kx, ky, knob * .7f, onAccent);
        } else {
            // A small ring in the knob, like a thumb grip.
            stroke.setStrokeWidth(1.5f * density);
            stroke.setColor(state.stickActive ? 0x2E000000 : 0x40000000);
            canvas.drawCircle(kx, ky, knob * .55f, stroke);
        }
    }

    float stickBaseFor(float size) {
        stickGeometry(size);
        return stickBase;
    }

    private void stickGeometry(float size) {
        if (size == stickSize) return;
        stickSize = size;
        stickBase = StickMath.baseRadius(size, edgeStroke);
        stickKnob = StickMath.knobRadius(size);
    }

    /** How far past each edge of a stick of this box its knob and the knob's shadow can draw. */
    float knobOverhang(float width, float height) {
        float size = Math.min(width, height), knob = StickMath.knobRadius(size);
        float shadowReach = knob * (SHADOW_SCALE - 1 + SHADOW_DROP);
        return StickMath.overhang(size, edgeStroke, shadowReach);
    }

    /** How far the finger travels for full deflection on a stick of this size; ModernController uses it as the stick radius. */
    static float travel(float size) {
        return StickMath.travel(size);
    }

    static String pillLabel(PadLayout.Control control) {
        switch (control) {
            case LEFT_GRIP: return "L GRIP";
            case RIGHT_GRIP: return "R GRIP";
            default: return control.name();
        }
    }

    static int blend(int from, int to, float t) {
        if (t <= 0) return from;
        if (t >= 1) return to;
        return ColorUtils.blendARGB(from, to, t);
    }

    static int alpha(int color, float share) {
        return ColorUtils.setAlphaComponent(color, Math.max(0, Math.min(255, Math.round(Color.alpha(color) * share))));
    }
}
