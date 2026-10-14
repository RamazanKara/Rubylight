package com.limelight.ui;

import android.content.Context;
import android.content.res.TypedArray;
import android.util.AttributeSet;
import android.view.View;
import android.widget.FrameLayout;

import com.limelight.R;

/**
 * Keeps a screen's content in one readable column on tablets and in landscape. Children are measured
 * no wider than {@code android:maxWidth} (default {@code @dimen/content_max_width}) plus this frame's
 * own padding, and the column is centred. The frame itself stays full width, so its background and
 * the window's safe-area padding are unaffected.
 */
public class ContentFrame extends FrameLayout {
    private int maxContentWidth;

    public ContentFrame(Context context) {
        this(context, null);
    }

    public ContentFrame(Context context, AttributeSet attrs) {
        super(context, attrs);
        maxContentWidth = context.getResources().getDimensionPixelSize(R.dimen.content_max_width);
        if (attrs != null) {
            TypedArray a = context.obtainStyledAttributes(attrs, new int[] {android.R.attr.maxWidth});
            maxContentWidth = a.getDimensionPixelSize(0, maxContentWidth);
            a.recycle();
        }
    }

    /** A width of {@link Integer#MAX_VALUE} lets content span the whole window. */
    public void setMaxContentWidth(int width) {
        if (width != maxContentWidth) {
            maxContentWidth = width;
            requestLayout();
        }
    }

    public int getMaxContentWidth() {
        return maxContentWidth;
    }

    private int columnWidth() {
        long limit = (long) maxContentWidth + getPaddingLeft() + getPaddingRight();
        return (int) Math.min(Integer.MAX_VALUE, limit);
    }

    @Override
    protected void onMeasure(int widthMeasureSpec, int heightMeasureSpec) {
        int mode = MeasureSpec.getMode(widthMeasureSpec);
        int available = MeasureSpec.getSize(widthMeasureSpec);
        int column = columnWidth();
        if (mode == MeasureSpec.UNSPECIFIED || available <= column) {
            super.onMeasure(widthMeasureSpec, heightMeasureSpec);
            return;
        }
        super.onMeasure(MeasureSpec.makeMeasureSpec(column, MeasureSpec.EXACTLY), heightMeasureSpec);
        setMeasuredDimension(mode == MeasureSpec.EXACTLY ? available : column, getMeasuredHeight());
    }

    @Override
    protected void onLayout(boolean changed, int left, int top, int right, int bottom) {
        int column = columnWidth();
        int width = right - left;
        if (width <= column) {
            super.onLayout(changed, left, top, right, bottom);
            return;
        }
        super.onLayout(changed, left, top, left + column, bottom);
        int offset = (width - column) / 2;
        for (int i = 0; i < getChildCount(); i++) {
            View child = getChildAt(i);
            if (child.getVisibility() != GONE) child.offsetLeftAndRight(offset);
        }
    }
}
