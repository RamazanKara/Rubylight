package com.limelight.ui;

import android.content.Context;
import android.content.res.TypedArray;
import android.graphics.Canvas;
import android.graphics.Paint;
import android.graphics.drawable.GradientDrawable;
import android.util.AttributeSet;

import androidx.core.content.res.ResourcesCompat;

import com.google.android.material.button.MaterialButton;
import com.google.android.material.color.MaterialColors;
import com.limelight.R;

/**
 * A capsule button of the glass screens: solid moon for Play (the one thing that is never glass), or the control tint.
 * Focus rings it with the same ring as a cover, ink on the moon; the ring and the glow lie outside its bounds.
 */
public class GlassButton extends MaterialButton {
    private final boolean moon;
    private final FocusRing focusRing;
    private final Paint glow = new Paint(Paint.FILTER_BITMAP_FLAG);
    private final float glowBlur;
    private final float focusScale;
    private final float pressScale;
    private final Runnable invalidator = this::invalidate;

    public GlassButton(Context context, AttributeSet attrs) {
        super(context, attrs);
        TypedArray a = context.obtainStyledAttributes(attrs, R.styleable.GlassButton);
        moon = a.getBoolean(R.styleable.GlassButton_glassMoon, false);
        a.recycle();
        android.content.res.Resources res = context.getResources();
        boolean highContrast = AppTheme.isHighContrast(context);
        GradientDrawable shape = new GradientDrawable();
        shape.setCornerRadius(res.getDimension(R.dimen.nj_radius_capsule));
        if (moon) {
            if (highContrast) {
                shape.setColor(ResourcesCompat.getColor(res, R.color.nj_hc_color_moon, null));
            } else {
                shape.setOrientation(GradientDrawable.Orientation.TOP_BOTTOM);
                shape.setColors(new int[] {ResourcesCompat.getColor(res, R.color.nj_color_moon_hi, null), 0xFFEE9F2E});
            }
            setTextColor(ResourcesCompat.getColor(res, highContrast ? R.color.nj_hc_color_on_moon : R.color.nj_color_on_moon, null));
            setIconTint(android.content.res.ColorStateList.valueOf(
                    ResourcesCompat.getColor(res, highContrast ? R.color.nj_hc_color_on_moon : R.color.nj_color_on_moon, null)));
        } else {
            shape.setColor(MaterialColors.getColor(context, R.attr.glassControlFill, 0));
            shape.setStroke(Math.max(1, Math.round(res.getDisplayMetrics().density * 0.75f)),
                    MaterialColors.getColor(context, highContrast ? com.google.android.material.R.attr.colorOnSurface : R.attr.glassControlRim, 0));
        }
        setBackgroundTintList(null);
        setBackground(shape);
        glow.setColor(0x66F2A93B);
        glowBlur = moon && !highContrast ? res.getDimension(R.dimen.glass_play_glow) : 0f;
        boolean reduced = Motion.reduced(context);
        focusScale = reduced ? 1f : ResourcesCompat.getFloat(res, R.dimen.nj_focus_scale_control);
        pressScale = reduced ? 1f : 0.97f;
        focusRing = new FocusRing(this, moon);
        // The ring is the focus: Android's own highlight would show as square corners behind the capsule.
        if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.O) setDefaultFocusHighlightEnabled(false);
    }

    @Override
    protected void onFocusChanged(boolean gainFocus, int direction, android.graphics.Rect previously) {
        super.onFocusChanged(gainFocus, direction, previously);
        retarget();
    }

    @Override
    public void setPressed(boolean pressed) {
        boolean changed = pressed != isPressed();
        super.setPressed(pressed);
        if (changed) retarget();
    }

    private void retarget() {
        Springs.scale(this, isPressed() ? pressScale : isFocused() ? focusScale : 1f,
                isPressed() ? Springs.Kind.PRESS : Springs.Kind.FOCUS);
    }

    @Override
    public void draw(Canvas canvas) {
        float w = getWidth();
        float h = getHeight();
        float r = h / 2f;
        if (glowBlur > 0f) {
            GlassShadow shadow = GlassShadow.get(r, glowBlur, invalidator);
            if (shadow != null) shadow.draw(canvas, 0, 0, w, h, glow);
        }
        if (isFocused()) focusRing.drawHalo(canvas, 0, 0, w, h, r);
        super.draw(canvas);
        if (isFocused()) focusRing.drawRing(canvas, 0, 0, w, h, r);
    }
}
