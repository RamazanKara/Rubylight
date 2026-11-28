package com.limelight.nativeapi;

import android.content.Context;

import com.limelight.LimeLog;
import com.limelight.binding.PlatformBinding;
import com.limelight.computers.IdentityManager;
import com.limelight.utils.CacheHelper;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.jni.Json;
import com.limelight.nvstream.jni.NightjarJni;

import java.io.ByteArrayOutputStream;
import java.io.File;
import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.Collections;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

/**
 * Offline-first Host Hub Sync controller for Android: manages disk-cached snapshots, outbox
 * replay with 3-way conflict resolution via {@link NightjarJni#hubSyncPolicy}, and ETag SHA-256
 * cover art synchronization into {@link com.limelight.grid.assets.DiskAssetLoader}'s {@code boxart}
 * cache whenever a host advertises {@link HostCapabilities.Feature#HUB_SYNC}.
 */
public final class HubSyncController {
    public enum SyncStatus {
        SYNCED,
        OFFLINE_DEGRADED,
        CONFLICT_PENDING,
        PERMISSION_DENIED,
        UNSUPPORTED
    }

    /** Abstraction over JNI native API & pure Rust policy so Robolectric tests run offline. */
    public interface HubBackend {
        Json.Obj fetchSnapshot(String ifNoneMatch, boolean allDevices) throws NativeApiException;
        Json.Obj putProfile(String id, String ifMatch, Json.Obj write) throws NativeApiException;
        Json.Obj deleteProfile(String id, String ifMatch) throws NativeApiException;
        Json.Obj fetchCoverToFile(int appId, String ifNoneMatch, File destFile) throws NativeApiException;
        Json.Obj mergeThreeWay(Json.Obj base, Json.Obj local, Json.Obj remote) throws NativeApiException;
        Json.Arr coversToFetch(Json.Arr remoteCovers, Json.Obj localEtags) throws NativeApiException;
    }

    public interface ClientFactory {
        NativeApiClient create(ComputerDetails details);
    }

    /** In-memory & persisted Hub Sync state for one paired host. */
    public static final class HostState {
        public SyncStatus status = SyncStatus.UNSUPPORTED;
        public Json.Obj snapshot;
        public String snapshotEtag;
        public final List<Json.Obj> outbox = new ArrayList<>();
        public final Map<Integer, String> coverEtags = new LinkedHashMap<>();
        public Json.Obj pendingConflict;
        public String lastError;
    }

    private static final String DIR_NAME = "hub_sync";
    private static final ConcurrentHashMap<String, HostState> STATES = new ConcurrentHashMap<>();
    private static final ConcurrentHashMap<String, Object> HOST_LOCKS = new ConcurrentHashMap<>();
    private static final ExecutorService EXECUTOR =
            Executors.newSingleThreadExecutor(r -> new Thread(r, "HubSync"));

    private HubSyncController() {}

    /** Native JNI-backed implementation delegating to {@link NativeApiClient} and {@link NightjarJni}. */
    public static HubBackend nativeBackend(NativeApiClient client) {
        return new HubBackend() {
            @Override
            public Json.Obj fetchSnapshot(String ifNoneMatch, boolean allDevices) throws NativeApiException {
                Json.Obj arg = Json.obj().put("all_devices", allDevices);
                if (ifNoneMatch != null) arg.put("if_none_match", ifNoneMatch);
                return client.callJson("hub.snapshot", arg.toString());
            }

            @Override
            public Json.Obj putProfile(String id, String ifMatch, Json.Obj write) throws NativeApiException {
                Json.Obj arg = Json.obj().put("id", id).put("if_match", ifMatch).put("write", write);
                return client.callJson("hub.profile.put", arg.toString());
            }

            @Override
            public Json.Obj deleteProfile(String id, String ifMatch) throws NativeApiException {
                Json.Obj arg = Json.obj().put("id", id).put("if_match", ifMatch);
                return client.callJson("hub.profile.delete", arg.toString());
            }

            @Override
            public Json.Obj fetchCoverToFile(int appId, String ifNoneMatch, File destFile) throws NativeApiException {
                Json.Obj arg = Json.obj().put("app_id", appId).put("dest_path", destFile.getAbsolutePath());
                if (ifNoneMatch != null) arg.put("if_none_match", ifNoneMatch);
                return client.callJson("hub.cover.to_file", arg.toString());
            }

            @Override
            public Json.Obj mergeThreeWay(Json.Obj base, Json.Obj local, Json.Obj remote) throws NativeApiException {
                Json.Obj arg = Json.obj().put("base", base).put("local", local).put("remote", remote);
                return NativeApiClient.decode(NightjarJni.hubSyncPolicy("merge_three_way", arg.toString()));
            }

            @Override
            public Json.Arr coversToFetch(Json.Arr remoteCovers, Json.Obj localEtags) throws NativeApiException {
                Json.Obj arg = Json.obj().put("remote", remoteCovers).put("local_etags", localEtags);
                return NativeApiClient.decode(NightjarJni.hubSyncPolicy("covers_to_fetch", arg.toString())).getArray("app_ids");
            }
        };
    }

