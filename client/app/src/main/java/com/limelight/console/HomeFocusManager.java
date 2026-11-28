package com.limelight.console;

import android.app.Activity;
import android.content.res.Configuration;
import android.graphics.drawable.ColorDrawable;
import android.view.View;
import android.view.ViewGroup;
import android.view.ViewParent;

import androidx.recyclerview.widget.GridLayoutManager;
import androidx.recyclerview.widget.LinearLayoutManager;
import androidx.recyclerview.widget.RecyclerView;

import com.google.android.material.color.MaterialColors;
import com.limelight.R;
import com.limelight.ui.AppTheme;
import com.limelight.ui.BackdropView;
import com.limelight.ui.InputSource;
import com.limelight.ui.Motion;
import com.limelight.ui.ShellBar;
import com.limelight.ui.StreamMenuHint;

import java.util.function.BooleanSupplier;

/**
 * List surface, ambience wiring, column sizing and D-pad focus preservation for {@link ConsoleHomeActivity}.
 * Reads the activity's live adapter, shelves, ambience and focus state on every call so test hooks stay in sync.
 */
final class HomeFocusManager {
    private static final int FOCUS_TRIES = 8;

    interface Host {
        Activity activity();
        RecyclerView list();
        GridLayoutManager grid();
        void setGrid(GridLayoutManager grid);
        ConsoleHomeListAdapter adapter();
        void setAdapter(ConsoleHomeListAdapter adapter);
        ConsoleHomeAdapter shelves();
        void setShelves(ConsoleHomeAdapter shelves);
        HomeAmbience ambience();
        void setAmbience(HomeAmbience ambience);
        HomeHints hints();
        void setHints(HomeHints hints);
        HomeChrome chrome();
        HomeView view();
        HostFeed feed();
        CoverLoaders covers();
        boolean isTelevision();
        boolean isFocusPending();
        void setFocusPending(boolean pending);
        boolean isAtStart();
        void onInput(InputSource source, int vendor);
    }

    private final Host host;
    private int columns = 2;
    private int lastWidthPx;

    HomeFocusManager(Host host) {
        this.host = host;
    }

    void resetWidth() {
        lastWidthPx = 0;
    }

    /** Headers take the whole row; wide cards take two columns, or the whole row when it has at most three (and pair up on TV). */
    static int spanOf(int viewType, int columns, boolean television) {
        switch (viewType) {
            case ConsoleHomeListAdapter.HEADER:
            case ConsoleHomeListAdapter.LIST_GAME:
                return columns;
            case ConsoleHomeListAdapter.COVER_GAME:
                return 1;
            default:
                if (columns <= 3) return columns;
                return television ? Math.max(2, columns / 2) : 2;
        }
    }

    /** Columns for a list this wide at the chosen size: about 130 dp each at medium, even from four on. */
    static int columnsFor(int widthDp, HomeView.Size size) {
        int columns = Math.max(2, widthDp / Math.round(130 * size.scale));
        return columns >= 4 && columns % 2 == 1 ? columns - 1 : columns;
    }

    /** TV posters across a grid this wide: as many as fit at the shelves' width times the size, at least two. */
    static int tvColumnsFor(int contentPx, int cardPx) {
        int columns = Math.max(2, contentPx / Math.max(1, cardPx));
        // Even from four on, as on a tablet, so wide cards and PCs fill their rows.
        return columns >= 4 && columns % 2 == 1 ? columns - 1 : columns;
    }

