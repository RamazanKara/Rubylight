package com.limelight.console;

import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.view.View;
import android.widget.ImageView;
import android.widget.TextView;

import com.google.android.material.button.MaterialButton;
import com.google.android.material.dialog.MaterialAlertDialogBuilder;
import com.limelight.R;
import com.limelight.nativeapi.ConsoleRepository;
import com.limelight.nativeapi.ConsoleSnapshot;
import com.limelight.nativeapi.NativeCapabilities;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.NvApp;
import com.limelight.ui.NightjarActivity;
import com.limelight.ui.TopAppBar;
import com.limelight.utils.ServerHelper;
import com.limelight.utils.UiHelper;

import java.util.Collections;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

/**
 * One game on one host: Play, or Resume when it runs or is suspended there; Close game when this device
 * has host control on the host, otherwise the classic Quit; and the stream settings the game would start
 * with. Back returns to Home.
 */
public class GameDetailActivity extends NightjarActivity implements HostFeed.Listener {
    public static final String EXTRA_HOST_UUID = "com.limelight.console.HOST_UUID";
    public static final String EXTRA_APP_ID = "com.limelight.console.APP_ID";
    public static final String EXTRA_TITLE = "com.limelight.console.TITLE";
    public static final String EXTRA_HOST_NAME = "com.limelight.console.HOST_NAME";

    private static final long DEBOUNCE_MS = 250;
    private static final int COVER_WIDTH_DP = 132;

    private final Handler handler = new Handler(Looper.getMainLooper());
    private final ExecutorService worker = Executors.newSingleThreadExecutor(r -> {
        Thread thread = new Thread(r, "console-detail");
        thread.setDaemon(true);
        return thread;
    });
    private HostFeed feed;
    private CoverLoaders covers;
    private volatile ConsoleRepository repository;
    private String hostUuid;
    private int appId;
    private String titleExtra;
    private String hostNameExtra;
    private int generation;
    private String loadedSignature;
    private boolean shown;
    private boolean focusPending = true;
    private GameDetailModel model;

    private View loading;
    private ImageView cover;
    private TextView coverText;
    private TextView title;
    private TextView status;
    private TextView host;
    private TextView note;
    private MaterialButton play;
    private MaterialButton end;
    private TextView profileValues;
    private TextView profileLayers;

    private final Runnable reload = this::reloadNow;

    /** The data behind the screen; the debug demo overrides this with canned hosts. */
    protected ConsoleRepository createRepository(String uniqueId) {
        return ConsoleRepository.create(this, uniqueId);
    }

    /** Starts the stream; the debug demo overrides this so no stream is opened. */
    protected void play(NvApp app, ComputerDetails computer) {
        ServerHelper.doStart(this, app, computer, feed.binder());
    }

    /** Ends the game on the host, then calls {@code onComplete} on any thread. */
    protected void end(NvApp app, ComputerDetails computer, Runnable onComplete) {
        ServerHelper.doQuit(this, computer, app, feed.binder(), onComplete);
    }

    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        hostUuid = getIntent().getStringExtra(EXTRA_HOST_UUID);
        appId = getIntent().getIntExtra(EXTRA_APP_ID, 0);
        titleExtra = getIntent().getStringExtra(EXTRA_TITLE);
        hostNameExtra = getIntent().getStringExtra(EXTRA_HOST_NAME);
        if (hostUuid == null) {
            finish();
            return;
        }
        setContentView(R.layout.activity_game_detail);
        UiHelper.notifyNewRootView(this);

