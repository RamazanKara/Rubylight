package com.limelight.console;

import android.content.Context;
import android.content.res.ColorStateList;
import android.graphics.ColorMatrix;
import android.graphics.ColorMatrixColorFilter;
import android.graphics.drawable.Drawable;
import android.os.Build;
import android.os.SystemClock;
import android.view.KeyEvent;
import android.view.LayoutInflater;
import android.view.View;
import android.view.ViewGroup;
import android.widget.ImageView;
import android.widget.TextView;

import androidx.annotation.NonNull;
import androidx.appcompat.content.res.AppCompatResources;
import androidx.recyclerview.widget.RecyclerView;

import com.google.android.material.color.MaterialColors;
import com.limelight.R;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.ui.ArtFit;
import com.limelight.ui.FastTravel;

import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.function.Function;

/**
 * The cards of one Home shelf: games as covers (posters on TV), hosts as wide cards. Every shelf has its own adapter and
 * they all share one {@link Env} and one view pool, so a card scrolled off one shelf serves the next.
 */
final class ShelfAdapter extends RecyclerView.Adapter<ShelfAdapter.CardHolder> {
    static final int COVER_GAME = 0;
    static final int HOST = 1;
    static final int ALL_TILE = 2;

    /** One card of a shelf: a game, a host or a row's "All N" tile, found again across a refresh by its stable key. */
    static final class Item {
        final String key;
        final HomeModel.GameCard game;
        final HomeModel.HostCard host;
        /** The row an "All N" tile opens in All games; null on other cards. */
        final HomeModel.Row all;

        Item(String key, HomeModel.GameCard game, HomeModel.HostCard host) {
            this(key, game, host, null);
        }

        private Item(String key, HomeModel.GameCard game, HomeModel.HostCard host, HomeModel.Row all) {
            this.key = key;
            this.game = game;
            this.host = host;
            this.all = all;
        }

        /** The tile that ends a row with more games than it shows. */
        static Item allOf(HomeModel.Row row) {
            return new Item("all:" + row.key, null, null, row);
        }

        int type() {
            return game != null ? COVER_GAME : host != null ? HOST : ALL_TILE;
        }
    }

    /** What a shelf tells Home when one of its cards takes focus. */
    interface FocusListener {
        void onCardFocus(String shelf, Item item, View card);
    }

    /** What every shelf shares. */
    static final class Env {
        final Context context;
        final Function<String, ComputerDetails> hosts;
        final ConsoleHomeAdapter.Listener listener;
        /** Tells a held D-pad key from a press, so a run across the covers does not queue a focus tween per card. */
        final FastTravel travel = new FastTravel();
        CoverLoaders covers;
        int coverHeightPx;
        /** A game card's width where a grid sets it (All games); 0 keeps a shelf's width. */
        int cardWidthPx;
        FocusListener focus;

        Env(Context context, CoverLoaders covers, Function<String, ComputerDetails> hosts, ConsoleHomeAdapter.Listener listener) {
            this.context = context;
            this.covers = covers;
            this.hosts = hosts;
            this.listener = listener;
        }
    }

    private final Env env;
    private String shelf = "";
    private List<Item> items = new ArrayList<>();
    private final Map<String, Long> ids = new HashMap<>();

    ShelfAdapter(Env env) {
        this.env = env;
        setHasStableIds(true);
    }

    void setItems(String shelf, List<Item> next) {
        this.shelf = shelf;
        items = next;
        notifyDataSetChanged();
    }

    Item item(int position) {
        return position >= 0 && position < items.size() ? items.get(position) : null;
    }

    int positionOfKey(String key) {
        for (int i = 0; i < items.size(); i++) {
            if (items.get(i).key.equals(key)) return i;
        }
        return -1;
    }

    @Override
    public int getItemCount() {
        return items.size();
    }

    @Override
    public int getItemViewType(int position) {
        return items.get(position).type();
    }

    @Override
    public long getItemId(int position) {
        String key = items.get(position).key;
        Long id = ids.get(key);
        if (id == null) {
            id = (long) ids.size() + 1;
            ids.put(key, id);
        }
        return id;
    }