    /** Wires Home's in-page search and search buttons, falling back to All games when the layout has no field. */
    HomeSearch setUpSearch(androidx.activity.ComponentActivity activity, ShelfAdapter.Env searchEnv,
                           java.util.function.Supplier<HomeModel> model, Runnable openAllGames) {
        HomeSearch search = HomeSearch.of(activity, searchEnv, model, () -> {
            if (host.hints() != null) host.hints().onFocus(null, null);
            host.chrome().fitSetupCard();
        }, () -> {
            host.chrome().fitSetupCard();
            if (host.chrome().form() == com.limelight.ui.GlassForm.PHONE) {
                activity.findViewById(R.id.home_bottom_row).setVisibility(View.VISIBLE);
                activity.findViewById(R.id.home_foot_fade).setVisibility(View.VISIBLE);
            }
        });
        host.chrome().setSearch(search);
        View.OnClickListener open = v -> {
            if (search != null) search.open(v);
            else openAllGames.run();
        };
        for (int id : new int[] {R.id.home_search, R.id.home_search_top}) {
            View button = activity.findViewById(id);
            if (button != null) button.setOnClickListener(open);
        }
        return search;
    }

    void setUpGrid(ConsoleHomeAdapter.Listener listener) {
        Activity activity = host.activity();
        RecyclerView list = host.list();
        GridLayoutManager grid = new HomeGridLayoutManager(activity, columns, position -> {
            int first = host.adapter().firstCard();
            return first >= 0 && first < position;
        }, position -> position < host.adapter().getItemCount()
                && host.adapter().viewTypeAt(position) == ConsoleHomeListAdapter.HEADER,
                position -> host.adapter().keyAt(position), key -> host.adapter().positionOfKey(key));
        grid.setSpanSizeLookup(new GridLayoutManager.SpanSizeLookup() {
            @Override
            public int getSpanSize(int position) {
                return ConsoleHomeActivity.spanOf(host.adapter().viewTypeAt(position), columns, host.isTelevision());
            }
        });
        host.setGrid(grid);
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
        ConsoleHomeListAdapter adapter = new ConsoleHomeListAdapter(
                activity, null, host.feed()::host, listener, host.isTelevision());
        adapter.setListLayout(host.view().layout == HomeView.Layout.LIST);
        host.setAdapter(adapter);
        list.setAdapter(adapter);
    }

    /** The shelves: a vertical list of rows that snaps the focused row to the top and returns to each row's last card. */
    void setUpShelves(ConsoleHomeAdapter.Listener listener) {
        Activity activity = host.activity();
        RecyclerView list = host.list();
        ConsoleHomeAdapter shelves = new ConsoleHomeAdapter(activity, null, host.feed()::host, listener);
        host.setShelves(shelves);
        ShelfLayoutManager rows = new ShelfLayoutManager(activity, LinearLayoutManager.VERTICAL,
                () -> host.shelves().isFastTravel() || Motion.reduced(activity));
        rows.setRowFocus(shelves);
        shelves.setAboveHero(from -> ShellBar.nearestInTopRow(activity, from,
                R.id.home_pc, R.id.console_view, R.id.home_search, R.id.home_search_top));
        list.setLayoutManager(rows);
        list.setItemAnimator(null);
        list.setItemViewCacheSize(2);
        shelves.setGroup(host.view().group);
        shelves.setCardScale(host.view().size.scale);
        shelves.setTelevision(host.isTelevision());
        shelves.setHeroes(!host.isTelevision());
        list.setAdapter(shelves);
    }

