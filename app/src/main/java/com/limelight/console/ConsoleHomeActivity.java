package com.limelight.console;

import android.content.Intent;
import android.content.res.Configuration;
import android.graphics.drawable.ColorDrawable;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.view.View;
import android.view.ViewGroup;

import androidx.core.content.ContextCompat;
import androidx.recyclerview.widget.GridLayoutManager;
import androidx.recyclerview.widget.LinearLayoutManager;
import androidx.recyclerview.widget.RecyclerView;

import com.google.android.material.dialog.MaterialAlertDialogBuilder;
import com.limelight.AppView;
import com.limelight.R;
import com.limelight.computers.ComputerManagerService;
import com.limelight.computers.HostActions;
import com.limelight.computers.HostMenu;
import com.limelight.grid.PcGridAdapter;
import com.limelight.nativeapi.ConsoleRepository;
import com.limelight.nativeapi.ConsoleSnapshot;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.preferences.HostStreamSettings;
import com.limelight.ui.ActionSheet;
import com.limelight.ui.AppTheme;
import com.limelight.ui.BackdropView;
import com.limelight.ui.Motion;
import com.limelight.ui.NightjarActivity;
import com.limelight.ui.ShellBar;
import com.limelight.ui.TopAppBar;
import com.limelight.utils.ServerHelper;
import com.limelight.utils.UiHelper;

import java.util.Collections;
import java.util.HashSet;
import java.util.List;
import java.util.Set;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

/**
 * Home: the games to continue, the library across the paired hosts and the hosts. It paints what is saved
 * first and refreshes from the hosts that answer; a host without the native API shows its classic app list
 * and opens the classic library when tapped. The back button leaves the app.
 */