    @NonNull
    @Override
    public CardHolder onCreateViewHolder(@NonNull ViewGroup parent, int type) {
        LayoutInflater inflater = LayoutInflater.from(parent.getContext());
        boolean cover = type == COVER_GAME;
        int layout = cover ? R.layout.console_card_cover : type == ALL_TILE ? R.layout.console_card_all : R.layout.console_card_host;
        View view = inflater.inflate(layout, parent, false);
        int width = type != HOST && env.cardWidthPx > 0 ? env.cardWidthPx
                : view.getResources().getDimensionPixelSize(type == HOST ? R.dimen.shelf_host_width : R.dimen.shelf_card_width);
        RecyclerView.LayoutParams params = new RecyclerView.LayoutParams(width, ViewGroup.LayoutParams.WRAP_CONTENT);
        view.setLayoutParams(params);
        return new CardHolder(env, view, cover, type == ALL_TILE);
    }

    @Override
    public void onBindViewHolder(@NonNull CardHolder holder, int position) {
        Item item = items.get(position);
        if (item.game != null) holder.bindGame(shelf, item);
        else if (item.all != null) holder.bindAll(shelf, item);
        else holder.bindHost(shelf, item);
    }

    /** A poster in grey: a game the host lists but has not installed. One filter for every such card, set at bind. */
    private static final ColorMatrixColorFilter NOT_INSTALLED;

    static {
        ColorMatrix grey = new ColorMatrix();
        grey.setSaturation(0f);
        // A little darker too, so it sits back from the installed games around it.
        ColorMatrix dim = new ColorMatrix();
        dim.setScale(0.72f, 0.72f, 0.72f, 1f);
        grey.postConcat(dim);
        NOT_INSTALLED = new ColorMatrixColorFilter(grey);
    }

    /** The height of a cover: its card's width less the inset on both sides, times 3/2 for a TV poster, 4/3 on a phone. */
    static int coverHeight(Context context, boolean television) {
        return coverHeight(context, television, context.getResources().getDimensionPixelSize(R.dimen.shelf_card_width));
    }

    /** The same for a card of the given width. */
    static int coverHeight(Context context, boolean television, int cardWidthPx) {
        int inset = context.getResources().getDimensionPixelSize(R.dimen.cover_inset);
        int width = cardWidthPx - 2 * inset;
        return Math.max(1, television ? width * 3 / 2 : width * 4 / 3);
    }

    static final class CardHolder extends RecyclerView.ViewHolder {
        private final Env env;
        final ImageView image;
        final TextView imageText;
        /** A TV poster has neither: the hero names the focused game. */
        final TextView title;
        final TextView subtitle;
        final TextView badge;
        final View frame;
        final View more;
        final boolean cover;
        final boolean tile;
        /** A TV poster's chips: the star of a favourite and the store's glyph or name. The phone's cover has neither. */
        final ImageView star;
        final TextView store;
        /** An "All N" tile's count, the row's name and the store's glyph. */
        final TextView allCount;
        final TextView allName;
        final ImageView allGlyph;
        /** The host's three dots while the D-pad has marked them; the card keeps focus. */
        private boolean moreMarked;

        CardHolder(Env env, View view, boolean cover) {
            this(env, view, cover, false);
        }

        CardHolder(Env env, View view, boolean cover, boolean tile) {
            super(view);
            this.env = env;
            this.cover = cover;
            this.tile = tile;
            image = view.findViewById(cover ? R.id.console_cover : R.id.console_thumb);
            imageText = view.findViewById(cover ? R.id.console_cover_text : R.id.console_thumb_text);
            frame = view.findViewById(cover ? R.id.console_cover_frame : tile ? R.id.console_all_frame : R.id.console_thumb_frame);
            title = view.findViewById(R.id.console_title);
            subtitle = view.findViewById(R.id.console_subtitle);
            badge = view.findViewById(R.id.console_badge);
            more = view.findViewById(R.id.console_more);
            star = view.findViewById(R.id.console_star);
            store = view.findViewById(R.id.console_store);
            allCount = view.findViewById(R.id.console_all_count);
            allName = view.findViewById(R.id.console_all_name);
            allGlyph = view.findViewById(R.id.console_all_glyph);
            view.setOnHoverListener((card, event) -> {
                card.setHovered(event.getActionMasked() != android.view.MotionEvent.ACTION_HOVER_EXIT);
                return false;
            });
        }

