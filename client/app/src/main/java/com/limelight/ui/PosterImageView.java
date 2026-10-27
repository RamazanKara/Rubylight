package com.limelight.ui;

import android.content.Context;
import android.graphics.Bitmap;
import android.graphics.ColorFilter;
import android.graphics.drawable.Drawable;
import android.util.AttributeSet;

import androidx.appcompat.widget.AppCompatImageView;

/**
 * A game's cover in a poster frame. Whatever drawable it is given decides how it is fitted (see {@link ArtFit}):
 * art close to 2:3 is cropped, other art is shown whole over gradient bands. A placeholder or no drawable crops.
 * The bands are made on the loader's thread and arrive with the cover through {@link #setArt}.
 */
public class PosterImageView extends AppCompatImageView {
    public PosterImageView(Context context) {
        super(context);
    }

    public PosterImageView(Context context, AttributeSet attrs) {
        super(context, attrs);
    }

    public PosterImageView(Context context, AttributeSet attrs, int defStyleAttr) {
        super(context, attrs, defStyleAttr);
    }

    private ArtFit.Bands pendingBackdrop;

    /** Shows a decoded cover with the bands that were built for it, which is null for a cover that is cropped. */
    public void setArt(Bitmap art, ArtFit.Bands backdrop) {
        pendingBackdrop = backdrop;
        try {
            setImageBitmap(art);
        } finally {
            pendingBackdrop = null;
        }
    }

    @Override
    public void setImageDrawable(Drawable drawable) {
        super.setImageDrawable(drawable);
        ArtFit.apply(this, drawable, pendingBackdrop);
        ArtFit.filterBackdrop(this, getColorFilter());
    }

    /** The filter reaches the letterbox bands too: a game not installed is grey from edge to edge. */
    @Override
    public void setColorFilter(ColorFilter filter) {
        super.setColorFilter(filter);
        ArtFit.filterBackdrop(this, filter);
    }
}
