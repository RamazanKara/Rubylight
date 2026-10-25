package com.limelight.console;

import android.content.Context;
import android.view.LayoutInflater;
import android.view.View;
import android.view.ViewGroup;
import android.widget.TextView;

import androidx.annotation.NonNull;
import androidx.annotation.StringRes;
import androidx.recyclerview.widget.LinearLayoutManager;
import androidx.recyclerview.widget.RecyclerView;

import com.limelight.R;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.ui.Motion;

import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.function.Function;

/**
 * Home's shelves, one row each: games to continue, favourites, the library and the PCs. A shelf scrolls sideways on its own
 * (see {@link ShelfAdapter}); the list of shelves scrolls up and down and snaps the focused shelf to its top. Each shelf
 * remembers the card that had focus last, so a key up or down returns to it; a shelf not seen before takes the card nearest
 * the one focus came from. Cards are found again across a refresh by their stable keys.
 */
final class ConsoleHomeAdapter extends RecyclerView.Adapter<ConsoleHomeAdapter.ShelfHolder> implements ShelfLayoutManager.RowFocus {
    interface Listener {
        void onGame(HomeModel.GameCard card);

        /** A long press (or the D-pad centre held, or X) on a game card. */
        void onGameMenu(HomeModel.GameCard card);

        /** The Menu key on a game card: star or unstar it at once. */
        void onGameToggleFavourite(HomeModel.GameCard card);

        void onHost(HomeModel.HostCard card);

        /** The three dots, a long press, Menu, X or a right-click on a host card: that PC's options. */
        void onHostOptions(HomeModel.HostCard card);
    }

    /** One shelf: a heading and its cards. */
    private static final class Shelf {
        final String key;
        @StringRes final int title;
        final List<ShelfAdapter.Item> items;

        Shelf(String key, int title, List<ShelfAdapter.Item> items) {
            this.key = key;
            this.title = title;
            this.items = items;
        }
    }

    /** A position on Home: the shelf and the card on it. */
    static final class Cursor {
        final int row;
        final int card;

        Cursor(int row, int card) {
            this.row = row;
            this.card = card;
        }

        @Override public boolean equals(Object other) {
            return other instanceof Cursor && ((Cursor) other).row == row && ((Cursor) other).card == card;
        }

        @Override public int hashCode() {
            return row * 31 + card;
        }

        @Override public String toString() {
            return "row " + row + " card " + card;
        }
    }

    private final ShelfAdapter.Env env;
    private final List<Shelf> shelves = new ArrayList<>();
    private final Map<String, Long> ids = new HashMap<>();
    /** The card that last had focus on each shelf, and where each shelf is scrolled to. */
    private final Map<String, String> remembered = new HashMap<>();
    private final Map<String, int[]> scrolled = new HashMap<>();
    private final RecyclerView.RecycledViewPool pool = new RecyclerView.RecycledViewPool();
    private ShelfAdapter.FocusListener focusListener;

    ConsoleHomeAdapter(Context context, CoverLoaders covers, Function<String, ComputerDetails> hosts, Listener listener) {
        env = new ShelfAdapter.Env(context, covers, hosts, listener);
        env.focus = (shelf, item, card) -> {
            remembered.put(shelf, item.key);
            if (focusListener != null) focusListener.onCardFocus(shelf, item, card);
        };
        pool.setMaxRecycledViews(ShelfAdapter.COVER_GAME, 24);
        pool.setMaxRecycledViews(ShelfAdapter.HOST, 8);
        setHasStableIds(true);
    }

    /** Who hears of every card that takes focus (Home's hero and backdrop). */
    void setFocusListener(ShelfAdapter.FocusListener listener) {
        this.focusListener = listener;
    }

    /** True on TV: posters are 2:3, on a phone covers are 4/3. */
    void setTelevision(boolean television) {
        setCoverHeight(ShelfAdapter.coverHeight(env.context, television));
    }

    /** The cover loaders exist once the app's client identity is known. */
    void setCovers(CoverLoaders covers) {
        env.covers = covers;
        notifyDataSetChanged();
    }

    /** Library covers are this tall (a phone's cover 4/3, a TV poster 3/2). */
    void setCoverHeight(int px) {
        if (px != env.coverHeightPx) {
            env.coverHeightPx = px;
            notifyDataSetChanged();
        }
    }

    /** True while a key is held down along the cards, or the next one is a moment away from the last. */
    boolean isFastTravel() {
        return env.travel.isActive(android.os.SystemClock.uptimeMillis());
    }

