package com.limelight.ui;

import android.animation.ObjectAnimator;
import android.animation.ValueAnimator;
import android.content.Context;
import android.content.res.TypedArray;
import android.graphics.Canvas;
import android.graphics.LinearGradient;
import android.graphics.Paint;
import android.graphics.RadialGradient;
import android.graphics.Shader;
import android.graphics.drawable.Drawable;
import android.graphics.drawable.GradientDrawable;
import android.util.AttributeSet;
import android.view.View;
import android.view.animation.LinearInterpolator;
import android.widget.FrameLayout;

import androidx.core.content.res.ResourcesCompat;

import com.limelight.R;

/**
 * The moon's orbit: rings around the middle and one dot going round them while something is under way, the only thing that
 * moves. With {@code orbitMoon} it draws the moon and the bird in the middle (first run); otherwise a child sits there (the
 * cover while connecting). The dot is one view turned about the middle, so a frame touches a transform and nothing else.
 */
public class OrbitView extends FrameLayout {
    private static final long TURN_MS = 5200;
    /** Where the dot rests when it does not move: just below the right. */
    private static final float REST_DEGREES = 96f;

    private static final float GLOW_REACH = 1.9f;

    private final boolean drawMoon;
    private final float ringsStart;
    private final Paint ring = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint moon = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint glow = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Drawable bird;
    private final View dot;
    private final int dotSize;
    private ObjectAnimator turning;
    private boolean moving;
    private boolean bare;

    public OrbitView(Context context) {
        this(context, null);
    }

    public OrbitView(Context context, AttributeSet attrs) {
        super(context, attrs);
        TypedArray a = context.obtainStyledAttributes(attrs, R.styleable.OrbitView);
        drawMoon = a.getBoolean(R.styleable.OrbitView_orbitMoon, false);
        ringsStart = a.getFloat(R.styleable.OrbitView_orbitRingsStart, 0.72f);
        a.recycle();
        float density = getResources().getDisplayMetrics().density;
        boolean day = AppTheme.look(context) == AppTheme.Look.DAY;
        ring.setStyle(Paint.Style.STROKE);
        ring.setStrokeWidth(Math.max(1f, density * 0.75f));
        ring.setColor(ResourcesCompat.getColor(getResources(), day ? R.color.nj_day_sky_ring : R.color.nj_sky_ring, null));
        bird = drawMoon ? ResourcesCompat.getDrawable(getResources(), R.drawable.ic_glass_bird, context.getTheme()) : null;
        if (bird != null) {
            bird.mutate().setTint(ResourcesCompat.getColor(getResources(), R.color.nj_color_on_moon, null));
        }
        dotSize = Math.round(density * 8);
        dot = new View(context);
        GradientDrawable shape = new GradientDrawable();
        shape.setShape(GradientDrawable.OVAL);
        shape.setColor(ResourcesCompat.getColor(getResources(), AppTheme.isHighContrast(context) ? R.color.nj_hc_color_moon : R.color.nj_color_moon, null));
        dot.setBackground(shape);
        dot.setImportantForAccessibility(IMPORTANT_FOR_ACCESSIBILITY_NO);
        addView(dot, new LayoutParams(dotSize, dotSize));
        setClipChildren(false);
        setWillNotDraw(false);
    }

    /** No rings and no dot: only what sits in the middle. */
    public void setBare(boolean bare) {
        this.bare = bare;
        dot.setVisibility(bare ? GONE : VISIBLE);
        invalidate();
    }

    /** Starts the dot, or stops it where it is. With reduced motion it rests. */
    public void setMoving(boolean moving) {
        this.moving = moving;
        updateMotion();
    }

    public boolean isTurning() {
        return moving && shouldRun();
    }

    private boolean shouldRun() {
        return moving && windowVisible && isAttachedToWindow() && isShown() && !Motion.reduced(getContext());
    }

