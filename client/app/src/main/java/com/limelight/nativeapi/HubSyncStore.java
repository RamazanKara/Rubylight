package com.limelight.nativeapi;

import com.limelight.nvstream.jni.Json;

import java.io.ByteArrayOutputStream;
import java.io.File;
import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.util.List;
import java.util.Map;

/**
 * Disk persistence, ETag normalization, and snapshot/outbox mutation helpers for {@link HubSyncController}.
 */
final class HubSyncStore {
    private static final String DIR_NAME = "hub_sync";

    private HubSyncStore() {}

    /** Strips optional weak prefix ({@code W/}) and surrounding quotes from an HTTP {@code ETag}. */
    static String normalizeEtag(String etag) {
        if (etag == null) return "";
        String t = etag.trim();
        if (t.startsWith("W/")) t = t.substring(2).trim();
        if (t.length() >= 2 && t.startsWith("\"") && t.endsWith("\"")) {
            t = t.substring(1, t.length() - 1);
        }
        return t;
    }

    static String cappedCopyName(String baseName) {
        final String suffix = " (copy)";
        String src = baseName == null ? "Profile" : baseName;
        int maxBase = 120 - suffix.codePointCount(0, suffix.length());
        int count = src.codePointCount(0, src.length());
        if (count <= maxBase) return src + suffix;
        int end = src.offsetByCodePoints(0, maxBase);
        return src.substring(0, end) + suffix;
    }

    static File hostDir(File filesDir, String hostUuid) {
        String safe = hostUuid.replaceAll("[^A-Za-z0-9_-]", "_");
        return new File(new File(filesDir, DIR_NAME), safe.isEmpty() ? "unknown" : safe);
    }

    static void deleteHostDir(File filesDir, String hostUuid) {
        File dir = hostDir(filesDir, hostUuid);
        File[] files = dir.listFiles();
        if (files != null) {
            for (File f : files) f.delete();
        }
        dir.delete();
    }

    static HubSyncController.HostState loadFromDisk(File filesDir, String hostUuid) {
        HubSyncController.HostState state = new HubSyncController.HostState();
        File dir = hostDir(filesDir, hostUuid);
        String snapRaw = readText(new File(dir, "snapshot.json"));
        if (snapRaw != null) {
            try {
                state.snapshot = Json.parseObject(snapRaw);
                state.status = HubSyncController.SyncStatus.OFFLINE_DEGRADED;
            } catch (Json.SyntaxException ignored) {}
        }
        String etagRaw = readText(new File(dir, "snapshot_etag.txt"));
        if (etagRaw != null && !etagRaw.trim().isEmpty()) {
            state.snapshotEtag = etagRaw.trim();
        }
        String outboxRaw = readText(new File(dir, "outbox.json"));
        if (outboxRaw != null) {
            try {
                Json.Arr arr = Json.parseArray(outboxRaw);
                for (int i = 0; i < arr.length(); i++) {
                    Json.Obj o = arr.optObject(i);
                    if (o != null) state.outbox.add(o);
                }
                if (!state.outbox.isEmpty() && state.status == HubSyncController.SyncStatus.UNSUPPORTED) {
                    state.status = HubSyncController.SyncStatus.OFFLINE_DEGRADED;
                }
            } catch (Json.SyntaxException ignored) {}
        }
        String etagsRaw = readText(new File(dir, "cover_etags.json"));
        if (etagsRaw != null) {
            try {
                Json.Obj obj = Json.parseObject(etagsRaw);
                java.util.Iterator<String> keys = obj.keys();
                while (keys.hasNext()) {
                    String k = keys.next();
                    try {
                        String clean = normalizeEtag(obj.optString(k));
                        if (!clean.isEmpty()) state.coverEtags.put(Integer.parseInt(k), clean);
                    } catch (NumberFormatException ignored) {}
                }
            } catch (Json.SyntaxException ignored) {}
        }
        String conflictRaw = readText(new File(dir, "conflict.json"));
        if (conflictRaw != null) {
            try {
                state.pendingConflict = Json.parseObject(conflictRaw);
                state.status = HubSyncController.SyncStatus.CONFLICT_PENDING;
            } catch (Json.SyntaxException ignored) {}
        }
        String errRaw = readText(new File(dir, "last_error.txt"));
        if (errRaw != null && !errRaw.isEmpty()) {
            state.lastError = errRaw;
        }
        return state;
    }

