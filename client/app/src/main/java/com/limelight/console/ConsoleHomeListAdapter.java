package com.limelight.console;

import android.content.Context;
import android.content.res.ColorStateList;
import android.os.Build;
import android.os.SystemClock;
import android.view.KeyEvent;
import android.view.LayoutInflater;
import android.view.View;
import android.view.ViewGroup;
import android.widget.ImageView;
import android.widget.TextView;

import androidx.annotation.NonNull;
import androidx.recyclerview.widget.GridLayoutManager;
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
 * Home's rows as one grid: section headers, wide cards (games to continue, hosts) and library covers.
 * Cards are focusable, so the D-pad and a controller move over them like over the library grid; ids are
 * stable, so a refresh keeps the focused card focused.
 */
final class ConsoleHomeListAdapter extends RecyclerView.Adapter<RecyclerView.ViewHolder> {
    static final int HEADER = 0;
    static final int WIDE_GAME = 1;
    static final int COVER_GAME = 2;
    static final int HOST = 3;
    /** The shelf a TV card tells the focus listener it is on: the grid is one. */
    private static final String TV_SHELF = "home-grid";

    /**
     * How visible a section header is as it scrolls under the top of the list: opaque while most of it is in view,
     * gone once less than half is, so the foot of its letters never peeks out under the tab bar.
     */
    static float headerAlpha(int top, int bottom) {
        int height = bottom - top;
        if (height <= 0) return 1f;
        float shown = Math.max(0, Math.min(height, bottom)) / (float) height;
        return Math.max(0f, Math.min(1f, (shown - 0.5f) * 2f));
    }

    /** Fades the headers that are partly above the list's top edge; the others are left as they are. */
    static void fadeHeaders(RecyclerView list) {
        for (int i = 0; i < list.getChildCount(); i++) {
            View child = list.getChildAt(i);
            RecyclerView.ViewHolder holder = list.getChildViewHolder(child);
            if (holder.getItemViewType() != HEADER) continue;
            float alpha = child.getTop() >= 0 ? 1f : headerAlpha(child.getTop(), child.getBottom());
            if (child.getAlpha() != alpha) child.setAlpha(alpha);
        }
    }

    private static final class Row {
        final int type;
        final String key;
        final CharSequence title;
        final HomeModel.GameCard game;
        final HomeModel.HostCard host;

        Row(int type, String key, CharSequence title, HomeModel.GameCard game, HomeModel.HostCard host) {
            this.type = type;
            this.key = key;
            this.title = title;
            this.game = game;
            this.host = host;
        }
    }

    private final Context context;
    private CoverLoaders covers;
    private final Function<String, ComputerDetails> hosts;
    private final ConsoleHomeAdapter.Listener listener;
    private final List<Row> rows = new ArrayList<>();
    private final Map<String, Long> ids = new HashMap<>();
    private int coverHeightPx;
    /** Tells a held D-pad key from a press, so a run across the covers does not queue a focus tween per card. */
    private final FastTravel travel = new FastTravel();
    /** On TV the cards are the shelves' posters and PC cards, sharing this; null on a phone. */
    private final ShelfAdapter.Env tv;

    ConsoleHomeListAdapter(Context context, CoverLoaders covers, Function<String, ComputerDetails> hosts, ConsoleHomeAdapter.Listener listener) {
        this(context, covers, hosts, listener, false);
    }

    /** {@code television}: the cards are TV posters, which the hero names, instead of the phone's captioned covers. */
    ConsoleHomeListAdapter(Context context, CoverLoaders covers, Function<String, ComputerDetails> hosts, ConsoleHomeAdapter.Listener listener,
                           boolean television) {
        this.context = context;
        this.covers = covers;
        this.hosts = hosts;
        this.listener = listener;
        this.tv = television ? new ShelfAdapter.Env(context, covers, hosts, listener) : null;
        setHasStableIds(true);
    }

    /** The cover loaders exist once the app's client identity is known. */
    void setCovers(CoverLoaders covers) {
        this.covers = covers;
        if (tv != null) tv.covers = covers;
        notifyDataSetChanged();
    }

    /** Library covers are this tall (the grid's column width times 4/3, or 3/2 for a TV poster). */
    void setCoverHeight(int px) {
        if (px != coverHeightPx) {
            coverHeightPx = px;
            if (tv != null) tv.coverHeightPx = px;
            notifyDataSetChanged();
        }
    }

    /** A TV poster's width: its column's. */
    void setCardWidth(int px) {
        if (tv != null && px != tv.cardWidthPx) {
            tv.cardWidthPx = px;
            notifyDataSetChanged();
        }
    }

    /** Who hears of every card that takes focus (TV Home's hero and backdrop). */
    void setFocusListener(ShelfAdapter.FocusListener listener) {
        if (tv != null) tv.focus = listener;
    }

