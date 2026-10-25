package com.limelight.grid.assets;

import android.util.LruCache;

import com.limelight.LimeLog;

import java.lang.ref.SoftReference;
import java.util.concurrent.ConcurrentHashMap;

public class MemoryAssetLoader {
    // Covers decode at 300x450, an eighth larger than the old 300x400, so the budget is an eighth larger too.
    private static final int maxMemory = (int) (Runtime.getRuntime().maxMemory() / 1024);
    private static final LruCache<String, ScaledBitmap> memoryCache = new LruCache<String, ScaledBitmap>(maxMemory / 14) {
        @Override
        protected int sizeOf(String key, ScaledBitmap bitmap) {
            // Sizeof returns kilobytes
            return bitmap.bitmap.getByteCount() / 1024;
        }

        @Override
        protected void entryRemoved(boolean evicted, String key, ScaledBitmap oldValue, ScaledBitmap newValue) {
            super.entryRemoved(evicted, key, oldValue, newValue);

            if (evicted) {
                // Keep a soft reference around to the bitmap as long as we can
                evictionCache.put(key, new SoftReference<>(oldValue));
            }
        }
    };
    private static final ConcurrentHashMap<String, SoftReference<ScaledBitmap>> evictionCache = new ConcurrentHashMap<>();

    private static String constructKey(CachedAppAssetLoader.LoaderTuple tuple) {
        return tuple.computer.uuid+"-"+tuple.cacheKey;
    }

    public ScaledBitmap loadBitmapFromCache(CachedAppAssetLoader.LoaderTuple tuple) {
        final String key = constructKey(tuple);

        ScaledBitmap bmp = memoryCache.get(key);
        if (bmp != null) {
            // No log here: this runs for every card on every key repeat.
            return bmp;
        }

        SoftReference<ScaledBitmap> bmpRef = evictionCache.get(key);
        if (bmpRef != null) {
            bmp = bmpRef.get();
            if (bmp != null) {
                LimeLog.info("Eviction cache hit for tuple: "+tuple);

                // Put this entry back into the LRU cache
                evictionCache.remove(key);
                memoryCache.put(key, bmp);

                return bmp;
            }
            else {
                // The data is gone, so remove the dangling SoftReference now
                evictionCache.remove(key);
            }
        }

        return null;
    }

    public void populateCache(CachedAppAssetLoader.LoaderTuple tuple, ScaledBitmap bitmap) {
        memoryCache.put(constructKey(tuple), bitmap);
    }

    public void clearCache() {
        // We must evict first because that will push all items into the eviction cache
        memoryCache.evictAll();
        evictionCache.clear();
    }
}
