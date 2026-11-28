package com.limelight.nativeapi;

import android.content.Context;

import com.limelight.LimeLog;
import com.limelight.binding.PlatformBinding;
import com.limelight.computers.HostCapabilityCache;
import com.limelight.computers.IdentityManager;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.http.PairingManager;
import com.limelight.nvstream.jni.Json;
import com.limelight.nvstream.jni.NightjarJni;
import com.limelight.utils.CacheHelper;

import org.json.JSONException;

import java.io.File;
import java.util.ArrayList;
import java.util.Collections;
import java.util.HashMap;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.atomic.AtomicLong;

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

    private static final class PendingTrigger {
        final Context appContext;
        final ComputerDetails details;
        final ClientFactory clients;

        PendingTrigger(Context appContext, ComputerDetails details, ClientFactory clients) {
            this.appContext = appContext;
            this.details = details;
            this.clients = clients;
        }
    }

    private enum StepKind { OK, DROP_PERMANENT, CONFLICT, PERMISSION_DENIED, OFFLINE }

    private static final class ReplayStep {
        final StepKind kind;
        final Json.Obj freshSnapshot;
        final String freshSnapshotEtag;
        final Json.Obj savedProfile;
        final String deletedId;
        final Json.Obj conflict;
        final String error;

        ReplayStep(StepKind kind, Json.Obj freshSnapshot, String freshSnapshotEtag,
                   Json.Obj savedProfile, String deletedId, Json.Obj conflict, String error) {
            this.kind = kind;
            this.freshSnapshot = freshSnapshot;
            this.freshSnapshotEtag = freshSnapshotEtag;
            this.savedProfile = savedProfile;
            this.deletedId = deletedId;
            this.conflict = conflict;
            this.error = error;
        }
    }

    private static final long SYNC_COOLDOWN_MS = 30_000L;
    private static final ConcurrentHashMap<String, HostState> STATES = new ConcurrentHashMap<>();
    private static final ConcurrentHashMap<String, Object> HOST_LOCKS = new ConcurrentHashMap<>();
    private static final ConcurrentHashMap<String, AtomicLong> FORGET_EPOCHS = new ConcurrentHashMap<>();
    private static final Set<String> SYNC_INFLIGHT = ConcurrentHashMap.newKeySet();
    private static final ConcurrentHashMap<String, Long> LAST_SYNC_MS = new ConcurrentHashMap<>();
    private static final ConcurrentHashMap<String, PendingTrigger> AWAITING_CAPS = new ConcurrentHashMap<>();
    private static final ExecutorService EXECUTOR =
            Executors.newSingleThreadExecutor(r -> new Thread(r, "HubSync"));

    static {
        KnownCapabilities.setListener(HubSyncController::onCapabilitiesRecorded);
    }

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

    /** True when {@code details}' combined `/serverinfo` + cached native capabilities advertise {@code hub.sync}. */
    public static boolean hasHubSync(ComputerDetails details) {
        if (details == null || details.caps == null) return false;
        if (details.caps.has(HostCapabilities.Feature.HUB_SYNC)) return true;
        String known = KnownCapabilities.of(details.uuid);
        if (known == null) return false;
        try {
            return details.caps.withNativeCapabilities(known).has(HostCapabilities.Feature.HUB_SYNC);
        } catch (JSONException e) {
            LimeLog.warning("Unreadable native capabilities of " + details.uuid + ": " + e.getMessage());
            return false;
        }
    }

    static String normalizeEtag(String etag) {
        return HubSyncStore.normalizeEtag(etag);
    }

    /** Builds a live {@link HubBackend} for {@code details}, or returns {@code null} if unpaired/offline. */
    public static HubBackend backendFor(Context context, ComputerDetails details) {
        if (context == null || details == null) return null;
        HostEndpoint endpoint = HostEndpoint.of(details, new IdentityManager(context).getUniqueId(),
                PlatformBinding.getCryptoProvider(context));
        return endpoint == null ? null : nativeBackend(new NativeApiClient(new JniNativeApiTransport(), endpoint));
    }

    /** Resolves cached {@link NvApp} entries keyed by app ID so covers match {@link NvApp#getAssetCacheKey}. */
    public static Map<Integer, NvApp> loadKnownApps(Context context, ComputerDetails details) {
        if (context == null || details == null || details.uuid == null) return Collections.emptyMap();
        List<NvApp> apps = new CachedAppListSource(context.getCacheDir()).apps(details);
        if (apps == null || apps.isEmpty()) return Collections.emptyMap();
        Map<Integer, NvApp> byId = new HashMap<>(apps.size());
        for (NvApp app : apps) {
            if (app != null && app.isInitialized()) byId.put(app.getAppId(), app);
        }
        return byId;
    }

    private static Map<Integer, NvApp> loadKnownApps(Context context, String hostUuid) {
        if (context == null || hostUuid == null || hostUuid.isEmpty()) return Collections.emptyMap();
        ComputerDetails stub = new ComputerDetails();
        stub.uuid = hostUuid;
        stub.caps = HostCapabilityCache.load(context, hostUuid);
        return loadKnownApps(context, stub);
    }

    /** Resolves a conflict off the UI thread so network writes never block Android's main looper. */
    public static void resolveConflictAsync(Context context, String hostUuid, String choice, HubBackend backend) {
        if (context == null || hostUuid == null) return;
        final Context appCtx = context.getApplicationContext();
        EXECUTOR.execute(() -> resolveConflict(appCtx, hostUuid, choice, backend));
    }

    /** Called by {@link com.limelight.computers.ComputerManagerService} whenever a PC polls online. */
    public static void onHostOnline(Context context, ComputerDetails details, ClientFactory clients) {
        if (context == null || details == null || details.uuid == null || details.caps == null || clients == null) {
            return;
        }
        if (details.pairState == PairingManager.PairState.NOT_PAIRED) return;
        final Context appCtx = context.getApplicationContext();
        final String uuid = details.uuid;
        if (!hasHubSync(details)) {
            if (KnownCapabilities.of(uuid) == null && details.caps.has(HostCapabilities.Feature.NATIVE_API)) {
                AWAITING_CAPS.put(uuid, new PendingTrigger(appCtx, new ComputerDetails(details), clients));
            }
            return;
        }
        AWAITING_CAPS.remove(uuid);
        HostState cached = STATES.get(uuid);
        boolean pendingWork = cached != null && !cached.outbox.isEmpty();
        long now = System.currentTimeMillis();
        Long last = LAST_SYNC_MS.get(uuid);
        if (!pendingWork && last != null && (now - last) < SYNC_COOLDOWN_MS) return;
        if (!SYNC_INFLIGHT.add(uuid)) return;
        LAST_SYNC_MS.put(uuid, now);
        final ComputerDetails snapshot = new ComputerDetails(details);
        EXECUTOR.execute(() -> {
            try {
                NativeApiClient client = clients.create(snapshot);
                if (client != null) {
                    syncOnce(appCtx, uuid, nativeBackend(client), loadKnownApps(appCtx, snapshot));
                }
            } finally {
                SYNC_INFLIGHT.remove(uuid);
            }
        });
    }

    private static void onCapabilitiesRecorded(String hostUuid) {
        PendingTrigger pending = hostUuid == null ? null : AWAITING_CAPS.remove(hostUuid);
        if (pending != null) {
            onHostOnline(pending.appContext, pending.details, pending.clients);
        }
    }

    /** Removes persisted Hub Sync state when a PC is deleted/forgotten. */
    public static void forget(Context context, String hostUuid) {
        if (hostUuid == null) return;
        synchronized (lockFor(hostUuid)) {
            epochCounter(hostUuid).incrementAndGet();
            STATES.remove(hostUuid);
            AWAITING_CAPS.remove(hostUuid);
            SYNC_INFLIGHT.remove(hostUuid);
            LAST_SYNC_MS.remove(hostUuid);
            if (context != null) HubSyncStore.deleteHostDir(context.getFilesDir(), hostUuid);
        }
    }

    /** Reads cached state (from memory or disk) without hitting the network. */
    public static HostState stateFor(Context context, String hostUuid) {
        if (hostUuid == null || hostUuid.isEmpty()) return new HostState();
        synchronized (lockFor(hostUuid)) {
            HostState loaded = HubSyncStore.loadFromDisk(context.getFilesDir(), hostUuid);
            STATES.put(hostUuid, loaded);
            return loaded;
        }
    }

    /** Queues a profile write offline-safely and attempts sync if a backend is supplied. */
    public static HostState queueProfilePut(Context context, String hostUuid, String id, long baseRevision,
                                            Json.Obj baseProfile, Json.Obj write, HubBackend backend) {
        synchronized (lockFor(hostUuid)) {
            HostState state = HubSyncStore.loadFromDisk(context.getFilesDir(), hostUuid);
            HubSyncStore.upsertOutboxPut(state.outbox, id, baseRevision, baseProfile, write);
            if (state.status == SyncStatus.UNSUPPORTED) state.status = SyncStatus.OFFLINE_DEGRADED;
            HubSyncStore.saveToDisk(context.getFilesDir(), hostUuid, state);
            STATES.put(hostUuid, state);
        }
        if (backend != null) {
            return syncOnce(context, hostUuid, backend, loadKnownApps(context, hostUuid));
        }
        return stateFor(context, hostUuid);
    }

    /** Resolves a pending 3-way conflict ({@code keep_mine}, {@code use_host}, or {@code save_copy}). */
    public static HostState resolveConflict(Context context, String hostUuid, String choice, HubBackend backend) {
        synchronized (lockFor(hostUuid)) {
            HostState state = HubSyncStore.loadFromDisk(context.getFilesDir(), hostUuid);
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
                copyWrite.put("name", HubSyncStore.cappedCopyName(copyWrite.optString("name", "Profile")));
                String suffix = Long.toString(System.currentTimeMillis() % 100000L);
                String prefix = id.length() > 50 ? id.substring(0, 50) : id;
                state.outbox.add(Json.obj()
                        .put("op", "put")
                        .put("id", prefix + "-" + suffix)
                        .put("base_revision", 0L)
                        .put("write", copyWrite));
            } else if ("use_host".equals(choice)) {
                HubSyncStore.applyProfileIntoSnapshot(state, conflict.optObject("remote"));
            }
            state.status = state.outbox.isEmpty() ? SyncStatus.SYNCED : SyncStatus.OFFLINE_DEGRADED;
            HubSyncStore.saveToDisk(context.getFilesDir(), hostUuid, state);
            STATES.put(hostUuid, state);
        }
        if (backend != null) {
            return syncOnce(context, hostUuid, backend, loadKnownApps(context, hostUuid));
        }
        return stateFor(context, hostUuid);
    }

    /** Executes one Hub Sync pass (snapshot -> outbox replay -> changed covers) without holding locks over I/O. */
    public static HostState syncOnce(Context context, String hostUuid, HubBackend backend, Map<Integer, NvApp> knownApps) {
        final File filesDir = context.getFilesDir();
        final File cacheDir = context.getCacheDir();
        final Object lock = lockFor(hostUuid);
        final long startEpoch = epochCounter(hostUuid).get();

        String initialEtag;
        synchronized (lock) {
            initialEtag = HubSyncStore.loadFromDisk(filesDir, hostUuid).snapshotEtag;
        }

        Json.Obj snapReply;
        try {
            snapReply = backend.fetchSnapshot(initialEtag, false);
        } catch (NativeApiException e) {
            synchronized (lock) {
                if (epochCounter(hostUuid).get() != startEpoch) return new HostState();
                HostState st = HubSyncStore.loadFromDisk(filesDir, hostUuid);
                st.status = isPermissionError(e) ? SyncStatus.PERMISSION_DENIED : SyncStatus.OFFLINE_DEGRADED;
                st.lastError = e.getMessage();
                HubSyncStore.saveToDisk(filesDir, hostUuid, st);
                STATES.put(hostUuid, st);
                return st;
            }
        }

        synchronized (lock) {
            if (epochCounter(hostUuid).get() != startEpoch) return new HostState();
            HostState st = HubSyncStore.loadFromDisk(filesDir, hostUuid);
            if ("ok".equals(snapReply.optString("status"))) {
                st.snapshot = snapReply.getObject("snapshot");
                st.snapshotEtag = snapReply.optString(
                        "etag", "\"rev:" + st.snapshot.optLong("snapshot_revision", 0L) + "\"");
            }
            st.lastError = null;
            st.status = st.pendingConflict != null ? SyncStatus.CONFLICT_PENDING
                    : (!st.outbox.isEmpty() ? SyncStatus.OFFLINE_DEGRADED : SyncStatus.SYNCED);
            HubSyncStore.saveToDisk(filesDir, hostUuid, st);
            STATES.put(hostUuid, st);
        }

        while (true) {
            Json.Obj sentOp;
            synchronized (lock) {
                if (epochCounter(hostUuid).get() != startEpoch) return new HostState();
                HostState st = HubSyncStore.loadFromDisk(filesDir, hostUuid);
                if (st.pendingConflict != null || st.outbox.isEmpty()) break;
                sentOp = Json.parseObject(st.outbox.get(0).toString());
            }

            ReplayStep step = executeOutboxStep(sentOp, backend);
            synchronized (lock) {
                if (epochCounter(hostUuid).get() != startEpoch) return new HostState();
                HostState st = HubSyncStore.loadFromDisk(filesDir, hostUuid);
                if (step.freshSnapshot != null) {
                    st.snapshot = step.freshSnapshot;
                    if (step.freshSnapshotEtag != null) st.snapshotEtag = step.freshSnapshotEtag;
                }
                boolean sameHead = !st.outbox.isEmpty()
                        && sentOp.toString().equals(st.outbox.get(0).toString());
                if (step.kind == StepKind.OK) {
                    if (step.savedProfile != null) HubSyncStore.applyProfileIntoSnapshot(st, step.savedProfile);
                    if (step.deletedId != null) HubSyncStore.removeProfileFromSnapshot(st, step.deletedId);
                    if (sameHead) {
                        st.outbox.remove(0);
                    } else if (step.savedProfile != null && !st.outbox.isEmpty()
                            && sentOp.optString("id").equals(st.outbox.get(0).optString("id"))) {
                        st.outbox.get(0).put("base_revision", step.savedProfile.optLong("revision", 0L));
                        st.outbox.get(0).put("base", step.savedProfile);
                    }
                    HubSyncStore.saveToDisk(filesDir, hostUuid, st);
                    STATES.put(hostUuid, st);
                    continue;
                }
                if (step.kind == StepKind.DROP_PERMANENT) {
                    st.lastError = step.error;
                    if (sameHead) st.outbox.remove(0);
                    HubSyncStore.saveToDisk(filesDir, hostUuid, st);
                    STATES.put(hostUuid, st);
                    continue;
                }
                if (step.kind == StepKind.CONFLICT) {
                    st.pendingConflict = step.conflict;
                    st.status = SyncStatus.CONFLICT_PENDING;
                } else {
                    st.status = step.kind == StepKind.PERMISSION_DENIED
                            ? SyncStatus.PERMISSION_DENIED : SyncStatus.OFFLINE_DEGRADED;
                    st.lastError = step.error;
                }
                HubSyncStore.saveToDisk(filesDir, hostUuid, st);
                STATES.put(hostUuid, st);
                return st;
            }
        }

        Map<Integer, String> downloadedEtags = syncCoversUnlocked(
                cacheDir, hostUuid, filesDir, lock, startEpoch, backend, knownApps);

        synchronized (lock) {
            if (epochCounter(hostUuid).get() != startEpoch) return new HostState();
            HostState st = HubSyncStore.loadFromDisk(filesDir, hostUuid);
            st.coverEtags.putAll(downloadedEtags);
            st.status = st.pendingConflict != null ? SyncStatus.CONFLICT_PENDING
                    : (!st.outbox.isEmpty() ? SyncStatus.OFFLINE_DEGRADED : SyncStatus.SYNCED);
            HubSyncStore.saveToDisk(filesDir, hostUuid, st);
            STATES.put(hostUuid, st);
            return st;
        }
    }

    private static ReplayStep executeOutboxStep(Json.Obj op, HubBackend backend) {
        String kind = op.optString("op", "put");
        String id = op.getString("id");
        String ifMatch = Long.toString(op.optLong("base_revision", 0L));
        try {
            if ("delete".equals(kind)) {
                Json.Obj reply = backend.deleteProfile(id, ifMatch);
                if ("ok".equals(reply.optString("status"))) {
                    return new ReplayStep(StepKind.OK, null, null, null, id, null, null);
                }
                return new ReplayStep(StepKind.OFFLINE, null, null, null, null, null,
                        "Unexpected delete response: " + reply.optString("status"));
            }
            Json.Obj write = op.getObject("write");
            Json.Obj reply = backend.putProfile(id, ifMatch, write);
            String status = reply.optString("status");
            if ("ok".equals(status)) {
                return new ReplayStep(StepKind.OK, null, null, reply.optObject("profile"), null, null, null);
            }
            if (!"conflict".equals(status)) {
                return new ReplayStep(StepKind.OFFLINE, null, null, null, null, null,
                        "Unexpected profile write response: " + status);
            }
            return resolvePutConflictUnlocked(id, op.optObject("base"), write, backend);
        } catch (NativeApiException e) {
            return classifyReplayException(e);
        }
    }

    private static ReplayStep resolvePutConflictUnlocked(String id, Json.Obj base, Json.Obj write,
                                                         HubBackend backend) throws NativeApiException {
        Json.Obj fresh = backend.fetchSnapshot(null, false);
        Json.Obj freshSnap = "ok".equals(fresh.optString("status")) ? fresh.getObject("snapshot") : null;
        String freshEtag = "ok".equals(fresh.optString("status")) ? fresh.optString("etag", null) : null;
        Json.Obj remote = HubSyncStore.findProfileInSnapshot(freshSnap, id);
        if (remote == null) {
            Json.Obj deleted = Json.obj().put("id", id).put("server_revision", 0L)
                    .put("base", base).put("local", write)
                    .put("conflicting_fields", Json.arr().add("deleted_on_host"));
            return new ReplayStep(StepKind.CONFLICT, freshSnap, freshEtag, null, null, deleted, null);
        }
        Json.Obj merge = backend.mergeThreeWay(base, write, remote);
        String mergeStatus = merge.optString("status");
        if ("conflict".equals(mergeStatus)) {
            return new ReplayStep(StepKind.CONFLICT, freshSnap, freshEtag, null, null,
                    merge.getObject("conflict"), null);
        }
        if (!"merged".equals(mergeStatus)) {
            return new ReplayStep(StepKind.OFFLINE, freshSnap, freshEtag, null, null, null,
                    "Unexpected merge status: " + mergeStatus);
        }
        Json.Obj mergedWrite = merge.getObject("write");
        Json.Obj retry = backend.putProfile(id, Long.toString(remote.optLong("revision", 0L)), mergedWrite);
        String retryStatus = retry.optString("status");
        if ("ok".equals(retryStatus)) {
            return new ReplayStep(StepKind.OK, freshSnap, freshEtag, retry.optObject("profile"), null, null, null);
        }
        if ("conflict".equals(retryStatus)) {
            Json.Obj fresh2 = backend.fetchSnapshot(null, false);
            Json.Obj snap2 = "ok".equals(fresh2.optString("status")) ? fresh2.getObject("snapshot") : freshSnap;
            String etag2 = "ok".equals(fresh2.optString("status")) ? fresh2.optString("etag", freshEtag) : freshEtag;
            Json.Obj remote2 = HubSyncStore.findProfileInSnapshot(snap2, id);
            if (remote2 != null) {
                Json.Obj merge2 = backend.mergeThreeWay(remote, mergedWrite, remote2);
                if ("conflict".equals(merge2.optString("status"))) {
                    return new ReplayStep(StepKind.CONFLICT, snap2, etag2, null, null,
                            merge2.getObject("conflict"), null);
                }
            }
            return new ReplayStep(StepKind.OFFLINE, snap2, etag2, null, null, null,
                    "Profile changed again during sync.");
        }
        return new ReplayStep(StepKind.OFFLINE, freshSnap, freshEtag, null, null, null,
                "Unexpected retry status: " + retryStatus);
    }

    private static ReplayStep classifyReplayException(NativeApiException e) {
        if (isPermissionError(e)) {
            return new ReplayStep(StepKind.PERMISSION_DENIED, null, null, null, null, null, e.getMessage());
        }
        if (isPermanentWriteError(e)) {
            return new ReplayStep(StepKind.DROP_PERMANENT, null, null, null, null, null, e.getMessage());
        }
        return new ReplayStep(StepKind.OFFLINE, null, null, null, null, null, e.getMessage());
    }

    private static Map<Integer, String> syncCoversUnlocked(File cacheDir, String hostUuid, File filesDir,
                                                           Object lock, long startEpoch,
                                                           HubBackend backend, Map<Integer, NvApp> knownApps) {
        Json.Arr remoteCovers;
        Map<Integer, String> localEtags;
        synchronized (lock) {
            if (epochCounter(hostUuid).get() != startEpoch) return Collections.emptyMap();
            HostState st = HubSyncStore.loadFromDisk(filesDir, hostUuid);
            if (st.snapshot == null || st.pendingConflict != null || st.status == SyncStatus.PERMISSION_DENIED) {
                return Collections.emptyMap();
            }
            remoteCovers = st.snapshot.optArray("covers");
            localEtags = new LinkedHashMap<>(st.coverEtags);
        }
        if (remoteCovers == null || remoteCovers.length() == 0) return Collections.emptyMap();
        Map<Integer, String> downloaded = new LinkedHashMap<>();
        try {
            Json.Obj localEtagsJson = Json.obj();
            for (Map.Entry<Integer, String> entry : localEtags.entrySet()) {
                localEtagsJson.put(Integer.toString(entry.getKey()), HubSyncStore.normalizeEtag(entry.getValue()));
            }
            Json.Arr toFetch = backend.coversToFetch(remoteCovers, localEtagsJson);
            for (int i = 0; i < toFetch.length(); i++) {
                if (epochCounter(hostUuid).get() != startEpoch) return downloaded;
                int appId = toFetch.getInt(i);
                NvApp app = knownApps == null ? null : knownApps.get(appId);
                if (app == null) continue;
                File dest = CacheHelper.openPath(true, cacheDir, "boxart", hostUuid, app.getAssetCacheKey() + ".png");
                File tmp = new File(dest.getParentFile(), dest.getName() + ".hubtmp");
                Json.Obj reply = backend.fetchCoverToFile(appId, localEtags.get(appId), tmp);
                if ("ok".equals(reply.optString("status"))) {
                    if (tmp.isFile() && !tmp.renameTo(dest)) {
                        dest.delete();
                        if (!tmp.renameTo(dest)) tmp.delete();
                    }
                    String cleanEtag = HubSyncStore.normalizeEtag(reply.optString("etag", null));
                    if (!cleanEtag.isEmpty()) downloaded.put(appId, cleanEtag);
                } else if (tmp.exists()) {
                    tmp.delete();
                }
            }
        } catch (NativeApiException e) {
            LimeLog.warning("HubSync cover fetch deferred: " + e.getMessage());
        }
        return downloaded;
    }

    private static boolean isPermissionError(NativeApiException e) {
        String code = e.getCode();
        return e.getHttpStatus() == 403
                || "permission_required".equals(code)
                || "not_paired".equals(code)
                || "host_control_required".equals(code);
    }

    private static boolean isPermanentWriteError(NativeApiException e) {
        int status = e.getHttpStatus();
        String code = e.getCode();
        return status == 400 || status == 413 || status == 422
                || "malformed_request".equals(code)
                || "too_large".equals(code)
                || "invalid_value".equals(code);
    }

    private static Object lockFor(String hostUuid) {
        return HOST_LOCKS.computeIfAbsent(hostUuid, k -> new Object());
    }

    private static AtomicLong epochCounter(String hostUuid) {
        return FORGET_EPOCHS.computeIfAbsent(hostUuid, k -> new AtomicLong());
    }
}
