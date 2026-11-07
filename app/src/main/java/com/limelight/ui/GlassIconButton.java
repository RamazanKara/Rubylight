package com.limelight.ui;

import android.content.Context;
import android.graphics.Canvas;
import android.util.AttributeSet;

import androidx.appcompat.widget.AppCompatImageButton;

/** A bare icon inside a piece of glass, as the search's clear: no glass of its own, the same ring as every other control. */
public class GlassIconButton extends AppCompatImageButton {
    private final FocusRing focusRing;

    public GlassIconButton(Context context, AttributeSet attrs) {
        super(context, attrs);
        focusRing = new FocusRing(this, false);
        if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.O) setDefaultFocusHighlightEnabled(false);
    }

    @Override
    public void draw(Canvas canvas) {
        float w = getWidth();
        float h = getHeight();
        if (isFocused()) focusRing.drawHalo(canvas, 0, 0, w, h, h / 2f);
        super.draw(canvas);
        if (isFocused()) focusRing.drawRing(canvas, 0, 0, w, h, h / 2f);
    }
}
