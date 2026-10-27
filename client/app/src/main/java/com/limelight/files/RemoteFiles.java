package com.limelight.files;

import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;

import java.util.ArrayList;
import java.util.Collections;
import java.util.List;

/** The PC's file transfer folder (GET /api/native/v1/files), or why it could not be read. */
public final class RemoteFiles {
    public static final class Entry {
        public final String name;
        public final long size;
        public final long modifiedUnixMs;

        Entry(String name, long size, long modifiedUnixMs) {
            this.name = name;
            this.size = size;
            this.modifiedUnixMs = modifiedUnixMs;
        }
    }

    public final String folder;
    public final long freeBytes;
    public final long maxFileBytes;
    public final List<Entry> files;
    /** The problem id when the list could not be read ({@link FileProblems}), else null. */
    public final String problem;
    public final String message;

    private RemoteFiles(String folder, long freeBytes, long maxFileBytes, List<Entry> files, String problem,
                        String message) {
        this.folder = folder;
        this.freeBytes = freeBytes;
        this.maxFileBytes = maxFileBytes;
        this.files = files;
        this.problem = problem;
        this.message = message;
    }

    static RemoteFiles failed(String problem, String message) {
        return new RemoteFiles(null, 0, 0, Collections.emptyList(), problem, message);
    }

    /** {"ok": list} or {"error": {problem, message}}; anything else reads as a failure of the host's reply. */
    public static RemoteFiles parse(String reply) {
        try {
            JSONObject json = new JSONObject(reply);
            JSONObject ok = json.optJSONObject("ok");
            if (ok == null) {
                JSONObject error = json.getJSONObject("error");
                return failed(error.getString("problem"), error.optString("message", null));
            }
            JSONArray list = ok.getJSONArray("files");
            List<Entry> files = new ArrayList<>(list.length());
            for (int i = 0; i < list.length(); i++) {
                JSONObject file = list.getJSONObject(i);
                files.add(new Entry(file.getString("name"), file.getLong("size"), file.getLong("modified_unix_ms")));
            }
            return new RemoteFiles(ok.getString("folder"), ok.getLong("free_bytes"), ok.getLong("max_file_bytes"),
                    Collections.unmodifiableList(files), null, null);
        } catch (JSONException e) {
            return failed(FileProblems.HOST, "Unreadable file list: " + e.getMessage());
        }
    }
}