    /** Builds a live {@link HubBackend} for {@code details}, or returns {@code null} if unpaired/offline. */
    public static HubBackend backendFor(Context context, ComputerDetails details) {
        if (context == null || details == null) return null;
        HostEndpoint endpoint = HostEndpoint.of(details, new IdentityManager(context).getUniqueId(),
                PlatformBinding.getCryptoProvider(context));
        return endpoint == null ? null : nativeBackend(new NativeApiClient(new JniNativeApiTransport(), endpoint));
    }

    /** Resolves a conflict off the UI thread so network writes never block Android's main looper. */
    public static void resolveConflictAsync(Context context, String hostUuid, String choice, HubBackend backend) {
        if (context == null || hostUuid == null) return;
        final Context appCtx = context.getApplicationContext();
        EXECUTOR.execute(() -> resolveConflict(appCtx, hostUuid, choice, backend));
    }

    /** Called by {@link com.limelight.computers.ComputerManagerService} whenever a PC polls online. */
    public static void onHostOnline(Context context, ComputerDetails details, ClientFactory clients) {
        if (context == null || details == null || details.uuid == null || details.caps == null) return;
        if (!details.caps.has(HostCapabilities.Feature.HUB_SYNC)) return;
        final Context appCtx = context.getApplicationContext();
        EXECUTOR.execute(() -> {
            NativeApiClient client = clients.create(details);
            if (client != null) {
                syncOnce(appCtx, details.uuid, nativeBackend(client), Collections.emptyMap());
            }
        });
    }

    /** Removes persisted Hub Sync state when a PC is deleted/forgotten. */
    public static void forget(Context context, String hostUuid) {
        if (hostUuid == null) return;
        STATES.remove(hostUuid);
        if (context == null) return;
        File dir = hostDir(context.getFilesDir(), hostUuid);
        File[] files = dir.listFiles();
        if (files != null) {
            for (File f : files) f.delete();
        }
        dir.delete();
    }

    /** Reads cached state (from memory or disk) without hitting the network. */
    public static HostState stateFor(Context context, String hostUuid) {
        if (hostUuid == null || hostUuid.isEmpty()) return new HostState();
        synchronized (lockFor(hostUuid)) {
            HostState loaded = loadFromDisk(context.getFilesDir(), hostUuid);
            STATES.put(hostUuid, loaded);
            return loaded;
        }
    }

    /** Queues a profile write offline-safely and attempts sync if a backend is supplied. */
    public static HostState queueProfilePut(Context context, String hostUuid, String id, long baseRevision,
                                            Json.Obj baseProfile, Json.Obj write, HubBackend backend) {
        synchronized (lockFor(hostUuid)) {
            HostState state = loadFromDisk(context.getFilesDir(), hostUuid);
            upsertOutboxPut(state.outbox, id, baseRevision, baseProfile, write);
            if (state.status == SyncStatus.UNSUPPORTED) state.status = SyncStatus.OFFLINE_DEGRADED;
            saveToDisk(context.getFilesDir(), hostUuid, state);
            STATES.put(hostUuid, state);
        }
        if (backend != null) {
            return syncOnce(context, hostUuid, backend, Collections.emptyMap());
        }
        return stateFor(context, hostUuid);
    }

