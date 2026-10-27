package com.limelight.files;

import com.limelight.LimeLog;
import com.limelight.nativeapi.HostEndpoint;

import org.json.JSONArray;
import org.json.JSONException;

import java.io.IOException;
import java.util.Collections;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.concurrent.Executor;

/**
 * Runs the transfers of one screen and one PC on the Rust list ({@link FileTransferEngine}): the list decides
 * what may start (one upload and two downloads at a time) and what a failure means; this class opens the
 * files, hands them to workers and tells the screen. Every method runs on the main thread; {@code worker}
 * runs the blocking calls, several at once, and {@code main} brings their answers back.
 *
 * <p>A download writes straight into the document the user created. Nothing resumes (the host has no ranges),
 * so a download that did not finish deletes its document, and trying it again asks for a new one.
 */
public final class FileTransferController {
    /** Opens a file and detaches its descriptor for the Rust client, which closes it. */
    public interface Opener {
        int open() throws IOException;
    }

    public interface Ui {
        void onTransfersChanged(List<FileRow> rows);

        /** An upload landed: the PC's list changed. */
        void onUploaded();
    }

    private static final class Job {
        final boolean upload;
        final Opener opener;
        /** Deletes what a download left behind; null for an upload. */
        final Runnable discard;
        volatile boolean started;
        volatile boolean discarded;

        Job(boolean upload, Opener opener, Runnable discard) {
            this.upload = upload;
            this.opener = opener;
            this.discard = discard;
        }
    }

    private final FileTransferEngine engine;
    private final Executor worker;
    private final Executor main;
    private final Ui ui;
    private final HostEndpoint host;
    private final long handle;
    private final Map<Long, Job> jobs = new HashMap<>();
    private List<FileRow> rows = Collections.emptyList();
    private boolean closed;

    public FileTransferController(FileTransferEngine engine, HostEndpoint host, Executor worker, Executor main,
                                  Ui ui) {
        this.engine = engine;
        this.host = host;
        this.worker = worker;
        this.main = main;
        this.ui = ui;
        this.handle = engine.create();
    }

    public List<FileRow> rows() {
        return rows;
    }

    public HostEndpoint host() {
        return host;
    }

    /** Queues a file to send; its name on the PC is made one the host takes. Returns the id, or -1. */
    public long send(String localName, long size, Opener opener) {
        return add(engine.queueUpload(handle, localName, size), new Job(true, opener, null));
    }

    /** Queues a download of the PC's {@code name} into a document, which {@code discard} deletes on failure. */
    public long download(String name, String localName, long size, Opener opener, Runnable discard) {
        return add(engine.queueDownload(handle, name, localName, size), new Job(false, opener, discard));
    }

    private long add(long id, Job job) {
        if (closed || id < 0) {
            if (job.discard != null) worker.execute(job.discard);
            return -1;
        }
        jobs.put(id, job);
        changed();
        pump();
        return id;
    }

    public void cancel(long id) {
        if (closed) return;
        engine.cancel(handle, id);
        changed();
    }

    /** Whether a retry needs a new document first ({@link #retryDownload}). */
    public boolean needsNewDocument(long id) {
        Job job = jobs.get(id);
        return job != null && !job.upload;
    }

    /** Queues a failed or cancelled upload again, from its first byte. */
    public void retry(long id) {
        Job job = jobs.get(id);
        if (closed || job == null || !job.upload || !engine.retry(handle, id)) return;
        job.started = false;
        changed();
        pump();
    }

    /** Queues a failed or cancelled download again into a new document. */
    public void retryDownload(long id, Opener opener, Runnable discard) {
        Job job = jobs.get(id);
        if (closed || job == null || job.upload || !engine.retry(handle, id)) {
            worker.execute(discard);
            return;
        }
        jobs.put(id, new Job(false, opener, discard));
        changed();
        pump();
    }

    public void clearFinished() {
        if (closed) return;
        engine.clearFinished(handle);
        changed();
        for (Long id : jobs.keySet().toArray(new Long[0])) {
            if (!contains(id)) jobs.remove(id);
        }
    }

    public boolean isActive() {
        for (FileRow row : rows) {
            if (!row.isFinished()) return true;
        }
        return false;
    }

    /** Cancels everything; the screen is going. Running workers end on their own and clean up after them. */
    public void close() {
        if (closed) return;
        engine.cancelAll(handle);
        refresh();
        discardFinished();
        closed = true;
        engine.destroy(handle);
    }

    private boolean contains(long id) {
        for (FileRow row : rows) {
            if (row.id == id) return true;
        }
        return false;
    }

    private void refresh() {
        rows = FileRow.parse(engine.snapshot(handle));
    }

    private void changed() {
        refresh();
        discardFinished();
        ui.onTransfersChanged(rows);
    }

    /** Downloads that finished without running (cancelled while queued) delete their document here. */
    private void discardFinished() {
        for (FileRow row : rows) {
            Job job = jobs.get(row.id);
            if (job != null && !job.upload && !job.started && !job.discarded && row.isFinished()
                    && row.state != FileRow.State.DONE) {
                job.discarded = true;
                worker.execute(job.discard);
            }
        }
    }

    private void pump() {
        if (closed) return;
        String started = engine.startNext(handle);
        try {
            JSONArray starts = new JSONArray(started);
            for (int i = 0; i < starts.length(); i++) {
                long id = starts.getJSONObject(i).getLong("id");
                Job job = jobs.get(id);
                if (job != null) job.started = true;
                worker.execute(() -> run(id, job));
            }
        } catch (JSONException e) {
            throw new IllegalStateException("Unreadable transfer start: " + started, e);
        }
        if (starts(started)) changed();
    }

    private static boolean starts(String started) {
        return !"[]".equals(started);
    }

    /** On a worker. */
    private void run(long id, Job job) {
        int fd = -1;
        if (job != null) {
            try {
                fd = job.opener.open();
            } catch (IOException | RuntimeException e) {
                // The Rust list fails the transfer as a local problem.
                LimeLog.warning("File transfer: could not open the file: " + e.getMessage());
            }
        }
        FileRow row = FileRow.ofRun(engine.run(handle, id, host, fd, (transfer, done, total) ->
                main.execute(this::onProgress)));
        boolean landed = row != null && row.state == FileRow.State.DONE;
        if (job != null && !job.upload && !landed) {
            job.discarded = true;
            job.discard.run();
        }
        main.execute(() -> {
            if (closed) return;
            changed();
            if (landed && row.upload) ui.onUploaded();
            pump();
        });
    }

    private void onProgress() {
        if (closed) return;
        refresh();
        ui.onTransfersChanged(rows);
    }
}
