package com.limelight.console;

import android.content.Intent;
import android.content.res.Configuration;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.view.View;

import androidx.recyclerview.widget.GridLayoutManager;
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
import com.limelight.ui.NightjarActivity;
import com.limelight.ui.ShellBar;
import com.limelight.ui.TopAppBar;
import com.limelight.utils.ServerHelper;
import com.limelight.utils.UiHelper;

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
    private ConsoleHomeAdapter adapter;
    private CoverLoaders covers;
    private RecyclerView list;
    private GridLayoutManager grid;
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
        grid = new GridLayoutManager(this, columns);
        grid.setSpanSizeLookup(new GridLayoutManager.SpanSizeLookup() {
            @Override
            public int getSpanSize(int position) {
                return spanOf(adapter.viewTypeAt(position), columns);
            }
        });
        list.setLayoutManager(grid);
        list.setItemAnimator(null);
        list.addOnLayoutChangeListener((v, l, t, r, b, ol, ot, or, ob) -> fitColumns(r - l));

        feed = new HostFeed(this, this);
        adapter = new ConsoleHomeAdapter(this, null, feed::host, new ConsoleHomeAdapter.Listener() {
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
        });
        list.setAdapter(adapter);
        feed.bind();
    }

    /** Headers take the whole row; wide cards take two columns, or the whole row when it has at most three. */
    static int spanOf(int viewType, int columns) {
        switch (viewType) {
            case ConsoleHomeAdapter.HEADER:
                return columns;
            case ConsoleHomeAdapter.COVER_GAME:
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
        // A card keeps its margin around it (a TV cover pads itself with its glow instead).
        int margin = getResources().getDimensionPixelSize(R.dimen.cover_inset);
        int columnWidth = (widthPx - list.getPaddingLeft() - list.getPaddingRight()) / count - 2 * margin;
        boolean television = (getResources().getConfiguration().uiMode & android.content.res.Configuration.UI_MODE_TYPE_MASK)
                == android.content.res.Configuration.UI_MODE_TYPE_TELEVISION;
        list.post(() -> {
            if (count != columns) {
                columns = count;
                grid.setSpanCount(count);
            }
            adapter.setCoverHeight(Math.max(1, television ? columnWidth * 3 / 2 : columnWidth * 4 / 3));
        });
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
            adapter.setCovers(covers);
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
                show(mine, HomeModel.of(repo.cached(hosts), online), hosts.isEmpty(), !refresh);
                if (refresh) {
                    ConsoleSnapshot fresh = repo.refresh(hosts);
                    show(mine, HomeModel.of(fresh, online), hosts.isEmpty(), true);
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
    }

    private void openGame(HomeModel.GameCard card) {
        startActivity(new Intent(this, detailActivity())
                .putExtra(GameDetailActivity.EXTRA_HOST_UUID, card.hostUuid)
                .putExtra(GameDetailActivity.EXTRA_APP_ID, card.appId)
                .putExtra(GameDetailActivity.EXTRA_TITLE, card.title)
                .putExtra(GameDetailActivity.EXTRA_HOST_NAME, card.hostName));
    }

    /** Submits the model; when a card had focus, the same card (by its stable id) has it afterwards. */
    private void submitKeepingFocus(HomeModel model) {
        View focused = list.getFocusedChild();
        int at = focused == null ? -1 : list.getChildAdapterPosition(focused);
        String key = at < 0 ? null : adapter.keyAt(at);
        adapter.submit(model);
        if (key == null) {
            return;
        }
        // A card that left (an unstarred favourite) hands focus to its neighbour instead of dropping it.
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

    /** The long-press menu of a game card: star or unstar it. */
    private void askFavourite(HomeModel.GameCard card) {
        if (repository == null) {
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
        if (repo == null) {
            return;
        }
        final List<ComputerDetails> hosts = feed.hosts();
        final Set<String> online = onlineOf(hosts);
        final String key = card.source != null ? card.source.stableId() : "";
        starWorker.execute(() -> {
            try {
                repo.setFavourite(card.hostUuid, card.hostName, key, card.appId, card.title, starred);
                HomeModel model = HomeModel.of(repo.cached(hosts), online);
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

    private void openHost(HomeModel.HostCard card) {
        if (!card.online) {
            openHosts();
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