    /** True while a key is held down along the cards, or the next one is a moment away from the last. */
    boolean isFastTravel() {
        return (tv != null ? tv.travel : travel).isActive(SystemClock.uptimeMillis());
    }

    /** The item a TV card binds, or null for a header or out of range. */
    ShelfAdapter.Item itemAt(int position) {
        if (position < 0 || position >= rows.size() || rows.get(position).type == HEADER) return null;
        Row row = rows.get(position);
        return new ShelfAdapter.Item(row.key, row.game, row.host);
    }

    void submit(HomeModel model) {
        submit(model, false);
    }

    /**
     * {@code grouped}: the library under a heading per store, each part whole (a grid has no "All N" tile), as the
     * shelves have it; otherwise one Library part.
     */
    void submit(HomeModel model, boolean grouped) {
        rows.clear();
        if (!model.favourites.isEmpty()) {
            rows.add(new Row(HEADER, "header:favourites", context.getString(R.string.console_section_favourites), null, null));
            for (HomeModel.GameCard card : model.favourites) {
                rows.add(new Row(COVER_GAME, card.key, null, card, null));
            }
        }
        if (!model.continuePlaying.isEmpty()) {
            rows.add(new Row(HEADER, "header:continue", context.getString(R.string.console_section_continue), null, null));
            for (HomeModel.GameCard card : model.continuePlaying) {
                // A TV has no wide card: the hero says where the game runs.
                rows.add(new Row(tv != null ? COVER_GAME : WIDE_GAME, card.key, null, card, null));
            }
        }
        if (grouped) {
            for (HomeModel.Row part : model.rows) {
                List<HomeModel.GameCard> cards = model.whole(part);
                if (cards.isEmpty()) continue;
                rows.add(new Row(HEADER, "header:" + part.key, ShelfAdapter.CardHolder.rowTitle(context, part), null, null));
                for (HomeModel.GameCard card : cards) {
                    rows.add(new Row(COVER_GAME, card.key, null, card, null));
                }
            }
        } else if (!model.library.isEmpty()) {
            rows.add(new Row(HEADER, "header:library", context.getString(R.string.console_section_library), null, null));
            for (HomeModel.GameCard card : model.library) {
                rows.add(new Row(COVER_GAME, card.key, null, card, null));
            }
        }
        if (!model.hosts.isEmpty()) {
            rows.add(new Row(HEADER, "header:hosts", context.getString(R.string.console_section_hosts), null, null));
            for (HomeModel.HostCard card : model.hosts) {
                rows.add(new Row(HOST, "host:" + card.uuid, null, null, card));
            }
        }
        notifyDataSetChanged();
    }

    int viewTypeAt(int position) {
        return rows.get(position).type;
    }

    /** Position of the first card, or -1 when there is none. */
    int firstCard() {
        for (int i = 0; i < rows.size(); i++) {
            if (rows.get(i).type != HEADER) {
                return i;
            }
        }
        return -1;
    }

    /**
     * Where focus starts: the game that runs now (in Continue playing, else anywhere), otherwise the first card.
     * The Favourites row stays first on screen either way.
     */
    int initialFocus() {
        int anywhere = -1;
        for (int i = 0; i < rows.size(); i++) {
            HomeModel.GameCard game = rows.get(i).game;
            if (game != null && game.badge == HomeModel.Badge.RUNNING) {
                if (rows.get(i).type == WIDE_GAME) {
                    return i;
                }
                if (anywhere < 0) {
                    anywhere = i;
                }
            }
        }
        return anywhere >= 0 ? anywhere : firstCard();
    }

    /** The stable id of the card at the position, or null for a header or out of range. */
    String keyAt(int position) {
        if (position < 0 || position >= rows.size() || rows.get(position).type == HEADER) {
            return null;
        }
        return rows.get(position).key;
    }

    /** Position of the card with this stable id, or -1. */
    int positionOfKey(String key) {
        for (int i = 0; i < rows.size(); i++) {
            if (rows.get(i).type != HEADER && rows.get(i).key.equals(key)) {
                return i;
            }
        }
        return -1;
    }

    /** The nearest card at or before the position, else after it; -1 when there is none. */
    int nearestCard(int position) {
        for (int i = Math.min(position, rows.size() - 1); i >= 0; i--) {
            if (rows.get(i).type != HEADER) {
                return i;
            }
        }
        for (int i = Math.max(position, 0); i < rows.size(); i++) {
            if (rows.get(i).type != HEADER) {
                return i;
            }
        }
        return -1;
    }

    @Override
    public int getItemCount() {
        return rows.size();
    }

    @Override
    public int getItemViewType(int position) {
        return rows.get(position).type;
    }

