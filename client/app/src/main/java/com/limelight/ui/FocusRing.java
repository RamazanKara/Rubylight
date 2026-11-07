package com.limelight.ui;

import android.content.Context;
import android.graphics.Canvas;
import android.graphics.Paint;
import android.graphics.RectF;
import android.view.View;

import com.google.android.material.color.MaterialColors;
import com.limelight.R;

/**
 * The one focus look of the app's glass: a dark gap, a ring around it and a soft halo, drawn outside the view's bounds
 * so nothing moves when focus arrives. It shows on the next frame and is what a person sees from across a room.
 */
final class FocusRing {
    private final View view;
    private final Paint ring = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint gap = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint halo = new Paint(Paint.FILTER_BITMAP_FLAG);
    private final float ringWidth;
    private final float gapWidth;
    private final float haloBlur;
    private final RectF rect = new RectF();
    private final Runnable invalidator;

    FocusRing(View view, boolean onMoon) {
        this.view = view;
        Context context = view.getContext();
        boolean highContrast = AppTheme.isHighContrast(context);
        ringWidth = context.getResources().getDimension(highContrast ? R.dimen.nj_hc_focus_ring_width : R.dimen.glass_ring_width);
        gapWidth = context.getResources().getDimension(R.dimen.glass_ring_gap);
        haloBlur = highContrast ? 0f : context.getResources().getDimension(R.dimen.glass_halo_blur);
        ring.setStyle(Paint.Style.STROKE);
        ring.setStrokeWidth(ringWidth);
        ring.setColor(MaterialColors.getColor(context, onMoon ? R.attr.glassRingOnMoon : R.attr.glassRing, 0));
        gap.setStyle(Paint.Style.STROKE);
        gap.setStrokeWidth(gapWidth);
        gap.setColor(MaterialColors.getColor(context, R.attr.glassGap, 0));
        halo.setColor(MaterialColors.getColor(context, R.attr.glassHalo, 0));
        invalidator = view::invalidate;
    }

    /** Under the view's content. */
    void drawHalo(Canvas canvas, float left, float top, float right, float bottom, float radius) {
        if (haloBlur <= 0f) return;
        float grow = gapWidth + ringWidth;
        GlassShadow shadow = GlassShadow.get(radius + grow, haloBlur, invalidator);
        if (shadow != null) shadow.draw(canvas, left - grow, top - grow, right + grow, bottom + grow, halo);
    }

    /** Over the view's content, around the shape. */
    void drawRing(Canvas canvas, float left, float top, float right, float bottom, float radius) {
        float g = gapWidth / 2f;
        rect.set(left - g, top - g, right + g, bottom + g);
        canvas.drawRoundRect(rect, radius + g, radius + g, gap);
        float r = gapWidth + ringWidth / 2f;
        rect.set(left - r, top - r, right + r, bottom + r);
        canvas.drawRoundRect(rect, radius + r, radius + r, ring);
    }
}
