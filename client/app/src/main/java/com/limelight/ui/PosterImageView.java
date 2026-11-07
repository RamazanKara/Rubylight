package com.limelight.ui;

import android.content.Context;
import android.graphics.Bitmap;
import android.graphics.Canvas;
import android.graphics.PixelFormat;
import android.graphics.drawable.Drawable;
import android.util.AttributeSet;

import androidx.appcompat.widget.AppCompatImageView;

/**
 * A game's cover in its frame. Whatever shape the art has, it is cropped to fill the frame, centred (see {@link ArtFit}).
 */
public class PosterImageView extends AppCompatImageView {
    private int placeholder;

    public PosterImageView(Context context) {
        super(context);
    }

    public PosterImageView(Context context, AttributeSet attrs) {
        super(context, attrs);
    }

    public PosterImageView(Context context, AttributeSet attrs, int defStyleAttr) {
        super(context, attrs, defStyleAttr);
    }

    /** A colour for this view to fill with when no opaque art covers it: never painted under art that hides it. */
    public void setPlaceholder(int color) {
        placeholder = color;
        invalidate();
    }

    /** Whether the placeholder has to be painted under what is showing now. */
    boolean needsPlaceholder() {
        if (placeholder == 0) return false;
        Drawable art = getDrawable();
        return art == null || getScaleType() != ScaleType.CENTER_CROP && getScaleType() != ScaleType.MATRIX
                || art.getOpacity() != PixelFormat.OPAQUE;
    }

    @Override
    protected void onDraw(Canvas canvas) {
        if (needsPlaceholder()) canvas.drawColor(placeholder);
        super.onDraw(canvas);
    }

    /** Shows a decoded cover, cropped to fill the frame. */
    public void setArt(Bitmap art) {
        ArtFit.show(this, art);
    }
}
