package com.limelight.ui;

import android.content.Context;
import android.graphics.Bitmap;
import android.util.AttributeSet;

import androidx.appcompat.widget.AppCompatImageView;

/**
 * A game's cover in its frame. Whatever shape the art has, it is cropped to fill the frame, centred (see {@link ArtFit}).
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

    /** Shows a decoded cover, cropped to fill the frame. */
    public void setArt(Bitmap art) {
        ArtFit.show(this, art);
    }
}
