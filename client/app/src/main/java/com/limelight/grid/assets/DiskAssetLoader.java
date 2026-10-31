package com.limelight.grid.assets;

import android.app.ActivityManager;
import android.content.Context;
import android.graphics.Bitmap;
import android.graphics.BitmapFactory;
import android.graphics.ImageDecoder;
import android.os.Build;

import com.limelight.LimeLog;
import com.limelight.utils.CacheHelper;

import java.io.File;
import java.io.BufferedOutputStream;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;

public class DiskAssetLoader {
    // 5 MB
    private static final long MAX_ASSET_SIZE = 5 * 1024 * 1024;

    // A cover is decoded to cover this box, a 2:3 poster (a little taller than the old 300x400); its own shape is kept
    private static final int STANDARD_ASSET_WIDTH = 300;
    private static final int STANDARD_ASSET_HEIGHT = 450;

    private final boolean isLowRamDevice;
    private final File cacheDir;

    public DiskAssetLoader(Context context) {
        this(context.getCacheDir(),
                ((ActivityManager) context.getSystemService(Context.ACTIVITY_SERVICE)).isLowRamDevice());
    }

    DiskAssetLoader(File cacheDir, boolean isLowRamDevice) {
        this.cacheDir = cacheDir;
        this.isLowRamDevice = isLowRamDevice;
    }

    public boolean checkCacheExists(CachedAppAssetLoader.LoaderTuple tuple) {
        return CacheHelper.cacheFileExists(cacheDir, "boxart", tuple.computer.uuid, tuple.cacheKey + ".png");
    }

    /**
     * The decoded size of a cover: just large enough to cover the standard box, never larger than the file, and in
     * the file's own shape. A cover is cropped to fill its frame, so a banner or a square keeps the resolution its
     * cropped middle needs; a fixed size would squeeze it into a poster first.
     */
    static int[] targetSize(int width, int height) {
        if (width <= 0 || height <= 0) return new int[] {STANDARD_ASSET_WIDTH, STANDARD_ASSET_HEIGHT};
        float scale = Math.min(1f, Math.max((float) STANDARD_ASSET_WIDTH / width, (float) STANDARD_ASSET_HEIGHT / height));
        return new int[] {Math.max(1, Math.round(width * scale)), Math.max(1, Math.round(height * scale))};
    }

    // https://developer.android.com/topic/performance/graphics/load-bitmap.html
    public static int calculateInSampleSize(BitmapFactory.Options options, int reqWidth, int reqHeight) {
        // Raw height and width of image
        final int height = options.outHeight;
        final int width = options.outWidth;
        int inSampleSize = 1;

        if (height > reqHeight || width > reqWidth) {

            final int halfHeight = height / 2;
            final int halfWidth = width / 2;

            // Calculates the largest inSampleSize value that is a power of 2 and keeps both
            // height and width larger than the requested height and width.
            while ((halfHeight / inSampleSize) >= reqHeight && (halfWidth / inSampleSize) >= reqWidth) {
                inSampleSize *= 2;
            }
        }

        return inSampleSize;
    }

