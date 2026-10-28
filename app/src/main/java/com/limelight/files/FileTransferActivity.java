package com.limelight.files;

import android.content.ComponentName;
import android.content.ContentResolver;
import android.content.Context;
import android.content.Intent;
import android.content.ServiceConnection;
import android.content.pm.ProviderInfo;
import android.database.Cursor;
import android.net.Uri;
import android.os.Build;
import android.os.Bundle;
import android.os.Handler;
import android.os.IBinder;
import android.os.Looper;
import android.os.ParcelFileDescriptor;
import android.provider.DocumentsContract;
import android.provider.OpenableColumns;
import android.text.TextUtils;
import android.text.format.DateUtils;
import android.text.format.Formatter;
import android.util.TypedValue;
import android.view.View;
import android.widget.Button;
import android.widget.FrameLayout;
import android.widget.LinearLayout;
import android.widget.ProgressBar;
import android.widget.ScrollView;
import android.widget.TextView;
import android.widget.Toast;

import androidx.activity.result.ActivityResultLauncher;
import androidx.activity.result.contract.ActivityResultContracts;

import com.limelight.LimeLog;
import com.limelight.R;
import com.limelight.binding.PlatformBinding;
import com.limelight.computers.ComputerManagerListener;
import com.limelight.computers.ComputerManagerService;
import com.limelight.nativeapi.HostEndpoint;
import com.limelight.nativeapi.JniNativeApiTransport;
import com.limelight.nativeapi.NativeApiClient;
import com.limelight.nativeapi.NativeApiException;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.PairingManager;
import com.limelight.ui.NightjarActivity;
import com.limelight.ui.TopAppBar;
import com.limelight.utils.UiHelper;

import org.json.JSONException;

import java.io.FileNotFoundException;
import java.io.IOException;
import java.util.ArrayList;
import java.util.Collections;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

/**
 * File transfer (Experimental) with one PC: send files (from a picker or another app's share sheet), see the
 * PC's file transfer folder and download from it, with progress, cancel and try again. Opened from the PC
 * menu and the stream menu with {@link #EXTRA_PC_UUID}; a share always asks which PC first, and sends only
 * another app's files.
 *
 * <p>Transfers run while the screen exists, also in the background; leaving it cancels what has not finished.
 */
public class FileTransferActivity extends NightjarActivity implements FileTransferController.Ui {
    public static final String EXTRA_PC_UUID = "files_pc_uuid";

    private final Handler handler = new Handler(Looper.getMainLooper());
    private final ExecutorService workers = Executors.newCachedThreadPool(r -> new Thread(r, "File transfer"));
    private final Map<String, ComputerDetails> computers = new LinkedHashMap<>();
    /** Shared with this screen and waiting for a PC that may receive them. */
    private final List<Uri> waiting = new ArrayList<>();

    private ComputerManagerService.ComputerManagerBinder manager;
    private boolean bound;
    private boolean foreground;
    private boolean polling;
    private String uuid;
    private ComputerDetails computer;
    private FileTransferEngine engine;
    private FileTransferController controller;
    private FileProblems.Access uploadAccess = FileProblems.Access.CHECKING;
    private FileProblems.Access downloadAccess = FileProblems.Access.CHECKING;
    private int generation;
    private RemoteFiles.Entry pendingDownload;
    private long pendingRetry = -1;

    private TextView status;
    private LinearLayout pcs;
    private Button send;
    private Button refresh;
    private TextView transfersHeading;
    private LinearLayout transfers;
    private Button clearFinished;
    private TextView filesHeading;
    private TextView folder;
    private LinearLayout files;

    private final ActivityResultLauncher<String[]> picker = registerForActivityResult(
            new ActivityResultContracts.OpenMultipleDocuments(), this::onPicked);
    private final ActivityResultLauncher<String> creator = registerForActivityResult(
            new ActivityResultContracts.CreateDocument("application/octet-stream"), this::onCreated);