public class ConsoleHomeActivity extends NightjarActivity implements HostFeed.Listener {
    /** How long after coming to the front "no hosts" waits for the first host to answer. */
    private static final long SETTLE_MS = 1500;
    private static final long DEBOUNCE_MS = 250;
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
    /** The phone's Home: one grid of headers and cards. Null on TV. */
    private ConsoleHomeListAdapter adapter;
    private GridLayoutManager grid;
    /** The TV's Home: shelves under a hero. Null on a phone. */
    private ConsoleHomeAdapter shelves;
    private CoverLoaders covers;
    private RecyclerView list;
    private HomeAmbience ambience;
    private View loading;
    private View empty;
    private volatile ConsoleRepository repository;
    private int generation;
    private String refreshedSignature;
    private boolean settled;
    private boolean shown;
    private boolean focusPending = true;
    private int columns = 2;
    private int lastWidthPx;

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
        @Override public void pausePolling() { feed.pause(); }
        @Override public void resumePolling() { if (shown) feed.resume(); }
    });
    private final HomeWake wake = new HomeWake(this, handler, uuid -> feed == null ? null : feed.host(uuid),
            this::sendWake, () -> {
        refreshedSignature = null;
        scheduleReload();
    });
    /** A "which PC?" question is pending; further taps on games wait for its answer. */
    private boolean choosing;
    private ActionSheet hostMenu;
    private HomeModel model = HomeModel.empty();

    private final Runnable reload = this::reloadNow;
    private final Runnable settle = () -> {
        settled = true;
        reloadNow();
    };

    /** The data behind the screen; the debug demo overrides this with canned hosts. */
    protected ConsoleRepository createRepository(String uniqueId) {
        return ConsoleRepository.create(this, uniqueId);
    }

    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        setContentView(R.layout.activity_console_home);
        UiHelper.notifyNewRootView(this);

        TopAppBar.configure(this, R.id.console_toolbar, getString(R.string.console_title), 0);
        ShellBar.install(this, ShellBar.Destination.HOME);
        ShellBar.installBack(this, ShellBar.Destination.HOME);

        loading = findViewById(R.id.console_loading);
        empty = findViewById(R.id.console_empty);
        findViewById(R.id.console_empty_hosts).setOnClickListener(v -> openHosts());

        list = findViewById(R.id.console_list);
        feed = new HostFeed(this, this);
        ConsoleHomeAdapter.Listener listener = new ConsoleHomeAdapter.Listener() {
            @Override
            public void onGame(HomeModel.GameCard card) {
                openGame(card);
            }

            @Override
            public void onGameMenu(HomeModel.GameCard card) {
                askFavourite(card);
            }

            @Override
            public void onGameToggleFavourite(HomeModel.GameCard card) {
                setFavourite(card, !card.favourite);
            }

            @Override
            public void onHost(HomeModel.HostCard card) {
                openHost(card);
            }

            @Override
            public void onHostOptions(HomeModel.HostCard card) {
                showHostOptions(card);
            }

            @Override
            public void onAllGames(String filter) {
                startActivity(new Intent(ConsoleHomeActivity.this, allGamesActivity())
                        .putExtra(AllGamesActivity.EXTRA_FILTER, filter));
            }
        };
        if (isTelevision()) {
            setUpShelves(listener);
        } else {
            setUpGrid(listener);
        }
        feed.bind();
    }

    /** The TV layout has the backdrop behind the shelves; the phone layout is the plain grid. */
    private boolean isTelevision() {
        return findViewById(R.id.home_backdrop) != null;
    }

    private void setUpGrid(ConsoleHomeAdapter.Listener listener) {
        grid = new HomeGridLayoutManager(this, columns, position -> {
            int first = adapter.firstCard();
            return first >= 0 && first < position;
        });
        grid.setSpanSizeLookup(new GridLayoutManager.SpanSizeLookup() {
            @Override
            public int getSpanSize(int position) {
                return spanOf(adapter.viewTypeAt(position), columns);
            }
        });
        list.setLayoutManager(grid);
        list.setItemAnimator(null);
        list.addOnLayoutChangeListener((v, l, t, r, b, ol, ot, or, ob) -> {
            fitColumns(r - l);
            ConsoleHomeListAdapter.fadeHeaders(list);
        });
        list.addOnScrollListener(new RecyclerView.OnScrollListener() {
            @Override
            public void onScrolled(RecyclerView view, int dx, int dy) {
                ConsoleHomeListAdapter.fadeHeaders(view);
            }
        });
        adapter = new ConsoleHomeListAdapter(this, null, feed::host, listener);
        list.setAdapter(adapter);
    }

    /** Headers take the whole row; wide cards take two columns, or the whole row when it has at most three. */
    static int spanOf(int viewType, int columns) {
        switch (viewType) {
            case ConsoleHomeListAdapter.HEADER:
                return columns;
            case ConsoleHomeListAdapter.COVER_GAME:
                return 1;
            default:
                return columns <= 3 ? columns : 2;
        }
    }

    /** Columns for a list this wide: about 130 dp each, and an even number from four on so wide cards pair up. */
    static int columnsFor(int widthDp) {
        int columns = Math.max(2, widthDp / 130);
        return columns >= 4 && columns % 2 == 1 ? columns - 1 : columns;
    }

    private void fitColumns(int widthPx) {
        if (widthPx <= 0 || widthPx == lastWidthPx) {
            return;
        }
        lastWidthPx = widthPx;
        float density = getResources().getDisplayMetrics().density;
        int count = columnsFor(Math.round(widthPx / density));
        // A card keeps its margin around it.
        int margin = getResources().getDimensionPixelSize(R.dimen.cover_inset);
        int columnWidth = (widthPx - list.getPaddingLeft() - list.getPaddingRight()) / count - 2 * margin;
        list.post(() -> {
            if (count != columns) {
                columns = count;
                grid.setSpanCount(count);
            }
            adapter.setCoverHeight(Math.max(1, columnWidth * 4 / 3));
        });
    }

    /** The shelves: a vertical list of rows that snaps the focused row to the top and returns to each row's last card. */
    private void setUpShelves(ConsoleHomeAdapter.Listener listener) {
        shelves = new ConsoleHomeAdapter(this, null, feed::host, listener);
        ShelfLayoutManager rows = new ShelfLayoutManager(this, LinearLayoutManager.VERTICAL,
                () -> shelves.isFastTravel() || Motion.reduced(this));
        rows.setRowFocus(shelves);
        list.setLayoutManager(rows);
        list.setItemAnimator(null);
        list.setItemViewCacheSize(2);
        shelves.setTelevision(true);
        list.setAdapter(shelves);
        setUpAmbience();
    }

    private void setUpAmbience() {
        BackdropView backdrop = findViewById(R.id.home_backdrop);
        View hero = findViewById(R.id.home_hero);
        int canvas = ContextCompat.getColor(this, R.color.tv_canvas);
        // The window shows the canvas until the first backdrop covers it, then stops painting under it.
        getWindow().setBackgroundDrawable(new ColorDrawable(canvas));
        // The TV's safe-area padding sits on the content view, which would otherwise clip the backdrop that reaches past it.
        ((ViewGroup) findViewById(android.R.id.content)).setClipChildren(false);
        backdrop.setCanvasColor(canvas);
        backdrop.setDim(AppTheme.isHighContrast(this) ? 0.85f : 1f);
        backdrop.setOnCovered(() -> getWindow().setBackgroundDrawable(null));
        ambience = new HomeAmbience(this, backdrop, hero, feed::host, () -> covers, shelves::isFastTravel);
        ambience.setGamepad(com.limelight.ui.StreamMenuHint.hasGamepad());
        shelves.setFocusListener(ambience);
    }

    /** The hero's key hints follow the last key: a gamepad's A and X, or a remote's OK and Menu. */
    @Override
    public boolean dispatchKeyEvent(android.view.KeyEvent event) {
        if (ambience != null && event.getAction() == android.view.KeyEvent.ACTION_DOWN) {
            ambience.setGamepad(fromGamepad(event));
        }
        // Up from the top of the list goes to the current tab of the shell bar, whatever the focus search would pick.
        if (com.limelight.ui.ShellFocus.upToCurrentTab(this, event, list)) {
            return true;
        }
        return super.dispatchKeyEvent(event);
    }

    static boolean fromGamepad(android.view.KeyEvent event) {
        // A pad's D-pad and sticks often arrive as SOURCE_JOYSTICK keys, without the gamepad bit.
        int source = event.getSource();
        return (source & android.view.InputDevice.SOURCE_GAMEPAD) == android.view.InputDevice.SOURCE_GAMEPAD
                || (source & android.view.InputDevice.SOURCE_JOYSTICK) == android.view.InputDevice.SOURCE_JOYSTICK;
    }

    @Override
    protected void onResume() {
        super.onResume();
        shown = true;
        ShellBar.refresh(this);
        feed.prune();
        feed.resume();
        // A game may have started or ended while another screen was in front.
        refreshedSignature = null;
        handler.removeCallbacks(settle);
        handler.postDelayed(settle, SETTLE_MS);
        scheduleReload();
    }

    @Override
    protected void onPause() {
        shown = false;
        wake.cancel();
        feed.pause();
        handler.removeCallbacks(reload);
        handler.removeCallbacks(settle);
        super.onPause();
    }

    @Override
    protected void onDestroy() {
        handler.removeCallbacksAndMessages(null);
        hostActions.destroy();
        feed.unbind();
        worker.shutdownNow();
        starWorker.shutdownNow();
        if (ambience != null) {
            ambience.destroy();
        }
        if (covers != null) {
            covers.release();
        }
        super.onDestroy();
    }

    @Override
    public void onConfigurationChanged(Configuration newConfig) {
        super.onConfigurationChanged(newConfig);
        UiHelper.notifyNewRootView(this);
        lastWidthPx = 0;
    }

    @Override
    public void onHostsChanged() {
        scheduleReload();
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
                    .append('|').append(host.runningGameId).append('|').append(host.runningGameUuid).append(';');
        }
        return sb.toString();
    }

    private void reloadNow() {
        if (!shown || isFinishing() || isDestroyed()) {
            return;
        }
        String uniqueId = feed.uniqueId();
        if (uniqueId == null) {
            return;
        }
        if (repository == null) {
            repository = createRepository(uniqueId);
            covers = new CoverLoaders(this, uniqueId, COVER_WIDTH_DP);
            if (ambience != null) covers.setArtListener(ambience::onArt);
            if (shelves != null) {
                shelves.setCovers(covers);
            } else {
                adapter.setCovers(covers);
            }
        }
        final ConsoleRepository repo = repository;
        final List<ComputerDetails> hosts = feed.hosts();
        final String signature = signature(hosts);
        final boolean refresh = !signature.equals(refreshedSignature);
        refreshedSignature = signature;
        final int mine = ++generation;
        final Set<String> online = onlineOf(hosts);
        if (refresh) {
            loading.setVisibility(View.VISIBLE);
        }
        worker.execute(() -> {
            try {
                Set<String> denied = HomeModel.launchDenied(hosts);
                show(mine, HomeModel.of(repo.cached(hosts), online, denied), hosts.isEmpty(), !refresh);
                if (refresh) {
                    ConsoleSnapshot fresh = repo.refresh(hosts);
                    show(mine, HomeModel.of(fresh, online, denied), hosts.isEmpty(), true);
                }
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
            } catch (RuntimeException e) {
                com.limelight.LimeLog.warning("Home could not load: " + e);
                show(mine, HomeModel.empty(), hosts.isEmpty(), true);
            }
        });
    }

    private static Set<String> onlineOf(List<ComputerDetails> hosts) {
        Set<String> online = new HashSet<>();
        for (ComputerDetails host : hosts) {
            if (host.state == ComputerDetails.State.ONLINE) {
                online.add(host.uuid);
            }
        }
        return online;
    }

    private void show(int mine, HomeModel model, boolean noHosts, boolean done) {
        runOnUiThread(() -> {
            if (mine != generation || isFinishing() || isDestroyed()) {
                return;
            }
            this.model = model;
            submitKeepingFocus(model);
            if (done) {
                loading.setVisibility(View.GONE);
            }
            // "No hosts" waits until the hosts had time to answer, so it does not flash on every start.
            boolean showEmpty = model.isEmpty() && noHosts && settled;
            empty.setVisibility(showEmpty ? View.VISIBLE : View.GONE);
            list.setVisibility(showEmpty ? View.INVISIBLE : View.VISIBLE);
            focusFirstCard(showEmpty);
        });
    }

    /** With a D-pad or controller the first card has focus from the start, so a press opens something. */
    private void focusFirstCard(boolean emptyShown) {
        if (!focusPending) {
            return;
        }
        if (emptyShown) {
            focusPending = false;
            findViewById(R.id.console_empty_hosts).requestFocus();
            return;
        }
        if (shelves == null) {
            int first = adapter.initialFocus();
            if (first < 0 || list.isInTouchMode()) {
                return;
            }
            focusPending = false;
            list.post(() -> {
                RecyclerView.ViewHolder holder = list.findViewHolderForAdapterPosition(first);
                if (holder != null) {
                    holder.itemView.requestFocus();
                } else {
                    grid.scrollToPosition(first);
                    focusPending = true;
                }
            });
            return;
        }
        ConsoleHomeAdapter.Cursor first = shelves.initialFocus();
        if (first == null) {
            if (ambience != null) ambience.clear();
            return;
        }
        if (list.isInTouchMode()) {
            // No card takes focus until a key is pressed, but the hero is not left blank: it shows the card focus would start on.
            if (ambience != null && shelves.focusedCard(list) == null) ambience.refresh(shelves.itemAt(first));
            return;
        }
        focusPending = false;
        restoreFocus(first, 0);
    }

    /** Focuses the card once its shelf and it are laid out; a card out of range is brought in and asked for again. */
    private void restoreFocus(ConsoleHomeAdapter.Cursor target, int attempt) {
        list.post(() -> {
            View card = shelves.cardView(list, target);
            if (card != null) {
                if (!card.hasFocus()) {
                    card.requestFocus();
                } else if (ambience != null) {
                    // The card kept focus through the refresh: its state may have changed, so the hero is written again.
                    ambience.refresh(shelves.itemAt(target));
                }
            } else if (attempt < 3) {
                shelves.reveal(list, target);
                restoreFocus(target, attempt + 1);
            }
        });
    }

    private void openGame(HomeModel.GameCard card) {
        // A game in several PCs' libraries asks which one first; a found game that is not added opens as before.
        if (!card.notAdded() && card.hostCount > 1 && askHost(card)) {
            return;
        }
        GameOpener.open(opener, worker, card);
    }

    private void openGameOn(String hostUuid, int appId, String title, String hostName) {
        startActivity(new Intent(this, detailActivity())
                .putExtra(GameDetailActivity.EXTRA_HOST_UUID, hostUuid)
                .putExtra(GameDetailActivity.EXTRA_APP_ID, appId)
                .putExtra(GameDetailActivity.EXTRA_TITLE, title)
                .putExtra(GameDetailActivity.EXTRA_HOST_NAME, hostName));
    }

    /**
     * A game that several PCs have asks which one to open it on, from the saved libraries.
     *
     * @return false when there is nothing to ask (no library read yet): the card's own PC opens instead
     */
    private boolean askHost(HomeModel.GameCard card) {
        final ConsoleRepository repo = repository;
        final String libraryKey = HostChoice.libraryKeyOf(card.key);
        if (repo == null || libraryKey == null) {
            return false;
        }
        if (choosing) {
            return true;
        }
        choosing = true;
        final List<ComputerDetails> hosts = feed.hosts();
        final Set<String> online = onlineOf(hosts);
        // The saved libraries are on disk: read them on the star thread, not behind a refresh from the hosts.
        starWorker.execute(() -> {
            List<HostChoice> choices;
            try {
                choices = HostChoice.of(repo.cached(hosts), libraryKey, online);
            } catch (RuntimeException e) {
                com.limelight.LimeLog.warning("Home could not read the saved libraries: " + e);
                choices = Collections.emptyList();
            }
            final List<HostChoice> found = choices;
            runOnUiThread(() -> {
                if (!shown || isFinishing() || isDestroyed()) {
                    choosing = false;
                } else if (found.size() > 1) {
                    HostChooser.show(this, found, chosen ->
                            openGameOn(chosen.hostUuid, chosen.appId, card.title, chosen.hostName), () -> choosing = false);
                } else {
                    choosing = false;
                    openGameOn(card.hostUuid, card.appId, card.title, card.hostName);
                }
            });
        });
        return true;
    }

    /** Sends the wake packets to a PC that is away (off the UI thread); the debug demo and tests override this. */
    protected void sendWake(ComputerDetails computer) throws java.io.IOException {
        com.limelight.nvstream.wol.WakeOnLanSender.sendWolPacket(computer);
    }

    /** The games this screen is adding on a host now; see {@link GameOpener.Screen#adding}. */
    private final Set<String> adding = new HashSet<>();
    private final GameOpener.Screen opener = new GameOpener.Screen() {
        @Override public android.app.Activity activity() { return ConsoleHomeActivity.this; }
        @Override public ConsoleRepository repository() { return repository; }
        @Override public ComputerDetails host(String uuid) { return feed.host(uuid); }
        @Override public Class<? extends GameDetailActivity> detailActivity() { return ConsoleHomeActivity.this.detailActivity(); }
        @Override public Set<String> adding() { return adding; }
        @Override public void onAdded() {
            refreshedSignature = null;
            scheduleReload();
        }
    };

    /** Submits the model; when a card had focus, the same card (by its stable id) has it afterwards. */
    private void submitKeepingFocus(HomeModel model) {
        if (shelves == null) {
            submitGridKeepingFocus(model);
            return;
        }
        ConsoleHomeAdapter.Cursor at = shelves.focusedCard(list);
        String key = shelves.keyAt(at);
        shelves.submit(model);
        if (key == null) {
            return;
        }
        // A card that left (an unstarred favourite) hands focus to its neighbour instead of dropping it.
        ConsoleHomeAdapter.Cursor target = shelves.positionOfKey(key);
        if (target == null) {
            target = shelves.nearestCard(at);
        }
        if (target == null) {
            return;
        }
        restoreFocus(target, 0);
    }

    private void submitGridKeepingFocus(HomeModel model) {
        View focused = list.getFocusedChild();
        int at = focused == null ? -1 : list.getChildAdapterPosition(focused);
        String key = at < 0 ? null : adapter.keyAt(at);
        adapter.submit(model);
        if (key == null) {
            return;
        }
        int target = adapter.positionOfKey(key);
        if (target < 0) {
            target = adapter.nearestCard(at);
        }
        if (target < 0) {
            return;
        }
        final int position = target;
        list.post(() -> {
            RecyclerView.ViewHolder holder = list.findViewHolderForAdapterPosition(position);
            if (holder != null) {
                if (!holder.itemView.hasFocus()) {
                    holder.itemView.requestFocus();
                }
            } else {
                grid.scrollToPosition(position);
                list.post(() -> {
                    RecyclerView.ViewHolder later = list.findViewHolderForAdapterPosition(position);
                    if (later != null) {
                        later.itemView.requestFocus();
                    }
                });
            }
        });
    }

    /** The long-press menu of a game card: star or unstar it. A game not added has no app id to star. */
    void askFavourite(HomeModel.GameCard card) {
        if (repository == null || card.notAdded()) {
            return;
        }
        boolean remove = card.favourite;
        UiHelper.showDialog(this, new MaterialAlertDialogBuilder(this)
                .setTitle(card.title)
                .setItems(new CharSequence[]{getString(remove ? R.string.console_favourite_remove
                        : R.string.console_favourite_add)}, (dialog, which) -> setFavourite(card, !remove))
                .setNegativeButton(android.R.string.cancel, null).create());
    }

    /**
     * Stars or unstars on the star thread and paints the saved result right away, from the stored library;
     * the normal reload follows and takes over.
     */
    private void setFavourite(HomeModel.GameCard card, boolean starred) {
        final ConsoleRepository repo = repository;
        if (repo == null || card.notAdded()) {
            return;
        }
        final List<ComputerDetails> hosts = feed.hosts();
        final Set<String> online = onlineOf(hosts);
        final String key = card.source != null ? card.source.stableId() : "";
        starWorker.execute(() -> {
            try {
                repo.setFavourite(card.hostUuid, card.hostName, key, card.appId, card.title, starred);
                HomeModel model = HomeModel.of(repo.cached(hosts), online, HomeModel.launchDenied(hosts));
                runOnUiThread(() -> {
                    if (isFinishing() || isDestroyed()) {
                        return;
                    }
                    this.model = model;
                    submitKeepingFocus(model);
                    scheduleReload();
                });
            } catch (RuntimeException e) {
                com.limelight.LimeLog.warning("Home could not update a favourite: " + e);
            }
        });
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
        startActivity(new Intent(this, AppView.class)
                .putExtra(AppView.NAME_EXTRA, name)
                .putExtra(AppView.UUID_EXTRA, uuid));
    }

    /** The list of PCs: pairing and adding a PC. */
    private void openHosts() {
        ShellBar.open(this, ShellBar.Destination.HOME, ShellBar.Destination.PCS);
    }

    /** The PC's options, the same list as on the PCs screen (only for PCs this device is paired with). */
    private void showHostOptions(HomeModel.HostCard card) {
        ComputerDetails details = feed.host(card.uuid);
        if (hostMenu != null || details == null) {
            return;
        }
        String game = runningGameOf(card.uuid);
        ActionSheet sheet = HostMenu.build(this, details, PcGridAdapter.rowState(details, false), game,
                action -> onHostAction(details, action));
        hostMenu = sheet;
        sheet.setOnDismissListener(dialog -> hostMenu = null);
        sheet.show();
    }

    /** The title of the game running on the PC, from what Home shows; null when it does not say. */
    private String runningGameOf(String hostUuid) {
        for (HomeModel.GameCard card : model.continuePlaying) {
            if (card.hostUuid.equals(hostUuid) && card.badge == HomeModel.Badge.RUNNING) {
                return card.title;
            }
        }
        return null;
    }

    private void onHostAction(ComputerDetails details, int action) {
        ComputerManagerService.ComputerManagerBinder binder = feed.binder();
        switch (action) {
            case HostMenu.LIBRARY:
                openLibrary(details.uuid, details.displayName());
                break;
            case HostMenu.RESUME:
            case HostMenu.QUIT:
                if (binder == null) {
                    hostActions.showError(details.displayName(), getString(R.string.error_manager_not_running), null);
                } else if (action == HostMenu.RESUME) {
                    ServerHelper.doStart(this, HostMenu.runningApp(null, details), details, binder);
                } else {
                    UiHelper.displayQuitConfirmationDialog(this, () -> ServerHelper.doQuit(this, details,
                            HostMenu.runningApp(null, details), binder, null), null);
                }
                break;
            case HostMenu.WAKE:
                hostActions.wake(details);
                break;
            case HostMenu.SLEEP:
                hostActions.sleep(details);
                break;
            case HostMenu.PC_SETTINGS:
                HostStreamSettings.show(this, details.uuid, details.displayName());
                break;
            case HostMenu.RENAME:
                hostActions.rename(details);
                break;
            case HostMenu.UNPAIR:
                hostActions.confirmUnpair(details);
                break;
            case HostMenu.REMOVE:
                hostActions.confirmRemove(details);
                break;
            default:
                break;
        }
    }
}
