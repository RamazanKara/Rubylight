package com.limelight;

import android.app.Application;
import android.content.ActivityNotFoundException;
import android.content.ComponentName;
import android.content.ContentResolver;
import android.content.Intent;
import android.content.ServiceConnection;
import android.net.Uri;
import android.os.Build;
import android.os.Bundle;
import android.os.Handler;
import android.os.IBinder;
import android.os.Looper;
import android.text.format.DateUtils;
import android.text.format.Formatter;
import android.view.Gravity;
import android.widget.FrameLayout;
import android.widget.LinearLayout;
import android.widget.TextView;

import androidx.activity.OnBackPressedCallback;
import androidx.annotation.StringRes;
import androidx.lifecycle.AndroidViewModel;
import androidx.lifecycle.MutableLiveData;
import androidx.lifecycle.ViewModelProvider;

import com.google.android.material.button.MaterialButton;
import com.google.android.material.color.MaterialColors;
import com.google.android.material.progressindicator.LinearProgressIndicator;
import com.limelight.binding.PlatformBinding;
import com.limelight.computers.ComputerManagerService;
import com.limelight.nativeapi.HostEndpoint;
import com.limelight.nativeapi.JniNativeApiTransport;
import com.limelight.nativeapi.NativeApiClient;
import com.limelight.nativeapi.NativeApiException;
import com.limelight.nativeapi.NativeCapabilities;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvHTTP;
import com.limelight.nvstream.http.SharedFolders;
import com.limelight.nvstream.http.TransferCancel;
import com.limelight.ui.NightjarActivity;
import com.limelight.ui.TopAppBar;
import com.limelight.utils.ServerHelper;
import com.limelight.utils.UiHelper;

import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.util.ArrayDeque;
import java.util.List;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

/**
 * The folders a PC shares with this device: browses them, downloads a file to Downloads (MediaStore from Android
 * 10, the system's save dialog before) and uploads one picked in the system's file picker. One transfer at a time,
 * as the host allows; more wait their turn in the order asked. Every failure stays on screen until the next action.
 */
public final class FolderBrowserActivity extends NightjarActivity {
    public static final String EXTRA_PC_UUID = "folder_browser_pc_uuid";
    public static final String EXTRA_PC_NAME = "folder_browser_pc_name";
    private static final int UPLOAD = 1;
    private static final int SAVE = 2;
    private static final String STATE_SAVE_FOLDER = "folder_browser_save_folder";
    private static final String STATE_SAVE_FILE = "folder_browser_save_file";
    private static final String STATE_SAVE_SIZE = "folder_browser_save_size";
    /** The file the save dialog is open for; kept in the instance state, as the app may die while it is open. */
    private String saveFolder, saveFile;
    /** Its listed size, -1 when not known. */
    private long saveSize = -1;
    private LinearLayout content;
    private TextView transferText;
    private LinearProgressIndicator transferBar;
    private FolderWork work;
    private boolean bound;
    private ComputerManagerService.ComputerManagerBinder manager;
    private final ServiceConnection service = new ServiceConnection() {
        @Override public void onServiceConnected(ComponentName name, IBinder binder) {
            manager = (ComputerManagerService.ComputerManagerBinder) binder;
            start();
        }
        @Override public void onServiceDisconnected(ComponentName name) { manager = null; }
    };