    private final ServiceConnection service = new ServiceConnection() {
        @Override public void onServiceConnected(ComponentName name, IBinder binder) {
            ComputerManagerService.ComputerManagerBinder candidate = (ComputerManagerService.ComputerManagerBinder) binder;
            workers.execute(() -> {
                if (!candidate.waitForReady()) return;
                handler.post(() -> {
                    if (isFinishing() || isDestroyed()) return;
                    manager = candidate;
                    if (uuid != null) {
                        ComputerDetails known = manager.getComputer(uuid);
                        if (known != null) onComputer(new ComputerDetails(known));
                    }
                    startPolling();
                    render();
                });
            });
        }

        @Override public void onServiceDisconnected(ComponentName name) {
            manager = null;
            polling = false;
        }
    };

    @Override
    protected void onCreate(Bundle state) {
        UiHelper.setLocale(this);
        super.onCreate(state);
        FrameLayout content = TopAppBar.setUpPage(this, R.string.files_title, this::finish);
        content.addView(buildPage());
        uuid = chosenPc(getIntent());
        int refused = 0;
        for (Uri uri : sharedUris(getIntent())) {
            if (sendable(uri, getPackageName(), providerPackage(uri))) waiting.add(uri);
            else refused++;
        }
        if (refused > 0) {
            LimeLog.warning("File transfer: refused " + refused + " shared items that are not another app's files");
            Toast.makeText(this, getString(R.string.files_share_refused, refused), Toast.LENGTH_LONG).show();
        }
        engine = FileTransferEngine.jni();
        if (engine != null) {
            bound = bindService(new Intent(this, ComputerManagerService.class), service, BIND_AUTO_CREATE);
        }
        render();
        UiHelper.notifyNewRootView(this);
    }

    /** Opens the screen for one PC, from the PC menu or the stream menu. */
    public static Intent forPc(Context context, String pcUuid) {
        return new Intent(context, FileTransferActivity.class).putExtra(EXTRA_PC_UUID, pcUuid);
    }

    /**
     * The PC the screen is for, or null to ask. Only Nightjar's menus name one: a share always asks, so another
     * app cannot have files sent to a PC without the user choosing it.
     */
    static String chosenPc(Intent intent) {
        String action = intent.getAction();
        boolean share = Intent.ACTION_SEND.equals(action) || Intent.ACTION_SEND_MULTIPLE.equals(action);
        return share ? null : intent.getStringExtra(EXTRA_PC_UUID);
    }

    /**
     * Whether a shared file may be sent: a content URI that Nightjar's own providers do not serve. A file path or
     * one of Nightjar's providers would let any app have Nightjar send its private files (keys, settings).
     */
    static boolean sendable(Uri uri, String ownPackage, String providerPackage) {
        return ContentResolver.SCHEME_CONTENT.equals(uri.getScheme()) && !TextUtils.isEmpty(uri.getAuthority())
                && !ownPackage.equals(providerPackage);
    }

    /** The app whose provider serves {@code uri}, or null when none does (then the file fails to open). */
    private String providerPackage(Uri uri) {
        if (TextUtils.isEmpty(uri.getAuthority())) return null;
        ProviderInfo provider = getPackageManager().resolveContentProvider(uri.getAuthority(), 0);
        return provider == null ? null : provider.packageName;
    }

    /** The files of an ACTION_SEND or ACTION_SEND_MULTIPLE, in the order shared. */
    static List<Uri> sharedUris(Intent intent) {
        List<Uri> uris = new ArrayList<>();
        if (Intent.ACTION_SEND.equals(intent.getAction())) {
            Uri uri = intent.getParcelableExtra(Intent.EXTRA_STREAM);
            if (uri != null) uris.add(uri);
        } else if (Intent.ACTION_SEND_MULTIPLE.equals(intent.getAction())) {
            List<Uri> shared = intent.getParcelableArrayListExtra(Intent.EXTRA_STREAM);
            if (shared != null) {
                for (Uri uri : shared) if (uri != null) uris.add(uri);
            }
        }
        return uris;
    }

