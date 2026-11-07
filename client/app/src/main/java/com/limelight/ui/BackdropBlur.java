package com.limelight.ui;

import android.graphics.Bitmap;
import android.graphics.Canvas;
import android.graphics.Paint;
import android.graphics.Rect;

/**
 * A game's ambient: its whole cover squashed to a 96x54 thumbnail (54x96 in portrait), blurred, its colour lifted and
 * yellows turned toward orange so they never read as the Play button, then held to a luminance that keeps the screen's
 * text readable wherever it lands (under {@code ambient.textZoneMaxLuma} at night, over
 * {@code day.ambient.textZoneMinLuma} by day). The screen draws the thumbnail upscaled with bilinear filtering, which
 * is the rest of the blur. Baked once per cover on a worker thread, never per frame; about 20 KB per game. The fade to
 * the canvas is {@link BackdropView}'s.
 */
public final class BackdropBlur {
    public static final int WIDTH = 96;
    public static final int HEIGHT = 54;

    /** A PC tile's cover: the tile's 22sp name sits on it, and a white cover must still give 4.5:1. */
    static final float TILE_DARKEN = 0.35f;
    /** Three box passes of this radius are the design's Gaussian of width / 8. */
    static final int RADIUS = 12;
    static final int RADIUS_PORTRAIT = 6;
    static final int PASSES = 3;

    /** The token values (ambient.*, day.ambient.*); constants so the bake needs no Context on its thread. */
    static final float SATURATE = 1.3f;
    static final float DAY_SATURATE = 1.5f;
    static final float MAX_LUMA = 0.085f;
    static final float MIN_LUMA_DAY = 0.5f;

    private BackdropBlur() { }

    /** The night ambient, landscape. */
    public static Bitmap make(Bitmap cover, int canvas) {
        return make(cover, canvas, false, false);
    }

    /**
     * @param cover the decoded cover, or null for a game with no art, which gets the plain canvas
     * @param canvas the screen's own colour, also what a cover with transparency sits on
     * @param day the day bake: lifted toward white instead of held down
     * @param portrait a 54x96 bitmap for a portrait window, else 96x54
     * @return an opaque bitmap
     */
    public static Bitmap make(Bitmap cover, int canvas, boolean day, boolean portrait) {
        int width = portrait ? HEIGHT : WIDTH;
        int height = portrait ? WIDTH : HEIGHT;
        int[] pixels = blurred(cover, canvas, width, height, portrait ? RADIUS_PORTRAIT : RADIUS);
        if (cover != null) {
            float[] hsv = new float[3];
            for (int y = 0; y < height; y++) {
                for (int x = 0; x < width; x++) {
                    int c = day ? bakeDay(pixels[y * width + x], hsv) : bakeNight(pixels[y * width + x], hsv);
                    pixels[y * width + x] = dither(c, x, y);
                }
            }
        }
        return opaque(pixels, width, height);
    }

    /**
     * The ambient with a vertical fade already in it, so a full-screen gradient need not be blended over it on every
     * frame. The fade is smooth and the bitmap is scaled up with filtering, so its low resolution does not show.
     *
     * @param fadeColors the fade's colour at each stop, with its alpha
     * @param fadeStops where each colour lies, as a fraction of the view's height
     * @param viewWidth the view that shows the bitmap centre-cropped
     * @param viewHeight its height
     * @return a new opaque bitmap of the same size
     */
    static Bitmap faded(Bitmap ambient, int[] fadeColors, float[] fadeStops, int viewWidth, int viewHeight) {
        int width = ambient.getWidth();
        int height = ambient.getHeight();
        int[] pixels = new int[width * height];
        ambient.getPixels(pixels, 0, width, 0, 0, width, height);
        float scale = Math.max(viewWidth / (float) width, viewHeight / (float) height);
        float top = (viewHeight - height * scale) / 2f;
        for (int y = 0; y < height; y++) {
            float at = ((y + 0.5f) * scale + top) / viewHeight;
            int fade = fadeAt(fadeColors, fadeStops, at);
            float alpha = (fade >>> 24) / 255f;
            for (int x = 0; x < width; x++) {
                int c = pixels[y * width + x];
                int r = Math.round(((c >> 16) & 0xFF) * (1f - alpha) + ((fade >> 16) & 0xFF) * alpha);
                int g = Math.round(((c >> 8) & 0xFF) * (1f - alpha) + ((fade >> 8) & 0xFF) * alpha);
                int b = Math.round((c & 0xFF) * (1f - alpha) + (fade & 0xFF) * alpha);
                pixels[y * width + x] = 0xFF000000 | r << 16 | g << 8 | b;
            }
        }
        return opaque(pixels, width, height);
    }

