package com.limelight.ui;

import android.content.Context;
import android.graphics.Canvas;
import android.graphics.LinearGradient;
import android.graphics.Outline;
import android.graphics.Paint;
import android.graphics.RectF;
import android.graphics.Shader;
import android.util.AttributeSet;
import android.view.View;
import android.view.ViewOutlineProvider;
import android.widget.LinearLayout;

import androidx.core.content.res.ResourcesCompat;

import com.limelight.R;

import java.util.Collections;
import java.util.Set;
import java.util.WeakHashMap;
import java.util.function.BooleanSupplier;

/**
 * A game's cover: art clipped to a round rectangle with a soft shadow under it and a lit rim and sheen on it, all drawn
 * by this one view. Focus lifts it with a spring and rings it; a press pushes it in. While one cover of a {@link Group}
 * has focus the others step back. The art's slot keeps its size whatever is loading, so nothing jumps.
 */
public class CoverCard extends LinearLayout {
    /** Covers that step back together. */
    public static final class Group {
        private final Set<CoverCard> cards = Collections.newSetFromMap(new WeakHashMap<>());
        private CoverCard focused;

        private void set(CoverCard card) {
            CoverCard before = focused;
            focused = card;
            if ((before == null) != (card == null)) {
                for (CoverCard other : cards) other.applyDim();
            } else if (before != null) {
                before.applyDim();
                card.applyDim();
            }
        }

        /** Focus left this card and no other cover took it. */
        private void release(CoverCard card) {
            if (focused == card && !card.isFocused()) set(null);
        }

        /** No art to dim while nothing is focused. */
        boolean dims(CoverCard card) {
            return focused != null && focused != card;
        }
    }

    private View frame;
    private float restScale = 1f;
    private float stepBack;
    private final float radius;
    private final Paint rim = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint sheen = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint shadowPaint = new Paint(Paint.FILTER_BITMAP_FLAG);
    private float shadowBlur;
    private float shadowY;
    private final float dimAlpha;
    private final float focusScaleUsual;
    private float focusScale;
    private final float pressScale;
    private final FocusRing focusRing;
    private final RectF rect = new RectF();
    private final Runnable invalidator = this::invalidate;
    private Group group;
    private final Runnable releaser = () -> {
        if (group != null) group.release(this);
    };
    private BooleanSupplier instant = () -> false;
    private int shaderW;
    private int shaderH;
    private final boolean highContrast;

    public CoverCard(Context context) {
        this(context, null);
    }

    public CoverCard(Context context, AttributeSet attrs) {
        super(context, attrs);
        android.content.res.Resources res = context.getResources();
        setOrientation(VERTICAL);
        setWillNotDraw(false);
        setClipChildren(false);
        setClipToPadding(false);
        highContrast = AppTheme.isHighContrast(context);
        boolean television = GlassForm.of(context) == GlassForm.TELEVISION;
        radius = res.getDimension(R.dimen.glass_cover_radius);
        rim.setStyle(Paint.Style.STROKE);
        rim.setStrokeWidth(highContrast ? res.getDimension(R.dimen.nj_glass_chrome_solid_rim_width) : Math.max(1f, res.getDisplayMetrics().density * 0.75f));
        shadowPaint.setColor(ResourcesCompat.getColor(res, R.color.nj_cover_shadow, null));
        shadowBlur = highContrast ? 0f : res.getDimension(R.dimen.nj_cover_shadow_blur);
        shadowY = res.getDimension(R.dimen.nj_cover_shadow_y);
        dimAlpha = AppTheme.look(context) == AppTheme.Look.DAY
                ? ResourcesCompat.getFloat(res, R.dimen.nj_day_focus_neighbour_alpha)
                : ResourcesCompat.getFloat(res, R.dimen.nj_focus_neighbour_dim);
        boolean reduced = Motion.reduced(context);
        focusScaleUsual = reduced ? 1f : ResourcesCompat.getFloat(res, television ? R.dimen.nj_focus_scale_cover_tv : R.dimen.nj_focus_scale_cover);
        focusScale = focusScaleUsual;
        pressScale = reduced ? 1f : 0.97f;
        focusRing = new FocusRing(this, false);
    }

    /** The group that steps back together. */
    public void setGroup(Group group) {
        this.group = group;
        if (group != null) group.cards.add(this);
        applyDim();
    }

    /** While this is true (a held key), focus lands at once, with no spring. */
    public void setInstant(BooleanSupplier instant) {
        this.instant = instant;
    }

    /** A hero's cover is already large: a smaller lift. */
    public void setFocusScale(float scale) {
        focusScale = focusScaleUsual == 1f ? 1f : scale;
    }

    @Override
    protected void onFinishInflate() {
        super.onFinishInflate();
        frame = findViewById(R.id.console_cover_frame);
        if (frame == null) return;
        frame.setOutlineProvider(new ViewOutlineProvider() {
            @Override public void getOutline(View view, Outline outline) {
                outline.setRoundRect(0, 0, view.getWidth(), view.getHeight(), radius);
            }
        });
        frame.setClipToOutline(true);
    }

