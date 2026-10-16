package com.limelight.update;

import android.content.Context;

import java.io.ByteArrayOutputStream;
import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.nio.charset.StandardCharsets;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.util.Locale;

import okhttp3.Call;
import okhttp3.Request;
import okhttp3.Response;

/** Downloads a release APK into the cache and checks it against the release's SHA256SUMS file. */
public final class UpdateDownloader {
    public interface Progress {
        void onProgress(long done, long total);
    }

    private static final String DIRECTORY = "updates";
    private static final long MAX_SUMS_BYTES = 64 * 1024;

    private volatile Call call;
    private volatile boolean cancelled;

    public void cancel() {
        cancelled = true;
        Call current = call;
        if (current != null) {
            current.cancel();
        }
    }

    static File directory(Context context) {
        return new File(context.getCacheDir(), DIRECTORY);
    }

    /** Removes downloaded APKs left by earlier updates. */
    public static void cleanUp(Context context) {
        File[] files = directory(context).listFiles();
        for (int i = 0; files != null && i < files.length; i++) {
            files[i].delete();
        }
    }

    /**
     * Returns the verified APK. The checksum comes first, so a release without one costs no download.
     * Blocks on the network; call from a background thread.
     */
    public File download(Context context, ReleaseInfo release, boolean root, Progress progress) throws IOException {
        ReleaseInfo.Asset apk = release.apkAsset(root);
        if (apk == null) {
            throw new UpdateException(UpdateException.Reason.NO_APK);
        }
        ReleaseInfo.Asset sums = release.checksumAsset();
        if (sums == null) {
            throw new UpdateException(UpdateException.Reason.NO_CHECKSUMS);
        }
        requireHttps(apk.url);
        requireHttps(sums.url);

        String expected = ChecksumFile.find(fetchText(sums.url), apk.name);
        if (expected == null) {
            throw new UpdateException(UpdateException.Reason.CHECKSUM_ENTRY_MISSING);
        }

        cleanUp(context);
        File directory = directory(context);
        if (!directory.isDirectory() && !directory.mkdirs()) {
            throw new IOException("Cannot create " + directory);
        }
        File part = new File(directory, apk.name + ".part");
        File target = new File(directory, apk.name);
        try {
            String actual = fetchFile(apk.url, apk.size, part, progress);
            if (!actual.equals(expected)) {
                throw new UpdateException(UpdateException.Reason.CHECKSUM_MISMATCH);
            }
            if (!part.renameTo(target)) {
                throw new IOException("Cannot rename " + part);
            }
            return target;
        } finally {
            part.delete();
        }
    }

    private static void requireHttps(String url) throws UpdateException {
        if (!url.toLowerCase(Locale.ROOT).startsWith("https://")) {
            throw new UpdateException(UpdateException.Reason.DOWNLOAD_FAILED);
        }
    }

    private String fetchText(String url) throws IOException {
        if (cancelled) {
            throw new UpdateException(UpdateException.Reason.CANCELLED);
        }
        Request request = UpdateChecker.request(url).build();
        call = UpdateChecker.client().newCall(request);
        try (Response response = call.execute()) {
            if (!response.isSuccessful() || response.body() == null) {
                throw new UpdateException(UpdateException.Reason.DOWNLOAD_FAILED);
            }
            ByteArrayOutputStream text = new ByteArrayOutputStream();
            try (InputStream in = response.body().byteStream()) {
                byte[] buffer = new byte[4096];
                int read;
                while ((read = in.read(buffer)) > 0) {
                    text.write(buffer, 0, read);
                    if (text.size() > MAX_SUMS_BYTES) {
                        throw new UpdateException(UpdateException.Reason.DOWNLOAD_FAILED);
                    }
                }
            }
            return new String(text.toByteArray(), StandardCharsets.UTF_8);
        } catch (IOException e) {
            throw failure(e);
        }
    }

    /** Writes the response to the file and returns the SHA-256 of what was written. */
    private String fetchFile(String url, long expectedSize, File file, Progress progress) throws IOException {
        if (cancelled) {
            throw new UpdateException(UpdateException.Reason.CANCELLED);
        }
        Request request = UpdateChecker.request(url).build();
        call = UpdateChecker.client().newCall(request);
        try (Response response = call.execute()) {
            if (!response.isSuccessful() || response.body() == null) {
                throw new UpdateException(UpdateException.Reason.DOWNLOAD_FAILED);
            }
            long total = response.body().contentLength() > 0 ? response.body().contentLength() : expectedSize;
            MessageDigest digest = MessageDigest.getInstance("SHA-256");
            try (InputStream in = response.body().byteStream(); OutputStream out = new FileOutputStream(file)) {
                byte[] buffer = new byte[64 * 1024];
                long done = 0;
                int read;
                while ((read = in.read(buffer)) > 0) {
                    out.write(buffer, 0, read);
                    digest.update(buffer, 0, read);
                    done += read;
                    if (progress != null) {
                        progress.onProgress(done, total);
                    }
                }
                if (total > 0 && done != total) {
                    throw new UpdateException(UpdateException.Reason.DOWNLOAD_FAILED);
                }
            }
            return ChecksumFile.hex(digest.digest());
        } catch (NoSuchAlgorithmException e) {
            throw new IOException(e);
        } catch (IOException e) {
            throw failure(e);
        }
    }

    private UpdateException failure(IOException e) {
        if (e instanceof UpdateException) {
            return (UpdateException) e;
        }
        return new UpdateException(cancelled ? UpdateException.Reason.CANCELLED : UpdateException.Reason.DOWNLOAD_FAILED, e);
    }
}
