package com.limelight.ui;

import android.graphics.Bitmap;
import android.graphics.BlurMaskFilter;
import android.graphics.Canvas;
import android.graphics.Paint;
import android.graphics.Rect;
import android.graphics.RectF;
import android.os.Handler;
import android.os.Looper;
import android.util.LruCache;

import java.util.HashSet;
import java.util.Set;
import java.util.concurrent.Executor;
import java.util.concurrent.Executors;

/**
 * A soft shadow or halo around a round rectangle, as a nine-slice mask baked once on a worker thread and shared by
 * every view with that radius and blur. Drawing is nine bitmap quads: no blur on the GPU, no extra layer, on every
 * Android version. Until the mask is baked nothing is drawn, and the asker is invalidated when it is ready.
 */
public final class GlassShadow {
    private static final LruCache<String, GlassShadow> CACHE = new LruCache<>(16);
    private static final Set<String> BAKING = new HashSet<>();
    private static final Executor WORKER = Executors.newSingleThreadExecutor(runnable -> {
        Thread thread = new Thread(runnable, "glass-shadow");
        thread.setDaemon(true);
        thread.setPriority(Thread.MIN_PRIORITY);
        return thread;
    });
    private static final Handler MAIN = new Handler(Looper.getMainLooper());
    /** The mask is half size: a blur has no detail to lose. */
    private static final int SCALE = 2;
    /** Where the blur stops, in sigmas: what lies further is under 3 levels in 255 but a third of the pixels filled. */
    static final float REACH_SIGMAS = 2.25f;

    private final Bitmap mask;
    /** Reach of the blur past the shape, in view pixels. */
    private final int reach;
    /** Size of one corner of the slice, in view pixels. */
    private final int corner;
    private final Rect from = new Rect();
    private final RectF to = new RectF();

    private GlassShadow(Bitmap mask, int reach, int corner) {
        this.mask = mask;
        this.reach = reach;
        this.corner = corner;
    }

    /** The shadow for this radius and blur (a CSS blur radius, in pixels), or null while it bakes. */
    public static GlassShadow get(float radius, float blur, Runnable whenReady) {
        if (blur <= 0f) return null;
        int r = Math.round(radius);
        int b = Math.round(blur);
        String key = r + "/" + b;
        GlassShadow ready = CACHE.get(key);
        if (ready != null) return ready;
        if (BAKING.add(key)) {
            WORKER.execute(() -> {
                GlassShadow baked = bake(r, b);
                MAIN.post(() -> {
                    CACHE.put(key, baked);
                    BAKING.remove(key);
                    if (whenReady != null) whenReady.run();
                });
            });
        } else if (whenReady != null) {
            MAIN.postDelayed(whenReady, 64);
        }
        return null;
    }

    private static GlassShadow bake(int radius, int blur) {
        // A CSS blur radius is twice the Gaussian's sigma; BlurMaskFilter's radius is sigma / 0.57735.
        float sigma = blur / 2f;
        int reach = reachFor(blur);
        int corner = reach + radius + 1;
        int size = (corner * 2 + SCALE * 2) / SCALE;
        Bitmap bitmap = Bitmap.createBitmap(size, size, Bitmap.Config.ALPHA_8);
        Canvas canvas = new Canvas(bitmap);
        Paint paint = new Paint(Paint.ANTI_ALIAS_FLAG);
        float scaledSigma = sigma / SCALE;
        paint.setMaskFilter(new BlurMaskFilter(Math.max(0.5f, (scaledSigma - 0.5f) / 0.57735f + 0.5f), BlurMaskFilter.Blur.NORMAL));
        float inset = reach / (float) SCALE;
        float shapeRadius = radius / (float) SCALE;
        canvas.drawRoundRect(new RectF(inset, inset, size - inset, size - inset), shapeRadius, shapeRadius, paint);
        return new GlassShadow(bitmap, reach, corner);
    }

    /** Past this many pixels a blur (a CSS blur radius) is cut off. */
    static int reachFor(float blur) {
        return (int) Math.ceil(blur / 2f * REACH_SIGMAS);
    }

    /** How far the blur reaches past the shape. */
    public int reach() {
        return reach;
    }

    /**
     * Draws around {@code shape} in the paint's colour. The paint has no shader, and the caller clips out the shape
     * first when the surface over it is see-through.
     */
    public void draw(Canvas canvas, float left, float top, float right, float bottom, Paint paint) {
        int size = mask.getWidth();
        int slice = corner / SCALE;
        float x0 = left - reach;
        float x1 = x0 + corner;
        float x3 = right + reach;
        float x2 = x3 - corner;
        float y0 = top - reach;
        float y1 = y0 + corner;
        float y3 = bottom + reach;
        float y2 = y3 - corner;
        if (x2 < x1) x1 = x2 = (x0 + x3) / 2f;
        if (y2 < y1) y1 = y2 = (y0 + y3) / 2f;
        int mid0 = slice;
        int mid1 = size - slice;
        quad(canvas, 0, 0, mid0, mid0, x0, y0, x1, y1, paint);
        quad(canvas, mid0, 0, mid1, mid0, x1, y0, x2, y1, paint);
        quad(canvas, mid1, 0, size, mid0, x2, y0, x3, y1, paint);
        quad(canvas, 0, mid0, mid0, mid1, x0, y1, x1, y2, paint);
        quad(canvas, mid0, mid0, mid1, mid1, x1, y1, x2, y2, paint);
        quad(canvas, mid1, mid0, size, mid1, x2, y1, x3, y2, paint);
        quad(canvas, 0, mid1, mid0, size, x0, y2, x1, y3, paint);
        quad(canvas, mid0, mid1, mid1, size, x1, y2, x2, y3, paint);
        quad(canvas, mid1, mid1, size, size, x2, y2, x3, y3, paint);
    }

    private void quad(Canvas canvas, int sl, int st, int sr, int sb, float dl, float dt, float dr, float db, Paint paint) {
        if (dr <= dl || db <= dt) return;
        from.set(sl, st, sr, sb);
        to.set(dl, dt, dr, db);
        canvas.drawBitmap(mask, from, to, paint);
    }
}
