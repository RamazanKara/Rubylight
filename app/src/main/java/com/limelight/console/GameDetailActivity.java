package com.limelight.console;

import android.graphics.drawable.ColorDrawable;
import android.graphics.drawable.Drawable;
import android.net.ConnectivityManager;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.os.SystemClock;
import android.view.View;
import android.view.ViewGroup;
import android.widget.ImageView;
import android.widget.TextView;

import androidx.annotation.VisibleForTesting;
import androidx.appcompat.app.AlertDialog;
import androidx.core.content.ContextCompat;

import com.google.android.material.button.MaterialButton;
import com.limelight.R;
import com.limelight.nativeapi.ConsoleRepository;
import com.limelight.nativeapi.ConsoleSnapshot;
import com.limelight.nativeapi.NativeCapabilities;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.wol.WakeOnLanSender;
import com.limelight.pin.PinGate;
import com.limelight.ui.AppTheme;
import com.limelight.ui.BackdropView;
import com.limelight.ui.NavigationDialogBuilder;
import com.limelight.ui.NightjarActivity;
import com.limelight.ui.ShellBar;
import com.limelight.ui.TopAppBar;
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
    /** How long an automatic start waits for the page to load; a wake that follows has its own limit. */
    private static final long AUTO_LOAD_LIMIT_MS = 45_000;
    /** How often an automatic start asks again for what the PC has not said yet. */
    private static final long AUTO_RETRY_MS = 2_000;
    /** As on the desktop: how long a start waits for the network, a TV launcher's at boot being ahead of its Wi-Fi. */
    private static final long NETWORK_LIMIT_MS = 60_000;
    /** After the network came back: longer than a poll, so the countdown never names a PC not yet heard from. */
    private static final long NETWORK_SETTLE_MS = 6_000;
    private static final long NETWORK_CHECK_MS = 500;

    /** An automatic start: waiting for the PIN screen to clear, then the network, counting down, then going to the game. */
    private enum Auto { OFF, WAITING, NETWORK, COUNTING, GOING }

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
    /** What the last reload drew the PC from; a poll that brings nothing else does not reload. */
    private final HostsDrawn drawn = new HostsDrawn();
    private boolean shown;
    private boolean focusPending = true;
    private GameDetailModel model;
    /** Waking an offline PC to play: the wait, and the Play that follows once the PC answers and the page is fresh. */
    private HostWaker waker;
    private boolean playWhenReady;
    private Auto auto = Auto.OFF;
    private AlertDialog countdown;
    private long countdownEnd;
    private long networkLimit;
    /** When the wait for the network ends, now that it is back; 0 until then. */
    private long networkSettled;
    /** {@link #autoLoadLimit} is on its way. */
    private boolean autoLimitPosted;
    private String stableId;
    /** {@link #model} comes from a finished read of the PC, not from what was saved or a poll's redraw of it. */
    private boolean modelLive;
    /** The automatic start has acted on a live page. */
    private boolean autoActed;
    private final Runnable countdownTick = this::tickCountdown;
    private final Runnable networkTick = this::tickNetwork;
    private final Runnable autoLoadLimit = () -> {
        autoLimitPosted = false;
        autoLoadFailed();
    };

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
    private View quitOnLeave;
    private com.google.android.material.materialswitch.MaterialSwitch quitOnLeaveSwitch;
    private TextView quitOnLeaveNote;
    private GameDetailModel quitOnLeaveGame;
    private ComputerDetails quitOnLeaveComputer;
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
        boolean automatic = auto == Auto.GOING;
        endAuto();
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
        quitOnLeave = findViewById(R.id.detail_quit_on_leave);
        quitOnLeaveSwitch = findViewById(R.id.detail_quit_on_leave_switch);
        quitOnLeaveNote = findViewById(R.id.detail_quit_on_leave_note);
        quitOnLeave.setOnClickListener(v -> onQuitOnLeave());
        androidx.core.view.ViewCompat.setAccessibilityDelegate(quitOnLeave, new androidx.core.view.AccessibilityDelegateCompat() {
            @Override
            public void onInitializeAccessibilityNodeInfo(View host,
                    androidx.core.view.accessibility.AccessibilityNodeInfoCompat info) {
                super.onInitializeAccessibilityNodeInfo(host, info);
                info.setClassName(android.widget.Switch.class.getName());
                info.setCheckable(true);
                info.setChecked(quitOnLeaveSwitch.isChecked());
            }
        });
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
        // Only the start that opened this page: a page recreated later does nothing by itself.
        if (state == null && getIntent().getBooleanExtra(EXTRA_OPEN_PLAY, false)) auto = Auto.WAITING;
    }

    private boolean hasNetwork() {
        ConnectivityManager connectivity = (ConnectivityManager) getApplicationContext().getSystemService(CONNECTIVITY_SERVICE);
        return connectivity == null || connectivity.getActiveNetworkInfo() != null;
    }

    /** No network yet: a wait the player can still say no to, with the same ways out as the countdown. */
    private void waitForNetwork() {
        auto = Auto.NETWORK;
        networkLimit = SystemClock.elapsedRealtime() + NETWORK_LIMIT_MS;
        networkSettled = 0;
        countdown = new NavigationDialogBuilder(this)
                .setTitle(titleExtra)
                .setMessage(R.string.open_play_waiting_network)
                .setNegativeButton(android.R.string.cancel, (dialog, which) -> cancelCountdown())
                .setOnCancelListener(dialog -> cancelCountdown())
                .show();
        focusCancel();
        handler.postDelayed(networkTick, NETWORK_CHECK_MS);
    }

    /** Also after a touch, or the first press of a pad would only leave touch mode and the game would start. */
    private void focusCancel() {
        android.view.View cancel = countdown.getButton(AlertDialog.BUTTON_NEGATIVE);
        cancel.setFocusableInTouchMode(true);
        cancel.requestFocus();
    }

    private void tickNetwork() {
        if (auto != Auto.NETWORK) return;
        long now = SystemClock.elapsedRealtime();
        if (networkSettled == 0 && hasNetwork()) {
            networkSettled = now + NETWORK_SETTLE_MS;
            // What the page read without a network says nothing.
            modelLive = false;
            loadedSignature = null;
            scheduleReload();
        }
        if (networkSettled == 0 && now >= networkLimit) {
            countdown.dismiss();
            countdown = null;
            // No guard for a start that cannot happen.
            auto = Auto.GOING;
            autoFailed(WakeFeedback.said(this, hostNameExtra, getString(R.string.open_play_no_network)));
        } else if (networkSettled != 0 && now >= networkSettled) {
            countdown.dismiss();
            countdown = null;
            startCountdown();
        } else {
            handler.postDelayed(networkTick, NETWORK_CHECK_MS);
        }
    }

    /** The 3 seconds in which the player can still say no, naming the game and the PC. */
    private void startCountdown() {
        auto = Auto.COUNTING;
        countdownEnd = SystemClock.elapsedRealtime() + OpenPlayModel.timing()[0];
        countdown = new NavigationDialogBuilder(this)
                .setTitle(titleExtra)
                .setMessage(countdownText())
                .setNegativeButton(android.R.string.cancel, (dialog, which) -> cancelCountdown())
                .setOnCancelListener(dialog -> cancelCountdown())
                .show();
        focusCancel();
        handler.postDelayed(countdownTick, 100);
    }

    private String countdownText() {
        long left = Math.max(0, countdownEnd - SystemClock.elapsedRealtime());
        return getString(R.string.open_play_countdown, hostNameExtra, (int) ((left + 999) / 1000));
    }

    private void tickCountdown() {
        if (auto != Auto.COUNTING) return;
        if (SystemClock.elapsedRealtime() < countdownEnd) {
            countdown.setMessage(countdownText());
            handler.postDelayed(countdownTick, 100);
            return;
        }
        countdown.dismiss();
        countdown = null;
        auto = Auto.GOING;
        // Armed before anything can go wrong: only a good ending clears it.
        OpenPlay.arm(this);
        startAutoLimit();
        // Another device may have started in these seconds, so only a read from after them decides.
        generation++;
        modelLive = false;
        handler.removeCallbacks(reload);
        reloadNow();
    }

    /** The countdown is over without a start; the guard stays as it was. */
    private void stopCountdown() {
        handler.removeCallbacks(countdownTick);
        handler.removeCallbacks(networkTick);
        if (countdown != null) {
            countdown.dismiss();
            countdown = null;
        }
        auto = Auto.OFF;
    }

    /** Cancel, B, Back or Escape: Home, or the list of PCs where that is the first screen. */
    private void cancelCountdown() {
        stopCountdown();
        finish();
    }

    /** The page is loaded and the countdown over: wake the PC, or start the game. */
    private void actOnAuto() {
        ComputerDetails computer = currentHost();
        if (computer != null && computer.state == ComputerDetails.State.UNKNOWN) {
            // Not heard from yet is not offline: the next poll's news comes back here, within the limit.
            modelLive = false;
            return;
        }
        if (computer != null && computer.state == ComputerDetails.State.ONLINE && !model.hostOnline) {
            // It came back after this page was read, which is out of date: read it again, within the limit.
            modelLive = false;
            scheduleReload();
            return;
        }
        autoActed = true;
        if (wakeable(model)) {
            // The wake has its own limit.
            stopAutoLimit();
            wakeAndPlay();
        } else {
            startAuto(model);
        }
    }

    private void startAutoLimit() {
        if (autoLimitPosted) return;
        autoLimitPosted = true;
        handler.postDelayed(autoLoadLimit, AUTO_LOAD_LIMIT_MS);
    }

    private void stopAutoLimit() {
        autoLimitPosted = false;
        handler.removeCallbacks(autoLoadLimit);
    }

    /** The PC answers, or cannot be woken: the game starts, or the player learns why not. */
    private void startAuto(GameDetailModel m) {
        if (!m.canStart()) {
            autoFailed(WakeFeedback.said(this, m.hostName, whyNot(m)));
        } else if (m.playingElsewhere) {
            // Nothing went wrong, so the next start is not skipped for it. Never moved automatically.
            OpenPlay.disarm(this);
            autoFailed(WakeFeedback.said(this, m.hostName, getString(R.string.open_play_other_device, m.hostName)));
        } else if (m.libraryRead && (m.source == null ? !classicListCache
                : !stableId.isEmpty() && !OpenPlay.sameGame(m.source, stableId))) {
            // Its id is stale, or another game's by now; the PC would be asked for a game it no longer has.
            autoFailed(WakeFeedback.said(this, m.hostName, getString(R.string.open_play_not_listed, m.hostName)));
        } else if (m.sessionUnread && (m.source != null || classicListCache)) {
            // Free is not what a failed read says: ask again, within the limit. (With nothing to start, Play says so.)
            autoActed = false;
            modelLive = false;
            startAutoLimit();
            handler.removeCallbacks(reload);
            handler.postDelayed(reload, AUTO_RETRY_MS);
        } else {
            onPlay();
        }
    }

    /** Why Play cannot start the game on this page. */
    private String whyNot(GameDetailModel m) {
        if (m.blockedNote() != 0) return getString(m.blockedNote(), m.hostName);
        return getString(m.canWake() ? R.string.wol_no_mac : R.string.error_pc_offline);
    }

    /** The page never loaded in time: either the PC is away, or it answers without telling what it has. */
    private void autoLoadFailed() {
        ComputerDetails computer = currentHost();
        boolean online = computer != null && computer.state == ComputerDetails.State.ONLINE;
        autoFailed(WakeFeedback.said(this, hostNameExtra, !online ? getString(R.string.error_pc_offline)
                : model != null && modelSaidNoSession() ? getString(R.string.open_play_no_answer, hostNameExtra)
                : getString(R.string.open_play_no_list, hostNameExtra)));
    }

    /** The list came, and it was the session that kept the start waiting. */
    private boolean modelSaidNoSession() {
        return model.sessionUnread && (model.source != null || classicListCache);
    }

    /** The automatic part is over; what follows is the page's usual behaviour. */
    private void endAuto() {
        auto = Auto.OFF;
        stopAutoLimit();
    }

    /** An automatic start failed and says so: the player lands on Home, and the guard stays armed. */
    private void autoFailed(AlertDialog said) {
        if (auto != Auto.GOING) return;
        endAuto();
        if (said == null) {
            finish();
        } else {
            said.setOnDismissListener(dialog -> finish());
        }
    }

    /** The usual feedback, ending an automatic start: it does not offer to try again, it goes to Home. */
    private void wakeFailed(int message, Runnable retry) {
        autoFailed(WakeFeedback.failed(this, model.hostName, message, auto == Auto.GOING ? null : retry));
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
        ComputerDetails from = CoverLoaders.coverHost(shownComputer, m.source);
        backdrop.focus(HomeAmbience.gameKey(hostUuid, appId, titleExtra), covers.peek(from, app),
                covers.lacksArt(from, app), m.title, false);
    }

    @Override
    protected void onResume() {
        super.onResume();
        shown = true;
        feed.resume();
        // The game may have started or ended while the stream was in front.
        loadedSignature = null;
        scheduleReload();
        if (auto == Auto.WAITING && !PinGate.needsPinForApp(this)) {
            if (hasNetwork()) {
                startCountdown();
            } else {
                waitForNetwork();
            }
        }
    }

    @Override
    protected void onPause() {
        shown = false;
        if (auto == Auto.COUNTING || auto == Auto.NETWORK) {
            stopCountdown();
        } else if (auto == Auto.GOING) {
            // Leaving the page during the start, the wake's wait for one, is the player's choice.
            endAuto();
            OpenPlay.disarm(this);
        }
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
        // An automatic start and a wake wait on the polls themselves, so each one reloads as long as they last.
        // Otherwise every poll reports the PC, changed or not, and only news is worth reading the page again for.
        boolean waitsOnPolls = auto != Auto.OFF || playWhenReady || (waker != null && waker.isWaiting());
        if (waitsOnPolls || drawn.changed(drawnFrom(feed.host(hostUuid)), false)) {
            scheduleReload();
        }
    }

    private static List<ComputerDetails> drawnFrom(ComputerDetails host) {
        return host == null ? Collections.<ComputerDetails>emptyList() : Collections.singletonList(host);
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

    /** Whether the game is starred, as last read; the toggle flips it and re-renders without a reload. */
    private boolean favouriteCache;

    /** Counts toggles, so a read that began before one cannot undo it. */
    private int favouriteVersion;

    /** Host control as last read; a host that did not answer counts as without. */
    private volatile boolean controlCache;

    /** The PC's classic list, as last saved, has this id: a tile its native library does not list. */
    private volatile boolean classicListCache;

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
                loading.setVisibility(View.GONE);
                if (auto == Auto.GOING && !autoActed) {
                    if (live) actOnAuto();
                } else if (playWhenReady && (waker == null || !waker.isWaiting()) && (auto != Auto.GOING || live)) {
                    // The PC woke and the page is fresh from it: the Play that started the wait carries on, once.
                    playWhenReady = false;
                    if (auto == Auto.GOING) {
                        startAuto(next);
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
        title.setText(m.title);
        host.setText(m.hostName);
        setTitle(m.title);
        covers.bind(CoverLoaders.coverHost(computer, m.source), CoverLoaders.appOf(m.title, m.appId, m.source), cover, coverText);

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
        renderQuitOnLeave(m, computer);

        if (focusPending && play.isEnabled() && !play.isInTouchMode()) {
            focusPending = false;
            play.requestFocus();
        }
    }

    /** The game's own quit-on-leave switch, under the conditions of the library's long-press menu. */
    private void renderQuitOnLeave(GameDetailModel m, ComputerDetails computer) {
        boolean offered = offersQuitOnLeave(m, computer);
        quitOnLeave.setVisibility(offered ? View.VISIBLE : View.GONE);
        quitOnLeaveGame = offered ? m : null;
        quitOnLeaveComputer = offered ? computer : null;
        if (!offered) return;
        boolean enabled = com.limelight.preferences.AppQuitOnLeave.isEnabled(this, hostUuid, m.source.appUuid, m.appId);
        quitOnLeaveSwitch.setChecked(enabled);
        quitOnLeaveNote.setText(enabled ? R.string.library_quit_on_leave_on : R.string.library_quit_on_leave_off);
    }

    /**
     * Only for a game the host has added and this device may start: without the launch permission every Disconnect would
     * ask the PC to quit and fail. It also needs the game's entry in the library, whose UUID is the key the library,
     * AppQuitOnLeave and the stream check on a host that reports one; a page without it would write the id-only key.
     */
    static boolean offersQuitOnLeave(GameDetailModel m, ComputerDetails computer) {
        return computer != null && computer.hasPermission(ComputerDetails.PERMISSION_LAUNCH)
                && m.appId > 0 && m.source != null && m.source.added;
    }

    /** Flips it in place, as the library's menu does; the note and a spoken line say what leaving now does. */
    private void onQuitOnLeave() {
        GameDetailModel m = quitOnLeaveGame;
        if (m == null) return;
        boolean enabled = !quitOnLeaveSwitch.isChecked();
        com.limelight.preferences.AppQuitOnLeave.setEnabled(this, hostUuid, m.source.appUuid, m.appId, enabled);
        renderQuitOnLeave(m, quitOnLeaveComputer);
        quitOnLeave.announceForAccessibility(quitOnLeaveNote.getText());
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
            wakeFailed(R.string.error_pc_offline, null);
            return;
        }
        if (computer.state == ComputerDetails.State.ONLINE) {
            // It woke by itself since the page was drawn: refresh, then the second press plays.
            // An automatic start has no second press: it plays once the page is fresh.
            playWhenReady = auto == Auto.GOING;
            // No wake to time this: without a limit, a read that never comes back would leave it waiting for good.
            if (auto == Auto.GOING) startAutoLimit();
            modelLive = false;
            loadedSignature = null;
            scheduleReload();
            return;
        }
        if (!HostWaker.canWake(this, computer)) {
            wakeFailed(R.string.wol_no_mac, null);
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
                        wakeFailed(R.string.error_manager_not_running, null);
                        return;
                    }
                    // Reload at once: the page needs the PC's library and its running game before it plays.
                    // The wake's limit ended with the wake; a read that never comes back needs its own.
                    if (auto == Auto.GOING) startAutoLimit();
                    modelLive = false;
                    loadedSignature = null;
                    handler.removeCallbacks(reload);
                    reloadNow();
                }

                @Override
                public void onTimedOut() {
                    playWhenReady = false;
                    rerender();
                    autoFailed(WakeFeedback.timedOut(GameDetailActivity.this, model.hostName,
                            auto == Auto.GOING ? null : GameDetailActivity.this::wakeAndPlay));
                }

                @Override
                public void onSendFailed(IOException error) {
                    playWhenReady = false;
                    rerender();
                    wakeFailed(R.string.wol_fail, GameDetailActivity.this::wakeAndPlay);
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
            wakeFailed(R.string.error_pc_offline, null);
            return;
        }
        if (feed.binder() == null) {
            // The computer manager service is not connected yet: say so instead of ignoring the press.
            wakeFailed(R.string.error_manager_not_running, null);
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
                autoFailed(WakeFeedback.said(this, model.hostName, getString(R.string.open_play_no_list, model.hostName)));
                return;
            }
            play(app, computer);
            return;
        }
        if (auto == Auto.GOING) {
            // Another game runs there: the player decides now, so this is no longer an automatic start.
            endAuto();
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
