package com.limelight.grid;

import android.content.Context;
import android.graphics.Bitmap;
import android.graphics.BitmapFactory;
import android.os.Handler;
import android.os.Looper;
import android.util.LruCache;
import android.widget.ImageView;

import androidx.core.content.ContextCompat;

import com.limelight.R;
import com.limelight.grid.assets.DiskAssetLoader;
import com.limelight.nvstream.http.NvApp;
import com.limelight.ui.BackdropBlur;

import java.io.File;
import java.util.HashSet;
import java.util.Set;
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
    /** Covers that were not on disk: asked for again only when the grid is rebuilt. */
    private static final Set<String> MISSING = new HashSet<>();
    private static Executor executor = WORKER;

    private final Context context;
    private final Handler main = new Handler(Looper.getMainLooper());

    TileBackdrops(Context context) {
        this.context = context.getApplicationContext();
    }

    /** Tests run the decode on their own thread. */
    static void setWorker(Executor worker) {
        executor = worker == null ? WORKER : worker;
    }

    static void clear() {
        CACHE.evictAll();
        MISSING.clear();
    }

    static String key(String hostUuid, NvApp running) {
        return running == null ? null : hostUuid + ":" + running.getAssetCacheKey();
    }

    /** Puts the tile's background on {@code view}: the gradient at once, and the running game's cover over it when it has one. */
    void show(ImageView view, String hostUuid, NvApp running, PcGridAdapter.RowState state) {
        view.setBackgroundResource(gradient(state));
        String key = state == PcGridAdapter.RowState.RUNNING ? key(hostUuid, running) : null;
        Object shown = view.getTag(R.id.grid_backdrop);
        if (key == null) {
            view.setTag(R.id.grid_backdrop, null);
            view.setImageDrawable(null);
            return;
        }
        Bitmap cached = CACHE.get(key);
        if (cached != null) {
            view.setTag(R.id.grid_backdrop, key);
            view.setImageBitmap(cached);
            return;
        }
        if (key.equals(shown)) return;
        view.setTag(R.id.grid_backdrop, key);
        view.setImageDrawable(null);
        if (MISSING.contains(key)) return;
        File file = new DiskAssetLoader(context).getFile(hostUuid, running.getAssetCacheKey());
        int canvas = ContextCompat.getColor(context, R.color.tv_canvas);
        executor.execute(() -> {
            Bitmap tile = decode(file, canvas);
            main.post(() -> {
                if (tile == null) {
                    MISSING.add(key);
                    return;
                }
                CACHE.put(key, tile);
                if (key.equals(view.getTag(R.id.grid_backdrop))) view.setImageBitmap(tile);
            });
        });
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
