package com.limelight.ui;

import android.graphics.Bitmap;
import android.graphics.Canvas;
import android.graphics.Paint;
import android.graphics.Rect;

/**
 * The TV backdrop of a game: its cover cropped to 16:9, shrunk to a 96x54 thumbnail, box-blurred, darkened, and faded
 * to the canvas colour at the top and foot. The screen draws the thumbnail upscaled with bilinear filtering, which is
 * the rest of the blur, so no RenderEffect or per-frame work is involved. About 20 KB per game.
 *
 * <p>The top fade is what keeps the screen's text readable: the bar and the focus line sit over it, and a white cover
 * still comes out dark enough there for on-surface and secondary text to keep 4.5:1 (BackdropBlurTest).
 */
public final class BackdropBlur {
    public static final int WIDTH = 96;
    public static final int HEIGHT = 54;

    /** The art's brightness; the screen is meant to be tinted by the game, not lit by it. */
    static final float DARKEN = 0.55f;
    /** A PC tile's cover, darker than Home's backdrop: the tile's 22sp name sits on it, and a white cover must still give 4.5:1. */
    static final float TILE_DARKEN = 0.35f;
    static final int RADIUS = 6;
    static final int PASSES = 3;

    /** Fully the canvas colour (to {@link #TOP_WEIGHT}) down to this fraction of the height, then fading out by {@link #TOP_END}. */
    static final float TOP_SOLID = 0.28f;
    static final float TOP_END = 0.62f;
    static final float TOP_WEIGHT = 0.9f;
    /** The foot fades up to {@link #BOTTOM_WEIGHT} of the canvas colour, starting here. */
    static final float BOTTOM_START = 0.75f;
    static final float BOTTOM_WEIGHT = 0.6f;

    private BackdropBlur() { }

    /**
     * @param cover the decoded cover, or null for a game with no art, which gets the plain canvas
     * @param canvas the screen's own colour (tv_canvas), also what a cover with transparency sits on
     * @return an opaque {@link #WIDTH} x {@link #HEIGHT} bitmap
     */
    public static Bitmap make(Bitmap cover, int canvas) {
        int[] pixels = blurred(cover, canvas);
        // A game with no art stays the plain canvas, which is already as dark as the screen gets.
        if (cover != null) shade(pixels, canvas);
        return opaque(pixels);
    }

    /**
     * A PC tile's background: the cover of the game it is running, blurred and darkened as {@link #make} does, with no
     * fade to the canvas, since the tile is a card of its own. Its text sits on the tile's own scrim.
     *
     * @param cover the decoded cover; not null
     * @param canvas what a cover with transparency sits on
     */
    public static Bitmap tile(Bitmap cover, int canvas) {
        int[] pixels = blurred(cover, canvas);
        for (int i = 0; i < pixels.length; i++) {
            int c = pixels[i];
            pixels[i] = 0xFF000000 | Math.round(((c >> 16) & 0xFF) * TILE_DARKEN) << 16
                    | Math.round(((c >> 8) & 0xFF) * TILE_DARKEN) << 8 | Math.round((c & 0xFF) * TILE_DARKEN);
        }
        return opaque(pixels);
    }

    private static int[] blurred(Bitmap cover, int canvas) {
        Bitmap small = Bitmap.createBitmap(WIDTH, HEIGHT, Bitmap.Config.ARGB_8888);
        small.eraseColor(canvas | 0xFF000000);
        if (cover != null && cover.getWidth() > 0 && cover.getHeight() > 0) draw(cover, small);

        int[] pixels = new int[WIDTH * HEIGHT];
        small.getPixels(pixels, 0, WIDTH, 0, 0, WIDTH, HEIGHT);
        if (cover != null) {
            int[] scratch = new int[pixels.length];
            for (int pass = 0; pass < PASSES; pass++) {
                box(pixels, scratch, true);
                box(scratch, pixels, false);
            }
        }
        return pixels;
    }

    private static Bitmap opaque(int[] pixels) {
        Bitmap result = Bitmap.createBitmap(pixels, WIDTH, HEIGHT, Bitmap.Config.ARGB_8888);
        result.setHasAlpha(false);
        return result;
    }

