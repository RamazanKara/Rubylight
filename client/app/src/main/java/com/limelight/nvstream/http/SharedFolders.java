package com.limelight.nvstream.http;

import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;

import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;

import okhttp3.HttpUrl;
import okhttp3.MediaType;
import okhttp3.RequestBody;
import okhttp3.ResponseBody;
import okio.BufferedSink;
import okio.BufferedSource;

/**
 * Shared folders under the native API ({@code /api/native/v1/folders}), as nightjar-core's folders.rs speaks them:
 * the folders the PC shares with this device, their directories, and files moved in and out of them. Gate it on the
 * native {@code folders} feature, and uploads on {@link Folder#writable}. Every call blocks on HTTPS.
 *
 * <p>A path is the names below the folder joined by {@code /}, empty for the folder itself. The host checks every
 * name; this side refuses only what the routes can't carry and {@code .part} uploads, which the host refuses too.
 * The host serves a file whole with its exact length and takes no ranges, so a failed download starts over.
 */
public final class SharedFolders {
    /** The suffix of a transfer in progress, on the host and here. */
    public static final String PART_SUFFIX = ".part";
    /** The most names a folder path has (the host's limit). */
    public static final int MAX_FOLDER_DEPTH = 64;
    /** A folder list, or an upload's answer or a refusal. */
    static final int MAX_JSON_BYTES = 1024 * 1024;
    /** A directory listing. */
    static final int MAX_LIST_BYTES = 4 * 1024 * 1024;
    private static final String ROUTE = "api/native/v1/folders";

    /** A folder the PC shares with this device. */
    public static final class Folder {
        /** The id the routes take; stable across renames. */
        public final String id;
        /** What the console calls the folder. */
        public final String name;
        /** Whether this device may upload into it; an access from a newer host counts as read only. */
        public final boolean writable;

        Folder(String id, String name, boolean writable) {
            this.id = id;
            this.name = name;
            this.writable = writable;
        }
    }

    public enum Kind {
        FILE, DIR,
        /** A kind from a newer host: neither opened nor downloaded. */
        UNKNOWN
    }

    /** One name in a listed directory. */
    public static final class Entry {
        public final String name;
        public final Kind kind;
        /** Bytes; 0 for a directory. */
        public final long size;
        public final long modifiedUnixMs;

        Entry(String name, Kind kind, long size, long modifiedUnixMs) {
            this.name = name;
            this.kind = kind;
            this.size = size;
            this.modifiedUnixMs = modifiedUnixMs;
        }
    }

    /** A directory of a shared folder: folders first, then files, each by name, as the host sorts them. */
    public static final class Listing {
        /** The listed directory below the folder; empty for the folder itself. */
        public final String path;
        public final List<Entry> entries;

        Listing(String path, List<Entry> entries) {
            this.path = path;
            this.entries = Collections.unmodifiableList(entries);
        }
    }

    /** Bytes moved so far and the total, at most every 64 KiB. */
    public interface Progress {
        void onProgress(long done, long total);
    }

    /** A refusal the host documents, by HTTP status and error code. */
    public enum Refusal {
        /** 403 {@code forbidden}: this device has no grant for the folder, or no write grant for an upload. */
        FORBIDDEN(403, "forbidden"),
        /** 403 {@code folders_disabled}: the host shares no folders. */
        FOLDERS_DISABLED(403, "folders_disabled"),
        /** 404 {@code not_found}: no such folder, directory or file. */
        NOT_FOUND(404, "not_found"),
        /** 400 {@code invalid_name}. */
        INVALID_NAME(400, "invalid_name"),
        /** 400 {@code incomplete_upload}: the body ended before its length. */
        INCOMPLETE_UPLOAD(400, "incomplete_upload"),
        /** 411 {@code length_required}. */
        LENGTH_REQUIRED(411, "length_required"),
        /** 413 {@code too_large}. */
        TOO_LARGE(413, "too_large"),
        /** 507 {@code insufficient_storage}. */
        INSUFFICIENT_STORAGE(507, "insufficient_storage"),
        /** 409 {@code no_console_user}: nobody is signed in on the PC. */
        NO_CONSOLE_USER(409, "no_console_user"),
        /** 409 {@code transfer_busy}: another transfer from this device is running. */
        TRANSFER_BUSY(409, "transfer_busy");

