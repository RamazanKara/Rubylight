package com.limelight.grid;

import android.content.Context;
import android.graphics.Bitmap;
import android.graphics.BitmapFactory;
import android.os.Handler;
import android.os.Looper;
import android.os.SystemClock;
import android.util.LruCache;
import android.widget.ImageView;


import com.limelight.R;
import com.limelight.grid.assets.DiskAssetLoader;
import com.limelight.grid.assets.PlaceholderArt;
import com.limelight.nvstream.http.NvApp;

import java.io.File;
import java.util.HashMap;
import java.util.Map;
import java.util.concurrent.Executor;
import java.util.concurrent.LinkedBlockingQueue;
import java.util.concurrent.ThreadPoolExecutor;
import java.util.concurrent.TimeUnit;

/**
 * The running game's cover on a PC: cropped once on a worker to the monitor's face (or whole, for a phone's running row), and
 * shown when it arrives. A PC with nothing to show, or one whose cover is not on disk yet, shows what lies under the view: its
 * own gradient and disc. A refresh of the grid costs a cache lookup.
 */
final class TileBackdrops {
    /** What the cover is cut to. */
    enum Cut {
        /** The monitor's face: the design's crop, a little below the cover's middle. */
        SCREEN(480, 480 * .62f),
        /** The whole cover. */
        COVER(320, 480);

        final int width;
        final float height;

        Cut(int width, float height) {
            this.width = width;
            this.height = height;
        }
    }

    /** How far down the cover the face's crop stands: 0 the top, 1 the foot. */
    private static final float FACE_CROP_Y = .58f;
    private static final Executor WORKER = worker();
    /** A few PCs, under a megabyte each. */
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

    static String key(String hostUuid, NvApp running, Cut cut) {
        return running == null ? null : hostUuid + ":" + running.getAssetCacheKey() + ":" + cut;
    }

    /** Puts the running game's cover on {@code view}, or nothing while there is none, so what lies under it shows. */
    void show(ImageView view, String hostUuid, NvApp running, PcGridAdapter.RowState state, Cut cut) {
        String key = state == PcGridAdapter.RowState.RUNNING ? key(hostUuid, running, cut) : null;
        Object shown = view.getTag(R.id.grid_backdrop);
        if (key == null) {
            view.setTag(R.id.grid_backdrop, null);
            view.setImageDrawable(null);
            return;
        }
        Bitmap cached = CACHE.get(key);
        if (cached != null) {
            cover(view, key, cached);
            return;
        }
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
        executor.execute(() -> {
            Bitmap tile = decode(file, cut);
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
    }

    private static Bitmap decode(File file, Cut cut) {
        // The host's grey stand-in is no cover: the PC keeps its gradient.
        if (!PlaceholderArt.isCover(file)) return null;
        BitmapFactory.Options bounds = new BitmapFactory.Options();
        bounds.inJustDecodeBounds = true;
        BitmapFactory.decodeFile(file.getPath(), bounds);
        if (bounds.outWidth <= 0 || bounds.outHeight <= 0) return null;
        BitmapFactory.Options options = new BitmapFactory.Options();
        options.inSampleSize = DiskAssetLoader.calculateInSampleSize(bounds, cut.width, Math.round(cut.height));
        Bitmap cover = BitmapFactory.decodeFile(file.getPath(), options);
        if (cover == null) return null;
        return cut == Cut.SCREEN ? cropToFace(cover) : cover;
    }

    /** The largest part of the cover with the face's shape, centred across and a little below the middle down. */
    static Bitmap cropToFace(Bitmap cover) {
        float aspect = Cut.SCREEN.width / Cut.SCREEN.height;
        int w = cover.getWidth();
        int h = cover.getHeight();
        int cropW = Math.min(w, Math.round(h * aspect));
        int cropH = Math.min(h, Math.round(cropW / aspect));
        if (cropW == w && cropH == h) return cover;
        Bitmap face = Bitmap.createBitmap(cover, (w - cropW) / 2, Math.round((h - cropH) * FACE_CROP_Y), cropW, cropH);
        if (face != cover) cover.recycle();
        return face;
    }

    private static Executor worker() {
        ThreadPoolExecutor pool = new ThreadPoolExecutor(1, 1, 5, TimeUnit.SECONDS, new LinkedBlockingQueue<>(), runnable -> {
            Thread thread = new Thread(runnable, "pc-cover");
            thread.setDaemon(true);
            thread.setPriority(Thread.MIN_PRIORITY);
            return thread;
        });
        pool.allowCoreThreadTimeOut(true);
        return pool;
    }
}