        /**
         * What a screen reader says for a game card, whether or not the card shows it: the title, the store, Running
         * or Suspended, Not installed or Not added, where it is and the star.
         */
        static String describe(Context context, HomeModel.GameCard card, String where, CharSequence state) {
            StringBuilder description = new StringBuilder(card.title);
            String storeName = card.storeName();
            if (storeName != null) description.append(", ").append(storeName);
            if (state != null) description.append(", ").append(state);
            CharSequence availability = availabilityOf(context, card);
            if (availability != null) description.append(", ").append(availability);
            description.append(", ").append(where);
            if (card.favourite) description.append(", ").append(context.getString(R.string.console_favourite_description));
            return description.toString();
        }

        /** The hero's line over the focused game's title: store · state · not installed · where. */
        static String metaOf(Context context, HomeModel.GameCard card) {
            StringBuilder meta = new StringBuilder();
            String storeName = card.storeName();
            if (storeName != null) meta.append(storeName).append(" · ");
            CharSequence state = stateOf(context, card);
            if (state != null) meta.append(state).append(" · ");
            CharSequence availability = availabilityOf(context, card);
            if (availability != null) meta.append(availability).append(" · ");
            return meta.append(whereOf(context, card)).toString();
        }

        /** Not installed, Not added (the host adds it on request) or Add it on the PC; null for a game that can start. */
        static CharSequence availabilityOf(Context context, HomeModel.GameCard card) {
            if (card.notAdded()) {
                return context.getString(card.addable ? R.string.console_chip_not_added : R.string.console_add_on_pc);
            }
            return card.notInstalled() ? context.getString(R.string.console_chip_not_installed) : null;
        }

        private void setFrameHeight() {
            ViewGroup.LayoutParams card = itemView.getLayoutParams();
            if (env.cardWidthPx > 0 && card != null && card.width != env.cardWidthPx) {
                card.width = env.cardWidthPx;
                itemView.setLayoutParams(card);
            }
            if (env.coverHeightPx > 0 && frame != null) {
                ViewGroup.LayoutParams params = frame.getLayoutParams();
                if (params.height != env.coverHeightPx) {
                    params.height = env.coverHeightPx;
                    frame.setLayoutParams(params);
                }
            }
        }