    @Override
    public long getItemId(int position) {
        String key = rows.get(position).key;
        Long id = ids.get(key);
        if (id == null) {
            id = (long) ids.size() + 1;
            ids.put(key, id);
        }
        return id;
    }

    @NonNull
    @Override
    public RecyclerView.ViewHolder onCreateViewHolder(@NonNull ViewGroup parent, int type) {
        LayoutInflater inflater = LayoutInflater.from(parent.getContext());
        if (tv != null && type != HEADER) {
            boolean cover = type != HOST;
            View view = inflater.inflate(cover ? R.layout.console_card_cover : R.layout.console_card_host, parent, false);
            // A card fills its cell: one column for a poster, the span of a PC's card.
            ViewGroup.LayoutParams params = view.getLayoutParams();
            params.width = ViewGroup.LayoutParams.MATCH_PARENT;
            view.setLayoutParams(params);
            return new ShelfAdapter.CardHolder(tv, view, cover);
        }
        switch (type) {
            case HEADER:
                return new RecyclerView.ViewHolder(inflater.inflate(R.layout.console_section_header, parent, false)) { };
            case COVER_GAME:
                return new CardHolder(inflater.inflate(R.layout.console_card_cover, parent, false), true);
            case HOST:
                return new CardHolder(inflater.inflate(R.layout.console_card_host, parent, false), false);
            default:
                return new CardHolder(inflater.inflate(R.layout.console_card_wide, parent, false), false);
        }
    }

    @Override
    public void onBindViewHolder(@NonNull RecyclerView.ViewHolder holder, int position) {
        Row row = rows.get(position);
        if (row.type == HEADER) {
            ((TextView) holder.itemView).setText(row.title);
        } else if (holder instanceof ShelfAdapter.CardHolder) {
            ShelfAdapter.Item item = new ShelfAdapter.Item(row.key, row.game, row.host);
            if (row.game != null) ((ShelfAdapter.CardHolder) holder).bindGame(TV_SHELF, item);
            else ((ShelfAdapter.CardHolder) holder).bindHost(TV_SHELF, item);
        } else if (row.game != null) {
            ((CardHolder) holder).bindGame(row.game, row.type == COVER_GAME);
        } else {
            ((CardHolder) holder).bindHost(row.host);
        }
    }

    private final class CardHolder extends RecyclerView.ViewHolder {
        final ImageView image;
        final TextView imageText;
        final TextView title;
        final TextView subtitle;
        final TextView badge;
        final View frame;
        final View more;
        final boolean cover;
        /** The host's three dots while the D-pad has marked them; the card keeps focus. */
        private boolean moreMarked;

        CardHolder(View view, boolean cover) {
            super(view);
            this.cover = cover;
            image = view.findViewById(cover ? R.id.console_cover : R.id.console_thumb);
            imageText = view.findViewById(cover ? R.id.console_cover_text : R.id.console_thumb_text);
            frame = view.findViewById(cover ? R.id.console_cover_frame : R.id.console_thumb_frame);
            title = view.findViewById(R.id.console_title);
            subtitle = view.findViewById(R.id.console_subtitle);
            badge = view.findViewById(R.id.console_badge);
            more = view.findViewById(R.id.console_more);
            view.setOnHoverListener((card, event) -> {
                card.setHovered(event.getActionMasked() != android.view.MotionEvent.ACTION_HOVER_EXIT);
                return false;
            });
        }