    /** The colour of a gradient between stops, with the same RGB throughout, at this fraction. */
    static int fadeAt(int[] colors, float[] stops, float at) {
        if (at <= stops[0]) return colors[0];
        for (int i = 1; i < stops.length; i++) {
            if (at <= stops[i]) {
                float t = (at - stops[i - 1]) / (stops[i] - stops[i - 1]);
                int a = Math.round((colors[i - 1] >>> 24) + ((colors[i] >>> 24) - (colors[i - 1] >>> 24)) * t);
                return a << 24 | (colors[i] & 0xFFFFFF);
            }
        }
        return colors[colors.length - 1];
    }

    /** Saturation lifted, yellows turned toward orange, luminance held to {@link #MAX_LUMA}. */
    static int bakeNight(int color, float[] hsv) {
        android.graphics.Color.colorToHSV(color, hsv);
        hsv[1] = Math.min(1f, hsv[1] * SATURATE);
        hsv[0] = yellowToOrange(hsv[0]);
        int lifted = android.graphics.Color.HSVToColor(hsv);
        float r = linear((lifted >> 16) & 0xFF);
        float g = linear((lifted >> 8) & 0xFF);
        float b = linear(lifted & 0xFF);
        float scale = Math.min(1f, MAX_LUMA / Math.max(luma(r, g, b), 1e-5f));
        return 0xFF000000 | srgb(r * scale) << 16 | srgb(g * scale) << 8 | srgb(b * scale);
    }

    /** More saturation, and each pixel mixed toward white until its luminance reaches {@link #MIN_LUMA_DAY}. */
    static int bakeDay(int color, float[] hsv) {
        android.graphics.Color.colorToHSV(color, hsv);
        hsv[1] = Math.min(1f, hsv[1] * DAY_SATURATE);
        int lifted = android.graphics.Color.HSVToColor(hsv);
        float r = linear((lifted >> 16) & 0xFF);
        float g = linear((lifted >> 8) & 0xFF);
        float b = linear(lifted & 0xFF);
        float lum = luma(r, g, b);
        float mix = Math.max(0f, (MIN_LUMA_DAY - lum) / (1f - lum + 1e-5f));
        return 0xFF000000 | srgb(r + (1f - r) * mix) << 16 | srgb(g + (1f - g) * mix) << 8 | srgb(b + (1f - b) * mix);
    }

    /** Hues in the yellow band, degrees, go to 22; the band's ends fade over 15 degrees so no hue jumps. */
    static float yellowToOrange(float hue) {
        float band = clamp01((hue - 25f) / 15f) * clamp01((95f - hue) / 15f);
        return hue + (22f - hue) * band;
    }

    private static float clamp01(float v) {
        return v < 0f ? 0f : Math.min(v, 1f);
    }

    private static float luma(float r, float g, float b) {
        return 0.2126f * r + 0.7152f * g + 0.0722f * b;
    }

    private static float linear(int channel) {
        float c = channel / 255f;
        return c <= 0.04045f ? c / 12.92f : (float) Math.pow((c + 0.055f) / 1.055f, 2.4f);
    }

    private static int srgb(float c) {
        float v = c <= 0.0031308f ? c * 12.92f : 1.055f * (float) Math.pow(c, 1f / 2.4f) - 0.055f;
        return Math.max(0, Math.min(255, Math.round(v * 255f)));
    }

    /** One level of noise, so the upscaled gradient does not band. The same every time for a pixel. */
    private static int dither(int color, int x, int y) {
        int hash = (x * 73856093) ^ (y * 19349663);
        int d = ((hash >>> 7) & 3) - 1;
        if (d > 1) d = 0;
        int r = Math.max(0, Math.min(255, ((color >> 16) & 0xFF) + d));
        int g = Math.max(0, Math.min(255, ((color >> 8) & 0xFF) + d));
        int b = Math.max(0, Math.min(255, (color & 0xFF) + d));
        return 0xFF000000 | r << 16 | g << 8 | b;
    }