    static void saveToDisk(File filesDir, String hostUuid, HubSyncController.HostState state) {
        File dir = hostDir(filesDir, hostUuid);
        if (state.snapshot != null) writeAtomicText(new File(dir, "snapshot.json"), state.snapshot.toString());
        if (state.snapshotEtag != null && !state.snapshotEtag.isEmpty()) {
            writeAtomicText(new File(dir, "snapshot_etag.txt"), state.snapshotEtag);
        }
        Json.Arr outboxArr = Json.arr();
        for (Json.Obj op : state.outbox) outboxArr.add(op);
        writeAtomicText(new File(dir, "outbox.json"), outboxArr.toString());
        Json.Obj etagsObj = Json.obj();
        for (Map.Entry<Integer, String> entry : state.coverEtags.entrySet()) {
            String clean = normalizeEtag(entry.getValue());
            if (!clean.isEmpty()) etagsObj.put(Integer.toString(entry.getKey()), clean);
        }
        writeAtomicText(new File(dir, "cover_etags.json"), etagsObj.toString());
        File conflictFile = new File(dir, "conflict.json");
        if (state.pendingConflict != null) writeAtomicText(conflictFile, state.pendingConflict.toString());
        else if (conflictFile.exists()) conflictFile.delete();
        File errFile = new File(dir, "last_error.txt");
        if (state.lastError != null && !state.lastError.isEmpty()) writeAtomicText(errFile, state.lastError);
        else if (errFile.exists()) errFile.delete();
    }

    static void upsertOutboxPut(List<Json.Obj> outbox, String id, long baseRev, Json.Obj base, Json.Obj write) {
        for (int i = 0; i < outbox.size(); i++) {
            Json.Obj existing = outbox.get(i);
            if (id.equals(existing.optString("id"))) {
                Json.Obj updated = Json.obj()
                        .put("op", "put")
                        .put("id", id)
                        .put("base_revision", existing.optLong("base_revision", baseRev))
                        .put("base", existing.optObject("base") != null ? existing.optObject("base") : base)
                        .put("write", write);
                outbox.set(i, updated);
                return;
            }
        }
        outbox.add(Json.obj().put("op", "put").put("id", id).put("base_revision", baseRev)
                .put("base", base).put("write", write));
    }

    static Json.Obj findProfileInSnapshot(Json.Obj snapshot, String id) {
        if (snapshot == null) return null;
        Json.Arr profiles = snapshot.optArray("profiles");
        if (profiles == null) return null;
        for (int i = 0; i < profiles.length(); i++) {
            Json.Obj p = profiles.optObject(i);
            if (p != null && id.equals(p.optString("id"))) return p;
        }
        return null;
    }

    static void applyProfileIntoSnapshot(HubSyncController.HostState state, Json.Obj profile) {
        if (state.snapshot == null || profile == null) return;
        Json.Arr profiles = state.snapshot.optArray("profiles");
        if (profiles == null) {
            profiles = Json.arr();
            state.snapshot.put("profiles", profiles);
        }
        String id = profile.optString("id");
        Json.Arr rebuilt = Json.arr();
        boolean replaced = false;
        for (int i = 0; i < profiles.length(); i++) {
            Json.Obj p = profiles.optObject(i);
            if (p != null && id.equals(p.optString("id"))) {
                rebuilt.add(profile);
                replaced = true;
            } else if (p != null) {
                rebuilt.add(p);
            }
        }
        if (!replaced) rebuilt.add(profile);
        state.snapshot.put("profiles", rebuilt);
    }

    static void removeProfileFromSnapshot(HubSyncController.HostState state, String id) {
        if (state.snapshot == null) return;
        Json.Arr profiles = state.snapshot.optArray("profiles");
        if (profiles == null) return;
        Json.Arr rebuilt = Json.arr();
        for (int i = 0; i < profiles.length(); i++) {
            Json.Obj p = profiles.optObject(i);
            if (p != null && !id.equals(p.optString("id"))) rebuilt.add(p);
        }
        state.snapshot.put("profiles", rebuilt);
    }

    private static String readText(File file) {
        if (!file.isFile()) return null;
        try (FileInputStream in = new FileInputStream(file)) {
            ByteArrayOutputStream out = new ByteArrayOutputStream();
            byte[] buf = new byte[4096];
            int n;
            while ((n = in.read(buf)) >= 0) out.write(buf, 0, n);
            return out.toString(StandardCharsets.UTF_8.name());
        } catch (IOException e) {
            return null;
        }
    }

    private static void writeAtomicText(File target, String text) {
        File parent = target.getParentFile();
        if (parent != null && !parent.exists()) parent.mkdirs();
        File tmp = new File(parent, target.getName() + ".tmp");
        try (FileOutputStream out = new FileOutputStream(tmp)) {
            out.write(text.getBytes(StandardCharsets.UTF_8));
        } catch (IOException e) {
            return;
        }
        if (!tmp.renameTo(target)) {
            target.delete();
            if (!tmp.renameTo(target)) tmp.delete();
        }
    }
}
