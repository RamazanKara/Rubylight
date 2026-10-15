package com.limelight.ui;

import android.content.Context;
import android.graphics.Canvas;
import android.graphics.drawable.Drawable;
import android.util.AttributeSet;
import android.view.View;

import com.limelight.R;

/**
 * The pointer drawn on this device in Local cursor touch mode. It moves by translation only, so a
 * move costs no layout pass. {@link #setCursor} swaps the image, for example for a host cursor shape.
 */
public class LocalCursorView extends View {
    private Drawable cursor;
    private float hotspotX, hotspotY;

    public LocalCursorView(Context context) {
        this(context, null);
    }

    public LocalCursorView(Context context, AttributeSet attrs) {
        super(context, attrs);
        setClickable(false);
        setFocusable(false);
        setImportantForAccessibility(IMPORTANT_FOR_ACCESSIBILITY_NO);
        // The arrow's tip sits 1dp inside the image so its outline is not clipped.
        float tip = getResources().getDisplayMetrics().density;
        setCursor(context.getDrawable(R.drawable.ic_local_cursor), tip, tip);
    }

    /** Draws cursor with its hotspot, in pixels from its top-left corner, at the pointer position. */
    public void setCursor(Drawable cursor, float hotspotX, float hotspotY) {
        this.cursor = cursor;
        this.hotspotX = hotspotX;
        this.hotspotY = hotspotY;
        if (cursor != null) cursor.setBounds(0, 0, cursor.getIntrinsicWidth(), cursor.getIntrinsicHeight());
        requestLayout();
        invalidate();
    }

    /** Puts the hotspot at (x, y) in the parent's coordinates. */
    public void moveTo(float x, float y) {
        setTranslationX(x - getLeft() - hotspotX);
        setTranslationY(y - getTop() - hotspotY);
    }

    @Override
    protected void onMeasure(int widthMeasureSpec, int heightMeasureSpec) {
        int width = cursor != null ? cursor.getIntrinsicWidth() : 0;
        int height = cursor != null ? cursor.getIntrinsicHeight() : 0;
        setMeasuredDimension(width, height);
    }

    @Override
    protected void onDraw(Canvas canvas) {
        if (cursor != null) cursor.draw(canvas);
    }
}
