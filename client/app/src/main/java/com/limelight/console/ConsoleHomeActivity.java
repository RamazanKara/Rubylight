package com.limelight.console;

import android.app.Activity;
import android.content.Intent;
import android.content.res.Configuration;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.preference.PreferenceManager;
import android.view.KeyEvent;
import android.view.MotionEvent;
import android.view.View;
import android.widget.Button;
import android.widget.TextView;

import androidx.recyclerview.widget.GridLayoutManager;
import androidx.recyclerview.widget.RecyclerView;

import com.limelight.LimeLog;
import com.limelight.R;
import com.limelight.binding.video.DecoderBenchmark;
import com.limelight.computers.ComputerManagerService;
import com.limelight.computers.HostActions;
import com.limelight.computers.HostMenu;
import com.limelight.computers.PairingWatch;
import com.limelight.files.RunningTransfers;
import com.limelight.nativeapi.ConsoleRepository;
import com.limelight.nativeapi.ConsoleSnapshot;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.wol.WakeOnLanSender;
import com.limelight.ui.GlassForm;
import com.limelight.ui.InputSource;
import com.limelight.ui.NightjarActivity;
import com.limelight.ui.ShellBar;
import com.limelight.ui.ShellFocus;
import com.limelight.ui.TopAppBar;
import com.limelight.update.UpdateFlow;
import com.limelight.utils.ServerHelper;
import com.limelight.utils.UiHelper;

import java.io.IOException;
import java.lang.ref.WeakReference;
import java.util.Collections;
import java.util.HashSet;
import java.util.List;
import java.util.Set;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.function.Consumer;

/**
 * Home: the games to continue, the library across the paired hosts and the hosts. It paints what is saved
 * first and refreshes from the hosts that answer; a host without the native API shows its classic app list
 * and opens the classic library when tapped. The back button leaves the app.
 */
public class ConsoleHomeActivity extends NightjarActivity implements HostFeed.Listener {
    /** How long after coming to the front "no hosts" waits for the first host to answer. */
    private static final long SETTLE_MS = 1500;
    private static final long DEBOUNCE_MS = 250;
    /** Follow-up refresh after returning from another screen so host session teardown is reflected on Home. */
    private static final long RESUME_REFRESH_MS = 1500;
    /** How long Home waits for saved PCs to answer before it shows them offline: a poll that gets no answer is done by then. */
    private static final long WAIT_LIMIT_MS = 10_000;
    private static final int COVER_WIDTH_DP = 140;

    private final Handler handler = new Handler(Looper.getMainLooper());
    private final ExecutorService worker = Executors.newSingleThreadExecutor(r -> {
        Thread thread = new Thread(r, "console-home");
        thread.setDaemon(true);
        return thread;
    });
    /** Stars have their own thread, so a toggle does not wait behind a refresh from the hosts. */
    private final ExecutorService starWorker = Executors.newSingleThreadExecutor(r -> {
        Thread thread = new Thread(r, "console-home-star");
        thread.setDaemon(true);
        return thread;
    });
    private HostFeed feed;
    private PairingWatch pairingWatch;
    private Button pairingBanner;
    /** What the automatic settings will use, after the first pairing ({@link SetupCard}); gone for nearly everyone. */
    private View setupCard;
    /** Whether the card's PC was among the hosts when the card was last worked out: a poll asks again only when that changed. */
    private boolean setupCardHostThere;
    /** The game Up left for the card's close button; Down from the button returns to it. */
    private WeakReference<View> setupCardCameFrom;
    private List<PairingWatch.Waiting> waitingToPair = Collections.emptyList();
    /** The phone's Home: one grid of headers and cards. Null on TV. */
    private ConsoleHomeListAdapter adapter;
    private GridLayoutManager grid;
    /** The TV's Home: shelves under a hero. Null on a phone. */
    private ConsoleHomeAdapter shelves;
    private CoverLoaders covers;
    private RecyclerView list;
    private HomeAmbience ambience;
    private HomeChrome chrome;
    private HomeHints hints;
    private HostCapsule capsule;
    private HomeSearch search;
    private ShelfAdapter.Env searchEnv;
    private View loading;
    private View empty;
    private volatile ConsoleRepository repository;
    private int generation;
    private String refreshedSignature;
    /** What the last reload drew the PCs from; a poll that brings nothing else does not reload. */
    private final HostsDrawn drawn = new HostsDrawn();
    private boolean settled;
    private boolean shown;
    private boolean pausedOnce;
    private boolean focusPending = true;
    /**
     * Until the first key or touch Home is at its start, whatever arrives. The hosts' answer comes right after what was saved and
     * can change the shelves: the start follows it, from the top of the list.
     */
    private boolean atStart = true;