    void setUpAmbience() {
        Activity activity = host.activity();
        RecyclerView list = host.list();
        BackdropView backdrop = activity.findViewById(R.id.home_backdrop);
        View hero = activity.findViewById(R.id.home_hero);
        // It fades at each change of game; its lines do not overlap, so that needs no offscreen layer.
        if (hero != null) hero.forceHasOverlappingRendering(false);
        int canvas = host.isTelevision() ? AppTheme.tvCanvas(activity)
                : MaterialColors.getColor(backdrop, R.attr.glassCanvas);
        host.chrome().setUpBackdrop(backdrop, canvas);
        // The window shows the canvas until the first backdrop covers it, then stops painting under it.
        activity.getWindow().setBackgroundDrawable(new ColorDrawable(canvas));
        // The TV's safe-area padding sits on the content view, which would otherwise clip the backdrop that reaches past it.
        ((ViewGroup) activity.findViewById(android.R.id.content)).setClipChildren(false);
        backdrop.setCanvasColor(canvas);
        backdrop.setDim(AppTheme.isHighContrast(activity) ? 0.85f : 1f);
        backdrop.setOnCovered(() -> activity.getWindow().setBackgroundDrawable(null));
        BooleanSupplier fast = () -> host.shelves() != null
                ? host.shelves().isFastTravel() : host.adapter().isFastTravel();
        HomeAmbience ambience = new HomeAmbience(activity, backdrop, hero, host.feed()::host, host::covers, fast);
        host.setAmbience(ambience);
        ambience.setStage(new HomeHeroStage(activity, backdrop, list, host.chrome().form(), canvas, host.feed()::host, host::covers));
        InputSource start = InputSource.last() != null ? InputSource.last() : host.isTelevision()
                ? (StreamMenuHint.hasGamepad() ? InputSource.PAD : InputSource.REMOTE) : InputSource.NONE;
        ambience.setInput(start, 0);
        HomeHints hints = new HomeHints(activity, activity.findViewById(R.id.home_hints), start);
        host.setHints(hints);
        ShelfAdapter.FocusListener focus = (shelf, item, card) -> {
            hints.onFocus(HomeAmbience.goWord(activity, item.host == null),
                    item.all != null ? null : activity.getString(R.string.console_hint_options));
            // The hero's own lines leave the ambient on the hero's game.
            if (!HomeHero.SHELF.equals(shelf)) ambience.onCardFocus(shelf, item, card);
        };
        if (host.shelves() != null) {
            host.shelves().setFocusListener(focus);
        } else {
            host.adapter().setFocusListener(focus);
        }
        list.getViewTreeObserver().addOnTouchModeChangeListener(touch -> {
            if (touch || hints.source() != InputSource.NONE) return;
            host.onInput(InputSource.leavingTouch(StreamMenuHint.hasGamepad(),
                    activity.getResources().getConfiguration().keyboard == Configuration.KEYBOARD_QWERTY), 0);
        });
        // Focus on the chrome has nothing for the hints to say.
        list.getViewTreeObserver().addOnGlobalFocusChangeListener((before, now) -> {
            if (now == null || !isInList(list, now)) hints.onFocus(null, null);
        });
    }

    private static boolean isInList(RecyclerView list, View view) {
        for (ViewParent up = view.getParent(); up != null; up = up.getParent()) {
            if (up == list) return true;
        }
        return false;
    }

    private void fitColumns(int widthPx) {
        if (widthPx <= 0 || widthPx == lastWidthPx) {
            return;
        }
        lastWidthPx = widthPx;
        Activity activity = host.activity();
        RecyclerView list = host.list();
        int content = widthPx - list.getPaddingLeft() - list.getPaddingRight();
        if (host.isTelevision()) {
            int card = Math.round(activity.getResources().getDimensionPixelSize(R.dimen.shelf_card_width)
                    * host.view().size.scale);
            int count = ConsoleHomeActivity.tvColumnsFor(content, card);
            // Each poster fills its column, so the grid spans the screen.
            int columnWidth = content / count;
            list.post(() -> {
                if (count != columns) {
                    columns = count;
                    host.grid().setSpanCount(count);
                }
                host.adapter().setCardWidth(columnWidth);
                host.adapter().setCoverHeight(ShelfAdapter.coverHeight(activity, true, columnWidth));
            });
            return;
        }
        float density = activity.getResources().getDisplayMetrics().density;
        int count = ConsoleHomeActivity.columnsFor(Math.round(widthPx / density), host.view().size);
        // A card keeps its margin around it.
        int margin = activity.getResources().getDimensionPixelSize(R.dimen.cover_inset);
        int columnWidth = content / count - 2 * margin;
        list.post(() -> {
            if (count != columns) {
                columns = count;
                host.grid().setSpanCount(count);
            }
            host.adapter().setCoverHeight(Math.max(1, columnWidth * 4 / 3));
        });
    }

