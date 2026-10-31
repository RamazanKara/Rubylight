package com.limelight.console;

import android.graphics.drawable.ColorDrawable;
import android.graphics.drawable.Drawable;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.os.SystemClock;
import android.view.View;
import android.view.ViewGroup;
import android.widget.Toast;
import android.widget.ImageView;
import android.widget.TextView;

import androidx.annotation.VisibleForTesting;
import androidx.core.content.ContextCompat;

import com.google.android.material.button.MaterialButton;
import com.google.android.material.dialog.MaterialAlertDialogBuilder;
import com.limelight.R;
import com.limelight.nativeapi.ConsoleRepository;
import com.limelight.nativeapi.ConsoleSnapshot;
import com.limelight.nativeapi.NativeCapabilities;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.wol.WakeOnLanSender;
import com.limelight.ui.AppTheme;
import com.limelight.ui.BackdropView;
import com.limelight.ui.NightjarActivity;
import com.limelight.ui.ShellBar;
import com.limelight.ui.TopAppBar;
import com.limelight.utils.LaunchSource;
import com.limelight.utils.ServerHelper;
import com.limelight.utils.UiHelper;

import java.io.IOException;
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
    /** Waking an offline PC to play: the wait, and the Play that follows once the PC answers and the page is fresh. */
    private HostWaker waker;
    private boolean playWhenReady;

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
    /** TV only (null on a phone): the backdrop, and the chip line with the installed and HDR chips. */
    private BackdropView backdrop;
    private View chips;
    private TextView installed;
    private TextView hdr;
    private TextView store;
    private ComputerDetails shownComputer;

    private final Runnable reload = this::reloadNow;

    /** The data behind the screen; the debug demo overrides this with canned hosts. */
    protected ConsoleRepository createRepository(String uniqueId) {
        return ConsoleRepository.create(this, uniqueId);
    }

    /** Starts the stream; the debug demo overrides this so no stream is opened. */
    protected void play(NvApp app, ComputerDetails computer) {
        ServerHelper.doStart(this, app, computer, feed.binder(), LaunchSource.CONSOLE_PLAY);
    }

    /** Sends the wake packets to an offline host (off the UI thread); the debug demo and tests override this. */
    protected void sendWake(ComputerDetails computer) throws IOException {
        WakeOnLanSender.sendWolPacket(this, computer);
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

        // The page heads itself with the game's title beside the cover; the bar keeps only the way back.
        if (titleExtra != null) setTitle(titleExtra);
        TopAppBar.setUpNavigation(TopAppBar.configure(this, R.id.detail_toolbar, "", 0),
                View.NO_ID, v -> getOnBackPressedDispatcher().onBackPressed());
        // A game's page belongs to Home: Home in the bar is the page it came from, so it closes this one. The
        // other two replace Home's stack the way they do from Home itself.
        ShellBar.install(this, ShellBar.Destination.HOME);
        findViewById(R.id.shell_home).setOnClickListener(v -> finish());
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
        chips = findViewById(R.id.detail_chips);
        installed = findViewById(R.id.detail_installed);
        hdr = findViewById(R.id.detail_hdr);
        store = findViewById(R.id.detail_store);
        backdrop = findViewById(R.id.detail_backdrop);
        if (backdrop != null) setUpAmbience();
        play.setOnClickListener(v -> onPlay());
        end.setOnClickListener(v -> onEnd());
        favourite.setOnClickListener(v -> onFavourite());
        title.setText(titleExtra);
        host.setText(hostNameExtra);
        play.setEnabled(false);
        // The first D-pad press after a touch (or an adb start) lands on Play, not on the back arrow at the top.
        if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.O) play.setFocusedByDefault(true);

        feed = new HostFeed(this, this);
        feed.bind();
    }

    /**
     * The TV page sits on the game's backdrop. Opened from Home, it is the picture Home was showing for the game,
     * shown at once, so the page change keeps the screen's light; otherwise it fades in when the cover is loaded.
     */
    private void setUpAmbience() {
        int canvas = AppTheme.tvCanvas(this);
        getWindow().setBackgroundDrawable(new ColorDrawable(canvas));
        ((ViewGroup) findViewById(android.R.id.content)).setClipChildren(false);
        backdrop.setCanvasColor(canvas);
        backdrop.setDim(AppTheme.isHighContrast(this) ? 0.85f : 1f);
        backdrop.setOnCovered(() -> getWindow().setBackgroundDrawable(null));
        backdrop.adopt(HomeAmbience.gameKey(hostUuid, appId, titleExtra));
    }

    /** Asks the backdrop for this game's cover; a cover still loading asks again when it arrives (see the art listener). */
    private void askBackdrop() {
        GameDetailModel m = model;
        if (backdrop == null || covers == null || m == null) return;
        NvApp app = CoverLoaders.appOf(m.title, m.appId, m.source);
        backdrop.focus(HomeAmbience.gameKey(hostUuid, appId, titleExtra), covers.peek(shownComputer, app),
                covers.lacksArt(shownComputer, app), false);
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
        // The feed stops polling with the screen, so a wait could not see the PC come up: it ends here.
        cancelWake();
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
            covers = new CoverLoaders(this, uniqueId, Math.round(
                    getResources().getDimension(R.dimen.detail_cover_width) / getResources().getDisplayMetrics().density));
            if (backdrop != null) {
                covers.setArtListener(app -> {
                    if (app.getAppId() == appId) askBackdrop();
                });
            }
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
                if (playWhenReady && (waker == null || !waker.isWaiting())) {
                    // The PC woke and the page is fresh from it: the Play that started the wait carries on, once.
                    playWhenReady = false;
                    if (next.canStart()) {
                        onPlay();
                    } else {
                        WakeFeedback.failed(this, next.hostName, R.string.error_pc_offline, null);
                    }
                }
            }
        });
    }

    @VisibleForTesting
    void render(GameDetailModel m, ComputerDetails computer) {
        title.setText(m.title);
        host.setText(m.hostName);
        setTitle(m.title);
        covers.bind(computer, CoverLoaders.appOf(m.title, m.appId, m.source), cover, coverText);

        CharSequence chip = null;
        if (m.state == GameDetailModel.State.RUNNING) {
            chip = getString(R.string.console_badge_running);
        } else if (m.state == GameDetailModel.State.SUSPENDED) {
            chip = getString(R.string.console_badge_suspended);
        }
        status.setText(chip);
        status.setVisibility(chip == null ? View.GONE : View.VISIBLE);
        if (chips == null) {
            // As on the library cards: a running game's chip leads with the green dot.
            status.setCompoundDrawablesRelativeWithIntrinsicBounds(
                    m.state == GameDetailModel.State.RUNNING ? R.drawable.status_dot_ok : 0, 0, 0, 0);
        }
        if (chips != null) {
            shownComputer = computer;
            renderChips(m, chip != null);
            askBackdrop();
        }

        renderPlay(m);
        if (!m.hostOnline) {
            note.setText(waker != null && waker.isWaiting() ? getString(R.string.console_wake_waiting, m.hostName)
                    : getString(R.string.library_offline));
            note.setVisibility(View.VISIBLE);
        } else if (m.blockedNote() != 0) {
            // Play is off: say why ("Not installed on Living-room PC").
            note.setText(getString(m.blockedNote(), m.hostName));
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

    /** The TV chip line: the state with its dot, installed or not when the host says, and HDR when the game has it. */
    private void renderChips(GameDetailModel m, boolean hasState) {
        int dot = m.state == GameDetailModel.State.RUNNING ? R.drawable.tv_dot_live : R.drawable.tv_dot_suspended;
        status.setCompoundDrawablesRelativeWithIntrinsicBounds(hasState ? dot : 0, 0, 0, 0);
        Boolean isInstalled = m.source != null ? m.source.installed : null;
        installed.setText(Boolean.FALSE.equals(isInstalled) ? R.string.console_chip_not_installed : R.string.console_detail_installed);
        installed.setVisibility(isInstalled == null ? View.GONE : View.VISIBLE);
        hdr.setVisibility(m.source != null && m.source.hdrSupported ? View.VISIBLE : View.GONE);
        if (store != null) renderStore(m.source);
        boolean any = hasState || isInstalled != null || hdr.getVisibility() == View.VISIBLE
                || (store != null && store.getVisibility() == View.VISIBLE);
        chips.setVisibility(any ? View.VISIBLE : View.GONE);
        chips.setContentDescription(chipsDescription((ViewGroup) chips));
    }

    /** The store chip (TV layout only): the store's glyph, at the chip text's size, beside its name; gone for a game no store owns. */
    private void renderStore(ConsoleSnapshot.Source source) {
        String key = source == null ? null : source.storeSource;
        String name = source == null ? null : Stores.name(key, source.storeName);
        store.setText(name);
        store.setVisibility(name == null ? View.GONE : View.VISIBLE);
        int glyph = Stores.glyph(key);
        Drawable icon = glyph == 0 ? null : ContextCompat.getDrawable(this, glyph);
        if (icon != null) {
            icon = icon.mutate();
            icon.setTint(store.getCurrentTextColor());
            int size = Math.round(store.getTextSize());
            icon.setBounds(0, 0, size, size);
        }
        store.setCompoundDrawablesRelative(icon, null, null, null);
    }

    /** What a screen reader hears for the chip line, read as one stop: the shown chips' texts, in order. */
    static CharSequence chipsDescription(ViewGroup chips) {
        StringBuilder spoken = new StringBuilder();
        for (int i = 0; i < chips.getChildCount(); i++) {
            View child = chips.getChildAt(i);
            if (child.getVisibility() != View.VISIBLE || !(child instanceof TextView)) continue;
            CharSequence text = ((TextView) child).getText();
            if (text == null || text.length() == 0) continue;
            if (spoken.length() > 0) spoken.append(", ");
            spoken.append(text);
        }
        return spoken.length() == 0 ? null : spoken.toString();
    }

    /** Play, Resume, or for a PC that is away Wake PC and play (Waking PC while the wait lasts); never greyed out then. */
    private void renderPlay(GameDetailModel m) {
        boolean waiting = waker != null && waker.isWaiting();
        if (wakeable(m)) {
            play.setText(waiting ? R.string.wol_waking_pc : R.string.console_wake_and_play);
        } else {
            play.setText(m.resumes() ? R.string.applist_menu_resume : R.string.library_play);
        }
        play.setEnabled(m.canStart() || wakeable(m));
    }

    /** The PC is away and a wake packet can be sent to it (it has a MAC address); otherwise Play stays greyed out. */
    private boolean wakeable(GameDetailModel m) {
        return m.canWake() && HostWaker.canWake(this, currentHost());
    }

    /** Draws the page again from what it has, so notes that depended on the wait (waiting for the PC) clear. */
    private void rerender() {
        ComputerDetails computer = currentHost();
        if (model != null && computer != null) {
            render(model, computer);
        } else if (model != null) {
            renderPlay(model);
        }
    }

    private void cancelWake() {
        playWhenReady = false;
        if (waker != null) {
            waker.cancel();
        }
    }

    /** Play on a PC that is away: send the wake packets, wait for it, and let {@link #show} carry on with the game. */
    private void wakeAndPlay() {
        ComputerDetails computer = currentHost();
        if (waker != null && waker.isWaiting()) {
            return;
        }
        if (computer == null) {
            WakeFeedback.failed(this, model.hostName, R.string.error_pc_offline, null);
            return;
        }
        if (computer.state == ComputerDetails.State.ONLINE) {
            // It woke by itself since the page was drawn: refresh, then the second press plays.
            loadedSignature = null;
            scheduleReload();
            return;
        }
        if (!HostWaker.canWake(this, computer)) {
            WakeFeedback.failed(this, model.hostName, R.string.wol_no_mac, null);
            return;
        }
        if (waker == null) {
            waker = new HostWaker(handler, r -> new Thread(r, "console-wake").start(), this::sendWake,
                    () -> feed.host(hostUuid), SystemClock::elapsedRealtime, new HostWaker.Listener() {
                @Override
                public void onOnline() {
                    if (feed.uniqueId() == null) {
                        // The service is gone, so the page cannot reload or start anything: say so, start nothing.
                        playWhenReady = false;
                        rerender();
                        WakeFeedback.failed(GameDetailActivity.this, model.hostName, R.string.error_manager_not_running,
                                null);
                        return;
                    }
                    // Reload at once: the page needs the PC's library and its running game before it plays.
                    loadedSignature = null;
                    handler.removeCallbacks(reload);
                    reloadNow();
                }

                @Override
                public void onTimedOut() {
                    playWhenReady = false;
                    rerender();
                    WakeFeedback.timedOut(GameDetailActivity.this, model.hostName, GameDetailActivity.this::wakeAndPlay);
                }

                @Override
                public void onSendFailed(IOException error) {
                    playWhenReady = false;
                    rerender();
                    WakeFeedback.failed(GameDetailActivity.this, model.hostName, R.string.wol_fail,
                            GameDetailActivity.this::wakeAndPlay);
                }
            });
        }
        playWhenReady = true;
        waker.start();
        render(model, computer);
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
        if (model != null && wakeable(model)) {
            wakeAndPlay();
            return;
        }
        if (model == null || !model.canStart()) {
            return;
        }
        if (computer == null) {
            WakeFeedback.failed(this, model.hostName, R.string.error_pc_offline, null);
            return;
        }
        if (feed.binder() == null) {
            // The computer manager service is not connected yet: say so instead of ignoring the press.
            WakeFeedback.failed(this, model.hostName, R.string.error_manager_not_running, null);
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