    /**
     * A PC tile's background: the cover of the game it is running, blurred and darkened, since the tile is a card of
     * its own. Its text sits on the tile's own scrim.
     *
     * @param cover the decoded cover; not null
     * @param canvas what a cover with transparency sits on
     */
    public static Bitmap tile(Bitmap cover, int canvas) {
        int[] pixels = blurred(cover, canvas, WIDTH, HEIGHT, RADIUS);
        for (int i = 0; i < pixels.length; i++) {
            int c = pixels[i];
            pixels[i] = 0xFF000000 | Math.round(((c >> 16) & 0xFF) * TILE_DARKEN) << 16
                    | Math.round(((c >> 8) & 0xFF) * TILE_DARKEN) << 8 | Math.round((c & 0xFF) * TILE_DARKEN);
        }
        return opaque(pixels, WIDTH, HEIGHT);
    }

    private static int[] blurred(Bitmap cover, int canvas, int width, int height, int radius) {
        Bitmap small = Bitmap.createBitmap(width, height, Bitmap.Config.ARGB_8888);
        small.eraseColor(canvas | 0xFF000000);
        if (cover != null && cover.getWidth() > 0 && cover.getHeight() > 0) draw(cover, small);

        int[] pixels = new int[width * height];
        small.getPixels(pixels, 0, width, 0, 0, width, height);
        if (cover != null) {
            int[] scratch = new int[pixels.length];
            for (int pass = 0; pass < PASSES; pass++) {
                box(pixels, scratch, width, height, radius, true);
                box(scratch, pixels, width, height, radius, false);
            }
        }
        return pixels;
    }

    private static Bitmap opaque(int[] pixels, int width, int height) {
        Bitmap result = Bitmap.createBitmap(pixels, width, height, Bitmap.Config.ARGB_8888);
        result.setHasAlpha(false);
        return result;
    }

    /** The whole cover, halved until it is near thumbnail size so the final bilinear step does not alias. */
    private static void draw(Bitmap cover, Bitmap into) {
        Rect source = new Rect(0, 0, cover.getWidth(), cover.getHeight());
        Bitmap current = cover;
        // The artwork loader decodes straight to the GPU on newer Android; a software canvas cannot draw those.
        if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.O
                && cover.getConfig() == Bitmap.Config.HARDWARE) {
            current = cover.copy(Bitmap.Config.ARGB_8888, false);
            if (current == null) return;
        }
        Paint paint = new Paint(Paint.FILTER_BITMAP_FLAG);
        while (source.width() >= 4 * into.getWidth() && source.height() >= 4 * into.getHeight()) {
            Bitmap half = Bitmap.createBitmap(source.width() / 2, source.height() / 2, Bitmap.Config.ARGB_8888);
            new Canvas(half).drawBitmap(current, source, new Rect(0, 0, half.getWidth(), half.getHeight()), paint);
            current = half;
            source = new Rect(0, 0, half.getWidth(), half.getHeight());
        }
        new Canvas(into).drawBitmap(current, source, new Rect(0, 0, into.getWidth(), into.getHeight()), paint);
    }

    /** One box blur pass over rows or columns, clamping at the edges. */
    private static void box(int[] src, int[] dst, int width, int height, int radius, boolean horizontal) {
        int length = horizontal ? width : height;
        int lines = horizontal ? height : width;
        int step = horizontal ? 1 : width;
        int lineStep = horizontal ? width : 1;
        int divisor = 2 * radius + 1;
        for (int line = 0; line < lines; line++) {
            int base = line * lineStep;
            int r = 0, g = 0, b = 0;
            for (int i = -radius; i <= radius; i++) {
                int c = src[base + clamp(i, length) * step];
                r += (c >> 16) & 0xFF;
                g += (c >> 8) & 0xFF;
                b += c & 0xFF;
            }
            for (int i = 0; i < length; i++) {
                dst[base + i * step] = 0xFF000000 | (r / divisor) << 16 | (g / divisor) << 8 | b / divisor;
                int in = src[base + clamp(i + radius + 1, length) * step];
                int out = src[base + clamp(i - radius, length) * step];
                r += ((in >> 16) & 0xFF) - ((out >> 16) & 0xFF);
                g += ((in >> 8) & 0xFF) - ((out >> 8) & 0xFF);
                b += (in & 0xFF) - (out & 0xFF);
            }
        }
    }

    private static int clamp(int index, int length) {
        return index < 0 ? 0 : Math.min(index, length - 1);
    }
}