    private View buildPage() {
        ScrollView scroll = new ScrollView(this);
        LinearLayout page = new LinearLayout(this);
        page.setOrientation(LinearLayout.VERTICAL);
        int pad = dp(16);
        page.setPadding(pad, pad, pad, pad);
        scroll.addView(page);

        status = text(page, android.R.style.TextAppearance_Material_Body1);
        pcs = new LinearLayout(this);
        pcs.setOrientation(LinearLayout.VERTICAL);
        page.addView(pcs);

        LinearLayout actions = new LinearLayout(this);
        actions.setOrientation(LinearLayout.HORIZONTAL);
        page.addView(actions);
        send = button(actions, R.string.files_send);
        send.setOnClickListener(v -> picker.launch(new String[] {"*/*"}));
        refresh = button(actions, R.string.files_refresh);
        refresh.setOnClickListener(v -> loadAccess());

        transfersHeading = text(page, android.R.style.TextAppearance_Material_Title);
        transfersHeading.setText(R.string.files_transfers);
        transfers = new LinearLayout(this);
        transfers.setOrientation(LinearLayout.VERTICAL);
        page.addView(transfers);
        clearFinished = button(page, R.string.files_clear_finished);
        clearFinished.setOnClickListener(v -> {
            if (controller != null) controller.clearFinished();
        });

        filesHeading = text(page, android.R.style.TextAppearance_Material_Title);
        filesHeading.setText(R.string.files_on_pc);
        folder = text(page, android.R.style.TextAppearance_Material_Body1);
        files = new LinearLayout(this);
        files.setOrientation(LinearLayout.VERTICAL);
        page.addView(files);
        return scroll;
    }

    private int dp(int value) {
        return (int) TypedValue.applyDimension(TypedValue.COMPLEX_UNIT_DIP, value, getResources().getDisplayMetrics());
    }

    private TextView text(LinearLayout parent, int appearance) {
        TextView view = new TextView(this);
        view.setTextAppearance(this, appearance);
        view.setPadding(0, dp(8), 0, dp(4));
        parent.addView(view);
        return view;
    }