    private final HostActions hostActions = new HostActions(this, new HostActions.Callbacks() {
        @Override public ComputerManagerService.ComputerManagerBinder binder() { return feed == null ? null : feed.binder(); }
        @Override public boolean isForeground() { return shown; }
        @Override public void onPaired(ComputerDetails computer) { }
        @Override public void onRemoved(ComputerDetails computer) {
            feed.forget(computer.uuid);
            refreshedSignature = null;
            scheduleReload();
        }
        @Override public void onRenamed(ComputerDetails computer) {
            refreshedSignature = null;
            scheduleReload();
        }
        @Override public void onWakeSent(ComputerDetails computer) { }
        // A form change waited for the wake; asked once the wake's end is done, as on the PCs page.
        @Override public void onWakeEnded(ComputerDetails computer) { followFormSoon(); }
        @Override public void pausePolling() { feed.pause(); }
        @Override public void resumePolling() { if (shown) feed.resume(); }
        @Override public void pairingChanged() {
            pairingWatch.refreshSoon();
            if (shown) pairingWatch.poll(feed.hosts());
        }
    });
    private final HomeWake wake = new HomeWake(this, handler, uuid -> feed == null ? null : feed.host(uuid),
            this::sendWake, () -> {
        refreshedSignature = null;
        scheduleReload();
        // A form change waited for the wake.
        followForm();
    }, this::followFormSoon);
    /** Grid or rows, by store or not, the card size: the stored choice, else this device's look. */
    private HomeView view;
    private HomeModel model = HomeModel.empty();
    /** What {@link #model} was made from: the hero's Play reads it as the game's page reads its own. */
    private ConsoleSnapshot shownSnapshot = ConsoleSnapshot.empty();
    /** The games this screen is adding on a host now; see {@link GameOpener.Screen#adding}. */
    private final Set<String> adding = new HashSet<>();
    private final GameOpener.Screen opener = new GameOpener.Screen() {
        @Override public Activity activity() { return ConsoleHomeActivity.this; }
        @Override public ConsoleRepository repository() { return repository; }
        @Override public ComputerDetails host(String uuid) { return feed.host(uuid); }
        @Override public Class<? extends GameDetailActivity> detailActivity() { return ConsoleHomeActivity.this.detailActivity(); }
        @Override public Set<String> adding() { return adding; }
        @Override public void onAdded() {
            refreshedSignature = null;
            scheduleReload();
            // A form change waited for the add.
            followFormSoon();
        }
    };
    private final HomeViewSheet viewSheet = new HomeViewSheet(this);
    private final HomeFocusManager focus = new HomeFocusManager(new HomeFocusManager.Host() {
        @Override public Activity activity() { return ConsoleHomeActivity.this; }
        @Override public RecyclerView list() { return list; }
        @Override public GridLayoutManager grid() { return grid; }
        @Override public void setGrid(GridLayoutManager g) { grid = g; }
        @Override public ConsoleHomeListAdapter adapter() { return adapter; }
        @Override public void setAdapter(ConsoleHomeListAdapter a) { adapter = a; }
        @Override public ConsoleHomeAdapter shelves() { return shelves; }
        @Override public void setShelves(ConsoleHomeAdapter s) { shelves = s; }
        @Override public HomeAmbience ambience() { return ambience; }
        @Override public void setAmbience(HomeAmbience a) { ambience = a; }
        @Override public HomeHints hints() { return hints; }
        @Override public void setHints(HomeHints h) { hints = h; }
        @Override public HomeChrome chrome() { return chrome; }
        @Override public HomeView view() { return view; }
        @Override public HostFeed feed() { return feed; }
        @Override public CoverLoaders covers() { return covers; }
        @Override public boolean isTelevision() { return ConsoleHomeActivity.this.isTelevision(); }
        @Override public boolean isFocusPending() { return focusPending; }
        @Override public void setFocusPending(boolean pending) { focusPending = pending; }
        @Override public boolean isAtStart() { return atStart; }
        @Override public void onInput(InputSource source, int vendor) { ConsoleHomeActivity.this.onInput(source, vendor); }
    });
    private final HomeGameActions gameActions = new HomeGameActions(new HomeGameActions.Host() {
        @Override public Activity activity() { return ConsoleHomeActivity.this; }
        @Override public HostFeed feed() { return feed; }
        @Override public HostActions hostActions() { return hostActions; }
        @Override public ConsoleRepository repository() { return repository; }
        @Override public ConsoleSnapshot shownSnapshot() { return shownSnapshot; }
        @Override public HomeModel model() { return model; }
        @Override public HomeView view() { return view; }
        @Override public GameOpener.Screen opener() { return opener; }
        @Override public ExecutorService worker() { return worker; }
        @Override public ExecutorService starWorker() { return starWorker; }
        @Override public Handler handler() { return handler; }
        @Override public boolean isShown() { return shown; }
        @Override public void play(NvApp app, ComputerDetails pc, String source) { ConsoleHomeActivity.this.play(app, pc, source); }
        @Override public void end(NvApp app, ComputerDetails pc, Runnable onComplete) { ConsoleHomeActivity.this.end(app, pc, onComplete); }
        @Override public void close(NvApp app, ComputerDetails pc, Consumer<Boolean> onDone) { ConsoleHomeActivity.this.close(app, pc, onDone); }
        @Override public void openGameOn(String uuid, int appId, String title, String hostName) { ConsoleHomeActivity.this.openGameOn(uuid, appId, title, hostName); }
        @Override public void invalidateAndReload() {
            refreshedSignature = null;
            scheduleReload();
        }
        @Override public void applyModel(HomeModel next) {
            model = next;
            focus.submitKeepingFocus(next);
        }
    });

