package com.limelight.grid;

import android.content.Context;
import android.graphics.Bitmap;
import android.graphics.BitmapFactory;
import android.os.Handler;
import android.os.Looper;
import android.os.SystemClock;
import android.util.LruCache;
import android.widget.ImageView;

import androidx.core.content.ContextCompat;

import com.limelight.R;
import com.limelight.grid.assets.DiskAssetLoader;
import com.limelight.nvstream.http.NvApp;
import com.limelight.ui.BackdropBlur;

import java.io.File;
import java.util.HashMap;
import java.util.Map;
import java.util.concurrent.Executor;
import java.util.concurrent.LinkedBlockingQueue;
import java.util.concurrent.ThreadPoolExecutor;
import java.util.concurrent.TimeUnit;

/**
 * The backgrounds of the TV PC tiles. A PC playing a game gets that game's cover, blurred and darkened once on a worker
 * into a 96x54 thumbnail (BackdropBlur.tile) that the tile stretches with bilinear filtering; any other PC, or one whose
 * cover is not on disk yet, gets the gradient of its state. No live blur, and a refresh of the grid costs a cache lookup.
 */
final class TileBackdrops {
    /** The state's gradient: online, waiting on something (pairing, waking, checking), or away. */
    static int gradient(PcGridAdapter.RowState state) {
        switch (state) {
            case RUNNING:
            case READY:
                return R.drawable.tv_tile_bg_live;
            case OFFLINE:
                return R.drawable.tv_tile_bg_idle;
            default:
                return R.drawable.tv_tile_bg_attention;
        }
    }

    private static final Executor WORKER = worker();
    /** A few PCs, about 20 KB each. */
    private static final LruCache<String, Bitmap> CACHE = new LruCache<>(8);
    /** How long a cover that was not on disk is left alone before the next bind looks again; it may have been fetched since. */
    static final long RETRY_MS = 5000;
    private static Executor executor = WORKER;

    private final Context context;
    private final Handler main = new Handler(Looper.getMainLooper());
    /** Covers that were not on disk, and when: per grid, so a new PCs screen always looks again. */
    private final Map<String, Long> missing = new HashMap<>();

    TileBackdrops(Context context) {
        this.context = context.getApplicationContext();
    }

    /** Tests run the decode on their own thread. */
    static void setWorker(Executor worker) {
        executor = worker == null ? WORKER : worker;
    }

    static void clear() {
        CACHE.evictAll();
    }

    static String key(String hostUuid, NvApp running) {
        return running == null ? null : hostUuid + ":" + running.getAssetCacheKey();
    }

    /**
     * Puts the tile's background on {@code view}: the running game's cover when it has one, and the state's gradient
     * otherwise, or until the cover is decoded. The opaque cover replaces the gradient rather than covering it.
     */
    void show(ImageView view, String hostUuid, NvApp running, PcGridAdapter.RowState state) {
        String key = state == PcGridAdapter.RowState.RUNNING ? key(hostUuid, running) : null;
        Object shown = view.getTag(R.id.grid_backdrop);
        if (key == null) {
            view.setTag(R.id.grid_backdrop, null);
            view.setImageDrawable(null);
            view.setBackgroundResource(gradient(state));
            return;
        }
        Bitmap cached = CACHE.get(key);
        if (cached != null) {
            cover(view, key, cached);
            return;
        }
        view.setBackgroundResource(gradient(state));
        Long missedAt = missing.get(key);
        boolean retry = missedAt != null && SystemClock.uptimeMillis() - missedAt >= RETRY_MS;
        if (!retry && (key.equals(shown) || missedAt != null)) {
            // Already on its way to this view, or not on disk a moment ago.
            if (!key.equals(shown)) view.setImageDrawable(null);
            view.setTag(R.id.grid_backdrop, key);
            return;
        }
        missing.remove(key);
        view.setTag(R.id.grid_backdrop, key);
        view.setImageDrawable(null);
        File file = new DiskAssetLoader(context).getFile(hostUuid, running.getAssetCacheKey());
        int canvas = ContextCompat.getColor(context, R.color.tv_canvas);
        executor.execute(() -> {
            Bitmap tile = decode(file, canvas);
            main.post(() -> {
                if (tile == null) {
                    missing.put(key, SystemClock.uptimeMillis());
                    return;
                }
                CACHE.put(key, tile);
                if (key.equals(view.getTag(R.id.grid_backdrop))) cover(view, key, tile);
            });
        });
    }

    private static void cover(ImageView view, String key, Bitmap tile) {
        view.setTag(R.id.grid_backdrop, key);
        view.setImageBitmap(tile);
        view.setBackground(null);
    }

    /** The cover, decoded at about 4x the thumbnail's width so BackdropBlur's halving has something to work on. */
    private static Bitmap decode(File file, int canvas) {
        if (file == null || !file.isFile()) return null;
        BitmapFactory.Options bounds = new BitmapFactory.Options();
        bounds.inJustDecodeBounds = true;
        BitmapFactory.decodeFile(file.getPath(), bounds);
        if (bounds.outWidth <= 0 || bounds.outHeight <= 0) return null;
        BitmapFactory.Options options = new BitmapFactory.Options();
        options.inSampleSize = DiskAssetLoader.calculateInSampleSize(bounds, 4 * BackdropBlur.WIDTH, 4 * BackdropBlur.HEIGHT);
        Bitmap cover = BitmapFactory.decodeFile(file.getPath(), options);
        if (cover == null) return null;
        Bitmap tile = BackdropBlur.tile(cover, canvas);
        cover.recycle();
        return tile;
    }

    private static Executor worker() {
        ThreadPoolExecutor pool = new ThreadPoolExecutor(1, 1, 5, TimeUnit.SECONDS, new LinkedBlockingQueue<>(), runnable -> {
            Thread thread = new Thread(runnable, "tile-blur");
            thread.setDaemon(true);
            thread.setPriority(Thread.MIN_PRIORITY);
            return thread;
        });
        pool.allowCoreThreadTimeOut(true);
        return pool;
    }
}