        void bindGame(String shelf, Item item) {
            HomeModel.GameCard card = item.game;
            Context context = env.context;
            setFrameHeight();
            image.setImageTintList(null);
            // Set once here, never animated: the art that loads later is drawn through it.
            image.setColorFilter(cover && card.notInstalled() ? NOT_INSTALLED : null);
            // A cover is fitted to its poster by the art it is given (ArtFit).
            ArtFit.reset(image);
            if (card.appId == 0) {
                // A game not added has no app id, so no cover the host can send: its title on the poster's surface.
                image.setImageDrawable(null);
                imageText.setText(card.title);
                imageText.setVisibility(View.VISIBLE);
            } else if (env.covers != null) {
                env.covers.bind(env.hosts.apply(card.hostUuid), CoverLoaders.appOf(card.title, card.appId, card.source), image,
                        imageText);
            }
            String where = whereOf(context, card);
            if (title != null) title.setText(card.title);
            if (subtitle != null) subtitle.setText(where);
            CharSequence state = stateOf(context, card);
            badge.setText(state);
            badge.setVisibility(state == null ? View.GONE : View.VISIBLE);
            if (star != null) {
                badge.setCompoundDrawablesRelativeWithIntrinsicBounds(
                        card.badge == HomeModel.Badge.RUNNING ? R.drawable.ic_live_dot : 0, 0, 0, 0);
                star.setVisibility(card.favourite ? View.VISIBLE : View.GONE);
            }
            if (store != null) bindStore(context, card);
            itemView.setContentDescription(describe(context, card, where, state));
            itemView.setOnClickListener(v -> env.listener.onGame(card));
            itemView.setOnLongClickListener(v -> {
                env.listener.onGameMenu(card);
                return true;
            });
            itemView.setOnFocusChangeListener((v, hasFocus) -> {
                // The card's own focus tween is skipped while a key is held: the new card is focused at once, the old at rest.
                if (env.travel.isActive(SystemClock.uptimeMillis())) {
                    // Focus changes before the card's drawable state does; settle the state it is moving to.
                    v.refreshDrawableState();
                    FastTravel.settle(v);
                }
                if (hasFocus && env.focus != null) env.focus.onCardFocus(shelf, item, v);
            });
            itemView.setOnKeyListener((v, keyCode, event) -> {
                if (event.getAction() == KeyEvent.ACTION_DOWN && movesFocus(keyCode)) {
                    env.travel.onMove(event.getRepeatCount(), SystemClock.uptimeMillis());
                }
                if (keyCode == KeyEvent.KEYCODE_BUTTON_X || keyCode == KeyEvent.KEYCODE_MENU) {
                    // X and Menu open the game's options (where it is starred), like a long press and like a PC's card.
                    if (event.getAction() == KeyEvent.ACTION_UP && !event.isCanceled()) env.listener.onGameMenu(card);
                    return true;
                }
                return false;
            });
        }

        /**
         * The chip at the poster's foot: the store's glyph, with Not installed or what adding takes beside it. A store
         * without a glyph shows its name instead; a game of no store shows the chip only when it cannot start.
         */
        private void bindStore(Context context, HomeModel.GameCard card) {
            int glyph = Stores.glyph(card.store());
            CharSequence label = availabilityOf(context, card);
            // On a narrow poster (All games' grid) the words win: the focus caption names the store anyway.
            if (label != null && glyph != 0 && !fits(context, label)) glyph = 0;
            if (label == null && glyph == 0) label = card.storeName();
            if (label == null && glyph == 0) {
                store.setVisibility(View.GONE);
                return;
            }
            store.setText(label);
            Drawable mark = null;
            if (glyph != 0) {
                mark = AppCompatResources.getDrawable(context, glyph);
                int size = context.getResources().getDimensionPixelSize(R.dimen.card_chip_glyph);
                if (mark != null) mark.setBounds(0, 0, size, size);
            }
            store.setCompoundDrawablesRelative(mark, null, null, null);
            // A glyph alone sits in a round chip.
            int side = label == null ? (context.getResources().getDimensionPixelSize(R.dimen.card_chip_height)
                    - context.getResources().getDimensionPixelSize(R.dimen.card_chip_glyph)) / 2
                    : context.getResources().getDimensionPixelSize(R.dimen.space_s);
            store.setPaddingRelative(side, 0, side, 0);
            store.setVisibility(View.VISIBLE);
        }

        /** Whether the store's glyph and this label fit side by side in the chip on this card's poster. */
        private boolean fits(Context context, CharSequence label) {
            android.content.res.Resources res = context.getResources();
            int card = env.cardWidthPx > 0 ? env.cardWidthPx : res.getDimensionPixelSize(R.dimen.shelf_card_width);
            int margin = res.getDimensionPixelSize(R.dimen.space_s);
            int room = card - 2 * res.getDimensionPixelSize(R.dimen.cover_inset) - 2 * margin;
            float need = store.getPaint().measureText(label.toString()) + res.getDimensionPixelSize(R.dimen.card_chip_glyph)
                    + store.getCompoundDrawablePadding() + 2 * margin;
            return need <= room;
        }