    /** The cover's middle 16:9 band, halved until it is near thumbnail size so the final bilinear step does not alias. */
    private static void draw(Bitmap cover, Bitmap into) {
        int width = cover.getWidth();
        int height = cover.getHeight();
        int cropHeight = Math.min(height, Math.max(1, Math.round(width * (float) HEIGHT / WIDTH)));
        int cropWidth = Math.min(width, Math.max(1, Math.round(cropHeight * (float) WIDTH / HEIGHT)));
        Rect source = new Rect((width - cropWidth) / 2, (height - cropHeight) / 2,
                (width - cropWidth) / 2 + cropWidth, (height - cropHeight) / 2 + cropHeight);
        Bitmap current = cover;
        // The artwork loader decodes straight to the GPU on newer Android; a software canvas cannot draw those.
        if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.O
                && cover.getConfig() == Bitmap.Config.HARDWARE) {
            current = cover.copy(Bitmap.Config.ARGB_8888, false);
            if (current == null) return;
        }
        Paint paint = new Paint(Paint.FILTER_BITMAP_FLAG);
        while (source.width() >= 4 * WIDTH) {
            Bitmap half = Bitmap.createBitmap(source.width() / 2, source.height() / 2, Bitmap.Config.ARGB_8888);
            new Canvas(half).drawBitmap(current, source, new Rect(0, 0, half.getWidth(), half.getHeight()), paint);
            current = half;
            source = new Rect(0, 0, half.getWidth(), half.getHeight());
        }
        new Canvas(into).drawBitmap(current, source, new Rect(0, 0, WIDTH, HEIGHT), paint);
    }

    /** One box blur pass over rows or columns, clamping at the edges. */
    private static void box(int[] src, int[] dst, boolean horizontal) {
        int length = horizontal ? WIDTH : HEIGHT;
        int lines = horizontal ? HEIGHT : WIDTH;
        int step = horizontal ? 1 : WIDTH;
        int lineStep = horizontal ? WIDTH : 1;
        int divisor = 2 * RADIUS + 1;
        for (int line = 0; line < lines; line++) {
            int base = line * lineStep;
            int r = 0, g = 0, b = 0;
            for (int i = -RADIUS; i <= RADIUS; i++) {
                int c = src[base + clamp(i, length) * step];
                r += (c >> 16) & 0xFF;
                g += (c >> 8) & 0xFF;
                b += c & 0xFF;
            }
            for (int i = 0; i < length; i++) {
                dst[base + i * step] = 0xFF000000 | (r / divisor) << 16 | (g / divisor) << 8 | b / divisor;
                int in = src[base + clamp(i + RADIUS + 1, length) * step];
                int out = src[base + clamp(i - RADIUS, length) * step];
                r += ((in >> 16) & 0xFF) - ((out >> 16) & 0xFF);
                g += ((in >> 8) & 0xFF) - ((out >> 8) & 0xFF);
                b += (in & 0xFF) - (out & 0xFF);
            }
        }
    }

    private static int clamp(int index, int length) {
        return index < 0 ? 0 : Math.min(index, length - 1);
    }

    private static void shade(int[] pixels, int canvas) {
        int cr = (canvas >> 16) & 0xFF;
        int cg = (canvas >> 8) & 0xFF;
        int cb = canvas & 0xFF;
        for (int y = 0; y < HEIGHT; y++) {
            float weight = canvasWeight((y + 0.5f) / HEIGHT);
            for (int x = 0; x < WIDTH; x++) {
                int c = pixels[y * WIDTH + x];
                int r = Math.round(mix(((c >> 16) & 0xFF) * DARKEN, cr, weight));
                int g = Math.round(mix(((c >> 8) & 0xFF) * DARKEN, cg, weight));
                int b = Math.round(mix((c & 0xFF) * DARKEN, cb, weight));
                pixels[y * WIDTH + x] = 0xFF000000 | r << 16 | g << 8 | b;
            }
        }
    }

    private static float mix(float from, float to, float weight) {
        return from + (to - from) * weight;
    }

    /** How much of the canvas colour covers the art at this fraction of the screen's height (0 at the top). */
    static float canvasWeight(float t) {
        float top = TOP_WEIGHT * (1f - smooth(TOP_SOLID, TOP_END, t));
        float foot = BOTTOM_WEIGHT * smooth(BOTTOM_START, 1f, t);
        return Math.min(1f, top + foot);
    }

    private static float smooth(float from, float to, float t) {
        float x = Math.max(0f, Math.min(1f, (t - from) / (to - from)));
        return x * x * (3f - 2f * x);
    }
}
