package com.limelight.files;

import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;

import java.util.ArrayList;
import java.util.Collections;
import java.util.List;

/** One transfer as the Rust list reports it (files.rs {@code row}). */
public final class FileRow {
    public enum State { QUEUED, RUNNING, DONE, FAILED, CANCELLED }

    public final long id;
    public final boolean upload;
    /** The name on the PC: what an upload is sent as, what a download fetches. */
    public final String name;
    /** The name on this device. */
    public final String localName;
    public final long size;
    public final State state;
    public final long done;
    public final long total;
    /** Where it landed when done: the PC's name for an upload (" (n)" when the name was taken). */
    public final String savedName;
    /** The problem id of a failed transfer ({@link FileProblems}), else null. */
    public final String problem;
    /** The host's or the client's words for a failure, for the details. */
    public final String message;
    public final boolean cancelling;
    public final boolean canRetry;

    FileRow(JSONObject row) throws JSONException {
        id = row.getLong("id");
        upload = "upload".equals(row.getString("direction"));
        name = row.getString("name");
        localName = row.getString("local_name");
        size = row.getLong("size");
        state = State.valueOf(row.getString("state").toUpperCase(java.util.Locale.ROOT));
        done = row.optLong("done");
        total = row.optLong("total");
        savedName = row.optString("saved_name", null);
        problem = row.optString("problem", null);
        message = row.optString("message", null);
        cancelling = row.optBoolean("cancelling");
        canRetry = row.optBoolean("can_retry");
    }

    public boolean isFinished() {
        return state == State.DONE || state == State.FAILED || state == State.CANCELLED;
    }

    /** 0 to 100, or -1 while it is not known. */
    public int percent() {
        if (state == State.DONE) return 100;
        if (state != State.RUNNING || total <= 0) return -1;
        return (int) Math.min(100, done * 100 / total);
    }

    /** The rows of a snapshot; a reply that is not one is a bug in the bridge, so it throws. */
    public static List<FileRow> parse(String snapshot) {
        try {
            JSONArray rows = new JSONArray(snapshot);
            List<FileRow> parsed = new ArrayList<>(rows.length());
            for (int i = 0; i < rows.length(); i++) {
                parsed.add(new FileRow(rows.getJSONObject(i)));
            }
            return Collections.unmodifiableList(parsed);
        } catch (JSONException | IllegalArgumentException e) {
            throw new IllegalStateException("Unreadable transfer list: " + e.getMessage(), e);
        }
    }

    /** The row of a {@code run} answer, or null when the list or the start was gone. */
    static FileRow ofRun(String reply) {
        try {
            JSONObject ok = new JSONObject(reply).optJSONObject("ok");
            return ok == null ? null : new FileRow(ok);
        } catch (JSONException | IllegalArgumentException e) {
            throw new IllegalStateException("Unreadable transfer answer: " + e.getMessage(), e);
        }
    }
}