        final int status;
        final String code;

        Refusal(int status, String code) {
            this.status = status;
            this.code = code;
        }

        static Refusal of(int status, String code) {
            for (Refusal refusal : values()) {
                if (refusal.status == status && refusal.code.equals(code)) {
                    return refusal;
                }
            }
            return null;
        }
    }

    /** The host answered with a documented refusal. */
    public static final class RefusedException extends IOException {
        public final Refusal refusal;
        public final int status;

        RefusedException(Refusal refusal, int status, String message) {
            super("Host error " + status + " " + refusal.code + ": " + message);
            this.refusal = refusal;
            this.status = status;
        }
    }

    /** Reading the file to upload, or writing the download, failed on this device; the cause says how. */
    public static final class LocalFileException extends IOException {
        public LocalFileException(IOException cause) {
            super(cause.getMessage(), cause);
        }
    }

    /** A folder id or path the host would not read, refused here before any request. */
    public static final class InvalidPathException extends IllegalArgumentException {
        InvalidPathException(String message) {
            super(message);
        }
    }

    /** The transfer was cancelled through its {@link TransferCancel}. */
    public static final class CancelledException extends IOException {
        CancelledException() {
            super("Cancelled");
        }
    }

    private final NvHTTP http;

    public SharedFolders(NvHTTP http) {
        this.http = http;
    }

    public List<Folder> folders() throws IOException {
        try (NvHTTP.HostCall call = http.hostCall("GET", ROUTE, null, null)) {
            return parseFolders(json(call, 200, MAX_JSON_BYTES));
        }
    }

    public Listing entries(String id, String path) throws IOException {
        try (NvHTTP.HostCall call = http.hostCall("GET", route(id, "entries"), pathQuery(path), null)) {
            return parseEntries(json(call, 200, MAX_LIST_BYTES));
        }
    }

    /**
     * Writes the file at {@code path} to {@code sink} and returns its length. Only a whole file returns: a short
     * one, a refusal or a cancel throws, and the caller then throws away what the sink has.
     */
    public long download(String id, String path, OutputStream sink, Progress progress, TransferCancel cancel)
            throws IOException {
        checkPath(path);
        if (path.isEmpty()) {
            throw new InvalidPathException("A download needs a file path");
        }
        try (NvHTTP.HostCall call = http.hostCall("GET", route(id, "content"), pathQuery(path), null, cancel)) {
            if (call.response.code() != 200) {
                throw failure(call.response.code(), call.response.message(), call.response.body());
            }
            ResponseBody body = call.response.body();
            long total = body.contentLength();
            BufferedSource source = body.source();
            byte[] buffer = new byte[64 * 1024];
            long done = 0;
            int read;
            while ((read = source.read(buffer)) != -1) {
                try {
                    sink.write(buffer, 0, read);
                } catch (IOException e) {
                    throw new LocalFileException(e);
                }
                done += read;
                progress.onProgress(done, total);
            }
            if (total >= 0 && done != total) {
                throw new IOException("The download ended after " + done + " of " + total + " bytes");
            }
            try {
                sink.flush();
            } catch (IOException e) {
                throw new LocalFileException(e);
            }
            return done;
        } catch (LocalFileException e) {
            throw e;
        } catch (IOException e) {
            throw cancel.isCancelled() ? new CancelledException() : e;
        }
    }

