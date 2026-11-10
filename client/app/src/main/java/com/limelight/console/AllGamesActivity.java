package com.limelight.console;

import android.content.res.Configuration;
import android.graphics.drawable.ColorDrawable;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
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
import com.limelight.R;
import com.limelight.nativeapi.ConsoleRepository;
import com.limelight.nativeapi.ConsoleSnapshot;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.ui.AppTheme;
import com.limelight.ui.NightjarActivity;
import com.limelight.ui.ShellBar;
import com.limelight.ui.TopAppBar;
import com.limelight.utils.UiHelper;

import java.util.ArrayList;
import java.util.HashSet;
import java.util.List;
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
    /** The narrowest a poster's card gets before the grid drops a column. */
    private static final int MIN_CARD_WIDTH_DP = 120;
    /** How far a focused TV poster's glow reaches past its 24 dp bleed: the grid's frame keeps that much room. */
    static final int GLOW_ROOM_DP = 40;
    /** A TV's safe edge, the usual overscan margin: this share of the screen's height in from its bottom. */
    static final float SAFE_EDGE = 0.05f;
    /** The room a focused TV row keeps between its glow and the frame's bottom. */
    static final int FIT_ROOM_DP = 4;
    private static final long DEBOUNCE_MS = 250;
    private static final String SHELF = "all-games";

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
    /** On a TV, the room above the first row inside the grid's frame, which a focused row keeps for its glow; 0 elsewhere. */
    private int roomAbove;
    private RecyclerView list;
    private LinearLayout filterRow;
    private View loading;
    private View empty;
    private TextView focusMeta;
    private TextView focusTitle;
    private volatile ConsoleRepository repository;
    private CoverLoaders covers;
    private HomeModel model = HomeModel.empty();
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
    private int lastWidthPx;
    private int lastHeightPx;
    /** Each column's width; on a TV a poster shrunk to fit the frame's height is narrower, the slack spread between. */
    private int slotPx;
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
        @Override public android.app.Activity activity() { return AllGamesActivity.this; }
        @Override public ConsoleRepository repository() { return repository; }
        @Override public ComputerDetails host(String uuid) { return feed.host(uuid); }
        @Override public Class<? extends GameDetailActivity> detailActivity() { return AllGamesActivity.this.detailActivity(); }
        @Override public Set<String> adding() { return adding; }
        @Override public void onAdded() {
            refreshedSignature = null;
            scheduleReload();
        }
    };

    /** Its chrome follows the form: a rotation into another form lays it out again. */
    @Override
    protected boolean followsForm() {
        return true;
    }

    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        String requested = state != null ? state.getString(EXTRA_FILTER) : getIntent().getStringExtra(EXTRA_FILTER);
        if (requested != null) filter = requested;
        setContentView(R.layout.activity_all_games);
        UiHelper.notifyNewRootView(this);

        TopAppBar.configure(this, R.id.all_games_toolbar, getString(R.string.console_all_games_title), 0);
        // All games belongs to Home: Home in the bar is the page it came from, so it closes this one.
        ShellBar.install(this, ShellBar.Destination.HOME);
        findViewById(R.id.shell_home).setOnClickListener(v -> finish());
        if (isTelevision()) {
            getWindow().setBackgroundDrawable(new ColorDrawable(AppTheme.tvCanvas(this)));
            // The sky of Home, PCs and Settings; the grid fills the screen, so no moon behind the posters.
            com.limelight.ui.NightSkyView sky = findViewById(R.id.all_games_sky);
            sky.setMoonShown(false);
            sky.setVisibility(View.VISIBLE);
            // Edge to edge, not inset by the TV margin as a lighter frame.
            sky.bleedPastPadding();
            // Rows scrolling up fade out under the heading and filters instead of stopping on a hard edge.
            View grid = findViewById(R.id.all_games_grid);
            grid.setVerticalFadingEdgeEnabled(true);
            // The frame clips the grid. A focused poster's glow reaches past its 24 dp bleed and the row's top padding, and
            // was cut square there: the frame stands further out at the top and sides, and the list pads by as much. The
            // room above goes to the first row, not the padding: the fade starts at the padding's edge, and rows scrolled
            // into an unfaded strip would stand under the focused title.
            int room = Math.round(GLOW_ROOM_DP * getResources().getDisplayMetrics().density);
            android.view.ViewGroup.MarginLayoutParams frame = (android.view.ViewGroup.MarginLayoutParams)
                    ((View) grid.getParent()).getLayoutParams();
            frame.topMargin -= room;
            // That room lies over the focus line and filters: a touch there is theirs, not the grid's.
            ((GridFrame) grid.getParent()).passTouchesAbove(room);
            frame.setMarginStart(frame.getMarginStart() - room);
            frame.setMarginEnd(frame.getMarginEnd() - room);
            int above = grid.getPaddingTop() + room;
            roomAbove = above;
            // The last row can come up to the place every focused row takes: as much room below it as above (fitColumns).
            int fitRoom = Math.round(FIT_ROOM_DP * getResources().getDisplayMetrics().density);
            grid.setPaddingRelative(grid.getPaddingStart() + room, 0, grid.getPaddingEnd() + room, above + fitRoom);
            ((RecyclerView) grid).addItemDecoration(new RecyclerView.ItemDecoration() {
                @Override
                public void getItemOffsets(@NonNull android.graphics.Rect out, @NonNull View card, @NonNull RecyclerView parent,
                                           @NonNull RecyclerView.State state) {
                    int at = parent.getChildAdapterPosition(card);
                    int columns = AllGamesActivity.this.grid.getSpanCount();
                    out.top = at != RecyclerView.NO_POSITION && at < columns ? above : 0;
                    // Posters narrower than their columns keep the first at the start and the last at the end, the gaps even.
                    int slack = Math.max(0, slotPx - env.cardWidthPx);
                    if (at == RecyclerView.NO_POSITION || slack == 0 || columns < 2) return;
                    int start = at % columns * slack / (columns - 1);
                    boolean rtl = parent.getLayoutDirection() == View.LAYOUT_DIRECTION_RTL;
                    out.left = rtl ? slack - start : start;
                    out.right = rtl ? start : slack - start;
                }
            });
            grid.setFadingEdgeLength(getResources().getDimensionPixelSize(R.dimen.all_games_fade) + room);
            // The frame ends at the TV's safe edge, so a focused row's ring and glow fit whole. As the page's weighted last
            // child it ended the glow room short of the page's margin: a LinearLayout gives a negative margin no space.
            View gridFrame = (View) grid.getParent();
            gridFrame.addOnLayoutChangeListener((v, l, t, r, b, ol, ot, or, ob) -> {
                if (t != ot || b != ob) v.post(() -> reachSafeEdge(v));
            });
            // A handheld in the forced TV layout rotates without being made again: the screen's height changes, the
            // frame's own place may not.
            findViewById(R.id.all_games_page).addOnLayoutChangeListener((v, l, t, r, b, ol, ot, or, ob) -> {
                if (r - l != or - ol || b - t != ob - ot) v.post(() -> reachSafeEdge(gridFrame));
            });
        }

        loading = findViewById(R.id.all_games_loading);
        empty = findViewById(R.id.all_games_empty);
        filterRow = findViewById(R.id.all_games_filters);
        focusMeta = findViewById(R.id.all_games_focus_meta);
        focusTitle = findViewById(R.id.all_games_focus_title);
        list = findViewById(R.id.all_games_grid);
        if (!isTelevision()) alignToGutter();

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
                                                         @NonNull android.graphics.Rect rect, boolean immediate,
                                                         boolean focusedChildVisible) {
                android.graphics.Rect withRoom = new android.graphics.Rect(rect);
                if (roomAbove > 0) withRoom.top = Math.min(rect.top, 0) - roomAbove;
                return super.requestChildRectangleOnScreen(parent, child, withRoom, immediate, focusedChildVisible);
            }
        };
        list.setLayoutManager(grid);
        list.setItemAnimator(null);
        list.setAdapter(adapter);
        list.addOnLayoutChangeListener((v, l, t, r, b, ol, ot, or, ob) -> fitColumns(r - l, b - t));
        feed.bind();
    }

    /**
     * The layout's insets are the TV's, inside its overscan margin. Elsewhere the title, the filters and the poster grid
     * line up with the shell bar on the screen gutter, and the focus line goes: a phone's posters carry their own titles.
     */
    private void alignToGutter() {
        int gutter = getResources().getDimensionPixelSize(R.dimen.screen_gutter);
        for (int id : new int[] {R.id.all_games_title, R.id.all_games_filters_scroll}) {
            View view = findViewById(id);
            view.setPaddingRelative(gutter, view.getPaddingTop(), gutter, view.getPaddingBottom());
        }
        findViewById(R.id.all_games_focus).setVisibility(View.GONE);
        // The grid's frame reaches 24 dp past the content on both sides; each phone card keeps its cover inset as margin.
        int bleed = Math.round(24 * getResources().getDisplayMetrics().density);
        int inset = getResources().getDimensionPixelSize(R.dimen.cover_inset);
        int side = bleed + gutter - inset;
        list.setPaddingRelative(side, list.getPaddingTop(), side, list.getPaddingBottom());
    }

    private boolean isTelevision() {
        return (getResources().getConfiguration().uiMode & Configuration.UI_MODE_TYPE_MASK)
                == Configuration.UI_MODE_TYPE_TELEVISION;
    }

    /** As many posters as fit at their narrowest, at most six: six across a TV's screen. */
    static int columnsFor(int contentWidthPx, int minCardWidthPx) {
        return Math.max(2, Math.min(MAX_COLUMNS, contentWidthPx / Math.max(1, minCardWidthPx)));
    }

    /**
     * Each poster fills its column, so the grid spans the screen's width whatever it is. On a TV a poster is at most as
     * tall as lets a focused row's ring and glow fit the frame; one shrunk for that is narrower than its column.
     */
    private void fitColumns(int widthPx, int heightPx) {
        if (widthPx <= 0 || (widthPx == lastWidthPx && heightPx == lastHeightPx)) return;
        lastWidthPx = widthPx;
        lastHeightPx = heightPx;
        int content = widthPx - list.getPaddingLeft() - list.getPaddingRight();
        int count = columnsFor(content, Math.round(MIN_CARD_WIDTH_DP * getResources().getDisplayMetrics().density));
        int slot = content / count;
        int cover = ShelfAdapter.coverHeight(this, isTelevision(), slot);
        int card = slot;
        int tallest = roomAbove > 0 ? tallestCover(heightPx) : 0;
        if (tallest > 0 && cover > tallest) {
            int poster = tallest * 2 / 3;
            cover = poster * 3 / 2;
            card = poster + 2 * getResources().getDimensionPixelSize(R.dimen.cover_inset);
        }
        if (count == grid.getSpanCount() && slot == slotPx && card == env.cardWidthPx && cover == env.coverHeightPx) return;
        int width = card;
        int height = cover;
        list.post(() -> {
            slotPx = slot;
            env.cardWidthPx = width;
            env.coverHeightPx = height;
            grid.setSpanCount(count);
            adapter.notifyDataSetChanged();
            list.invalidateItemDecorations();
        });
    }

    /**
     * The tallest TV poster whose focused ring and glow fit the frame of the given height. A focused poster grows about its
     * middle, so they reach as far below it as above; above, the first row's room holds them, so the frame keeps as much
     * below the poster, and a little room.
     */
    private int tallestCover(int frameHeightPx) {
        int inset = getResources().getDimensionPixelSize(R.dimen.cover_inset);
        int fitRoom = Math.round(FIT_ROOM_DP * getResources().getDisplayMetrics().density);
        return frameHeightPx - 2 * (roomAbove + inset) - fitRoom;
    }

    /** Gives the grid's frame the height that ends it at the TV's safe edge. */
    private static void reachSafeEdge(View frame) {
        int screen = frame.getRootView().getHeight();
        if (screen <= 0) return;
        int[] at = new int[2];
        frame.getLocationInWindow(at);
        int height = screen - Math.round(screen * SAFE_EDGE) - at[1];
        LinearLayout.LayoutParams params = (LinearLayout.LayoutParams) frame.getLayoutParams();
        if (height <= 0 || (params.height == height && params.weight == 0)) return;
        params.height = height;
        params.weight = 0;
        frame.setLayoutParams(params);
    }

    @Override
    protected void onSaveInstanceState(@NonNull Bundle out) {
        super.onSaveInstanceState(out);
        out.putString(EXTRA_FILTER, filter);
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
        if (refresh) loading.setVisibility(View.VISIBLE);
        worker.execute(() -> {
            try {
                show(mine, HomeModel.of(repo.cached(hosts), online, HomeModel.launchDenied(hosts)), !refresh);
                if (refresh) {
                    ConsoleSnapshot fresh = repo.refresh(hosts);
                    show(mine, HomeModel.of(fresh, online, HomeModel.launchDenied(hosts)), true);
                    redrawAtNextPoll(mine);
                }
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
            } catch (RuntimeException e) {
                com.limelight.LimeLog.warning("All games could not load: " + e);
                show(mine, HomeModel.empty(), true);
                redrawAtNextPoll(mine);
            }
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
            bindFilters();
            bindGrid(false);
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

    /** A chip's name: All, Installed, Games, the store's or collection's, Other apps, Not added or Hidden. */
    static CharSequence labelOf(android.content.Context context, HomeModel.Filter f) {
        if (f.storeName != null) return f.storeName;
        switch (f.key) {
            case HomeModel.FILTER_INSTALLED:
                return context.getString(R.string.console_filter_installed);
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
    }

    /** The name, then the count a shade lighter. */
    private static CharSequence chipText(CharSequence label, int count) {
        SpannableStringBuilder text = new SpannableStringBuilder(label).append("  ");
        int start = text.length();
        text.append(String.format(java.util.Locale.getDefault(), "%d", count));
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
        List<ShelfAdapter.Item> items = new ArrayList<>();
        for (HomeModel.GameCard card : cardsOf(model, filter)) items.add(new ShelfAdapter.Item(card.key, card, null));
        adapter.setItems(SHELF, items);
        boolean none = items.isEmpty();
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
        CharSequence openPlay = OpenPlay.menuLabel(this, card.hostUuid, card.appId);
        UiHelper.showDialog(this, new com.limelight.ui.NavigationDialogBuilder(this)
                .setTitle(card.title)
                .setItems(copies.isEmpty() ? new CharSequence[]{star, openPlay}
                        : new CharSequence[]{star, openPlay, hiding}, (dialog, which) -> {
                    if (which == 0) {
                        setFavourite(card, !remove);
                    } else if (which == 1) {
                        OpenPlay.toggle(this, card.hostUuid, card.hostName, card.appId, card.source != null ? card.source.stableId() : "", card.title);
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
                com.limelight.LimeLog.warning("All games could not hide a game: " + e);
            }
            runOnUiThread(() -> {
                for (ConsoleSnapshot.Source copy : copies) {
                    com.limelight.utils.FrontendSync.appListChanged(this, feed.host(copy.hostUuid));
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
                com.limelight.LimeLog.warning("All games could not update a favourite: " + e);
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
