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
 * Home's shelves, one row each: games to continue, favourites, the library's rows (see {@link HomeModel#rows}) and the PCs. A shelf scrolls sideways on its own
 * (see {@link ShelfAdapter}); the list of shelves scrolls up and down and snaps the focused shelf to its top. Each shelf
 * remembers the card that had focus last, so a key up or down returns to it; a shelf not seen before takes the card nearest
 * the one focus came from. Cards are found again across a refresh by their stable keys.
 */
final class ConsoleHomeAdapter extends RecyclerView.Adapter<ConsoleHomeAdapter.ShelfHolder> implements ShelfLayoutManager.RowFocus {
    interface Listener {
        void onGame(HomeModel.GameCard card);

        /** A long press (or the D-pad centre held), X or Menu on a game card (on a phone's Home, Menu stars it instead). */
        void onGameMenu(HomeModel.GameCard card);

        /** The Menu key on a game card of a phone's Home: star or unstar it at once. */
        void onGameToggleFavourite(HomeModel.GameCard card);

        void onHost(HomeModel.HostCard card);

        /** The three dots, a long press, Menu, X or a right-click on a host card: that PC's options. */
        void onHostOptions(HomeModel.HostCard card);

        /** A row's "All N" tile: All games under that row's filter. */
        default void onAllGames(String filter) {
        }

        /** The hero's Play: starts or resumes the game, or opens its page where that has to happen first. */
        default void onHeroPlay(HomeModel.GameCard card) {
        }

        /** The hero's Quit game. */
        default void onHeroQuit(HomeModel.GameCard card) {
        }

        /** The hero shows this game now: the ambient follows it. */
        default void onHeroShown(HomeModel.GameCard card) {
        }

        /** What Go on the hero's Play or Favourite says while it has focus; null when neither has it any more. */
        default void onHeroFocus(CharSequence go) {
        }
    }

    /** The shelf's view: a row of cards, the phone's carousel or the handheld's hero. */
    private static final int ROW = 0;
    private static final int HERO_PHONE = 1;
    private static final int HERO_WIDE = 2;

    /** One shelf: a heading and its cards. */
    private static final class Shelf {
        final String key;
        final CharSequence title;
        List<ShelfAdapter.Item> items;
        /** How many games the row has, also past those it shows; 0 says no count. */
        int count;
        /** What the heading's chevron opens in All games; null without one. */
        String filter;
        int type = ROW;
        /** The game a hero shows large; null on a row. */
        HomeModel.GameCard hero;

        Shelf(String key, CharSequence title, List<ShelfAdapter.Item> items) {
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
    /** The shelves on screen now: their places are written down before a new list of shelves takes them over. */
    private final java.util.Set<ShelfHolder> attached = new java.util.HashSet<>();
    private final RecyclerView.RecycledViewPool pool = new RecyclerView.RecycledViewPool();
    private ShelfAdapter.FocusListener focusListener;
    private boolean television;
    private HomeView.Group group = HomeView.Group.STORE;
    private float cardScale = 1f;
    /** A phone's and a handheld's Home open on a hero; a TV's hero is the focused card's. */
    private boolean heroes;
    private final com.limelight.ui.GlassForm form;
    private final Listener listener;
    /** The hero's cards are bigger than a shelf's, and a press on them does something else. */
    private final ShelfAdapter.Env heroEnv;
    private final ShelfAdapter.Env panelEnv;
    private HomeHero hero;
    /** The game the hero shows, kept across a refresh. */
    private String heroKey;
    /** The game that led the hero's list at the last refresh. */
    private String heroLead;

    ConsoleHomeAdapter(Context context, CoverLoaders covers, Function<String, ComputerDetails> hosts, Listener listener) {
        env = new ShelfAdapter.Env(context, covers, hosts, listener);
        this.listener = listener;
        form = com.limelight.ui.GlassForm.of(context);
        Listener onHero = new HeroCards();
        heroEnv = new ShelfAdapter.Env(context, covers, hosts, onHero);
        panelEnv = new ShelfAdapter.Env(context, covers, hosts, onHero);
        panelEnv.line = card -> HomeHeroViews.lineOf(context, card, System.currentTimeMillis());
        int cover = HomeHero.coverWidth(context);
        heroEnv.cardWidthPx = cover;
        heroEnv.coverHeightPx = cover * 3 / 2;
        env.focus = (shelf, item, card) -> {
            remembered.put(shelf, item.key);
            if (focusListener != null) focusListener.onCardFocus(shelf, item, card);
        };
        pool.setMaxRecycledViews(ShelfAdapter.COVER_GAME, 24);
        pool.setMaxRecycledViews(ShelfAdapter.HOST, 8);
        pool.setMaxRecycledViews(ShelfAdapter.ALL_TILE, 4);
        pool.setMaxRecycledViews(ShelfAdapter.HERO_COVER, 6);
        pool.setMaxRecycledViews(ShelfAdapter.ROW_GAME, 6);
        setHasStableIds(true);
    }

    /** Who hears of every card that takes focus (Home's hero and backdrop). */
    void setFocusListener(ShelfAdapter.FocusListener listener) {
        this.focusListener = listener;
        // The hero's own cards tell the hints, not the ambient: the hero's game keeps it.
        ShelfAdapter.FocusListener hero = (shelf, item, card) -> {
            remembered.put(shelf, item.key);
            if (focusListener != null) focusListener.onCardFocus(HomeHero.SHELF, item, card);
        };
        heroEnv.focus = hero;
        panelEnv.focus = hero;
    }

    /** Whether Home opens on a hero. */
    void setHeroes(boolean heroes) {
        this.heroes = heroes;
    }

    /** The game the hero shows, or null with no hero. */
    ShelfAdapter.Item heroItem() {
        if (hero == null) return null;
        HomeModel.GameCard card = hero.games.get(hero.indexOf(heroKey));
        return new ShelfAdapter.Item(card.key, card, null);
    }

    /** The hero's Play, when the hero is laid out. */
    View heroPlay(RecyclerView list) {
        if (hero == null) return null;
        RecyclerView.ViewHolder holder = list.findViewHolderForAdapterPosition(0);
        return holder == null ? null : holder.itemView.findViewById(R.id.hero_play);
    }

    /** A press on a card of the hero: a neighbour comes to the middle, a line of the panel comes to the hero. */
    private final class HeroCards implements Listener {
        @Override public void onGame(HomeModel.GameCard card) {
            if (hero == null) return;
            if (card.key.equals(heroItem().key)) {
                listener.onGame(card);
                return;
            }
            showHero(card);
        }

        @Override public void onGameMenu(HomeModel.GameCard card) {
            listener.onGameMenu(card);
        }

        @Override public void onGameToggleFavourite(HomeModel.GameCard card) {
            listener.onGameToggleFavourite(card);
        }

        @Override public void onHost(HomeModel.HostCard card) {
            listener.onHost(card);
        }

        @Override public void onHostOptions(HomeModel.HostCard card) {
            listener.onHostOptions(card);
        }
    }

    /** The holder of the hero, told by the phone's carousel or the panel's press. */
    private ShelfHolder heroHolder;

    private void showHero(HomeModel.GameCard card) {
        heroKey = card.key;
        listener.onHeroShown(card);
        if (form != com.limelight.ui.GlassForm.PHONE) {
            // The game that came to the hero leaves the panel, the one it replaces takes its place.
            for (Shelf shelf : shelves) {
                if (!HomeHero.SHELF.equals(shelf.key)) continue;
                shelf.items = items(panelCards(hero, card));
                shelf.hero = card;
                if (heroHolder != null) heroHolder.adapter.setItems(shelf.key, shelf.items);
            }
        }
        if (heroHolder == null || heroHolder.hero == null) return;
        heroHolder.hero.show(hero, heroKey, true);
    }

    /** The panel beside a wide hero: the others of its games. The library lends a hero no panel. */
    static List<HomeModel.GameCard> panelCards(HomeHero hero, HomeModel.GameCard shown) {
        if (hero.source == HomeHero.Source.LIBRARY) return new ArrayList<>();
        List<HomeModel.GameCard> cards = new ArrayList<>(hero.games);
        cards.remove(shown);
        return cards;
    }

    /** True on TV: posters are 2:3, on a phone covers are 4/3. */
    void setTelevision(boolean television) {
        this.television = television;
        setCardScale(cardScale);
    }

    /** A game card's width against the usual one ({@link HomeView.Size}); the cover's height follows. */
    void setCardScale(float scale) {
        cardScale = scale;
        boolean handheld = com.limelight.ui.GlassForm.of(env.context) == com.limelight.ui.GlassForm.HANDHELD;
        int usual = env.context.getResources().getDimensionPixelSize(handheld ? R.dimen.shelf_card_width_handheld : R.dimen.shelf_card_width);
        // The usual size keeps the shelves' own width, as before the size could be chosen.
        env.cardWidthPx = scale == 1f && !handheld ? 0 : Math.round(usual * scale);
        setCoverHeight(ShelfAdapter.coverHeight(env.context, television, env.cardWidthPx > 0 ? env.cardWidthPx : usual));
    }

    /** A row per store, the games apart from the apps, or the library as one row ({@link HomeModel#rows(HomeView.Group)}). */
    void setGroup(HomeView.Group group) {
        this.group = group;
    }

    /** The cover loaders exist once the app's client identity is known. */
    void setCovers(CoverLoaders covers) {
        env.covers = covers;
        heroEnv.covers = covers;
        panelEnv.covers = covers;
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
        for (ShelfHolder holder : attached) saveScroll(holder);
        shelves.clear();
        hero = heroes ? HomeHero.pick(model, group) : null;
        if (hero != null) addHero();
        if (hero == null) addGames("continue", R.string.console_section_continue, model.continuePlaying);
        // The hero holds the favourites when nothing is to continue: not twice on one screen.
        if (hero == null || hero.source != HomeHero.Source.FAVOURITES) addGames("favourites", R.string.console_section_favourites, model.favourites);
        // The library's rows: Recently played, one per store (largest first), Other apps, the games not added; by
        // type, Games and Other apps; or, ungrouped, the library in one row.
        for (HomeModel.Row row : model.rows(group)) {
            List<ShelfAdapter.Item> items = items(row.cards);
            if (row.hasMore()) items.add(ShelfAdapter.Item.allOf(row));
            Shelf shelf = new Shelf(row.key, ShelfAdapter.CardHolder.rowTitle(env.context, row), items);
            shelf.count = row.total;
            if (form == com.limelight.ui.GlassForm.PHONE) shelf.filter = row.filter;
            shelves.add(shelf);
        }
        if (!model.hosts.isEmpty()) {
            List<ShelfAdapter.Item> items = new ArrayList<>();
            for (HomeModel.HostCard card : model.hosts) {
                items.add(new ShelfAdapter.Item("host:" + card.uuid, null, card));
            }
            shelves.add(new Shelf("shelf:hosts", env.context.getString(R.string.console_section_hosts), items));
        }
        notifyDataSetChanged();
    }

    private void addGames(String name, @StringRes int title, List<HomeModel.GameCard> cards) {
        if (cards.isEmpty()) return;
        Shelf shelf = new Shelf("shelf:" + name, env.context.getString(title), items(cards));
        shelf.count = cards.size();
        shelves.add(shelf);
    }

    private void addHero() {
        HomeModel.GameCard shown = hero.games.get(hero.indexOf(hero.shownAfterRefresh(heroKey, heroLead)));
        heroKey = shown.key;
        heroLead = hero.games.get(0).key;
        boolean phone = form == com.limelight.ui.GlassForm.PHONE;
        // The handheld shows its game large and the others beside it.
        List<HomeModel.GameCard> cards = phone ? new ArrayList<>(hero.games) : panelCards(hero, shown);
        Shelf shelf = new Shelf(HomeHero.SHELF, env.context.getString(hero.source == HomeHero.Source.FAVOURITES
                ? R.string.console_section_favourites : R.string.console_section_continue), items(cards));
        shelf.type = phone ? HERO_PHONE : HERO_WIDE;
        shelf.hero = shown;
        shelves.add(shelf);
    }

    private static List<ShelfAdapter.Item> items(List<HomeModel.GameCard> cards) {
        List<ShelfAdapter.Item> items = new ArrayList<>(cards.size() + 1);
        for (HomeModel.GameCard card : cards) {
            items.add(new ShelfAdapter.Item(card.key, card, null));
        }
        return items;
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
            if (shelves.get(row).type != ROW) continue;
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

    /** A game is on several shelves: this card on this shelf, else the first place it is, or null. */
    Cursor positionOfKey(String shelf, String key) {
        for (int row = 0; row < shelves.size(); row++) {
            if (!shelves.get(row).key.equals(shelf)) continue;
            List<ShelfAdapter.Item> items = shelves.get(row).items;
            for (int card = 0; card < items.size(); card++) {
                if (items.get(card).key.equals(key)) return new Cursor(row, card);
            }
        }
        return positionOfKey(key);
    }

    /** Every shelf starts at its first card again. */
    void forgetScroll() {
        scrolled.clear();
        for (ShelfHolder holder : attached) {
            if (holder.hero == null) holder.layout.scrollToPosition(0);
        }
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
        // Into the hero, focus lands on its Play.
        if (shelf.hero != null) return shelf.hero.play;
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
    public int getItemViewType(int position) {
        return shelves.get(position).type;
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
        int layout = type == HERO_PHONE ? R.layout.console_hero_phone : type == HERO_WIDE ? R.layout.console_hero_wide : R.layout.console_shelf;
        View view = LayoutInflater.from(parent.getContext()).inflate(layout, parent, false);
        return new ShelfHolder(view, type);
    }

    @Override
    public void onBindViewHolder(@NonNull ShelfHolder holder, int position) {
        Shelf shelf = shelves.get(position);
        // A holder that still shows this shelf (a refresh, a poll, a return from the game page) is where its user left it.
        // Only a holder that takes up a different shelf is put where that shelf was left.
        boolean moved = !shelf.key.equals(holder.key);
        if (moved) saveScroll(holder);
        holder.key = shelf.key;
        holder.title.setText(shelf.title);
        holder.cards.setContentDescription(shelf.title);
        holder.adapter.setItems(shelf.key, shelf.items);
        if (holder.hero != null) {
            heroHolder = holder;
            holder.hero.show(hero, heroKey, false);
            return;
        }
        holder.bindHead(shelf);
        if (!moved) return;
        int[] at = scrolled.get(shelf.key);
        if (at != null && at[0] < shelf.items.size()) holder.layout.scrollToPositionWithOffset(at[0], at[1]);
        else holder.layout.scrollToPositionWithOffset(0, 0);
    }

    @Override
    public void onViewRecycled(@NonNull ShelfHolder holder) {
        saveScroll(holder);
        holder.key = null;
    }

    @Override
    public void onViewAttachedToWindow(@NonNull ShelfHolder holder) {
        attached.add(holder);
    }

    @Override
    public void onViewDetachedFromWindow(@NonNull ShelfHolder holder) {
        saveScroll(holder);
        attached.remove(holder);
    }

    private void saveScroll(ShelfHolder holder) {
        // The hero stands by its game, not by a scroll offset.
        if (holder.key == null || holder.hero != null) return;
        int first = holder.layout.findFirstVisibleItemPosition();
        View view = first < 0 ? null : holder.layout.findViewByPosition(first);
        if (view != null) scrolled.put(holder.key, new int[]{first, view.getLeft() - holder.cards.getPaddingLeft()});
    }

    final class ShelfHolder extends RecyclerView.ViewHolder {
        final TextView title;
        final RecyclerView cards;
        final ShelfAdapter adapter;
        final ShelfLayoutManager layout;
        /** A hero's own views; null on a row. */
        final HomeHeroViews hero;
        final TextView count;
        final View more;
        final View head;
        String key;

        ShelfHolder(View view, int type) {
            super(view);
            title = view.findViewById(R.id.shelf_title);
            cards = view.findViewById(R.id.shelf_cards);
            count = view.findViewById(R.id.shelf_count);
            more = view.findViewById(R.id.shelf_more);
            head = view.findViewById(R.id.shelf_head);
            // Moving down into a shelf announces it by its name.
            title.setLabelFor(R.id.shelf_cards);
            adapter = type == HERO_PHONE ? new ShelfAdapter(heroEnv, ShelfAdapter.HERO_COVER)
                    : type == HERO_WIDE ? new ShelfAdapter(panelEnv, ShelfAdapter.ROW_GAME) : new ShelfAdapter(env);
            layout = new ShelfLayoutManager(view.getContext(), type == HERO_WIDE ? LinearLayoutManager.VERTICAL : LinearLayoutManager.HORIZONTAL,
                    () -> isFastTravel() || Motion.reduced(view.getContext()));
            layout.setRecycleChildrenOnDetach(true);
            layout.setItemPrefetchEnabled(true);
            cards.setLayoutManager(layout);
            if (type == HERO_WIDE) cards.addItemDecoration(new HomeHeroViews.PanelLines(view.getContext()));
            hero = type == HERO_PHONE ? HomeHeroViews.phone(view, cards, layout, adapter, listener, ConsoleHomeAdapter.this::onCarouselMiddle)
                    : type == HERO_WIDE ? HomeHeroViews.wide(view, env.context, listener, () -> heroEnv.covers, env.hosts) : null;
            if (type == ROW && form == com.limelight.ui.GlassForm.HANDHELD) {
                android.content.res.Resources res = view.getResources();
                head.setPaddingRelative(res.getDimensionPixelSize(R.dimen.space_xl), 0, head.getPaddingEnd(), 0);
                head.setMinimumHeight(res.getDimensionPixelSize(R.dimen.home_head_handheld));
                cards.setPaddingRelative(res.getDimensionPixelSize(R.dimen.glass_edge), 0, cards.getPaddingEnd(), 0);
            }
            if (type == ROW && form == com.limelight.ui.GlassForm.TELEVISION) {
                // The focused cover grows by a tenth of its height and wears a ring: its heading must stay clear of that.
                android.content.res.Resources res = view.getResources();
                head.setMinimumHeight(res.getDimensionPixelSize(R.dimen.home_head_tv));
                head.setPaddingRelative(head.getPaddingStart(), res.getDimensionPixelSize(R.dimen.home_head_tv_drop), head.getPaddingEnd(), 0);
            }
            cards.setAdapter(adapter);
            cards.setRecycledViewPool(pool);
            cards.setItemAnimator(null);
            cards.setItemViewCacheSize(4);
        }

        /** The heading's count and chevron. */
        void bindHead(Shelf shelf) {
            if (count == null) return;
            count.setText(shelf.count > 0 ? String.format(java.util.Locale.getDefault(), "%d", shelf.count) : null);
            more.setVisibility(shelf.filter != null ? View.VISIBLE : View.GONE);
            if (shelf.filter != null) {
                head.setOnClickListener(v -> listener.onAllGames(shelf.filter));
                head.setContentDescription(env.context.getResources().getQuantityString(R.plurals.console_all_tile_description,
                        shelf.count, shelf.title, shelf.count));
            } else {
                head.setOnClickListener(null);
                head.setClickable(false);
                head.setContentDescription(null);
            }
        }
    }

    /** The middle of the carousel changed under a finger or a key. */
    private void onCarouselMiddle(HomeModel.GameCard card) {
        heroKey = card.key;
    }
}