    private Button button(LinearLayout parent, int label) {
        Button button = new com.google.android.material.button.MaterialButton(this, null,
                com.google.android.material.R.attr.materialButtonOutlinedStyle);
        button.setText(label);
        LinearLayout.LayoutParams params = new LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.WRAP_CONTENT, LinearLayout.LayoutParams.WRAP_CONTENT);
        params.setMarginEnd(dp(8));
        parent.addView(button, params);
        return button;
    }

    private void startPolling() {
        if (manager == null || !foreground || polling) return;
        polling = true;
        manager.startPolling(new ComputerManagerListener() {
            @Override public void notifyComputerUpdated(ComputerDetails details) {
                ComputerDetails copy = new ComputerDetails(details);
                handler.post(() -> onComputer(copy));
            }
        });
    }

    private void stopPolling() {
        if (manager != null && polling) manager.stopPolling();
        polling = false;
    }

    private void onComputer(ComputerDetails details) {
        if (isFinishing() || isDestroyed() || details.uuid == null) return;
        computers.put(details.uuid, details);
        if (uuid != null && uuid.equalsIgnoreCase(details.uuid)) {
            boolean first = computer == null;
            computer = details;
            // The first time the PC is reachable: its endpoint and what this device may do there.
            if (controller == null && details.state == ComputerDetails.State.ONLINE) {
                HostEndpoint endpoint = manager == null ? null : HostEndpoint.of(details, manager.getUniqueId(),
                        PlatformBinding.getCryptoProvider(getApplicationContext()));
                if (endpoint != null) {
                    controller = new FileTransferController(engine, endpoint, workers, handler::post, this);
                    loadAccess();
                }
            } else if (first) {
                render();
            }
        }
        if (uuid == null) render();
    }

    /** Asks the PC what this device may do, then reads its folder. */
    private void loadAccess() {
        FileTransferController current = controller;
        if (current == null || computer == null) {
            render();
            return;
        }
        int asked = ++generation;
        uploadAccess = FileProblems.Access.CHECKING;
        downloadAccess = FileProblems.Access.CHECKING;
        status.setText(R.string.files_checking);
        HostCapabilities known = computer.caps;
        workers.execute(() -> {
            String failure = null;
            HostCapabilities caps = known;
            try {
                caps = known.withNativeCapabilities(
                        new NativeApiClient(new JniNativeApiTransport(), current.host()).fileCapabilities());
            } catch (NativeApiException | JSONException e) {
                failure = e.getMessage();
            }
            RemoteFiles list = null;
            if (failure == null && (FileProblems.access(caps, true) == FileProblems.Access.READY
                    || FileProblems.access(caps, false) == FileProblems.Access.READY)) {
                list = RemoteFiles.parse(engine.list(current.host()));
            }
            HostCapabilities read = caps;
            String problem = failure;
            RemoteFiles folderList = list;
            handler.post(() -> {
                if (asked != generation || isFinishing() || isDestroyed()) return;
                if (problem != null) {
                    uploadAccess = FileProblems.Access.CHECKING;
                    downloadAccess = FileProblems.Access.CHECKING;
                    render();
                    status.setText(getString(R.string.files_check_failed, problem));
                    return;
                }
                uploadAccess = FileProblems.access(read, true);
                downloadAccess = FileProblems.access(read, false);
                render();
                showFiles(folderList);
                sendWaiting();
            });
        });
    }

    private void refreshFiles() {
        FileTransferController current = controller;
        if (current == null) return;
        int asked = generation;
        workers.execute(() -> {
            RemoteFiles list = RemoteFiles.parse(engine.list(current.host()));
            handler.post(() -> {
                if (asked == generation && !isFinishing() && !isDestroyed()) showFiles(list);
            });
        });
    }

    private void sendWaiting() {
        if (uploadAccess != FileProblems.Access.READY || controller == null) return;
        List<Uri> now = new ArrayList<>(waiting);
        waiting.clear();
        for (Uri uri : now) queueUpload(uri);
        render();
    }

    private void onPicked(List<Uri> uris) {
        if (uris == null || uris.isEmpty()) return;
        waiting.addAll(uris);
        sendWaiting();
    }

    private void queueUpload(Uri uri) {
        ContentResolver resolver = getContentResolver();
        String name = null;
        long size = -1;
        try (Cursor cursor = resolver.query(uri, new String[] {OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE},
                null, null, null)) {
            if (cursor != null && cursor.moveToFirst()) {
                if (!cursor.isNull(0)) name = cursor.getString(0);
                if (!cursor.isNull(1)) size = cursor.getLong(1);
            }
        } catch (RuntimeException e) {
            LimeLog.warning("File transfer: no details for a shared file: " + e.getMessage());
        }
        if (name == null) name = uri.getLastPathSegment() != null ? uri.getLastPathSegment() : "file";
        if (size < 0) {
            try (ParcelFileDescriptor fd = resolver.openFileDescriptor(uri, "r")) {
                if (fd != null) size = fd.getStatSize();
            } catch (IOException | RuntimeException e) {
                LimeLog.warning("File transfer: could not open a shared file: " + e.getMessage());
            }
        }
        if (size < 0) {
            // The host needs the length before the first byte.
            status.setText(getString(R.string.files_size_unknown, name));
            return;
        }
        controller.send(name, size, () -> detach(resolver.openFileDescriptor(uri, "r")));
    }

    private static int detach(ParcelFileDescriptor fd) throws IOException {
        if (fd == null) throw new FileNotFoundException("no descriptor");
        return fd.detachFd();
    }

    private void download(RemoteFiles.Entry entry) {
        pendingDownload = entry;
        pendingRetry = -1;
        creator.launch(entry.name);
    }

    private void onCreated(Uri uri) {
        RemoteFiles.Entry entry = pendingDownload;
        long retry = pendingRetry;
        pendingDownload = null;
        pendingRetry = -1;
        if (uri == null || controller == null) return;
        ContentResolver resolver = getContentResolver();
        FileTransferController.Opener opener = () -> detach(resolver.openFileDescriptor(uri, "w"));
        Runnable discard = () -> {
            try {
                DocumentsContract.deleteDocument(resolver, uri);
            } catch (FileNotFoundException | RuntimeException e) {
                LimeLog.warning("File transfer: could not delete an unfinished download: " + e.getMessage());
            }
        };
        if (retry >= 0) {
            controller.retryDownload(retry, opener, discard);
        } else if (entry != null) {
            controller.download(entry.name, displayName(uri, entry.name), entry.size, opener, discard);
        } else {
            // The screen was recreated while the picker was open: nothing to download into it.
            workers.execute(discard);
        }
    }

    private String displayName(Uri uri, String fallback) {
        try (Cursor cursor = getContentResolver().query(uri, new String[] {OpenableColumns.DISPLAY_NAME},
                null, null, null)) {
            if (cursor != null && cursor.moveToFirst() && !cursor.isNull(0)) return cursor.getString(0);
        } catch (RuntimeException ignored) {
            // The name the PC has is as good.
        }
        return fallback;
    }

    @Override
    public void onTransfersChanged(List<FileRow> rows) {
        if (isFinishing() || isDestroyed()) return;
        showTransfers(rows);
    }

    @Override
    public void onUploaded() {
        refreshFiles();
    }

    private void render() {
        if (status == null) return;
        boolean picking = uuid == null;
        pcs.removeAllViews();
        int note;
        if (engine == null) {
            note = R.string.files_no_library;
        } else if (picking) {
            note = showPcs() ? R.string.files_pick_pc : R.string.files_no_pc;
        } else if (computer == null || controller == null) {
            note = computer == null ? R.string.files_checking : R.string.files_offline;
        } else if (uploadAccess != FileProblems.Access.READY) {
            note = FileProblems.accessText(uploadAccess, true);
        } else {
            note = FileProblems.accessText(downloadAccess, false);
        }
        status.setText(note == 0 ? "" : getString(note));
        status.setVisibility(note == 0 && waiting.isEmpty() ? View.GONE : View.VISIBLE);
        if (!waiting.isEmpty() && !picking) {
            status.append((note == 0 ? "" : "\n\n") + getString(R.string.files_waiting_to_send, waiting.size()));
        }
        boolean open = controller != null;
        send.setVisibility(open ? View.VISIBLE : View.GONE);
        send.setEnabled(uploadAccess == FileProblems.Access.READY);
        refresh.setVisibility(open ? View.VISIBLE : View.GONE);
        boolean listed = open && (uploadAccess == FileProblems.Access.READY || downloadAccess == FileProblems.Access.READY);
        filesHeading.setVisibility(listed ? View.VISIBLE : View.GONE);
        folder.setVisibility(listed ? View.VISIBLE : View.GONE);
        files.setVisibility(listed ? View.VISIBLE : View.GONE);
        showTransfers(open ? controller.rows() : Collections.emptyList());
    }

    /** The PCs a share can go to; false when there is none. */
    private boolean showPcs() {
        boolean any = false;
        for (ComputerDetails details : computers.values()) {
            if (details.state != ComputerDetails.State.ONLINE || details.pairState != PairingManager.PairState.PAIRED
                    || !details.caps.has(HostCapabilities.Feature.NATIVE_API)) {
                continue;
            }
            any = true;
            Button pick = button(pcs, 0);
            pick.setText(details.displayName());
            pick.setOnClickListener(v -> {
                uuid = details.uuid;
                computer = null;
                onComputer(details);
                render();
            });
        }
        return any;
    }

    private void showTransfers(List<FileRow> rows) {
        transfers.removeAllViews();
        boolean finished = false;
        for (FileRow row : rows) {
            transfers.addView(transferRow(row));
            finished |= row.isFinished();
        }
        transfersHeading.setVisibility(rows.isEmpty() ? View.GONE : View.VISIBLE);
        clearFinished.setVisibility(finished ? View.VISIBLE : View.GONE);
    }

    private View transferRow(FileRow row) {
        LinearLayout item = new LinearLayout(this);
        item.setOrientation(LinearLayout.VERTICAL);
        item.setPadding(0, dp(6), 0, dp(6));
        TextView title = text(item, android.R.style.TextAppearance_Material_Subhead);
        title.setText(row.upload && !row.name.equals(row.localName) ?
                getString(R.string.files_upload_name, row.localName, row.name) : row.upload ? row.name : row.localName);
        TextView detail = text(item, android.R.style.TextAppearance_Material_Body1);
        detail.setText(describe(this, row));
        if (row.state == FileRow.State.RUNNING) {
            ProgressBar bar = new ProgressBar(this, null, android.R.attr.progressBarStyleHorizontal);
            int percent = row.percent();
            bar.setIndeterminate(percent < 0);
            bar.setMax(100);
            if (percent >= 0) bar.setProgress(percent);
            item.addView(bar);
        }
        LinearLayout actions = new LinearLayout(this);
        item.addView(actions);
        if (!row.isFinished() && !row.cancelling) {
            button(actions, android.R.string.cancel).setOnClickListener(v -> controller.cancel(row.id));
        }
        if (row.canRetry) {
            button(actions, R.string.files_retry).setOnClickListener(v -> {
                if (controller.needsNewDocument(row.id)) {
                    pendingDownload = null;
                    pendingRetry = row.id;
                    creator.launch(row.localName);
                } else {
                    controller.retry(row.id);
                }
            });
        }
        return item;
    }

    /** The state line of a transfer. */
    static String describe(Context context, FileRow row) {
        switch (row.state) {
            case QUEUED:
                return context.getString(R.string.files_queued);
            case RUNNING:
                if (row.cancelling) return context.getString(R.string.files_cancelling);
                if (row.percent() < 0) {
                    return context.getString(R.string.files_receiving_unknown, Formatter.formatShortFileSize(context, row.done));
                }
                return context.getString(row.upload ? R.string.files_sending : R.string.files_receiving, row.percent(),
                        Formatter.formatShortFileSize(context, row.done), Formatter.formatShortFileSize(context, row.total));
            case DONE:
                if (!row.upload) return context.getString(R.string.files_saved);
                return row.savedName != null && !row.savedName.equals(row.name) ?
                        context.getString(R.string.files_sent_as, row.savedName) : context.getString(R.string.files_sent);
            case CANCELLED:
                return context.getString(R.string.files_cancelled);
            case FAILED:
            default:
                return context.getString(FileProblems.text(row.problem));
        }
    }

    private void showFiles(RemoteFiles list) {
        files.removeAllViews();
        if (list == null) return;
        if (list.problem != null) {
            folder.setText(FileProblems.text(list.problem));
            return;
        }
        folder.setText(getString(R.string.files_folder, list.folder, Formatter.formatShortFileSize(this, list.freeBytes)));
        if (downloadAccess != FileProblems.Access.READY) return;
        if (list.files.isEmpty()) {
            text(files, android.R.style.TextAppearance_Material_Body1).setText(R.string.files_empty);
            return;
        }
        for (RemoteFiles.Entry entry : list.files) {
            LinearLayout item = new LinearLayout(this);
            item.setOrientation(LinearLayout.VERTICAL);
            TextView name = text(item, android.R.style.TextAppearance_Material_Subhead);
            name.setText(entry.name);
            TextView detail = text(item, android.R.style.TextAppearance_Material_Body1);
            detail.setText(getString(R.string.files_entry, Formatter.formatShortFileSize(this, entry.size),
                    DateUtils.formatDateTime(this, entry.modifiedUnixMs,
                            DateUtils.FORMAT_SHOW_DATE | DateUtils.FORMAT_SHOW_TIME)));
            LinearLayout actions = new LinearLayout(this);
            item.addView(actions);
            button(actions, R.string.files_download).setOnClickListener(v -> download(entry));
            files.addView(item);
        }
    }

    @Override
    protected void onResume() {
        super.onResume();
        foreground = true;
        startPolling();
    }

    @Override
    protected void onPause() {
        foreground = false;
        stopPolling();
        super.onPause();
    }

    @Override
    protected void onDestroy() {
        generation++;
        if (controller != null) controller.close();
        stopPolling();
        if (bound) unbindService(service);
        workers.shutdown();
        super.onDestroy();
    }
}
