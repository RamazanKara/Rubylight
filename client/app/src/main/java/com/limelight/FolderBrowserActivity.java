package com.limelight;

import android.app.Application;
import android.content.ActivityNotFoundException;
import android.content.ComponentName;
import android.content.ContentResolver;
import android.content.ContentValues;
import android.content.Intent;
import android.content.ServiceConnection;
import android.database.Cursor;
import android.net.Uri;
import android.os.Build;
import android.os.Bundle;
import android.os.Handler;
import android.os.IBinder;
import android.os.Looper;
import android.provider.DocumentsContract;
import android.provider.MediaStore;
import android.provider.OpenableColumns;
import android.text.format.DateUtils;
import android.text.format.Formatter;
import android.view.Gravity;
import android.webkit.MimeTypeMap;
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
import java.util.List;
import java.util.Locale;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

/**
 * The folders a PC shares with this device: browses them, downloads a file to Downloads (MediaStore from Android
 * 10, the system's save dialog before) and uploads one picked in the system's file picker. One transfer at a time,
 * as the host allows; every failure stays on screen until the next action.
 */
public final class FolderBrowserActivity extends NightjarActivity {
    public static final String EXTRA_PC_UUID = "folder_browser_pc_uuid";
    public static final String EXTRA_PC_NAME = "folder_browser_pc_name";
    private static final int UPLOAD = 1;
    private static final int SAVE = 2;
    private static final String STATE_SAVE_FOLDER = "folder_browser_save_folder";
    private static final String STATE_SAVE_FILE = "folder_browser_save_file";
    /** The file the save dialog is open for; kept in the instance state, as the app may die while it is open. */
    private String saveFolder, saveFile;
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
        boolean busy = work.loading || work.transfer != null;
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
                        !busy, R.drawable.ic_download, () -> download(entry));
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
            work.download(work.folder.id, path, null);
            return;
        }
        // Before Android 10 an app writes to Downloads only with storage permission: the user picks the file instead.
        saveFolder = work.folder.id;
        saveFile = path;
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
    }

    @Override protected void onActivityResult(int request, int result, Intent data) {
        super.onActivityResult(request, result, data);
        String folderId = saveFolder, file = saveFile;
        saveFolder = saveFile = null;
        if (result != RESULT_OK || data == null || data.getData() == null) return;
        if (request == UPLOAD) work.upload(data.getData());
        else if (request == SAVE && (folderId == null || file == null)) work.dropPicked(data.getData(), getString(R.string.folders_save_lost));
        else if (request == SAVE) work.download(folderId, file, data.getData());
    }

    @Override protected void onDestroy() {
        if (bound) unbindService(service);
        super.onDestroy();
    }

    static String mimeType(String name) {
        int dot = name.lastIndexOf('.');
        String type = dot < 0 ? null : MimeTypeMap.getSingleton()
                .getMimeTypeFromExtension(name.substring(dot + 1).toLowerCase(Locale.ROOT));
        return type == null ? "application/octet-stream" : type;
    }

    /** What to tell the user about a refusal; {@code upload} tells a missing write grant from a missing grant. */
    @StringRes
    static int refusalMessage(SharedFolders.Refusal refusal, boolean upload) {
        switch (refusal) {
            case FORBIDDEN: return upload ? R.string.folders_upload_forbidden : R.string.folders_forbidden;
            case FOLDERS_DISABLED: return R.string.folders_disabled;
            case NOT_FOUND: return R.string.folders_not_found;
            case INVALID_NAME: return R.string.folders_invalid_name;
            case INCOMPLETE_UPLOAD:
            case LENGTH_REQUIRED: return R.string.folders_incomplete;
            case TOO_LARGE: return R.string.folders_too_large;
            case INSUFFICIENT_STORAGE: return R.string.folders_storage_full;
            case NO_CONSOLE_USER: return R.string.folders_no_console_user;
            case TRANSFER_BUSY: return R.string.folders_busy;
            default: throw new IllegalArgumentException(refusal.name());
        }
    }

    /** A transfer in progress. */
    static final class Transfer {
        final boolean upload;
        /** The file's name; an upload learns it from the picked document's provider off the main thread. */
        volatile String name;
        final TransferCancel cancel = new TransferCancel();
        volatile long done, total;
        private long shownAt;

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
        private ComputerDetails computer;
        private String uniqueId;
        private int generation;
        List<SharedFolders.Folder> folders;
        SharedFolders.Folder folder;
        String path = "";
        SharedFolders.Listing listing;
        boolean loading, canRetry;
        String error, message;
        Transfer transfer;
        Connector connector = this::connect;
        /** A save picked before the PC was known again, as after the app died with the save dialog open. */
        private String deferredFolder, deferredFile;
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
                download(deferredFolder, deferredFile, target);
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
            if (transfer != null) return true;
            if (folder == null) return false;
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
            if (computer == null && target != null) {
                // The app came back from the save dialog after it died: the PC is known again in start().
                deferredFolder = folderId;
                deferredFile = file;
                deferredTarget = target;
                return;
            }
            if (computer == null) return;
            if (transfer != null) {
                if (target != null) dropPicked(target, text(R.string.folders_busy));
                return;
            }
            String name = SharedFolders.fileName(file);
            Transfer run = begin(new Transfer(false, name, -1));
            ContentResolver resolver = getApplication().getContentResolver();
            transfers.execute(() -> {
                Uri uri = target;
                try {
                    if (uri == null) uri = pendingDownload(resolver, name);
                    try (OutputStream out = local(openOutput(resolver, uri), name)) {
                        new SharedFolders(connector.http()).download(folderId, file, out,
                                (bytes, total) -> progress(run, bytes, total), run.cancel);
                    }
                    if (target == null) publish(resolver, uri);
                    // Downloads may have stored it under another name, as "a (1).txt" next to an "a.txt".
                    String stored = displayName(resolver, uri, name);
                    end(run, null, text(target == null ? R.string.folders_downloaded : R.string.folders_saved, stored),
                            false, null);
                } catch (IOException | RuntimeException e) {
                    end(run, e, null, false, uri == null ? null : discard(resolver, uri, target != null));
                }
            });
        }

        /** Uploads the picked document into the directory being shown. */
        void upload(Uri source) {
            if (transfer != null || folder == null || !folder.writable) return;
            ContentResolver resolver = getApplication().getContentResolver();
            SharedFolders.Folder into = folder;
            String dir = path;
            String shown = source.getLastPathSegment();
            Transfer run = begin(new Transfer(true, shown == null ? "" : shown, -1));
            transfers.execute(() -> {
                // The provider answers on its own time: asked here, not on the main thread.
                String name = null;
                long size = -1;
                try (Cursor cursor = resolver.query(source, new String[] {OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE},
                        null, null, null)) {
                    if (cursor != null && cursor.moveToFirst()) {
                        name = cursor.isNull(0) ? null : cursor.getString(0);
                        size = cursor.isNull(1) ? -1 : cursor.getLong(1);
                    }
                } catch (RuntimeException e) {
                    end(run, new SharedFolders.LocalFileException(new IOException(e.getMessage(), e)), null, false, null);
                    return;
                }
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
                try (InputStream in = local(openInput(resolver, source), name)) {
                    String stored = new SharedFolders(connector.http()).upload(into.id, SharedFolders.childPath(dir, name),
                            in, size, (bytes, total) -> progress(run, bytes, total), run.cancel);
                    end(run, null, text(R.string.folders_uploaded, stored), true, null);
                } catch (IOException | RuntimeException e) {
                    end(run, e, null, false, null);
                }
            });
        }

        /** Opening a document fails on this device, not on the network. */
        private static OutputStream openOutput(ContentResolver resolver, Uri uri) throws SharedFolders.LocalFileException {
            try {
                return resolver.openOutputStream(uri, "wt");
            } catch (IOException e) {
                throw new SharedFolders.LocalFileException(e);
            }
        }

        private static InputStream openInput(ContentResolver resolver, Uri uri) throws SharedFolders.LocalFileException {
            try {
                return resolver.openInputStream(uri);
            } catch (IOException e) {
                throw new SharedFolders.LocalFileException(e);
            }
        }

        /** {@code out}, whose flush and close failures are this device's, not the network's. */
        private static OutputStream local(OutputStream out, String name) throws SharedFolders.LocalFileException {
            if (out == null) throw new SharedFolders.LocalFileException(new IOException("Cannot open " + name));
            return new java.io.FilterOutputStream(out) {
                @Override public void write(byte[] bytes, int offset, int length) throws IOException {
                    out.write(bytes, offset, length);
                }
                @Override public void close() throws IOException {
                    try {
                        super.close();
                    } catch (SharedFolders.LocalFileException e) {
                        throw e;
                    } catch (IOException e) {
                        throw new SharedFolders.LocalFileException(e);
                    }
                }
            };
        }

        /** {@code in}, whose close failure is this device's, not the network's. */
        private static InputStream local(InputStream in, String name) throws SharedFolders.LocalFileException {
            if (in == null) throw new SharedFolders.LocalFileException(new IOException("Cannot open " + name));
            return new java.io.FilterInputStream(in) {
                @Override public void close() throws IOException {
                    try {
                        super.close();
                    } catch (IOException e) {
                        throw new SharedFolders.LocalFileException(e);
                    }
                }
            };
        }

        /** The name the provider shows for {@code uri}, or {@code name} when it gives none. */
        private static String displayName(ContentResolver resolver, Uri uri, String name) {
            try (Cursor cursor = resolver.query(uri, new String[] {OpenableColumns.DISPLAY_NAME}, null, null, null)) {
                if (cursor != null && cursor.moveToFirst() && !cursor.isNull(0) && !cursor.getString(0).isEmpty()) {
                    return cursor.getString(0);
                }
            } catch (RuntimeException e) {
                LimeLog.warning("No display name for " + uri + ": " + e.getMessage());
            }
            return name;
        }

        void cancelTransfer() {
            if (transfer != null) transfer.cancel.cancel();
        }

        private Transfer begin(Transfer run) {
            transfer = run;
            error = message = null;
            changed();
            return run;
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
                if (transfer != run) return;
                transfer = null;
                this.error = error;
                canRetry = false;
                changed();
            });
        }

        /** Ends {@code run}; {@code left} names a partial file a failure could not delete. */
        private void end(Transfer run, Exception failure, String done, boolean reload, String left) {
            main.post(() -> {
                if (transfer != run) return;
                transfer = null;
                if (failure != null) {
                    failed(failure, run.upload);
                    if (left != null) error = text(R.string.folders_partial_left, error, left);
                    canRetry = false;
                } else {
                    message = done;
                }
                if (reload && folder != null) {
                    String shown = message;
                    open(folder, path);
                    message = shown;
                }
                changed();
            });
        }

        /** A new row in Downloads, hidden from other apps until {@link #publish}. */
        private static Uri pendingDownload(ContentResolver resolver, String name) throws IOException {
            if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) throw new IllegalStateException("Downloads needs Android 10");
            ContentValues values = new ContentValues();
            values.put(MediaStore.MediaColumns.DISPLAY_NAME, name);
            values.put(MediaStore.MediaColumns.MIME_TYPE, mimeType(name));
            values.put(MediaStore.MediaColumns.IS_PENDING, 1);
            Uri uri = resolver.insert(MediaStore.Downloads.EXTERNAL_CONTENT_URI, values);
            if (uri == null) throw new SharedFolders.LocalFileException(new IOException("Downloads did not take " + name));
            return uri;
        }

        private static void publish(ContentResolver resolver, Uri uri) throws IOException {
            if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) throw new IllegalStateException("Downloads needs Android 10");
            ContentValues values = new ContentValues();
            values.put(MediaStore.MediaColumns.IS_PENDING, 0);
            if (resolver.update(uri, values, null, null) != 1) throw new SharedFolders.LocalFileException(new IOException("Downloads did not publish " + uri));
        }

        /**
         * Deletes what a failed download left: the pending Downloads row, or the document the user picked. Returns
         * null once it is gone, or the name it stays under for the user to delete.
         */
        static String discard(ContentResolver resolver, Uri uri, boolean document) {
            try {
                boolean gone = document ? DocumentsContract.deleteDocument(resolver, uri)
                        : resolver.delete(uri, null, null) > 0;
                if (gone) return null;
                LimeLog.warning("Failed download not removed: " + uri);
            } catch (Exception e) {
                LimeLog.warning("Failed download not removed: " + uri + ": " + e.getMessage());
            }
            return displayName(resolver, uri, uri.toString());
        }

        private void failed(Exception e, boolean upload) {
            canRetry = !(e instanceof SharedFolders.RefusedException)
                    || ((SharedFolders.RefusedException) e).refusal != SharedFolders.Refusal.FORBIDDEN;
            if (e instanceof SharedFolders.CancelledException) {
                error = text(R.string.folders_cancelled);
            } else if (e instanceof SharedFolders.RefusedException) {
                error = text(refusalMessage(((SharedFolders.RefusedException) e).refusal, upload));
            } else if (e instanceof SharedFolders.InvalidPathException) {
                error = text(R.string.folders_bad_path, String.valueOf(e.getMessage()));
                canRetry = false;
            } else if (e instanceof SharedFolders.LocalFileException || e instanceof SecurityException) {
                error = text(R.string.folders_local_failed, String.valueOf(e.getMessage()));
            } else {
                error = text(R.string.folders_failed, String.valueOf(e.getMessage()));
            }
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
            cancelTransfer();
            executor.shutdownNow();
            transfers.shutdownNow();
        }
    }
}
