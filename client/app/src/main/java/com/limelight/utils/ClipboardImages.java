package com.limelight.utils;

import android.content.ClipData;
import android.content.ContentResolver;
import android.content.Context;
import android.graphics.Bitmap;
import android.graphics.BitmapFactory;
import android.net.Uri;

import androidx.annotation.StringRes;
import androidx.core.content.FileProvider;

import com.limelight.BuildConfig;
import com.limelight.LimeLog;
import com.limelight.R;
import com.limelight.nvstream.http.ClipboardImageException;

import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;

/**
 * Images between this device's clipboard and the host's PNG clipboard. Android's clipboard holds an image as a
 * content URI: an outgoing one is copied to the cache and, unless it already is a PNG, converted to one; an incoming
 * PNG is streamed to the cache and shared through the app's FileProvider for as long as it is the newest. Neither
 * way holds an image's bytes in memory, and only a conversion decodes one, after checking it fits the heap.
 */
public final class ClipboardImages {
    /** Under the cache directory, as res/xml/report_paths.xml names it for the FileProvider. */
    static final String DIRECTORY = "clipboard";
    /** Under the cache directory, for outgoing images; not shared. */
    static final String OUTGOING = "clipboard-out";

    private ClipboardImages() {
    }

    /** The first image the clip holds, or null when it holds none. Asks the provider its type, so off the main thread. */
    public static Uri imageUri(ClipData clip, ContentResolver resolver) {
        if (clip == null) {
            return null;
        }
        boolean described = clip.getDescription() != null && clip.getDescription().hasMimeType("image/*");
        for (int i = 0; i < clip.getItemCount(); i++) {
            Uri uri = clip.getItemAt(i).getUri();
            if (uri == null) {
                continue;
            }
            String type = described ? null : resolver.getType(uri);
            if (described || (type != null && type.startsWith("image/"))) {
                return uri;
            }
        }
        return null;
    }

    /**
     * The image at {@code uri} as a PNG file within the host's limits, for the caller to {@link #discard}: a PNG as
     * it is, any other image Android decodes converted. Bounds are read before decoding, so an image over the pixel
     * limit or the free heap is refused without its bitmap.
     */
    public static File readPng(Context context, Uri uri) throws IOException {
        File copy = newFile(context, OUTGOING);
        boolean kept = false;
        try {
            try (InputStream in = context.getContentResolver().openInputStream(uri)) {
                if (in == null) {
                    throw new IOException("The clipboard image could not be opened");
                }
                try (OutputStream out = new FileOutputStream(copy)) {
                    copyAtMost(in, out, ClipboardImageException.MAX_BYTES);
                }
            }
            if (isPng(copy)) {
                ClipboardImageException.pngSize(copy);
                kept = true;
                return copy;
            }
            File png = newFile(context, OUTGOING);
            boolean converted = false;
            try {
                toPng(copy, png, availableHeap());
                converted = true;
                return png;
            } finally {
                if (!converted) {
                    discard(png);
                }
            }
        } catch (OutOfMemoryError e) {
            throw new ClipboardImageException(ClipboardImageException.Reason.TOO_LARGE, "Out of memory for the image");
        } finally {
            if (!kept) {
                discard(copy);
            }
        }
    }

    private static boolean isPng(File file) throws IOException {
        byte[] head = new byte[8];
        try (InputStream in = new java.io.FileInputStream(file)) {
            int got = 0, read;
            while (got < head.length && (read = in.read(head, got, head.length - got)) != -1) {
                got += read;
            }
            return got == head.length && ClipboardImageException.isPng(head);
        }
    }

    /** The bytes this process can still allocate on its Java heap. */
    static long availableHeap() {
        Runtime runtime = Runtime.getRuntime();
        return runtime.maxMemory() - (runtime.totalMemory() - runtime.freeMemory());
    }

