package com.limelight.ui;

import android.graphics.Bitmap;
import android.graphics.Canvas;
import android.graphics.Color;
import android.graphics.ColorFilter;
import android.graphics.LinearGradient;
import android.graphics.Paint;
import android.graphics.PixelFormat;
import android.graphics.Rect;
import android.graphics.Shader;
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
 * so a banner or a 3:4 box never loses its edges to a crop.
 *
 * <p>The bands above and below a letterboxed cover are a vertical gradient: next to the art it is the average
 * colour of the art's own top (or bottom) rows, darkened a little, and it fades into the card's surface towards the
 * poster's edge. That is two colours, read once on the loader's thread ({@link #backdropFor}), kept with the
 * decoded cover and only attached when the cover is bound. There is no blur, so no blobs, and the poster draws
 * nothing but one gradient.
 */
public final class ArtFit {
    /** A poster's aspect (height over width). */
    public static final float POSTER_ASPECT = 1.5f;
    /** How far from {@link #POSTER_ASPECT} a cover may be and still be cropped, as a fraction of it. */
    public static final float POSTER_TOLERANCE = 0.05f;
    /** Anything smaller than this on its longer side is a placeholder, not art. */
    private static final int MIN_ART_PX = 16;
    /** How many pixel rows at each edge of the art make up its edge colour. */
    static final int EDGE_ROWS = 4;
    /** The edge colours are multiplied by this, so the art next to them keeps its contrast. */
    private static final float DARKEN = 0.75f;

    private ArtFit() { }

    /**
     * What a letterboxed cover sits on: the darkened average colours of the art's top and bottom rows, and the
     * art's aspect (height over width), which decides how tall the bands are.
     */
    public static final class Bands {
        public final int top;
        public final int bottom;
        public final float aspect;

        Bands(int top, int bottom, float aspect) {
            this.top = top;
            this.bottom = bottom;
            this.aspect = aspect;
        }
    }

    /** True when a cover of this size is shown whole instead of being cropped. */
    public static boolean letterboxes(int width, int height) {
        if (width <= 0 || height <= 0 || Math.max(width, height) < MIN_ART_PX) return false;
        float aspect = (float) height / width;
        return Math.abs(aspect - POSTER_ASPECT) > POSTER_ASPECT * POSTER_TOLERANCE;
    }

    /**
     * Sets the view's scale type, and its frame's backdrop, for the drawable it now shows. The bands come from
     * {@link #backdropFor}; without them a letterboxed cover sits on the frame's surface.
     */
    static void apply(ImageView image, Drawable drawable, Bands bands) {
        int width = drawable == null ? 0 : drawable.getIntrinsicWidth();
        int height = drawable == null ? 0 : drawable.getIntrinsicHeight();
        boolean letterbox = letterboxes(width, height);
        image.setScaleType(letterbox ? ImageView.ScaleType.FIT_CENTER : ImageView.ScaleType.CENTER_CROP);
        ViewParent parent = image.getParent();
        if (!(parent instanceof View)) return;
        View frame = (View) parent;
        int surface = MaterialColors.getColor(frame, com.google.android.material.R.attr.colorSurfaceContainerHigh);
        if (letterbox && bands != null) {
            frame.setBackground(new Backdrop(bands, surface));
        } else if (frame.getBackground() instanceof Backdrop) {
            frame.setBackground(new ColorDrawable(surface));
        }
    }

    /** The frame's own surface again, for a view that is about to be bound to another cover. */
    public static void reset(ImageView image) {
        image.setScaleType(ImageView.ScaleType.CENTER_CROP);
        apply(image, null, null);
    }

    /**
     * The bands for a cover, or null when it is cropped to fill its poster. Runs on a loader thread, never on the
     * UI thread, and reads the pixels: the art must not be a hardware bitmap.
     */
    public static Bands backdropFor(Bitmap art) {
        if (art == null || !letterboxes(art.getWidth(), art.getHeight())) return null;
        // Bitmap.Config.HARDWARE does not exist before Android 8, where no bitmap is one.
        if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.O && art.getConfig() == Bitmap.Config.HARDWARE) return null;
        int rows = Math.min(EDGE_ROWS, art.getHeight());
        return new Bands(darkened(average(art, 0, rows)), darkened(average(art, art.getHeight() - rows, rows)),
                (float) art.getHeight() / art.getWidth());
    }

    /** The average colour of {@code rows} full rows of the art from row {@code firstRow}, alpha ignored. */
    static int average(Bitmap art, int firstRow, int rows) {
        int width = art.getWidth();
        int[] pixels = new int[width * rows];
        art.getPixels(pixels, 0, width, 0, firstRow, width, rows);
        long red = 0;
        long green = 0;
        long blue = 0;
        for (int pixel : pixels) {
            red += Color.red(pixel);
            green += Color.green(pixel);
            blue += Color.blue(pixel);
        }
        int count = pixels.length;
        return Color.rgb((int) (red / count), (int) (green / count), (int) (blue / count));
    }

    static int darkened(int color) {
        return Color.rgb(Math.round(Color.red(color) * DARKEN), Math.round(Color.green(color) * DARKEN),
                Math.round(Color.blue(color) * DARKEN));
    }

    /**
     * The poster's background: the surface at the poster's top edge, rising to the top colour where the art starts,
     * then the bottom colour where the art ends, falling back to the surface at the bottom edge. The part under
     * the art is never seen. Art taller than a poster has bars at the sides instead, which get the plain
     * top-to-bottom gradient.
     */
    static final class Backdrop extends Drawable {
        private final Bands bands;
        private final int surface;
        private final Paint paint = new Paint();

        Backdrop(Bands bands, int surface) {
            this.bands = bands;
            this.surface = surface;
        }

        Bands bands() {
            return bands;
        }

        int surface() {
            return surface;
        }

        /** Where the art starts, as a fraction of the poster's height; 0 when it fills the height. */
        float band(Rect bounds) {
            float artHeight = Math.min(bounds.height(), bounds.width() * bands.aspect);
            return (bounds.height() - artHeight) / 2f / bounds.height();
        }

        @Override
        protected void onBoundsChange(Rect bounds) {
            if (bounds.isEmpty()) {
                paint.setShader(null);
                return;
            }
            float band = band(bounds);
            Shader shader;
            if (band <= 0f) {
                shader = new LinearGradient(0, bounds.top, 0, bounds.bottom, bands.top, bands.bottom, Shader.TileMode.CLAMP);
            } else {
                shader = new LinearGradient(0, bounds.top, 0, bounds.bottom,
                        new int[] {surface, bands.top, bands.bottom, surface},
                        new float[] {0f, band, 1f - band, 1f}, Shader.TileMode.CLAMP);
            }
            paint.setShader(shader);
        }

        @Override
        public void draw(Canvas canvas) {
            canvas.drawRect(getBounds(), paint);
        }

        @Override
        public void setAlpha(int alpha) {
            paint.setAlpha(alpha);
        }

        @Override
        public void setColorFilter(ColorFilter filter) {
            paint.setColorFilter(filter);
        }

        @Override
        public int getOpacity() {
            return PixelFormat.OPAQUE;
        }
    }
}
