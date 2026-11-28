package com.limelight.nativeapi;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

import android.app.Application;

import org.robolectric.RuntimeEnvironment;

import com.limelight.utils.CacheHelper;
import com.limelight.nvstream.jni.Json;

import org.junit.After;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

import java.io.File;
import java.io.FileOutputStream;
import java.util.Collections;
import java.util.concurrent.atomic.AtomicInteger;

/**
 * Unit tests for {@link HubSyncController}: offline disk boot, outbox replay, 3-way conflict
 * handling, 403 permission handling without retry loops, and ETag-cached cover downloads.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class HubSyncControllerTest {
    private static final String HOST_UUID = "hub-test-host-uuid";

    @After
    public void tearDown() {
        Application app = RuntimeEnvironment.getApplication();
        HubSyncController.forget(app, HOST_UUID);
    }

    @Test
    public void offlineDraftPersistsAndReplaysWhenHostReturnsWithEtagCovers() {
        Application app = RuntimeEnvironment.getApplication();
        Json.Obj write = Json.obj().put("scope", "device").put("name", "Travel 720p");
        HubSyncController.HostState offline = HubSyncController.queueProfilePut(
                app, HOST_UUID, "travel-720", 0L, null, write, null);
        assertEquals(1, offline.outbox.size());
        assertEquals(HubSyncController.SyncStatus.OFFLINE_DEGRADED, offline.status);

        AtomicInteger coverDownloads = new AtomicInteger();
        HubSyncController.HubBackend backend = new HubSyncController.HubBackend() {
            @Override
            public Json.Obj fetchSnapshot(String ifNoneMatch, boolean allDevices) {
                return Json.obj().put("status", "ok").put("etag", "\"rev:5\"")
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
                return Json.obj().put("status", "ok").put("etag", "sha256:new");
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
                    if (!c.getString("etag").equals(localEtags.optString(id))) arr.add(c.getInt("app_id"));
                }
                return arr;
            }
        };

        HubSyncController.HostState synced = HubSyncController.syncOnce(app, HOST_UUID, backend, Collections.emptyMap());
        assertEquals(HubSyncController.SyncStatus.SYNCED, synced.status);
        assertTrue(synced.outbox.isEmpty());
        assertEquals(1, coverDownloads.get());
        File art = CacheHelper.openPath(false, app.getCacheDir(), "boxart", HOST_UUID, "1042.png");
        assertTrue(art.isFile());

        // Second sync with unchanged ETag performs zero cover downloads.
        HubSyncController.syncOnce(app, HOST_UUID, backend, Collections.emptyMap());
        assertEquals(1, coverDownloads.get());
    }

    @Test
    public void conflictAndPermissionDeniedPauseOutboxCleanly() {
        Application app = RuntimeEnvironment.getApplication();
        Json.Obj base = Json.obj().put("id", "couch").put("revision", 7L).put("name", "Old");
        Json.Obj write = Json.obj().put("scope", "device").put("name", "Local Edit");
        HubSyncController.queueProfilePut(app, HOST_UUID, "couch", 7L, base, write, null);

        HubSyncController.HubBackend conflictBackend = new HubSyncController.HubBackend() {
            @Override
            public Json.Obj fetchSnapshot(String ifNoneMatch, boolean allDevices) {
                return Json.obj().put("status", "ok").put("etag", "\"rev:8\"")
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
                return Json.obj().put("status", "conflict").put("conflict", Json.obj()
                        .put("id", "couch")
                        .put("server_revision", 8L)
                        .put("conflicting_fields", Json.arr().add("name"))
                        .put("local", l)
                        .put("remote", r));
            }

            @Override public Json.Arr coversToFetch(Json.Arr remoteCovers, Json.Obj localEtags) { return Json.arr(); }
        };

        HubSyncController.HostState conflicted = HubSyncController.syncOnce(
                app, HOST_UUID, conflictBackend, Collections.emptyMap());
        assertEquals(HubSyncController.SyncStatus.CONFLICT_PENDING, conflicted.status);
        assertNotNull(conflicted.pendingConflict);

        HubSyncController.HostState resolved = HubSyncController.resolveConflict(
                app, HOST_UUID, "use_host", null);
        assertNull(resolved.pendingConflict);
        assertTrue(resolved.outbox.isEmpty());

        HubSyncController.HubBackend deniedBackend = new HubSyncController.HubBackend() {
            @Override
            public Json.Obj fetchSnapshot(String ifNoneMatch, boolean allDevices) throws NativeApiException {
                throw new NativeApiException(NativeApiException.Kind.API, "Denied", 403, "permission_required", null, 0L);
            }
            @Override public Json.Obj putProfile(String id, String ifMatch, Json.Obj w) { return Json.obj(); }
            @Override public Json.Obj deleteProfile(String id, String ifMatch) { return Json.obj(); }
            @Override public Json.Obj fetchCoverToFile(int appId, String ifNoneMatch, File destFile) { return Json.obj(); }
            @Override public Json.Obj mergeThreeWay(Json.Obj b, Json.Obj l, Json.Obj r) { return Json.obj(); }
            @Override public Json.Arr coversToFetch(Json.Arr remoteCovers, Json.Obj localEtags) { return Json.arr(); }
        };

        HubSyncController.HostState denied = HubSyncController.syncOnce(app, HOST_UUID, deniedBackend, Collections.emptyMap());
        assertEquals(HubSyncController.SyncStatus.PERMISSION_DENIED, denied.status);
    }
}