        TopAppBar.setUpNavigation(TopAppBar.configure(this, R.id.detail_toolbar, titleExtra == null ? "" : titleExtra, 0),
                View.NO_ID, v -> getOnBackPressedDispatcher().onBackPressed());
        loading = findViewById(R.id.detail_loading);
        cover = findViewById(R.id.detail_cover);
        coverText = findViewById(R.id.detail_cover_text);
        title = findViewById(R.id.detail_title);
        status = findViewById(R.id.detail_status);
        host = findViewById(R.id.detail_host);
        note = findViewById(R.id.detail_note);
        play = findViewById(R.id.detail_play);
        end = findViewById(R.id.detail_end);
        profileValues = findViewById(R.id.detail_profile_values);
        profileLayers = findViewById(R.id.detail_profile_layers);
        play.setOnClickListener(v -> onPlay());
        end.setOnClickListener(v -> onEnd());
        title.setText(titleExtra);
        host.setText(hostNameExtra);
        play.setEnabled(false);

        feed = new HostFeed(this, this);
        feed.bind();
    }

    @Override
    protected void onResume() {
        super.onResume();
        shown = true;
        feed.resume();
        // The game may have started or ended while the stream was in front.
        loadedSignature = null;
        scheduleReload();
    }

    @Override
    protected void onPause() {
        shown = false;
        if (feed != null) {
            feed.pause();
        }
        handler.removeCallbacks(reload);
        super.onPause();
    }

    @Override
    protected void onDestroy() {
        handler.removeCallbacksAndMessages(null);
        if (feed != null) {
            feed.unbind();
        }
        worker.shutdownNow();
        if (covers != null) {
            covers.release();
        }
        super.onDestroy();
    }

    @Override
    public void onHostsChanged() {
        scheduleReload();
    }

    private void scheduleReload() {
        handler.removeCallbacks(reload);
        handler.postDelayed(reload, DEBOUNCE_MS);
    }

    private void reloadNow() {
        if (!shown || isFinishing() || isDestroyed()) {
            return;
        }
        String uniqueId = feed.uniqueId();
        ComputerDetails current = feed.host(hostUuid);
        if (uniqueId == null || current == null) {
            return;
        }
        if (repository == null) {
            repository = createRepository(uniqueId);
            covers = new CoverLoaders(this, uniqueId, COVER_WIDTH_DP);
        }
        final ConsoleRepository repo = repository;
        final ComputerDetails computer = new ComputerDetails(current);
        final String signature = ConsoleHomeActivity.signature(Collections.singletonList(computer));
        final boolean refresh = !signature.equals(loadedSignature);
        loadedSignature = signature;
        final int mine = ++generation;
        final boolean online = computer.state == ComputerDetails.State.ONLINE;
        final boolean nativeHost = computer.caps.has(HostCapabilities.Feature.NATIVE_API);
        if (refresh) {
            loading.setVisibility(View.VISIBLE);
        }
        worker.execute(() -> {
            try {
                ConsoleSnapshot snapshot = repo.cached(Collections.singletonList(computer));
                boolean control = false;
                if (refresh) {
                    show(mine, computer, online, nativeHost, snapshot, false, false);
                    snapshot = repo.refresh(Collections.singletonList(computer));
                    NativeCapabilities capabilities = online ? repo.capabilities(computer) : null;
                    control = capabilities != null && capabilities.hostControl;
                    controlCache = control;
                } else {
                    control = controlCache;
                }
                show(mine, computer, online, nativeHost, snapshot, control, true);
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
            } catch (RuntimeException e) {
                com.limelight.LimeLog.warning("Game detail could not load: " + e);
                show(mine, computer, online, nativeHost, ConsoleSnapshot.empty(), false, true);
            }
        });
    }

    /** Host control as last read; a host that did not answer counts as without. */
    private volatile boolean controlCache;

    private void show(int mine, ComputerDetails computer, boolean online, boolean nativeHost, ConsoleSnapshot snapshot,
                      boolean control, boolean done) {
        GameDetailModel next = GameDetailModel.of(snapshot, hostUuid, appId, titleExtra, hostNameExtra, online,
                nativeHost, control);
        runOnUiThread(() -> {
            if (mine != generation || isFinishing() || isDestroyed()) {
                return;
            }
            model = next;
            render(next, computer);
            if (done) {
                loading.setVisibility(View.GONE);
            }
        });
    }

    private void render(GameDetailModel m, ComputerDetails computer) {
        title.setText(m.title);
        host.setText(m.hostName);
        ((com.google.android.material.appbar.MaterialToolbar) findViewById(R.id.detail_toolbar)).setTitle(m.title);
        covers.bind(computer, CoverLoaders.appOf(m.title, m.appId, m.source), cover, coverText);

        CharSequence chip = null;
        if (m.state == GameDetailModel.State.RUNNING) {
            chip = getString(R.string.console_detail_status_running, m.hostName);
        } else if (m.state == GameDetailModel.State.SUSPENDED) {
            chip = getString(R.string.console_detail_status_suspended, m.hostName);
        }
        status.setText(chip);
        status.setVisibility(chip == null ? View.GONE : View.VISIBLE);

        play.setText(m.resumes() ? R.string.applist_menu_resume : R.string.library_play);
        play.setEnabled(m.canStart());
        if (!m.hostOnline) {
            note.setText(R.string.library_offline);
            note.setVisibility(View.VISIBLE);
        } else if (m.otherRunningTitle != null) {
            note.setText(getString(R.string.console_detail_replaces, m.otherRunningTitle));
            note.setVisibility(View.VISIBLE);
        } else {
            note.setVisibility(View.GONE);
        }

        switch (m.endAction) {
            case CLOSE_GAME:
                end.setText(R.string.stream_close_game);
                end.setVisibility(View.VISIBLE);
                break;
            case QUIT:
                end.setText(R.string.applist_menu_quit);
                end.setVisibility(View.VISIBLE);
                break;
            default:
                end.setVisibility(View.GONE);
                break;
        }

        StreamProfileSummary profile = StreamProfileSummary.of(this, computer,
                m.source != null ? m.source.appUuid : null, m.appId);
        profileValues.setText(profile.values);
        profileLayers.setText(profile.layersOrMain(this));

        if (focusPending && play.isEnabled() && !play.isInTouchMode()) {
            focusPending = false;
            play.requestFocus();
        }
    }

    private NvApp app() {
        return CoverLoaders.appOf(model.title, model.appId, model.source);
    }

    private ComputerDetails currentHost() {
        return feed.host(hostUuid);
    }

    private void onPlay() {
        ComputerDetails computer = currentHost();
        if (model == null || computer == null || feed.binder() == null || !model.canStart()) {
            return;
        }
        NvApp app = app();
        boolean askFirst = model.otherRunningTitle != null
                && !computer.caps.has(HostCapabilities.Feature.HOST_CONFIRMS_REPLACEMENT)
                && computer.hasPermission(ComputerDetails.PERMISSION_LAUNCH);
        if (!askFirst) {
            play(app, computer);
            return;
        }
        // A host that confirms replacement asks the player itself; elsewhere the client quits the game first.
        UiHelper.showDialog(this, new MaterialAlertDialogBuilder(this)
                .setMessage(getString(R.string.library_replace_message_named, model.otherRunningTitle, model.title))
                .setPositiveButton(R.string.library_replace_confirm, (dialog, which) -> play(app, computer))
                .setNegativeButton(android.R.string.cancel, null).create());
    }

    private void onEnd() {
        ComputerDetails computer = currentHost();
        if (model == null || computer == null || feed.binder() == null
                || model.endAction == GameDetailModel.EndAction.NONE) {
            return;
        }
        NvApp app = app();
        int message = model.endAction == GameDetailModel.EndAction.CLOSE_GAME
                ? R.string.stream_close_game_confirm : R.string.applist_quit_confirmation;
        int action = model.endAction == GameDetailModel.EndAction.CLOSE_GAME
                ? R.string.stream_close_game : R.string.applist_menu_quit;
        UiHelper.showDialog(this, new MaterialAlertDialogBuilder(this)
                .setMessage(message)
                .setPositiveButton(action, (dialog, which) -> end(app, computer, () -> runOnUiThread(() -> {
                    loadedSignature = null;
                    scheduleReload();
                })))
                .setNegativeButton(android.R.string.cancel, null).create());
    }
}