    @Override
    protected void onDetachedFromWindow() {
        super.onDetachedFromWindow();
        removeCallbacks(releaser);
        releaser.run();
    }

    private void applyDim() {
        if (frame != null) frame.setAlpha(group != null && group.dims(this) ? dimAlpha : 1f - stepBack * (1f - dimAlpha));
    }

    /**
     * A carousel's place for this cover: its scale at rest and how far it stepped back, 0 to 1. A press and focus go on top
     * of the scale; while pressed the spring keeps its own target and the next change takes this one up.
     */
    public void setRest(float scale, float back) {
        if (scale == restScale && back == stepBack) return;
        restScale = scale;
        stepBack = back;
        applyDim();
        if (isPressed()) return;
        Springs.finish(this);
        float at = scale * (isFocused() ? focusScale : 1f);
        setScaleX(at);
        setScaleY(at);
    }

    /** The hero's deeper shadow; the look's own is kept where the look has none (high contrast). */
    public void setShadow(float blur, float y) {
        if (highContrast) return;
        shadowBlur = blur;
        shadowY = y;
        invalidate();
    }

    @Override
    public boolean hasOverlappingRendering() {
        return false;
    }

    @Override
    protected void onFocusChanged(boolean gainFocus, int direction, android.graphics.Rect previously) {
        super.onFocusChanged(gainFocus, direction, previously);
        if (group != null) {
            // A move between covers loses focus here before the next one gains it: on the next turn, if at all.
            if (gainFocus) group.set(this);
            else if (group.focused == this) post(releaser);
        }
        retarget();
        invalidate();
    }

    @Override
    public void setPressed(boolean pressed) {
        boolean changed = pressed != isPressed();
        super.setPressed(pressed);
        if (changed) retarget();
    }

    @Override
    public void setHovered(boolean hovered) {
        boolean changed = hovered != isHovered();
        super.setHovered(hovered);
        if (changed) invalidate();
    }

    private void retarget() {
        float target = restScale * (isPressed() ? pressScale : isFocused() ? focusScale : 1f);
        if (instant.getAsBoolean()) {
            Springs.scale(this, target, Springs.Kind.FOCUS);
            Springs.finish(this);
            return;
        }
        Springs.scale(this, target, isPressed() ? Springs.Kind.PRESS : Springs.Kind.FOCUS);
    }

    /** A card coming back from the recycled pool starts at rest. */
    public void reset() {
        Springs.finish(this);
        setScaleX(restScale * (isFocused() ? focusScale : 1f));
        setScaleY(restScale * (isFocused() ? focusScale : 1f));
        applyDim();
    }

    @Override
    protected void dispatchDraw(Canvas canvas) {
        if (frame == null) {
            super.dispatchDraw(canvas);
            return;
        }
        float l = frame.getLeft();
        float t = frame.getTop();
        float r = frame.getRight();
        float b = frame.getBottom();
        boolean focused = isFocused();
        if (focused) focusRing.drawHalo(canvas, l, t, r, b, radius);
        if (shadowBlur > 0f) {
            GlassShadow shadow = GlassShadow.get(radius, shadowBlur, invalidator);
            if (shadow != null) shadow.draw(canvas, l, t + shadowY, r, b + shadowY, shadowPaint);
        }
        super.dispatchDraw(canvas);
        drawFinish(canvas, l, t, r, b);
        if (focused) focusRing.drawRing(canvas, l, t, r, b, radius);
    }

    /** The sheen across the top and the rim, lit from above. */
    private void drawFinish(Canvas canvas, float l, float t, float r, float b) {
        int w = (int) (r - l);
        int h = (int) (b - t);
        if (w != shaderW || h != shaderH) {
            shaderW = w;
            shaderH = h;
            android.content.res.Resources res = getResources();
            int top = highContrast ? ResourcesCompat.getColor(res, R.color.nj_hc_color_rim, null)
                    : ResourcesCompat.getColor(res, R.color.nj_cover_rim_top, null);
            int bottom = highContrast ? top : ResourcesCompat.getColor(res, R.color.nj_cover_rim_bottom, null);
            rim.setShader(new LinearGradient(0, 0, w * 0.2f, h, new int[] {top, bottom, bottom}, new float[] {0f, 0.3f, 1f}, Shader.TileMode.CLAMP));
            int sheenTop = ResourcesCompat.getColor(res, R.color.nj_cover_sheen_top, null);
            sheen.setShader(new LinearGradient(0, 0, w * 0.26f, h * 0.26f, sheenTop, sheenTop & 0xFFFFFF, Shader.TileMode.CLAMP));
        }
        int save = canvas.save();
        canvas.translate(l, t);
        float dim = frame.getAlpha();
        int alpha = Math.round(255 * dim);
        rim.setAlpha(isHovered() ? 255 : alpha);
        rect.set(0, 0, w, h);
        if (!highContrast) {
            sheen.setAlpha(alpha);
            canvas.drawRoundRect(rect, radius, radius, sheen);
        }
        float inset = rim.getStrokeWidth() / 2f;
        rect.inset(inset, inset);
        canvas.drawRoundRect(rect, radius - inset, radius - inset, rim);
        canvas.restoreToCount(save);
    }
}