    /**
     * Converts the image in {@code image} that Android decodes to a PNG in {@code png}, refusing one over the host's
     * side or pixel limits, or whose bitmap would not fit in {@code heap} bytes, as {@code TOO_LARGE}: a smaller copy
     * is never sent in its place.
     */
    static void toPng(File image, File png, long heap) throws IOException {
        BitmapFactory.Options bounds = new BitmapFactory.Options();
        bounds.inJustDecodeBounds = true;
        BitmapFactory.decodeFile(image.getPath(), bounds);
        if (bounds.outWidth <= 0 || bounds.outHeight <= 0) {
            throw new ClipboardImageException(ClipboardImageException.Reason.NOT_PNG, "Not an image Android can read");
        }
        checkDecode(bounds.outWidth, bounds.outHeight, heap);
        Bitmap bitmap;
        try {
            bitmap = BitmapFactory.decodeFile(image.getPath());
        } catch (OutOfMemoryError e) {
            throw tooLargeToDecode(bounds.outWidth, bounds.outHeight);
        }
        if (bitmap == null) {
            throw new ClipboardImageException(ClipboardImageException.Reason.NOT_PNG, "The image could not be decoded");
        }
        try (OutputStream out = new FileOutputStream(png)) {
            if (!bitmap.compress(Bitmap.CompressFormat.PNG, 100, out)) {
                throw new IOException("The image could not be converted to PNG");
            }
        } catch (OutOfMemoryError e) {
            throw tooLargeToDecode(bounds.outWidth, bounds.outHeight);
        } finally {
            bitmap.recycle();
        }
        ClipboardImageException.pngSize(png);
    }

    /** Refuses a {@code width}x{@code height} image over the host's limits or whose ARGB bitmap needs over {@code heap}. */
    static void checkDecode(int width, int height, long heap) throws ClipboardImageException {
        long pixels = (long) width * height;
        if (width > ClipboardImageException.MAX_SIDE || height > ClipboardImageException.MAX_SIDE
                || pixels > ClipboardImageException.MAX_PIXELS) {
            throw new ClipboardImageException(ClipboardImageException.Reason.TOO_LARGE, "The image is "
                    + width + "x" + height);
        }
        if (pixels * 4 > heap) {
            throw tooLargeToDecode(width, height);
        }
    }

    private static ClipboardImageException tooLargeToDecode(int width, int height) {
        return new ClipboardImageException(ClipboardImageException.Reason.TOO_LARGE, "A " + width + "x" + height
                + " bitmap does not fit in this app's memory");
    }

    /** Copies up to {@code limit} bytes; more is {@link ClipboardImageException.Reason#TOO_LARGE}. */
    static long copyAtMost(InputStream in, OutputStream out, long limit) throws IOException {
        byte[] buffer = new byte[64 * 1024];
        long length = 0;
        int read;
        while ((read = in.read(buffer)) != -1) {
            if (length + read > limit) {
                throw new ClipboardImageException(ClipboardImageException.Reason.TOO_LARGE,
                        "The image has over " + limit + " bytes");
            }
            out.write(buffer, 0, read);
            length += read;
        }
        return length;
    }

    /**
     * A new file for an incoming image, next to the current one: that stays until this one is on the clipboard
     * ({@link #keepOnly}).
     */
    public static File newIncoming(Context context) throws IOException {
        return newFile(context, DIRECTORY);
    }

    private static File newFile(Context context, String name) throws IOException {
        File directory = new File(context.getCacheDir(), name);
        if (!directory.isDirectory() && !directory.mkdirs()) {
            throw new IOException("Cannot create the clipboard image directory");
        }
        return File.createTempFile("clipboard-", ".png", directory);
    }

    /** The content URI the clipboard holds {@code image} by. */
    public static Uri share(Context context, File image) {
        return FileProvider.getUriForFile(context, BuildConfig.APPLICATION_ID + ".reports", image);
    }

    /**
     * Removes the incoming images before {@code image}, now that it is on the clipboard: the clipboard holds one
     * clip, and this one replaced theirs. One that stays is logged; it is the cache's, and replaced next time.
     */
    public static void keepOnly(Context context, File image) {
        File[] files = new File(context.getCacheDir(), DIRECTORY).listFiles();
        if (files == null) {
            return;
        }
        for (File file : files) {
            if (!file.equals(image)) {
                discard(file);
            }
        }
    }

    /** Deletes an image file this class made; one that stays is logged. */
    public static void discard(File file) {
        if (file.exists() && !file.delete()) {
            LimeLog.warning("Clipboard image not removed: " + file);
        }
    }

    /**
     * What to tell the user about a refusal of the image clipboard, for an image going to the host when {@code send}
     * and coming to this device when not.
     */
    @StringRes
    public static int message(ClipboardImageException.Reason reason, boolean send) {
        switch (reason) {
            case TOO_LARGE: return send ? R.string.clipboard_image_too_large : R.string.clipboard_image_too_large_here;
            case NOT_PNG: return R.string.clipboard_image_not_png;
            case UNSUPPORTED: return R.string.clipboard_image_unsupported;
            case FORBIDDEN: return R.string.clipboard_image_forbidden;
            case NO_HOST_SUPPORT: return R.string.clipboard_image_no_host;
            default: throw new IllegalArgumentException(reason.name());
        }
    }
}