    @Override protected void onCreate(Bundle state) {
        UiHelper.setLocale(this);
        super.onCreate(state);
        if (state != null) {
            saveFolder = state.getString(STATE_SAVE_FOLDER);
            saveFile = state.getString(STATE_SAVE_FILE);
            saveSize = state.getLong(STATE_SAVE_SIZE, -1);
        }
        FrameLayout page = TopAppBar.setUpPage(this, R.string.folders_title, this::navigateBack);
        content = new LinearLayout(this);
        content.setOrientation(LinearLayout.VERTICAL);
        TopAppBar.addScrollingBody(page, content);
        work = new ViewModelProvider(this).get(FolderWork.class);
        work.changed.observe(this, ignored -> refresh());
        work.progressed.observe(this, run -> { if (run != null && run == work.transfer) showProgress(run); });
        getOnBackPressedDispatcher().addCallback(this, new OnBackPressedCallback(true) {
            @Override public void handleOnBackPressed() { navigateBack(); }
        });
        bound = bindService(new Intent(this, ComputerManagerService.class), service, BIND_AUTO_CREATE);
        refresh();
        UiHelper.notifyNewRootView(this);
    }

    /** Opens the shared folders of {@code computer}. */
    public static void open(android.app.Activity activity, ComputerDetails computer) {
        activity.startActivity(new Intent(activity, FolderBrowserActivity.class)
                .putExtra(EXTRA_PC_UUID, computer.uuid).putExtra(EXTRA_PC_NAME, computer.displayName()));
    }

    private void start() {
        String uuid = getIntent().getStringExtra(EXTRA_PC_UUID);
        ComputerDetails saved = uuid == null ? null : manager.getComputer(uuid);
        if (saved == null || saved.serverCert == null) {
            work.fail(getString(R.string.folders_pair_first));
            work.dropDeferred();
            return;
        }
        work.start(new ComputerDetails(saved), manager.getUniqueId());
    }

    private void navigateBack() {
        if (!work.up()) finish();
    }

    private void refresh() {
        content.removeAllViews();
        String pc = getIntent().getStringExtra(EXTRA_PC_NAME);
        // A transfer keeps its own folder and file, so the page stays usable while one runs or waits.
        boolean busy = work.loading;
        transferText = null;
        transferBar = null;
        text(R.style.TextAppearance_Nightjar_Body, work.folder == null
                ? getString(R.string.folders_on_pc, pc == null ? "" : pc)
                : work.folder.name + (work.path.isEmpty() ? "" : " / " + work.path.replace("/", " / ")));
        if (work.transfer != null) {
            transferText = text(R.style.TextAppearance_Nightjar_Supporting, "");
            transferBar = new LinearProgressIndicator(this);
            transferBar.setMax(1000);
            content.addView(transferBar);
            showProgress(work.transfer);
            if (!work.waiting.isEmpty()) {
                text(R.style.TextAppearance_Nightjar_Supporting, getString(R.string.folders_waiting, work.waiting.size()));
            }
            button(R.attr.nightjarButtonTonalStyle, getString(android.R.string.cancel), true, 0, work::cancelTransfer);
        } else if (work.loading) {
            text(R.style.TextAppearance_Nightjar_Supporting, getString(R.string.folders_loading));
            LinearProgressIndicator bar = new LinearProgressIndicator(this);
            bar.setIndeterminate(true);
            content.addView(bar);
        }
        if (work.error != null) {
            TextView error = text(R.style.TextAppearance_Nightjar_Body, work.error);
            error.setTextColor(MaterialColors.getColor(error, androidx.appcompat.R.attr.colorError));
            if (work.canRetry && !busy) {
                button(R.attr.nightjarButtonTonalStyle, getString(R.string.folders_retry), true, 0, work::reload);
            }
        }
        if (work.message != null) {
            text(R.style.TextAppearance_Nightjar_Supporting, work.message);
        }
        if (work.folder == null) {
            if (work.folders != null && work.folders.isEmpty()) {
                text(R.style.TextAppearance_Nightjar_Supporting, getString(R.string.folders_no_grant));
            }
            if (work.folders != null) {
                for (SharedFolders.Folder folder : work.folders) {
                    button(R.attr.nightjarButtonTextStyle, folder.name + "\n" + getString(folder.writable
                                    ? R.string.folders_read_write : R.string.folders_read_only), !busy,
                            R.drawable.ic_folder, () -> work.open(folder, ""));
                }
            }
        } else {
            button(R.attr.nightjarButtonTonalStyle, getString(work.path.isEmpty() ? R.string.folders_all
                    : R.string.folders_up), !busy, 0, work::up);
            if (work.folder.writable && work.listing != null) {
                button(com.google.android.material.R.attr.materialButtonStyle, getString(R.string.folders_upload), !busy,
                        R.drawable.ic_upload, () -> choose(new Intent(Intent.ACTION_OPEN_DOCUMENT)
                                .addCategory(Intent.CATEGORY_OPENABLE).setType("*/*"), UPLOAD));
            }
            if (work.listing != null && work.listing.entries.isEmpty()) {
                text(R.style.TextAppearance_Nightjar_Supporting, getString(R.string.folders_empty));
            }
            if (work.listing != null) {
                for (SharedFolders.Entry entry : work.listing.entries) {
                    entry(entry, busy);
                }
            }
        }
        com.limelight.ui.SubPageFocus.apply(this, findViewById(R.id.page_content));
    }

