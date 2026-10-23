package com.limelight.console;

import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.view.View;
import android.widget.Toast;
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
import java.util.function.Consumer;

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
    private MaterialButton favourite;
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

    /** Closes the game on the host; {@code onDone} learns on any thread whether it is closed. */
    protected void close(NvApp app, ComputerDetails computer, Consumer<Boolean> onDone) {
        ServerHelper.doQuitReporting(this, computer, app, feed.binder(), onDone);
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
        favourite = findViewById(R.id.detail_favourite);
        profileValues = findViewById(R.id.detail_profile_values);
        profileLayers = findViewById(R.id.detail_profile_layers);
        play.setOnClickListener(v -> onPlay());
        end.setOnClickListener(v -> onEnd());
        favourite.setOnClickListener(v -> onFavourite());
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
        final int starVersion = favouriteVersion;
        final boolean online = computer.state == ComputerDetails.State.ONLINE;
        final boolean nativeHost = computer.caps.has(HostCapabilities.Feature.NATIVE_API);
        if (refresh) {
            loading.setVisibility(View.VISIBLE);
        }
        worker.execute(() -> {
            try {
                ConsoleSnapshot snapshot = repo.cached(Collections.singletonList(computer));
                // Read here, after the saved library says which id the host gives the game, and in this task:
                // a toggle made meanwhile bumps the version and the stale answer is dropped.
                Boolean starred = repo.isFavourite(hostUuid, keyIn(snapshot), appId);
                boolean control = false;
                if (refresh) {
                    show(mine, computer, online, nativeHost, snapshot, false, false, starVersion, starred);
                    snapshot = repo.refresh(Collections.singletonList(computer));
                    NativeCapabilities capabilities = online ? repo.capabilities(computer) : null;
                    control = capabilities != null && capabilities.hostControl;
                    controlCache = control;
                } else {
                    control = controlCache;
                }
                show(mine, computer, online, nativeHost, snapshot, control, true, starVersion, starred);
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
            } catch (RuntimeException e) {
                com.limelight.LimeLog.warning("Game detail could not load: " + e);
                show(mine, computer, online, nativeHost, ConsoleSnapshot.empty(), false, true, starVersion, null);
            }
        });
    }

    /** Whether the game is starred, as last read; the toggle flips it and re-renders without a reload. */
    private boolean favouriteCache;

    /** Counts toggles, so a read that began before one cannot undo it. */
    private int favouriteVersion;

    /** Host control as last read; a host that did not answer counts as without. */
    private volatile boolean controlCache;

    /** The host's stable id of this game in the library, or empty. */
    private String keyIn(ConsoleSnapshot snapshot) {
        ConsoleSnapshot.Host listed = snapshot.host(hostUuid);
        if (listed != null) {
            for (ConsoleSnapshot.Source candidate : listed.sources) {
                if (candidate.appId == appId) {
                    return candidate.stableId();
                }
            }
        }
        return "";
    }

    private void show(int mine, ComputerDetails computer, boolean online, boolean nativeHost, ConsoleSnapshot snapshot,
                      boolean control, boolean done, int starVersion, Boolean starred) {
        GameDetailModel next = GameDetailModel.of(snapshot, hostUuid, appId, titleExtra, hostNameExtra, online,
                nativeHost, control);
        runOnUiThread(() -> {
            if (mine != generation || isFinishing() || isDestroyed()) {
                return;
            }
            model = next;
            if (starred != null && starVersion == favouriteVersion) {
                favouriteCache = starred;
            }
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

        renderFavourite();

        StreamProfileSummary profile = StreamProfileSummary.of(this, computer,
                m.source != null ? m.source.appUuid : null, m.appId);
        profileValues.setText(profile.values);
        profileLayers.setText(profile.layersOrMain(this));

        if (focusPending && play.isEnabled() && !play.isInTouchMode()) {
            focusPending = false;
            play.requestFocus();
        }
    }

    private void renderFavourite() {
        favourite.setText(favouriteCache ? R.string.console_favourite_remove : R.string.console_favourite_add);
        favourite.setIconResource(favouriteCache ? R.drawable.ic_star_filled : R.drawable.ic_star);
    }

    private void onFavourite() {
        ConsoleRepository repo = repository;
        if (model == null || repo == null) {
            return;
        }
        boolean starred = !favouriteCache;
        favouriteCache = starred;
        favouriteVersion++;
        renderFavourite();
        String starTitle = model.title;
        String starHost = model.hostName;
        String starKey = model.source != null ? model.source.stableId() : "";
        worker.execute(() -> repo.setFavourite(hostUuid, starHost, starKey, appId, starTitle, starred));
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
        PlayPlan plan = PlayPlan.of(model, computer.caps.has(HostCapabilities.Feature.HOST_CONFIRMS_REPLACEMENT),
                computer.hasPermission(ComputerDetails.PERMISSION_LAUNCH),
                computer.caps.has(HostCapabilities.Feature.NATIVE_API), controlCache);
        if (plan.kind == PlayPlan.Kind.START) {
            play(app, computer);
            return;
        }
        // A host that confirms replacement asks the player itself; elsewhere the client asks, then closes the game.
        NvApp running = plan.runningApp();
        UiHelper.showDialog(this, ReplaceRunningPrompt.build(this, plan.runningTitle, model.title,
                plan.canResume ? () -> resumeRunning(running) : null,
                () -> replaceRunning(plan, running, app)));
    }

    /** What the host runs may have changed while the prompt was open: Resume only acts on what was named. */
    private void resumeRunning(NvApp running) {
        ComputerDetails computer = currentHost();
        if (computer == null || PlayPlan.recheck(running, computer.runningGameId, computer.runningGameUuid)
                != PlayPlan.Check.SAME) {
            runningChanged();
            return;
        }
        play(running, computer);
    }

    private void replaceRunning(PlayPlan plan, NvApp running, NvApp app) {
        ComputerDetails computer = currentHost();
        if (computer == null) {
            return;
        }
        PlayPlan.Check check = PlayPlan.recheck(running, computer.runningGameId, computer.runningGameUuid);
        if (check == PlayPlan.Check.CHANGED) {
            // Another game than the one named runs now: closing it would not be what the player agreed to.
            runningChanged();
            return;
        }
        if (check == PlayPlan.Check.SAME && plan.closeNatively) {
            // Like Close game: through the native API, and only a closed game is followed by the start.
            close(running, computer, closed -> {
                if (closed) {
                    runOnUiThread(() -> {
                        if (!isFinishing() && !isDestroyed()) {
                            play(app, computer);
                        }
                    });
                }
            });
            return;
        }
        // Gone already, or the classic quit: the connection quits what runs, then starts.
        play(app, computer);
    }

    private void runningChanged() {
        Toast.makeText(this, R.string.library_replace_changed, Toast.LENGTH_LONG).show();
        loadedSignature = null;
        scheduleReload();
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
