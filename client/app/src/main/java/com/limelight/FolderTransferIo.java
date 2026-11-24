package com.limelight;

import android.app.Application;
import android.content.ContentResolver;
import android.content.ContentValues;
import android.database.Cursor;
import android.net.Uri;
import android.os.Build;
import android.provider.DocumentsContract;
import android.provider.MediaStore;
import android.provider.OpenableColumns;
import android.webkit.MimeTypeMap;

import androidx.annotation.StringRes;

import com.limelight.nvstream.http.SharedFolders;

import java.io.FilterInputStream;
import java.io.FilterOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.util.Locale;

/**
 * Local storage, MediaStore/SAF document helpers, MIME resolution and transfer error formatting for
 * {@link FolderBrowserActivity}.
 */
final class FolderTransferIo {
    private FolderTransferIo() { }

    /** Display name and byte length read from a picked upload document's provider. */
    static final class UploadMeta {
        final String name;
        final long size;

        UploadMeta(String name, long size) {
            this.name = name;
            this.size = size;
        }
    }

    /** User-facing failure text and whether retrying the operation makes sense. */
    static final class Failure {
        final String message;
        final boolean canRetry;

        Failure(String message, boolean canRetry) {
            this.message = message;
            this.canRetry = canRetry;
        }
    }

    static String mimeType(String name) {
        int dot = name.lastIndexOf('.');
        String type = dot < 0 ? null : MimeTypeMap.getSingleton()
                .getMimeTypeFromExtension(name.substring(dot + 1).toLowerCase(Locale.ROOT));
        return type == null ? "application/octet-stream" : type;
    }

    /** What to tell the user about a refusal; {@code upload} tells a missing write grant from a missing grant. */
    @StringRes
    static int refusalMessage(SharedFolders.Refusal refusal, boolean upload) {
        switch (refusal) {
            case FORBIDDEN: return upload ? R.string.folders_upload_forbidden : R.string.folders_forbidden;
            case FOLDERS_DISABLED: return R.string.folders_disabled;
            case FOLDER_UNREADABLE: return R.string.folders_unreadable;
            case NOT_FOUND: return R.string.folders_not_found;
            case INVALID_NAME: return R.string.folders_invalid_name;
            case INCOMPLETE_UPLOAD:
            case LENGTH_REQUIRED: return R.string.folders_incomplete;
            case TOO_LARGE: return R.string.folders_too_large;
            case INSUFFICIENT_STORAGE: return R.string.folders_storage_full;
            case NO_CONSOLE_USER: return R.string.folders_no_console_user;
            case TRANSFER_BUSY: return R.string.folders_busy;
            default: throw new IllegalArgumentException(refusal.name());
        }
    }

    /** Reads the display name and size of a document picked for upload, off the main thread. */
    static UploadMeta queryUpload(ContentResolver resolver, Uri source) throws SharedFolders.LocalFileException {
        String name = null;
        long size = -1;
        try (Cursor cursor = resolver.query(source, new String[] {OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE},
                null, null, null)) {
            if (cursor != null && cursor.moveToFirst()) {
                name = cursor.isNull(0) ? null : cursor.getString(0);
                size = cursor.isNull(1) ? -1 : cursor.getLong(1);
            }
        } catch (RuntimeException e) {
            throw new SharedFolders.LocalFileException(new IOException(e.getMessage(), e));
        }
        return new UploadMeta(name, size);
    }

    /** Opening a document fails on this device, not on the network. */
    static OutputStream openOutput(ContentResolver resolver, Uri uri) throws SharedFolders.LocalFileException {
        try {
            return resolver.openOutputStream(uri, "wt");
        } catch (IOException e) {
            throw new SharedFolders.LocalFileException(e);
        }
    }

    static InputStream openInput(ContentResolver resolver, Uri uri) throws SharedFolders.LocalFileException {
        try {
            return resolver.openInputStream(uri);
        } catch (IOException e) {
            throw new SharedFolders.LocalFileException(e);
        }
    }