    private void showProgress(Transfer run) {
        if (transferText == null) return;
        long done = run.done, total = run.total;
        transferText.setText(getString(run.upload ? R.string.folders_uploading : R.string.folders_downloading, run.name,
                Formatter.formatShortFileSize(this, done), Formatter.formatShortFileSize(this, Math.max(total, done))));
        transferBar.setIndeterminate(total <= 0);
        if (total > 0) transferBar.setProgressCompat((int) (Math.min(done, total) * 1000 / total), false);
    }

    private void entry(SharedFolders.Entry entry, boolean busy) {
        String modified = entry.modifiedUnixMs > 0 ? DateUtils.formatDateTime(this, entry.modifiedUnixMs,
                DateUtils.FORMAT_SHOW_DATE | DateUtils.FORMAT_SHOW_TIME | DateUtils.FORMAT_ABBREV_ALL) : "";
        switch (entry.kind) {
            case DIR:
                button(R.attr.nightjarButtonTextStyle, entry.name + (modified.isEmpty() ? "" : "\n" + modified), !busy,
                        R.drawable.ic_folder, () -> work.open(work.folder, SharedFolders.childPath(work.path, entry.name)));
                break;
            case FILE:
                String size = Formatter.formatShortFileSize(this, entry.size);
                button(R.attr.nightjarButtonTextStyle, entry.name + "\n" + (modified.isEmpty() ? size : size + " · " + modified),
                        !busy && !work.queued(work.folder.id, SharedFolders.childPath(work.path, entry.name)),
                        R.drawable.ic_download, () -> download(entry));
                break;
            default:
                button(R.attr.nightjarButtonTextStyle, entry.name + "\n" + getString(R.string.folders_unsupported_entry),
                        false, R.drawable.ic_file, () -> { });
                break;
        }
    }