    void submit(HomeModel model) {
        shelves.clear();
        addGames("continue", R.string.console_section_continue, model.continuePlaying);
        addGames("favourites", R.string.console_section_favourites, model.favourites);
        addGames("library", R.string.console_section_library, model.library);
        if (!model.hosts.isEmpty()) {
            List<ShelfAdapter.Item> items = new ArrayList<>();
            for (HomeModel.HostCard card : model.hosts) {
                items.add(new ShelfAdapter.Item("host:" + card.uuid, null, card));
            }
            shelves.add(new Shelf("shelf:hosts", R.string.console_section_hosts, items));
        }
        notifyDataSetChanged();
    }

    private void addGames(String name, @StringRes int title, List<HomeModel.GameCard> cards) {
        if (cards.isEmpty()) return;
        List<ShelfAdapter.Item> items = new ArrayList<>();
        for (HomeModel.GameCard card : cards) {
            items.add(new ShelfAdapter.Item(card.key, card, null));
        }
        shelves.add(new Shelf("shelf:" + name, title, items));
    }

    // ---- Cursors: where cards are, found again by key. ----

    int rowCount() {
        return shelves.size();
    }

    String rowKey(int row) {
        return shelves.get(row).key;
    }

    int cardCount(int row) {
        return shelves.get(row).items.size();
    }

    /** The item at the cursor, or null when there is none. */
    ShelfAdapter.Item itemAt(Cursor at) {
        if (at == null || at.row < 0 || at.row >= shelves.size()) return null;
        List<ShelfAdapter.Item> items = shelves.get(at.row).items;
        return at.card >= 0 && at.card < items.size() ? items.get(at.card) : null;
    }

    /** The stable id of the card at the cursor, or null. */
    String keyAt(Cursor at) {
        ShelfAdapter.Item item = itemAt(at);
        return item == null ? null : item.key;
    }

    /** The first card of the first shelf, or null when there is none. */
    Cursor firstCard() {
        return shelves.isEmpty() ? null : new Cursor(0, 0);
    }

    /**
     * Where focus starts: the game that runs now (in Continue playing, else anywhere), otherwise the first card of the
     * first shelf.
     */
    Cursor initialFocus() {
        Cursor anywhere = null;
        for (int row = 0; row < shelves.size(); row++) {
            List<ShelfAdapter.Item> items = shelves.get(row).items;
            for (int card = 0; card < items.size(); card++) {
                HomeModel.GameCard game = items.get(card).game;
                if (game != null && game.badge == HomeModel.Badge.RUNNING) {
                    if (shelves.get(row).key.equals("shelf:continue")) return new Cursor(row, card);
                    if (anywhere == null) anywhere = new Cursor(row, card);
                }
            }
        }
        return anywhere != null ? anywhere : firstCard();
    }

    /** Where the card with this stable id is, or null. */
    Cursor positionOfKey(String key) {
        for (int row = 0; row < shelves.size(); row++) {
            List<ShelfAdapter.Item> items = shelves.get(row).items;
            for (int card = 0; card < items.size(); card++) {
                if (items.get(card).key.equals(key)) return new Cursor(row, card);
            }
        }
        return null;
    }

    /** The card at or before the cursor on its shelf, else the shelf's first, else the nearest shelf's; null with no cards. */
    Cursor nearestCard(Cursor at) {
        if (shelves.isEmpty() || at == null) return firstCard();
        int row = Math.max(0, Math.min(at.row, shelves.size() - 1));
        int count = shelves.get(row).items.size();
        return new Cursor(row, Math.max(0, Math.min(at.card, count - 1)));
    }

    /** The shelf and card that hold focus now, or null. */
    Cursor focusedCard(RecyclerView list) {
        View focused = list.getFocusedChild();
        if (focused == null) return null;
        RecyclerView.ViewHolder holder = list.getChildViewHolder(focused);
        if (!(holder instanceof ShelfHolder)) return null;
        ShelfHolder shelf = (ShelfHolder) holder;
        View card = shelf.cards.getFocusedChild();
        if (card == null) return null;
        int row = shelf.getBindingAdapterPosition();
        int at = shelf.cards.getChildAdapterPosition(card);
        return row < 0 || at < 0 ? null : new Cursor(row, at);
    }

    /** The card's view when its shelf and the card are laid out, else null. */
    View cardView(RecyclerView list, Cursor at) {
        if (at == null) return null;
        RecyclerView.ViewHolder holder = list.findViewHolderForAdapterPosition(at.row);
        if (!(holder instanceof ShelfHolder)) return null;
        RecyclerView.ViewHolder card = ((ShelfHolder) holder).cards.findViewHolderForAdapterPosition(at.card);
        return card == null ? null : card.itemView;
    }