    /** Resolves a pending 3-way conflict ({@code keep_mine}, {@code use_host}, or {@code save_copy}). */
    public static HostState resolveConflict(Context context, String hostUuid, String choice, HubBackend backend) {
        synchronized (lockFor(hostUuid)) {
            HostState state = loadFromDisk(context.getFilesDir(), hostUuid);
            Json.Obj conflict = state.pendingConflict;
            if (conflict == null) return state;
            String id = conflict.getString("id");
            state.outbox.removeIf(op -> id.equals(op.optString("id")));
            state.pendingConflict = null;
            if ("keep_mine".equals(choice)) {
                state.outbox.add(0, Json.obj()
                        .put("op", "put")
                        .put("id", id)
                        .put("base_revision", conflict.optLong("server_revision", 0L))
                        .put("base", conflict.optObject("remote"))
                        .put("write", conflict.getObject("local")));
            } else if ("save_copy".equals(choice)) {
                Json.Obj copyWrite = Json.parseObject(conflict.getObject("local").toString());
                copyWrite.put("name", copyWrite.optString("name", "Profile") + " (copy)");
                String suffix = Long.toString(System.currentTimeMillis() % 100000L);
                String prefix = id.length() > 50 ? id.substring(0, 50) : id;
                state.outbox.add(Json.obj()
                        .put("op", "put")
                        .put("id", prefix + "-" + suffix)
                        .put("base_revision", 0L)
                        .put("write", copyWrite));
            } else if ("use_host".equals(choice)) {
                applyProfileIntoSnapshot(state, conflict.optObject("remote"));
            }
            state.status = state.outbox.isEmpty() ? SyncStatus.SYNCED : SyncStatus.OFFLINE_DEGRADED;
            saveToDisk(context.getFilesDir(), hostUuid, state);
            STATES.put(hostUuid, state);
        }
        if (backend != null) {
            return syncOnce(context, hostUuid, backend, Collections.emptyMap());
        }
        return stateFor(context, hostUuid);
    }

    /** Executes one synchronous Hub Sync pass (snapshot -> outbox replay -> changed covers). */
    public static HostState syncOnce(Context context, String hostUuid, HubBackend backend, Map<Integer, NvApp> knownApps) {
        synchronized (lockFor(hostUuid)) {
            File filesDir = context.getFilesDir();
            File cacheDir = context.getCacheDir();
            HostState state = loadFromDisk(filesDir, hostUuid);
            try {
                Json.Obj snapReply = backend.fetchSnapshot(state.snapshotEtag, false);
                if ("ok".equals(snapReply.optString("status"))) {
                    state.snapshot = snapReply.getObject("snapshot");
                    state.snapshotEtag = snapReply.optString("etag", "\"rev:" + state.snapshot.optLong("snapshot_revision", 0L) + "\"");
                }
                state.lastError = null;
                state.status = state.pendingConflict != null ? SyncStatus.CONFLICT_PENDING : SyncStatus.SYNCED;
            } catch (NativeApiException e) {
                if (isPermissionError(e)) {
                    state.status = SyncStatus.PERMISSION_DENIED;
                    state.lastError = e.getMessage();
                    saveToDisk(filesDir, hostUuid, state);
                    STATES.put(hostUuid, state);
                    return state;
                }
                state.status = SyncStatus.OFFLINE_DEGRADED;
                state.lastError = e.getMessage();
                saveToDisk(filesDir, hostUuid, state);
                STATES.put(hostUuid, state);
                return state;
            }

            if (!replayOutbox(state, backend)) {
                saveToDisk(filesDir, hostUuid, state);
                STATES.put(hostUuid, state);
                return state;
            }

            syncCovers(cacheDir, hostUuid, state, backend, knownApps);
            if (state.pendingConflict != null) state.status = SyncStatus.CONFLICT_PENDING;
            else if (state.status != SyncStatus.PERMISSION_DENIED) state.status = SyncStatus.SYNCED;
            saveToDisk(filesDir, hostUuid, state);
            STATES.put(hostUuid, state);
            return state;
        }
    }

