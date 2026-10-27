package com.limelight.nvstream.http;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertThrows;
import static org.junit.Assert.assertTrue;

import android.app.Application;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

import java.io.ByteArrayInputStream;
import java.io.ByteArrayOutputStream;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.security.PrivateKey;
import java.security.cert.X509Certificate;
import java.util.ArrayList;
import java.util.List;

import okhttp3.Interceptor;
import okhttp3.MediaType;
import okhttp3.OkHttpClient;
import okhttp3.Protocol;
import okhttp3.Request;
import okhttp3.Response;
import okhttp3.ResponseBody;
import okio.Buffer;
import okio.BufferedSource;

/** Shared folders as the host's folders routes speak them (nightjar-core folders.rs). */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class SharedFoldersTest {
    private final List<Request> requests = new ArrayList<>();
    private final List<byte[]> uploaded = new ArrayList<>();

    private SharedFolders folders(Interceptor host) throws Exception {
        NvHTTP http = new NvHTTP(new ComputerDetails.AddressTuple("192.0.2.1", 47989), 47984,
                "1234567890abcdef", null, new LimelightCryptoProvider() {
            public X509Certificate getClientCertificate() { return null; }
            public PrivateKey getClientPrivateKey() { return null; }
            public byte[] getPemEncodedClientCertificate() { return new byte[0]; }
            public String encodeBase64String(byte[] data) { return ""; }
        });
        java.lang.reflect.Field field = NvHTTP.class.getDeclaredField("httpClientLongConnectTimeout");
        field.setAccessible(true);
        field.set(http, new OkHttpClient.Builder().addInterceptor(chain -> {
            requests.add(chain.request());
            if (chain.request().body() != null) {
                Buffer body = new Buffer();
                chain.request().body().writeTo(body);
                uploaded.add(body.readByteArray());
            }
            // Application interceptors run before OkHttp's own cancel check: stand in for it.
            if (chain.call().isCanceled()) throw new IOException("Canceled");
            return host.intercept(chain);
        }).build());
        return new SharedFolders(http);
    }

    private static Interceptor answer(int code, String json) {
        return chain -> response(chain, code, ResponseBody.create(json, MediaType.get("application/json")));
    }

    private static Response response(Interceptor.Chain chain, int code, ResponseBody body) {
        return new Response.Builder().request(chain.request()).protocol(Protocol.HTTP_1_1).code(code)
                .message("Fixture").body(body).build();
    }

    private static String refusal(String code) {
        return "{\"error\":{\"code\":\"" + code + "\",\"message\":\"refused\"}}";
    }

    @Test
    public void theFolderListAsksTheFoldersRouteAndReadsTheGrants() throws Exception {
        List<SharedFolders.Folder> list = folders(answer(200, "{\"folders\":[{\"id\":\"games\",\"name\":\"Games\","
                + "\"access\":\"write\"},{\"id\":\"music\",\"name\":\"Music\",\"access\":\"read\"},"
                + "{\"id\":\"new\",\"name\":\"New\",\"access\":\"append\"}]}")).folders();
        assertEquals(3, list.size());
        assertEquals("games", list.get(0).id);
        assertEquals("Games", list.get(0).name);
        assertTrue(list.get(0).writable);
        assertFalse(list.get(1).writable);
        // An access from a newer host counts as read only.
        assertFalse(list.get(2).writable);
        Request request = requests.get(0);
        assertEquals("GET", request.method());
        assertEquals("/api/native/v1/folders", request.url().encodedPath());
        assertEquals("1234567890abcdef", request.url().queryParameter("uniqueid"));
        assertNull(request.url().queryParameter("path"));
    }

    @Test
    public void aListingIsReadStrictly() throws Exception {
        SharedFolders.Listing listing = folders(answer(200, "{\"path\":\"Saves\",\"entries\":["
                + "{\"name\":\"Slot 1\",\"kind\":\"dir\",\"size\":0,\"modified_unix_ms\":1700000000000},"
                + "{\"name\":\"a&b.sav\",\"kind\":\"file\",\"size\":5000000000,\"modified_unix_ms\":0},"
                + "{\"name\":\"link\",\"kind\":\"other\",\"size\":0,\"modified_unix_ms\":0}]}")).entries("games", "Saves");
        assertEquals("Saves", listing.path);
        assertEquals(SharedFolders.Kind.DIR, listing.entries.get(0).kind);
        assertEquals(1700000000000L, listing.entries.get(0).modifiedUnixMs);
        assertEquals(SharedFolders.Kind.FILE, listing.entries.get(1).kind);
        assertEquals(5000000000L, listing.entries.get(1).size);
        assertEquals(SharedFolders.Kind.UNKNOWN, listing.entries.get(2).kind);
        assertEquals("/api/native/v1/folders/games/entries", requests.get(0).url().encodedPath());
        assertEquals("Saves", requests.get(0).url().queryParameter("path"));
        for (String bad : new String[] {"{\"entries\":[]}", "{\"path\":\"\",\"entries\":[{\"name\":\"a\",\"kind\":\"file\","
                + "\"size\":\"5\",\"modified_unix_ms\":0}]}", "{\"path\":\"\",\"entries\":[{\"name\":\"a\",\"kind\":\"file\","
                + "\"size\":-1,\"modified_unix_ms\":0}]}", "{\"path\":\"\",\"entries\":[1]}", "[]"}) {
            assertThrows(bad, IOException.class, () -> SharedFolders.parseEntries(bad));
        }
        assertThrows(IOException.class, () -> SharedFolders.parseFolders("{\"folders\":[{\"id\":1,\"name\":\"a\",\"access\":\"read\"}]}"));
    }

    @Test
    public void anAnswerOverItsLimitIsRefused() throws Exception {
        String big = "{\"folders\":[],\"pad\":\"" + "a".repeat(SharedFolders.MAX_JSON_BYTES) + "\"}";
        assertThrows(IOException.class, () -> folders(answer(200, big)).folders());
    }

    @Test
    public void eachDocumentedRefusalIsKnownByStatusAndCode() throws Exception {
        for (SharedFolders.Refusal expected : SharedFolders.Refusal.values()) {
            SharedFolders.RefusedException refused = assertThrows(SharedFolders.RefusedException.class,
                    () -> folders(answer(expected.status, refusal(expected.code))).folders());
            assertEquals(expected, refused.refusal);
            assertEquals(expected.status, refused.status);
        }
        // A code under another status, an unknown code or a page that is no error body stay plain HTTP errors.
        for (Interceptor other : new Interceptor[] {answer(409, refusal("forbidden")), answer(403, refusal("new_reason")),
                answer(500, "<html>"), answer(204, "")}) {
            IOException failure = assertThrows(IOException.class, () -> folders(other).folders());
            assertTrue(failure.toString(), failure instanceof HostHttpResponseException);
        }
    }

    @Test
    public void aDownloadWritesTheWholeFileAndReportsProgress() throws Exception {
        ByteArrayOutputStream sink = new ByteArrayOutputStream();
        List<long[]> progress = new ArrayList<>();
        long length = folders(chain -> response(chain, 200, ResponseBody.create("hello",
                MediaType.get("application/octet-stream")))).download("games", "Saves/a b.sav", sink,
                (done, total) -> progress.add(new long[] {done, total}), new TransferCancel());
        assertEquals(5, length);
        assertEquals("hello", sink.toString("UTF-8"));
        assertEquals(5, progress.get(progress.size() - 1)[0]);
        assertEquals(5, progress.get(progress.size() - 1)[1]);
        assertEquals("/api/native/v1/folders/games/content", requests.get(0).url().encodedPath());
        assertEquals("path=Saves%2Fa%20b.sav", requests.get(0).url().encodedQuery().split("&uniqueid")[0]);
    }

    @Test
    public void aShortDownloadFails() throws Exception {
        ResponseBody shortBody = new ResponseBody() {
            @Override public MediaType contentType() { return MediaType.get("application/octet-stream"); }
            @Override public long contentLength() { return 10; }
            @Override public BufferedSource source() { return new Buffer().writeUtf8("hello"); }
        };
        IOException failure = assertThrows(IOException.class, () -> folders(chain -> response(chain, 200, shortBody))
                .download("games", "a.sav", new ByteArrayOutputStream(), (done, total) -> { }, new TransferCancel()));
        assertFalse(failure instanceof SharedFolders.LocalFileException);
        assertTrue(failure.getMessage(), failure.getMessage().contains("5 of 10"));
    }

    @Test
    public void aRefusedDownloadWritesNothing() throws Exception {
        ByteArrayOutputStream sink = new ByteArrayOutputStream();
        assertEquals(SharedFolders.Refusal.TRANSFER_BUSY, assertThrows(SharedFolders.RefusedException.class,
                () -> folders(answer(409, refusal("transfer_busy"))).download("games", "a.sav", sink,
                        (done, total) -> { }, new TransferCancel())).refusal);
        assertEquals(0, sink.size());
    }

    @Test
    public void aCancelledTransferSaysSo() throws Exception {
        TransferCancel cancel = new TransferCancel();
        cancel.cancel();
        assertThrows(SharedFolders.CancelledException.class, () -> folders(answer(200, "")).download("games", "a.sav",
                new ByteArrayOutputStream(), (done, total) -> { }, cancel));
        assertThrows(SharedFolders.CancelledException.class, () -> folders(answer(201, "")).upload("games", "a.sav",
                new ByteArrayInputStream(new byte[1]), 1, (done, total) -> { }, cancel));
    }

    @Test
    public void anUploadPutsItsLengthAndReturnsTheStoredName() throws Exception {
        byte[] file = "hello".getBytes(StandardCharsets.UTF_8);
        String stored = folders(answer(201, "{\"name\":\"a (1).txt\",\"size\":5}")).upload("games", "Docs/a.txt",
                new ByteArrayInputStream(file), file.length, (done, total) -> { }, new TransferCancel());
        assertEquals("a (1).txt", stored);
        Request request = requests.get(0);
        assertEquals("PUT", request.method());
        assertEquals("/api/native/v1/folders/games/content", request.url().encodedPath());
        assertEquals("Docs/a.txt", request.url().queryParameter("path"));
        assertEquals("100-continue", request.header("Expect"));
        assertEquals(5, request.body().contentLength());
        assertTrue(request.body().isOneShot());
        assertEquals("application/octet-stream", request.body().contentType().toString());
        assertEquals("hello", new String(uploaded.get(0), StandardCharsets.UTF_8));
    }

    @Test
    public void anUploadTheHostStoredShortOrAnsweredWithoutCreatingFails() throws Exception {
        byte[] file = new byte[5];
        assertThrows(IOException.class, () -> folders(answer(201, "{\"name\":\"a.txt\",\"size\":4}")).upload("games",
                "a.txt", new ByteArrayInputStream(file), 5, (done, total) -> { }, new TransferCancel()));
        assertThrows(HostHttpResponseException.class, () -> folders(answer(200, "{\"name\":\"a.txt\",\"size\":5}"))
                .upload("games", "a.txt", new ByteArrayInputStream(file), 5, (done, total) -> { }, new TransferCancel()));
    }

    @Test
    public void aFileShorterThanItsLengthFailsOnThisDevice() throws Exception {
        assertThrows(SharedFolders.LocalFileException.class, () -> folders(answer(201, "{\"name\":\"a.txt\",\"size\":10}"))
                .upload("games", "a.txt", new ByteArrayInputStream(new byte[5]), 10, (done, total) -> { },
                        new TransferCancel()));
    }

    @Test
    public void namesThatCannotTravelAreRefusedBeforeAnyRequest() throws Exception {
        SharedFolders folders = folders(answer(201, "{}"));
        for (String name : new String[] {"a.part", "Docs/b.PART", "", "Docs/"}) {
            assertThrows(name, IllegalArgumentException.class, () -> folders.upload("games", name,
                    new ByteArrayInputStream(new byte[0]), 0, (done, total) -> { }, new TransferCancel()));
        }
        for (String id : new String[] {"", "a/b", "a b", "ü", "a".repeat(65)}) {
            assertThrows(id, IllegalArgumentException.class, () -> folders.entries(id, ""));
        }
        for (String path : new String[] {"/a", "a//b", "a/", String.join("/", java.util.Collections.nCopies(65, "a"))}) {
            assertThrows(path, IllegalArgumentException.class, () -> folders.entries("games", path));
        }
        assertThrows(IllegalArgumentException.class, () -> folders.download("games", "", new ByteArrayOutputStream(),
                (done, total) -> { }, new TransferCancel()));
        assertTrue(requests.isEmpty());
        SharedFolders.checkId("Games_2-x");
        SharedFolders.checkPath(String.join("/", java.util.Collections.nCopies(64, "a")));
    }

    @Test
    public void pathsJoinAndSplitByName() {
        assertEquals("a", SharedFolders.childPath("", "a"));
        assertEquals("a/b", SharedFolders.childPath("a", "b"));
        assertEquals("", SharedFolders.parentPath("a"));
        assertEquals("a/b", SharedFolders.parentPath("a/b/c"));
        assertEquals("c.txt", SharedFolders.fileName("a/b/c.txt"));
        assertTrue(SharedFolders.hasPartSuffix("x.Part"));
        assertFalse(SharedFolders.hasPartSuffix("part"));
        assertFalse(SharedFolders.hasPartSuffix("x.parts"));
        // The host percent-decodes the whole argument, the slashes between names with it.
        assertEquals("path=a%20b%2Fc%26d%3D", SharedFolders.pathQuery("a b/c&d="));
    }
}