    public ScaledBitmap loadBitmapFromCache(CachedAppAssetLoader.LoaderTuple tuple, int sampleSize) {
        File file = getFile(tuple.computer.uuid, tuple.cacheKey);

        // Don't bother with anything if it doesn't exist
        if (!file.exists()) {
            return null;
        }

        // Make sure the cached asset doesn't exceed the maximum size
        if (file.length() > MAX_ASSET_SIZE) {
            LimeLog.warning("Removing cached tuple exceeding size threshold: "+tuple);
            file.delete();
            return null;
        }

        Bitmap bmp;

        // For OSes prior to P, we have to use the ugly BitmapFactory API
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.P) {
            // Lookup bounds of the downloaded image
            BitmapFactory.Options decodeOnlyOptions = new BitmapFactory.Options();
            decodeOnlyOptions.inJustDecodeBounds = true;
            BitmapFactory.decodeFile(file.getAbsolutePath(), decodeOnlyOptions);
            if (decodeOnlyOptions.outWidth <= 0 || decodeOnlyOptions.outHeight <= 0) {
                // Dimensions set to -1 on error. Return value always null.
                return null;
            }

            LimeLog.info("Tuple "+tuple+" has cached art of size: "+decodeOnlyOptions.outWidth+"x"+decodeOnlyOptions.outHeight);

            // Load the image scaled to the appropriate size
            BitmapFactory.Options options = new BitmapFactory.Options();
            options.inSampleSize = calculateInSampleSize(decodeOnlyOptions,
                    STANDARD_ASSET_WIDTH / sampleSize,
                    STANDARD_ASSET_HEIGHT / sampleSize);
            if (isLowRamDevice) {
                options.inPreferredConfig = Bitmap.Config.RGB_565;
                options.inDither = true;
            }
            else if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                options.inPreferredConfig = Bitmap.Config.HARDWARE;
            }

            bmp = BitmapFactory.decodeFile(file.getAbsolutePath(), options);
            if (bmp != null) {
                LimeLog.info("Tuple "+tuple+" decoded from disk cache with sample size: "+options.inSampleSize);
                return new ScaledBitmap(decodeOnlyOptions.outWidth, decodeOnlyOptions.outHeight, bmp);
            }
        }
        else {
            // On P, we can get a bitmap back in one step with ImageDecoder
            final ScaledBitmap scaledBitmap = new ScaledBitmap();
            try {
                scaledBitmap.bitmap = ImageDecoder.decodeBitmap(ImageDecoder.createSource(file), new ImageDecoder.OnHeaderDecodedListener() {
                    @Override
                    public void onHeaderDecoded(ImageDecoder imageDecoder, ImageDecoder.ImageInfo imageInfo, ImageDecoder.Source source) {
                        scaledBitmap.originalWidth = imageInfo.getSize().getWidth();
                        scaledBitmap.originalHeight = imageInfo.getSize().getHeight();

                        int[] target = targetSize(scaledBitmap.originalWidth, scaledBitmap.originalHeight);
                        imageDecoder.setTargetSize(target[0], target[1]);
                        if (isLowRamDevice) {
                            imageDecoder.setMemorySizePolicy(ImageDecoder.MEMORY_POLICY_LOW_RAM);
                        }
                    }
                });
                return scaledBitmap;
            } catch (IOException e) {
                e.printStackTrace();
                return null;
            }
        }

        return null;
    }

    public File getFile(String computerUuid, String cacheKey) {
        return CacheHelper.openPath(false, cacheDir, "boxart", computerUuid, cacheKey + ".png");
    }

    public void deleteAssetsForComputer(String computerUuid) {
        File dir = CacheHelper.openPath(false, cacheDir, "boxart", computerUuid);
        File[] files = dir.listFiles();
        if (files != null) {
            for (File f : files) {
                f.delete();
            }
        }
        dir.delete();
    }

    public void populateCacheWithStream(CachedAppAssetLoader.LoaderTuple tuple, InputStream input) {
        File temporary = null;
        try {
            File target = CacheHelper.openPath(true, cacheDir, "boxart", tuple.computer.uuid, tuple.cacheKey + ".png");
            temporary = File.createTempFile("boxart-", ".tmp", target.getParentFile());
            try (OutputStream out = new BufferedOutputStream(new FileOutputStream(temporary))) {
                CacheHelper.writeInputStreamToOutputStream(input, out, MAX_ASSET_SIZE);
            }
            // Readers and concurrent downloads must only see complete files.
            if (!temporary.renameTo(target)) {
                throw new IOException("Unable to publish cached box art");
            }
        } catch (IOException e) {
            LimeLog.warning("Unable to populate cache with tuple: "+tuple);
            e.printStackTrace();
        } finally {
            if (temporary != null) {
                temporary.delete();
            }
        }
    }
}
