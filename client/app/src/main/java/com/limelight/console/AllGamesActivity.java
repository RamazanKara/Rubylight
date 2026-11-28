package com.limelight.console;

import android.app.Activity;
import android.content.Context;
import android.content.res.Configuration;
import android.graphics.Rect;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.preference.PreferenceManager;
import android.text.SpannableStringBuilder;
import android.text.Spanned;
import android.text.TextPaint;
import android.text.style.CharacterStyle;
import android.view.LayoutInflater;
import android.view.View;
import android.view.ViewGroup;
import android.widget.LinearLayout;
import android.widget.TextView;

import androidx.annotation.NonNull;
import androidx.recyclerview.widget.GridLayoutManager;
import androidx.recyclerview.widget.RecyclerView;

import com.google.android.material.button.MaterialButton;
import com.limelight.LimeLog;
import com.limelight.R;
import com.limelight.computers.ComputerManagerService;
import com.limelight.computers.HostActions;
import com.limelight.nativeapi.ConsoleRepository;
import com.limelight.nativeapi.ConsoleSnapshot;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.ui.GlassForm;
import com.limelight.ui.NavigationDialogBuilder;
import com.limelight.ui.NightjarActivity;
import com.limelight.ui.ShellBar;
import com.limelight.ui.TopAppBar;
import com.limelight.utils.FrontendSync;
import com.limelight.utils.UiHelper;

import java.util.ArrayList;
import java.util.HashSet;
import java.util.List;
import java.util.Locale;
import java.util.Set;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

/**
 * All games: every game of the paired hosts as one poster grid, under a filter for each source the library has (All,
 * Installed, each store, Other apps, Not added). A row's "All N" tile on Home opens it under that row's filter. It
 * paints what is saved first and refreshes from the hosts once each time it comes to the front.
 */
public class AllGamesActivity extends NightjarActivity implements HostFeed.Listener {
    /** The filter to start under: {@link HomeModel#FILTER_ALL}, "store:steam", ... */
    public static final String EXTRA_FILTER = "filter";
    static final int MAX_COLUMNS = 6;
    private static final int COVER_WIDTH_DP = 140;
    /** How far a focused TV poster's glow reaches past its 24 dp bleed: the grid's frame keeps that much room. */
    static final int GLOW_ROOM_DP = 40;
    /** A TV's safe edge, the usual overscan margin: this share of the screen's height in from its bottom. */
    static final float SAFE_EDGE = 0.05f;
    /** The room a focused TV row keeps between its glow and the frame's bottom. */
    static final int FIT_ROOM_DP = 4;
    private static final long DEBOUNCE_MS = 250;
    private static final String SHELF = "all-games";
    private static final String STATE_QUERY = "query";

    private final Handler handler = new Handler(Looper.getMainLooper());
    private final ExecutorService worker = Executors.newSingleThreadExecutor(r -> {
        Thread thread = new Thread(r, "all-games");
        thread.setDaemon(true);
        return thread;
    });
    private HostFeed feed;
    private ShelfAdapter.Env env;
    private ShelfAdapter adapter;
    private GridLayoutManager grid;
    private AllGamesTvFrame tvFrame;
    private AllGamesSearchBar searchBar;
    private AllGamesFactFilter factFilter;
    private RecyclerView list;
    private LinearLayout filterRow;
    private View loading;
    private View empty;
    private TextView focusMeta;
    private TextView focusTitle;
    private volatile ConsoleRepository repository;
    private CoverLoaders covers;
    private HomeModel model = HomeModel.empty();
    /** Home's View, whose Sort by and Order All games follows. */
    private HomeView view = HomeView.defaults(false);
    private String filter = HomeModel.FILTER_ALL;
    private String filterSignature;
    private String refreshedSignature;
    /** What the last reload drew the PCs from; a poll that brings nothing else does not reload. */
    private final HostsDrawn drawn = new HostsDrawn();
    private int generation;
    private boolean shown;
    private boolean loaded;
    private boolean focusPending = true;
    /** The card that has focus last, by key and place; a bind keeps focus on it even before the grid lays out. */
    private String focusedKey;
    private int focusedAt = -1;
    /** Each focus request outdates the ones still posted. */
    private int focusTicket;
    private final Runnable reload = this::reloadNow;

    /** The data behind the screen; the debug demo overrides this with canned hosts. */
    protected ConsoleRepository createRepository(String uniqueId) {
        return ConsoleRepository.create(this, uniqueId);
    }