    private void download(SharedFolders.Entry entry) {
        String path = SharedFolders.childPath(work.path, entry.name);
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            work.download(work.folder.id, path, entry.size, null);
            return;
        }
        // Before Android 10 an app writes to Downloads only with storage permission: the user picks the file instead.
        saveFolder = work.folder.id;
        saveFile = path;
        saveSize = entry.size;
        choose(new Intent(Intent.ACTION_CREATE_DOCUMENT).addCategory(Intent.CATEGORY_OPENABLE)
                .setType(mimeType(entry.name)).putExtra(Intent.EXTRA_TITLE, entry.name), SAVE);
    }

    private TextView text(int appearance, CharSequence value) {
        TextView view = new TextView(this);
        view.setTextAppearance(this, appearance);
        view.setText(value);
        content.addView(view);
        return view;
    }

    private MaterialButton button(int styleAttr, String text, boolean enabled, int icon, Runnable action) {
        MaterialButton button = new MaterialButton(this, null, styleAttr);
        button.setText(text);
        button.setAllCaps(false);
        button.setEnabled(enabled);
        if (icon != 0) {
            button.setIconResource(icon);
        }
        if (styleAttr == R.attr.nightjarButtonTextStyle) {
            button.setGravity(Gravity.START | Gravity.CENTER_VERTICAL);
        }
        button.setOnClickListener(view -> action.run());
        content.addView(button);
        return button;
    }

    private void choose(Intent intent, int request) {
        try {
            startActivityForResult(intent, request);
        } catch (ActivityNotFoundException e) {
            saveFolder = saveFile = null;
            work.fail(getString(R.string.folders_no_picker));
        }
    }

    @Override protected void onSaveInstanceState(Bundle state) {
        super.onSaveInstanceState(state);
        state.putString(STATE_SAVE_FOLDER, saveFolder);
        state.putString(STATE_SAVE_FILE, saveFile);
        state.putLong(STATE_SAVE_SIZE, saveSize);
    }

    @Override protected void onActivityResult(int request, int result, Intent data) {
        super.onActivityResult(request, result, data);
        String folderId = saveFolder, file = saveFile;
        long size = saveSize;
        saveFolder = saveFile = null;
        saveSize = -1;
        if (result != RESULT_OK || data == null || data.getData() == null) return;
        if (request == UPLOAD) work.upload(data.getData());
        else if (request == SAVE && (folderId == null || file == null)) work.dropPicked(data.getData(), getString(R.string.folders_save_lost));
        else if (request == SAVE) work.download(folderId, file, size, data.getData());
    }

    @Override protected void onDestroy() {
        if (bound) unbindService(service);
        super.onDestroy();
    }

    static String mimeType(String name) {
        return FolderTransferIo.mimeType(name);
    }

    /** What to tell the user about a refusal; {@code upload} tells a missing write grant from a missing grant. */
    @StringRes
    static int refusalMessage(SharedFolders.Refusal refusal, boolean upload) {
        return FolderTransferIo.refusalMessage(refusal, upload);
    }

    /** A transfer in progress. */
    static final class Transfer {
        final boolean upload;
        /** The file's name; an upload learns it from the picked document's provider off the main thread. */
        volatile String name;
        final TransferCancel cancel = new TransferCancel();
        volatile long done, total;
        private long shownAt;
        /** What it does on the transfer thread, with the folder and file it was asked for; set before it is queued. */
        Runnable job;
        /** The folder and file of a download into Downloads, so it is not queued twice; null otherwise. */
        String key;
        /** The document picked for a download (before Android 10): deleted when the transfer is dropped unstarted. */
        Uri picked;

        Transfer(boolean upload, String name, long total) {
            this.upload = upload;
            this.name = name;
            this.total = total;
        }
    }

    /**
     * The browser's state, kept across rotation; read and written on the main thread only, the work runs on one
     * background thread. A load that ends after the user moved on is dropped by its generation.
     */
    public static final class FolderWork extends AndroidViewModel {
        /** The PC's HTTPS connection; tests stand in for it. */
        interface Connector {
            NvHTTP http() throws IOException;
        }

        final MutableLiveData<Long> changed = new MutableLiveData<>(0L);
        /** A transfer moved on; at most 5 times a second, besides its end. */
        final MutableLiveData<Transfer> progressed = new MutableLiveData<>();
        private final ExecutorService executor = Executors.newSingleThreadExecutor();
        private final ExecutorService transfers = Executors.newSingleThreadExecutor();
        private final Handler main = new Handler(Looper.getMainLooper());
        /** The page was left: a transfer that ends now starts nothing, the executors are shut down. */
        private boolean cleared;
        /** The PC; tests set it. */
        ComputerDetails computer;
        private String uniqueId;
        private int generation;
        List<SharedFolders.Folder> folders;
        SharedFolders.Folder folder;
        String path = "";
        SharedFolders.Listing listing;
        boolean loading, canRetry;
        String error, message;
        Transfer transfer;
        /** Transfers asked for while one runs, in the order asked; each starts when the one before it ends. */
        final ArrayDeque<Transfer> waiting = new ArrayDeque<>();
        Connector connector = this::connect;
        /** A save picked before the PC was known again, as after the app died with the save dialog open. */
        private String deferredFolder, deferredFile;
        private long deferredSize = -1;
        private Uri deferredTarget;

        public FolderWork(Application application) { super(application); }

        void start(ComputerDetails computer, String uniqueId) {
            if (this.computer != null) return;
            this.computer = computer;
            this.uniqueId = uniqueId;
            reload();
            if (deferredTarget != null) {
                Uri target = deferredTarget;
                deferredTarget = null;
                download(deferredFolder, deferredFile, deferredSize, target);
            }
        }

        /** Deletes the document a deferred save created, as the PC it was for is not known. */
        void dropDeferred() {
            if (deferredTarget == null) return;
            Uri target = deferredTarget;
            deferredTarget = null;
            dropPicked(target, error);
        }

        /**
         * Deletes the document the save dialog created for a download that will not run, and shows {@code why},
         * with where the document stays when it cannot be deleted.
         */
        void dropPicked(Uri document, String why) {
            ContentResolver resolver = getApplication().getContentResolver();
            transfers.execute(() -> {
                String left = discard(resolver, document, true);
                main.post(() -> {
                    error = left == null ? why : text(R.string.folders_partial_left, why, left);
                    canRetry = false;
                    changed();
                });
            });
        }

        void fail(String error) {
            this.error = error;
            canRetry = false;
            changed();
        }

        /** Loads the folder list, or the directory being shown. */
        void reload() {
            if (computer == null) return;
            if (folder == null) loadFolders();
            else open(folder, path);
        }

        private void loadFolders() {
            int current = begin();
            executor.execute(() -> {
                try {
                    NativeCapabilities capabilities = capabilities();
                    if (!capabilities.has(NativeCapabilities.FOLDERS)) {
                        finish(current, () -> {
                            folders = null;
                            error = text(R.string.folders_unsupported);
                            canRetry = true;
                        });
                        return;
                    }
                    List<SharedFolders.Folder> list = new SharedFolders(connector.http()).folders();
                    finish(current, () -> folders = list);
                } catch (IOException | NativeApiException | RuntimeException e) {
                    finish(current, () -> failed(e, false));
                }
            });
        }

        void open(SharedFolders.Folder target, String targetPath) {
            int current = begin();
            folder = target;
            path = targetPath;
            listing = null;
            changed();
            executor.execute(() -> {
                try {
                    SharedFolders.Listing result = new SharedFolders(connector.http()).entries(target.id, targetPath);
                    finish(current, () -> listing = result);
                } catch (IOException | RuntimeException e) {
                    // A RuntimeException is a folder id or path this side refuses; it must not end the app.
                    finish(current, () -> failed(e, false));
                }
            });
        }

        /** One level up, or false at the folder list. */
        boolean up() {
            // Leaving the page would cancel the transfers: back stays on the folder list while any runs or waits.
            if (folder == null) return transfer != null;
            if (path.isEmpty()) {
                folder = null;
                listing = null;
                if (folders == null) loadFolders();
                else {
                    generation++;
                    loading = false;
                    error = message = null;
                    changed();
                }
            } else {
                open(folder, SharedFolders.parentPath(path));
            }
            return true;
        }

        /**
         * Downloads {@code file} of the folder {@code folderId} into Downloads, or into {@code target} picked in the
         * save dialog. A picked document that gets no download is deleted, never left empty without a word.
         */
        void download(String folderId, String file, Uri target) {
            download(folderId, file, -1, target);
        }

        /** {@code listed} is the size the directory listed, -1 when not known: the download stops there. */
        void download(String folderId, String file, long listed, Uri target) {
            if (computer == null && target != null) {
                // The app came back from the save dialog after it died: the PC is known again in start().
                deferredFolder = folderId;
                deferredFile = file;
                deferredSize = listed;
                deferredTarget = target;
                return;
            }
            if (computer == null) return;
            // A picked document is a file of its own; into Downloads, the same file is asked for once.
            if (target == null && queued(folderId, file)) return;
            String name = SharedFolders.fileName(file);
            Transfer run = new Transfer(false, name, -1);
            if (target == null) run.key = key(folderId, file);
            run.picked = target;
            ContentResolver resolver = getApplication().getContentResolver();
            run.job = () -> {
                Uri uri = target;
                try {
                    if (uri == null) uri = FolderTransferIo.pendingDownload(resolver, name);
                    try (OutputStream out = FolderTransferIo.local(FolderTransferIo.openOutput(resolver, uri), name)) {
                        new SharedFolders(connector.http()).download(folderId, file, listed, out,
                                (bytes, total) -> progress(run, bytes, total), run.cancel);
                    }
                    if (target == null) FolderTransferIo.publish(resolver, uri);
                    // Downloads may have stored it under another name, as "a (1).txt" next to an "a.txt".
                    String stored = FolderTransferIo.displayName(resolver, uri, name);
                    end(run, null, text(target == null ? R.string.folders_downloaded : R.string.folders_saved, stored),
                            false, null);
                } catch (IOException | RuntimeException e) {
                    end(run, e, null, false, uri == null ? null : discard(resolver, uri, target != null));
                }
            };
            enqueue(run);
        }

        /** Whether a download of {@code file} of the folder {@code folderId} into Downloads runs or waits. */
        boolean queued(String folderId, String file) {
            String key = key(folderId, file);
            if (transfer != null && key.equals(transfer.key)) return true;
            for (Transfer run : waiting) {
                if (key.equals(run.key)) return true;
            }
            return false;
        }

        private static String key(String folderId, String file) { return folderId + "\n" + file; }

        /** Uploads the picked document into the directory being shown. */
        void upload(Uri source) {
            if (folder == null || !folder.writable) return;
            ContentResolver resolver = getApplication().getContentResolver();
            SharedFolders.Folder into = folder;
            String dir = path;
            String shown = source.getLastPathSegment();
            Transfer run = new Transfer(true, shown == null ? "" : shown, -1);
            run.job = () -> {
                // The provider answers on its own time: asked here, not on the main thread.
                FolderTransferIo.UploadMeta meta;
                try {
                    meta = FolderTransferIo.queryUpload(resolver, source);
                } catch (SharedFolders.LocalFileException e) {
                    end(run, e, null, false, null);
                    return;
                }
                String name = meta.name;
                long size = meta.size;
                if (name == null || name.isEmpty() || name.contains("/") || SharedFolders.hasPartSuffix(name)) {
                    end(run, text(R.string.folders_invalid_name));
                    return;
                }
                if (size < 0) {
                    end(run, text(R.string.folders_size_unknown));
                    return;
                }
                run.name = name;
                run.total = size;
                try (InputStream in = FolderTransferIo.local(FolderTransferIo.openInput(resolver, source), name)) {
                    String stored = new SharedFolders(connector.http()).upload(into.id, SharedFolders.childPath(dir, name),
                            in, size, (bytes, total) -> progress(run, bytes, total), run.cancel);
                    end(run, null, text(R.string.folders_uploaded, stored), true, null);
                } catch (IOException | RuntimeException e) {
                    end(run, e, null, false, null);
                }
            };
            enqueue(run);
        }

        /** Cancel ends the transfer that runs and drops those that wait: one press leaves nothing going. */
        void cancelTransfer() {
            dropWaiting();
            if (transfer != null) transfer.cancel.cancel();
        }

        /** Drops the waiting transfers; a document picked for one of them is deleted, never left empty. */
        private void dropWaiting() {
            java.util.List<Uri> picked = new java.util.ArrayList<>();
            for (Transfer run : waiting) {
                if (run.picked != null) picked.add(run.picked);
            }
            waiting.clear();
            if (picked.isEmpty()) return;
            ContentResolver resolver = getApplication().getContentResolver();
            // Its own thread: the transfer thread is busy with the one that runs, or shut down with the page.
            new Thread(() -> {
                for (Uri document : picked) discard(resolver, document, true);
            }, "Folder cleanup").start();
        }

        /** Starts {@code run}, or lets it wait behind the one running. */
        private void enqueue(Transfer run) {
            if (transfer != null) {
                waiting.add(run);
            } else {
                transfer = run;
                error = message = null;
                transfers.execute(run.job);
            }
            changed();
        }

        /** Starts the next waiting transfer once one ended; what the last one showed stays until the next action. */
        private void next() {
            Transfer run = waiting.poll();
            if (run == null) return;
            transfer = run;
            transfers.execute(run.job);
        }

        private void progress(Transfer run, long done, long total) {
            run.done = done;
            run.total = total;
            long now = android.os.SystemClock.elapsedRealtime();
            if (now - run.shownAt >= 200 || done == total) {
                run.shownAt = now;
                progressed.postValue(run);
            }
        }

        /** Ends {@code run} with {@code error}, a refusal found on this side before anything was sent. */
        private void end(Transfer run, String error) {
            main.post(() -> {
                if (cleared || transfer != run) return;
                transfer = null;
                this.error = error;
                canRetry = false;
                next();
                changed();
            });
        }

        /** Ends {@code run}; {@code left} names a partial file a failure could not delete. */
        private void end(Transfer run, Exception failure, String done, boolean reload, String left) {
            main.post(() -> {
                if (cleared || transfer != run) return;
                transfer = null;
                if (failure != null) {
                    failed(failure, run.upload);
                    if (left != null) error = text(R.string.folders_partial_left, error, left);
                    canRetry = false;
                } else {
                    message = done;
                }
                if (reload && folder != null) {
                    // An earlier transfer's failure stays shown as well.
                    String shown = message, kept = error;
                    open(folder, path);
                    message = shown;
                    error = kept;
                }
                next();
                changed();
            });
        }

        static String discard(ContentResolver resolver, Uri uri, boolean document) {
            return FolderTransferIo.discard(resolver, uri, document);
        }

        private void failed(Exception e, boolean upload) {
            FolderTransferIo.Failure failure = FolderTransferIo.describeFailure(getApplication(), e, upload);
            error = failure.message;
            canRetry = failure.canRetry;
        }

        private int begin() {
            loading = true;
            error = message = null;
            changed();
            return ++generation;
        }

        private void finish(int current, Runnable apply) {
            main.post(() -> {
                if (current != generation) return;
                loading = false;
                apply.run();
                changed();
            });
        }

        private NvHTTP connect() throws IOException {
            return new NvHTTP(ServerHelper.getCurrentAddressFromComputer(computer), computer.httpsPort, uniqueId,
                    computer.serverCert, PlatformBinding.getCryptoProvider(getApplication()));
        }

        private NativeCapabilities capabilities() throws IOException, NativeApiException {
            HostEndpoint endpoint = HostEndpoint.of(computer.uuid, ServerHelper.getCurrentAddressFromComputer(computer),
                    computer.httpsPort, computer.serverCert, uniqueId, PlatformBinding.getCryptoProvider(getApplication()));
            if (endpoint == null) throw new IOException("The PC's certificate is not known");
            return new NativeApiClient(new JniNativeApiTransport(), endpoint).capabilities();
        }

        private String text(int id, Object... args) { return getApplication().getString(id, args); }

        private void changed() { changed.setValue(changed.getValue() + 1); }

        @Override protected void onCleared() {
            cleared = true;
            cancelTransfer();
            executor.shutdownNow();
            transfers.shutdownNow();
        }
    }
}
