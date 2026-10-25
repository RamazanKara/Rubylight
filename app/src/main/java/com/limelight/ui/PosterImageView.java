package com.limelight.ui;

import android.content.Context;
import android.graphics.drawable.Drawable;
import android.util.AttributeSet;

import androidx.appcompat.widget.AppCompatImageView;

/**
 * A game's cover in a poster frame. Whatever drawable it is given decides how it is fitted (see {@link ArtFit}):
 * portrait art is cropped, other art is shown whole over its own blur. A placeholder or no drawable crops.
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

    @Override
    public void setImageDrawable(Drawable drawable) {
        super.setImageDrawable(drawable);
        ArtFit.apply(this, drawable);
    }
}