    /** A stopped activity's window goes away with its views still "shown": nothing else would stop the turn. */
    @Override
    protected void onWindowVisibilityChanged(int visibility) {
        super.onWindowVisibilityChanged(visibility);
        windowVisible = visibility == VISIBLE;
        if (dot != null) updateMotion();
    }

    private boolean windowVisible = true;

    private float orbitRadius() {
        return Math.min(getWidth(), getHeight()) * 0.5f - dotSize;
    }

    @Override
    protected void onLayout(boolean changed, int left, int top, int right, int bottom) {
        super.onLayout(changed, left, top, right, bottom);
        dot.bringToFront();
        float r = orbitRadius();
        float cx = (right - left) / 2f;
        float cy = (bottom - top) / 2f;
        // Laid out at the orbit's top; the pivot is the middle of the orbit.
        dot.layout(Math.round(cx - dotSize / 2f), Math.round(cy - r - dotSize / 2f), Math.round(cx + dotSize / 2f), Math.round(cy - r + dotSize / 2f));
        dot.setPivotX(dotSize / 2f);
        dot.setPivotY(r + dotSize / 2f);
        if (turning == null) dot.setRotation(REST_DEGREES);
    }

    @Override
    protected void onSizeChanged(int w, int h, int oldw, int oldh) {
        super.onSizeChanged(w, h, oldw, oldh);
        if (!drawMoon || w == 0 || h == 0) return;
        float r = moonRadius();
        moon.setShader(new LinearGradient(0, h / 2f - r, 0, h / 2f + r,
                ResourcesCompat.getColor(getResources(), R.color.nj_color_moon_hi, null),
                ResourcesCompat.getColor(getResources(), R.color.nj_color_moon_lo, null), Shader.TileMode.CLAMP));
        glow.setShader(new RadialGradient(w / 2f, h / 2f, r * GLOW_REACH, new int[] {0x66F2A93B, 0x2EF2A93B, 0x00F2A93B},
                new float[] {0.6f, 0.76f, 1f}, Shader.TileMode.CLAMP));
    }

    private float moonRadius() {
        return Math.min(getWidth(), getHeight()) * 0.3f;
    }

    @Override
    protected void onDraw(Canvas canvas) {
        if (bare) return;
        float cx = getWidth() / 2f;
        float cy = getHeight() / 2f;
        float r = orbitRadius();
        canvas.drawCircle(cx, cy, r, ring);
        canvas.drawCircle(cx, cy, r * ringsStart, ring);
        if (drawMoon) {
            float m = moonRadius();
            canvas.drawCircle(cx, cy, m * GLOW_REACH, glow);
            canvas.drawCircle(cx, cy, m, moon);
            if (bird != null) {
                // On the launcher icon's grid the moon is half the drawable.
                int half = Math.round(m * 2f);
                bird.setBounds(Math.round(cx) - half, Math.round(cy) - half, Math.round(cx) + half, Math.round(cy) + half);
                bird.draw(canvas);
            }
        }
    }

    @Override
    protected void onAttachedToWindow() {
        super.onAttachedToWindow();
        updateMotion();
    }

    @Override
    protected void onDetachedFromWindow() {
        stopTurning();
        super.onDetachedFromWindow();
    }

    @Override
    protected void onVisibilityChanged(View changedView, int visibility) {
        super.onVisibilityChanged(changedView, visibility);
        // The constructor's addView can land here before the fields are set.
        if (dot != null) updateMotion();
    }

    private void updateMotion() {
        if (!shouldRun()) {
            stopTurning();
        } else if (turning == null && !"robolectric".equals(android.os.Build.FINGERPRINT)) {
            // An animator that never ends would keep a test's looper from ever idling.
            float from = dot.getRotation();
            turning = ObjectAnimator.ofFloat(dot, View.ROTATION, from, from + 360f);
            turning.setDuration(TURN_MS);
            turning.setInterpolator(new LinearInterpolator());
            turning.setRepeatCount(ValueAnimator.INFINITE);
            turning.start();
        }
    }

    private void stopTurning() {
        if (turning != null) {
            turning.cancel();
            turning = null;
        }
    }
}