    /** The screen a game opens; the debug demo returns its own. */
    protected Class<? extends GameDetailActivity> detailActivity() {
        return GameDetailActivity.class;
    }

    /** The games this screen is adding on a host now; see {@link GameOpener.Screen#adding}. */
    private final Set<String> adding = new HashSet<>();
    private final GameOpener.Screen opener = new GameOpener.Screen() {
        @Override public Activity activity() { return AllGamesActivity.this; }
        @Override public ConsoleRepository repository() { return repository; }
        @Override public ComputerDetails host(String uuid) { return feed.host(uuid); }
        @Override public Class<? extends GameDetailActivity> detailActivity() { return AllGamesActivity.this.detailActivity(); }
        @Override public Set<String> adding() { return adding; }
        @Override public void onAdded() {
            refreshedSignature = null;
            scheduleReload();
        }
    };

    /** The PC capsule's options act through these, as on Home. */
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
        // A form change waited for the wake.
        @Override public void onWakeEnded(ComputerDetails computer) { followFormSoon(); }
        @Override public void pausePolling() { feed.pause(); }
        @Override public void resumePolling() { if (shown) feed.resume(); }
    });
    private HostCapsule capsule;
    private View topRow;
    private View tools;

    /** Its chrome follows the form: a rotation into another form lays it out again. */
    @Override
    protected boolean followsForm() {
        return true;
    }

    /** A pairing or a wake from the PC capsule keeps this screen until it is done; it then follows the form. */
    @Override
    protected boolean holdsForm() {
        return hostActions.isPairing() || hostActions.isWaking() || hostActions.isShowingError();
    }

    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        String requested = state != null ? state.getString(EXTRA_FILTER) : getIntent().getStringExtra(EXTRA_FILTER);
        if (requested != null) filter = requested;
        setContentView(R.layout.activity_all_games);
        UiHelper.notifyNewRootView(this);
        view = HomeView.read(PreferenceManager.getDefaultSharedPreferences(this), isTelevision());

        TopAppBar.configure(this, R.id.all_games_toolbar, getString(R.string.console_all_games_title), 0);
        // All games belongs to Home: Home in the bar is the page it came from, so it closes this one.
        ShellBar.install(this, ShellBar.Destination.HOME);
        findViewById(R.id.shell_home).setOnClickListener(v -> finish());
        setUpTopRow();

        loading = findViewById(R.id.all_games_loading);
        empty = findViewById(R.id.all_games_empty);
        filterRow = findViewById(R.id.all_games_filters);
        focusMeta = findViewById(R.id.all_games_focus_meta);
        focusTitle = findViewById(R.id.all_games_focus_title);
        list = findViewById(R.id.all_games_grid);

        feed = new HostFeed(this, this);
        env = new ShelfAdapter.Env(this, null, feed::host, new ConsoleHomeAdapter.Listener() {
            @Override public void onGame(HomeModel.GameCard card) { GameOpener.open(opener, worker, card); }
            @Override public void onGameMenu(HomeModel.GameCard card) { askFavourite(card); }
            @Override public void onGameToggleFavourite(HomeModel.GameCard card) { setFavourite(card, !card.favourite); }
            @Override public void onHost(HomeModel.HostCard card) { }
            @Override public void onHostOptions(HomeModel.HostCard card) { }
        });
        env.coverHeightPx = ShelfAdapter.coverHeight(this, isTelevision());
        env.focus = (shelf, item, card) -> {
            focusedKey = item.key;
            focusedAt = adapter.positionOfKey(item.key);
            showFocused(item.game);
        };
        adapter = new ShelfAdapter(env);
        grid = new GridLayoutManager(this, MAX_COLUMNS) {
            // A focused row scrolled to the frame's very top stood in the focus line, its title over the poster and its glow
            // cut square: scrolling a card into view keeps the room the first row has above it.
            @Override
            public boolean requestChildRectangleOnScreen(@NonNull RecyclerView parent, @NonNull View child,
                                                         @NonNull Rect rect, boolean immediate,
                                                         boolean focusedChildVisible) {
                Rect withRoom = new Rect(rect);
                int roomAbove = tvFrame == null ? 0 : tvFrame.roomAbove();
                if (roomAbove > 0) withRoom.top = Math.min(rect.top, 0) - roomAbove;
                return super.requestChildRectangleOnScreen(parent, child, withRoom, immediate, focusedChildVisible);
            }
        };
        list.setLayoutManager(grid);
        list.setItemAnimator(null);
        list.setAdapter(adapter);
        tvFrame = new AllGamesTvFrame(this, isTelevision(), list, grid, adapter, env);
        tvFrame.install();
        factFilter = new AllGamesFactFilter(this, filterRow, () -> model, () -> filter, this::bindGrid);
        capsule = new HostCapsule(this, findViewById(android.R.id.content), findViewById(R.id.all_games_pc), new HostCapsule.Screen() {
            @Override public HomeModel model() { return model; }
            @Override public ComputerDetails host(String uuid) { return feed.host(uuid); }
            @Override public ComputerManagerService.ComputerManagerBinder binder() { return feed.binder(); }
            @Override public HostActions actions() { return hostActions; }
            @Override public void setHostFilter(String uuid) {
                HomeHostFilter.set(AllGamesActivity.this, uuid);
                scheduleReload();
            }
        });
        searchBar = new AllGamesSearchBar(this, topRow, tools, () -> focusPending = false,
                this::bindGrid, this::focusFirstMatch);
        searchBar.install(state != null ? state.getString(STATE_QUERY) : null);
        feed.bind();
    }

    /**
     * Home's top row: on a handheld and a TV the tabs in a glass capsule, then the PC capsule and Search at the row's end. A
     * phone keeps its tabs as the bar across the top, and the capsule and Search stand at the end of the title's line.
     */
    private void setUpTopRow() {
        topRow = findViewById(R.id.all_games_top_row);
        tools = findViewById(R.id.all_games_tools);
        GlassForm form = GlassForm.of(this);
        View bar = findViewById(R.id.shell_bar);
        if (form == GlassForm.PHONE) {
            ViewGroup row = (ViewGroup) topRow;
            ViewGroup glass = findViewById(R.id.all_games_tabs);
            glass.removeView(bar);
            row.removeView(glass);
            row.addView(bar, 0, new LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f));
            findViewById(R.id.all_games_top_spacer).setVisibility(View.GONE);
            row.removeView(tools);
            ((ViewGroup) findViewById(R.id.all_games_title_row)).addView(tools);
            // The title's line is narrower than a TV's row: a long name leaves the title its room.
            ((TextView) tools.findViewById(R.id.home_pc_name)).setMaxWidth(
                    getResources().getDimensionPixelSize(R.dimen.glass_capsule) * 3);
            return;
        }
        HomeChrome.styleTabs(this, form);
        HomeChrome.keepRingUnclipped(bar, findViewById(R.id.shell_bar_items));
        ShellBar.linkTopRow(this, R.id.all_games_pc, R.id.all_games_search);
    }

    boolean isSearching() {
        return searchBar != null && searchBar.isSearching();
    }

    void openSearch() {
        if (searchBar != null) searchBar.open();
    }

    /** All the filter's games again, and focus back on Search. */
    void closeSearch() {
        if (searchBar != null) searchBar.close();
    }

    private boolean focusFirstMatch() {
        if (adapter.getItemCount() == 0) return false;
        focusCard(0, 0);
        return true;
    }

    private boolean isTelevision() {
        return (getResources().getConfiguration().uiMode & Configuration.UI_MODE_TYPE_MASK)
                == Configuration.UI_MODE_TYPE_TELEVISION;
    }

    /** As many posters as fit at their narrowest, at most six: six across a TV's screen. */
    static int columnsFor(int contentWidthPx, int minCardWidthPx) {
        return Math.max(2, Math.min(MAX_COLUMNS, contentWidthPx / Math.max(1, minCardWidthPx)));
    }

    @Override
    protected void onSaveInstanceState(@NonNull Bundle out) {
        super.onSaveInstanceState(out);
        out.putString(EXTRA_FILTER, filter);
        if (isSearching()) out.putString(STATE_QUERY, searchBar.query());
    }

    @Override
    protected void onResume() {
        super.onResume();
        shown = true;
        ShellBar.refresh(this);
        feed.prune();
        feed.resume();
        refreshedSignature = null;
        scheduleReload();
    }

    @Override
    protected void onPause() {
        shown = false;
        feed.pause();
        handler.removeCallbacks(reload);
        super.onPause();
    }

    @Override
    protected void onDestroy() {
        handler.removeCallbacksAndMessages(null);
        if (factFilter != null) factFilter.destroy();
        hostActions.destroy();
        feed.unbind();
        worker.shutdownNow();
        if (covers != null) covers.release();
        super.onDestroy();
    }

    @Override
    public void onHostsChanged() {
        // Every poll reports its PC, changed or not; only news is worth reading the library and binding the grid for.
        if (drawn.changed(feed.hosts(), false)) scheduleReload();
    }

    private void scheduleReload() {
        handler.removeCallbacks(reload);
        handler.postDelayed(reload, DEBOUNCE_MS);
    }

    private void reloadNow() {
        if (!shown || isFinishing() || isDestroyed()) return;
        String uniqueId = feed.uniqueId();
        if (uniqueId == null) return;
        if (repository == null) {
            repository = createRepository(uniqueId);
            covers = new CoverLoaders(this, uniqueId, COVER_WIDTH_DP);
            env.covers = covers;
        }
        final ConsoleRepository repo = repository;
        final List<ComputerDetails> hosts = feed.hosts();
        drawn.set(hosts, false);
        final String signature = ConsoleHomeActivity.signature(hosts);
        final boolean refresh = !signature.equals(refreshedSignature);
        refreshedSignature = signature;
        final int mine = ++generation;
        final Set<String> online = onlineOf(hosts);
        final String hostFilter = HomeHostFilter.chosen(this);
        if (refresh) loading.setVisibility(View.VISIBLE);
        worker.execute(() -> {
            try {
                show(mine, HomeModel.of(repo.cached(hosts), online, HomeModel.launchDenied(hosts), hostFilter).sorted(view), !refresh);
                if (refresh) {
                    ConsoleSnapshot fresh = repo.refresh(hosts);
                    show(mine, HomeModel.of(fresh, online, HomeModel.launchDenied(hosts), hostFilter).sorted(view), true);
                    redrawAtNextPoll(mine);
                }
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
                clearRefreshedSignature(mine);
                redrawAtNextPoll(mine);
            } catch (RuntimeException e) {
                LimeLog.warning("All games could not load: " + e);
                clearRefreshedSignature(mine);
                show(mine, HomeModel.empty(), true);
                redrawAtNextPoll(mine);
            }
        });
    }

    /** Clears the refreshed signature if no newer reload took over, so the next poll retries the refresh. */
    private void clearRefreshedSignature(int mine) {
        runOnUiThread(() -> {
            if (mine == generation) refreshedSignature = null;
        });
    }

    /** A read of the PCs changed what is saved, and a failed load drew nothing: the next poll draws again, once. */
    private void redrawAtNextPoll(int mine) {
        runOnUiThread(() -> {
            if (mine == generation) drawn.forget();
        });
    }

    private static Set<String> onlineOf(List<ComputerDetails> hosts) {
        Set<String> online = new HashSet<>();
        for (ComputerDetails host : hosts) {
            if (host.state == ComputerDetails.State.ONLINE) online.add(host.uuid);
        }
        return online;
    }

    private void show(int mine, HomeModel next, boolean done) {
        runOnUiThread(() -> {
            if (mine != generation || isFinishing() || isDestroyed()) return;
            model = next;
            if (done) {
                loaded = true;
                loading.setVisibility(View.INVISIBLE);
            }
            // A filter the library no longer has (the last game of a store went) falls back to All once it has loaded.
            if (loaded && !next.filters.isEmpty() && filterOf(next, filter) == null) filter = HomeModel.FILTER_ALL;
            capsule.show(next);
            bindFilters();
            bindGrid(false);
            if (done) factFilter.loadFacts();
        });
    }

    static HomeModel.Filter filterOf(HomeModel model, String key) {
        for (HomeModel.Filter f : model.filters) {
            if (f.key.equals(key)) return f;
        }
        return null;
    }

    /** The cards a filter shows, in the order All games lists them. */
    static List<HomeModel.GameCard> cardsOf(HomeModel model, String filter) {
        return model.cardsUnder(filter);
    }

    /** The cards a filter shows whose titles have every word of the search, in Home's search's order; all without one. */
    static List<HomeModel.GameCard> cardsOf(HomeModel model, String filter, String query) {
        List<HomeModel.GameCard> cards = cardsOf(model, filter);
        return query.trim().isEmpty() ? cards : HomeSearch.matches(cards, query);
    }

    /** A chip's name: All, Installed, Games, the store's or collection's, Other apps, Not added or Hidden. */
    static CharSequence labelOf(Context context, HomeModel.Filter f) {
        if (f.storeName != null) return f.storeName;
        switch (f.key) {
            case HomeModel.FILTER_INSTALLED:
                return context.getString(R.string.console_filter_installed);
            case HomeModel.FILTER_NOT_INSTALLED:
                return context.getString(R.string.console_chip_not_installed);
            case HomeModel.FILTER_GAMES:
                return context.getString(R.string.console_row_games);
            case HomeModel.FILTER_OTHER:
                return context.getString(R.string.console_row_other);
            case HomeModel.FILTER_UNADDED:
                return context.getString(R.string.console_chip_not_added);
            case HomeModel.FILTER_NO_COLLECTION:
                return context.getString(R.string.console_row_no_collection);
            case HomeModel.FILTER_HIDDEN:
                return context.getString(R.string.console_filter_hidden);
            default:
                return context.getString(R.string.console_filter_all);
        }
    }

    /**
     * The chips, made again only when the library's sources change; when only a count changes the chips keep their
     * views, and focus, and just say the new count. The chosen one is selected.
     */
    private void bindFilters() {
        StringBuilder signature = new StringBuilder();
        for (HomeModel.Filter f : model.filters) signature.append(f.key).append(';');
        if (!signature.toString().equals(filterSignature)) {
            filterSignature = signature.toString();
            View focused = filterRow.findFocus();
            String focusedKey = focused == null ? null : (String) focused.getTag();
            filterRow.removeAllViews();
            LayoutInflater inflater = LayoutInflater.from(this);
            for (HomeModel.Filter f : model.filters) {
                MaterialButton chip = (MaterialButton) inflater.inflate(R.layout.view_filter_chip, filterRow, false);
                chip.setTag(f.key);
                String store = f.key.startsWith(HomeModel.STORE_FILTER)
                        ? f.key.substring(HomeModel.STORE_FILTER.length()) : null;
                int glyph = Stores.glyph(store);
                if (glyph != 0) chip.setIconResource(glyph);
                chip.setOnClickListener(v -> choose(f.key));
                filterRow.addView(chip);
                if (f.key.equals(focusedKey)) chip.requestFocus();
            }
        }
        for (int i = 0; i < filterRow.getChildCount() && i < model.filters.size(); i++) {
            HomeModel.Filter f = model.filters.get(i);
            MaterialButton chip = (MaterialButton) filterRow.getChildAt(i);
            CharSequence label = labelOf(this, f);
            chip.setText(chipText(label, f.count));
            chip.setContentDescription(getResources().getQuantityString(R.plurals.console_filter_description,
                    f.count, label, f.count));
        }
        for (int i = 0; i < filterRow.getChildCount(); i++) {
            View chip = filterRow.getChildAt(i);
            chip.setSelected(filter.equals(chip.getTag()));
        }
        factFilter.bindFactChips();
    }

    /** The name, then the count a shade lighter. */
    private static CharSequence chipText(CharSequence label, int count) {
        SpannableStringBuilder text = new SpannableStringBuilder(label).append("  ");
        int start = text.length();
        text.append(String.format(Locale.getDefault(), "%d", count));
        text.setSpan(new CharacterStyle() {
            @Override
            public void updateDrawState(TextPaint paint) {
                paint.setAlpha(paint.getAlpha() * 7 / 10);
            }
        }, start, text.length(), Spanned.SPAN_EXCLUSIVE_EXCLUSIVE);
        return text;
    }

    private void choose(String key) {
        if (key.equals(filter)) return;
        filter = key;
        bindFilters();
        bindGrid(true);
    }

    /** Shows the filter's cards; the focused card keeps focus across a refresh, a new filter starts at the top. */
    private void bindGrid(boolean newFilter) {
        // By key, not by the focused view's position: a bind that comes before the grid laid out the last one has
        // no position for it (the cached library, then a refresh that answers at once).
        String before = list.hasFocus() ? focusedKey : null;
        int at = focusedAt;
        String query = searchBar == null ? "" : searchBar.query();
        List<ShelfAdapter.Item> items = new ArrayList<>();
        for (HomeModel.GameCard card : factFilter.withFacts(cardsOf(model, filter, query))) {
            items.add(new ShelfAdapter.Item(card.key, card, null));
        }
        adapter.setItems(SHELF, items);
        boolean none = items.isEmpty();
        ((TextView) empty).setText(query.trim().isEmpty() ? R.string.console_all_games_empty : R.string.library_no_matches);
        empty.setVisibility(none && loaded ? View.VISIBLE : View.GONE);
        if (newFilter) {
            grid.scrollToPosition(0);
            showFocused(null);
        }
        if (before != null) {
            int target = adapter.positionOfKey(before);
            if (target < 0) target = Math.min(at, items.size() - 1);
            if (target >= 0) focusCard(target, 0);
            else focusSelectedChip();
            return;
        }
        if (focusPending && !list.isInTouchMode()) {
            if (!none) {
                focusPending = false;
                focusCard(0, 0);
            } else if (loaded) {
                focusPending = false;
                focusSelectedChip();
            }
        }
    }

    private void focusCard(int position, int attempt) {
        focusCard(position, attempt, ++focusTicket);
    }

    private void focusCard(int position, int attempt, int ticket) {
        list.post(() -> {
            if (ticket != focusTicket) return;
            RecyclerView.ViewHolder holder = list.findViewHolderForAdapterPosition(position);
            if (holder != null) {
                if (!holder.itemView.hasFocus()) holder.itemView.requestFocus();
                else showFocused(adapter.item(position).game);
            } else if (attempt < 3) {
                grid.scrollToPosition(position);
                focusCard(position, attempt + 1, ticket);
            }
        });
    }

    private void focusSelectedChip() {
        focusTicket++;
        for (int i = 0; i < filterRow.getChildCount(); i++) {
            View chip = filterRow.getChildAt(i);
            if (chip.isSelected()) {
                chip.requestFocus();
                return;
            }
        }
    }

    /** The line over the grid: the focused game's store, state and host, and its title. */
    private void showFocused(HomeModel.GameCard card) {
        focusMeta.setText(card == null ? "" : ShelfAdapter.CardHolder.metaOf(this, card));
        focusTitle.setText(card == null ? "" : card.title);
    }

    private void askFavourite(HomeModel.GameCard card) {
        if (repository == null || card.notAdded()) return;
        boolean remove = card.favourite;
        List<ConsoleSnapshot.Source> copies = model.copiesOf(card);
        boolean hide = !card.hidden();
        CharSequence star = getString(remove ? R.string.console_favourite_remove : R.string.console_favourite_add);
        CharSequence hiding = getString(hide ? R.string.applist_menu_hide_app : R.string.applist_menu_show_app);
        CharSequence openPlay = OpenPlay.menuLabel(this, card.hostUuid, card.appId,
                card.source != null ? card.source.stableId() : "");
        UiHelper.showDialog(this, new NavigationDialogBuilder(this)
                .setTitle(card.title)
                .setItems(copies.isEmpty() ? new CharSequence[]{star, openPlay}
                        : new CharSequence[]{star, openPlay, hiding}, (dialog, which) -> {
                    if (which == 0) {
                        setFavourite(card, !remove);
                    } else if (which == 1) {
                        OpenPlay.toggle(this, card.hostUuid, card.hostName, card.appId,
                                card.source != null ? card.source.stableId() : "", card.title);
                    } else {
                        setHidden(copies, hide);
                    }
                })
                .setNegativeButton(android.R.string.cancel, null).create());
    }

    private void setHidden(List<ConsoleSnapshot.Source> copies, boolean hide) {
        final ConsoleRepository repo = repository;
        if (repo == null) return;
        worker.execute(() -> {
            try {
                repo.setHidden(copies, hide);
            } catch (RuntimeException e) {
                LimeLog.warning("All games could not hide a game: " + e);
            }
            runOnUiThread(() -> {
                for (ConsoleSnapshot.Source copy : copies) {
                    FrontendSync.appListChanged(this, feed.host(copy.hostUuid));
                }
                scheduleReload();
            });
        });
    }

    private void setFavourite(HomeModel.GameCard card, boolean starred) {
        final ConsoleRepository repo = repository;
        if (repo == null || card.notAdded()) return;
        final String key = card.source != null ? card.source.stableId() : "";
        worker.execute(() -> {
            try {
                repo.setFavourite(card.hostUuid, card.hostName, key, card.appId, card.title, starred);
            } catch (RuntimeException e) {
                LimeLog.warning("All games could not update a favourite: " + e);
            }
            runOnUiThread(this::scheduleReload);
        });
    }

    /** For tests: the filter shown now. */
    String filter() {
        return filter;
    }

    /** For tests and the demo's captures: the chips' row. */
    ViewGroup filterRow() {
        return filterRow;
    }
}