    /**
     * Uploads {@code length} bytes of {@code file} as {@code path} and returns the name the host stored it under,
     * which gains " (n)" when the name was taken. The host refuses by the head before any byte is sent.
     */
    public String upload(String id, String path, InputStream file, long length, Progress progress,
                         TransferCancel cancel) throws IOException {
        String name = fileName(path);
        if (name.isEmpty() || hasPartSuffix(name)) {
            throw new IllegalArgumentException("The host takes no upload named \"" + name + "\"");
        }
        if (length < 0) {
            throw new IllegalArgumentException("An upload needs its length");
        }
        checkPath(path);
        RequestBody body = new RequestBody() {
            @Override public MediaType contentType() { return MediaType.get("application/octet-stream"); }
            @Override public long contentLength() { return length; }
            // The stream is read once: a failure after it started is never retried.
            @Override public boolean isOneShot() { return true; }

            @Override
            public void writeTo(BufferedSink out) throws IOException {
                byte[] buffer = new byte[64 * 1024];
                long done = 0;
                if (length == 0 && hasMore(file)) {
                    throw new LocalFileException(new IOException("The file changed during the upload: it is no longer empty"));
                }
                while (done < length) {
                    int read;
                    try {
                        read = file.read(buffer, 0, (int) Math.min(buffer.length, length - done));
                    } catch (IOException e) {
                        throw new LocalFileException(e);
                    }
                    if (read == -1) {
                        throw new LocalFileException(new IOException("The file ended after " + done + " of "
                                + length + " bytes"));
                    }
                    if (done + read == length && hasMore(file)) {
                        // Before the last bytes go: the host never sees a whole body, so it keeps no file.
                        throw new LocalFileException(new IOException("The file changed during the upload: it has over "
                                + length + " bytes"));
                    }
                    out.write(buffer, 0, read);
                    done += read;
                    progress.onProgress(done, length);
                }
            }
        };
        try (NvHTTP.HostCall call = http.hostCall("PUT", route(id, "content"), pathQuery(path), body, cancel)) {
            JSONObject stored;
            try {
                stored = new JSONObject(json(call, 201, MAX_JSON_BYTES));
                String storedName = typed(stored, "name", String.class);
                long size = unsigned(stored, "size");
                if (size != length) {
                    throw new IOException("The host stored " + size + " of " + length + " bytes");
                }
                return storedName;
            } catch (JSONException e) {
                throw new IOException("Malformed upload answer: " + e.getMessage(), e);
            }
        } catch (LocalFileException e) {
            throw e;
        } catch (IOException e) {
            throw cancel.isCancelled() ? new CancelledException() : e;
        }
    }

    /** Whether {@code file} has a byte past the length it was uploaded with. */
    private static boolean hasMore(InputStream file) throws LocalFileException {
        try {
            return file.read(new byte[1], 0, 1) != -1;
        } catch (IOException e) {
            throw new LocalFileException(e);
        }
    }

    /** The body of an answer with the {@code success} status, or the refusal or error any other status stands for. */
    private static String json(NvHTTP.HostCall call, int success, int limit) throws IOException {
        int status = call.response.code();
        if (status != success) {
            throw failure(status, call.response.message(), call.response.body());
        }
        return read(call.response.body(), limit);
    }

    static IOException failure(int status, String message, ResponseBody body) throws IOException {
        if (status >= 200 && status <= 299) {
            return new HostHttpResponseException(status, message);
        }
        String text;
        try {
            text = read(body, MAX_JSON_BYTES);
        } catch (IOException e) {
            return new HostHttpResponseException(status, message);
        }
        try {
            JSONObject error = new JSONObject(text).getJSONObject("error");
            Refusal refusal = Refusal.of(status, error.optString("code"));
            if (refusal != null) {
                return new RefusedException(refusal, status, error.optString("message"));
            }
            return new HostHttpResponseException(status, error.optString("message", message));
        } catch (JSONException e) {
            return new HostHttpResponseException(status, message);
        }
    }

    private static String read(ResponseBody body, int limit) throws IOException {
        if (body.source().request(limit + 1L)) {
            throw new IOException("The host's answer is over " + limit + " bytes");
        }
        return new String(body.bytes(), StandardCharsets.UTF_8);
    }

    static List<Folder> parseFolders(String json) throws IOException {
        try {
            JSONArray folders = typed(new JSONObject(json), "folders", JSONArray.class);
            List<Folder> list = new ArrayList<>();
            for (int i = 0; i < folders.length(); i++) {
                JSONObject folder = typed(folders, i, JSONObject.class);
                list.add(new Folder(typed(folder, "id", String.class), typed(folder, "name", String.class),
                        "write".equals(typed(folder, "access", String.class))));
            }
            return list;
        } catch (JSONException e) {
            throw new IOException("Malformed folder list: " + e.getMessage(), e);
        }
    }

