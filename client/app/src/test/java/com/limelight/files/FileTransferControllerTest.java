package com.limelight.files;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import android.app.Application;

import com.limelight.nativeapi.HostEndpoint;
import com.limelight.nvstream.jni.FileTransferBridge;

import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

import java.io.IOException;
import java.util.ArrayDeque;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.concurrent.Executor;
import java.util.concurrent.atomic.AtomicInteger;

/**
 * The controller on a fake transfer list that keeps the Rust list's rules (one upload and two downloads at a
 * time, retry only after a failure or cancel) and answers each run as the test says.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class FileTransferControllerTest {
    private static final HostEndpoint HOST = new HostEndpoint("pc", "10.0.0.2", 47989, 47984, "pem", "client",
            new byte[0], new byte[0]);

    /** Runs what was handed to it only when the test says so. */
    private static final class Manual implements Executor {
        final ArrayDeque<Runnable> queue = new ArrayDeque<>();

        @Override public void execute(Runnable command) {
            queue.add(command);
        }

        boolean runOne() {
            Runnable next = queue.poll();
            if (next == null) return false;
            next.run();
            return true;
        }
    }

    private static final class Transfer {
        final long id;
        final boolean upload;
        final String name;
        final String localName;
        final long size;
        String state = "queued";
        long done;
        String problem;
        boolean cancelling;

        Transfer(long id, boolean upload, String name, String localName, long size) {
            this.id = id;
            this.upload = upload;
            this.name = name;
            this.localName = localName;
            this.size = size;
        }

        boolean finished() {
            return state.equals("done") || state.equals("failed") || state.equals("cancelled");
        }

        JSONObject json() {
            try {
                JSONObject row = new JSONObject().put("id", id).put("direction", upload ? "upload" : "download")
                        .put("name", name).put("local_name", localName).put("size", size).put("state", state)
                        .put("cancelling", cancelling)
                        .put("can_retry", state.equals("failed") || state.equals("cancelled"));
                if (state.equals("running")) row.put("done", done).put("total", size);
                if (state.equals("done")) row.put("saved_name", name).put("done", size).put("total", size);
                if (state.equals("failed")) row.put("problem", problem).put("message", problem);
                return row;
            } catch (JSONException e) {
                throw new AssertionError(e);
            }
        }
    }

    private static final class FakeEngine implements FileTransferEngine {
        final Map<Long, Transfer> transfers = new LinkedHashMap<>();
        /** The problem the next run of an id ends with; absent: it lands. */
        final Map<Long, String> outcomes = new HashMap<>();
        final List<Integer> fds = new ArrayList<>();
        long next = 1;
        boolean destroyed;
        int cancelAlls;

        @Override public long create() { return 42; }
        @Override public void destroy(long handle) { destroyed = true; }

        @Override public long queueUpload(long handle, String localName, long size) {
            return queue(true, localName, localName, size);
        }

        @Override public long queueDownload(long handle, String name, String localName, long size) {
            return queue(false, name, localName, size);
        }

        private long queue(boolean upload, String name, String localName, long size) {
            if (destroyed) return -1;
            long id = next++;
            transfers.put(id, new Transfer(id, upload, name, localName, size));
            return id;
        }

        @Override public boolean cancel(long handle, long id) {
            Transfer t = transfers.get(id);
            if (t == null || t.finished()) return false;
            if (t.state.equals("queued")) t.state = "cancelled";
            else t.cancelling = true;
            return true;
        }

        @Override public void cancelAll(long handle) {
            cancelAlls++;
            for (Long id : transfers.keySet()) cancel(handle, id);
        }

        @Override public boolean retry(long handle, long id) {
            Transfer t = transfers.get(id);
            if (t == null || !(t.state.equals("failed") || t.state.equals("cancelled"))) return false;
            t.state = "queued";
            t.problem = null;
            t.cancelling = false;
            t.done = 0;
            return true;
        }

        @Override public void clearFinished(long handle) {
            transfers.values().removeIf(Transfer::finished);
        }

        @Override public String snapshot(long handle) {
            JSONArray rows = new JSONArray();
            for (Transfer t : transfers.values()) rows.put(t.json());
            return rows.toString();
        }

        @Override public String startNext(long handle) {
            int uploads = 0;
            int downloads = 0;
            for (Transfer t : transfers.values()) {
                if (t.state.equals("running")) {
                    if (t.upload) uploads++;
                    else downloads++;
                }
            }
            JSONArray started = new JSONArray();
            for (Transfer t : transfers.values()) {
                if (!t.state.equals("queued")) continue;
                if (t.upload ? uploads >= 1 : downloads >= 2) continue;
                if (t.upload) uploads++;
                else downloads++;
                t.state = "running";
                try {
                    started.put(new JSONObject().put("id", t.id));
                } catch (JSONException e) {
                    throw new AssertionError(e);
                }
            }
            return started.toString();
        }

        @Override public String list(HostEndpoint host) {
            throw new AssertionError("the controller never lists");
        }

        @Override public String run(long handle, long id, HostEndpoint host, int fd,
                                    FileTransferBridge.Progress progress) {
            fds.add(fd);
            Transfer t = transfers.get(id);
            if (destroyed || t == null || !t.state.equals("running")) {
                return "{\"error\":{\"problem\":\"local\",\"message\":\"no such transfer\"}}";
            }
            t.done = t.size / 2;
            progress.onProgress(id, t.done, t.size);
            String problem = fd < 0 ? FileProblems.LOCAL : outcomes.remove(id);
            if (t.cancelling) {
                t.state = "cancelled";
                t.cancelling = false;
            } else if (problem != null) {
                t.state = "failed";
                t.problem = problem;
            } else {
                t.state = "done";
            }
            try {
                return new JSONObject().put("ok", t.json()).toString();
            } catch (JSONException e) {
                throw new AssertionError(e);
            }
        }
    }

    private static final class Screen implements FileTransferController.Ui {
        List<FileRow> rows = new ArrayList<>();
        int changes;
        int uploaded;

        @Override public void onTransfersChanged(List<FileRow> rows) {
            this.rows = rows;
            changes++;
        }

        @Override public void onUploaded() {
            uploaded++;
        }

        FileRow row(long id) {
            for (FileRow row : rows) if (row.id == id) return row;
            throw new AssertionError("no row " + id);
        }
    }

    private final FakeEngine engine = new FakeEngine();
    private final Manual worker = new Manual();
    private final Manual main = new Manual();
    private final Screen screen = new Screen();
    private final FileTransferController controller =
            new FileTransferController(engine, HOST, worker, main, screen);

    /** Runs the workers and the answers they post until nothing is left. */
    private void settle() {
        boolean ran = true;
        while (ran) {
            ran = worker.runOne();
            ran |= main.runOne();
        }
    }

    private static FileTransferController.Opener fd(int value) {
        return () -> value;
    }

    @Test
    public void uploadsGoOneAtATimeAndEachLandingRefreshesThePcsList() {
        long first = controller.send("a.txt", 10, fd(11));
        long second = controller.send("b.txt", 20, fd(12));
        assertEquals(FileRow.State.RUNNING, screen.row(first).state);
        assertEquals(FileRow.State.QUEUED, screen.row(second).state);
        assertTrue(controller.isActive());

        settle();
        assertEquals(FileRow.State.DONE, screen.row(first).state);
        assertEquals(FileRow.State.DONE, screen.row(second).state);
        assertEquals(java.util.Arrays.asList(11, 12), engine.fds);
        assertEquals(2, screen.uploaded);
        assertFalse(controller.isActive());
    }

    @Test
    public void aFailedDownloadDeletesItsDocumentAndTryingAgainWritesANewOne() {
        AtomicInteger firstDeleted = new AtomicInteger();
        AtomicInteger secondDeleted = new AtomicInteger();
        long id = controller.download("big.bin", "big.bin", 100, fd(21), firstDeleted::incrementAndGet);
        engine.outcomes.put(id, FileProblems.NETWORK);
        settle();
        FileRow failed = screen.row(id);
        assertEquals(FileRow.State.FAILED, failed.state);
        assertEquals(FileProblems.NETWORK, failed.problem);
        assertTrue(failed.canRetry);
        assertEquals(1, firstDeleted.get());
        assertEquals(0, screen.uploaded);

        assertTrue("nothing resumes: a download needs a new document", controller.needsNewDocument(id));
        controller.retryDownload(id, fd(22), secondDeleted::incrementAndGet);
        settle();
        assertEquals(FileRow.State.DONE, screen.row(id).state);
        assertEquals(java.util.Arrays.asList(21, 22), engine.fds);
        assertEquals(1, firstDeleted.get());
        assertEquals(0, secondDeleted.get());
    }

    @Test
    public void aDownloadCancelledBeforeItStartedDeletesItsDocumentOnce() {
        AtomicInteger deleted = new AtomicInteger();
        controller.download("1", "1", 1, fd(31), () -> { });
        controller.download("2", "2", 1, fd(32), () -> { });
        long third = controller.download("3", "3", 1, fd(33), deleted::incrementAndGet);
        assertEquals("two downloads at a time", FileRow.State.QUEUED, screen.row(third).state);

        controller.cancel(third);
        assertEquals(FileRow.State.CANCELLED, screen.row(third).state);
        settle();
        controller.cancel(third);
        settle();
        assertEquals(1, deleted.get());
        assertFalse("it never ran", engine.fds.contains(33));
    }

    @Test
    public void aCancelledRunningUploadEndsCancelledAndCanBeSentAgainFromTheStart() {
        long id = controller.send("a", 10, fd(41));
        controller.cancel(id);
        assertTrue(screen.row(id).cancelling);
        settle();
        assertEquals(FileRow.State.CANCELLED, screen.row(id).state);
        assertEquals(0, screen.uploaded);

        assertFalse(controller.needsNewDocument(id));
        controller.retry(id);
        settle();
        assertEquals(FileRow.State.DONE, screen.row(id).state);
        assertEquals(java.util.Arrays.asList(41, 41), engine.fds);
        assertEquals(1, screen.uploaded);
    }

    @Test
    public void aFileThatWillNotOpenFailsItsTransferAsALocalProblem() {
        AtomicInteger deleted = new AtomicInteger();
        long upload = controller.send("gone", 5, () -> {
            throw new IOException("revoked");
        });
        long download = controller.download("x", "x", 5, () -> {
            throw new SecurityException("no access");
        }, deleted::incrementAndGet);
        settle();
        assertEquals(FileProblems.LOCAL, screen.row(upload).problem);
        assertEquals(FileProblems.LOCAL, screen.row(download).problem);
        assertEquals(java.util.Arrays.asList(-1, -1), engine.fds);
        assertEquals(1, deleted.get());
    }

    @Test
    public void clearFinishedKeepsWhatIsStillGoing() {
        long done = controller.send("a", 1, fd(51));
        settle();
        long running = controller.send("b", 1, fd(52));
        controller.clearFinished();
        assertEquals(1, screen.rows.size());
        assertEquals(running, screen.rows.get(0).id);
        assertFalse(controller.needsNewDocument(done));
    }

    @Test
    public void closingCancelsEverythingAndLateAnswersNoLongerReachTheScreen() {
        AtomicInteger deleted = new AtomicInteger();
        controller.send("a", 1, fd(61));
        controller.download("b", "b", 1, fd(62), deleted::incrementAndGet);
        int before = screen.changes;
        controller.close();
        assertEquals(1, engine.cancelAlls);
        assertTrue(engine.destroyed);
        settle();
        assertEquals("nothing after close", before, screen.changes);
        assertEquals("the download that never landed left no document", 1, deleted.get());
        assertEquals(-1, controller.send("c", 1, fd(63)));
    }

    @Test
    public void aListThatRefusesTheTransferDeletesTheDocumentAndReturnsMinusOne() {
        AtomicInteger deleted = new AtomicInteger();
        engine.destroyed = true;
        assertEquals(-1, controller.download("a", "a", 1, fd(71), deleted::incrementAndGet));
        settle();
        assertEquals(1, deleted.get());
        assertTrue(screen.rows.isEmpty());
    }
}
