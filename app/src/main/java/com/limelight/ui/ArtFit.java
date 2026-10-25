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
 * portrait boxart close to 2:3, but many covers are 3:4, landscape banners or squares. A cover within
 * {@link #POSTER_TOLERANCE} of the poster's 2:3 is cropped to fill it; anything else is shown whole, centered,
 * over a blurred, darkened copy of itself, so a banner or a 3:4 box never loses its edges to a crop and the
 * poster still has no empty bands.
 *
 * <p>The decision is made when the drawable is set, from its size. The blurred copy is a tiny bitmap stretched by
 * the bilinear filter, so there is no blur pass at run time; it is built once on the loader's thread ({@link
 * #backdropFor}), kept with the decoded cover and only attached when the cover is bound.
 */
public final class ArtFit {
    /** A poster's aspect (height over width). */
    public static final float POSTER_ASPECT = 1.5f;
    /** How far from {@link #POSTER_ASPECT} a cover may be and still be cropped, as a fraction of it. */
    public static final float POSTER_TOLERANCE = 0.05f;
    /** Anything smaller than this on its longer side is a placeholder, not art. */
    private static final int MIN_ART_PX = 16;
    private static final int BLUR_WIDTH = 8;
    private static final int BLUR_HEIGHT = 12;
    /** The letterbox backdrop is multiplied by this, so the art over it keeps its contrast. */
    private static final int DARKEN = 0xFFB0B0B0;

    private ArtFit() { }

    /** True when a cover of this size is shown whole over its own blur instead of being cropped. */
    public static boolean letterboxes(int width, int height) {
        if (width <= 0 || height <= 0 || Math.max(width, height) < MIN_ART_PX) return false;
        float aspect = (float) height / width;
        return Math.abs(aspect - POSTER_ASPECT) > POSTER_ASPECT * POSTER_TOLERANCE;
    }

    /**
     * Sets the view's scale type, and its frame's backdrop, for the drawable it now shows. The backdrop is the
     * tiny blurred copy made by {@link #backdropFor}; without one a letterboxed cover sits on the frame's surface.
     */
    static void apply(ImageView image, Drawable drawable, Bitmap backdrop) {
        int width = drawable == null ? 0 : drawable.getIntrinsicWidth();
        int height = drawable == null ? 0 : drawable.getIntrinsicHeight();
        boolean letterbox = letterboxes(width, height);
        image.setScaleType(letterbox ? ImageView.ScaleType.FIT_CENTER : ImageView.ScaleType.CENTER_CROP);
        ViewParent parent = image.getParent();
        if (!(parent instanceof View)) return;
        View frame = (View) parent;
        if (letterbox && backdrop != null) {
            frame.setBackground(backdrop(image, backdrop));
        } else if (frame.getBackground() instanceof Backdrop) {
            frame.setBackground(new ColorDrawable(MaterialColors.getColor(frame,
                    com.google.android.material.R.attr.colorSurfaceContainerHigh)));
        }
    }

    /** The frame's own surface again, for a view that is about to be bound to another cover. */
    public static void reset(ImageView image) {
        image.setScaleType(ImageView.ScaleType.CENTER_CROP);
        apply(image, null, null);
    }

    private static Drawable backdrop(ImageView image, Bitmap tiny) {
        Backdrop backdrop = new Backdrop(image.getResources(), tiny);
        backdrop.setColorFilter(new LightingColorFilter(DARKEN, 0));
        return backdrop;
    }

    /**
     * The backdrop for a cover, or null when it is cropped to fill its poster. Runs on a loader thread, never on the
     * UI thread, and reads the pixels: the art must not be a hardware bitmap.
     */
    public static Bitmap backdropFor(Bitmap art) {
        if (art == null || !letterboxes(art.getWidth(), art.getHeight())) return null;
        // Bitmap.Config.HARDWARE does not exist before Android 8, where no bitmap is one.
        if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.O && art.getConfig() == Bitmap.Config.HARDWARE) return null;
        return blurred(art);
    }

    /** The centered 2:3 part of a cover of this size as x, y, width, height: the part a poster would show. */
    static int[] posterCrop(int width, int height) {
        int cropWidth = width;
        int cropHeight = height;
        if ((float) height / width > POSTER_ASPECT) cropHeight = Math.max(1, Math.round(width * POSTER_ASPECT));
        else cropWidth = Math.max(1, Math.round(height / POSTER_ASPECT));
        return new int[] {(width - cropWidth) / 2, (height - cropHeight) / 2, cropWidth, cropHeight};
    }

    /**
     * A tiny copy of the poster-shaped middle of the art: cropped to 2:3 first, so the blur has no band for a
     * banner's bars, then halved until both sides are within twice the target so the average is not aliased.
     */
    static Bitmap blurred(Bitmap art) {
        int[] crop = posterCrop(art.getWidth(), art.getHeight());
        Bitmap current = Bitmap.createBitmap(art, crop[0], crop[1], crop[2], crop[3]);
        while (current.getWidth() > BLUR_WIDTH * 2 || current.getHeight() > BLUR_HEIGHT * 2) {
            Bitmap half = Bitmap.createScaledBitmap(current, Math.max(BLUR_WIDTH, current.getWidth() / 2),
                    Math.max(BLUR_HEIGHT, current.getHeight() / 2), true);
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
