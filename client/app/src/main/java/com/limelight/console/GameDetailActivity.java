package com.limelight.console;

import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.os.SystemClock;
import android.view.View;
import android.view.ViewGroup;

import androidx.annotation.VisibleForTesting;

import com.limelight.R;
import com.limelight.nativeapi.ConsoleRepository;
import com.limelight.nativeapi.ConsoleSnapshot;
import com.limelight.nativeapi.NativeCapabilities;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.wol.WakeOnLanSender;
import com.limelight.ui.NightjarActivity;
import com.limelight.utils.LaunchSource;
import com.limelight.utils.ServerHelper;
import com.limelight.utils.UiHelper;

import java.io.IOException;
import java.util.Collections;
import java.util.List;
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
    /** "When Nightjar opens": the page counts down and starts the game by itself (see {@link OpenPlay}). */
    static final String EXTRA_OPEN_PLAY = "com.limelight.console.OPEN_PLAY";
    /** With {@link #EXTRA_OPEN_PLAY}: what the host calls the saved game, whatever its app id is by now. */
    static final String EXTRA_STABLE_ID = "com.limelight.console.STABLE_ID";

    private static final long DEBOUNCE_MS = 250;

    /** An automatic start: waiting for the PIN screen to clear, then the network, counting down, then going to the game. */
    enum Auto { OFF, WAITING, NETWORK, COUNTING, GOING }

    private final Handler handler = new Handler(Looper.getMainLooper());
    private final ExecutorService worker = Executors.newSingleThreadExecutor(r -> {
        Thread thread = new Thread(r, "console-detail");
        thread.setDaemon(true);
        return thread;
    });
    private final GameDetailView views = new GameDetailView(this);
    private final GameDetailOpenPlay openPlay = new GameDetailOpenPlay(this, handler);

    private HostFeed feed;
    CoverLoaders covers;
    private volatile ConsoleRepository repository;
    String hostUuid;
    int appId;
    String titleExtra;
    String hostNameExtra;
    private int generation;
    String loadedSignature;
    /** What the last reload drew the PC from; a poll that brings nothing else does not reload. */
    private final HostsDrawn drawn = new HostsDrawn();
    private boolean shown;
    boolean focusPending = true;
    GameDetailModel model;
    /** Waking an offline PC to play: the wait, and the Play that follows once the PC answers and the page is fresh. */
    private HostWaker waker;
    private boolean playWhenReady;
    Auto auto = Auto.OFF;
    /** The way the device was held when the page was laid out: upright it is the sheet, sideways the hero grown to a page. */
    private int laidOutFor;
    /** The way it is held now, as the system last said: this screen's resources keep the configuration they were made with. */
    private int heldNow;
    long countdownEnd;
    String stableId;
    /** {@link #model} comes from a finished read of the PC, not from what was saved or a poll's redraw of it. */
    boolean modelLive;

    /** Whether the game is starred, as last read; the toggle flips it and re-renders without a reload. */
    boolean favouriteCache;
    /** Counts toggles, so a read that began before one cannot undo it. */
    private int favouriteVersion;
    /** Host control as last read; a host that did not answer counts as without. */
    private volatile boolean controlCache;
    /** The PC's classic list, as last saved, has this id: a tile its native library does not list. */
    volatile boolean classicListCache;

    private final Runnable reload = this::reloadNow;

    /** The data behind the screen; the debug demo overrides this with canned hosts. */
    protected ConsoleRepository createRepository(String uniqueId) {
        return ConsoleRepository.create(this, uniqueId);
    }

    /** Starts the stream; the debug demo overrides this so no stream is opened. */
    protected void play(NvApp app, ComputerDetails computer) {
        boolean automatic = auto == Auto.GOING;
        openPlay.endAuto();
        ServerHelper.doStart(this, app, computer, feed.binder(), automatic ? LaunchSource.OPEN_PLAY : LaunchSource.CONSOLE_PLAY);
        // The stream's end lands on Home, not on this page.
        if (automatic) finish();
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

    /** Its chrome follows the form: a rotation into another form lays it out again. */
    @Override
    protected boolean followsForm() {
        return true;
    }

    /**
     * An automatic start (its wait for the network, its countdown), a press of Play that waits for the PC and a wake run
     * on this page, and laying it out again would end them: a pause cancels each, and none is saved. The turn waits.
     */
    @Override
    protected boolean holdsForm() {
        return auto != Auto.OFF || playWhenReady || isWakeWaiting();
    }

    boolean isWakeWaiting() {
        return waker != null && waker.isWaiting();
    }

    /** That work is over, or may be: the page asks again whether it was turned meanwhile. */
    void followTurnSoon() {
        followFormSoon();
        handler.post(this::followTurn);
    }

    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        hostUuid = getIntent().getStringExtra(EXTRA_HOST_UUID);
        appId = getIntent().getIntExtra(EXTRA_APP_ID, 0);
        titleExtra = getIntent().getStringExtra(EXTRA_TITLE);
        hostNameExtra = getIntent().getStringExtra(EXTRA_HOST_NAME);
        stableId = getIntent().getStringExtra(EXTRA_STABLE_ID);
        if (stableId == null) stableId = "";
        if (hostUuid == null) {
            finish();
            return;
        }
        laidOutFor = getResources().getConfiguration().orientation;
        heldNow = laidOutFor;
        setContentView(R.layout.activity_game_detail);
        UiHelper.notifyNewRootView(this);
        views.bind(hostUuid, appId, titleExtra, hostNameExtra, this::onPlay, this::onEnd, this::onFavourite);

        feed = new HostFeed(this, this);
        feed.bind();
        // Only the start that opened this page: a page recreated later does nothing by itself.
        if (state == null && getIntent().getBooleanExtra(EXTRA_OPEN_PLAY, false)) auto = Auto.WAITING;
    }

    void stopAutoLimit() {
        openPlay.stopAutoLimit();
    }

    /** Why Play cannot start the game on this page. */
    String whyNot(GameDetailModel m) {
        if (m.blockedNote() != 0) return getString(m.blockedNote(), m.hostName);
        return getString(m.canWake() ? R.string.wol_no_mac : R.string.error_pc_offline);
    }

    @Override
    protected void onResume() {
        super.onResume();
        shown = true;
        followTurn();
        feed.resume();
        // The game may have started or ended while the stream was in front.
        loadedSignature = null;
        scheduleReload();
        openPlay.onResume();
    }

    @Override
    protected void onPause() {
        shown = false;
        openPlay.onPause();
        // The feed stops polling with the screen, so a wait could not see the PC come up: it ends here.
        cancelWake();
        if (feed != null) {
            feed.pause();
        }
        handler.removeCallbacks(reload);
        super.onPause();
    }

    @Override
    public void onConfigurationChanged(android.content.res.Configuration newConfig) {
        super.onConfigurationChanged(newConfig);
        heldNow = newConfig.orientation;
        followTurn();
    }

    /**
     * The page has a layout for each way of holding the device and handles the turn itself, so it is laid out again for
     * the new one. A turn that changes the form did that already; this is the turn that does not, a window under 600 dp
     * wide on its side, which is a phone's form either way. A start or a wake under way keeps the page as it is
     * ({@link #holdsForm}); it asks again when that ends, with each drawing of the page and on a resume.
     */
    private void followTurn() {
        int now = heldNow;
        if (now == android.content.res.Configuration.ORIENTATION_UNDEFINED || now == laidOutFor || isFinishing() || isDestroyed() || recreatingForForm() || holdsForm()) return;
        laidOutFor = now;
        recreate();
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
        // An automatic start and a wake wait on the polls themselves, so each one reloads as long as they last.
        // Otherwise every poll reports the PC, changed or not, and only news is worth reading the page again for.
        boolean waitsOnPolls = auto != Auto.OFF || playWhenReady || isWakeWaiting();
        if (waitsOnPolls || drawn.changed(drawnFrom(feed.host(hostUuid)), false)) {
            scheduleReload();
        }
    }

    private static List<ComputerDetails> drawnFrom(ComputerDetails host) {
        return host == null ? Collections.<ComputerDetails>emptyList() : Collections.singletonList(host);
    }

    void scheduleReload() {
        scheduleReloadDelayed(DEBOUNCE_MS);
    }

    void scheduleReloadDelayed(long delayMs) {
        handler.removeCallbacks(reload);
        handler.postDelayed(reload, delayMs);
    }

    void forceLiveReload() {
        generation++;
        modelLive = false;
        handler.removeCallbacks(reload);
        reloadNow();
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
            // As wide as the cover is laid out: the sideways page's is larger than the portrait page's.
            View cover = findViewById(R.id.detail_poster);
            float coverWidth = cover != null && cover.getLayoutParams().width > 0 ? cover.getLayoutParams().width
                    : getResources().getDimension(R.dimen.detail_cover_width);
            covers = new CoverLoaders(this, uniqueId, Math.round(coverWidth / getResources().getDisplayMetrics().density));
            if (views.hasBackdrop()) {
                covers.setArtListener(app -> {
                    if (app.getAppId() == appId) views.askBackdrop();
                });
            }
        }
        final ConsoleRepository repo = repository;
        final ComputerDetails computer = new ComputerDetails(current);
        drawn.set(drawnFrom(current), false);
        final String signature = ConsoleHomeActivity.signature(Collections.singletonList(computer));
        // An automatic start acts only on a live page: a redraw of what the page has will not do.
        final boolean refresh = !signature.equals(loadedSignature) || (auto == Auto.GOING && !modelLive);
        loadedSignature = signature;
        final int mine = ++generation;
        final int starVersion = favouriteVersion;
        final boolean online = computer.state == ComputerDetails.State.ONLINE;
        final boolean nativeHost = computer.caps.has(HostCapabilities.Feature.NATIVE_API);
        if (refresh) {
            views.setLoading(true);
        }
        worker.execute(() -> {
            try {
                ConsoleSnapshot snapshot = repo.cached(Collections.singletonList(computer));
                // Read here, after the saved library says which id the host gives the game, and in this task:
                // a toggle made meanwhile bumps the version and the stale answer is dropped.
                Boolean starred = repo.isFavourite(hostUuid, keyIn(snapshot), appId);
                boolean control = false;
                if (refresh) {
                    show(mine, computer, online, nativeHost, snapshot, false, false, false, starVersion, starred);
                    snapshot = repo.refresh(Collections.singletonList(computer));
                    NativeCapabilities capabilities = online ? repo.capabilities(computer) : null;
                    control = capabilities != null && capabilities.hostControl;
                    controlCache = control;
                    classicListCache = repo.classicListHas(computer, OpenPlay.listedAs(snapshot.host(hostUuid), stableId, appId), stableId);
                } else {
                    control = controlCache;
                }
                show(mine, computer, online, nativeHost, snapshot, control, true, refresh, starVersion, starred);
                if (refresh) {
                    redrawAtNextPoll(mine);
                }
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
            } catch (RuntimeException e) {
                com.limelight.LimeLog.warning("Game detail could not load: " + e);
                show(mine, computer, online, nativeHost, ConsoleSnapshot.empty(), false, true, false, starVersion, null);
                redrawAtNextPoll(mine);
            }
        });
    }

    /** A read of the PC changed what is saved, and a failed load drew nothing: the next poll draws again, once. */
    private void redrawAtNextPoll(int mine) {
        runOnUiThread(() -> {
            if (mine == generation) {
                drawn.forget();
            }
        });
    }

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
                      boolean control, boolean done, boolean live, int starVersion, Boolean starred) {
        GameDetailModel next = GameDetailModel.of(snapshot, hostUuid, OpenPlay.listedAs(snapshot.host(hostUuid), stableId, appId), titleExtra, hostNameExtra, online,
                nativeHost, control);
        runOnUiThread(() -> {
            if (mine != generation || isFinishing() || isDestroyed()) {
                return;
            }
            model = next;
            modelLive = live;
            if (starred != null && starVersion == favouriteVersion) {
                favouriteCache = starred;
            }
            render(next, computer);
            if (done) {
                views.setLoading(false);
                if (auto == Auto.GOING && !openPlay.autoActed) {
                    if (live) openPlay.actOnAuto();
                } else if (playWhenReady && !isWakeWaiting() && (auto != Auto.GOING || live)) {
                    // The PC woke and the page is fresh from it: the Play that started the wait carries on, once.
                    playWhenReady = false;
                    if (auto == Auto.GOING) {
                        openPlay.startAuto(next);
                    } else if (next.canStart()) {
                        onPlay();
                    } else {
                        WakeFeedback.said(this, next.hostName, whyNot(next));
                    }
                }
            }
        });
    }

    @VisibleForTesting
    void render(GameDetailModel m, ComputerDetails computer) {
        // A wake or a start that ended is drawn here: a turn it kept waiting is followed now.
        if (heldNow != laidOutFor) followTurnSoon();
        views.render(m, computer);
    }

    static boolean offersQuitOnLeave(GameDetailModel m, ComputerDetails computer) {
        return GameDetailView.offersQuitOnLeave(m, computer);
    }

    static CharSequence chipsDescription(ViewGroup chips) {
        return GameDetailView.chipsDescription(chips);
    }

    /** The PC is away and a wake packet can be sent to it (it has a MAC address); otherwise Play stays greyed out. */
    boolean wakeable(GameDetailModel m) {
        return m.canWake() && HostWaker.canWake(this, currentHost());
    }

    /** Draws the page again from what it has, so notes that depended on the wait (waiting for the PC) clear. */
    void rerender() {
        ComputerDetails computer = currentHost();
        if (model != null && computer != null) {
            render(model, computer);
        } else if (model != null) {
            views.renderPlay(model);
        }
    }

    private void cancelWake() {
        playWhenReady = false;
        if (waker != null) {
            waker.cancel();
        }
        followTurnSoon();
    }

    /** Play on a PC that is away: send the wake packets, wait for it, and let {@link #show} carry on with the game. */
    void wakeAndPlay() {
        ComputerDetails computer = currentHost();
        if (isWakeWaiting()) {
            return;
        }
        if (computer == null) {
            openPlay.wakeFailed(R.string.error_pc_offline, null);
            return;
        }
        if (computer.state == ComputerDetails.State.ONLINE) {
            // It woke by itself since the page was drawn: refresh, then the second press plays.
            // An automatic start has no second press: it plays once the page is fresh.
            playWhenReady = auto == Auto.GOING;
            // No wake to time this: without a limit, a read that never comes back would leave it waiting for good.
            if (auto == Auto.GOING) openPlay.startAutoLimit();
            modelLive = false;
            loadedSignature = null;
            scheduleReload();
            return;
        }
        if (!HostWaker.canWake(this, computer)) {
            openPlay.wakeFailed(R.string.wol_no_mac, null);
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
                        openPlay.wakeFailed(R.string.error_manager_not_running, null);
                        return;
                    }
                    // Reload at once: the page needs the PC's library and its running game before it plays.
                    // The wake's limit ended with the wake; a read that never comes back needs its own.
                    if (auto == Auto.GOING) openPlay.startAutoLimit();
                    modelLive = false;
                    loadedSignature = null;
                    handler.removeCallbacks(reload);
                    reloadNow();
                }

                @Override
                public void onTimedOut() {
                    playWhenReady = false;
                    rerender();
                    openPlay.autoFailed(WakeFeedback.timedOut(GameDetailActivity.this, model.hostName,
                            auto == Auto.GOING ? null : GameDetailActivity.this::wakeAndPlay));
                }

                @Override
                public void onSendFailed(IOException error) {
                    playWhenReady = false;
                    rerender();
                    openPlay.wakeFailed(R.string.wol_fail, GameDetailActivity.this::wakeAndPlay);
                }
            });
        }
        playWhenReady = true;
        waker.start();
        render(model, computer);
    }

    private void onFavourite() {
        ConsoleRepository repo = repository;
        if (model == null || repo == null) {
            return;
        }
        boolean starred = !favouriteCache;
        favouriteCache = starred;
        favouriteVersion++;
        views.renderFavourite(favouriteCache);
        String starTitle = model.title;
        String starHost = model.hostName;
        String starKey = model.source != null ? model.source.stableId() : "";
        worker.execute(() -> repo.setFavourite(hostUuid, starHost, starKey, appId, starTitle, starred));
    }

    private NvApp app() {
        return CoverLoaders.appOf(model.title, model.appId, model.source);
    }

    ComputerDetails currentHost() {
        return feed.host(hostUuid);
    }

    void onPlay() {
        ComputerDetails computer = currentHost();
        if (model != null && wakeable(model)) {
            wakeAndPlay();
            return;
        }
        if (model == null || !model.canStart()) {
            return;
        }
        if (computer == null) {
            openPlay.wakeFailed(R.string.error_pc_offline, null);
            return;
        }
        if (feed.binder() == null) {
            // The computer manager service is not connected yet: say so instead of ignoring the press.
            openPlay.wakeFailed(R.string.error_manager_not_running, null);
            return;
        }
        NvApp app = app();
        boolean confirms = computer.caps.has(HostCapabilities.Feature.HOST_CONFIRMS_REPLACEMENT);
        boolean launch = computer.hasPermission(ComputerDetails.PERMISSION_LAUNCH);
        boolean nativeHost = computer.caps.has(HostCapabilities.Feature.NATIVE_API);
        PlayPlan plan = auto == Auto.GOING
                ? PlayPlan.automatic(model, confirms, launch, nativeHost, controlCache,
                        computer.hasRunningGame() && !app.matchesRunningApp(computer.runningGameId, computer.runningGameUuid),
                        computer.runningGameId, computer.runningGameUuid)
                : PlayPlan.of(model, confirms, launch, nativeHost, controlCache);
        if (plan.kind == PlayPlan.Kind.START) {
            if (auto == Auto.GOING && model.source == null && !classicListCache) {
                // Nothing says this id is still good: the PC's list could not be read, and none is saved.
                openPlay.autoFailed(WakeFeedback.said(this, model.hostName, getString(R.string.open_play_no_list, model.hostName)));
                return;
            }
            play(app, computer);
            return;
        }
        if (auto == Auto.GOING) {
            // Another game runs there: the player decides now, so this is no longer an automatic start.
            openPlay.endAuto();
            OpenPlay.disarm(this);
        }
        // A host that confirms replacement asks the player itself; elsewhere the client asks, then closes the game.
        UiHelper.showDialog(this, ReplaceRunningPrompt.ask(this, plan, model.title, app, new ReplaceRunningPrompt.Screen() {
            @Override public ComputerDetails currentHost() {
                return GameDetailActivity.this.currentHost();
            }

            @Override public void start(NvApp starting, ComputerDetails on) {
                play(starting, on);
            }

            @Override public void resume(NvApp running, ComputerDetails on) {
                play(running, on);
            }

            @Override public void close(NvApp running, ComputerDetails on, Consumer<Boolean> onDone) {
                GameDetailActivity.this.close(running, on, onDone);
            }

            @Override public void reload() {
                loadedSignature = null;
                scheduleReload();
            }
        }));
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
        UiHelper.showDialog(this, new com.limelight.ui.NavigationDialogBuilder(this).startOnCancel()
                .setMessage(message)
                .setPositiveButton(action, (dialog, which) -> end(app, computer, () -> runOnUiThread(() -> {
                    loadedSignature = null;
                    scheduleReload();
                })))
                .setNegativeButton(android.R.string.cancel, null).create());
    }
}
