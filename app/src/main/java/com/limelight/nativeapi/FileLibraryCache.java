package com.limelight.nativeapi;

import com.limelight.LimeLog;

import org.json.JSONException;
import org.json.JSONObject;

import java.io.File;
import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.io.IOException;
import java.nio.charset.StandardCharsets;

/** {@link LibraryCache} as one file per host in a directory the app owns. */
public final class FileLibraryCache implements LibraryCache {
    private final File directory;

    public FileLibraryCache(File directory) {
        this.directory = directory;
    }

    private File file(String hostUuid) {
        return new File(directory, hostUuid.replaceAll("[^A-Za-z0-9_-]", "_") + ".json");
    }

    @Override
    public synchronized Entry load(String hostUuid) {
        File file = file(hostUuid);
        if (!file.isFile()) {
            return null;
        }
        try (FileInputStream in = new FileInputStream(file)) {
            byte[] bytes = new byte[(int) file.length()];
            int read = 0;
            while (read < bytes.length) {
                int n = in.read(bytes, read, bytes.length - read);
                if (n < 0) {
                    break;
                }
                read += n;
            }
            JSONObject json = new JSONObject(new String(bytes, 0, read, StandardCharsets.UTF_8));
            return new Entry(json.getJSONObject("library").toString(), json.optLong("fetched_ms"));
        } catch (IOException | JSONException e) {
            LimeLog.warning("Unreadable library cache for " + hostUuid + ": " + e.getMessage());
            return null;
        }
    }

    @Override
    public synchronized void store(String hostUuid, Entry entry) {
        File file = file(hostUuid);
        File temp = new File(directory, file.getName() + ".tmp");
        try {
            if (!directory.isDirectory() && !directory.mkdirs()) {
                throw new IOException("Cannot create " + directory);
            }
            String body = new JSONObject().put("fetched_ms", entry.fetchedAtMs)
                    .put("library", new JSONObject(entry.libraryJson)).toString();
            try (FileOutputStream out = new FileOutputStream(temp)) {
                out.write(body.getBytes(StandardCharsets.UTF_8));
            }
            if (!temp.renameTo(file)) {
                // Some file systems refuse to rename over an existing file.
                if (!file.delete() || !temp.renameTo(file)) {
                    throw new IOException("Cannot replace " + file);
                }
            }
        } catch (IOException | JSONException e) {
            LimeLog.warning("Could not cache the library of " + hostUuid + ": " + e.getMessage());
            temp.delete();
        }
    }

    @Override
    public synchronized void forget(String hostUuid) {
        file(hostUuid).delete();
    }
}