    static Listing parseEntries(String json) throws IOException {
        try {
            JSONObject listing = new JSONObject(json);
            String path = typed(listing, "path", String.class);
            JSONArray entries = typed(listing, "entries", JSONArray.class);
            List<Entry> list = new ArrayList<>();
            for (int i = 0; i < entries.length(); i++) {
                JSONObject entry = typed(entries, i, JSONObject.class);
                String kind = typed(entry, "kind", String.class);
                list.add(new Entry(typed(entry, "name", String.class),
                        "file".equals(kind) ? Kind.FILE : "dir".equals(kind) ? Kind.DIR : Kind.UNKNOWN,
                        unsigned(entry, "size"), unsigned(entry, "modified_unix_ms")));
            }
            return new Listing(path, list);
        } catch (JSONException e) {
            throw new IOException("Malformed folder listing: " + e.getMessage(), e);
        }
    }

    /** {@code object.key} as a {@code type}; org.json's getters would convert "1" or 1 instead of failing. */
    private static <T> T typed(JSONObject object, String key, Class<T> type) throws JSONException {
        Object value = object.opt(key);
        if (!type.isInstance(value)) {
            throw new JSONException(key + " is not a " + type.getSimpleName());
        }
        return type.cast(value);
    }

    private static <T> T typed(JSONArray array, int index, Class<T> type) throws JSONException {
        Object value = array.opt(index);
        if (!type.isInstance(value)) {
            throw new JSONException("[" + index + "] is not a " + type.getSimpleName());
        }
        return type.cast(value);
    }

    private static long unsigned(JSONObject object, String key) throws JSONException {
        Object value = object.opt(key);
        if (!(value instanceof Integer || value instanceof Long) || ((Number) value).longValue() < 0) {
            throw new JSONException(key + " is not an unsigned integer");
        }
        return ((Number) value).longValue();
    }

    /** {@code api/native/v1/folders/{id}/{route}}. */
    static String route(String id, String route) {
        checkId(id);
        return ROUTE + "/" + id + "/" + route;
    }

    /** {@code path=...}, percent-encoded. */
    static String pathQuery(String path) {
        checkPath(path);
        return new HttpUrl.Builder().scheme("https").host("host").addQueryParameter("path", path).build()
                .encodedQuery();
    }

    /**
     * A folder id as the host's setting allows it: 1 to 64 ASCII letters, digits, {@code -} or {@code _}. It is a
     * segment of the URL path, so nothing else is sent.
     */
    static void checkId(String id) {
        boolean valid = id.length() >= 1 && id.length() <= 64;
        for (int i = 0; valid && i < id.length(); i++) {
            char c = id.charAt(i);
            valid = (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') || (c >= '0' && c <= '9') || c == '-' || c == '_';
        }
        if (!valid) {
            throw new InvalidPathException("Not a shared folder id: \"" + id + "\"");
        }
    }

    /** A path the host could read: at most {@link #MAX_FOLDER_DEPTH} names, none of them empty. */
    static void checkPath(String path) {
        if (path.isEmpty()) {
            return;
        }
        String[] names = path.split("/", -1);
        boolean valid = names.length <= MAX_FOLDER_DEPTH;
        for (String name : names) {
            valid &= !name.isEmpty();
        }
        if (!valid) {
            throw new InvalidPathException("Not a shared folder path: \"" + path + "\"");
        }
    }

    /** {@code name} below the directory {@code dir} (empty for the folder itself). */
    public static String childPath(String dir, String name) {
        return dir.isEmpty() ? name : dir + "/" + name;
    }

    /** The directory holding {@code path}; empty for a name in the folder itself. */
    public static String parentPath(String path) {
        int slash = path.lastIndexOf('/');
        return slash < 0 ? "" : path.substring(0, slash);
    }

    /** The last name of {@code path}. */
    public static String fileName(String path) {
        return path.substring(path.lastIndexOf('/') + 1);
    }

    /** Whether {@code name} ends in {@link #PART_SUFFIX}, in any case, as the host checks it. */
    public static boolean hasPartSuffix(String name) {
        return name.regionMatches(true, name.length() - PART_SUFFIX.length(), PART_SUFFIX, 0, PART_SUFFIX.length());
    }
}