    /** With a D-pad or controller the first card has focus from the start, so a press opens something. */
    void focusFirstCard(boolean emptyShown) {
        if (!host.isFocusPending() && !host.isAtStart()) {
            return;
        }
        if (emptyShown) {
            host.setFocusPending(false);
            host.activity().findViewById(R.id.console_empty_hosts).requestFocus();
            return;
        }
        RecyclerView list = host.list();
        HomeAmbience ambience = host.ambience();
        ConsoleHomeAdapter shelves = host.shelves();
        if (shelves == null) {
            ConsoleHomeListAdapter adapter = host.adapter();
            int first = adapter.initialFocus();
            if (ambience != null && first < 0) {
                ambience.clear();
            }
            if (first < 0 || list.isInTouchMode()) {
                // TV: no card takes focus until a key is pressed, but the hero shows the one focus would start on.
                if (ambience != null && first >= 0 && list.getFocusedChild() == null) {
                    ambience.refresh(adapter.itemAt(first));
                }
                return;
            }
            host.setFocusPending(false);
            list.post(() -> {
                // Asked again here: the hosts' answer may have changed the list since.
                int now = host.adapter().initialFocus();
                if (now < 0) return;
                RecyclerView.ViewHolder holder = list.findViewHolderForAdapterPosition(now);
                if (holder != null) {
                    holder.itemView.requestFocus();
                } else {
                    // A TV brings the card in under its section's title where that leaves the card on screen.
                    revealCard(now);
                    host.setFocusPending(true);
                }
            });
            return;
        }
        ShelfAdapter.Item hero = shelves.heroItem();
        if (hero != null) {
            // The hero's game holds the ambient until focus goes elsewhere, and a key starts on its Play.
            if (ambience != null && shelves.focusedCard(list) == null) ambience.refresh(hero);
            if (list.isInTouchMode()) return;
            host.setFocusPending(false);
            list.post(() -> {
                View play = host.shelves().heroPlay(list);
                if (play != null) play.requestFocus();
                else host.setFocusPending(true);
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
        host.setFocusPending(false);
        restoreFocus(first, 0);
    }

    /** Moves D-pad Down focus from the TV/handheld top bar directly into the pairing banner, setup/empty action, hero Play, or first app card. */
    boolean focusDownFromTopRow(View pairingBanner, View setupCard, View empty) {
        if (pairingBanner != null && pairingBanner.getVisibility() == View.VISIBLE
                && pairingBanner.isFocusable() && pairingBanner.requestFocus()) {
            return true;
        }
        if (setupCard != null && setupCard.getVisibility() == View.VISIBLE) {
            View close = setupCard.findViewById(R.id.home_setup_dismiss);
            if (close != null && close.isFocusable() && close.requestFocus()) return true;
        }
        if (empty != null && empty.getVisibility() == View.VISIBLE) {
            View hosts = host.activity().findViewById(R.id.console_empty_hosts);
            if (hosts != null && hosts.requestFocus()) return true;
        }
        RecyclerView list = host.list();
        if (list == null || list.getVisibility() != View.VISIBLE) return false;
        ConsoleHomeAdapter shelves = host.shelves();
        if (shelves != null) {
            if (shelves.heroItem() != null) {
                View play = shelves.heroPlay(list);
                if (play != null && play.requestFocus()) return true;
                list.scrollToPosition(0);
                list.post(() -> {
                    ConsoleHomeAdapter current = host.shelves();
                    if (current == null || current.heroItem() == null) return;
                    View p = current.heroPlay(list);
                    if (p != null) p.requestFocus();
                });
                return true;
            }
            ConsoleHomeAdapter.Cursor first = shelves.initialFocus();
            if (first != null) {
                View card = shelves.cardView(list, first);
                if (card != null && card.requestFocus()) return true;
                String cardKey = shelves.keyAt(first);
                if (cardKey != null) {
                    shelves.reveal(list, first);
                    restoreFocus(shelves.rowKey(first.row), cardKey, 0);
                    return true;
                }
            }
        } else {
            ConsoleHomeListAdapter adapter = host.adapter();
            int first = adapter != null ? adapter.initialFocus() : -1;
            if (first >= 0) {
                RecyclerView.ViewHolder holder = list.findViewHolderForAdapterPosition(first);
                if (holder != null && holder.itemView.requestFocus()) return true;
                revealCard(first);
                list.post(() -> {
                    RecyclerView.ViewHolder h = list.findViewHolderForAdapterPosition(first);
                    if (h != null) h.itemView.requestFocus();
                });
                return true;
            }
        }
        return false;
    }

    /**
     * Focuses the card once its shelf and it are laid out; a card out of range is brought in and asked for again. A place on the
     * list means nothing once the list has changed, so the card goes by its keys, and at the start it is whatever starts now.
     */
    void restoreFocus(ConsoleHomeAdapter.Cursor target, int attempt) {
        ConsoleHomeAdapter shelves = host.shelves();
        restoreFocus(shelves.rowKey(target.row), shelves.keyAt(target), attempt);
    }

    private void restoreFocus(String shelf, String key, int attempt) {
        RecyclerView list = host.list();
        list.post(() -> {
            ConsoleHomeAdapter shelves = host.shelves();
            ConsoleHomeAdapter.Cursor target = host.isAtStart() ? shelves.initialFocus() : shelves.positionOfKey(shelf, key);
            if (target == null || attempt >= FOCUS_TRIES) return;
            if (list.hasPendingAdapterUpdates()) {
                // The list is about to lay out what it was just given: a frame from now it knows its cards.
                list.postOnAnimation(() -> restoreFocus(shelf, key, attempt + 1));
                return;
            }
            View card = shelves.cardView(list, target);
            if (card != null) {
                if (!card.hasFocus()) {
                    card.requestFocus();
                } else if (host.ambience() != null) {
                    // The card kept focus through the refresh: its state may have changed, so the hero is written again.
                    host.ambience().refresh(shelves.itemAt(target));
                }
            } else {
                shelves.reveal(list, target);
                restoreFocus(shelf, key, attempt + 1);
            }
        });
    }

    /** Submits the model; when a card had focus, the same card (by its stable id) has it afterwards. */
    void submitKeepingFocus(HomeModel model) {
        ConsoleHomeAdapter shelves = host.shelves();
        if (shelves == null) {
            submitGridKeepingFocus(model);
            return;
        }
        RecyclerView list = host.list();
        if (host.isAtStart()) {
            // Nobody chose anything yet: the shelves start over, and focusFirstCard places the start.
            shelves.forgetScroll();
            shelves.submit(model);
            list.scrollToPosition(0);
            return;
        }
        ConsoleHomeAdapter.Cursor at = shelves.focusedCard(list);
        String key = shelves.keyAt(at);
        String shelf = at == null ? null : shelves.rowKey(at.row);
        shelves.submit(model);
        if (key == null) {
            return;
        }
        // A card that left (an unstarred favourite) hands focus to its neighbour instead of dropping it.
        ConsoleHomeAdapter.Cursor target = shelves.positionOfKey(shelf, key);
        if (target == null) {
            target = shelves.nearestCard(at);
        }
        if (target == null) {
            return;
        }
        restoreFocus(target, 0);
    }

    private void submitGridKeepingFocus(HomeModel model) {
        RecyclerView list = host.list();
        ConsoleHomeListAdapter adapter = host.adapter();
        View focused = list.getFocusedChild();
        int at = focused == null ? -1 : list.getChildAdapterPosition(focused);
        String key = at < 0 ? null : adapter.keyAt(at);
        adapter.submit(model, host.view().group);
        if (key == null) {
            return;
        }
        final String focusKey = key;
        final int focusAt = at;
        list.post(() -> {
            int position = resolveGridPosition(adapter, focusKey, focusAt);
            if (position < 0) {
                return;
            }
            RecyclerView.ViewHolder holder = list.findViewHolderForAdapterPosition(position);
            if (holder != null) {
                if (!holder.itemView.hasFocus()) {
                    holder.itemView.requestFocus();
                }
            } else {
                revealCard(position);
                list.post(() -> {
                    int latest = resolveGridPosition(adapter, focusKey, focusAt);
                    if (latest < 0) {
                        return;
                    }
                    RecyclerView.ViewHolder later = list.findViewHolderForAdapterPosition(latest);
                    if (later != null) {
                        later.itemView.requestFocus();
                    }
                });
            }
        });
    }

    private static int resolveGridPosition(ConsoleHomeListAdapter adapter, String key, int fallbackAt) {
        int count = adapter.getItemCount();
        if (count == 0) {
            return -1;
        }
        int position = adapter.positionOfKey(key);
        if (position < 0) {
            position = adapter.nearestCard(fallbackAt);
        }
        return position >= 0 && position < count ? position : -1;
    }

    /**
     * Brings the card at {@code position} in so it can take focus. On a TV its section's title shows above it while
     * that leaves the card on screen; a card further down a long section comes in itself under an empty title's
     * height, since from the title it would lie past the screen and never be laid out to take focus.
     */
    private void revealCard(int position) {
        if (position < 0 || position >= host.adapter().getItemCount()) {
            return;
        }
        ConsoleHomeListAdapter.Reveal reveal = revealFor(position);
        host.grid().scrollToPositionWithOffset(reveal.position, reveal.offset);
    }

    /** Where {@link #revealCard} starts the list for the card at {@code position}, from what the list has laid out. */
    ConsoleHomeListAdapter.Reveal revealFor(int position) {
        ConsoleHomeListAdapter adapter = host.adapter();
        if (position < 0 || position >= adapter.getItemCount()) {
            return new ConsoleHomeListAdapter.Reveal(0, 0);
        }
        GridLayoutManager grid = host.grid();
        RecyclerView list = host.list();
        int heading = adapter.sectionStart(position);
        int spanCount = grid.getSpanCount();
        int rows = heading < 0
                ? 0
                : grid.getSpanSizeLookup().getSpanGroupIndex(position, spanCount)
                        - grid.getSpanSizeLookup().getSpanGroupIndex(heading, spanCount);
        int viewport = list.getHeight() - list.getPaddingTop() - list.getPaddingBottom();
        return adapter.reveal(position, rows, headingHeight(), cardRowHeight(position), viewport, host.isTelevision());
    }

    /** The height a section heading takes: a laid-out one's, else the TV title's share of the list. */
    private int headingHeight() {
        RecyclerView list = host.list();
        for (int i = 0; i < list.getChildCount(); i++) {
            View child = list.getChildAt(i);
            if (list.getChildViewHolder(child).getItemViewType() == ConsoleHomeListAdapter.HEADER
                    && child.getHeight() > 0) {
                return child.getHeight();
            }
        }
        return host.activity().getResources().getDimensionPixelSize(R.dimen.home_head_tv);
    }

    /** The height a row of cards like the one at {@code position} takes, from a laid-out one; 0 when none is. */
    private int cardRowHeight(int position) {
        RecyclerView list = host.list();
        int type = host.adapter().viewTypeAt(position);
        for (int i = 0; i < list.getChildCount(); i++) {
            View child = list.getChildAt(i);
            if (list.getChildViewHolder(child).getItemViewType() == type && child.getHeight() > 0) {
                return child.getHeight();
            }
        }
        return 0;
    }
}