        void bindGame(HomeModel.GameCard card, boolean libraryCover) {
            if (libraryCover && coverHeightPx > 0) {
                ViewGroup.LayoutParams params = frame.getLayoutParams();
                if (params.height != coverHeightPx) {
                    params.height = coverHeightPx;
                    frame.setLayoutParams(params);
                }
            }
            image.setImageTintList(null);
            // A store's part, shown when the library is grouped, has the games not installed too: in grey, as on TV.
            image.setColorFilter(libraryCover && card.notInstalled() ? ShelfAdapter.NOT_INSTALLED : null);
            // Every cover crops to fill its card, whatever its shape (ArtFit).
            ArtFit.reset(image);
            if (card.appId == 0) {
                // A game not added has no app id, so no cover the host can send: its title on the cover's surface.
                image.setImageDrawable(null);
                imageText.setText(card.title);
                imageText.setVisibility(View.VISIBLE);
            } else if (covers != null) covers.bind(hosts.apply(card.hostUuid), CoverLoaders.appOf(card.title, card.appId, card.source), image,
                    imageText);
            title.setText(card.title);
            String where = card.hostCount > 1 ? context.getString(R.string.console_card_on_hosts, card.hostCount)
                    : context.getString(R.string.console_card_on_host, card.hostName);
            subtitle.setText(where);
            CharSequence state = null;
            if (card.badge == HomeModel.Badge.RUNNING) {
                state = context.getString(R.string.console_badge_running);
            } else if (card.badge == HomeModel.Badge.SUSPENDED) {
                state = context.getString(R.string.console_badge_suspended);
            }
            badge.setText(state);
            badge.setVisibility(state == null ? View.GONE : View.VISIBLE);
            String description = state == null ? card.title + ", " + where : card.title + ", " + state + ", " + where;
            itemView.setContentDescription(card.favourite
                    ? description + ", " + context.getString(R.string.console_favourite_description) : description);
            OfflineLook.bind(itemView, subtitle, badge, OfflineLook.allAway(card.hostUuids, hosts));
            itemView.setOnClickListener(v -> listener.onGame(card));
            itemView.setOnLongClickListener(v -> {
                listener.onGameMenu(card);
                return true;
            });
            if (libraryCover) {
                // The card's own focus tween is skipped while a key is held: the new card is focused at once, the old at rest.
                itemView.setOnFocusChangeListener((v, hasFocus) -> {
                    if (travel.isActive(SystemClock.uptimeMillis())) {
                        // Focus changes before the card's drawable state does; settle the state it is moving to.
                        v.refreshDrawableState();
                        FastTravel.settle(v);
                    }
                });
            }
            itemView.setOnKeyListener((v, keyCode, event) -> {
                if (libraryCover && event.getAction() == KeyEvent.ACTION_DOWN && movesFocus(keyCode)) {
                    travel.onMove(event.getRepeatCount(), SystemClock.uptimeMillis());
                }
                if (keyCode != KeyEvent.KEYCODE_MENU) {
                    return false;
                }
                if (event.getAction() == KeyEvent.ACTION_UP) {
                    listener.onGameToggleFavourite(card);
                }
                return true;
            });
        }

        private boolean movesFocus(int keyCode) {
            return keyCode == KeyEvent.KEYCODE_DPAD_LEFT || keyCode == KeyEvent.KEYCODE_DPAD_RIGHT
                    || keyCode == KeyEvent.KEYCODE_DPAD_UP || keyCode == KeyEvent.KEYCODE_DPAD_DOWN;
        }

        void bindHost(HomeModel.HostCard card) {
            imageText.setVisibility(View.GONE);
            image.setImageResource(R.drawable.ic_computer);
            image.setScaleType(ImageView.ScaleType.CENTER_INSIDE);
            image.setImageTintList(ColorStateList.valueOf(
                    MaterialColors.getColor(image, com.google.android.material.R.attr.colorOnSurfaceVariant)));
            image.setVisibility(View.VISIBLE);
            title.setText(card.name);
            String line = hostLine(card);
            subtitle.setText(line);
            badge.setVisibility(View.GONE);
            itemView.setContentDescription(card.name + ", " + line);
            itemView.setAlpha(card.online ? 1f : OfflineLook.DIMMED);
            itemView.setOnClickListener(v -> listener.onHost(card));
            itemView.setOnLongClickListener(v -> {
                listener.onHostOptions(card);
                return true;
            });
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) {
                // A mouse's right button.
                itemView.setOnContextClickListener(v -> {
                    listener.onHostOptions(card);
                    return true;
                });
            }
            // The dots are marked by the D-pad, never a focus stop of their own (a style can make buttons focusable).
            more.setFocusable(false);
            more.setContentDescription(context.getString(R.string.library_more_for, card.name));
            more.setOnClickListener(v -> listener.onHostOptions(card));
            markMore(false);
            itemView.setOnFocusChangeListener((v, hasFocus) -> {
                if (!hasFocus) markMore(false);
            });
            itemView.setOnKeyListener((v, keyCode, event) -> onHostKey(card, keyCode, event));
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
                if (!down) {
                    listener.onHostOptions(card);
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

        /** True when no card follows this one in its row, so the end direction has nowhere else to go. */
        private boolean atRowEnd() {
            int position = getBindingAdapterPosition();
            if (position == RecyclerView.NO_POSITION) {
                return false;
            }
            if (position == getItemCount() - 1) {
                return true;
            }
            View parent = (View) itemView.getParent();
            if (parent instanceof RecyclerView
                    && ((RecyclerView) parent).getLayoutManager() instanceof GridLayoutManager) {
                GridLayoutManager grid = (GridLayoutManager) ((RecyclerView) parent).getLayoutManager();
                GridLayoutManager.SpanSizeLookup spans = grid.getSpanSizeLookup();
                int count = grid.getSpanCount();
                return spans.getSpanIndex(position, count) + spans.getSpanSize(position) >= count;
            }
            return true;
        }

        private String hostLine(HomeModel.HostCard card) {
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