        /** A row's name, as its heading and its "All N" tile say it. */
        static CharSequence rowTitle(Context context, HomeModel.Row row) {
            switch (row.kind) {
                case STORE:
                    return row.storeName;
                case RECENT:
                    return context.getString(R.string.console_row_recent);
                case UNADDED:
                    return context.getString(R.string.console_row_unadded);
                default:
                    return context.getString(HomeModel.FILTER_ALL.equals(row.filter)
                            ? R.string.console_section_library : R.string.console_row_other);
            }
        }

        /** What a screen reader says for a row's "All N" tile. */
        static String describeAll(Context context, HomeModel.Row row) {
            return context.getResources().getQuantityString(R.plurals.console_all_tile_description, row.total,
                    rowTitle(context, row), row.total);
        }

        void bindAll(String shelf, Item item) {
            HomeModel.Row row = item.all;
            Context context = env.context;
            setFrameHeight();
            allCount.setText(String.format(java.util.Locale.getDefault(), "%d", row.total));
            allName.setText(rowTitle(context, row));
            int glyph = Stores.glyph(row.store);
            if (glyph != 0) allGlyph.setImageResource(glyph);
            allGlyph.setVisibility(glyph != 0 ? View.VISIBLE : View.GONE);
            itemView.setContentDescription(describeAll(context, row));
            itemView.setOnClickListener(v -> env.listener.onAllGames(row.filter));
            itemView.setOnLongClickListener(null);
            itemView.setLongClickable(false);
            itemView.setOnFocusChangeListener((v, hasFocus) -> {
                if (env.travel.isActive(SystemClock.uptimeMillis())) {
                    v.refreshDrawableState();
                    FastTravel.settle(v);
                }
                if (hasFocus && env.focus != null) env.focus.onCardFocus(shelf, item, v);
            });
            itemView.setOnKeyListener((v, keyCode, event) -> {
                if (event.getAction() == KeyEvent.ACTION_DOWN && movesFocus(keyCode)) {
                    env.travel.onMove(event.getRepeatCount(), SystemClock.uptimeMillis());
                }
                return false;
            });
        }

        static String whereOf(Context context, HomeModel.GameCard card) {
            return card.hostCount > 1 ? context.getString(R.string.console_card_on_hosts, card.hostCount)
                    : context.getString(R.string.console_card_on_host, card.hostName);
        }

        static CharSequence stateOf(Context context, HomeModel.GameCard card) {
            if (card.badge == HomeModel.Badge.RUNNING) return context.getString(R.string.console_badge_running);
            if (card.badge == HomeModel.Badge.SUSPENDED) return context.getString(R.string.console_badge_suspended);
            return null;
        }

        private static boolean movesFocus(int keyCode) {
            return keyCode == KeyEvent.KEYCODE_DPAD_LEFT || keyCode == KeyEvent.KEYCODE_DPAD_RIGHT
                    || keyCode == KeyEvent.KEYCODE_DPAD_UP || keyCode == KeyEvent.KEYCODE_DPAD_DOWN;
        }

        void bindHost(String shelf, Item item) {
            HomeModel.HostCard card = item.host;
            Context context = env.context;
            imageText.setVisibility(View.GONE);
            image.setImageResource(R.drawable.ic_computer);
            image.setScaleType(ImageView.ScaleType.CENTER_INSIDE);
            image.setImageTintList(ColorStateList.valueOf(
                    MaterialColors.getColor(image, com.google.android.material.R.attr.colorOnSurfaceVariant)));
            image.setVisibility(View.VISIBLE);
            title.setText(card.name);
            String line = hostLine(context, card);
            subtitle.setText(line);
            badge.setVisibility(View.GONE);
            itemView.setContentDescription(card.name + ", " + line);
            itemView.setOnClickListener(v -> env.listener.onHost(card));
            itemView.setOnLongClickListener(v -> {
                env.listener.onHostOptions(card);
                return true;
            });
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) {
                // A mouse's right button.
                itemView.setOnContextClickListener(v -> {
                    env.listener.onHostOptions(card);
                    return true;
                });
            }
            // The dots are marked by the D-pad, never a focus stop of their own (a style can make buttons focusable).
            more.setFocusable(false);
            more.setContentDescription(context.getString(R.string.library_more_for, card.name));
            more.setOnClickListener(v -> env.listener.onHostOptions(card));
            markMore(false);
            itemView.setOnFocusChangeListener((v, hasFocus) -> {
                if (!hasFocus) markMore(false);
                else if (env.focus != null) env.focus.onCardFocus(shelf, item, v);
            });
            itemView.setOnKeyListener((v, keyCode, event) -> {
                if (event.getAction() == KeyEvent.ACTION_DOWN && movesFocus(keyCode)) {
                    env.travel.onMove(event.getRepeatCount(), SystemClock.uptimeMillis());
                }
                return onHostKey(card, keyCode, event);
            });
        }