    // Takes the button of transfers a closed Files page left running away again (RunningTransfers.showOn).
    private Runnable transfersButton;
    private final Runnable reload = this::reloadNow;
    private final Runnable followUpRefresh = () -> {
        if (!shown || isFinishing() || isDestroyed()) return;
        if (feed != null) HostFeed.invalidateRecentStreams(feed.binder());
        refreshedSignature = null;
        reloadNow();
    };
    private final Runnable settle = () -> {
        settled = true;
        reloadNow();
    };
    private final Runnable stopWaiting = () -> feed.stopWaiting();
    private Object markedHostOwner;
    private Runnable clearMarkedHost;
    private final androidx.activity.OnBackPressedCallback unmarkBack =
            new androidx.activity.OnBackPressedCallback(false) {
                @Override
                public void handleOnBackPressed() {
                    Runnable clear = clearMarkedHost;
                    clearMarkedHost = null;
                    markedHostOwner = null;
                    setEnabled(false);
                    if (clear != null) clear.run();
                }
            };

    /** The data behind the screen; the debug demo overrides this with canned hosts. */
    protected ConsoleRepository createRepository(String uniqueId) {
        return ConsoleRepository.create(this, uniqueId);
    }

    /** Its chrome follows the form: a rotation into another form lays it out again. */
    @Override
    protected boolean followsForm() {
        return true;
    }

    /** A pairing, a wake or a game being added keeps this screen until it is done; it then follows the form. */
    @Override
    protected boolean holdsForm() {
        return hostActions.isPairing() || hostActions.isWaking() || hostActions.isShowingError() || wake.isWaiting()
                || !adding.isEmpty();
    }

    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        if (state == null) {
            com.limelight.files.FileShareIntents.releaseStaleGrants(this, com.limelight.files.RunningTransfers.activeGrantUris());
        }
        // Once per phone and app version; a task brought back with Home in front never creates the PC list, which asks too.
        DecoderBenchmark.runIfNeeded(this);
        setContentView(R.layout.activity_console_home);
        UiHelper.notifyNewRootView(this);

        TopAppBar.configure(this, R.id.console_toolbar, getString(R.string.console_title), 0);
        ShellBar.install(this, ShellBar.Destination.HOME);
        ShellBar.installBack(this, ShellBar.Destination.HOME);
        getOnBackPressedDispatcher().addCallback(this, unmarkBack);

        loading = findViewById(R.id.console_loading);
        empty = findViewById(R.id.console_empty);
        findViewById(R.id.console_empty_hosts).setOnClickListener(v -> openHosts());

