package com.limelight.nativeapi;

import android.app.Application;

import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.http.PairingManager;
import com.limelight.nvstream.jni.Json;
import com.limelight.nvstream.jni.NightjarJni;
import com.limelight.utils.CacheHelper;

import org.junit.After;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

import java.io.File;
import java.io.FileOutputStream;
import java.util.Collections;
import java.util.Map;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertTrue;

/**
 * Unit tests for {@link HubSyncController}: offline draft persistence, 3-way conflict handling,
 * {@link KnownCapabilities} activation, {@link NvApp#getAssetCacheKey} cover path, quoted ETag
 * normalization, non-blocking lock handling, and permanent error / copy-name capping.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 34)
public class HubSyncControllerTest {
    private static final String HOST_UUID = "test-pc-hub-uuid";

    @After
    public void tearDown() {
        Application app = RuntimeEnvironment.getApplication();
        HubSyncController.forget(app, HOST_UUID);
        KnownCapabilities.clear();
    }

    @Test
    public void offlineDraftPersistsAndReplaysWithQuotedEtagAndAssetCacheKey() {
        Application app = RuntimeEnvironment.getApplication();
        Json.Obj write = Json.obj().put("scope", "device").put("name", "Travel 720p");
        HubSyncController.HostState offline = HubSyncController.queueProfilePut(
                app, HOST_UUID, "travel-720", 0L, null, write, null);
        assertEquals(1, offline.outbox.size());
        assertEquals(HubSyncController.SyncStatus.OFFLINE_DEGRADED, offline.status);

        NvApp nvApp = new NvApp("Horizon", 1042, true);
        nvApp.setAppUuid("00000000-0000-0000-0000-000000001042");
        nvApp.setArtVersion("v4");
        Map<Integer, NvApp> knownApps = Collections.singletonMap(1042, nvApp);

        AtomicInteger coverDownloads = new AtomicInteger();
        HubSyncController.HubBackend backend = new HubSyncController.HubBackend() {
            @Override
            public Json.Obj fetchSnapshot(String ifNoneMatch, boolean allDevices) {
                return Json.obj().put("status", "ok").put("etag", "\"rev:5:abcd\"")
                        .put("snapshot", Json.obj()
                                .put("snapshot_revision", 5L)
                                .put("profiles", Json.arr())
                                .put("covers", Json.arr().add(Json.obj().put("app_id", 1042).put("etag", "sha256:new"))));
            }

            @Override
            public Json.Obj putProfile(String id, String ifMatch, Json.Obj w) {
                assertEquals("travel-720", id);
                assertEquals("0", ifMatch);
                return Json.obj().put("status", "ok")
                        .put("profile", Json.obj().put("id", id).put("revision", 1L).put("name", w.getString("name")));
            }

            @Override
            public Json.Obj deleteProfile(String id, String ifMatch) {
                return Json.obj().put("status", "ok");
            }

            @Override
            public Json.Obj fetchCoverToFile(int appId, String ifNoneMatch, File destFile) {
                coverDownloads.incrementAndGet();
                destFile.getParentFile().mkdirs();
                try (FileOutputStream out = new FileOutputStream(destFile)) {
                    out.write(new byte[] {1, 2, 3, 4});
                } catch (Exception e) {
                    throw new RuntimeException(e);
                }
                return Json.obj().put("status", "ok").put("etag", "\"sha256:new\"");
            }

            @Override
            public Json.Obj mergeThreeWay(Json.Obj base, Json.Obj local, Json.Obj remote) {
                return Json.obj().put("status", "merged").put("write", local);
            }

            @Override
            public Json.Arr coversToFetch(Json.Arr remoteCovers, Json.Obj localEtags) {
                Json.Arr arr = Json.arr();
                for (int i = 0; i < remoteCovers.length(); i++) {
                    Json.Obj c = remoteCovers.getObject(i);
                    String id = Integer.toString(c.getInt("app_id"));
                    String local = HubSyncController.normalizeEtag(localEtags.optString(id));
                    String remote = HubSyncController.normalizeEtag(c.getString("etag"));
                    if (!remote.equals(local)) arr.add(c.getInt("app_id"));
                }
                return arr;
            }
        };

        HubSyncController.HostState synced = HubSyncController.syncOnce(app, HOST_UUID, backend, knownApps);
        assertEquals(HubSyncController.SyncStatus.SYNCED, synced.status);
        assertTrue(synced.outbox.isEmpty());
        assertEquals("sha256:new", synced.coverEtags.get(1042));
        assertEquals(1, coverDownloads.get());
        File art = CacheHelper.openPath(false, app.getCacheDir(), "boxart", HOST_UUID, nvApp.getAssetCacheKey() + ".png");
        assertTrue(art.isFile());

        // Second sync with normalized ETag performs zero cover downloads.
        HubSyncController.syncOnce(app, HOST_UUID, backend, knownApps);
        assertEquals(1, coverDownloads.get());
    }

    @Test
    public void conflictRetryPermanent4xxAndCopyNameCapWorkCleanly() {
        Application app = RuntimeEnvironment.getApplication();
        Json.Obj base = Json.obj().put("id", "couch").put("revision", 7L).put("name", "Old");
        String maxName = new String(new char[120]).replace('\0', 'a');
        Json.Obj write = Json.obj().put("scope", "device").put("name", maxName);
        HubSyncController.queueProfilePut(app, HOST_UUID, "couch", 7L, base, write, null);

        HubSyncController.HubBackend conflictBackend = new HubSyncController.HubBackend() {
            @Override
            public Json.Obj fetchSnapshot(String ifNoneMatch, boolean allDevices) {
                return Json.obj().put("status", "ok").put("etag", "\"rev:8:0001\"")
                        .put("snapshot", Json.obj()
                                .put("snapshot_revision", 8L)
                                .put("profiles", Json.arr().add(Json.obj().put("id", "couch").put("revision", 8L).put("name", "PC Edit")))
                                .put("covers", Json.arr()));
            }

            @Override
            public Json.Obj putProfile(String id, String ifMatch, Json.Obj w) {
                return Json.obj().put("status", "conflict").put("current_revision", 8L);
            }

            @Override public Json.Obj deleteProfile(String id, String ifMatch) { return Json.obj(); }
            @Override public Json.Obj fetchCoverToFile(int appId, String ifNoneMatch, File destFile) { return Json.obj(); }

            @Override
            public Json.Obj mergeThreeWay(Json.Obj b, Json.Obj l, Json.Obj r) {
                return Json.obj().put("status", "conflict").put("conflict",
                        Json.obj().put("id", "couch").put("server_revision", 8L)
                                .put("conflicting_fields", Json.arr().add("name"))
                                .put("local", l).put("remote", r));
            }

            @Override public Json.Arr coversToFetch(Json.Arr rc, Json.Obj le) { return Json.arr(); }
        };

        HubSyncController.HostState conflicted = HubSyncController.syncOnce(app, HOST_UUID, conflictBackend, Collections.emptyMap());
        assertEquals(HubSyncController.SyncStatus.CONFLICT_PENDING, conflicted.status);
        assertNotNull(conflicted.pendingConflict);

        // SaveAsCopy truncates base name cleanly so the "(copy)" title never exceeds 120 chars.
        HubSyncController.HostState copyQueued = HubSyncController.resolveConflict(app, HOST_UUID, "save_copy", null);
        assertEquals(1, copyQueued.outbox.size());
        String copyName = copyQueued.outbox.get(0).getObject("write").getString("name");
        assertTrue(copyName.endsWith(" (copy)"));
        assertTrue(copyName.codePointCount(0, copyName.length()) <= 120);

        // Permanent 422 invalid_value drops the poisoned op rather than wedging outbox[0].
        HubSyncController.HubBackend permanentErr = new HubSyncController.HubBackend() {
            @Override
            public Json.Obj fetchSnapshot(String ifNoneMatch, boolean allDevices) throws NativeApiException {
                return conflictBackend.fetchSnapshot(ifNoneMatch, allDevices);
            }

            @Override
            public Json.Obj putProfile(String id, String ifMatch, Json.Obj w) throws NativeApiException {
                throw new NativeApiException(NativeApiException.Kind.API, "Invalid setting", 422, "invalid_value", null, 0L);
            }

            @Override public Json.Obj deleteProfile(String id, String ifMatch) { return Json.obj(); }
            @Override public Json.Obj fetchCoverToFile(int appId, String ifNoneMatch, File destFile) { return Json.obj(); }
            @Override public Json.Obj mergeThreeWay(Json.Obj b, Json.Obj l, Json.Obj r) { return Json.obj(); }
            @Override public Json.Arr coversToFetch(Json.Arr rc, Json.Obj le) { return Json.arr(); }
        };
        HubSyncController.HostState cleared = HubSyncController.syncOnce(app, HOST_UUID, permanentErr, Collections.emptyMap());
        assertEquals(HubSyncController.SyncStatus.SYNCED, cleared.status);
        assertTrue(cleared.outbox.isEmpty());
        assertEquals("Invalid setting", cleared.lastError);

        // Permission error stops outbox cleanly with PERMISSION_DENIED.
        HubSyncController.queueProfilePut(app, HOST_UUID, "couch2", 0L, null, write, null);
        HubSyncController.HubBackend deniedBackend = new HubSyncController.HubBackend() {
            @Override
            public Json.Obj fetchSnapshot(String ifNoneMatch, boolean allDevices) throws NativeApiException {
                throw new NativeApiException(NativeApiException.Kind.API, "Missing List permission", 403, "permission_required", null, 0L);
            }

            @Override public Json.Obj putProfile(String id, String ifMatch, Json.Obj w) { return Json.obj(); }
            @Override public Json.Obj deleteProfile(String id, String ifMatch) { return Json.obj(); }
            @Override public Json.Obj fetchCoverToFile(int appId, String ifNoneMatch, File destFile) { return Json.obj(); }
            @Override public Json.Obj mergeThreeWay(Json.Obj b, Json.Obj l, Json.Obj r) { return Json.obj(); }
            @Override public Json.Arr coversToFetch(Json.Arr rc, Json.Obj le) { return Json.arr(); }
        };

        HubSyncController.HostState denied = HubSyncController.syncOnce(app, HOST_UUID, deniedBackend, Collections.emptyMap());
        assertEquals(HubSyncController.SyncStatus.PERMISSION_DENIED, denied.status);
        assertEquals(1, denied.outbox.size());
    }

    @Test
    public void hubSyncFeatureMergesKnownCapabilitiesAndStateForDoesNotBlockOnNetwork() throws Exception {
        Application app = RuntimeEnvironment.getApplication();
        ComputerDetails pc = new ComputerDetails();
        pc.uuid = HOST_UUID;
        pc.pairState = PairingManager.PairState.PAIRED;
        String xml = "<root status_code=\"200\"><appversion>7.1.431.-1</appversion><PairStatus>1</PairStatus>"
                + "<RustHostVersion>7.0.0</RustHostVersion><Permission>4294967295</Permission>"
                + "<NativeApiVersion>1</NativeApiVersion></root>";
        String capsJson = Json.parseObject(NightjarJni.capabilitiesCall(
                Json.obj().put("op", "from_xml").put("xml", xml).toString()))
                .getObject("ok").getString("json");
        pc.caps = HostCapabilities.fromJson(capsJson);
        assertTrue(pc.caps.isKnown());
        assertFalse(HubSyncController.hasHubSync(pc));

        // onHostOnline waits until native capabilities record hub.sync, triggers once, and debounces polls.
        CountDownLatch pollLatch = new CountDownLatch(1);
        AtomicInteger factoryCalls = new AtomicInteger();
        HubSyncController.ClientFactory factory = details -> {
            factoryCalls.incrementAndGet();
            pollLatch.countDown();
            return null;
        };
        HubSyncController.onHostOnline(app, pc, factory);
        assertEquals(0, factoryCalls.get());

        KnownCapabilities.record(HOST_UUID,
                "{\"api_version\":1,\"host\":{\"name\":\"Test\",\"version\":\"3.1.0\",\"unique_id\":\"" + HOST_UUID + "\"},"
                        + "\"features\":[\"hub.sync\"],"
                        + "\"permissions\":{\"list\":true,\"view\":true,\"launch\":true,\"host_control\":false,\"file_upload\":false,\"file_download\":false}}");
        assertTrue(HubSyncController.hasHubSync(pc));
        assertTrue(pollLatch.await(5, TimeUnit.SECONDS));
        HubSyncController.onHostOnline(app, pc, factory);
        assertEquals(1, factoryCalls.get());

        CountDownLatch inNetwork = new CountDownLatch(1);
        CountDownLatch releaseNetwork = new CountDownLatch(1);
        HubSyncController.HubBackend slowBackend = new HubSyncController.HubBackend() {
            @Override
            public Json.Obj fetchSnapshot(String ifNoneMatch, boolean allDevices) {
                inNetwork.countDown();
                try {
                    releaseNetwork.await(5, TimeUnit.SECONDS);
                } catch (InterruptedException ignored) {}
                return Json.obj().put("status", "ok").put("etag", "\"rev:1\"")
                        .put("snapshot", Json.obj().put("snapshot_revision", 1L)
                                .put("profiles", Json.arr()).put("covers", Json.arr()));
            }

            @Override public Json.Obj putProfile(String id, String ifMatch, Json.Obj w) { return Json.obj(); }
            @Override public Json.Obj deleteProfile(String id, String ifMatch) { return Json.obj(); }
            @Override public Json.Obj fetchCoverToFile(int appId, String ifNoneMatch, File destFile) { return Json.obj(); }
            @Override public Json.Obj mergeThreeWay(Json.Obj b, Json.Obj l, Json.Obj r) { return Json.obj(); }
            @Override public Json.Arr coversToFetch(Json.Arr rc, Json.Obj le) { return Json.arr(); }
        };

        Thread bg = new Thread(() -> HubSyncController.syncOnce(app, HOST_UUID, slowBackend, Collections.emptyMap()));
        bg.start();
        assertTrue(inNetwork.await(5, TimeUnit.SECONDS));
        // stateFor must return immediately without blocking on in-flight HTTPS sync.
        HubSyncController.HostState readWhileSyncing = HubSyncController.stateFor(app, HOST_UUID);
        assertNotNull(readWhileSyncing);
        releaseNetwork.countDown();
        bg.join(5000);
    }
}
