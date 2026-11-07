package com.limelight.ui;

import android.content.Context;
import android.util.AttributeSet;
import android.view.View;
import android.view.ViewGroup;

import com.limelight.R;

/**
 * A row of pills that starts a new line when the next one does not fit, instead of squeezing it. Each child keeps its
 * own width (a long German label stays one line), its start margin is the gap within a line and is dropped at a
 * line's start, and lines sit {@code @dimen/space_m} apart. Children wider than the row are measured to its width.
 */
public class WrapRow extends ViewGroup {
    private final int lineGap;

    public WrapRow(Context context) {
        this(context, null);
    }

    public WrapRow(Context context, AttributeSet attrs) {
        super(context, attrs);
        lineGap = context.getResources().getDimensionPixelSize(R.dimen.space_m);
    }

    @Override
    protected void onMeasure(int widthMeasureSpec, int heightMeasureSpec) {
        int mode = MeasureSpec.getMode(widthMeasureSpec);
        int avail = mode == MeasureSpec.UNSPECIFIED ? Integer.MAX_VALUE
                : MeasureSpec.getSize(widthMeasureSpec) - getPaddingLeft() - getPaddingRight();
        int x = 0;
        int lineHeight = 0;
        int height = 0;
        int widest = 0;
        boolean any = false;
        for (int i = 0; i < getChildCount(); i++) {
            View child = getChildAt(i);
            if (child.getVisibility() == GONE) continue;
            MarginLayoutParams lp = (MarginLayoutParams) child.getLayoutParams();
            int childWidthSpec = getChildMeasureSpec(
                    mode == MeasureSpec.UNSPECIFIED ? widthMeasureSpec : MeasureSpec.makeMeasureSpec(avail, MeasureSpec.AT_MOST),
                    0, lp.width);
            child.measure(childWidthSpec, getChildMeasureSpec(heightMeasureSpec, 0, lp.height));
            int w = child.getMeasuredWidth();
            int gap = lp.getMarginStart();
            if (any && x + gap + w > avail) {
                height += lineHeight + lineGap;
                x = 0;
                lineHeight = 0;
                gap = 0;
            } else if (!any) {
                gap = 0;
            }
            x += gap + w;
            widest = Math.max(widest, x);
            lineHeight = Math.max(lineHeight, child.getMeasuredHeight() + lp.topMargin + lp.bottomMargin);
            any = true;
        }
        height += lineHeight;
        setMeasuredDimension(resolveSize(widest + getPaddingLeft() + getPaddingRight(), widthMeasureSpec),
                resolveSize(height + getPaddingTop() + getPaddingBottom(), heightMeasureSpec));
    }

    @Override
    protected void onLayout(boolean changed, int l, int t, int r, int b) {
        boolean rtl = getLayoutDirection() == LAYOUT_DIRECTION_RTL;
        int avail = r - l - getPaddingLeft() - getPaddingRight();
        int x = 0;
        int y = getPaddingTop();
        int lineHeight = 0;
        boolean any = false;
        for (int i = 0; i < getChildCount(); i++) {
            View child = getChildAt(i);
            if (child.getVisibility() == GONE) continue;
            MarginLayoutParams lp = (MarginLayoutParams) child.getLayoutParams();
            int w = child.getMeasuredWidth();
            int gap = any ? lp.getMarginStart() : 0;
            if (any && x + gap + w > avail) {
                y += lineHeight + lineGap;
                x = 0;
                lineHeight = 0;
                gap = 0;
            }
            int start = x + gap;
            int left = rtl ? getWidth() - getPaddingRight() - start - w : getPaddingLeft() + start;
            int top = y + lp.topMargin;
            child.layout(left, top, left + w, top + child.getMeasuredHeight());
            x = start + w;
            lineHeight = Math.max(lineHeight, child.getMeasuredHeight() + lp.topMargin + lp.bottomMargin);
            any = true;
        }
    }

    @Override
    protected boolean checkLayoutParams(ViewGroup.LayoutParams p) {
        return p instanceof MarginLayoutParams;
    }

    @Override
    protected LayoutParams generateDefaultLayoutParams() {
        return new MarginLayoutParams(LayoutParams.WRAP_CONTENT, LayoutParams.WRAP_CONTENT);
    }

    @Override
    public LayoutParams generateLayoutParams(AttributeSet attrs) {
        return new MarginLayoutParams(getContext(), attrs);
    }

    @Override
    protected LayoutParams generateLayoutParams(ViewGroup.LayoutParams p) {
        return p instanceof MarginLayoutParams ? new MarginLayoutParams((MarginLayoutParams) p) : new MarginLayoutParams(p);
    }
}