        list = findViewById(R.id.console_list);
        // A RecyclerView makes itself focusable in touch mode whatever its layout says. A finger that ends key navigation then
        // hands the keys' focus to the list, which gives it to the first shelf strip below Play; that shelf's snap starts a
        // scroll, and the list takes the finger's down to stop it, so the tap never reached Play. Keys still focus the list.
        list.setFocusableInTouchMode(false);
        feed = new HostFeed(this, this);
        pairingBanner = findViewById(R.id.home_pairing_banner);
        pairingBanner.setOnClickListener(v -> hostActions.approvePairing(waitingToPair));
        setupCard = findViewById(R.id.home_setup_card);
        findViewById(R.id.home_setup_dismiss).setOnClickListener(v -> {
            boolean focused = v.isFocused();
            SetupCard.dismiss(this);
            showSetupCard();
            // A pad or a remote was on the button that just went: focus goes back to the games.
            if (focused) list.requestFocus();
        });
        pairingWatch = new PairingWatch(hostActions.pairingReader(), HostMenu::mayApprovePairing,
                task -> new Thread(task, "Devices waiting to pair").start(), handler::post, this::showWaitingToPair);
        ConsoleHomeAdapter.Listener listener = cardListener();
        view = HomeView.read(PreferenceManager.getDefaultSharedPreferences(this), isTelevision());
        if (view.layout == HomeView.Layout.ROWS) {
            focus.setUpShelves(listener);
        } else {
            focus.setUpGrid(listener);
        }
        chrome = new HomeChrome(this, list, view.layout == HomeView.Layout.ROWS);
        capsule = new HostCapsule(this, findViewById(R.id.glass_host), findViewById(R.id.home_pc), new HostCapsule.Screen() {
            @Override public HomeModel model() { return model; }
            @Override public ComputerDetails host(String uuid) { return feed.host(uuid); }
            @Override public ComputerManagerService.ComputerManagerBinder binder() { return feed.binder(); }
            @Override public HostActions actions() { return hostActions; }
            @Override public void setHostFilter(String uuid) {
                HomeHostFilter.set(ConsoleHomeActivity.this, uuid);
                scheduleReload();
            }
        });
        focus.setUpAmbience();
        HomeViewSheet.setUpButton(this, () -> viewSheet.show(() -> view, next -> view = next, model, isTelevision()));
        setUpSearch(listener);
        feed.bind();
    }

    private ConsoleHomeAdapter.Listener cardListener() {
        return new ConsoleHomeAdapter.Listener() {
            @Override public void onGame(HomeModel.GameCard card) { gameActions.openGame(card); }
            @Override public void onGameMenu(HomeModel.GameCard card) { askFavourite(card); }
            @Override public void onGameToggleFavourite(HomeModel.GameCard card) { gameActions.setFavourite(card, !card.favourite); }
            @Override public void onHost(HomeModel.HostCard card) { openHost(card); }
            @Override public void onHostOptions(HomeModel.HostCard card) { capsule.showOptions(card); }
            @Override public void onAllGames(String filter) {
                startActivity(new Intent(ConsoleHomeActivity.this, allGamesActivity())
                        .putExtra(AllGamesActivity.EXTRA_FILTER, filter));
            }
            @Override public void onHeroPlay(HomeModel.GameCard card) { playHero(card); }
            @Override public void onHeroQuit(HomeModel.GameCard card) { gameActions.quitHero(card); }
            @Override public void onHeroShown(HomeModel.GameCard card) {
                if (ambience != null) ambience.heroShown(card);
            }
            @Override public void onHeroFocus(CharSequence go) {
                if (go != null && hints != null) hints.onFocus(go.length() == 0 ? null : go, getString(R.string.console_hint_options));
            }
            @Override public void onHostMoreMark(Object owner, Runnable clear) {
                if (clear != null) {
                    markedHostOwner = owner;
                    clearMarkedHost = clear;
                } else if (markedHostOwner == owner) {
                    markedHostOwner = null;
                    clearMarkedHost = null;
                }
                unmarkBack.setEnabled(clearMarkedHost != null);
            }
        };
    }

    /** Search on the page where the layout has the field; a TV's opens All games. */
    private void setUpSearch(ConsoleHomeAdapter.Listener listener) {
        searchEnv = new ShelfAdapter.Env(this, covers, feed::host, listener);
        search = focus.setUpSearch(this, searchEnv, () -> model, () -> startActivity(new Intent(this, allGamesActivity())));
    }

    /** By the window's type; every layout has the backdrop now. */
    private boolean isTelevision() {
        return GlassForm.of(this) == GlassForm.TELEVISION;
    }

    /** For tests and the demo's captures: the view Home shows. */
    HomeView view() {
        return view;
    }

    /** Headers take the whole row; wide cards take two columns, or the whole row when it has at most three. */
    static int spanOf(int viewType, int columns) { return HomeFocusManager.spanOf(viewType, columns, false); }

    /** The same on a TV, where wide cards and PCs pair up at every width: four across left no room for a PC's name. */
    static int spanOf(int viewType, int columns, boolean television) { return HomeFocusManager.spanOf(viewType, columns, television); }

    /** Columns for a list this wide: about 130 dp each, and an even number from four on so wide cards pair up. */
    static int columnsFor(int widthDp) { return HomeFocusManager.columnsFor(widthDp, HomeView.Size.MEDIUM); }

    /** The same for cards of the chosen size: about 130 dp each at the medium size. */
    static int columnsFor(int widthDp, HomeView.Size size) { return HomeFocusManager.columnsFor(widthDp, size); }

    /** TV posters across a grid this wide: as many as fit at the shelves' width times the size, at least two. */
    static int tvColumnsFor(int contentPx, int cardPx) { return HomeFocusManager.tvColumnsFor(contentPx, cardPx); }

    /** The last input names the keys in the hints: on the next input, with no animation. */
    private void onInput(InputSource source, int vendor) {
        if (source == null) return;
        InputSource.remember(source);
        if (ambience != null) ambience.setInput(source, vendor);
        if (hints != null) hints.onInput(source, vendor);
    }

    @Override
    public boolean dispatchTouchEvent(MotionEvent event) {
        if (event.getActionMasked() == MotionEvent.ACTION_DOWN) {
            atStart = false;
            onInput(InputSource.of(event), 0);
        }
        return super.dispatchTouchEvent(event);
    }

    @Override
    public boolean dispatchGenericMotionEvent(MotionEvent event) {
        if (InputSource.of(event) != null) atStart = false;
        onInput(InputSource.of(event), event.getDevice() == null ? 0 : event.getDevice().getVendorId());
        return super.dispatchGenericMotionEvent(event);
    }

    @Override
    public boolean dispatchKeyEvent(KeyEvent event) {
        if (event.getAction() == KeyEvent.ACTION_DOWN) {
            atStart = false;
            // The key that ends touch mode never gets here: focus stands, and this one moves it. A refresh must not take it back.
            if (InputSource.isDpad(event.getKeyCode())) focusPending = false;
            onInput(InputSource.of(event), InputSource.vendorOf(event));
        }
        if (upToSetupCard(event)) return true;
        // Up from the top of the list goes to the current tab of the shell bar, whatever the focus search would pick
        // (View, at the bar's end, included).
        // On a phone the tabs are at the foot: Up from the top card stays in the top row.
        if (chrome != null && chrome.form() != GlassForm.PHONE
                && ShellFocus.upToCurrentTab(this, event, list,
                        findViewById(R.id.home_top_row), findViewById(R.id.console_view))) {
            return true;
        }
        if (event.getAction() == KeyEvent.ACTION_DOWN && event.hasNoModifiers()
                && event.getKeyCode() == KeyEvent.KEYCODE_DPAD_DOWN
                && chrome != null && chrome.form() != GlassForm.PHONE) {
            View topRow = findViewById(R.id.home_top_row);
            if (topRow != null && topRow.hasFocus()) {
                return focus.focusDownFromTopRow(setupCard, empty);
            }
        }
        return super.dispatchKeyEvent(event);
    }

    /**
     * Up from the list's first row stops on the set-up card's close button: the card stands between that row and the top
     * row, and the button is at its far end, where a focus search from the left columns never looks. Up from the button
     * goes on to the current tab, as it does from the list without the card, and Down from it back to where Up came from.
     */
    private boolean upToSetupCard(KeyEvent event) {
        if (event.getAction() != KeyEvent.ACTION_DOWN || !event.hasNoModifiers()
                || setupCard == null || setupCard.getVisibility() != View.VISIBLE) {
            return false;
        }
        View close = setupCard.findViewById(R.id.home_setup_dismiss);
        if (event.getKeyCode() == KeyEvent.KEYCODE_DPAD_DOWN) {
            if (!close.isFocused()) return false;
            View back = setupCardCameFrom == null ? null : setupCardCameFrom.get();
            setupCardCameFrom = null;
            // The card it left may be gone since (a reload, a scroll): then the list finds its own.
            return (back != null && back.isAttachedToWindow() && inList(back) && back.requestFocus()) || list.requestFocus();
        }
        if (event.getKeyCode() != KeyEvent.KEYCODE_DPAD_UP) return false;
        if (close.isFocused()) {
            // On a phone the tabs are at the foot: Up from the button stays where the focus search leaves it.
            return chrome != null && chrome.form() != GlassForm.PHONE
                    && ShellFocus.upToCurrentTab(this, event, setupCard, findViewById(R.id.home_top_row));
        }
        View focused = list.findFocus();
        if (focused == null || !close.isFocusable()) return false;
        // Still inside the list: this press moves among the rows.
        if (inList(focused.focusSearch(View.FOCUS_UP))) return false;
        if (!close.requestFocus()) return false;
        setupCardCameFrom = new WeakReference<>(focused);
        return true;
    }

    private boolean inList(View view) {
        for (View at = view; at != null; at = at.getParent() instanceof View ? (View) at.getParent() : null) {
            if (at == list) return true;
        }
        return false;
    }

    @Override
    protected void onResume() {
        super.onResume();
        shown = true;
        // Settings may have been changed since: the card speaks for Auto only.
        showSetupCard();
        ShellBar.refresh(this);
        feed.prune();
        feed.resume();
        // A game may have started or ended while another screen was in front.
        refreshedSignature = null;
        handler.removeCallbacks(followUpRefresh);
        if (pausedOnce) {
            handler.postDelayed(followUpRefresh, RESUME_REFRESH_MS);
        }
        handler.removeCallbacks(settle);
        handler.postDelayed(settle, SETTLE_MS);
        handler.removeCallbacks(stopWaiting);
        if (feed.waiting()) {
            handler.postDelayed(stopWaiting, WAIT_LIMIT_MS);
        }
        scheduleReload();
        // However Home was opened: after pairing or from a task brought back, no PC list is under it to ask.
        checkForUpdates();
        transfersButton = RunningTransfers.showOn(this);
    }

    /** At most every 12 hours, a limit shared with the PC list; the offer shows only while Home is in front. */
    protected void checkForUpdates() {
        UpdateFlow.autoCheck(this, () -> shown);
    }

    @Override
    protected void onPause() {
        shown = false;
        pausedOnce = true;
        if (transfersButton != null) { transfersButton.run(); transfersButton = null; }
        pairingWatch.pause();
        wake.cancel();
        feed.pause();
        handler.removeCallbacks(reload);
        handler.removeCallbacks(followUpRefresh);
        handler.removeCallbacks(settle);
        handler.removeCallbacks(stopWaiting);
        super.onPause();
    }

    @Override
    protected void onDestroy() {
        handler.removeCallbacksAndMessages(null);
        if (pairingWatch != null) pairingWatch.close();
        hostActions.destroy();
        feed.unbind();
        worker.shutdownNow();
        starWorker.shutdownNow();
        if (chrome != null) chrome.destroy();
        if (ambience != null) ambience.destroy();
        if (covers != null) covers.release();
        super.onDestroy();
    }

    @Override
    public void onConfigurationChanged(Configuration newConfig) {
        super.onConfigurationChanged(newConfig);
        if (recreatingForForm()) return;
        UiHelper.notifyNewRootView(this);
        focus.resetWidth();
    }

    @Override
    public void onHostsChanged() {
        // Every poll reports its PC, changed or not; only news is worth reading the library and binding the cards for.
        if (drawn.changed(feed.hosts(), feed.waiting())) {
            scheduleReload();
        }
        // The card waits for its PC to be among the hosts and goes when that PC is unpaired or removed. A poll works it
        // out again only then: the settings it speaks for change on other screens, and a resume reads them.
        String cardFor = SetupCard.host(this);
        if (cardFor != null && (feed.host(cardFor) != null) != setupCardHostThere) {
            showSetupCard();
        }
        // The watch has no timer of its own: it counts on every report.
        if (shown) {
            pairingWatch.poll(feed.hosts());
        }
    }

    /**
     * The set-up card, while there is one: its PC is paired, the settings are Recommended's Auto, and nothing waits to
     * pair (the banner stands in the same place and asks for an answer; the card only tells).
     */
    private void showSetupCard() {
        String uuid = SetupCard.host(this);
        ComputerDetails host = uuid == null ? null : feed.host(uuid);
        setupCardHostThere = host != null;
        SetupCard.Text text = host == null || !waitingToPair.isEmpty() ? null : SetupCard.text(this, host);
        setupCard.setVisibility(text == null ? View.GONE : View.VISIBLE);
        if (chrome != null) chrome.fitSetupCard();
        if (text == null) return;
        ((TextView) setupCard.findViewById(R.id.home_setup_title)).setText(text.title);
        ((TextView) setupCard.findViewById(R.id.home_setup_values)).setText(text.values);
        setupCard.findViewById(R.id.home_setup_text).setContentDescription(text.title + ". " + text.values);
    }

    /** "TV wants to pair" while something waits. */
    protected void showWaitingToPair(List<PairingWatch.Waiting> waiting) {
        waitingToPair = waiting;
        showSetupCard();
        pairingBanner.setVisibility(waiting.isEmpty() ? View.GONE : View.VISIBLE);
        if (waiting.size() == 1) {
            pairingBanner.setText(getString(R.string.pairing_wants_to_pair, waiting.get(0).shownName()));
        } else if (!waiting.isEmpty()) {
            pairingBanner.setText(getString(R.string.pairing_many_want_to_pair, waiting.size()));
        }
    }

    private void scheduleReload() {
        handler.removeCallbacks(reload);
        handler.postDelayed(reload, DEBOUNCE_MS);
    }

    /** What changes how Home looks or what it asks the hosts: a host's identity, state, address and game. */
    static String signature(List<ComputerDetails> hosts) {
        StringBuilder sb = new StringBuilder();
        for (ComputerDetails host : hosts) {
            sb.append(host.uuid).append('|').append(host.displayName()).append('|').append(host.state).append('|')
                    .append(host.activeAddress == null ? "" : host.activeAddress.address + ":" + host.activeAddress.port)
                    .append('|').append(host.runningGameId).append('|').append(host.runningGameUuid)
                    .append('|').append(host.sessionIdle).append(';');
        }
        return sb.toString();
    }

    private void reloadNow() {
        if (!shown || isFinishing() || isDestroyed()) {
            return;
        }
        String uniqueId = feed.uniqueId();
        if (uniqueId == null) {
            // No manager to read the PCs from; once Home stops waiting for it, it says there are none.
            if (!feed.waiting() && feed.binder() == null) {
                show(++generation, ConsoleSnapshot.empty(), HomeModel.empty(), true, true);
            }
            return;
        }
        if (repository == null) {
            repository = createRepository(uniqueId);
            covers = new CoverLoaders(this, uniqueId, COVER_WIDTH_DP);
            if (ambience != null) covers.setArtListener(ambience::onArt);
            if (searchEnv != null) searchEnv.covers = covers;
            if (shelves != null) {
                shelves.setCovers(covers);
            } else {
                adapter.setCovers(covers);
            }
        }
        final ConsoleRepository repo = repository;
        final List<ComputerDetails> hosts = feed.hosts();
        drawn.set(hosts, feed.waiting());
        final String signature = signature(hosts);
        final boolean refresh = !signature.equals(refreshedSignature);
        refreshedSignature = signature;
        final int mine = ++generation;
        final Set<String> online = HomeGameActions.onlineOf(hosts);
        final String hostFilter = HomeHostFilter.chosen(this);
        if (refresh) {
            loading.setVisibility(View.VISIBLE);
        }
        worker.execute(() -> {
            try {
                Set<String> denied = HomeModel.launchDenied(hosts);
                ConsoleSnapshot saved = repo.cached(hosts);
                show(mine, saved, HomeModel.of(saved, online, denied, hostFilter).sorted(view), hosts.isEmpty(), !refresh);
                if (refresh) {
                    ConsoleSnapshot fresh = repo.refresh(hosts);
                    show(mine, fresh, HomeModel.of(fresh, online, denied, hostFilter).sorted(view), hosts.isEmpty(), true);
                    redrawAtNextPoll(mine);
                }
            } catch (InterruptedException e) {
                clearRefreshedSignature(mine);
                Thread.currentThread().interrupt();
            } catch (RuntimeException e) {
                clearRefreshedSignature(mine);
                LimeLog.warning("Home could not load: " + e);
                show(mine, ConsoleSnapshot.empty(), HomeModel.empty(), hosts.isEmpty(), true);
                redrawAtNextPoll(mine);
            }
        });
    }

    private void clearRefreshedSignature(int mine) {
        runOnUiThread(() -> {
            if (mine == generation) {
                refreshedSignature = null;
            }
        });
    }

    /** A read of the PCs changed what is saved, and a failed load drew nothing: the next poll draws again, once. */
    private void redrawAtNextPoll(int mine) {
        runOnUiThread(() -> {
            if (mine == generation) {
                drawn.forget();
            }
        });
    }

    private void show(int mine, ConsoleSnapshot snapshot, HomeModel model, boolean noHosts, boolean done) {
        runOnUiThread(() -> {
            if (mine != generation || isFinishing() || isDestroyed()) {
                return;
            }
            shownSnapshot = snapshot;
            this.model = model;
            if (capsule != null) capsule.show(model);
            focus.submitKeepingFocus(model);
            if (search != null) search.onModel();
            // While a saved PC has yet to answer, an empty Home is still loading, not "No PCs yet".
            boolean waiting = model.isEmpty() && noHosts && feed.waiting();
            if (done && !waiting) {
                loading.setVisibility(View.GONE);
            }
            // "No hosts" waits until the hosts had time to answer, so it does not flash on every start.
            boolean showEmpty = model.isEmpty() && noHosts && settled && !waiting;
            empty.setVisibility(showEmpty ? View.VISIBLE : View.GONE);
            if (search == null || !search.isOpen()) list.setVisibility(showEmpty ? View.INVISIBLE : View.VISIBLE);
            focusFirstCard(showEmpty);
        });
    }

    private void focusFirstCard(boolean emptyShown) {
        focus.focusFirstCard(emptyShown);
    }

    private void restoreFocus(ConsoleHomeAdapter.Cursor target, int attempt) {
        focus.restoreFocus(target, attempt);
    }

    /** Where the grid starts the list for the card at {@code position}, from what the list has laid out. */
    ConsoleHomeListAdapter.Reveal revealFor(int position) {
        return focus.revealFor(position);
    }

    private void playHero(HomeModel.GameCard card) {
        gameActions.playHero(card);
    }

    /** The long-press menu of a game card: star or unstar it, hide it. A game not added has no app id for either. */
    void askFavourite(HomeModel.GameCard card) {
        gameActions.askFavourite(card);
    }

    /** Starts the stream; the debug demo overrides this so no stream is opened. */
    protected void play(NvApp app, ComputerDetails computer, String source) {
        ServerHelper.doStart(this, app, computer, feed.binder(), source);
    }

    /** Ends the game on the host, then calls {@code onComplete} on any thread. */
    protected void end(NvApp app, ComputerDetails computer, Runnable onComplete) {
        ServerHelper.doQuit(this, computer, app, feed.binder(), onComplete);
    }

    /** Closes the game on the host; {@code onDone} learns on any thread whether it is closed. */
    protected void close(NvApp app, ComputerDetails computer, Consumer<Boolean> onDone) {
        ServerHelper.doQuitReporting(this, computer, app, feed.binder(), onDone);
    }

    private void openGameOn(String hostUuid, int appId, String title, String hostName) {
        startActivity(new Intent(this, detailActivity())
                .putExtra(GameDetailActivity.EXTRA_HOST_UUID, hostUuid)
                .putExtra(GameDetailActivity.EXTRA_APP_ID, appId)
                .putExtra(GameDetailActivity.EXTRA_TITLE, title)
                .putExtra(GameDetailActivity.EXTRA_HOST_NAME, hostName));
    }

    /** Sends the wake packets to a PC that is away (off the UI thread); the debug demo and tests override this. */
    protected void sendWake(ComputerDetails computer) throws IOException {
        WakeOnLanSender.sendWolPacket(this, computer);
    }

    /** The screen a game opens; the debug demo returns its own. */
    protected Class<? extends GameDetailActivity> detailActivity() {
        return GameDetailActivity.class;
    }

    /** The screen an "All N" tile opens; the debug demo returns its own. */
    protected Class<? extends AllGamesActivity> allGamesActivity() {
        return AllGamesActivity.class;
    }

    private void openHost(HomeModel.HostCard card) {
        if (!card.online) {
            // A PC that is away is woken from here; one that cannot be (no MAC address yet) goes to the list of PCs.
            if (!wake.wake(card.uuid)) {
                openHosts();
            }
            return;
        }
        openLibrary(card.uuid, card.name);
    }

    private void openLibrary(String uuid, String name) {
        HostCapsule.openLibrary(this, uuid, name);
    }

    /** The list of PCs: pairing and adding a PC. */
    private void openHosts() {
        ShellBar.open(this, ShellBar.Destination.HOME, ShellBar.Destination.PCS);
    }
}