    /** Brings the card in range of the layout when it is not laid out; ask again after the next layout. */
    void reveal(RecyclerView list, Cursor at) {
        if (at == null) return;
        RecyclerView.ViewHolder holder = list.findViewHolderForAdapterPosition(at.row);
        if (!(holder instanceof ShelfHolder)) {
            list.scrollToPosition(at.row);
            return;
        }
        ((ShelfHolder) holder).cards.scrollToPosition(at.card);
    }

    // ---- Vertical moves: the card a key lands on in the shelf it moves to. ----

    @Override
    public View cardIn(RecyclerView rows, View row, View from) {
        RecyclerView.ViewHolder holder = rows.getChildViewHolder(row);
        if (!(holder instanceof ShelfHolder)) return null;
        ShelfHolder shelf = (ShelfHolder) holder;
        String key = remembered.get(shelf.key);
        if (key != null) {
            int at = shelf.adapter.positionOfKey(key);
            RecyclerView.ViewHolder card = at < 0 ? null : shelf.cards.findViewHolderForAdapterPosition(at);
            if (card != null) return card.itemView;
        }
        return nearestTo(shelf.cards, from);
    }

    /** The laid-out card whose centre is the closest across to {@code from}'s. */
    static View nearestTo(RecyclerView cards, View from) {
        int[] where = new int[2];
        from.getLocationInWindow(where);
        int centre = where[0] + from.getWidth() / 2;
        View best = null;
        int distance = Integer.MAX_VALUE;
        for (int i = 0; i < cards.getChildCount(); i++) {
            View card = cards.getChildAt(i);
            if (!card.isFocusable()) continue;
            card.getLocationInWindow(where);
            int d = Math.abs(where[0] + card.getWidth() / 2 - centre);
            if (d < distance) {
                distance = d;
                best = card;
            }
        }
        return best;
    }

    // ---- RecyclerView.Adapter ----

    @Override
    public int getItemCount() {
        return shelves.size();
    }

    @Override
    public long getItemId(int position) {
        String key = shelves.get(position).key;
        Long id = ids.get(key);
        if (id == null) {
            id = (long) ids.size() + 1;
            ids.put(key, id);
        }
        return id;
    }

    @NonNull
    @Override
    public ShelfHolder onCreateViewHolder(@NonNull ViewGroup parent, int type) {
        View view = LayoutInflater.from(parent.getContext()).inflate(R.layout.console_shelf, parent, false);
        return new ShelfHolder(view);
    }

    @Override
    public void onBindViewHolder(@NonNull ShelfHolder holder, int position) {
        Shelf shelf = shelves.get(position);
        holder.key = shelf.key;
        holder.title.setText(shelf.title);
        holder.adapter.setItems(shelf.key, shelf.items);
        // A shelf comes back where it was left: the same cards in view, not the first ones.
        int[] at = scrolled.get(shelf.key);
        if (at != null && at[0] < shelf.items.size()) holder.layout.scrollToPositionWithOffset(at[0], at[1]);
    }

    @Override
    public void onViewRecycled(@NonNull ShelfHolder holder) {
        saveScroll(holder);
    }

    private void saveScroll(ShelfHolder holder) {
        if (holder.key == null) return;
        int first = holder.layout.findFirstVisibleItemPosition();
        View view = first < 0 ? null : holder.layout.findViewByPosition(first);
        if (view != null) scrolled.put(holder.key, new int[]{first, view.getLeft() - holder.cards.getPaddingLeft()});
    }

    final class ShelfHolder extends RecyclerView.ViewHolder {
        final TextView title;
        final RecyclerView cards;
        final ShelfAdapter adapter;
        final ShelfLayoutManager layout;
        String key;

        ShelfHolder(View view) {
            super(view);
            title = view.findViewById(R.id.shelf_title);
            cards = view.findViewById(R.id.shelf_cards);
            adapter = new ShelfAdapter(env);
            layout = new ShelfLayoutManager(view.getContext(), LinearLayoutManager.HORIZONTAL,
                    () -> isFastTravel() || Motion.reduced(view.getContext()));
            layout.setRecycleChildrenOnDetach(true);
            layout.setItemPrefetchEnabled(true);
            cards.setLayoutManager(layout);
            cards.setAdapter(adapter);
            cards.setRecycledViewPool(pool);
            cards.setItemAnimator(null);
            cards.setItemViewCacheSize(4);
        }
    }
}
