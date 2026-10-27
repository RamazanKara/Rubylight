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
import com.limelight.R;
import com.limelight.nvstream.http.ClipboardImageException;

import java.io.ByteArrayOutputStream;
import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.InputStream;

/**
 * Images between this device's clipboard and the host's PNG clipboard. Android's clipboard holds an image as a
 * content URI: an outgoing one is read and, unless it already is a PNG, converted to one; an incoming PNG is kept
 * in the cache and shared through the app's FileProvider for as long as it is the newest.
 */
public final class ClipboardImages {
    /** Under the cache directory, as res/xml/report_paths.xml names it for the FileProvider. */
    static final String DIRECTORY = "clipboard";

    private ClipboardImages() {
    }

    /** The first image the clip holds, or null when it holds none. */
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
     * The image at {@code uri} as a PNG within the host's limits: a PNG as it is, any other image Android decodes
     * converted. Bounds are read before decoding, so an image over the pixel limit is refused without its bitmap.
     */
    public static byte[] readPng(ContentResolver resolver, Uri uri) throws IOException {
        byte[] bytes;
        try (InputStream in = resolver.openInputStream(uri)) {
            if (in == null) {
                throw new IOException("The clipboard image could not be opened");
            }
            bytes = readAtMost(in, ClipboardImageException.MAX_BYTES);
        }
        if (ClipboardImageException.isPng(bytes)) {
            ClipboardImageException.pngSize(bytes);
            return bytes;
        }
        return toPng(bytes);
    }

    /** Converts an image Android decodes to a PNG, refusing one over the host's side or pixel limits. */
    static byte[] toPng(byte[] image) throws IOException {
        BitmapFactory.Options bounds = new BitmapFactory.Options();
        bounds.inJustDecodeBounds = true;
        BitmapFactory.decodeByteArray(image, 0, image.length, bounds);
        if (bounds.outWidth <= 0 || bounds.outHeight <= 0) {
            throw new ClipboardImageException(ClipboardImageException.Reason.NOT_PNG, "Not an image Android can read");
        }
        long pixels = (long) bounds.outWidth * bounds.outHeight;
        if (bounds.outWidth > ClipboardImageException.MAX_SIDE || bounds.outHeight > ClipboardImageException.MAX_SIDE
                || pixels > ClipboardImageException.MAX_PIXELS) {
            throw new ClipboardImageException(ClipboardImageException.Reason.TOO_LARGE, "The image is "
                    + bounds.outWidth + "x" + bounds.outHeight);
        }
        Bitmap bitmap = BitmapFactory.decodeByteArray(image, 0, image.length);
        if (bitmap == null) {
            throw new ClipboardImageException(ClipboardImageException.Reason.NOT_PNG, "The image could not be decoded");
        }
        try {
            ByteArrayOutputStream png = new ByteArrayOutputStream();
            if (!bitmap.compress(Bitmap.CompressFormat.PNG, 100, png)) {
                throw new IOException("The image could not be converted to PNG");
            }
            byte[] bytes = png.toByteArray();
            ClipboardImageException.pngSize(bytes);
            return bytes;
        } finally {
            bitmap.recycle();
        }
    }

    /** Up to {@code limit} bytes; more is {@link ClipboardImageException.Reason#TOO_LARGE}. */
    static byte[] readAtMost(InputStream in, int limit) throws IOException {
        ByteArrayOutputStream out = new ByteArrayOutputStream();
        byte[] buffer = new byte[64 * 1024];
        int read;
        while ((read = in.read(buffer)) != -1) {
            if (out.size() + (long) read > limit) {
                throw new ClipboardImageException(ClipboardImageException.Reason.TOO_LARGE,
                        "The image has over " + limit + " bytes");
            }
            out.write(buffer, 0, read);
        }
        return out.toByteArray();
    }

    /**
     * Keeps {@code png} as the newest clipboard image and returns its content URI. The ones before it go: the
     * clipboard holds one clip, and this one replaces theirs.
     */
    public static Uri store(Context context, byte[] png) throws IOException {
        return FileProvider.getUriForFile(context, BuildConfig.APPLICATION_ID + ".reports", write(context, png));
    }

    /** Writes {@code png} as the only file in the clipboard image directory. */
    static File write(Context context, byte[] png) throws IOException {
        File directory = new File(context.getCacheDir(), DIRECTORY);
        if (!directory.isDirectory() && !directory.mkdirs()) {
            throw new IOException("Cannot create the clipboard image directory");
        }
        File[] old = directory.listFiles();
        if (old != null) {
            for (File file : old) {
                if (!file.delete()) {
                    throw new IOException("Cannot remove the previous clipboard image " + file.getName());
                }
            }
        }
        File image = new File(directory, "clipboard-" + System.currentTimeMillis() + ".png");
        try (FileOutputStream out = new FileOutputStream(image)) {
            out.write(png);
        }
        return image;
    }

    /** What to tell the user about a refusal of the image clipboard. */
    @StringRes
    public static int message(ClipboardImageException.Reason reason) {
        switch (reason) {
            case TOO_LARGE: return R.string.clipboard_image_too_large;
            case NOT_PNG: return R.string.clipboard_image_not_png;
            case UNSUPPORTED: return R.string.clipboard_image_unsupported;
            case FORBIDDEN: return R.string.clipboard_image_forbidden;
            default: throw new IllegalArgumentException(reason.name());
        }
    }
}