        private void markMore(boolean marked) {
            moreMarked = marked;
            more.setHovered(marked);
        }

        /**
         * Menu, X and Shift+F10 open the options at once. Toward the end of the row with nothing further
         * that way, the D-pad marks the three dots (the card keeps focus, as in the list of PCs); Center
         * opens them, the way back unmarks.
         */
        private boolean onHostKey(HomeModel.HostCard card, int keyCode, KeyEvent event) {
            boolean down = event.getAction() == KeyEvent.ACTION_DOWN;
            if (keyCode == KeyEvent.KEYCODE_MENU || keyCode == KeyEvent.KEYCODE_BUTTON_X
                    || (keyCode == KeyEvent.KEYCODE_F10 && event.isShiftPressed())) {
                markMore(false);
                if (!down && !event.isCanceled()) {
                    env.listener.onHostOptions(card);
                }
                return true;
            }
            if (!event.hasNoModifiers()) {
                return false;
            }
            boolean rtl = itemView.getLayoutDirection() == View.LAYOUT_DIRECTION_RTL;
            int towardMore = rtl ? KeyEvent.KEYCODE_DPAD_LEFT : KeyEvent.KEYCODE_DPAD_RIGHT;
            int towardCard = rtl ? KeyEvent.KEYCODE_DPAD_RIGHT : KeyEvent.KEYCODE_DPAD_LEFT;
            if (!moreMarked) {
                if (keyCode == towardMore && atRowEnd()) {
                    if (down) markMore(true);
                    return true;
                }
                return false;
            }
            if (keyCode == towardMore) {
                // The press that marked the dots, and the ones after it, stay on them.
                return true;
            }
            if (keyCode == towardCard) {
                if (down) markMore(false);
                return true;
            }
            if (keyCode == KeyEvent.KEYCODE_DPAD_CENTER || keyCode == KeyEvent.KEYCODE_ENTER
                    || keyCode == KeyEvent.KEYCODE_NUMPAD_ENTER || keyCode == KeyEvent.KEYCODE_SPACE
                    || keyCode == KeyEvent.KEYCODE_BUTTON_A) {
                if (!down && !event.isCanceled()) {
                    markMore(false);
                    more.performClick();
                }
                return true;
            }
            if (keyCode == KeyEvent.KEYCODE_BACK || keyCode == KeyEvent.KEYCODE_BUTTON_B
                    || keyCode == KeyEvent.KEYCODE_ESCAPE) {
                if (!down && !event.isCanceled()) markMore(false);
                return true;
            }
            markMore(false);
            return false;
        }

        /** True when no card follows this one on its shelf, so the end direction has nowhere else to go. */
        private boolean atRowEnd() {
            int position = getBindingAdapterPosition();
            if (position == RecyclerView.NO_POSITION) {
                return false;
            }
            RecyclerView.Adapter<?> adapter = getBindingAdapter();
            return adapter == null || position >= adapter.getItemCount() - 1;
        }

        static String hostLine(Context context, HomeModel.HostCard card) {
            switch (card.line) {
                case GAMES:
                    return context.getString(R.string.console_host_games, card.gameCount);
                case SAVED_GAMES:
                    return context.getString(R.string.console_host_saved_games, card.gameCount);
                case NO_LIBRARY:
                    return context.getString(R.string.console_host_no_library);
                default:
                    return context.getString(R.string.console_host_offline);
            }
        }
    }
}
