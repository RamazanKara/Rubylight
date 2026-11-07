package com.limelight.ui;

import android.content.Context;
import android.graphics.Canvas;
import android.graphics.Paint;
import android.graphics.RectF;
import android.util.AttributeSet;
import android.view.View;
import android.widget.LinearLayout;

import androidx.dynamicanimation.animation.FloatValueHolder;
import androidx.dynamicanimation.animation.SpringAnimation;
import androidx.dynamicanimation.animation.SpringForce;

import com.google.android.material.color.MaterialColors;
import com.limelight.R;

/**
 * The tabs and the pill that marks the current one. The pill's edges are two springs, the leading edge stiffer than the
 * trailing one, so it stretches toward where it is going and any new target retargets from where it is. It starts each
 * screen at the tab the last screen had selected, so moving between destinations reads as one bar.
 */
public class GlassTabs extends LinearLayout {
    /** The tab the last bar marked, as an index; screens replace each other but the pill carries on. */
    private static int lastSelected = -1;

    private final Paint pill = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint pillRim = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final float inset;
    private final RectF rect = new RectF();
    private float left = -1f;
    private float right;
    private SpringAnimation leftSpring;
    private SpringAnimation rightSpring;
    private View target;
    private FocusRing ring;

    public GlassTabs(Context context) {
        this(context, null);
    }

    public GlassTabs(Context context, AttributeSet attrs) {
        super(context, attrs);
        setWillNotDraw(false);
        inset = context.getResources().getDimension(R.dimen.glass_tab_inset);
        pill.setColor(MaterialColors.getColor(context, R.attr.glassSelectedFill, 0));
        pillRim.setStyle(Paint.Style.STROKE);
        pillRim.setStrokeWidth(Math.max(1f, context.getResources().getDisplayMetrics().density * 0.75f));
        pillRim.setColor(MaterialColors.getColor(context, R.attr.glassSelectedRim, 0));
    }

    /** For tests: forgets the last bar. */
    static void clearLast() {
        lastSelected = -1;
    }

    /** Springs the pill to the tab at once, ahead of the screen change the tap starts. */
    public void pillTo(View tab) {
        target = tab;
        moveTo(tab, true);
    }

    @Override
    protected void onLayout(boolean changed, int l, int t, int r, int b) {
        super.onLayout(changed, l, t, r, b);
        View selected = target != null ? target : selectedTab();
        if (selected == null) return;
        if (left < 0f) {
            View from = lastSelected >= 0 && lastSelected < getChildCount() ? getChildAt(lastSelected) : selected;
            if (from.getVisibility() != VISIBLE || from.getWidth() == 0) from = selected;
            left = from.getLeft() + inset;
            right = from.getRight() - inset;
            lastSelected = indexOfChild(selected);
            if (from != selected) moveTo(selected, true);
            return;
        }
        moveTo(selected, false);
    }

    private View selectedTab() {
        for (int i = 0; i < getChildCount(); i++) {
            View child = getChildAt(i);
            if (child.isSelected() && child.getVisibility() == VISIBLE) return child;
        }
        return null;
    }

    private void moveTo(View tab, boolean animate) {
        float toLeft = tab.getLeft() + inset;
        float toRight = tab.getRight() - inset;
        lastSelected = indexOfChild(tab);
        if (!animate || Motion.reduced(getContext())) {
            if (leftSpring != null) leftSpring.cancel();
            if (rightSpring != null) rightSpring.cancel();
            left = toLeft;
            right = toRight;
            invalidate();
            return;
        }
        boolean forward = toLeft > left;
        spring(true, toLeft, !forward);
        spring(false, toRight, forward);
    }

    private void spring(boolean leftEdge, float to, boolean leading) {
        SpringAnimation spring = leftEdge ? leftSpring : rightSpring;
        if (spring == null) {
            spring = new SpringAnimation(new FloatValueHolder(leftEdge ? left : right));
            spring.setSpring(new SpringForce(to));
            spring.addUpdateListener((animation, value, velocity) -> {
                if (animation == leftSpring) left = value;
                else right = value;
                invalidate();
            });
            if (leftEdge) leftSpring = spring;
            else rightSpring = spring;
        }
        SpringForce base = Springs.force(this, Springs.Kind.TAB_PILL);
        spring.getSpring().setDampingRatio(base.getDampingRatio())
                .setStiffness(leading ? base.getStiffness() * 1.6f : base.getStiffness());
        spring.animateToFinalPosition(to);
    }

    @Override
    protected void dispatchDraw(Canvas canvas) {
        if (left >= 0f && getChildCount() > 0) {
            float top = inset;
            float bottom = getHeight() - inset;
            float r = (bottom - top) / 2f;
            rect.set(left, top, right, bottom);
            canvas.drawRoundRect(rect, r, r, pill);
            float half = pillRim.getStrokeWidth() / 2f;
            rect.inset(half, half);
            canvas.drawRoundRect(rect, r - half, r - half, pillRim);
        }
        super.dispatchDraw(canvas);
        View focused = getFocusedChild();
        if (focused != null) {
            if (ring == null) ring = new FocusRing(this, false);
            float top = inset;
            float bottom = getHeight() - inset;
            ring.drawRing(canvas, focused.getLeft() + inset, top, focused.getRight() - inset, bottom, (bottom - top) / 2f);
        }
    }

    private final android.view.ViewTreeObserver.OnGlobalFocusChangeListener focusLeaves = (from, to) -> {
        if ((from != null && from.getParent() == this) || (to != null && to.getParent() == this)) invalidate();
    };

    @Override
    protected void onAttachedToWindow() {
        super.onAttachedToWindow();
        getViewTreeObserver().addOnGlobalFocusChangeListener(focusLeaves);
    }

    @Override
    protected void onDetachedFromWindow() {
        getViewTreeObserver().removeOnGlobalFocusChangeListener(focusLeaves);
        super.onDetachedFromWindow();
    }

    @Override
    public void requestChildFocus(View child, View focused) {
        super.requestChildFocus(child, focused);
        invalidate();
    }

    @Override
    public void clearChildFocus(View child) {
        super.clearChildFocus(child);
        invalidate();
    }
}