    private static boolean replayOutbox(HostState state, HubBackend backend) {
        while (!state.outbox.isEmpty()) {
            Json.Obj op = state.outbox.get(0);
            String kind = op.optString("op", "put");
            String id = op.getString("id");
            String ifMatch = Long.toString(op.optLong("base_revision", 0L));
            try {
                if ("delete".equals(kind)) {
                    Json.Obj reply = backend.deleteProfile(id, ifMatch);
                    if ("ok".equals(reply.optString("status"))) {
                        removeProfileFromSnapshot(state, id);
                        state.outbox.remove(0);
                        continue;
                    }
                    state.outbox.remove(0);
                    continue;
                }
                Json.Obj write = op.getObject("write");
                Json.Obj reply = backend.putProfile(id, ifMatch, write);
                String status = reply.optString("status");
                if ("ok".equals(status)) {
                    applyProfileIntoSnapshot(state, reply.optObject("profile"));
                    state.outbox.remove(0);
                    continue;
                }
                if ("conflict".equals(status)) {
                    Json.Obj fresh = backend.fetchSnapshot(null, false);
                    if ("ok".equals(fresh.optString("status"))) {
                        state.snapshot = fresh.getObject("snapshot");
                        state.snapshotEtag = fresh.optString("etag", state.snapshotEtag);
                    }
                    Json.Obj remote = findProfileInSnapshot(state.snapshot, id);
                    if (remote == null) {
                        state.pendingConflict = Json.obj().put("id", id).put("server_revision", 0L)
                                .put("base", op.optObject("base")).put("local", write)
                                .put("conflicting_fields", Json.arr().add("deleted_on_host"));
                        state.status = SyncStatus.CONFLICT_PENDING;
                        return false;
                    }
                    Json.Obj merge = backend.mergeThreeWay(op.optObject("base"), write, remote);
                    if ("merged".equals(merge.optString("status"))) {
                        Json.Obj mergedWrite = merge.getObject("write");
                        Json.Obj retry = backend.putProfile(id, Long.toString(remote.optLong("revision", 0L)), mergedWrite);
                        if ("ok".equals(retry.optString("status"))) {
                            applyProfileIntoSnapshot(state, retry.optObject("profile"));
                            state.outbox.remove(0);
                            continue;
                        }
                    } else if ("conflict".equals(merge.optString("status"))) {
                        state.pendingConflict = merge.getObject("conflict");
                        state.status = SyncStatus.CONFLICT_PENDING;
                        return false;
                    }
                    return false;
                }
            } catch (NativeApiException e) {
                if (isPermissionError(e)) {
                    state.status = SyncStatus.PERMISSION_DENIED;
                    state.lastError = e.getMessage();
                    return false;
                }
                state.status = SyncStatus.OFFLINE_DEGRADED;
                state.lastError = e.getMessage();
                return false;
            }
        }
        return true;
    }

    private static void syncCovers(File cacheDir, String hostUuid, HostState state,
                                   HubBackend backend, Map<Integer, NvApp> knownApps) {
        if (state.snapshot == null) return;
        Json.Arr remoteCovers = state.snapshot.optArray("covers");
        if (remoteCovers == null || remoteCovers.length() == 0) return;
        try {
            Json.Obj localEtagsJson = Json.obj();
            for (Map.Entry<Integer, String> entry : state.coverEtags.entrySet()) {
                localEtagsJson.put(Integer.toString(entry.getKey()), entry.getValue());
            }
            Json.Arr toFetch = backend.coversToFetch(remoteCovers, localEtagsJson);
            for (int i = 0; i < toFetch.length(); i++) {
                int appId = toFetch.getInt(i);
                NvApp app = knownApps == null ? null : knownApps.get(appId);
                String key = app != null ? app.getAssetCacheKey() : Integer.toString(appId);
                File dest = CacheHelper.openPath(true, cacheDir, "boxart", hostUuid, key + ".png");
                Json.Obj reply = backend.fetchCoverToFile(appId, state.coverEtags.get(appId), dest);
                if ("ok".equals(reply.optString("status"))) {
                    String newEtag = reply.optString("etag", null);
                    if (newEtag != null && !newEtag.isEmpty()) {
                        state.coverEtags.put(appId, newEtag);
                    }
                }
            }
        } catch (NativeApiException e) {
            LimeLog.warning("HubSync cover fetch deferred: " + e.getMessage());
        }
    }

