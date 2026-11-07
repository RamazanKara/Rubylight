package com.limelight.console;

import android.content.Context;
import android.util.AttributeSet;

import androidx.annotation.Nullable;
import androidx.recyclerview.widget.RecyclerView;

/**
 * Home's list on a phone and a handheld. It runs under the glass top row; scrolled, it fades out there rather than sliding
 * whole under the PC capsule. Its foot has the page's own fade over the tab bar, so only the top edge fades.
 */
public class HomeList extends RecyclerView {
    public HomeList(Context context) {
        super(context);
    }

    public HomeList(Context context, @Nullable AttributeSet attrs) {
        super(context, attrs);
    }

    public HomeList(Context context, @Nullable AttributeSet attrs, int defStyle) {
        super(context, attrs, defStyle);
    }

    @Override
    protected float getBottomFadingEdgeStrength() {
        return 0f;
    }
}
