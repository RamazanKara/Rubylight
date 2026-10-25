package com.limelight.ui;

import android.graphics.Bitmap;
import android.graphics.LightingColorFilter;
import android.graphics.drawable.BitmapDrawable;
import android.graphics.drawable.ColorDrawable;
import android.graphics.drawable.Drawable;
import android.view.View;
import android.view.ViewParent;
import android.widget.ImageView;

import com.google.android.material.color.MaterialColors;

/**
 * How a game's cover fills its poster. A host sends whatever art the store has: Steam and Playnite send
 * portrait boxart, but many covers are landscape banners or squares. A portrait cover (height at least 1.25
 * times its width) is cropped to fill the poster; anything else is shown whole, centered, over a blurred,
 * darkened copy of itself, so a banner never loses its logo to a crop and the poster still has no empty bands.
 *
 * <p>The decision is made once, when the bitmap is set, from the bitmap's size. The blurred copy is a tiny
 * bitmap stretched by the bilinear filter, so there is no blur pass at run time.
 */
public final class ArtFit {
    /** The aspect (height over width) from which a cover is cropped rather than letterboxed. */
    public static final float PORTRAIT_MIN_ASPECT = 1.25f;
    /** Anything smaller than this on its longer side is a placeholder, not art. */
    private static final int MIN_ART_PX = 16;
    private static final int BLUR_WIDTH = 8;
    private static final int BLUR_HEIGHT = 12;
    /** The letterbox backdrop is multiplied by this, so the art over it keeps its contrast. */
    private static final int DARKEN = 0xFF8C8C8C;

    private ArtFit() { }

    /** True when a cover of this size is shown whole over its own blur instead of being cropped. */
    public static boolean letterboxes(int width, int height) {
        if (width <= 0 || height <= 0 || Math.max(width, height) < MIN_ART_PX) return false;
        return (float) height / width < PORTRAIT_MIN_ASPECT;
    }

    /** Sets the view's scale type, and its frame's backdrop, for the drawable it now shows. */
    static void apply(ImageView image, Drawable drawable) {
        int width = drawable == null ? 0 : drawable.getIntrinsicWidth();
        int height = drawable == null ? 0 : drawable.getIntrinsicHeight();
        boolean letterbox = letterboxes(width, height);
        image.setScaleType(letterbox ? ImageView.ScaleType.FIT_CENTER : ImageView.ScaleType.CENTER_CROP);
        ViewParent parent = image.getParent();
        if (!(parent instanceof View)) return;
        View frame = (View) parent;
        if (letterbox && drawable instanceof BitmapDrawable && ((BitmapDrawable) drawable).getBitmap() != null) {
            frame.setBackground(backdrop(image, ((BitmapDrawable) drawable).getBitmap()));
        } else if (frame.getBackground() instanceof Backdrop) {
            frame.setBackground(new ColorDrawable(MaterialColors.getColor(frame,
                    com.google.android.material.R.attr.colorSurfaceContainerHigh)));
        }
    }

    /** The frame's own surface again, for a view that is about to be bound to another cover. */
    public static void reset(ImageView image) {
        image.setScaleType(ImageView.ScaleType.CENTER_CROP);
        apply(image, null);
    }

    private static Drawable backdrop(ImageView image, Bitmap art) {
        Backdrop backdrop = new Backdrop(image.getResources(), blurred(art));
        backdrop.setColorFilter(new LightingColorFilter(DARKEN, 0));
        return backdrop;
    }

    /** A tiny copy of the art: halved until it is close to the target size so the average is not aliased. */
    static Bitmap blurred(Bitmap art) {
        Bitmap current = art;
        while (current.getWidth() > BLUR_WIDTH * 4 && current.getHeight() > BLUR_HEIGHT * 4) {
            Bitmap half = Bitmap.createScaledBitmap(current, current.getWidth() / 2, current.getHeight() / 2, true);
            if (current != art) current.recycle();
            current = half;
        }
        Bitmap tiny = Bitmap.createScaledBitmap(current, BLUR_WIDTH, BLUR_HEIGHT, true);
        if (current != art && current != tiny) current.recycle();
        return tiny;
    }

    /** Marks the frame backgrounds this class owns, so a rebind knows what to replace. */
    private static final class Backdrop extends BitmapDrawable {
        Backdrop(android.content.res.Resources resources, Bitmap bitmap) {
            super(resources, bitmap);
            setFilterBitmap(true);
            // Stretch the tiny copy over the whole poster: the blur has no aspect to keep.
            setGravity(android.view.Gravity.FILL);
        }
    }
}