    private static boolean isPermissionError(NativeApiException e) {
        String code = e.getCode();
        return e.getHttpStatus() == 403
                || "permission_required".equals(code)
                || "not_paired".equals(code)
                || "host_control_required".equals(code);
    }

    private static void upsertOutboxPut(List<Json.Obj> outbox, String id, long baseRev, Json.Obj base, Json.Obj write) {
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

    private static Json.Obj findProfileInSnapshot(Json.Obj snapshot, String id) {
        if (snapshot == null) return null;
        Json.Arr profiles = snapshot.optArray("profiles");
        if (profiles == null) return null;
        for (int i = 0; i < profiles.length(); i++) {
            Json.Obj p = profiles.optObject(i);
            if (p != null && id.equals(p.optString("id"))) return p;
        }
        return null;
    }

    private static void applyProfileIntoSnapshot(HostState state, Json.Obj profile) {
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

    private static void removeProfileFromSnapshot(HostState state, String id) {
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

    private static Object lockFor(String hostUuid) {
        return HOST_LOCKS.computeIfAbsent(hostUuid, k -> new Object());
    }

    private static File hostDir(File filesDir, String hostUuid) {
        String safe = hostUuid.replaceAll("[^A-Za-z0-9_-]", "_");
        return new File(new File(filesDir, DIR_NAME), safe.isEmpty() ? "unknown" : safe);
    }

    private static HostState loadFromDisk(File filesDir, String hostUuid) {
        HostState state = new HostState();
        File dir = hostDir(filesDir, hostUuid);
        String snapRaw = readText(new File(dir, "snapshot.json"));
        if (snapRaw != null) {
            try {
                state.snapshot = Json.parseObject(snapRaw);
                state.snapshotEtag = "\"rev:" + state.snapshot.optLong("snapshot_revision", 0L) + "\"";
                state.status = SyncStatus.OFFLINE_DEGRADED;
            } catch (Json.SyntaxException ignored) {}
        }
        String outboxRaw = readText(new File(dir, "outbox.json"));
        if (outboxRaw != null) {
            try {
                Json.Arr arr = Json.parseArray(outboxRaw);
                for (int i = 0; i < arr.length(); i++) {
                    Json.Obj o = arr.optObject(i);
                    if (o != null) state.outbox.add(o);
                }
                if (!state.outbox.isEmpty() && state.status == SyncStatus.UNSUPPORTED) {
                    state.status = SyncStatus.OFFLINE_DEGRADED;
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
                        state.coverEtags.put(Integer.parseInt(k), obj.optString(k));
                    } catch (NumberFormatException ignored) {}
                }
            } catch (Json.SyntaxException ignored) {}
        }
        String conflictRaw = readText(new File(dir, "conflict.json"));
        if (conflictRaw != null) {
            try {
                state.pendingConflict = Json.parseObject(conflictRaw);
                state.status = SyncStatus.CONFLICT_PENDING;
            } catch (Json.SyntaxException ignored) {}
        }
        return state;
    }

    private static void saveToDisk(File filesDir, String hostUuid, HostState state) {
        File dir = hostDir(filesDir, hostUuid);
        if (state.snapshot != null) writeAtomicText(new File(dir, "snapshot.json"), state.snapshot.toString());
        Json.Arr outboxArr = Json.arr();
        for (Json.Obj op : state.outbox) outboxArr.add(op);
        writeAtomicText(new File(dir, "outbox.json"), outboxArr.toString());
        Json.Obj etagsObj = Json.obj();
        for (Map.Entry<Integer, String> entry : state.coverEtags.entrySet()) {
            etagsObj.put(Integer.toString(entry.getKey()), entry.getValue());
        }
        writeAtomicText(new File(dir, "cover_etags.json"), etagsObj.toString());
        File conflictFile = new File(dir, "conflict.json");
        if (state.pendingConflict != null) writeAtomicText(conflictFile, state.pendingConflict.toString());
        else if (conflictFile.exists()) conflictFile.delete();
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