    /** {@code out}, whose flush and close failures are this device's, not the network's. */
    static OutputStream local(OutputStream out, String name) throws SharedFolders.LocalFileException {
        if (out == null) throw new SharedFolders.LocalFileException(new IOException("Cannot open " + name));
        return new FilterOutputStream(out) {
            @Override public void write(byte[] bytes, int offset, int length) throws IOException {
                out.write(bytes, offset, length);
            }
            @Override public void close() throws IOException {
                try {
                    super.close();
                } catch (SharedFolders.LocalFileException e) {
                    throw e;
                } catch (IOException e) {
                    throw new SharedFolders.LocalFileException(e);
                }
            }
        };
    }

    /** {@code in}, whose close failure is this device's, not the network's. */
    static InputStream local(InputStream in, String name) throws SharedFolders.LocalFileException {
        if (in == null) throw new SharedFolders.LocalFileException(new IOException("Cannot open " + name));
        return new FilterInputStream(in) {
            @Override public void close() throws IOException {
                try {
                    super.close();
                } catch (IOException e) {
                    throw new SharedFolders.LocalFileException(e);
                }
            }
        };
    }

    /** The name the provider shows for {@code uri}, or {@code name} when it gives none. */
    static String displayName(ContentResolver resolver, Uri uri, String name) {
        try (Cursor cursor = resolver.query(uri, new String[] {OpenableColumns.DISPLAY_NAME}, null, null, null)) {
            if (cursor != null && cursor.moveToFirst() && !cursor.isNull(0) && !cursor.getString(0).isEmpty()) {
                return cursor.getString(0);
            }
        } catch (RuntimeException e) {
            LimeLog.warning("No display name for " + uri + ": " + e.getMessage());
        }
        return name;
    }

    /** A new row in Downloads, hidden from other apps until {@link #publish}. */
    static Uri pendingDownload(ContentResolver resolver, String name) throws IOException {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) throw new IllegalStateException("Downloads needs Android 10");
        ContentValues values = new ContentValues();
        values.put(MediaStore.MediaColumns.DISPLAY_NAME, name);
        values.put(MediaStore.MediaColumns.MIME_TYPE, mimeType(name));
        values.put(MediaStore.MediaColumns.IS_PENDING, 1);
        Uri uri = resolver.insert(MediaStore.Downloads.EXTERNAL_CONTENT_URI, values);
        if (uri == null) throw new SharedFolders.LocalFileException(new IOException("Downloads did not take " + name));
        return uri;
    }

    static void publish(ContentResolver resolver, Uri uri) throws IOException {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) throw new IllegalStateException("Downloads needs Android 10");
        ContentValues values = new ContentValues();
        values.put(MediaStore.MediaColumns.IS_PENDING, 0);
        if (resolver.update(uri, values, null, null) != 1) throw new SharedFolders.LocalFileException(new IOException("Downloads did not publish " + uri));
    }

    /**
     * Deletes what a failed download left: the pending Downloads row, or the document the user picked. Returns
     * null once it is gone, or the name it stays under for the user to delete.
     */
    static String discard(ContentResolver resolver, Uri uri, boolean document) {
        try {
            boolean gone = document ? DocumentsContract.deleteDocument(resolver, uri)
                    : resolver.delete(uri, null, null) > 0;
            if (gone) return null;
            LimeLog.warning("Failed download not removed: " + uri);
        } catch (Exception e) {
            LimeLog.warning("Failed download not removed: " + uri + ": " + e.getMessage());
        }
        return displayName(resolver, uri, uri.toString());
    }

    static Failure describeFailure(Application app, Exception e, boolean upload) {
        boolean canRetry = !(e instanceof SharedFolders.RefusedException)
                || ((SharedFolders.RefusedException) e).refusal != SharedFolders.Refusal.FORBIDDEN;
        String error;
        if (e instanceof SharedFolders.CancelledException) {
            error = app.getString(R.string.folders_cancelled);
        } else if (e instanceof SharedFolders.RefusedException) {
            error = app.getString(refusalMessage(((SharedFolders.RefusedException) e).refusal, upload));
        } else if (e instanceof SharedFolders.InvalidPathException) {
            error = app.getString(R.string.folders_bad_path, String.valueOf(e.getMessage()));
            canRetry = false;
        } else if (e instanceof SharedFolders.LocalFileException || e instanceof SecurityException) {
            error = app.getString(R.string.folders_local_failed, String.valueOf(e.getMessage()));
        } else {
            error = app.getString(R.string.folders_failed, String.valueOf(e.getMessage()));
        }
        return new Failure(error, canRetry);
    }
}
